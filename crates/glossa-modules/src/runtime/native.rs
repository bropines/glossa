//! Native modules: shared libraries exporting the C ABI.
//!
//! A native module is a `.dll` / `.so` / `.dylib` beside a `module.toml` with
//! `runtime = "native"`. It exports one symbol, `glossa_module_v1`, returning
//! a [`ModuleVTable`]; see [`glossa_module_sdk::abi`] and
//! `include/glossa_module.h` for the contract. Every payload crossing the
//! boundary is NUL-terminated UTF-8 JSON, so this runtime is only a
//! [`Transport`]: it carries a JSON request in and a JSON [`Reply`] out, and
//! [`ProtocolProvider`] does the rest (language codes, permissions, errors) —
//! exactly as for Python modules.
//!
//! # Ownership
//!
//! Each side frees only what it allocated:
//!
//! * the strings we pass to the module's `call` are ours, borrowed for the
//!   duration of the call;
//! * the reply `call` returns belongs to the module and goes back through the
//!   module's own `free` once we have copied it;
//! * the strings our host trampolines return are `CString::into_raw`
//!   allocations, released by our `free` trampoline when the module hands
//!   them back.
//!
//! Mixing allocators across a DLL boundary is the classic way to corrupt a
//! heap on Windows (a module linked against another C runtime has another
//! heap), which is why there is no "free whatever you like" shortcut.
//!
//! # Threading
//!
//! Calls into one module are serialised by a mutex, so a module never sees
//! two concurrent `call`s, but successive calls may arrive on different
//! threads. The host table passed into a call is valid only until that call
//! returns, and only on the calling thread: the `host` closure behind it is
//! not required to be thread-safe, so a host call from any other thread is
//! answered with an error instead of being run.
//!
//! # Limits, stated plainly
//!
//! * **No time limit.** In-process code cannot be pre-empted, so
//!   `RuntimeContext::call_timeout` cannot be enforced; a call that overruns
//!   it is only logged. A module that loops forever hangs its worker thread.
//! * **No isolation.** A module that crashes — a null dereference, a stack
//!   overflow, a Rust panic built with `panic = "abort"` — takes Glossa down
//!   with it, and it runs with all of Glossa's privileges. Network
//!   permissions bind only the HTTP it does *through the host*. This is why
//!   native modules sit behind `RuntimeContext::allow_native`; prefer the
//!   Python or JavaScript runtimes for anything untrusted.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use glossa_module_sdk::abi::{HostVTable, ModuleVTable, ENTRY_SYMBOL};
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};
use glossa_module_sdk::PROTOCOL_VERSION;
use libloading::Library;
use parking_lot::Mutex;

use crate::manifest::Manifest;
use crate::protocol::{ProtocolProvider, Transport};
use crate::provider::Provider;
use crate::registry::RuntimeContext;

type ModuleCallFn = unsafe extern "C" fn(*const c_char, *const c_char, *const HostVTable) -> *mut c_char;
type ModuleFreeFn = unsafe extern "C" fn(*mut c_char);
type EntryFn = unsafe extern "C" fn() -> *const ModuleVTable;

/// Load the native module in `dir` described by `manifest`.
///
/// The library is opened and its entry point checked here, so a broken
/// one is reported when the modules load rather than on the first call.
/// That is cheap: the module's own setup (a model, a session) belongs in
/// its first `call`. A module whose library or model comes from its
/// `[[assets]]` is not opened at all until they are downloaded.
pub fn load(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let stem = if manifest.module.entry.trim().is_empty() { &manifest.module.id } else { &manifest.module.entry };
    let path = resolve_library(dir, stem)?;
    let transport = NativeTransport::open(&path, ctx.call_timeout)?;
    tracing::debug!(module = %manifest.module.id, path = %path.display(), "native module loaded");
    Ok(Arc::new(ProtocolProvider::new(manifest, transport, ctx.cache.clone()).configured(ctx)))
}

// -- finding the library --------------------------------------------------------

/// The platform conventions for shared-library names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryPlatform {
    /// `name.dll`.
    Windows,
    /// `libname.dylib`.
    MacOs,
    /// `libname.so` — Linux and the other Unixes.
    Unix,
}

