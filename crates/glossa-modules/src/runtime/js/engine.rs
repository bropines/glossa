//! The JavaScript engine glue shared by all three dialects.
//!
//! Each call runs in a **fresh** engine context: the scripts are evaluated,
//! the entry function is called, the result is read out, and the context is
//! dropped. Nothing stays resident between calls, which keeps memory flat no
//! matter how many script modules are installed, and a script that corrupts
//! its own globals cannot poison the next call. Parsing a service script
//! takes a few milliseconds — small next to the network round trip that
//! follows.
//!
//! Scripts reach the host through one native function, `__host_call(method,
//! json)`, which returns a JSON reply envelope; the friendlier APIs each
//! dialect expects (`host.http`, pot's `fetch`, the classic `RequestData`
//! flow) are thin JavaScript preludes on top of it. During a call the native
//! finds its way back to the Rust side through a thread-local [`Bridge`] that
//! the call installs and removes — the context never outlives the call, so the
//! bridge can never dangle.

use std::cell::RefCell;

use base64::Engine as _;
use boa_engine::builtins::promise::PromiseState;
use boa_engine::object::builtins::JsPromise;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Source};
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};

/// How far a script may run before it is stopped: generous for real work,
/// fatal for an accidental infinite loop.
const LOOP_LIMIT: u64 = 50_000_000;
const RECURSION_LIMIT: usize = 400;

/// Common helpers every dialect gets: `console`, `atob`/`btoa`, `__call`.
pub const PRELUDE: &str = include_str!("prelude.js");

type HostFn<'a> = dyn Fn(&str, serde_json::Value) -> Reply + 'a;

thread_local! {
    /// The host callback for the call running on this thread, as a raw fat
    /// pointer. Set by [`Bridge::install`] for exactly the duration of a call.
    static BRIDGE: RefCell<Option<*const HostFn<'static>>> = const { RefCell::new(None) };
}

/// Scopes a host callback to the current call. Dropping it removes the
/// callback, so a native invoked after the call (which cannot happen, since
/// the context is dropped first) would find nothing rather than a dangling
/// pointer.
pub struct Bridge<'a> {
    _host: std::marker::PhantomData<&'a HostFn<'a>>,
}

impl<'a> Bridge<'a> {
    pub fn install(host: &'a HostFn<'a>) -> Bridge<'a> {
        // SAFETY: we erase the lifetime to store the pointer in a
        // thread-local. The pointer is only dereferenced by `host_call`
        // while this `Bridge` is alive (it is cleared in `Drop`), and the
        // `Bridge` borrows `host` for `'a`, so the referent outlives every use.
        let ptr: *const HostFn<'static> =
            unsafe { std::mem::transmute::<*const HostFn<'a>, *const HostFn<'static>>(host as *const HostFn<'a>) };
        BRIDGE.with(|b| *b.borrow_mut() = Some(ptr));
        Bridge { _host: std::marker::PhantomData }
    }
}

impl Drop for Bridge<'_> {
    fn drop(&mut self) {
        BRIDGE.with(|b| *b.borrow_mut() = None);
    }
}

fn host_call(method: &str, params: serde_json::Value) -> Reply {
    BRIDGE.with(|b| match *b.borrow() {
        // SAFETY: see `Bridge::install` — the pointer is valid while set.
        Some(ptr) => unsafe { (*ptr)(method, params) },
        None => Reply::err(ModuleError::other("host call outside of a module call")),
    })
}

/// A context with limits and the host natives installed, and the common
/// prelude evaluated.
pub fn new_context() -> Result<Context, String> {
    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_loop_iteration_limit(LOOP_LIMIT);
    ctx.runtime_limits_mut().set_recursion_limit(RECURSION_LIMIT);

    let natives: [(&str, usize, NativeFunction); 5] = [
        ("__host_call", 2, NativeFunction::from_fn_ptr(native_host_call)),
        ("__b64encode", 1, NativeFunction::from_fn_ptr(native_b64encode)),
        ("__b64decode", 1, NativeFunction::from_fn_ptr(native_b64decode)),
        ("__digest", 3, NativeFunction::from_fn_ptr(native_digest)),
        ("__log", 2, NativeFunction::from_fn_ptr(native_log)),
    ];
    for (name, len, f) in natives {
        ctx.register_global_callable(js_string!(name), len, f).map_err(|e| e.to_string())?;
    }
    eval(&mut ctx, PRELUDE, "prelude.js")?;
    Ok(ctx)
}

