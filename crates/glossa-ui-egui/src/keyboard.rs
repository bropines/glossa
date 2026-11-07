//! The on-screen keyboard window.
//!
//! It is drawn like a real keyboard: ANSI rows with their stagger and wide
//! modifier keys, keycaps with a lower edge that sinks when pressed, and the
//! shifted character printed above the plain one. The keys that type nothing
//! follow the system's keyboards (`SHELL.keyboard`): Ctrl, Win and Alt on
//! Windows, Super on Linux, Delete, Return and fn, Control, Option, Cmd on a
//! Mac. The layout follows the
//! first language (English or Russian, switchable); in Russian the Latin
//! letter of each key sits small in its corner, as on Russian keyboards.
//! Keys type into the main window's source pane.
//!
//! Its size is its own, apart from the interface scale: Ctrl+wheel (Cmd on
//! macOS) over it,
//! or dragging the window's edge, scales the keys, and the scale is kept in
//! `appearance.keyboard_scale`.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use glossa_core::theme::WidgetState;
use glossa_core::LangId;

use crate::app::GlossaApp;
use crate::style::{c, Look};

/// Points per key width ("1u") at scale 1.
const UNIT: f32 = 40.0;
/// The keyboard case's margin around the keys, in units.
const CASE: f32 = 0.3;
/// The key rows' width and count, in units.
const COLS: f32 = 15.0;
const ROWS: f32 = 5.0;
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 3.0;
/// How much one Ctrl+plus or Ctrl+minus scales the keyboard.
const KEY_STEP: f32 = 1.1;
/// Holding a key down types it again after this, then at the repeat rate.
const REPEAT_DELAY: Duration = Duration::from_millis(450);
const REPEAT_EVERY: Duration = Duration::from_millis(45);

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The character key at this row and column of a [`Layout`].
    Char(usize, usize),
    Backspace,
    Tab,
    Caps,
    Enter,
    Shift,
    Space,
    /// Drawn so the keyboard looks whole; does nothing. Its label's locale
    /// key.
    Inert(&'static str),
    /// The context menu key, drawn so too.
    Menu,
}

struct Key {
    kind: Kind,
    /// Width in units.
    width: f32,
}

