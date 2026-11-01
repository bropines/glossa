//! The frame every Glossa window shares: a title bar drawn in the theme's
//! colours, caption buttons, and edges to resize from.
//!
//! With *window style* on (`appearance.custom_frame`, the default) windows
//! are created undecorated and draw this frame themselves, so the options,
//! dictionary and history windows look like the main one instead of
//! wearing a system caption that ignores the theme. macOS keeps its system
//! decorations: its traffic lights are what users expect there.
//!
//! The title bar follows Microsoft's guidance for Windows 11 apps
//! (learn.microsoft.com/windows/apps/design/basics/titlebar-design): 32 px
//! tall, the icon at 16 px from the left edge with the title 16 px after
//! it, 46 x 32 caption buttons with 10 px glyphs, subtle fills on hover and
//! press, the close button red (#C42B1C) with a white glyph, and the title
//! and glyphs dimmed while the window is inactive. Classic themes that give
//! the caption buttons plates (`[window_button.*] back`) keep them.
//!
//! What the system frame does that this one does not: the snap layouts
//! flyout on hovering maximise (it needs the window to answer
//! `WM_NCHITTEST` with `HTMAXBUTTON`, which winit does not let the app do),
//! and the window menu on Alt+Space.
//!
//! Usage, in a viewport's closure before anything else is laid out:
//!
//! ```ignore
//! crate::chrome::frame(ui, &app.look, &Caption::new(&title));
//! egui::CentralPanel::default().show(ui, |ui| { ... });
//! ```
//!
//! and [`builder`] for the viewport, so the window is undecorated (and as
//! transparent as the main window) exactly when the frame is drawn.

use egui::{pos2, vec2, Color32, CursorIcon, Id, LayerId, Order, Rect, Sense, Stroke, Ui, ViewportCommand};
use glossa_core::theme::WidgetState;

use crate::app::GlossaApp;
use crate::style::{c, Look};

/// Viewport names of the tool windows, for [`viewport`] and [`raise`].
pub const OPTIONS: &str = "glossa-options";
pub const DICTIONARY: &str = "glossa-dictionary";
pub const HISTORY: &str = "glossa-history";
pub const KEYBOARD: &str = "glossa-keyboard";
pub const ABOUT: &str = "glossa-about";

/// The title bar's height, in points: WinUI's standard title bar.
pub const TITLE_HEIGHT: f32 = 32.0;
/// A caption button's width; it is as tall as the title bar.
const BUTTON_WIDTH: f32 = 46.0;
/// The close button's fill on hover (WinUI's `CloseButtonBackgroundPointerOver`).
const CLOSE_RED: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);
/// How far in from the window's edges a drag resizes it.
const EDGE: f32 = 6.0;
/// Corners grab a little more, as the system frame's do.
const CORNER: f32 = 14.0;

/// The viewport id of a tool window, by its name.
pub fn viewport(name: &str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(name)
}

/// Whether windows draw their own frame: *window style* is on, and this is
/// not macOS.
pub fn custom(cfg: &glossa_core::Config) -> bool {
    cfg.appearance.custom_frame && !cfg!(target_os = "macos")
}

/// The start of a tool window's viewport: its title and icon, undecorated
/// when the frame is drawn by [`frame`], and transparent when the main
/// window is (translucency and backdrops then reach every window).
pub fn builder(app: &GlossaApp, title: &str) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_icon(crate::app_icon(32))
        .with_decorations(!custom(&app.cfg))
        .with_transparent(app.transparent_root())
}

/// Bring an open window to the front: show it, un-minimise it and give it
/// the keyboard focus. Opening a window that is already open does this.
pub fn raise(ctx: &egui::Context, id: egui::ViewportId) {
    ctx.send_viewport_cmd_to(id, ViewportCommand::Visible(true));
    ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
    ctx.request_repaint_of(id);
}

/// What a window's title bar shows and allows.
pub struct Caption<'a> {
    pub title: &'a str,
    pub minimize: bool,
    pub maximize: bool,
    pub resizable: bool,
}

impl<'a> Caption<'a> {
    /// A resizable window with all three caption buttons.
    pub fn new(title: &'a str) -> Self {
        Caption { title, minimize: true, maximize: true, resizable: true }
    }
}