/// Evaluate `code`, naming it `name` in error messages.
pub fn eval(ctx: &mut Context, code: &str, name: &str) -> Result<JsValue, String> {
    ctx.eval(Source::from_bytes(code)).map_err(|e| format!("{name}: {}", describe(&e, ctx)))
}

/// Parse a script without running it, so a syntax error is reported when the
/// module loads (and in `glossa-cli modules`) instead of on its first use.
pub fn check_syntax(code: &str, name: &str) -> Result<(), String> {
    let mut ctx = Context::default();
    boa_engine::Script::parse(Source::from_bytes(code), None, &mut ctx)
        .map(|_| ())
        .map_err(|e| format!("{name}: {}", describe(&e, &mut ctx)))
}

/// Define a global property holding a JSON value.
pub fn set_global_json(ctx: &mut Context, name: &str, value: &serde_json::Value) -> Result<(), String> {
    let v = JsValue::from_json(value, ctx).map_err(|e| describe(&e, ctx))?;
    ctx.register_global_property(js_string!(name), v, Attribute::all()).map_err(|e| describe(&e, ctx))
}

/// Whether a global function of this name exists.
pub fn has_function(ctx: &mut Context, name: &str) -> bool {
    ctx.global_object().get(js_string!(name), ctx).ok().is_some_and(|v| v.as_callable().is_some())
}

pub fn get_global(ctx: &mut Context, name: &str) -> JsValue {
    ctx.global_object().get(js_string!(name), ctx).unwrap_or_default()
}

/// Call a global function. A returned promise is driven to completion.
pub fn call_global(ctx: &mut Context, name: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let f = get_global(ctx, name);
    let Some(callable) = f.as_callable() else {
        return Err(format!("the script has no function `{name}`"));
    };
    let value =
        callable.call(&JsValue::undefined(), args, ctx).map_err(|e| format!("{name}: {}", describe(&e, ctx)))?;
    settle(ctx, value).map_err(|e| format!("{name}: {e}"))
}

/// If `value` is a promise, run the job queue until it settles.
pub fn settle(ctx: &mut Context, value: JsValue) -> Result<JsValue, String> {
    let Some(obj) = value.as_object() else { return Ok(value) };
    let Ok(promise) = JsPromise::from_object(obj.clone()) else { return Ok(value) };
    // Every host operation completes synchronously, so the queue drains in a
    // bounded number of turns; the cap only guards against a script that
    // keeps scheduling work forever.
    for _ in 0..10_000 {
        match promise.state() {
            PromiseState::Fulfilled(v) => return Ok(v),
            PromiseState::Rejected(reason) => {
                let err = JsError::from_opaque(reason);
                return Err(describe(&err, ctx));
            }
            PromiseState::Pending => {
                ctx.run_jobs().map_err(|e| describe(&e, ctx))?;
            }
        }
    }
    Err("the script's promise never settled".into())
}

pub fn to_json(ctx: &mut Context, value: &JsValue) -> Result<serde_json::Value, String> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    value.to_json(ctx).map(|v| v.unwrap_or(serde_json::Value::Null)).map_err(|e| describe(&e, ctx))
}

pub fn from_json(ctx: &mut Context, value: &serde_json::Value) -> Result<JsValue, String> {
    JsValue::from_json(value, ctx).map_err(|e| describe(&e, ctx))
}

pub fn to_string(ctx: &mut Context, value: &JsValue) -> String {
    if value.is_null_or_undefined() {
        return String::new();
    }
    value.to_string(ctx).map(|s| s.to_std_string_escaped()).unwrap_or_default()
}

/// A readable message for a script error: the thrown message, or the thrown
/// value itself when it is not an Error object.
pub fn describe(err: &JsError, ctx: &mut Context) -> String {
    let opaque = match err.clone().into_opaque(ctx) {
        Ok(v) => v,
        Err(e) => return e.to_string(),
    };
    if let Some(obj) = opaque.as_object() {
        let message = obj.get(js_string!("message"), ctx).ok().filter(|v| !v.is_undefined());
        let name = obj.get(js_string!("name"), ctx).ok().filter(|v| !v.is_undefined());
        if let Some(m) = message {
            let m = to_string(ctx, &m);
            let n = name.map(|n| to_string(ctx, &n)).unwrap_or_default();
            return if n.is_empty() || n == "Error" { m } else { format!("{n}: {m}") };
        }
    }
    to_string(ctx, &opaque)
}