impl LibraryPlatform {
    pub fn current() -> Self {
        if cfg!(windows) {
            LibraryPlatform::Windows
        } else if cfg!(target_vendor = "apple") {
            LibraryPlatform::MacOs
        } else {
            LibraryPlatform::Unix
        }
    }
}

/// The file names tried, in order, for a manifest `entry` of `stem`.
///
/// The exact name comes first, so an author can always spell the file out
/// (`entry = "shout.so"`). Then the platform's conventions, first as written
/// and then with `-` replaced by `_`, because cargo turns the package
/// `example-rot13` into `libexample_rot13.so` / `example_rot13.dll` and a
/// manifest naturally names the package. A `lib` prefix is tried on Windows
/// too, since MinGW toolchains like to add one.
pub fn library_candidates(stem: &str, platform: LibraryPlatform) -> Vec<String> {
    let mut stems = vec![stem.to_owned()];
    let normalised = stem.replace('-', "_");
    if normalised != stem {
        stems.push(normalised);
    }
    let mut out = vec![stem.to_owned()];
    for s in &stems {
        let names = match platform {
            LibraryPlatform::Windows => vec![format!("{s}.dll"), format!("lib{s}.dll")],
            LibraryPlatform::MacOs => vec![format!("lib{s}.dylib"), format!("{s}.dylib")],
            LibraryPlatform::Unix => vec![format!("lib{s}.so"), format!("{s}.so")],
        };
        for n in names {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// Find the library for `stem` inside `dir`.
///
/// `stem` must be a bare file name: a module may only load a library from its
/// own directory, never `../somewhere/else.dll` or an absolute path.
pub fn resolve_library(dir: &Path, stem: &str) -> Result<PathBuf, String> {
    let stem = stem.trim();
    let plain = Path::new(stem).file_name().is_some_and(|f| f == std::ffi::OsStr::new(stem));
    if stem.is_empty() || !plain || stem.contains(['/', '\\', ':']) {
        return Err(format!("entry {stem:?} must be a plain file name inside the module directory"));
    }
    let tried = library_candidates(stem, LibraryPlatform::current());
    tried
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("no library for entry {stem:?} in {} (tried: {})", dir.display(), tried.join(", ")))
}

// -- the transport ----------------------------------------------------------------

/// A loaded native module.
///
/// Field order matters: Rust drops fields in declaration order, and the
/// function pointers point into the library's code, so `library` is declared
/// last and is unloaded only after nothing can reach the module any more.
/// Unloading happens when the last reference to the provider goes; a module
/// must therefore not leave threads of its own running past its last call.
pub struct NativeTransport {
    call: ModuleCallFn,
    free: ModuleFreeFn,
    /// Serialises calls: a module is entitled to assume one call at a time.
    lock: Mutex<()>,
    path: PathBuf,
    call_timeout: Duration,
    library: Library,
}

impl NativeTransport {
    /// Load the library at `path` and check its entry point.
    pub fn open(path: &Path, call_timeout: Duration) -> Result<Self, String> {
        let library = open_library(path).map_err(|e| format!("cannot load {}: {e}", path.display()))?;
        let symbol = CStr::from_bytes_with_nul(ENTRY_SYMBOL).expect("the entry symbol is NUL-terminated");
        // SAFETY: `glossa_module_v1` is declared by the ABI as
        // `const GlossaModuleVTable *(void)`, which is `EntryFn`. A library
        // exporting the name with another signature breaks the contract we
        // cannot check — the price of loading native code at all.
        let entry: EntryFn = unsafe { library.get::<EntryFn>(symbol) }
            .map(|s| *s)
            .map_err(|e| format!("{} does not export glossa_module_v1: {e}", path.display()))?;
        // SAFETY: calling the entry point as declared; it takes no arguments
        // and returns a pointer to static data.
        let table = unsafe { entry() };
        // SAFETY: the pointer came from the entry point, which promises a
        // table valid for the library's lifetime (or null, which is checked).
        let (call, free) = unsafe { read_vtable(table) }.map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { call, free, lock: Mutex::new(()), path: path.to_owned(), call_timeout, library })
    }

    /// The library file this module was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The loaded library, for callers that want to look up more symbols.
    pub fn library(&self) -> &Library {
        &self.library
    }
}

/// Open a library with flags that make failures happen early and keep
/// dependency lookup local.
fn open_library(path: &Path) -> Result<Library, libloading::Error> {
    // An absolute path stops the loader from searching anywhere else for the
    // module itself (on Windows it is also what makes DLL_LOAD_DIR work).
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    #[cfg(windows)]
    {
        use libloading::os::windows::{
            Library as WinLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        // Resolve the module's own dependencies from its directory and the
        // system directories, never the current directory (DLL planting).
        // SAFETY: loading runs the library's initialisers; see the module
        // docs — trusting native code is what `allow_native` opts into.
        unsafe {
            WinLibrary::load_with_flags(&path, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)
        }
        .map(Library::from)
    }
    #[cfg(unix)]
    {
        use libloading::os::unix::{Library as UnixLibrary, RTLD_LOCAL, RTLD_NOW};
        // RTLD_NOW: an unresolved symbol fails the load, not a call minutes
        // later. RTLD_LOCAL: one module's symbols cannot interpose another's.
        // SAFETY: as above, loading runs the library's initialisers.
        unsafe { UnixLibrary::open(Some(&path), RTLD_NOW | RTLD_LOCAL) }.map(Library::from)
    }
    #[cfg(not(any(windows, unix)))]
    {
        // SAFETY: as above, loading runs the library's initialisers.
        unsafe { Library::new(&path) }
    }
}

/// Check a module's table and copy its functions out.
///
/// # Safety
/// `table` must be null or point to a readable `ModuleVTable`.
unsafe fn read_vtable(table: *const ModuleVTable) -> Result<(ModuleCallFn, ModuleFreeFn), String> {
    // SAFETY: the caller guarantees null or a valid table.
    let Some(table) = (unsafe { table.as_ref() }) else {
        return Err("glossa_module_v1 returned a null table".into());
    };
    check_protocol(table.protocol_version)?;
    Ok((table.call, table.free))
}

/// Refuse a module built for another protocol version. The version is read
/// before anything else in the table, so a future table with a different
/// layout is rejected without being trusted further.
fn check_protocol(version: u32) -> Result<(), String> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(format!(
            "module speaks native protocol {version}, this Glossa speaks {PROTOCOL_VERSION}; rebuild it against a matching glossa-module-sdk"
        ))
    }
}

