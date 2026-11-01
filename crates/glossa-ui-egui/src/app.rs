//! The application: state, event routing, and the frame loop.
//!
//! Work happens in two places. [`GlossaApp::logic`] runs on every wake-up,
//! even while the main window is hidden in the tray: it drains engine events,
//! hotkeys, tray clicks and background tasks, and decides which windows must
//! exist. [`GlossaApp::ui`] draws whatever is visible. When something must
//! appear while everything is hidden (the popup, after a hotkey), `logic`
//! asks for a paint while hidden so `ui` runs and can open it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::{Action, Config, LangId, LangPair, Locale};
use glossa_engine::{Engine, Event, Slot, TranslationOutcome};
use glossa_platform::{AudioPlayer, Clipboard, HotkeyService, Session};

use crate::actions::Task;
use crate::fonts::Fonts;
use crate::style::Look;

/// How much one Ctrl+plus or Ctrl+minus changes the text zoom.
const TEXT_ZOOM_STEP: f32 = 1.1;

/// What the main window holds.
pub struct MainState {
    pub source: String,
    pub outcome: Option<Arc<TranslationOutcome>>,
    pub error: Option<String>,
    pub busy: bool,
    pub job: u64,
    /// While a streaming service writes: the translation so far.
    pub partial: Option<String>,
    /// Sources translated this session, for back/forward.
    pub back: Vec<String>,
    pub forward: Vec<String>,
    /// When the source last changed, for instant translation.
    pub edited_at: Option<Instant>,
    /// The source as last translated, so an unchanged text is not re-sent.
    pub translated_source: String,
    /// Stacked layout: one result per provider id, by job.
    pub stacked: HashMap<String, Result<Arc<TranslationOutcome>, String>>,
    pub stacked_jobs: HashMap<u64, String>,
    /// Spell checking of `source`.
    pub spell: crate::spell::Spelling,
}

impl MainState {
    fn new(cfg: &Config) -> Self {
        MainState {
            source: if cfg.general.save_contents_on_exit { cfg.contents.source.clone() } else { String::new() },
            outcome: None,
            error: None,
            busy: false,
            job: 0,
            partial: None,
            back: Vec::new(),
            forward: Vec::new(),
            edited_at: None,
            translated_source: String::new(),
            stacked: HashMap::new(),
            stacked_jobs: HashMap::new(),
            spell: Default::default(),
        }
    }
}

/// A short message shown at the bottom of the main window or as a popup.
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct GlossaApp {
    pub engine: Engine,
    events: Receiver<Event>,
    pub cfg: Config,
    pub look: Look,
    pub fonts: Fonts,
    pub locale: Arc<Locale>,
    pub ctx: egui::Context,
    pub launch: crate::Launch,

    pub hotkeys: Option<HotkeyService>,
    pub hotkey_errors: Vec<String>,
    pub clipboard: Option<Clipboard>,
    pub audio: AudioPlayer,
    pub session: Session,
    tray: Option<crate::tray::Tray>,
    /// macOS: the menu bar's commands.
    pub app_menu: crate::app_menu::AppMenu,
    mouse: Option<crate::mouse::MouseHub>,
    /// The button mouse mode shows by the pointer.
    pub mouse_icon: Option<crate::mouse::MouseIcon>,
    /// The last text mouse mode acted on, to ignore repeats.
    pub last_mouse_text: Option<(String, Instant)>,
    server: Option<glossa_engine::server::ServerHandle>,
    pub server_error: Option<String>,
    /// Where the long-lived parts go back when the GUI unloads.
    slot: Rc<RefCell<Option<crate::Shell>>>,
    wake: Arc<crate::Wake>,
    /// Events that woke the GUI, acted on in the first frame.
    pending: Vec<crate::tray::Pending>,
    /// Closing the window to unload the graphics, not to quit.
    unloading: bool,
    /// Since when nothing has been on screen.
    hidden_since: Option<Instant>,

    pub main: MainState,
    pub popup: Option<crate::popup::PopupState>,
    pub ocr: Option<crate::ocr::OcrState>,
    pub watch: Option<crate::ocr::Watch>,
    /// A copy-mode or in-place recognition running after its overlay closed.
    pub ocr_background: Option<crate::ocr::Background>,
    /// The small notice that takes no focus: by an OCR region ("Copied 120
    /// characters"), or by the pointer for messages while the main window
    /// is hidden.
    pub notice: Option<crate::notice::Notice>,
    pub in_place: Option<crate::inplace::InPlace>,
    /// The window showing a recognised region's text and translation.
    pub ocr_result: Option<crate::ocr_result::ResultWindow>,
    pub options: Option<crate::options::OptionsState>,
    pub welcome: Option<crate::welcome::WelcomeState>,
    pub dictionary: Option<crate::dictionary::DictState>,
    pub history: Option<crate::history::HistoryState>,
    pub about_open: bool,
    pub keyboard_open: bool,

    pub tasks_tx: Sender<Task>,
    tasks: Receiver<Task>,
    pub toast: Option<Toast>,
    /// The main window's visibility as we last set it.
    pub main_visible: bool,
    /// macOS: shown in the Dock and Cmd+Tab (a full window is open).
    in_app_switcher: bool,
    /// When the system's light or dark mode was last looked at, and what the
    /// windows said it was then (see `follow_system_mode`).
    mode_checked: Option<(Instant, Option<egui::Theme>)>,
    started: bool,
    quitting: bool,
    /// Quitting to start again (see [`GlossaApp::restart`]).
    restarting: bool,
    frames: u64,
    /// The text zoom changed (Ctrl+wheel, Ctrl+plus/minus) and is waiting
    /// to be saved.
    zoom_changed: Option<Instant>,
    /// The main window was created with an alpha channel.
    transparent_root: bool,
    /// The main window's backdrop needs (re)applying.
    backdrop_dirty: bool,
    /// Child windows' backdrops as applied: title -> (window, kind, settings
    /// generation), so a recreated window or new settings apply again.
    child_backdrops: HashMap<String, (isize, glossa_core::config::Backdrop, u64)>,
    /// Borderless child windows whose corners the system rounds (macOS),
    /// by title, with the window they were rounded for.
    rounded_windows: HashMap<String, isize>,
    /// Child windows' frames as styled (rounded corners, border colour):
    /// title -> (the root pass it was last drawn in, the settings generation
    /// it was styled for). A window not drawn in the previous pass was
    /// closed in between, and its new window is styled again.
    child_frames: HashMap<String, (u64, Option<u64>)>,
    backdrop_generation: u64,
    /// Whether the popup's backdrop really blurs (else it is shaded).
    pub popup_blurred: bool,
    /// A target language picked in the popup's header, used by later
    /// popups too until "automatic direction" is picked again (for this run
    /// of the program only).
    pub popup_target: Option<glossa_core::LangId>,
    /// The last OCR region, for "repeat last recognition".
    pub last_region: Option<glossa_core::ocr::Rect>,
    /// Problems met while starting, shown once (a banner in the main window,
    /// or a notice when it is hidden) until put away.
    pub startup_warnings: Vec<String>,
}