/// The characters on the four character rows: what each key types plain
/// and with Shift. Every layout has the same keys in the same places.
struct Layout {
    lang: LangId,
    plain: [&'static str; 4],
    shifted: [&'static str; 4],
}

const ENGLISH: Layout = Layout {
    lang: LangId::ENGLISH,
    plain: ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"],
    shifted: ["~!@#$%^&*()_+", "QWERTYUIOP{}|", "ASDFGHJKL:\"", "ZXCVBNM<>?"],
};

/// The standard Russian (ЙЦУКЕН) layout, as Windows has it.
const RUSSIAN: Layout = Layout {
    lang: LangId::RUSSIAN,
    plain: ["ё1234567890-=", "йцукенгшщзхъ\\", "фывапролджэ", "ячсмитьбю."],
    shifted: ["Ё!\"№;%:?*()_+", "ЙЦУКЕНГШЩЗХЪ/", "ФЫВАПРОЛДЖЭ", "ЯЧСМИТЬБЮ,"],
};

/// The physical key at each character position, to light up keys pressed
/// on the real keyboard whatever its layout.
const PHYSICAL: [&[egui::Key]; 4] = {
    use egui::Key::*;
    [
        &[Backtick, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0, Minus, Equals],
        &[Q, W, E, R, T, Y, U, I, O, P, OpenBracket, CloseBracket, Backslash],
        &[A, S, D, F, G, H, J, K, L, Semicolon, Quote],
        &[Z, X, C, V, B, N, M, Comma, Period, Slash],
    ]
};

impl Layout {
    fn chars(&self, row: usize, col: usize) -> (char, char) {
        let at = |s: &str| s.chars().nth(col).unwrap_or(' ');
        (at(self.plain[row]), at(self.shifted[row]))
    }
}

/// The rows of keys, left to right, each 15 units wide.
fn rows() -> [Vec<Key>; 5] {
    let k = |kind: Kind, width: f32| Key { kind, width };
    let chars = |row: usize, n: usize| (0..n).map(move |col| k(Kind::Char(row, col), 1.0));
    let mut r0: Vec<Key> = chars(0, 13).collect();
    r0.push(k(Kind::Backspace, 2.0));
    let mut r1 = vec![k(Kind::Tab, 1.5)];
    r1.extend(chars(1, 12));
    r1.push(k(Kind::Char(1, 12), 1.5));
    let mut r2 = vec![k(Kind::Caps, 1.75)];
    r2.extend(chars(2, 11));
    r2.push(k(Kind::Enter, 2.25));
    let mut r3 = vec![k(Kind::Shift, 2.25)];
    r3.extend(chars(3, 10));
    r3.push(k(Kind::Shift, 2.75));
    use glossa_platform::shell::BottomKey;
    let r4 = glossa_platform::shell::SHELL
        .keyboard
        .bottom
        .iter()
        .map(|&(key, width)| match key {
            BottomKey::Space => k(Kind::Space, width),
            BottomKey::Named(name) => k(Kind::Inert(name), width),
            BottomKey::Menu => k(Kind::Menu, width),
        })
        .collect();
    [r0, r1, r2, r3, r4]
}

/// The keyboard's state while its window is open.
#[derive(Clone)]
struct KbState {
    russian: bool,
    /// Shift latched by a click: it applies to the next key, then lets go.
    shift: bool,
    caps: bool,
    scale: f32,
    /// The size the window was opened at, kept so the viewport builder does
    /// not ask for it again every frame.
    open_size: Vec2,
    /// The window's size beyond the keyboard's area (title row, margins).
    chrome: Vec2,
    /// A size asked of the window after a scale change, until it arrives.
    pending: Option<(Vec2, Instant)>,
    /// The window has not been fitted to the keyboard yet.
    fresh: bool,
    /// The scale changed and is waiting to be saved.
    changed_at: Option<Instant>,
    /// The key held with the pointer, for auto-repeat: (row, index, next).
    held: Option<(usize, usize, Instant)>,
    /// Keys held on the real keyboard, by position.
    physical: Vec<egui::Key>,
    physical_shift: bool,
}

impl KbState {
    fn new(app: &GlossaApp) -> Self {
        let scale = app.cfg.appearance.keyboard_scale.clamp(MIN_SCALE, MAX_SCALE);
        // A first guess at the chrome; the first frame measures it.
        let chrome = vec2(16.0, 52.0);
        KbState {
            russian: app.cfg.general.first_language == LangId::RUSSIAN,
            shift: false,
            caps: false,
            scale,
            open_size: chrome + keyboard_size(scale),
            chrome,
            pending: None,
            fresh: true,
            changed_at: None,
            held: None,
            physical: Vec::new(),
            physical_shift: false,
        }
    }

    fn layout(&self) -> &'static Layout {
        if self.russian {
            &RUSSIAN
        } else {
            &ENGLISH
        }
    }

    fn shifted(&self) -> bool {
        self.shift || self.physical_shift
    }

    /// What the character key at `row`, `col` types now.
    fn typed(&self, row: usize, col: usize) -> char {
        let (plain, shifted) = self.layout().chars(row, col);
        let upper = if plain.is_alphabetic() { self.shifted() != self.caps } else { self.shifted() };
        if upper {
            shifted
        } else {
            plain
        }
    }
}

/// The keyboard's size in points at `scale`, case included.
fn keyboard_size(scale: f32) -> Vec2 {
    vec2(COLS + 2.0 * CASE, ROWS + 2.0 * CASE) * UNIT * scale
}

fn state_id() -> egui::Id {
    egui::Id::unique("glossa-keyboard-state")
}

pub fn show_keyboard(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.keyboard_open {
        return;
    }
    let mut st: KbState = ctx.data(|d| d.get_temp(state_id())).unwrap_or_else(|| KbState::new(app));
    let title = app.t("msg.keyboard");
    let builder = crate::chrome::builder(app, &title)
        .with_inner_size(st.open_size)
        .with_min_inner_size(st.chrome + keyboard_size(MIN_SCALE));
    let (look, custom) = (app.look.clone(), crate::chrome::custom(&app.cfg));
    let mut open = true;
    // No `zoom_input` here: Ctrl+wheel over the keyboard sizes the keyboard,
    // not the interface.
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::KEYBOARD), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(ui, &look, custom, &crate::chrome::Caption::new(&title));
        egui::CentralPanel::default().show(ui, |ui| contents(app, ui, &mut st));
    });
    app.child_backdrop(&title, false);
    // Save a new scale once it has been still for a moment, or at once when
    // the window closes.
    if let Some(at) = st.changed_at {
        let wait = Duration::from_millis(800);
        if open && at.elapsed() < wait {
            ctx.request_repaint_after(wait - at.elapsed());
        } else {
            st.changed_at = None;
            let scale = (st.scale * 100.0).round() / 100.0;
            app.cfg.appearance.keyboard_scale = scale;
            if let Err(e) = app.engine.update_config(move |c| c.appearance.keyboard_scale = scale) {
                tracing::warn!("saving the keyboard size: {e}");
            }
        }
    }
    if open {
        ctx.data_mut(|d| d.insert_temp(state_id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<KbState>(state_id()));
    }
    app.keyboard_open = open;
}