/// Turn a module's reply text into a result.
fn parse_reply(bytes: &[u8]) -> Result<serde_json::Value, ModuleError> {
    serde_json::from_slice::<Reply>(bytes)
        .map_err(|e| {
            ModuleError::bad_response(format!("module reply is not a {{\"result\"}} / {{\"error\"}} envelope: {e}"))
        })?
        .into_result()
}

impl Transport for NativeTransport {
    /// Native code cannot be interrupted, so neither the cancel token nor
    /// the time limit can end a call; a cancelled call's host calls are
    /// refused (see `HostApi::handle`), which is what brings it back sooner.
    fn call_with(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        control: &crate::protocol::CallControl,
    ) -> Result<serde_json::Value, crate::protocol::CallError> {
        Ok(self.run(method, params, host, control.timeout.unwrap_or(self.call_timeout))?)
    }

    /// Nothing to free. Unloading a library while it might still be in use —
    /// another thread mid-`call`, a module thread still running, a callback
    /// pointer handed to the OS — would leave code pointers dangling into
    /// unmapped memory, and on some platforms (Rust cdylibs with thread-local
    /// destructors under glibc) `dlclose` is not even honoured. Unlike a
    /// Python child process, a loaded library also costs little while idle.
    fn suspend(&self) {}
}

