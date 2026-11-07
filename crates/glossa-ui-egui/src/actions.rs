//! What each [`Action`] does.
//!
//! Anything that touches another program — reading its selection (which may
//! send it a copy keystroke and wait), pasting over it, grabbing the screen —
//! runs on a short-lived thread and reports back as a [`Task`], so the UI
//! never stalls for the hundreds of milliseconds those take.

use std::time::Duration;

use glossa_core::{Action, LangId, LangPair};
use glossa_engine::TranslateOptions;
use glossa_platform::selection::CaptureOptions;

use crate::app::GlossaApp;

/// A finished background step.
pub enum Task {
    /// The selection (or the reason there is none) for an action.
    Selection { action: Action, text: Option<String>, cursor: Option<(i32, i32)>, error: Option<String> },
    /// The whole desktop, for the OCR overlay.
    Screenshot { action: Action, image: image::RgbaImage, origin: (i32, i32), error: Option<String> },
    /// A translation made for an action with no window (copy, replace, listen).
    Translated { action: Action, text: String, error: Option<glossa_core::Error> },
    /// A step finished; tell the user how it went.
    Done { message: String, error: bool },
    /// Text selected with the mouse in mouse mode.
    Mouse { text: String, cursor: Option<(i32, i32)> },
}

pub fn perform(app: &mut GlossaApp, action: Action, text: Option<String>) {
    tracing::debug!(action = action.id(), with_text = text.is_some(), "action");
    match action {
        Action::ShowMainWindow => {
            if let Some(t) = text {
                app.main.source = t;
                app.translate_main();
            }
            app.show_main();
        }
        Action::ShowOptions => crate::options::open(app),
        Action::ShowDictionary => crate::dictionary::open(app, text),
        Action::ShowHistory => crate::history::open(app),
        Action::ShowKeyboard => app.open_keyboard(),
        Action::Quit => app.quit(),
        Action::SwapLanguages => {
            let pair = app.cfg.general.pair;
            if pair.is_swappable() {
                app.set_pair(pair.swapped());
            }
        }
        Action::NextService => {
            let enabled = app.cfg.services.enabled.clone();
            if !enabled.is_empty() {
                let i = enabled
                    .iter()
                    .position(|s| *s == app.cfg.services.active)
                    .map(|i| (i + 1) % enabled.len())
                    .unwrap_or(0);
                app.set_active_service(&enabled[i]);
            }
        }
        Action::ToggleMouseMode => {
            if !crate::mouse::MouseHub::supported() {
                let m = app.t("mouse.unavailable");
                app.notify(m, true);
                return;
            }
            let on = !app.cfg.mouse_mode.enabled;
            let _ = app.engine.update_config(move |c| c.mouse_mode.enabled = on);
        }
        Action::SpeechInput => {
            let m = app.locale.fmt("msg.not_available", &app.t("action.speech_input"));
            app.notify(m, true);
        }
        Action::TranslateClipboard | Action::TranslateClipboardInMain | Action::TranslateClipboardInPopup => {
            let Some(clip) = app.clipboard_text() else {
                let m = app.t("msg.nothing_to_translate");
                app.notify(m, true);
                return;
            };
            if action == Action::TranslateClipboardInMain || (action == Action::TranslateClipboard && app.main_visible)
            {
                app.main.source = clip;
                app.translate_main();
                app.show_main();
            } else {
                crate::popup::open(app, clip, None);
            }
        }
        Action::DictionaryFromClipboard => {
            let clip = app.clipboard_text();
            crate::dictionary::open(app, clip);
        }
        Action::TextRecognition
        | Action::RecognizeAndTranslate
        | Action::RecognizeToClipboard
        | Action::TranslateRegionInPlace
        | Action::WatchRegion => {
            if let Some(w) = app.watch.take() {
                // Pressing the watch hotkey again stops watching.
                w.stop();
                if action == Action::WatchRegion {
                    let m = app.t("msg.watch_stopped");
                    app.notify(m, false);
                    return;
                }
            }
            grab_screen(app, action);
        }
        Action::TranslateScreenInPlace => grab_monitor(app, action),
        Action::RepeatLastRecognition => match app.last_region {
            Some(rect) => crate::ocr::recognize_region(app, rect),
            None => grab_screen(app, Action::TextRecognition),
        },
        // Everything else works on the selection: use the given text, or go
        // and fetch the selection first. (Keep in step with `needs_selection`.)
        Action::ShowPopup
        | Action::ListenSelection
        | Action::CopyTranslation
        | Action::ReplaceSelection
        | Action::ConvertLayout
        | Action::ListenTranslation => match text {
            Some(t) => with_selection(app, action, t, None),
            None => capture(app, action),
        },
    }
}

