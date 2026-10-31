//! # Glossa Module SDK
//!
//! Everything a module needs to talk to Glossa, and nothing else.
//!
//! Glossa modules come in three runtimes — JavaScript, Python and native —
//! and the Python and native ones speak the same **Glossa Module Protocol**:
//! JSON messages, one request in, one response out, with the module able to
//! call back into the host for HTTP, logging and a small cache while it works.
//! This crate holds:
//!
//! * [`wire`] — the message types, shared by every runtime;
//! * [`abi`] — the C ABI a native module exports (a version number and two
//!   functions, so it can be written in C, C++, Zig or Go as well);
//! * [`export_module!`] — a macro that turns a Rust type implementing
//!   [`Module`] into a loadable shared library.
//!
//! ```ignore
//! use glossa_module_sdk::{export_module, Host, Module, wire::*};
//!
//! #[derive(Default)]
//! struct Shout;
//!
//! impl Module for Shout {
//!     fn translate(&mut self, host: &Host, req: TranslateParams) -> Result<TranslateResult, ModuleError> {
//!         host.log(LogLevel::Info, "shouting");
//!         Ok(TranslateResult::text(req.text.to_uppercase()))
//!     }
//! }
//!
//! export_module!(Shout);
//! ```
//!
//! Build it as a `cdylib`, drop the library next to a `module.toml` with
//! `runtime = "native"`, and Glossa loads it.

pub mod abi;
pub mod wire;

use std::ffi::{CStr, CString};

pub use wire::*;

/// The protocol version this SDK speaks. A host refuses a module whose major
/// version differs from its own.
pub const PROTOCOL_VERSION: u32 = 1;

/// A module. Every method has a default that answers "unsupported", so a
/// module implements only what its manifest claims.
pub trait Module: Send {
    fn translate(&mut self, host: &Host, req: TranslateParams) -> Result<TranslateResult, ModuleError> {
        let _ = (host, req);
        Err(ModuleError::unsupported("translate"))
    }

    fn detect(&mut self, host: &Host, req: DetectParams) -> Result<DetectResult, ModuleError> {
        let _ = (host, req);
        Err(ModuleError::unsupported("detect"))
    }

    fn dictionary(&mut self, host: &Host, req: DictionaryParams) -> Result<ArticleResult, ModuleError> {
        let _ = (host, req);
        Err(ModuleError::unsupported("dictionary"))
    }

    fn speak(&mut self, host: &Host, req: SpeakParams) -> Result<SpeakResult, ModuleError> {
        let _ = (host, req);
        Err(ModuleError::unsupported("speak"))
    }

    fn recognize(&mut self, host: &Host, req: RecognizeParams) -> Result<RecognizeResult, ModuleError> {
        let _ = (host, req);
        Err(ModuleError::unsupported("recognize"))
    }

    /// Route one call. The generated glue calls this; a module overrides it
    /// only to support methods beyond the standard five.
    fn dispatch(&mut self, host: &Host, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        fn parse<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, ModuleError> {
            serde_json::from_value(v).map_err(|e| ModuleError::new(ErrorKind::BadRequest, e.to_string()))
        }
        fn emit<T: serde::Serialize>(v: T) -> Result<serde_json::Value, ModuleError> {
            serde_json::to_value(v).map_err(|e| ModuleError::other(e.to_string()))
        }
        match method {
            "translate" => emit(self.translate(host, parse(params)?)?),
            "detect" => emit(self.detect(host, parse(params)?)?),
            "dictionary" => emit(self.dictionary(host, parse(params)?)?),
            "speak" => emit(self.speak(host, parse(params)?)?),
            "recognize" => emit(self.recognize(host, parse(params)?)?),
            other => Err(ModuleError::unsupported(other)),
        }
    }
}

/// The host's services, as a native module sees them.
pub struct Host<'a> {
    vtable: &'a abi::HostVTable,
}

