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
//! passed on to the list, so its search box and arrow keys work. Where the
//! system gives the list the focus anyway (macOS, X11), that counts as the
//! focus staying put. It closes on Escape, on a click anywhere else in the
//! window it belongs to, when the focus goes to a window that is neither
//! that one nor the list, or when its content calls `ui.close()` (see
//! [`keep_open`]).
//!
//! It opens hidden for a frame or two, to measure its content and take its
//! size before it shows. Where windows cannot be placed (Wayland gives no
//! window positions), an ordinary in-window popup stands in, and on macOS
//! it does whenever the list fits in its window: making a window there
//! takes long enough to see.

use egui::{pos2, vec2, Context, Event, Id, Key, Pos2, Rect, Response, Ui, Vec2};
use glossa_platform::window;

/// The frame's inner margin plus its one-point border, on each side.
const MARGIN: i8 = 4;
const EDGE: f32 = MARGIN as f32 + 1.0;
/// Longest a menu gets before it scrolls.
const MENU_MAX_HEIGHT: f32 = 760.0;
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
    /// What the focus has done since it opened.
    focus: FocusTrack,
    /// The pointer was moved onto the list for it (see [`seed_pointer`]).
    seeded: bool,
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
    /// Drawn inside the owner's window rather than in one of its own,
    /// decided when it opens (see [`in_window`]).
    in_window: Option<bool>,
    /// The in-window list opens above the control (no room below).
    above: bool,
    /// The window's size when it was last put in place (macOS, see
    /// [`show_window`]).
    placed_at: Option<Vec2>,
    /// When it opened, for the log.
    opened: Option<std::time::Instant>,
}

/// What a dropdown saw in one frame, for [`keep_open`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Seen {
    /// A mouse button went down in the window the list belongs to, outside
    /// the control and the list.
    press_outside: bool,
    /// The window the list belongs to has the keyboard focus.
    owner_focused: bool,
    /// The list's own window has it (the system gave it the focus despite
    /// being asked not to, or the user clicked it where clicks activate).
    list_focused: bool,
}

/// What the focus has done since the dropdown opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FocusTrack {
    /// The owner or the list had the focus at some point.
    had_focus: bool,
    /// Frames in a row in which neither had it.
    unfocused_frames: u8,
}

/// Frames in a row neither window may have the focus before the list
/// closes. The focus moving from one window to another arrives as two
/// events (one loses it, the other gains it), which can land in different
/// frames; a single frame with neither focused is a hand-over, not a loss.
const FOCUS_GRACE: u8 = 2;

