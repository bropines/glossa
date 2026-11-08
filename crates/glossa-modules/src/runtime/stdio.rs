//! The stdio transport: a module as a child process speaking the module
//! protocol over its stdin and stdout. Python modules, Java modules and any
//! other program (`runtime = "process"`) all use it; the runtimes differ only
//! in how they build the command line (a [`Launcher`]).
//!
//! # Framing
//!
//! One JSON object per line, UTF-8, in both directions.
//!
//! * Host → module, a request: `{"id": 7, "method": "translate", "params": {…}}`.
//! * Module → host, the answer: `{"id": 7, "result": …}` or
//!   `{"id": 7, "error": {"kind": "…", "message": "…"}}`.
//! * While a request is in flight the module may call back:
//!   `{"id": "h1", "method": "http" | "log" | "cache_get" | "cache_set" | …, "params": {…}}`,
//!   and the host answers `{"id": "h1", "result": …}` or `{"id": "h1", "error": …}`.
//!   Host-call ids are strings so they can never be mistaken for the answer.
//!   Host calls are not interpreted here: they go to the provider's host
//!   handler as they are, so a host call added to the protocol reaches every
//!   stdio runtime without a change in this file.
//!
//! Anything on stdout that is not such an object is logged and otherwise
//! ignored, so a stray `print` cannot wedge the protocol (the SDKs also keep
//! their language's standard output off the protocol stream). stderr goes to
//! the log.
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

use crate::protocol::Transport;

/// How long a module may sit unused before its process is stopped.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Replies bigger than this are refused (a speech module's audio fits easily).
const MAX_FRAME: usize = 64 * 1024 * 1024;
/// Log lines from the module are cut here.
const MAX_LOG_LINE: usize = 2000;
/// How many recent stderr lines are kept to explain a crash.
const RECENT_LINES: usize = 30;

/// One way to start the module's process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Set on top of Glossa's own environment.
    pub env: Vec<(OsString, OsString)>,
}

/// Builds the [`Launch`] for each spawn. It runs on the first call and after
/// every stop, so a runtime may look for its interpreter lazily.
pub type Launcher = Box<dyn Fn() -> Result<Launch, ModuleError> + Send + Sync>;

/// What the transport needs besides the command line.
#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub module_id: String,
    /// The runtime's name for log lines: "Python", "Java", "process".
    pub kind: &'static str,
    pub call_timeout: Duration,
    /// Zero: never stop for idleness.
    pub idle_timeout: Duration,
}

impl StdioConfig {
    pub fn new(module_id: &str, kind: &'static str, call_timeout: Duration, idle_timeout_secs: Option<u64>) -> Self {
        Self {
            module_id: module_id.to_owned(),
            kind,
            call_timeout,
            idle_timeout: idle_timeout_secs.map(Duration::from_secs).unwrap_or(DEFAULT_IDLE_TIMEOUT),
        }
    }
}

/// A [`Transport`] to a child process over stdio.
pub struct StdioTransport {
    cfg: Arc<StdioConfig>,
    launcher: Launcher,
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

impl StdioTransport {
    pub fn new(cfg: StdioConfig, launcher: Launcher) -> Self {
        Self {
            cfg: Arc::new(cfg),
            launcher,
            inner: Arc::new(Mutex::new(Inner { proc: None, next_id: 0, generation: 0, last_used: Instant::now() })),
            stop_after_call: AtomicBool::new(false),
        }
    }

    /// Whether the module's process is running now.
    pub fn is_running(&self) -> bool {
        self.inner.lock().proc.is_some()
    }

    fn spawn(&self) -> Result<Proc, ModuleError> {
        let cfg = &self.cfg;
        let launch = (self.launcher)()?;
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args)
            .current_dir(&launch.cwd)
            .env("GLOSSA_MODULE_ID", &cfg.module_id)
            .env("GLOSSA_PROTOCOL", crate::PROTOCOL_VERSION.to_string())
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_window(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| ModuleError::other(format!("cannot start {}: {e}", Path::new(&launch.program).display())))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(ModuleError::other("the module process has no stdio"));
        };
        let recent = Arc::new(Mutex::new(VecDeque::with_capacity(RECENT_LINES)));
        let frames = read_frames(stdout, cfg.module_id.clone(), recent.clone());
        let stderr_done = drain_stderr(stderr, cfg.module_id.clone(), recent.clone());
        tracing::debug!(module = %cfg.module_id, pid = child.id(), "{} module started", cfg.kind);
        Ok(Proc { child, stdin, frames, recent, stderr_done })
    }

    /// Spawn a process and set its idle watcher going.
    fn start(&self, inner: &mut Inner) -> Result<Proc, ModuleError> {
        let proc = self.spawn()?;
        inner.generation += 1;
        if !self.cfg.idle_timeout.is_zero() {
            watch_idle(Arc::downgrade(&self.inner), inner.generation, self.cfg.clone());
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

impl Transport for StdioTransport {
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
            tracing::debug!(module = %self.cfg.module_id, "{} module suspended after its call", self.cfg.kind);
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
                    tracing::debug!(module = %self.cfg.module_id, "{} module suspended", self.cfg.kind);
                }
            }
            None => self.stop_after_call.store(true, Ordering::SeqCst),
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.inner.lock().proc = None;
    }
}

