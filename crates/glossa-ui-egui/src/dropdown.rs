//! Dropdown lists and menus in windows of their own.
//!
//! egui draws its popups inside the window they belong to, so a short main
//! window clips a language list, or egui opens it off to the side. These
//! open in a borderless, always-on-top window placed in screen coordinates
//! under the control (above it only when its monitor has no room below),
//! aligned with its left edge and at least as wide, like a native combo box:
//! [`show`] for any content, [`menu`] for a button's menu, [`context_menu`]
//! for a right click.
//!
//! The list's window does not take the focus (on Windows it is made
//! non-activating), so the window it belongs to keeps an active title bar
//! and gets the focus back with nothing in between. Keys pressed there are
//! passed on to the list, so its search box and arrow keys work. It closes
//! on Escape, on a click anywhere else, when the program loses the focus,
//! or when its content calls `ui.close()`.
//!
//! It opens hidden for a frame or two, to measure its content and take its
//! size before it shows. Where windows cannot be placed (Wayland gives no
//! window positions), an ordinary in-window popup stands in.

use egui::{pos2, vec2, Context, Event, Id, Key, Pos2, Rect, Response, Ui, Vec2};
use glossa_platform::window;

/// The frame's inner margin plus its one-point border, on each side.
const MARGIN: i8 = 4;
const EDGE: f32 = MARGIN as f32 + 1.0;
/// Longest a menu gets before it scrolls.
const MENU_MAX_HEIGHT: f32 = 640.0;
/// Longest a [`ComboBox`]'s list gets before it scrolls, unless it says.
const COMBO_MAX_HEIGHT: f32 = 400.0;

/// An open dropdown, kept in egui's memory under its id.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    /// Frames since it opened.
    age: u32,
    /// The content's natural size in points, measured last frame.
    content: Option<Vec2>,
    /// The window is on screen. It starts hidden, until measured and sized.
    shown: bool,
    /// The list's window had the focus last frame (where the platform gave
    /// it the focus despite asking it not to).
    focused: bool,
    /// The fallback popup's rectangle last frame, in the parent's points.
    area: Option<Rect>,
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Option<isize>,
    /// Frames in a row the window has not had the size asked for.
    resizing: u8,
    /// Opened for a window that does not keep the keyboard focus (the
    /// selection popup): the focus moving says nothing about it.
    ignore_focus: bool,
    /// The pointer was over the list last frame.
    hovered: bool,
}

fn state(ctx: &Context, id: Id) -> Option<State> {
    ctx.data(|d| d.get_temp::<State>(id))
}

pub fn is_open(ctx: &Context, id: Id) -> bool {
    state(ctx, id).is_some()
}

/// Whether the dropdown `id` is open with the pointer over it.
pub fn is_hovered(ctx: &Context, id: Id) -> bool {
    state(ctx, id).is_some_and(|s| s.hovered)
}

/// Where the pass number of the last pass that showed any dropdown is kept.
fn any_open_id() -> Id {
    Id::unique("glossa-dropdown-any-open")
}

/// Whether a dropdown was showing in this pass or the one before: a window
/// that closes on Escape asks this first, since the Escape that closes a
/// list reaches the window it belongs to as well.
pub fn recently_open(ctx: &Context) -> bool {
    let last: Option<u64> = ctx.data(|d| d.get_temp(any_open_id()));
    last.is_some_and(|p| p + 1 >= ctx.cumulative_pass_nr())
}

/// Open the dropdown `id` (again, from scratch, if it is open). The window
/// it belongs to gives up its keyboard focus: what is typed now goes to the
/// list.
pub fn open(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(id, State::default()));
    ctx.memory_mut(|m| {
        if let Some(f) = m.focused() {
            m.surrender_focus(f);
        }
    });
    ctx.request_repaint();
}

pub fn close(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.remove::<State>(id));
}

pub fn toggle(ctx: &Context, id: Id) {
    if is_open(ctx, id) {
        close(ctx, id);
    } else {
        open(ctx, id);
    }
}

/// [`toggle`] for a window that does not keep the keyboard focus: the list
/// stays open whatever has the focus, until a pick, Escape, a click elsewhere
/// in that window, or the window going away.
pub fn toggle_unfocused(ctx: &Context, id: Id) {
    if is_open(ctx, id) {
        close(ctx, id);
    } else {
        ctx.data_mut(|d| d.insert_temp(id, State { ignore_focus: true, ..State::default() }));
        ctx.request_repaint();
    }
}

