//! The window a recognised screen region opens in: the picture, the text
//! read from it, its translation, and what can be done with them.
//!
//! It is one of the app's tool windows (the shared frame from
//! [`crate::chrome`], dropdowns from [`crate::dropdown`], icons from
//! [`crate::icons`]), laid out top to bottom:
//!
//! - the captured picture, as a thumbnail that grows when clicked, with the
//!   boxes the engine found drawn over it on request, and what read it;
//! - the recognised text, editable, in the text areas' font and zoom;
//! - the direction bar: the text's language, swap, the target language,
//!   the translator ("Google Lens" when Lens translated the picture itself)
//!   and Translate;
//! - the translation;
//! - the action bar: copy the text or the translation, translate in place,
//!   send to the main window, read aloud, copy or save the picture, select
//!   again. Every action has a shortcut, shown in its tooltip.
//!
//! The window opens at once with the picture while the engine reads it, so
//! the overlay can close the moment a region is chosen. Its size and place
//! are remembered (`[windows.ocr-result]`); the first time it opens beside
//! the region. Escape closes it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{pos2, vec2, Key, KeyboardShortcut, Modifiers, Pos2, Rect, RichText, Sense, Vec2};
use glossa_core::config::{OcrTranslateWith, WindowGeometry};
use glossa_core::ocr::Rect as GRect;
use glossa_core::{Action, Capabilities, Error, LangId, LangPair};
use glossa_engine::{OcrOutcome, RecognizeOptions, Slot, TranslateOptions, TranslationOutcome};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};
use crate::style::c;

/// The viewport's name.
pub(crate) const VIEWPORT: &str = "glossa-ocr-result";
/// The key in `config.windows` for its size and place.
const GEOMETRY: &str = "ocr-result";
/// Google Lens's provider id (`glossa_ocr::lens::LENS_ID`; the UI does not
/// depend on the OCR crate).
const LENS: &str = "lens";
const DEFAULT_SIZE: Vec2 = vec2(620.0, 580.0);
const MIN_SIZE: Vec2 = vec2(440.0, 400.0);
/// The thumbnail's height, in points, before it is made larger.
const THUMB_HEIGHT: f32 = 84.0;
/// How long "Copied" stays in the status line.
const FLASH: Duration = Duration::from_millis(1800);

pub struct ResultWindow {
    /// The captured region, in its own pixels.
    image: image::RgbaImage,
    /// Where it was on the virtual desktop, in pixels.
    region: GRect,
    texture: Option<egui::TextureHandle>,
    /// The language of the text: what the engine is told to read, and the
    /// source language of translations.
    from: LangId,
    to: LangId,
    /// The translator picked here: a service id, [`LENS`] for Lens's own
    /// translation of the picture, `None` for the active service.
    translator: Option<String>,
    /// A translation was asked for when the window opened.
    translate: bool,
    ocr_job: u64,
    /// The running recognition only fetches Lens's translation of the same
    /// picture; the text stays as it is.
    relens: bool,
    recognising: bool,
    error: Option<String>,
    outcome: Option<Arc<OcrOutcome>>,
    /// The text, as read and then as edited.
    text: String,
    translation: Option<Arc<TranslationOutcome>>,
    /// The text `translation` is of, to tell when the text was edited since.
    translated_text: String,
    partial: Option<String>,
    tr_job: u64,
    translating: bool,
    tr_error: Option<String>,
    large: bool,
    boxes: bool,
    /// Where the engine found text, in the picture's pixels: worked out once
    /// per result.
    found: Vec<GRect>,
    /// What the viewport was created with. Kept as they were: the window's
    /// actual size and place are the user's from then on.
    pos: Option<Pos2>,
    size: Vec2,
    raise: bool,
    flash: Option<(String, Instant)>,
}

/// What the buttons and shortcuts do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    CopyText,
    CopyTranslation,
    InPlace,
    ToMain,
    SelectAgain,
    SaveImage,
    CopyImage,
    ReadAloud,
    Translate,
    Swap,
}

/// The shortcuts, most specific first: egui lets a pattern without Shift
/// match a press with it.
const SHORTCUTS: [(Command, KeyboardShortcut); 10] = [
    (Command::CopyText, KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C)),
    (Command::CopyTranslation, KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::T)),
    (Command::Translate, KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter)),
    (Command::Swap, KeyboardShortcut::new(Modifiers::COMMAND, Key::I)),
    (Command::InPlace, KeyboardShortcut::new(Modifiers::COMMAND, Key::P)),
    (Command::ToMain, KeyboardShortcut::new(Modifiers::COMMAND, Key::M)),
    (Command::SelectAgain, KeyboardShortcut::new(Modifiers::COMMAND, Key::R)),
    (Command::SaveImage, KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
    (Command::ReadAloud, KeyboardShortcut::new(Modifiers::COMMAND, Key::L)),
    (Command::CopyImage, KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::I)),
];

/// A shortcut as words ("Ctrl+Shift+C"; "Cmd" on macOS), not egui's symbols,
/// which most fonts lack.
fn shortcut_text(cmd: Command) -> String {
    let Some((_, sc)) = SHORTCUTS.iter().find(|(c, _)| *c == cmd) else { return String::new() };
    let mut out = String::new();
    if sc.modifiers.command {
        out.push_str(glossa_platform::shell::SHELL.command_key);
        out.push('+');
    }
    if sc.modifiers.shift {
        out.push_str("Shift+");
    }
    out.push_str(sc.logical_key.name());
    out
}

