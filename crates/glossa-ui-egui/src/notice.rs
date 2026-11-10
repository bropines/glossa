//! Small notices that take no focus and go away by themselves: "Copied 120
//! characters" by an OCR region, and every message the app has while the
//! main window is hidden ("nothing to translate", OCR unavailable, a copy or
//! replace that failed), shown by the pointer.
//!
//! Glossa spends most of its life in the tray, so a message drawn inside the
//! main window (the toast) would go unseen after a hotkey. A notice is a
//! borderless, always-on-top window of its own that never takes the focus,
//! so the program the user is working in keeps it; a click dismisses it.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, RichText, Sense};
use glossa_core::ocr::Rect as GRect;

use crate::app::GlossaApp;

const TITLE: &str = "Glossa notice";
/// How long a notice stays. An error stays long enough to be read, which
/// depends on how much there is to read: see [`error_life`].
const LIFE: Duration = Duration::from_millis(3500);
const WIDTH: f32 = 360.0;
/// Most lines of the message shown; a longer one ends in an ellipsis (the
/// whole of it is in the log).
const MAX_ROWS: usize = 6;

/// How long an error stays: eight seconds to notice it and start reading,
/// and time to read it at an unhurried 15 characters a second, up to half a
/// minute. It never needs waiting out: a click puts it away.
fn error_life(chars: usize) -> Duration {
    Duration::from_millis(8000 + chars as u64 * 1000 / 15).min(Duration::from_secs(30))
}

pub struct Notice {
    text: String,
    /// A line or two under the message (the start of what was copied).
    preview: Option<String>,
    error: bool,
    /// Work still going on (a spinner, and no timeout).
    busy: bool,
    /// What it belongs to, in desktop pixels: it goes under this.
    region: GRect,
    shown: Instant,
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Option<isize>,
}

impl Notice {
    fn life(&self) -> Duration {
        if self.error {
            let preview = self.preview.as_ref().map_or(0, |p| p.chars().count());
            error_life(self.text.chars().count() + preview)
        } else {
            LIFE
        }
    }
}

/// Show a notice under `region` (desktop pixels), replacing any other.
pub fn show(app: &mut GlossaApp, region: GRect, text: String, preview: Option<String>, error: bool, busy: bool) {
    if let Some(p) = &preview {
        app.fonts.ensure(&app.ctx, p);
    }
    app.fonts.ensure(&app.ctx, &text);
    app.notice = Some(Notice { text, preview, error, busy, region, shown: Instant::now(), hwnd: None });
    app.ctx.request_repaint();
}

/// Show a notice by the mouse pointer: for messages with nowhere else to go.
/// `detail` is a line of small print under it (what a service said).
pub fn at_cursor(app: &mut GlossaApp, text: String, detail: Option<String>, error: bool) {
    let (x, y) = glossa_platform::window::cursor_position().unwrap_or((200, 200));
    // Under the pointer's arrow, which is about this tall at 100%.
    let scale = glossa_platform::window::monitor_at(x, y).map(|m| m.desktop_scale()).unwrap_or(1.0);
    let region = GRect::new(x as f32, y as f32, 0.0, 16.0 * scale);
    show(app, region, text, detail, error, false);
}

/// Drop the notice once its time is up.
pub fn tick(app: &mut GlossaApp) {
    if let Some(n) = &app.notice {
        if !n.busy && n.shown.elapsed() > n.life() {
            app.notice = None;
        }
    }
}

