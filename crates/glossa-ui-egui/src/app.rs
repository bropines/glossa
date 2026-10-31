//! The application: state, event routing, and the frame loop.
//!
//! Work happens in two places. [`GlossaApp::logic`] runs on every wake-up,
//! even while the main window is hidden in the tray: it drains engine events,
//! hotkeys, tray clicks and background tasks, and decides which windows must
//! exist. [`GlossaApp::ui`] draws whatever is visible. When something must
//! appear while everything is hidden (the popup, after a hotkey), `logic`
//! asks for a paint while hidden so `ui` runs and can open it.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::{Action, Config, LangId, LangPair, Locale};
use glossa_engine::{Engine, Event, Slot, TranslationOutcome};
use glossa_platform::{AudioPlayer, Clipboard, HotkeyService, Session};

use crate::actions::Task;
use crate::fonts::Fonts;
use crate::style::Look;

/// What the main window holds.
pub struct MainState {
    pub source: String,
    pub outcome: Option<Arc<TranslationOutcome>>,
    pub error: Option<String>,
    pub busy: bool,
    pub job: u64,
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
}

impl MainState {
    fn new(cfg: &Config) -> Self {
        MainState {
            source: if cfg.general.save_contents_on_exit { cfg.contents.source.clone() } else { String::new() },
            outcome: None,
            error: None,
            busy: false,
            job: 0,
            back: Vec::new(),
            forward: Vec::new(),
            edited_at: None,
            translated_source: String::new(),
            stacked: HashMap::new(),
            stacked_jobs: HashMap::new(),
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
    server: Option<glossa_engine::server::ServerHandle>,
    pub server_error: Option<String>,

    pub main: MainState,
    pub popup: Option<crate::popup::PopupState>,
    pub ocr: Option<crate::ocr::OcrState>,
    pub watch: Option<crate::ocr::Watch>,
    pub options: Option<crate::options::OptionsState>,
    pub dictionary: Option<crate::dictionary::DictState>,
    pub history: Option<crate::history::HistoryState>,
    pub about_open: bool,
    pub keyboard_open: bool,

    pub tasks_tx: Sender<Task>,
    tasks: Receiver<Task>,
    pub toast: Option<Toast>,
    /// The main window's visibility as we last set it.
    pub main_visible: bool,
    started: bool,
    quitting: bool,
    frames: u64,
    /// The last OCR region, for "repeat last recognition".
    pub last_region: Option<glossa_core::ocr::Rect>,
}

impl GlossaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: Engine, launch: crate::Launch) -> Self {
        let ctx = cc.egui_ctx.clone();
        let cfg = engine.config();
        let look = Look::new(engine.current_theme(), &cfg);
        look.apply(&ctx, cfg.appearance.ui_scale);
        let mut fonts = Fonts::default();
        fonts.install_base(&ctx, &cfg.appearance.font_family);
        egui_extras_install_image_loaders(&ctx);

        let events = engine.subscribe();
        {
            let ctx = ctx.clone();
            engine.set_waker(move || ctx.request_repaint());
        }

        let session = Session::detect();
        let (tasks_tx, tasks) = mpsc::channel();

        // Hotkeys must be created on this (the event loop's) thread.
        let mut hotkey_errors = Vec::new();
        let hotkeys = match HotkeyService::new() {
            Ok(mut h) => {
                let c = ctx.clone();
                h.set_waker(move || c.request_repaint());
                Some(h)
            }
            Err(e) => {
                hotkey_errors.push(e.to_string());
                None
            }
        };

        let tray = crate::tray::Tray::new(&ctx, &engine.locale()).map_err(|e| tracing::warn!("no tray icon: {e}")).ok();

        let (server, server_error) = if cfg.server.enabled {
            match glossa_engine::server::start(engine.clone()) {
                Ok(h) => (Some(h), None),
                Err(e) => {
                    tracing::warn!("control API not started: {e}");
                    (None, Some(e.to_string()))
                }
            }
        } else {
            (None, None)
        };

        let start_visible = !launch.hidden
            && match cfg.general.startup_action {
                glossa_core::config::StartupAction::ShowWindow => true,
                glossa_core::config::StartupAction::MinimizeToTray => tray.is_none(),
                glossa_core::config::StartupAction::RestorePrevious => true,
            }
            || launch.screenshot.as_ref().is_some_and(|(w, _)| w != "popup");

