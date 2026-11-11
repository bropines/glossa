//! An image written to a temporary file for a program that wants a path.

use std::io::Write;
use std::path::{Path, PathBuf};

use glossa_core::{fsio, Error, Result};

/// Deleted when dropped, whatever happened in between.
pub struct TempImage {
    path: PathBuf,
}

impl TempImage {
    /// Write `bytes` to a new file in Glossa's own temporary folder
    /// ([`fsio::temp_dir`]: private, under the cache folder, swept at
    /// start), named after their format so tools that go by the extension
    /// are happy. A screenshot holds whatever was on the screen, so it is
    /// not put where every user of the machine can read it.
    pub fn write(bytes: &[u8]) -> Result<TempImage> {
        let dir = fsio::temp_dir().map_err(|e| Error::Platform(format!("no folder for temporary files: {e}")))?;
        Self::write_in(&dir, bytes)
    }

    /// The same in `dir`. The file is created new and for the user alone:
    /// a name that exists already (a link someone planted) is never
    /// written through.
    pub fn write_in(dir: &Path, bytes: &[u8]) -> Result<TempImage> {
        let suffix = format!(".{}", extension_for(bytes));
        let (path, mut file) = fsio::create_temp_in(dir, "glossa-ocr-", &suffix)
            .map_err(|e| Error::Platform(format!("cannot create a temporary file in {}: {e}", dir.display())))?;
        // From here on the file is removed whatever happens.
        let image = TempImage { path };
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|e| Error::Platform(format!("cannot write {}: {e}", image.path.display())))?;
        Ok(image)
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

    #[cfg(unix)]
    #[test]
    fn the_file_is_the_users_alone_and_outside_the_shared_temp_folder() {
        use std::os::unix::fs::PermissionsExt;
        let t = TempImage::write(b"BMxx").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(t.path()), 0o600);
        let dir = t.path().parent().unwrap();
        assert_eq!(mode(dir), 0o700);
        assert_ne!(dir, std::env::temp_dir(), "a folder of its own");
    }
}
