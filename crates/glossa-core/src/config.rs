//! User settings.
//!
//! One TOML file, one section per page of the options dialog, in the same
//! order. Every field has a default, so a config file may hold only the
//! handful of settings a user changed and a new build can add settings
//! without breaking old files. Unknown keys are ignored, for the same reason.
//!
//! [`crate::legacy`] imports a classic `Options.json` into this shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::hotkey::{Action, Hotkey};
use crate::lang::{LangId, LangPair};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub general: General,
    pub history: HistoryConfig,
    pub hotkeys: HotkeysConfig,
    pub network: Network,
    pub services: Services,
    pub languages: Languages,
    pub appearance: Appearance,
    pub exceptions: Exceptions,
    pub advanced: Advanced,
    pub mouse_mode: MouseMode,
    /// What clicks on the tray icon (the menu-bar icon on macOS) do.
    pub tray: TrayConfig,
    pub ocr: OcrConfig,
    pub dictionary: DictionaryConfig,
    pub updates: Updates,
    pub server: Server,
    pub modules: ModulesConfig,
    /// What was in the panes when the program last closed, if
    /// [`General::save_contents_on_exit`] is on.
    pub contents: Contents,
    /// Window positions and sizes, by window name.
    pub windows: BTreeMap<String, WindowGeometry>,
    /// A classic installation whose services, themes and interface languages
    /// are used in place, without copying them.
    pub classic: ClassicInstall,
    /// Named sets of services the user switches between (see
    /// [`crate::preset`]). Built-in presets are not listed here. Not
    /// written while empty, so a file without presets has no trace of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<crate::preset::Preset>,
}

/// Where a classic installation lives and which of its parts to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassicInstall {
    /// The folder holding the classic executable and its `Services`,
    /// `Themes`, `Locales` and `Data` folders.
    pub path: Option<PathBuf>,
    pub use_services: bool,
    pub use_themes: bool,
    pub use_locales: bool,
}

impl Default for ClassicInstall {
    fn default() -> Self {
        Self { path: None, use_services: true, use_themes: true, use_locales: true }
    }
}

impl ClassicInstall {
    fn part(&self, enabled: bool, name: &str) -> Option<PathBuf> {
        let dir = self.path.as_ref()?.join(name);
        (enabled && dir.is_dir()).then_some(dir)
    }

    pub fn services_dir(&self) -> Option<PathBuf> {
        self.part(self.use_services, "Services")
    }

    pub fn themes_dir(&self) -> Option<PathBuf> {
        self.part(self.use_themes, "Themes")
    }

    pub fn locales_dir(&self) -> Option<PathBuf> {
        self.part(self.use_locales, "Locales")
    }

    /// Whether `dir` looks like a classic installation.
    pub fn looks_like_install(dir: &Path) -> bool {
        dir.join("Services").is_dir() && (dir.join("Data").is_dir() || dir.join("Themes").is_dir())
    }
}

// -- Basics ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Locale code of the interface (`en`, `ru`), or empty to follow the
    /// system.
    pub ui_language: String,
    pub start_with_system: bool,

    /// The user's own language. Detection uses the pair below to pick a
    /// direction: text in the first language goes to the second, and anything
    /// else comes back to the first.
    pub first_language: LangId,
    pub second_language: LangId,
    /// Language for speech input.
    pub speech_language: LangId,
    /// Pick the target from the first/second pair automatically ("smart
    /// detection").
    pub smart_direction: bool,
    /// Detect the source language even when the user picked one.
    pub always_detect_language: bool,

    /// The direction the main window is set to.
    pub pair: LangPair,
    /// Recently used directions, newest first, for the quick-pick menu.
    pub recent_pairs: Vec<LangPair>,

    /// Translate as you type, after a short pause.
    pub instant_translation: bool,
    pub instant_delay_ms: u64,
    pub spell_checking: bool,
    /// Also translate the translation back, in a third pane — a quick check
    /// that the meaning survived.
    pub back_translation: bool,
    /// Read text aloud phonetically rather than with the target voice.
    pub read_phonetically: bool,
    pub clear_on_drag_drop: bool,
    /// Clear the output when the input changes, so a stale translation is
    /// never on screen next to new text.
    pub auto_cleanup_translation: bool,
    pub save_contents_on_exit: bool,

    pub show_top_pane: bool,
    pub show_middle_pane: bool,
    pub show_services_pane: bool,
    /// How several services' answers are laid out.
    pub results_layout: ResultsLayout,

    pub minimize_to_tray_on_minimize: bool,
    pub minimize_to_tray_on_close: bool,
    pub startup_action: StartupAction,
    /// Open the main window at start with a note that Glossa lives in the
    /// tray, until the note is put away. On for a fresh configuration (the
    /// first run), off for a file written before the note existed: those
    /// users know.
    #[serde(default)]
    pub show_tray_hint: bool,
    /// Show the welcome guide at start: what Glossa is, its hotkeys, how to
    /// use it, where to keep keys and data. On for a fresh configuration,
    /// off for a file written before the guide existed.
    #[serde(default)]
    pub show_welcome: bool,
    /// Where API keys, the proxy password and the Lens token are kept: in
    /// this file, or in the system's keychain (see [`crate::secrets`]).
    pub secret_storage: crate::secrets::SecretStorage,
    /// Who works out the language of the text: the detector built into
    /// Glossa, an enabled service, or the first with the second for text
    /// it is not sure about.
    #[serde(default)]
    pub detect: DetectMode,
}

/// Who detects the language of the text (`[general] detect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectMode {
    /// The detector built into Glossa only: nothing is sent anywhere to
    /// find out the language.
    Offline,
    /// An enabled service only, as before the built-in detector existed.
    Service,
    /// The built-in detector when it is sure, else an enabled service
    /// (short or ambiguous text), else the built-in one's best guess.
    #[default]
    OfflineThenService,
}

impl DetectMode {
    /// The value as `config.toml` and the command line write it.
    pub fn name(self) -> &'static str {
        match self {
            DetectMode::Offline => "offline",
            DetectMode::Service => "service",
            DetectMode::OfflineThenService => "offline_then_service",
        }
    }

    pub fn from_name(name: &str) -> Option<DetectMode> {
        [DetectMode::Offline, DetectMode::Service, DetectMode::OfflineThenService]
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name.trim()))
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            ui_language: String::new(),
            start_with_system: false,
            first_language: LangId::RUSSIAN,
            second_language: LangId::ENGLISH,
            speech_language: LangId::RUSSIAN,
            smart_direction: true,
            always_detect_language: false,
            pair: LangPair::new(LangId::AUTO, LangId::AUTO),
            recent_pairs: vec![
                LangPair::new(LangId::RUSSIAN, LangId::ENGLISH),
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
            ],
            instant_translation: false,
            instant_delay_ms: 700,
            spell_checking: true,
            back_translation: false,
            read_phonetically: false,
            clear_on_drag_drop: true,
            auto_cleanup_translation: false,
            save_contents_on_exit: true,
            show_top_pane: true,
            show_middle_pane: true,
            show_services_pane: true,
            results_layout: ResultsLayout::Tabs,
            minimize_to_tray_on_minimize: false,
            minimize_to_tray_on_close: true,
            startup_action: StartupAction::MinimizeToTray,
            show_tray_hint: true,
            show_welcome: true,
            secret_storage: crate::secrets::SecretStorage::File,
            detect: DetectMode::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultsLayout {
    /// One service at a time, chosen from the tab strip — the classic layout.
    #[default]
    Tabs,
    /// Every enabled service at once, one card each, filling in as they
    /// answer.
    Stacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupAction {
    ShowWindow,
    #[default]
    MinimizeToTray,
    RestorePrevious,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub clear_on_exit: bool,
    /// Show each entry's translation in the list, not just the source.
    pub expand_items: bool,
    /// Oldest non-favourite entries are dropped beyond this.
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { enabled: true, clear_on_exit: false, expand_items: false, max_entries: 5000 }
    }
}

// -- Hotkeys -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeysConfig {
    pub enabled: bool,
    pub bindings: BTreeMap<Action, Hotkey>,
    /// Hotkeys that open module panels, by `module/panel` name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub panels: BTreeMap<String, Hotkey>,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        let bindings = default_hotkeys(Os::current())
            .iter()
            .map(|(a, s)| (*a, s.parse().expect("built-in hotkey must parse")))
            .collect();
        Self { enabled: true, bindings, panels: BTreeMap::new() }
    }
}

/// The operating system, for defaults that differ between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    /// Linux and the other Unix desktops.
    Unix,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Unix
        }
    }
}

/// The hotkeys a new config starts with on `os`. Only new configs: a saved
/// file keeps its own bindings, and the classic import brings the classic
/// ones.
///
/// - Ctrl+Shift+Q (popup) and Ctrl+Shift+O (recognise) everywhere, as users
///   know them.
/// - No default for "listen": Ctrl+E, the obvious one, is a browser's
///   search box, the end of the line in a shell or a macOS text field, and
///   the search in Explorer; a global hotkey would steal it everywhere.
/// - Windows: no Ctrl+Alt+letter, because Windows reports AltGr as Ctrl+Alt
///   and such a hotkey would swallow AltGr+letter (`@` is AltGr+Q on German
///   keyboards; ó and ć are AltGr+O and AltGr+C on Polish ones). Win+Alt is
///   free of both AltGr and the system's own Win shortcuts for these
///   letters.
/// - Linux: AltGr is a modifier of its own there (ISO Level 3), so
///   Ctrl+Alt+letter is safe and free on the common desktops.
/// - macOS: Control+Option (⌃⌥), which no system shortcut uses and which
///   types no characters (Option alone does); it is VoiceOver's modifier
///   only while VoiceOver runs. Command+Shift combinations clash with the
///   system (⌘⇧Q logs out) and with most apps.
/// - Ctrl+Shift+C would be the obvious chord for copying from the screen,
///   but it is Copy in Windows Terminal and most Linux terminals.
pub fn default_hotkeys(os: Os) -> &'static [(Action, &'static str)] {
    match os {
        Os::Windows => &[
            (Action::ShowPopup, "Ctrl+Shift+Q"),
            (Action::TextRecognition, "Ctrl+Shift+O"),
            (Action::RecognizeAndTranslate, "Win+Alt+O"),
            (Action::RecognizeToClipboard, "Win+Alt+C"),
            (Action::TranslateScreenInPlace, "Win+Alt+S"),
            (Action::ShowMainWindow, "Win+Alt+Q"),
        ],
        Os::MacOs | Os::Unix => &[
            (Action::ShowPopup, "Ctrl+Shift+Q"),
            (Action::TextRecognition, "Ctrl+Shift+O"),
            (Action::RecognizeAndTranslate, "Ctrl+Alt+O"),
            (Action::RecognizeToClipboard, "Ctrl+Alt+C"),
            (Action::TranslateScreenInPlace, "Ctrl+Alt+S"),
            (Action::ShowMainWindow, "Ctrl+Alt+Q"),
        ],
    }
}

impl HotkeysConfig {
    /// Which actions share a chord. Registering a chord twice fails on every
    /// platform, so the options page shows these before applying.
    pub fn conflicts(&self) -> Vec<(Hotkey, Vec<Action>)> {
        let mut by_key: BTreeMap<String, (Hotkey, Vec<Action>)> = BTreeMap::new();
        for (action, hk) in &self.bindings {
            by_key.entry(hk.to_string()).or_insert_with(|| (hk.clone(), Vec::new())).1.push(*action);
        }
        by_key.into_values().filter(|(_, actions)| actions.len() > 1).collect()
    }
}

// -- Internet ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    pub timeout_secs: u64,
    pub proxy: Proxy,
    /// Sent with every request unless a provider sets its own.
    pub user_agent: String,
    /// Retries for connection errors, timeouts and 429/5xx answers.
    pub max_retries: u32,
    /// Nothing leaves this computer: every request to a host other than
    /// this machine (`localhost`, `127.0.0.1`, `::1`) is refused, and so is
    /// every service that does not say it works offline. A translator, an
    /// OCR engine or a voice running here keeps working.
    #[serde(default)]
    pub offline_only: bool,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            timeout_secs: 10,
            proxy: Proxy::default(),
            user_agent: String::new(),
            max_retries: 1,
            offline_only: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Proxy {
    pub mode: ProxyMode,
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Stored in the clear, as the classic program did. The options page says
    /// so next to the field.
    pub password: String,
}

impl Default for Proxy {
    fn default() -> Self {
        Self {
            mode: ProxyMode::System,
            scheme: ProxyScheme::Http,
            host: "127.0.0.1".into(),
            port: 8080,
            username: String::new(),
            password: String::new(),
        }
    }
}

impl Proxy {
    /// The proxy as a URL (`socks5://user:pass@host:port`), when one is
    /// configured manually.
    pub fn url(&self) -> Option<String> {
        if self.mode != ProxyMode::Manual || self.host.trim().is_empty() {
            return None;
        }
        let auth = if self.username.is_empty() {
            String::new()
        } else {
            format!("{}:{}@", pct(&self.username), pct(&self.password))
        };
        Some(format!("{}://{}{}:{}", self.scheme.as_str(), auth, self.host.trim(), self.port))
    }
}

fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The four choices of the classic proxy drop-down, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    System,
    None,
    Manual,
    /// The classic "auto-detect" choice. Proxy auto-discovery (WPAD, PAC
    /// scripts) is not implemented, so this is an alias of `System`; it is
    /// kept so imported classic settings and existing files still load.
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyScheme {
    #[default]
    Http,
    Https,
    Socks4,
    Socks5,
}

impl ProxyScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyScheme::Http => "http",
            ProxyScheme::Https => "https",
            ProxyScheme::Socks4 => "socks4",
            ProxyScheme::Socks5 => "socks5",
        }
    }
}

