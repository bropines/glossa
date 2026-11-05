//! Screen-region OCR: the overlay and its toolbar, the copy mode's notice
//! (a [`crate::notice`]), and watching a region. What was read is shown in
//! [`crate::ocr_result`].
//!
//! The overlay is a borderless window over the whole desktop showing a
//! screenshot taken just before it opened, dimmed except for the rectangle
//! the user drags. What releasing the mouse does depends on the action:
//!
//! - **text recognition** follows `ocr.after_select`: show the text (and
//!   its translation) in the result window, copy it, translate it in place, or
//!   offer a toolbar to choose (Snipping Tool style, with keys: Enter for
//!   the default, C copy, T translate, I in place, Esc cancel);
//! - **recognise to clipboard** closes the overlay at once, recognises in
//!   the background and puts the text on the clipboard, with a small notice
//!   by the region that takes no focus;
//! - **recognise and translate** opens the result window with the text and
//!   its translation;
//! - **translate in place** closes the overlay and opens
//!   [`crate::inplace`] over the region when the translation is ready;
//! - **watch** keeps re-reading the same rectangle and pops up a
//!   translation whenever its pixels change — subtitles for a game or a
//!   video.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use egui::{pos2, vec2, Color32, Rect, RichText, Sense};
use glossa_core::config::OcrAfterSelect;
use glossa_core::ocr::Rect as GRect;
use glossa_core::{Action, Error, LangId};
use glossa_engine::{OcrOutcome, RecognizeOptions, Slot};

use crate::icons::{self, Icon};

use crate::app::GlossaApp;

pub enum Phase {
    Selecting,
    /// A region is selected and the toolbar asks what to do with it.
    Choosing,
}

/// What to do with a selected region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Recognise and show the text (and a translation, when so set).
    Show,
    /// Recognise and show the text with its translation.
    Translate,
    Copy,
    InPlace,
    CopyImage,
    SaveImage,
    Cancel,
}

pub struct OcrState {
    pub action: Action,
    pub image: image::RgbaImage,
    /// Where the screenshot's top-left is on the virtual desktop, in pixels.
    pub origin: (i32, i32),
    pub scale: f32,
    texture: Option<egui::TextureHandle>,
    drag_start: Option<egui::Pos2>,
    /// The selection, in overlay points.
    pub selection: Option<Rect>,
    pub phase: Phase,
    pub lang: LangId,
    /// Where the window was asked to be, in desktop points.
    wanted: egui::Pos2,
    /// How far the window sits from `wanted`, in points: macOS keeps a
    /// window below its menu bar until it is lifted above it. The
    /// screenshot is drawn that much the other way, so it lies over the
    /// screen it was taken from, and selections are read through it.
    offset: egui::Vec2,
    /// Whether the window has been lifted above the menu bar and taken out
    /// of Stage Manager.
    lifted: bool,
    /// How many times the window has been put back where it belongs.
    replaced: u32,
}

/// A recognition running after the overlay closed: copy mode, or in place.
pub struct Background {
    job: u64,
    in_place: bool,
    region: GRect,
    /// The captured region, for painting the translation over.
    crop: Option<image::RgbaImage>,
}

const ID: &str = "glossa-ocr";
const TITLE: &str = "Glossa OCR";

pub fn begin(app: &mut GlossaApp, action: Action, image: image::RgbaImage, origin: (i32, i32)) {
    if action == Action::TranslateScreenInPlace {
        // The whole monitor whose top-left is `origin`: nothing to select.
        let monitor = glossa_platform::window::monitors().into_iter().find(|m| (m.x, m.y) == origin);
        let region = match monitor {
            Some(m) => m.bounds(),
            None => GRect::new(origin.0 as f32, origin.1 as f32, image.width() as f32, image.height() as f32),
        };
        start_in_place(app, image, region, app.cfg.ocr.language);
        return;
    }
    let scale = glossa_platform::window::monitor_at(origin.0, origin.1)
        .or_else(|| glossa_platform::window::monitors().into_iter().find(|m| m.is_primary))
        .map(|m| m.scale)
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0);
    // The origin is in desktop coordinates (points on macOS), the image in
    // pixels.
    let d = glossa_platform::window::desktop_scale(scale);
    let wanted = pos2(origin.0 as f32 / d, origin.1 as f32 / d);
    app.ocr = Some(OcrState {
        action,
        image,
        origin,
        scale,
        texture: None,
        drag_start: None,
        selection: None,
        phase: Phase::Selecting,
        lang: app.cfg.ocr.language,
        wanted,
        offset: egui::Vec2::ZERO,
        lifted: false,
        replaced: 0,
    });
    // An engine with a slow first call (Apple Vision after the Mac starts)
    // gets ready while the user drags.
    app.engine.warm_up_ocr();
    app.ctx.request_repaint();
}

