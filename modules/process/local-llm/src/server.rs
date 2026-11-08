//! Starting and stopping llama-server.
//!
//! llama-server runs under a small supervisor, this same program started
//! with `--supervise`, whose stdin is a pipe from the module. When the
//! module goes away for any reason (stdin closed by Glossa, a crash, or
//! Glossa killing it after a call timeout) that pipe closes, and the
//! supervisor kills llama-server. So no model is left loaded behind a dead
//! module, on every system, without signals, job objects or `libc`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::http;

/// How llama-server is started. A request whose settings differ from the
/// running server's restarts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The program, and arguments before ours (the test server's flag).
    pub program: Vec<String>,
    pub model: PathBuf,
    pub context: u32,
    pub threads: u32,
    pub gpu: bool,
}

impl Config {
    /// llama-server's command line, after the program.
    pub fn args(&self, port: u16) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "--model".into(),
            self.model.display().to_string(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--ctx-size".into(),
            self.context.to_string(),
            // One request at a time is all Glossa sends; one slot keeps
            // the KV cache to a single context.
            "--parallel".into(),
            "1".into(),
            "--n-gpu-layers".into(),
            if self.gpu { "all" } else { "0" }.into(),
            // No prompt cache in RAM (8 GB by default), no web page, no
            // thinking (Qwen3), and only warnings and errors in the log.
            "--cache-ram".into(),
            "0".into(),
            "--no-webui".into(),
            "--reasoning".into(),
            "off".into(),
            "--log-verbosity".into(),
            "2".into(),
        ];
        if self.threads > 0 {
            a.push("--threads".into());
            a.push(self.threads.to_string());
        }
        a
    }
}

/// A running llama-server (through its supervisor).
pub struct Running {
    pub config: Config,
    pub port: u16,
    supervisor: Child,
    log: Arc<Mutex<VecDeque<String>>>,
}

/// How long the model may take to load. Glossa's call timeout (30 s by
/// default) usually ends the call first.
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);

impl Running {
    /// Start llama-server and wait until it has loaded the model.
    pub fn start(config: Config) -> Result<Running, String> {
        let program = config.program.first().ok_or("no llama-server program")?;
        // A real llama-server (the test server is this program with a flag).
        #[cfg(unix)]
        if config.program.len() == 1 {
            if let Some(dir) = Path::new(program).parent() {
                fix_library_links(dir);
            }
        }
        let port = free_port()?;
        let me = std::env::current_exe().map_err(|e| format!("finding this program: {e}"))?;
        let mut cmd = Command::new(me);
        cmd.arg("--supervise")
            .args(&config.program)
            .args(config.args(port))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut supervisor = cmd.spawn().map_err(|e| format!("starting {program}: {e}"))?;
        let log = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(err) = supervisor.stderr.take() {
            let log = Arc::clone(&log);
            std::thread::spawn(move || forward_log(err, &log));
        }
        let mut running = Running { config, port, supervisor, log };
        running.wait_until_loaded()?;
        Ok(running)
    }