/// The actions that work on the selection in the foreground program.
pub fn needs_selection(action: Action) -> bool {
    matches!(
        action,
        Action::ShowPopup
            | Action::ListenSelection
            | Action::CopyTranslation
            | Action::ReplaceSelection
            | Action::ConvertLayout
            | Action::ListenTranslation
    )
}

/// Read the selection in the foreground program, blocking. `None` when the
/// program is in the exception list and must be left alone.
///
/// While the GUI sits unloaded this runs straight from the idle loop, before
/// any window exists: creating the GUI's windows disturbs the focus, and the
/// selection is gone by the time they are up.
pub fn read_selection(cfg: &glossa_core::Config, action: Action) -> Option<Task> {
    // Exceptions: leave terminals, games and the like alone.
    let foreground = glossa_platform::window::foreground_window();
    tracing::debug!(window = ?foreground.as_ref().map(|w| (&w.process_name, &w.class_name, &w.focus_class)), "capturing the selection");
    if let Some(win) = foreground {
        if !cfg.exceptions.allows_window(&win.process_name, &win.class_name, &win.focus_class) {
            tracing::debug!(program = %win.process_name, class = %win.class_name, "skipped: in the exception list");
            return None;
        }
    }
    let opts = CaptureOptions::from_config(&cfg.advanced);
    let cursor = glossa_platform::window::cursor_position();
    let started = std::time::Instant::now();
    let (text, error) = match glossa_platform::selection::capture_selection(&opts) {
        Ok(t) => (t.filter(|s| !s.trim().is_empty()), None),
        Err(e) => (None, Some(e.to_string())),
    };
    tracing::debug!(
        chars = text.as_ref().map(|t| t.chars().count()),
        ?error,
        ms = started.elapsed().as_millis() as u64,
        "selection captured"
    );
    Some(Task::Selection { action, text, cursor, error })
}

/// [`read_selection`] for an action a click started where the click took
/// the focus (a Windows tray icon activates the taskbar): the window marked
/// before the click gets it back first, so the selection is read from it.
pub fn read_selection_after_click(
    cfg: &glossa_core::Config,
    action: Action,
    mark: Option<glossa_platform::window::FocusMark>,
) -> Option<Task> {
    if let Some(m) = mark {
        if glossa_platform::window::refocus(m) {
            // Give the window a moment to take the focus before the copy
            // keystroke goes to it.
            std::thread::sleep(std::time::Duration::from_millis(80));
        } else {
            tracing::debug!("could not give the focus back before reading the selection");
        }
    }
    read_selection(cfg, action)
}

/// [`read_selection_after_click`] on a worker thread, for a tray click
/// while the GUI is up.
pub fn capture_after_click(app: &mut GlossaApp, action: Action, mark: Option<glossa_platform::window::FocusMark>) {
    let cfg = app.cfg.clone();
    let tx = app.tasks_tx.clone();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        if let Some(task) = read_selection_after_click(&cfg, action, mark) {
            let _ = tx.send(task);
            ctx.request_repaint();
        }
    });
}

/// Read the selection on a worker thread.
fn capture(app: &mut GlossaApp, action: Action) {
    let cfg = app.cfg.clone();
    let tx = app.tasks_tx.clone();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        if let Some(task) = read_selection(&cfg, action) {
            let _ = tx.send(task);
            ctx.request_repaint();
        }
    });
}

fn grab_screen(app: &mut GlossaApp, action: Action) {
    let tx = app.tasks_tx.clone();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        // Let the hotkey's own key-up and any menu close before the shot.
        std::thread::sleep(Duration::from_millis(120));
        let task = match glossa_platform::capture::capture_virtual_desktop() {
            Ok((image, x, y)) => Task::Screenshot { action, image, origin: (x, y), error: None },
            Err(e) => Task::Screenshot {
                action,
                image: image::RgbaImage::new(1, 1),
                origin: (0, 0),
                error: Some(e.to_string()),
            },
        };
        let _ = tx.send(task);
        ctx.request_repaint();
    });
}