impl NativeTransport {
    fn run(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        limit: Duration,
    ) -> Result<serde_json::Value, ModuleError> {
        let method_c =
            CString::new(method).map_err(|_| ModuleError::new(ErrorKind::BadRequest, "method contains NUL"))?;
        // serde_json escapes NUL inside strings, so this only fails on a bug.
        let params_c = CString::new(params.to_string()).map_err(|_| ModuleError::other("params contain NUL"))?;

        let ctx = HostCtx { host, thread: std::thread::current().id() };
        let vtable = HostVTable { ctx: &ctx as *const HostCtx as *mut c_void, call: host_call, free: host_free };

        let guard = self.lock.lock();
        let started = Instant::now();
        // SAFETY: `call` came from a validated table of a library we keep
        // loaded; the strings and `vtable` (and `ctx` behind it) live on this
        // stack frame until after the call returns, which is all the ABI lets
        // the module rely on.
        let raw = unsafe { (self.call)(method_c.as_ptr(), params_c.as_ptr(), &vtable) };
        let bytes = if raw.is_null() {
            None
        } else {
            // SAFETY: a non-null reply is a NUL-terminated string owned by the
            // module until we pass it to its `free`; we copy it first.
            let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes().to_vec();
            // SAFETY: handing the module's own allocation back to the module's
            // own deallocator, exactly once. Done under the lock, in case the
            // module's allocator is no more thread-safe than its `call`.
            unsafe { (self.free)(raw) };
            Some(bytes)
        };
        drop(guard);

        let elapsed = started.elapsed();
        if elapsed > limit {
            tracing::warn!(
                path = %self.path.display(),
                method,
                ms = elapsed.as_millis() as u64,
                "native module call overran the call timeout (native code cannot be interrupted)"
            );
        }
        match bytes {
            Some(bytes) => parse_reply(&bytes),
            None => Err(ModuleError::bad_response(format!("module returned no reply to {method}"))),
        }
    }
}

// -- host callbacks -------------------------------------------------------------

/// What the `ctx` pointer of a [`HostVTable`] points at. A `&dyn Fn` is a fat
/// pointer and cannot travel as a `void *`, so it rides inside this struct.
struct HostCtx<'a> {
    host: &'a dyn Fn(&str, serde_json::Value) -> Reply,
    /// The thread that made the call; the only one allowed to call back.
    thread: ThreadId,
}

/// `HostVTable::call`: answer a module's host call.
///
/// Never unwinds: a panic in the host is caught and becomes an error reply.
unsafe extern "C" fn host_call(ctx: *mut c_void, method: *const c_char, params: *const c_char) -> *mut c_char {
    let reply = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `ctx` is either null or the `HostCtx` that `call` put on
        // its stack for the duration of the module call.
        let Some(ctx) = (unsafe { (ctx as *const HostCtx).as_ref() }) else {
            return Reply::err(ModuleError::new(ErrorKind::BadRequest, "null host context"));
        };
        if std::thread::current().id() != ctx.thread {
            return Reply::err(ModuleError::new(
                ErrorKind::BadRequest,
                "host calls must come from the thread that called the module",
            ));
        }
        if method.is_null() {
            return Reply::err(ModuleError::new(ErrorKind::BadRequest, "null host method"));
        }
        // SAFETY: non-null, and the ABI requires a NUL-terminated string.
        let method = unsafe { CStr::from_ptr(method) }.to_string_lossy();
        let params = if params.is_null() {
            serde_json::Value::Null
        } else {
            // SAFETY: as for `method`.
            match serde_json::from_slice(unsafe { CStr::from_ptr(params) }.to_bytes()) {
                Ok(v) => v,
                Err(e) => return Reply::err(ModuleError::new(ErrorKind::BadRequest, format!("host call params: {e}"))),
            }
        };
        (ctx.host)(&method, params)
    }))
    .unwrap_or_else(|_| Reply::err(ModuleError::other("the host panicked while answering a module call")));
    reply_into_raw(&reply)
}

/// `HostVTable::free`: release a string [`host_call`] returned.
unsafe extern "C" fn host_free(_ctx: *mut c_void, s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: the ABI says only strings returned by `host_call` come back
        // here, and those are `CString::into_raw` allocations.
        drop(unsafe { CString::from_raw(s) });
    }
}