/// The window's contents: the layout switch, then the keyboard filling the
/// rest of `ui`.
fn contents(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut KbState) {
    let look = app.look.clone();
    let now = Instant::now();
    real_keys(app, ui, st);

    ui.horizontal(|ui| {
        for (russian, label) in [(false, "EN"), (true, "RU")] {
            let r = ui.selectable_label(st.russian == russian, label);
            if r.clicked() {
                st.russian = russian;
            }
            // Space and Enter on the real keyboard type; they must not press
            // a focused button here.
            r.surrender_focus();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{:.0}%", st.scale * 100.0)).small().color(look.weak_text()))
                .on_hover_text(crate::widgets::shortcut_text(&app.t("keyboard.size_tip")));
        });
    });

    let area = ui.available_rect_before_wrap();
    let inner = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
    if let Some(inner) = inner {
        st.chrome = (inner - area.size()).max(Vec2::ZERO);
    }
    let fit = (area.width() / (COLS + 2.0 * CASE)).min(area.height() / (ROWS + 2.0 * CASE)).max(1.0);

    // Ctrl+wheel (or a pinch) scales the keyboard and the window with it,
    // and so do Ctrl+plus, Ctrl+minus and Ctrl+0 (the interface scale's keys
    // elsewhere; egui's own handling of them is off).
    let zoom = ui.input(|i| i.zoom_delta());
    let (bigger, smaller, reset) = ui.ctx().input_mut(|i| {
        use egui::gui_zoom::kb_shortcuts as k;
        (
            i.consume_shortcut(&k::ZOOM_IN) || i.consume_shortcut(&k::ZOOM_IN_SECONDARY),
            i.consume_shortcut(&k::ZOOM_OUT),
            i.consume_shortcut(&k::ZOOM_RESET),
        )
    });
    let mut request = st.fresh && inner.is_some();
    if request {
        st.fresh = false;
    }
    let old_scale = st.scale;
    if (zoom - 1.0).abs() > 1e-3 {
        // egui's step is over 20% a notch, coarse for a window this wide.
        st.scale *= zoom.powf(0.5);
    }
    if bigger {
        st.scale *= KEY_STEP;
    }
    if smaller {
        st.scale /= KEY_STEP;
    }
    if reset {
        st.scale = 1.0;
    }
    st.scale = st.scale.clamp(MIN_SCALE, MAX_SCALE);
    if (st.scale - old_scale).abs() > 1e-4 {
        st.changed_at = Some(now);
        request = true;
    }
    if request {
        let want = (st.chrome + keyboard_size(st.scale)).round();
        if inner.is_none_or(|s| (s - want).abs().max_elem() > 1.0) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
            st.pending = Some((want, now));
        }
    }
    // While the window catches up with a new scale, draw at that scale; the
    // rest of the time the keys fill the window, so dragging its edge scales
    // them too. A window manager that refuses the size (larger than the
    // screen) gets its way after a moment.
    let unit = match st.pending {
        Some((want, since)) => {
            let arrived = inner.is_some_and(|s| (s - want).abs().max_elem() <= 1.5);
            if arrived || since.elapsed() > Duration::from_secs(1) {
                st.pending = None;
                fit
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(30));
                st.scale * UNIT
            }
        }
        None => fit,
    };
    if st.pending.is_none() {
        let scale = (fit / UNIT).clamp(MIN_SCALE, MAX_SCALE);
        if (scale - st.scale).abs() > 0.004 {
            st.scale = scale;
            st.changed_at = Some(now);
        }
    }

    let (area, _) = ui.allocate_exact_size(area.size(), Sense::hover());
    let size = vec2(COLS + 2.0 * CASE, ROWS + 2.0 * CASE) * unit;
    let board = Rect::from_center_size(area.center(), size);
    let colors = Colors::new(&look);
    let painter = ui.painter().clone();
    paint_case(&painter, board, unit, &colors);

    if !ui.input(|i| i.pointer.primary_down()) {
        st.held = None;
    }
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let origin = board.min + vec2(CASE, CASE) * unit;
    let mut fired: Option<Kind> = None;
    for (r, row) in rows().iter().enumerate() {
        let mut x = 0.0;
        for (i, key) in row.iter().enumerate() {
            let slot = Rect::from_min_size(origin + vec2(x, r as f32) * unit, vec2(key.width, 1.0) * unit);
            x += key.width;
            let live = !matches!(key.kind, Kind::Inert(_) | Kind::Menu);
            let id = ui.scope_id().with(("key", r, i));
            let resp = ui.interact(slot, id, if live { Sense::click() } else { Sense::hover() });
            let down = live && resp.is_pointer_button_down_on();
            if live && primary_pressed && resp.contains_pointer() {
                fired = Some(key.kind);
                if !matches!(key.kind, Kind::Shift | Kind::Caps) {
                    st.held = Some((r, i, now + REPEAT_DELAY));
                }
            } else if down {
                if let Some((hr, hi, next)) = st.held {
                    if (hr, hi) == (r, i) {
                        if now >= next {
                            fired = Some(key.kind);
                            st.held = Some((r, i, now + REPEAT_EVERY));
                        }
                        ui.ctx().request_repaint_after(REPEAT_EVERY);
                    }
                }
            }
            let physical = match key.kind {
                Kind::Char(row, col) => PHYSICAL[row].get(col).is_some_and(|k| st.physical.contains(k)),
                Kind::Backspace => st.physical.contains(&egui::Key::Backspace),
                Kind::Tab => st.physical.contains(&egui::Key::Tab),
                Kind::Enter => st.physical.contains(&egui::Key::Enter),
                Kind::Space => st.physical.contains(&egui::Key::Space),
                _ => false,
            };
            let lit = match key.kind {
                Kind::Shift => st.shifted(),
                Kind::Caps => st.caps,
                _ => false,
            };
            let cap = Cap {
                pressed: down || physical,
                hovered: live && resp.hovered(),
                lit,
                modifier: !matches!(key.kind, Kind::Char(..) | Kind::Space),
            };
            paint_key(&painter, slot, unit, &colors, &cap, |painter, face, text| {
                legend(app, st, painter, face, unit, key.kind, text, &colors);
            });
        }
    }
    if let Some(kind) = fired {
        press(app, st, kind);
    }
}

