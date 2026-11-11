//! Windows (and anything else that is not Unix): nothing is changed. The
//! data folder is in the user's profile, whose access list already admits
//! only the user, the system and administrators, and what is created in it
//! inherits that. A folder is not flushed after a rename either: it cannot
//! be opened like a file there.

use std::fs::{DirBuilder, OpenOptions};
use std::path::Path;

pub(super) fn restrict(_path: &Path) {}

pub(super) fn private_file(_options: &mut OpenOptions) {}

pub(super) fn private_dir(_builder: &mut DirBuilder) {}

pub(super) fn sync_dir(_dir: &Path) {}