/// A tooltip: what it does and its shortcut.
fn tip(app: &GlossaApp, key: &str, cmd: Command) -> String {
    format!("{} ({})", app.t(key), shortcut_text(cmd))
}

/// Open the window for `image`, captured at `region`, and start reading it.
/// An open window takes the new picture where it is.
pub fn open(app: &mut GlossaApp, image: image::RgbaImage, region: GRect, lang: LangId, translate_to: Option<LangId>) {
    let (pos, size, raise) = match app.ocr_result.as_ref() {
        Some(old) => (old.pos, old.size, true),
        None => {
            let (pos, size) = placement(app, region);
            (Some(pos), size, false)
        }
    };
    let to = translate_to.unwrap_or_else(|| crate::ocr::target_language(app));
    let mut st = ResultWindow {
        image,
        region,
        texture: None,
        from: lang,
        to,
        translator: None,
        translate: translate_to.is_some(),
        ocr_job: 0,
        relens: false,
        recognising: false,
        error: None,
        outcome: None,
        text: String::new(),
        translation: None,
        translated_text: String::new(),
        partial: None,
        tr_job: 0,
        translating: false,
        tr_error: None,
        large: false,
        boxes: false,
        found: Vec::new(),
        pos,
        size,
        raise,
        flash: None,
    };
    recognise(app, &mut st, translate_to, false);
    if raise {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(VIEWPORT));
    }
    app.ocr_result = Some(st);
    app.ctx.request_repaint();
}

/// Where the window opens and how big: as it was left, else beside the
/// region on its monitor.
fn placement(app: &GlossaApp, region: GRect) -> (Pos2, Vec2) {
    if let Some(g) = app.cfg.windows.get(GEOMETRY).filter(|g| g.width >= MIN_SIZE.x && g.height >= MIN_SIZE.y) {
        return (pos2(g.x, g.y), vec2(g.width, g.height));
    }
    let (cx, cy) = region.centre();
    let monitor = glossa_platform::window::monitor_at(cx as i32, cy as i32)
        .or_else(|| glossa_platform::window::monitors().into_iter().find(|m| m.is_primary));
    let scale = monitor.as_ref().map(|m| m.desktop_scale()).unwrap_or(1.0);
    let (x, y) = place_beside(region, DEFAULT_SIZE * scale, monitor.map(|m| m.work_area));
    (pos2(x / scale, y / scale), DEFAULT_SIZE)
}

/// Where a window of `size` goes next to `region` (both in desktop pixels):
/// to its right, else its left, else below or above it, kept inside `work`.
fn place_beside(region: GRect, size: Vec2, work: Option<GRect>) -> (f32, f32) {
    let work = work.unwrap_or(GRect::new(-1e6, -1e6, 2e6, 2e6));
    let gap = 12.0;
    let (right_edge, bottom_edge) = (work.x + work.width, work.y + work.height);
    let clamp_x = |x: f32| x.min(right_edge - size.x).max(work.x);
    let clamp_y = |y: f32| y.min(bottom_edge - size.y).max(work.y);
    let right = region.x + region.width + gap;
    if right + size.x <= right_edge {
        return (right, clamp_y(region.y));
    }
    let left = region.x - size.x - gap;
    if left >= work.x {
        return (left, clamp_y(region.y));
    }
    let below = region.y + region.height + gap;
    if below + size.y <= bottom_edge {
        return (clamp_x(region.x), below);
    }
    let above = region.y - size.y - gap;
    (clamp_x(region.x), clamp_y(above))
}

/// Read the picture (again). `relens` asks Lens for its translation into
/// `translate_to` and keeps the text.
fn recognise(app: &mut GlossaApp, st: &mut ResultWindow, translate_to: Option<LangId>, relens: bool) {
    let png = glossa_platform::capture::encode_png(&st.image);
    let opts = RecognizeOptions {
        lang: st.from,
        translate_to,
        provider: relens.then(|| LENS.to_owned()),
        // The boxes need the layout; engines read the same text either way.
        layout: true,
        translate_with: relens.then_some(OcrTranslateWith::OcrEngine),
        ..Default::default()
    };
    st.ocr_job = app.engine.recognize_async_with(Slot::Ocr, png, opts);
    st.relens = relens;
    if relens {
        st.translating = true;
        st.tr_error = None;
    } else {
        st.recognising = true;
        st.error = None;
    }
}

/// Whether `job` is this window's recognition.
pub fn owns_job(app: &GlossaApp, job: u64) -> bool {
    app.ocr_result.as_ref().is_some_and(|st| st.ocr_job == job && (st.recognising || st.translating))
}