/// Recognise an image file dropped on the main window.
pub fn recognize_bytes(app: &mut GlossaApp, bytes: Vec<u8>, translate_to: Option<LangId>) {
    let job = app.engine.recognize_async(Slot::Ocr, bytes, app.cfg.ocr.language, translate_to);
    app.main.busy = true;
    // Results for a dropped image land in the main window.
    DROPPED.store(job, Ordering::SeqCst);
}

static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn translate_target(app: &GlossaApp) -> Option<LangId> {
    if !app.cfg.ocr.translate_after {
        return None;
    }
    Some(target_language(app))
}

/// The language to translate recognised text into: the main window's
/// target, or the first language when that is "auto".
pub(crate) fn target_language(app: &GlossaApp) -> LangId {
    let to = app.cfg.general.pair.to;
    if to.is_language() {
        to
    } else {
        app.cfg.general.first_language
    }
}

pub fn on_result(app: &mut GlossaApp, job: u64, result: Result<Arc<OcrOutcome>, Arc<Error>>, message: Option<String>) {
    if job != 0 && job == DROPPED.load(Ordering::SeqCst) {
        app.main.busy = false;
        match result {
            Ok(o) => {
                app.main.source = o.ocr.text.clone();
                app.main.translated_source = o.ocr.text.clone();
                if let Some(t) = &o.translation {
                    app.main.outcome = Some(Arc::new(t.clone()));
                } else {
                    app.main.translated_source.clear();
                    app.translate_main();
                }
            }
            Err(_) => app.main.error = message,
        }
        return;
    }
    if let Some(w) = &app.watch {
        if job == w.job.load(Ordering::SeqCst) {
            w.busy.store(false, Ordering::SeqCst);
            if let Ok(o) = result {
                let near = w.anchor;
                let source = o.ocr.text.clone();
                let tr = o.translation.clone().map(Arc::new);
                crate::popup::show_text(app, source, tr, near);
            }
            return;
        }
    }
    if app.ocr_background.as_ref().is_some_and(|b| b.job == job) {
        if let Some(bg) = app.ocr_background.take() {
            background_done(app, bg, result, message);
        }
        return;
    }
    if crate::ocr_result::owns_job(app, job) {
        crate::ocr_result::on_recognized(app, result, message);
    }
}

/// A copy-mode or in-place recognition finished.
fn background_done(
    app: &mut GlossaApp,
    bg: Background,
    result: Result<Arc<OcrOutcome>, Arc<Error>>,
    message: Option<String>,
) {
    let o = match result {
        Ok(o) => o,
        Err(_) => {
            notice(app, bg.region, message.unwrap_or_default(), None, true, false);
            return;
        }
    };
    if !bg.in_place {
        let text = glossa_engine::ocr::copy_text(&o.ocr, app.cfg.ocr.copy_joined_lines);
        copy_with_notice(app, &text, bg.region);
        return;
    }
    app.notice = None;
    match bg.crop {
        Some(crop) if !o.blocks.is_empty() => crate::inplace::open(app, crop, bg.region, o),
        _ => {
            // Nothing with a place to paint on (an engine that gives text
            // only): show the translation in the popup instead.
            let near = (bg.region.x as i32, (bg.region.y + bg.region.height) as i32);
            match o.translation.clone() {
                Some(t) => crate::popup::show_text(app, o.ocr.text.clone(), Some(Arc::new(t)), near),
                None => {
                    let m = app.t("ocr.nothing_to_paint");
                    notice(app, bg.region, m, None, true, false);
                }
            }
        }
    }
}

/// Put recognised text on the clipboard and say so by the region.
fn copy_with_notice(app: &mut GlossaApp, text: &str, region: GRect) {
    let result = match app.clipboard.as_mut() {
        Some(c) => c.set_text(text).map_err(|e| e.to_string()),
        None => {
            app.ctx.copy_text(text.to_owned());
            Ok(())
        }
    };
    match result {
        Ok(()) => {
            let count = text.chars().count().to_string();
            let m = app.locale.fmt("ocr.copied_chars", &count);
            let preview: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
            notice(app, region, m, Some(preview), false, false);
        }
        Err(e) => notice(app, region, e, None, true, false),
    }
}