/// Draw the frame: the title bar along the top (as a panel, so whatever is
/// laid out next gets the rest of the window), the border, and the resize
/// edges. Does nothing when the system draws the frame ([`custom`] is
/// false), so a window can call it unconditionally.
///
/// The close button asks the window to close the way the system's does
/// (`ViewportCommand::Close`), so the window's usual `close_requested`
/// handling applies.
pub fn frame(ui: &mut Ui, look: &Look, custom: bool, caption: &Caption) {
    if !custom {
        return;
    }
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    title_bar(ui, look, caption, maximized);
    if maximized {
        return;
    }
    // Windows 11 rounds the corners and draws the border itself (in the
    // theme's colour; see `glossa_platform::backdrop::style_frame`); a
    // square outline under rounded corners would be cut at each one.
    if !glossa_platform::backdrop::frame_drawn_by_system() {
        let rect = ui.ctx().content_rect();
        let stroke = Stroke::new(look.theme.metrics.border_width.max(1.0), look.border());
        ui.ctx().layer_painter(LayerId::new(Order::Foreground, Id::unique("chrome-border"))).rect_stroke(
            rect,
            0.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    }
    if caption.resizable {
        resize_edges(ui, "chrome");
    }
}

/// The title bar alone: what [`frame`] draws at the top.
fn title_bar(ui: &mut Ui, look: &Look, caption: &Caption, maximized: bool) {
    let t = &look.theme;
    let active = ui.input(|i| i.viewport().focused.unwrap_or(true));
    let fill = t.window.caption.map(c).unwrap_or(c(t.window.back)).gamma_multiply(look.window_alpha);
    let caption_back = t.window.caption.unwrap_or(t.window.back);
    let mut text = c(t.window.caption_text.unwrap_or(t.window.text));
    if !active {
        // WinUI dims the title of an inactive window to its disabled text.
        text = c(t.window.caption_text.unwrap_or(t.window.text).lerp(caption_back, 0.45));
    }
    egui::Panel::top(Id::unique("chrome-title"))
        .exact_size(TITLE_HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(fill))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let drag = ui.interact(rect, Id::unique("chrome-title-drag"), Sense::click_and_drag());
            if drag.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() && caption.maximize {
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }

            // The app's own icon, 16 points square like WinUI's, drawn from
            // an image made at the screen's pixel size so it stays sharp.
            let ppp = ui.ctx().pixels_per_point();
            let snap = |v: f32| (v * ppp).round() / ppp;
            let icon = Rect::from_min_size(
                pos2(snap(rect.left() + 16.0), snap(rect.center().y - 8.0)),
                vec2(snap(16.0), snap(16.0)),
            );
            let texture = app_icon_texture(ui.ctx(), (16.0 * ppp).round() as u32);
            ui.painter().image(texture.id(), icon, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

            // Caption buttons, right to left.
            let mut buttons = vec![Button::Close];
            if caption.maximize {
                buttons.push(if maximized { Button::Restore } else { Button::Maximize });
            }
            if caption.minimize {
                buttons.push(Button::Minimize);
            }
            let mut x = rect.right();
            for b in &buttons {
                x -= BUTTON_WIDTH;
                let r = Rect::from_min_size(pos2(x, rect.top()), vec2(BUTTON_WIDTH, TITLE_HEIGHT));
                if caption_button(ui, look, r, *b, active).clicked() {
                    ui.ctx().send_viewport_cmd(match b {
                        Button::Close => ViewportCommand::Close,
                        Button::Maximize => ViewportCommand::Maximized(true),
                        Button::Restore => ViewportCommand::Maximized(false),
                        Button::Minimize => ViewportCommand::Minimized(true),
                    });
                }
            }

            // The title, cut short before the buttons.
            let left = icon.right() + 16.0;
            let font = egui::FontId::proportional(t.fonts.size * 1.14);
            let galley = ui.painter().layout(caption.title.to_owned(), font, text, f32::INFINITY);
            let clip = Rect::from_min_max(pos2(left, rect.top()), pos2(x - 8.0, rect.bottom()));
            ui.painter().with_clip_rect(clip).galley(pos2(left, rect.center().y - galley.size().y / 2.0), galley, text);
        });
}

