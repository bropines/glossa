//! Service modules that ship inside the program.
//!
//! Each is an ordinary Glossa JavaScript module (`module.toml` + `main.js`)
//! kept in `modules/bundled/<id>/` at the top of the repository, readable as
//! documentation of the service's protocol, and compiled in with
//! `include_str!`, so a fresh install has them with no files on disk. They
//! run in the same JavaScript runtime as installed modules, under the same
//! network permissions.
//!
//! They are registered right after the Rust built-ins and before any module
//! directory is scanned, so a module with the same id in the user's modules
//! folder (or any other) replaces the bundled one: copy
//! `modules/bundled/<id>` there and edit it.

use std::sync::Arc;

use glossa_modules::host_api::ModuleCache;
use glossa_modules::Provider;

/// One bundled module's sources.
pub struct Bundled {
    pub id: &'static str,
    pub manifest: &'static str,
    pub script: &'static str,
}

macro_rules! bundled {
    ($($id:literal),* $(,)?) => {
        &[$(Bundled {
            id: $id,
            manifest: include_str!(concat!("../../../../modules/bundled/", $id, "/module.toml")),
            script: include_str!(concat!("../../../../modules/bundled/", $id, "/main.js")),
        }),*]
    };
}

/// Every bundled module, in the order the options page lists them:
/// translators, then dictionaries.
pub static BUNDLED: &[Bundled] = bundled![
    "yandex-web",
    "deepl-web",
    "lara",
    "modernmt",
    "multitran",
    "lingvo-live",
    "oxford-learners",
    "wiktionary",
    "youdao",
    "dic-academic",
    "translate-academic",
    "wikipedia",
    "urban-dictionary",
];

/// The bundled modules as providers. One that fails to load (which the tests
/// below rule out) is logged and left out rather than stopping the program.
pub fn providers(cache: &Arc<ModuleCache>) -> Vec<Arc<dyn Provider>> {
    BUNDLED
        .iter()
        .filter_map(|b| {
            let label = format!("bundled/{}/main.js", b.id);
            glossa_modules::runtime::js::load_embedded(b.manifest, b.script, &label, cache.clone())
                .map_err(|e| tracing::error!("bundled module {}: {e}", b.id))
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests;