/// Whether a dropdown stays open after a frame in which it saw `seen`.
/// `ignore_focus` is for a list belonging to a window that never keeps the
/// focus (the selection popup). Pure, so the rules are tested rather than
/// promised:
///
/// - a press elsewhere in the window it belongs to closes it;
/// - the focus moving between that window and the list keeps it;
/// - the focus going anywhere else (another program, another of Glossa's
///   windows) closes it, after [`FOCUS_GRACE`] frames;
/// - a list opened while neither window had the focus (the owner was not
///   in the foreground) stays until one of them gets it and loses it again,
///   rather than closing at once.
fn keep_open(track: &mut FocusTrack, ignore_focus: bool, seen: Seen) -> bool {
    if seen.press_outside {
        return false;
    }
    if ignore_focus {
        return true;
    }
    if seen.owner_focused || seen.list_focused {
        *track = FocusTrack { had_focus: true, unfocused_frames: 0 };
        return true;
    }
    if !track.had_focus {
        return true;
    }
    track.unfocused_frames = track.unfocused_frames.saturating_add(1);
    track.unfocused_frames < FOCUS_GRACE
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

/// The id of [`context_menu`]'s dropdown for `target`.
pub fn context_menu_id(target: &Response) -> Id {
    target.id.with("dropdown-context")
}

/// A context menu for `target`: opens at the pointer on a right click.
pub fn context_menu<R>(ui: &Ui, target: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    context_menu_in(ui, context_menu_id(target), target.secondary_clicked(), target.rect.center(), add)
}

/// A context menu `id` for an area whose parts take the clicks themselves
/// (selectable text, links): `clicked` says the right button was clicked
/// over it, found out by the caller.
pub fn context_menu_in<R>(ui: &Ui, id: Id, clicked: bool, fallback: Pos2, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let at_id = id.with("at");
    if clicked {
        let at = ui.ctx().input(|i| i.pointer.interact_pos()).unwrap_or(fallback);
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

    // A click anywhere else in the window it belongs to closes it, as does
    // the focus leaving for a window that is neither that one nor the list
    // (decided below, once the list has seen this frame's input).
    // (From the events: a press and release in one frame leave no press
    // origin behind.)
    let (presses, owner_focused, inner, ppp) = ctx.input(|i| {
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
    let press_outside = presses.iter().any(|p| !inside(p));

    let monitors = window::monitors();
    let can_window = inner.is_some() && !monitors.is_empty();
    let room = ctx.content_rect();
    let last_size_id = id.with("last-size");
    if st.in_window.is_none() {
        let last: Option<Vec2> = ctx.data(|d| d.get_temp(last_size_id));
        let wanted = last.unwrap_or(vec2(min_width, max_height));
        let (inside, above) = in_window(room, anchor, wanted, max_height < MENU_MAX_HEIGHT);
        st.in_window = Some(!can_window || (cfg!(target_os = "macos") && inside));
        st.above = above;
        st.opened = Some(std::time::Instant::now());
    }
    let (result, close_now, list_focused) = if st.in_window == Some(false) {
        show_window(&ctx, id, &mut st, anchor, inner.unwrap_or(Rect::ZERO), ppp, &monitors, min_width, max_height, add)
    } else {
        let space = if st.above { anchor.min.y - room.min.y } else { room.max.y - anchor.max.y };
        let max_height = max_height.min(space - 2.0 * EDGE - 4.0).max(60.0);
        let (r, c) = show_area(&ctx, id, &mut st, anchor, min_width, max_height, add);
        // In the same window: its focus is the owner's.
        (r, c, false)
    };
    // The focus as both windows saw it in this same frame: the list's
    // window has just run its pass, so a click that moved the focus to it
    // is seen together with the owner losing it.
    if let Some(size) = st.content.or(st.area.map(|a| a.size())) {
        ctx.data_mut(|d| d.insert_temp(last_size_id, size));
    }
    let seen = Seen { press_outside, owner_focused, list_focused };
    let keep = keep_open(&mut st.focus, st.ignore_focus, seen);
    if !keep {
        tracing::trace!(?seen, ?presses, "dropdown closed from outside");
    }
    if close_now || !keep {
        if close_now {
            tracing::trace!("dropdown closed from inside");
        }
        close(&ctx, id);
    } else {
        ctx.data_mut(|d| d.insert_temp(id, st));
        if !st.shown {
            ctx.request_repaint();
        }
    }
    result
}

/// Whether a list of about `wanted` points fits inside the window's `room`
/// at `anchor`, and whether it goes above the control for that (below when
/// there is room). A list that scrolls anyway (`scrolls`) fits wherever it
/// gets a fair height; a menu has to fit whole.
fn in_window(room: Rect, anchor: Rect, wanted: Vec2, scrolls: bool) -> (bool, bool) {
    let below = room.max.y - anchor.max.y;
    let above = anchor.min.y - room.min.y;
    let wide_enough = wanted.x <= room.width();
    if wanted.y <= below {
        (wide_enough, false)
    } else if wanted.y <= above {
        (wide_enough, true)
    } else {
        let best = below.max(above);
        (wide_enough && scrolls && best >= 200.0, above > below)
    }
}

/// A list in a window of its own. On macOS this is the slow path (a window
/// and its GL context made on every open): lists that fit in their window
/// are drawn there instead.
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
) -> (Option<R>, bool, bool) {
    // The anchor on screen, in the desktop's units: physical pixels, except
    // on macOS, whose desktop is laid out in points (a Retina screen's ppp
    // of 2 would put the list twice as far from the corner).
    let zoom = ctx.zoom_factor();
    let desktop = window::desktop_scale(ppp / zoom.max(0.1)) * zoom;
    let to_px = |p: Pos2| (inner.min + p.to_vec2()) * desktop;
    let (a0, a1) = (to_px(anchor.min), to_px(anchor.max));
    let anchor_px = glossa_core::ocr::Rect::new(a0.x, a0.y, a1.x - a0.x, a1.y - a0.y);
    // The list's window lives on the anchor's monitor, at that monitor's
    // scale; egui's zoom (the interface scale) applies on top.
    let centre = a0 + (a1 - a0) / 2.0;
    let scale = monitors
        .iter()
        .find(|m| m.contains(centre.x, centre.y))
        .map(|m| m.desktop_scale())
        .unwrap_or_else(|| window::desktop_scale(ppp / zoom.max(0.1)));
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
    let seed = st.shown && !st.seeded;
    let mut seeded = false;
    ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
        focused = ui.input(|i| i.focused);
        hovered = ui.input(|i| i.pointer.has_pointer());
        if seed {
            seeded = seed_pointer(ui);
        }
        if let Some(events) = forwarded.take().filter(|e| !focused && !e.is_empty()) {
            ui.ctx().input_mut(|i| i.events.extend(events));
        }
        if ui.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
            close_now = true;
        }
        actual = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        let Some(add) = add.take() else { return };
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (r, content, closing) = body(ui, true, size.x - 2.0 * EDGE, max_height, settling, add);
            result = Some(r);
            measured = Some(content);
            close_now |= closing;
        });
    });
    st.hovered = hovered;
    st.seeded |= seeded || st.age > 12;
    // macOS keeps a window's bottom-left corner where it was when the
    // window changes size, so the list made shorter to fit its content
    // would slide down from the control: put it back, once it has the size.
    if cfg!(target_os = "macos") && actual.is_some() && actual != st.placed_at {
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::OuterPosition(pos));
        st.placed_at = actual;
    }

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
            let ms = st.opened.map_or(0, |t| t.elapsed().as_millis());
            tracing::debug!(age = st.age, ms, "dropdown window shown");
        }
    }
    let border = ctx.global_style().visuals.window_stroke.color;
    keep_from_activating(st, &title, (border.r(), border.g(), border.b()));
    (result, close_now, focused)
}

