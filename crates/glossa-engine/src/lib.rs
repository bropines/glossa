//! The core of the program, independent of any user interface.
//!
//! [`Engine`] owns the settings, the provider registry, the history and the
//! theme and language catalogues, and runs every job — translate, detect,
//! look up, speak, recognise — on a worker thread. A front end (the desktop
//! GUI, the command line, the control API) asks for work and listens for
//! [`Event`]s; it never blocks on the network.
//!
//! Jobs belong to a [`Slot`]: the main window, the popup, and so on. Starting
//! a job in a slot cancels the one already running there, so typing in the
//! main window with instant translation on never lets an older, slower answer
//! overwrite a newer one.

pub mod catalog;
pub mod export;
pub mod pipeline;
pub mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::history::{HistoryEntry, HistoryResult};
use glossa_core::ocr::{ImageSource, OcrRequest};
use glossa_core::{
    Action, Article, Audio, Capabilities, Config, Detection, DictRequest, Error, History, LangId, LangPair, Locale,
    OcrResult, Paths, ProviderInfo, Result, SpeakRequest, Theme,
};
use glossa_modules::host_api::ModuleCache;
use glossa_modules::{CancelToken, Host, OptionSpec, OptionValues, Origin, Provider, Registry, RuntimeContext};
use glossa_net::HttpClient;
use parking_lot::{Mutex, RwLock};

pub use catalog::{LocaleCatalog, LocaleEntry, ThemeCatalog, ThemeEntry};
pub use export::ExportFormat;
pub use pipeline::{Pipeline, Progress, TranslateOptions, TranslationOutcome};

/// The presets an AI endpoint can start from, for front ends.
pub mod ai {
    pub use glossa_services::openai::{preset, AiPreset, PRESETS};
    /// The id of the built-in OpenAI-compatible translator.
    pub const BUILTIN_ID: &str = glossa_services::OpenAi::ID;
}

/// Where a job's result goes. One job per slot at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Main,
    Popup,
    Dictionary,
    Ocr,
    Speech,
    /// A one-off request (the control API, the command line).
    Detached,
}

/// What the engine tells a front end.
#[derive(Debug, Clone)]
pub enum Event {
    Translated {
        slot: Slot,
        job: u64,
        result: std::result::Result<Arc<TranslationOutcome>, Arc<Error>>,
    },
    Dictionary {
        slot: Slot,
        job: u64,
        query: String,
        result: std::result::Result<Arc<Article>, Arc<Error>>,
    },
    Recognized {
        slot: Slot,
        job: u64,
        result: std::result::Result<Arc<OcrOutcome>, Arc<Error>>,
    },
    Spoken {
        slot: Slot,
        job: u64,
        result: std::result::Result<Arc<Audio>, Arc<Error>>,
    },
    /// A streaming translation so far: the whole text written up to now.
    /// [`Event::Translated`] still follows with the final result.
    Progress {
        slot: Slot,
        job: u64,
        text: String,
    },
    /// A job started (for a busy indicator).
    Started {
        slot: Slot,
        job: u64,
    },
    HistoryChanged,
    ConfigChanged,
    ModulesReloaded,
    /// Someone outside the UI (the control API) asked for an action, with
    /// optional text to act on instead of the selection.
    Action {
        action: Action,
        text: Option<String>,
    },
}

/// OCR, plus a translation when one was asked for.
#[derive(Debug, Clone)]
pub struct OcrOutcome {
    pub ocr: OcrResult,
    /// The translation: the OCR provider's own, or one made afterwards.
    pub translation: Option<TranslationOutcome>,
}

impl OcrOutcome {
    pub fn translated_text(&self) -> Option<&str> {
        self.translation.as_ref().map(|t| t.translation.text.as_str()).or(self.ocr.translation.as_deref())
    }
}

/// A provider, as the settings page lists it.
#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub info: ProviderInfo,
    pub options: Vec<OptionSpec>,
    /// "Built-in", "JavaScript", "Python", "QTranslate script"…
    pub origin: String,
    pub dir: Option<PathBuf>,
    pub enabled: bool,
}

/// Modules that loaded, failed or were skipped, for the settings page.
#[derive(Debug, Clone, Default)]
pub struct ModuleStatus {
    pub loaded: Vec<String>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// What importing a classic installation did.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub options_imported: bool,
    pub history_entries: usize,
    pub notes: Vec<String>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    paths: Paths,
    config: RwLock<Config>,
    registry: RwLock<Arc<Registry>>,
    http: RwLock<Arc<HttpClient>>,
    history: Mutex<History>,
    dictionary_history: Mutex<Vec<String>>,
    themes: RwLock<ThemeCatalog>,
    locales: RwLock<LocaleCatalog>,
    locale: RwLock<Arc<Locale>>,
    modules: RwLock<ModuleStatus>,
    module_cache: Arc<ModuleCache>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    waker: RwLock<Option<Waker>>,
    slots: Mutex<HashMap<Slot, (u64, CancelToken)>>,
    next_job: AtomicU64,
    last_spoken: Mutex<Option<(String, Instant)>>,
    /// Built-in providers are created once; modules are reloaded around them.
    builtins: Vec<Arc<dyn Provider>>,
}

