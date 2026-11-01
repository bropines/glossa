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
pub struct MouseHub {
    watcher: Option<MouseWatcher>,
    tx: Sender<Gesture>,
    rx: Receiver<Gesture>,
    wake: WakeFn,
    pub error: Option<String>,
}

impl MouseHub {
    pub fn new(wake: WakeFn) -> MouseHub {
        let (tx, rx) = mpsc::channel();
        MouseHub { watcher: None, tx, rx, wake, error: None }
    }

    /// Start or stop watching to match the setting.
    pub fn sync(&mut self, enabled: bool) {
        match (enabled, self.watcher.is_some()) {
            (true, false) => {
                let (tx, wake) = (self.tx.clone(), self.wake.clone());
                match MouseWatcher::start(move |g| {
                    let _ = tx.send(g);
                    wake();
                }) {
                    Ok(w) => {
                        self.watcher = Some(w);
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::warn!("mouse mode: {e}");
                        self.error = Some(e.to_string());
                    }
                }
            }
            (false, true) => self.watcher = None,
            _ => {}
        }
    }

    pub fn poll(&self) -> Vec<Gesture> {
        self.rx.try_iter().collect()
    }
}

/// Turn a gesture into the selected text, or nothing. Blocks for a moment
/// (the application finishes updating its selection after the button comes
/// up); call it off the UI thread.
pub fn selection_task(cfg: &Config, gesture: Gesture) -> Option<Task> {
    let m = &cfg.mouse_mode;
    let (text, cursor) = match gesture {
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
    Some(Task::Mouse { text, cursor })
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

/// Draw the button while there is one; it goes away after a few seconds.
pub fn show_icon(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(icon) = app.mouse_icon.take() else { return };
    let life = Duration::from_secs(4);
    if icon.shown.elapsed() > life {
        return;
    }
    ctx.request_repaint_after(life.saturating_sub(icon.shown.elapsed()));
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
    if clicked {
        crate::popup::open(app, icon.text, icon.cursor);
        return;
    }
    // Keep it while the pointer is over it.
    let icon = if hovered { MouseIcon { shown: Instant::now(), ..icon } } else { icon };
    app.mouse_icon = Some(icon);
}
