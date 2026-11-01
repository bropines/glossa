//! Python modules: a child process speaking the module protocol over stdio.
//!
//! # Framing
//!
//! One JSON object per line, UTF-8, in both directions.
//!
//! * Host → module, a request: `{"id": 7, "method": "translate", "params": {…}}`.
//! * Module → host, the answer: `{"id": 7, "result": …}` or
//!   `{"id": 7, "error": {"kind": "…", "message": "…"}}`.
//! * While a request is in flight the module may call back:
//!   `{"id": "h1", "method": "http" | "log" | "cache_get" | "cache_set", "params": {…}}`,
//!   and the host answers `{"id": "h1", "result": …}` or `{"id": "h1", "error": …}`.
//!   Host-call ids are strings so they can never be mistaken for the answer.
//!
//! Anything on stdout that is not such an object is logged and otherwise
//! ignored, so a stray `print` cannot wedge the protocol (the Python SDK also
//! moves `sys.stdout` to stderr). stderr goes to the log.
//!
//! # Process lifetime
//!
//! The process starts on the first call, not when the module loads: an
//! installed module costs nothing until it is used. It is stopped after
//! `idle_timeout_secs` without calls (default 300; `0` keeps it), on
//! [`Transport::suspend`], when a call exceeds the call timeout, and when the
//! provider is dropped. A process that died is started again on the next call.
//! Calls are serialised: one request at a time per process.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use glossa_module_sdk::wire::{ModuleError, Reply};
use parking_lot::Mutex;
use serde_json::Value;

use crate::manifest::Manifest;
use crate::protocol::{ProtocolProvider, Transport};
use crate::provider::Provider;
use crate::registry::RuntimeContext;

/// How long a module may sit unused before its process is stopped.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Replies bigger than this are refused (a speech module's audio fits easily).
const MAX_FRAME: usize = 64 * 1024 * 1024;
/// Log lines from the module are cut here.
const MAX_LOG_LINE: usize = 2000;
/// How many recent stderr lines are kept to explain a crash.
const RECENT_LINES: usize = 30;

pub fn load(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let transport = PythonTransport::new(dir, &manifest, ctx)?;
    Ok(Arc::new(ProtocolProvider::new(manifest, transport, ctx.cache.clone())))
}

/// Everything needed to start the module's process.
#[derive(Debug, Clone)]
struct Config {
    module_id: String,
    dir: PathBuf,
    script: PathBuf,
    /// From the settings; empty to search.
    interpreter: String,
    call_timeout: Duration,
    /// Zero: never stop for idleness.
    idle_timeout: Duration,
    sdk_dirs: Vec<PathBuf>,
}

/// A [`Transport`] to a Python child process.
pub struct PythonTransport {
    cfg: Arc<Config>,
    inner: Arc<Mutex<Inner>>,
    /// Set by a `suspend` that found a call in flight: the process stops
    /// when that call ends. Suspending must not wait for the call, which
    /// can take as long as the call timeout, because the settings are
    /// applied (and the old modules suspended) on the UI thread.
    stop_after_call: AtomicBool,
}

struct Inner {
    proc: Option<Proc>,
    next_id: u64,
    /// Bumped per spawn, so an idle watcher knows whether its process is
    /// still the current one.
    generation: u64,
    last_used: Instant,
}

/// A running module process. Dropping it kills the process.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Frame>,
    recent: Arc<Mutex<VecDeque<String>>>,
    stderr_done: Receiver<()>,
}

