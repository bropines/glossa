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

pub mod actions;
pub mod assets;
#[cfg(feature = "js")]
pub mod bundled;
pub mod catalog;
pub mod dbus;
pub mod export;
pub mod langdetect;
pub mod module_dev;
pub mod ocr;
pub mod panels;
pub mod pipeline;
pub mod presets;
pub mod python;
pub mod server;
pub mod trust;
pub mod update;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::config::{ConfigSource, OcrTranslateWith};
use glossa_core::error::catch_panic;
use glossa_core::history::{HistoryEntry, HistoryResult};
use glossa_core::ocr::{ImageSource, OcrGranularity, OcrRequest};
use glossa_core::secrets::{SecretSlot, SecretStorage, SecretStore, SecretSync};
use glossa_core::{
    Action, Article, Audio, Capabilities, Config, Detection, DictRequest, Error, History, LangId, LangPair, Locale,
    OcrResult, Paths, ProviderInfo, Result, SpeakRequest, Theme,
};
use glossa_modules::host_api::ModuleCache;
use glossa_modules::{
    CancelToken, Host, OptionKind, OptionSpec, OptionValues, Origin, Provider, Registry, RuntimeContext,
};
use glossa_net::HttpClient;
use parking_lot::{Mutex, RwLock};

pub use actions::ActionRequest;
pub use assets::{AssetJobState, AssetReport};
pub use catalog::{LocaleCatalog, LocaleEntry, ThemeCatalog, ThemeEntry};
pub use export::ExportFormat;
pub use glossa_modules::assets::{AssetState, AssetStatus, Progress as AssetProgress};
pub use ocr::{TranslatedBlock, TranslatedLine};
pub use panels::{Effect, Panel, PanelId, PanelInfo, PanelNotice, PanelOutcome, PanelRequest};
pub use pipeline::{Pipeline, Progress, TranslateOptions, TranslateReport, TranslationOutcome};
pub use presets::{PresetChange, PresetEntry, PresetReport};
pub use python::{python_dirs, ModulePython, PythonReport, PYTHON_JOB};
pub use update::{Release, UpdateState, UpdateStatus};

/// The presets an AI endpoint can start from, for front ends.
pub mod ai {
    pub use glossa_services::openai::{preset, AiPreset, PRESETS};
    /// The id of the built-in OpenAI-compatible translator.
    pub const BUILTIN_ID: &str = glossa_services::OpenAi::ID;

    /// Whether `id` is an AI endpoint: the built-in one or one the user
    /// added (`ai-…`).
    pub fn is_endpoint(id: &str) -> bool {
        id == BUILTIN_ID || id.starts_with("ai-")
    }
}

/// Where a job's result goes. One job per slot at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Main,
    Popup,
    Dictionary,
    Ocr,
    /// Translations made in the OCR result window, apart from recognition so
    /// that translating there does not cancel a region being recognised.
    OcrResult,
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
    /// A module's files changed state: a download started, moved on,
    /// finished or failed, or they were checked or deleted.
    AssetsChanged {
        module: String,
    },
    /// Someone outside the UI (the control API) asked for an action, with
    /// optional text to act on instead of the selection.
    Action {
        action: Action,
        text: Option<String>,
    },
    /// A preset applied on a worker ([`Engine::apply_preset_async`]):
    /// what was skipped, or why it failed. [`Event::ConfigChanged`] comes
    /// first when it worked.
    PresetApplied {
        result: std::result::Result<PresetReport, Arc<Error>>,
    },
    /// Presets or the current one changed, from any front end: saved,
    /// renamed, copied, deleted, imported, or `applied` (that preset). A
    /// window with unsaved settings follows it with
    /// [`glossa_core::preset::follow`] instead of writing the old ones back.
    PresetsChanged {
        applied: Option<Arc<glossa_core::Preset>>,
    },
    /// A check for a newer version finished
    /// ([`Engine::check_for_updates_async`]). `automatic` is true for the
    /// daily one nobody asked for, which a front end mentions only when it
    /// found a version the user has not been told about.
    UpdateChecked {
        automatic: bool,
        result: std::result::Result<UpdateStatus, Arc<Error>>,
    },
}

/// OCR, plus a translation when one was asked for.
#[derive(Debug, Clone)]
pub struct OcrOutcome {
    pub ocr: OcrResult,
    /// The translation: the OCR provider's own (under the provider's id), or
    /// one made afterwards.
    pub translation: Option<TranslationOutcome>,
    /// Block-by-block translations with their places, for an in-place
    /// rendering; empty unless one was asked for.
    pub blocks: Vec<TranslatedBlock>,
}

impl OcrOutcome {
    pub fn translated_text(&self) -> Option<&str> {
        self.translation.as_ref().map(|t| t.translation.text.as_str()).or(self.ocr.translation.as_deref())
    }
}

/// How to recognise: see [`Engine::recognize_async_with`].
#[derive(Debug, Clone, Default)]
pub struct RecognizeOptions {
    /// The language in the picture; `AUTO` lets the provider decide.
    pub lang: LangId,
    /// Translate into this language as well.
    pub translate_to: Option<LangId>,
    /// This OCR provider only, no fail-over.
    pub provider: Option<String>,
    /// Translate block by block for painting over the picture
    /// ([`OcrOutcome::blocks`]).
    pub in_place: bool,
    /// Ask for the layout too (paragraphs or lines with their places), as
    /// joining lines by paragraph needs; `in_place` implies it.
    pub layout: bool,
    /// Who translates, overriding `ocr.translate_with` for this request (the
    /// OCR result window's translator choice).
    pub translate_with: Option<OcrTranslateWith>,
    /// How much structure to ask for (words for hOCR or ALTO export), when
    /// more than `in_place` and `layout` imply.
    pub granularity: Option<OcrGranularity>,
}

/// The translation an OCR provider made in the same request, as an outcome
/// like any other: it shows under the provider's name and goes into the
/// history under it.
fn provider_translation(ocr: &OcrResult, to: LangId) -> Option<TranslationOutcome> {
    let text = ocr.translation.clone().filter(|t| !t.trim().is_empty())?;
    let from = ocr.detected_lang.filter(|l| l.is_language()).unwrap_or(LangId::AUTO);
    Some(TranslationOutcome {
        translation: glossa_core::Translation {
            text,
            detected_from: ocr.detected_lang,
            provider_id: ocr.provider_id.clone(),
            elapsed_ms: ocr.elapsed_ms,
            ..Default::default()
        },
        pair: LangPair::new(from, to),
        source: ocr.text.clone(),
        detected: None,
        back: None,
        failed: Vec::new(),
        truncated: false,
    })
}

/// A provider, as the settings page lists it.
#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub info: ProviderInfo,
    pub options: Vec<OptionSpec>,
    /// The settings page in the interface language: `options` laid out and
    /// translated, with its conditions and checks. `info.name` and
    /// `info.description` are already translated when the module
    /// translates them.
    pub page: Arc<glossa_modules::SettingsPage>,
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
    /// False when `history.json` exists but could be neither read nor copied
    /// aside: saving would destroy the only copy of the user's entries.
    history_writable: AtomicBool,
    dictionary_history: Mutex<Vec<String>>,
    /// What changed since the histories were last written, and whether a
    /// worker is on its way to write them (see [`Engine::history_touched`]).
    saver: Mutex<Saver>,
    /// How long after a change the histories are written.
    save_delay: Mutex<Duration>,
    /// One writer of `history.json` at a time in this process.
    history_io: Mutex<()>,
    /// The settings file could not be read at start and is still on disk
    /// as it was: where to copy it before the first save replaces it.
    damaged_config: Mutex<Option<PathBuf>>,
    themes: RwLock<ThemeCatalog>,
    locales: RwLock<LocaleCatalog>,
    locale: RwLock<Arc<Locale>>,
    /// What was wrong in the locale files: unreadable ones, and the chosen
    /// language's skipped strings.
    locale_issues: RwLock<Vec<glossa_core::locale::LocaleIssue>>,
    /// What the current locale was loaded from, so a rescan that finds the
    /// same language and the same files keeps it (and the front end has
    /// nothing to rebuild).
    locale_stamp: RwLock<Option<catalog::LocaleStamp>>,
    modules: RwLock<ModuleStatus>,
    module_cache: Arc<ModuleCache>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    waker: RwLock<Option<Waker>>,
    slots: Mutex<HashMap<Slot, (u64, CancelToken)>>,
    next_job: AtomicU64,
    last_spoken: Mutex<Option<(String, Instant)>>,
    /// Built-in providers are created once; modules are reloaded around them.
    builtins: Vec<Arc<dyn Provider>>,
    /// The built-in OCR providers and the settings they were made from:
    /// remade when the OCR settings change, so those apply without a restart.
    ocr_builtins: Mutex<(glossa_core::config::OcrConfig, Vec<Arc<dyn Provider>>)>,
    /// The JavaScript modules compiled into the program, also created once.
    bundled: Vec<Arc<dyn Provider>>,
    startup_warnings: Mutex<Vec<StartupWarning>>,
    /// The system keychain, once the front end attaches one (see
    /// [`Engine::attach_secret_store`]).
    secret_store: RwLock<Option<Arc<dyn SecretStore>>>,
    /// Whether the system is in dark mode, as the front end last said (see
    /// [`Engine::set_system_dark`]); `None` when it cannot tell.
    system_dark: RwLock<Option<bool>>,
    secret_sync: Mutex<SecretSync>,
    /// Background downloads of module files, by module id.
    asset_jobs: Mutex<HashMap<String, assets::AssetJob>>,
    /// Background installs of the managed Python and modules' environments,
    /// by module id (or [`PYTHON_JOB`]).
    python_jobs: Mutex<HashMap<String, python::PythonJob>>,
    /// The last check for a newer version (`update-check.json`).
    update: Mutex<UpdateState>,
    /// A check for a newer version is running.
    update_busy: AtomicBool,
    /// Python modules' state as last read from disk, for front ends that
    /// draw it every frame; emptied whenever something may have changed it
    /// (see [`Engine::emit`]).
    python_cache: Mutex<HashMap<String, python::ModulePython>>,
    /// Module folders the user created or copied in the module editor in
    /// this session, canonical: the editor's saves approve these.
    authored: Mutex<std::collections::HashSet<PathBuf>>,
    /// Settings forced for this run only ([`Engine::set_session_overrides`]).
    session: RwLock<SessionOverrides>,
}

/// Settings a front end forces for one run without writing them to the
/// settings file (`glossa-cli --offline`, `--detect`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionOverrides {
    /// Treat `[network] offline_only` as on. It can only add the
    /// restriction: `false` leaves the setting as the file has it.
    pub offline_only: bool,
    /// Use this instead of `[general] detect`.
    pub detect: Option<glossa_core::config::DetectMode>,
}

impl SessionOverrides {
    fn apply(&self, config: &mut Config) {
        config.network.offline_only |= self.offline_only;
        if let Some(detect) = self.detect {
            config.general.detect = detect;
        }
    }
}

/// What the history worker has to write.
#[derive(Debug, Default)]
struct Saver {
    history: bool,
    dictionary: bool,
    /// A worker is waiting to write.
    scheduled: bool,
}

/// How long after a change the histories are written to disk: long enough
/// that a run of translations is one write, short enough that a crash or a
/// power cut loses seconds, not the session.
const HISTORY_SAVE_DELAY: Duration = Duration::from_secs(3);

/// Something that went wrong at start without stopping it
/// ([`Engine::startup_warnings`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupWarning {
    /// A locale key whose text has a `%s` for each of `args`.
    pub key: &'static str,
    pub args: Vec<String>,
}

impl StartupWarning {
    /// The message in `locale`'s language.
    pub fn message(&self, locale: &glossa_core::Locale) -> String {
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        locale.fmt_args(self.key, &args)
    }
}