/// A recognition of this window's finished.
pub fn on_recognized(app: &mut GlossaApp, result: Result<Arc<OcrOutcome>, Arc<Error>>, message: Option<String>) {
    let no_lens = app.t("ocr.result.no_lens_translation");
    let Some(st) = app.ocr_result.as_mut() else { return };
    if st.relens {
        st.translating = false;
        match result {
            Ok(o) => match o.translation.clone() {
                Some(t) => {
                    app.fonts.ensure(&app.ctx, &t.translation.text);
                    st.translated_text = st.text.clone();
                    st.to = t.pair.to;
                    st.translation = Some(Arc::new(t));
                    st.found = found_boxes(&o.ocr);
                    st.outcome = Some(o);
                }
                None => st.tr_error = Some(no_lens),
            },
            Err(_) => st.tr_error = message,
        }
        return;
    }
    st.recognising = false;
    match result {
        Ok(o) => {
            app.fonts.ensure(&app.ctx, &o.ocr.text);
            st.text = o.ocr.text.clone();
            st.translation = o.translation.clone().map(Arc::new);
            if let Some(t) = &st.translation {
                app.fonts.ensure(&app.ctx, &t.translation.text);
                st.translated_text = st.text.clone();
                st.to = t.pair.to;
                st.translator = Some(t.translation.provider_id.clone());
            }
            st.found = found_boxes(&o.ocr);
            st.outcome = Some(o);
        }
        Err(_) => st.error = Some(message.unwrap_or_default()),
    }
}

/// A translation made in this window finished.
pub fn on_translated(app: &mut GlossaApp, job: u64, result: Result<Arc<TranslationOutcome>, Arc<Error>>) {
    let message = result.as_ref().err().map(|e| app.describe(e));
    let Some(st) = app.ocr_result.as_mut() else { return };
    if job != st.tr_job {
        return;
    }
    st.translating = false;
    st.partial = None;
    match result {
        Ok(t) => {
            app.fonts.ensure(&app.ctx, &t.translation.text);
            st.translated_text = st.text.clone();
            st.translator = Some(t.translation.provider_id.clone());
            st.translation = Some(t);
            st.tr_error = None;
        }
        Err(_) => st.tr_error = message,
    }
}

/// A streaming translator's text so far.
pub fn on_progress(app: &mut GlossaApp, job: u64, text: String) {
    let Some(st) = app.ocr_result.as_mut() else { return };
    if job == st.tr_job {
        app.fonts.ensure(&app.ctx, &text);
        st.partial = Some(text);
    }
}

/// The text's language as best known: the one set, else the detected one.
fn source_language(st: &ResultWindow) -> LangId {
    if st.from.is_language() {
        return st.from;
    }
    st.translation
        .as_ref()
        .and_then(|t| t.translation.detected_from.or(Some(t.pair.from)))
        .filter(|l| l.is_language())
        .or_else(|| st.outcome.as_ref().and_then(|o| o.ocr.detected_lang))
        .filter(|l| l.is_language())
        .unwrap_or(LangId::AUTO)
}

/// Whether the text is still what the engine read.
fn unedited(st: &ResultWindow) -> bool {
    st.outcome.as_ref().is_some_and(|o| o.ocr.text.trim() == st.text.trim())
}

fn translate(app: &mut GlossaApp, st: &mut ResultWindow) {
    let text = st.text.trim().to_owned();
    if text.is_empty() || st.recognising {
        return;
    }
    // Lens translates pictures, not text: only the text as it read it.
    if st.translator.as_deref() == Some(LENS) && unedited(st) {
        recognise(app, st, Some(st.to), true);
        return;
    }
    let provider = st.translator.clone().filter(|t| t != LENS);
    let pair = LangPair::new(source_language(st), st.to);
    let opts = TranslateOptions { provider, ..Default::default() };
    st.tr_job = app.engine.translate_async(Slot::OcrResult, text, pair, opts);
    st.translating = true;
    st.partial = None;
    st.tr_error = None;
}

/// The translation becomes the text, the direction turns round.
fn swap(app: &mut GlossaApp, st: &mut ResultWindow) {
    let Some(t) = st.translation.clone() else { return };
    let back_to = source_language(st);
    st.from = st.to;
    st.to = if back_to.is_language() { back_to } else { app.cfg.general.first_language };
    st.text = t.translation.text.clone();
    st.translation = None;
    st.translated_text.clear();
    translate(app, st);
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.ocr_result.take() else { return };
    let title = app.t("ocr.result.title");
    let mut builder = crate::chrome::builder(app, &title).with_inner_size(st.size).with_min_inner_size(MIN_SIZE);
    if let Some(p) = st.pos {
        builder = builder.with_position(p);
    }
    let custom = crate::chrome::custom(&app.cfg);
    let mut open = true;
    let mut geometry = None;
    ctx.show_viewport_immediate(crate::chrome::viewport(VIEWPORT), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        if std::mem::take(&mut st.raise) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        app.text_zoom_keys(ui);
        open &= body(app, ui, &mut st);
        geometry = ui.input(|i| {
            let v = i.viewport();
            Some((v.outer_rect?, v.inner_rect?, v.maximized.unwrap_or(false)))
        });
    });
    app.child_backdrop(&title, false);
    if let Some((outer, inner, maximized)) = geometry.filter(|(_, inner, _)| inner.width() >= MIN_SIZE.x * 0.5) {
        let g =
            WindowGeometry { x: outer.min.x, y: outer.min.y, width: inner.width(), height: inner.height(), maximized };
        if !maximized && app.cfg.windows.get(GEOMETRY) != Some(&g) {
            app.cfg.windows.insert(GEOMETRY.into(), g);
        }
    }
    if open {
        app.ocr_result = Some(st);
    }
}