/// A list that appears under a pointer that is not moving knows nothing of
/// the pointer until it moves (the window system reports positions as it
/// moves), and a click before that is dropped: the user sees the item under
/// the pointer and clicks, and nothing happens. Where the pointer is over
/// the list and the list has no position for it, put the pointer where it
/// already is, which makes the system report it. Not on macOS, where moving
/// the pointer from a program stalls it for a moment, and a click there
/// carries its own position anyway. Returns whether it is done (or not
/// needed).
fn seed_pointer(ui: &Ui) -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    let (known, inner, ppp) =
        ui.input(|i| (i.pointer.latest_pos().is_some(), i.viewport().inner_rect, i.pixels_per_point()));
    if known {
        return true;
    }
    let (Some(inner), Some((x, y))) = (inner, window::cursor_position()) else { return false };
    let local = pos2(x as f32 / ppp, y as f32 / ppp) - inner.min.to_vec2();
    if !Rect::from_min_size(Pos2::ZERO, inner.size()).contains(local) {
        return true; // not over the list: nothing to lose
    }
    tracing::trace!(?local, "dropdown: telling the list where the pointer is");
    ui.ctx().send_viewport_cmd(egui::ViewportCommand::CursorPosition(local));
    true
}

/// On Windows, make the list's window one that clicks do not activate, and
/// round its corners as the system rounds menus (Windows 11).
#[cfg(windows)]
fn keep_from_activating(st: &mut State, title: &str, border: (u8, u8, u8)) {
    if st.hwnd.is_none() {
        st.hwnd = window::own_window_by_title(title);
        if let Some(hwnd) = st.hwnd {
            if let Err(e) = glossa_platform::backdrop::style_frame(hwnd, Some(border)) {
                tracing::debug!("dropdown corners: {e}");
            }
        }
    }
    if let Some(hwnd) = st.hwnd {
        window::keep_from_activating(hwnd);
    }
}

