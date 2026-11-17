//! File operations that wait out a file held open by someone else.
//!
//! On Windows a file that is open cannot be deleted or renamed, and so cannot
//! the folder holding it: a module process stopped a moment ago (it is ended
//! in the background, within [`crate::proc_tree::EXIT_GRACE`]), or an
//! antivirus scanning a program that was just unpacked, keeps it busy for a
//! while. Such an error is retried for a few seconds there. Elsewhere an open
//! file does not get in the way, and the operation runs once.

use std::io;
use std::time::{Duration, Instant};

/// How long to keep trying on Windows.
const PATIENCE: Duration = Duration::from_secs(5);

/// Run `op`, retrying it while it fails because a file is in use (Windows).
pub fn retry<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let until = Instant::now() + PATIENCE;
    loop {
        match op() {
            Err(e) if is_busy(&e) && Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
            other => return other,
        }
    }
}

/// `std::fs::rename`, retried while a file is in use.
pub fn rename(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    retry(|| std::fs::rename(from, to))
}

/// Whether `e` means a file is open in another process (Windows only:
/// ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION,
/// ERROR_DIR_NOT_EMPTY, which `remove_dir_all` reports when a file inside
/// could not go yet).
fn is_busy(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33 | 145))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_and_other_errors_are_returned_at_once() {
        let mut calls = 0;
        assert_eq!(
            retry(|| {
                calls += 1;
                Ok(7)
            })
            .unwrap(),
            7
        );
        assert_eq!(calls, 1);
        let started = Instant::now();
        let mut calls = 0;
        let err = retry::<()>(|| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(windows)]
    #[test]
    fn a_sharing_violation_is_retried_until_it_clears() {
        let mut calls = 0;
        let v = retry(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(calls)
            }
        })
        .unwrap();
        assert_eq!(v, 3);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_held_open_is_deleted_once_it_is_closed() {
        let dir = std::env::temp_dir().join(format!("glossa-fs-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("held.bin");
        std::fs::write(&file, b"x").unwrap();
        // Open without FILE_SHARE_DELETE, as a running program's image is.
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().read(true).share_mode(1).open(&file).unwrap()
        };
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            drop(held);
        });
        retry(|| std::fs::remove_dir_all(&dir)).unwrap();
        closer.join().unwrap();
        assert!(!dir.exists());
    }
}