/// Stop the process of generation `generation` once it has been idle for
/// the configured time. Holds only a weak reference, so it never keeps a
/// dropped transport alive, and never waits on a busy one (busy is not idle).
fn watch_idle(inner: Weak<Mutex<Inner>>, generation: u64, cfg: Arc<StdioConfig>) {
    let idle = cfg.idle_timeout;
    let tick = (idle / 4).clamp(Duration::from_millis(50), Duration::from_secs(5));
    let spawned = std::thread::Builder::new().name(format!("mod-idle-{}", cfg.module_id)).spawn(move || loop {
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
            tracing::debug!(module = %cfg.module_id, "{} module stopped after {idle:?} idle", cfg.kind);
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
    let spawned = std::thread::Builder::new().name(format!("mod-out-{module}")).spawn(move || {
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
                            crate::log_tap::emit(&module, "stdout", &text);
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
    let spawned = std::thread::Builder::new().name(format!("mod-err-{module}")).spawn(move || {
        let mut reader = BufReader::new(stderr);
        while let Ok(Some((line, _))) = read_line_bounded(&mut reader, MAX_LOG_LINE * 4) {
            let text = clip(&String::from_utf8_lossy(&line));
            if !text.trim().is_empty() {
                tracing::info!(module = %module, "{text}");
                crate::log_tap::emit(&module, "stderr", &text);
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

/// Keep consoles from flashing up on Windows.
pub(crate) fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

// -- finding programs -------------------------------------------------------------

/// `${MODULE_DIR}` in a manifest's command, arguments, environment or
/// working directory stands for the module's directory.
pub fn expand(s: &str, dir: &Path) -> String {
    s.replace("${MODULE_DIR}", &dir.to_string_lossy())
}

/// Find `name` the way a shell would: a path with a separator is taken as it
/// is (relative to `base`), a bare name is searched on `PATH`. On Windows a
/// name without an extension also tries each `PATHEXT` extension, so `npx`
/// finds `npx.cmd`; the standard library runs `.bat` / `.cmd` files through
/// `cmd.exe /d /c` with its own argument escaping.
pub fn find_program(name: &str, base: &Path) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let has_separator = name.contains('/') || (cfg!(windows) && name.contains('\\'));
    if has_separator || Path::new(name).is_absolute() {
        let path = if Path::new(name).is_absolute() { PathBuf::from(name) } else { base.join(name) };
        return with_extensions(&path).into_iter().find(|p| is_executable(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| with_extensions(&dir.join(name)))
        .find(|p| is_executable(p))
}

/// `path` itself, then (on Windows, when it has no extension) `path` with
/// each executable extension.
fn with_extensions(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_owned()];
    if cfg!(windows) && path.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for ext in exts.split(';').map(str::trim).filter(|e| e.starts_with('.')) {
            let mut p = path.as_os_str().to_owned();
            p.push(ext.to_ascii_lowercase());
            out.push(PathBuf::from(p));
        }
        // An extensionless file is not runnable on Windows; try it last.
        out.rotate_left(1);
    }
    out
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_module_sdk::wire::ErrorKind;

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
    fn programs_are_found_on_path_and_relative_to_the_module() {
        let dir = std::env::temp_dir().join(format!("glossa-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = if cfg!(windows) { "tool.cmd" } else { "tool" };
        std::fs::write(dir.join(file), "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join(file), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_program("./tool", &dir), Some(dir.join(file)));
        assert_eq!(find_program("./absent", &dir), None);
        assert_eq!(find_program("", &dir), None);
        let absolute = dir.join(file);
        assert_eq!(find_program(&absolute.to_string_lossy(), Path::new("/")), Some(absolute));
        // Something every system has on PATH.
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(find_program(shell, &dir).is_some());
        assert!(find_program("glossa-surely-not-a-program", &dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