#[cfg(not(windows))]
fn keep_from_activating(_: &mut State, _: &str, _: (u8, u8, u8)) {}

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
    let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).constrain(true);
    let area = if st.above {
        area.fixed_pos(anchor.left_top()).pivot(egui::Align2::LEFT_BOTTOM)
    } else {
        area.fixed_pos(anchor.left_bottom())
    };
    let shown = area.show(ctx, |ui| {
        let width = st.content.map_or(min_width, |c| c.x.max(min_width)) - 2.0 * EDGE;
        ui.set_min_width(width.max(0.0));
        out = Some(body(ui, false, width.max(1.0), max_height, settling, add));
    });
    st.area = Some(shown.response.rect);
    if let Some((_, c, _)) = &out {
        // As in a window of its own: the width only grows while it is open.
        let w = (c.x + 2.0 * EDGE).ceil();
        st.content = Some(vec2(st.content.map_or(w, |old| old.x.max(w)), c.y + 2.0 * EDGE));
    }
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
    width: f32,
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
                        // Rows and fields that stretch fill `width`, not the
                        // room the window or the screen leaves: measured
                        // against that room, the list would widen itself a
                        // little more every frame (or reach across the
                        // screen). Wider content still shows, and widens it.
                        ui.set_max_width(width.min(ui.available_width()).max(1.0));
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
                // In the list itself: drawn in the owner's window, the list
                // also sees the click on the combo that opened it.
                .is_some_and(|r| !r.sense.senses_drag() && r.layer_id == ui.layer_id());
            if picked {
                ui.close();
            }
        });
        button
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_goes_below_the_control_then_above_then_out_of_the_window() {
        let room = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 500.0));
        let high = Rect::from_min_size(pos2(10.0, 50.0), vec2(200.0, 24.0));
        let low = Rect::from_min_size(pos2(10.0, 400.0), vec2(200.0, 24.0));
        assert_eq!(in_window(room, high, vec2(200.0, 300.0), false), (true, false));
        assert_eq!(in_window(room, low, vec2(200.0, 300.0), false), (true, true));
        // Too tall either way: a menu leaves, a scrolling list stays.
        assert_eq!(in_window(room, low, vec2(200.0, 700.0), false), (false, true));
        assert_eq!(in_window(room, low, vec2(200.0, 700.0), true), (true, true));
        // Wider than the window.
        assert!(!in_window(room, high, vec2(900.0, 100.0), true).0);
    }

    fn items(ui: &mut Ui, _: bool) {
        for s in ["Use the system proxy", "No proxy", "Manual"] {
            let _ = ui.selectable_label(s == "Manual", s);
        }
    }

    #[test]
    fn a_list_window_keeps_the_width_of_its_content() {
        let ctx = Context::default();
        let mut widths = Vec::new();
        let mut w = 120.0;
        for _ in 0..12 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, 300.0))),
                ..Default::default()
            };
            let mut measured = Vec2::ZERO;
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    measured = body(ui, true, w - 2.0 * EDGE, 400.0, false, items).1;
                });
            });
            out.textures_delta.clear();
            w = (measured.x + 2.0 * EDGE).ceil().max(w);
            widths.push(w);
        }
        eprintln!("window widths: {widths:?}");
        assert!(widths[11] < 260.0, "{widths:?}");
        assert_eq!(widths[11], widths[6], "{widths:?}");
    }

    fn search_and_items(ui: &mut Ui, settling: bool) {
        let mut query = String::new();
        ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search..."));
        items(ui, settling);
    }

    #[test]
    fn a_window_bigger_than_asked_does_not_widen_the_list() {
        let ctx = Context::default();
        let mut asked = 140.0;
        for _ in 0..12 {
            // The system gives the window a third more than asked (a scale
            // the window was created at, before it moved to its monitor).
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(asked * 1.33, 300.0))),
                ..Default::default()
            };
            let mut measured = Vec2::ZERO;
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    measured = body(ui, true, asked - 2.0 * EDGE, 400.0, false, search_and_items).1;
                });
            });
            out.textures_delta.clear();
            asked = (measured.x + 2.0 * EDGE).ceil().max(asked);
        }
        assert!(asked < 200.0, "the list grew to {asked}");
    }

    #[test]
    fn a_list_in_the_window_is_as_wide_as_its_content() {
        let ctx = Context::default();
        let anchor = Rect::from_min_size(pos2(20.0, 20.0), vec2(100.0, 24.0));
        let mut st = State::default();
        let mut last = Rect::NOTHING;
        for _ in 0..6 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let ctx = ui.ctx().clone();
                show_area(&ctx, Id::unique("list"), &mut st, anchor, 100.0, 400.0, search_and_items);
            });
            out.textures_delta.clear();
            last = st.area.unwrap();
        }
        eprintln!("area: {last:?}");
        assert!(last.width() < 260.0, "{last:?}");
    }

    fn frame(owner: bool, list: bool) -> Seen {
        Seen { press_outside: false, owner_focused: owner, list_focused: list }
    }

    #[test]
    fn a_click_in_the_list_that_takes_the_focus_keeps_it_open() {
        let mut t = FocusTrack::default();
        assert!(keep_open(&mut t, false, frame(true, false)));
        // macOS, X11: the click moves the focus from the owner to the list,
        // both seen in the same frame.
        assert!(keep_open(&mut t, false, frame(false, true)));
        assert!(keep_open(&mut t, false, frame(false, true)));
        // And back.
        assert!(keep_open(&mut t, false, frame(true, false)));
    }

    #[test]
    fn a_hand_over_split_across_frames_is_not_a_loss() {
        let mut t = FocusTrack::default();
        assert!(keep_open(&mut t, false, frame(true, false)));
        assert!(keep_open(&mut t, false, frame(false, false)), "one frame with neither focused");
        assert!(keep_open(&mut t, false, frame(false, true)));
    }

    #[test]
    fn the_focus_going_elsewhere_closes_it() {
        let mut t = FocusTrack::default();
        assert!(keep_open(&mut t, false, frame(true, false)));
        assert!(keep_open(&mut t, false, frame(false, false)));
        assert!(!keep_open(&mut t, false, frame(false, false)));
        // From the list too.
        let mut t = FocusTrack::default();
        assert!(keep_open(&mut t, false, frame(false, true)));
        assert!(keep_open(&mut t, false, frame(false, false)));
        assert!(!keep_open(&mut t, false, frame(false, false)));
    }

    #[test]
    fn an_owner_not_in_the_foreground_does_not_close_it_by_itself() {
        let mut t = FocusTrack::default();
        for _ in 0..20 {
            assert!(keep_open(&mut t, false, frame(false, false)));
        }
        // Once it had the focus, losing it counts.
        assert!(keep_open(&mut t, false, frame(true, false)));
        assert!(keep_open(&mut t, false, frame(false, false)));
        assert!(!keep_open(&mut t, false, frame(false, false)));
    }

    #[test]
    fn a_click_outside_closes_it_whatever_the_focus() {
        let outside = Seen { press_outside: true, owner_focused: true, list_focused: false };
        assert!(!keep_open(&mut FocusTrack::default(), false, outside));
        assert!(!keep_open(&mut FocusTrack::default(), true, outside));
    }

    #[test]
    fn a_list_of_a_window_without_focus_ignores_the_focus() {
        let mut t = FocusTrack { had_focus: true, unfocused_frames: 0 };
        for _ in 0..10 {
            assert!(keep_open(&mut t, true, frame(false, false)));
        }
    }
}
