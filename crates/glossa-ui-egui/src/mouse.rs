//! Mouse mode: select text with the mouse and get it translated, no hotkey.
//!
//! The watcher (`glossa_platform::mouse`) reports gestures that may have
//! selected something; [`selection_task`] then reads the selection through UI
//! Automation only — never a synthetic Ctrl+C, which on every drag would
//! clobber the clipboard and send keystrokes to whatever is under the mouse.
//! What happens next follows `mouse_mode.behaviour`: a small button by the
//! pointer (the classic default), the popup at once, or the popup read aloud.
//!
//! Like the hotkeys, the watcher lives as long as the process ([`MouseHub`]
//! moves between the shell and the GUI), so it works while the graphics are
//! unloaded.
//!
//! With `appearance.popup.close_on_outside_click` the watcher also reports
//! presses outside Glossa's windows, and such a press closes the popup and
//! the button instead of their timers. That works with mouse mode off too:
//! the watcher then runs for the presses alone.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{vec2, Sense};
use glossa_core::config::{CopyAction, MouseBehaviour};
use glossa_core::{Action, Config, LangId};
use glossa_platform::mouse::{Gesture, MouseWatcher};

use crate::actions::Task;
use crate::app::GlossaApp;
use crate::tray::WakeFn;

/// The watcher and the channel its gestures arrive on.
///
/// The watcher runs for mouse mode, for `close_on_outside_click`, or for
/// both. Selections are passed on only with mouse mode on, and presses only
/// while a popup or the button is on screen, so clicking about with nothing
/// of Glossa's showing never wakes it.
pub struct MouseHub {
    watcher: Option<MouseWatcher>,
    /// Each gesture with the moment the watcher saw it.
    tx: Sender<(Gesture, Instant)>,
    rx: Receiver<(Gesture, Instant)>,
    wake: WakeFn,
    /// Why the watcher did not start, the last time it was asked to; shown
    /// in Options → Advanced until it starts.
    error: Option<String>,
    /// Mouse mode as last synced, so a failure is reported once per attempt
    /// rather than on every settings change.
    wanted: bool,
    /// `close_on_outside_click`, where the system can do it, as last synced.
    outside: bool,
    /// A popup or the button is on screen.
    showing: bool,
}

impl MouseHub {
    pub fn new(wake: WakeFn) -> MouseHub {
        let (tx, rx) = mpsc::channel();
        MouseHub { watcher: None, tx, rx, wake, error: None, wanted: false, outside: false, showing: false }
    }

    /// Whether mouse mode can work on this system at all.
    pub fn supported() -> bool {
        MouseWatcher::supported()
    }

    /// Whether this system can close the popup on a click elsewhere.
    pub fn can_close_on_outside_click() -> bool {
        MouseWatcher::supported() && MouseWatcher::can_report_presses()
    }

    /// Whether mouse mode is running: what the tray's check mark shows.
    pub fn running(&self) -> bool {
        self.wanted && self.watcher.is_some()
    }

    /// Whether a press outside closes the popup right now: the setting is
    /// on, the system can tell, and the watcher runs.
    pub fn closes_on_outside_click(&self) -> bool {
        self.outside && self.watcher.is_some()
    }

    /// Why mouse mode is not running though it is switched on, if it failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Tell the watcher whether a popup or the button is on screen: presses
    /// are only wanted then.
    pub fn set_showing(&mut self, showing: bool) {
        if showing != self.showing {
            self.showing = showing;
            self.apply_presses();
        }
    }

    fn apply_presses(&self) {
        if let Some(w) = &self.watcher {
            w.report_presses(self.outside && self.showing);
        }
    }

    /// Start or stop watching to match the settings. Returns the reason when
    /// mouse mode was switched on just now and could not start, for the
    /// caller to tell the user.
    pub fn sync(&mut self, cfg: &Config) -> Option<String> {
        let enabled = cfg.mouse_mode.enabled;
        self.outside = cfg.appearance.popup.close_on_outside_click && Self::can_close_on_outside_click();
        let newly = enabled && !self.wanted;
        self.wanted = enabled;
        let needed = enabled || self.outside;
        let failed = match (needed, self.watcher.is_some()) {
            (true, false) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                match MouseWatcher::start(move |g| {
                    let _ = tx.send((g, Instant::now()));
                    wake();
                }) {
                    Ok(w) => {
                        self.watcher = Some(w);
                        self.error = None;
                        None
                    }
                    Err(e) => {
                        tracing::warn!("mouse watcher: {e}");
                        // Without mouse mode only the popup's closing is
                        // lost, and its timer still works: no message.
                        self.error = enabled.then(|| e.to_string());
                        (newly).then(|| e.to_string())
                    }
                }
            }
            (false, true) => {
                self.watcher = None;
                None
            }
            (false, false) => {
                self.error = None;
                None
            }
            (true, true) => None,
        };
        if !enabled {
            self.error = None;
        }
        self.apply_presses();
        failed
    }

    /// What arrived since the last call: selections only with mouse mode
    /// on (the watcher may be running for the popup alone).
    pub fn poll(&self) -> Vec<(Gesture, Instant)> {
        self.rx.try_iter().filter(|(g, _)| self.wanted || *g == Gesture::Pressed).collect()
    }
}