/// Turn a module error thrown by a script (via the prelude's `ModuleError`)
/// into the protocol's error type; anything else becomes a generic error.
pub fn script_error(message: String) -> ModuleError {
    // The prelude encodes typed errors as "glossa:<kind>:<message>".
    if let Some(rest) = message.split("glossa:").nth(1) {
        if let Some((kind, msg)) = rest.split_once(':') {
            let kind: Option<ErrorKind> = serde_json::from_value(serde_json::Value::String(kind.to_owned())).ok();
            if let Some(kind) = kind {
                return ModuleError::new(kind, msg.trim().to_owned());
            }
        }
    }
    ModuleError::other(message)
}

// -- natives ---------------------------------------------------------------

fn arg_string(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<String> {
    match args.get(i) {
        Some(v) if !v.is_null_or_undefined() => Ok(v.to_string(ctx)?.to_std_string_escaped()),
        _ => Ok(String::new()),
    }
}

/// `__host_call(method, paramsJson) -> replyJson`
fn native_host_call(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let method = arg_string(args, 0, ctx)?;
    let params_text = arg_string(args, 1, ctx)?;
    let params: serde_json::Value = if params_text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&params_text)
            .map_err(|e| JsNativeError::typ().with_message(format!("__host_call: bad params: {e}")))?
    };
    let reply = host_call(&method, params);
    let text =
        serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"error":{"kind":"other","message":"bad reply"}}"#.into());
    Ok(JsValue::from(js_string!(text)))
}

/// `__b64encode(binaryString) -> base64`. The input is a "binary string"
/// (each char code 0–255 is one byte), as `btoa` takes.
fn native_b64encode(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let s = match args.first() {
        Some(v) => v.to_string(ctx)?,
        None => return Ok(JsValue::from(js_string!(""))),
    };
    let mut bytes = Vec::with_capacity(s.len());
    for unit in s.iter() {
        if unit > 0xff {
            return Err(JsNativeError::typ().with_message("btoa: character outside Latin-1").into());
        }
        bytes.push(unit as u8);
    }
    Ok(JsValue::from(js_string!(base64::engine::general_purpose::STANDARD.encode(bytes))))
}

/// `__b64decode(base64) -> binaryString`
fn native_b64decode(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let s = arg_string(args, 0, ctx)?;
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            cleaned.trim_end_matches('=').to_owned() + &"=".repeat((4 - cleaned.trim_end_matches('=').len() % 4) % 4),
        )
        .map_err(|e| JsNativeError::typ().with_message(format!("atob: {e}")))?;
    let units: Vec<u16> = bytes.into_iter().map(u16::from).collect();
    Ok(JsValue::from(boa_engine::JsString::from(&units[..])))
}

/// `__digest(algorithm, dataHex, keyHex?) -> hex`. Data and key travel as hex
/// so the JavaScript side decides the text encoding (CryptoJS uses UTF-8).
fn native_digest(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    use hmac::{Hmac, Mac};
    use md5::Md5;
    use sha1::Sha1;
    use sha2::{Digest, Sha256, Sha512};

    let alg = arg_string(args, 0, ctx)?.to_ascii_lowercase();
    let data = hex_decode(&arg_string(args, 1, ctx)?)
        .ok_or_else(|| JsNativeError::typ().with_message("__digest: data is not hex"))?;
    let key = match args.get(2) {
        Some(v) if !v.is_null_or_undefined() => Some(
            hex_decode(&v.to_string(ctx)?.to_std_string_escaped())
                .ok_or_else(|| JsNativeError::typ().with_message("__digest: key is not hex"))?,
        ),
        _ => None,
    };
    macro_rules! hmac_with {
        ($h:ty, $k:expr) => {{
            let mut m =
                <Hmac<$h> as Mac>::new_from_slice($k).map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
            m.update(&data);
            m.finalize().into_bytes().to_vec()
        }};
    }
    let out: Vec<u8> = match (alg.as_str(), key.as_deref()) {
        ("md5", None) => Md5::digest(&data).to_vec(),
        ("sha1", None) => Sha1::digest(&data).to_vec(),
        ("sha256", None) => Sha256::digest(&data).to_vec(),
        ("sha512", None) => Sha512::digest(&data).to_vec(),
        ("md5", Some(k)) => hmac_with!(Md5, k),
        ("sha1", Some(k)) => hmac_with!(Sha1, k),
        ("sha256", Some(k)) => hmac_with!(Sha256, k),
        ("sha512", Some(k)) => hmac_with!(Sha512, k),
        (other, _) => {
            return Err(JsNativeError::typ().with_message(format!("__digest: unsupported algorithm {other}")).into())
        }
    };
    Ok(JsValue::from(js_string!(hex_encode(&out))))
}

