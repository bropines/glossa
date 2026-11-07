//! Finding modules on disk and keeping the list of providers.
//!
//! [`Registry::load_dir`] scans one directory. Each entry may be:
//!
//! | what is there                         | loaded as               |
//! |---------------------------------------|-------------------------|
//! | `<dir>/<name>/module.toml`            | whatever it declares    |
//! | `<dir>/<name>/Service.js`             | a classic QTranslate script (its `Common.js` is looked for beside it, then one level up) |
//! | `<dir>/<name>/info.json` + `main.js`  | a pot-app plugin        |
//! | `<dir>/<name>.potext`                 | a pot-app plugin archive, unpacked into the cache |
//!
//! Directories are loaded in priority order and a later module with an id
//! already present replaces the earlier one, so a module in the user's own
//! directory overrides a bundled one of the same id.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use glossa_core::{Capabilities, ProviderInfo};

use crate::assets::{AssetGate, ModuleAssets};
use crate::host_api::ModuleCache;
use crate::manifest::{Manifest, Runtime};
use crate::provider::Provider;
use crate::python_env::{PythonDirs, PythonEnv};
use crate::runtime;

/// Where a provider came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Compiled into the program.
    Builtin,
    /// A JavaScript module whose sources are compiled into the program
    /// (`modules/bundled/` in the repository). A module with the same id in
    /// any module directory replaces it.
    Bundled,
    /// Loaded from a module directory.
    Module { dir: PathBuf, runtime: Runtime, version: String },
}

pub struct Entry {
    pub provider: Arc<dyn Provider>,
    pub origin: Origin,
    /// The files the module declares for this system, if any.
    pub assets: Option<Arc<ModuleAssets>>,
    /// A Python module's interpreter and environment.
    pub python: Option<Arc<PythonEnv>>,
}

impl Entry {
    pub fn info(&self) -> &ProviderInfo {
        self.provider.info()
    }
}