// -- Services ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Services {
    /// Translation services in tab order. Ids not listed come after, in
    /// discovery order.
    pub order: Vec<String>,
    /// Which ones appear in the tab strip.
    pub enabled: Vec<String>,
    /// The selected tab.
    pub active: String,
    /// The popup keeps its own current service, switched from the strip that
    /// appears when the pointer is over it. Empty: follow `active`.
    pub popup_active: String,
    /// Dictionary services, the same way.
    pub dictionary_order: Vec<String>,
    pub dictionary_enabled: Vec<String>,
    pub dictionary_active: String,
    /// Speech service for "listen".
    pub tts: String,
    /// When the active service fails, try the next enabled one.
    pub fail_over: bool,
    /// Per-module settings, keyed by module id, then option key. Values are
    /// whatever the module's manifest declares.
    pub options: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    /// Every provider that has been offered once. A provider not in this
    /// list is new: it is switched on if it works without setup
    /// ([`ProviderInfo::ready_to_use`](crate::ProviderInfo)), otherwise just
    /// listed. Either way it is added here, so a service the user switched
    /// off stays off.
    pub seen: Vec<String>,
    /// OpenAI-compatible endpoints the user added (Options → Services → AI
    /// endpoints). Each becomes a translator of its own; its settings (URL,
    /// key, model, prompt…) live in `options` under its id like any
    /// provider's.
    pub ai_endpoints: Vec<AiEndpoint>,
    /// The preset applied last, shown beside the service strip. Empty when
    /// none was.
    pub preset: String,
}

/// One user-defined OpenAI-compatible endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiEndpoint {
    /// The provider id: `ai-` and a slug, unique.
    pub id: String,
    /// Shown in the service strip and lists.
    pub name: String,
    /// The preset it was made from (`openrouter`, `ollama`…), for its icon and
    /// defaults; `custom` for none.
    #[serde(default)]
    pub preset: String,
}

impl Services {
    /// A fresh, unused endpoint id for a name: `ai-` + a slug of it.
    pub fn new_ai_endpoint_id(&self, name: &str) -> String {
        let mut slug: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if slug.is_empty() {
            slug = "endpoint".into();
        }
        let base = format!("ai-{slug}");
        let taken = |id: &str| self.ai_endpoints.iter().any(|e| e.id == id) || self.options.contains_key(id);
        if !taken(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).expect("unbounded")
    }
}

impl Default for Services {
    fn default() -> Self {
        Self {
            // Nothing is listed here on purpose: which services exist is only
            // known once built-ins and modules are loaded, and the engine
            // fills these in from what it finds (`Engine::adopt_providers`).
            order: Vec::new(),
            enabled: Vec::new(),
            active: String::new(),
            popup_active: String::new(),
            dictionary_order: Vec::new(),
            dictionary_enabled: Vec::new(),
            dictionary_active: String::new(),
            tts: String::new(),
            fail_over: true,
            options: BTreeMap::new(),
            seen: Vec::new(),
            ai_endpoints: Vec::new(),
            preset: String::new(),
        }
    }
}

impl Services {
    /// A module's option, or `None` if the user never set it (the module's
    /// manifest default then applies).
    pub fn option(&self, module: &str, key: &str) -> Option<&serde_json::Value> {
        self.options.get(module)?.get(key)
    }

    pub fn set_option(&mut self, module: &str, key: &str, value: serde_json::Value) {
        self.options.entry(module.to_owned()).or_default().insert(key.to_owned(), value);
    }

    /// `ids` sorted by the user's order, unknown ones last in their original
    /// order.
    pub fn sorted<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
        let mut ids: Vec<&str> = ids.into_iter().collect();
        let rank = |id: &str| self.order.iter().position(|o| o == id).unwrap_or(usize::MAX);
        ids.sort_by_key(|id| rank(id));
        ids
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Languages {
    /// Languages hidden from every drop-down.
    pub disabled: Vec<LangId>,
}

impl Languages {
    pub fn is_enabled(&self, lang: LangId) -> bool {
        !self.disabled.contains(&lang)
    }
}

// -- Appearance ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Theme name, as listed in the theme picker: the one used while
    /// `follow_system_theme` is off.
    pub theme: String,
    /// Follow the system's light or dark mode: `theme_light` while it is
    /// light, `theme_dark` while it is dark, switching when the system does.
    /// On for a fresh configuration, off for a file written before it existed
    /// (those users chose one theme).
    #[serde(default)]
    pub follow_system_theme: bool,
    /// The theme while the system is light, with `follow_system_theme`.
    pub theme_light: String,
    /// The theme while the system is dark, with `follow_system_theme`.
    pub theme_dark: String,
    /// Draw our own window frame in theme colours rather than the system one.
    pub custom_frame: bool,
    /// UI font face, or empty for the theme's.
    pub font_family: String,
    /// UI scale, 1.0 is 100%.
    pub ui_scale: f32,
    /// Size of the text in the source and translation panes, in points.
    pub text_size: f32,
    /// A zoom on the text areas alone (the panes, the popup's text, the
    /// dictionary article), changed with Ctrl+wheel over them; 1.0 is
    /// `text_size` as set. The interface scale is `ui_scale`, set in Options.
    pub text_scale: f32,
    /// Size of toolbar and popup icons, as a multiple of their base size.
    pub icon_scale: f32,
    /// Size of the on-screen keyboard, apart from the interface scale:
    /// Ctrl+wheel over it, or resizing its window, changes this.
    pub keyboard_scale: f32,
    /// 0 (opaque) to 90 percent transparent, for the main window — the same
    /// scale as the popup's.
    pub window_transparency: u8,
    /// How windows are drawn. Takes effect at the next start.
    pub renderer: Renderer,
    /// After this many seconds with every window hidden, the graphics are
    /// unloaded and only the tray, hotkeys and engine stay resident (a few MB
    /// instead of tens). 0 keeps them loaded.
    pub unload_graphics_after_secs: u32,
    /// The system material behind the main window and the other windows
    /// (options, dictionary, history). See [`Backdrop`].
    #[serde(default)]
    pub backdrop: Backdrop,
    /// 0 to 100 percent: how much of the windows' own fills and of the
    /// material's tint is drawn over a backdrop. 100 is the theme as made;
    /// lower lets more of what is behind show through. Mica tints itself, so
    /// there only the fills change.
    #[serde(default = "full_percent")]
    pub backdrop_opacity: u8,
    /// Keep the main window above other windows.
    pub main_always_on_top: bool,
    pub popup: PopupConfig,
}

fn full_percent() -> u8 {
    100
}

/// A material the window system draws behind a window: what WinUI calls a
/// system backdrop, and macOS a visual-effect (vibrancy) view.
///
/// Only the request lives here; `glossa-platform::backdrop` applies it, and
/// does nothing where the system has no such material (Linux, older
/// Windows). The theme decides how translucent the window's own fills are
/// while one is active ([`crate::theme::Theme::backdrop_fill`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backdrop {
    /// An ordinary opaque window.
    #[default]
    None,
    /// Windows 11 Mica: the desktop wallpaper, heavily blurred and tinted,
    /// for long-lived windows. On macOS the window-background material.
    Mica,
    /// Mica Alt, the stronger tint WinUI uses behind tabbed windows.
    #[serde(alias = "mica_alt", alias = "mica-alt")]
    MicaAlt,
    /// Acrylic: what is behind the window, blurred and tinted, as WinUI
    /// draws flyouts. On macOS the popover material.
    Acrylic,
    /// A plain blur of what is behind the window, with no tint of its own.
    Blur,
}

impl Backdrop {
    pub const ALL: [Backdrop; 5] =
        [Backdrop::None, Backdrop::Mica, Backdrop::MicaAlt, Backdrop::Acrylic, Backdrop::Blur];

    /// The name used in `config.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Backdrop::None => "none",
            Backdrop::Mica => "mica",
            Backdrop::MicaAlt => "micaalt",
            Backdrop::Acrylic => "acrylic",
            Backdrop::Blur => "blur",
        }
    }

    /// Whether any material is asked for at all.
    pub fn is_some(self) -> bool {
        self != Backdrop::None
    }

    /// Whether the system tints the material itself (Mica, Acrylic). A plain
    /// blur shows the colours behind the window unchanged, so the window's
    /// own fill has to supply the contrast.
    pub fn is_tinted(self) -> bool {
        matches!(self, Backdrop::Mica | Backdrop::MicaAlt | Backdrop::Acrylic)
    }
}

impl std::fmt::Display for Backdrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Backdrop {
    type Err = String;

    /// Lenient: case, spaces, `-` and `_` are ignored (`"Mica Alt"`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key: String = s.trim().to_ascii_lowercase().chars().filter(|c| !matches!(c, '_' | '-' | ' ')).collect();
        Backdrop::ALL
            .into_iter()
            .find(|b| b.as_str() == key)
            .ok_or_else(|| format!("unknown backdrop {s:?} (none, mica, micaalt, acrylic, blur)"))
    }
}

/// The graphics back end.
///
/// `config.toml` has always been written with `open_gl`, while
/// `GLOSSA_RENDERER` spoke of `glow`; both spellings (and `opengl`, `gl`,
/// `wgpu`) are read in either place, and the file keeps being written the
/// old way so older builds still load it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    /// OpenGL. The lightest in practice.
    #[default]
    #[serde(alias = "glow", alias = "opengl", alias = "gl")]
    OpenGl,
    /// The GPU through Direct3D 12, Vulkan or Metal.
    #[serde(alias = "wgpu")]
    Gpu,
    /// A software rasteriser (WARP on Windows): no GPU, more CPU and memory.
    Software,
}

impl Renderer {
    pub fn as_str(self) -> &'static str {
        match self {
            Renderer::OpenGl => "glow",
            Renderer::Gpu => "gpu",
            Renderer::Software => "software",
        }
    }

    /// A renderer by any of its names, as `GLOSSA_RENDERER` or the file may
    /// give it (case-insensitive): `glow`, `open_gl`, `opengl`, `gl`; `gpu`,
    /// `wgpu`; `software`.
    pub fn from_name(name: &str) -> Option<Renderer> {
        match name.trim().to_ascii_lowercase().as_str() {
            "glow" | "open_gl" | "opengl" | "open-gl" | "gl" => Some(Renderer::OpenGl),
            "gpu" | "wgpu" => Some(Renderer::Gpu),
            "software" | "cpu" | "warp" => Some(Renderer::Software),
            _ => None,
        }
    }
}

/// Keep a text zoom within what stays readable and fits a pane: 50% to
/// 400%. A value that is not a number (a hand-edited file) is 100%.
pub fn clamp_text_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.5, 4.0)
    } else {
        1.0
    }
}

impl Default for Appearance {
    fn default() -> Self {
        // The pair that looks native here.
        let (light, dark) = if cfg!(windows) {
            ("Fluent Light", "Fluent Dark")
        } else if cfg!(target_os = "macos") {
            ("macOS Light", "macOS Dark")
        } else {
            ("Glossa Light", "Glossa Dark")
        };
        Self {
            theme: "Glossa Dark".into(),
            follow_system_theme: true,
            theme_light: light.into(),
            theme_dark: dark.into(),
            custom_frame: true,
            font_family: String::new(),
            ui_scale: 1.0,
            text_size: 11.0,
            text_scale: 1.0,
            icon_scale: 1.2,
            keyboard_scale: 1.0,
            window_transparency: 0,
            renderer: Renderer::OpenGl,
            unload_graphics_after_secs: 45,
            backdrop: Backdrop::None,
            backdrop_opacity: 100,
            main_always_on_top: false,
            popup: PopupConfig::default(),
        }
    }
}

impl Appearance {
    /// The theme in effect: with `follow_system_theme`, the light or the
    /// dark one by the system's mode (`system_dark`), otherwise, or when the
    /// mode cannot be told, `theme`.
    pub fn effective_theme(&self, system_dark: Option<bool>) -> &str {
        match (self.follow_system_theme, system_dark) {
            (true, Some(true)) => &self.theme_dark,
            (true, Some(false)) => &self.theme_light,
            _ => &self.theme,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PopupConfig {
    pub auto_size: bool,
    /// Place it next to the selection, kept on screen.
    pub auto_position: bool,
    /// Take keyboard focus when it opens.
    pub activate_on_open: bool,
    /// Stop auto-hiding once the user drags it somewhere.
    pub pin_when_dragging: bool,
    /// Show the main window, with the same text, whenever the popup opens.
    /// The main window does not take the focus, so the program the text
    /// came from keeps it.
    pub open_main_too: bool,
    /// Draw the coloured border around it.
    pub show_border: bool,
    /// Hide after this many seconds without the pointer over it. 0 never
    /// hides.
    pub auto_hide_secs: u32,
    /// 0 (opaque) to 90 percent transparent, as the classic slider went.
    pub transparency: u8,
    pub frame_thickness: f32,
    pub icons: PopupIcons,
    /// Overrides for the theme's popup colours. `None` keeps the theme's.
    pub back: Option<Color>,
    pub text: Option<Color>,
    pub frame: Option<Color>,
    /// Width bounds, in logical pixels. The classic popup measured 350 for a
    /// single word and 600 for a sentence.
    pub min_width: f32,
    pub max_width: f32,
    pub max_height: f32,
    /// The system material behind the popup. Acrylic suits it best: unlike
    /// Mica it shows what is under the popup rather than the wallpaper.
    #[serde(default)]
    pub backdrop: Backdrop,
    /// 0 to 90 percent: how strongly a translucent popup is shaded toward
    /// black when nothing blurs what is behind it (no backdrop asked for, or
    /// the system could not apply one), so its text stays readable over a
    /// busy page. 0 leaves the fill as the theme and transparency make it.
    #[serde(default)]
    pub shade: u8,
    /// [`Appearance::backdrop_opacity`] for the popup.
    #[serde(default = "full_percent")]
    pub backdrop_opacity: u8,
    /// Size of the popup's header icons, as a multiple of the toolbar icons'
    /// size (`icon_scale`). The header grows with them.
    #[serde(default = "one")]
    pub icon_scale: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            auto_size: true,
            auto_position: true,
            activate_on_open: false,
            pin_when_dragging: true,
            open_main_too: false,
            show_border: true,
            auto_hide_secs: 5,
            transparency: 15,
            frame_thickness: 2.0,
            icons: PopupIcons::default(),
            back: None,
            text: None,
            frame: None,
            min_width: 350.0,
            max_width: 600.0,
            max_height: 480.0,
            backdrop: Backdrop::None,
            shade: 0,
            backdrop_opacity: 100,
            icon_scale: 1.0,
        }
    }
}

impl PopupConfig {
    /// Opacity as the toolkit wants it, 0.1 to 1.0.
    pub fn opacity(&self) -> f32 {
        1.0 - (self.transparency.min(90) as f32 / 100.0)
    }