fn native_log(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let level = arg_string(args, 0, ctx)?;
    let message = arg_string(args, 1, ctx)?;
    let _ = host_call("log", serde_json::json!({ "level": level, "message": message }));
    Ok(JsValue::undefined())
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: &str) -> serde_json::Value {
        let mut ctx = new_context().unwrap();
        let v = eval(&mut ctx, code, "test").unwrap();
        let v = settle(&mut ctx, v).unwrap();
        to_json(&mut ctx, &v).unwrap()
    }

    #[test]
    fn base64_round_trips_binary_strings() {
        assert_eq!(run("btoa('hello')"), "aGVsbG8=");
        assert_eq!(run("atob('aGVsbG8=')"), "hello");
        assert_eq!(run("atob(btoa(String.fromCharCode(0, 200, 255)))"), "\u{0}\u{c8}\u{ff}");
    }

    #[test]
    fn digests_match_known_vectors() {
        assert_eq!(run("__digest('md5', '')"), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(
            run("__digest('sha256', '616263')"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        assert_eq!(
            run("__digest('sha256', '7768617420646f2079612077616e7420666f72206e6f7468696e673f', '4a656665')"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn host_calls_reach_the_installed_bridge_and_only_then() {
        let seen = RefCell::new(Vec::new());
        let host = |m: &str, p: serde_json::Value| {
            seen.borrow_mut().push((m.to_owned(), p.clone()));
            Reply::ok(serde_json::json!({"echo": p}))
        };
        {
            let _bridge = Bridge::install(&host);
            let mut ctx = new_context().unwrap();
            let v = eval(&mut ctx, "__call('ping', {a: 1}).echo.a", "t").unwrap();
            assert_eq!(to_json(&mut ctx, &v).unwrap(), 1);
        }
        assert_eq!(seen.borrow()[0].0, "ping");
        // After the bridge is dropped, host calls fail cleanly.
        let mut ctx = new_context().unwrap();
        let err = eval(&mut ctx, "__call('ping', {})", "t").unwrap_err();
        assert!(err.contains("outside of a module call"), "{err}");
    }

    #[test]
    fn promises_settle_and_rejections_surface() {
        assert_eq!(run("(async () => { await null; return 5; })()"), 5);
        let mut ctx = new_context().unwrap();
        let v = eval(&mut ctx, "(async () => { throw new Error('boom'); })()", "t").unwrap();
        assert!(settle(&mut ctx, v).unwrap_err().contains("boom"));
    }

    #[test]
    fn runaway_loops_are_stopped() {
        let mut ctx = new_context().unwrap();
        assert!(eval(&mut ctx, "while (true) {}", "t").is_err());
    }

    #[test]
    fn typed_module_errors_survive_the_trip() {
        let mut ctx = new_context().unwrap();
        let err = eval(&mut ctx, "throw new ModuleError('missing_credential', 'api_key')", "t").unwrap_err();
        let e = script_error(err);
        assert_eq!(e.kind, ErrorKind::MissingCredential);
        assert_eq!(e.message, "api_key");
    }

    #[test]
    fn annex_b_substr_is_available() {
        assert_eq!(run("'abcdef'.substr(1, 3)"), "bcd");
        assert_eq!(run("'abcdef'.substr(-2)"), "ef");
        assert_eq!(run("(65536 + 10).toString(16).substr(1)"), "000a");
    }

    #[test]
    fn annex_b_escape_and_unescape_are_available() {
        assert_eq!(run("escape('a b+c/ä€')"), "a%20b+c/%E4%u20AC");
        assert_eq!(run("unescape('a%20b%E4%u20AC')"), "a bä€");
        // The idiom old crypto code uses to get a UTF-8 "binary string".
        assert_eq!(run("unescape(encodeURIComponent('ж'))"), "\u{d0}\u{b6}");
        assert_eq!(run("decodeURIComponent(escape(unescape(encodeURIComponent('жук'))))"), "жук");
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(hex_encode(&[0, 15, 255]), "000fff");
        assert_eq!(hex_decode("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }
}