/// A button's menu: opens under the button when it is clicked, closes when
/// clicked again.
pub fn menu<R>(ui: &Ui, button: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = button.id.with("dropdown-menu");
    if button.clicked() {
        toggle(ui.ctx(), id);
    }
    show(ui, id, button.rect, button.rect.width(), MENU_MAX_HEIGHT, |ui, _| add(ui))
}

/// A context menu for `target`: opens at the pointer on a right click.
pub fn context_menu<R>(ui: &Ui, target: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = target.id.with("dropdown-context");
    let at_id = id.with("at");
    if target.secondary_clicked() {
        let at = ui.ctx().input(|i| i.pointer.interact_pos()).unwrap_or(target.rect.center());
        ui.ctx().data_mut(|d| d.insert_temp(at_id, at));
        open(ui.ctx(), id);
    }
    let at: Pos2 = ui.ctx().data(|d| d.get_temp(at_id))?;
    show(ui, id, Rect::from_min_size(at, Vec2::ZERO), 0.0, MENU_MAX_HEIGHT, |ui, _| add(ui))
}

/// Show the dropdown `id`, if it is open, for the control at `anchor` (in
/// the current window's points): at least `min_width` wide (wider when the
/// content is), at most `max_height` tall before it scrolls.
///
/// `add` draws the content in a top-down, full-width layout; its second
/// argument is true while the dropdown is still settling (opening, before
/// it shows), the time to scroll the current item into view. Calling
/// `ui.close()` in it closes the dropdown.
pub fn show<R>(
    ui: &Ui,
    id: Id,
    anchor: Rect,
    min_width: f32,
    max_height: f32,
    add: impl FnOnce(&mut Ui, bool) -> R,
) -> Option<R> {
    let ctx = ui.ctx().clone();
    let mut st = state(&ctx, id)?;
    st.age = st.age.saturating_add(1);
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(any_open_id(), pass));

    // A click anywhere else in the window it belongs to, or the program
    // losing the focus, closes it.
    // (From the events: a press and release in one frame leave no press
    // origin behind.)
    let (presses, parent_focused, inner, ppp) = ctx.input(|i| {
        let presses: Vec<Pos2> = i
            .events
            .iter()
            .filter_map(|e| match e {
                Event::PointerButton { pos, pressed: true, .. } => Some(*pos),
                _ => None,
            })
            .collect();
        (presses, i.focused, i.viewport().inner_rect, i.pixels_per_point())
    });
    let inside = |p: &Pos2| anchor.contains(*p) || st.area.is_some_and(|a| a.contains(*p));
    let lost_focus = st.age > 3 && !st.ignore_focus && !parent_focused && !st.focused;
    if presses.iter().any(|p| !inside(p)) || lost_focus {
        tracing::trace!(?presses, lost_focus, "dropdown closed from outside");
        close(&ctx, id);
        return None;
    }

    let monitors = window::monitors();
    let windowed = inner.is_some() && !monitors.is_empty();
    let (result, close_now) = if windowed {
        show_window(&ctx, id, &mut st, anchor, inner.unwrap_or(Rect::ZERO), ppp, &monitors, min_width, max_height, add)
    } else {
        show_area(&ctx, id, &mut st, anchor, min_width, max_height, add)
    };
    if close_now {
        tracing::trace!("dropdown closed from inside");
        close(&ctx, id);
    } else {
        ctx.data_mut(|d| d.insert_temp(id, st));
        if !st.shown {
            ctx.request_repaint();
        }
    }
    result
}