    /// The popup's fill once the shade is applied: `back` darkened toward
    /// black by `shade` percent and made more opaque by the same share of
    /// what it lacks. `blurred` says a real backdrop is in effect, which
    /// needs no shade, so `back` comes back unchanged.
    pub fn shaded(&self, back: Color, blurred: bool) -> Color {
        if blurred || self.shade == 0 {
            return back;
        }
        let t = self.shade.min(90) as f32 / 100.0;
        let darker = back.lerp(Color::BLACK.with_alpha(back.a), t);
        let alpha = back.a as f32 + (255.0 - back.a as f32) * t;
        darker.with_alpha(alpha.round().clamp(0.0, 255.0) as u8)
    }
}

/// The four buttons along the popup's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PopupIcons {
    pub dictionary: bool,
    pub listen: bool,
    pub copy: bool,
    pub replace: bool,
}

impl Default for PopupIcons {
    fn default() -> Self {
        Self { dictionary: true, listen: true, copy: true, replace: true }
    }
}

impl PopupIcons {
    /// The classic options stored these as a bit mask: 2 dictionary, 4
    /// listen, 8 copy, 16 replace. (A full set was 30.)
    pub fn from_legacy(mask: u32) -> Self {
        Self { dictionary: mask & 2 != 0, listen: mask & 4 != 0, copy: mask & 8 != 0, replace: mask & 16 != 0 }
    }
}

// -- Exceptions ----------------------------------------------------------------

/// Programs where selection capture and mouse mode should stay out of the
/// way — a terminal, a game, a remote desktop window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exceptions {
    pub mode: ExceptionMode,
    pub rules: Vec<ExceptionRule>,
    /// Skip windows that are obviously not text (list views, scroll bars,
    /// hotkey fields) without needing a rule for each.
    pub smart_detection: bool,
}

impl Default for Exceptions {
    fn default() -> Self {
        Self::default_for(Os::current())
    }
}

impl Exceptions {
    /// What a new config starts with on `os`. The classic list names
    /// Windows controls and Remote Desktop; X11 windows have classes too, so
    /// Linux keeps it. macOS reports no window classes and its apps by their
    /// (localised) names, so it starts with an empty list.
    pub fn default_for(os: Os) -> Self {
        let rules = match os {
            Os::MacOs => Vec::new(),
            Os::Windows | Os::Unix => windows_rules(),
        };
        Self { mode: ExceptionMode::DisabledFor, rules, smart_detection: true }
    }

    /// On macOS, swap the untouched Windows defaults (saved by earlier
    /// builds, or carried over from a classic installation) for this
    /// system's. They can never match there, and the list read as nonsense.
    /// A list the user changed is left alone.
    pub fn adopt_platform_defaults(&mut self, os: Os) {
        if os == Os::MacOs && self.rules == windows_rules() {
            self.rules = Self::default_for(os).rules;
        }
    }
}

fn windows_rules() -> Vec<ExceptionRule> {
    let any = |class: &str| ExceptionRule { program: String::new(), window_class: class.into() };
    vec![
        any("SysListView32"),
        any("SysTreeView32"),
        any("ListBox"),
        any("ScrollBar"),
        any("ComboBox"),
        any("msctls_hotkey32"),
        any("ConsoleWindowClass"),
        ExceptionRule { program: "mstsc.exe".into(), window_class: String::new() },
    ]
}

impl Exceptions {
    /// Whether the program is allowed to act on a window of this process and
    /// class.
    pub fn allows(&self, program: &str, window_class: &str) -> bool {
        self.allows_window(program, window_class, "")
    }

    /// The same, also knowing the class of the focused control inside the
    /// window. A rule's class matches either; smart detection refuses
    /// controls that never hold selectable text.
    pub fn allows_window(&self, program: &str, window_class: &str, focus_class: &str) -> bool {
        if self.smart_detection && is_non_text_control(program, focus_class) {
            return false;
        }
        let hit = self
            .rules
            .iter()
            .any(|r| r.matches(program, window_class) || (!focus_class.is_empty() && r.matches(program, focus_class)));
        match self.mode {
            ExceptionMode::DisabledFor => !hit,
            ExceptionMode::EnabledFor => hit,
        }
    }
}

/// Standard Windows controls that show items or take keys but hold no text a
/// user would select: pressing the hotkey there would only send a stray copy
/// keystroke into a list or a hotkey field. Explorer's file view (a
/// `DirectUIHWND` since Windows 7, not the classic list view) is one too: a
/// copy keystroke there copies files.
fn is_non_text_control(program: &str, class: &str) -> bool {
    if program.eq_ignore_ascii_case("explorer.exe") && class == "DirectUIHWND" {
        return true;
    }
    const CLASSES: [&str; 13] = [
        "SysListView32",
        "SysTreeView32",
        "ListBox",
        "ComboLBox",
        "ScrollBar",
        "msctls_hotkey32",
        "msctls_trackbar32",
        "msctls_updown32",
        "SysTabControl32",
        "SysHeader32",
        "ToolbarWindow32",
        "msctls_statusbar32",
        "Button",
    ];
    CLASSES.iter().any(|c| c.eq_ignore_ascii_case(class))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionMode {
    /// Work everywhere except the listed programs.
    #[default]
    DisabledFor,
    /// Work only in the listed programs.
    EnabledFor,
}

/// An empty field matches anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionRule {
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub window_class: String,
}

impl ExceptionRule {
    pub fn matches(&self, program: &str, window_class: &str) -> bool {
        let prog_ok = self.program.is_empty() || self.program.eq_ignore_ascii_case(program);
        let class_ok = self.window_class.is_empty() || self.window_class == window_class;
        prog_ok && class_ok
    }
}

// -- Advanced -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    /// Command to open links with, or empty for the system default.
    pub browser: String,
    /// The keystroke used to copy the selection out of another program.
    pub copy_action: CopyAction,
    pub layout_indicator: LayoutIndicator,
    /// The Google domain to talk to (`com`, `ru`, `com.ua`).
    pub preferred_domain: String,
    /// Translate the program's own menus with the active service, for
    /// interface languages that have no locale file.
    pub gui_translation: bool,
    /// A second "listen" within a few seconds plays slower.
    pub slower_listen_repeat: bool,
    pub remove_line_breaks: bool,
    /// How long to wait for another program to put the selection on the
    /// clipboard after the synthetic copy.
    pub copy_wait_ms: u64,
    /// Terminal windows, where Ctrl+C interrupts the running command instead
    /// of copying (Windows Terminal with nothing selected, mintty, PuTTY,
    /// VS Code's terminal…). There the selection is read through UI
    /// Automation, then with [`Advanced::terminal_copy_action`] instead of
    /// [`Advanced::copy_action`]. A rule's class matches the window's class,
    /// the focused control's, or the focused UI Automation element's (the
    /// HTML class in Electron apps). Windows only: on macOS Cmd+C copies in
    /// terminals, and on Linux no keystroke is sent at all.
    pub terminals: Vec<ExceptionRule>,
    /// The copy keystroke in [`Advanced::terminals`]: none by default
    /// (`accessibility_only`). `ctrl_insert` copies in the Windows console,
    /// Windows Terminal, mintty and WezTerm, but a terminal that does not
    /// bind it passes it to the shell as an escape sequence.
    pub terminal_copy_action: CopyAction,
    /// Which built-in [`Advanced::terminals`] rules this file has been given
    /// ([`TERMINAL_RULE_SETS`]); newer ones are added once on load, so a rule
    /// the user deleted stays deleted.
    pub terminal_rules_version: u32,
    /// On a Wayland desktop that offers neither the PRIMARY selection nor
    /// the RemoteDesktop portal, the selection cannot be read at all. With
    /// this on, the selection actions then take whatever is on the
    /// clipboard (copy first, then press the hotkey). Off by default: the
    /// clipboard may hold something the user never meant to send to a
    /// translation service.
    pub clipboard_as_selection: bool,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            browser: String::new(),
            copy_action: CopyAction::CtrlC,
            layout_indicator: LayoutIndicator::None,
            preferred_domain: "com".into(),
            gui_translation: false,
            slower_listen_repeat: true,
            remove_line_breaks: false,
            copy_wait_ms: 250,
            terminals: TERMINAL_RULE_SETS.iter().flat_map(|(_, rules)| rules.iter().map(|r| r.rule())).collect(),
            terminal_copy_action: CopyAction::AccessibilityOnly,
            terminal_rules_version: latest_terminal_rules(),
            clipboard_as_selection: false,
        }
    }
}

/// A built-in terminal rule: a program (empty for any) and a class (empty
/// for any).
#[derive(Debug, Clone, Copy)]
pub struct TerminalRule(pub &'static str, pub &'static str);

impl TerminalRule {
    fn rule(&self) -> ExceptionRule {
        ExceptionRule { program: self.0.into(), window_class: self.1.into() }
    }
}

/// The built-in terminal rules, by the version that introduced them. A new
/// terminal goes into a new entry with the next version, so existing
/// configs receive it once ([`Advanced::migrate_terminal_rules`]).
pub const TERMINAL_RULE_SETS: &[(u32, &[TerminalRule])] = &[(
    1,
    &[
        // Windows Terminal (any build: the class is the same).
        TerminalRule("", "CASCADIA_HOSTING_WINDOW_CLASS"),
        // The classic console (conhost), also excepted by default.
        TerminalRule("", "ConsoleWindowClass"),
        // xterm.js, which VS Code, Cursor, Hyper, Tabby and web terminals
        // use: the focused element's class when UI Automation reaches it.
        // VS Code's editor in the same window keeps Ctrl+C.
        TerminalRule("", "xterm-helper-textarea"),
        TerminalRule("mintty.exe", ""),
        TerminalRule("wezterm-gui.exe", ""),
        TerminalRule("alacritty.exe", ""),
        TerminalRule("putty.exe", ""),
        TerminalRule("kitty.exe", ""),
        TerminalRule("MobaXterm.exe", ""),
        TerminalRule("ConEmu.exe", ""),
        TerminalRule("ConEmu64.exe", ""),
        TerminalRule("Hyper.exe", ""),
        TerminalRule("Tabby.exe", ""),
    ],
)];

fn latest_terminal_rules() -> u32 {
    TERMINAL_RULE_SETS.iter().map(|(v, _)| *v).max().unwrap_or(0)
}

impl Advanced {
    /// Add the built-in terminal rules newer than this config's
    /// [`Advanced::terminal_rules_version`], skipping any the user already
    /// has, and bring the version up to date. Idempotent.
    pub fn migrate_terminal_rules(&mut self) {
        for (version, rules) in TERMINAL_RULE_SETS {
            if *version <= self.terminal_rules_version {
                continue;
            }
            for rule in rules.iter().map(TerminalRule::rule) {
                let present = self
                    .terminals
                    .iter()
                    .any(|r| r.program.eq_ignore_ascii_case(&rule.program) && r.window_class == rule.window_class);
                if !present {
                    self.terminals.push(rule);
                }
            }
        }
        self.terminal_rules_version = self.terminal_rules_version.max(latest_terminal_rules());
    }

    /// Whether a window is a terminal per [`Advanced::terminals`]: `classes`
    /// are the window's class and those of its focused control and focused
    /// UI Automation element (empty ones are ignored).
    pub fn is_terminal(&self, program: &str, classes: &[&str]) -> bool {
        is_terminal(&self.terminals, program, classes)
    }
}