impl<'a> Host<'a> {
    /// # Safety
    /// `vtable` must be the pointer the host passed to `call`, valid for the
    /// duration of that call.
    #[doc(hidden)]
    pub unsafe fn from_raw(vtable: *const abi::HostVTable) -> Option<Host<'a>> {
        vtable.as_ref().map(|vtable| Host { vtable })
    }

    /// Make an HTTP request through the host, which applies the user's proxy,
    /// timeout and the module's network permissions.
    pub fn http(&self, req: &HttpRequest) -> Result<HttpResponse, ModuleError> {
        let out = self.call_host("http", &serde_json::to_value(req).expect("request serialises"))?;
        serde_json::from_value(out).map_err(|e| ModuleError::other(format!("host http reply: {e}")))
    }

    /// GET a URL and return the body as text.
    pub fn get_text(&self, url: &str) -> Result<String, ModuleError> {
        let resp = self.http(&HttpRequest::get(url))?;
        resp.ok_text()
    }

    pub fn log(&self, level: LogLevel, message: &str) {
        let _ = self.call_host("log", &serde_json::json!({ "level": level, "message": message }));
    }

    /// A value from the module's small persistent cache, if present and not
    /// expired.
    pub fn cache_get(&self, key: &str) -> Option<serde_json::Value> {
        self.call_host("cache_get", &serde_json::json!({ "key": key }))
            .ok()
            .filter(|v| !v.is_null())
    }

    pub fn cache_set(&self, key: &str, value: serde_json::Value, ttl_secs: u64) {
        let _ = self.call_host("cache_set", &serde_json::json!({ "key": key, "value": value, "ttl_secs": ttl_secs }));
    }

    fn call_host(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let method_c = CString::new(method).expect("method names have no NUL");
        let params_c = CString::new(params.to_string()).map_err(|e| ModuleError::other(e.to_string()))?;
        // SAFETY: the host guarantees the vtable functions are valid for the
        // call's duration; the strings outlive the call.
        let raw = unsafe { (self.vtable.call)(self.vtable.ctx, method_c.as_ptr(), params_c.as_ptr()) };
        if raw.is_null() {
            return Err(ModuleError::other(format!("host returned nothing for {method}")));
        }
        // SAFETY: the host returned a NUL-terminated string it owns until we
        // hand it back with `free`.
        let text = unsafe { CStr::from_ptr(raw) }.to_string_lossy().into_owned();
        unsafe { (self.vtable.free)(self.vtable.ctx, raw) };
        let reply: Reply = serde_json::from_str(&text).map_err(|e| ModuleError::other(format!("host reply: {e}")))?;
        reply.into_result()
    }
}

/// Export a [`Module`] implementation from a `cdylib`.
///
/// The type must implement `Default`; one instance lives for as long as the
/// module is loaded, and calls on it are serialised by the host.
#[macro_export]
macro_rules! export_module {
    ($ty:ty) => {
        static __GLOSSA_INSTANCE: ::std::sync::Mutex<Option<$ty>> = ::std::sync::Mutex::new(None);

        unsafe extern "C" fn __glossa_call(
            method: *const ::std::os::raw::c_char,
            params: *const ::std::os::raw::c_char,
            host: *const $crate::abi::HostVTable,
        ) -> *mut ::std::os::raw::c_char {
            $crate::abi::__entry::<$ty>(&__GLOSSA_INSTANCE, method, params, host)
        }

        unsafe extern "C" fn __glossa_free(s: *mut ::std::os::raw::c_char) {
            $crate::abi::__free(s)
        }

        static __GLOSSA_VTABLE: $crate::abi::ModuleVTable = $crate::abi::ModuleVTable {
            protocol_version: $crate::PROTOCOL_VERSION,
            call: __glossa_call,
            free: __glossa_free,
        };

        #[no_mangle]
        pub extern "C" fn glossa_module_v1() -> *const $crate::abi::ModuleVTable {
            &__GLOSSA_VTABLE
        }
    };
}