/// What the runtimes need from the program.
#[derive(Clone)]
pub struct RuntimeContext {
    /// `[modules] python`: `auto` (or empty), `system`, `managed`, or an
    /// interpreter; see [`crate::python_env::PythonChoice`].
    pub python: String,
    /// Where the managed Python, `uv` and the modules' environments go.
    pub python_dirs: PythonDirs,
    /// The `java` for Java modules (an executable or a Java home); empty to
    /// look in `JAVA_HOME`, `PATH` and the usual install folders.
    pub java: String,
    pub allow_native: bool,
    pub allow_legacy_scripts: bool,
    pub call_timeout: Duration,
    pub cache: Arc<ModuleCache>,
    /// Where `.potext` archives are unpacked.
    pub cache_dir: PathBuf,
    /// Where module assets are downloaded to, one folder per module.
    pub models_dir: PathBuf,
    /// Download a module's missing assets on its first call instead of
    /// failing with [`glossa_core::Error::AssetsMissing`].
    pub auto_download_assets: bool,
    /// Module ids the user switched off.
    pub disabled: Vec<String>,
    /// The Google domain and interface language some classic scripts read
    /// from their `Options` object.
    pub preferred_domain: String,
    pub ui_language_code: String,
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self {
            python: String::new(),
            python_dirs: PythonDirs::default(),
            java: String::new(),
            allow_native: true,
            allow_legacy_scripts: true,
            call_timeout: Duration::from_secs(30),
            cache: ModuleCache::new(),
            cache_dir: std::env::temp_dir().join("glossa-modules"),
            models_dir: std::env::temp_dir().join("glossa-models"),
            auto_download_assets: false,
            disabled: Vec::new(),
            preferred_domain: "com".into(),
            ui_language_code: "en".into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl LoadReport {
    pub fn merge(&mut self, other: LoadReport) {
        self.loaded.extend(other.loaded);
        self.failed.extend(other.failed);
        self.skipped.extend(other.skipped);
    }
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider compiled into the program.
    pub fn add_builtin(&mut self, provider: Arc<dyn Provider>) {
        self.insert(Entry { provider, origin: Origin::Builtin, assets: None, python: None });
    }

    /// Add a module compiled into the program. Add these before loading the
    /// module directories, so a module there with the same id wins.
    pub fn add_bundled(&mut self, provider: Arc<dyn Provider>) {
        self.insert(Entry { provider, origin: Origin::Bundled, assets: None, python: None });
    }

    /// Add or replace by id. The provider is put behind a panic barrier
    /// ([`Guarded`](crate::provider::Guarded)).
    pub fn insert(&mut self, mut entry: Entry) {
        entry.provider = crate::provider::Guarded::wrap(entry.provider);
        let id = entry.info().id.clone();
        if let Some(existing) = self.entries.iter_mut().find(|e| e.info().id == id) {
            tracing::debug!(%id, "module overrides an earlier provider with the same id");
            existing.provider.suspend();
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.entries.iter().find(|e| e.info().id == id).map(|e| e.provider.clone())
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.info().id == id)
    }

    /// Every provider that can do `cap`, in registration order.
    pub fn with_capability(&self, cap: Capabilities) -> Vec<Arc<dyn Provider>> {
        self.entries.iter().filter(|e| e.info().capabilities.contains(cap)).map(|e| e.provider.clone()).collect()
    }

    pub fn suspend_all(&self) {
        for e in &self.entries {
            e.provider.suspend();
        }
    }

    /// Load every module directory in priority order (lowest first).
    pub fn load_dirs(&mut self, dirs: &[PathBuf], ctx: &RuntimeContext) -> LoadReport {
        let mut report = LoadReport::default();
        for dir in dirs {
            report.merge(self.load_dir(dir, ctx));
        }
        report
    }

    pub fn load_dir(&mut self, dir: &Path, ctx: &RuntimeContext) -> LoadReport {
        let mut report = LoadReport::default();
        let Ok(read) = std::fs::read_dir(dir) else {
            return report;
        };
        let mut items: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
        items.sort();
        for path in items {
            match self.load_one(&path, ctx) {
                Ok(Some(entry)) => {
                    let id = entry.info().id.clone();
                    if ctx.disabled.contains(&id) {
                        report.skipped.push((path, format!("{id} is disabled")));
                        continue;
                    }
                    report.loaded.push(id);
                    self.insert(entry);
                }
                Ok(None) => {}
                Err(Skip::Reason(why)) => report.skipped.push((path, why)),
                Err(Skip::Failed(why)) => {
                    tracing::warn!(path = %path.display(), "module failed to load: {why}");
                    report.failed.push((path, why));
                }
            }
        }
        report
    }

    /// Load the one module at `path` (a folder with a `module.toml`, a
    /// classic script, a pot-app plugin, or a `.potext` archive) exactly as
    /// [`Self::load_dir`] would, without adding it anywhere. For tools that
    /// check a single module (`glossa-cli module test`), so they exercise the
    /// same loading code as the program. The provider comes back behind the
    /// usual panic barrier.
    pub fn load_module(path: &Path, ctx: &RuntimeContext) -> Result<Entry, String> {
        match Registry::new().load_one(path, ctx) {
            Ok(Some(mut entry)) => {
                entry.provider = crate::provider::Guarded::wrap(entry.provider);
                Ok(entry)
            }
            Ok(None) => {
                Err(format!("{} holds no module (no module.toml, Service.js or info.json + main.js)", path.display()))
            }
            Err(Skip::Reason(why) | Skip::Failed(why)) => Err(why),
        }
    }

    fn load_one(&self, path: &Path, ctx: &RuntimeContext) -> Result<Option<Entry>, Skip> {
        if path.is_file() {
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("potext")) {
                let unpacked = runtime::pot::unpack(path, &ctx.cache_dir).map_err(Skip::Failed)?;
                return self.load_one(&unpacked, ctx);
            }
            return Ok(None);
        }
        if !path.is_dir() {
            return Ok(None);
        }

        let manifest_path = path.join("module.toml");
        if manifest_path.is_file() {
            let mut manifest = Manifest::load(&manifest_path).map_err(|e| Skip::Failed(e.to_string()))?;
            let version = manifest.module.version.clone();
            let runtime = manifest.module.runtime;
            let origin = Origin::Module { dir: path.to_owned(), runtime, version };
            let assets = ModuleAssets::for_manifest(&manifest, &ctx.models_dir)
                .map_err(|e| Skip::Failed(format!("{}: {e}", manifest_path.display())))?
                .map(Arc::new);
            let python = python_env(path, &manifest, ctx)?;
            let Some(assets) = assets else {
                let provider = instantiate(path, manifest, ctx, python.as_ref())?;
                return Ok(Some(Entry { provider, origin, assets: None, python }));
            };
            assets.apply_to(&mut manifest);
            // Ready files: load as usual, so a broken module is reported
            // now. Missing ones: list the module, start nothing; the gate
            // loads it once the files are there.
            let provider: Arc<dyn Provider> = if assets.ready() {
                instantiate(path, manifest, ctx, python.as_ref())?
            } else {
                tracing::debug!(module = %manifest.module.id, missing = ?assets.missing(), "module waits for its files");
                let (dir, m, c, py) = (path.to_owned(), manifest.clone(), ctx.clone(), python.clone());
                let make: crate::assets::MakeProvider = Box::new(move || {
                    instantiate(&dir, m.clone(), &c, py.as_ref()).map_err(|s| match s {
                        Skip::Reason(r) | Skip::Failed(r) => r,
                    })
                });
                Arc::new(AssetGate::new(&manifest, assets.clone(), ctx.auto_download_assets, make))
            };
            return Ok(Some(Entry { provider, origin, assets: Some(assets), python }));
        }

        if path.join("Service.js").is_file() {
            if !ctx.allow_legacy_scripts {
                return Err(Skip::Reason("classic scripts are switched off".into()));
            }
            let provider = runtime_js_qtranslate(path, ctx)?;
            return Ok(Some(Entry {
                provider,
                origin: Origin::Module { dir: path.to_owned(), runtime: Runtime::Qtranslate, version: String::new() },
                assets: None,
                python: None,
            }));
        }

        if path.join("info.json").is_file() && path.join("main.js").is_file() {
            let provider = runtime_js_pot(path, ctx)?;
            return Ok(Some(Entry {
                provider,
                origin: Origin::Module { dir: path.to_owned(), runtime: Runtime::Pot, version: String::new() },
                assets: None,
                python: None,
            }));
        }

        Ok(None)
    }
}

/// A Python module's environment as the settings describe it; `None` for
/// other runtimes.
fn python_env(dir: &Path, manifest: &Manifest, ctx: &RuntimeContext) -> Result<Option<Arc<PythonEnv>>, Skip> {
    if manifest.module.runtime != Runtime::Python {
        return Ok(None);
    }
    #[cfg(feature = "python")]
    {
        runtime::python::env_for(dir, manifest, ctx).map(|e| Some(Arc::new(e))).map_err(Skip::Failed)
    }
    #[cfg(not(feature = "python"))]
    {
        let _ = (dir, ctx);
        Ok(None)
    }
}

/// Start the runtime for a module with a manifest.
fn instantiate(
    dir: &Path,
    manifest: Manifest,
    ctx: &RuntimeContext,
    python: Option<&Arc<PythonEnv>>,
) -> Result<Arc<dyn Provider>, Skip> {
    match manifest.module.runtime {
        Runtime::Js => runtime_js_glossa(dir, manifest, ctx),
        Runtime::Qtranslate => runtime_js_qtranslate(dir, ctx),
        Runtime::Pot => runtime_js_pot(dir, ctx),
        Runtime::Python => {
            #[cfg(feature = "python")]
            {
                let env = python.cloned().ok_or_else(|| Skip::Failed("no Python environment".into()))?;
                runtime::python::load(dir, manifest, ctx, env).map_err(Skip::Failed)
            }
            #[cfg(not(feature = "python"))]
            {
                let _ = (dir, manifest, python);
                Err(Skip::Reason("this build has no Python runtime".into()))
            }
        }
        Runtime::Process | Runtime::Java => {
            #[cfg(feature = "process")]
            {
                runtime::process::load(dir, manifest, ctx).map_err(Skip::Failed)
            }
            #[cfg(not(feature = "process"))]
            {
                let _ = (dir, manifest);
                Err(Skip::Reason("this build has no process runtime".into()))
            }
        }
        Runtime::Native => {
            if !ctx.allow_native {
                return Err(Skip::Reason("native modules are switched off".into()));
            }
            #[cfg(feature = "native")]
            {
                runtime::native::load(dir, manifest, ctx).map_err(Skip::Failed)
            }
            #[cfg(not(feature = "native"))]
            {
                let _ = (dir, manifest);
                Err(Skip::Reason("this build has no native runtime".into()))
            }
        }
    }
}

enum Skip {
    Reason(String),
    Failed(String),
}

#[cfg(feature = "js")]
fn runtime_js_glossa(dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    runtime::js::load_glossa(dir, manifest, ctx).map_err(Skip::Failed)
}

#[cfg(feature = "js")]
fn runtime_js_qtranslate(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    runtime::js::load_qtranslate(dir, ctx).map_err(Skip::Failed)
}

#[cfg(feature = "js")]
fn runtime_js_pot(dir: &Path, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    runtime::js::load_pot(dir, ctx).map_err(Skip::Failed)
}

#[cfg(not(feature = "js"))]
fn runtime_js_glossa(_: &Path, _: Manifest, _: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    Err(Skip::Reason("this build has no JavaScript runtime".into()))
}

#[cfg(not(feature = "js"))]
fn runtime_js_qtranslate(_: &Path, _: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    Err(Skip::Reason("this build has no JavaScript runtime".into()))
}

#[cfg(not(feature = "js"))]
fn runtime_js_pot(_: &Path, _: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
    Err(Skip::Reason("this build has no JavaScript runtime".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::FnProvider;
    use glossa_core::Translation;

    #[test]
    fn later_providers_replace_earlier_ones_with_the_same_id() {
        let mut r = Registry::new();
        r.add_builtin(Arc::new(FnProvider::translator("x", |_, _| Ok(Translation::plain("one")))));
        r.add_builtin(Arc::new(FnProvider::translator("y", |_, _| Ok(Translation::plain("y")))));
        r.add_builtin(Arc::new(FnProvider::translator("x", |_, _| Ok(Translation::plain("two")))));
        assert_eq!(r.entries().len(), 2);
        assert_eq!(r.entries()[0].info().id, "x", "keeps its position");
        assert_eq!(r.with_capability(Capabilities::TRANSLATE).len(), 2);
        assert!(r.with_capability(Capabilities::OCR).is_empty());
        assert!(r.get("x").is_some());
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn a_panicking_provider_fails_the_call_not_the_program() {
        let mut r = Registry::new();
        r.add_builtin(Arc::new(FnProvider::translator("boom", |_, req| {
            let v: Vec<u8> = Vec::new();
            Ok(Translation::plain(v[req.text.len()].to_string()))
        })));
        let p = r.get("boom").unwrap();
        let http = glossa_net::mock::MockHttp::new();
        let req = glossa_core::TranslateRequest::new(
            "hi",
            glossa_core::LangPair::new(glossa_core::LangId::AUTO, glossa_core::LangId::ENGLISH),
        );
        let err = crate::provider::with_test_host(&http, &Default::default(), |h| p.translate(h, &req)).unwrap_err();
        match err {
            glossa_core::Error::Module { module, detail } => {
                assert_eq!(module, "boom");
                assert!(detail.contains("translation panicked") && detail.contains("index out of bounds"), "{detail}");
            }
            other => panic!("expected a module error, got {other:?}"),
        }
        // Wrapping is not stacked when a provider is re-inserted.
        let again = r.get("boom").unwrap();
        r.add_builtin(again);
        assert!(r.get("boom").unwrap().is_guarded());
    }

    #[test]
    fn missing_directories_load_nothing_quietly() {
        let mut r = Registry::new();
        let report = r.load_dir(Path::new("/definitely/not/here"), &RuntimeContext::default());
        assert!(report.loaded.is_empty() && report.failed.is_empty());
    }

    #[test]
    fn broken_manifests_are_reported_not_fatal() {
        let dir = std::env::temp_dir().join(format!("glossa-reg-{}", std::process::id()));
        let bad = dir.join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("module.toml"), "[module]\nid = \"BAD ID\"\nname=\"x\"\nruntime=\"js\"").unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        let mut r = Registry::new();
        let report = r.load_dir(&dir, &RuntimeContext::default());
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report.loaded.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "js")]
    #[test]
    fn a_module_waiting_for_its_files_is_listed_but_not_loaded() {
        use glossa_net::Method;
        let root = std::env::temp_dir().join(format!("glossa-reg-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let module = root.join("modules").join("needs-model");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            module.join("module.toml"),
            r#"capabilities = ["translate"]
[module]
id = "needs-model"
name = "Needs a model"
runtime = "js"
env = { OWN = "${ASSETS_DIR}/x" }

[permissions]
network = ["dl.test"]

[[assets]]
name = "model.bin"
url = "https://dl.test/model.bin"
sha256 = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
size = 11
"#,
        )
        .unwrap();
        std::fs::write(module.join("main.js"), "function translate(req) { return host.assetPath('model.bin'); }")
            .unwrap();
        let ctx = RuntimeContext { models_dir: root.join("models"), ..Default::default() };
        let mut r = Registry::new();
        let report = r.load_dir(&root.join("modules"), &ctx);
        assert_eq!(report.loaded, ["needs-model"], "{report:?}");
        let entry = r.entry("needs-model").unwrap();
        let assets = entry.assets.clone().expect("assets are known");
        assert_eq!(assets.missing(), ["model.bin"]);

        let req = glossa_core::TranslateRequest::new(
            "hi",
            glossa_core::LangPair::new(glossa_core::LangId::AUTO, glossa_core::LangId::ENGLISH),
        );
        let http = glossa_net::mock::MockHttp::new().on(Method::Get, "https://dl.test/", 200, "hello world");
        let p = entry.provider.clone();
        let err = crate::provider::with_test_host(&http, &Default::default(), |h| p.translate(h, &req)).unwrap_err();
        assert!(matches!(err, glossa_core::Error::AssetsMissing { .. }), "{err:?}");

        assets.download(&http, &crate::provider::CancelToken::new(), &|_| {}).unwrap();
        let out = crate::provider::with_test_host(&http, &Default::default(), |h| p.translate(h, &req)).unwrap();
        assert_eq!(std::path::PathBuf::from(out.text), root.join("models").join("needs-model").join("model.bin"));

        // Loaded again with the files there, it starts as usual, and a
        // process runtime would see the paths in its environment.
        let mut manifest = Manifest::load(&module.join("module.toml")).unwrap();
        assets.apply_to(&mut manifest);
        let dir = root.join("models").join("needs-model");
        assert_eq!(manifest.module.env["GLOSSA_ASSETS_DIR"], dir.to_string_lossy());
        assert_eq!(manifest.module.env["GLOSSA_ASSET_MODEL_BIN"], dir.join("model.bin").to_string_lossy());
        assert_eq!(manifest.module.env["OWN"], format!("{}/x", dir.to_string_lossy()));
        let mut r = Registry::new();
        r.load_dir(&root.join("modules"), &ctx);
        assert!(r.entry("needs-model").unwrap().assets.as_ref().unwrap().ready());
        std::fs::remove_dir_all(&root).ok();
    }
}
