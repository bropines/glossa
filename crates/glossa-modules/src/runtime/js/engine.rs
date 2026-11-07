//! The JavaScript engine glue shared by all three dialects.
//!
//! The engine is QuickJS (through `rquickjs`), a small embeddable engine
//! written in C. Each call runs in a **fresh** runtime and context: the
//! scripts are evaluated, the entry function is called, the result is read
//! out, and everything is dropped. Nothing stays resident between calls,
//! which keeps memory flat no matter how many script modules are installed,
//! and a script that corrupts its own globals cannot poison the next call.
//! Creating a runtime and evaluating the preludes takes well under a
//! millisecond — small next to the network round trip that follows.
//!
//! [`with_context`] is the only way in: it hands a closure a `Ctx`, and every
//! JavaScript value the closure touches borrows from it, so no value can
//! outlive the runtime it belongs to.
//!
//! Every call gets a heap limit ([`MEMORY_LIMIT`]) and a time limit
//! ([`TIME_LIMIT`], enforced by the interrupt handler). Time spent inside host
//! calls (a network request) does not count: the limit is on the script's own
//! work, and the network has its own timeouts.
//!
//! Scripts reach the host through one native function, `__host_call(method,
//! json)`, which returns a JSON reply envelope; the friendlier APIs each
//! dialect expects (`host.http`, pot's `fetch`, the classic `RequestData`
//! flow) are thin JavaScript preludes on top of it. During a call the native
//! finds its way back to the Rust side through a thread-local [`Bridge`] that
//! the call installs and removes — the runtime never outlives the call, so the
//! bridge can never dangle.
//!
//! A Rust panic inside a native (or inside the host callback it reaches) is
//! caught by `rquickjs` at the C boundary and resumed once control is back in
//! Rust, so the `catch_unwind` around every provider still sees it.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use base64::Engine as _;
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};
use rquickjs::context::EvalOptions;
use rquickjs::function::{Args, Rest};
use rquickjs::{qjs, Context, Ctx, Exception, Function, Runtime, Value};

/// The most memory one call's JavaScript heap may use. Generous for real
/// work (a large screenshot travels through a pot-app plugin as a base64
/// string, and gets copied a few times on the way), fatal for a runaway
/// allocation.
pub const MEMORY_LIMIT: usize = 128 << 20;

/// How long a script may compute in one call, not counting the time it waits
/// for the host.
pub const TIME_LIMIT: Duration = Duration::from_secs(10);

/// Each call runs on a stack of its own, this big, rather than on whatever
/// thread called it: scripts run on engine workers (8 MB), UI threads (2 MB),
/// the control API's threads and, when modules are probed, the main thread
/// (1 MB on Windows). Only the pages a call touches are ever committed, and
/// host calls (HTTP, the image API) run on it too.
const CALL_STACK: usize = 8 << 20;

/// How much of that stack the engine may use. QuickJS's frames differ a lot
/// by compiler (an MSVC frame is several times an Apple clang one), so this
/// is generous: thousands of nested JavaScript calls on every target (boa
/// allowed 400), with half the stack left for Rust and the host.
const STACK_LIMIT: usize = 4 << 20;

/// Common helpers every dialect gets: `console`, `atob`/`btoa`, `__call`.
pub const PRELUDE: &str = include_str!("prelude.js");

/// `URL`, `URLSearchParams`, `Headers` and friends, for Glossa modules (and
/// the npm libraries bundled into them) and pot-app plugins.
pub const WEB_JS: &str = include_str!("web.js");

type HostFn<'a> = dyn Fn(&str, serde_json::Value) -> Reply + 'a;

thread_local! {
    /// The host callback for the call running on this thread, as a raw fat
    /// pointer. Set by [`Bridge::install`] for exactly the duration of a call.
    static BRIDGE: RefCell<Option<*const HostFn<'static>>> = const { RefCell::new(None) };
    /// Time spent in host calls during the current call, which the time limit
    /// leaves out.
    static HOST_TIME: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    /// Set by the interrupt handler when it stops a script, so the uncatchable
    /// "interrupted" error can be told apart and reported as the time limit.
    static TIMED_OUT: Cell<bool> = const { Cell::new(false) };
    /// A panic caught in a native, to be resumed once control is back in Rust.
    static PANIC: RefCell<Option<Box<dyn std::any::Any + Send>>> = const { RefCell::new(None) };
}

/// Run a native's body, turning a panic into a JavaScript exception and
/// keeping the payload. [`describe`] (which every error path goes through)
/// and [`with_context`] resume it, so the panic reaches the `catch_unwind`
/// around the provider whichever way the engine was entered — rquickjs does
/// this for its own entry points, but not for the bytecode path.
fn guarded<'js>(ctx: &Ctx<'js>, f: impl FnOnce() -> rquickjs::Result<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            PANIC.with(|p| *p.borrow_mut() = Some(payload));
            Err(Exception::throw_internal(ctx, "panic in the host"))
        }
    }
}