fn notice(app: &mut GlossaApp, region: GRect, text: String, preview: Option<String>, error: bool, busy: bool) {
    crate::notice::show(app, region, text, preview, error, busy);
}

pub fn tick(app: &mut GlossaApp) {
    let Some(w) = &app.watch else { return };
    if w.stop.load(Ordering::SeqCst) {
        app.watch = None;
        return;
    }
    let pending = w.pending.lock().take();
    if let Some(png) = pending {
        if !w.busy.swap(true, Ordering::SeqCst) {
            let job = app.engine.recognize_async(Slot::Detached, png, app.cfg.ocr.language, translate_target(app));
            if let Some(w) = &app.watch {
                w.job.store(job, Ordering::SeqCst);
            }
        }
    }
}

/// Read a region again without the overlay ("repeat last recognition"),
/// into the result window.
pub fn recognize_region(app: &mut GlossaApp, rect: GRect) {
    match glossa_platform::capture::capture_rect(rect) {
        Ok(img) => {
            let (lang, to) = (app.cfg.ocr.language, translate_target(app));
            crate::ocr_result::open(app, img, rect, lang, to);
        }
        Err(e) => app.notify(e.to_string(), true),
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.ocr.take() else { return };
    let (w, h) = (st.image.width() as f32 / st.scale, st.image.height() as f32 / st.scale);
    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_decorations(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(st.wanted)
        .with_inner_size([w, h])
        .with_active(true);
    let mut keep = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(ID), builder, |ui, _| {
        keep = overlay(app, ui, &mut st, vec2(w, h));
    });
    if !st.lifted {
        // Over the menu bar and the Stage Manager strip (which otherwise
        // slides in when the drag reaches the screen's edge), and not a
        // window Stage Manager switches stages for. Both need the
        // NSWindow, which exists from the first frame on.
        st.lifted = glossa_platform::window::lift_over_everything(TITLE);
    }
    app.float_child(TITLE);
    if keep {
        app.ocr = Some(st);
    }
}

fn overlay(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OcrState, size: egui::Vec2) -> bool {
    let ctx = ui.ctx().clone();
    // Put the window back where it was asked to be while AppKit keeps it
    // below the menu bar (it lets go once the window is lifted), and draw
    // the screenshot shifted by however far off it still is.
    let actual = ui.input(|i| i.viewport().outer_rect);
    let offset = actual.map_or(egui::Vec2::ZERO, |r| st.wanted - r.min);
    st.offset = if offset.length() > 0.5 { offset } else { egui::Vec2::ZERO };
    if st.offset != egui::Vec2::ZERO && st.replaced < 10 {
        st.replaced += 1;
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(st.wanted));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.request_repaint();
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
        return false;
    }
    let texture = st
        .texture
        .get_or_insert_with(|| {
            let size = [st.image.width() as usize, st.image.height() as usize];
            let img = egui::ColorImage::from_rgba_unmultiplied(size, st.image.as_raw());
            ctx.load_texture("ocr-screenshot", img, egui::TextureOptions::LINEAR)
        })
        .clone();
    let look = app.look.clone();
    let mut keep = true;
    egui::CentralPanel::no_frame().show(ui, |ui| {
        let full = ui.max_rect();
        let resp = ui.allocate_rect(full, Sense::click_and_drag());
        let p = ui.painter();
        let picture = Rect::from_min_size(full.min + st.offset, size);
        p.image(texture.id(), picture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let dim = Color32::from_black_alpha(110);
        match st.selection {
            Some(sel) => {
                for r in [
                    Rect::from_min_max(full.min, pos2(full.max.x, sel.min.y)),
                    Rect::from_min_max(pos2(full.min.x, sel.max.y), full.max),
                    Rect::from_min_max(pos2(full.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)),
                    Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(full.max.x, sel.max.y)),
                ] {
                    p.rect_filled(r, 0.0, dim);
                }
                p.rect_stroke(sel, 0.0, egui::Stroke::new(1.5, look.accent()), egui::StrokeKind::Outside);
                let px = format!("{} × {}", (sel.width() * st.scale) as i32, (sel.height() * st.scale) as i32);
                p.text(
                    sel.left_top() - vec2(0.0, 4.0),
                    egui::Align2::LEFT_BOTTOM,
                    px,
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
            None => {
                p.rect_filled(full, 0.0, dim);
                p.text(
                    full.center_top() + vec2(0.0, 40.0),
                    egui::Align2::CENTER_CENTER,
                    app.t("ocr.hint"),
                    egui::FontId::proportional(16.0),
                    Color32::WHITE,
                );
            }
        }

        // A new drag starts a new selection, also while the toolbar asks.
        {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.drag_started() {
                // Where the button went down: egui calls it a drag only once
                // the pointer has moved a little, and the corner must not
                // move with that.
                st.drag_start = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
                st.phase = Phase::Selecting;
            }
            if let (Some(a), Some(b)) = (st.drag_start, resp.interact_pointer_pos()) {
                if resp.dragged() {
                    st.selection = Some(Rect::from_two_pos(a, b));
                }
            }
            if resp.drag_stopped() {
                st.drag_start = None;
                if let Some(sel) = st.selection.filter(|s| s.width() > 4.0 && s.height() > 4.0) {
                    match first_choice(app, st.action) {
                        Some(choice) => keep = run(app, st, sel, choice),
                        None => st.phase = Phase::Choosing,
                    }
                } else {
                    st.selection = None;
                }
            }
        }
        if resp.secondary_clicked() {
            // Right click: cancel, as a quick way out.
            keep = false;
        }

        if let (Some(sel), Phase::Choosing) = (st.selection, &st.phase) {
            if let Some(choice) = toolbar(app, ui, sel, full) {
                keep &= run(app, st, sel, choice);
            }
        }
    });
    keep
}

/// What releasing the mouse does for `action`; `None` asks with the toolbar.
fn first_choice(app: &GlossaApp, action: Action) -> Option<Choice> {
    match action {
        Action::RecognizeToClipboard => Some(Choice::Copy),
        Action::RecognizeAndTranslate => Some(Choice::Translate),
        Action::TranslateRegionInPlace => Some(Choice::InPlace),
        // Watching is started by `run` too.
        Action::WatchRegion => Some(Choice::Show),
        _ => match app.cfg.ocr.after_select {
            OcrAfterSelect::Show => Some(Choice::Show),
            OcrAfterSelect::Copy => Some(Choice::Copy),
            OcrAfterSelect::InPlace => Some(Choice::InPlace),
            OcrAfterSelect::Ask => None,
        },
    }
}

/// What Enter does in the toolbar: the configured action, or translating
/// when the setting is to ask.
fn default_choice(app: &GlossaApp) -> Choice {
    match app.cfg.ocr.after_select {
        OcrAfterSelect::Copy => Choice::Copy,
        OcrAfterSelect::InPlace => Choice::InPlace,
        OcrAfterSelect::Show | OcrAfterSelect::Ask => Choice::Translate,
    }
}

/// The selection in screenshot pixels, clamped to the screenshot.
fn pixel_rect(st: &OcrState, sel: Rect) -> (u32, u32, u32, u32) {
    // The selection is in window points; the screenshot is drawn `offset`
    // from the window's corner.
    let sel = sel.translate(-st.offset);
    let s = st.scale;
    let x = ((sel.min.x * s).max(0.0) as u32).min(st.image.width().saturating_sub(1));
    let y = ((sel.min.y * s).max(0.0) as u32).min(st.image.height().saturating_sub(1));
    let w = ((sel.width() * s) as u32).min(st.image.width().saturating_sub(x)).max(1);
    let h = ((sel.height() * s) as u32).min(st.image.height().saturating_sub(y)).max(1);
    (x, y, w, h)
}

/// Act on the selection. Returns whether the overlay stays open.
fn run(app: &mut GlossaApp, st: &mut OcrState, sel: Rect, choice: Choice) -> bool {
    if choice == Choice::Cancel {
        return false;
    }
    let (x, y, w, h) = pixel_rect(st, sel);
    let crop = image::imageops::crop_imm(&st.image, x, y, w, h).to_image();
    // Image pixels to desktop coordinates (the same on Windows and Linux;
    // half of them on a Retina Mac, whose desktop is in points).
    let k = glossa_platform::window::desktop_scale(st.scale) / st.scale;
    let region =
        GRect::new(st.origin.0 as f32 + x as f32 * k, st.origin.1 as f32 + y as f32 * k, w as f32 * k, h as f32 * k);
    app.last_region = Some(region);
    if st.action == Action::WatchRegion {
        app.watch = Some(Watch::start(region, Duration::from_millis(app.cfg.ocr.watch_interval_ms), app.ctx.clone()));
        let m = app.t("msg.watching");
        app.notify(m, false);
        return false;
    }
    match choice {
        Choice::CopyImage => {
            if let Some(cb) = app.clipboard.as_mut() {
                if let Err(e) = cb.set_image(&crop) {
                    app.notify(e.to_string(), true);
                    return true;
                }
            }
            let m = app.t("msg.copied");
            notice(app, region, m, None, false, false);
            false
        }
        Choice::SaveImage => {
            save_image(app, &crop);
            false
        }
        Choice::Copy => {
            start_background(app, crop, region, st.lang, false);
            false
        }
        Choice::InPlace => {
            start_background(app, crop, region, st.lang, true);
            false
        }
        Choice::Show | Choice::Translate | Choice::Cancel => {
            let translate_to =
                if choice == Choice::Translate { Some(target_language(app)) } else { translate_target(app) };
            crate::ocr_result::open(app, crop, region, st.lang, translate_to);
            false
        }
    }
}

/// Recognise `crop`, captured at `region`, behind the scenes: to the
/// clipboard, or translated block by block and painted over the region.
fn start_background(app: &mut GlossaApp, crop: image::RgbaImage, region: GRect, lang: LangId, in_place: bool) {
    let png = glossa_platform::capture::encode_png(&crop);
    let opts = RecognizeOptions {
        lang,
        translate_to: in_place.then(|| target_language(app)),
        in_place,
        layout: app.cfg.ocr.copy_joined_lines,
        ..Default::default()
    };
    let job = app.engine.recognize_async_with(Slot::Ocr, png, opts);
    app.ocr_background = Some(Background { job, in_place, region, crop: in_place.then_some(crop) });
    let m = app.t("msg.recognizing");
    notice(app, region, m, None, false, true);
}

/// Translate a picture already captured (the result window's) in place.
pub(crate) fn start_in_place(app: &mut GlossaApp, crop: image::RgbaImage, region: GRect, lang: LangId) {
    start_background(app, crop, region, lang, true);
}

pub(crate) fn save_image(app: &mut GlossaApp, img: &image::RgbaImage) {
    if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("glossa-ocr.png").save_file() {
        if let Err(e) = img.save(&path) {
            app.notify(e.to_string(), true);
        }
    }
}

/// Where a panel of `width` × `height` goes: under the selection, else
/// above it, kept on the overlay.
fn panel_pos(sel: Rect, full: Rect, width: f32, height: f32) -> egui::Pos2 {
    let x = sel.min.x.min(full.max.x - width - 8.0).max(full.min.x + 8.0);
    let below = sel.max.y + 8.0;
    if below + height < full.max.y {
        pos2(x, below)
    } else if sel.min.y - height - 8.0 > full.min.y {
        pos2(x, sel.min.y - height - 8.0)
    } else {
        pos2(x, (sel.max.y - height - 8.0).max(full.min.y + 8.0))
    }
}

/// The Snipping Tool style bar under a fresh selection: what to do with
/// it, each with its key.
fn toolbar(app: &mut GlossaApp, ui: &mut egui::Ui, sel: Rect, full: Rect) -> Option<Choice> {
    let look = app.look.clone();
    let default = default_choice(app);
    let mut chosen = ui.input(|i| {
        if i.modifiers.any() {
            return None;
        }
        if i.key_pressed(egui::Key::Enter) {
            Some(default)
        } else if i.key_pressed(egui::Key::C) {
            Some(Choice::Copy)
        } else if i.key_pressed(egui::Key::T) {
            Some(Choice::Translate)
        } else if i.key_pressed(egui::Key::I) {
            Some(Choice::InPlace)
        } else {
            None
        }
    });
    let default_label = match default {
        Choice::Copy => app.t("ocr.copy_text"),
        Choice::InPlace => app.t("ocr.in_place"),
        _ => app.t("ocr.translate"),
    };
    let id = egui::Id::unique("ocr-toolbar");
    // Last frame's size, to keep the bar on screen.
    let size = ui.ctx().memory(|m| m.area_rect(id).map(|r| r.size())).unwrap_or(vec2(560.0, 74.0));
    let pos = panel_pos(sel, full, size.x, size.y);
    let radius = look.theme.metrics.overlay_radius.max(look.theme.metrics.button_radius).max(4.0);
    egui::Area::new(id).fixed_pos(pos).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style())
            .fill(look.popup_back)
            .stroke(egui::Stroke::new(look.popup_frame_width.max(1.0), look.popup_frame))
            .corner_radius(radius)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let mut button = |ui: &mut egui::Ui, icon: Icon, label: String, tip: String, choice: Choice| {
                        let r = icons::command_button(ui, &look, icon, &label, &tip, true);
                        if choice == default {
                            // What Enter does is underlined in the accent.
                            let y = r.rect.bottom() - 1.5;
                            ui.painter().line_segment(
                                [pos2(r.rect.left() + 8.0, y), pos2(r.rect.right() - 8.0, y)],
                                egui::Stroke::new(2.0, look.accent()),
                            );
                        }
                        if r.clicked() {
                            chosen = Some(choice);
                        }
                    };
                    let named = |key: &str, keycap: &str| (app.t(key), format!("{} ({keycap})", app.t(key)));
                    let (l, t) = named("ocr.copy_text", "C");
                    button(ui, Icon::Copy, l, t, Choice::Copy);
                    let (l, t) = named("ocr.translate", "T");
                    button(ui, Icon::Translate, l, t, Choice::Translate);
                    let (l, t) = named("ocr.in_place", "I");
                    button(ui, Icon::Overlay, l, t, Choice::InPlace);
                    ui.separator();
                    button(ui, Icon::Image, String::new(), app.t("ocr.copy_image"), Choice::CopyImage);
                    button(ui, Icon::Save, String::new(), app.t("ocr.save_image"), Choice::SaveImage);
                    ui.separator();
                    let cancel = format!("{} (Esc)", app.t("ocr.cancel"));
                    button(ui, Icon::Close, String::new(), cancel, Choice::Cancel);
                });
                ui.label(
                    RichText::new(app.locale.fmt("ocr.toolbar_keys", &default_label)).small().color(look.weak_text()),
                );
            });
    });
    chosen
}