    fn wait_until_loaded(&mut self) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = self.supervisor.try_wait() {
                return Err(format!("llama-server stopped while loading the model ({status}){}", self.last_lines()));
            }
            // 503 while the model loads, 200 once it is ready.
            if let Ok(r) = http::request(self.port, "GET", "/health", None, Duration::from_secs(5)) {
                if r.status == 200 {
                    eprintln!(
                        "local-llm: llama-server ready on port {} in {} ms",
                        self.port,
                        started.elapsed().as_millis()
                    );
                    return Ok(());
                }
            }
            if started.elapsed() > LOAD_TIMEOUT {
                return Err(format!(
                    "llama-server did not load the model within {} s{}",
                    LOAD_TIMEOUT.as_secs(),
                    self.last_lines()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether the server is still there.
    pub fn alive(&mut self) -> bool {
        matches!(self.supervisor.try_wait(), Ok(None))
    }

    /// The last lines llama-server logged, for an error message.
    pub fn last_lines(&self) -> String {
        let log = self.log.lock().map(|l| l.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
        if log.is_empty() {
            String::new()
        } else {
            format!(": {}", log.join(" | "))
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Closing its stdin tells the supervisor to kill llama-server.
        drop(self.supervisor.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !matches!(self.supervisor.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.supervisor.kill();
        let _ = self.supervisor.wait();
    }
}

/// Pass llama-server's log on to Glossa's (stderr lines are logged under
/// the module's id) and keep the last few for error messages.
fn forward_log(err: impl Read, log: &Mutex<VecDeque<String>>) {
    for line in BufReader::new(err).lines().map_while(Result::ok) {
        let line = line.trim_end().to_owned();
        if line.is_empty() {
            continue;
        }
        eprintln!("llama-server: {line}");
        if let Ok(mut l) = log.lock() {
            if l.len() == 4 {
                l.pop_front();
            }
            l.push_back(line);
        }
    }
}

/// A free port on 127.0.0.1. Another program could take it before
/// llama-server binds it; that start then fails and the next call retries.
fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| format!("finding a free port: {e}"))?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: Glossa runs without a console, and a console
    // program started from it would otherwise open a window of its own.
    cmd.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn no_window(_: &mut Command) {}

/// `--supervise PROGRAM ARGS…`: run llama-server until stdin closes (the
/// module is gone) or llama-server exits, whichever comes first.
pub fn supervise(args: &[String]) -> i32 {
    let Some((program, rest)) = args.split_first() else {
        eprintln!("local-llm: --supervise needs a program");
        return 2;
    };
    let mut cmd = Command::new(program);
    cmd.args(rest).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    no_window(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("local-llm: starting {program}: {e}");
            return 1;
        }
    };
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let _ = tx.send(());
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(1),
            Ok(None) => {}
            Err(_) => return 1,
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return 0;
            }
        }
    }
}

/// The llama-server program in the unpacked llama.cpp archive: at its top
/// (Windows) or one folder down (`llama-b<build>/`, macOS and Linux).
pub fn find_server(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let mut subdirs: Vec<PathBuf> =
        std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subdirs.sort();
    subdirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Glossa unpacks archives without their symbolic links, and the llama.cpp
/// builds for macOS and Linux link their libraries by version links
/// (`libllama.0.dylib` → `libllama.0.9.0.dylib`, `libllama.so.0` →
/// `libllama.so.0.9.0`). Put them back beside the real files. Glossa's own
/// checks look only at the archive's regular files, so the links do not
/// disturb them.
#[cfg(unix)]
pub fn fix_library_links(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        for link in version_links(&name) {
            let path = dir.join(&link);
            if std::fs::symlink_metadata(&path).is_err() {
                if let Err(err) = std::os::unix::fs::symlink(&name, &path) {
                    eprintln!("local-llm: linking {link} to {name}: {err}");
                }
            }
        }
    }
}

/// The version links a library file needs: `libx.1.2.3.dylib` gives
/// `libx.1.dylib` and `libx.dylib`; `libx.so.1.2.3` gives `libx.so.1` and
/// `libx.so`. Anything else needs none.
#[cfg(any(unix, test))]
pub fn version_links(name: &str) -> Vec<String> {
    let numeric = |parts: &[&str]| {
        !parts.is_empty() && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    if !name.starts_with("lib") {
        return Vec::new();
    }
    if let Some(stem) = name.strip_suffix(".dylib") {
        let parts: Vec<&str> = stem.split('.').collect();
        if parts.len() >= 4 && numeric(&parts[parts.len() - 3..]) {
            let base = parts[..parts.len() - 3].join(".");
            return vec![format!("{base}.{}.dylib", parts[parts.len() - 3]), format!("{base}.dylib")];
        }
        return Vec::new();
    }
    if let Some(i) = name.find(".so.") {
        let base = &name[..i + 3];
        let version: Vec<&str> = name[i + 4..].split('.').collect();
        if version.len() >= 2 && numeric(&version) {
            return vec![format!("{base}.{}", version[0]), base.to_owned()];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_links_for_both_naming_schemes() {
        assert_eq!(version_links("libllama.0.9.0.dylib"), ["libllama.0.dylib", "libllama.dylib"]);
        assert_eq!(version_links("libggml-base.0.29.0.dylib"), ["libggml-base.0.dylib", "libggml-base.dylib"]);
        assert_eq!(version_links("libllama.so.0.9.0"), ["libllama.so.0", "libllama.so"]);
        assert_eq!(version_links("libllama-common.so.0.9.0"), ["libllama-common.so.0", "libllama-common.so"]);
        assert!(version_links("libllama-server-impl.dylib").is_empty());
        assert!(version_links("libllama.0.dylib").is_empty());
        assert!(version_links("libggml-cpu-haswell.so").is_empty());
        assert!(version_links("libllama.so.0").is_empty());
        assert!(version_links("llama-server").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn links_are_made_once_beside_the_libraries() {
        let dir = std::env::temp_dir().join(format!("local-llm-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libllama.0.9.0.dylib"), b"x").unwrap();
        std::fs::write(dir.join("llama-server"), b"x").unwrap();
        fix_library_links(&dir);
        fix_library_links(&dir); // again: nothing to do, no error
        assert_eq!(std::fs::read_link(dir.join("libllama.0.dylib")).unwrap(), Path::new("libllama.0.9.0.dylib"));
        assert_eq!(std::fs::read(dir.join("libllama.dylib")).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_server_is_found_at_the_top_or_one_folder_down() {
        let dir = std::env::temp_dir().join(format!("local-llm-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        std::fs::create_dir_all(dir.join("llama-b1")).unwrap();
        assert_eq!(find_server(&dir), None);
        std::fs::write(dir.join("llama-b1").join(name), b"x").unwrap();
        assert_eq!(find_server(&dir), Some(dir.join("llama-b1").join(name)));
        std::fs::write(dir.join(name), b"x").unwrap();
        assert_eq!(find_server(&dir), Some(dir.join(name)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arguments_follow_the_settings() {
        let c = Config { program: vec!["s".into()], model: "m.gguf".into(), context: 2048, threads: 0, gpu: false };
        let a = c.args(1234).join(" ");
        assert!(a.contains("--port 1234") && a.contains("--ctx-size 2048") && a.contains("--n-gpu-layers 0"), "{a}");
        assert!(a.contains("--host 127.0.0.1") && !a.contains("--threads"), "{a}");
        let c = Config { threads: 4, gpu: true, ..c };
        let a = c.args(1).join(" ");
        assert!(a.contains("--threads 4") && a.contains("--n-gpu-layers all"), "{a}");
    }
}
