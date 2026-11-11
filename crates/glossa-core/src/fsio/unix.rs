//! Unix: everything is made for the user alone (folders `0700`, files
//! `0600`), whatever the umask says, and a rename is made durable by
//! flushing its folder.

use std::fs::{DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub(super) fn restrict(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    // A link is left alone: its target may be a folder the user shares on
    // purpose.
    if meta.file_type().is_symlink() {
        return;
    }
    let want = if meta.is_dir() { 0o700 } else { 0o600 };
    if meta.permissions().mode() & 0o777 != want {
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(want)) {
            tracing::warn!("could not make {} private: {e}", path.display());
        }
    }
}

pub(super) fn private_file(options: &mut OpenOptions) {
    options.mode(0o600);
}

pub(super) fn private_dir(builder: &mut DirBuilder) {
    builder.mode(0o700);
}

pub(super) fn sync_dir(dir: &Path) {
    // The rename itself is in the folder; without this a power cut can
    // bring the old name back. Not every file system can do it.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}