/// The window's content. Returns whether it stays open.
fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow) -> bool {
    let look = app.look.clone();
    let ctx = ui.ctx().clone();
    // Escape closes, unless it is closing a dropdown of this window.
    if ui.input(|i| i.key_pressed(Key::Escape)) && !crate::dropdown::recently_open(&ctx) {
        return false;
    }
    let mut command = ui.input_mut(|i| SHORTCUTS.iter().find(|(_, sc)| i.consume_shortcut(sc)).map(|(c, _)| *c));
    if let Some((_, at)) = &st.flash {
        match FLASH.checked_sub(at.elapsed()) {
            Some(left) => ctx.request_repaint_after(left),
            None => st.flash = None,
        }
    }

    let margin = if crate::chrome::custom(&app.cfg) { 12 } else { 10 };
    egui::Panel::bottom("ocr-result-actions")
        .frame(egui::Frame::NONE.fill(look.window_back()).inner_margin(egui::Margin::symmetric(margin - 4, 6)))
        .show_separator_line(true)
        .show(ui, |ui| {
            if let Some(c) = action_bar(app, ui, st) {
                command = Some(c);
            }
        });
    let frame = egui::Frame::new().fill(look.window_back()).inner_margin(egui::Margin::symmetric(margin, 10));
    egui::CentralPanel::no_frame().frame(frame).show(ui, |ui| {
        picture(app, ui, st);
        ui.add_space(8.0);
        let bar_h = ui.spacing().interact_size.y.max(26.0) + 12.0;
        let status_h = ui.text_style_height(&egui::TextStyle::Small) + 6.0;
        let avail = (ui.available_height() - bar_h - status_h).max(120.0);
        let text_h = (avail * 0.46).max(60.0);
        if let Some(c) = text_pane(app, ui, st, text_h) {
            command = Some(c);
        }
        if let Some(c) = direction_bar(app, ui, st) {
            command = Some(c);
        }
        // What is left, now that the bars took what they needed.
        let tr_h = (ui.available_height() - status_h - ui.spacing().item_spacing.y).max(60.0);
        translation_pane(app, ui, st, tr_h);
        status_line(app, ui, st);
    });
    match command {
        Some(c) => run(app, st, c),
        None => true,
    }
}

/// The picture and what read it.
fn picture(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow) {
    let look = app.look.clone();
    let ctx = ui.ctx().clone();
    let texture = st
        .texture
        .get_or_insert_with(|| {
            let px = [st.image.width() as usize, st.image.height() as usize];
            let img = egui::ColorImage::from_rgba_unmultiplied(px, st.image.as_raw());
            ctx.load_texture("ocr-result-picture", img, egui::TextureOptions::LINEAR)
        })
        .clone();
    let natural = vec2(st.image.width() as f32, st.image.height() as f32) / ctx.pixels_per_point();
    let avail_w = ui.available_width();
    let bounds = if st.large {
        vec2(avail_w, (ui.available_height() * 0.42).max(140.0))
    } else {
        vec2((avail_w * 0.5).max(120.0), THUMB_HEIGHT)
    };
    let k = (bounds.x / natural.x.max(1.0)).min(bounds.y / natural.y.max(1.0)).min(if st.large { 2.0 } else { 1.0 });
    let shown = (natural * k).max(vec2(8.0, 8.0));

    let (smaller, larger) = (app.t("ocr.result.smaller"), app.t("ocr.result.larger"));
    let draw_picture = |ui: &mut egui::Ui, st: &mut ResultWindow| {
        let (rect, resp) = ui.allocate_exact_size(shown, Sense::click());
        let p = ui.painter();
        p.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
        p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, look.border()), egui::StrokeKind::Outside);
        if st.boxes {
            let scale = rect.width() / st.image.width().max(1) as f32;
            for r in &st.found {
                let b = Rect::from_min_size(rect.min + vec2(r.x, r.y) * scale, vec2(r.width, r.height) * scale);
                p.rect_stroke(b, 1.0, egui::Stroke::new(1.0, look.accent()), egui::StrokeKind::Outside);
            }
        }
        let hint = if st.large { &smaller } else { &larger };
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hint).clicked() {
            st.large = !st.large;
        }
    };

    if st.large {
        draw_picture(ui, st);
        ui.add_space(6.0);
        ui.horizontal(|ui| picture_info(app, ui, st, true));
    } else {
        ui.horizontal(|ui| {
            draw_picture(ui, st);
            ui.add_space(8.0);
            ui.vertical(|ui| picture_info(app, ui, st, false));
        });
    }
}

/// The boxes worth drawing over the picture: its lines where the engine
/// found lines (whatever sits above them), else its blocks; the axis-aligned
/// box around each, so rotated text is covered too.
fn found_boxes(ocr: &glossa_core::OcrResult) -> Vec<GRect> {
    let ocr = ocr.clone().with_levels();
    let lines: Vec<GRect> =
        ocr.nodes().filter(|n| n.level == Some(glossa_core::ocr::OcrLevel::Line)).filter_map(|n| n.bbox()).collect();
    if !lines.is_empty() {
        return lines;
    }
    ocr.blocks.iter().filter_map(|b| b.bbox()).collect()
}