/// Act on a key.
fn press(app: &mut GlossaApp, st: &mut KbState, kind: Kind) {
    let typed = match kind {
        Kind::Char(row, col) => Some(st.typed(row, col)),
        Kind::Space => Some(' '),
        Kind::Tab => Some('\t'),
        Kind::Enter => Some('\n'),
        Kind::Backspace => {
            app.main.source.pop();
            app.main.edited_at = Some(Instant::now());
            None
        }
        Kind::Shift => {
            st.shift = !st.shift;
            None
        }
        Kind::Caps => {
            st.caps = !st.caps;
            None
        }
        Kind::Inert(_) | Kind::Menu => None,
    };
    if let Some(ch) = typed {
        app.main.source.push(ch);
        app.main.edited_at = Some(Instant::now());
        st.shift = false;
    }
}

/// Typing on the real keyboard while this window has focus goes to the
/// source pane too, and lights the keys it presses.
fn real_keys(app: &mut GlossaApp, ui: &egui::Ui, st: &mut KbState) {
    let (events, shift) = ui.input(|i| (i.events.clone(), i.modifiers.shift));
    st.physical_shift = shift;
    for e in events {
        match e {
            egui::Event::Key { key, physical_key, pressed, modifiers, .. } => {
                let at = physical_key.unwrap_or(key);
                st.physical.retain(|k| *k != at);
                if pressed {
                    st.physical.push(at);
                    if !modifiers.command && !modifiers.alt {
                        match key {
                            egui::Key::Backspace => press(app, st, Kind::Backspace),
                            egui::Key::Enter => press(app, st, Kind::Enter),
                            egui::Key::Tab => press(app, st, Kind::Tab),
                            _ => {}
                        }
                    }
                }
            }
            egui::Event::Text(t) => {
                app.main.source.push_str(&t);
                app.main.edited_at = Some(Instant::now());
            }
            egui::Event::WindowFocused(false) => st.physical.clear(),
            _ => {}
        }
    }
}