fn resume_panic() {
    if let Some(payload) = PANIC.with(|p| p.borrow_mut().take()) {
        std::panic::resume_unwind(payload);
    }
}

/// Scopes a host callback to the current call. Dropping it removes the
/// callback, so a native invoked after the call (which cannot happen, since
/// the runtime is dropped first) would find nothing rather than a dangling
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
    let ptr = BRIDGE.with(|b| *b.borrow());
    let Some(ptr) = ptr else {
        return Reply::err(ModuleError::other("host call outside of a module call"));
    };
    let started = Instant::now();
    // SAFETY: see `Bridge::install` — the pointer is valid while set. The
    // borrow of BRIDGE is released first, so a host callback that itself
    // runs a script (none does today) would not hit a RefCell panic.
    let reply = unsafe { (*ptr)(method, params) };
    HOST_TIME.with(|t| t.set(t.get() + started.elapsed()));
    reply
}

/// Run `f` in a fresh runtime and context with the limits set, the natives
/// installed and the common prelude evaluated. The `Err` side is a failure
/// to set the engine up; what the closure returns is passed through.
pub fn with_context<R>(f: impl for<'js> FnOnce(&Ctx<'js>) -> R) -> Result<R, String> {
    with_runtime(true, f)
}

fn with_runtime<R>(prelude: bool, f: impl for<'js> FnOnce(&Ctx<'js>) -> R) -> Result<R, String> {
    // The runtime is created on the new stack, so QuickJS measures its limit
    // from there. A panic unwinds back across the switch.
    stacker::grow(CALL_STACK, move || run_on_this_stack(prelude, f))
}

fn run_on_this_stack<R>(prelude: bool, f: impl for<'js> FnOnce(&Ctx<'js>) -> R) -> Result<R, String> {
    let rt = Runtime::new().map_err(|e| format!("could not start the JavaScript engine: {e}"))?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_LIMIT);

    // The limit clock starts now and pauses during host calls. A script run
    // from inside another call's host callback (none does) would share the
    // counters; save and restore them so the outer call is unaffected.
    let saved_host_time = HOST_TIME.with(|t| t.replace(Duration::ZERO));
    let saved_timed_out = TIMED_OUT.with(|t| t.replace(false));
    let started = Instant::now();
    // QuickJS calls this every few thousand instructions, so reading the
    // clock each time costs nothing measurable.
    rt.set_interrupt_handler(Some(Box::new(move || {
        let worked = started.elapsed().saturating_sub(HOST_TIME.with(Cell::get));
        let stop = worked > TIME_LIMIT;
        if stop {
            TIMED_OUT.with(|t| t.set(true));
        }
        stop
    })));

    let result = (|| {
        let ctx = Context::full(&rt).map_err(|e| format!("could not start the JavaScript engine: {e}"))?;
        ctx.with(|ctx| {
            if prelude {
                install_natives(&ctx).map_err(|e| describe(&ctx, e))?;
                shared::PRELUDE.eval(&ctx)?;
            }
            Ok(f(&ctx))
        })
    })();

    HOST_TIME.with(|t| t.set(saved_host_time));
    TIMED_OUT.with(|t| t.set(saved_timed_out));
    // A script that caught the exception standing in for a panic does not
    // get to hide it.
    resume_panic();
    result
}

type Native = for<'js> fn(Ctx<'js>, Rest<Value<'js>>) -> rquickjs::Result<Value<'js>>;

fn install_natives(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let natives: [(&str, Native); 5] = [
        ("__host_call", native_host_call),
        ("__b64encode", native_b64encode),
        ("__b64decode", native_b64decode),
        ("__digest", native_digest),
        ("__log", native_log),
    ];
    let g = ctx.globals();
    for (name, f) in natives {
        g.set(name, native(ctx, name, f)?)?;
    }
    Ok(())
}

fn native<'js>(ctx: &Ctx<'js>, name: &str, f: Native) -> rquickjs::Result<Function<'js>> {
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
        let c = ctx.clone();
        guarded(&c, move || f(ctx, args))
    })?
    .with_name(name)
}

fn eval_options(name: &str) -> EvalOptions {
    let mut options = EvalOptions::default();
    // Classic and pot-app scripts are sloppy-mode code (implicit globals,
    // `arguments.callee`), as browsers and Windows Script Host ran them.
    options.strict = false;
    options.filename = Some(name.to_owned());
    options
}