/// The engine. Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Start with the settings found under `paths`. Nothing in the
    /// settings file stops the start, and reading it changes nothing on
    /// disk (a command that only reads must leave the file alone):
    ///
    /// * a value this version cannot use takes its default, the rest of
    ///   the file is kept, and a warning names the value;
    /// * a file that is empty or not TOML at all is left where it is; the
    ///   last good copy (`config.toml.bak`) is used, or the defaults, and
    ///   the first save copies the unreadable file aside before replacing
    ///   it.
    ///
    /// [`Engine::startup_warnings`] says what happened. What earlier
    /// versions left readable by other users is made private, and what a
    /// killed process left behind is swept ([`Paths::secure`]).
    pub fn start(paths: Paths) -> Result<Engine> {
        paths.secure();
        // Not in this crate's tests, whose engines share one process and
        // delete their folders when they are done.
        #[cfg(not(test))]
        glossa_core::fsio::set_temp_dir(paths.temp_dir());
        let file = paths.config_file();
        let loaded = Config::load_checked(&file)?;
        let mut config = loaded.config;
        let mut warnings = Vec::new();
        let keep = Config::damaged_path(&file, glossa_core::now_unix());
        let mut damaged = None;
        match loaded.source {
            ConfigSource::File | ConfigSource::Missing => {}
            ConfigSource::Backup { error } if error.is_empty() => {
                warnings.push(StartupWarning { key: "startup.config_empty", args: Vec::new() });
                // Nothing in it to keep, but nothing is written at start either.
                damaged = Some(keep);
            }
            ConfigSource::Backup { error } => {
                warnings.push(StartupWarning {
                    key: "startup.config_restored",
                    args: vec![keep.display().to_string(), error],
                });
                damaged = Some(keep);
            }
            ConfigSource::Defaults { error } => {
                // The approvals went with the broken file. Granting everything
                // installed (the upgrade migration) would approve what was
                // waiting or declined, so every hand-installed module asks again.
                config.modules.trust_initialised = true;
                warnings.push(StartupWarning {
                    key: "startup.config_broken",
                    args: vec![keep.display().to_string(), error],
                });
                damaged = Some(keep);
            }
        }
        for issue in loaded.issues.iter().take(MAX_VALUE_WARNINGS) {
            tracing::warn!("{}: {}: {}; the default is used", file.display(), issue.path, issue.problem);
            warnings.push(StartupWarning {
                key: "startup.config_value",
                args: vec![issue.path.clone(), issue.problem.clone()],
            });
        }
        if loaded.issues.len() > MAX_VALUE_WARNINGS {
            warnings.push(StartupWarning {
                key: "startup.config_more_values",
                args: vec![(loaded.issues.len() - MAX_VALUE_WARNINGS).to_string()],
            });
        }
        let engine = Engine::build(paths, config, damaged)?;
        engine.inner.startup_warnings.lock().splice(0..0, warnings);
        Ok(engine)
    }

    /// Problems met while starting that the user should hear about once (a
    /// broken `config.toml` that was set aside). Each has a locale key and
    /// its arguments, for the front end to show in the interface language.
    pub fn startup_warnings(&self) -> Vec<StartupWarning> {
        self.inner.startup_warnings.lock().clone()
    }

    /// Start with explicit settings (tests, `--config`).
    pub fn with_config(paths: Paths, config: Config) -> Result<Engine> {
        Engine::build(paths, config, None)
    }

    /// `damaged`: the settings file on disk is not where `config` came from
    /// and must be copied there before anything replaces it.
    fn build(paths: Paths, mut config: Config, damaged: Option<PathBuf>) -> Result<Engine> {
        config.sanitise();
        // With an unreadable file on disk nothing is written at start; the
        // token is saved with the first change of settings.
        if ensure_token(&mut config) && damaged.is_none() {
            // Other programs (glossa-cli among them) read the token from the
            // file, so it has to be there from the first run.
            if let Err(e) = config.save(&paths.config_file()) {
                tracing::warn!("could not save the new control API token: {e}");
            }
        }
        // A proxy that cannot be used (a scheme nothing here speaks, in the
        // settings or in HTTPS_PROXY) must not keep the program from
        // starting, least of all the window where it would be corrected.
        let (http, proxy_problem) = HttpClient::new_or_direct(&config.network);
        let http = Arc::new(http);
        let mut startup_warnings = Vec::new();
        if let Some(e) = proxy_problem {
            tracing::warn!("{e}; connecting without a proxy");
            startup_warnings.push(StartupWarning { key: "startup.proxy_unusable", args: vec![e.to_string()] });
        }
        let (history, history_writable) = if config.history.enabled {
            load_history(&paths.history_file(), config.history.max_entries)
        } else {
            (History::new(config.history.max_entries), true)
        };
        let dictionary_history = std::fs::read_to_string(paths.dictionary_history_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let builtins = glossa_services::builtin_providers();
        let ocr_builtins =
            Mutex::new((config.ocr.clone(), glossa_ocr::builtin_providers(&config.ocr, &paths.cache_dir)));
        let module_cache = ModuleCache::new();
        #[cfg(feature = "js")]
        let bundled = bundled::providers(&module_cache);
        #[cfg(not(feature = "js"))]
        let bundled = Vec::new();

        let update = update::load_state(&paths.update_state_file());
        let inner = Inner {
            paths,
            config: RwLock::new(config),
            registry: RwLock::new(Arc::new(Registry::new())),
            http: RwLock::new(http),
            history: Mutex::new(history),
            history_writable: AtomicBool::new(history_writable),
            dictionary_history: Mutex::new(dictionary_history),
            saver: Mutex::new(Saver::default()),
            save_delay: Mutex::new(HISTORY_SAVE_DELAY),
            history_io: Mutex::new(()),
            damaged_config: Mutex::new(damaged),
            themes: RwLock::new(ThemeCatalog::default()),
            locales: RwLock::new(LocaleCatalog::default()),
            locale: RwLock::new(Arc::new(Locale::english())),
            locale_issues: RwLock::new(Vec::new()),
            locale_stamp: RwLock::new(None),
            modules: RwLock::new(ModuleStatus::default()),
            module_cache,
            subscribers: Mutex::new(Vec::new()),
            waker: RwLock::new(None),
            slots: Mutex::new(HashMap::new()),
            next_job: AtomicU64::new(1),
            last_spoken: Mutex::new(None),
            builtins,
            ocr_builtins,
            bundled,
            startup_warnings: Mutex::new(startup_warnings),
            secret_store: RwLock::new(None),
            system_dark: RwLock::new(None),
            secret_sync: Mutex::new(SecretSync::new()),
            asset_jobs: Mutex::new(HashMap::new()),
            python_jobs: Mutex::new(HashMap::new()),
            update: Mutex::new(update),
            update_busy: AtomicBool::new(false),
            python_cache: Mutex::new(HashMap::new()),
            authored: Mutex::new(Default::default()),
            session: RwLock::new(SessionOverrides::default()),
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

    /// The settings in effect: those of the settings file, with this run's
    /// overrides ([`Engine::set_session_overrides`]) over them. The
    /// overrides are never written to the file by the engine; a front end
    /// that sets them must not write this copy back whole.
    pub fn config(&self) -> Config {
        let mut config = self.inner.config.read().clone();
        self.inner.session.read().apply(&mut config);
        config
    }

    /// Force settings for this run only. Nothing is written to the settings
    /// file, and a later [`Engine::update_config`] keeps them in force.
    pub fn set_session_overrides(&self, overrides: SessionOverrides) -> Result<()> {
        let before = self.config();
        *self.inner.session.write() = overrides;
        let after = self.config();
        if before.network != after.network {
            *self.inner.http.write() = Arc::new(HttpClient::new(&after.network)?);
            // The module runtimes hold "may download on first use".
            self.reload_modules();
        }
        Ok(())
    }

    /// Change settings, save them, and apply whatever the change affects.
    ///
    /// Network settings are checked first: a proxy entered in the settings
    /// that cannot be used is an error (naming the proxy, for the front end
    /// to show at the field), and nothing is changed or saved, so a wrong
    /// entry cannot reach the file and meet the user at every later start.
    pub fn update_config(&self, change: impl FnOnce(&mut Config)) -> Result<()> {
        let (before, after, http) = {
            let mut cfg = self.inner.config.write();
            let before = cfg.clone();
            let mut after = before.clone();
            change(&mut after);
            after.sanitise();
            // The token is always required, so clearing it (an emptied
            // field in the options) means "make a new one", saved below.
            ensure_token(&mut after);
            let http = if before.network != after.network {
                // This run's overrides stay in force for the new client.
                let mut effective = after.clone();
                self.inner.session.read().apply(&mut effective);
                let (client, problem) = HttpClient::new_or_direct(&effective.network);
                match problem {
                    Some(e) if glossa_net::proxy_is_manual(&after.network) => return Err(e),
                    // The system's proxy is not the settings' to correct:
                    // go without, as at start, until the system names one
                    // that works.
                    Some(e) => tracing::warn!("{e}; connecting without a proxy"),
                    None => {}
                }
                Some(client)
            } else {
                None
            };
            *cfg = after.clone();
            (before, after, http)
        };
        self.write_config(&after)?;
        if let Some(http) = http {
            *self.inner.http.write() = Arc::new(http);
        }
        if before.modules != after.modules
            || before.services.ai_endpoints != after.services.ai_endpoints
            || before.classic != after.classic
            || before.ocr != after.ocr
            || before.dictionary.offline != after.dictionary.offline
            || before.network.offline_only != after.network.offline_only
        {
            self.reload_modules();
        }
        if before.classic != after.classic || before.appearance.theme != after.appearance.theme {
            self.reload_themes();
        }
        if before.classic != after.classic || before.general.ui_language != after.general.ui_language {
            self.reload_locales();
        }
        if !before.history.enabled && after.history.enabled {
            // Switched on after a start with it off: the file was never read,
            // and saving the empty list would replace it.
            let mut history = self.inner.history.lock();
            if history.is_empty() {
                let (loaded, writable) = load_history(&self.inner.paths.history_file(), after.history.max_entries);
                *history = loaded;
                self.inner.history_writable.store(writable, Ordering::SeqCst);
            }
        }
        if before.history.max_entries != after.history.max_entries {
            self.inner.history.lock().set_max_entries(after.history.max_entries);
        }
        self.emit(Event::ConfigChanged);
        Ok(())
    }

    /// Save the settings as they are (window positions, pane contents).
    pub fn save_config(&self) -> Result<()> {
        let cfg = self.inner.config.read().clone();
        self.write_config(&cfg)
    }

    /// Write `cfg` to the settings file, with its secrets moved to the
    /// system keychain when the settings say so (see
    /// [`glossa_core::secrets`]). A file that could not be read at start is
    /// copied aside first; if that fails nothing is written, so the user's
    /// text is never the price of a save.
    fn write_config(&self, cfg: &Config) -> Result<()> {
        let file = self.inner.paths.config_file();
        {
            let mut damaged = self.inner.damaged_config.lock();
            if let Some(keep) = damaged.as_ref() {
                Config::keep_damaged(&file, keep)?;
                *damaged = None;
            }
        }
        let store = self.inner.secret_store.read().clone();
        let slots = self.secret_slots(cfg);
        let (out, errors) = self.inner.secret_sync.lock().prepare(cfg, &slots, store.as_deref());
        for e in errors {
            tracing::warn!("keychain: {e}; kept in the settings file instead");
        }
        out.save(&file)
    }

    /// Every place in `cfg` known now to hold a secret: the loaded
    /// services' options of the secret kind, any option the file marks as
    /// kept in the keychain, the proxy password, the Lens token, the
    /// control API's token. The secret sync adds those it has seen in the
    /// keychain before, so a service that is no longer loaded (a module
    /// removed or failing) does not turn its key into plain text.
    fn secret_slots(&self, cfg: &Config) -> Vec<SecretSlot> {
        let registry = self.registry();
        let mut slots = SecretSlot::marked(cfg);
        for (service, values) in &cfg.services.options {
            let specs = registry.get(service).map(|p| p.options().to_vec()).unwrap_or_default();
            for key in values.keys() {
                if specs.iter().any(|s| &s.key == key && s.kind == OptionKind::Secret) {
                    let slot = SecretSlot::ServiceOption { service: service.clone(), key: key.clone() };
                    if !slots.contains(&slot) {
                        slots.push(slot);
                    }
                }
            }
        }
        slots.extend(SecretSlot::fixed());
        slots
    }

    /// Attach the system keychain (`None` where there is none) and fetch the
    /// secrets the settings file keeps there. Call it right after start,
    /// before anything is translated or the control API is served: until
    /// then those settings read as the placeholder.
    ///
    /// A secret the keychain does not give (it is locked, refuses, or is
    /// not there) reads as empty from here on, so its service says a key is
    /// missing rather than sending the placeholder as one, and a start-up
    /// warning names the keychain. The file keeps its placeholder, so the
    /// secret is back when the keychain answers.
    pub fn attach_secret_store(&self, store: Option<Arc<dyn SecretStore>>) {
        *self.inner.secret_store.write() = store.clone();
        let (wanted, problems, unresolved, new_token) = {
            let mut cfg = self.inner.config.write();
            let slots = self.secret_slots(&cfg);
            let mut sync = self.inner.secret_sync.lock();
            let problems = sync.resolve(&mut cfg, &slots, store.as_deref());
            // A token the keychain does not hold is replaced like any
            // missing one. One it holds but would not give is left empty
            // (the control API then refuses everything): writing a new one
            // would put this program at odds with a running Glossa that
            // did get the old one.
            let new_token = !sync.is_unresolved(&SecretSlot::ControlToken) && ensure_token(&mut cfg);
            (cfg.general.secret_storage == SecretStorage::System, problems, sync.has_unresolved(), new_token)
        };
        for problem in &problems {
            tracing::warn!("keychain: {problem}");
        }
        match &store {
            None if wanted || unresolved => {
                self.inner
                    .startup_warnings
                    .lock()
                    .push(StartupWarning { key: "startup.no_keychain", args: Vec::new() });
            }
            Some(store) if unresolved => {
                self.inner
                    .startup_warnings
                    .lock()
                    .push(StartupWarning { key: "startup.keychain_error", args: vec![store.name().to_owned()] });
            }
            _ => {}
        }
        if new_token {
            if let Err(e) = self.save_config() {
                tracing::warn!("could not save the new control API token: {e}");
            }
        }
    }

    /// The name of the attached keychain, if any.
    pub fn secret_store_name(&self) -> Option<&'static str> {
        self.inner.secret_store.read().as_ref().map(|s| s.name())
    }

    pub fn registry(&self) -> Arc<Registry> {
        self.inner.registry.read().clone()
    }

    /// Where the managed Python, `uv`, the modules' environments and `uv`'s
    /// cache go.
    pub fn python_dirs(&self) -> glossa_modules::python_env::PythonDirs {
        let mut dirs = python::python_dirs(&self.inner.paths);
        dirs.proxy = glossa_net::proxy_url(&self.config().network);
        dirs
    }

    pub fn module_status(&self) -> ModuleStatus {
        self.inner.modules.read().clone()
    }

    /// Rebuild the provider list: built-ins, the modules compiled into the
    /// program, then module directories in priority order (the classic
    /// installation's services, modules installed beside the program, the
    /// user's modules, extra directories from the settings). A module with the
    /// id of an earlier provider replaces it.
    pub fn reload_modules(&self) {
        let started = Instant::now();
        let cfg = self.config();
        let paths = &self.inner.paths;
        let ctx = self.runtime_context(&cfg);
        let mut reg = Registry::new();
        let ocr_builtins = {
            let mut ocr = self.inner.ocr_builtins.lock();
            if ocr.0 != cfg.ocr {
                *ocr = (cfg.ocr.clone(), glossa_ocr::builtin_providers(&cfg.ocr, &paths.cache_dir));
            }
            ocr.1.clone()
        };
        for p in self.inner.builtins.iter().chain(&ocr_builtins) {
            if !cfg.modules.disabled.contains(&p.info().id) {
                reg.add_builtin(p.clone());
            }
        }
        for p in &self.inner.bundled {
            if !cfg.modules.disabled.contains(&p.info().id) {
                reg.add_bundled(p.clone());
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
            ms = started.elapsed().as_millis() as u64,
            "providers ready"
        );
        let old = std::mem::replace(&mut *self.inner.registry.write(), Arc::new(reg));
        let suspending = Instant::now();
        old.suspend_all();
        if suspending.elapsed() > Duration::from_millis(200) {
            tracing::debug!(ms = suspending.elapsed().as_millis() as u64, "suspending the old modules was slow");
        }
        *self.inner.modules.write() =
            ModuleStatus { loaded: report.loaded, failed: report.failed, skipped: report.skipped };
        if !cfg.modules.trust_initialised {
            self.initialise_trust();
        }
        self.bind_grants();
        self.adopt_providers();
        self.emit(Event::ModulesReloaded);
    }

    /// What the module runtimes get from the settings.
    fn runtime_context(&self, cfg: &Config) -> RuntimeContext {
        let paths = &self.inner.paths;
        RuntimeContext {
            python: cfg.modules.python.clone(),
            python_dirs: self.python_dirs(),
            java: cfg.modules.java.clone(),
            allow_native: cfg.modules.allow_native,
            allow_legacy_scripts: cfg.modules.allow_legacy_scripts,
            call_timeout: Duration::from_millis(cfg.modules.call_timeout_ms),
            rate_limit_cooldown: true,
            cache: self.inner.module_cache.clone(),
            cache_dir: paths.cache_dir.join("modules"),
            models_dir: paths.models_dir(),
            // Offline only: a module's first call must not start a download.
            auto_download_assets: cfg.modules.auto_download_assets && !cfg.network.offline_only,
            disabled: cfg.modules.disabled.clone(),
            trust: self.trust_policy(cfg),
            preferred_domain: cfg.advanced.preferred_domain.clone(),
            ui_language_code: self.locale().code.clone(),
        }
    }

    /// Bring the service lists up to date with the providers that exist now.
    ///
    /// Nothing about which services exist is hard-coded in the settings: a
    /// provider that shows up for the first time (a built-in on the first
    /// run, a module installed later) is switched on if it works without
    /// setup, otherwise only listed, and remembered as seen either way, so a
    /// service the user switched off stays off. Empty choices (the active
    /// tab, the speech service) are then filled from what is enabled.
    fn adopt_providers(&self) {
        let registry = self.registry();
        let mut cfg = self.inner.config.write();
        let before = cfg.services.clone();
        let s = &mut cfg.services;
        for entry in registry.entries() {
            let info = entry.info();
            // Adopted once approved: switching on a module that cannot run
            // yet would only make calls fail over past it.
            if entry.pending || s.seen.contains(&info.id) {
                continue;
            }
            s.seen.push(info.id.clone());
            if !info.ready_to_use {
                continue;
            }
            let id = info.id.clone();
            if info.capabilities.contains(Capabilities::TRANSLATE) && !s.enabled.contains(&id) {
                s.enabled.push(id.clone());
            }
            if info.capabilities.contains(Capabilities::DICTIONARY) && !s.dictionary_enabled.contains(&id) {
                s.dictionary_enabled.push(id.clone());
            }
            if info.capabilities.contains(Capabilities::LISTEN) && s.tts.is_empty() {
                s.tts = id;
            }
        }
        if s.active.is_empty() {
            s.active = s.enabled.first().cloned().unwrap_or_default();
        }
        if s.dictionary_active.is_empty() {
            s.dictionary_active = s.dictionary_enabled.first().cloned().unwrap_or_default();
        }
        if s.tts.is_empty() {
            if let Some(p) = registry.with_capability(Capabilities::LISTEN).first() {
                s.tts = p.info().id.clone();
            }
        }
        if cfg.services != before {
            let snapshot = cfg.clone();
            drop(cfg);
            if let Err(e) = self.write_config(&snapshot) {
                tracing::warn!("could not save the service lists: {e}");
            }
        }
    }

    fn reload_catalogs(&self) {
        self.reload_themes();
        self.reload_locales();
    }

    fn reload_themes(&self) {
        let cfg = self.config();
        let paths = &self.inner.paths;
        let mut theme_dirs = paths.theme_dirs();
        theme_dirs.push(paths.user_theme_dir());
        let classic_themes: Vec<PathBuf> = cfg.classic.themes_dir().into_iter().collect();
        *self.inner.themes.write() = ThemeCatalog::discover(&theme_dirs, &classic_themes);
    }

    /// Rescan the locale folders (headers only) and load the chosen language
    /// again only if it, or one of its files, changed: a new `Locale` makes
    /// the front end rebuild its menus.
    fn reload_locales(&self) {
        let cfg = self.config();
        let paths = &self.inner.paths;
        let mut locale_dirs: Vec<PathBuf> = cfg.classic.locales_dir().into_iter().collect();
        locale_dirs.extend(paths.locale_dirs());
        let locales = LocaleCatalog::discover(&locale_dirs, Some(&paths.user_locale_dir()));
        let stamp = locales.stamp(&cfg.general.ui_language);
        if self.inner.locale_stamp.read().as_ref() != Some(&stamp) {
            let (locale, issues) = locales.load_checked(&cfg.general.ui_language);
            *self.inner.locale.write() = Arc::new(locale);
            *self.inner.locale_issues.write() = issues;
            *self.inner.locale_stamp.write() = Some(stamp);
        }
        *self.inner.locales.write() = locales;
    }

    /// Every provider with `cap`, in the user's order, with what the settings
    /// page needs to show about each.
    pub fn providers(&self, cap: Capabilities) -> Vec<ProviderSummary> {
        let cfg = self.config();
        let lang = self.locale().code.clone();
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
                    Origin::Bundled => ("Bundled".to_owned(), None),
                    Origin::Module { dir, runtime, .. } => (runtime.label().to_owned(), Some(dir.clone())),
                };
                let page = e.provider.settings_page(&lang);
                let mut info = e.info().clone();
                if let Some(name) = &page.name {
                    info.name = name.clone();
                }
                if let Some(description) = &page.description {
                    info.description = description.clone();
                }
                ProviderSummary {
                    info,
                    options: e.provider.options().to_vec(),
                    page,
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

    /// Check a provider id a user typed (`glossa-cli -s`, the API's
    /// `service`) before using it: the pipeline would quietly skip an id
    /// that does not exist and let another service answer, which hides a
    /// typo. The error names a few existing ids that look like it.
    pub fn require_provider(&self, id: &str, cap: Capabilities) -> std::result::Result<(), String> {
        let registry = self.registry();
        let what = cap.names().first().copied().unwrap_or("this");
        if let Some(p) = registry.get(id) {
            return if p.info().capabilities.contains(cap) {
                Ok(())
            } else {
                Err(format!("service {id:?} cannot do {what}"))
            };
        }
        let capable: Vec<String> = registry.with_capability(cap).iter().map(|p| p.info().id.clone()).collect();
        let near = glossa_core::text::closest(id, capable.iter().map(String::as_str), 3);
        let hint = if near.is_empty() {
            let some: Vec<&str> = capable.iter().take(5).map(String::as_str).collect();
            format!(" (services that can do {what} include: {})", some.join(", "))
        } else {
            format!("; did you mean {}?", near.join(", "))
        };
        Err(format!("unknown service {id:?}{hint}"))
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

    /// The theme in effect: the chosen one, or with
    /// `appearance.follow_system_theme` the light or dark one by the
    /// system's mode.
    pub fn current_theme(&self) -> Theme {
        let name = self.current_theme_name();
        self.theme(&name)
    }

    pub fn current_theme_name(&self) -> String {
        let dark = *self.inner.system_dark.read();
        self.inner.config.read().appearance.effective_theme(dark).to_owned()
    }

    /// Whether the system is in dark mode, as last told
    /// ([`Engine::set_system_dark`]): cheap, unlike asking the system.
    pub fn system_dark(&self) -> Option<bool> {
        *self.inner.system_dark.read()
    }

    /// Tell the engine whether the system is in dark mode (`None`: it cannot
    /// be told). When that changes the theme in effect, the windows are told
    /// to restyle (`Event::ConfigChanged`). Cheap to call often.
    pub fn set_system_dark(&self, dark: Option<bool>) {
        if *self.inner.system_dark.read() == dark {
            return;
        }
        let before = self.current_theme_name();
        *self.inner.system_dark.write() = dark;
        if self.current_theme_name() != before {
            tracing::debug!(dark = ?dark, theme = %self.current_theme_name(), "the system changed mode");
            self.emit(Event::ConfigChanged);
        }
    }

    /// Re-scan theme and locale folders (after the user edits a theme file).
    pub fn refresh_catalogs(&self) {
        self.reload_catalogs();
        self.emit(Event::ConfigChanged);
    }

    pub fn locales(&self) -> Vec<LocaleEntry> {
        self.inner.locales.read().entries().to_vec()
    }

    /// An interface language by code or name, with the user's files laid
    /// over it, whatever language the program itself shows.
    pub fn load_locale(&self, wanted: &str) -> Locale {
        self.inner.locales.read().load(wanted)
    }

    /// Problems in the locale files, for Options.
    pub fn locale_issues(&self) -> Vec<glossa_core::locale::LocaleIssue> {
        let mut issues = self.inner.locales.read().broken().to_vec();
        issues.extend(self.inner.locale_issues.read().iter().cloned());
        issues
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
        // A finished call may have installed a module's Python on the way
        // (automatic downloads), so anything but streaming progress counts.
        if !matches!(event, Event::Progress { .. }) {
            self.inner.python_cache.lock().clear();
        }
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

    /// Run `f` on a worker thread. A thread that cannot be started is only
    /// logged: for a job a front end waits on, use [`Engine::spawn_or`].
    fn spawn(&self, name: &str, f: impl FnOnce() + Send + 'static) {
        self.spawn_or(name, f, |_| {});
    }

    /// Run `f` on a worker thread, or, when the system gives no thread (out
    /// of memory, a thread limit), call `failed` here with the reason. A job
    /// someone waits for must end in an event either way: without one the
    /// window that asked shows "working" for ever.
    fn spawn_or(&self, name: &str, f: impl FnOnce() + Send + 'static, failed: impl FnOnce(Error)) {
        // 8 MB of address space, committed only as used: module scripts
        // run here, and turning a deeply nested result (an OCR layout) into
        // JSON recurses through the JavaScript engine.
        // Jobs turn their own panics into error results (`catch_panic`);
        // this catches whatever is left (emitting the event, say) so a bug
        // costs one job, not the program.
        let job = name.to_owned();
        let f = move || {
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                let msg = glossa_core::error::panic_message(payload.as_ref());
                tracing::error!("worker thread for {job} panicked: {msg}");
            }
        };
        if let Err(e) = Self::start_thread(name, f) {
            tracing::error!("could not start a worker thread for {name}: {e}");
            failed(Error::Internal(format!("could not start a worker thread for {name}: {e}")));
        }
    }

    fn start_thread(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
        #[cfg(test)]
        if tests::THREADS_FAIL.with(|fail| fail.get()) {
            return Err(std::io::Error::other("no threads left (test)"));
        }
        std::thread::Builder::new().name(format!("glossa-{name}")).stack_size(8 << 20).spawn(f).map(|_| ())
    }

    fn with_pipeline<T>(&self, cancel: &CancelToken, f: impl FnOnce(&Pipeline) -> T) -> T {
        let registry = self.registry();
        let config = self.config();
        let http = self.inner.http.read().clone();
        // Every request of this job carries its cancel flag, so a wait
        // before a retry ends when the job is cancelled.
        let http = glossa_net::Cancellable::new(http.as_ref(), cancel.flag());
        let p = Pipeline { registry: &registry, config: &config, http: &http, cancel };
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
            // Judged with the settings being tested: an address typed but
            // not applied yet is the one the request would go to.
            if p.config.network.offline_only && !provider.is_local(&options) {
                return Err(Error::OfflineOnly(format!("{} is an online service", provider.info().name)));
            }
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
        if opts.progress.is_none() && matches!(slot, Slot::Main | Slot::Popup | Slot::OcrResult) {
            let (me, cancel) = (self.clone(), cancel.clone());
            opts.progress = Some(Progress::new(move |text| {
                if !cancel.is_cancelled() && me.current(slot, job) {
                    me.emit(Event::Progress { slot, job, text: text.to_owned() });
                }
            }));
        }
        let me = self.clone();
        self.spawn_or(
            "translate",
            move || {
                let result =
                    catch_panic("translation", || me.with_pipeline(&cancel, |p| p.translate(&text, pair, &opts)));
                if cancel.is_cancelled() || !me.current(slot, job) {
                    return;
                }
                if let (Ok(out), true) = (&result, matches!(slot, Slot::Main | Slot::Popup | Slot::OcrResult)) {
                    me.record_history(out);
                }
                me.emit(Event::Translated { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
            },
            |e| self.emit(Event::Translated { slot, job, result: Err(Arc::new(e)) }),
        );
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
            // Fail-over follows the same setting as translation: with it off,
            // a failing dictionary reports its own error instead of another
            // dictionary's article turning up in its place.
            let fail_over = cfg.services.fail_over;
            let mut last = p.no_provider("dictionary lookup");
            for provider in p.candidates(Capabilities::DICTIONARY, provider, fail_over) {
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
                        last = Error::NoResults { provider: provider.info().id.clone(), query: text.clone() };
                        if !fail_over {
                            return Err(last);
                        }
                    }
                    Err(e) if fail_over && e.should_try_next_provider() => {
                        tracing::debug!(provider = %provider.info().id, error = %e, "dictionary failed, trying the next");
                        last = e;
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(last)
        })
    }

    pub fn dictionary_async(&self, slot: Slot, text: String, pair: LangPair, provider: Option<String>) -> u64 {
        let (job, cancel) = self.begin(slot);
        let me = self.clone();
        let query = text.clone();
        self.spawn_or(
            "dictionary",
            move || {
                let result = catch_panic("dictionary lookup", || me.lookup(&text, pair, provider.as_deref(), &cancel));
                if cancel.is_cancelled() || !me.current(slot, job) {
                    return;
                }
                if result.is_ok() {
                    me.remember_lookup(&text);
                }
                me.emit(Event::Dictionary { slot, job, query: text, result: result.map(Arc::new).map_err(Arc::new) });
            },
            |e| self.emit(Event::Dictionary { slot, job, query, result: Err(Arc::new(e)) }),
        );
        job
    }

    /// Let the chosen OCR engine get ready for a recognition that is about to
    /// come (the region overlay just opened), on a worker thread. Returns at
    /// once; does nothing for engines without a slow first call.
    pub fn warm_up_ocr(&self) {
        let Some(provider) = self.registry().get(&self.config().ocr.provider) else { return };
        // Warming up may open a connection (Lens does).
        if self.with_pipeline(&CancelToken::new(), |p| p.offline_refusal(provider.as_ref())).is_some() {
            return;
        }
        let spawned = std::thread::Builder::new().name("glossa-ocr-warm-up".into()).spawn(move || provider.warm_up());
        if let Err(e) = spawned {
            tracing::debug!("OCR warm-up not started: {e}");
        }
    }

    /// Recognise text in an image, translating it when `translate_to` is set
    /// (by the OCR provider itself if it can and `ocr.translate_with` allows,
    /// else by the translation pipeline). Blocking.
    pub fn recognize(&self, image: Vec<u8>, lang: LangId, translate_to: Option<LangId>) -> Result<OcrOutcome> {
        let opts = RecognizeOptions { lang, translate_to, ..Default::default() };
        self.recognize_with(image, &opts, &CancelToken::new())
    }

    /// [`Engine::recognize`] with one OCR provider (`windows-ocr`, `oneocr`,
    /// a module's id…) and no fail-over to the others. Blocking.
    pub fn recognize_using(
        &self,
        provider: &str,
        image: Vec<u8>,
        lang: LangId,
        translate_to: Option<LangId>,
    ) -> Result<OcrOutcome> {
        let opts = RecognizeOptions { lang, translate_to, provider: Some(provider.to_owned()), ..Default::default() };
        self.recognize_with(image, &opts, &CancelToken::new())
    }

    /// Recognise and translate block by block, for painting the translation
    /// over the picture: [`OcrOutcome::blocks`] holds each paragraph's
    /// translation and place. Blocking.
    pub fn recognize_in_place(
        &self,
        image: Vec<u8>,
        lang: LangId,
        to: LangId,
        provider: Option<&str>,
    ) -> Result<OcrOutcome> {
        let opts = RecognizeOptions {
            lang,
            translate_to: Some(to),
            provider: provider.map(str::to_owned),
            in_place: true,
            layout: true,
            ..Default::default()
        };
        self.recognize_with(image, &opts, &CancelToken::new())
    }

    /// [`Engine::recognize`] asking the provider for `granularity` (words
    /// for hOCR or ALTO export, lines for an overlay), with `provider` alone
    /// when given. Blocking.
    pub fn recognize_detailed(
        &self,
        image: Vec<u8>,
        lang: LangId,
        translate_to: Option<LangId>,
        provider: Option<&str>,
        granularity: OcrGranularity,
    ) -> Result<OcrOutcome> {
        let opts = RecognizeOptions {
            lang,
            translate_to,
            provider: provider.map(str::to_owned),
            granularity: Some(granularity),
            ..Default::default()
        };
        self.recognize_with(image, &opts, &CancelToken::new())
    }

    /// Recognise with every option ([`RecognizeOptions`]). Blocking.
    pub fn recognize_with_options(&self, image: Vec<u8>, opts: &RecognizeOptions) -> Result<OcrOutcome> {
        self.recognize_with(image, opts, &CancelToken::new())
    }

    fn recognize_with(&self, image: Vec<u8>, opts: &RecognizeOptions, cancel: &CancelToken) -> Result<OcrOutcome> {
        let cfg = self.config();
        if let Some(dir) = &cfg.ocr.save_image_dir {
            let name = ocr_image_name(glossa_core::now_unix());
            if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join(name), &image)) {
                tracing::warn!("could not save the OCR image: {e}");
            }
        }
        let registry = self.registry();
        let http = self.inner.http.read().clone();
        let options_of = |p: &dyn Provider| {
            glossa_modules::provider::resolve_options(p.options(), cfg.services.options.get(&p.info().id))
        };
        let is_local = |p: &dyn Provider| p.is_local(&options_of(p));
        let offline_only = cfg.network.offline_only;
        let refused = |p: &dyn Provider| Error::OfflineOnly(format!("{} is an online service", p.info().name));
        let providers: Vec<Arc<dyn Provider>> = match opts.provider.as_deref() {
            Some(id) => {
                let p = registry.get(id).ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
                if !p.info().capabilities.contains(Capabilities::OCR) {
                    return Err(Error::Unsupported { provider: p.info().name.clone(), capability: "text recognition" });
                }
                if offline_only && !is_local(p.as_ref()) {
                    return Err(refused(p.as_ref()));
                }
                vec![p]
            }
            None => {
                let chosen =
                    registry.get(&cfg.ocr.provider).filter(|p| p.info().capabilities.contains(Capabilities::OCR));
                let mut v: Vec<Arc<dyn Provider>> = Vec::new();
                match &chosen {
                    Some(p) if offline_only && !is_local(p.as_ref()) => {
                        // The engine the user chose would send the picture
                        // out. Saying so beats quietly using another one.
                        return Err(refused(p.as_ref()));
                    }
                    Some(p) => v.push(p.clone()),
                    None => {}
                }
                // The picture goes to the chosen engine and, when that one
                // fails, only to engines that keep it on this computer. An
                // online engine is never a fallback: choosing Tesseract
                // must not end in an upload because Tesseract was missing.
                v.extend(registry.with_capability(Capabilities::OCR).into_iter().filter(|p| {
                    quiet_ocr_fallback(
                        &p.info().id,
                        &cfg.ocr.provider,
                        p.info().capabilities.contains(Capabilities::OFFLINE) || is_local(p.as_ref()),
                    )
                }));
                v
            }
        };
        if providers.is_empty() {
            return Err(if offline_only {
                Error::OfflineOnly("no text recognition engine on this computer is available".into())
            } else {
                Error::NoProvider("text recognition")
            });
        }
        let to = opts.translate_to.filter(|l| l.is_language());
        let mut req = OcrRequest::new(ImageSource::Bytes(image));
        req.lang = opts.lang;
        // The provider translates in the same request only when allowed to;
        // otherwise the current translator does it afterwards.
        let translate_with = opts.translate_with.unwrap_or(cfg.ocr.translate_with);
        req.translate_to = to.filter(|_| translate_with == OcrTranslateWith::OcrEngine);
        req.preserve_line_breaks = cfg.ocr.preserve_line_breaks;
        // An explicit granularity wins; in-place translation and joining
        // lines by paragraph need at least the blocks.
        req.granularity = opts.granularity.unwrap_or(if opts.in_place || opts.layout {
            OcrGranularity::Blocks
        } else {
            OcrGranularity::FullText
        });
        // The first engine's error is the one reported: it is the one the
        // user chose, and the fallbacks' complaints (not installed, not
        // configured) would only hide it.
        let mut first: Option<Error> = None;
        for provider in providers {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let opts_values = glossa_modules::provider::resolve_options(
                provider.options(),
                cfg.services.options.get(&provider.info().id),
            );
            let http = glossa_net::Cancellable::new(http.as_ref(), cancel.flag());
            let host = Host {
                http: &http,
                options: &opts_values,
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
                    if req.translate_to.is_none() {
                        // A provider that translated unasked is not what the
                        // user chose.
                        ocr.translation = None;
                        for b in &mut ocr.blocks {
                            b.translation = None;
                        }
                    }
                    let Some(to) = to else {
                        return Ok(OcrOutcome { ocr, translation: None, blocks: Vec::new() });
                    };
                    return Ok(if opts.in_place {
                        self.translate_in_place(ocr, to, cancel)
                    } else {
                        self.translate_recognised(ocr, to, cancel)
                    });
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    tracing::debug!(provider = %provider.info().id, error = %e, "OCR provider failed");
                    let go_on = ocr_falls_over(&e);
                    first.get_or_insert(e);
                    if !go_on {
                        break;
                    }
                }
            }
        }
        Err(first.unwrap_or(Error::NoProvider("text recognition")))
    }

    /// The translation of recognised text: the provider's own when it made
    /// one, else the pipeline's. A failed translation leaves the text alone.
    fn translate_recognised(&self, ocr: OcrResult, to: LangId, cancel: &CancelToken) -> OcrOutcome {
        if let Some(own) = provider_translation(&ocr, to) {
            return OcrOutcome { ocr, translation: Some(own), blocks: Vec::new() };
        }
        let from = ocr.detected_lang.filter(|l| l.is_language()).unwrap_or(LangId::AUTO);
        let translation = self
            .with_pipeline(cancel, |p| p.translate(&ocr.text, LangPair::new(from, to), &TranslateOptions::default()))
            .map_err(|e| tracing::debug!("translating the OCR text failed: {e}"))
            .ok();
        OcrOutcome { ocr, translation, blocks: Vec::new() }
    }

    /// Block-by-block translations: the provider's own (Lens), else each
    /// paragraph through the current translator.
    fn translate_in_place(&self, ocr: OcrResult, to: LangId, cancel: &CancelToken) -> OcrOutcome {
        let own = ocr::provider_blocks(&ocr);
        if !own.is_empty() {
            let translation = provider_translation(&ocr, to);
            return OcrOutcome { ocr, translation, blocks: own };
        }
        let paragraphs = ocr::paragraphs(&ocr);
        if paragraphs.is_empty() {
            // No geometry to paint on (an engine that returns text only).
            return self.translate_recognised(ocr, to, cancel);
        }
        let texts: Vec<String> = paragraphs.iter().map(|p| p.text.clone()).collect();
        let from = ocr.detected_lang.filter(|l| l.is_language()).unwrap_or(LangId::AUTO);
        match self.translate_texts(&texts, LangPair::new(from, to), cancel) {
            Ok((translated, outcome)) => {
                let blocks = paragraphs
                    .into_iter()
                    .zip(translated)
                    .map(|(p, t)| ocr::TranslatedBlock {
                        source: p.text,
                        translation: t,
                        bounds: p.bounds,
                        angle: p.angle,
                        lines: Vec::new(),
                    })
                    .collect();
                OcrOutcome { ocr, translation: Some(outcome), blocks }
            }
            Err(e) => {
                tracing::debug!("translating the OCR blocks failed: {e}");
                OcrOutcome { ocr, translation: None, blocks: Vec::new() }
            }
        }
    }

    /// Translate several texts with the current translator, keeping them
    /// apart: one request with the texts as separate paragraphs, or, when
    /// the translator merges them, one request per text. Blocking.
    pub fn translate_blocks(&self, texts: &[String], pair: LangPair) -> Result<Vec<String>> {
        self.translate_texts(texts, pair, &CancelToken::new()).map(|(t, _)| t)
    }

    /// [`Engine::translate_blocks`], also returning the whole translation as
    /// one outcome (the texts and their translations joined by blank lines),
    /// for the history and for showing it as text.
    fn translate_texts(
        &self,
        texts: &[String],
        pair: LangPair,
        cancel: &CancelToken,
    ) -> Result<(Vec<String>, TranslationOutcome)> {
        if texts.is_empty() {
            return Err(Error::EmptyInput);
        }
        let opts = TranslateOptions::default();
        let joined = ocr::batch(texts);
        let mut whole = self.with_pipeline(cancel, |p| p.translate(&joined, pair, &opts))?;
        if let Some(parts) = ocr::unbatch(&whole.translation.text, texts.len()) {
            return Ok((parts, whole));
        }
        tracing::debug!(blocks = texts.len(), "the translator merged the paragraphs; translating them one by one");
        // One by one, in the direction the batch settled on, so each short
        // text is not detected again on its own.
        let pair = LangPair::new(whole.pair.from, whole.pair.to);
        let mut parts = Vec::with_capacity(texts.len());
        for t in texts {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let out = self.with_pipeline(cancel, |p| p.translate(t, pair, &opts))?;
            parts.push(out.translation.text);
        }
        whole.translation.text = parts.join("\n\n");
        Ok((parts, whole))
    }

    /// Recognise on a worker; the result arrives as [`Event::Recognized`].
    /// Results in [`Slot::Ocr`] that carry a translation go into the
    /// history.
    pub fn recognize_async(&self, slot: Slot, image: Vec<u8>, lang: LangId, translate_to: Option<LangId>) -> u64 {
        self.recognize_async_with(slot, image, RecognizeOptions { lang, translate_to, ..Default::default() })
    }

    /// [`Engine::recognize_async`] with every option.
    pub fn recognize_async_with(&self, slot: Slot, image: Vec<u8>, opts: RecognizeOptions) -> u64 {
        let (job, cancel) = self.begin(slot);
        let me = self.clone();
        self.spawn_or(
            "ocr",
            move || {
                let result = catch_panic("text recognition", || me.recognize_with(image, &opts, &cancel));
                if cancel.is_cancelled() || !me.current(slot, job) {
                    return;
                }
                if let (Ok(out), Slot::Ocr) = (&result, slot) {
                    if let Some(t) = &out.translation {
                        me.record_history(t);
                    }
                }
                me.emit(Event::Recognized { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
            },
            |e| self.emit(Event::Recognized { slot, job, result: Err(Arc::new(e)) }),
        );
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
        let is_local = |p: &dyn Provider| {
            p.is_local(&glossa_modules::provider::resolve_options(p.options(), cfg.services.options.get(&p.info().id)))
        };
        let offline_only = cfg.network.offline_only;
        // The chosen voice, then only voices on this computer: text meant
        // for the system voice is not sent to an online one because the
        // system voice lacked the language.
        let mut providers: Vec<Arc<dyn Provider>> = registry.get(&cfg.services.tts).into_iter().collect();
        providers.extend(
            registry
                .with_capability(Capabilities::LISTEN)
                .into_iter()
                .filter(|p| p.info().id != cfg.services.tts && is_local(p.as_ref())),
        );
        if offline_only {
            providers.retain(|p| is_local(p.as_ref()));
        }
        let req = SpeakRequest { text: text.to_owned(), lang, slow };
        let mut last = if offline_only {
            Error::OfflineOnly("no voice on this computer is available".into())
        } else {
            Error::NoProvider("speech")
        };
        for provider in providers.into_iter().filter(|p| p.info().capabilities.contains(Capabilities::LISTEN)) {
            let opts = glossa_modules::provider::resolve_options(
                provider.options(),
                cfg.services.options.get(&provider.info().id),
            );
            let http = glossa_net::Cancellable::new(http.as_ref(), cancel.flag());
            let host = Host {
                http: &http,
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
        self.spawn_or(
            "speech",
            move || {
                let result = catch_panic("speech", || me.speak_with(&text, lang, slow, &cancel));
                if cancel.is_cancelled() || !me.current(slot, job) {
                    return;
                }
                me.emit(Event::Spoken { slot, job, result: result.map(Arc::new).map_err(Arc::new) });
            },
            |e| self.emit(Event::Spoken { slot, job, result: Err(Arc::new(e)) }),
        );
        job
    }

    // -- history -------------------------------------------------------------------

    /// Add a translation made outside the windows to the history, as the
    /// desktop program does for its own (the command line's
    /// `--save-history`, the control API's `"history": true`). Does nothing
    /// when the history is switched off. Returns whether it was recorded.
    pub fn record_translation(&self, out: &TranslationOutcome) -> bool {
        self.record_history(out)
    }

    /// Write the history to `history.json` now. A worker does this a few
    /// seconds after every change ([`Engine::history_touched`]) and
    /// [`Engine::shutdown`] does it at the end; a front end that is about
    /// to exit calls it itself (`glossa-cli`). Nothing is written when the
    /// history is off, or when the file could not be read at start and
    /// could not be copied aside.
    ///
    /// The file is shared with other programs (the desktop program and
    /// `glossa-cli translate --save-history`): what they wrote since this
    /// one last looked is merged in rather than written over (see
    /// [`History::save_merged`]). The writing happens on a copy, so a
    /// window reading the history never waits for the disk.
    pub fn save_history(&self) -> Result<()> {
        if !self.config().history.enabled || !self.inner.history_writable.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _one_writer = self.inner.history_io.lock();
        self.inner.saver.lock().history = false;
        let mut copy = self.inner.history.lock().clone();
        copy.save_merged(&self.inner.paths.history_file())?;
        if self.inner.history.lock().adopt_saved(copy) {
            self.emit(Event::HistoryChanged);
        }
        Ok(())
    }

    /// Write the dictionary's recent words now.
    fn save_dictionary_history(&self) -> Result<()> {
        self.inner.saver.lock().dictionary = false;
        let words = self.dictionary_history();
        let json = serde_json::to_vec(&words)?;
        glossa_core::fsio::write_atomic(&self.inner.paths.dictionary_history_file(), &json)?;
        Ok(())
    }

    /// The history (or the dictionary's recent words) changed: have a
    /// worker write it in a few seconds, so that a crash, a kill or a power
    /// cut loses those seconds and not the whole session. Changes in the
    /// meantime ride along with the same write. Never on the caller's
    /// thread, which may be drawing.
    fn history_touched(&self, dictionary: bool) {
        let mut saver = self.inner.saver.lock();
        if dictionary {
            saver.dictionary = true;
        } else {
            saver.history = true;
        }
        if saver.scheduled {
            return;
        }
        // The worker holds the engine only while it writes: an engine
        // nobody else holds any more is not kept alive (or written for).
        let weak = Arc::downgrade(&self.inner);
        let delay = *self.inner.save_delay.lock();
        let spawned = std::thread::Builder::new().name("glossa-history-save".into()).spawn(move || loop {
            std::thread::sleep(delay);
            let Some(inner) = weak.upgrade() else { return };
            let engine = Engine { inner };
            let (history, dictionary) = {
                let mut saver = engine.inner.saver.lock();
                if !saver.history && !saver.dictionary {
                    saver.scheduled = false;
                    return;
                }
                (saver.history, saver.dictionary)
            };
            engine.save_touched(history, dictionary);
        });
        saver.scheduled = spawned.is_ok();
    }

    fn save_touched(&self, history: bool, dictionary: bool) {
        // With "clear the history on exit" the session's history is not
        // meant to outlive the session, so it is not put on disk during it
        // (a kill would leave it there); `shutdown` writes what remains.
        if history && !self.config().history.clear_on_exit {
            if let Err(e) = self.save_history() {
                tracing::warn!("history not saved: {e}");
            }
        } else {
            self.inner.saver.lock().history = false;
        }
        if dictionary {
            if let Err(e) = self.save_dictionary_history() {
                tracing::warn!("dictionary history not saved: {e}");
            }
        }
    }

    /// Write whatever of the histories is waiting to be written, now: for
    /// a front end that has been told the session is ending (a signal, the
    /// system shutting down) and may not get to [`Engine::shutdown`].
    pub fn flush_history(&self) {
        let (history, dictionary) = {
            let saver = self.inner.saver.lock();
            (saver.history, saver.dictionary)
        };
        self.save_touched(history, dictionary);
    }

    fn record_history(&self, out: &TranslationOutcome) -> bool {
        if !self.inner.config.read().history.enabled {
            return false;
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
            self.history_touched(false);
            self.emit(Event::HistoryChanged);
        }
        id != 0
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        self.inner.history.lock().iter().cloned().collect()
    }

    /// The newest `n` entries, newest first, without copying the rest.
    pub fn recent_history(&self, n: usize) -> Vec<HistoryEntry> {
        self.inner.history.lock().iter().take(n).cloned().collect()
    }

    pub fn search_history(&self, needle: &str, favorites_only: bool) -> Vec<HistoryEntry> {
        self.inner.history.lock().search(needle, favorites_only).into_iter().cloned().collect()
    }

    pub fn set_favorite(&self, id: u64, favorite: bool) {
        if self.inner.history.lock().set_favorite(id, favorite) {
            self.history_touched(false);
            self.emit(Event::HistoryChanged);
        }
    }

    pub fn remove_history(&self, id: u64) {
        if self.inner.history.lock().remove(id) {
            self.history_touched(false);
            self.emit(Event::HistoryChanged);
        }
    }

    /// Put back an entry [`Engine::remove_history`] deleted, for an undo.
    /// False when it could not go back (the same text was translated again
    /// in the meantime).
    pub fn restore_history(&self, entry: HistoryEntry) -> bool {
        let restored = self.inner.history.lock().restore(entry);
        if restored {
            self.emit(Event::HistoryChanged);
        }
        restored
    }

    pub fn clear_history(&self, including_favorites: bool) {
        self.inner.history.lock().clear(including_favorites);
        self.history_touched(false);
        self.emit(Event::HistoryChanged);
    }

    pub fn export_history(&self, path: &Path, format: ExportFormat, favorites_only: bool) -> Result<usize> {
        let history = self.inner.history.lock();
        let entries = history.search("", favorites_only);
        std::fs::write(path, export::render(&entries, format))?;
        Ok(entries.len())
    }

    fn remember_lookup(&self, word: &str) {
        {
            let mut h = self.inner.dictionary_history.lock();
            h.retain(|w| w != word);
            h.insert(0, word.to_owned());
            h.truncate(200);
        }
        self.history_touched(true);
    }

    pub fn dictionary_history(&self) -> Vec<String> {
        self.inner.dictionary_history.lock().clone()
    }

    pub fn clear_dictionary_history(&self) {
        self.inner.dictionary_history.lock().clear();
        self.history_touched(true);
    }

    /// Put the looked-up words back as they were before
    /// [`Engine::clear_dictionary_history`] (an undone clear), keeping
    /// whatever was looked up since at the top.
    pub fn restore_dictionary_history(&self, words: Vec<String>) {
        let mut h = self.inner.dictionary_history.lock();
        for w in words {
            if !h.contains(&w) {
                h.push(w);
            }
        }
        h.truncate(200);
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
            self.history_touched(false);
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
            // What another program added meanwhile comes in first, so that
            // clearing clears it too rather than leaving it for next time.
            if let Err(e) = self.save_history() {
                tracing::warn!("history not saved: {e}");
            }
            self.inner.history.lock().clear(false);
        }
        if let Err(e) = self.save_history() {
            tracing::warn!("history not saved: {e}");
        }
        if let Err(e) = self.save_dictionary_history() {
            tracing::warn!("dictionary history not saved: {e}");
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

/// Give the control API a token if it has none. The token is always
/// required, so an empty one (a first run, a cleared field, a hand edit)
/// is replaced rather than switching the check off. Returns whether it was.
fn ensure_token(config: &mut Config) -> bool {
    if !config.server.token.trim().is_empty() {
        return false;
    }
    config.server.token = random_token();
    true
}

/// Read `history.json`, keeping a copy of a file that cannot be parsed.
/// The flag says whether saving over the file later is safe.
fn load_history(path: &Path, max_entries: usize) -> (History, bool) {
    match History::load_recovering(path, max_entries) {
        Ok((history, None)) => (history, true),
        Ok((history, Some(copy))) => {
            tracing::warn!(
                "{} could not be read; it was copied to {} and a new history started",
                path.display(),
                copy.display()
            );
            (history, true)
        }
        Err(e) => {
            tracing::warn!("{e}; the history will not be saved this session");
            (History::new(max_entries), false)
        }
    }
}

/// A random token for the control API: 256 bits from the operating
/// system's random source, in hex. Anything that can guess it can drive
/// Glossa, so nothing weaker (the clock, the process id, a hash seed) goes
/// into it.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How many unusable values in the settings file are each named at start;
/// the rest are counted.
const MAX_VALUE_WARNINGS: usize = 5;

/// The settings file as a front end needs it before (or without) starting
/// an engine: to find a running Glossa's control API, say. Read like
/// [`Config::load`], with the proxy password, the Lens token and the
/// control API's token fetched from `store` where the file keeps them
/// there. One the keychain does not give reads as empty.
pub fn stored_config(paths: &Paths, store: Option<&dyn SecretStore>) -> Result<Config> {
    let mut config = Config::load(&paths.config_file())?;
    for problem in SecretSync::new().resolve(&mut config, &SecretSlot::fixed(), store) {
        tracing::warn!("keychain: {problem}");
    }
    Ok(config)
}

/// Whether OCR should try the next engine after `e`. Unlike translation, an
/// engine that is not installed or not set up (Lens without chrome-lens-py,
/// Tesseract missing, an empty command) does not stop the search: the
/// default engine needs a separate install, and without it recognition
/// should still reach the system's offline one.
fn ocr_falls_over(e: &Error) -> bool {
    e.should_try_next_provider() || e.is_configuration_fault()
}

/// Whether the engine `id` may be tried when the chosen one (`chosen`)
/// fails. Only an engine that keeps the picture on this computer (`local`):
/// a screenshot is never uploaded to a service the user did not choose,
/// whatever went wrong with the one they did. OneOCR copies about 55 MB of
/// the Snipping Tool's libraries on first use, and an AI endpoint, local or
/// not, was chosen for translation: both run only when chosen.
fn quiet_ocr_fallback(id: &str, chosen: &str, local: bool) -> bool {
    local && id != chosen && id != glossa_ocr::oneocr::ONEOCR_ID && !ai::is_endpoint(id)
}

/// The file name of a saved OCR picture: the time, and a number that makes
/// two pictures taken in the same second (a watched region, an API client
/// in a loop) two files.
fn ocr_image_name(unix_secs: i64) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("glossa-{unix_secs}-{millis:03}-{n}.png")
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::secrets::IN_STORE;

    thread_local! {
        /// Makes [`Engine::start_thread`] fail on this test's thread.
        pub(super) static THREADS_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn pictures_go_to_an_ai_endpoint_or_oneocr_only_when_chosen() {
        assert!(quiet_ocr_fallback("tesseract", "lens", true));
        assert!(!quiet_ocr_fallback("lens", "lens", true));
        assert!(!quiet_ocr_fallback(ai::BUILTIN_ID, "lens", true));
        assert!(!quiet_ocr_fallback("ai-local-vllm", "lens", true));
        assert!(!quiet_ocr_fallback(glossa_ocr::oneocr::ONEOCR_ID, "lens", true));
        // An engine that sends the picture out is never a fallback.
        assert!(!quiet_ocr_fallback("lens", "tesseract", false));
        assert!(!quiet_ocr_fallback("ocr-space", "lens", false));
    }

    #[test]
    fn saved_ocr_pictures_taken_in_one_second_get_different_names() {
        let names: std::collections::HashSet<String> = (0..50).map(|_| ocr_image_name(1_700_000_000)).collect();
        assert_eq!(names.len(), 50);
        assert!(names.iter().all(|n| n.starts_with("glossa-1700000000-") && n.ends_with(".png")));
    }

    /// The defect: an offline engine was chosen, it failed, and the picture
    /// went to Google Lens. The picture now stays here: the chosen engine's
    /// error is reported and nothing is sent.
    #[cfg(unix)]
    #[test]
    fn a_failing_offline_engine_never_falls_back_to_an_online_one() {
        let (e, dir) = engine("ocr-no-upload");
        e.update_config(|c| {
            c.ocr.provider = "tesseract".into();
            c.ocr.tesseract.path = dir.join("no-tesseract").to_string_lossy().into_owned();
            // Lens through its command line: were it tried, the error would
            // name this file.
            c.ocr.lens.mode = glossa_core::config::LensMode::Cli;
            c.ocr.lens.cli_path = dir.join("lens-was-called").to_string_lossy().into_owned();
        })
        .unwrap();
        let registry = e.registry();
        let lens = registry.get("lens").unwrap();
        assert!(!lens.is_local(&Default::default()), "Lens is online");
        assert!(registry.get("tesseract").unwrap().is_local(&Default::default()));
        let command = registry.get(glossa_ocr::command::COMMAND_OCR_ID).unwrap();
        assert!(!command.is_local(&Default::default()), "a command is not known to stay offline");
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let err = e.recognize(png, LangId::ENGLISH, None).unwrap_err();
        let said = err.to_string();
        assert!(said.contains("no-tesseract") && !said.contains("lens-was-called"), "{err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn offline_only_refuses_online_ocr_voices_and_translators_with_a_clear_error() {
        let (e, dir) = engine("offline-only");
        e.update_config(|c| {
            c.network.offline_only = true;
            c.ocr.provider = "lens".into();
            c.services.tts = "google".into();
            c.services.enabled = vec!["google".into(), "bing".into()];
            c.services.active = "google".into();
        })
        .unwrap();
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let err = e.recognize(png.clone(), LangId::ENGLISH, None).unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(ref m) if m.contains("Lens")), "{err:?}");
        let err = e.recognize_using("lens", png, LangId::ENGLISH, None).unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");
        let err = e
            .translate("hello", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions::default())
            .unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");
        assert!(err.to_string().contains("offline only"), "{err}");
        let err = e.dictionary("hello", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), Some("google-dictionary"));
        assert!(matches!(err, Err(Error::OfflineOnly(_))), "{err:?}");
        let err = e.test_provider("google", None).unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");
        // The engine's own client refuses too, for anything that gets past
        // the checks above (a module, a download).
        let http = e.inner.http.read().clone();
        let err =
            glossa_net::Http::send(http.as_ref(), glossa_net::HttpRequest::get("https://example.com/")).unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");

        // An AI endpoint on this computer is local; the same one pointed
        // elsewhere is not.
        let id = e.add_ai_endpoint("Mine", "ollama").unwrap();
        let endpoint = e.registry().get(&id).unwrap();
        let options =
            |url: &str| -> OptionValues { [("base_url".to_owned(), serde_json::json!(url))].into_iter().collect() };
        assert!(endpoint.is_local(&options("http://localhost:11434/v1")));
        assert!(endpoint.is_local(&options("http://127.0.0.1:1234/v1/chat/completions")));
        assert!(!endpoint.is_local(&options("https://api.openai.com/v1")));
        assert!(!endpoint.is_local(&options("http://localhost.evil.example/v1")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `glossa-cli --offline`: in force for the run, absent from the file,
    /// and still in force after the settings are changed and saved.
    #[test]
    fn session_overrides_apply_without_being_saved() {
        let (e, dir) = engine("session");
        e.update_config(|c| {
            c.services.enabled = vec!["google".into()];
            c.services.active = "google".into();
        })
        .unwrap();
        e.set_session_overrides(SessionOverrides {
            offline_only: true,
            detect: Some(glossa_core::config::DetectMode::Offline),
        })
        .unwrap();
        assert!(e.config().network.offline_only);
        let translate =
            || e.translate("hello", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions::default());
        assert!(matches!(translate(), Err(Error::OfflineOnly(_))));
        e.update_config(|c| c.network.timeout_secs = 7).unwrap();
        assert!(matches!(translate(), Err(Error::OfflineOnly(_))), "a rebuilt client is still offline");
        let http = e.inner.http.read().clone();
        let sent = glossa_net::Http::send(http.as_ref(), glossa_net::HttpRequest::get("https://example.com/"));
        assert!(matches!(sent, Err(Error::OfflineOnly(_))), "{sent:?}");
        e.shutdown();
        let saved = Config::load(&e.paths().config_file()).unwrap();
        assert!(!saved.network.offline_only, "the override is not written");
        assert_eq!(saved.general.detect, glossa_core::config::DetectMode::OfflineThenService);
        assert_eq!(saved.network.timeout_secs, 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The defect: when no worker thread could be started nothing was
    /// emitted, and the window that asked waited for ever.
    #[test]
    fn a_job_whose_thread_cannot_start_ends_in_an_error_event() {
        let (e, dir) = engine("no-threads");
        let events = e.subscribe();
        THREADS_FAIL.with(|f| f.set(true));
        let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
        let t = e.translate_async(Slot::Main, "hello".into(), pair, TranslateOptions::default());
        let d = e.dictionary_async(Slot::Dictionary, "hello".into(), pair, None);
        let o = e.recognize_async(Slot::Ocr, vec![1], LangId::ENGLISH, None);
        let s = e.speak_async(Slot::Speech, "hello".into(), LangId::ENGLISH, None);
        e.apply_preset_async("Private".into());
        THREADS_FAIL.with(|f| f.set(false));
        let mut ended = Vec::new();
        let mut preset = false;
        for ev in events.try_iter() {
            match ev {
                Event::Translated { job, result: Err(err), .. } if matches!(*err, Error::Internal(_)) => {
                    ended.push(job)
                }
                Event::Dictionary { job, query, result: Err(_), .. } => {
                    assert_eq!(query, "hello");
                    ended.push(job);
                }
                Event::Recognized { job, result: Err(_), .. } | Event::Spoken { job, result: Err(_), .. } => {
                    ended.push(job)
                }
                Event::PresetApplied { result: Err(_) } => preset = true,
                _ => {}
            }
        }
        assert_eq!(ended, vec![t, d, o, s], "every job reported its failure");
        assert!(preset);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ocr_falls_over_past_engines_that_are_not_installed() {
        assert!(ocr_falls_over(&Error::Config("chrome-lens-py was not found".into())));
        assert!(ocr_falls_over(&Error::FileNotFound("lens_scan".into())));
        assert!(ocr_falls_over(&Error::MissingCredential { provider: "x".into(), what: "command".into() }));
        assert!(ocr_falls_over(&Error::Network("down".into())));
        assert!(!ocr_falls_over(&Error::Cancelled));
        assert!(!ocr_falls_over(&Error::EmptyInput));
    }

    /// Lens is not installed, so a picture reaches the next engine
    /// (Tesseract, here a script printing its TSV); when nothing works, the
    /// chosen engine's error is the one shown.
    #[cfg(unix)]
    #[test]
    fn recognition_reaches_an_offline_engine_when_lens_is_missing() {
        use std::os::unix::fs::PermissionsExt;
        let (e, dir) = engine("ocr-fallover");
        let fake = dir.join("tesseract");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf 'level\\tpage_num\\tblock_num\\tpar_num\\tline_num\\tword_num\\tleft\\ttop\\twidth\\theight\\tconf\\ttext\\n\
             1\\t1\\t0\\t0\\t0\\t0\\t0\\t0\\t100\\t30\\t-1\\t\\n5\\t1\\t1\\t1\\t1\\t1\\t2\\t2\\t40\\t20\\t91\\tOffline\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        e.update_config(|c| {
            c.ocr.provider = "lens".into();
            c.ocr.lens.mode = glossa_core::config::LensMode::Cli;
            c.ocr.lens.cli_path = dir.join("no-lens_scan").to_string_lossy().into_owned();
            c.ocr.tesseract.path = fake.to_string_lossy().into_owned();
        })
        .unwrap();
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let out = e.recognize(png.clone(), LangId::ENGLISH, None).unwrap();
        assert_eq!(out.ocr.text, "Offline");
        assert_eq!(out.ocr.provider_id, "tesseract");

        e.update_config(|c| c.ocr.tesseract.path = dir.join("no-tesseract").to_string_lossy().into_owned()).unwrap();
        let err = e.recognize(png, LangId::ENGLISH, None).unwrap_err();
        assert!(matches!(err, Error::FileNotFound(ref p) if p.contains("no-lens_scan")), "{err:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The front end rebuilds its menus when the locale is a new one, so a
    /// theme change or a rescan that finds nothing new must keep it.
    #[test]
    fn the_locale_is_replaced_only_when_the_language_or_its_files_change() {
        let (e, dir) = engine("locale-stamp");
        e.update_config(|c| c.general.ui_language = "en".into()).unwrap();
        let en = e.locale();
        e.update_config(|c| c.appearance.theme = "Glossa Light".into()).unwrap();
        e.refresh_catalogs();
        assert!(Arc::ptr_eq(&en, &e.locale()), "nothing about the language changed");
        e.update_config(|c| c.general.ui_language = "ru".into()).unwrap();
        let ru = e.locale();
        assert_eq!(ru.code, "ru");
        let user = e.paths().user_locale_dir();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("ru.json"), r#"{ "code": "ru", "strings": { "main.translate": "Пере" } }"#).unwrap();
        e.refresh_catalogs();
        assert_eq!(e.locale().get("main.translate"), "Пере", "a new file for the language loads");
        std::fs::remove_dir_all(&dir).ok();
    }

    pub(crate) fn engine(name: &str) -> (Engine, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-engine-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.modules.allow_legacy_scripts = true;
        (Engine::with_config(paths, cfg).unwrap(), dir)
    }

    #[derive(Default)]
    struct MemoryKeychain(Mutex<HashMap<String, String>>);

    impl SecretStore for MemoryKeychain {
        fn name(&self) -> &'static str {
            "memory"
        }
        fn get(&self, account: &str) -> std::result::Result<Option<String>, String> {
            Ok(self.0.lock().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> std::result::Result<(), String> {
            self.0.lock().insert(account.into(), secret.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> std::result::Result<(), String> {
            self.0.lock().remove(account);
            Ok(())
        }
    }

    #[test]
    fn keys_kept_in_the_keychain_survive_a_restart_and_stay_out_of_the_file() {
        let (e, dir) = engine("keychain");
        let keychain: Arc<dyn SecretStore> = Arc::new(MemoryKeychain::default());
        e.attach_secret_store(Some(keychain.clone()));
        e.update_config(|c| {
            c.general.secret_storage = SecretStorage::System;
            c.services.set_option("deepl", "api_key", serde_json::json!("dk-secret"));
            c.network.proxy.password = "hunter2".into();
        })
        .unwrap();
        // The engine itself still sees the key.
        assert_eq!(e.config().services.option("deepl", "api_key"), Some(&serde_json::json!("dk-secret")));
        let file = std::fs::read_to_string(e.paths().config_file()).unwrap();
        assert!(!file.contains("dk-secret") && !file.contains("hunter2"), "{file}");
        assert!(file.contains(IN_STORE));
        drop(e);

        let again = Engine::start(Paths::for_exe_dir(dir.clone(), Some(dir.join("home")))).unwrap();
        again.attach_secret_store(Some(keychain));
        assert_eq!(again.config().services.option("deepl", "api_key"), Some(&serde_json::json!("dk-secret")));
        assert_eq!(again.config().network.proxy.password, "hunter2");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_theme_in_effect_follows_the_system_mode() {
        let (e, dir) = engine("system-mode");
        e.update_config(|c| {
            c.appearance.follow_system_theme = true;
            c.appearance.theme_light = "Glossa Light".into();
            c.appearance.theme_dark = "Glossa Midnight".into();
        })
        .unwrap();
        let events = e.subscribe();
        e.set_system_dark(Some(true));
        assert_eq!(e.current_theme_name(), "Glossa Midnight");
        assert!(events.try_iter().any(|ev| matches!(ev, Event::ConfigChanged)), "the windows are told");
        e.set_system_dark(Some(false));
        assert_eq!(e.current_theme_name(), "Glossa Light");
        // The same mode again changes nothing and says nothing.
        let _ = events.try_iter().count();
        e.set_system_dark(Some(false));
        assert_eq!(events.try_iter().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_keychain_where_one_is_wanted_is_said_once_at_start() {
        let (e, dir) = engine("no-keychain");
        e.update_config(|c| c.general.secret_storage = SecretStorage::System).unwrap();
        e.attach_secret_store(None);
        assert!(e.startup_warnings().iter().any(|w| w.key == "startup.no_keychain"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_config_does_not_stop_the_start() {
        let dir = std::env::temp_dir().join(format!("glossa-engine-{}-broken", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(paths.config_file(), "[general\nthis is not toml").unwrap();
        let e = Engine::start(paths.clone()).expect("starts with the defaults");
        let warnings = e.startup_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "startup.config_broken");
        let backup = PathBuf::from(&warnings[0].args[0]);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "[general\nthis is not toml");
        let text = warnings[0].message(&glossa_core::Locale::english());
        assert!(text.contains(&warnings[0].args[0]) && !text.contains("%s"), "{text}");
        // The new token was saved to a fresh, valid file.
        assert!(Config::load(&paths.config_file()).is_ok());
        assert!(Engine::start(paths).unwrap().startup_warnings().is_empty(), "only once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn starts_with_the_builtin_providers() {
        let (e, dir) = engine("start");
        let translators = e.providers(Capabilities::TRANSLATE);
        assert!(translators.iter().any(|p| p.info.id == "google"));
        assert!(translators.iter().all(|p| p.origin == "Built-in" || p.origin == "Bundled" || p.dir.is_some()));
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
    fn services_are_adopted_as_they_appear_not_hard_coded() {
        let (e, dir) = engine("adopt");
        let s = e.config().services;
        // A fresh install switches on what works without setup, nothing else:
        // the keyless built-ins, then the bundled modules.
        assert_eq!(s.enabled, vec!["google", "bing", "mymemory", "yandex-web", "deepl-web", "lara", "modernmt"]);
        assert_eq!(s.active, "google");
        assert_eq!(
            s.dictionary_enabled,
            vec![
                "google-dictionary",
                "yandex-web",
                "multitran",
                "lingvo-live",
                "oxford-learners",
                "wiktionary",
                "youdao",
                "dic-academic",
                "translate-academic",
                "wikipedia",
                "urban-dictionary"
            ]
        );
        assert!(!s.enabled.contains(&"yandex".to_string()), "the keyed Yandex needs setup");
        assert_eq!(s.tts, "google");
        assert!(s.seen.contains(&"deepl".to_string()), "listed, not enabled: it needs a key");
        assert!(!s.enabled.contains(&"deepl".to_string()));

        // The user's choice sticks across reloads.
        e.update_config(|c| c.services.enabled.retain(|id| id != "bing")).unwrap();
        e.reload_modules();
        assert!(!e.config().services.enabled.contains(&"bing".to_string()));

        // A module installed later that works without setup is switched on.
        let m = e.paths().user_module_dir().join("ready");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"ready\"\nname = \"Ready\"\nruntime = \"js\"\nready_to_use = true\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return req.text; }").unwrap();
        e.reload_modules();
        e.approve_module("ready", None).unwrap();
        let s = e.config().services;
        assert!(s.enabled.contains(&"ready".to_string()), "{:?}", s.enabled);
        assert!(Config::load(&e.paths().config_file()).unwrap().services.enabled.contains(&"ready".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundled_modules_are_listed_and_a_user_module_replaces_one() {
        let (e, dir) = engine("bundled");
        let yandex = e.providers(Capabilities::TRANSLATE).into_iter().find(|p| p.info.id == "yandex-web").unwrap();
        assert_eq!(yandex.origin, "Bundled");
        assert_eq!(yandex.info.name, "Yandex");
        assert!(yandex.dir.is_none(), "nothing on disk");

        // Switched off by the user: not loaded at all.
        e.update_config(|c| c.modules.disabled.push("lara".into())).unwrap();
        e.reload_modules();
        assert!(e.registry().get("lara").is_none());

        // The same id in the user's modules folder wins.
        let m = e.paths().user_module_dir().join("my-yandex");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"yandex-web\"\nname = \"My Yandex\"\nruntime = \"js\"\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return 'mine: ' + req.text; }").unwrap();
        e.reload_modules();
        e.approve_module("yandex-web", None).unwrap();
        let p = e.providers(Capabilities::TRANSLATE).into_iter().find(|p| p.info.id == "yandex-web").unwrap();
        assert_eq!(p.info.name, "My Yandex");
        assert_eq!(p.origin, "JavaScript");
        let out = e
            .translate(
                "hi",
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
                &TranslateOptions { provider: Some("yandex-web".into()), fail_over: Some(false), ..Default::default() },
            )
            .unwrap();
        assert_eq!(out.translation.text, "mine: hi");
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
    fn blocks_are_translated_in_one_request_or_one_by_one() {
        let (e, dir) = engine("blocks");
        let classic = dir.join("QTranslate");
        std::fs::create_dir_all(classic.join("Data")).unwrap();
        std::fs::write(classic.join("Data").join("Options.json"), "{}").unwrap();
        // One translator keeps the paragraphs apart, the other runs them
        // together (as some do with short texts).
        for (name, body) in [("Keeps", "a.toUpperCase()"), ("Merges", "a.replace(/\\s+/g, ' ').toUpperCase()")] {
            let folder = classic.join("Services").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join("Service.js"),
                format!(
                    r#"function serviceHeader(){{return new ServiceHeader(900,"{name}","",Capability.TRANSLATE)}}
                       function serviceHost(){{return "https://tr.test"}}
                       SupportedLanguages=[-1,"auto"]; SupportedLanguages[17]="en"; SupportedLanguages[46]="ru";
                       function serviceTranslateRequest(a,b,c){{return new RequestData(HttpMethod.UNDEFINED)}}
                       function serviceTranslateResponse(a,b,c,d){{return new ResponseData({body},c,d)}}"#
                ),
            )
            .unwrap();
        }
        e.import_classic(&classic).unwrap();
        let texts = vec!["open the\nsettings".to_owned(), "press enter".to_owned()];
        let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
        for id in ["qt-keeps", "qt-merges"] {
            e.update_config(|c| {
                c.services.active = id.into();
                c.services.fail_over = false;
            })
            .unwrap();
            let out = e.translate_blocks(&texts, pair).unwrap();
            assert_eq!(out, ["OPEN THE SETTINGS", "PRESS ENTER"], "{id}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_provider_translation_counts_as_the_providers_own() {
        let ocr = OcrResult {
            text: "Hello".into(),
            translation: Some("Привет".into()),
            detected_lang: Some(LangId::ENGLISH),
            provider_id: "lens".into(),
            ..Default::default()
        };
        let t = provider_translation(&ocr, LangId::RUSSIAN).unwrap();
        assert_eq!(t.translation.provider_id, "lens", "shown and recorded under Lens");
        assert_eq!(t.translation.text, "Привет");
        assert_eq!(t.pair, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
        assert_eq!(t.source, "Hello");
        let blank = OcrResult { translation: Some(" ".into()), ..ocr };
        assert!(provider_translation(&blank, LangId::RUSSIAN).is_none());
    }

    #[test]
    fn dictionaries_respect_fail_over_and_name_who_answered() {
        let (e, dir) = engine("dict-failover");
        let classic = dir.join("QTranslate");
        std::fs::create_dir_all(classic.join("Data")).unwrap();
        std::fs::write(classic.join("Data").join("Options.json"), "{}").unwrap();
        // Two dictionaries that need no network: one never finds anything,
        // the other always answers.
        for (name, body) in [("Empty", "\"\""), ("Answers", "'<b>' + a + '</b>'")] {
            let folder = classic.join("Services").join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join("Service.js"),
                format!(
                    r#"function serviceHeader(){{return new ServiceHeader(900,"{name}","",Capability.DICTIONARY)}}
                       function serviceHost(){{return "https://dict.test"}}
                       SupportedLanguages=[-1,"auto"]; SupportedLanguages[17]="en"; SupportedLanguages[46]="ru";
                       function serviceDictionaryRequest(a,b,c){{return new RequestData(HttpMethod.UNDEFINED)}}
                       function serviceDictionaryResponse(a,b,c,d){{return new ResponseData({body},c,d)}}"#
                ),
            )
            .unwrap();
        }
        e.import_classic(&classic).unwrap();
        e.update_config(|c| {
            c.services.dictionary_enabled = vec!["qt-answers".into()];
            c.services.fail_over = false;
        })
        .unwrap();
        let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);

        let err = e.dictionary("house", pair, Some("qt-empty")).unwrap_err();
        assert!(err.to_string().contains("house"), "the asked-for dictionary's own failure: {err}");

        e.update_config(|c| c.services.fail_over = true).unwrap();
        let a = e.dictionary("house", pair, Some("qt-empty")).unwrap();
        assert_eq!(a.provider_id, "qt-answers", "the article says which dictionary produced it");
        assert_eq!(a.html.as_deref(), Some("<b>house</b>"));
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
            truncated: false,
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
    fn a_cleared_token_is_replaced_and_saved_at_once() {
        let (e, dir) = engine("token-clear");
        let first = e.config().server.token;
        e.update_config(|c| c.server.token.clear()).unwrap();
        let second = e.config().server.token;
        assert_eq!(second.len(), 64, "never empty");
        assert_ne!(second, first);
        assert_eq!(Config::load(&e.paths().config_file()).unwrap().server.token, second, "saved");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_history_is_kept_aside_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("glossa-engine-{}-history-corrupt", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let broken = b"{\"entries\": [ {\"id\": 1, \"source\": \"precious\"";
        std::fs::write(paths.history_file(), broken).unwrap();
        let e = Engine::with_config(paths.clone(), Config::default()).unwrap();
        assert!(e.history().is_empty());
        let copies: Vec<_> = std::fs::read_dir(&paths.data_dir)
            .unwrap()
            .filter_map(|f| f.ok())
            .filter(|f| f.file_name().to_string_lossy().starts_with("history.json.corrupt-"))
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(std::fs::read(copies[0].path()).unwrap(), broken);
        e.shutdown();
        assert_eq!(std::fs::read(copies[0].path()).unwrap(), broken, "the copy survives the next save");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn switching_the_history_on_reads_the_file_before_anything_saves() {
        let (e, dir) = engine("history-on");
        let mut saved = History::new(100);
        saved.record(
            "kept",
            HistoryResult { provider_id: "g".into(), from: LangId::ENGLISH, to: LangId::RUSSIAN, text: "x".into() },
            1,
        );
        saved.save(&e.paths().history_file()).unwrap();
        let paths = e.paths().clone();
        drop(e);
        let mut cfg = Config::load(&paths.config_file()).unwrap();
        cfg.history.enabled = false;
        let e = Engine::with_config(paths.clone(), cfg).unwrap();
        e.update_config(|c| c.history.enabled = true).unwrap();
        e.shutdown();
        assert_eq!(History::load(&paths.history_file(), 100).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_services_are_named_with_suggestions() {
        let (e, dir) = engine("require-provider");
        assert!(e.require_provider("google", Capabilities::TRANSLATE).is_ok());
        let err = e.require_provider("gogle", Capabilities::TRANSLATE).unwrap_err();
        assert!(err.contains("unknown service \"gogle\"") && err.contains("google"), "{err}");
        let err = e.require_provider("lens", Capabilities::TRANSLATE).unwrap_err();
        assert!(err.contains("cannot do translate"), "{err}");
        let err = e.require_provider("zzzzzzzzzz", Capabilities::OCR).unwrap_err();
        assert!(err.contains("include: ") && err.contains("lens"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cli_and_api_share_one_translate_shape() {
        let out = TranslationOutcome {
            translation: glossa_core::Translation {
                text: "Привет".into(),
                provider_id: "google".into(),
                romanisation: Some("Privet".into()),
                source_romanisation: Some("həˈlō".into()),
                elapsed_ms: 12,
                ..Default::default()
            },
            pair: LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
            source: "hello".into(),
            detected: None,
            back: None,
            failed: vec![("bing".into(), "timed out".into())],
            truncated: false,
        };
        let v = serde_json::to_value(out.report()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "text": "Привет", "from": "en", "to": "ru", "service": "google",
                "romanization": "Privet", "source_romanization": "həˈlō", "alternatives": [],
                "back": null, "failed": [["bing", "timed out"]], "elapsed_ms": 12,
                "truncated": false
            })
        );
    }

    #[test]
    fn tokens_are_long_and_distinct() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
