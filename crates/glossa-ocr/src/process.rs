//! Running a helper program with a deadline.
//!
//! Both the chrome-lens CLI and a user's OCR command are external programs
//! that may hang (a stuck network call, a prompt nobody will answer). They run
//! with stdin closed, their output collected on threads so a chatty stderr
//! cannot fill a pipe and stall them, and they are killed when the deadline
//! passes or the user cancels.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use glossa_core::{Error, Result};
use glossa_modules::CancelToken;

/// A finished program.
#[derive(Debug, Default)]
pub struct Output {
    /// `None` when it was ended by a signal.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// The last few lines of stderr, for an error message.
    pub fn stderr_tail(&self) -> String {
        tail(&String::from_utf8_lossy(&self.stderr), 6, 600)
    }
}

/// Output beyond this is dropped: OCR text is never this large, and a runaway
/// program must not take our memory with it.
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;

/// Keep consoles from flashing up on Windows.
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Run `cmd` to completion, killing it after `timeout` or on cancel.
pub fn run(mut cmd: Command, timeout: Duration, cancel: Option<&CancelToken>) -> Result<Output> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut attempt = 0;
    let mut child = loop {
        match cmd.spawn() {
            Ok(c) => break c,
            // ETXTBSY: on Linux a script that was written a moment ago can
            // still be open in a process another thread is forking. It passes.
            Err(e) if e.raw_os_error() == Some(26) && cfg!(target_os = "linux") && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::FileNotFound(program)),
            Err(e) => return Err(Error::Platform(format!("cannot start {program}: {e}"))),
        }
    };
    let stdout = collect(child.stdout.take());
    let stderr = collect(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(e) => {
                kill(&mut child);
                return Err(Error::Platform(format!("{program}: {e}")));
            }
        }
        if cancel.is_some_and(|c| c.is_cancelled()) {
            kill(&mut child);
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            kill(&mut child);
            return Err(Error::Timeout(timeout.as_millis() as u64));
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    Ok(Output { code, stdout: join(stdout), stderr: join(stderr) })
}

fn collect<R: Read + Send + 'static>(pipe: Option<R>) -> Option<Receiver<Vec<u8>>> {
    pipe.map(|p| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut limited = p.take(MAX_OUTPUT);
            let _ = limited.read_to_end(&mut buf);
            let _ = tx.send(buf);
            // Keep draining past the limit so the child is never blocked on a
            // full pipe.
            let _ = std::io::copy(&mut limited.into_inner(), &mut std::io::sink());
        });
        rx
    })
}

/// What a pipe carried. A grandchild that inherited the pipe can hold it open
/// after the child exits, so the wait is bounded rather than "until EOF".
fn join(rx: Option<Receiver<Vec<u8>>>) -> Vec<u8> {
    rx.and_then(|rx| rx.recv_timeout(Duration::from_secs(2)).ok()).unwrap_or_default()
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The last `lines` lines of `text`, at most `max_chars` characters.
pub fn tail(text: &str, lines: usize, max_chars: usize) -> String {
    let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = all.len().saturating_sub(lines);
    let joined = all[start..].join("\n");
    let count = joined.chars().count();
    if count <= max_chars {
        joined
    } else {
        joined.chars().skip(count - max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("a\nb\n\nc\nd", 2, 100), "c\nd");
        assert_eq!(tail("abcdef", 5, 3), "def");
        assert_eq!(tail("", 3, 10), "");
    }

    #[cfg(unix)]
    #[test]
    fn output_and_exit_code_are_collected() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
        let out = run(cmd, Duration::from_secs(10), None).unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "out\n");
        assert_eq!(out.stderr_tail(), "err");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_program_is_killed_at_the_deadline() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let err = run(cmd, Duration::from_millis(200), None).unwrap_err();
        assert!(matches!(err, Error::Timeout(200)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_kills_the_program() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        assert!(matches!(run(cmd, Duration::from_secs(30), Some(&cancel)), Err(Error::Cancelled)));
    }

    #[test]
    fn a_missing_program_is_file_not_found() {
        let cmd = Command::new("glossa-definitely-not-a-program");
        assert!(matches!(run(cmd, Duration::from_secs(1), None), Err(Error::FileNotFound(_))));
    }
}