/// Whether any of the terminal `rules` matches a window of `program` with
/// these classes (see [`Advanced::is_terminal`]).
pub fn is_terminal(rules: &[ExceptionRule], program: &str, classes: &[&str]) -> bool {
    rules.iter().any(|r| {
        let program_ok = r.program.is_empty() || r.program.eq_ignore_ascii_case(program);
        let class_ok = r.window_class.is_empty() || classes.iter().any(|c| !c.is_empty() && *c == r.window_class);
        // A rule with neither field would match every window.
        program_ok && class_ok && !(r.program.is_empty() && r.window_class.is_empty())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyAction {
    /// Ctrl+C.
    #[default]
    CtrlC,
    /// Ctrl+Insert, for terminals where Ctrl+C means "interrupt".
    CtrlInsert,
    /// Ask the accessibility API only; never send a keystroke.
    AccessibilityOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutIndicator {
    #[default]
    None,
    /// A small flag near the caret when the layout changes.
    Caret,
    /// A badge on the tray icon.
    Tray,
}

/// Selecting text with the mouse, without a hotkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseMode {
    pub enabled: bool,
    pub behaviour: MouseBehaviour,
    /// Only react while Ctrl is held.
    pub ctrl_only: bool,
    /// The old switch for the tray click (`false`: the click opens the main
    /// window instead of toggling mouse mode). Read from old files and moved
    /// into [`TrayConfig::click`] on load; never written back.
    #[serde(skip_serializing)]
    pub tray_click_toggles: Option<bool>,
}

impl Default for MouseMode {
    fn default() -> Self {
        Self { enabled: false, behaviour: MouseBehaviour::ShowIcon, ctrl_only: false, tray_click_toggles: None }
    }
}

/// What a click on the tray icon does: an action, the same ones hotkeys
/// run, the tray menu, or nothing. Written as the action's id, `"menu"` or
/// `"none"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayAction {
    Nothing,
    Menu,
    Run(Action),
}

impl TrayAction {
    pub fn id(self) -> &'static str {
        match self {
            TrayAction::Nothing => "none",
            TrayAction::Menu => "menu",
            TrayAction::Run(a) => a.id(),
        }
    }

    pub fn from_id(id: &str) -> Option<TrayAction> {
        match id {
            "none" | "nothing" | "" => Some(TrayAction::Nothing),
            "menu" => Some(TrayAction::Menu),
            other => Action::from_id(other).map(TrayAction::Run),
        }
    }
}

impl Serialize for TrayAction {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for TrayAction {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let id = String::deserialize(d)?;
        TrayAction::from_id(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown tray action `{id}`")))
    }
}

/// What clicks on the tray icon do. Linux's indicators report no clicks at
/// all (a click opens the menu there), so these apply to Windows and macOS.
///
/// Read leniently: a value that names no action (a typo, an action from a
/// newer build) falls back to that click's default with a warning, rather
/// than failing the whole file, which would set the config aside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrayConfig {
    /// A single left click. Showing the main window hides it again when it
    /// is already showing.
    pub click: TrayAction,
    /// A double left click. Anything but `none` makes a single click wait
    /// out the system's double-click time before it acts.
    pub double_click: TrayAction,
    /// A click with the middle button (the wheel).
    pub middle_click: TrayAction,
}

impl<'de> Deserialize<'de> for TrayConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Raw {
            click: Option<String>,
            double_click: Option<String>,
            middle_click: Option<String>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(TrayConfig::from_ids(
            [raw.click.as_deref(), raw.double_click.as_deref(), raw.middle_click.as_deref()],
            Os::current(),
        ))
    }
}

impl TrayConfig {
    /// The defaults on `os`: what the icon did before the clicks could be
    /// chosen. The macOS menu-bar icon opens its menu, as menu-bar items do;
    /// elsewhere a click toggles mouse mode, as the classic program did.
    pub fn for_os(os: Os) -> TrayConfig {
        let click = match os {
            Os::MacOs => TrayAction::Menu,
            _ => TrayAction::Run(Action::ToggleMouseMode),
        };
        TrayConfig { click, double_click: double_click_default(click), middle_click: TrayAction::Nothing }
    }

    /// The settings from the ids a file gives (click, double click, middle
    /// click; `None` where absent). An id that names nothing is replaced by
    /// that click's default on `os`, with a warning; the double click's
    /// default follows the click actually chosen.
    pub fn from_ids(ids: [Option<&str>; 3], os: Os) -> TrayConfig {
        let parse = |key: &str, id: Option<&str>| {
            let id = id?;
            let action = TrayAction::from_id(id);
            if action.is_none() {
                tracing::warn!("config: [tray] {key} = \"{id}\" names no action; using the default");
            }
            action
        };
        let click = parse("click", ids[0]).unwrap_or(TrayConfig::for_os(os).click);
        TrayConfig {
            click,
            double_click: parse("double_click", ids[1]).unwrap_or(double_click_default(click)),
            middle_click: parse("middle_click", ids[2]).unwrap_or(TrayAction::Nothing),
        }
    }

    /// Fold the old `mouse_mode.tray_click_toggles` switch in. Turned off,
    /// it meant the click opens the main window; it never applied on macOS,
    /// where the click opened the menu. A click the file already sets to
    /// something else than the default wins.
    /// The classic program's "click on the tray icon switches mouse mode",
    /// imported over whatever the click was: on, the click toggles mouse
    /// mode; off, it opens the main window. Not on macOS, where the classic
    /// switch never applied and the click opens the menu.
    pub fn import_classic_switch(&mut self, on: bool, os: Os) {
        if os == Os::MacOs {
            return;
        }
        self.click =
            if on { TrayAction::Run(Action::ToggleMouseMode) } else { TrayAction::Run(Action::ShowMainWindow) };
    }

    pub fn adopt_tray_click_toggles(&mut self, toggles: Option<bool>, os: Os) {
        if toggles == Some(false) && os != Os::MacOs && self.click == TrayConfig::for_os(os).click {
            self.click = TrayAction::Run(Action::ShowMainWindow);
        }
    }
}

/// A double click cannot be told apart while the click opens the menu, so
/// it does nothing by default there (and the options page has nothing to
/// warn about on a fresh macOS install).
fn double_click_default(click: TrayAction) -> TrayAction {
    if click == TrayAction::Menu {
        TrayAction::Nothing
    } else {
        TrayAction::Run(Action::ShowMainWindow)
    }
}

impl Default for TrayConfig {
    fn default() -> Self {
        TrayConfig::for_os(Os::current())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseBehaviour {
    /// A small icon near the pointer; click it to translate.
    #[default]
    ShowIcon,
    /// Translate straight away.
    ShowTranslation,
    /// Translate and read aloud.
    ShowTranslationAndRead,
}

// -- OCR, dictionary, updates ---------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    /// Provider id: `lens` (chrome-lens), `windows-ocr`, `oneocr`,
    /// `macos-vision`, `tesseract`, `command-ocr` or a module's.
    pub provider: String,
    /// Language of the text in the picture; `AUTO` lets the provider decide.
    pub language: LangId,
    /// Translate after recognising, into the main window's target language.
    pub translate_after: bool,
    /// Keep line breaks from the picture.
    pub preserve_line_breaks: bool,
    /// Save each captured region here, if set.
    pub save_image_dir: Option<PathBuf>,
    /// How often a watched region is checked for changes.
    pub watch_interval_ms: u64,
    /// What plain "text recognition" does once a region is selected. The
    /// dedicated actions (to the clipboard, translate, in place) ignore it.
    pub after_select: OcrAfterSelect,
    /// Who translates recognised text: the OCR engine in the same request
    /// when it can (Google Lens), or the current translator.
    pub translate_with: OcrTranslateWith,
    /// Join the lines of recognised text when copying it (prose), rather
    /// than keeping the picture's line breaks (code, lists).
    pub copy_joined_lines: bool,
    pub lens: LensConfig,
    /// The Windows 11 Snipping Tool's engine.
    pub oneocr: OneOcrConfig,
    /// Tesseract, run as a program.
    pub tesseract: TesseractConfig,
    /// Apple Vision, on macOS.
    pub vision: VisionConfig,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            provider: "lens".into(),
            language: LangId::AUTO,
            translate_after: true,
            preserve_line_breaks: true,
            save_image_dir: None,
            watch_interval_ms: 800,
            after_select: OcrAfterSelect::default(),
            translate_with: OcrTranslateWith::default(),
            copy_joined_lines: false,
            lens: LensConfig::default(),
            oneocr: OneOcrConfig::default(),
            tesseract: TesseractConfig::default(),
            vision: VisionConfig::default(),
        }
    }
}

/// What text recognition does after a region is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrAfterSelect {
    /// Show the text, and its translation when `translate_after` is on, beside
    /// the region.
    #[default]
    Show,
    /// Copy the text to the clipboard and close.
    Copy,
    /// Paint the translation over the region.
    InPlace,
    /// Offer a toolbar to choose.
    Ask,
}

/// Who translates the text an OCR engine read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrTranslateWith {
    /// The OCR engine, in the same request, when it can (Google Lens); the
    /// current translator otherwise.
    #[default]
    OcrEngine,
    /// Always the current translator.
    Translator,
}

/// Where OneOCR's files are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OneOcrConfig {
    /// A folder holding `oneocr.dll`, `onnxruntime.dll` and
    /// `oneocr.onemodel`. Empty finds the Snipping Tool's and copies the
    /// libraries into the cache folder.
    pub folder: String,
}

/// How to run Tesseract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TesseractConfig {
    /// The `tesseract` executable. Empty looks on `PATH` and in the usual
    /// install folders (`C:\Program Files\Tesseract-OCR`, Homebrew's).
    pub path: String,
    /// Where the `.traineddata` files are, if not Tesseract's own folder.
    pub tessdata_dir: String,
    /// Languages for pictures whose language is automatic, in Tesseract's
    /// form (`eng+rus`). Empty uses English.
    pub auto_languages: String,
    /// Page segmentation mode (`--psm`); `None` keeps Tesseract's default.
    pub psm: Option<u8>,
    /// OCR engine mode (`--oem`); `None` keeps Tesseract's default.
    pub oem: Option<u8>,
    /// More arguments, split the way a shell would split them
    /// (`-c preserve_interword_spaces=1`).
    pub extra_args: String,
}

/// How to ask Apple Vision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionConfig {
    /// `accurate` (the default) or `fast`.
    pub level: VisionLevel,
    /// Let Vision correct words with its language model. Off reads codes,
    /// names and other non-words more literally.
    pub language_correction: bool,
    /// Vision's language identifiers (`en-US, ru-RU`) to read when the
    /// picture's language is automatic. Empty lets macOS 13 and later
    /// detect the language; older systems then read English.
    pub auto_languages: String,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self { level: VisionLevel::Accurate, language_correction: true, auto_languages: String::new() }
    }
}

/// Apple Vision's trade-off between speed and accuracy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionLevel {
    #[default]
    Accurate,
    Fast,
}

/// How to reach chrome-lens-py.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensConfig {
    /// `auto`: use a running daemon if there is one, else start one with the
    /// CLI, else run the CLI per image. `daemon`, `cli` force one path.
    pub mode: LensMode,
    /// A daemon URL to use instead of discovering one, e.g.
    /// `http://127.0.0.1:8765`.
    pub endpoint: String,
    /// Bearer token for `endpoint`.
    pub token: String,
    /// The `lens_scan` executable, if it is not on `PATH`.
    pub cli_path: String,
}