/// Evaluate `code` as a global script, naming it `name` in error messages.
pub fn eval<'js>(ctx: &Ctx<'js>, code: &str, name: &str) -> Result<Value<'js>, String> {
    ctx.eval_with_options::<Value, _>(code, eval_options(name)).map_err(|e| format!("{name}: {}", describe(ctx, e)))
}

/// Compile a script to QuickJS bytecode without running it. Loading the
/// bytecode in a later call ([`eval_compiled`]) skips the parse, which is
/// most of the cost of a large script (a library bundled from npm).
pub fn compile(code: &str, name: &str) -> Result<Vec<u8>, String> {
    // Compiling runs nothing, so it needs neither the natives nor the prelude
    // (and the prelude is itself compiled this way).
    with_runtime(false, |ctx| {
        let source =
            std::ffi::CString::new(code).map_err(|_| format!("{name}: the script contains a NUL character"))?;
        let file = std::ffi::CString::new(name).unwrap_or_default();
        let flags = (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32;
        // SAFETY: `source` is NUL-terminated as JS_Eval requires and both
        // strings outlive the call. The compiled function is owned by us and
        // freed once written out; the written buffer is copied and freed.
        // The functions' source text is left out (it would more than double
        // the bytecode); only `Function.prototype.toString` misses it.
        unsafe {
            let raw = ctx.as_raw().as_ptr();
            let f = qjs::JS_Eval(raw, source.as_ptr(), code.len() as _, file.as_ptr(), flags);
            if qjs::JS_IsException(f) {
                return Err(format!("{name}: {}", describe(ctx, rquickjs::Error::Exception)));
            }
            let mut size = 0;
            let buf = qjs::JS_WriteObject(
                raw,
                &mut size,
                f,
                (qjs::JS_WRITE_OBJ_BYTECODE | qjs::JS_WRITE_OBJ_STRIP_SOURCE) as i32,
            );
            qjs::JS_FreeValue(raw, f);
            if buf.is_null() {
                return Err(format!("{name}: {}", describe(ctx, rquickjs::Error::Exception)));
            }
            let out = std::slice::from_raw_parts(buf, size as usize).to_vec();
            qjs::js_free(raw, buf.cast());
            Ok(out)
        }
    })?
}

/// Run bytecode made by [`compile`] as a global script.
pub fn eval_compiled<'js>(ctx: &Ctx<'js>, bytecode: &[u8], name: &str) -> Result<Value<'js>, String> {
    // SAFETY: the buffer came from `compile` in this process (same engine
    // build); JS_EvalFunction takes ownership of the function it is given,
    // and the value it returns is handed to rquickjs, which frees it.
    unsafe {
        let raw = ctx.as_raw().as_ptr();
        let f = qjs::JS_ReadObject(raw, bytecode.as_ptr(), bytecode.len() as _, qjs::JS_READ_OBJ_BYTECODE as i32);
        if qjs::JS_IsException(f) {
            return Err(format!("{name}: {}", describe(ctx, rquickjs::Error::Exception)));
        }
        let v = qjs::JS_EvalFunction(raw, f);
        if qjs::JS_IsException(v) {
            return Err(format!("{name}: {}", describe(ctx, rquickjs::Error::Exception)));
        }
        Ok(Value::from_raw(ctx.clone(), v))
    }
}

/// A script that is either parsed on every call or, past a size where the
/// parse starts to cost more than a millisecond, compiled once and kept as
/// bytecode. Small scripts stay text so an installed module that is not in
/// use keeps costing nothing but its source.
pub struct Script {
    pub name: String,
    pub source: String,
    always_compile: bool,
    bytecode: std::sync::OnceLock<Option<Vec<u8>>>,
}

/// Scripts at least this long are kept compiled.
const COMPILE_ABOVE: usize = 16 << 10;

impl Script {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Script {
        Script { name: name.into(), source: source.into(), always_compile: false, bytecode: std::sync::OnceLock::new() }
    }

    /// A script every call evaluates (the preludes): compiled on first use
    /// whatever its size, and kept for the life of the process.
    pub fn shared(name: &str, source: &str) -> Script {
        Script { always_compile: true, ..Script::new(name, source) }
    }

    fn keeps_bytecode(&self) -> bool {
        self.always_compile || self.source.len() >= COMPILE_ABOVE
    }

    /// Check the syntax now (when the module loads) rather than on first
    /// use. The compile this takes is kept for a script that is kept
    /// compiled, so it is not done again on the first call.
    pub fn check(&self) -> Result<(), String> {
        if self.bytecode.get().is_some() {
            return Ok(());
        }
        let bytecode = compile(&self.source, &self.name)?;
        let _ = self.bytecode.set(self.keeps_bytecode().then_some(bytecode));
        Ok(())
    }

