//! One running copy per user, session and data folder, and a way for a
//! second launch to hand its request to that copy.
//!
//! The lock and the channel are local to the user's session and do not
//! depend on the control API, which may be switched off, may have lost its
//! port to another program, or may belong to another user's Glossa on the
//! same machine:
//!
//! - **Windows**: a named mutex `Local\Glossa-<key>` (the `Local\` namespace
//!   is per logon session) and a named pipe `\\.\pipe\glossa-<session>-<key>`
//!   (pipe names are machine-wide, hence the session id) that rejects remote
//!   clients and, by the default pipe security, takes writes only from the
//!   same user, administrators and the system.
//! - **Linux and the BSDs**: `flock` on `glossa-<key>.lock` and a Unix socket
//!   `glossa-<key>.sock` (mode 0600) in `$XDG_RUNTIME_DIR`, which is the
//!   user's own and lives as long as the login.
//! - **macOS**: the same in `$TMPDIR`, a per-user folder.
//!
//! Where neither runtime folder exists, the cache folder holds both. `<key>`
//! is a hash of the user's name and the data folder, so copies with
//! different `GLOSSA_HOME` folders (test setups) run side by side, and two
//! users sharing a portable folder do not meet.
//!
//! The message is a short frame: a 4-byte little-endian length, then
//! `v1\n<action id>\n<0|1: has text>\n<text>`; the primary answers `OK`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glossa_core::{Action, Paths};

/// What a later launch asks the running copy to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub action: Action,
    /// Text to act on instead of the selection.
    pub text: Option<String>,
}

impl Message {
    pub fn show() -> Message {
        Message { action: Action::ShowMainWindow, text: None }
    }

    fn encode(&self) -> Vec<u8> {
        let text = self.text.as_deref();
        let body = format!("v1\n{}\n{}\n{}", self.action.id(), u8::from(text.is_some()), text.unwrap_or(""));
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(body.as_bytes());
        out
    }

    fn decode(body: &[u8]) -> Option<Message> {
        let body = std::str::from_utf8(body).ok()?;
        let mut parts = body.splitn(4, '\n');
        if parts.next()? != "v1" {
            return None;
        }
        let action = Action::from_id(parts.next()?)?;
        let has_text = parts.next()? == "1";
        let text = parts.next().unwrap_or("");
        Some(Message { action, text: has_text.then(|| text.to_owned()) })
    }
}

/// The largest message accepted: text for the popup, not a file.
const MAX_MESSAGE: u32 = 4 << 20;

