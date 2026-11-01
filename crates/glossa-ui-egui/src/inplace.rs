//! A translation painted over the picture it came from, the way Chrome's
//! Lens overlay shows one.
//!
//! The window sits exactly over the screen region that was recognised and
//! shows the captured image with each block's translation laid over it:
//! the block's box is filled with the background colour sampled from just
//! around it, and the text is drawn in the colour of the original letters,
//! at the original angle, sized to fit. Lens hands over the translation
//! line by line (each line where a source line was, never wrapped, scaled
//! down to fit, as Chromium does); a translator's paragraph is reflowed into
//! its paragraph's box instead. A strip under the image (above it at the
//! bottom of the screen) toggles the original and copies the translation;
//! Escape or a click anywhere else closes it.
//!
//! Over a whole monitor (Translate the whole screen) the window covers the
//! monitor exactly and the strip floats over the picture, at the bottom in
//! the middle: only the places that had text are painted over.

use std::sync::Arc;

use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Sense, Vec2};
use glossa_core::ocr::Rect as GRect;
use glossa_engine::OcrOutcome;

use crate::app::GlossaApp;

const TITLE: &str = "Glossa in-place translation";
/// The strip with the buttons, in points.
const BAR: f32 = 34.0;
/// The window is at least this wide so the strip fits; a narrower region
/// leaves the rest of the window transparent.
const MIN_WIDTH: f32 = 240.0;

pub struct InPlace {
    /// The captured region, in its own pixels.
    image: image::RgbaImage,
    /// Where it is on the virtual desktop, in pixels.
    region: GRect,
    /// The display scale of the monitor it is on.
    scale: f32,
    outcome: Arc<OcrOutcome>,
    items: Vec<Item>,
    texture: Option<egui::TextureHandle>,
    show_original: bool,
    /// Whether the window has had the focus; losing it after that means a
    /// click elsewhere, which closes it.
    focused: bool,
    bar_on_top: bool,
    /// The region is a whole monitor: no room around it for the strip.
    full_screen: bool,
}

/// One thing to paint: a line (Lens) or a paragraph (anything else).
struct Item {
    /// The box, unrotated, around its centre, in image pixels.
    centre: Vec2,
    size: Vec2,
    angle: f32,
    text: String,
    /// Reflow into the box (a paragraph) rather than keep one line.
    wrap: bool,
    background: Color32,
    text_color: Color32,
    /// The font size that fits, in points, worked out on the first paint.
    font_size: Option<f32>,
}

/// Open the window for a finished in-place recognition of `image`, which
/// was captured at `region`.
pub fn open(app: &mut GlossaApp, image: image::RgbaImage, region: GRect, outcome: Arc<OcrOutcome>) {
    let (cx, cy) = region.centre();
    let monitor = glossa_platform::window::monitor_at(cx as i32, cy as i32)
        .or_else(|| glossa_platform::window::monitors().into_iter().find(|m| m.is_primary));
    let scale = monitor.as_ref().map(|m| m.desktop_scale()).unwrap_or(1.0);
    let full_screen = monitor.as_ref().is_some_and(|m| covers(region, m.bounds()));
    let bottom = monitor.map(|m| m.work_area.y + m.work_area.height).unwrap_or(f32::MAX);
    let bar_on_top = !full_screen && region.y + region.height + BAR * scale > bottom;
    tracing::debug!(?region, scale, bar_on_top, blocks = outcome.blocks.len(), "in-place window");
    let items = items_for(&image, &outcome);
    for item in &items {
        app.fonts.ensure(&app.ctx, &item.text);
    }
    app.in_place = Some(InPlace {
        image,
        region,
        scale,
        outcome,
        items,
        texture: None,
        show_original: false,
        focused: false,
        bar_on_top,
        full_screen,
    });
    app.ctx.request_repaint();
}

