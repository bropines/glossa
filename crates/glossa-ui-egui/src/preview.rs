//! A small picture of how the appearance settings will look, for the top
//! of Options → Appearance: the main window and a popup drawn with the
//! draft's theme, transparency, backdrop and window style, over a made-up
//! desktop so see-through and blurred windows show as such. It follows the
//! draft, so a change shows before Apply.

use egui::{pos2, vec2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use glossa_core::config::Backdrop;
use glossa_core::theme::WidgetState;
use glossa_platform::shell::SHELL;

use crate::icons::{self, Icon};
use crate::style::{c, Look};

/// Draw the preview for `cfg` (the draft) in a strip `height` points tall.
pub fn show(ui: &mut egui::Ui, look: &Look, cfg: &glossa_core::Config, height: f32, labels: &Labels) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter_at(rect);
    desktop(&p, rect);

    let scale = cfg.appearance.ui_scale.clamp(0.75, 1.5);
    // The main window on the left, the popup over its lower right.
    let main_w = (rect.width() * 0.62).min(420.0 * scale).max(220.0);
    let main_h = (rect.height() - 24.0).min(250.0 * scale);
    let main = Rect::from_min_size(rect.min + vec2(18.0, (rect.height() - main_h) / 2.0), vec2(main_w, main_h));
    main_window(&p, look, cfg, main, scale, labels);

    let pop_w = (main_w * 0.62).max(170.0);
    let pop_h = 86.0 * scale;
    let pop_x = (main.right() - pop_w * 0.35).min(rect.right() - pop_w - 14.0);
    let popup_rect = Rect::from_min_size(pos2(pop_x, main.bottom() - pop_h + 6.0), vec2(pop_w, pop_h));
    popup(&p, look, cfg, popup_rect, scale, labels);
}

/// The texts the preview shows, in the interface language.
pub struct Labels {
    pub source: String,
    pub translation: String,
    pub from: String,
    pub to: String,
    pub translate: String,
    pub popup_title: String,
}

/// A dark desktop with bright diagonal bands, so a see-through window
/// shows them and a blurred one softens them.
fn desktop(p: &egui::Painter, r: Rect) {
    let radius = CornerRadius::same(10);
    p.rect_filled(r, radius, Color32::from_rgb(0x0b, 0x10, 0x24));
    let band = |x0: f32, w: f32, col: Color32| {
        let pts = vec![
            pos2(r.left() + x0 * r.width(), r.bottom()),
            pos2(r.left() + (x0 + w) * r.width(), r.bottom()),
            pos2(r.left() + (x0 + w + 0.35) * r.width(), r.top()),
            pos2(r.left() + (x0 + 0.35) * r.width(), r.top()),
        ];
        p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
    };
    band(0.05, 0.10, Color32::from_rgb(0x1f, 0x4f, 0xc8));
    band(0.22, 0.04, Color32::from_rgb(0x7a, 0xb8, 0xff));
    band(0.48, 0.14, Color32::from_rgb(0x5b, 0x2a, 0xa8));
    band(0.70, 0.05, Color32::from_rgb(0xff, 0x9a, 0x3c));
}

/// What a backdrop does to the desktop behind a window: frosted, tinted
/// light or dark. Nothing without one.
fn frost(p: &egui::Painter, r: Rect, radius: CornerRadius, kind: Backdrop, dark: bool) {
    if !kind.is_some() {
        return;
    }
    let tint = match (kind, dark) {
        (Backdrop::Blur, _) => Color32::from_rgba_unmultiplied(0x30, 0x34, 0x48, 150),
        (Backdrop::Mica | Backdrop::MicaAlt, true) => Color32::from_rgba_unmultiplied(0x24, 0x24, 0x28, 235),
        (Backdrop::Mica | Backdrop::MicaAlt, false) => Color32::from_rgba_unmultiplied(0xe8, 0xe8, 0xec, 235),
        (_, true) => Color32::from_rgba_unmultiplied(0x2a, 0x2a, 0x30, 200),
        (_, false) => Color32::from_rgba_unmultiplied(0xf0, 0xf0, 0xf4, 200),
    };
    p.rect_filled(r, radius, tint);
}

