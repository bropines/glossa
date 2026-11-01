//! Icons, drawn with the painter.
//!
//! No image files: every icon is a few strokes, so it stays sharp at any
//! scale, takes the theme's icon colour, and costs nothing to ship.

use egui::{pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};
use glossa_core::theme::WidgetState;

use crate::style::Look;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Listen,
    Copy,
    Mic,
    Swap,
    Clear,
    Menu,
    Back,
    Forward,
    Dictionary,
    Replace,
    Pin,
    Close,
    Restore,
    Star,
    StarFilled,
    Search,
    Gear,
    Camera,
    History,
    Keyboard,
    Trash,
    Up,
    Down,
    Open,
    Translate,
    Globe,
    /// A floppy disk: save to a file.
    Save,
    /// A framed landscape: a picture.
    Image,
    /// A picture with lines of text laid over it: translate in place.
    Overlay,
    /// Crop marks: select a region again.
    Crop,
    /// Boxes inside a frame: show where text was found.
    Boxes,
    /// Arrows to opposite corners: make larger.
    Expand,
    /// Arrows to the centre: make smaller.
    Shrink,
}

/// Draw `icon` into `rect`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let u = s / 16.0; // one "unit" of a 16-unit grid
    let w = (1.3 * u).max(1.0);
    let st = Stroke::new(w, color);
    let p = |x: f32, y: f32| pos2(c.x + (x - 8.0) * u, c.y + (y - 8.0) * u);
    let line = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], st);
    };
    let poly = |pts: Vec<Pos2>, closed: bool| {
        if closed {
            painter.add(Shape::closed_line(pts, st));
        } else {
            painter.add(Shape::line(pts, st));
        }
    };
    match icon {
        Icon::Listen => {
            // Headphones, as the classic popup drew them.
            let arc: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
                    p(8.0 + 5.5 * a.cos(), 9.0 + 5.5 * a.sin())
                })
                .collect();
            poly(arc, false);
            painter.rect_filled(Rect::from_min_max(p(2.0, 9.0), p(4.5, 14.0)), 1.0 * u, color);
            painter.rect_filled(Rect::from_min_max(p(11.5, 9.0), p(14.0, 14.0)), 1.0 * u, color);
        }
        Icon::Copy => {
            painter.rect_stroke(Rect::from_min_max(p(5.0, 5.0), p(13.5, 14.0)), 1.0 * u, st, egui::StrokeKind::Middle);
            poly(vec![p(3.0, 11.0), p(3.0, 2.5), p(10.5, 2.5)], false);
        }
        Icon::Mic => {
            painter.rect_stroke(Rect::from_min_max(p(6.0, 2.0), p(10.0, 10.0)), 2.0 * u, st, egui::StrokeKind::Middle);
            let arc: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32 / 10.0);
                    p(8.0 + 4.5 * a.cos(), 8.0 + 4.0 * a.sin())
                })
                .collect();
            poly(arc, false);
            line(p(8.0, 12.0), p(8.0, 14.5));
            line(p(5.5, 14.5), p(10.5, 14.5));
        }
        Icon::Swap => {
            line(p(2.5, 5.5), p(13.0, 5.5));
            poly(vec![p(10.0, 2.5), p(13.0, 5.5), p(10.0, 8.5)], false);
            line(p(13.5, 10.5), p(3.0, 10.5));
            poly(vec![p(6.0, 7.5), p(3.0, 10.5), p(6.0, 13.5)], false);
        }
        Icon::Clear => {
            // A blank page with a folded corner: "new".
            poly(vec![p(3.5, 1.5), p(10.0, 1.5), p(13.0, 4.5), p(13.0, 14.5), p(3.5, 14.5)], true);
            poly(vec![p(10.0, 1.5), p(10.0, 4.5), p(13.0, 4.5)], false);
        }
        Icon::Menu => {
            for y in [3.5, 8.0, 12.5] {
                painter.circle_filled(p(8.0, y), 1.2 * u, color);
            }
        }
        Icon::Back => poly(vec![p(10.5, 3.0), p(5.5, 8.0), p(10.5, 13.0)], false),
        Icon::Forward => poly(vec![p(5.5, 3.0), p(10.5, 8.0), p(5.5, 13.0)], false),
        Icon::Dictionary => {
            painter.rect_stroke(Rect::from_min_max(p(3.0, 1.5), p(13.0, 14.5)), 1.0 * u, st, egui::StrokeKind::Middle);
            line(p(5.5, 1.5), p(5.5, 14.5));
            line(p(7.5, 5.0), p(11.0, 5.0));
            line(p(7.5, 7.5), p(11.0, 7.5));
        }
        Icon::Replace => {
            // "a → b": the classic replace glyph.
            let font = egui::FontId::proportional(7.5 * u);
            painter.text(p(5.0, 5.5), egui::Align2::CENTER_CENTER, "b", font.clone(), color);
            painter.text(p(11.0, 11.0), egui::Align2::CENTER_CENTER, "a", font, color);
            poly(vec![p(3.5, 9.0), p(3.5, 13.0), p(7.0, 13.0)], false);
        }
        Icon::Pin => {
            line(p(8.0, 9.5), p(8.0, 15.0));
            poly(vec![p(5.0, 9.5), p(11.0, 9.5)], false);
            poly(vec![p(6.0, 9.5), p(6.5, 2.0), p(9.5, 2.0), p(10.0, 9.5)], false);
        }
        Icon::Close => {
            line(p(3.5, 3.5), p(12.5, 12.5));
            line(p(12.5, 3.5), p(3.5, 12.5));
        }
        Icon::Restore => {
            painter.rect_stroke(Rect::from_min_max(p(3.0, 5.5), p(10.5, 13.0)), 0.0, st, egui::StrokeKind::Middle);
            poly(vec![p(5.5, 5.5), p(5.5, 3.0), p(13.0, 3.0), p(13.0, 10.5), p(10.5, 10.5)], false);
        }
        Icon::Star | Icon::StarFilled => {
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 6.5 } else { 2.8 };
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    p(8.0 + r * a.cos(), 8.5 + r * a.sin())
                })
                .collect();
            if icon == Icon::StarFilled {
                painter.add(Shape::convex_polygon(pts.clone(), color, Stroke::NONE));
                // A concave star filled as convex would overfill; redraw the
                // outline on top so the shape still reads as a star.
            }
            poly(pts, true);
        }
        Icon::Search => {
            painter.circle_stroke(p(7.0, 7.0), 4.5 * u, st);
            line(p(10.3, 10.3), p(14.0, 14.0));
        }
        Icon::Gear => {
            painter.circle_stroke(p(8.0, 8.0), 2.3 * u, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                line(p(8.0 + 4.0 * a.cos(), 8.0 + 4.0 * a.sin()), p(8.0 + 6.3 * a.cos(), 8.0 + 6.3 * a.sin()));
            }
            painter.circle_stroke(p(8.0, 8.0), 4.2 * u, st);
        }
        Icon::Camera => {
            // A selection frame with a corner mark: "recognise a region".
            for (x, y, dx, dy) in
                [(2.0, 2.0, 1.0, 1.0), (14.0, 2.0, -1.0, 1.0), (2.0, 14.0, 1.0, -1.0), (14.0, 14.0, -1.0, -1.0)]
            {
                poly(vec![p(x, y + 4.0 * dy), p(x, y), p(x + 4.0 * dx, y)], false);
            }
            let font = egui::FontId::proportional(7.0 * u);
            painter.text(p(8.0, 8.0), egui::Align2::CENTER_CENTER, "T", font, color);
        }
        Icon::History => {
            painter.circle_stroke(p(8.0, 8.0), 6.0 * u, st);
            line(p(8.0, 4.5), p(8.0, 8.0));
            line(p(8.0, 8.0), p(10.5, 10.0));
        }
        Icon::Keyboard => {
            painter.rect_stroke(Rect::from_min_max(p(1.5, 4.0), p(14.5, 12.5)), 1.0 * u, st, egui::StrokeKind::Middle);
            for (x, y) in [(4.0, 6.5), (6.5, 6.5), (9.0, 6.5), (11.5, 6.5)] {
                painter.circle_filled(p(x, y), 0.7 * u, color);
            }
            line(p(5.0, 10.0), p(11.0, 10.0));
        }
        Icon::Trash => {
            line(p(2.5, 4.0), p(13.5, 4.0));
            poly(vec![p(6.0, 4.0), p(6.0, 2.0), p(10.0, 2.0), p(10.0, 4.0)], false);
            poly(vec![p(4.0, 4.0), p(5.0, 14.5), p(11.0, 14.5), p(12.0, 4.0)], false);
        }
        Icon::Up => poly(vec![p(3.0, 10.5), p(8.0, 5.5), p(13.0, 10.5)], false),
        Icon::Down => poly(vec![p(3.0, 5.5), p(8.0, 10.5), p(13.0, 5.5)], false),
        Icon::Open => {
            poly(vec![p(9.0, 2.5), p(13.5, 2.5), p(13.5, 7.0)], false);
            line(p(13.5, 2.5), p(7.5, 8.5));
            poly(vec![p(11.5, 9.5), p(11.5, 13.5), p(2.5, 13.5), p(2.5, 4.5), p(6.5, 4.5)], false);
        }
        Icon::Translate => {
            let font = egui::FontId::proportional(7.0 * u);
            painter.text(p(5.0, 6.0), egui::Align2::CENTER_CENTER, "A", font.clone(), color);
            painter.text(p(11.0, 11.0), egui::Align2::CENTER_CENTER, "Я", font, color);
        }
        Icon::Globe => {
            painter.circle_stroke(p(8.0, 8.0), 6.0 * u, st);
            line(p(2.0, 8.0), p(14.0, 8.0));
            let mer: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (i as f32 / 12.0) - std::f32::consts::FRAC_PI_2;
                    p(8.0 + 2.8 * a.cos(), 8.0 + 6.0 * a.sin())
                })
                .collect();
            poly(mer, false);
        }
        Icon::Save => {
            poly(vec![p(2.5, 2.5), p(11.5, 2.5), p(13.5, 4.5), p(13.5, 13.5), p(2.5, 13.5)], true);
            poly(vec![p(5.0, 2.5), p(5.0, 6.0), p(10.5, 6.0), p(10.5, 2.5)], false);
            painter.rect_stroke(Rect::from_min_max(p(5.0, 9.0), p(11.0, 13.5)), 0.5 * u, st, egui::StrokeKind::Middle);
        }
        Icon::Image => {
            painter.rect_stroke(Rect::from_min_max(p(1.5, 2.5), p(14.5, 13.5)), 1.0 * u, st, egui::StrokeKind::Middle);
            poly(vec![p(2.0, 12.5), p(6.0, 7.5), p(9.0, 10.5), p(10.5, 9.0), p(14.0, 12.5)], false);
            painter.circle_stroke(p(10.8, 5.6), 1.3 * u, st);
        }
        Icon::Overlay => {
            painter.rect_stroke(Rect::from_min_max(p(1.5, 2.5), p(14.5, 13.5)), 1.0 * u, st, egui::StrokeKind::Middle);
            painter.rect_filled(Rect::from_min_max(p(4.0, 5.0), p(12.0, 7.0)), 0.5 * u, color);
            painter.rect_filled(Rect::from_min_max(p(4.0, 9.0), p(9.5, 11.0)), 0.5 * u, color);
        }
        Icon::Crop => {
            poly(vec![p(4.5, 1.5), p(4.5, 11.5), p(14.5, 11.5)], false);
            poly(vec![p(1.5, 4.5), p(11.5, 4.5), p(11.5, 14.5)], false);
        }
        Icon::Boxes => {
            painter.rect_stroke(Rect::from_min_max(p(1.5, 1.5), p(14.5, 14.5)), 1.0 * u, st, egui::StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(4.0, 4.0), p(12.0, 7.0)), 0.0, st, egui::StrokeKind::Middle);
            painter.rect_stroke(Rect::from_min_max(p(4.0, 9.0), p(9.5, 12.0)), 0.0, st, egui::StrokeKind::Middle);
        }
        Icon::Expand => {
            line(p(9.5, 6.5), p(13.5, 2.5));
            poly(vec![p(9.5, 2.5), p(13.5, 2.5), p(13.5, 6.5)], false);
            line(p(6.5, 9.5), p(2.5, 13.5));
            poly(vec![p(2.5, 9.5), p(2.5, 13.5), p(6.5, 13.5)], false);
        }
        Icon::Shrink => {
            line(p(9.5, 6.5), p(13.5, 2.5));
            poly(vec![p(9.5, 2.5), p(9.5, 6.5), p(13.5, 6.5)], false);
            line(p(6.5, 9.5), p(2.5, 13.5));
            poly(vec![p(2.5, 9.5), p(6.5, 9.5), p(6.5, 13.5)], false);
        }
    }
}