    /// Evaluate the script in `ctx`, from bytecode when it is large.
    pub fn eval<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>, String> {
        let bytecode = self.bytecode.get_or_init(|| {
            // A failed compile is left to `eval` below, which reports it.
            self.keeps_bytecode().then(|| compile(&self.source, &self.name).ok()).flatten()
        });
        match bytecode {
            Some(b) => eval_compiled(ctx, b, &self.name),
            None => eval(ctx, &self.source, &self.name),
        }
    }
}

/// The preludes, compiled once per process.
pub mod shared {
    use super::Script;
    use std::sync::LazyLock;

    pub static PRELUDE: LazyLock<Script> = LazyLock::new(|| Script::shared("prelude.js", super::PRELUDE));
    pub static WEB: LazyLock<Script> = LazyLock::new(|| Script::shared("web.js", super::WEB_JS));
    pub static GLOSSA: LazyLock<Script> =
        LazyLock::new(|| Script::shared("glossa.js", super::super::glossa::GLOSSA_JS));
    pub static POT: LazyLock<Script> = LazyLock::new(|| Script::shared("pot.js", super::super::pot::POT_JS));
    pub static LEGACY_HOST: LazyLock<Script> =
        LazyLock::new(|| Script::shared("host objects", super::super::legacy::LEGACY_HOST));
    pub static COMPAT: LazyLock<Script> = LazyLock::new(|| Script::shared("Common.js", super::super::legacy::COMPAT));
}

/// Parse a script without running it, so a syntax error is reported when the
/// module loads (and in `glossa-cli modules`) instead of on its first use.
pub fn check_syntax(code: &str, name: &str) -> Result<(), String> {
    compile(code, name).map(|_| ())
}

/// Define a global property holding a JSON value.
pub fn set_global_json(ctx: &Ctx<'_>, name: &str, value: &serde_json::Value) -> Result<(), String> {
    let v = from_json(ctx, value)?;
    ctx.globals().set(name, v).map_err(|e| describe(ctx, e))
}

/// Whether a global function of this name exists.
pub fn has_function(ctx: &Ctx<'_>, name: &str) -> bool {
    get_global(ctx, name).is_function()
}

pub fn get_global<'js>(ctx: &Ctx<'js>, name: &str) -> Value<'js> {
    ctx.globals().get::<_, Value>(name).unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
}

/// A JavaScript string.
pub fn str<'js>(ctx: &Ctx<'js>, s: &str) -> Value<'js> {
    rquickjs::String::from_str(ctx.clone(), s)
        .map(|s| s.into_value())
        .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
}

/// A JavaScript number holding an integer.
pub fn int<'js>(ctx: &Ctx<'js>, n: i32) -> Value<'js> {
    Value::new_int(ctx.clone(), n)
}

pub fn bool<'js>(ctx: &Ctx<'js>, b: bool) -> Value<'js> {
    Value::new_bool(ctx.clone(), b)
}

/// Call a function value with `this` undefined. A returned promise is driven
/// to completion.
pub fn call<'js>(ctx: &Ctx<'js>, f: &Value<'js>, args: &[Value<'js>]) -> Result<Value<'js>, String> {
    let Some(f) = f.as_function() else {
        return Err("not a function".into());
    };
    let mut a = Args::new(ctx.clone(), args.len());
    for v in args {
        a.push_arg(v.clone()).map_err(|e| describe(ctx, e))?;
    }
    let value = f.call_arg::<Value>(a).map_err(|e| describe(ctx, e))?;
    settle(ctx, value)
}

/// Call a global function. A returned promise is driven to completion.
pub fn call_global<'js>(ctx: &Ctx<'js>, name: &str, args: &[Value<'js>]) -> Result<Value<'js>, String> {
    let f = get_global(ctx, name);
    if !f.is_function() {
        return Err(format!("the script has no function `{name}`"));
    }
    call(ctx, &f, args).map_err(|e| format!("{name}: {e}"))
}

/// If `value` is a promise, run the job queue until it settles.
pub fn settle<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>, String> {
    let Some(promise) = value.as_promise().cloned() else { return Ok(value) };
    // Every host operation completes synchronously, so the queue drains in a
    // bounded number of turns; the time limit stops a script that keeps
    // scheduling work forever.
    loop {
        match promise.result::<Value>() {
            Some(Ok(v)) => return Ok(v),
            Some(Err(e)) => return Err(describe(ctx, e)),
            None => {
                if !ctx.execute_pending_job() {
                    // A failing job leaves its exception behind; report the
                    // time limit if that is what stopped it.
                    if TIMED_OUT.with(Cell::get) {
                        return Err(time_limit_message());
                    }
                    return Err("the script's promise never settled".into());
                }
            }
        }
    }
}

pub fn to_json<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<serde_json::Value, String> {
    if value.is_undefined() {
        return Ok(serde_json::Value::Null);
    }
    let text = ctx.json_stringify(value.clone()).map_err(|e| describe(ctx, e))?;
    match text {
        None => Ok(serde_json::Value::Null),
        Some(s) => {
            let s = s.to_string().map_err(|e| e.to_string())?;
            serde_json::from_str(&s).map_err(|e| e.to_string())
        }
    }
}

pub fn from_json<'js>(ctx: &Ctx<'js>, value: &serde_json::Value) -> Result<Value<'js>, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    ctx.json_parse(text).map_err(|e| describe(ctx, e))
}