/// A press outside Glossa's windows: the popup and the button go, unless
/// the popup is pinned. A selection still being read from before the press
/// is dropped when it arrives (see `actions::on_task`), so the popup does
/// not come back after the user has clicked away.
pub fn on_press(app: &mut GlossaApp, at: Instant) {
    if !app.closes_on_outside_click() {
        return;
    }
    app.last_outside_press = Some(at);
    app.mouse_icon = None;
    if app.popup.as_ref().is_some_and(|p| !p.pinned) {
        tracing::debug!("popup closed by a click outside");
        app.popup = None;
    }
    app.ctx.request_repaint();
}

/// Turn a gesture into the selected text, or nothing. Blocks for a moment
/// (the application finishes updating its selection after the button comes
/// up); call it off the UI thread.
///
/// `seen`: when the watcher reported the gesture, carried into the task.
pub fn selection_task(cfg: &Config, gesture: Gesture, seen: Instant) -> Option<Task> {
    let m = &cfg.mouse_mode;
    let (text, cursor) = match gesture {
        Gesture::Pressed => return None,
        Gesture::Selected { at, ctrl } => {
            if m.ctrl_only && !ctrl {
                return None;
            }
            std::thread::sleep(Duration::from_millis(60));
            let mut quiet = cfg.clone();
            quiet.advanced.copy_action = CopyAction::AccessibilityOnly;
            match crate::actions::read_selection(&quiet, Action::ShowPopup)? {
                Task::Selection { text: Some(t), .. } => (t, Some(at)),
                _ => return None,
            }
        }
        Gesture::Text { text, at } => {
            let fg = glossa_platform::window::foreground_window();
            if fg.is_some_and(|w| !cfg.exceptions.allows_window(&w.process_name, &w.class_name, &w.focus_class)) {
                return None;
            }
            (text, at)
        }
    };
    let text = text.trim().to_owned();
    // Long passages are rarely meant: a drag across a whole page, say.
    if text.is_empty() || text.chars().count() > 5000 {
        return None;
    }
    Some(Task::Mouse { text, cursor, seen })
}

/// A small button by the pointer that opens the translation when clicked.
pub struct MouseIcon {
    pub text: String,
    pub cursor: Option<(i32, i32)>,
    pos: egui::Pos2,
    shown: Instant,
}

const ICON_TITLE: &str = "Glossa mouse";
const ICON_SIZE: f32 = 26.0;

/// What to do with text selected in mouse mode.
pub fn on_selection(app: &mut GlossaApp, text: String, cursor: Option<(i32, i32)>) {
    // The same selection again (a triple click after a double one, a
    // re-click) is not news.
    if app.last_mouse_text.as_ref().is_some_and(|(t, at)| *t == text && at.elapsed() < Duration::from_secs(3)) {
        return;
    }
    app.last_mouse_text = Some((text.clone(), Instant::now()));
    match app.cfg.mouse_mode.behaviour {
        MouseBehaviour::ShowTranslation => crate::popup::open(app, text, cursor),
        MouseBehaviour::ShowTranslationAndRead => {
            app.speak(&text, LangId::AUTO);
            crate::popup::open(app, text, cursor);
        }
        MouseBehaviour::ShowIcon => {
            let (x, y) = cursor.or_else(glossa_platform::window::cursor_position).unwrap_or((200, 200));
            let scale = glossa_platform::window::monitor_at(x, y).map(|m| m.desktop_scale()).unwrap_or(1.0);
            let pos = egui::pos2((x as f32 + 12.0 * scale) / scale, (y as f32 + 8.0 * scale) / scale);
            app.mouse_icon = Some(MouseIcon { text, cursor, pos, shown: Instant::now() });
            app.ctx.request_repaint();
        }
    }
}

/// Draw the button while there is one; it goes away after a few seconds,
/// or at the next press outside it with `close_on_outside_click`.
pub fn show_icon(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(icon) = app.mouse_icon.take() else { return };
    let life = Duration::from_secs(4);
    let timed = !app.closes_on_outside_click();
    if timed {
        if icon.shown.elapsed() > life {
            return;
        }
        ctx.request_repaint_after(life.saturating_sub(icon.shown.elapsed()));
    }
    let builder = egui::ViewportBuilder::default()
        .with_title(ICON_TITLE)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        // macOS's OpenGL refuses a translucent window unless the main window
        // was made with an alpha channel; it would not open at all.
        .with_transparent(cfg!(not(target_os = "macos")) || app.transparent_root())
        .with_active(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(icon.pos)
        .with_inner_size([ICON_SIZE, ICON_SIZE]);
    let look = app.look.clone();
    let mut clicked = false;
    let mut hovered = false;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(ICON_TITLE), builder, |ui, _| {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (rect, resp) = ui.allocate_exact_size(vec2(ICON_SIZE, ICON_SIZE), Sense::click());
            hovered = resp.hovered();
            let fill = if hovered { look.accent() } else { look.accent().gamma_multiply(0.85) };
            ui.painter().rect_filled(rect, 6.0, fill);
            crate::icons::paint(ui.painter(), rect.shrink(5.0), crate::icons::Icon::Translate, egui::Color32::WHITE);
            clicked = resp.on_hover_text(app.t("mouse.translate")).clicked();
        });
    });
    app.float_child(ICON_TITLE);
    if clicked {
        crate::popup::open(app, icon.text, icon.cursor);
        return;
    }
    // Keep it while the pointer is over it.
    let icon = if hovered { MouseIcon { shown: Instant::now(), ..icon } } else { icon };
    app.mouse_icon = Some(icon);
}