enum Frame {
    Message(Value),
    /// A line too long to be a frame we accept.
    Oversize,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Proc {
    /// Why the process is gone: its exit status and the end of its stderr.
    fn exit_detail(&mut self) -> String {
        let _ = self.child.kill(); // in case it merely closed stdout
        let status = self.child.wait().ok();
        let _ = self.stderr_done.recv_timeout(Duration::from_millis(500));
        let recent: Vec<String> = self.recent.lock().iter().rev().take(12).rev().cloned().collect();
        let mut out = match status.and_then(|s| s.code()) {
            Some(code) => format!(" (exit code {code})"),
            None => String::new(),
        };
        if !recent.is_empty() {
            out.push_str(":\n");
            out.push_str(&recent.join("\n"));
        }
        out
    }
}

impl PythonTransport {
    pub fn new(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<Self, String> {
        let entry = match manifest.module.entry.trim() {
            "" => "main.py",
            e => e,
        };
        let script = dir.join(entry);
        if !script.is_file() {
            return Err(format!("{} not found", script.display()));
        }
        let cfg = Config {
            module_id: manifest.module.id.clone(),
            dir: dir.to_owned(),
            script,
            interpreter: ctx.python.clone(),
            call_timeout: ctx.call_timeout,
            idle_timeout: manifest.module.idle_timeout_secs.map(Duration::from_secs).unwrap_or(DEFAULT_IDLE_TIMEOUT),
            sdk_dirs: sdk_dirs(),
        };
        Ok(Self {
            cfg: Arc::new(cfg),
            inner: Arc::new(Mutex::new(Inner { proc: None, next_id: 0, generation: 0, last_used: Instant::now() })),
            stop_after_call: AtomicBool::new(false),
        })
    }

    /// Whether the module's process is running now.
    pub fn is_running(&self) -> bool {
        self.inner.lock().proc.is_some()
    }

    fn spawn(&self) -> Result<Proc, ModuleError> {
        let cfg = &self.cfg;
        let (program, pre_args) = interpreter(&cfg.interpreter).ok_or_else(|| {
            ModuleError::other("no Python 3 interpreter found; install Python or set its path in the settings")
        })?;
        let mut cmd = Command::new(&program);
        cmd.args(&pre_args)
            .arg(&cfg.script)
            .current_dir(&cfg.dir)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .env("PYTHONUNBUFFERED", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("GLOSSA_MODULE_ID", &cfg.module_id)
            .env("GLOSSA_PROTOCOL", crate::PROTOCOL_VERSION.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The module's own directory is already first on sys.path (it holds
        // the script), so a vendored glossa_module.py wins over ours.
        let mut path: Vec<PathBuf> = cfg.sdk_dirs.clone();
        if let Some(existing) = std::env::var_os("PYTHONPATH") {
            path.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(&path) {
            cmd.env("PYTHONPATH", joined);
        }
        hide_window(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| ModuleError::other(format!("cannot start {}: {e}", Path::new(&program).display())))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(ModuleError::other("the module process has no stdio"));
        };
        let recent = Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_LINES)));
        let frames = read_frames(stdout, cfg.module_id.clone(), recent.clone());
        let stderr_done = drain_stderr(stderr, cfg.module_id.clone(), recent.clone());
        tracing::debug!(module = %cfg.module_id, pid = child.id(), "Python module started");
        Ok(Proc { child, stdin, frames, recent, stderr_done })
    }

    /// Spawn a process and set its idle watcher going.
    fn start(&self, inner: &mut Inner) -> Result<Proc, ModuleError> {
        let proc = self.spawn()?;
        inner.generation += 1;
        if !self.cfg.idle_timeout.is_zero() {
            watch_idle(
                Arc::downgrade(&self.inner),
                inner.generation,
                self.cfg.idle_timeout,
                self.cfg.module_id.clone(),
            );
        }
        Ok(proc)
    }