/// `String(value)`, with `null` and `undefined` as the empty string.
pub fn to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> String {
    if value.is_null() || value.is_undefined() {
        return String::new();
    }
    js_to_string(ctx, value).unwrap_or_default()
}

/// JavaScript's ToString, decoded leniently: a lone surrogate (which UTF-8
/// cannot hold) becomes U+FFFD instead of failing the conversion.
fn js_to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> rquickjs::Result<String> {
    // SAFETY: JS_ToCStringLen returns a buffer owned by the context (or null
    // with an exception pending), freed below before returning.
    unsafe {
        let raw = ctx.as_raw().as_ptr();
        let mut len = 0;
        let ptr = rquickjs::qjs::JS_ToCStringLen(raw, &mut len, value.as_raw());
        if ptr.is_null() {
            return Err(rquickjs::Error::Exception);
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, len);
        let s = String::from_utf8_lossy(bytes).into_owned();
        rquickjs::qjs::JS_FreeCString(raw, ptr);
        Ok(s)
    }
}

fn time_limit_message() -> String {
    format!("the script ran for more than {} s and was stopped", TIME_LIMIT.as_secs())
}

/// A readable message for a script error: the thrown message, or the thrown
/// value itself when it is not an Error object. Hitting a limit is reported
/// as such, and a syntax error carries its line.
pub fn describe(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    resume_panic();
    if TIMED_OUT.with(Cell::get) {
        // Clear the uncatchable "interrupted" exception.
        let _ = ctx.catch();
        return time_limit_message();
    }
    let rquickjs::Error::Exception = err else {
        return err.to_string();
    };
    let thrown = ctx.catch();
    let Some(obj) = thrown.as_object() else {
        return to_string(ctx, &thrown);
    };
    let get = |k: &str| obj.get::<_, Value>(k).ok().filter(|v| !v.is_undefined()).map(|v| to_string(ctx, &v));
    let Some(message) = get("message") else {
        return to_string(ctx, &thrown);
    };
    let name = get("name").unwrap_or_default();
    if name == "InternalError" && message == "out of memory" {
        return format!("the script used more than {} MB of memory and was stopped", MEMORY_LIMIT >> 20);
    }
    if (name == "RangeError" && message.contains("call stack")) || message == "stack overflow" {
        return "the script recursed too deeply (stack overflow)".into();
    }
    let mut text = if name.is_empty() || name == "Error" { message } else { format!("{name}: {message}") };
    // QuickJS keeps the position in the stack ("    at main.js:3:14" for a
    // syntax error, "    at translate (main.js:3:14)" for a thrown one).
    // The first frame in the module's own code is the one worth naming; the
    // preludes' frames say nothing to the author.
    if let Some(at) = get("stack").and_then(|s| s.lines().find_map(user_frame)) {
        text = format!("{text} (at {at})");
    }
    text
}

/// Scripts Glossa itself evaluates in every call; a stack frame in one of
/// these is not where the author's mistake is.
const INTERNAL_SCRIPTS: &[&str] = &["prelude.js", "web.js", "glossa.js", "pot.js", "host objects", "options"];

/// `file:line[:column]` from one line of a QuickJS stack, unless it points
/// into one of Glossa's own preludes or at native code.
fn user_frame(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("at ")?;
    // "name (file:line:col)" or plain "file:line:col".
    let place = match (rest.rfind('('), rest.ends_with(')')) {
        (Some(open), true) => &rest[open + 1..rest.len() - 1],
        _ => rest,
    };
    let loc = parse_location(place)?;
    (!INTERNAL_SCRIPTS.contains(&loc.file.as_str())).then(|| place.to_owned())
}

/// Read `file:line` or `file:line:column`.
fn parse_location(place: &str) -> Option<glossa_module_sdk::wire::ErrorLocation> {
    let mut parts = place.rsplitn(3, ':');
    let last = parts.next()?;
    let middle = parts.next()?;
    let (file, line, column) = match parts.next() {
        Some(file) if middle.parse::<u32>().is_ok() => (file, middle.parse().ok()?, last.parse().ok()?),
        _ => (middle, last.parse().ok()?, 0),
    };
    if file.is_empty() || file.contains('<') {
        return None;
    }
    Some(glossa_module_sdk::wire::ErrorLocation { file: file.to_owned(), line, column })
}

