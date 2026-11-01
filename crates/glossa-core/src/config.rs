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
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        let mut bind = |a: Action, s: &str| {
            bindings.insert(a, s.parse().expect("built-in hotkey must parse"));
        };
        bind(Action::ShowPopup, "Ctrl+Shift+Q");
        bind(Action::ListenSelection, "Ctrl+E");
        bind(Action::TextRecognition, "Ctrl+Shift+O");
        bind(Action::RecognizeAndTranslate, "Ctrl+Alt+O");
        bind(Action::ShowMainWindow, "Ctrl+Alt+Q");
        Self { enabled: true, bindings }
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
}

impl Default for Network {
    fn default() -> Self {
        Self { timeout_secs: 10, proxy: Proxy::default(), user_agent: String::new(), max_retries: 1 }
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
    /// Theme name, as listed in the theme picker.
    pub theme: String,
    /// Draw our own window frame in theme colours rather than the system one.
    pub custom_frame: bool,
    /// UI font face, or empty for the theme's.
    pub font_family: String,
    /// UI scale, 1.0 is 100%.
    pub ui_scale: f32,
    /// Size of the text in the source and translation panes, in points.
    pub text_size: f32,
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
    pub popup: PopupConfig,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    /// OpenGL. The lightest in practice.
    #[default]
    OpenGl,
    /// The GPU through Direct3D 12, Vulkan or Metal.
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
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "Glossa Dark".into(),
            custom_frame: true,
            font_family: String::new(),
            ui_scale: 1.0,
            text_size: 11.0,
            icon_scale: 1.2,
            keyboard_scale: 1.0,
            window_transparency: 0,
            renderer: Renderer::OpenGl,
            unload_graphics_after_secs: 45,
            backdrop: Backdrop::None,
            popup: PopupConfig::default(),
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
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            auto_size: true,
            auto_position: true,
            activate_on_open: false,
            pin_when_dragging: true,
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
        let any = |class: &str| ExceptionRule { program: String::new(), window_class: class.into() };
        Self {
            mode: ExceptionMode::DisabledFor,
            rules: vec![
                any("SysListView32"),
                any("SysTreeView32"),
                any("ListBox"),
                any("ScrollBar"),
                any("ComboBox"),
                any("msctls_hotkey32"),
                any("ConsoleWindowClass"),
                ExceptionRule { program: "mstsc.exe".into(), window_class: String::new() },
            ],
            smart_detection: true,
        }
    }
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
        }
    }
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
    /// A click on the tray icon toggles mouse mode instead of opening the
    /// main window.
    pub tray_click_toggles: bool,
}

impl Default for MouseMode {
    fn default() -> Self {
        Self { enabled: false, behaviour: MouseBehaviour::ShowIcon, ctrl_only: false, tray_click_toggles: true }
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
    /// `command-ocr` or a module's.
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
    pub lens: LensConfig,
    /// The Windows 11 Snipping Tool's engine.
    pub oneocr: OneOcrConfig,
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
            lens: LensConfig::default(),
            oneocr: OneOcrConfig::default(),
        }
    }
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
    #[default]
    Auto,
    Daemon,
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
    /// Required on every request when set. Generated on first run.
    pub token: String,
}

impl Default for Server {
    fn default() -> Self {
        Self { enabled: true, bind: "127.0.0.1:60828".into(), token: String::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModulesConfig {
    /// Extra directories to look for modules in, after the built-in ones.
    pub dirs: Vec<PathBuf>,
    /// The Python interpreter for Python modules. Empty finds one on `PATH`.
    pub python: String,
    /// Load native (shared library) modules at all. They run with the
    /// program's full privileges, so this is a deliberate switch.
    pub allow_native: bool,
    /// Load classic `Service.js` scripts.
    pub allow_legacy_scripts: bool,
    /// Time budget for one module call, milliseconds.
    pub call_timeout_ms: u64,
    /// Module ids switched off.
    pub disabled: Vec<String>,
}

impl Default for ModulesConfig {
    fn default() -> Self {
        Self {
            dirs: Vec::new(),
            python: String::new(),
            allow_native: true,
            allow_legacy_scripts: true,
            call_timeout_ms: 30_000,
            disabled: Vec::new(),
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
        self.appearance.icon_scale = self.appearance.icon_scale.clamp(0.75, 3.0);
        self.appearance.keyboard_scale = self.appearance.keyboard_scale.clamp(0.5, 3.0);
        self.appearance.window_transparency = self.appearance.window_transparency.min(90);
        self.dictionary.zoom = self.dictionary.zoom.clamp(50, 400);
        self.general.instant_delay_ms = self.general.instant_delay_ms.clamp(150, 5000);
        self.general.recent_pairs.dedup();
        self.general.recent_pairs.truncate(12);
        self.history.max_entries = self.history.max_entries.clamp(10, 1_000_000);
        self.modules.call_timeout_ms = self.modules.call_timeout_ms.clamp(1000, 600_000);
        self.ocr.watch_interval_ms = self.ocr.watch_interval_ms.clamp(200, 60_000);
        self.advanced.copy_wait_ms = self.advanced.copy_wait_ms.clamp(20, 3000);
        if !self.general.second_language.is_language() {
            self.general.second_language = LangId::ENGLISH;
        }
        if !self.general.first_language.is_language() {
            self.general.first_language = LangId::ENGLISH;
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
        h.bindings.insert(Action::ShowHistory, "Ctrl+E".parse().unwrap());
        let conflicts = h.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0.to_string(), "Ctrl+E");
        assert!(conflicts[0].1.contains(&Action::ListenSelection));
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
    fn exceptions_follow_the_classic_rules() {
        let e = Exceptions::default();
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
        let mut e = Exceptions::default();
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
    fn popup_opacity_and_legacy_icon_mask() {
        let p = PopupConfig { transparency: 15, ..Default::default() };
        assert!((p.opacity() - 0.85).abs() < 1e-6);
        assert_eq!(PopupIcons::from_legacy(30), PopupIcons::default());
        let some = PopupIcons::from_legacy(2 | 8);
        assert!(some.dictionary && some.copy && !some.listen && !some.replace);
    }
}