/// The theme's colours for the keyboard.
struct Colors {
    case: Color32,
    case_border: Color32,
    face: Color32,
    face_hover: Color32,
    face_pressed: Color32,
    modifier: Color32,
    lit: Color32,
    lit_text: Color32,
    border: Color32,
    text: Color32,
    weak: Color32,
    gleam: Color32,
    dark: bool,
}

impl Colors {
    fn new(look: &Look) -> Self {
        let t = &look.theme;
        let dark = t.dark;
        let face = c(t.button.get(WidgetState::Normal).back()).to_opaque();
        let lit = t.primary_button(WidgetState::Normal);
        Colors {
            case: c(t.window.back).to_opaque().lerp_to_gamma(Color32::BLACK, if dark { 0.3 } else { 0.1 }),
            case_border: c(t.window.border),
            face,
            face_hover: c(t.button.get(WidgetState::Hover).back()).to_opaque(),
            face_pressed: c(t.button.get(WidgetState::Pressed).back()).to_opaque(),
            // Modifier keys are a shade apart from the letters, as on many
            // real keyboards.
            modifier: face.lerp_to_gamma(Color32::BLACK, if dark { 0.18 } else { 0.07 }),
            lit: c(lit.back()),
            lit_text: c(lit.text),
            border: c(t.button.get(WidgetState::Normal).border),
            text: c(t.button.get(WidgetState::Normal).text),
            weak: c(t.window.text_disabled),
            gleam: Color32::from_white_alpha(if dark { 18 } else { 110 }),
            dark,
        }
    }

    /// The keycap's lower edge under a face of colour `face`.
    fn edge(&self, face: Color32) -> Color32 {
        face.lerp_to_gamma(Color32::BLACK, if self.dark { 0.5 } else { 0.25 })
    }
}

fn paint_case(painter: &Painter, board: Rect, unit: f32, colors: &Colors) {
    let radius = 0.22 * unit;
    let shadow = egui::epaint::Shadow {
        offset: [0, (0.06 * unit).round() as i8],
        blur: (0.25 * unit).clamp(2.0, 40.0) as u8,
        spread: 0,
        color: Color32::from_black_alpha(if colors.dark { 90 } else { 40 }),
    };
    painter.add(shadow.as_shape(board, radius));
    painter.rect(board, radius, colors.case, Stroke::new(1.0, colors.case_border), egui::StrokeKind::Inside);
}

/// How one key looks this frame.
struct Cap {
    pressed: bool,
    hovered: bool,
    lit: bool,
    modifier: bool,
}