impl Default for LensConfig {
    fn default() -> Self {
        Self { mode: LensMode::Auto, endpoint: String::new(), token: String::new(), cli_path: String::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensMode {
    /// A running chrome-lens-py daemon when there is one (it keeps a warm
    /// connection to Google), else the built-in client.
    #[default]
    Auto,
    /// The built-in client only: no Python at all.
    Native,
    /// A chrome-lens-py daemon only.
    Daemon,
    /// chrome-lens-py's CLI, per image.
    Cli,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictionaryConfig {
    pub show_services_pane: bool,
    /// Article zoom, percent.
    pub zoom: u32,
    pub exact_search: bool,
    /// Offline dictionary files (StarDict `.ifo`, XDXF, DSL, tab-separated).
    pub offline: Vec<PathBuf>,
    pub language: LangId,
}

impl Default for DictionaryConfig {
    fn default() -> Self {
        Self { show_services_pane: true, zoom: 100, exact_search: true, offline: Vec::new(), language: LangId::RUSSIAN }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Updates {
    pub check_automatically: bool,
}

/// The local control API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub enabled: bool,
    /// Loopback only by default; binding elsewhere needs a token.
    pub bind: String,
    /// Required on every request but `/health`. Never empty while the engine
    /// runs: an empty one is replaced with a new random token and saved at
    /// once (`Engine::with_config`, `Engine::update_config`).
    pub token: String,
    /// Let pot-app's routes (`/selection_translate`, `/translate`…) through
    /// without the token, but only from a loopback address and without an
    /// `Origin` header, so key bindings made for pot-app work unchanged.
    /// Off unless the user turns it on: with it, any local process (another
    /// user's included) can open the popup with text of its choosing.
    ///
    /// This replaces `pot_compat_without_token`, which was on by default
    /// and written into every `config.toml`, so a file cannot say whether
    /// its user wanted it. The old key is deliberately not an alias: it is
    /// ignored, and the routes need the token until this one is set.
    pub pot_routes_without_token: bool,
    /// Offer the same actions on the session bus (`io.github.bropines.Glossa`)
    /// on Linux and the BSDs. Needs no token: the session bus is the user's
    /// own. Ignored on Windows and macOS.
    pub dbus: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: "127.0.0.1:60828".into(),
            token: String::new(),
            pot_routes_without_token: false,
            dbus: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModulesConfig {
    /// Extra directories to look for modules in, after the built-in ones.
    pub dirs: Vec<PathBuf>,
    /// Which Python runs Python modules: `auto` (a system Python 3.8+ for a
    /// module without dependencies, else the managed one Glossa downloads),
    /// `system`, `managed`, or a path to an interpreter (or a command line
    /// such as `py -3`). Empty means `auto`.
    pub python: String,
    /// `java` for Java modules: the executable or a Java home folder. Empty
    /// looks in `JAVA_HOME`, on `PATH` and in the usual install folders.
    pub java: String,
    /// Load native (shared library) modules at all. They run with the
    /// program's full privileges, so this is a deliberate switch.
    pub allow_native: bool,
    /// Load classic `Service.js` scripts.
    pub allow_legacy_scripts: bool,
    /// Time budget for one module call, milliseconds.
    pub call_timeout_ms: u64,
    /// Module ids switched off.
    pub disabled: Vec<String>,
    /// Download the files a module declares (models, dictionaries) on its
    /// first use without asking. Off: the module's options offer the
    /// download instead.
    pub auto_download_assets: bool,
    /// What the user approved for each module installed by hand, by module
    /// id (`[modules.granted.<id>]`). A module whose manifest asks for more
    /// than its grant waits for approval instead of running; see
    /// `glossa_modules::trust`. Modules shipped with Glossa need none.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub granted: BTreeMap<String, ModuleGrant>,
    /// Set once the modules installed before approvals existed have been
    /// granted what they declared then, so an update does not switch off
    /// modules that were already working. Missing in older settings files,
    /// which is what starts that one-time migration.
    pub trust_initialised: bool,
}

/// What the user approved for one module: the permission-relevant parts of
/// its manifest at the time, and their hash. Written by Glossa; editing it
/// by hand approves whatever it then says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleGrant {
    /// SHA-256 of the approved set, in hex: equal for an unchanged manifest.
    pub hash: String,
    /// The runtime (`js`, `python`, `process`, `native`…).
    pub runtime: String,
    /// The program a process module runs, with its arguments and
    /// environment; empty for sandboxed modules.
    pub program: Vec<String>,
    /// Hosts it may reach (`[permissions] network`).
    pub network: Vec<String>,
    /// Files it may download: name, size, host and SHA-256 of each.
    pub assets: Vec<String>,
    /// Python packages it may install, and the index they come from.
    pub packages: Vec<String>,
    /// Its actions (buttons), by id.
    pub actions: Vec<String>,
    /// Permission lines contributed by other features (panels, tray
    /// entries, hotkeys), as `glossa_modules::trust` writes them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    /// The module's folder, canonical, when approved: another folder
    /// claiming the same id waits for its own approval. Empty (a grant
    /// written by hand) applies to the id in any folder.
    pub dir: String,
}

impl Default for ModulesConfig {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            python: "auto".into(),
            java: String::new(),
            allow_native: true,
            allow_legacy_scripts: true,
            call_timeout_ms: 30_000,
            disabled: Vec::new(),
            auto_download_assets: false,
            granted: BTreeMap::new(),
            trust_initialised: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Contents {
    pub source: String,
    pub translation: String,
    pub back_translation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

// -- loading and saving ---------------------------------------------------------

/// A config file that could not be parsed and was moved aside
/// ([`Config::load_or_recover`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredConfig {
    /// Where the unreadable file went.
    pub backup: PathBuf,
    /// What was wrong with it.
    pub error: String,
}

impl Config {
    /// Read a config file. A missing file is not an error: it means "all
    /// defaults". A malformed one is, so the user's settings are never
    /// silently replaced.
    pub fn load(path: &Path) -> crate::Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// [`Config::load`] for starting the program: a file that cannot be
    /// parsed is moved aside to `config.toml.broken-<unix time>` and the
    /// defaults are used, so a typo in a hand edit cannot keep the program
    /// from starting. What happened is returned for the user to be told.
    /// Errors other than parsing (a file that cannot be read) still fail.
    pub fn load_or_recover(path: &Path) -> crate::Result<(Config, Option<RecoveredConfig>)> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Config::default(), None)),
            Err(e) => return Err(e.into()),
        };
        match Config::from_toml(&text) {
            Ok(c) => Ok((c, None)),
            Err(e) => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let mut name = path.file_name().unwrap_or_default().to_os_string();
                name.push(format!(".broken-{secs}"));
                let backup = path.with_file_name(name);
                // Moved, not copied: the next save writes a fresh file, and
                // the user's text survives beside it.
                std::fs::rename(path, &backup).map_err(|io| {
                    crate::Error::Config(format!(
                        "{e}; and the file could not be moved aside to {}: {io}",
                        backup.display()
                    ))
                })?;
                tracing::error!("{e}; moved it to {} and started with the defaults", backup.display());
                Ok((Config::default(), Some(RecoveredConfig { backup, error: e.to_string() })))
            }
        }
    }

    pub fn from_toml(text: &str) -> crate::Result<Config> {
        let mut config: Config = toml::from_str(text).map_err(|e| crate::Error::Config(format!("config file: {e}")))?;
        config.sanitise();
        Ok(config)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config always serialises")
    }

    /// Write atomically: to a sibling temp file, then rename over the
    /// original, so a crash mid-write cannot leave half a config.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Pull out-of-range values back into range. Applied on every load, so
    /// the rest of the program can trust the numbers.
    pub fn sanitise(&mut self) {
        self.network.timeout_secs = self.network.timeout_secs.clamp(1, 300);
        self.network.max_retries = self.network.max_retries.min(5);
        let p = &mut self.appearance.popup;
        p.transparency = p.transparency.min(90);
        p.shade = p.shade.min(90);
        p.frame_thickness = p.frame_thickness.clamp(0.0, 10.0);
        p.auto_hide_secs = p.auto_hide_secs.min(600);
        p.max_width = p.max_width.clamp(160.0, 4000.0);
        p.min_width = p.min_width.clamp(80.0, p.max_width);
        p.max_height = p.max_height.clamp(80.0, 4000.0);
        self.appearance.ui_scale = self.appearance.ui_scale.clamp(0.5, 3.0);
        self.appearance.text_size = self.appearance.text_size.clamp(6.0, 48.0);
        self.appearance.text_scale = clamp_text_scale(self.appearance.text_scale);
        self.appearance.icon_scale = self.appearance.icon_scale.clamp(0.75, 3.0);
        self.appearance.keyboard_scale = self.appearance.keyboard_scale.clamp(0.5, 3.0);
        self.appearance.window_transparency = self.appearance.window_transparency.min(90);
        self.dictionary.zoom = self.dictionary.zoom.clamp(50, 400);
        self.advanced.migrate_terminal_rules();
        let toggles = self.mouse_mode.tray_click_toggles.take();
        self.tray.adopt_tray_click_toggles(toggles, Os::current());
        self.exceptions.adopt_platform_defaults(Os::current());
        self.general.instant_delay_ms = self.general.instant_delay_ms.clamp(150, 5000);
        // Keep the first (newest) of each pair, wherever the repeats are.
        let mut seen = Vec::with_capacity(self.general.recent_pairs.len());
        self.general.recent_pairs.retain(|p| {
            let new = !seen.contains(p);
            seen.push(*p);
            new
        });
        self.general.recent_pairs.truncate(12);
        self.history.max_entries = self.history.max_entries.clamp(10, 1_000_000);
        self.modules.call_timeout_ms = self.modules.call_timeout_ms.clamp(1000, 600_000);
        self.ocr.watch_interval_ms = self.ocr.watch_interval_ms.clamp(200, 60_000);
        self.advanced.copy_wait_ms = self.advanced.copy_wait_ms.clamp(20, 3000);
        // Not a real language (auto, a hand-edited number): the defaults.
        let defaults = General::default();
        if !self.general.second_language.is_language() {
            self.general.second_language = defaults.second_language;
        }
        if !self.general.first_language.is_language() {
            self.general.first_language = defaults.first_language;
        }
        if self.general.pair.to == LangId::UNKNOWN {
            self.general.pair.to = LangId::AUTO;
        }
    }

    /// Remember a direction as recently used.
    pub fn remember_pair(&mut self, pair: LangPair) {
        if !pair.from.is_language() || !pair.to.is_language() {
            return;
        }
        self.general.recent_pairs.retain(|p| *p != pair);
        self.general.recent_pairs.insert(0, pair);
        self.general.recent_pairs.truncate(12);
    }

    /// Choose a target for text detected as `detected`, the way smart
    /// detection does: the first language goes to the second, anything else
    /// comes home to the first.
    pub fn smart_target(&self, detected: LangId) -> LangId {
        if detected == self.general.first_language {
            self.general.second_language
        } else {
            self.general.first_language
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_theme_follows_the_system_for_a_fresh_configuration_only() {
        let fresh = Config::default();
        assert!(fresh.appearance.follow_system_theme);
        assert_eq!(fresh.appearance.effective_theme(Some(true)), fresh.appearance.theme_dark);
        assert_eq!(fresh.appearance.effective_theme(Some(false)), fresh.appearance.theme_light);
        // The mode cannot be told: the one chosen theme.
        assert_eq!(fresh.appearance.effective_theme(None), fresh.appearance.theme);
        // A file from before: the theme it chose, whatever the system does.
        let old = Config::from_toml("[appearance]\ntheme = \"Glossa Midnight\"\n").unwrap();
        assert!(!old.appearance.follow_system_theme);
        assert_eq!(old.appearance.effective_theme(Some(false)), "Glossa Midnight");
    }

    use super::*;

    #[test]
    fn an_empty_file_is_all_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c, {
            let mut d = Config::default();
            d.sanitise();
            d
        });
    }

    #[test]
    fn a_partial_file_keeps_defaults_for_the_rest() {
        let c = Config::from_toml(
            r#"
            [general]
            first_language = "uk"
            instant_translation = true
            [appearance.popup]
            transparency = 40
            "#,
        )
        .unwrap();
        assert_eq!(c.general.first_language, LangId::from_code("uk").unwrap());
        assert!(c.general.instant_translation);
        assert_eq!(c.appearance.popup.transparency, 40);
        // Untouched sibling fields keep their defaults.
        assert_eq!(c.general.second_language, LangId::ENGLISH);
        assert_eq!(c.appearance.popup.auto_hide_secs, 5);
    }

    #[test]
    fn a_broken_file_is_moved_aside_and_defaults_are_used() {
        let dir = std::env::temp_dir().join(format!("glossa-recover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[general\nbroken = ").unwrap();
        let (c, recovered) = Config::load_or_recover(&path).unwrap();
        assert_eq!(c, Config::default());
        let r = recovered.expect("reported");
        assert!(!path.exists(), "moved, so the next save starts afresh");
        assert_eq!(std::fs::read_to_string(&r.backup).unwrap(), "[general\nbroken = ", "kept verbatim");
        let name = r.backup.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("config.toml.broken-") && name[19..].parse::<u64>().is_ok(), "{name}");
        assert!(r.error.contains("config file"), "{}", r.error);
        // A good file, or none, is not a recovery.
        std::fs::write(&path, "[general]\ninstant_translation = true\n").unwrap();
        let (c, recovered) = Config::load_or_recover(&path).unwrap();
        assert!(c.general.instant_translation && recovered.is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(Config::load_or_recover(&path).unwrap().1.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_keys_are_ignored() {
        assert!(Config::from_toml("[general]\nfrom_the_future = 1\n[nope]\nx = 2").is_ok());
    }

    #[test]
    fn malformed_files_are_errors_not_silent_resets() {
        assert!(Config::from_toml("[general\nbroken").is_err());
        assert!(Config::from_toml("[network]\ntimeout_secs = \"ten\"").is_err());
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let c = Config::from_toml(
            "[network]\ntimeout_secs = 0\n[appearance.popup]\ntransparency = 250\n[appearance]\nui_scale = 99.0\nkeyboard_scale = 0.1",
        )
        .unwrap();
        assert_eq!(c.network.timeout_secs, 1);
        assert_eq!(c.appearance.popup.transparency, 90);
        assert_eq!(c.appearance.ui_scale, 3.0);
        assert_eq!(c.appearance.keyboard_scale, 0.5);
        let c = Config::from_toml(
            "[appearance]
text_scale = 0.01",
        )
        .unwrap();
        assert_eq!(c.appearance.text_scale, 0.5);
        assert_eq!(Config::from_toml("").unwrap().appearance.text_scale, 1.0, "absent means 100%");
        assert_eq!(clamp_text_scale(f32::NAN), 1.0);
    }

    #[test]
    fn toml_round_trip() {
        let mut c = Config::default();
        c.general.pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
        c.services.set_option("deepl", "api_key", serde_json::json!("secret"));
        c.hotkeys.bindings.insert(Action::WatchRegion, "Ctrl+Alt+W".parse().unwrap());
        c.windows
            .insert("main".into(), WindowGeometry { x: 10.0, y: 20.0, width: 640.0, height: 480.0, maximized: false });
        let back = Config::from_toml(&c.to_toml()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.services.option("deepl", "api_key"), Some(&serde_json::json!("secret")));
    }

    #[test]
    fn ocr_choices_load_by_name_and_default_when_absent() {
        let c = Config::from_toml(
            "[ocr]
provider = \"lens\"",
        )
        .unwrap();
        assert_eq!(c.ocr.after_select, OcrAfterSelect::Show);
        assert_eq!(c.ocr.translate_with, OcrTranslateWith::OcrEngine);
        assert!(!c.ocr.copy_joined_lines);
        let c = Config::from_toml(
            "[ocr]
after_select = \"ask\"
translate_with = \"translator\"
copy_joined_lines = true",
        )
        .unwrap();
        assert_eq!(c.ocr.after_select, OcrAfterSelect::Ask);
        assert_eq!(c.ocr.translate_with, OcrTranslateWith::Translator);
        assert!(c.ocr.copy_joined_lines);
    }

    #[test]
    fn save_and_load_through_the_filesystem() {
        let dir = std::env::temp_dir().join(format!("glossa-cfg-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        let mut c = Config::default();
        c.general.ui_language = "ru".into();
        c.save(&path).unwrap();
        assert!(!path.with_extension("toml.tmp").exists(), "temp file should be renamed away");
        assert_eq!(Config::load(&path).unwrap().general.ui_language, "ru");
        std::fs::remove_dir_all(&dir).ok();
        // Missing file: defaults, not an error.
        assert!(Config::load(&dir.join("missing.toml")).is_ok());
    }

    #[test]
    fn default_hotkeys_do_not_conflict_and_conflicts_are_found() {
        let mut h = HotkeysConfig::default();
        assert!(h.conflicts().is_empty());
        h.bindings.insert(Action::ShowHistory, "Ctrl+Shift+Q".parse().unwrap());
        let conflicts = h.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0.to_string(), "Ctrl+Shift+Q");
        assert!(conflicts[0].1.contains(&Action::ShowPopup));
        assert!(conflicts[0].1.contains(&Action::ShowHistory));
    }

    #[test]
    fn proxy_url_is_built_only_for_manual_mode_and_escapes_credentials() {
        let mut p = Proxy { mode: ProxyMode::Manual, scheme: ProxyScheme::Socks5, port: 2080, ..Default::default() };
        assert_eq!(p.url().as_deref(), Some("socks5://127.0.0.1:2080"));
        p.username = "me".into();
        p.password = "p@ss:w".into();
        assert_eq!(p.url().as_deref(), Some("socks5://me:p%40ss%3Aw@127.0.0.1:2080"));
        p.mode = ProxyMode::System;
        assert_eq!(p.url(), None);
    }

    #[test]
    fn ai_endpoint_ids_are_slugs_and_unique() {
        let mut s = Services::default();
        assert_eq!(s.new_ai_endpoint_id("DeepSeek (work)"), "ai-deepseek-work");
        assert_eq!(s.new_ai_endpoint_id("Ollama — локально"), "ai-ollama");
        assert_eq!(s.new_ai_endpoint_id("Модель"), "ai-endpoint");
        s.ai_endpoints.push(AiEndpoint { id: "ai-ollama".into(), name: "Ollama".into(), preset: "ollama".into() });
        assert_eq!(s.new_ai_endpoint_id("Ollama"), "ai-ollama-2");
    }

    #[test]
    fn default_hotkeys_avoid_altgr_and_common_shortcuts() {
        for os in [Os::Windows, Os::MacOs, Os::Unix] {
            let keys: Vec<(Action, Hotkey)> =
                default_hotkeys(os).iter().map(|(a, s)| (*a, s.parse::<Hotkey>().unwrap())).collect();
            assert!(!keys.iter().any(|(a, _)| *a == Action::ListenSelection), "{os:?}: no default for listen");
            assert!(!keys.iter().any(|(_, k)| k.to_string() == "Ctrl+E"), "{os:?}");
            let popup = keys.iter().find(|(a, _)| *a == Action::ShowPopup).unwrap();
            assert_eq!(popup.1.to_string(), "Ctrl+Shift+Q", "{os:?}: users rely on it");
            let mut seen = std::collections::HashSet::new();
            assert!(keys.iter().all(|(_, k)| seen.insert(k.to_string())), "{os:?}: no chord twice");
            if os == Os::Windows {
                // AltGr is Ctrl+Alt on Windows: Ctrl+Alt+<letter> would eat
                // AltGr+<letter> characters.
                for (a, k) in &keys {
                    assert!(!(k.mods.ctrl && k.mods.alt && !k.mods.meta), "{a:?}: {k} collides with AltGr");
                }
            }
        }
        let c = HotkeysConfig::default();
        assert_eq!(c.bindings.len(), default_hotkeys(Os::current()).len());
        assert!(c.conflicts().is_empty());
    }

    #[test]
    fn saved_hotkeys_are_kept_when_the_defaults_change() {
        let c = Config::from_toml("[hotkeys.bindings]\nlisten_selection = \"Ctrl+E\"\nshow_popup = \"Ctrl+Alt+Q\"\n")
            .unwrap();
        assert_eq!(c.hotkeys.bindings.len(), 2);
        assert_eq!(c.hotkeys.bindings[&Action::ListenSelection].to_string(), "Ctrl+E");
        assert_eq!(c.hotkeys.bindings[&Action::ShowPopup].to_string(), "Ctrl+Alt+Q");
    }

    #[test]
    fn terminals_are_recognised_by_program_or_any_class() {
        let a = Advanced::default();
        assert_eq!(a.terminal_copy_action, CopyAction::AccessibilityOnly, "no keystroke into a terminal");
        assert!(a.is_terminal("WindowsTerminal.exe", &["CASCADIA_HOSTING_WINDOW_CLASS", "", ""]));
        assert!(a.is_terminal("MINTTY.EXE", &["mintty", "", ""]), "program names ignore case");
        assert!(a.is_terminal("conhost.exe", &["ConsoleWindowClass"]));
        // VS Code: only its terminal, which UI Automation names by xterm.js's class.
        assert!(
            a.is_terminal("Code.exe", &["Chrome_WidgetWin_1", "Chrome_RenderWidgetHostHWND", "xterm-helper-textarea"])
        );
        assert!(!a.is_terminal("Code.exe", &["Chrome_WidgetWin_1", "Chrome_RenderWidgetHostHWND", "inputarea"]));
        assert!(!a.is_terminal("notepad.exe", &["Notepad", "Edit", ""]));
        assert!(!a.is_terminal("", &["", "", ""]));
        // A rule with neither field never matches everything.
        let mut b = a.clone();
        b.terminals.push(ExceptionRule { program: String::new(), window_class: String::new() });
        assert!(!b.is_terminal("notepad.exe", &["Notepad"]));
    }

    #[test]
    fn terminal_rules_reach_existing_configs_once() {
        // A file from before the rules existed gets all of them.
        let c = Config::from_toml("[advanced]\ncopy_action = \"ctrl_insert\"\n").unwrap();
        assert_eq!(c.advanced.terminals, Advanced::default().terminals);
        assert_eq!(c.advanced.copy_action, CopyAction::CtrlInsert, "the user's setting stays");
        assert_eq!(c.advanced.terminal_rules_version, latest_terminal_rules());

        // An older version with the user's own rules and one built-in rule
        // (with different case) gains the rest, without duplicates.
        let old = "[advanced]\nterminal_rules_version = 0\n\
                   [[advanced.terminals]]\nprogram = \"myterm.exe\"\n\
                   [[advanced.terminals]]\nprogram = \"MINTTY.EXE\"\n";
        let c = Config::from_toml(old).unwrap();
        assert_eq!(c.advanced.terminals[0].program, "myterm.exe");
        assert_eq!(c.advanced.terminals.iter().filter(|r| r.program.eq_ignore_ascii_case("mintty.exe")).count(), 1);
        assert_eq!(c.advanced.terminals.len(), Advanced::default().terminals.len() + 1);
        assert_eq!(c.advanced.terminal_rules_version, latest_terminal_rules());

        // Up to date and the user deleted some: they stay deleted, also
        // after a save and a reload.
        let mut c = Config::default();
        c.advanced.terminals.retain(|r| r.program != "putty.exe");
        let back = Config::from_toml(&c.to_toml()).unwrap();
        assert!(!back.advanced.terminals.iter().any(|r| r.program == "putty.exe"));
        assert_eq!(back.advanced.terminals, c.advanced.terminals);
    }

    #[test]
    fn macos_drops_the_untouched_windows_exceptions() {
        assert!(Exceptions::default_for(Os::MacOs).rules.is_empty());
        let mut stock = Exceptions::default_for(Os::Windows);
        stock.adopt_platform_defaults(Os::MacOs);
        assert!(stock.rules.is_empty());
        // Kept elsewhere, and kept on macOS once the user changed the list.
        let mut win = Exceptions::default_for(Os::Windows);
        win.adopt_platform_defaults(Os::Windows);
        assert_eq!(win, Exceptions::default_for(Os::Windows));
        let mut edited = Exceptions::default_for(Os::Windows);
        edited.rules.pop();
        let before = edited.clone();
        edited.adopt_platform_defaults(Os::MacOs);
        assert_eq!(edited, before);
    }

    #[test]
    fn exceptions_follow_the_classic_rules() {
        let e = Exceptions::default_for(Os::Windows);
        // Disabled for list views in any program, and for all of mstsc.
        assert!(!e.allows("explorer.exe", "SysListView32"));
        assert!(!e.allows("mstsc.exe", "TscShellContainerClass"));
        assert!(e.allows("notepad.exe", "Edit"));
        // Program names compare case-insensitively, as Windows does.
        assert!(!e.allows("MSTSC.EXE", "anything"));

        let only = Exceptions {
            mode: ExceptionMode::EnabledFor,
            rules: vec![ExceptionRule { program: "firefox.exe".into(), window_class: String::new() }],
            smart_detection: false,
        };
        assert!(only.allows("firefox.exe", "MozillaWindowClass"));
        assert!(!only.allows("notepad.exe", "Edit"));
    }

    #[test]
    fn control_rules_and_smart_detection_look_at_the_focused_control() {
        // The top-level window of Explorer is CabinetWClass; the file list
        // inside it is what the default SysListView32 rule is about.
        let mut e = Exceptions::default_for(Os::Windows);
        assert!(!e.allows_window("explorer.exe", "CabinetWClass", "SysListView32"));
        assert!(e.allows_window("explorer.exe", "CabinetWClass", "Edit"), "the address bar is fine");

        // Smart detection covers controls with no rule of their own.
        e.rules.clear();
        assert!(!e.allows_window("app.exe", "AppWindow", "msctls_trackbar32"));
        assert!(e.allows_window("app.exe", "AppWindow", "RichEdit20W"));
        assert!(!e.allows_window("explorer.exe", "CabinetWClass", "DirectUIHWND"), "Explorer's file view");
        e.smart_detection = false;
        assert!(e.allows_window("app.exe", "AppWindow", "msctls_trackbar32"));
    }

    #[test]
    fn smart_direction_sends_the_first_language_to_the_second() {
        let c = Config::default(); // first = ru, second = en
        assert_eq!(c.smart_target(LangId::RUSSIAN), LangId::ENGLISH);
        assert_eq!(c.smart_target(LangId::ENGLISH), LangId::RUSSIAN);
        // A third language comes home to the first.
        assert_eq!(c.smart_target(LangId::from_code("de").unwrap()), LangId::RUSSIAN);
    }

    #[test]
    fn recent_pairs_are_most_recent_first_and_unique() {
        let mut c = Config::default();
        let de_en = LangPair::new(LangId::from_code("de").unwrap(), LangId::ENGLISH);
        c.remember_pair(de_en);
        c.remember_pair(LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));
        c.remember_pair(de_en);
        assert_eq!(c.general.recent_pairs[0], de_en);
        assert_eq!(c.general.recent_pairs.iter().filter(|p| **p == de_en).count(), 1);
        // Pairs involving auto are not remembered.
        let before = c.general.recent_pairs.clone();
        c.remember_pair(LangPair::new(LangId::AUTO, LangId::ENGLISH));
        assert_eq!(c.general.recent_pairs, before);
    }

    #[test]
    fn invalid_languages_fall_back_to_the_defaults_and_recent_pairs_are_unique() {
        let c = Config::from_toml("[general]\nfirst_language = 1\nsecond_language = 1\n").unwrap();
        assert_eq!(c.general.first_language, LangId::RUSSIAN, "the default, not English");
        assert_eq!(c.general.second_language, LangId::ENGLISH);

        let c = Config::from_toml(
            "[[general.recent_pairs]]\nfrom = 46\nto = 17\n[[general.recent_pairs]]\nfrom = 17\nto = 46\n\
             [[general.recent_pairs]]\nfrom = 46\nto = 17\n",
        )
        .unwrap();
        assert_eq!(
            c.general.recent_pairs,
            vec![LangPair::new(LangId::RUSSIAN, LangId::ENGLISH), LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)],
            "repeats anywhere go, the first stays"
        );
        let mut c = Config::default();
        c.general.recent_pairs = (0..40).map(|i| LangPair::new(LangId(10 + i), LangId::ENGLISH)).collect();
        c.sanitise();
        assert_eq!(c.general.recent_pairs.len(), 12);
    }

    #[test]
    fn renderer_reads_every_spelling_and_writes_the_old_one() {
        for (text, want) in [
            ("open_gl", Renderer::OpenGl),
            ("glow", Renderer::OpenGl),
            ("opengl", Renderer::OpenGl),
            ("gpu", Renderer::Gpu),
            ("wgpu", Renderer::Gpu),
            ("software", Renderer::Software),
        ] {
            let c = Config::from_toml(&format!("[appearance]\nrenderer = \"{text}\"")).unwrap();
            assert_eq!(c.appearance.renderer, want, "{text}");
            assert_eq!(Renderer::from_name(text), Some(want), "{text}");
        }
        assert!(Config::default().to_toml().contains("renderer = \"open_gl\""));
        assert_eq!(Renderer::from_name("GLOW"), Some(Renderer::OpenGl));
        assert_eq!(Renderer::from_name("vulkan"), None);
    }

    #[test]
    fn dbus_defaults_on() {
        assert!(Config::from_toml("").unwrap().server.dbus);
        assert!(!Config::from_toml("[server]\ndbus = false").unwrap().server.dbus);
    }

    #[test]
    fn pot_routes_need_the_token_unless_switched_on() {
        assert!(!Config::from_toml("").unwrap().server.pot_routes_without_token);
        assert!(
            Config::from_toml("[server]\npot_routes_without_token = true").unwrap().server.pot_routes_without_token
        );
        // The old key was on by default and written into every file, so it
        // cannot tell a choice from the default: it opens nothing any more.
        let old = Config::from_toml("[server]\npot_compat_without_token = true").unwrap();
        assert!(!old.server.pot_routes_without_token);
        assert!(!old.to_toml().contains("pot_compat_without_token"), "and is dropped on the next save");
    }

    #[test]
    fn backdrops_default_to_none_and_read_their_names() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.appearance.backdrop, Backdrop::None);
        assert_eq!(c.appearance.popup.backdrop, Backdrop::None);
        assert_eq!(c.appearance.popup.shade, 0);

        let c = Config::from_toml(
            "[appearance]\nbackdrop = \"mica\"\n[appearance.popup]\nbackdrop = \"acrylic\"\nshade = 200\n",
        )
        .unwrap();
        assert_eq!(c.appearance.backdrop, Backdrop::Mica);
        assert_eq!(c.appearance.popup.backdrop, Backdrop::Acrylic);
        assert_eq!(c.appearance.popup.shade, 90, "clamped like the transparency");

        for (text, want) in [("micaalt", Backdrop::MicaAlt), ("mica_alt", Backdrop::MicaAlt), ("blur", Backdrop::Blur)]
        {
            let c = Config::from_toml(&format!("[appearance]\nbackdrop = \"{text}\"")).unwrap();
            assert_eq!(c.appearance.backdrop, want, "{text}");
        }
        assert!(Config::from_toml("[appearance]\nbackdrop = \"glass\"").is_err());
    }

    #[test]
    fn backdrop_names_round_trip() {
        for b in Backdrop::ALL {
            assert_eq!(b.as_str().parse::<Backdrop>().unwrap(), b);
            let mut c = Config::default();
            c.appearance.backdrop = b;
            c.appearance.popup.backdrop = b;
            let back = Config::from_toml(&c.to_toml()).unwrap();
            assert_eq!(back.appearance.backdrop, b);
            assert_eq!(back.appearance.popup.backdrop, b);
        }
        assert_eq!("Mica Alt".parse::<Backdrop>().unwrap(), Backdrop::MicaAlt);
        assert!(Backdrop::Acrylic.is_tinted() && !Backdrop::Blur.is_tinted() && !Backdrop::None.is_some());
    }

    #[test]
    fn popup_shade_darkens_only_without_a_backdrop() {
        let back = Color::rgba(0xf0, 0xf0, 0xf0, 0x80);
        let mut p = PopupConfig::default();
        assert_eq!(p.shaded(back, false), back, "no shade by default");
        p.shade = 50;
        assert_eq!(p.shaded(back, true), back, "a real blur needs no shade");
        assert_eq!(p.shaded(back, false), Color::rgba(0x78, 0x78, 0x78, 0xc0));
    }

    #[test]
    fn the_tray_hint_is_for_a_first_run_only() {
        // No file yet: the first run shows it.
        assert!(Config::default().general.show_tray_hint);
        // A file from before the hint existed: its user knows the tray.
        let old = Config::from_toml("[general]\nfirst_language = 17\n").unwrap();
        assert!(!old.general.show_tray_hint);
        // Put away, it stays away.
        let mut c = Config::default();
        c.general.show_tray_hint = false;
        assert!(!Config::from_toml(&c.to_toml()).unwrap().general.show_tray_hint);
    }

    #[test]
    fn popup_opacity_and_legacy_icon_mask() {
        let p = PopupConfig { transparency: 15, ..Default::default() };
        assert!((p.opacity() - 0.85).abs() < 1e-6);
        assert_eq!(PopupIcons::from_legacy(30), PopupIcons::default());
        let some = PopupIcons::from_legacy(2 | 8);
        assert!(some.dictionary && some.copy && !some.listen && !some.replace);
    }

    /// `docs/configuration.md` describes every key `config.toml` can hold.
    /// A new setting fails this test until it is documented there.
    ///
    /// The page is split at headings whose first code span is a table name
    /// (`` ### `[general]` ``, `` ### `[[exceptions.rules]]` ``). A key must
    /// appear as `` `key` `` in its table's section; a nested table or array
    /// of tables either has a heading of its own or is described, with its
    /// keys, in its parent's section.
    #[test]
    fn every_config_key_is_documented() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/configuration.md");
        let doc = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut sections: BTreeMap<String, String> = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in doc.lines() {
            if line.starts_with('#') {
                current = line
                    .split('`')
                    .nth(1)
                    .filter(|s| s.starts_with('['))
                    .map(|s| s.trim_matches(|c| c == '[' || c == ']').to_owned());
                if let Some(name) = &current {
                    sections.entry(name.clone()).or_default();
                }
            } else if let Some(name) = &current {
                let text = sections.get_mut(name).expect("added at the heading");
                text.push_str(line);
                text.push('\n');
            }
        }

        // A config with every optional value set and one entry in every
        // list and map, so that each key is written out.
        let mut c = Config::default();
        c.classic.path = Some(PathBuf::from("classic"));
        c.ocr.save_image_dir = Some(PathBuf::from("shots"));
        c.appearance.popup.back = Some(Color::BLACK);
        c.appearance.popup.text = Some(Color::WHITE);
        c.appearance.popup.frame = Some(Color::BLACK);
        c.windows.insert("main".into(), WindowGeometry { x: 0.0, y: 0.0, width: 1.0, height: 1.0, maximized: false });
        c.services.ai_endpoints.push(AiEndpoint { id: "ai-x".into(), name: "X".into(), preset: "custom".into() });
        c.services.set_option("deepl", "api_key", serde_json::json!("fake"));
        c.dictionary.offline.push(PathBuf::from("dict.ifo"));
        c.modules.dirs.push(PathBuf::from("modules"));
        c.languages.disabled.push(LangId::RUSSIAN);
        c.services.preset = "Work".into();
        let one = vec!["google".to_owned()];
        c.presets.push(crate::preset::Preset {
            name: "Work".into(),
            order: one.clone(),
            enabled: one.clone(),
            active: "google".into(),
            dictionary_order: one.clone(),
            dictionary_enabled: one,
            dictionary_active: "google".into(),
            tts: "google".into(),
            ocr: "lens".into(),
            pair: Some(LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)),
            detect: Some(DetectMode::Offline),
            options: [("google".to_owned(), [("domain".to_owned(), serde_json::json!("com"))].into_iter().collect())]
                .into_iter()
                .collect(),
        });
        let root: toml::Table = toml::from_str(&c.to_toml()).expect("config serialises to a table");