/// What read the picture, how long it took, what it found, and the
/// picture's toggles.
fn picture_info(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow, row: bool) {
    let look = app.look.clone();
    let weak = look.weak_text();
    let mut lines: Vec<RichText> = Vec::new();
    if st.recognising {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new(app.t("msg.recognizing")).strong());
        });
    } else if let Some(o) = &st.outcome {
        let engine = provider_name(app, &o.ocr.provider_id);
        lines.push(RichText::new(engine).strong());
        let mut facts = vec![format!("{} × {} px", st.image.width(), st.image.height())];
        facts.push(app.locale.fmt("msg.ms", &o.ocr.elapsed_ms.to_string()));
        if let Some(l) = o.ocr.detected_lang.filter(|l| l.is_language()) {
            facts.push(crate::langs::name(l));
        }
        lines.push(RichText::new(facts.join(" · ")).small().color(weak));
        let count = app.locale.fmt_args(
            "ocr.result.counts",
            &[
                &st.text.chars().count().to_string(),
                &st.text.lines().filter(|l| !l.trim().is_empty()).count().to_string(),
            ],
        );
        lines.push(RichText::new(count).small().color(weak));
    } else if st.error.is_some() {
        lines.push(RichText::new(app.t("ocr.result.failed")).strong().color(look.error()));
    }
    let toggles = |ui: &mut egui::Ui, st: &mut ResultWindow| {
        let (icon, key) =
            if st.large { (Icon::Shrink, "ocr.result.smaller") } else { (Icon::Expand, "ocr.result.larger") };
        if icons::sized_button(ui, &look, icon, &app.t(key), 24.0, true).clicked() {
            st.large = !st.large;
        }
        let has_boxes = !st.found.is_empty();
        crate::widgets::icon_toggle(ui, &look, Icon::Boxes, &mut st.boxes, &app.t("ocr.result.boxes"));
        if !has_boxes {
            st.boxes = false;
        }
    };
    if row {
        for (i, l) in lines.into_iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("·").small().color(weak));
            }
            ui.label(l);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| toggles(ui, st));
    } else {
        for l in lines {
            ui.add(egui::Label::new(l).truncate());
        }
        ui.horizontal(|ui| toggles(ui, st));
    }
}

fn provider_name(app: &GlossaApp, id: &str) -> String {
    app.engine.registry().get(id).map(|p| p.info().name.clone()).unwrap_or_else(|| id.to_owned())
}

/// Listen and copy in a pane's bottom-right corner, as in the main window.
fn corner_buttons(app: &GlossaApp, ui: &mut egui::Ui, rect: Rect, enabled: bool) -> (bool, bool) {
    let look = &app.look;
    let size = 22.0 * look.icon_scale;
    let corner =
        Rect::from_min_size(rect.right_bottom() - vec2(2.0 * size + 8.0, size + 2.0), vec2(2.0 * size + 6.0, size));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(corner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let copy = icons::sized_button(&mut child, look, Icon::Copy, &app.t("popup.copy"), 22.0, enabled).clicked();
    let listen = icons::sized_button(&mut child, look, Icon::Listen, &app.t("msg.listen_tip"), 22.0, enabled).clicked();
    (listen, copy)
}

fn text_pane(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow, height: f32) -> Option<Command> {
    let look = app.look.clone();
    let font = look.text_font();
    let margin = crate::main_window::PANE_MARGIN;
    let inner_h = height - 2.0 * margin as f32;
    let mut listen_copy = (false, false);
    let mut command = None;
    let shown = crate::main_window::pane_frame(&look).show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), inner_h));
        ui.set_max_height(inner_h);
        let rect = ui.max_rect();
        if st.recognising && st.text.is_empty() {
            ui.centered_and_justified(|ui| ui.add(egui::Spinner::new().size(22.0)));
            return;
        }
        if let Some(e) = st.error.clone() {
            ui.label(RichText::new(e).color(look.error()));
            ui.add_space(6.0);
            if ui.button(app.t("ocr.result.select_again")).clicked() {
                command = Some(Command::SelectAgain);
            }
            return;
        }
        egui::ScrollArea::vertical().id_salt("ocr-result-text").max_height(inner_h).show(ui, |ui| {
            let color = c(look.theme.edit.normal.text);
            let halign = if glossa_core::text::guessed_direction(&st.text).is_rtl() {
                egui::Align::RIGHT
            } else {
                egui::Align::LEFT
            };
            let layout_font = font.clone();
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                crate::bidi::editor_layout(ui, buf.as_str(), &layout_font, color, wrap_width, halign)
            };
            let edit = egui::TextEdit::multiline(&mut st.text)
                .font(font)
                .text_color(color)
                .align(egui::Align2([halign, egui::Align::TOP]))
                .layouter(&mut layouter)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .lock_focus(false);
            let r = ui.add_sized(vec2(ui.available_width(), inner_h - 4.0), edit);
            if r.changed() {
                app.fonts.ensure(&app.ctx, &st.text);
            }
        });
        listen_copy = corner_buttons(app, ui, rect, !st.text.trim().is_empty());
    });
    app.text_zoom_wheel(ui, shown.response.rect);
    if listen_copy.0 {
        let (text, lang) = (st.text.clone(), source_language(st));
        app.speak(&text, lang);
    }
    if listen_copy.1 {
        let text = glossa_engine::ocr::text_for_copy(&st.text, app.cfg.ocr.copy_joined_lines);
        copy(app, st, &text);
    }
    command
}