    /// One request and its host calls, on a running process. The flag says
    /// whether the process is still fit for the next call.
    fn exchange(
        &self,
        proc: &mut Proc,
        id: u64,
        method: &str,
        params: Value,
        host: &dyn Fn(&str, Value) -> Reply,
    ) -> (Result<Value, ModuleError>, bool) {
        let frame = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Err(e) = write_frame(&mut proc.stdin, &frame) {
            return (Err(ModuleError::other(format!("cannot reach the module: {e}{}", proc.exit_detail()))), false);
        }
        let timeout = self.cfg.call_timeout;
        let mut deadline = Instant::now() + timeout;
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            match proc.frames.recv_timeout(wait) {
                Ok(Frame::Oversize) => {
                    return (
                        Err(ModuleError::bad_response(format!("the module sent a line over {MAX_FRAME} bytes"))),
                        false,
                    );
                }
                Ok(Frame::Message(msg)) => {
                    if msg.get("id").and_then(Value::as_u64) == Some(id) {
                        return (parse_reply(&msg), true);
                    }
                    let call_id = msg.get("id").and_then(Value::as_str).map(str::to_owned);
                    let call_method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
                    let (Some(call_id), Some(call_method)) = (call_id, call_method) else {
                        tracing::debug!(module = %self.cfg.module_id, "ignoring an unexpected frame: {}", clip(&msg.to_string()));
                        continue;
                    };
                    // The clock stops while the host works: a slow network
                    // call has its own timeout and is not the module hanging.
                    let started = Instant::now();
                    let reply = host(&call_method, msg.get("params").cloned().unwrap_or(Value::Null));
                    deadline += started.elapsed();
                    let mut answer = serde_json::to_value(&reply).unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(obj) = answer.as_object_mut() {
                        obj.insert("id".into(), Value::String(call_id));
                    }
                    if let Err(e) = write_frame(&mut proc.stdin, &answer) {
                        return (
                            Err(ModuleError::other(format!("cannot reach the module: {e}{}", proc.exit_detail()))),
                            false,
                        );
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    tracing::warn!(module = %self.cfg.module_id, "module call timed out; stopping its process");
                    return (
                        Err(ModuleError::other(format!("{method} timed out after {} ms", timeout.as_millis()))),
                        false,
                    );
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return (
                        Err(ModuleError::other(format!("the module process exited{}", proc.exit_detail()))),
                        false,
                    );
                }
            }
        }
    }
}

impl Transport for PythonTransport {
    fn call(&self, method: &str, params: Value, host: &dyn Fn(&str, Value) -> Reply) -> Result<Value, ModuleError> {
        let mut inner = self.inner.lock();
        inner.last_used = Instant::now();

        let mut proc = match inner.proc.take() {
            Some(mut p) => match p.child.try_wait() {
                Ok(None) => p,
                _ => {
                    tracing::debug!(module = %self.cfg.module_id, "module process had exited{}", p.exit_detail());
                    drop(p);
                    self.start(&mut inner)?
                }
            },
            None => self.start(&mut inner)?,
        };

        inner.next_id += 1;
        let id = inner.next_id;
        let (result, alive) = self.exchange(&mut proc, id, method, params, host);
        inner.last_used = Instant::now();
        if self.stop_after_call.swap(false, Ordering::SeqCst) {
            tracing::debug!(module = %self.cfg.module_id, "Python module suspended after its call");
        } else if alive {
            inner.proc = Some(proc);
        }
        // Otherwise `proc` is dropped here, which kills it.
        result
    }