/// The plate under a flat icon button the pointer is on: the theme's caption
/// button plate when it has one, else a faint fill of the border colour.
fn hover_plate(look: &Look, pressed: bool) -> Color32 {
    let plate =
        look.theme.window_button.get(WidgetState::Hover).back.map(crate::style::c).unwrap_or_else(|| look.border());
    plate.gamma_multiply(if pressed { 0.85 } else { 0.6 })
}

/// A flat command button: an icon and a label beside it, a plate under the
/// pointer, as in a WinUI command bar. An empty `label` makes it an icon
/// button of the same height. `tooltip` (with the shortcut, say) shows on
/// hover.
pub fn command_button(ui: &mut Ui, look: &Look, icon: Icon, label: &str, tooltip: &str, enabled: bool) -> Response {
    let icon_size = (16.0 * look.icon_scale).round();
    let pad = 8.0;
    let height = ui.spacing().interact_size.y.max(icon_size + 8.0).round();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER));
    let width = match &galley {
        Some(g) => (pad + icon_size + 6.0 + g.size().x + pad).round(),
        None => height,
    };
    let (rect, mut resp) =
        ui.allocate_exact_size(vec2(width, height), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let state = if !enabled {
            WidgetState::Disabled
        } else if resp.is_pointer_button_down_on() {
            WidgetState::Pressed
        } else if resp.hovered() {
            WidgetState::Hover
        } else {
            WidgetState::Normal
        };
        if matches!(state, WidgetState::Hover | WidgetState::Pressed) {
            let radius = look.theme.metrics.button_radius.max(3.0);
            ui.painter().rect_filled(rect, radius, hover_plate(look, state == WidgetState::Pressed));
        }
        let icon_rect = match &galley {
            Some(_) => Rect::from_center_size(
                pos2(rect.left() + pad + icon_size / 2.0, rect.center().y),
                vec2(icon_size, icon_size),
            ),
            None => Rect::from_center_size(rect.center(), vec2(icon_size, icon_size)),
        };
        paint(ui.painter(), icon_rect, icon, look.icon(state));
        if let Some(g) = galley {
            let colour = if enabled { look.text() } else { look.weak_text() };
            let at = pos2(icon_rect.right() + 6.0, rect.center().y - g.size().y / 2.0);
            ui.painter().galley(at, g, colour);
        }
    }
    if !tooltip.is_empty() {
        resp = resp.on_hover_text(tooltip);
    }
    resp
}