/// Whether `region` is `monitor` (to within a pixel or two of rounding).
fn covers(region: GRect, monitor: GRect) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() <= 2.0;
    near(region.x, monitor.x)
        && near(region.y, monitor.y)
        && near(region.width, monitor.width)
        && near(region.height, monitor.height)
}

/// What to paint for each translated block, with colours sampled from the
/// image around and inside it.
fn items_for(image: &image::RgbaImage, outcome: &OcrOutcome) -> Vec<Item> {
    let mut out = Vec::new();
    for block in &outcome.blocks {
        if block.lines.is_empty() {
            out.push(item(image, block.bounds, block.angle, &block.translation, true));
        } else {
            for line in &block.lines {
                out.push(item(image, line.bounds, line.angle, &line.text, false));
            }
        }
    }
    out
}

fn item(image: &image::RgbaImage, r: GRect, angle: f32, text: &str, wrap: bool) -> Item {
    let (bg, fg) = sample_colours(image, r);
    Item {
        centre: vec2(r.x + r.width / 2.0, r.y + r.height / 2.0),
        size: vec2(r.width, r.height),
        angle,
        text: text.trim().to_owned(),
        wrap,
        background: Color32::from_rgb(bg[0], bg[1], bg[2]),
        text_color: Color32::from_rgb(fg[0], fg[1], fg[2]),
        font_size: None,
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.in_place.take() else { return };
    let s = st.scale;
    let (w, h) = (st.region.width / s, st.region.height / s);
    let x = st.region.x / s;
    let y = st.region.y / s - if st.bar_on_top { BAR } else { 0.0 };
    // Over a whole monitor the strip floats on the picture.
    let inner = if st.full_screen { vec2(w, h) } else { vec2(w.max(MIN_WIDTH), h + BAR) };
    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_transparent(true)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_position(pos2(x, y))
        .with_inner_size(inner)
        .with_active(true);
    let mut keep = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(TITLE), builder, |ui, _| {
        keep = body(app, ui, &mut st, vec2(w, h));
    });
    if keep {
        app.in_place = Some(st);
    }
}

fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut InPlace, size: Vec2) -> bool {
    let ctx = ui.ctx().clone();
    let (escape, close, focused, space, copy) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.viewport().close_requested(),
            i.viewport().focused,
            i.key_pressed(egui::Key::Space),
            i.key_pressed(egui::Key::C) && !i.modifiers.any(),
        )
    });
    if escape || close {
        return false;
    }
    match focused {
        Some(true) => st.focused = true,
        Some(false) if st.focused => return false, // clicked elsewhere
        _ => {}
    }
    if space {
        st.show_original = !st.show_original;
    }
    if copy {
        copy_translation(app, st);
    }
    let texture = st
        .texture
        .get_or_insert_with(|| {
            let px = [st.image.width() as usize, st.image.height() as usize];
            let img = egui::ColorImage::from_rgba_unmultiplied(px, st.image.as_raw());
            ctx.load_texture("in-place", img, egui::TextureOptions::LINEAR)
        })
        .clone();
    let look = app.look.clone();
    let mut keep = true;
    egui::CentralPanel::no_frame().show(ui, |ui| {
        let full = ui.max_rect();
        let top = if st.bar_on_top { full.min.y + BAR } else { full.min.y };
        let picture = Rect::from_min_size(pos2(full.min.x, top), size);
        let bar = if st.full_screen {
            let width = (full.width() - 32.0).min(620.0);
            Rect::from_center_size(pos2(full.center().x, full.max.y - 32.0 - BAR / 2.0), vec2(width, BAR))
        } else {
            let bar_y = if st.bar_on_top { full.min.y } else { picture.max.y };
            Rect::from_min_size(pos2(full.min.x, bar_y), vec2(full.width(), BAR))
        };
        let p = ui.painter().clone();
        p.image(texture.id(), picture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let held = ui.allocate_rect(picture, Sense::click());
        // Holding the mouse on the picture shows the original, as the
        // userscript's long press does.
        let peek = held.is_pointer_button_down_on();
        if !st.show_original && !peek {
            let k = picture.width() / st.image.width().max(1) as f32;
            for item in &mut st.items {
                paint_item(&p, picture.min, k, item);
            }
        }
        if st.full_screen {
            let r = egui::CornerRadius::same((BAR / 2.0) as u8);
            let shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) };
            p.add(shadow.as_shape(bar, r));
            p.rect(
                bar,
                r,
                look.popup_back.to_opaque(),
                egui::Stroke::new(1.0, look.border()),
                egui::StrokeKind::Inside,
            );
        } else {
            p.rect_filled(bar, 0.0, look.popup_back);
        }
        let inset = if st.full_screen { vec2(12.0, 2.0) } else { vec2(4.0, 2.0) };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(inset)));
        child.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y = BAR - 4.0;
            use crate::icons::{command_button, Icon};
            // Names while the strip has room for them; icons with tooltips
            // over a narrow region.
            let named = bar.width() >= 480.0;
            let shown = |label: &str| if named { label.to_owned() } else { String::new() };
            let (label, key) =
                if st.show_original { (app.t("ocr.translation"), "Space") } else { (app.t("ocr.original"), "Space") };
            if command_button(ui, &look, Icon::Image, &shown(&label), &format!("{label} ({key})"), true).clicked() {
                st.show_original = !st.show_original;
            }
            let copy_tr = app.t("history.copy_translation");
            if command_button(ui, &look, Icon::Copy, &shown(&copy_tr), &format!("{copy_tr} (C)"), true).clicked() {
                copy_translation(app, st);
            }
            let copy_text = app.t("ocr.copy_text");
            if command_button(ui, &look, Icon::Copy, &shown(&copy_text), &copy_text, true).clicked() {
                let text = glossa_engine::ocr::copy_text(&st.outcome.ocr, app.cfg.ocr.copy_joined_lines);
                app.copy_text(&text);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::icons::sized_button(ui, &look, crate::icons::Icon::Close, &app.t("ocr.close"), 18.0, true)
                    .clicked()
                {
                    keep = false;
                }
                let provider = st
                    .outcome
                    .translation
                    .as_ref()
                    .map(|t| t.translation.provider_id.clone())
                    .unwrap_or_else(|| st.outcome.ocr.provider_id.clone());
                let name = app.engine.registry().get(&provider).map(|p| p.info().name.clone()).unwrap_or(provider);
                ui.add(egui::Label::new(egui::RichText::new(name).small().color(look.weak_text())).truncate())
                    .on_hover_text(app.t("ocr.in_place_keys"));
            });
        });
    });
    keep
}

fn copy_translation(app: &mut GlossaApp, st: &InPlace) {
    let text = st
        .outcome
        .translated_text()
        .map(str::to_owned)
        .unwrap_or_else(|| st.outcome.blocks.iter().map(|b| b.translation.as_str()).collect::<Vec<_>>().join("\n"));
    app.copy_text(&text);
}

/// Paint one item: its box in the background colour, then its text fitted
/// inside, both turned by its angle. `origin` is the picture's top-left on
/// screen and `k` points per image pixel.
fn paint_item(p: &egui::Painter, origin: Pos2, k: f32, item: &mut Item) {
    let centre = origin + item.centre * k;
    let size = item.size * k;
    if size.x < 2.0 || size.y < 2.0 || item.text.is_empty() {
        return;
    }
    // A line box hugs the letters; grow it a little so their edges are
    // covered too. A paragraph box already has some air.
    let pad = if item.wrap { vec2(3.0, 3.0) * k } else { vec2(0.06 * size.y, 0.16 * size.y) };
    fill_rotated(p, centre, size + pad * 2.0, item.angle, item.background);
    let font = FontId::proportional(*item.font_size.get_or_insert_with(|| fit(p, &item.text, size, item.wrap)));
    let galley = if item.wrap {
        p.layout(item.text.clone(), font, item.text_color, size.x)
    } else {
        p.layout_no_wrap(item.text.clone(), font, item.text_color)
    };
    // Left-aligned and centred vertically in the box, as Chromium sets it.
    let offset = vec2(-size.x / 2.0, -galley.size().y / 2.0);
    let pos = centre + rotate(offset, item.angle);
    p.add(egui::epaint::TextShape::new(pos, galley, item.text_color).with_angle(item.angle));
}

fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (sin, cos) = angle.sin_cos();
    vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

fn fill_rotated(p: &egui::Painter, centre: Pos2, size: Vec2, angle: f32, colour: Color32) {
    if angle.abs() < 0.002 {
        p.rect_filled(Rect::from_center_size(centre, size), 1.0, colour);
        return;
    }
    let h = size / 2.0;
    let corners = [vec2(-h.x, -h.y), vec2(h.x, -h.y), vec2(h.x, h.y), vec2(-h.x, h.y)]
        .map(|c| centre + rotate(c, angle))
        .to_vec();
    p.add(egui::Shape::convex_polygon(corners, colour, egui::Stroke::NONE));
}

/// The largest font size, in points, at which `text` fits a box of `size`
/// points: on one line (scaled down rather than wrapped, as Chromium does
/// for a line), or wrapped to the box's width (a paragraph).
fn fit(p: &egui::Painter, text: &str, size: Vec2, wrap: bool) -> f32 {
    let fits = |s: f32| {
        let font = FontId::proportional(s);
        if wrap {
            let g = p.layout(text.to_owned(), font, Color32::WHITE, size.x);
            g.size().x <= size.x + 0.5 && g.size().y <= size.y
        } else {
            let g = p.layout_no_wrap(text.to_owned(), font, Color32::WHITE);
            // A row is taller than the letters a detector boxes; let it
            // overhang a little rather than shrink the text further.
            g.size().x <= size.x && g.size().y <= size.y * 1.3
        }
    };
    let (mut lo, mut hi) = (4.0_f32, (size.y * 1.2).clamp(4.0, 120.0));
    if fits(hi) {
        return hi;
    }
    for _ in 0..12 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The background around a box and the colour of the text in it, as RGB.
///
/// The background is the median of a ring of pixels just outside the box,
/// which is what the letters were drawn on. The text colour is the average
/// of the pixels inside that differ most from it; when nothing inside
/// differs enough (a blurry or tiny box), or the two are too alike to read,
/// it is black or white, whichever stands out.
pub fn sample_colours(image: &image::RgbaImage, r: GRect) -> ([u8; 3], [u8; 3]) {
    let (w, h) = (image.width() as i64, image.height() as i64);
    if w == 0 || h == 0 {
        return ([255; 3], [0; 3]);
    }
    let px = |x: i64, y: i64| {
        let p = image.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32).0;
        [p[0], p[1], p[2]]
    };
    let (x0, y0) = (r.x.floor() as i64, r.y.floor() as i64);
    let (x1, y1) = ((r.x + r.width).ceil() as i64, (r.y + r.height).ceil() as i64);
    // The ring: two pixels out from the box, sampled at most ~100 times a side.
    let mut ring: Vec<[u8; 3]> = Vec::new();
    let gap = 2;
    let step_x = ((x1 - x0) / 100).max(1);
    let step_y = ((y1 - y0) / 100).max(1);
    let mut x = x0 - gap;
    while x <= x1 + gap {
        ring.push(px(x, y0 - gap));
        ring.push(px(x, y1 + gap));
        x += step_x;
    }
    let mut y = y0 - gap;
    while y <= y1 + gap {
        ring.push(px(x0 - gap, y));
        ring.push(px(x1 + gap, y));
        y += step_y;
    }
    let bg = median(&mut ring);

    let mut inside: Vec<([u8; 3], u32)> = Vec::new();
    let step_x = ((x1 - x0) / 60).max(1);
    let step_y = ((y1 - y0) / 30).max(1);
    let mut y = y0.max(0);
    while y < y1.min(h) {
        let mut x = x0.max(0);
        while x < x1.min(w) {
            let c = px(x, y);
            inside.push((c, distance(c, bg)));
            x += step_x;
        }
        y += step_y;
    }
    inside.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
    let top = &inside[..(inside.len() / 6).max(1).min(inside.len())];
    let fallback = if luminance(bg) > 0.5 { [0; 3] } else { [255; 3] };
    let fg = if top.is_empty() || top[0].1 < 60 {
        fallback
    } else {
        let n = top.len() as u32;
        let sum = top.iter().fold([0u32; 3], |a, (c, _)| [a[0] + c[0] as u32, a[1] + c[1] as u32, a[2] + c[2] as u32]);
        [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
    };
    let fg = if (luminance(fg) - luminance(bg)).abs() < 0.3 { fallback } else { fg };
    (bg, fg)
}

fn median(colours: &mut [[u8; 3]]) -> [u8; 3] {
    if colours.is_empty() {
        return [255; 3];
    }
    let mid = colours.len() / 2;
    let mut out = [0u8; 3];
    for (c, slot) in out.iter_mut().enumerate() {
        colours.sort_by_key(|p| p[c]);
        *slot = colours[mid][c];
    }
    out
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3).map(|i| (a[i] as i32 - b[i] as i32).unsigned_abs()).sum()
}