/// The language direction, the translator and Translate.
fn direction_bar(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow) -> Option<Command> {
    let look = app.look.clone();
    let mut command = None;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let disabled = app.cfg.languages.disabled.clone();
        let auto = app.t("main.auto_detect");
        let search = app.t("lang.search");
        let translate_w = 110.0;
        let translator_w = 150.0;
        let swap_w = 26.0 * look.icon_scale;
        let spacing = ui.spacing().item_spacing.x * 4.0;
        let combo_w = ((ui.available_width() - translate_w - translator_w - swap_w - spacing) / 2.0).max(90.0);
        let detected = source_language(st);
        let from_label = match (st.from, detected) {
            (LangId::AUTO, d) if d.is_language() => app.locale.fmt("main.auto_detect_as", &crate::langs::name(d)),
            _ => auto.clone(),
        };
        if let Some(l) =
            crate::widgets::lang_combo(ui, "ocr-result-from", st.from, true, &from_label, &search, &disabled, combo_w)
        {
            st.from = l;
            if unedited(st) && st.outcome.is_some() {
                // Engines read better knowing the language: read it again.
                let to = (st.translation.is_some() || st.translate).then_some(st.to);
                recognise(app, st, to, false);
            } else if st.translation.is_some() {
                translate(app, st);
            }
        }
        let can_swap = st.translation.is_some() && !st.translating;
        if icons::sized_button(ui, &look, Icon::Swap, &tip(app, "msg.swap_tip", Command::Swap), 26.0, can_swap)
            .clicked()
        {
            command = Some(Command::Swap);
        }
        if let Some(l) =
            crate::widgets::lang_combo(ui, "ocr-result-to", st.to, false, &auto, &search, &disabled, combo_w)
        {
            st.to = l;
            translate(app, st);
        }
        if let Some(id) = translator_picker(app, ui, st, translator_w) {
            st.translator = Some(id);
            translate(app, st);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = egui::Button::new(app.t("main.translate")).min_size(vec2(translate_w - 10.0, 26.0));
            let enabled = !st.text.trim().is_empty() && !st.recognising;
            if ui.add_enabled(enabled, b).on_hover_text(shortcut_text(Command::Translate)).clicked() {
                command = Some(Command::Translate);
            }
        });
    });
    ui.add_space(6.0);
    command
}

/// The translators to choose from: Google Lens (its own translation of the
/// picture) and the enabled translation services.
fn translators(app: &GlossaApp) -> Vec<(String, String)> {
    let reg = app.engine.registry();
    let mut out = Vec::new();
    if let Some(lens) = reg.get(LENS) {
        out.push((LENS.to_owned(), lens.info().name.clone()));
    }
    out.extend(app.cfg.services.enabled.iter().filter_map(|id| {
        reg.get(id)
            .filter(|p| p.info().capabilities.contains(Capabilities::TRANSLATE))
            .map(|p| (id.clone(), p.info().name.clone()))
    }));
    out
}

fn translator_picker(app: &mut GlossaApp, ui: &mut egui::Ui, st: &ResultWindow, width: f32) -> Option<String> {
    let id = ui.make_persistent_id("ocr-result-translator");
    let current = st.translator.clone().unwrap_or_else(|| app.cfg.services.active.clone());
    let label = provider_name(app, &current);
    let button = crate::dropdown::combo_button(ui, &label, width, crate::dropdown::is_open(ui.ctx(), id));
    let button = button.on_hover_text(app.t("ocr.result.translator"));
    if button.clicked() {
        crate::dropdown::toggle(ui.ctx(), id);
    }
    let items = translators(app);
    let lens_note = app.t("ocr.result.lens_note");
    let mark_id = id.with("mark");
    if !crate::dropdown::is_open(ui.ctx(), id) {
        ui.ctx().data_mut(|d| d.remove::<usize>(mark_id));
    }
    let mut picked = None;
    crate::dropdown::show(ui, id, button.rect, button.rect.width(), 420.0, |ui, _| {
        // The keyboard's place in the list, the current translator at first.
        let stored: Option<usize> = ui.ctx().data(|d| d.get_temp(mark_id));
        let mut mark = stored.unwrap_or_else(|| items.iter().position(|(p, _)| *p == current).unwrap_or(0));
        let (down, up, enter) =
            ui.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Enter)));
        if down {
            mark += 1;
        }
        if up {
            mark = mark.saturating_sub(1);
        }
        mark = mark.min(items.len().saturating_sub(1));
        if enter {
            picked = items.get(mark).map(|(p, _)| p.clone());
            ui.close();
        }
        for (n, (pid, name)) in items.iter().enumerate() {
            let mut r = ui.selectable_label(n == mark, name);
            if pid == LENS {
                r = r.on_hover_text(&lens_note);
            }
            if r.clicked() {
                picked = Some(pid.clone());
                ui.close();
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(mark_id, mark));
    });
    picked
}