/// Read one frame: its length, then its body.
fn read_frame(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// The outcome of [`acquire`].
pub enum Instance {
    /// This process is the one; keep the guard for as long as it runs.
    Primary(InstanceGuard),
    /// Another copy holds the lock: hand over with [`send`] and leave.
    Secondary,
}

type Handler = Box<dyn Fn(Message) + Send>;

/// Messages received before a handler was set wait here.
#[derive(Default)]
struct Inbox {
    handler: Option<Handler>,
    waiting: Vec<Message>,
}

impl Inbox {
    fn deliver(&mut self, m: Message) {
        match &self.handler {
            Some(h) => h(m),
            None => self.waiting.push(m),
        }
    }
}

/// Held by the primary copy. Dropping it releases the lock and stops
/// listening, so a restarted copy can take over at once.
pub struct InstanceGuard {
    inbox: Arc<Mutex<Inbox>>,
    stop: Arc<AtomicBool>,
    listener: Option<std::thread::JoinHandle<()>>,
    imp: imp::Lock,
}

impl InstanceGuard {
    /// Call `f` for each message from a later launch, starting with those
    /// that arrived before this was set. `f` runs on the listener's thread.
    pub fn set_handler(&self, f: impl Fn(Message) + Send + 'static) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        for m in std::mem::take(&mut inbox.waiting) {
            f(m);
        }
        inbox.handler = Some(Box::new(f));
    }

    /// Whether later launches can reach this copy (the pipe or socket could
    /// be created).
    pub fn listening(&self) -> bool {
        self.listener.is_some()
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.listener.take() {
            // Wake the listener out of its blocking accept with a
            // connection of our own; it sees the flag and leaves.
            self.imp.wake_listener();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !t.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
        self.imp.release();
    }
}

/// Take the single-instance lock for this user, session and data folder.
/// A primary starts listening for later launches straight away; messages
/// wait until [`InstanceGuard::set_handler`]. Where the platform offers no
/// lock at all, every launch is primary.
pub fn acquire(paths: &Paths) -> Instance {
    let names = Names::for_paths(paths);
    match imp::Lock::acquire(&names) {
        None => Instance::Secondary,
        Some((lock, server)) => {
            let inbox = Arc::new(Mutex::new(Inbox::default()));
            let stop = Arc::new(AtomicBool::new(false));
            let listener = server.and_then(|server| {
                let (inbox, stop) = (inbox.clone(), stop.clone());
                std::thread::Builder::new()
                    .name("glossa-instance".into())
                    .spawn(move || {
                        imp::serve(server, &stop, &mut |m| inbox.lock().unwrap_or_else(|e| e.into_inner()).deliver(m))
                    })
                    .map_err(|e| tracing::warn!("single-instance listener not started: {e}"))
                    .ok()
            });
            Instance::Primary(InstanceGuard { inbox, stop, listener, imp: lock })
        }
    }
}

/// Whether a copy holds the lock for this data folder, without taking it
/// (for `glossa-cli doctor`). On Unix the lock is tested by taking it for
/// an instant.
pub fn is_running(paths: &Paths) -> bool {
    imp::is_locked(&Names::for_paths(paths))
}

/// Hand `message` to the primary copy for this data folder. Retries until
/// `timeout` while the primary is still starting; an error means no copy
/// answered.
pub fn send(paths: &Paths, message: &Message, timeout: Duration) -> std::io::Result<()> {
    let names = Names::for_paths(paths);
    let frame = message.encode();
    let deadline = Instant::now() + timeout;
    // The exchange runs on a thread of its own so that a primary that
    // accepted the connection and then hung cannot hang this launch.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let mut conn = loop {
                match imp::connect(&names) {
                    Ok(c) => break c,
                    Err(e) if Instant::now() >= deadline => return Err(e),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            };
            conn.write_all(&frame)?;
            conn.flush()?;
            let mut ack = [0u8; 2];
            conn.read_exact(&mut ack)?;
            if &ack == b"OK" {
                Ok(())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "unexpected answer"))
            }
        })();
        let _ = tx.send(result);
    });
    let wait = deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(1);
    rx.recv_timeout(wait)
        .unwrap_or_else(|_| Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the running copy did not answer")))
}

/// Handle one connection on the primary's side: read the frame, deliver it,
/// answer `OK`. An empty frame is the guard waking its own listener.
fn handle_connection(conn: &mut (impl Read + Write), deliver: &mut dyn FnMut(Message)) {
    match read_frame(conn) {
        Ok(body) if body.is_empty() => {}
        Ok(body) => match Message::decode(&body) {
            Some(m) => {
                let _ = conn.write_all(b"OK");
                let _ = conn.flush();
                tracing::info!(action = m.action.id(), "a later launch handed over");
                deliver(m);
            }
            None => tracing::warn!("single-instance channel: a message that does not parse"),
        },
        Err(e) => tracing::debug!("single-instance channel: {e}"),
    }
}

/// The names of the lock and the channel.
#[derive(Debug, Clone)]
struct Names {
    /// The hash naming everything: user and data folder.
    key: String,
    /// Where the Unix lock file and socket go.
    #[cfg_attr(windows, allow(dead_code))]
    dir: PathBuf,
}

