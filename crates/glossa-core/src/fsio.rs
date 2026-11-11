//! Writing the user's files so that they survive a crash and stay private.
//!
//! The settings file holds API keys, the proxy password and the control
//! API's token; the history holds what the user translated; a screenshot
//! for an OCR program holds whatever was on screen. On Unix everything
//! here is created for the user alone (folders `0700`, files `0600`),
//! whatever the umask says. On Windows the data folder is in the user's
//! profile, whose access list already admits only the user, the system and
//! administrators, and files created in it inherit that; nothing is
//! changed there (a portable copy on a shared drive is as private as the
//! folder it was put in).
//!
//! [`write_atomic`] is the one way a data file is replaced: a temporary
//! file of its own name in the same folder, written, flushed to the disk,
//! renamed over the original, and the folder flushed too, so a crash or a
//! power cut leaves either the old file or the new one, and two programs
//! saving at once cannot write into each other's temporary file.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A name no other call and no other process will choose: the process id,
/// a counter and the clock's nanoseconds.
fn unique(prefix: &str, suffix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{prefix}{}-{n}-{nanos:x}{suffix}", std::process::id())
}

/// Create `dir` (and its parents) and make it the user's alone.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    make_private(dir);
    Ok(())
}

/// Take away everyone else's access to an existing file or folder. Does
/// nothing on Windows (see the module's description) and nothing when the
/// path is missing or belongs to someone else.
pub fn make_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        // A link is left alone: its target may be a folder the user shares
        // on purpose.
        if meta.file_type().is_symlink() {
            return;
        }
        // Only the others' access goes. The owner's own bits stay as they
        // are: a data folder pointed at a folder with other things in it
        // (`GLOSSA_HOME=~`) must not cost a script its right to run.
        let now = meta.permissions().mode() & 0o777;
        let want = if meta.is_dir() { 0o700 } else { now & 0o700 };
        if now != want {
            if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(want)) {
                tracing::warn!("could not make {} private: {e}", path.display());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Create a new file for the user alone. Fails when the name is taken, so
/// a file (or a link) someone else put there is never written through.
pub fn create_private_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Replace `path` with `bytes`, durably and for the user alone (see the
/// module's description). The folder is created when missing.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, true)
}

/// [`write_atomic`] without waiting for the disk: for copies that are a
/// convenience rather than the only one (a backup made on every save).
pub fn write_atomic_unsynced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, false)
}