fn translation_pane(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut ResultWindow, height: f32) {
    let look = app.look.clone();
    let margin = crate::main_window::PANE_MARGIN;
    let inner_h = height - 2.0 * margin as f32;
    let mut listen_copy = (false, false);
    let mut translate_now = false;
    let shown = crate::main_window::pane_frame(&look).show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), inner_h));
        ui.set_max_height(inner_h);
        let rect = ui.max_rect();
        let text_colour = c(look.theme.edit.normal.text);
        egui::ScrollArea::vertical().id_salt("ocr-result-translation").stick_to_bottom(st.translating).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(p) = st.partial.clone().filter(|_| st.translating) {
                let rtl = glossa_core::text::guessed_direction(&p).is_rtl();
                crate::bidi::label(ui, RichText::new(p).font(look.text_font()).color(text_colour), rtl);
            } else if st.translating || (st.recognising && st.translate) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(RichText::new(app.t("ocr.result.translating")).color(look.weak_text()));
                });
            } else if let Some(e) = st.tr_error.clone() {
                ui.label(RichText::new(e).color(look.error()).font(look.text_font()));
            } else if let Some(t) = st.translation.clone() {
                let rtl = t.pair.to.is_rtl() || glossa_core::text::guessed_direction(&t.translation.text).is_rtl();
                let text = RichText::new(t.translation.text.clone()).font(look.text_font()).color(text_colour);
                crate::bidi::label(ui, text, rtl);
            } else if !st.recognising && st.error.is_none() && !st.text.trim().is_empty() {
                ui.add_space(6.0);
                let hint = app.locale.fmt("ocr.result.translate_hint", &shortcut_text(Command::Translate));
                let r = ui.add(egui::Label::new(RichText::new(hint).color(look.weak_text())).sense(Sense::click()));
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    translate_now = true;
                }
            }
        });
        listen_copy = corner_buttons(app, ui, rect, st.translation.is_some());
    });
    app.text_zoom_wheel(ui, shown.response.rect);
    if translate_now {
        translate(app, st);
    }
    if let Some(t) = st.translation.clone() {
        if listen_copy.0 {
            app.speak(&t.translation.text, t.pair.to);
        }
        if listen_copy.1 {
            copy(app, st, &t.translation.text);
        }
    }
}

/// Who translated and how long it took, that the text changed since, or a
/// moment's "Copied".
fn status_line(app: &mut GlossaApp, ui: &mut egui::Ui, st: &ResultWindow) {
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some((m, _)) = st.flash.as_ref().filter(|(m, _)| !m.is_empty()) {
            ui.label(RichText::new(m).small().color(look.accent()));
            return;
        }
        let Some(t) = &st.translation else { return };
        if !st.translating && st.translated_text.trim() != st.text.trim() {
            ui.label(RichText::new(app.t("ocr.result.stale")).small().color(look.weak_text()));
            return;
        }
        let mut s = format!(
            "{} · {} → {}",
            provider_name(app, &t.translation.provider_id),
            crate::langs::name(t.pair.from),
            crate::langs::name(t.pair.to)
        );
        if t.translation.elapsed_ms > 0 {
            s.push_str(" · ");
            s.push_str(&app.locale.fmt("msg.ms", &t.translation.elapsed_ms.to_string()));
        }
        ui.label(RichText::new(s).small().color(look.weak_text()));
    });
}

/// The buttons along the bottom: the frequent actions with their names,
/// the rest as icons.
fn action_bar(app: &mut GlossaApp, ui: &mut egui::Ui, st: &ResultWindow) -> Option<Command> {
    let look = app.look.clone();
    let has_text = !st.text.trim().is_empty();
    let has_translation = st.translation.is_some();
    let mut command = None;
    // Names only while they fit beside the icons on the right; the tooltips
    // say them anyway.
    let named = ["ocr.copy_text", "history.copy_translation", "ocr.in_place", "ocr.result.to_main"];
    let font = egui::TextStyle::Button.resolve(ui.style());
    let icon = (16.0 * look.icon_scale).round();
    let button_h = ui.spacing().interact_size.y.max(icon + 12.0);
    // In order of importance: each gets its name while there is room.
    let mut room = ui.available_width() - 4.0 * button_h - 9.0 * 2.0 - 24.0;
    let mut labelled = [false; 4];
    for (i, k) in named.iter().enumerate() {
        let named_w =
            ui.painter().layout_no_wrap(app.t(k), font.clone(), egui::Color32::PLACEHOLDER).size().x + icon + 22.0;
        let others = (named.len() - i - 1) as f32 * button_h;
        if named_w + others <= room {
            labelled[i] = true;
            room -= named_w;
        } else {
            room -= button_h;
        }
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut button = |ui: &mut egui::Ui, icon: Icon, key: &str, cmd: Command, enabled: bool, named: bool| {
            let label = if named { app.t(key) } else { String::new() };
            if icons::command_button(ui, &look, icon, &label, &tip(app, key, cmd), enabled).clicked() {
                command = Some(cmd);
            }
        };
        button(ui, Icon::Copy, named[0], Command::CopyText, has_text, labelled[0]);
        button(ui, Icon::Copy, named[1], Command::CopyTranslation, has_translation, labelled[1]);
        button(ui, Icon::Overlay, named[2], Command::InPlace, !st.recognising, labelled[2]);
        button(ui, Icon::Open, named[3], Command::ToMain, has_text, labelled[3]);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            button(ui, Icon::Crop, "ocr.result.select_again", Command::SelectAgain, true, false);
            button(ui, Icon::Save, "ocr.save_image", Command::SaveImage, true, false);
            button(ui, Icon::Image, "ocr.copy_image", Command::CopyImage, true, false);
            button(ui, Icon::Listen, "ocr.result.read_aloud", Command::ReadAloud, has_text, false);
        });
    });
    command
}

