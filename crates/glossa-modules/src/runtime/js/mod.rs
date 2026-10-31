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
    let name = script.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    engine::check_syntax(&script.source, &name)?;
    Ok(Arc::new(ProtocolProvider::new(
        manifest,
        glossa::GlossaTransport { script: Arc::new(script) },
        ctx.cache.clone(),
    )))
}

/// A Glossa JavaScript module whose `module.toml` and script are compiled
/// into the program rather than read from a folder. `label` names the script
/// in error messages (`bundled/<id>/main.js`). The syntax is not checked here:
/// that costs a parse per module at every start, and the tests of the crate
/// that embeds the sources do it once instead.
pub fn load_embedded(
    manifest: &str,
    script: &str,
    label: &str,
    cache: Arc<crate::host_api::ModuleCache>,
) -> Result<Arc<dyn Provider>, String> {
    let manifest = Manifest::parse(manifest).map_err(|e| format!("{label}: module.toml: {e}"))?;
    if manifest.module.runtime != crate::manifest::Runtime::Js {
        return Err(format!("{label}: an embedded module must have runtime = \"js\""));
    }
    let script = glossa::GlossaScript { path: label.into(), source: script.trim_start_matches('\u{feff}').to_owned() };
    Ok(Arc::new(ProtocolProvider::new(manifest, glossa::GlossaTransport { script: Arc::new(script) }, cache)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_syntax_error_fails_the_load_with_the_file_name() {
        let dir = std::env::temp_dir().join(format!("glossa-js-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.js"), "function translate(req) { return req.text + ; }").unwrap();
        let manifest = Manifest::parse(
            "capabilities = [\"translate\"]
[module]
id = \"bad\"
name = \"Bad\"
runtime = \"js\"
",
        )
        .unwrap();
        let err = load_glossa(&dir, manifest.clone(), &RuntimeContext::default()).err().expect("must fail");
        assert!(err.starts_with("main.js:"), "{err}");
        std::fs::write(dir.join("main.js"), "function translate(req) { return req.text; }").unwrap();
        assert!(load_glossa(&dir, manifest, &RuntimeContext::default()).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
