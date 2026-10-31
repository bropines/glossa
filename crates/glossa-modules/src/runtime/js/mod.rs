//! JavaScript modules, in three dialects:
//!
//! * **Glossa** ([`glossa`]) — `module.toml` + a script speaking the module
//!   protocol, with a synchronous `host` object.
//! * **Classic QTranslate** ([`legacy`]) — an unmodified `Service.js` from a
//!   classic installation, driven through its request/response functions.
//! * **pot-app** ([`pot`]) — an unmodified `info.json` + `main.js` plugin.
//!
//! All three are [`Transport`](crate::protocol::Transport)s wrapped in a
//! [`ProtocolProvider`], so language mapping, network permissions and error
//! handling are shared with the Python and native runtimes. The engine is
//! Boa, a JavaScript engine written in Rust — no C toolchain, no external
//! runtime, and nothing resident between calls (see [`engine`]).

pub mod engine;
pub mod glossa;
pub mod legacy;
pub mod pot;

use std::path::Path;
use std::sync::Arc;

use crate::manifest::Manifest;
use crate::protocol::ProtocolProvider;
use crate::provider::Provider;
use crate::registry::RuntimeContext;

pub fn load_glossa(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let script = glossa::GlossaScript::from_dir(dir, &manifest.module.entry)?;
    Ok(Arc::new(ProtocolProvider::new(
        manifest,
        glossa::GlossaTransport { script: Arc::new(script) },
        ctx.cache.clone(),
    )))
}

pub fn load_qtranslate(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let script = legacy::LegacyScript::from_dir(dir)?;
    let meta = script.probe()?;
    let manifest = meta.manifest(&script.folder);
    Ok(Arc::new(ProtocolProvider::new(
        manifest,
        legacy::LegacyTransport {
            script: Arc::new(script),
            ui_language_code: ctx.ui_language_code.clone(),
            preferred_domain: ctx.preferred_domain.clone(),
        },
        ctx.cache.clone(),
    )))
}

pub fn load_pot(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, String> {
    let plugin = pot::PotPlugin::from_dir(dir)?;
    let manifest = plugin.manifest.clone();
    Ok(Arc::new(ProtocolProvider::new(manifest, pot::PotTransport { plugin: Arc::new(plugin) }, ctx.cache.clone())))
}
