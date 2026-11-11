//! Putting a module into the user's modules folder: from a folder, from a
//! `.zip` of one, or a pot-app `.potext` file (which is loaded as it is).
//!
//! The copy is made beside the modules folder and moved into place when it
//! is whole, so a failed or interrupted install leaves the folder as it was
//! and a half-copied module is never loaded. Installing over a module of
//! the same name replaces it, files it no longer has included.
//!
//! Nothing here runs the module or approves it: it waits for the user's
//! approval like any module found in the folder.

use std::io::Read;
use std::path::{Path, PathBuf};

/// An archive may not unpack to more than this, or to more files than
/// [`MAX_ENTRIES`]: a module with models downloads them as assets.
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

/// Why a module was not installed.
#[derive(Debug)]
pub enum InstallError {
    /// The folder or archive holds no module (`module.toml`, a classic
    /// `Service.js`, a pot-app `info.json`), or the file is of no kind
    /// that can.
    NotAModule,
    Failed(glossa_core::Error),
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Failed(e.into())
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::NotAModule => f.write_str("no module.toml, Service.js or info.json found there"),
            InstallError::Failed(e) => e.fmt(f),
        }
    }
}

/// Whether `dir` is a module folder of a kind the registry loads.
pub fn is_module_dir(dir: &Path) -> bool {
    ["module.toml", "Service.js", "info.json"].iter().any(|f| dir.join(f).is_file())
}

/// Install the module at `from` into `user_dir` and return where it now is.
/// Blocks on the disk: call it on a worker thread.
pub fn install(from: &Path, user_dir: &Path) -> Result<PathBuf, InstallError> {
    let name = from.file_name().ok_or(InstallError::NotAModule)?.to_owned();
    std::fs::create_dir_all(user_dir)?;
    // Already there (the user picked a folder inside the modules folder):
    // nothing to copy, and copying it onto itself would destroy it.
    if let (Ok(a), Ok(b)) = (from.canonicalize(), user_dir.canonicalize()) {
        if a.starts_with(&b) {
            return if from.is_file() || is_module_dir(from) {
                Ok(from.to_owned())
            } else {
                Err(InstallError::NotAModule)
            };
        }
    }
    let staging = user_dir.parent().unwrap_or(user_dir).join(format!(".module-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = stage(from, &staging, &name).and_then(|(staged, name)| {
        let target = user_dir.join(name);
        put_in_place(&staged, &target)?;
        Ok(target)
    });
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Make the module ready inside `staging`; returns it and the name it gets
/// in the modules folder.
fn stage(from: &Path, staging: &Path, name: &std::ffi::OsStr) -> Result<(PathBuf, std::ffi::OsString), InstallError> {
    if from.is_dir() {
        if !is_module_dir(from) {
            return Err(InstallError::NotAModule);
        }
        let staged = staging.join(name);
        copy_dir(from, &staged)?;
        return Ok((staged, name.to_owned()));
    }
    let ext = from.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        // Loaded from the archive itself.
        "potext" => {
            let staged = staging.join(name);
            std::fs::copy(from, &staged)?;
            Ok((staged, name.to_owned()))
        }
        "zip" => {
            let root = staging.join("unpacked");
            unpack(from, &root)?;
            // The module itself, or the one folder holding it (how a
            // repository's download and most hand-made archives look).
            if is_module_dir(&root) {
                let stem = from.file_stem().ok_or(InstallError::NotAModule)?;
                return Ok((root, stem.to_owned()));
            }
            let mut inside = std::fs::read_dir(&root)?.flatten().map(|e| e.path()).filter(|p| p.is_dir());
            match (inside.next(), inside.next()) {
                (Some(only), None) if is_module_dir(&only) => {
                    let name = only.file_name().ok_or(InstallError::NotAModule)?.to_owned();
                    Ok((only, name))
                }
                _ => Err(InstallError::NotAModule),
            }
        }
        _ => Err(InstallError::NotAModule),
    }
}

/// Move `staged` to `target`, replacing what is there. The old copy is
/// moved aside first and put back if the new one cannot take its place.
fn put_in_place(staged: &Path, target: &Path) -> std::io::Result<()> {
    let old = staged.with_file_name(".replaced");
    let had_old = target.symlink_metadata().is_ok();
    if had_old {
        std::fs::rename(target, &old)?;
    }
    if let Err(e) = std::fs::rename(staged, target) {
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        return Err(e);
    }
    Ok(())
}