fn luminance(c: [u8; 3]) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_whole_monitor_is_told_from_a_region() {
        let monitor = GRect::new(-1920.0, 0.0, 1920.0, 1080.0);
        assert!(covers(GRect::new(-1920.0, 0.0, 1920.0, 1080.0), monitor));
        assert!(covers(GRect::new(-1919.5, 0.4, 1920.6, 1079.2), monitor), "rounding");
        assert!(!covers(GRect::new(-1800.0, 100.0, 600.0, 300.0), monitor));
    }

    use super::*;

    /// A cream image with a block of dark blue "text" in a box.
    fn picture() -> image::RgbaImage {
        let mut img = image::RgbaImage::from_pixel(100, 60, image::Rgba([250, 240, 200, 255]));
        for y in 22..38 {
            for x in 20..80 {
                // Letters cover part of the box; the gaps are background.
                if (x / 3 + y / 4) % 3 == 0 {
                    img.put_pixel(x, y, image::Rgba([20, 30, 120, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn colours_come_from_around_and_inside_the_box() {
        let (bg, fg) = sample_colours(&picture(), GRect::new(20.0, 20.0, 60.0, 20.0));
        assert_eq!(bg, [250, 240, 200]);
        assert_eq!(fg, [20, 30, 120]);
    }

    #[test]
    fn unreadable_or_missing_text_colours_fall_back_to_black_or_white() {
        let blank = image::RgbaImage::from_pixel(50, 50, image::Rgba([30, 30, 30, 255]));
        assert_eq!(sample_colours(&blank, GRect::new(10.0, 10.0, 20.0, 10.0)), ([30, 30, 30], [255, 255, 255]));
        // A box at the very edge samples clamped pixels instead of panicking.
        let (bg, _) = sample_colours(&picture(), GRect::new(-5.0, -5.0, 200.0, 200.0));
        assert_eq!(bg, [250, 240, 200]);
        assert_eq!(sample_colours(&image::RgbaImage::new(0, 0), GRect::new(0.0, 0.0, 1.0, 1.0)).1, [0, 0, 0]);
    }

    #[test]
    fn rotation_turns_clockwise_on_screen() {
        let v = rotate(vec2(1.0, 0.0), std::f32::consts::FRAC_PI_2);
        assert!((v.x).abs() < 1e-6 && (v.y - 1.0).abs() < 1e-6, "x axis turns down: {v:?}");
    }
}
