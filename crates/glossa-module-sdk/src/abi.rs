//! The native module ABI.
//!
//! A native module is a shared library exporting one symbol:
//!
//! ```c
//! const GlossaModuleVTable *glossa_module_v1(void);
//! ```
//!
//! The table carries the protocol version and two functions. All data crosses
//! the boundary as NUL-terminated UTF-8 JSON, so there is no struct layout to
//! keep in step and no Rust ABI involved — a module built with any compiler,
//! or in any language with a C FFI, works with any host of the same protocol
//! major version.
//!
//! ```c
//! typedef struct {
//!     void *ctx;
//!     char *(*call)(void *ctx, const char *method, const char *params_json);
//!     void  (*free)(void *ctx, char *s);
//! } GlossaHostVTable;
//!
//! typedef struct {
//!     uint32_t protocol_version;               /* 1 */
//!     char *(*call)(const char *method,        /* "translate", ... */
//!                   const char *params_json,
//!                   const GlossaHostVTable *host);
//!     void  (*free)(char *s);                  /* frees what call returned */
//! } GlossaModuleVTable;
//! ```
//!
//! `call` returns a [`Reply`](crate::wire::Reply) as JSON: `{"result": ...}`
//! or `{"error": {"kind": ..., "message": ...}}`. The host frees it with the
//! module's own `free`, so each side frees only what it allocated.
//!
//! The rest of the contract, which [`export_module!`](crate::export_module)
//! honours for Rust authors and `include/glossa_module.h` spells out for
//! everyone else:
//!
//! * strings passed *into* a function are borrowed for that call only;
//! * a string returned by the host's `call` goes back through the host's
//!   `free` (never the module's allocator — on Windows the two may live in
//!   different C runtimes with different heaps);
//! * the host never makes two calls into one module at once, but successive
//!   calls may come from different threads;
//! * the [`HostVTable`] is valid only during the call it was passed to, and
//!   only on that call's thread;
//! * nothing may unwind across the boundary (the macro catches panics).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::Mutex;

use crate::wire::{ErrorKind, ModuleError, Reply};
use crate::{Host, Module};

/// What the host passes into every call.
#[repr(C)]
pub struct HostVTable {
    pub ctx: *mut c_void,
    pub call: unsafe extern "C" fn(ctx: *mut c_void, method: *const c_char, params: *const c_char) -> *mut c_char,
    pub free: unsafe extern "C" fn(ctx: *mut c_void, s: *mut c_char),
}

/// What the module exports.
#[repr(C)]
pub struct ModuleVTable {
    pub protocol_version: u32,
    pub call:
        unsafe extern "C" fn(method: *const c_char, params: *const c_char, host: *const HostVTable) -> *mut c_char,
    pub free: unsafe extern "C" fn(s: *mut c_char),
}

// SAFETY: the table is immutable static data holding function pointers.
unsafe impl Sync for ModuleVTable {}

/// The symbol name the host looks up.
pub const ENTRY_SYMBOL: &[u8] = b"glossa_module_v1\0";

/// Glue for [`export_module!`](crate::export_module). Not public API.
///
/// # Safety
/// The pointers must be what the host passed to `call`.
#[doc(hidden)]
pub unsafe fn __entry<M: Module + Default>(
    instance: &Mutex<Option<M>>,
    method: *const c_char,
    params: *const c_char,
    host: *const HostVTable,
) -> *mut c_char {
    let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let method = match method.as_ref() {
            Some(_) => CStr::from_ptr(method).to_string_lossy().into_owned(),
            None => return Reply::err(ModuleError::new(ErrorKind::BadRequest, "null method")),
        };
        let params: serde_json::Value = match params.as_ref() {
            Some(_) => match serde_json::from_slice(CStr::from_ptr(params).to_bytes()) {
                Ok(v) => v,
                Err(e) => return Reply::err(ModuleError::new(ErrorKind::BadRequest, e.to_string())),
            },
            None => serde_json::Value::Null,
        };
        let Some(host) = Host::from_raw(host) else {
            return Reply::err(ModuleError::new(ErrorKind::BadRequest, "null host"));
        };
        let mut guard = match instance.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let module = guard.get_or_insert_with(M::default);
        match module.dispatch(&host, &method, params) {
            Ok(v) => Reply::ok(v),
            Err(e) => Reply::err(e),
        }
    }))
    .unwrap_or_else(|panic| {
        let msg = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "module panicked".into());
        Reply::err(ModuleError::new(ErrorKind::Other, format!("panic: {msg}")))
    });
    let text = serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"error":{"kind":"other","message":"unserialisable reply"}}"#.into());
    CString::new(text).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `s` must come from [`__entry`].
#[doc(hidden)]
pub unsafe fn __free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