    fn suspend(&self) {
        // A short wait rides out the idle watcher's brief look; a call in
        // flight holds the lock for its whole length.
        match self.inner.try_lock_for(Duration::from_millis(50)) {
            Some(mut inner) => {
                if inner.proc.take().is_some() {
                    tracing::debug!(module = %self.cfg.module_id, "Python module suspended");
                }
            }
            None => self.stop_after_call.store(true, Ordering::SeqCst),
        }
    }
}

impl Drop for PythonTransport {
    fn drop(&mut self) {
        self.inner.lock().proc = None;
    }
}

/// Stop the process of generation `generation` once it has been idle for
/// `idle`. Holds only a weak reference, so it never keeps a dropped
/// transport alive, and never waits on a busy one (busy is not idle).
fn watch_idle(inner: Weak<Mutex<Inner>>, generation: u64, idle: Duration, module: String) {
    let tick = (idle / 4).clamp(Duration::from_millis(50), Duration::from_secs(5));
    let spawned = std::thread::Builder::new().name(format!("py-idle-{module}")).spawn(move || loop {
        std::thread::sleep(tick);
        let Some(inner) = inner.upgrade() else { return };
        let Some(mut guard) = inner.try_lock() else { continue };
        if guard.generation != generation {
            return;
        }
        if guard.proc.is_none() {
            return;
        }
        if guard.last_used.elapsed() >= idle {
            guard.proc = None;
            tracing::debug!(module = %module, "Python module stopped after {idle:?} idle");
            return;
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the idle watcher: {e}");
    }
}

fn write_frame(stdin: &mut ChildStdin, value: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stdin.write_all(&line)?;
    stdin.flush()
}

/// The module's answer as a result.
fn parse_reply(msg: &Value) -> Result<Value, ModuleError> {
    if let Some(e) = msg.get("error").filter(|e| !e.is_null()) {
        return Err(serde_json::from_value::<ModuleError>(e.clone()).unwrap_or_else(|_| {
            let message = e.get("message").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| match e {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            ModuleError::other(message)
        }));
    }
    match msg.get("result") {
        Some(v) => Ok(v.clone()),
        None => Err(ModuleError::bad_response("the reply has neither a result nor an error")),
    }
}

/// Read stdout on a thread: protocol frames go to the channel, other lines to
/// the log.
fn read_frames(
    stdout: impl Read + Send + 'static,
    module: String,
    recent: Arc<Mutex<VecDeque<String>>>,
) -> Receiver<Frame> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name(format!("py-out-{module}")).spawn(move || {
        let mut reader = BufReader::with_capacity(64 * 1024, stdout);
        loop {
            match read_line_bounded(&mut reader, MAX_FRAME) {
                Ok(None) | Err(_) => return,
                Ok(Some((_, true))) => {
                    if tx.send(Frame::Oversize).is_err() {
                        return;
                    }
                }
                Ok(Some((line, false))) => {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    match serde_json::from_slice::<Value>(&line) {
                        Ok(v) if v.is_object() && v.get("id").is_some() => {
                            if tx.send(Frame::Message(v)).is_err() {
                                return;
                            }
                        }
                        _ => {
                            let text = clip(&String::from_utf8_lossy(&line));
                            tracing::info!(module = %module, "stdout: {text}");
                            push_recent(&recent, text);
                        }
                    }
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the module reader: {e}");
    }
    rx
}

/// Drain stderr into the log so the pipe never fills; signal when it closes.
fn drain_stderr(
    stderr: impl Read + Send + 'static,
    module: String,
    recent: Arc<Mutex<VecDeque<String>>>,
) -> Receiver<()> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name(format!("py-err-{module}")).spawn(move || {
        let mut reader = BufReader::new(stderr);
        while let Ok(Some((line, _))) = read_line_bounded(&mut reader, MAX_LOG_LINE * 4) {
            let text = clip(&String::from_utf8_lossy(&line));
            if !text.trim().is_empty() {
                tracing::info!(module = %module, "{text}");
                push_recent(&recent, text);
            }
        }
        let _ = tx.send(());
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the module log reader: {e}");
    }
    rx
}

fn push_recent(recent: &Mutex<VecDeque<String>>, line: String) {
    let mut r = recent.lock();
    if r.len() >= RECENT_LINES {
        r.pop_front();
    }
    r.push_back(line);
}

fn clip(s: &str) -> String {
    let s = s.trim_end();
    if s.chars().count() <= MAX_LOG_LINE {
        s.to_owned()
    } else {
        s.chars().take(MAX_LOG_LINE).chain(std::iter::once('…')).collect()
    }
}

/// One line without its terminator, at most `max` bytes kept; the flag says
/// it was longer. `None` at end of stream.
fn read_line_bounded<R: BufRead>(r: &mut R, max: usize) -> std::io::Result<Option<(Vec<u8>, bool)>> {
    let mut buf = Vec::new();
    let mut truncated = false;
    let mut any = false;
    loop {
        let available = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(any.then_some((buf, truncated)));
        }
        any = true;
        let (chunk_len, consumed, done) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (i, i + 1, true),
            None => (available.len(), available.len(), false),
        };
        let room = max.saturating_sub(buf.len());
        if chunk_len > room {
            truncated = true;
            buf.extend_from_slice(&available[..room]);
        } else {
            buf.extend_from_slice(&available[..chunk_len]);
        }
        r.consume(consumed);
        if done {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Some((buf, truncated)));
        }
    }
}

// -- finding Python and the SDK ---------------------------------------------------

/// The interpreter to run: the configured one, else the first Python 3 found.
fn interpreter(configured: &str) -> Option<(OsString, Vec<String>)> {
    let configured = configured.trim().trim_matches('"');
    if !configured.is_empty() {
        // A path to an executable, or a command line such as `py -3`.
        if Path::new(configured).exists() {
            return Some((configured.into(), Vec::new()));
        }
        let mut words = configured.split_whitespace();
        let program = words.next()?;
        return Some((program.into(), words.map(str::to_owned).collect()));
    }
    static FOUND: Mutex<Option<(OsString, Vec<String>)>> = Mutex::new(None);
    if let Some(found) = FOUND.lock().clone() {
        return Some(found);
    }
    let found = probe_python()?;
    *FOUND.lock() = Some(found.clone());
    Some(found)
}

/// Ask each candidate for its version and real executable. Going by the
/// executable it reports skips launchers (`py`) and the Windows Store's
/// `python.exe` alias, which opens the Store instead of running anything.
fn probe_python() -> Option<(OsString, Vec<String>)> {
    let mut candidates: Vec<(&str, Vec<String>)> = vec![("python3", vec![]), ("python", vec![])];
    if cfg!(windows) {
        candidates.push(("py", vec!["-3".into()]));
        // Version managers (pyenv-win, conda's activation scripts) put
        // `python.bat` / `python.cmd` shims on PATH, which CreateProcess does
        // not run. cmd resolves them; the probe then reports the real
        // python.exe, so modules themselves start without cmd.
        candidates.push(("cmd", vec!["/d".into(), "/c".into(), "python3".into()]));
        candidates.push(("cmd", vec!["/d".into(), "/c".into(), "python".into()]));
    }
    for (program, args) in candidates {
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .args(["-c", "import sys; print(sys.version_info[0]); print(sys.executable)"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        let Ok(mut child) = cmd.spawn() else { continue };
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        if !status.is_some_and(|s| s.success()) {
            continue;
        }
        let mut out = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_string(&mut out);
        }
        let mut lines = out.lines();
        if lines.next().map(str::trim) != Some("3") {
            continue;
        }
        let exe = lines.next().map(str::trim).unwrap_or("");
        return Some(if !exe.is_empty() && Path::new(exe).is_file() {
            (exe.into(), Vec::new())
        } else {
            (program.into(), args)
        });
    }
    None
}

/// Directories holding the bundled `glossa_module.py`, found the way
/// [`glossa_core::paths::Paths`] finds resources: beside the executable, in
/// a Unix `share/glossa`, in a macOS app bundle's `Resources`, in a
/// development checkout, and `$GLOSSA_PYTHON_SDK`.
pub fn sdk_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("GLOSSA_PYTHON_SDK").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        out.push(exe_dir.join("sdk").join("python"));
        if let Some(prefix) = exe_dir.parent() {
            out.push(prefix.join("share").join("glossa").join("sdk").join("python"));
            // Glossa.app/Contents/MacOS/glossa -> Contents/Resources.
            out.push(prefix.join("Resources").join("sdk").join("python"));
        }
        // target/debug/glossa and target/debug/deps/<test> -> the repository.
        for up in [2, 3] {
            if let Some(repo) = exe_dir.ancestors().nth(up) {
                out.push(repo.join("sdk").join("python"));
            }
        }
    }
    // The checkout this crate was built from, for builds whose target
    // directory lives elsewhere.
    out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("sdk").join("python"));
    let mut seen: Vec<PathBuf> = Vec::new();
    out.retain(|p| {
        let keep = p.join("glossa_module.py").is_file() && !seen.contains(p);
        if keep {
            seen.push(p.clone());
        }
        keep
    });
    out
}

/// Keep consoles from flashing up on Windows.
fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{with_test_host, OptionValues};
    use glossa_core::{LangId, LangPair, TranslateRequest};
    use glossa_module_sdk::wire::ErrorKind;
    use glossa_net::mock::MockHttp;

    fn have_python() -> bool {
        let ok = interpreter("").is_some();
        if !ok {
            eprintln!("note: no Python 3 interpreter found; skipping the Python runtime test");
        }
        ok
    }

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    fn ctx(timeout: Duration) -> RuntimeContext {
        RuntimeContext { call_timeout: timeout, ..Default::default() }
    }

    /// A throwaway module directory.
    struct TempModule {
        dir: PathBuf,
        manifest: Manifest,
    }

    impl TempModule {
        fn new(name: &str, extra_manifest: &str, main_py: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glossa-py-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let toml = format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"{name}\"\nname = \"{name}\"\nruntime = \"python\"\n{extra_manifest}\n[permissions]\nnetwork = [\"api.test\"]\n"
            );
            std::fs::write(dir.join("module.toml"), &toml).unwrap();
            std::fs::write(dir.join("main.py"), main_py).unwrap();
            let manifest = Manifest::parse(&toml).unwrap();
            Self { dir, manifest }
        }

        fn transport(&self, timeout: Duration) -> PythonTransport {
            PythonTransport::new(&self.dir, &self.manifest, &ctx(timeout)).unwrap()
        }
    }