#[allow(clippy::too_many_arguments)] // the geometry of the anchor and window
fn show_window<R>(
    ctx: &Context,
    id: Id,
    st: &mut State,
    anchor: Rect,
    inner: Rect,
    ppp: f32,
    monitors: &[window::Monitor],
    min_width: f32,
    max_height: f32,
    add: impl FnOnce(&mut Ui, bool) -> R,
) -> (Option<R>, bool) {
    // The anchor on screen, in physical pixels.
    let to_px = |p: Pos2| (inner.min + p.to_vec2()) * ppp;
    let (a0, a1) = (to_px(anchor.min), to_px(anchor.max));
    let anchor_px = glossa_core::ocr::Rect::new(a0.x, a0.y, a1.x - a0.x, a1.y - a0.y);
    // The list's window lives on the anchor's monitor, at that monitor's
    // scale; egui's zoom (the interface scale) applies on top.
    let zoom = ctx.zoom_factor();
    let centre = a0 + (a1 - a0) / 2.0;
    let scale = monitors
        .iter()
        .find(|m| m.contains(centre.x, centre.y))
        .map(|m| m.scale)
        .filter(|s| *s > 0.0)
        .unwrap_or(ppp / zoom.max(0.1));
    let own_ppp = scale * zoom;
    let wanted = match st.content {
        Some(c) => vec2(c.x.max(min_width), c.y.min(max_height)),
        None => vec2(min_width.max(40.0), max_height),
    };
    let place = window::place_dropdown(anchor_px, (wanted.x * own_ppp, wanted.y * own_ppp), monitors);
    let size = vec2(wanted.x, place.height / own_ppp).max(vec2(20.0, 20.0)).round();
    let pos = pos2(place.x / own_ppp, place.y / own_ppp);

    // Several windows of this process may have a list open; each window
    // needs a title of its own so it can be found by it.
    let title = format!("Glossa list {:016x}", id.value());
    let builder = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_active(false)
        .with_visible(st.shown)
        .with_position(pos)
        .with_inner_size(size);
    let viewport = egui::ViewportId::from_hash_of(id.with("viewport"));

    // What the user types goes to the window that has the focus, which is
    // still the one the list belongs to: hand it on.
    let forwarded: Vec<Event> = ctx.input(|i| {
        i.events
            .iter()
            .filter(|e| matches!(e, Event::Key { .. } | Event::Text(_) | Event::Paste(_) | Event::Copy | Event::Cut))
            .cloned()
            .collect()
    });

    let settling = !st.shown;
    let mut result = None;
    let mut close_now = false;
    let mut measured = None;
    let mut actual = None;
    let mut focused = false;
    let mut hovered = false;
    // The viewport callback may in principle run more than once; the content
    // is drawn the first time.
    let (mut add, mut forwarded) = (Some(add), Some(forwarded));
    ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
        focused = ui.input(|i| i.focused);
        hovered = ui.input(|i| i.pointer.has_pointer());
        if let Some(events) = forwarded.take().filter(|e| !focused && !e.is_empty()) {
            ui.ctx().input_mut(|i| i.events.extend(events));
        }
        if ui.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
            close_now = true;
        }
        actual = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        let Some(add) = add.take() else { return };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (r, content, closing) = body(ui, true, max_height, settling, add);
            result = Some(r);
            measured = Some(content);
            close_now |= closing;
        });
    });
    st.focused = focused;
    st.hovered = hovered;

    if let Some(c) = measured {
        let c = (c + Vec2::splat(2.0 * EDGE)).ceil();
        // The width only grows while it is open (a search narrowing the
        // list should not make it jump); the height follows the content.
        let w = st.content.map_or(c.x, |old| old.x.max(c.x));
        let new = vec2(w, c.y);
        // Content that changed size without any input (a list filled in by
        // a worker) needs another frame for the window to follow.
        if st.content.is_some_and(|old| (old - new).abs().max_elem() > 0.5) {
            ctx.request_repaint();
        }
        st.content = Some(new);
    }
    // A window being resized has to be drawn again at its new size, input
    // or not; a few frames, in case the system will not give it that size.
    let sized = actual.is_some_and(|a| (a - size).abs().max_elem() < 1.5);
    st.resizing = if sized { 0 } else { st.resizing.saturating_add(1) };
    if st.shown && (1..=10).contains(&st.resizing) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    if !st.shown {
        let settled = sized && st.age >= 2;
        if settled || st.age > 8 {
            st.shown = true;
        }
    }
    keep_from_activating(st, &title);
    (result, close_now)
}

/// On Windows, make the list's window one that clicks do not activate.
#[cfg(windows)]
fn keep_from_activating(st: &mut State, title: &str) {
    if st.hwnd.is_none() {
        st.hwnd = window::own_window_by_title(title);
    }
    if let Some(hwnd) = st.hwnd {
        window::keep_from_activating(hwnd);
    }
}

#[cfg(not(windows))]
fn keep_from_activating(_: &mut State, _: &str) {}

/// The fallback: an ordinary popup inside the window.
fn show_area<R>(
    ctx: &Context,
    id: Id,
    st: &mut State,
    anchor: Rect,
    min_width: f32,
    max_height: f32,
    add: impl FnOnce(&mut Ui, bool) -> R,
) -> (Option<R>, bool) {
    let settling = st.age <= 1;
    let mut out = None;
    let shown = egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom())
        .constrain(true)
        .show(ctx, |ui| {
            ui.set_min_width((min_width - 2.0 * EDGE).max(0.0));
            out = Some(body(ui, false, max_height, settling, add));
        });
    st.area = Some(shown.response.rect);
    st.shown = true;
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    match out {
        Some((r, _, closing)) => (Some(r), closing || escape),
        None => (None, escape),
    }
}