/// Draw the notice, if there is one.
pub fn draw(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(n) = app.notice.as_ref() else { return };
    if !n.busy {
        ctx.request_repaint_after(n.life().saturating_sub(n.shown.elapsed()));
    }
    let look = app.look.clone();
    let (cx, cy) = n.region.centre();
    let monitor = glossa_platform::window::monitor_at(cx as i32, cy as i32)
        .or_else(|| glossa_platform::window::monitors().into_iter().find(|m| m.is_primary));
    let scale = monitor.as_ref().map(|m| m.desktop_scale()).unwrap_or(1.0);

    // The message wraps (an error may run to a few lines); measure it so the
    // window is made the right size from the start.
    let margin = vec2(10.0, 6.0);
    let spinner = if n.busy { 14.0 + ctx.global_style().spacing.item_spacing.x } else { 0.0 };
    let text_w = WIDTH - 2.0 * margin.x - spinner;
    let body = egui::TextStyle::Body.resolve(&ctx.global_style());
    let small = egui::TextStyle::Small.resolve(&ctx.global_style());
    // Laid out here, in the colour it is drawn in, and drawn as it was
    // measured: cut after `MAX_ROWS` lines with an ellipsis, not clipped
    // mid-line by the window's edge.
    let colour = if n.error { look.error() } else { look.popup_text };
    let mut job = egui::text::LayoutJob::simple(n.text.clone(), body.clone(), colour, text_w);
    job.wrap.max_rows = MAX_ROWS;
    job.wrap.overflow_character = Some('…');
    let galley = ctx.fonts_mut(|f| f.layout_job(job));
    let text_h = galley.size().y;
    let preview_h = if n.preview.is_some() { small.size + 6.0 } else { 0.0 };
    let size = vec2(WIDTH, (text_h + preview_h + 2.0 * margin.y + 4.0).ceil());
    let pos = notice_pos(n.region, size * scale, monitor.map(|m| m.work_area)) / scale;
    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        // Rounded corners need the corners see-through; macOS's OpenGL makes
        // a translucent window only when the main one has an alpha channel.
        .with_transparent(!glossa_platform::shell::SHELL.transparency_needs_alpha_root || app.transparent_root())
        .with_active(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(pos)
        .with_inner_size(size);
    let mut dismissed = false;
    let (preview, error, busy) = (n.preview.clone(), n.error, n.busy);
    let radius = look.theme.metrics.overlay_radius.max(look.theme.metrics.button_radius).clamp(4.0, 12.0);
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(TITLE), builder, |ui, _| {
        let frame = egui::Frame::new()
            .fill(look.popup_back)
            .stroke(egui::Stroke::new(1.0, if error { look.error() } else { look.accent() }))
            .corner_radius(radius)
            .inner_margin(egui::Margin::symmetric(margin.x as i8, margin.y as i8));
        egui::CentralPanel::no_frame().frame(frame).show(ui, |ui| {
            let resp = ui.interact(ui.max_rect(), egui::Id::unique("notice-click"), Sense::click());
            ui.horizontal_top(|ui| {
                if busy {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                ui.label(galley.clone());
            });
            if let Some(p) = &preview {
                ui.add(egui::Label::new(RichText::new(p).small().color(look.weak_text())).truncate());
            }
            dismissed = resp.clicked();
        });
    });
    keep_from_activating(app);
    app.float_child(TITLE);
    if dismissed {
        app.notice = None;
    }
}

/// Make a click on the notice not activate it, so dismissing it leaves the
/// focus where it was (where the system allows it: Windows).
fn keep_from_activating(app: &mut GlossaApp) {
    let Some(n) = app.notice.as_mut() else { return };
    if n.hwnd.is_none() {
        n.hwnd = glossa_platform::window::own_window_by_title(TITLE);
    }
    if let Some(hwnd) = n.hwnd {
        glossa_platform::window::keep_from_activating(hwnd);
    }
}

/// Where the notice goes, in desktop pixels: under the region's left edge,
/// else above it, kept inside the work area.
fn notice_pos(region: GRect, size: egui::Vec2, work: Option<GRect>) -> egui::Pos2 {
    let work = work.unwrap_or(GRect::new(f32::MIN / 4.0, f32::MIN / 4.0, f32::MAX / 2.0, f32::MAX / 2.0));
    let gap = 8.0;
    let mut y = region.y + region.height + gap;
    if y + size.y > work.y + work.height {
        y = region.y - size.y - gap;
    }
    if y < work.y {
        y = work.y + work.height - size.y - gap;
    }
    let x = region.x.min(work.x + work.width - size.x).max(work.x);
    pos2(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_sits_under_the_region_or_above_it_near_the_bottom() {
        let work = Some(GRect::new(0.0, 0.0, 1920.0, 1040.0));
        let size = vec2(340.0, 64.0);
        assert_eq!(notice_pos(GRect::new(100.0, 100.0, 300.0, 50.0), size, work), pos2(100.0, 158.0));
        // Too close to the taskbar: above.
        assert_eq!(notice_pos(GRect::new(100.0, 950.0, 300.0, 50.0), size, work), pos2(100.0, 878.0));
        // Too close to the right edge: pulled in.
        assert_eq!(notice_pos(GRect::new(1800.0, 100.0, 100.0, 50.0), size, work).x, 1580.0);
    }

    #[test]
    fn an_error_stays_long_enough_to_read() {
        assert!(error_life(0) >= Duration::from_secs(8));
        // Two lines of text: more than the six seconds it used to get.
        assert!(error_life(120) >= Duration::from_secs(15));
        assert!(error_life(120) > error_life(40));
        assert_eq!(error_life(100_000), Duration::from_secs(30));
    }

    #[test]
    fn a_notice_by_the_pointer_goes_under_it() {
        let work = Some(GRect::new(0.0, 0.0, 1920.0, 1040.0));
        let at_pointer = GRect::new(500.0, 400.0, 0.0, 16.0);
        assert_eq!(notice_pos(at_pointer, vec2(360.0, 40.0), work), pos2(500.0, 424.0));
    }
}