impl Names {
    fn for_paths(paths: &Paths) -> Names {
        let user = ["USERDOMAIN", "USERNAME", "USER", "LOGNAME"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .collect::<Vec<_>>()
            .join("\\");
        Names { key: instance_key(&user, &paths.data_dir), dir: runtime_dir(&paths.cache_dir) }
    }
}

/// The hash in every name: FNV-1a, stable across builds and versions (a
/// newer Glossa must find an older one's lock), over the user and the data
/// folder (made absolute; on Windows, where paths ignore case, lower-cased).
fn instance_key(user: &str, data_dir: &Path) -> String {
    let dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let mut dir = dir.to_string_lossy().into_owned();
    if cfg!(windows) {
        dir = dir.to_lowercase();
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in user.bytes().chain([0]).chain(dir.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The per-user folder for the lock file and socket on Unix systems.
fn runtime_dir(cache_dir: &Path) -> PathBuf {
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute() && p.is_dir());
    let found = if cfg!(target_os = "macos") { var("TMPDIR") } else { var("XDG_RUNTIME_DIR") };
    found.unwrap_or_else(|| cache_dir.to_path_buf())
}

#[cfg(windows)]
mod imp {
    use std::fs::File;
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

    use super::{handle_connection, Message, Names};

    pub struct Lock {
        mutex: HANDLE,
        pipe: String,
    }

    // SAFETY: a mutex handle is a kernel object reference, usable from any
    // thread; it is only closed once, in `release`.
    unsafe impl Send for Lock {}

    pub struct Server {
        pipe: String,
        /// The first instance, created while acquiring so a later launch can
        /// connect before the listener thread runs.
        first: HANDLE,
    }

    // SAFETY: as for `Lock`; the handle moves to the listener thread once.
    unsafe impl Send for Server {}

    fn pipe_name(names: &Names) -> String {
        let mut session = 0u32;
        // SAFETY: plain out-parameter call.
        let _ = unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
        format!(r"\\.\pipe\glossa-{session}-{}", names.key)
    }

    fn create_pipe(name: &str, first: bool) -> Option<HANDLE> {
        let mode = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
        // SAFETY: the name outlives the call; default security (the owner,
        // administrators and SYSTEM may write).
        let h = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(name),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                65536,
                0,
                None,
            )
        };
        (h != INVALID_HANDLE_VALUE && !h.is_invalid()).then_some(h)
    }

    impl Lock {
        pub fn acquire(names: &Names) -> Option<(Lock, Option<Server>)> {
            let name = HSTRING::from(format!(r"Local\Glossa-{}", names.key));
            // SAFETY: the name outlives the call; the handle is closed in
            // `release` (or right here for a secondary).
            let mutex = match unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) } {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("single-instance mutex: {e}; running without the lock");
                    return Some((Lock { mutex: HANDLE::default(), pipe: String::new() }, None));
                }
            };
            // SAFETY: read right after the call that set it.
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // SAFETY: our own handle, closed once.
                unsafe {
                    let _ = CloseHandle(mutex);
                }
                return None;
            }
            let pipe = pipe_name(names);
            // A restarting copy may still be closing its pipe: try for a
            // moment before going without.
            let mut first = None;
            for _ in 0..20 {
                first = create_pipe(&pipe, true);
                if first.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if first.is_none() {
                tracing::warn!("single-instance pipe {pipe} could not be created; later launches use the control API");
            }
            let server = first.map(|first| Server { pipe: pipe.clone(), first });
            Some((Lock { mutex, pipe }, server))
        }

        pub fn wake_listener(&self) {
            // Between two clients the listener briefly has no pipe instance
            // to connect to; try again for a moment.
            for _ in 0..10 {
                if let Ok(mut f) = std::fs::OpenOptions::new().read(true).write(true).open(&self.pipe) {
                    use std::io::Write;
                    let _ = f.write_all(&0u32.to_le_bytes());
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        pub fn release(&mut self) {
            if !self.mutex.is_invalid() {
                // SAFETY: our own handle, closed once.
                unsafe {
                    let _ = CloseHandle(self.mutex);
                }
                self.mutex = HANDLE::default();
            }
        }
    }

    pub fn serve(server: Server, stop: &AtomicBool, deliver: &mut dyn FnMut(Message)) {
        let mut next = Some(server.first);
        while !stop.load(Ordering::SeqCst) {
            let Some(h) = next.take().or_else(|| create_pipe(&server.pipe, false)) else {
                tracing::warn!("single-instance pipe lost; later launches use the control API");
                return;
            };
            // SAFETY: a valid pipe handle; blocking connect.
            let connected = unsafe { ConnectNamedPipe(h, None) }.is_ok()
                // SAFETY: read right after the call that set it.
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            // SAFETY: the handle is ours; the File owns and closes it.
            let mut file = unsafe { File::from_raw_handle(h.0) };
            if connected && !stop.load(Ordering::SeqCst) {
                handle_connection(&mut file, deliver);
                // SAFETY: still the pipe handle owned by `file`.
                let _ = unsafe { DisconnectNamedPipe(h) };
            }
            drop(file);
        }
    }

    pub fn connect(names: &Names) -> std::io::Result<File> {
        std::fs::OpenOptions::new().read(true).write(true).open(pipe_name(names))
    }

    pub fn is_locked(names: &Names) -> bool {
        let name = HSTRING::from(format!(r"Local\Glossa-{}", names.key));
        // SAFETY: the name outlives the call; the handle is closed at once.
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) } {
            Ok(h) => {
                // SAFETY: our own handle, closed once.
                unsafe {
                    let _ = CloseHandle(h);
                }
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{handle_connection, Message, Names};

    pub struct Lock {
        file: Option<File>,
        socket: PathBuf,
        listening: bool,
    }

    pub type Server = UnixListener;

    fn paths(names: &Names) -> (PathBuf, PathBuf) {
        let base = format!("glossa-{}", names.key);
        (names.dir.join(format!("{base}.lock")), names.dir.join(format!("{base}.sock")))
    }

    impl Lock {
        pub fn acquire(names: &Names) -> Option<(Lock, Option<Server>)> {
            let (lock_path, socket) = paths(names);
            let _ = std::fs::create_dir_all(&names.dir);
            let file = match std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("single-instance lock {}: {e}; running without it", lock_path.display());
                    let lock = Lock { file: None, socket, listening: false };
                    return Some((lock, None));
                }
            };
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return None,
                Err(std::fs::TryLockError::Error(e)) => {
                    tracing::warn!("single-instance lock {}: {e}; running without it", lock_path.display());
                    let lock = Lock { file: None, socket, listening: false };
                    return Some((lock, None));
                }
            }
            // Holding the lock, any socket file left is from a copy that
            // crashed.
            let _ = std::fs::remove_file(&socket);
            let server = match UnixListener::bind(&socket) {
                Ok(l) => {
                    let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
                    Some(l)
                }
                Err(e) => {
                    tracing::warn!(
                        "single-instance socket {}: {e}; later launches use the control API",
                        socket.display()
                    );
                    None
                }
            };
            let listening = server.is_some();
            Some((Lock { file: Some(file), socket, listening }, server))
        }

        pub fn wake_listener(&self) {
            if let Ok(mut s) = UnixStream::connect(&self.socket) {
                use std::io::Write;
                let _ = s.write_all(&0u32.to_le_bytes());
            }
        }

        pub fn release(&mut self) {
            if self.listening {
                let _ = std::fs::remove_file(&self.socket);
            }
            // Closing the file drops the lock; the file itself stays, so
            // another copy never locks a file that is being deleted.
            self.file = None;
        }
    }

    pub fn serve(server: Server, stop: &AtomicBool, deliver: &mut dyn FnMut(Message)) {
        for conn in server.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let Ok(mut conn) = conn else { continue };
            let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = conn.set_write_timeout(Some(Duration::from_secs(2)));
            handle_connection(&mut conn, deliver);
        }
    }

    pub fn connect(names: &Names) -> std::io::Result<UnixStream> {
        let s = UnixStream::connect(paths(names).1)?;
        s.set_read_timeout(Some(Duration::from_secs(3)))?;
        s.set_write_timeout(Some(Duration::from_secs(3)))?;
        Ok(s)
    }

    pub fn is_locked(names: &Names) -> bool {
        let Ok(file) = std::fs::OpenOptions::new().write(true).open(paths(names).0) else { return false };
        matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use std::sync::atomic::AtomicBool;

    use super::{Message, Names};

    pub struct Lock;
    pub struct Server;

    impl Lock {
        pub fn acquire(_: &Names) -> Option<(Lock, Option<Server>)> {
            Some((Lock, None))
        }
        pub fn wake_listener(&self) {}
        pub fn release(&mut self) {}
    }

    pub fn serve(_: Server, _: &AtomicBool, _: &mut dyn FnMut(Message)) {}

    pub fn is_locked(_: &Names) -> bool {
        false
    }

    pub fn connect(_: &Names) -> std::io::Result<std::io::Cursor<Vec<u8>>> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no single-instance channel on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> (Paths, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-instance-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        (Paths::for_exe_dir(dir.clone(), Some(dir.join("home"))), dir)
    }

    #[test]
    fn messages_round_trip() {
        for m in [
            Message::show(),
            Message { action: Action::ShowPopup, text: Some("two\nlines".into()) },
            Message { action: Action::ShowPopup, text: Some(String::new()) },
        ] {
            let frame = m.encode();
            let body = read_frame(&mut frame.as_slice()).unwrap();
            assert_eq!(Message::decode(&body), Some(m));
        }
        assert_eq!(Message::decode(b"v2\nshow_popup\n0\n"), None);
        assert_eq!(Message::decode(b"v1\nself_destruct\n0\n"), None);
        let huge = (MAX_MESSAGE + 1).to_le_bytes();
        assert!(read_frame(&mut huge.as_slice()).is_err());
    }

    /// One end of a connection: what the other side sent, and what this
    /// side writes back.
    struct Conn {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Conn {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Conn {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn served(input: Vec<u8>) -> (Vec<Message>, Vec<u8>) {
        let mut conn = Conn { input: std::io::Cursor::new(input), output: Vec::new() };
        let mut got = Vec::new();
        handle_connection(&mut conn, &mut |m| got.push(m));
        (got, conn.output)
    }

    #[test]
    fn a_good_message_is_acknowledged_and_delivered() {
        let m = Message { action: Action::ShowPopup, text: Some("hi".into()) };
        let (got, reply) = served(m.encode());
        assert_eq!(got, vec![m]);
        assert_eq!(reply, b"OK", "the sender waits for this before it exits");
    }

    #[test]
    fn bad_or_empty_frames_are_neither_acknowledged_nor_delivered() {
        // An empty frame is how the listener is woken to shut down.
        let (got, reply) = served(0u32.to_le_bytes().to_vec());
        assert!(got.is_empty() && reply.is_empty());
        // Unknown action: a newer build's message an older one cannot act on.
        let body = b"v1\nfuture_action\n0\n";
        let mut frame = (body.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(body);
        let (got, reply) = served(frame);
        assert!(got.is_empty() && reply.is_empty());
        // A frame cut short.
        let mut cut = Message::show().encode();
        cut.truncate(cut.len() - 2);
        let (got, reply) = served(cut);
        assert!(got.is_empty() && reply.is_empty());
    }

    #[test]
    fn decoding_is_strict_about_the_header_and_lenient_about_the_text() {
        // The text flag decides, whatever follows it.
        assert_eq!(
            Message::decode(b"v1\nshow_popup\n0\nignored"),
            Some(Message { action: Action::ShowPopup, text: None })
        );
        assert_eq!(
            Message::decode(b"v1\nshow_popup\n1"),
            Some(Message { action: Action::ShowPopup, text: Some(String::new()) }),
            "a flagged but absent text is empty"
        );
        assert_eq!(Message::decode(b"v1\nshow_popup"), None, "the flag is required");
        assert_eq!(Message::decode(b""), None);
        assert_eq!(Message::decode(&[0xff, 0xfe]), None, "not UTF-8");
        // Text keeps every newline and character it had.
        let text = "a\nb\n\nc\r\n — ✓";
        let m = Message { action: Action::ShowPopup, text: Some(text.into()) };
        let frame = m.encode();
        assert_eq!(u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize, frame.len() - 4, "length prefix");
        assert_eq!(Message::decode(&frame[4..]).unwrap().text.as_deref(), Some(text));
    }

    #[test]
    fn frames_up_to_the_limit_are_read_and_short_reads_fail() {
        let mut exact = MAX_MESSAGE.to_le_bytes().to_vec();
        exact.extend(std::iter::repeat_n(b'x', MAX_MESSAGE as usize));
        assert_eq!(read_frame(&mut exact.as_slice()).unwrap().len(), MAX_MESSAGE as usize);
        assert!(read_frame(&mut [1u8, 0].as_slice()).is_err(), "a cut length");
        let mut short = 10u32.to_le_bytes().to_vec();
        short.extend_from_slice(b"abc");
        assert!(read_frame(&mut short.as_slice()).is_err(), "a cut body");
    }

    #[test]
    fn keys_differ_by_data_folder_and_user_and_are_stable() {
        let a = instance_key("me", Path::new("/data/one"));
        assert_eq!(a, instance_key("me", Path::new("/data/one")));
        assert_ne!(a, instance_key("me", Path::new("/data/two")));
        assert_ne!(a, instance_key("you", Path::new("/data/one")));
        assert_eq!(a.len(), 16);
        // FNV-1a must not change between versions: a newer copy has to
        // find an older one.
        assert_eq!(instance_key("", Path::new("")), "af63bd4c8601b7df");
    }

    #[test]
    fn a_second_launch_hands_over_and_a_different_home_does_not() {
        let (paths, dir) = temp_paths("handover");
        let Instance::Primary(guard) = acquire(&paths) else { panic!("the first is primary") };
        assert!(guard.listening());
        // Sent before the handler exists: kept for it.
        send(&paths, &Message { action: Action::ShowPopup, text: Some("early".into()) }, Duration::from_secs(5))
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        guard.set_handler(move |m| {
            let _ = tx.send(m);
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().text.as_deref(), Some("early"));

        assert!(matches!(acquire(&paths), Instance::Secondary), "the same home is taken");
        assert!(is_running(&paths));
        send(&paths, &Message::show(), Duration::from_secs(5)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Message::show());

        let (other, other_dir) = temp_paths("handover-other");
        let Instance::Primary(other_guard) = acquire(&other) else { panic!("another home is its own instance") };
        drop(other_guard);

        // Released: the next launch is primary again, and nobody answers
        // on the old channel.
        drop(guard);
        assert!(send(&paths, &Message::show(), Duration::from_millis(200)).is_err());
        assert!(!is_running(&paths));
        let Instance::Primary(again) = acquire(&paths) else { panic!("the lock was released") };
        assert!(again.listening());
        drop(again);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&other_dir).ok();
    }
}
