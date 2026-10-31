//! Screen-region OCR: the overlay, its result panel, and watching a region.
//!
//! The overlay is a borderless window over the whole desktop showing a
//! screenshot taken just before it opened, dimmed except for the rectangle
//! the user drags. Releasing the mouse recognises that rectangle; the result
//! appears beside it with copy / translate / save actions. The silent variant
//! puts the text straight on the clipboard; the watch variant keeps
//! re-reading the same rectangle and pops up a translation whenever its pixels
//! change — subtitles for a game or a video.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use egui::{pos2, vec2, Color32, Rect, RichText, Sense};
use glossa_core::ocr::Rect as GRect;
use glossa_core::{Action, Error, LangId, LangPair};
use glossa_engine::{OcrOutcome, Slot};

use crate::app::GlossaApp;

pub enum Phase {
    Selecting,
    Recognizing,
    Done(Arc<OcrOutcome>),
    Failed(String),
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
    pub job: u64,
    pub lang: LangId,
    edited_text: Option<String>,
}

const ID: &str = "glossa-ocr";

pub fn begin(app: &mut GlossaApp, action: Action, image: image::RgbaImage, origin: (i32, i32)) {
    let scale = glossa_platform::window::monitor_at(origin.0, origin.1)
        .or_else(|| glossa_platform::window::monitors().into_iter().find(|m| m.is_primary))
        .map(|m| m.scale as f32)
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0);
    app.ocr = Some(OcrState {
        action,
        image,
        origin,
        scale,
        texture: None,
        drag_start: None,
        selection: None,
        phase: Phase::Selecting,
        job: 0,
        lang: app.cfg.ocr.language,
        edited_text: None,
    });
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

