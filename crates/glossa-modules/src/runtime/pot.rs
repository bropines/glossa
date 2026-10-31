//! pot-app plugin archives.
//!
//! A `.potext` file is a zip of `info.json`, `main.js` and an icon. We unpack
//! it once into the cache directory (keyed by name and size, so a replaced
//! archive is unpacked again) and load the directory like any other.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Unpack `archive` and return the directory holding its files.
pub fn unpack(archive: &Path, cache_dir: &Path) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let stem = archive.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "plugin".into());
    let target = cache_dir.join("potext").join(format!("{stem}-{}", meta.len()));
    if target.join("info.json").is_file() {
        return Ok(target);
    }
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| format!("{}: not a pot plugin archive: {e}", archive.display()))?;
    std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // `enclosed_name` rejects absolute paths and `..` components, so a
        // crafted archive cannot write outside the target directory.
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if entry.size() > 16 * 1024 * 1024 {
            return Err(format!("{}: entry {} is implausibly large", archive.display(), out.display()));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&out, buf).map_err(|e| e.to_string())?;
    }
    // Some archives wrap everything in one folder.
    if !target.join("info.json").is_file() {
        if let Ok(rd) = std::fs::read_dir(&target) {
            for e in rd.flatten() {
                if e.path().join("info.json").is_file() {
                    return Ok(e.path());
                }
            }
        }
        return Err(format!("{}: archive has no info.json", archive.display()));
    }
    Ok(target)
}
