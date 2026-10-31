//! Native modules: shared libraries exporting the C ABI.
//!
//! (Implementation pending — see the stub below.)

use std::path::Path;
use std::sync::Arc;

use crate::manifest::Manifest;
use crate::provider::Provider;
use crate::registry::RuntimeContext;

pub fn load(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let _ = (dir, manifest, ctx);
    Err("the native runtime is not implemented yet".into())
}