/// The app icon as a texture `px` pixels square, made once per size.
fn app_icon_texture(ctx: &egui::Context, px: u32) -> egui::TextureHandle {
    let px = px.clamp(8, 256);
    let id = Id::unique(("chrome-app-icon", px));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let icon = crate::app_icon(px);
    let image = egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &icon.rgba);
    let t = ctx.load_texture(format!("app-icon-{px}"), image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Button {
    Close,
    Maximize,
    Restore,
    Minimize,
}

fn caption_button(ui: &mut Ui, look: &Look, r: Rect, b: Button, active: bool) -> egui::Response {
    let t = &look.theme;
    let resp = ui.interact(r, Id::unique(("chrome-caption", b as u8)), Sense::click());
    let state = if resp.is_pointer_button_down_on() {
        WidgetState::Pressed
    } else if resp.hovered() {
        WidgetState::Hover
    } else {
        WidgetState::Normal
    };
    let style = t.window_button.get(state);
    let caption_back = t.window.caption.unwrap_or(t.window.back);
    let (plate, glyph) = match (b, state) {
        (Button::Close, WidgetState::Hover) => (Some(CLOSE_RED), Color32::WHITE),
        (Button::Close, WidgetState::Pressed) => (Some(CLOSE_RED.gamma_multiply(0.9)), Color32::from_white_alpha(179)),
        (_, WidgetState::Normal) => (style.back.map(c), c(style.main)),
        // The theme's plate if it has one, else WinUI's subtle fills:
        // `SubtleFillColorSecondary` on hover, `Tertiary` when pressed.
        (_, s) => {
            let subtle = match (t.dark, s == WidgetState::Pressed) {
                (true, false) => Color32::from_white_alpha(0x0f),
                (true, true) => Color32::from_white_alpha(0x0a),
                (false, false) => Color32::from_black_alpha(0x09),
                (false, true) => Color32::from_black_alpha(0x06),
            };
            (Some(style.back.map(c).unwrap_or(subtle)), c(style.main))
        }
    };
    if let Some(p) = plate {
        ui.painter().rect_filled(r, 0.0, p);
    }
    let glyph = if active || state != WidgetState::Normal {
        glyph
    } else {
        c(t.window_button.normal.main.lerp(caption_back, 0.45))
    };
    paint_glyph(ui.painter(), r.center(), b, glyph);
    resp
}

/// The caption glyphs, 10 points square and drawn with hairlines snapped to
/// physical pixels, like Segoe Fluent Icons' at that size.
fn paint_glyph(painter: &egui::Painter, center: egui::Pos2, b: Button, color: Color32) {
    let ppp = painter.ctx().pixels_per_point();
    let px = 1.0 / ppp;
    let snap = |v: f32| (v * ppp).round() / ppp + px / 2.0;
    let stroke = Stroke::new(px, color);
    let (cx, cy) = (snap(center.x), snap(center.y));
    let h = 5.0;
    match b {
        Button::Minimize => {
            painter.line_segment([pos2(cx - h, cy), pos2(cx + h, cy)], stroke);
        }
        Button::Maximize => {
            let r = Rect::from_min_max(pos2(cx - h, cy - h), pos2(cx + h, cy + h));
            painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Middle);
        }
        Button::Restore => {
            let front = Rect::from_min_max(pos2(cx - h, cy - h + 2.0), pos2(cx + h - 2.0, cy + h));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
            let back = [pos2(cx - h + 2.0, cy - h + 2.0), pos2(cx - h + 2.0, cy - h), pos2(cx + h, cy - h)];
            painter.line_segment([back[0], back[1]], stroke);
            painter.line_segment([back[1], back[2]], stroke);
            painter.line_segment([back[2], pos2(cx + h, cy + h - 2.0)], stroke);
            painter.line_segment([pos2(cx + h, cy + h - 2.0), pos2(cx + h - 2.0, cy + h - 2.0)], stroke);
        }
        Button::Close => {
            let s = Stroke::new(px * 1.2, color);
            painter.line_segment([pos2(cx - h, cy - h), pos2(cx + h, cy + h)], s);
            painter.line_segment([pos2(cx + h, cy - h), pos2(cx - h, cy + h)], s);
        }
    }
}