impl GlossaApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        slot: Rc<RefCell<Option<crate::Shell>>>,
        launch: crate::Launch,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let shell = slot.borrow_mut().take().expect("the shell is handed to each GUI run");
        let engine = shell.engine.clone();
        let cfg = engine.config();
        let welcome = cfg.general.show_welcome.then(crate::welcome::WelcomeState::default);
        let transparent_root = crate::style::needs_transparent_root(&cfg);
        let look = Look::new(engine.current_theme(), &cfg);
        look.apply(&ctx, cfg.appearance.ui_scale);
        // Ctrl+plus/minus/0 zoom the text areas (see `text_zoom_keys`), not
        // the whole interface as egui would by default.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let mut fonts = Fonts::default();
        fonts.install_base(&ctx, &cfg.appearance.font_family, &look.theme);
        *shell.wake.ctx.lock() = Some(ctx.clone());

        crate::langs::set_locale(engine.locale());
        let events = engine.subscribe();
        let (tasks_tx, tasks) = mpsc::channel();
        let start_visible = !launch.hidden;
        if let Some(t) = &shell.tray {
            t.set_hotkeys_checked(cfg.hotkeys.enabled);
        }
        // After winit put up its default menu (it does so before the app is
        // created), so this one replaces it.
        let app_menu = crate::app_menu::AppMenu::install(&ctx, &engine.locale());
        GlossaApp {
            app_menu,
            locale: engine.locale(),
            main: MainState::new(&cfg),
            engine,
            events,
            cfg,
            look,
            fonts,
            ctx,
            launch,
            hotkeys: shell.hotkeys,
            hotkey_errors: shell.hotkey_errors,
            clipboard: shell.clipboard,
            audio: shell.audio,
            session: Session::detect(),
            tray: shell.tray,
            mouse: Some(shell.mouse),
            mouse_icon: None,
            last_mouse_text: None,
            server: shell.server,
            server_error: shell.server_error,
            slot,
            wake: shell.wake,
            pending: shell.pending,
            unloading: false,
            hidden_since: None,
            popup: None,
            ocr: None,
            watch: None,
            ocr_background: None,
            notice: None,
            in_place: None,
            ocr_result: None,
            options: None,
            welcome,
            dictionary: None,
            history: None,
            about_open: false,
            keyboard_open: false,
            tasks_tx,
            tasks,
            toast: None,
            main_visible: start_visible,
            in_app_switcher: false,
            mode_checked: None,
            started: false,
            quitting: false,
            restarting: false,
            frames: 0,
            zoom_changed: None,
            transparent_root,
            backdrop_dirty: true,
            child_backdrops: HashMap::new(),
            rounded_windows: HashMap::new(),
            child_frames: HashMap::new(),
            backdrop_generation: 0,
            popup_blurred: false,
            popup_target: None,
            last_region: None,
            startup_warnings: shell.startup_warnings,
        }
    }

    /// The translated interface string for `key`.
    pub fn t(&self, key: &str) -> String {
        self.locale.get(key).to_owned()
    }

    pub fn register_hotkeys(&mut self) {
        let Some(h) = self.hotkeys.as_mut() else { return };
        h.unregister_all();
        self.hotkey_errors.clear();
        if !self.cfg.hotkeys.enabled {
            return;
        }
        for (action, hotkey) in &self.cfg.hotkeys.bindings {
            if let Err(e) = h.register(*action, hotkey) {
                tracing::warn!("hotkey {hotkey} for {}: {e}", action.id());
                self.hotkey_errors.push(crate::hotkey_error_text(&self.locale, Some(*action), &e));
            }
        }
    }

    /// Tell the user something: at the bottom of the main window while it
    /// is on screen, else in a notice by the pointer (the main window is
    /// usually hidden in the tray when a hotkey's action has news).
    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        if error {
            tracing::info!("message for the user: {text}");
        }
        if self.main_on_screen() {
            self.toast = Some(Toast { text, error, at: Instant::now() });
            self.ctx.request_repaint();
        } else {
            crate::notice::at_cursor(self, text, error);
        }
    }

    /// Whether the main window is showing (not hidden, not minimised).
    pub fn main_on_screen(&self) -> bool {
        self.main_visible && !self.ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().minimized.unwrap_or(false))
    }

    pub fn copy_text(&mut self, text: &str) {
        match self.clipboard.as_mut() {
            Some(c) => match c.set_text(text) {
                Ok(()) => {
                    let msg = self.t("msg.copied");
                    self.notify(msg, false);
                }
                Err(e) => self.notify(e.to_string(), true),
            },
            None => self.ctx.copy_text(text.to_owned()),
        }
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard.as_mut().and_then(|c| c.get_text()).filter(|t| !t.trim().is_empty())
    }

    pub fn speak(&mut self, text: &str, lang: LangId) {
        if text.trim().is_empty() {
            return;
        }
        self.engine.speak_async(Slot::Speech, text.to_owned(), lang, None);
    }

    pub fn open_url(&self, url: &str) {
        let browser = self.cfg.advanced.browser.trim();
        let result = if browser.is_empty() {
            open_with_system(url)
        } else {
            std::process::Command::new(browser).arg(url).spawn().map(|_| ())
        };
        if let Err(e) = result {
            tracing::warn!("could not open {url}: {e}");
        }
    }

    // -- main window helpers ------------------------------------------------

    pub fn show_main(&mut self) {
        self.main_visible = true;
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }

    /// Keep the main window above the others, or not, as set.
    pub fn apply_main_window_level(&self) {
        let level = if self.cfg.appearance.main_always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        };
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::WindowLevel(level));
    }

    pub fn hide_main(&mut self) {
        self.main_visible = false;
        self.remember_main_geometry();
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(false));
    }

    fn remember_main_geometry(&mut self) {
        let info = self.ctx.input(|i| i.viewport().clone());
        if let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) {
            let ppp = info.native_pixels_per_point.unwrap_or(1.0).max(0.1);
            let _ = ppp;
            self.cfg.windows.insert(
                "main".into(),
                glossa_core::config::WindowGeometry {
                    x: outer.min.x,
                    y: outer.min.y,
                    width: inner.width(),
                    height: inner.height(),
                    maximized: info.maximized.unwrap_or(false),
                },
            );
        }
    }

    fn apply_main_backdrop(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let kind = self.look.window_backdrop;
        if !self.transparent_root && kind != glossa_core::config::Backdrop::None {
            return; // needs a restart: the window has no alpha channel
        }
        let dark = self.look.theme.dark;
        // winit re-applies the system theme after creation, which would tint
        // Mica the system's way; say which way it should be.
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::SetTheme(if dark { egui::SystemTheme::Dark } else { egui::SystemTheme::Light }),
        );
        let options = glossa_platform::backdrop::Options::new(dark).with_tint(self.cfg.appearance.backdrop_opacity);
        if let Err(e) = glossa_platform::backdrop::apply_with(frame, kind, options) {
            tracing::debug!("main window backdrop: {e}");
        }
    }

    fn style_main_frame(&self, frame: &eframe::Frame) {
        if crate::chrome::custom(&self.cfg) {
            if let Err(e) = glossa_platform::backdrop::style_frame_of(frame, Some(self.frame_border())) {
                tracing::debug!("main window frame: {e}");
            }
        }
    }

    /// Open the on-screen keyboard, or bring it to the front if it is open.
    pub fn open_keyboard(&mut self) {
        if self.keyboard_open {
            crate::chrome::raise(&self.ctx, crate::chrome::viewport(crate::chrome::KEYBOARD));
        }
        self.keyboard_open = true;
        self.ctx.request_repaint();
    }

    /// Open the About window, or bring it to the front if it is open.
    pub fn open_about(&mut self) {
        if self.about_open {
            crate::chrome::raise(&self.ctx, crate::chrome::viewport(crate::chrome::ABOUT));
        }
        self.about_open = true;
        self.ctx.request_repaint();
    }

    /// Whether the main window was created with an alpha channel, which the
    /// other windows then share (see [`crate::chrome::builder`]).
    pub fn transparent_root(&self) -> bool {
        self.transparent_root
    }

    /// The border colour the system draws around a custom-framed window.
    fn frame_border(&self) -> (u8, u8, u8) {
        let b = self.look.theme.window.border;
        (b.r, b.g, b.b)
    }

    /// Round the corners and colour the border of a child window that draws
    /// its own frame, once per window it is shown in and again after the
    /// settings change. Called after each pass that drew the window.
    fn style_child_frame(&mut self, title: &str) {
        if !crate::chrome::custom(&self.cfg) {
            return;
        }
        let pass = self.ctx.cumulative_pass_nr();
        let generation = self.backdrop_generation;
        let border = self.frame_border();
        let entry = self.child_frames.entry(title.to_owned()).or_insert((0, None));
        if pass > entry.0 + 2 {
            entry.1 = None; // closed and opened again: a new window
        }
        entry.0 = pass;
        if entry.1 == Some(generation) {
            return;
        }
        let Some(hwnd) = glossa_platform::backdrop::find_own_window(title) else { return };
        if let Err(e) = glossa_platform::backdrop::style_frame(hwnd, Some(border)) {
            tracing::debug!("frame for {title}: {e}");
        }
        entry.1 = Some(generation);
    }

    /// Round a borderless child window's corners by `radius` where the
    /// system does not (macOS, `glossa_platform::window::round_corners`),
    /// once per window. Returns whether its corners are rounded now, so the
    /// frame drawn in it can follow them.
    pub fn round_child(&mut self, title: &str, radius: f32) -> bool {
        if radius <= 0.0 {
            return false;
        }
        let Some(window) = glossa_platform::window::own_window_by_title(title) else { return false };
        if self.rounded_windows.get(title) == Some(&window) {
            return true;
        }
        let rounded = glossa_platform::window::round_corners(window, radius);
        if rounded {
            self.rounded_windows.insert(title.to_owned(), window);
        }
        rounded
    }

    /// Whether the child window titled `title` had its corners rounded by
    /// [`Self::round_child`].
    pub fn child_rounded(&self, title: &str) -> bool {
        self.rounded_windows.contains_key(title)
    }

    /// Give a child window (popup, options…) its backdrop once it exists,
    /// and again when it is recreated or the settings change; a tool window
    /// also gets its frame styled. egui does not expose child windows'
    /// handles, so the window is found by its title.
    pub fn child_backdrop(&mut self, title: &str, popup: bool) {
        use glossa_core::config::Backdrop;
        use glossa_platform::backdrop;
        if !popup {
            self.style_child_frame(title);
        }
        if !self.transparent_root {
            return;
        }
        let kind = if popup { self.look.popup_backdrop } else { self.look.window_backdrop };
        let applied = self.child_backdrops.get(title).copied();
        if kind == Backdrop::None && applied.is_none_or(|(_, k, _)| k == Backdrop::None) {
            return;
        }
        let Some(hwnd) = backdrop::find_own_window(title) else { return };
        if applied == Some((hwnd, kind, self.backdrop_generation)) {
            return;
        }
        let dark = self.look.theme.dark;
        let options = if popup {
            backdrop::Options::unfocused(dark).with_tint(self.cfg.appearance.popup.backdrop_opacity)
        } else {
            backdrop::Options::new(dark).with_tint(self.cfg.appearance.backdrop_opacity)
        };
        let result = backdrop::apply_hwnd_with(hwnd, kind, options);
        if popup {
            self.popup_blurred = result.as_ref().is_ok_and(|a| a.blurs());
        }
        if let Err(e) = result {
            tracing::debug!("backdrop for {title}: {e}");
        }
        self.child_backdrops.insert(title.to_owned(), (hwnd, kind, self.backdrop_generation));
    }

    /// Ctrl+plus, Ctrl+minus and Ctrl+0 in a window whose content is text
    /// (the main window, the popup, the dictionary) zoom its text areas.
    /// Each such window calls this once a frame with its own input. The
    /// interface scale is not touched: that is set in Options only.
    pub fn text_zoom_keys(&mut self, ui: &egui::Ui) {
        use egui::gui_zoom::kb_shortcuts as k;
        let (bigger, smaller, reset) = ui.ctx().input_mut(|i| {
            (
                i.consume_shortcut(&k::ZOOM_IN) || i.consume_shortcut(&k::ZOOM_IN_SECONDARY),
                i.consume_shortcut(&k::ZOOM_OUT),
                i.consume_shortcut(&k::ZOOM_RESET),
            )
        });
        let scale = self.look.text_scale;
        if reset {
            self.set_text_scale(1.0);
        } else if bigger {
            self.set_text_scale(scale * TEXT_ZOOM_STEP);
        } else if smaller {
            self.set_text_scale(scale / TEXT_ZOOM_STEP);
        }
    }

    /// Ctrl+wheel with the pointer over `area`, a text area just drawn,
    /// zooms the text areas (all of them: the zoom is shared).
    pub fn text_zoom_wheel(&mut self, ui: &egui::Ui, area: egui::Rect) {
        let (delta, over) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos().is_some_and(|p| area.contains(p))));
        if over && (delta - 1.0).abs() > 1e-3 {
            // egui's wheel zoom is about 20% a notch; text wants finer steps
            // (about 10%, as Ctrl+plus).
            self.set_text_scale(self.look.text_scale * delta.sqrt());
        }
    }

    fn set_text_scale(&mut self, scale: f32) {
        let scale = glossa_core::config::clamp_text_scale(scale);
        if (scale - self.look.text_scale).abs() < 1e-3 {
            return;
        }
        self.look.text_scale = scale;
        self.cfg.appearance.text_scale = scale;
        self.zoom_changed = Some(Instant::now());
        self.ctx.request_repaint();
    }

    /// Save a changed text zoom once the wheel has been still for a moment,
    /// rather than on every notch.
    fn save_zoom_later(&mut self) {
        let Some(at) = self.zoom_changed else { return };
        let wait = std::time::Duration::from_millis(800);
        if at.elapsed() < wait {
            self.ctx.request_repaint_after(wait - at.elapsed());
            return;
        }
        self.zoom_changed = None;
        let scale = self.look.text_scale;
        let _ = self.engine.update_config(move |c| c.appearance.text_scale = scale);
    }

    /// Translate what is in the main window's source pane.
    pub fn translate_main(&mut self) {
        let text = self.main.source.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if self.main.translated_source != text && !self.main.translated_source.is_empty() {
            let prev = std::mem::take(&mut self.main.translated_source);
            if self.main.back.last() != Some(&prev) {
                self.main.back.push(prev);
            }
            self.main.forward.clear();
        }
        self.main.translated_source = text.clone();
        self.main.edited_at = None;
        self.main.error = None;
        let pair = self.cfg.general.pair;
        let opts = glossa_engine::TranslateOptions {
            provider: Some(self.cfg.services.active.clone()).filter(|s| !s.is_empty()),
            back_translate: self.cfg.general.back_translation,
            ..Default::default()
        };
        if self.cfg.general.results_layout == glossa_core::config::ResultsLayout::Stacked {
            self.main.stacked.clear();
            self.main.stacked_jobs.clear();
            for id in self.cfg.services.enabled.clone() {
                let o = glossa_engine::TranslateOptions {
                    provider: Some(id.clone()),
                    fail_over: Some(false),
                    ..opts.clone()
                };
                let job = self.engine.translate_async(Slot::Detached, text.clone(), pair, o);
                self.main.stacked_jobs.insert(job, id);
            }
        }
        self.main.busy = true;
        self.main.partial = None;
        self.main.job = self.engine.translate_async(Slot::Main, text, pair, opts);
    }

    pub fn set_pair(&mut self, pair: LangPair) {
        self.cfg.general.pair = pair;
        self.cfg.remember_pair(pair);
        let p = pair;
        let _ = self.engine.update_config(move |c| {
            c.general.pair = p;
            c.remember_pair(p);
        });
    }

    pub fn set_active_service(&mut self, id: &str) {
        self.cfg.services.active = id.to_owned();
        let id = id.to_owned();
        let _ = self.engine.update_config(move |c| c.services.active = id);
        if !self.main.source.trim().is_empty() {
            self.main.translated_source.clear();
            self.translate_main();
        }
    }

    /// Reload settings-derived state after the configuration changed.
    fn refresh_from_config(&mut self) {
        let before_hotkeys = self.cfg.hotkeys.clone();
        // A text zoom not saved yet stays: the file has the old one.
        let unsaved_zoom = self.zoom_changed.map(|_| self.look.text_scale);
        let windows = std::mem::take(&mut self.cfg.windows);
        self.cfg = self.engine.config();
        if self.cfg.windows.is_empty() {
            self.cfg.windows = windows;
        }
        let locale = self.engine.locale();
        if glossa_platform::shell::SHELL.app_menu && !Arc::ptr_eq(&locale, &self.locale) {
            self.app_menu = crate::app_menu::AppMenu::install(&self.ctx, &locale);
        }
        self.locale = locale;
        crate::langs::set_locale(self.locale.clone());
        if let Some(scale) = unsaved_zoom {
            self.cfg.appearance.text_scale = scale;
        }
        self.look = Look::new(self.engine.current_theme(), &self.cfg);
        self.look.apply(&self.ctx, self.cfg.appearance.ui_scale);
        self.fonts.install_base(&self.ctx, &self.cfg.appearance.font_family, &self.look.theme);
        self.backdrop_dirty = true;
        self.backdrop_generation += 1;
        let mouse_failed = self.mouse.as_mut().and_then(|m| m.sync(self.cfg.mouse_mode.enabled));
        if let Some(e) = mouse_failed {
            let m = self.locale.fmt("mouse.failed", &e);
            self.notify(m, true);
        }
        if let Some(t) = &self.tray {
            let c = self.look.theme.accent;
            let accent = [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0];
            t.set_look(accent, self.look.theme.dark, self.mouse_running());
        }
        if before_hotkeys != self.cfg.hotkeys {
            self.register_hotkeys();
        }
        if let Some(t) = &self.tray {
            t.set_hotkeys_checked(self.cfg.hotkeys.enabled);
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Progress { slot: Slot::Main, job, text } if job == self.main.job => {
                self.fonts.ensure(&self.ctx, &text);
                self.main.partial = Some(text);
            }
            Event::Progress { slot: Slot::Popup, job, text } => {
                self.fonts.ensure(&self.ctx, &text);
                if let Some(p) = self.popup.as_mut() {
                    p.on_progress(job, text);
                }
            }
            Event::Translated { slot: Slot::Main, job, result } if job == self.main.job => {
                self.main.busy = false;
                self.main.partial = None;
                match result {
                    Ok(out) => {
                        self.main.error = None;
                        self.main.outcome = Some(out);
                    }
                    Err(e) => {
                        self.main.error = Some(self.describe(&e));
                    }
                }
            }
            Event::Progress { slot: Slot::OcrResult, job, text } => crate::ocr_result::on_progress(self, job, text),
            Event::Translated { slot: Slot::OcrResult, job, result } => {
                crate::ocr_result::on_translated(self, job, result)
            }
            Event::Translated { slot: Slot::Detached, job, result } => {
                if let Some(id) = self.main.stacked_jobs.remove(&job) {
                    let r = result.map_err(|e| self.describe(&e));
                    self.main.stacked.insert(id, r);
                } else if let Some(p) = self.popup.as_mut() {
                    p.on_detached(job, result);
                }
            }
            Event::Translated { slot: Slot::Popup, job, result } => {
                let msg = result.as_ref().err().map(|e| self.describe(e));
                if let Some(p) = self.popup.as_mut() {
                    p.on_result(job, result, msg);
                }
            }
            Event::Dictionary { job, query, result, .. } => {
                let msg = result.as_ref().err().map(|e| self.describe(e));
                if let Some(d) = self.dictionary.as_mut() {
                    d.on_result(job, query, result, msg);
                }
            }
            Event::Recognized { job, result, .. } => {
                let msg = result.as_ref().err().map(|e| self.describe(e));
                crate::ocr::on_result(self, job, result, msg);
            }
            Event::Spoken { result, .. } => match result {
                Ok(audio) => {
                    if let Err(e) = self.audio.play(audio.bytes.clone()) {
                        let m = format!("{}: {e}", self.t("msg.cannot_play"));
                        self.notify(m, true);
                    }
                }
                Err(e) => {
                    let m = self.describe(&e);
                    self.notify(m, true);
                }
            },
            Event::ConfigChanged | Event::ModulesReloaded => self.refresh_from_config(),
            Event::HistoryChanged => {
                if let Some(h) = self.history.as_mut() {
                    h.dirty = true;
                }
            }
            Event::Action { action, text } => self.perform(action, text),
            Event::Started { .. } | Event::Translated { .. } | Event::Progress { .. } => {}
        }
    }

    /// A human message for an engine error, in the interface language where
    /// the classic program had one.
    pub fn describe(&self, e: &glossa_core::Error) -> String {
        use glossa_core::Error as E;
        let key = match e {
            E::EmptyInput => Some("msg.nothing_to_translate"),
            E::LanguageNotDetected => Some("msg.cannot_detect"),
            E::SourceLanguageUnsupported { .. } => Some("msg.source_unsupported"),
            E::TargetLanguageUnsupported { .. } => Some("msg.target_unsupported"),
            E::Timeout(_) => Some("msg.timeout"),
            E::Network(_) => Some("msg.no_connection"),
            E::Unsupported { .. } => Some("msg.capability_unsupported"),
            E::NoTextRecognised => Some("msg.no_text_in_image"),
            _ => None,
        };
        match key {
            // The localised headline, with the specifics after it.
            Some(k) => {
                let head = self.t(k);
                match e {
                    E::Network(_) | E::Unsupported { .. } => format!("{head}\n{e}"),
                    _ => head,
                }
            }
            None => e.to_string(),
        }
    }

    /// Run an action from a hotkey, the tray, the control API or a menu.
    pub fn perform(&mut self, action: Action, text: Option<String>) {
        crate::actions::perform(self, action, text);
    }

    /// Whether mouse mode is watching the mouse now.
    pub fn mouse_running(&self) -> bool {
        self.mouse.as_ref().is_some_and(|m| m.running())
    }

    /// Why mouse mode is switched on but not running, if it failed to start.
    pub fn mouse_error(&self) -> Option<String> {
        self.mouse.as_ref().and_then(|m| m.error()).map(str::to_owned)
    }

    /// Whether the control API is serving now (it starts with the program).
    pub fn server_running(&self) -> bool {
        self.server.is_some()
    }

    /// The program is on its way out (Quit, the system ending it): windows
    /// closing now are not the user closing them.
    /// With `appearance.follow_system_theme`, tell the engine the system's
    /// light or dark mode: at once when a window reports the mode changed,
    /// and every few seconds besides (not every system reports it). The
    /// engine restyles the windows when the theme in effect changes.
    fn follow_system_mode(&mut self, ctx: &egui::Context) {
        if !self.cfg.appearance.follow_system_theme {
            return;
        }
        let reported = ctx.input(|i| i.raw.system_theme);
        let due = match self.mode_checked {
            None => true,
            Some((at, seen)) => seen != reported || at.elapsed() >= Duration::from_secs(5),
        };
        if due {
            self.mode_checked = Some((Instant::now(), reported));
            self.engine.set_system_dark(glossa_platform::appearance::system_prefers_dark());
        }
        ctx.request_repaint_after(Duration::from_secs(5));
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting
    }

    pub fn quit(&mut self) {
        self.quitting = true;
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Quit, and start a fresh copy once everything is shut down: for
    /// settings that apply at start-up, such as the graphics engine.
    pub fn restart(&mut self) {
        self.restarting = true;
        self.quit();
    }

    /// With nothing on screen for a while, close the GUI so its graphics
    /// are freed; the tray, hotkeys and engine stay up in `main`, which
    /// starts the GUI again when something needs it.
    fn maybe_unload(&mut self, needs_ui: bool) {
        if self.unloading {
            return;
        }
        let delay = self.cfg.appearance.unload_graphics_after_secs;
        let busy = self.main.busy || self.watch.is_some() || self.toast.is_some() || self.ocr_background.is_some();
        if !glossa_platform::idle::IdlePump::supported()
            || self.launch.screenshot.is_some()
            || delay == 0
            || self.main_visible
            || needs_ui
            || busy
        {
            self.hidden_since = None;
            return;
        }
        let since = *self.hidden_since.get_or_insert_with(Instant::now);
        let wait = Duration::from_secs(delay as u64);
        if since.elapsed() >= wait {
            tracing::debug!("nothing on screen for {delay} s; unloading the GUI");
            self.unloading = true;
            self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        } else {
            self.ctx.request_repaint_after(wait - since.elapsed());
        }
    }

    fn save_state(&mut self) {
        self.remember_main_geometry();
        let source = self.main.source.clone();
        let translation = self.main.outcome.as_ref().map(|o| o.translation.text.clone()).unwrap_or_default();
        let windows = self.cfg.windows.clone();
        let _ = self.engine.update_config(move |c| {
            if c.general.save_contents_on_exit {
                c.contents.source = source;
                c.contents.translation = translation;
            }
            c.windows = windows;
        });
    }
}