/// Put `text` on the clipboard and say so in the status line.
fn copy(app: &mut GlossaApp, st: &mut ResultWindow, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let result = match app.clipboard.as_mut() {
        Some(cb) => cb.set_text(text).map_err(|e| e.to_string()),
        None => {
            app.ctx.copy_text(text.to_owned());
            Ok(())
        }
    };
    let m = match result {
        Ok(()) => app.locale.fmt("ocr.copied_chars", &text.chars().count().to_string()),
        Err(e) => e,
    };
    st.flash = Some((m, Instant::now()));
}

/// Do `cmd`. Returns whether the window stays open.
fn run(app: &mut GlossaApp, st: &mut ResultWindow, cmd: Command) -> bool {
    match cmd {
        Command::CopyText => {
            let text = glossa_engine::ocr::text_for_copy(&st.text, app.cfg.ocr.copy_joined_lines);
            copy(app, st, &text);
        }
        Command::CopyTranslation => {
            if let Some(t) = st.translation.clone() {
                copy(app, st, &t.translation.text);
            }
        }
        Command::CopyImage => {
            let result = match app.clipboard.as_mut() {
                Some(cb) => cb.set_image(&st.image).map_err(|e| e.to_string()),
                None => Err(app.t("ocr.result.no_clipboard")),
            };
            let m = result.map(|_| app.t("msg.copied")).unwrap_or_else(|e| e);
            st.flash = Some((m, Instant::now()));
        }
        Command::SaveImage => crate::ocr::save_image(app, &st.image),
        Command::InPlace => {
            if !st.recognising {
                crate::ocr::start_in_place(app, st.image.clone(), st.region, st.from);
            }
        }
        Command::ToMain => {
            let text = st.text.trim().to_owned();
            if text.is_empty() {
                return true;
            }
            app.main.source = text.clone();
            match st.translation.clone().filter(|_| st.translated_text.trim() == text) {
                // Already translated: no second request.
                Some(t) => {
                    app.main.translated_source = text;
                    app.main.error = None;
                    app.main.outcome = Some(t);
                }
                None => {
                    app.main.translated_source.clear();
                    app.translate_main();
                }
            }
            app.show_main();
            return false;
        }
        Command::SelectAgain => {
            let action = if st.translate { Action::RecognizeAndTranslate } else { Action::TextRecognition };
            app.perform(action, None);
            return false;
        }
        Command::ReadAloud => match st.translation.clone() {
            Some(t) => app.speak(&t.translation.text, t.pair.to),
            None => {
                let (text, lang) = (st.text.clone(), source_language(st));
                app.speak(&text, lang);
            }
        },
        Command::Translate => translate(app, st),
        Command::Swap => swap(app, st),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_goes_beside_the_region_or_below_it() {
        let work = Some(GRect::new(0.0, 0.0, 1920.0, 1040.0));
        let size = vec2(600.0, 500.0);
        // Room on the right.
        assert_eq!(place_beside(GRect::new(100.0, 100.0, 300.0, 80.0), size, work), (412.0, 100.0));
        // Not on the right, so on the left, kept above the taskbar.
        assert_eq!(place_beside(GRect::new(1400.0, 900.0, 300.0, 80.0), size, work), (788.0, 540.0));
        // A region across the whole width: below it.
        assert_eq!(place_beside(GRect::new(0.0, 100.0, 1920.0, 80.0), size, work), (0.0, 192.0));
        // Across the whole width near the bottom: above it.
        assert_eq!(place_beside(GRect::new(0.0, 800.0, 1920.0, 200.0), size, work), (0.0, 288.0));
    }

    #[test]
    fn shortcuts_read_as_words() {
        let ctrl = glossa_platform::shell::SHELL.command_key;
        assert_eq!(shortcut_text(Command::CopyText), format!("{ctrl}+Shift+C"));
        assert_eq!(shortcut_text(Command::Translate), format!("{ctrl}+Enter"));
    }

    #[test]
    fn found_boxes_are_lines_where_there_are_lines() {
        use glossa_core::ocr::OcrBlock;
        let line = |y: f32| OcrBlock { bounds: Some(GRect::new(0.0, y, 10.0, 5.0)), ..Default::default() };
        let para = OcrBlock {
            bounds: Some(GRect::new(0.0, 0.0, 10.0, 20.0)),
            children: vec![line(0.0), line(8.0)],
            level: Some(glossa_core::ocr::OcrLevel::Block),
            ..Default::default()
        };
        let lone = OcrBlock { bounds: Some(GRect::new(0.0, 40.0, 10.0, 5.0)), ..Default::default() };
        let ocr = glossa_core::OcrResult { blocks: vec![para, lone], ..Default::default() };
        assert_eq!(found_boxes(&ocr).len(), 3);
    }
}