/// Copy a folder. Symbolic links are left out: one could point outside the
/// module, or back at a folder above it.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Unpack a zip into `target`. Entries that would land outside it, and
/// symbolic links, are left out.
fn unpack(archive: &Path, target: &Path) -> Result<(), InstallError> {
    let bad = |what: String| InstallError::Failed(glossa_core::Error::Config(format!("{}: {what}", archive.display())));
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(format!("not a zip archive: {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(bad(format!("{} files is more than a module has", zip.len())));
    }
    std::fs::create_dir_all(target)?;
    let mut left = MAX_UNPACKED;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
        let Some(rel) = entry.enclosed_name() else { continue };
        if entry.is_symlink() {
            continue;
        }
        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The size in the header is the archive's word; count what is
        // actually written.
        let mut limited = entry.take(left + 1);
        let written = std::io::copy(&mut limited, &mut std::fs::File::create(&out)?)?;
        if written > left {
            return Err(bad("it unpacks to more than 1 GiB".into()));
        }
        left -= written;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glossa-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, text) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn a_folder_is_copied_whole_and_replaces_an_older_copy() {
        let dir = scratch("folder");
        let (src, user) = (dir.join("src/my-mod"), dir.join("data/modules"));
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join("module.toml"), "one").unwrap();
        std::fs::write(src.join("lib/a.js"), "a").unwrap();
        // An older copy with a file the new one no longer has.
        std::fs::create_dir_all(user.join("my-mod")).unwrap();
        std::fs::write(user.join("my-mod/stale.js"), "old").unwrap();

        let at = install(&src, &user).unwrap();
        assert_eq!(at, user.join("my-mod"));
        assert_eq!(std::fs::read_to_string(at.join("lib/a.js")).unwrap(), "a");
        assert!(!at.join("stale.js").exists(), "the old copy is replaced, not merged into");
        // Nothing is left beside the modules folder, and nothing extra in it.
        assert_eq!(std::fs::read_dir(dir.join("data")).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&user).unwrap().count(), 1);

        // Picking the installed copy itself changes nothing.
        assert_eq!(install(&at, &user).unwrap(), at);
        assert!(at.join("module.toml").is_file());

        // Not a module: refused, and the installed one is untouched.
        let empty = dir.join("src/empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(matches!(install(&empty, &user), Err(InstallError::NotAModule)));
        assert!(matches!(install(&src.join("module.toml"), &user), Err(InstallError::NotAModule)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_zip_installs_with_or_without_a_folder_inside() {
        let dir = scratch("zip");
        let user = dir.join("data/modules");
        let flat = dir.join("flat-mod.zip");
        zip_of(&flat, &[("module.toml", "m"), ("main.js", "x")]);
        let at = install(&flat, &user).unwrap();
        assert_eq!(at, user.join("flat-mod"));
        assert!(at.join("main.js").is_file());

        let nested = dir.join("download-main.zip");
        zip_of(&nested, &[("nested-mod/module.toml", "m"), ("nested-mod/lib/a.js", "a")]);
        let at = install(&nested, &user).unwrap();
        assert_eq!(at, user.join("nested-mod"));
        assert!(at.join("lib/a.js").is_file());

        // An archive that climbs out of its folder installs nothing there.
        let sly = dir.join("sly.zip");
        zip_of(&sly, &[("../../escaped.txt", "x"), ("readme.txt", "no module here")]);
        assert!(matches!(install(&sly, &user), Err(InstallError::NotAModule)));
        assert!(!dir.join("escaped.txt").exists() && !dir.join("data/escaped.txt").exists());
        assert!(!user.join("sly").exists());

        let junk = dir.join("junk.zip");
        std::fs::write(&junk, "not a zip").unwrap();
        assert!(matches!(install(&junk, &user), Err(InstallError::Failed(_))));
        assert_eq!(std::fs::read_dir(dir.join("data")).unwrap().count(), 1, "no staging folder is left");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_potext_file_is_copied_as_it_is() {
        let dir = scratch("potext");
        let user = dir.join("modules");
        let plugin = dir.join("plugin.potext");
        std::fs::write(&plugin, "zip bytes").unwrap();
        let at = install(&plugin, &user).unwrap();
        assert_eq!(std::fs::read_to_string(at).unwrap(), "zip bytes");
        std::fs::remove_dir_all(&dir).ok();
    }
}