/// A region being watched for changes.
pub struct Watch {
    pub stop: Arc<AtomicBool>,
    pub busy: Arc<AtomicBool>,
    pub job: Arc<std::sync::atomic::AtomicU64>,
    /// The newest changed frame, waiting to be recognised.
    pub pending: Arc<parking_lot::Mutex<Option<Vec<u8>>>>,
    pub anchor: (i32, i32),
}

impl Watch {
    fn idle(job: u64, anchor: (i32, i32)) -> Watch {
        Watch {
            stop: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(true)),
            job: Arc::new(std::sync::atomic::AtomicU64::new(job)),
            pending: Arc::new(parking_lot::Mutex::new(None)),
            anchor,
        }
    }

    fn start(region: GRect, interval: Duration, ctx: egui::Context) -> Watch {
        let w = Watch::idle(0, (region.x as i32, (region.y + region.height) as i32));
        w.busy.store(false, Ordering::SeqCst);
        let (stop, pending) = (w.stop.clone(), w.pending.clone());
        std::thread::Builder::new()
            .name("glossa-watch".into())
            .spawn(move || {
                let mut last: Option<image::RgbaImage> = None;
                while !stop.load(Ordering::SeqCst) {
                    if let Ok(img) = glossa_platform::capture::capture_rect(region) {
                        let changed =
                            last.as_ref().is_none_or(|l| glossa_platform::capture::images_differ(l, &img, 0.02));
                        if changed {
                            *pending.lock() = Some(glossa_platform::capture::encode_png(&img));
                            last = Some(img);
                            ctx.request_repaint();
                        }
                    }
                    std::thread::sleep(interval);
                }
            })
            .ok();
        w
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_go_under_the_selection_or_above_it() {
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
        let sel = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 50.0));
        assert_eq!(panel_pos(sel, full, 300.0, 60.0), pos2(100.0, 158.0));
        let low = Rect::from_min_size(pos2(100.0, 700.0), vec2(200.0, 80.0));
        assert_eq!(panel_pos(low, full, 300.0, 60.0), pos2(100.0, 632.0));
    }
}
