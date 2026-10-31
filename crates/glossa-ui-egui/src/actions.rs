//! What each [`Action`] does.
//!
//! Anything that touches another program — reading its selection (which may
//! send it a copy keystroke and wait), pasting over it, grabbing the screen —
//! runs on a short-lived thread and reports back as a [`Task`], so the UI
//! never stalls for the hundreds of milliseconds those take.

use std::path::Path;
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
    Translated { action: Action, text: String, error: Option<String> },
    /// A step finished; tell the user how it went.
    Done { message: String, error: bool },
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
        Action::ShowKeyboard => app.keyboard_open = true,
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
                let i = enabled.iter().position(|s| *s == app.cfg.services.active).map(|i| (i + 1) % enabled.len()).unwrap_or(0);
                app.set_active_service(&enabled[i]);
            }
        }
        Action::ToggleMouseMode => {
            let on = !app.cfg.mouse_mode.enabled;
            let _ = app.engine.update_config(move |c| c.mouse_mode.enabled = on);
        }
        Action::SpeechInput => {
            let m = format!("{} — not available in this version.", app.t("action.speech_input"));
            app.notify(m, true);
        }
        Action::TranslateClipboard | Action::TranslateClipboardInMain | Action::TranslateClipboardInPopup => {
            let Some(clip) = app.clipboard_text() else {
                let m = app.t("msg.nothing_to_translate");
                app.notify(m, true);
                return;
            };
            if action == Action::TranslateClipboardInMain || (action == Action::TranslateClipboard && app.main_visible) {
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
        Action::TextRecognition | Action::RecognizeAndTranslate | Action::RecognizeToClipboard | Action::WatchRegion => {
            if let Some(w) = app.watch.take() {
                // Pressing the watch hotkey again stops watching.
                w.stop();
                if action == Action::WatchRegion {
                    app.notify("Stopped watching the region.", false);
                    return;
                }
            }
            grab_screen(app, action);
        }
        Action::RepeatLastRecognition => match app.last_region {
            Some(rect) => crate::ocr::recognize_region(app, rect),
            None => grab_screen(app, Action::TextRecognition),
        },
        // Everything else works on the selection: use the given text, or go
        // and fetch the selection first.
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

/// Read the selection in the foreground program on a worker thread.
fn capture(app: &mut GlossaApp, action: Action) {
    // Exceptions: leave terminals, games and the like alone.
    if let Some(win) = glossa_platform::window::foreground_window() {
        if !app.cfg.exceptions.allows(&win.process_name, &win.class_name) {
            tracing::debug!(program = %win.process_name, class = %win.class_name, "skipped: in the exception list");
            return;
        }
    }
    let opts = CaptureOptions::from_config(&app.cfg.advanced);
    let tx = app.tasks_tx.clone();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        let cursor = glossa_platform::window::cursor_position();
        let (text, error) = match glossa_platform::selection::capture_selection(&opts) {
            Ok(t) => (t.filter(|s| !s.trim().is_empty()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let _ = tx.send(Task::Selection { action, text, cursor, error });
        ctx.request_repaint();
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
            Err(e) => Task::Screenshot { action, image: image::RgbaImage::new(1, 1), origin: (0, 0), error: Some(e.to_string()) },
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
                app.notify(e, true);
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
            std::thread::spawn(move || {
                let _ = glossa_platform::selection::replace_selection(&converted);
                let _ = glossa_platform::layout::switch_keyboard_layout_of_foreground();
            });
        }
        Action::CopyTranslation | Action::ReplaceSelection | Action::ListenTranslation => {
            let engine = app.engine.clone();
            let tx = app.tasks_tx.clone();
            let ctx = app.ctx.clone();
            let opts = TranslateOptions { provider: Some(app.cfg.services.active.clone()).filter(|s| !s.is_empty()), ..Default::default() };
            std::thread::spawn(move || {
                let task = match engine.translate(&text, LangPair::new(LangId::AUTO, LangId::AUTO), &opts) {
                    Ok(out) => Task::Translated { action, text: out.translation.text, error: None },
                    Err(e) => Task::Translated { action, text: String::new(), error: Some(e.to_string()) },
                };
                let _ = tx.send(task);
                ctx.request_repaint();
            });
        }
        _ => {}
    }
}

/// Development aid behind `--screenshot WHAT`: open the named window once
/// at start, so it can be photographed from outside.
pub fn screenshot_flow(app: &mut GlossaApp, ctx: &egui::Context, what: &str, _path: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    match what {
        "options" => crate::options::open(app),
        "dictionary" => crate::dictionary::open(app, Some("serendipity".into())),
        "history" => crate::history::open(app),
        "about" => app.about_open = true,
        "keyboard" => app.keyboard_open = true,
        "popup" => crate::popup::open(app, "The quick brown fox jumps over the lazy dog.".into(), Some((400, 300))),
        _ => {
            if app.main.source.is_empty() {
                app.main.source = "Hello! This is Glossa, the resurrected QTranslate.".into();
            }
            app.translate_main();
        }
    }
    ctx.request_repaint();
}