/// The place [`describe`] appended to a message (" (at main.js:3:14)"), and
/// the message without it.
pub fn split_location(message: &str) -> (&str, Option<glossa_module_sdk::wire::ErrorLocation>) {
    if let Some(open) = message.rfind(" (at ") {
        if message.ends_with(')') {
            if let Some(loc) = parse_location(&message[open + 5..message.len() - 1]) {
                return (&message[..open], Some(loc));
            }
        }
    }
    (message, None)
}

/// Turn a module error thrown by a script (via the prelude's `ModuleError`)
/// into the protocol's error type; anything else becomes a generic error.
/// Where the script threw goes into the error's `location`; a typed error's
/// message loses it (a `missing_credential` message is an option key), a
/// generic one keeps it, so the place shows wherever the message is shown.
pub fn script_error(message: String) -> ModuleError {
    let (bare, location) = split_location(&message);
    // The prelude encodes typed errors as "glossa:<kind>:<message>".
    if let Some(rest) = bare.split("glossa:").nth(1) {
        if let Some((kind, msg)) = rest.split_once(':') {
            let kind: Option<ErrorKind> = serde_json::from_value(serde_json::Value::String(kind.to_owned())).ok();
            if let Some(kind) = kind {
                return ModuleError::new(kind, msg.trim().to_owned()).at(location);
            }
        }
    }
    ModuleError::other(message).at(location)
}

// -- natives ---------------------------------------------------------------

fn arg_string<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], i: usize) -> rquickjs::Result<String> {
    match args.get(i) {
        Some(v) if !v.is_null() && !v.is_undefined() => js_to_string(ctx, v),
        _ => Ok(String::new()),
    }
}

fn type_error(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, message)
}

/// `__host_call(method, paramsJson) -> replyJson`
fn native_host_call<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    let method = arg_string(&ctx, &args, 0)?;
    let params_text = arg_string(&ctx, &args, 1)?;
    let params: serde_json::Value = if params_text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&params_text).map_err(|e| type_error(&ctx, &format!("__host_call: bad params: {e}")))?
    };
    let reply = host_call(&method, params);
    let text =
        serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"error":{"kind":"other","message":"bad reply"}}"#.into());
    Ok(str(&ctx, &text))
}