fn main_window(p: &egui::Painter, look: &Look, cfg: &glossa_core::Config, r: Rect, s: f32, labels: &Labels) {
    let t = &look.theme;
    let radius = CornerRadius::same((10.0 * s) as u8);
    // A soft shadow under the window.
    p.rect_filled(r.translate(vec2(0.0, 4.0)).expand(2.0), radius, Color32::from_black_alpha(70));
    frost(p, r, radius, look.window_backdrop, t.dark);
    p.rect(r, radius, look.window_back(), Stroke::new(1.0, c(t.window.border)), StrokeKind::Inside);

    let text = c(t.window.text);
    let weak = look.weak_text();
    let font = |size: f32| FontId::proportional(size * s);
    let pad = 8.0 * s;

    // Title bar: the system's buttons, or Glossa's own caption.
    let bar = Rect::from_min_size(r.min, vec2(r.width(), 22.0 * s));
    let own = cfg.appearance.custom_frame && SHELL.own_frame;
    if own {
        let y = bar.center().y;
        let glyph = Stroke::new(1.0, text);
        let x = bar.right() - 12.0 * s;
        let k = 3.0 * s;
        p.line_segment([pos2(x - k, y - k), pos2(x + k, y + k)], glyph);
        p.line_segment([pos2(x + k, y - k), pos2(x - k, y + k)], glyph);
        p.rect_stroke(
            Rect::from_center_size(pos2(x - 22.0 * s, y), vec2(2.0 * k, 2.0 * k)),
            0.0,
            glyph,
            StrokeKind::Middle,
        );
        p.line_segment([pos2(x - 44.0 * s - k, y), pos2(x - 44.0 * s + k, y)], glyph);
        p.text(pos2(bar.left() + pad, y), egui::Align2::LEFT_CENTER, "Glossa", font(10.0), text);
    } else {
        for (i, v) in [0xff5f57u32, 0xfebc2e, 0x28c840].iter().enumerate() {
            let col = Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, *v as u8);
            p.circle_filled(pos2(bar.left() + (12.0 + i as f32 * 12.0) * s, bar.center().y), 3.5 * s, col);
        }
        p.text(pos2(bar.left() + 52.0 * s, bar.center().y), egui::Align2::LEFT_CENTER, "Glossa", font(10.0), weak);
    }

    // Toolbar icons on the right.
    let icon_y = bar.bottom() + 10.0 * s;
    for (i, icon) in [Icon::Menu, Icon::Gear, Icon::Keyboard, Icon::Dictionary, Icon::History].iter().enumerate() {
        let c0 = pos2(r.right() - pad - 6.0 * s - i as f32 * 18.0 * s, icon_y);
        icons::paint(p, Rect::from_center_size(c0, vec2(11.0 * s, 11.0 * s)), *icon, look.icon(WidgetState::Normal));
    }

    let inner_l = r.left() + pad;
    let inner_r = r.right() - pad;
    let field = |rect: Rect| {
        p.rect(
            rect,
            CornerRadius::same((5.0 * s) as u8),
            look.edit_back(),
            Stroke::new(1.0, c(t.edit.normal.border)),
            StrokeKind::Inside,
        );
    };

    // Source pane, direction bar, translation pane, service tabs.
    let tabs_h = 16.0 * s;
    let dir_h = 20.0 * s;
    let top = icon_y + 10.0 * s;
    let bottom = r.bottom() - pad - tabs_h - 4.0 * s;
    let pane_h = ((bottom - top) - dir_h - 12.0 * s) / 2.0;
    let src = Rect::from_min_max(pos2(inner_l, top), pos2(inner_r, top + pane_h));
    field(src);
    p.text(src.min + vec2(6.0 * s, 5.0 * s), egui::Align2::LEFT_TOP, &labels.source, font(10.5), text);

    let dir = Rect::from_min_size(pos2(inner_l, src.bottom() + 6.0 * s), vec2(inner_r - inner_l, dir_h));
    let button = t.button.get(WidgetState::Normal);
    let combo_w = (dir.width() - 80.0 * s) / 2.0;
    for (i, label) in [&labels.from, &labels.to].iter().enumerate() {
        let x = dir.left() + i as f32 * (combo_w + 8.0 * s);
        let cr = Rect::from_min_size(pos2(x, dir.top()), vec2(combo_w, dir_h));
        p.rect(
            cr,
            CornerRadius::same((5.0 * s) as u8),
            c(button.back()),
            Stroke::new(1.0, c(button.border)),
            StrokeKind::Inside,
        );
        p.text(cr.left_center() + vec2(6.0 * s, 0.0), egui::Align2::LEFT_CENTER, *label, font(9.5), c(button.text));
        let a = cr.right_center() - vec2(8.0 * s, 1.0 * s);
        p.add(Shape::convex_polygon(
            vec![a + vec2(-3.0 * s, -2.0 * s), a + vec2(3.0 * s, -2.0 * s), a + vec2(0.0, 2.0 * s)],
            c(button.text),
            Stroke::NONE,
        ));
    }
    let go = Rect::from_min_max(pos2(dir.right() - 64.0 * s, dir.top()), dir.max);
    let accent = look.accent();
    p.rect_filled(go, CornerRadius::same((5.0 * s) as u8), accent);
    p.text(go.center(), egui::Align2::CENTER_CENTER, &labels.translate, font(9.5), crate::style::text_on(accent));

    let out = Rect::from_min_max(pos2(inner_l, dir.bottom() + 6.0 * s), pos2(inner_r, bottom));
    field(out);
    p.text(out.min + vec2(6.0 * s, 5.0 * s), egui::Align2::LEFT_TOP, &labels.translation, font(10.5), text);

    let tabs_top = r.bottom() - pad - tabs_h;
    let tab_w = ((inner_r - inner_l) - 3.0 * 4.0 * s) / 4.0;
    for (i, (letter, col)) in [("G", 0x4285f4u32), ("M", 0x0078d4), ("Y", 0xb388ff), ("D", 0x4fd1c5)].iter().enumerate()
    {
        let x = inner_l + i as f32 * (tab_w + 4.0 * s);
        let tr = Rect::from_min_size(pos2(x, tabs_top), vec2(tab_w, tabs_h));
        p.rect_filled(tr, CornerRadius::same((4.0 * s) as u8), c(button.back()));
        let dot = Rect::from_min_size(tr.min + vec2(3.0 * s, 3.0 * s), vec2(tabs_h - 6.0 * s, tabs_h - 6.0 * s));
        let colour = Color32::from_rgb((col >> 16) as u8, (col >> 8) as u8, *col as u8);
        p.rect_filled(dot, CornerRadius::same(2), colour);
        p.text(dot.center(), egui::Align2::CENTER_CENTER, *letter, font(7.5), Color32::WHITE);
        if i == 0 {
            p.line_segment([tr.left_bottom(), tr.right_bottom()], Stroke::new(2.0 * s, accent));
        }
    }
}