fn reply_into_raw(reply: &Reply) -> *mut c_char {
    let text = serde_json::to_string(reply)
        .unwrap_or_else(|_| r#"{"error":{"kind":"other","message":"unserialisable host reply"}}"#.into());
    // serde_json never emits a raw NUL, so `new` cannot fail in practice.
    CString::new(text).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{with_test_host, OptionValues};
    use glossa_core::{DictRequest, LangId, LangPair, TranslateRequest};
    use glossa_net::mock::MockHttp;
    use std::process::Command;
    use std::sync::OnceLock;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glossa-native-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- names -------------------------------------------------------------

    #[test]
    fn candidates_follow_platform_conventions() {
        assert_eq!(
            library_candidates("example-rot13", LibraryPlatform::Unix),
            ["example-rot13", "libexample-rot13.so", "example-rot13.so", "libexample_rot13.so", "example_rot13.so"]
        );
        assert_eq!(
            library_candidates("example-rot13", LibraryPlatform::Windows),
            ["example-rot13", "example-rot13.dll", "libexample-rot13.dll", "example_rot13.dll", "libexample_rot13.dll"]
        );
        assert_eq!(library_candidates("shout", LibraryPlatform::MacOs), ["shout", "libshout.dylib", "shout.dylib"]);
    }

    #[test]
    fn entries_must_stay_inside_the_module_directory() {
        let dir = temp_dir("escape");
        for bad in ["../evil", "/usr/lib/libc.so.6", "sub/lib.so", "C:\\x.dll", "c:x.dll", "..", ""] {
            let err = resolve_library(&dir, bad).unwrap_err();
            assert!(err.contains("plain file name"), "{bad}: {err}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolution_prefers_the_exact_name_and_reports_what_it_tried() {
        let dir = temp_dir("resolve");
        let err = resolve_library(&dir, "my-mod").unwrap_err();
        assert!(err.contains("tried: my-mod, ") && err.contains("my_mod"), "{err}");

        let conventional = library_candidates("my-mod", LibraryPlatform::current()).pop().unwrap();
        std::fs::write(dir.join(&conventional), b"").unwrap();
        assert_eq!(resolve_library(&dir, "my-mod").unwrap(), dir.join(&conventional), "normalised name found");

        std::fs::write(dir.join("my-mod"), b"").unwrap();
        assert_eq!(resolve_library(&dir, "my-mod").unwrap(), dir.join("my-mod"), "exact name wins");
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- tables and replies ------------------------------------------------------

    unsafe extern "C" fn null_call(_: *const c_char, _: *const c_char, _: *const HostVTable) -> *mut c_char {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn null_free(_: *mut c_char) {}

    #[test]
    fn tables_from_another_protocol_are_refused() {
        let future = ModuleVTable { protocol_version: PROTOCOL_VERSION + 1, call: null_call, free: null_free };
        // SAFETY: pointers to live tables (or null).
        let err = unsafe { read_vtable(&future) }.unwrap_err();
        assert!(err.contains(&format!("protocol {}", PROTOCOL_VERSION + 1)) && err.contains("rebuild"), "{err}");
        assert!(unsafe { read_vtable(std::ptr::null()) }.unwrap_err().contains("null"));
        let current = ModuleVTable { protocol_version: PROTOCOL_VERSION, call: null_call, free: null_free };
        assert!(unsafe { read_vtable(&current) }.is_ok());
        assert!(check_protocol(0).is_err());
    }

    #[test]
    fn replies_parse_into_results_and_errors() {
        assert_eq!(parse_reply(br#"{"result":{"text":"x"}}"#).unwrap(), serde_json::json!({"text": "x"}));
        let e = parse_reply(br#"{"error":{"kind":"rate_limited","message":"slow down","retry_after_secs":3}}"#)
            .unwrap_err();
        assert_eq!((e.kind, e.retry_after_secs), (ErrorKind::RateLimited, Some(3)));
        assert_eq!(parse_reply(b"not json").unwrap_err().kind, ErrorKind::BadResponse);
        assert_eq!(parse_reply(br#"{"text":"bare"}"#).unwrap_err().kind, ErrorKind::BadResponse);
    }

    /// Call the host trampoline the way a module would and read the reply.
    fn call_host(vtable: &HostVTable, method: Option<&str>, params: Option<&[u8]>) -> Reply {
        let method = method.map(|m| CString::new(m).unwrap());
        let params = params.map(|p| CString::new(p).unwrap());
        // SAFETY: a table built by the test around a live `HostCtx`.
        let raw = unsafe {
            (vtable.call)(
                vtable.ctx,
                method.as_ref().map_or(std::ptr::null(), |m| m.as_ptr()),
                params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        };
        assert!(!raw.is_null());
        // SAFETY: the trampoline returned a NUL-terminated string we free once.
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_owned();
        unsafe { (vtable.free)(vtable.ctx, raw) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn host_trampolines_answer_and_never_unwind() {
        let host = |m: &str, p: serde_json::Value| match m {
            "echo" => Reply::ok(p),
            "boom" => panic!("host bug"),
            _ => Reply::err(ModuleError::unsupported(m)),
        };
        let ctx = HostCtx { host: &host, thread: std::thread::current().id() };
        let vtable = HostVTable { ctx: &ctx as *const HostCtx as *mut c_void, call: host_call, free: host_free };

        assert_eq!(call_host(&vtable, Some("echo"), Some(br#"{"a":1}"#)), Reply::ok(serde_json::json!({"a": 1})));
        assert_eq!(call_host(&vtable, Some("echo"), None), Reply::ok(serde_json::Value::Null), "null params are null");
        let kind = |r: Reply| r.into_result().unwrap_err().kind;
        assert_eq!(kind(call_host(&vtable, Some("echo"), Some(b"{oops"))), ErrorKind::BadRequest);
        assert_eq!(kind(call_host(&vtable, None, None)), ErrorKind::BadRequest);
        assert_eq!(kind(call_host(&vtable, Some("nope"), None)), ErrorKind::Unsupported);
        assert_eq!(kind(call_host(&vtable, Some("boom"), None)), ErrorKind::Other, "a host panic stays on our side");

        // Only the address crosses threads; the trampoline must refuse before
        // dereferencing it.
        let ctx_addr = vtable.ctx as usize;
        let wrong_thread = std::thread::scope(|s| {
            s.spawn(move || {
                let vt = HostVTable { ctx: ctx_addr as *mut c_void, call: host_call, free: host_free };
                kind(call_host(&vt, Some("echo"), None))
            })
            .join()
            .unwrap()
        });
        assert_eq!(wrong_thread, ErrorKind::BadRequest, "callbacks from other threads are refused");

        let null_ctx = HostVTable { ctx: std::ptr::null_mut(), call: host_call, free: host_free };
        assert_eq!(kind(call_host(&null_ctx, Some("echo"), None)), ErrorKind::BadRequest);
        // Freeing null is a no-op.
        unsafe { host_free(std::ptr::null_mut(), std::ptr::null_mut()) };
    }

    // -- the Rust example ------------------------------------------------------------

    /// Build `glossa-example-rot13` once per test run and return its cdylib.
    ///
    /// Cargo is asked for JSON messages so the artifact path comes from cargo
    /// itself, whatever `CARGO_TARGET_DIR` or profile is in effect.
    fn rot13_library() -> &'static Path {
        static LIB: OnceLock<PathBuf> = OnceLock::new();
        LIB.get_or_init(|| build_example("glossa-example-rot13"))
    }

    fn build_example(package: &str) -> PathBuf {
        let out = Command::new(env!("CARGO"))
            .current_dir(workspace_root())
            .args(["build", "--quiet", "-p", package, "--lib", "--message-format=json"])
            .output()
            .expect("run cargo");
        assert!(out.status.success(), "building {package} failed:\n{}", String::from_utf8_lossy(&out.stderr));
        let target = package.replace('-', "_");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == target.as_str())
            .flat_map(|m| m["filenames"].as_array().cloned().unwrap_or_default())
            .filter_map(|f| f.as_str().map(PathBuf::from))
            .find(|f| f.extension().is_some_and(|e| e == "so" || e == "dll" || e == "dylib"))
            .expect("cargo reported no cdylib for the example")
    }

    #[cfg(feature = "image")]
    #[test]
    fn the_native_ocr_example_finds_lines_through_the_image_api() {
        let dir = temp_dir("lines");
        std::fs::copy(workspace_root().join("modules/native/example-lines/module.toml"), dir.join("module.toml"))
            .unwrap();
        let lib = build_example("glossa-example-lines");
        std::fs::copy(&lib, dir.join(lib.file_name().unwrap())).unwrap();
        let manifest = Manifest::load(&dir.join("module.toml")).unwrap();
        let provider = load(&dir, manifest, &RuntimeContext::default()).expect("load the example");
        // Two dark bars on white, the second shorter.
        let mut img = image::GrayImage::from_pixel(60, 40, image::Luma([255]));
        for x in 5..50 {
            for y in 6..12 {
                img.put_pixel(x, y, image::Luma([0]));
            }
        }
        for x in 5..30 {
            for y in 24..30 {
                img.put_pixel(x, y, image::Luma([0]));
            }
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let mut req = glossa_core::OcrRequest::new(glossa_core::ocr::ImageSource::Bytes(bytes.clone()));
            req.granularity = glossa_core::ocr::OcrGranularity::Lines;
            let r = provider.recognize(host, &req).unwrap();
            assert_eq!(r.text, "line 1\nline 2");
            assert_eq!(r.blocks.len(), 2);
            assert_eq!(r.blocks[0].bounds, Some(glossa_core::ocr::Rect::new(5.0, 6.0, 45.0, 6.0)));
            assert_eq!(r.blocks[1].bounds, Some(glossa_core::ocr::Rect::new(5.0, 24.0, 25.0, 6.0)));
            assert_eq!(r.image_size, Some((60, 40)));
        });
        drop(provider);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Lay the example out as an installed module: its real `module.toml`
    /// plus the library, under the name cargo gave it.
    fn install_rot13(tag: &str) -> PathBuf {
        let dir = temp_dir(tag);
        let src = workspace_root().join("modules/native/example-rot13/module.toml");
        std::fs::copy(src, dir.join("module.toml")).unwrap();
        let lib = rot13_library();
        std::fs::copy(lib, dir.join(lib.file_name().unwrap())).unwrap();
        dir
    }

    #[test]
    fn the_rust_example_loads_and_answers_through_the_provider_api() {
        let dir = install_rot13("rot13");
        let manifest = Manifest::load(&dir.join("module.toml")).unwrap();
        let provider = load(&dir, manifest, &RuntimeContext::default()).expect("load the example");
        assert_eq!(provider.info().id, "example-rot13");

        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = provider
                .translate(
                    host,
                    &TranslateRequest::new("Hello, World!", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)),
                )
                .unwrap();
            assert_eq!(t.text, "Uryyb, Jbeyq!");
            assert_eq!(t.detected_from, Some(LangId::ENGLISH), "detected_from echoes the request's `from`");

            assert_eq!(provider.detect(host, "anything").unwrap().lang, LangId::ENGLISH);

            let a = provider
                .dictionary(
                    host,
                    &DictRequest { text: "abc".into(), from: LangId::ENGLISH, to: LangId::ENGLISH, exact: true },
                )
                .unwrap();
            assert_eq!(a.headword, "abc");
            assert_eq!(a.explanations[0].definitions, ["nop"]);
            assert_eq!(a.examples[0].target, "nop");

            let unsupported = provider
                .speak(host, &glossa_core::SpeakRequest { text: "x".into(), lang: LangId::ENGLISH, slow: false });
            assert!(matches!(unsupported, Err(glossa_core::Error::Unsupported { .. })), "{unsupported:?}");
        });
        assert!(http.requests().is_empty(), "rot13 needs no network");
        drop(provider); // unloads the library before its file goes
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_rust_example_calls_back_into_the_host() {
        let dir = install_rot13("rot13-host");
        let path = resolve_library(&dir, "glossa-example-rot13").unwrap();
        let transport = NativeTransport::open(&path, Duration::from_secs(30)).unwrap();
        let calls = std::cell::RefCell::new(Vec::new());
        let host = |m: &str, p: serde_json::Value| {
            calls.borrow_mut().push((m.to_owned(), p));
            Reply::ok(serde_json::Value::Null)
        };
        let out =
            transport.call("translate", serde_json::json!({"text": "abc", "from": "en", "to": "ru"}), &host).unwrap();
        assert_eq!(out["text"], "nop");
        {
            let calls = calls.borrow();
            assert_eq!(calls.len(), 1, "{calls:?}");
            assert_eq!(calls[0].0, "log");
            assert_eq!(calls[0].1["level"], "debug");
        }

        let err = transport.call("translate", serde_json::json!({"from": "en"}), &host).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadRequest, "bad params come back as a module error: {err}");
        let err = transport.call("teleport", serde_json::Value::Null, &host).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        drop(transport);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn calls_from_many_threads_are_serialised() {
        let dir = install_rot13("rot13-threads");
        let manifest = Manifest::load(&dir.join("module.toml")).unwrap();
        let provider = load(&dir, manifest, &RuntimeContext::default()).unwrap();
        std::thread::scope(|s| {
            for i in 0..8 {
                let provider = &provider;
                s.spawn(move || {
                    let http = MockHttp::new();
                    let opts = OptionValues::new();
                    with_test_host(&http, &opts, |host| {
                        for _ in 0..25 {
                            let req =
                                TranslateRequest::new(format!("n{i}"), LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
                            assert_eq!(provider.translate(host, &req).unwrap().text, format!("a{i}"));
                        }
                    });
                });
            }
        });
        drop(provider);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- C modules ---------------------------------------------------------------------

    /// Compile C source into a shared library with the system `cc`, or `None`
    /// when there is no C compiler (the C tests then skip).
    fn compile_c(source: &Path, out: &Path) -> Option<()> {
        let include = workspace_root().join("include");
        let mut cmd = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()));
        cmd.arg("-shared").arg("-std=c99").arg("-Wall").arg("-Werror").arg("-I").arg(&include);
        if !cfg!(windows) {
            cmd.arg("-fPIC");
        }
        let status = match cmd.arg("-o").arg(out).arg(source).status() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skipping C module test: no C compiler ({e})");
                return None;
            }
        };
        assert!(status.success(), "compiling {} failed", source.display());
        Some(())
    }

    fn platform_library_name(stem: &str) -> String {
        library_candidates(stem, LibraryPlatform::current()).swap_remove(1)
    }

    #[test]
    fn the_c_example_loads_and_shouts() {
        let dir = temp_dir("c-shout");
        let example = workspace_root().join("modules/native/example-c");
        std::fs::copy(example.join("module.toml"), dir.join("module.toml")).unwrap();
        if compile_c(&example.join("shout.c"), &dir.join(platform_library_name("shout"))).is_none() {
            return;
        }
        let manifest = Manifest::load(&dir.join("module.toml")).unwrap();
        let provider = load(&dir, manifest, &RuntimeContext::default()).expect("load the C example");
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let mut req =
                TranslateRequest::new("héllo \"quoted\"\nline", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
            req.style = Some("text: decoy".into());
            let t = provider.translate(host, &req).unwrap();
            assert_eq!(t.text, "HéLLO \"QUOTED\"\nLINE", "ASCII uppercased, escapes and UTF-8 intact");
            assert!(provider.detect(host, "x").is_err(), "the C example only translates");
        });
        drop(provider);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn libraries_without_the_entry_point_or_with_another_protocol_are_refused() {
        let dir = temp_dir("c-bad");
        let missing = dir.join("missing.c");
        std::fs::write(&missing, "int glossa_something_else(void) { return 1; }\n").unwrap();
        let future = dir.join("future.c");
        std::fs::write(
            &future,
            "#include \"glossa_module.h\"\n\
             static char *c(const char *m, const char *p, const GlossaHostVTable *h) { (void)m; (void)p; (void)h; return 0; }\n\
             static void f(char *s) { (void)s; }\n\
             static const GlossaModuleVTable t = { GLOSSA_PROTOCOL_VERSION + 1, c, f };\n\
             GLOSSA_EXPORT const GlossaModuleVTable *glossa_module_v1(void) { return &t; }\n",
        )
        .unwrap();
        let (missing_lib, future_lib) =
            (dir.join(platform_library_name("missing")), dir.join(platform_library_name("future")));
        if compile_c(&missing, &missing_lib).is_none() || compile_c(&future, &future_lib).is_none() {
            return;
        }
        let err = NativeTransport::open(&missing_lib, Duration::from_secs(1)).err().unwrap();
        assert!(err.contains("does not export glossa_module_v1"), "{err}");
        let err = NativeTransport::open(&future_lib, Duration::from_secs(1)).err().unwrap();
        assert!(err.contains(&format!("protocol {}", PROTOCOL_VERSION + 1)), "{err}");
        assert!(NativeTransport::open(&dir.join("absent.so"), Duration::from_secs(1))
            .err()
            .unwrap()
            .contains("cannot load"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn registry_loads_native_modules_unless_switched_off() {
        let root = temp_dir("registry");
        let installed = install_rot13("registry-src");
        let module_dir = root.join("rot13");
        std::fs::rename(&installed, &module_dir).unwrap();

        let mut registry = crate::Registry::new();
        let off = RuntimeContext { allow_native: false, ..RuntimeContext::default() };
        let report = registry.load_dir(&root, &off);
        assert!(report.loaded.is_empty() && report.skipped.len() == 1, "{report:?}");

        let report = registry.load_dir(&root, &RuntimeContext::default());
        assert_eq!(report.loaded, ["example-rot13"], "{report:?}");
        drop(registry);
        std::fs::remove_dir_all(&root).ok();
    }
}