        let mut app = GlossaApp {
            locale: engine.locale(),
            main: MainState::new(&cfg),
            engine,
            events,
            cfg,
            look,
            fonts,
            ctx,
            launch,
            hotkeys,
            hotkey_errors,
            clipboard: Clipboard::new().map_err(|e| tracing::warn!("no clipboard: {e}")).ok(),
            audio: AudioPlayer::new(),
            session,
            tray,
            server,
            server_error,
            popup: None,
            ocr: None,
            watch: None,
            options: None,
            dictionary: None,
            history: None,
            about_open: false,
            keyboard_open: false,
            tasks_tx,
            tasks,
            toast: None,
            main_visible: start_visible,
            started: false,
            quitting: false,
            frames: 0,
            last_region: None,
        };
        app.register_hotkeys();
        app
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
                self.hotkey_errors.push(format!("{}: {e}", action.label()));
            }
        }
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast { text: text.into(), error, at: Instant::now() });
        self.ctx.request_repaint();
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

    /// Translate what is in the main window's source pane.
    pub fn translate_main(&mut self) {
        let text = self.main.source.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if self.main.translated_source != text {
            if !self.main.translated_source.is_empty() {
                let prev = std::mem::take(&mut self.main.translated_source);
                if self.main.back.last() != Some(&prev) {
                    self.main.back.push(prev);
                }
                self.main.forward.clear();
            }
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
                let o = glossa_engine::TranslateOptions { provider: Some(id.clone()), fail_over: Some(false), ..opts.clone() };
                let job = self.engine.translate_async(Slot::Detached, text.clone(), pair, o);
                self.main.stacked_jobs.insert(job, id);
            }
        }
        self.main.busy = true;
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
        let windows = std::mem::take(&mut self.cfg.windows);
        self.cfg = self.engine.config();
        if self.cfg.windows.is_empty() {
            self.cfg.windows = windows;
        }
        self.locale = self.engine.locale();
        self.look = Look::new(self.engine.current_theme(), &self.cfg);
        self.look.apply(&self.ctx, self.cfg.appearance.ui_scale);
        if before_hotkeys != self.cfg.hotkeys {
            self.register_hotkeys();
        }
        if let Some(t) = &self.tray {
            t.set_hotkeys_checked(self.cfg.hotkeys.enabled);
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Translated { slot: Slot::Main, job, result } if job == self.main.job => {
                self.main.busy = false;
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
            Event::Started { .. } | Event::Translated { .. } => {}
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

    pub fn quit(&mut self) {
        self.quitting = true;
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
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
        if !self.started {
            self.started = true;
            if !self.main_visible {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(false));
            }
            if let Some(text) = self.launch.popup_text.clone() {
                crate::popup::open(self, text, None);
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
        let tray_events = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for e in tray_events {
            crate::tray::handle(self, e);
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

        // Closing the main window hides it to the tray, when so configured.
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.quitting {
            if self.cfg.general.minimize_to_tray_on_close && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_main();
            } else {
                self.quitting = true;
            }
        }
        if self.main_visible && self.cfg.general.minimize_to_tray_on_minimize && self.tray.is_some() {
            if ctx.input(|i| i.viewport().minimized).unwrap_or(false) {
                self.hide_main();
            }
        }

        // Something must be drawn while the main window is hidden: ask for a
        // paint anyway, so `ui` runs and can open the window.
        let needs_ui = self.popup.is_some()
            || self.ocr.is_some()
            || self.options.is_some()
            || self.dictionary.is_some()
            || self.history.is_some()
            || self.about_open
            || self.keyboard_open;
        if !self.main_visible && needs_ui {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        let ctx = ui.ctx().clone();
        if self.main_visible {
            crate::main_window::show(self, ui);
        }
        crate::popup::show(self, &ctx);
        crate::ocr::show(self, &ctx);
        crate::options::show(self, &ctx);
        crate::dictionary::show(self, &ctx);
        crate::history::show(self, &ctx);
        crate::main_window::show_about(self, &ctx);
        crate::main_window::show_keyboard(self, &ctx);
        if let Some((what, path)) = self.launch.screenshot.clone() {
            crate::actions::screenshot_flow(self, &ctx, &what, &path);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_state();
        if let Some(s) = self.server.take() {
            s.stop();
        }
        self.engine.shutdown();
    }
}

fn egui_extras_install_image_loaders(_ctx: &egui::Context) {
    // No image loaders are needed: icons are painted and screenshots are
    // uploaded as textures directly.
}

/// Open a URL or file with the system's default handler.
pub fn open_with_system(target: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", target]).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(target).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}