        fn documented(sections: &BTreeMap<String, String>, section: &str, key: &str) -> bool {
            sections.get(section).is_some_and(|text| text.contains(&format!("`{key}`")))
        }

        fn check(
            table: &toml::Table,
            path: &str,
            section: &str,
            sections: &BTreeMap<String, String>,
            missing: &mut Vec<String>,
        ) {
            for (key, value) in table {
                let child = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                let nested: Vec<&toml::Table> = match value {
                    toml::Value::Table(t) => vec![t],
                    toml::Value::Array(items) => items.iter().filter_map(toml::Value::as_table).collect(),
                    _ => Vec::new(),
                };
                let has_heading = !nested.is_empty() && sections.contains_key(&child);
                if !has_heading && !documented(sections, section, key) {
                    missing.push(child.clone());
                }
                let sub = if has_heading { child.as_str() } else { section };
                match child.as_str() {
                    // Maps keyed by user data: the keys are actions, module
                    // ids and option names, not settings.
                    "hotkeys.bindings" | "hotkeys.panels" | "services.options" | "presets.options" => {}
                    // Keyed by window name; each value is a geometry.
                    "windows" => {
                        for geometry in nested.iter().flat_map(|t| t.values()).filter_map(toml::Value::as_table) {
                            check(geometry, &child, sub, sections, missing);
                        }
                    }
                    _ => {
                        for t in nested {
                            check(t, &child, sub, sections, missing);
                        }
                    }
                }
            }
        }

