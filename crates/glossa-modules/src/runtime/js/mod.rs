//! JavaScript modules. (Being written.)

use std::path::Path;
use std::sync::Arc;

use crate::manifest::Manifest;
use crate::provider::Provider;
use crate::registry::RuntimeContext;

pub fn load_glossa(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let _ = (dir, manifest, ctx);
    Err("the JavaScript runtime is not implemented yet".into())
}

pub fn load_qtranslate(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let _ = (dir, ctx);
    Err("the JavaScript runtime is not implemented yet".into())
}

pub fn load_pot(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let _ = (dir, ctx);
    Err("the JavaScript runtime is not implemented yet".into())
}