    impl Drop for TempModule {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn no_host(method: &str, _: Value) -> Reply {
        Reply::err(ModuleError::other(format!("unexpected host call {method}")))
    }

    fn translate(t: &PythonTransport, text: &str) -> Result<Value, ModuleError> {
        t.call("translate", serde_json::json!({"text": text, "from": "auto", "to": "en", "options": {}}), &no_host)
    }

    #[test]
    fn the_example_module_reverses_text_through_the_provider() {
        if !have_python() {
            return;
        }
        let dir = repo().join("modules").join("python").join("example-reverse");
        let manifest = Manifest::load(&dir.join("module.toml")).unwrap();
        let provider = load(&dir, manifest, &ctx(Duration::from_secs(20))).unwrap();
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let req = TranslateRequest::new("Hello, мир 🌍", LangPair::new(LangId::AUTO, LangId::RUSSIAN));
            let t = provider.translate(host, &req).unwrap();
            assert_eq!(t.text, "🌍 рим ,olleH");
            // The process stays up between calls.
            assert_eq!(provider.translate(host, &TranslateRequest::new("ab", req.pair())).unwrap().text, "ba");
        });
        assert!(http.requests().is_empty(), "the example never touches the network");
        provider.suspend();
    }

    #[test]
    fn host_calls_are_answered_while_a_request_is_in_flight() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "callbacks",
            "",
            r#"
