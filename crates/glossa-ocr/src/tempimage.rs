//! An image written to a temporary file for a program that wants a path.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use glossa_core::{Error, Result};

/// Deleted when dropped, whatever happened in between.
pub struct TempImage {
    path: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempImage {
    /// Write `bytes` under the system temp directory, named after their
    /// format so tools that go by the extension are happy.
    pub fn write(bytes: &[u8]) -> Result<TempImage> {
        Self::write_in(&std::env::temp_dir(), bytes)
    }

    pub fn write_in(dir: &Path, bytes: &[u8]) -> Result<TempImage> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let name = format!("glossa-ocr-{}-{n}-{nanos:x}.{}", std::process::id(), extension_for(bytes));
        let path = dir.join(name);
        std::fs::write(&path, bytes).map_err(|e| Error::Platform(format!("cannot write {}: {e}", path.display())))?;
        Ok(TempImage { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The file extension for encoded image bytes, by their magic number.
pub fn extension_for(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"BM") {
        "bmp"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else {
        // Captured regions are PNG; an unknown blob gets the likeliest name.
        "png"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_follow_the_content() {
        assert_eq!(extension_for(&[0x89, b'P', b'N', b'G', 1]), "png");
        assert_eq!(extension_for(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(extension_for(b"RIFF\0\0\0\0WEBPVP8 "), "webp");
        assert_eq!(extension_for(b"BM.."), "bmp");
        assert_eq!(extension_for(b"??"), "png");
    }

    #[test]
    fn the_file_goes_away_on_drop() {
        let t = TempImage::write(b"BMxx").unwrap();
        let p = t.path().to_owned();
        assert!(p.is_file());
        assert_eq!(p.extension().unwrap(), "bmp");
        drop(t);
        assert!(!p.exists());
    }
}