/// `__b64encode(binaryString) -> base64`. The input is a "binary string"
/// (each char code 0–255 is one byte), as `btoa` takes.
fn native_b64encode<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    let s = arg_string(&ctx, &args, 0)?;
    let mut bytes = Vec::with_capacity(s.len());
    for c in s.chars() {
        if c as u32 > 0xff {
            return Err(type_error(&ctx, "btoa: character outside Latin-1"));
        }
        bytes.push(c as u8);
    }
    Ok(str(&ctx, &base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// `__b64decode(base64) -> binaryString`
fn native_b64decode<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    let s = arg_string(&ctx, &args, 0)?;
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let unpadded = cleaned.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(unpadded.to_owned() + &"=".repeat((4 - unpadded.len() % 4) % 4))
        .map_err(|e| type_error(&ctx, &format!("atob: {e}")))?;
    // Each byte becomes the character with that code (U+0000–U+00FF).
    let binary: String = bytes.into_iter().map(char::from).collect();
    Ok(str(&ctx, &binary))
}

/// `__digest(algorithm, dataHex, keyHex?) -> hex`. Data and key travel as hex
/// so the JavaScript side decides the text encoding (CryptoJS uses UTF-8).
fn native_digest<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    use hmac::{Hmac, Mac};
    use md5::Md5;
    use sha1::Sha1;
    use sha2::{Digest, Sha256, Sha512};

    let alg = arg_string(&ctx, &args, 0)?.to_ascii_lowercase();
    let data = hex_decode(&arg_string(&ctx, &args, 1)?).ok_or_else(|| type_error(&ctx, "__digest: data is not hex"))?;
    let key = match args.get(2) {
        Some(v) if !v.is_null() && !v.is_undefined() => {
            Some(hex_decode(&js_to_string(&ctx, v)?).ok_or_else(|| type_error(&ctx, "__digest: key is not hex"))?)
        }
        _ => None,
    };
    macro_rules! hmac_with {
        ($h:ty, $k:expr) => {{
            let mut m = <Hmac<$h> as Mac>::new_from_slice($k).map_err(|e| type_error(&ctx, &e.to_string()))?;
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
        (other, _) => return Err(type_error(&ctx, &format!("__digest: unsupported algorithm {other}"))),
    };
    Ok(str(&ctx, &hex_encode(&out)))
}

fn native_log<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    let level = arg_string(&ctx, &args, 0)?;
    let message = arg_string(&ctx, &args, 1)?;
    let _ = host_call("log", serde_json::json!({ "level": level, "message": message }));
    Ok(Value::new_undefined(ctx))
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
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: &str) -> serde_json::Value {
        with_context(|ctx| {
            let v = eval(ctx, code, "test").unwrap();
            let v = settle(ctx, v).unwrap();
            to_json(ctx, &v).unwrap()
        })
        .unwrap()
    }

    fn run_err(code: &str) -> String {
        with_context(|ctx| eval(ctx, code, "t").and_then(|v| settle(ctx, v)).map(|_| ()).unwrap_err()).unwrap()
    }

    #[test]
    fn base64_round_trips_binary_strings() {
        assert_eq!(run("btoa('hello')"), "aGVsbG8=");
        assert_eq!(run("atob('aGVsbG8=')"), "hello");
        assert_eq!(run("atob(btoa(String.fromCharCode(0, 200, 255)))"), "\u{0}\u{c8}\u{ff}");
        assert!(run_err("btoa('ж')").contains("Latin-1"));
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
            assert_eq!(run("__call('ping', {a: 1}).echo.a"), 1);
        }
        assert_eq!(seen.borrow()[0].0, "ping");
        // After the bridge is dropped, host calls fail cleanly.
        let err = run_err("__call('ping', {})");
        assert!(err.contains("outside of a module call"), "{err}");
    }

    #[test]
    fn promises_settle_and_rejections_surface() {
        assert_eq!(run("(async () => { await null; return 5; })()"), 5);
        assert!(run_err("(async () => { throw new Error('boom'); })()").contains("boom"));
    }

    #[test]
    fn runaway_loops_hit_the_time_limit() {
        // A shorter wait would need a configurable limit; ten seconds of a
        // test is acceptable for the one place that proves the limit works.
        let started = Instant::now();
        let err = run_err("while (true) {}");
        assert!(err.contains("was stopped"), "{err}");
        assert!(started.elapsed() < TIME_LIMIT + Duration::from_secs(5));
        // A script cannot catch its way past the limit.
        let err = run_err("for (;;) { try { while (true) {} } catch (e) {} }");
        assert!(err.contains("was stopped"), "{err}");
        // The next call starts with a clean slate.
        assert_eq!(run("1 + 1"), 2);
    }

    #[test]
    fn time_in_host_calls_does_not_count() {
        let host = |_: &str, _: serde_json::Value| {
            HOST_TIME.with(|t| t.set(t.get() + TIME_LIMIT));
            Reply::ok(serde_json::Value::Null)
        };
        let _bridge = Bridge::install(&host);
        // Pretend the host call took the whole limit; the script still runs.
        assert_eq!(run("__call('slow', null); var s = 0; for (var i = 0; i < 1e5; i++) s += i; s"), 4999950000i64);
    }

    #[test]
    fn runaway_allocation_hits_the_memory_limit() {
        let err = run_err("var a = []; for (;;) a.push(new Array(1e5).fill(1));");
        assert!(err.contains("memory"), "{err}");
        let err = run_err("var s = 'x'; for (;;) s += s;");
        assert!(err.contains("memory") || err.contains("too long"), "{err}");
        assert_eq!(run("'still ' + 'fine'"), "still fine");
    }

    #[test]
    fn deep_recursion_fails_the_call() {
        let err = run_err("function f(n) { return f(n + 1) + 1; } f(0)");
        assert!(err.contains("stack overflow"), "{err}");
        let depth = run("var d = 0; function h() { d++; h(); } try { h(); } catch (e) {} d");
        eprintln!("maximum recursion depth: {depth}");
        // Past the depth boa allowed (400) on every target, debug builds
        // included (MSVC's frames are the largest: about 850 deep there).
        assert_eq!(run("function g(n) { return n ? g(n - 1) + 1 : 0; } g(600)"), 600);
    }

    #[test]
    fn a_panicking_host_unwinds_through_the_engine() {
        let host = |_: &str, _: serde_json::Value| -> Reply { panic!("host bug") };
        let _bridge = Bridge::install(&host);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run("__call('x', null)")));
        assert!(caught.is_err());
        // The engine is usable afterwards.
        drop(_bridge);
        assert_eq!(run("2 * 3"), 6);
    }

    #[test]
    fn large_scripts_run_from_bytecode_like_from_source() {
        // Past the size where scripts are kept compiled.
        let filler = "// padding\n".repeat(COMPILE_ABOVE / 10);
        let source = format!("{filler}var big = 1;\nfunction answer() {{ return big + 41; }}\nimplicit = 'sloppy';");
        let script = Script::new("big.js", source);
        for _ in 0..2 {
            let v = with_context(|ctx| {
                script.eval(ctx).unwrap();
                let v = call_global(ctx, "answer", &[]).unwrap();
                (to_json(ctx, &v).unwrap(), to_string(ctx, &get_global(ctx, "implicit")))
            })
            .unwrap();
            assert_eq!(v, (serde_json::json!(42), "sloppy".to_owned()));
        }
        assert!(script.bytecode.get().is_some_and(Option::is_some), "kept compiled");
        // A small script stays source.
        let small = Script::new("small.js", "1");
        with_context(|ctx| small.eval(ctx).map(|_| ()).unwrap()).unwrap();
        assert!(small.bytecode.get().is_some_and(Option::is_none));
        // Errors from bytecode carry the script's name and the same message.
        let failing = Script::new("bad.js", format!("{filler}throw new ModuleError('not_found', 'x');"));
        let err = with_context(|ctx| failing.eval(ctx).map(|_| ()).unwrap_err()).unwrap();
        assert!(err.starts_with("bad.js: "), "{err}");
        assert_eq!(script_error(err).kind, ErrorKind::NotFound);
    }

    #[test]
    fn a_panicking_host_unwinds_through_bytecode_too() {
        let host = |_: &str, _: serde_json::Value| -> Reply { panic!("host bug") };
        let _bridge = Bridge::install(&host);
        let bytecode = compile("try { __call('x', null); } catch (e) { 'swallowed' }", "p.js").unwrap();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_context(|ctx| eval_compiled(ctx, &bytecode, "p.js").map(|_| ())).unwrap()
        }));
        assert!(caught.is_err(), "a script cannot catch a host panic away");
    }

    #[test]
    fn typed_module_errors_survive_the_trip() {
        let err = run_err("throw new ModuleError('missing_credential', 'api_key')");
        let e = script_error(err);
        assert_eq!(e.kind, ErrorKind::MissingCredential);
        assert_eq!(e.message, "api_key");
    }

    #[test]
    fn runtime_errors_carry_the_place_in_the_script() {
        let err = with_context(|ctx| {
            eval(ctx, "function f() {\n  return optons.x;\n}\nf();", "main.js").map(|_| ()).unwrap_err()
        })
        .unwrap();
        let e = script_error(err.clone());
        let loc = e.location.clone().unwrap_or_else(|| panic!("no location in {err:?}"));
        assert_eq!((loc.file.as_str(), loc.line), ("main.js", 2), "{err}");
        assert!(loc.column > 0, "{err}");
        assert!(e.message.contains("optons"), "{e:?}");

        // A typed error keeps its message clean and still says where.
        let err = with_context(|ctx| {
            eval(ctx, "\n\nthrow new ModuleError('missing_credential', 'api_key');", "main.js").map(|_| ()).unwrap_err()
        })
        .unwrap();
        let e = script_error(err);
        assert_eq!(e.kind, ErrorKind::MissingCredential);
        assert_eq!(e.message, "api_key");
        assert_eq!(e.location.map(|l| l.file), Some("main.js".into()));
    }

    #[test]
    fn frames_in_the_preludes_are_not_the_authors_mistake() {
        assert_eq!(user_frame("    at translate (main.js:3:14)").as_deref(), Some("main.js:3:14"));
        assert_eq!(user_frame("    at main.js:2").as_deref(), Some("main.js:2"));
        assert_eq!(user_frame("    at ModuleError (prelude.js:10:5)"), None);
        assert_eq!(user_frame("    at <eval> (<input>:1:1)"), None);
        assert_eq!(user_frame("    at push (native)"), None);
        assert_eq!(split_location("x (at a.js:1:2)").1.map(|l| (l.line, l.column)), Some((1, 2)));
        assert_eq!(split_location("plain (at noon)").1, None);
    }

    #[test]
    fn syntax_errors_name_the_file_and_line() {
        let err = check_syntax("var a = 1;\nfunction f( { }", "main.js").unwrap_err();
        assert!(err.starts_with("main.js: SyntaxError"), "{err}");
        assert!(err.contains("main.js:2"), "{err}");
        assert!(check_syntax("while (true) {}", "loop.js").is_ok(), "nothing runs");
    }

    #[test]
    fn scripts_run_in_sloppy_mode() {
        assert_eq!(run("implicitGlobal = 3; implicitGlobal"), 3);
        assert_eq!(run("(function () { return typeof arguments.callee; })()"), "function");
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
    fn lone_surrogates_do_not_break_strings() {
        let s = with_context(|ctx| {
            let v = eval(ctx, "'a' + '\\ud800' + 'b'", "t").unwrap();
            to_string(ctx, &v)
        })
        .unwrap();
        assert!(s.starts_with('a') && s.ends_with('b') && s.contains('\u{fffd}'), "{s:?}");
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(hex_encode(&[0, 15, 255]), "000fff");
        assert_eq!(hex_decode("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }
}