/// The frame, the scrolling list and the content. Returns the content's
/// result, its natural size, and whether it asked to close.
fn body<R>(
    ui: &mut Ui,
    fill_window: bool,
    max_height: f32,
    settling: bool,
    add: impl FnOnce(&mut Ui, bool) -> R,
) -> (R, Vec2, bool) {
    let visuals = ui.visuals().clone();
    let frame = egui::Frame::new()
        .fill(visuals.window_fill)
        .stroke(egui::Stroke::new(1.0, visuals.window_stroke.color))
        .inner_margin(egui::Margin::same(MARGIN))
        .corner_radius(if fill_window { egui::CornerRadius::ZERO } else { visuals.menu_corner_radius });
    let frame = if fill_window { frame } else { frame.shadow(visuals.popup_shadow) };
    frame
        .show(ui, |ui| {
            if fill_window {
                // The window is the list: no gaps for the clear colour to
                // show through.
                ui.set_min_size(ui.available_size());
            }
            // Flat buttons, as in egui's own menus; the content measured at
            // its natural width, without wrapping.
            egui::containers::menu::menu_style(ui.style_mut());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.scope_builder(egui::UiBuilder::new().closable(), |ui| {
                let scroll = egui::ScrollArea::vertical()
                    .max_height(if fill_window { f32::INFINITY } else { max_height })
                    .auto_shrink([!fill_window, !fill_window])
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| add(ui, settling)).inner
                    });
                (scroll.inner, scroll.content_size, ui.should_close())
            })
            .inner
        })
        .inner
}

/// A button that looks like egui's combo box and opens a dropdown: the
/// text on the left (cut short when it does not fit), a down arrow on the
/// right.
pub fn combo_button(ui: &mut Ui, text: &str, width: f32, open: bool) -> Response {
    let height = ui.spacing().interact_size.y;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let v = if open { ui.visuals().widgets.open } else { *ui.style().interact(&resp) };
        let painter = ui.painter();
        painter.rect(rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
        let pad = ui.spacing().button_padding.x;
        let icon = ui.spacing().icon_width;
        let text_rect = Rect::from_min_max(
            pos2(rect.left() + pad, rect.top()),
            pos2(rect.right() - pad - icon - 2.0, rect.bottom()),
        );
        let font = egui::TextStyle::Button.resolve(ui.style());
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, v.text_color());
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_rect.width().max(1.0));
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(pos2(text_rect.left(), rect.center().y - galley.size().y / 2.0), galley, v.text_color());
        let arrow = Rect::from_center_size(
            pos2(rect.right() - pad - icon / 2.0, rect.center().y),
            vec2(icon * 0.7, icon * 0.45),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![arrow.left_top(), arrow.right_top(), arrow.center_bottom()],
            v.fg_stroke.color,
            egui::Stroke::NONE,
        ));
    }
    resp
}

/// A stand-in for [`egui::ComboBox`] whose list opens in a window of its
/// own (see the module docs), so a small window does not clip it. The same
/// builder calls; the list closes when one of its items is clicked.
pub struct ComboBox {
    id_salt: egui::IdSalt,
    width: Option<f32>,
    height: f32,
    text: String,
}

impl ComboBox {
    pub fn from_id_salt(salt: impl egui::AsIdSalt) -> Self {
        Self { id_salt: egui::IdSalt::new(salt), width: None, height: COMBO_MAX_HEIGHT, text: String::new() }
    }

    /// The button's width (the list is at least as wide).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// How tall the list gets before it scrolls.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn selected_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Draw the button and, while it is open, the list `add` fills. Returns
    /// the button's response.
    pub fn show_ui(self, ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Response {
        let id = ui.make_persistent_id(("combo", self.id_salt));
        let width = self.width.unwrap_or(ui.spacing().combo_width).min(ui.available_width()).max(40.0);
        let button = combo_button(ui, &self.text, width, is_open(ui.ctx(), id));
        if button.clicked() {
            toggle(ui.ctx(), id);
        }
        show(ui, id, button.rect, button.rect.width(), self.height, |ui, _| {
            add(ui);
            // An item was picked: a click on a widget that only clicks (the
            // scroll bar also drags, and keeps the list open).
            let ctx = ui.ctx().clone();
            let picked = ctx
                .interaction_snapshot(|s| s.clicked)
                .and_then(|clicked| ctx.read_response(clicked))
                .is_some_and(|r| !r.sense.senses_drag());
            if picked {
                ui.close();
            }
        });
        button
    }
}
