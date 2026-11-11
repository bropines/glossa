//! The message a window shows at its bottom edge: "Copied", or an error.
//!
//! A message belongs to the window that raised it. [`GlossaApp::notify`]
//! files it under the viewport being drawn at that moment, and every window
//! with a frame of its own (the main window, Options, Dictionary, History,
//! the module editor, module panels, the OCR result, Welcome, About, the
//! keyboard) calls [`draw`]. A message raised in a window that draws none
//! (the popup, an overlay), or in one that closed before it could show it,
//! goes by the pointer instead ([`crate::notice`]), so none is lost.
//!
//! "Copied" goes away by itself. An error stays until it is closed: it is
//! often read a moment late, and it can be copied for a bug report.

use std::time::{Duration, Instant};

use egui::{vec2, RichText};

use crate::app::GlossaApp;

/// How long a message that is not an error stays.
const LIFE: Duration = Duration::from_secs(4);
const MAX_WIDTH: f32 = 460.0;
/// How far above the window's bottom edge it sits: clear of what windows
/// keep there (the main window's service strip, a row of OK and Cancel).
const LIFT: f32 = 58.0;

pub struct Toast {
    pub text: String,
    /// What the service or the system said, in small print.
    pub detail: Option<String>,
    pub error: bool,
    at: Instant,
    /// The frame it was raised in, and the last frame a window drew it in.
    born: u64,
    drawn: Option<u64>,
}

/// What [`sweep`] does with a message after a frame.
#[derive(Debug, PartialEq, Eq)]
enum Fate {
    Keep,
    /// Its window is gone, and had shown it.
    Drop,
    /// No window ever showed it: it goes by the pointer.
    Pointer,
}

impl Toast {
    pub fn new(text: String, detail: Option<String>, error: bool, frame: u64) -> Self {
        Toast { text, detail, error, at: Instant::now(), born: frame, drawn: None }
    }

    /// The whole message, for the clipboard.
    fn full_text(&self) -> String {
        match &self.detail {
            Some(d) => format!("{}\n{d}", self.text),
            None => self.text.clone(),
        }
    }

    fn fate(&self, frame: u64) -> Fate {
        match self.drawn {
            Some(f) if f == frame => Fate::Keep,
            // Raised after its window drew its message in this frame (or by
            // an event, before the frame): the next frame shows it.
            _ if self.born >= frame => Fate::Keep,
            Some(_) => Fate::Drop,
            None => Fate::Pointer,
        }
    }
}

/// Let the messages that are not errors run out.
pub fn tick(app: &mut GlossaApp, ctx: &egui::Context) {
    app.toasts.retain(|_, t| t.error || t.at.elapsed() <= LIFE);
    if app.toasts.values().any(|t| !t.error) {
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

/// After every window has been drawn: a message no window showed goes by
/// the pointer, and one whose window has closed goes away with it.
pub fn sweep(app: &mut GlossaApp) {
    let frame = app.frame_number();
    let gone: Vec<egui::ViewportId> =
        app.toasts.iter().filter(|(_, t)| t.fate(frame) != Fate::Keep).map(|(id, _)| *id).collect();
    for id in gone {
        let Some(t) = app.toasts.remove(&id) else { continue };
        if t.fate(frame) == Fate::Pointer {
            crate::notice::at_cursor(app, t.text, t.detail, t.error);
        }
    }
}

/// Draw the message of the window being drawn, if it has one.
pub fn draw(app: &mut GlossaApp, ctx: &egui::Context) {
    let id = ctx.viewport_id();
    let frame = app.frame_number();
    let Some(t) = app.toasts.get_mut(&id) else { return };
    t.drawn = Some(frame);
    let (text, detail, error) = (t.text.clone(), t.detail.clone(), t.error);
    let look = app.look.clone();
    let (copy, copy_tip, close) = (app.t("toast.copy"), app.t("toast.copy_tip"), app.t("popup.close"));
    let width = (ctx.content_rect().width() - 48.0).clamp(120.0, MAX_WIDTH);
    let (mut copied, mut closed) = (false, false);
    egui::Area::new(egui::Id::unique("toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -LIFT))
        .order(egui::Order::Foreground)
        // Only an error has buttons; "Copied" must not swallow a click
        // meant for what is under it.
        .interactable(error)
        .show(ctx, |ui| {
            let mut frame = egui::Frame::popup(ui.style()).corner_radius(ui.visuals().menu_corner_radius);
            if error {
                frame = frame.stroke(egui::Stroke::new(1.0, look.error()));
            }
            frame.show(ui, |ui| {
                ui.set_max_width(width);
                let colour = if error { look.error() } else { look.text() };
                ui.add(egui::Label::new(RichText::new(&text).color(colour)).wrap());
                if let Some(d) = &detail {
                    ui.add(egui::Label::new(RichText::new(d).small().color(look.weak_text())).wrap());
                }
                if error {
                    ui.horizontal(|ui| {
                        copied = ui.small_button(&copy).on_hover_text(&copy_tip).clicked();
                        closed = ui.small_button(&close).clicked();
                    });
                }
            });
        });
    if copied {
        let full = app.toasts.get(&id).map(Toast::full_text).unwrap_or_default();
        app.copy_text_quietly(&full);
    }
    if closed {
        app.toasts.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_no_window_showed_goes_by_the_pointer_one_frame_later() {
        let mut t = Toast::new("x".into(), None, true, 7);
        assert_eq!(t.fate(7), Fate::Keep, "its window may draw it in the next frame");
        assert_eq!(t.fate(8), Fate::Pointer);
        t.drawn = Some(8);
        assert_eq!(t.fate(8), Fate::Keep);
        assert_eq!(t.fate(9), Fate::Drop, "its window closed after showing it");
    }

    #[test]
    fn copying_takes_the_small_print_too() {
        let t = Toast::new("DeepL answered with an error (HTTP 400).".into(), Some("bad target".into()), true, 1);
        assert_eq!(t.full_text(), "DeepL answered with an error (HTTP 400).\nbad target");
        assert_eq!(Toast::new("Copied".into(), None, false, 1).full_text(), "Copied");
    }
}