from glossa_module import Module, ModuleError
mod = Module()

@mod.translate
def translate(req, host):
    host.log("info", "fetching")
    r = host.http("POST", "https://api.test/t", headers={"X-A": "1"}, body="q=" + req["text"])
    data = host.http_json("GET", "https://api.test/json")
    host.cache_set("k", {"n": 1}, ttl_secs=60)
    cached = host.cache_get("k")
    try:
        host.http("GET", "https://denied.test/")
        denied = "no"
    except ModuleError as e:
        denied = e.kind
    return {"text": "%s|%s|%s|%s|%s" % (r["status"], r["body"], data["v"], cached["n"], denied)}

mod.run()
"#,
        );
        let t = m.transport(Duration::from_secs(20));
        let calls = std::sync::Mutex::new(Vec::new());
        let cache = std::sync::Mutex::new(Value::Null);
        let host = |method: &str, params: Value| -> Reply {
            calls.lock().unwrap().push((method.to_owned(), params.clone()));
            match method {
                "http" => match params["url"].as_str().unwrap_or("") {
                    "https://api.test/t" => Reply::ok(
                        serde_json::json!({"status": 201, "headers": {}, "body": format!("got {}", params["body"].as_str().unwrap_or(""))}),
                    ),
                    "https://api.test/json" => {
                        Reply::ok(serde_json::json!({"status": 200, "headers": {}, "body": "{\"v\": 42}"}))
                    }
                    _ => Reply::err(ModuleError::new(ErrorKind::Network, "not in the module's permissions")),
                },
                "cache_set" => {
                    *cache.lock().unwrap() = params["value"].clone();
                    Reply::ok(Value::Null)
                }
                "cache_get" => Reply::ok(cache.lock().unwrap().clone()),
                _ => Reply::ok(Value::Null),
            }
        };
        let out = t
            .call("translate", serde_json::json!({"text": "hi", "from": "en", "to": "ru", "options": {}}), &host)
            .unwrap();
        assert_eq!(out["text"], "201|got q=hi|42|1|network");
        let calls = calls.lock().unwrap();
        let methods: Vec<&str> = calls.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(methods, ["log", "http", "http", "cache_set", "cache_get", "http"]);
        assert_eq!(calls[1].1["method"], "POST");
        assert_eq!(calls[1].1["headers"]["X-A"], "1");
        assert_eq!(calls[3].1["ttl_secs"], 60);
    }

    #[test]
    fn a_hung_module_is_killed_and_the_next_call_starts_afresh() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "sleepy",
            "",
            r#"