fn write_atomic_with(path: &Path, bytes: &[u8], sync: bool) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !dir.is_dir() {
        create_private_dir(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(unique(&format!(".{name}."), TEMP_SUFFIX));
    let written = (|| {
        let mut file = create_private_new(&tmp)?;
        file.write_all(bytes)?;
        if sync {
            file.sync_all()?;
        }
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    if sync {
        // The rename itself is in the folder; without this a power cut can
        // bring the old name back. Not every file system can do it.
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// What [`write_atomic`]'s temporary files end with.
const TEMP_SUFFIX: &str = ".tmp";

/// Remove what a killed process left in `dir`: [`write_atomic`]'s
/// temporary files (and the fixed-name ones of earlier versions, which
/// could hold the settings readable by everyone) older than `older_than`.
/// With `everything`, any plain file that old goes: for a folder that
/// holds nothing but temporary files.
pub fn sweep_stale(dir: &Path, older_than: Duration, everything: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = everything
            || (name.starts_with('.') && name.ends_with(TEMP_SUFFIX))
            || matches!(name.as_str(), "config.toml.tmp" | "history.json.tmp");
        if !ours {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age >= older_than);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// An advisory lock on a file beside the data it guards, held until
/// dropped: programs that take it before reading and replacing the data do
/// so one at a time.
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// Wait for the lock at `path` (created when missing). `None` where the
    /// file system has no locks or the file cannot be made: the caller goes
    /// on without one, as before there were locks.
    pub fn acquire(path: &Path) -> Option<FileLock> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).ok()?;
        match file.lock() {
            Ok(()) => Some(FileLock { file }),
            Err(e) => {
                tracing::debug!("no lock on {}: {e}", path.display());
                None
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

// -- temporary files ---------------------------------------------------------------

static TEMP_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Where this program's temporary files (a screenshot for an OCR program,
/// speech from the system voice) go from now on: a folder of its own under
/// the cache folder, rather than the temporary folder every user of the
/// machine shares.
pub fn set_temp_dir(dir: PathBuf) {
    *TEMP_DIR.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// The folder for temporary files, created private. Before
/// [`set_temp_dir`] (a tool, a test) this is a folder of the process's own
/// under the system's temporary folder, made by this process under a name
/// nobody could have taken first.
pub fn temp_dir() -> io::Result<PathBuf> {
    let chosen = TEMP_DIR.read().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(dir) = chosen {
        if !dir.is_dir() {
            create_private_dir(&dir)?;
        }
        return Ok(dir);
    }
    static OWN: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut own = OWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = own.as_ref().filter(|d| d.is_dir()) {
        return Ok(dir.clone());
    }
    let mut last = None;
    for _ in 0..8 {
        let dir = std::env::temp_dir().join(unique("glossa-tmp-", ""));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Not `create_dir_all`: a folder that is there already is someone
        // else's, and files put in it would be theirs to swap.
        match builder.create(&dir) {
            Ok(()) => {
                *own = Some(dir.clone());
                return Ok(dir);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free temporary name")))
}

/// A new, empty, private file in [`temp_dir`] whose name starts with
/// `prefix` and ends with `suffix` (an extension, for programs that go by
/// it). The caller removes it.
pub fn create_temp(prefix: &str, suffix: &str) -> io::Result<(PathBuf, File)> {
    create_temp_in(&temp_dir()?, prefix, suffix)
}

/// [`create_temp`] in a folder of the caller's choice.
pub fn create_temp_in(dir: &Path, prefix: &str, suffix: &str) -> io::Result<(PathBuf, File)> {
    let mut last = None;
    for _ in 0..8 {
        let path = dir.join(unique(prefix, suffix));
        match create_private_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free temporary name")))
}

// -- the log -----------------------------------------------------------------------

/// A log file that does not grow without bound: once it holds `limit`
/// bytes it becomes `<name>.1` (replacing the one before) and a new file
/// is started, so at most twice the limit is kept.
pub struct RotatingLog {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    limit: u64,
}

impl RotatingLog {
    /// Open `path` for appending, private, rotating first when it is
    /// already over `limit`.
    pub fn open(path: &Path, limit: u64) -> io::Result<RotatingLog> {
        let mut log = RotatingLog { path: path.to_owned(), file: None, size: 0, limit: limit.max(1) };
        log.reopen()?;
        if log.size >= log.limit {
            log.rotate();
        }
        Ok(log)
    }

    /// Where the previous log is kept.
    pub fn old_path(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".1");
        path.with_file_name(name)
    }

    fn reopen(&mut self) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&self.path)?;
        self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        make_private(&self.path);
        Ok(())
    }

    /// Start a new file. When the old one cannot be moved (another handle
    /// holds it open on Windows) the log simply goes on in it, and the
    /// next attempt waits for another `limit` bytes.
    fn rotate(&mut self) {
        self.file = None;
        let moved = std::fs::rename(&self.path, Self::old_path(&self.path));
        if self.reopen().is_err() {
            return;
        }
        if moved.is_err() {
            self.size = 0;
        }
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size >= self.limit {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else { return Ok(buf.len()) };
        let n = file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glossa-fsio-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_nothing_behind() {
        let dir = scratch("atomic");
        let path = dir.join("data").join("config.toml");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["config.toml"], "no temporary file stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn files_and_folders_are_the_users_alone_whatever_they_were() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let data = dir.join("data");
        let path = data.join("config.toml");
        write_atomic(&path, b"key").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&data), 0o700, "a folder made for the file is private too");
        // A file from an earlier version, readable by everyone: replacing it
        // makes it private, and so does tightening it in place.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&path, b"key2").unwrap();
        assert_eq!(mode(&path), 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
        make_private(&path);
        make_private(&data);
        assert_eq!((mode(&path), mode(&data)), (0o600, 0o700));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_writers_never_leave_a_torn_file() {
        let dir = scratch("race");
        let path = dir.join("history.json");
        let threads: Vec<_> = (0..8u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        write_atomic_unsynced(&path, &vec![b'a' + i; 4096]).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 4096);
        assert!(bytes.iter().all(|b| *b == bytes[0]), "one writer's bytes, whole");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_temporary_file_is_new_private_and_never_follows_an_existing_name() {
        let dir = scratch("temp");
        let (path, mut file) = create_temp_in(&dir, "glossa-ocr-", ".png").unwrap();
        file.write_all(b"x").unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
        assert_eq!(create_private_new(&path).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_sweep_takes_stale_temporary_files_only() {
        let dir = scratch("sweep");
        for name in [".config.toml.1-0-0.tmp", "config.toml.tmp", "config.toml", "notes.tmp"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        // Nothing is old enough yet.
        sweep_stale(&dir, Duration::from_secs(3600), false);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);
        sweep_stale(&dir, Duration::ZERO, false);
        let mut left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["config.toml", "notes.tmp"]);
        sweep_stale(&dir, Duration::ZERO, true);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_lock_is_held_by_one_at_a_time() {
        let dir = scratch("lock");
        let path = dir.join("history.json.lock");
        let first = FileLock::acquire(&path).expect("locks work here");
        let (tx, rx) = std::sync::mpsc::channel();
        let p = path.clone();
        let waiter = std::thread::spawn(move || {
            let _second = FileLock::acquire(&p).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(150)).is_err(), "the second waits");
        drop(first);
        rx.recv_timeout(Duration::from_secs(5)).expect("and gets it once the first lets go");
        waiter.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_rotates_at_the_limit_and_keeps_one_old_file() {
        let dir = scratch("log");
        let path = dir.join("glossa.log");
        let old = RotatingLog::old_path(&path);
        let mut log = RotatingLog::open(&path, 100).unwrap();
        for _ in 0..25 {
            log.write_all(&[b'a'; 10]).unwrap();
        }
        // 250 bytes through a 100-byte limit: two rotations.
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 100);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 50);
        drop(log);
        // An oversized log from an earlier version is rotated on opening.
        std::fs::write(&path, vec![b'b'; 500]).unwrap();
        let mut log = RotatingLog::open(&path, 100).unwrap();
        log.write_all(b"new").unwrap();
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 500);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "never more than one old file");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }
}
