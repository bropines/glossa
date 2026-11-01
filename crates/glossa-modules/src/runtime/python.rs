//! Python modules: a child process speaking the module protocol over stdio.
//!
//! The transport (framing, host calls, timeouts, idle stop, restarts, logging)
//! is the shared one in [`super::stdio`]; this file only finds the
//! interpreter and the bundled SDK and builds the command line:
//! `python <entry> [args…]` in the module's directory, with the SDK on
//! `PYTHONPATH` and UTF-8 forced for the standard streams.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_module_sdk::wire::ModuleError;
use parking_lot::Mutex;

use super::stdio::{expand, hide_window, Launch, StdioConfig, StdioTransport};
use crate::manifest::Manifest;
use crate::protocol::ProtocolProvider;
use crate::provider::Provider;
use crate::registry::RuntimeContext;

pub use super::stdio::DEFAULT_IDLE_TIMEOUT;

pub fn load(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let transport = transport(dir, &manifest, ctx)?;
    Ok(Arc::new(ProtocolProvider::new(manifest, transport, ctx.cache.clone())))
}

/// The stdio transport for a Python module. The interpreter is looked for
/// when the process first starts, not here: loading a module costs nothing.
pub fn transport(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<StdioTransport, String> {
    let entry = match manifest.module.entry.trim() {
        "" => "main.py",
        e => e,
    };
    let script = dir.join(entry);
    if !script.is_file() {
        return Err(format!("{} not found", script.display()));
    }
    let m = &manifest.module;
    let cfg = StdioConfig::new(&m.id, "Python", ctx.call_timeout, m.idle_timeout_secs);
    let configured = ctx.python.clone();
    let dir = dir.to_owned();
    let extra_args: Vec<OsString> = m.args.iter().map(|a| expand(a, &dir).into()).collect();
    let extra_env: Vec<(OsString, OsString)> = m.env.iter().map(|(k, v)| (k.into(), expand(v, &dir).into())).collect();
    let sdk = sdk_dirs();
    let launcher = move || -> Result<Launch, ModuleError> {
        let (program, pre_args) = interpreter(&configured).ok_or_else(|| {
            ModuleError::other("no Python 3 interpreter found; install Python or set its path in the settings")
        })?;
        let mut args: Vec<OsString> = pre_args.into_iter().map(OsString::from).collect();
        args.push(script.clone().into_os_string());
        args.extend(extra_args.iter().cloned());
        let mut env: Vec<(OsString, OsString)> = vec![
            ("PYTHONIOENCODING".into(), "utf-8".into()),
            ("PYTHONUTF8".into(), "1".into()),
            ("PYTHONUNBUFFERED".into(), "1".into()),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ];
        // The module's own directory is already first on sys.path (it holds
        // the script), so a vendored glossa_module.py wins over ours.
        let mut path: Vec<PathBuf> = sdk.clone();
        if let Some(existing) = std::env::var_os("PYTHONPATH") {
            path.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(&path) {
            env.push(("PYTHONPATH".into(), joined));
        }
        env.extend(extra_env.iter().cloned());
        Ok(Launch { program, args, cwd: dir.clone(), env })
    };
    Ok(StdioTransport::new(cfg, Box::new(launcher)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Transport;
    use crate::provider::{with_test_host, OptionValues};
    use glossa_core::{LangId, LangPair, TranslateRequest};
    use glossa_module_sdk::wire::ErrorKind;
    use glossa_module_sdk::wire::Reply;
    use glossa_net::mock::MockHttp;
    use serde_json::Value;
    use std::sync::mpsc;

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

        fn transport(&self, timeout: Duration) -> StdioTransport {
            transport(&self.dir, &self.manifest, &ctx(timeout)).unwrap()
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

    fn translate(t: &StdioTransport, text: &str) -> Result<Value, ModuleError> {
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
        assert!(transport(&m.dir, &m.manifest, &ctx(Duration::from_secs(1))).is_err());
    }

    #[test]
    fn the_bundled_sdk_is_found() {
        assert!(sdk_dirs().iter().any(|d| d.join("glossa_module.py").is_file()));
    }
}