import time
from glossa_module import Module
mod = Module()

@mod.translate
def translate(req, host):
    if req["text"] == "sleep":
        time.sleep(60)
    return {"text": "awake"}

mod.run()
"#,
        );
        let t = m.transport(Duration::from_millis(1500));
        assert_eq!(translate(&t, "warm up").unwrap()["text"], "awake");
        let started = Instant::now();
        let err = translate(&t, "sleep").unwrap_err();
        assert!(err.message.contains("timed out"), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!t.is_running(), "the hung process was stopped");
        assert_eq!(translate(&t, "again").unwrap()["text"], "awake");
    }

    #[test]
    fn a_crash_is_reported_with_its_stderr_and_the_module_restarts() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "crashy",
            "",
            r#"
import os, sys
from glossa_module import Module
mod = Module()

@mod.translate
def translate(req, host):
    if req["text"] == "die":
        sys.stderr.write("about to fall over\n")
        sys.stderr.flush()
        os._exit(3)
    return {"text": "fine"}

mod.run()
"#,
        );
        let t = m.transport(Duration::from_secs(20));
        let err = translate(&t, "die").unwrap_err();
        assert!(err.message.contains("exited"), "{err:?}");
        assert!(err.message.contains("exit code 3"), "{err:?}");
        assert!(err.message.contains("about to fall over"), "{err:?}");
        assert_eq!(translate(&t, "live").unwrap()["text"], "fine");
    }

    #[test]
    fn printing_to_stdout_does_not_corrupt_the_protocol() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "chatty",
            "",
            r#"
import os
print("hello from import time")
from glossa_module import Module
mod = Module()
print("after Module()")

@mod.translate
def translate(req, host):
    print("debugging", req["text"])
    print('{"id": 999999, "result": {"text": "forged"}}')
    os.write(1, b"raw fd write\n")
    return {"text": "real"}

mod.run()
"#,
        );
        let t = m.transport(Duration::from_secs(20));
        assert_eq!(translate(&t, "x").unwrap()["text"], "real");
        assert_eq!(translate(&t, "y").unwrap()["text"], "real");
    }

    #[test]
    fn module_errors_keep_their_kind_and_exceptions_their_traceback() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "errors",
            "",
            r#"
from glossa_module import Module, ModuleError
mod = Module()

@mod.translate
def translate(req, host):
    if req["text"] == "key":
        raise ModuleError("missing_credential", "api_key")
    return {"text": 1 / 0}