pub fn sized_button(ui: &mut Ui, look: &Look, icon: Icon, tooltip: &str, size: f32, enabled: bool) -> Response {
    let size = (size * look.icon_scale).round();
    let (rect, mut resp) =
        ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let state = if !enabled {
            WidgetState::Disabled
        } else if resp.is_pointer_button_down_on() {
            WidgetState::Pressed
        } else if resp.hovered() {
            WidgetState::Hover
        } else {
            WidgetState::Normal
        };
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, 3.0, hover_plate(look, resp.is_pointer_button_down_on()));
        }
        paint(ui.painter(), rect.shrink(size * 0.17), icon, look.icon(state));
    }
    if !tooltip.is_empty() {
        resp = resp.on_hover_text(tooltip);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon paints without panicking at a range of sizes.
    #[test]
    fn all_icons_paint() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            for icon in [
                Icon::Listen,
                Icon::Copy,
                Icon::Mic,
                Icon::Swap,
                Icon::Clear,
                Icon::Menu,
                Icon::Back,
                Icon::Forward,
                Icon::Dictionary,
                Icon::Replace,
                Icon::Pin,
                Icon::Close,
                Icon::Restore,
                Icon::Star,
                Icon::StarFilled,
                Icon::Search,
                Icon::Gear,
                Icon::Camera,
                Icon::History,
                Icon::Keyboard,
                Icon::Trash,
                Icon::Up,
                Icon::Down,
                Icon::Open,
                Icon::Translate,
                Icon::Globe,
                Icon::Save,
                Icon::Image,
                Icon::Overlay,
                Icon::Crop,
                Icon::Boxes,
                Icon::Expand,
                Icon::Shrink,
            ] {
                for s in [8.0, 16.0, 48.0] {
                    paint(painter, Rect::from_min_size(pos2(0.0, 0.0), vec2(s, s)), icon, Color32::WHITE);
                }
            }
        });
        out.textures_delta.clear();
    }
}
