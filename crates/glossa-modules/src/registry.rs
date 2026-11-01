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

use crate::host_api::ModuleCache;
use crate::manifest::{Manifest, Runtime};
use crate::provider::Provider;
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
}

impl Entry {
    pub fn info(&self) -> &ProviderInfo {
        self.provider.info()
    }
}

/// What the runtimes need from the program.
#[derive(Clone)]
pub struct RuntimeContext {
    /// The Python interpreter for Python modules; empty to search `PATH`.
    pub python: String,
    /// The `java` for Java modules (an executable or a Java home); empty to
    /// look in `JAVA_HOME`, `PATH` and the usual install folders.
    pub java: String,
    pub allow_native: bool,
    pub allow_legacy_scripts: bool,
    pub call_timeout: Duration,
    pub cache: Arc<ModuleCache>,
    /// Where `.potext` archives are unpacked.
    pub cache_dir: PathBuf,
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
            java: String::new(),
            allow_native: true,
            allow_legacy_scripts: true,
            call_timeout: Duration::from_secs(30),
            cache: ModuleCache::new(),
            cache_dir: std::env::temp_dir().join("glossa-modules"),
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
        self.insert(Entry { provider, origin: Origin::Builtin });
    }

    /// Add a module compiled into the program. Add these before loading the
    /// module directories, so a module there with the same id wins.
    pub fn add_bundled(&mut self, provider: Arc<dyn Provider>) {
        self.insert(Entry { provider, origin: Origin::Bundled });
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
            let manifest = Manifest::load(&manifest_path).map_err(|e| Skip::Failed(e.to_string()))?;
            let version = manifest.module.version.clone();
            let runtime = manifest.module.runtime;
            let provider = self.instantiate(path, manifest, ctx)?;
            return Ok(Some(Entry { provider, origin: Origin::Module { dir: path.to_owned(), runtime, version } }));
        }

        if path.join("Service.js").is_file() {
            if !ctx.allow_legacy_scripts {
                return Err(Skip::Reason("classic scripts are switched off".into()));
            }
            let provider = runtime_js_qtranslate(path, ctx)?;
            return Ok(Some(Entry {
                provider,
                origin: Origin::Module { dir: path.to_owned(), runtime: Runtime::Qtranslate, version: String::new() },
            }));
        }

        if path.join("info.json").is_file() && path.join("main.js").is_file() {
            let provider = runtime_js_pot(path, ctx)?;
            return Ok(Some(Entry {
                provider,
                origin: Origin::Module { dir: path.to_owned(), runtime: Runtime::Pot, version: String::new() },
            }));
        }

        Ok(None)
    }

    fn instantiate(&self, dir: &Path, manifest: Manifest, ctx: &RuntimeContext) -> Result<Arc<dyn Provider>, Skip> {
        match manifest.module.runtime {
            Runtime::Js => runtime_js_glossa(dir, manifest, ctx),
            Runtime::Qtranslate => runtime_js_qtranslate(dir, ctx),
            Runtime::Pot => runtime_js_pot(dir, ctx),
            Runtime::Python => {
                #[cfg(feature = "python")]
                {
                    runtime::python::load(dir, manifest, ctx).map_err(Skip::Failed)
                }
                #[cfg(not(feature = "python"))]
                {
                    let _ = (dir, manifest);
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
        let req = glossa_core::TranslateRequest::new("hi", glossa_core::LangPair::new(glossa_core::LangId::AUTO, glossa_core::LangId::ENGLISH));
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
}