/// The engine. Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Start with the settings found under `paths`.
    pub fn start(paths: Paths) -> Result<Engine> {
        let config = Config::load(&paths.config_file())?;
        Engine::with_config(paths, config)
    }

    /// Start with explicit settings (tests, `--config`).
    pub fn with_config(paths: Paths, mut config: Config) -> Result<Engine> {
        config.sanitise();
        if config.server.token.is_empty() {
            config.server.token = random_token();
            // Other programs (glossa-cli among them) read the token from the
            // file, so it has to be there from the first run.
            if let Err(e) = config.save(&paths.config_file()) {
                tracing::warn!("could not save the new control API token: {e}");
            }
        }
        let http = Arc::new(HttpClient::new(&config.network)?);
        let history = if config.history.enabled {
            History::load(&paths.history_file(), config.history.max_entries).unwrap_or_else(|e| {
                tracing::warn!("history could not be read ({e}); starting a new one");
                History::new(config.history.max_entries)
            })
        } else {
            History::new(config.history.max_entries)
        };
        let dictionary_history = std::fs::read_to_string(paths.dictionary_history_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let mut builtins = glossa_services::builtin_providers();
        builtins.extend(glossa_ocr::builtin_providers(&config.ocr));

        let inner = Inner {
            paths,
            config: RwLock::new(config),
            registry: RwLock::new(Arc::new(Registry::new())),
            http: RwLock::new(http),
            history: Mutex::new(history),
            dictionary_history: Mutex::new(dictionary_history),
            themes: RwLock::new(ThemeCatalog::default()),
            locales: RwLock::new(LocaleCatalog::default()),
            locale: RwLock::new(Arc::new(Locale::english())),
            modules: RwLock::new(ModuleStatus::default()),
            module_cache: ModuleCache::new(),
            subscribers: Mutex::new(Vec::new()),
            waker: RwLock::new(None),
            slots: Mutex::new(HashMap::new()),
            next_job: AtomicU64::new(1),
            last_spoken: Mutex::new(None),
            builtins,
        };
        let engine = Engine { inner: Arc::new(inner) };
        engine.reload_modules();
        engine.reload_catalogs();
        Ok(engine)
    }

    // -- state -------------------------------------------------------------

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    /// Change settings, save them, and apply whatever the change affects.
    pub fn update_config(&self, change: impl FnOnce(&mut Config)) -> Result<()> {
        let (before, after) = {
            let mut cfg = self.inner.config.write();
            let before = cfg.clone();
            change(&mut cfg);
            cfg.sanitise();
            (before, cfg.clone())
        };
        after.save(&self.inner.paths.config_file())?;
        if before.network != after.network {
            *self.inner.http.write() = Arc::new(HttpClient::new(&after.network)?);
        }
        if before.modules != after.modules
            || before.services.ai_endpoints != after.services.ai_endpoints
            || before.classic != after.classic
            || before.ocr != after.ocr
            || before.dictionary.offline != after.dictionary.offline
        {
            self.reload_modules();
        }
        if before.classic != after.classic
            || before.appearance.theme != after.appearance.theme
            || before.general.ui_language != after.general.ui_language
        {
            self.reload_catalogs();
        }
        if before.history.max_entries != after.history.max_entries {
            self.inner.history.lock().set_max_entries(after.history.max_entries);
        }
        self.emit(Event::ConfigChanged);
        Ok(())
    }

    /// Save the settings as they are (window positions, pane contents).
    pub fn save_config(&self) -> Result<()> {
        self.inner.config.read().save(&self.inner.paths.config_file())
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.inner.registry.read().clone()
    }

    pub fn module_status(&self) -> ModuleStatus {
        self.inner.modules.read().clone()
    }

    /// Rebuild the provider list: built-ins, then module directories in
    /// priority order (the classic installation's services, bundled modules,
    /// the user's modules, extra directories from the settings).
    pub fn reload_modules(&self) {
        let cfg = self.config();
        let paths = &self.inner.paths;
        let ctx = RuntimeContext {
            python: cfg.modules.python.clone(),
            allow_native: cfg.modules.allow_native,
            allow_legacy_scripts: cfg.modules.allow_legacy_scripts,
            call_timeout: Duration::from_millis(cfg.modules.call_timeout_ms),
            cache: self.inner.module_cache.clone(),
            cache_dir: paths.cache_dir.join("modules"),
            disabled: cfg.modules.disabled.clone(),
            preferred_domain: cfg.advanced.preferred_domain.clone(),
            ui_language_code: self.locale().code.clone(),
        };
        let mut reg = Registry::new();
        for p in &self.inner.builtins {
            if !cfg.modules.disabled.contains(&p.info().id) {
                reg.add_builtin(p.clone());
            }
        }
        for e in &cfg.services.ai_endpoints {
            if !cfg.modules.disabled.contains(&e.id) {
                reg.add_builtin(Arc::new(glossa_services::OpenAi::endpoint(&e.id, &e.name, &e.preset)));
            }
        }
        if !cfg.dictionary.offline.is_empty() {
            let offline = glossa_dict::OfflineDictionaries::new();
            for path in &cfg.dictionary.offline {
                if let Err(e) = offline.add_path(path) {
                    tracing::warn!("offline dictionary {}: {e}", path.display());
                }
            }
            reg.add_builtin(Arc::new(offline));
        }
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(classic) = cfg.classic.services_dir() {
            dirs.push(classic);
        }
        dirs.extend(paths.module_dirs());
        dirs.push(paths.user_module_dir());
        dirs.extend(cfg.modules.dirs.iter().cloned());
        dirs.dedup();
        let report = reg.load_dirs(&dirs, &ctx);
        tracing::info!(
            providers = reg.entries().len(),
            modules = report.loaded.len(),
            failed = report.failed.len(),
            "providers ready"
        );
        let old = std::mem::replace(&mut *self.inner.registry.write(), Arc::new(reg));
        old.suspend_all();
        *self.inner.modules.write() =
            ModuleStatus { loaded: report.loaded, failed: report.failed, skipped: report.skipped };
        self.emit(Event::ModulesReloaded);
    }

    fn reload_catalogs(&self) {
        let cfg = self.config();
        let paths = &self.inner.paths;
        let mut theme_dirs = paths.theme_dirs();
        theme_dirs.push(paths.user_theme_dir());
        let classic_themes: Vec<PathBuf> = cfg.classic.themes_dir().into_iter().collect();
        *self.inner.themes.write() = ThemeCatalog::discover(&theme_dirs, &classic_themes);

        let mut locale_dirs: Vec<PathBuf> = cfg.classic.locales_dir().into_iter().collect();
        locale_dirs.extend(paths.locale_dirs());
        let locales = LocaleCatalog::discover(&locale_dirs);
        *self.inner.locale.write() = Arc::new(locales.load(&cfg.general.ui_language));
        *self.inner.locales.write() = locales;
    }

    /// Every provider with `cap`, in the user's order, with what the settings
    /// page needs to show about each.
    pub fn providers(&self, cap: Capabilities) -> Vec<ProviderSummary> {
        let cfg = self.config();
        let reg = self.registry();
        let (order, enabled) = if cap == Capabilities::DICTIONARY {
            (&cfg.services.dictionary_order, &cfg.services.dictionary_enabled)
        } else {
            (&cfg.services.order, &cfg.services.enabled)
        };
        let mut out: Vec<ProviderSummary> = reg
            .entries()
            .iter()
            .filter(|e| e.info().capabilities.contains(cap))
            .map(|e| {
                let (origin, dir) = match &e.origin {
                    Origin::Builtin => ("Built-in".to_owned(), None),
                    Origin::Module { dir, runtime, .. } => (runtime.label().to_owned(), Some(dir.clone())),
                };
                ProviderSummary {
                    info: e.info().clone(),
                    options: e.provider.options().to_vec(),
                    origin,
                    dir,
                    enabled: enabled.contains(&e.info().id),
                }
            })
            .collect();
        let rank = |id: &str| order.iter().position(|o| o == id).unwrap_or(usize::MAX);
        out.sort_by_key(|p| (rank(&p.info.id), !p.enabled));
        out
    }

    /// The service's own page for this query, for "open in browser".
    pub fn web_link(&self, provider: &str, text: &str, pair: LangPair) -> Option<String> {
        self.registry().get(provider)?.web_link(text, pair)
    }

    // -- themes and languages -------------------------------------------------

    pub fn theme_names(&self) -> Vec<String> {
        self.inner.themes.read().names()
    }

    pub fn theme_entries(&self) -> Vec<ThemeEntry> {
        self.inner.themes.read().entries().to_vec()
    }

    pub fn theme(&self, name: &str) -> Theme {
        self.inner.themes.read().load(name)
    }

    pub fn current_theme(&self) -> Theme {
        let name = self.inner.config.read().appearance.theme.clone();
        self.theme(&name)
    }

    /// Re-scan theme and locale folders (after the user edits a theme file).
    pub fn refresh_catalogs(&self) {
        self.reload_catalogs();
        self.emit(Event::ConfigChanged);
    }

    pub fn locales(&self) -> Vec<LocaleEntry> {
        self.inner.locales.read().entries().to_vec()
    }

    pub fn locale(&self) -> Arc<Locale> {
        self.inner.locale.read().clone()
    }

    /// Translate an interface string.
    pub fn tr(&self, key: &str) -> String {
        self.inner.locale.read().get(key).to_owned()
    }

    // -- events -----------------------------------------------------------------

    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.inner.subscribers.lock().push(tx);
        rx
    }

    /// Called after every event, so a GUI can wake its loop.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *self.inner.waker.write() = Some(Arc::new(waker));
    }

    pub fn emit(&self, event: Event) {
        self.inner.subscribers.lock().retain(|tx| tx.send(event.clone()).is_ok());
        if let Some(w) = self.inner.waker.read().clone() {
            w();
        }
    }

    /// Ask the front end to perform an action (used by the control API).
    pub fn request_action(&self, action: Action, text: Option<String>) {
        self.emit(Event::Action { action, text });
    }

    // -- jobs --------------------------------------------------------------------

    fn begin(&self, slot: Slot) -> (u64, CancelToken) {
        let job = self.inner.next_job.fetch_add(1, Ordering::SeqCst);
        let cancel = CancelToken::new();
        if slot != Slot::Detached {
            if let Some((_, old)) = self.inner.slots.lock().insert(slot, (job, cancel.clone())) {
                old.cancel();
            }
        }
        self.emit(Event::Started { slot, job });
        (job, cancel)
    }

    /// Whether `job` is still the newest in its slot.
    fn current(&self, slot: Slot, job: u64) -> bool {
        slot == Slot::Detached || self.inner.slots.lock().get(&slot).is_some_and(|(j, _)| *j == job)
    }

    /// Cancel whatever runs in `slot`.
    pub fn cancel(&self, slot: Slot) {
        if let Some((_, c)) = self.inner.slots.lock().remove(&slot) {
            c.cancel();
        }
    }

    fn spawn(&self, name: &str, f: impl FnOnce() + Send + 'static) {
        if let Err(e) = std::thread::Builder::new().name(format!("glossa-{name}")).spawn(f) {
            tracing::error!("could not start a worker thread: {e}");
        }
    }

    fn with_pipeline<T>(&self, cancel: &CancelToken, f: impl FnOnce(&Pipeline) -> T) -> T {
        let registry = self.registry();
        let config = self.config();
        let http = self.inner.http.read().clone();
        let p = Pipeline { registry: &registry, config: &config, http: http.as_ref(), cancel };
        f(&p)
    }

    // -- AI endpoints ----------------------------------------------------------------

    /// Add an OpenAI-compatible endpoint made from a preset (see
    /// [`glossa_services::openai::PRESETS`]). It becomes a translator of its
    /// own, enabled in the service strip; returns its id.
    pub fn add_ai_endpoint(&self, name: &str, preset: &str) -> Result<String> {
        let name = name.trim();
        let name = if name.is_empty() { glossa_services::openai::preset(preset).name } else { name };
        let mut id = String::new();
        self.update_config(|c| {
            id = c.services.new_ai_endpoint_id(name);
            c.services.ai_endpoints.push(glossa_core::config::AiEndpoint {
                id: id.clone(),
                name: name.to_owned(),
                preset: preset.to_owned(),
            });
            c.services.enabled.push(id.clone());
            c.services.order.push(id.clone());
        })?;
        Ok(id)
    }

    /// Rename an endpoint the user added.
    pub fn rename_ai_endpoint(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Ok(());
        }
        self.update_config(|c| {
            if let Some(e) = c.services.ai_endpoints.iter_mut().find(|e| e.id == id) {
                e.name = name;
            }
        })
    }

    /// Remove an endpoint and everything that refers to it.
    pub fn remove_ai_endpoint(&self, id: &str) -> Result<()> {
        self.update_config(|c| {
            let s = &mut c.services;
            s.ai_endpoints.retain(|e| e.id != id);
            s.enabled.retain(|x| x != id);
            s.order.retain(|x| x != id);
            s.options.remove(id);
            if s.active == id {
                s.active = s.enabled.first().cloned().unwrap_or_default();
            }
            if s.popup_active == id {
                s.popup_active.clear();
            }
        })
    }

    /// The models a provider offers (AI endpoints answer this). `options`
    /// overrides the saved settings, so the options page can ask with what
    /// the user typed but has not applied yet.
    pub fn list_models(&self, provider_id: &str, options: Option<&OptionValues>) -> Result<Vec<String>> {
        let provider = self.registry().get(provider_id).ok_or_else(|| Error::NoSuchModule(provider_id.to_owned()))?;
        self.with_pipeline(&CancelToken::new(), |p| {
            let options = match options {
                Some(o) => glossa_modules::provider::resolve_options(provider.options(), Some(o)),
                None => p.options_for(provider.as_ref()),
            };
            provider.list_models(&p.host(&options))
        })
    }

    /// Translate a short sample with one provider and the given settings, for
    /// a "test" button. Returns the translation and how long it took.
    pub fn test_provider(&self, provider_id: &str, options: Option<&OptionValues>) -> Result<(String, Duration)> {
        let provider = self.registry().get(provider_id).ok_or_else(|| Error::NoSuchModule(provider_id.to_owned()))?;
        let cfg = self.config();
        let to = if cfg.general.first_language.is_language() && cfg.general.first_language != LangId::ENGLISH {
            cfg.general.first_language
        } else {
            LangId::from_code("de").unwrap_or(LangId::RUSSIAN)
        };
        let req =
            glossa_core::TranslateRequest::new("Hello! This is a connection test.", LangPair::new(LangId::ENGLISH, to));
        let started = Instant::now();
        let t = self.with_pipeline(&CancelToken::new(), |p| {
            let options = match options {
                Some(o) => glossa_modules::provider::resolve_options(provider.options(), Some(o)),
                None => p.options_for(provider.as_ref()),
            };
            provider.translate(&p.host(&options), &req)
        })?;
        Ok((t.text, started.elapsed()))
    }

    /// Translate, blocking the caller.
    pub fn translate(&self, text: &str, pair: LangPair, opts: &TranslateOptions) -> Result<TranslationOutcome> {
        self.with_pipeline(&CancelToken::new(), |p| p.translate(text, pair, opts))
    }

    /// Translate on a worker; the result arrives as [`Event::Translated`].
    pub fn translate_async(&self, slot: Slot, text: String, pair: LangPair, mut opts: TranslateOptions) -> u64 {
        let (job, cancel) = self.begin(slot);
        // Windows that show translations get them as they are written.
        if opts.progress.is_none() && matches!(slot, Slot::Main | Slot::Popup) {
            let (me, cancel) = (self.clone(), cancel.clone());
            opts.progress = Some(Progress::new(move |text| {
                if !cancel.is_cancelled() && me.current(slot, job) {
                    me.emit(Event::Progress { slot, job, text: text.to_owned() });
                }
            }));
        }
        let me = self.clone();
        self.spawn("translate", move || {
            let result = me.with_pipeline(&cancel, |p| p.translate(&text, pair, &opts));
            if cancel.is_cancelled() || !me.current(slot, job) {
                return;
            }
            if let (Ok(out), true) = (&result, matches!(slot, Slot::Main | Slot::Popup)) {
                me.record_history(out);
            }
            me.emit(Event::Translated { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
        });
        job
    }

    pub fn detect(&self, text: &str) -> Result<Detection> {
        self.with_pipeline(&CancelToken::new(), |p| {
            let prepared = p.prepare(text)?;
            p.detect(&prepared, None)
        })
    }

    /// Look a word up, blocking.
    pub fn dictionary(&self, text: &str, pair: LangPair, provider: Option<&str>) -> Result<Article> {
        let cancel = CancelToken::new();
        self.lookup(text, pair, provider, &cancel)
    }

    fn lookup(&self, text: &str, pair: LangPair, provider: Option<&str>, cancel: &CancelToken) -> Result<Article> {
        let cfg = self.config();
        self.with_pipeline(cancel, |p| {
            let text = p.prepare(text)?;
            let from = if pair.from.is_language() {
                pair.from
            } else {
                p.detect(&text, None).map(|d| d.lang).unwrap_or(cfg.general.second_language)
            };
            let to = if pair.to.is_language() && pair.to != from { pair.to } else { cfg.smart_target(from) };
            let req = DictRequest { text: text.clone(), from, to, exact: cfg.dictionary.exact_search };
            let mut last = Error::NoProvider("dictionary lookup");
            for provider in p.candidates(Capabilities::DICTIONARY, provider, true) {
                p.host_check()?;
                let opts = p.options_for(provider.as_ref());
                let host = Host {
                    http: p.http,
                    options: &opts,
                    ui_language: LangId::from_code(&cfg.general.ui_language).unwrap_or(LangId::ENGLISH),
                    preferred_domain: &cfg.advanced.preferred_domain,
                    cancel,
                };
                let started = Instant::now();
                match provider.dictionary(&host, &req) {
                    Ok(mut a) if !a.is_empty() => {
                        a.provider_id = provider.info().id.clone();
                        a.elapsed_ms = started.elapsed().as_millis() as u64;
                        return Ok(a);
                    }
                    Ok(_) => {
                        last = Error::Module {
                            module: provider.info().id.clone(),
                            detail: format!("no results for {text:?}"),
                        }
                    }
                    Err(e) if e.should_try_next_provider() => last = e,
                    Err(e) => return Err(e),
                }
            }
            Err(last)
        })
    }

    pub fn dictionary_async(&self, slot: Slot, text: String, pair: LangPair, provider: Option<String>) -> u64 {
        let (job, cancel) = self.begin(slot);
        let me = self.clone();
        self.spawn("dictionary", move || {
            let result = me.lookup(&text, pair, provider.as_deref(), &cancel);
            if cancel.is_cancelled() || !me.current(slot, job) {
                return;
            }
            if result.is_ok() {
                me.remember_lookup(&text);
            }
            me.emit(Event::Dictionary { slot, job, query: text, result: result.map(Arc::new).map_err(Arc::new) });
        });
        job
    }

    /// Recognise text in an image, translating it when `translate_to` is set
    /// (by the OCR provider itself if it can, else by the translation
    /// pipeline). Blocking.
    pub fn recognize(&self, image: Vec<u8>, lang: LangId, translate_to: Option<LangId>) -> Result<OcrOutcome> {
        self.recognize_with(image, lang, translate_to, &CancelToken::new())
    }

    fn recognize_with(
        &self,
        image: Vec<u8>,
        lang: LangId,
        translate_to: Option<LangId>,
        cancel: &CancelToken,
    ) -> Result<OcrOutcome> {
        let cfg = self.config();
        if let Some(dir) = &cfg.ocr.save_image_dir {
            let name = format!("glossa-{}.png", glossa_core::now_unix());
            if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join(name), &image)) {
                tracing::warn!("could not save the OCR image: {e}");
            }
        }
        let registry = self.registry();
        let http = self.inner.http.read().clone();
        let mut providers: Vec<Arc<dyn Provider>> = registry.get(&cfg.ocr.provider).into_iter().collect();
        providers.extend(
            registry.with_capability(Capabilities::OCR).into_iter().filter(|p| p.info().id != cfg.ocr.provider),
        );
        if providers.is_empty() {
            return Err(Error::NoProvider("text recognition"));
        }
        let mut req = OcrRequest::new(ImageSource::Bytes(image));
        req.lang = lang;
        req.translate_to = translate_to;
        req.preserve_line_breaks = cfg.ocr.preserve_line_breaks;
        let mut last = Error::NoProvider("text recognition");
        for provider in providers {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let opts = glossa_modules::provider::resolve_options(
                provider.options(),
                cfg.services.options.get(&provider.info().id),
            );
            let host = Host {
                http: http.as_ref(),
                options: &opts,
                ui_language: LangId::ENGLISH,
                preferred_domain: &cfg.advanced.preferred_domain,
                cancel,
            };
            let started = Instant::now();
            match provider.recognize(&host, &req) {
                Ok(mut ocr) => {
                    ocr.provider_id = provider.info().id.clone();
                    ocr.elapsed_ms = started.elapsed().as_millis() as u64;
                    if ocr.is_empty() {
                        return Err(Error::NoTextRecognised);
                    }
                    // Translate ourselves when asked and the provider did not.
                    let translation = match translate_to {
                        Some(to) if ocr.translation.as_deref().is_none_or(|t| t.trim().is_empty()) => {
                            let from = ocr.detected_lang.filter(|l| l.is_language()).unwrap_or(LangId::AUTO);
                            self.with_pipeline(cancel, |p| {
                                p.translate(&ocr.text, LangPair::new(from, to), &TranslateOptions::default())
                            })
                            .map_err(|e| tracing::debug!("translating the OCR text failed: {e}"))
                            .ok()
                        }
                        _ => None,
                    };
                    return Ok(OcrOutcome { ocr, translation });
                }
                Err(e) => {
                    tracing::debug!(provider = %provider.info().id, error = %e, "OCR provider failed");
                    let stop = !e.should_try_next_provider();
                    last = e;
                    if stop {
                        break;
                    }
                }
            }
        }
        Err(last)
    }

    pub fn recognize_async(&self, slot: Slot, image: Vec<u8>, lang: LangId, translate_to: Option<LangId>) -> u64 {
        let (job, cancel) = self.begin(slot);
        let me = self.clone();
        self.spawn("ocr", move || {
            let result = me.recognize_with(image, lang, translate_to, &cancel);
            if cancel.is_cancelled() || !me.current(slot, job) {
                return;
            }
            me.emit(Event::Recognized { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
        });
        job
    }

    /// Speak `text`. With `slow` unset, a second request for the same text
    /// within five seconds plays slower, as the classic "listen again" did.
    pub fn speak(&self, text: &str, lang: LangId, slow: Option<bool>) -> Result<Audio> {
        self.speak_with(text, lang, slow, &CancelToken::new())
    }

    fn speak_with(&self, text: &str, lang: LangId, slow: Option<bool>, cancel: &CancelToken) -> Result<Audio> {
        let cfg = self.config();
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::EmptyInput);
        }
        let slow = slow.unwrap_or_else(|| {
            let mut last = self.inner.last_spoken.lock();
            let repeat = cfg.advanced.slower_listen_repeat
                && last.as_ref().is_some_and(|(t, at)| t == text && at.elapsed() < Duration::from_secs(5));
            *last = Some((text.to_owned(), Instant::now()));
            repeat
        });
        let lang = if lang.is_language() { lang } else { self.detect(text).map(|d| d.lang).unwrap_or(LangId::ENGLISH) };
        let registry = self.registry();
        let http = self.inner.http.read().clone();
        let mut providers: Vec<Arc<dyn Provider>> = registry.get(&cfg.services.tts).into_iter().collect();
        providers.extend(
            registry.with_capability(Capabilities::LISTEN).into_iter().filter(|p| p.info().id != cfg.services.tts),
        );
        let req = SpeakRequest { text: text.to_owned(), lang, slow };
        let mut last = Error::NoProvider("speech");
        for provider in providers.into_iter().filter(|p| p.info().capabilities.contains(Capabilities::LISTEN)) {
            let opts = glossa_modules::provider::resolve_options(
                provider.options(),
                cfg.services.options.get(&provider.info().id),
            );
            let host = Host {
                http: http.as_ref(),
                options: &opts,
                ui_language: LangId::ENGLISH,
                preferred_domain: &cfg.advanced.preferred_domain,
                cancel,
            };
            match provider.speak(&host, &req) {
                Ok(a) if !a.bytes.is_empty() => return Ok(a),
                Ok(_) => last = Error::bad_response(&provider.info().name, "no audio"),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn speak_async(&self, slot: Slot, text: String, lang: LangId, slow: Option<bool>) -> u64 {
        let (job, cancel) = self.begin(slot);
        let me = self.clone();
        self.spawn("speech", move || {
            let result = me.speak_with(&text, lang, slow, &cancel);
            if cancel.is_cancelled() || !me.current(slot, job) {
                return;
            }
            me.emit(Event::Spoken { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
        });
        job
    }

    // -- history -------------------------------------------------------------------

    fn record_history(&self, out: &TranslationOutcome) {
        if !self.inner.config.read().history.enabled {
            return;
        }
        let t = &out.translation;
        let id = self.inner.history.lock().record(
            &out.source,
            HistoryResult {
                provider_id: t.provider_id.clone(),
                from: out.pair.from,
                to: out.pair.to,
                text: t.text.clone(),
            },
            glossa_core::now_unix(),
        );
        if id != 0 {
            self.emit(Event::HistoryChanged);
        }
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        self.inner.history.lock().iter().cloned().collect()
    }

    pub fn search_history(&self, needle: &str, favorites_only: bool) -> Vec<HistoryEntry> {
        self.inner.history.lock().search(needle, favorites_only).into_iter().cloned().collect()
    }

    pub fn set_favorite(&self, id: u64, favorite: bool) {
        if self.inner.history.lock().set_favorite(id, favorite) {
            self.emit(Event::HistoryChanged);
        }
    }

    pub fn remove_history(&self, id: u64) {
        if self.inner.history.lock().remove(id) {
            self.emit(Event::HistoryChanged);
        }
    }

    pub fn clear_history(&self, including_favorites: bool) {
        self.inner.history.lock().clear(including_favorites);
        self.emit(Event::HistoryChanged);
    }

    pub fn export_history(&self, path: &Path, format: ExportFormat, favorites_only: bool) -> Result<usize> {
        let history = self.inner.history.lock();
        let entries = history.search("", favorites_only);
        std::fs::write(path, export::render(&entries, format))?;
        Ok(entries.len())
    }

    fn remember_lookup(&self, word: &str) {
        let mut h = self.inner.dictionary_history.lock();
        h.retain(|w| w != word);
        h.insert(0, word.to_owned());
        h.truncate(200);
    }

    pub fn dictionary_history(&self) -> Vec<String> {
        self.inner.dictionary_history.lock().clone()
    }

    pub fn clear_dictionary_history(&self) {
        self.inner.dictionary_history.lock().clear();
    }

    // -- classic installation -----------------------------------------------------

    /// Import a classic installation's settings and history, and point the
    /// settings at it so its services, themes and languages are used in place.
    pub fn import_classic(&self, dir: &Path) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        let data = dir.join("Data");
        let options_path = data.join("Options.json");
        if options_path.is_file() {
            let text = std::fs::read_to_string(&options_path)?;
            let mut notes = Vec::new();
            self.update_config(|c| {
                match glossa_core::legacy::import_options(&text, c) {
                    Ok(n) => notes = n,
                    Err(e) => notes.push(format!("Options.json: {e}")),
                }
                c.classic.path = Some(dir.to_owned());
            })?;
            report.options_imported = true;
            report.notes.extend(notes);
        } else {
            self.update_config(|c| c.classic.path = Some(dir.to_owned()))?;
        }
        let history_path = data.join("History.json");
        if history_path.is_file() {
            let text = std::fs::read_to_string(&history_path)?;
            let mut imported = History::new(self.config().history.max_entries);
            match glossa_core::legacy::import_history(&text, &mut imported, glossa_core::now_unix()) {
                Ok(n) => {
                    let mut h = self.inner.history.lock();
                    // Classic files are newest first; record oldest first so
                    // the newest ends on top.
                    let entries: Vec<HistoryEntry> = imported.iter().cloned().collect();
                    for e in entries.into_iter().rev() {
                        let fav = e.favorite;
                        for r in e.results {
                            let id = h.record(&e.source, r, e.created);
                            if fav {
                                h.set_favorite(id, true);
                            }
                        }
                    }
                    report.history_entries = n;
                }
                Err(e) => report.notes.push(format!("History.json: {e}")),
            }
            self.emit(Event::HistoryChanged);
        }
        Ok(report)
    }

    // -- lifetime ------------------------------------------------------------------

    /// Save everything that must survive a restart. Safe to call more than
    /// once.
    pub fn shutdown(&self) {
        let cfg = self.config();
        if cfg.history.clear_on_exit {
            self.inner.history.lock().clear(false);
        }
        if cfg.history.enabled {
            if let Err(e) = self.inner.history.lock().save(&self.inner.paths.history_file()) {
                tracing::warn!("history not saved: {e}");
            }
        }
        let words = self.dictionary_history();
        if let Ok(json) = serde_json::to_string(&words) {
            let _ = std::fs::write(self.inner.paths.dictionary_history_file(), json);
        }
        if let Err(e) = self.save_config() {
            tracing::warn!("settings not saved: {e}");
        }
        self.registry().suspend_all();
    }
}

impl Pipeline<'_> {
    fn host_check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// A random token for the control API.
pub fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // Two independently seeded SipHash states mixed with the time and the
    // process id: unguessable enough for a loopback API key, without pulling
    // in a random number crate.
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(name: &str) -> (Engine, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-engine-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.modules.allow_legacy_scripts = true;
        (Engine::with_config(paths, cfg).unwrap(), dir)
    }

    #[test]
    fn starts_with_the_builtin_providers() {
        let (e, dir) = engine("start");
        let translators = e.providers(Capabilities::TRANSLATE);
        assert!(translators.iter().any(|p| p.info.id == "google"));
        assert!(translators.iter().all(|p| p.origin == "Built-in" || p.dir.is_some()));
        assert!(e.providers(Capabilities::OCR).iter().any(|p| p.info.id == "lens"));
        assert!(!e.config().server.token.is_empty(), "a control API token is generated");
        assert_eq!(
            Config::load(&e.paths().config_file()).unwrap().server.token,
            e.config().server.token,
            "and saved, so glossa-cli can use it"
        );
        // Enabled providers come first, in the configured order.
        assert_eq!(translators[0].info.id, "google");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ai_endpoints_become_translators_and_leave_no_trace() {
        let (e, dir) = engine("ai-endpoints");
        let id = e.add_ai_endpoint("Local Qwen", "ollama").unwrap();
        assert_eq!(id, "ai-local-qwen");
        let p = e.registry().get(&id).expect("registered as a provider");
        assert_eq!(p.info().name, "Local Qwen");
        assert!(e.config().services.enabled.contains(&id));
        e.rename_ai_endpoint(&id, "Qwen at home").unwrap();
        assert_eq!(e.registry().get(&id).unwrap().info().name, "Qwen at home");
        e.update_config(|c| {
            c.services.active = id.clone();
            c.services.options.entry(id.clone()).or_default().insert("model".into(), serde_json::json!("qwen3:8b"));
        })
        .unwrap();
        e.remove_ai_endpoint(&id).unwrap();
        let c = e.config();
        assert!(e.registry().get(&id).is_none());
        assert!(!c.services.enabled.contains(&id) && !c.services.order.contains(&id));
        assert!(!c.services.options.contains_key(&id));
        assert_ne!(c.services.active, id);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A one-shot OpenAI-compatible server that streams `words` as events.
    fn sse_server(words: &'static [&'static str]) -> u16 {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            assert!(String::from_utf8_lossy(&body).contains("\"stream\":true"));
            let mut out = stream;
            out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            for w in words {
                let event = serde_json::json!({ "choices": [{ "delta": { "content": w } }] });
                out.write_all(format!("data: {event}\n\n").as_bytes()).unwrap();
                out.flush().unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
            out.write_all(b"data: [DONE]\n\n").unwrap();
        });
        port
    }

    #[test]
    fn streamed_translations_arrive_piece_by_piece() {
        let (e, dir) = engine("stream");
        let port = sse_server(&["При", "вет, ", "мир"]);
        let id = e.add_ai_endpoint("Test", "custom").unwrap();
        e.update_config(|c| {
            let o = c.services.options.entry(id.clone()).or_default();
            o.insert("base_url".into(), serde_json::json!(format!("http://127.0.0.1:{port}/v1")));
            o.insert("model".into(), serde_json::json!("test"));
        })
        .unwrap();
        let rx = e.subscribe();
        let opts = TranslateOptions { provider: Some(id.clone()), fail_over: Some(false), ..Default::default() };
        let job = e.translate_async(
            Slot::Popup,
            "Hello, world".into(),
            LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
            opts,
        );
        let mut progress = Vec::new();
        let result = loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("an event") {
                Event::Progress { job: j, text, .. } if j == job => progress.push(text),
                Event::Translated { job: j, result, .. } if j == job => break result,
                _ => {}
            }
        };
        assert_eq!(result.unwrap().translation.text, "Привет, мир");
        assert_eq!(progress, vec!["При", "Привет, ", "Привет, мир"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_changes_are_saved_and_announced() {
        let (e, dir) = engine("config");
        let rx = e.subscribe();
        e.update_config(|c| c.general.instant_translation = true).unwrap();
        assert!(e.config().general.instant_translation);
        assert!(Config::load(&e.paths().config_file()).unwrap().general.instant_translation);
        assert!(matches!(rx.try_recv(), Ok(Event::ConfigChanged)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn importing_a_classic_installation() {
        let (e, dir) = engine("import");
        let classic = dir.join("QTranslate");
        std::fs::create_dir_all(classic.join("Data")).unwrap();
        std::fs::create_dir_all(classic.join("Services").join("Echo")).unwrap();
        std::fs::write(
            classic.join("Data").join("Options.json"),
            r#"{ "AutoDetection": { "LanguageFirst": 55, "LanguageSecond": 17 }, "HotKeys": { "HotKeyPopupWindow": 1617 } }"#,
        )
        .unwrap();
        std::fs::write(
            classic.join("Data").join("History.json"),
            "[[\"newest\",[[1,17,46,\"новейший\"]],false],[\"oldest\",[[1,17,46,\"старейший\"]],true]]",
        )
        .unwrap();
        std::fs::write(
            classic.join("Services").join("Echo").join("Service.js"),
            r#"function serviceHeader(){return new ServiceHeader(900,"Echo","",Capability.TRANSLATE)}
               function serviceHost(){return "https://echo.test"}
               SupportedLanguages=[-1,"auto"]; SupportedLanguages[17]="en"; SupportedLanguages[46]="ru";
               function serviceTranslateRequest(a,b,c){return new RequestData(HttpMethod.UNDEFINED)}
               function serviceTranslateResponse(a,b,c,d){return new ResponseData(a,c,d)}"#,
        )
        .unwrap();
        let report = e.import_classic(&classic).unwrap();
        assert!(report.options_imported);
        assert_eq!(report.history_entries, 2);
        let cfg = e.config();
        assert_eq!(cfg.general.first_language, LangId::from_code("uk").unwrap());
        assert_eq!(cfg.classic.path.as_deref(), Some(classic.as_path()));
        let h = e.history();
        assert_eq!(h[0].source, "newest", "newest stays on top");
        assert!(h[1].favorite);
        // The classic service is now a provider.
        assert!(e.registry().get("qt-echo").is_some(), "{:?}", e.module_status());
        let out = e
            .translate(
                "hello",
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
                &TranslateOptions { provider: Some("qt-echo".into()), fail_over: Some(false), ..Default::default() },
            )
            .unwrap();
        assert_eq!(out.translation.text, "hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newer_jobs_in_a_slot_supersede_older_ones() {
        let (e, dir) = engine("slots");
        let (a, ca) = e.begin(Slot::Main);
        let (b, _cb) = e.begin(Slot::Main);
        assert!(ca.is_cancelled(), "the older job was cancelled");
        assert!(!e.current(Slot::Main, a));
        assert!(e.current(Slot::Main, b));
        // Detached jobs never cancel anything.
        let (_d, cd) = e.begin(Slot::Detached);
        let (_d2, _) = e.begin(Slot::Detached);
        assert!(!cd.is_cancelled());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_round_trip_through_shutdown() {
        let (e, dir) = engine("history");
        let out = TranslationOutcome {
            translation: glossa_core::Translation {
                text: "мир".into(),
                provider_id: "google".into(),
                ..Default::default()
            },
            pair: LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
            source: "world".into(),
            detected: None,
            back: None,
            failed: vec![],
        };
        e.record_history(&out);
        assert_eq!(e.history().len(), 1);
        e.shutdown();
        let again = Engine::start(e.paths().clone()).unwrap();
        assert_eq!(again.history()[0].source, "world");
        let path = dir.join("out.csv");
        assert_eq!(again.export_history(&path, ExportFormat::Csv, false).unwrap(), 1);
        assert!(std::fs::read_to_string(&path).unwrap().contains("мир"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tokens_are_long_and_distinct() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
