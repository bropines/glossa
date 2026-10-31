//! `export_module!` end to end, in-process: the generated entry point is
//! called exactly as the host calls it, through raw C strings and a host
//! table, so the glue (parsing, dispatch, host calls, panics, ownership) is
//! tested without loading a library.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};

use glossa_module_sdk::abi::{HostVTable, ModuleVTable};
use glossa_module_sdk::wire::*;
use glossa_module_sdk::{export_module, Host, Module, PROTOCOL_VERSION};

#[derive(Default)]
struct Probe {
    calls: u32,
}

impl Module for Probe {
    fn translate(&mut self, host: &Host, req: TranslateParams) -> Result<TranslateResult, ModuleError> {
        self.calls += 1;
        host.log(LogLevel::Info, "translating");
        if req.text == "panic" {
            panic!("asked to");
        }
        let body = host.get_text(&format!("https://api.test/?q={}", req.text))?;
        Ok(TranslateResult { alternatives: vec![format!("call {}", self.calls)], ..TranslateResult::text(body) })
    }
}

export_module!(Probe);

/// The host side of the test: records calls and answers `http`.
#[derive(Default)]
struct FakeHost {
    seen: RefCell<Vec<String>>,
}

unsafe extern "C" fn fake_call(ctx: *mut c_void, method: *const c_char, params: *const c_char) -> *mut c_char {
    // SAFETY: the test passes a live `FakeHost` and valid strings.
    let host = unsafe { &*(ctx as *const FakeHost) };
    let method = unsafe { CStr::from_ptr(method) }.to_str().unwrap().to_owned();
    let params: serde_json::Value = serde_json::from_slice(unsafe { CStr::from_ptr(params) }.to_bytes()).unwrap();
    host.seen.borrow_mut().push(method.clone());
    let reply = match method.as_str() {
        "http" => {
            let url = params["url"].as_str().unwrap();
            let q = url.split("q=").nth(1).unwrap_or("");
            Reply::ok(serde_json::json!({ "status": if q == "fail" { 500 } else { 200 }, "body": q.to_uppercase() }))
        }
        "log" => Reply::ok(serde_json::Value::Null),
        other => Reply::err(ModuleError::unsupported(other)),
    };
    CString::new(serde_json::to_string(&reply).unwrap()).unwrap().into_raw()
}

unsafe extern "C" fn fake_free(_ctx: *mut c_void, s: *mut c_char) {
    // SAFETY: only strings from `fake_call` come back.
    drop(unsafe { CString::from_raw(s) });
}

fn table() -> &'static ModuleVTable {
    // SAFETY: the generated entry point returns a pointer to a static.
    unsafe { &*glossa_module_v1() }
}

/// Call the module as the host would, freeing the reply with the module's
/// own `free`.
fn call(host: Option<&FakeHost>, method: &str, params: &str) -> Reply {
    let t = table();
    let vtable = host.map(|h| HostVTable { ctx: h as *const FakeHost as *mut c_void, call: fake_call, free: fake_free });
    let (m, p) = (CString::new(method).unwrap(), CString::new(params).unwrap());
    // SAFETY: valid strings and an optional live host table, as the ABI requires.
    let raw = unsafe { (t.call)(m.as_ptr(), p.as_ptr(), vtable.as_ref().map_or(std::ptr::null(), |v| v as *const HostVTable)) };
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_owned();
    unsafe { (t.free)(raw) };
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_generated_glue_dispatches_calls_back_and_keeps_state() {
    assert_eq!(table().protocol_version, PROTOCOL_VERSION);
    let host = FakeHost::default();

    let r = call(Some(&host), "translate", r#"{"text":"hi","from":"en","to":"ru"}"#).into_result().unwrap();
    let r: TranslateResult = serde_json::from_value(r).unwrap();
    assert_eq!(r.text, "HI", "the host's http answer reached the module");
    assert_eq!(host.seen.borrow().as_slice(), ["log", "http"]);

    let r = call(Some(&host), "translate", r#"{"text":"again","from":"en","to":"ru"}"#).into_result().unwrap();
    // Tests run in parallel against the one static instance, so only a lower
    // bound is certain.
    let n: u32 = r["alternatives"][0].as_str().unwrap().trim_start_matches("call ").parse().unwrap();
    assert!(n >= 2, "one instance lives across calls (call {n})");

    let e = call(Some(&host), "translate", r#"{"text":"fail","from":"en","to":"ru"}"#).into_result().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Http, "a failed host call surfaces as the module's error");
}

#[test]
fn bad_input_and_panics_become_error_replies() {
    let host = FakeHost::default();
    let kind = |r: Reply| r.into_result().unwrap_err().kind;
    assert_eq!(kind(call(Some(&host), "translate", "{not json")), ErrorKind::BadRequest);
    assert_eq!(kind(call(Some(&host), "translate", r#"{"from":"en"}"#)), ErrorKind::BadRequest, "missing text");
    assert_eq!(kind(call(Some(&host), "detect", r#"{"text":"x"}"#)), ErrorKind::Unsupported, "default method");
    assert_eq!(kind(call(Some(&host), "teleport", "{}")), ErrorKind::Unsupported);
    assert_eq!(kind(call(None, "translate", r#"{"text":"x","from":"en","to":"ru"}"#)), ErrorKind::BadRequest, "null host");

    let e = call(Some(&host), "translate", r#"{"text":"panic","from":"en","to":"ru"}"#).into_result().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Other);
    assert!(e.message.contains("asked to"), "{e}");
    // The instance survives a panic (its mutex is not left poisoned for good).
    assert!(call(Some(&host), "translate", r#"{"text":"ok","from":"en","to":"ru"}"#).into_result().is_ok());
}

#[test]
fn freeing_null_is_harmless() {
    // SAFETY: the ABI allows null.
    unsafe { (table().free)(std::ptr::null_mut()) };
}