mod.run()
"#,
        );
        let t = m.transport(Duration::from_secs(20));
        let e = translate(&t, "key").unwrap_err();
        assert_eq!(e.kind, ErrorKind::MissingCredential);
        assert_eq!(e.message, "api_key");
        let e = translate(&t, "boom").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Other);
        assert!(e.message.contains("ZeroDivisionError") && e.message.contains("Traceback"), "{e:?}");
        let e = t.call("speak", serde_json::json!({"text": "x", "lang": "en"}), &no_host).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported);
        assert!(t.is_running(), "errors do not cost the process");
    }

    #[test]
    fn idle_modules_are_stopped_and_suspend_stops_at_once() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "idle",
            "idle_timeout_secs = 1",
            "from glossa_module import Module\nmod = Module()\nmod.translate(lambda req, host: {'text': 'ok'})\nmod.run()\n",
        );
        let t = m.transport(Duration::from_secs(20));
        assert!(!t.is_running(), "nothing starts until the first call");
        assert_eq!(translate(&t, "a").unwrap()["text"], "ok");
        assert!(t.is_running());
        let deadline = Instant::now() + Duration::from_secs(10);
        while t.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!t.is_running(), "stopped after a second idle");
        assert_eq!(translate(&t, "b").unwrap()["text"], "ok", "and started again on demand");
        t.suspend();
        assert!(!t.is_running());
    }

    #[test]
    fn suspend_does_not_wait_for_a_call_in_flight() {
        if !have_python() {
            return;
        }
        let m = TempModule::new(
            "busy",
            "",
            "import time\nfrom glossa_module import Module\nmod = Module()\n\n@mod.translate\ndef translate(req, host):\n    if req['text'] == 'x':\n        host.log('info', 'busy')\n    time.sleep(1.5)\n    return {'text': 'done'}\n\nmod.run()\n",
        );
        let t = Arc::new(m.transport(Duration::from_secs(20)));
        let (busy_tx, busy_rx) = mpsc::channel();
        let worker = {
            let t = t.clone();
            std::thread::spawn(move || {
                let host = |method: &str, _: Value| {
                    if method == "log" {
                        let _ = busy_tx.send(());
                    }
                    Reply::ok(Value::Null)
                };
                t.call("translate", serde_json::json!({"text": "x", "from": "auto", "to": "en", "options": {}}), &host)
                    .map(|v| v["text"].clone())
            })
        };
        busy_rx.recv_timeout(Duration::from_secs(20)).expect("the module starts its call");
        let started = Instant::now();
        t.suspend();
        assert!(started.elapsed() < Duration::from_millis(500), "suspend waited {:?}", started.elapsed());
        assert_eq!(worker.join().unwrap().unwrap(), "done", "the call in flight finishes");
        assert!(!t.is_running(), "and the process stops after it");
        assert_eq!(translate(&t, "y").unwrap()["text"], "done", "the next call starts it again");
        assert!(t.is_running());
    }

    #[test]
    fn a_missing_entry_point_fails_the_load() {
        let m = TempModule::new("noentry", "entry = \"absent.py\"", "");
        assert!(PythonTransport::new(&m.dir, &m.manifest, &ctx(Duration::from_secs(1))).is_err());
    }

    #[test]
    fn replies_parse() {
        assert_eq!(parse_reply(&serde_json::json!({"id": 1, "result": {"a": 1}})).unwrap()["a"], 1);
        let e = parse_reply(&serde_json::json!({"id": 1, "error": {"kind": "rate_limited", "message": "slow down", "retry_after_secs": 5}})).unwrap_err();
        assert_eq!((e.kind, e.retry_after_secs), (ErrorKind::RateLimited, Some(5)));
        let e = parse_reply(&serde_json::json!({"id": 1, "error": {"kind": "weird", "message": "m"}})).unwrap_err();
        assert_eq!((e.kind, e.message.as_str()), (ErrorKind::Other, "m"));
        assert_eq!(parse_reply(&serde_json::json!({"id": 1, "error": "plain"})).unwrap_err().message, "plain");
        assert_eq!(parse_reply(&serde_json::json!({"id": 1, "error": null, "result": 2})).unwrap(), 2);
        assert!(parse_reply(&serde_json::json!({"id": 1})).is_err());
    }

    #[test]
    fn lines_are_read_with_a_bound() {
        let mut r = BufReader::with_capacity(4, &b"short\r\nthis one is long\nend"[..]);
        assert_eq!(read_line_bounded(&mut r, 8).unwrap(), Some((b"short".to_vec(), false)));
        assert_eq!(read_line_bounded(&mut r, 8).unwrap(), Some((b"this one".to_vec(), true)));
        assert_eq!(read_line_bounded(&mut r, 8).unwrap(), Some((b"end".to_vec(), false)));
        assert_eq!(read_line_bounded(&mut r, 8).unwrap(), None);
    }

    #[test]
    fn the_bundled_sdk_is_found() {
        assert!(sdk_dirs().iter().any(|d| d.join("glossa_module.py").is_file()));
    }
}