impl eframe::App for GlossaApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _frame = crate::watchdog::frame("logic");
        self.follow_system_mode(ctx);
        if !self.started {
            self.started = true;
            if !self.main_visible {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(false));
            }
            if let Some(text) = self.launch.popup_text.clone() {
                crate::popup::open(self, text, None);
            }
            if !self.main_visible && !self.startup_warnings.is_empty() {
                let text = format!("{}\n{}", self.t("main.startup_warnings"), self.startup_warnings.join("\n"));
                self.startup_warnings.clear();
                self.notify(text, true);
            }
            for p in std::mem::take(&mut self.pending) {
                match p {
                    crate::tray::Pending::Action(a, text) => self.perform(a, text),
                    crate::tray::Pending::Tray(e) => crate::tray::handle(self, e),
                    crate::tray::Pending::Task(t) => crate::actions::on_task(self, t),
                }
            }
        }
        while let Ok(ev) = self.events.try_recv() {
            self.handle_event(ev);
        }
        while let Ok(task) = self.tasks.try_recv() {
            crate::actions::on_task(self, task);
        }
        let mut fired = Vec::new();
        if let Some(h) = self.hotkeys.as_mut() {
            while let Some(a) = h.poll() {
                fired.push(a);
            }
        }
        for a in fired {
            self.perform(a, None);
        }
        crate::app_menu::poll(self);
        let tray_events = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for e in tray_events {
            crate::tray::handle(self, e);
        }
        // Mouse mode: read the selection on a worker, as the hotkeys do.
        for g in self.mouse.as_ref().map(|m| m.poll()).unwrap_or_default() {
            let (cfg, tx, ctx) = (self.cfg.clone(), self.tasks_tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                if let Some(task) = crate::mouse::selection_task(&cfg, g) {
                    let _ = tx.send(task);
                    ctx.request_repaint();
                }
            });
        }

        // Instant translation: after a pause in typing.
        if self.cfg.general.instant_translation {
            if let Some(at) = self.main.edited_at {
                let wait = Duration::from_millis(self.cfg.general.instant_delay_ms);
                if at.elapsed() >= wait {
                    self.translate_main();
                } else {
                    ctx.request_repaint_after(wait - at.elapsed());
                }
            }
        }

        crate::popup::tick(self);
        crate::ocr::tick(self);
        crate::notice::tick(self);

        // Closing the main window hides it to the tray, when so configured.
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.quitting && !self.unloading {
            if self.cfg.general.minimize_to_tray_on_close && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_main();
            } else {
                self.quitting = true;
            }
        }
        if self.main_visible
            && self.cfg.general.minimize_to_tray_on_minimize
            && self.tray.is_some()
            && ctx.input(|i| i.viewport().minimized).unwrap_or(false)
        {
            self.hide_main();
        }

        // Something must be drawn while the main window is hidden: ask for a
        // paint anyway, so `ui` runs and can open the window.
        let needs_ui = self.popup.is_some()
            || self.mouse_icon.is_some()
            || self.ocr.is_some()
            || self.notice.is_some()
            || self.in_place.is_some()
            || self.ocr_result.is_some()
            || self.options.is_some()
            || self.dictionary.is_some()
            || self.history.is_some()
            || self.about_open
            || self.keyboard_open
            || self.welcome.is_some();
        // Only until one of those windows exists: eframe runs the ui anyway
        // while a child window is shown (an immediate viewport not drawn in
        // a pass is dropped, so one that exists is on screen), and the
        // command asks for a repaint each time it is sent; sending it every
        // frame kept the program drawing ~10 frames a second for as long as
        // a popup stayed open.
        let child_shown = ctx.input(|i| i.raw.viewports.keys().any(|id| *id != egui::ViewportId::ROOT));
        if !self.main_visible && needs_ui && !child_shown {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
        }
        self.maybe_unload(needs_ui);
        // Full windows make Glossa an ordinary app in the switcher; the popup
        // and the menu-bar icon alone do not.
        let windowed = self.main_visible
            || self.options.is_some()
            || self.dictionary.is_some()
            || self.history.is_some()
            || self.about_open
            || self.keyboard_open
            || self.welcome.is_some();
        if glossa_platform::shell::SHELL.switcher_follows_windows && windowed != self.in_app_switcher {
            self.in_app_switcher = windowed;
            glossa_platform::window::set_in_app_switcher(windowed);
        }
        if let Some(t) = &self.toast {
            if t.at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        if self.watch.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.transparent_root {
            // Anything else veils a translucent window or backdrop.
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        use crate::watchdog::phase;
        let _frame = crate::watchdog::frame("backdrop");
        if std::mem::take(&mut self.backdrop_dirty) {
            self.apply_main_backdrop(ui.ctx(), frame);
            self.style_main_frame(frame);
        }
        self.frames += 1;
        let ctx = ui.ctx().clone();
        tracing::trace!(frame = self.frames, causes = ?ctx.repaint_causes(), "frame");
        self.fonts.serve_requests(&ctx);
        self.save_zoom_later();
        if self.main_visible {
            phase("main window");
            crate::main_window::show(self, ui);
        }
        phase("popup");
        crate::popup::show(self, &ctx);
        phase("mouse icon");
        crate::mouse::show_icon(self, &ctx);
        phase("ocr");
        crate::notice::draw(self, &ctx);
        crate::ocr::show(self, &ctx);
        crate::inplace::show(self, &ctx);
        crate::ocr_result::show(self, &ctx);
        phase("options");
        crate::options::show(self, &ctx);
        phase("dictionary");
        crate::dictionary::show(self, &ctx);
        phase("history");
        crate::history::show(self, &ctx);
        phase("about, keyboard");
        crate::about::show_about(self, &ctx);
        crate::welcome::show(self, &ctx);
        crate::keyboard::show_keyboard(self, &ctx);
        if let Some((what, path)) = self.launch.screenshot.clone() {
            crate::actions::screenshot_flow(self, &ctx, &what, &path);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_state();
        *self.wake.ctx.lock() = None;
        let shell = crate::Shell {
            engine: self.engine.clone(),
            hotkeys: self.hotkeys.take(),
            hotkey_errors: std::mem::take(&mut self.hotkey_errors),
            tray: self.tray.take(),
            server: self.server.take(),
            server_error: self.server_error.take(),
            clipboard: self.clipboard.take(),
            audio: std::mem::replace(&mut self.audio, AudioPlayer::new()),
            wake: self.wake.clone(),
            pending: Vec::new(),
            startup_warnings: Vec::new(),
            // Unless this was an unload, the window closing means quitting.
            quit: !self.unloading || self.quitting,
            restart: self.restarting,
            mouse: self.mouse.take().unwrap_or_else(|| {
                let w = self.wake.clone();
                crate::mouse::MouseHub::new(Arc::new(move || w.wake()))
            }),
        };
        *self.slot.borrow_mut() = Some(shell);
    }
}

/// Open a URL or file with the system's default handler.
pub fn open_with_system(target: &str) -> std::io::Result<()> {
    glossa_platform::shell::open(target)
}