fn popup(p: &egui::Painter, look: &Look, cfg: &glossa_core::Config, r: Rect, s: f32, labels: &Labels) {
    let t = &look.theme;
    let radius = CornerRadius::same((8.0 * s) as u8);
    let alpha = look.popup_opacity;
    p.rect_filled(r.translate(vec2(0.0, 4.0)).expand(2.0), radius, Color32::from_black_alpha(80));
    frost(p, r, radius, look.popup_backdrop, t.dark);
    let back = look.popup_back.gamma_multiply(alpha);
    let frame = if cfg.appearance.popup.show_border {
        Stroke::new(look.popup_frame_width.max(1.0), look.popup_frame)
    } else {
        Stroke::NONE
    };
    p.rect(r, radius, back, frame, StrokeKind::Inside);
    let pad = 8.0 * s;
    let font = |size: f32| FontId::proportional(size * s);
    let header_y = r.top() + 12.0 * s;
    p.text(pos2(r.left() + pad, header_y), egui::Align2::LEFT_CENTER, &labels.popup_title, font(9.0), look.weak_text());
    for (i, icon) in [Icon::Close, Icon::Pin, Icon::Copy, Icon::Listen].iter().enumerate() {
        let c0 = pos2(r.right() - pad - 4.0 * s - i as f32 * 15.0 * s, header_y);
        icons::paint(p, Rect::from_center_size(c0, vec2(9.0 * s, 9.0 * s)), *icon, look.icon(WidgetState::Normal));
    }
    let line_y = header_y + 10.0 * s;
    p.line_segment(
        [pos2(r.left() + pad, line_y), pos2(r.right() - pad, line_y)],
        Stroke::new(1.0, c(t.window.group_box)),
    );
    let text_pos: Pos2 = pos2(r.left() + pad, line_y + 8.0 * s);
    p.text(text_pos, egui::Align2::LEFT_TOP, &labels.translation, font(11.5), look.popup_text);
}