        let mut missing = Vec::new();
        check(&root, "", "", &sections, &mut missing);
        missing.dedup();
        assert!(missing.is_empty(), "config keys not described in docs/configuration.md: {missing:?}");
    }

    #[test]
    fn tray_clicks_default_to_what_the_icon_did_before() {
        let win = TrayConfig::for_os(Os::Windows);
        assert_eq!(win.click, TrayAction::Run(Action::ToggleMouseMode));
        assert_eq!(win.double_click, TrayAction::Run(Action::ShowMainWindow));
        assert_eq!(win.middle_click, TrayAction::Nothing);
        assert_eq!(TrayConfig::for_os(Os::Unix), win);
        let mac = TrayConfig::for_os(Os::MacOs);
        assert_eq!(mac.click, TrayAction::Menu);
        assert_eq!(mac.double_click, TrayAction::Nothing);
        assert_eq!(Config::default().tray, TrayConfig::for_os(Os::current()));
    }

    #[test]
    fn tray_actions_are_written_as_ids() {
        let c = Config::from_toml(
            "[tray]\nclick = \"text_recognition\"\ndouble_click = \"menu\"\nmiddle_click = \"none\"\n",
        )
        .unwrap();
        assert_eq!(c.tray.click, TrayAction::Run(Action::TextRecognition));
        assert_eq!(c.tray.double_click, TrayAction::Menu);
        assert_eq!(c.tray.middle_click, TrayAction::Nothing);
        let again = Config::from_toml(&c.to_toml()).unwrap();
        assert_eq!(again.tray, c.tray);
    }

    #[test]
    fn a_tray_value_that_names_nothing_falls_back_to_its_default() {
        // The rest of the file still loads.
        let c = Config::from_toml(
            "[general]\ninstant_translation = true\n[tray]\nclick = \"fly\"\nmiddle_click = \"show_history\"\n",
        )
        .unwrap();
        assert!(c.general.instant_translation);
        assert_eq!(c.tray.click, TrayConfig::default().click);
        assert_eq!(c.tray.middle_click, TrayAction::Run(Action::ShowHistory));
        let t = TrayConfig::from_ids([Some("text_recognition"), Some("nope"), None], Os::Windows);
        assert_eq!(t.click, TrayAction::Run(Action::TextRecognition));
        assert_eq!(t.double_click, TrayAction::Run(Action::ShowMainWindow));
        // A click set to open the menu takes the double click's default
        // with it, on any system.
        let m = TrayConfig::from_ids([Some("menu"), None, None], Os::Windows);
        assert_eq!(m.double_click, TrayAction::Nothing);
    }

    #[test]
    fn the_classic_switch_is_imported_both_ways() {
        let mut t = TrayConfig::for_os(Os::Windows);
        t.import_classic_switch(false, Os::Windows);
        assert_eq!(t.click, TrayAction::Run(Action::ShowMainWindow));
        t.import_classic_switch(true, Os::Windows);
        assert_eq!(t.click, TrayAction::Run(Action::ToggleMouseMode));
        let mut m = TrayConfig::for_os(Os::MacOs);
        m.import_classic_switch(false, Os::MacOs);
        assert_eq!(m.click, TrayAction::Menu);
    }

    #[test]
    fn the_old_tray_click_switch_moves_into_the_tray_section() {
        let off = Config::from_toml("[mouse_mode]\ntray_click_toggles = false\n").unwrap();
        let expected =
            if Os::current() == Os::MacOs { TrayAction::Menu } else { TrayAction::Run(Action::ShowMainWindow) };
        assert_eq!(off.tray.click, expected);
        assert_eq!(off.mouse_mode.tray_click_toggles, None);
        // Never written back.
        assert!(!off.to_toml().contains("tray_click_toggles"));
        // On: the default, nothing to change.
        let on = Config::from_toml("[mouse_mode]\ntray_click_toggles = true\n").unwrap();
        assert_eq!(on.tray, TrayConfig::default());
        // A click the file chooses itself wins over the old switch.
        let both =
            Config::from_toml("[mouse_mode]\ntray_click_toggles = false\n[tray]\nclick = \"show_history\"\n").unwrap();
        assert_eq!(both.tray.click, TrayAction::Run(Action::ShowHistory));
    }

    #[test]
    fn the_old_switch_is_read_per_system() {
        let mut t = TrayConfig::for_os(Os::Windows);
        t.adopt_tray_click_toggles(Some(false), Os::Windows);
        assert_eq!(t.click, TrayAction::Run(Action::ShowMainWindow));
        let mut m = TrayConfig::for_os(Os::MacOs);
        m.adopt_tray_click_toggles(Some(false), Os::MacOs);
        assert_eq!(m.click, TrayAction::Menu);
        let mut n = TrayConfig::for_os(Os::Unix);
        n.adopt_tray_click_toggles(None, Os::Unix);
        assert_eq!(n, TrayConfig::for_os(Os::Unix));
    }

    #[test]
    fn renderer_reads_the_rarer_spellings_from_the_file_and_the_environment() {
        // The file's aliases (serde) and `GLOSSA_RENDERER` (from_name) are
        // separate lists; both must keep `gl` for files written by hand.
        let c = Config::from_toml("[appearance]\nrenderer = \"gl\"").unwrap();
        assert_eq!(c.appearance.renderer, Renderer::OpenGl);
        for (name, want) in [
            ("gl", Renderer::OpenGl),
            ("open-gl", Renderer::OpenGl),
            ("open_gl", Renderer::OpenGl),
            ("  Gpu ", Renderer::Gpu),
            ("WGPU", Renderer::Gpu),
            ("cpu", Renderer::Software),
            ("warp", Renderer::Software),
            ("Software", Renderer::Software),
        ] {
            assert_eq!(Renderer::from_name(name), Some(want), "{name:?}");
        }
        assert_eq!(Renderer::from_name(""), None);
        // Every name the program writes reads back as the same renderer.
        for r in [Renderer::OpenGl, Renderer::Gpu, Renderer::Software] {
            assert_eq!(Renderer::from_name(r.as_str()), Some(r));
            let mut c = Config::default();
            c.appearance.renderer = r;
            assert_eq!(Config::from_toml(&c.to_toml()).unwrap().appearance.renderer, r);
        }
    }

    #[test]
    fn mica_alt_loads_by_its_hyphenated_alias_and_is_written_without_it() {
        let c = Config::from_toml("[appearance.popup]\nbackdrop = \"mica-alt\"").unwrap();
        assert_eq!(c.appearance.popup.backdrop, Backdrop::MicaAlt);
        assert!(c.to_toml().contains("backdrop = \"micaalt\""), "older builds read only the canonical name");
        // The lenient parser takes the same spellings as the file.
        for text in ["mica-alt", "MICA_ALT", " micaalt "] {
            assert_eq!(text.parse::<Backdrop>(), Ok(Backdrop::MicaAlt), "{text:?}");
        }
        assert!("glass".parse::<Backdrop>().unwrap_err().contains("glass"), "the error names the bad value");
    }

    #[test]
    fn every_enum_setting_loads_by_its_documented_name() {
        let c = Config::from_toml(
            r#"
            [general]
            results_layout = "stacked"
            startup_action = "restore_previous"
            secret_storage = "system"
            [network.proxy]
            mode = "auto"
            scheme = "socks4"
            [exceptions]
            mode = "enabled_for"
            [advanced]
            copy_action = "accessibility_only"
            layout_indicator = "caret"
            terminal_copy_action = "ctrl_c"
            [mouse_mode]
            behaviour = "show_translation_and_read"
            [ocr]
            after_select = "in_place"
            [ocr.vision]
            level = "fast"
            [ocr.lens]
            mode = "daemon"
            "#,
        )
        .unwrap();
        assert_eq!(c.general.results_layout, ResultsLayout::Stacked);
        assert_eq!(c.general.startup_action, StartupAction::RestorePrevious);
        assert_eq!(c.general.secret_storage, crate::secrets::SecretStorage::System);
        assert_eq!(c.network.proxy.mode, ProxyMode::Auto);
        assert_eq!(c.network.proxy.scheme, ProxyScheme::Socks4);
        assert_eq!(c.exceptions.mode, ExceptionMode::EnabledFor);
        assert_eq!(c.advanced.copy_action, CopyAction::AccessibilityOnly);
        assert_eq!(c.advanced.layout_indicator, LayoutIndicator::Caret);
        assert_eq!(c.advanced.terminal_copy_action, CopyAction::CtrlC);
        assert_eq!(c.mouse_mode.behaviour, MouseBehaviour::ShowTranslationAndRead);
        assert_eq!(c.ocr.after_select, OcrAfterSelect::InPlace);
        assert_eq!(c.ocr.vision.level, VisionLevel::Fast);
        assert_eq!(c.ocr.lens.mode, LensMode::Daemon);
        // And the whole lot survives a save.
        assert_eq!(Config::from_toml(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn an_unknown_enum_name_is_an_error_not_a_default() {
        // A typo must not silently turn a setting back to its default.
        for text in [
            "[general]\nresults_layout = \"grid\"",
            "[network.proxy]\nmode = \"pac\"",
            "[network.proxy]\nscheme = \"SOCKS5\"",
            "[advanced]\ncopy_action = \"ctrl-c\"",
            "[ocr.lens]\nmode = \"web\"",
        ] {
            assert!(Config::from_toml(text).is_err(), "{text}");
        }
    }

    #[test]
    fn fields_added_later_take_their_own_defaults_inside_a_present_table() {
        // A table written before these keys existed: each falls back to its
        // own default, not to the type's zero.
        let c = Config::from_toml(
            "[appearance]\ntheme = \"Glossa Dark\"\n[appearance.popup]\ntransparency = 10\n\
             [general]\nui_language = \"ru\"\n",
        )
        .unwrap();
        assert_eq!(c.appearance.backdrop_opacity, 100, "full_percent, not 0");
        assert_eq!(c.appearance.popup.backdrop_opacity, 100);
        assert_eq!(c.appearance.popup.icon_scale, 1.0, "one, not 0");
        assert_eq!(c.appearance.popup.shade, 0);
        assert!(!c.general.show_welcome, "files from before the guide skip it");
        assert_eq!(c.appearance.popup.max_width, 600.0, "a sibling key keeps the struct default");
        assert_eq!(c.ocr.tesseract.psm, None);
    }

    #[test]
    fn rules_and_endpoints_load_with_their_optional_fields_absent() {
        let c = Config::from_toml(
            "[[exceptions.rules]]\nwindow_class = \"Edit\"\n\
             [[services.ai_endpoints]]\nid = \"ai-x\"\nname = \"X\"\n",
        )
        .unwrap();
        assert_eq!(c.exceptions.rules, vec![ExceptionRule { program: String::new(), window_class: "Edit".into() }]);
        assert_eq!(c.services.ai_endpoints[0].preset, "", "preset is optional");
        // An endpoint without its id cannot be used, so it is an error.
        assert!(Config::from_toml("[[services.ai_endpoints]]\nname = \"X\"\n").is_err());
    }

    #[test]
    fn window_geometry_maximised_is_optional_and_the_rest_is_required() {
        let c = Config::from_toml("[windows.main]\nx = 1.0\ny = 2.0\nwidth = 300.0\nheight = 200.0\n").unwrap();
        assert!(!c.windows["main"].maximized);
        assert!(Config::from_toml("[windows.main]\nx = 1.0\ny = 2.0\n").is_err());
    }

    #[test]
    fn languages_load_by_number_or_by_code() {
        let c = Config::from_toml(
            "[general]\nfirst_language = \"de\"\n[general.pair]\nfrom = \"en\"\nto = 46\n\
             [languages]\ndisabled = [\"fr\", 17]\n",
        )
        .unwrap();
        assert_eq!(c.general.first_language, LangId::from_code("de").unwrap());
        assert_eq!(c.general.pair, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
        assert!(!c.languages.is_enabled(LangId::from_code("fr").unwrap()));
        assert!(!c.languages.is_enabled(LangId::ENGLISH));
        assert!(c.languages.is_enabled(LangId::RUSSIAN));
        assert!(Config::from_toml("[general]\nfirst_language = \"klingon\"").is_err());
        // Written back as numbers, which every build reads.
        assert!(c.to_toml().contains("first_language = "));
        assert_eq!(Config::from_toml(&c.to_toml()).unwrap().general.pair, c.general.pair);
    }

    #[test]
    fn an_unknown_target_language_becomes_auto() {
        let c = Config::from_toml("[general.pair]\nfrom = 17\nto = 0\n").unwrap();
        assert_eq!(c.general.pair.to, LangId::AUTO);
        assert_eq!(c.general.pair.from, LangId::ENGLISH);
    }

    #[test]
    fn popup_widths_are_clamped_against_each_other() {
        // A minimum above the maximum would make the popup's size range
        // empty; the minimum gives way.
        let c = Config::from_toml("[appearance.popup]\nmin_width = 900.0\nmax_width = 500.0\n").unwrap();
        assert_eq!(c.appearance.popup.max_width, 500.0);
        assert_eq!(c.appearance.popup.min_width, 500.0);
        let c = Config::from_toml(
            "[appearance.popup]\nmin_width = 1.0\nmax_width = 99999.0\nmax_height = 1.0\nframe_thickness = -3.0\n\
             auto_hide_secs = 100000\n",
        )
        .unwrap();
        let p = &c.appearance.popup;
        assert_eq!((p.min_width, p.max_width, p.max_height), (80.0, 4000.0, 80.0));
        assert_eq!(p.frame_thickness, 0.0);
        assert_eq!(p.auto_hide_secs, 600);
    }

    #[test]
    fn every_other_number_is_clamped_on_load() {
        let c = Config::from_toml(
            "[network]\ntimeout_secs = 100000\nmax_retries = 50\n\
             [appearance]\ntext_size = 1.0\nicon_scale = 10.0\nwindow_transparency = 255\n\
             [dictionary]\nzoom = 5\n[general]\ninstant_delay_ms = 1\n[history]\nmax_entries = 0\n\
             [modules]\ncall_timeout_ms = 1\n[ocr]\nwatch_interval_ms = 999999999\n\
             [advanced]\ncopy_wait_ms = 0\n",
        )
        .unwrap();
        assert_eq!(c.network.timeout_secs, 300);
        assert_eq!(c.network.max_retries, 5);
        assert_eq!(c.appearance.text_size, 6.0);
        assert_eq!(c.appearance.icon_scale, 3.0);
        assert_eq!(c.appearance.window_transparency, 90);
        assert_eq!(c.dictionary.zoom, 50);
        assert_eq!(c.general.instant_delay_ms, 150);
        assert_eq!(c.history.max_entries, 10, "0 would keep nothing");
        assert_eq!(c.modules.call_timeout_ms, 1000);
        assert_eq!(c.ocr.watch_interval_ms, 60_000);
        assert_eq!(c.advanced.copy_wait_ms, 20);
        // In range: untouched.
        assert_eq!(Config::from_toml("[dictionary]\nzoom = 150").unwrap().dictionary.zoom, 150);
    }

    #[test]
    fn a_saved_default_config_loads_as_itself() {
        // What a first run writes must read back unchanged, or every start
        // would rewrite (and possibly migrate) the user's file.
        let mut c = Config::default();
        c.sanitise();
        assert_eq!(Config::from_toml(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn services_sort_by_the_users_order_and_keep_the_rest_stable() {
        let s = Services { order: vec!["deepl".into(), "google".into()], ..Default::default() };
        assert_eq!(s.sorted(["yandex", "google", "bing", "deepl"]), ["deepl", "google", "yandex", "bing"]);
        assert!(s.sorted([]).is_empty());
        assert_eq!(s.option("deepl", "api_key"), None);
    }

    #[test]
    fn ai_endpoint_ids_avoid_option_keys_of_removed_endpoints() {
        // Options left behind by a removed endpoint must not be inherited by
        // a new one with the same name.
        let mut s = Services::default();
        s.set_option("ai-local", "model", serde_json::json!("old"));
        assert_eq!(s.new_ai_endpoint_id("Local"), "ai-local-2");
        assert_eq!(s.new_ai_endpoint_id("--"), "ai-endpoint");
    }

    #[test]
    fn a_proxy_without_a_host_or_a_username_has_no_credentials_in_its_url() {
        let mut p = Proxy { mode: ProxyMode::Manual, host: "   ".into(), ..Default::default() };
        assert_eq!(p.url(), None, "a blank host is not a proxy");
        p.host = " proxy.lan ".into();
        p.password = "unused".into();
        assert_eq!(p.url().as_deref(), Some("http://proxy.lan:8080"), "no user: no password either");
        p.mode = ProxyMode::Auto;
        assert_eq!(p.url(), None, "auto is the system proxy");
        p.mode = ProxyMode::None;
        assert_eq!(p.url(), None);
    }

    #[test]
    fn exception_rules_match_program_names_loosely_and_classes_exactly() {
        let r = ExceptionRule { program: "Game.exe".into(), window_class: "UnityWndClass".into() };
        assert!(r.matches("game.EXE", "UnityWndClass"));
        assert!(!r.matches("game.exe", "unitywndclass"), "classes are case-sensitive, as Windows reports them");
        assert!(!r.matches("other.exe", "UnityWndClass"));
        let any = ExceptionRule { program: String::new(), window_class: String::new() };
        assert!(any.matches("x", "y"));
    }

    #[test]
    fn enabled_for_mode_also_honours_the_focused_controls_class() {
        let only = Exceptions {
            mode: ExceptionMode::EnabledFor,
            rules: vec![ExceptionRule { program: String::new(), window_class: "Edit".into() }],
            smart_detection: true,
        };
        assert!(only.allows_window("app.exe", "Main", "Edit"), "the focused Edit is listed");
        assert!(!only.allows_window("app.exe", "Main", "Static"));
        // Smart detection refuses a list even where the user enabled it.
        let mut list = only.clone();
        list.rules[0].window_class = "SysListView32".into();
        assert!(!list.allows_window("app.exe", "Main", "SysListView32"));
        // Explorer's view is a non-text control only in Explorer.
        let e = Exceptions { rules: Vec::new(), ..Exceptions::default_for(Os::Windows) };
        assert!(e.allows_window("someapp.exe", "Main", "DirectUIHWND"));
        assert!(!e.allows_window("EXPLORER.EXE", "Main", "DirectUIHWND"));
    }

    #[test]
    fn popup_opacity_never_reaches_invisible() {
        let p = PopupConfig { transparency: 250, ..Default::default() };
        assert!((p.opacity() - 0.1).abs() < 1e-6, "an unsanitised value still leaves 10%");
        assert_eq!(PopupConfig { transparency: 0, ..Default::default() }.opacity(), 1.0);
        assert_eq!(
            PopupIcons::from_legacy(0),
            PopupIcons { dictionary: false, listen: false, copy: false, replace: false }
        );
        assert_eq!(PopupIcons::from_legacy(1 | 32), PopupIcons::from_legacy(0), "bits outside the four are ignored");
    }

    #[test]
    fn popup_shade_is_capped_and_keeps_an_opaque_fill_opaque() {
        let mut p = PopupConfig { shade: 200, ..Default::default() };
        let opaque = Color::rgba(200, 100, 50, 255);
        let s = p.shaded(opaque, false);
        assert_eq!(s.a, 255);
        // Capped at 90%: some of the colour is left.
        assert_eq!((s.r, s.g, s.b), (20, 10, 5));
        p.shade = 90;
        assert_eq!(p.shaded(opaque, false), s);
    }

    #[test]
    fn effective_theme_ignores_the_system_when_following_is_off() {
        let a = Appearance { follow_system_theme: false, theme: "Mine".into(), ..Default::default() };
        assert_eq!(a.effective_theme(Some(true)), "Mine");
        assert_eq!(a.effective_theme(Some(false)), "Mine");
    }

    #[test]
    fn classic_install_parts_need_the_folder_and_the_switch() {
        let dir = std::env::temp_dir().join(format!("glossa-classic-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("Services")).unwrap();
        assert!(!ClassicInstall::looks_like_install(&dir), "Services alone is not enough");
        std::fs::create_dir_all(dir.join("Themes")).unwrap();
        assert!(ClassicInstall::looks_like_install(&dir));

        let mut c = ClassicInstall { path: Some(dir.clone()), ..Default::default() };
        assert_eq!(c.services_dir(), Some(dir.join("Services")));
        assert_eq!(c.themes_dir(), Some(dir.join("Themes")));
        assert_eq!(c.locales_dir(), None, "no Locales folder");
        c.use_services = false;
        assert_eq!(c.services_dir(), None, "switched off");
        assert_eq!(ClassicInstall::default().themes_dir(), None, "no path");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn terminal_rule_migration_is_idempotent() {
        let mut a = Advanced { terminals: Vec::new(), terminal_rules_version: 0, ..Default::default() };
        a.migrate_terminal_rules();
        let once = a.clone();
        a.migrate_terminal_rules();
        assert_eq!(a, once);
        assert_eq!(a.terminals, Advanced::default().terminals);
        // A version from the future is kept, not lowered.
        let mut future = Advanced { terminal_rules_version: 99, terminals: Vec::new(), ..Default::default() };
        future.migrate_terminal_rules();
        assert_eq!(future.terminal_rules_version, 99);
        assert!(future.terminals.is_empty());
    }
}