/// Edges and corners that resize an undecorated window when dragged. They
/// sit on a foreground layer, so they win over whatever the window draws
/// near its border. Returns whether a resize started this frame.
pub fn resize_edges(ui: &mut Ui, salt: &str) -> bool {
    let s = ui.ctx().content_rect();
    let strips = [
        Rect::from_min_max(s.left_top(), pos2(s.right(), s.top() + EDGE)),
        Rect::from_min_max(pos2(s.left(), s.bottom() - EDGE), s.right_bottom()),
        Rect::from_min_max(s.left_top(), pos2(s.left() + EDGE, s.bottom())),
        Rect::from_min_max(pos2(s.right() - EDGE, s.top()), s.right_bottom()),
    ];
    let layer = LayerId::new(Order::Foreground, Id::unique(("resize-edges", salt)));
    let edges = ui.new_child(egui::UiBuilder::new().layer_id(layer).max_rect(s));
    let mut started = false;
    for (i, rect) in strips.into_iter().enumerate() {
        let r = edges.interact(rect, Id::unique(("resize-edge", salt, i)), Sense::drag());
        let at = r.interact_pointer_pos().or(r.hover_pos());
        let Some(dir) = at.and_then(|p| resize_direction(s, p)) else { continue };
        if r.hovered() || r.dragged() {
            edges.ctx().set_cursor_icon(resize_cursor(dir));
        }
        if r.drag_started() {
            edges.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
            started = true;
        }
    }
    started
}

/// Which way a drag at `p` resizes a window filling `s`: within [`EDGE`] of
/// a side, and diagonally within [`CORNER`] of a corner along either side.
fn resize_direction(s: Rect, p: egui::Pos2) -> Option<egui::viewport::ResizeDirection> {
    use egui::viewport::ResizeDirection as R;
    let (left, right) = (p.x < s.left() + EDGE, p.x > s.right() - EDGE);
    let (top, bottom) = (p.y < s.top() + EDGE, p.y > s.bottom() - EDGE);
    if !(left || right || top || bottom) {
        return None;
    }
    let west = p.x < s.left() + CORNER;
    let east = p.x > s.right() - CORNER;
    let north = p.y < s.top() + CORNER;
    let south = p.y > s.bottom() - CORNER;
    Some(match () {
        _ if (top && west) || (left && north) => R::NorthWest,
        _ if (top && east) || (right && north) => R::NorthEast,
        _ if (bottom && west) || (left && south) => R::SouthWest,
        _ if (bottom && east) || (right && south) => R::SouthEast,
        _ if top => R::North,
        _ if bottom => R::South,
        _ if left => R::West,
        _ => R::East,
    })
}

fn resize_cursor(dir: egui::viewport::ResizeDirection) -> CursorIcon {
    use egui::viewport::ResizeDirection as R;
    match dir {
        R::North | R::South => CursorIcon::ResizeVertical,
        R::East | R::West => CursorIcon::ResizeHorizontal,
        R::NorthWest | R::SouthEast => CursorIcon::ResizeNwSe,
        R::NorthEast | R::SouthWest => CursorIcon::ResizeNeSw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::viewport::ResizeDirection as R;

    #[test]
    fn edges_and_corners_resize_the_right_way() {
        let s = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        assert_eq!(resize_direction(s, pos2(200.0, 150.0)), None);
        assert_eq!(resize_direction(s, pos2(200.0, 2.0)), Some(R::North));
        assert_eq!(resize_direction(s, pos2(398.0, 150.0)), Some(R::East));
        assert_eq!(resize_direction(s, pos2(200.0, 299.0)), Some(R::South));
        assert_eq!(resize_direction(s, pos2(1.0, 150.0)), Some(R::West));
        // A corner reaches along both sides, not just its square.
        assert_eq!(resize_direction(s, pos2(10.0, 2.0)), Some(R::NorthWest));
        assert_eq!(resize_direction(s, pos2(2.0, 10.0)), Some(R::NorthWest));
        assert_eq!(resize_direction(s, pos2(390.0, 1.0)), Some(R::NorthEast));
        assert_eq!(resize_direction(s, pos2(399.0, 295.0)), Some(R::SouthEast));
        assert_eq!(resize_direction(s, pos2(3.0, 292.0)), Some(R::SouthWest));
        // Inside the corner square but away from both sides: not an edge.
        assert_eq!(resize_direction(s, pos2(10.0, 10.0)), None);
    }

    #[test]
    fn macos_keeps_system_decorations() {
        let mut cfg = glossa_core::Config::default();
        cfg.appearance.custom_frame = true;
        assert_eq!(custom(&cfg), !cfg!(target_os = "macos"));
        cfg.appearance.custom_frame = false;
        assert!(!custom(&cfg));
    }
}