/// Paint a keycap in `slot` and let `legend` print on its face.
fn paint_key(
    painter: &Painter,
    slot: Rect,
    unit: f32,
    colors: &Colors,
    cap: &Cap,
    legend: impl FnOnce(&Painter, Rect, Color32),
) {
    let gap = (0.05 * unit).max(1.0);
    let body = slot.shrink(gap);
    let edge = (0.09 * unit).max(2.0);
    let radius = (0.12 * unit).max(2.0);
    let mut face_col = if cap.modifier { colors.modifier } else { colors.face };
    if cap.lit {
        face_col = colors.lit;
    } else if cap.pressed {
        face_col = colors.face_pressed;
    } else if cap.hovered {
        face_col = colors.face_hover;
    }
    if cap.lit && cap.hovered {
        face_col = face_col.lerp_to_gamma(Color32::WHITE, 0.12);
    }
    // A pressed key sinks: its face drops onto the edge.
    let travel = if cap.pressed { edge * 0.7 } else { 0.0 };
    painter.rect_filled(body, radius, colors.edge(face_col));
    let face = Rect::from_min_max(body.min + vec2(0.0, travel), pos2(body.max.x, body.max.y - edge + travel));
    painter.rect(face, radius, face_col, Stroke::new(1.0, colors.border.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    if !cap.pressed {
        let y = face.top() + 1.5;
        painter.line_segment(
            [pos2(face.left() + radius, y), pos2(face.right() - radius, y)],
            Stroke::new(1.0, colors.gleam),
        );
    }
    let text = if cap.lit { colors.lit_text } else { colors.text };
    legend(painter, face, text);
}

/// Print a key's legend on its face.
#[allow(clippy::too_many_arguments)]
fn legend(
    app: &GlossaApp,
    st: &KbState,
    painter: &Painter,
    face: Rect,
    unit: f32,
    kind: Kind,
    text: Color32,
    colors: &Colors,
) {
    let font = |k: f32| FontId::proportional(k * unit);
    let weak = colors.weak;
    let names = glossa_platform::shell::SHELL.keyboard;
    match kind {
        Kind::Char(row, col) => {
            let (plain, shifted) = st.layout().chars(row, col);
            // The other layout's character, small in the corner, where it
            // differs: Latin letters on a Russian keyboard.
            let hint = st.russian.then(|| ENGLISH.chars(row, col)).and_then(|(p, s)| {
                if p.is_alphabetic() {
                    Some(p.to_ascii_uppercase())
                } else if p != plain {
                    Some(p)
                } else if s != shifted {
                    Some(s)
                } else {
                    None
                }
            });
            if plain.is_alphabetic() {
                let shown = st.typed(row, col).to_string();
                if let Some(h) = hint {
                    painter.text(face.left_top() + vec2(0.14, 0.08) * unit, Align2::LEFT_TOP, h, font(0.24), weak);
                    painter.text(
                        face.center() + vec2(0.1, 0.06) * unit,
                        Align2::CENTER_CENTER,
                        shown,
                        font(0.42),
                        text,
                    );
                } else {
                    painter.text(face.center(), Align2::CENTER_CENTER, shown, font(0.42), text);
                }
            } else {
                let on = st.shifted();
                let (top, bottom) = if on { (text, weak) } else { (weak, text) };
                let x = face.center().x;
                painter.text(pos2(x, face.top() + face.height() * 0.3), Align2::CENTER_CENTER, shifted, font(0.3), top);
                painter.text(
                    pos2(x, face.top() + face.height() * 0.7),
                    Align2::CENTER_CENTER,
                    plain,
                    font(0.34),
                    bottom,
                );
                if let Some(h) = hint {
                    painter.text(face.right_top() + vec2(-0.1, 0.07) * unit, Align2::RIGHT_TOP, h, font(0.22), weak);
                }
            }
        }
        Kind::Space => {
            let name = st.layout().lang.native_name();
            painter.text(face.center(), Align2::CENTER_CENTER, name, font(0.22), weak);
        }
        Kind::Menu => {
            // The menu key: a page with lines.
            let s = 0.3 * unit;
            let r = Rect::from_center_size(face.center(), vec2(s * 0.8, s));
            let st = Stroke::new((0.035 * unit).max(1.0), weak);
            painter.rect_stroke(r, 1.0, st, egui::StrokeKind::Middle);
            for k in 1..=3 {
                let y = r.top() + r.height() * k as f32 / 4.0;
                painter.line_segment([pos2(r.left() + s * 0.15, y), pos2(r.right() - s * 0.15, y)], st);
            }
        }
        Kind::Inert(key) => {
            painter.text(face.center(), Align2::CENTER_CENTER, app.t(key), font(0.22), weak);
        }
        Kind::Caps => {
            // The lock light, then the name.
            let led = face.left_top() + vec2(0.2, 0.2) * unit;
            let on = if st.caps { colors.lit_text } else { weak.gamma_multiply(0.5) };
            painter.circle_filled(led, (0.05 * unit).max(1.5), on);
            labelled(painter, face, unit, None, &app.t(names.caps), text);
        }
        Kind::Tab => labelled(painter, face, unit, Some(Glyph::Tab), &app.t(names.tab), text),
        Kind::Shift => labelled(painter, face, unit, Some(Glyph::Shift), &app.t(names.shift), text),
        Kind::Backspace => labelled(painter, face, unit, Some(Glyph::Backspace), &app.t(names.backspace), text),
        Kind::Enter => labelled(painter, face, unit, Some(Glyph::Enter), &app.t(names.enter), text),
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Tab,
    Shift,
    Backspace,
    Enter,
}

/// A modifier key's legend: its glyph and name side by side, centred.
fn labelled(painter: &Painter, face: Rect, unit: f32, glyph: Option<Glyph>, name: &str, color: Color32) {
    let galley = painter.layout_no_wrap(name.to_owned(), FontId::proportional(0.22 * unit), color);
    let g = 0.28 * unit;
    let gap = if glyph.is_some() { 0.1 * unit } else { 0.0 };
    let glyph_w = if glyph.is_some() { g } else { 0.0 };
    let total = glyph_w + gap + galley.size().x;
    let left = face.center().x - total / 2.0;
    if let Some(glyph) = glyph {
        let r = Rect::from_center_size(pos2(left + g / 2.0, face.center().y), vec2(g, g));
        paint_glyph(painter, r, glyph, color, (0.035 * unit).max(1.0));
    }
    let pos = pos2(left + glyph_w + gap, face.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
}

/// The arrows printed on modifier keys, drawn rather than taken from a font.
fn paint_glyph(painter: &Painter, r: Rect, glyph: Glyph, color: Color32, width: f32) {
    let st = Stroke::new(width, color);
    let p = |x: f32, y: f32| -> Pos2 { pos2(r.left() + x * r.width(), r.top() + y * r.height()) };
    match glyph {
        Glyph::Backspace => {
            painter.line_segment([p(0.05, 0.5), p(1.0, 0.5)], st);
            painter.add(Shape::line(vec![p(0.35, 0.22), p(0.05, 0.5), p(0.35, 0.78)], st));
        }
        Glyph::Enter => {
            painter.add(Shape::line(vec![p(0.95, 0.1), p(0.95, 0.65), p(0.05, 0.65)], st));
            painter.add(Shape::line(vec![p(0.3, 0.4), p(0.05, 0.65), p(0.3, 0.9)], st));
        }
        Glyph::Shift => {
            let pts = vec![
                p(0.5, 0.05),
                p(0.95, 0.5),
                p(0.72, 0.5),
                p(0.72, 0.95),
                p(0.28, 0.95),
                p(0.28, 0.5),
                p(0.05, 0.5),
            ];
            painter.add(Shape::closed_line(pts, st));
        }
        Glyph::Tab => {
            painter.line_segment([p(0.0, 0.5), p(0.85, 0.5)], st);
            painter.add(Shape::line(vec![p(0.58, 0.25), p(0.85, 0.5), p(0.58, 0.75)], st));
            painter.line_segment([p(0.98, 0.2), p(0.98, 0.8)], st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_is_fifteen_units() {
        for row in rows() {
            let w: f32 = row.iter().map(|k| k.width).sum();
            assert!((w - COLS).abs() < 1e-4, "a row is {w} units");
        }
    }

    #[test]
    fn layouts_fill_every_character_key() {
        let rows = rows();
        for layout in [&ENGLISH, &RUSSIAN] {
            for row in 0..4 {
                let keys = rows[row].iter().filter(|k| matches!(k.kind, Kind::Char(..))).count();
                assert_eq!(layout.plain[row].chars().count(), keys, "{:?} row {row}", layout.lang);
                assert_eq!(layout.shifted[row].chars().count(), keys, "{:?} row {row}", layout.lang);
                assert_eq!(PHYSICAL[row].len(), keys);
            }
        }
    }

    #[test]
    fn russian_digits_shift_as_on_windows() {
        let digits: String = RUSSIAN.shifted[0].chars().skip(1).collect();
        assert_eq!(digits, "!\"№;%:?*()_+");
        assert_eq!(RUSSIAN.chars(1, 0), ('й', 'Й'));
        assert_eq!(RUSSIAN.chars(3, 9), ('.', ','));
    }

    #[test]
    fn caps_lock_affects_letters_only_and_shift_inverts_it() {
        let mut st = KbState {
            russian: false,
            shift: false,
            caps: true,
            scale: 1.0,
            open_size: Vec2::ZERO,
            chrome: Vec2::ZERO,
            pending: None,
            fresh: false,
            changed_at: None,
            held: None,
            physical: Vec::new(),
            physical_shift: false,
        };
        assert_eq!(st.typed(1, 0), 'Q');
        assert_eq!(st.typed(0, 1), '1');
        st.shift = true;
        assert_eq!(st.typed(1, 0), 'q');
        assert_eq!(st.typed(0, 1), '!');
        st.russian = true;
        assert_eq!(st.typed(0, 3), '№');
        assert_eq!(st.typed(0, 0), 'ё');
    }
}