/// Capture the monitor under the pointer (the primary one when the pointer
/// cannot be told), for translating everything on it in place.
fn grab_monitor(app: &mut GlossaApp, action: Action) {
    let tx = app.tasks_tx.clone();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        // Let the hotkey's own key-up and any menu close before the shot.
        std::thread::sleep(Duration::from_millis(120));
        use glossa_platform::window;
        let monitor = window::cursor_position()
            .and_then(|(x, y)| window::monitor_at(x, y))
            .or_else(|| window::monitors().into_iter().find(|m| m.is_primary))
            .or_else(|| window::monitors().into_iter().next());
        let task = match monitor {
            Some(m) => match glossa_platform::capture::capture_rect(m.bounds()) {
                Ok(image) => Task::Screenshot { action, image, origin: (m.x, m.y), error: None },
                Err(e) => Task::Screenshot {
                    action,
                    image: image::RgbaImage::new(1, 1),
                    origin: (0, 0),
                    error: Some(e.to_string()),
                },
            },
            None => Task::Screenshot {
                action,
                image: image::RgbaImage::new(1, 1),
                origin: (0, 0),
                error: Some("no monitors found to capture".into()),
            },
        };
        let _ = tx.send(task);
        ctx.request_repaint();
    });
}

pub fn on_task(app: &mut GlossaApp, task: Task) {
    match task {
        Task::Selection { action, text, cursor, error } => match (text, error) {
            (Some(t), _) => with_selection(app, action, t, cursor),
            (None, Some(e)) => app.notify(e, true),
            (None, None) => {
                // Nothing selected: the classic fallback was the clipboard.
                if action == Action::ShowPopup {
                    if let Some(clip) = app.clipboard_text() {
                        crate::popup::open(app, clip, cursor);
                        return;
                    }
                }
                let m = app.t("msg.nothing_to_translate");
                app.notify(m, true);
            }
        },
        Task::Screenshot { action, image, origin, error } => match error {
            Some(e) => {
                let m = app.locale.fmt("msg.ocr_unavailable", &e);
                app.notify(m, true);
            }
            None => crate::ocr::begin(app, action, image, origin),
        },
        Task::Translated { action, text, error } => {
            if let Some(e) = error {
                let m = app.describe(&e);
                app.notify(m, true);
                return;
            }
            match action {
                Action::CopyTranslation => app.copy_text(&text),
                Action::ReplaceSelection => {
                    let tx = app.tasks_tx.clone();
                    let ctx = app.ctx.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = glossa_platform::selection::replace_selection(&text) {
                            let _ = tx.send(Task::Done { message: e.to_string(), error: true });
                            ctx.request_repaint();
                        }
                    });
                }
                Action::ListenTranslation => app.speak(&text, LangId::AUTO),
                _ => {}
            }
        }
        Task::Done { message, error } => app.notify(message, error),
        Task::Mouse { text, cursor } => crate::mouse::on_selection(app, text, cursor),
    }
}

/// Act on text that was selected (or handed over by the control API).
fn with_selection(app: &mut GlossaApp, action: Action, text: String, cursor: Option<(i32, i32)>) {
    match action {
        Action::ShowPopup => crate::popup::open(app, text, cursor),
        Action::ListenSelection => app.speak(&text, LangId::AUTO),
        Action::ConvertLayout => {
            let converted = glossa_platform::layout::convert_layout(&text);
            if converted == text {
                return;
            }
            let tx = app.tasks_tx.clone();
            let ctx = app.ctx.clone();
            std::thread::spawn(move || {
                if let Err(e) = glossa_platform::selection::replace_selection(&converted) {
                    let _ = tx.send(Task::Done { message: e.to_string(), error: true });
                    ctx.request_repaint();
                    return;
                }
                let _ = glossa_platform::layout::switch_keyboard_layout_of_foreground();
            });
        }
        Action::CopyTranslation | Action::ReplaceSelection | Action::ListenTranslation => {
            let engine = app.engine.clone();
            let tx = app.tasks_tx.clone();
            let ctx = app.ctx.clone();
            let opts = TranslateOptions {
                provider: Some(app.cfg.services.active.clone()).filter(|s| !s.is_empty()),
                ..Default::default()
            };
            std::thread::spawn(move || {
                let task = match engine.translate(&text, LangPair::new(LangId::AUTO, LangId::AUTO), &opts) {
                    Ok(out) => Task::Translated { action, text: out.translation.text, error: None },
                    Err(e) => Task::Translated { action, text: String::new(), error: Some(e) },
                };
                let _ = tx.send(task);
                ctx.request_repaint();
            });
        }
        _ => {}
    }
}