fn translate_target(app: &GlossaApp) -> Option<LangId> {
    if !app.cfg.ocr.translate_after {
        return None;
    }
    let to = app.cfg.general.pair.to;
    Some(if to.is_language() { to } else { app.cfg.general.first_language })
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
    let Some(st) = app.ocr.as_mut() else { return };
    if job != st.job {
        return;
    }
    match result {
        Ok(o) => {
            app.fonts.ensure(&app.ctx, &o.ocr.text);
            if let Some(t) = o.translated_text() {
                let t = t.to_owned();
                app.fonts.ensure(&app.ctx, &t);
            }
            match st.action {
                Action::RecognizeToClipboard => {
                    let text = o.ocr.text.clone();
                    app.ocr = None;
                    app.copy_text(&text);
                }
                _ => st.phase = Phase::Done(o),
            }
        }
        Err(_) => st.phase = Phase::Failed(message.unwrap_or_default()),
    }
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

/// Re-read a region without the overlay ("repeat last recognition").
pub fn recognize_region(app: &mut GlossaApp, rect: GRect) {
    match glossa_platform::capture::capture_rect(rect) {
        Ok(img) => {
            let png = glossa_platform::capture::encode_png(&img);
            let job = app.engine.recognize_async(Slot::Detached, png, app.cfg.ocr.language, translate_target(app));
            let watch = Watch::idle(job, (rect.x as i32, (rect.y + rect.height) as i32));
            app.watch = Some(watch);
        }
        Err(e) => app.notify(e.to_string(), true),
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.ocr.take() else { return };
    let (w, h) = (st.image.width() as f32 / st.scale, st.image.height() as f32 / st.scale);
    let builder = egui::ViewportBuilder::default()
        .with_title("Glossa OCR")
        .with_decorations(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(pos2(st.origin.0 as f32 / st.scale, st.origin.1 as f32 / st.scale))
        .with_inner_size([w, h])
        .with_active(true);
    let mut keep = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(ID), builder, |ui, _| {
        keep = overlay(app, ui, &mut st);
    });
    if keep {
        app.ocr = Some(st);
    }
}

fn overlay(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OcrState) -> bool {
    let ctx = ui.ctx().clone();
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
        p.image(texture.id(), full, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
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

        if matches!(st.phase, Phase::Selecting) {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            if resp.drag_started() {
                st.drag_start = resp.interact_pointer_pos();
            }
            if let (Some(a), Some(b)) = (st.drag_start, resp.interact_pointer_pos()) {
                if resp.dragged() {
                    st.selection = Some(Rect::from_two_pos(a, b));
                }
            }
            if resp.drag_stopped() {
                st.drag_start = None;
                if let Some(sel) = st.selection.filter(|s| s.width() > 4.0 && s.height() > 4.0) {
                    keep = start_recognition(app, st, sel);
                } else {
                    st.selection = None;
                }
            }
        }
        if resp.secondary_clicked() {
            // Right click: cancel, as a quick way out.
            if matches!(st.phase, Phase::Selecting) {
                keep = false;
            }
        }

        if let Some(sel) = st.selection {
            if !matches!(st.phase, Phase::Selecting) {
                keep &= result_panel(app, ui, st, sel, full);
            }
        }
    });
    keep
}

/// Crop the selection out of the screenshot and send it off. Returns whether
/// the overlay stays open.
fn start_recognition(app: &mut GlossaApp, st: &mut OcrState, sel: Rect) -> bool {
    let s = st.scale;
    let x = (sel.min.x * s).max(0.0) as u32;
    let y = (sel.min.y * s).max(0.0) as u32;
    let w = ((sel.width() * s) as u32).min(st.image.width().saturating_sub(x)).max(1);
    let h = ((sel.height() * s) as u32).min(st.image.height().saturating_sub(y)).max(1);
    let crop = image::imageops::crop_imm(&st.image, x, y, w, h).to_image();
    let png = glossa_platform::capture::encode_png(&crop);
    let region = GRect::new((st.origin.0 as u32 + x) as f32, (st.origin.1 as u32 + y) as f32, w as f32, h as f32);
    app.last_region = Some(region);
    if st.action == Action::WatchRegion {
        app.watch = Some(Watch::start(region, Duration::from_millis(app.cfg.ocr.watch_interval_ms), app.ctx.clone()));
        app.notify("Watching the region — press the hotkey again to stop.", false);
        return false;
    }
    let translate_to = match st.action {
        Action::RecognizeToClipboard => None,
        Action::RecognizeAndTranslate => Some(translate_target(app).unwrap_or(app.cfg.general.first_language)),
        _ => translate_target(app),
    };
    st.job = app.engine.recognize_async(Slot::Ocr, png, st.lang, translate_to);
    st.phase = Phase::Recognizing;
    true
}

fn result_panel(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OcrState, sel: Rect, full: Rect) -> bool {
    let look = app.look.clone();
    let mut keep = true;
    let width = sel.width().clamp(320.0, 560.0);
    let below = sel.max.y + 8.0;
    let pos = if below + 160.0 < full.max.y {
        pos2(sel.min.x.min(full.max.x - width - 8.0), below)
    } else {
        pos2(sel.min.x.min(full.max.x - width - 8.0), (sel.min.y - 230.0).max(8.0))
    };
    egui::Area::new(egui::Id::unique("ocr-result")).fixed_pos(pos).order(egui::Order::Foreground).show(
        ui.ctx(),
        |ui| {
            egui::Frame::popup(ui.style())
                .fill(look.popup_back)
                .stroke(egui::Stroke::new(look.popup_frame_width.max(1.0), look.popup_frame))
                .show(ui, |ui| {
                    ui.set_width(width);
                    match &st.phase {
                        Phase::Recognizing => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new());
                                ui.label(app.t("msg.recognizing"));
                            });
                        }
                        Phase::Failed(e) => {
                            ui.label(RichText::new(e).color(look.error()));
                            ui.horizontal(|ui| {
                                if ui.button(app.t("ocr.clear_selection")).clicked() {
                                    st.selection = None;
                                    st.phase = Phase::Selecting;
                                }
                                if ui.button(app.t("ocr.close")).clicked() {
                                    keep = false;
                                }
                            });
                        }
                        Phase::Done(o) => {
                            let o = o.clone();
                            let text = st.edited_text.get_or_insert_with(|| o.ocr.text.clone());
                            egui::ScrollArea::vertical().id_salt("ocr-text").max_height(140.0).show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(text).desired_width(f32::INFINITY).font(look.text_font()),
                                );
                            });
                            if let Some(t) = o.translated_text() {
                                ui.separator();
                                egui::ScrollArea::vertical().id_salt("ocr-tr").max_height(140.0).show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(t).font(look.text_font()).color(look.popup_text),
                                        )
                                        .selectable(true)
                                        .wrap(),
                                    );
                                });
                            }
                            let text = text.clone();
                            ui.horizontal_wrapped(|ui| {
                                if ui.button(app.t("ocr.copy_text")).clicked() {
                                    app.copy_text(&text);
                                }
                                if let Some(t) = o.translated_text() {
                                    if ui.button(app.t("history.copy_translation")).clicked() {
                                        let t = t.to_owned();
                                        app.copy_text(&t);
                                    }
                                }
                                if ui.button(app.t("ocr.translate")).clicked() {
                                    app.main.source = text.clone();
                                    app.main.translated_source.clear();
                                    app.translate_main();
                                    app.show_main();
                                    keep = false;
                                }
                                if ui.button(app.t("ocr.copy_image")).clicked() {
                                    if let Some(img) = crop_of(st, sel) {
                                        if let Some(cb) = app.clipboard.as_mut() {
                                            let _ = cb.set_image(&img);
                                        }
                                    }
                                }
                                if ui.button(app.t("ocr.save_image")).clicked() {
                                    if let (Some(img), Some(path)) = (
                                        crop_of(st, sel),
                                        rfd::FileDialog::new()
                                            .add_filter("PNG", &["png"])
                                            .set_file_name("glossa-ocr.png")
                                            .save_file(),
                                    ) {
                                        if let Err(e) = img.save(&path) {
                                            app.notify(e.to_string(), true);
                                        }
                                    }
                                }
                                if ui.button(app.t("ocr.clear_selection")).clicked() {
                                    st.selection = None;
                                    st.edited_text = None;
                                    st.phase = Phase::Selecting;
                                }
                                if ui.button(app.t("ocr.close")).clicked() {
                                    keep = false;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(app.t("ocr.language")).small());
                                let disabled = app.cfg.languages.disabled.clone();
                                let auto = app.t("main.auto_detect");
                                if let Some(l) =
                                    crate::widgets::lang_combo(ui, "ocr-lang", st.lang, true, &auto, &disabled, 160.0)
                                {
                                    st.lang = l;
                                    let _ = app.engine.update_config(move |c| c.ocr.language = l);
                                }
                                ui.label(
                                    RichText::new(format!("{} · {} ms", o.ocr.provider_id, o.ocr.elapsed_ms))
                                        .small()
                                        .color(look.weak_text()),
                                );
                            });
                        }
                        Phase::Selecting => {}
                    }
                });
        },
    );
    let _ = LangPair::new(LangId::AUTO, LangId::AUTO);
    keep
}

fn crop_of(st: &OcrState, sel: Rect) -> Option<image::RgbaImage> {
    let s = st.scale;
    let x = (sel.min.x * s).max(0.0) as u32;
    let y = (sel.min.y * s).max(0.0) as u32;
    let w = ((sel.width() * s) as u32).min(st.image.width().saturating_sub(x));
    let h = ((sel.height() * s) as u32).min(st.image.height().saturating_sub(y));
    (w > 0 && h > 0).then(|| image::imageops::crop_imm(&st.image, x, y, w, h).to_image())
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
                            last.as_ref().map_or(true, |l| glossa_platform::capture::images_differ(l, &img, 0.02));
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
