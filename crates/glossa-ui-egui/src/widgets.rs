//! Widgets shared by several windows.

use egui::{vec2, Color32, Response, RichText, Sense, Ui};
use glossa_core::theme::WidgetState;
use glossa_core::{Hotkey, LangId};

use crate::style::{c, Look};

/// A stable, pleasant colour for a service, from its id.
pub fn service_color(id: &str) -> Color32 {
    let known = match id {
        "google" | "google-dictionary" => Some((66, 133, 244)),
        "bing" => Some((0, 120, 212)),
        "yandex" => Some((252, 63, 29)),
        "deepl" | "deeplx" => Some((15, 43, 70)),
        "openai" => Some((16, 163, 127)),
        "libretranslate" => Some((30, 120, 200)),
        "mymemory" => Some((230, 126, 34)),
        "lingva" => Some((126, 87, 194)),
        "lens" => Some((52, 168, 83)),
        _ => None,
    };
    if let Some((r, g, b)) = known {
        return Color32::from_rgb(r, g, b);
    }
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    let rgb = egui::ecolor::Hsva::new(hue, 0.55, 0.75, 1.0);
    rgb.into()
}

/// A square badge with the service's initial.
pub fn service_badge(ui: &mut Ui, id: &str, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let col = service_color(id);
    ui.painter().rect_filled(rect, size * 0.2, col);
    let initial =
        name.trim_start_matches("qt-").chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.62),
        Color32::WHITE,
    );
}

/// The least room a service tab's name needs to be worth drawing: about
/// two letters and the ellipsis at `font_size`.
fn service_tab_min_text(font_size: f32) -> f32 {
    font_size * 2.2
}

/// One tab of the service strip: badge + name, the name cut with an
/// ellipsis when narrow and left out when very narrow.
pub fn service_tab(ui: &mut Ui, look: &Look, id: &str, name: &str, selected: bool, width: f32, tip: &str) -> Response {
    let h = look.theme.metrics.service_tab_height.clamp(20.0, 44.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(h), h), Sense::click());
    if ui.is_rect_visible(rect) {
        let state = if selected {
            WidgetState::Pressed
        } else if resp.hovered() {
            WidgetState::Hover
        } else {
            WidgetState::Normal
        };
        let b = look.theme.button.get(state);
        let p = ui.painter();
        p.rect_filled(rect, look.theme.metrics.button_radius, c(b.back()));
        if selected {
            p.line_segment(
                [rect.left_bottom() + vec2(2.0, -1.0), rect.right_bottom() + vec2(-2.0, -1.0)],
                egui::Stroke::new(2.0, look.accent()),
            );
        }
        let badge = h - 10.0;
        let font = egui::FontId::proportional(look.theme.fonts.size * 1.25);
        // The name is cut with an ellipsis when the tab is narrow; a tab too
        // narrow for a couple of letters and the ellipsis shows the badge
        // alone, centred, rather than a sliver of a letter. The tooltip
        // carries the whole name either way.
        let room = rect.width() - (5.0 + badge + 6.0) - 4.0;
        let named = room >= service_tab_min_text(font.size);
        let badge_min =
            if named { rect.min + vec2(5.0, 5.0) } else { egui::pos2(rect.center().x - badge / 2.0, rect.top() + 5.0) };
        let badge_rect = egui::Rect::from_min_size(badge_min, vec2(badge, badge));
        p.rect_filled(badge_rect, badge * 0.2, service_color(id));
        let initial = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
        p.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(badge * 0.62),
            Color32::WHITE,
        );
        if named {
            let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), font, c(b.text));
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let galley = ui.painter().layout_job(job);
            let text_left = badge_rect.right() + 6.0;
            ui.painter().galley(egui::pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, c(b.text));
        }
    }
    resp.on_hover_text(tip)
}

/// One tab of a service strip.
pub struct StripService {
    pub id: String,
    pub name: String,
    /// An AI endpoint (a language model): drawn in the strip's AI group.
    pub ai: bool,
}

/// A row of service tabs: the translators, then the AI services in a group
/// of their own on a blue plate, so a model ("Google Gemini") is not
/// mistaken for the translator it shares a name with. `ai_tip` is the
/// hover text of an AI tab, with `%s` for its name. Returns the service
/// clicked, when it is not the selected one.
pub fn service_strip(
    ui: &mut Ui,
    look: &Look,
    services: &[StripService],
    selected: &str,
    max_each: f32,
    ai_tip: &str,
) -> Option<String> {
    let n_ai = services.iter().filter(|s| s.ai).count();
    let n_plain = services.len() - n_ai;
    // The plate's padding and the gap before it.
    let (pad, gap) = (3.0, 6.0);
    let group = if n_ai > 0 && n_plain > 0 {
        gap + 2.0 * pad
    } else if n_ai > 0 {
        2.0 * pad
    } else {
        0.0
    };
    let room = ui.available_width() - group;
    let even = (room / services.len().max(1) as f32).clamp(24.0, max_each);
    // An AI tab gets the width of its whole name when there is room (up to
    // a third of the strip): cut to its first word, "Google Gemini" would
    // read as "Google" again.
    let h = look.theme.metrics.service_tab_height.clamp(20.0, 44.0);
    let font = egui::FontId::proportional(look.theme.fonts.size * 1.25);
    let ai_w = |name: &str| {
        let text = ui.fonts_mut(|f| f.layout_no_wrap(name.to_owned(), font.clone(), Color32::WHITE).size().x);
        (h - 10.0 + text + 18.0).clamp(even, (room / 3.0).max(even))
    };
    let ai_widths: Vec<f32> = services.iter().filter(|s| s.ai).map(|s| ai_w(&s.name)).collect();
    let plain_each = if n_plain > 0 {
        ((room - ai_widths.iter().sum::<f32>()) / n_plain as f32).clamp(24.0, max_each)
    } else {
        even
    };
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let mut tab = |ui: &mut Ui, s: &StripService, width: f32| {
            let tip = if s.ai { ai_tip.replacen("%s", &s.name, 1) } else { s.name.clone() };
            let on = s.id == selected;
            if service_tab(ui, look, &s.id, &s.name, on, width - 1.0, &tip).clicked() && !on {
                clicked = Some(s.id.clone());
            }
        };
        for s in services.iter().filter(|s| !s.ai) {
            tab(ui, s, plain_each);
        }
        if n_ai == 0 {
            return;
        }
        if n_plain > 0 {
            ui.add_space(gap - 1.0);
        }
        let plate = ui.painter().add(egui::Shape::Noop);
        let start = ui.cursor().left();
        ui.add_space(pad);
        let top = ui.cursor().top();
        let mut bottom = top;
        for (s, w) in services.iter().filter(|s| s.ai).zip(&ai_widths) {
            tab(ui, s, *w);
            bottom = bottom.max(ui.min_rect().bottom());
        }
        ui.add_space(pad);
        let rect = egui::Rect::from_min_max(egui::pos2(start, top - pad), egui::pos2(ui.cursor().left(), bottom + pad));
        let radius = look.theme.metrics.button_radius + pad;
        ui.painter().set(
            plate,
            egui::Shape::Rect(egui::epaint::RectShape::new(
                rect,
                radius,
                look.ai_accent().gamma_multiply(0.45),
                egui::Stroke::new(1.5, look.ai_accent()),
                egui::StrokeKind::Inside,
            )),
        );
    });
    clicked
}

/// How tall a language list gets before it scrolls, under its search box.
const LIST_HEIGHT: f32 = 380.0;

/// A language picker: a button showing the language, opening a searchable
/// list (names in the interface language and native names, see
/// [`crate::langs`]) in a window of its own under the button
/// (see [`crate::dropdown`]). Type to search; the arrow keys and Page
/// Up/Down move through the list, Enter picks. `search_hint` fills the empty
/// search box.
#[allow(clippy::too_many_arguments)] // the labels come from the caller's locale
pub fn lang_combo(
    ui: &mut Ui,
    id_salt: &str,
    current: LangId,
    allow_auto: bool,
    auto_label: &str,
    search_hint: &str,
    disabled: &[LangId],
    width: f32,
) -> Option<LangId> {
    use crate::dropdown;
    let id = ui.make_persistent_id(("lang-combo", id_salt));
    let search_id = id.with("search");
    let mark_id = id.with("mark");
    let forget = |ui: &Ui| {
        ui.ctx().data_mut(|d| {
            d.remove::<String>(search_id);
            d.remove::<usize>(mark_id);
        })
    };
    let label = if current == LangId::AUTO { auto_label.to_owned() } else { crate::langs::name(current) };
    let button = dropdown::combo_button(ui, &label, width, dropdown::is_open(ui.ctx(), id));
    if button.clicked() {
        dropdown::toggle(ui.ctx(), id);
        forget(ui);
    }
    if dropdown::is_open(ui.ctx(), id) {
        // The list shows every language's own name, in its own script.
        crate::fonts::request_language_fonts(ui.ctx());
    }
    let mut picked = None;
    dropdown::show(ui, id, button.rect, button.rect.width(), LIST_HEIGHT + 60.0, |ui, settling| {
        let mut search: String = ui.ctx().data(|d| d.get_temp(search_id)).unwrap_or_default();
        let r = ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text(search_hint)
                .desired_width(f32::INFINITY)
                .align(egui::Align2::LEFT_CENTER),
        );
        if !r.has_focus() {
            r.request_focus();
        }
        let needle = search.trim().to_lowercase();
        let mut items: Vec<LangId> = Vec::new();
        if allow_auto && (needle.is_empty() || auto_label.to_lowercase().contains(&needle)) {
            items.push(LangId::AUTO);
        }
        let mut langs: Vec<LangId> = LangId::all().filter(|l| !disabled.contains(l)).collect();
        crate::langs::sort(&mut langs);
        items.extend(langs.into_iter().filter(|l| crate::langs::matches(*l, &needle)));

        // The keyboard's place in the list: the current language at first,
        // the first match once the search changes.
        let stored: Option<usize> = ui.ctx().data(|d| d.get_temp(mark_id));
        let mut mark = match stored {
            Some(_) if r.changed() => 0,
            Some(m) => m,
            None => items.iter().position(|l| *l == current).unwrap_or(0),
        };
        let (down, up, page_down, page_up, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if down {
            mark += 1;
        }
        if page_down {
            mark += 10;
        }
        if up {
            mark = mark.saturating_sub(1);
        }
        if page_up {
            mark = mark.saturating_sub(10);
        }
        mark = mark.min(items.len().saturating_sub(1));
        let moved = down || up || page_down || page_up || r.changed();
        if enter && !items.is_empty() {
            picked = Some(items[mark]);
            ui.close();
        }

        let labels: Vec<String> = items
            .iter()
            .map(|&l| {
                if l == LangId::AUTO {
                    return auto_label.to_owned();
                }
                crate::langs::label(l)
            })
            .collect();
        // The list scrolls under the search box, which stays in view. A
        // scrolling list reports no wider than it is shown, so make room
        // for the longest name here: the dropdown grows to fit it.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let widest = ui.fonts_mut(|f| {
            labels
                .iter()
                .map(|t| f.layout_no_wrap(t.clone(), font.clone(), Color32::WHITE).size().x)
                .fold(0.0, f32::max)
        });
        ui.set_min_width(widest + 2.0 * ui.spacing().button_padding.x + ui.spacing().scroll.bar_width + 8.0);
        let marked_fill = ui.visuals().widgets.hovered.weak_bg_fill;
        let radius = ui.visuals().widgets.hovered.corner_radius;
        let room = dropdown::list_room(ui).min(LIST_HEIGHT);
        egui::ScrollArea::vertical().id_salt("languages").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                for (n, (l, text)) in items.iter().copied().zip(labels).enumerate() {
                    let behind = ui.painter().add(egui::Shape::Noop);
                    // A right-to-left name (Arabic, Hebrew, Persian…) needs
                    // the bidi layout; the others go as they are.
                    let text = crate::bidi::rich(
                        ui,
                        RichText::new(text).text_style(egui::TextStyle::Button),
                        egui::text::TextWrapping::no_max_width(),
                        false,
                    );
                    let resp = ui.selectable_label(l == current, text);
                    if n == mark && l != current {
                        ui.painter().set(behind, egui::Shape::rect_filled(resp.rect, radius, marked_fill));
                    }
                    if (moved && n == mark) || (settling && needle.is_empty() && l == current) {
                        resp.scroll_to_me(Some(egui::Align::Center));
                    }
                    if resp.clicked() {
                        picked = Some(l);
                        ui.close();
                    }
                }
            });
        });
        ui.ctx().data_mut(|d| {
            d.insert_temp(search_id, search);
            d.insert_temp(mark_id, mark);
        });
    });
    if picked.is_some() {
        forget(ui);
    }
    picked
}

/// An icon that toggles a setting: drawn pressed while on.
pub fn icon_toggle(ui: &mut Ui, look: &Look, icon: crate::icons::Icon, value: &mut bool, tooltip: &str) {
    let size = 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let state = if *value {
        WidgetState::Pressed
    } else if resp.hovered() {
        WidgetState::Hover
    } else {
        WidgetState::Normal
    };
    let b = look.theme.button.get(state);
    ui.painter().rect_filled(rect, look.theme.metrics.button_radius, c(b.back()));
    if *value {
        ui.painter().rect_stroke(
            rect,
            look.theme.metrics.button_radius,
            egui::Stroke::new(1.5, look.accent()),
            egui::StrokeKind::Inside,
        );
    }
    crate::icons::paint(ui.painter(), rect.shrink(5.0), icon, if *value { look.text() } else { look.weak_text() });
    if resp.on_hover_text(tooltip).clicked() {
        *value = !*value;
    }
}

/// A titled frame, like the classic group boxes.
pub fn group<R>(ui: &mut Ui, look: &Look, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(4.0);
    let frame = egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, c(look.theme.window.group_box)))
        .corner_radius(look.theme.metrics.button_radius)
        .inner_margin(egui::Margin { left: 10, right: 10, top: 14, bottom: 8 });
    let resp = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    // The title sits on the top border, over a patch of background.
    let rect = resp.response.rect;
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(look.theme.fonts.size * 1.3),
        look.text(),
    );
    let pos = rect.left_top() + vec2(10.0, -galley.size().y / 2.0);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(pos - vec2(4.0, 0.0), galley.size() + vec2(8.0, 0.0)),
        0.0,
        c(look.theme.window.back),
    );
    ui.painter().galley(pos, galley, look.text());
    resp.inner
}

/// A row: label on the left, control on the right, like the classic pages.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// A colour swatch that opens a picker.
pub fn color_edit(ui: &mut Ui, color: &mut glossa_core::Color) -> bool {
    let mut c32 = c(*color);
    let changed =
        egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed();
    if changed {
        *color = crate::style::from32(c32);
    }
    changed
}

/// A hotkey's keys as this system names them: `Cmd`, `Option` on macOS,
/// `Win` on Windows, `Super` on Linux, then the key.
pub fn hotkey_parts(hotkey: &Hotkey) -> Vec<String> {
    let names = glossa_platform::shell::SHELL.modifier_names;
    let m = &hotkey.mods;
    [(m.ctrl, names.ctrl), (m.alt, names.alt), (m.shift, names.shift), (m.meta, names.meta)]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name.to_owned())
        .chain(std::iter::once(hotkey.key.0.clone()))
        .collect()
}

/// Text that names a shortcut the way the strings (and classic `lang.json`
/// files) spell it, "Ctrl+Enter" or "Alt+Left Arrow", as this system says
/// it: egui's `command` modifier is Cmd on macOS, and Alt is Option there.
/// Only the words on their own are changed, not ones inside another word.
pub fn shortcut_text(text: &str) -> String {
    let shell = glossa_platform::shell::SHELL;
    let mut out = text.to_owned();
    if !shell.command_is_ctrl {
        out = replace_word(&out, "Ctrl", shell.command_key);
    }
    if shell.modifier_names.alt != "Alt" {
        out = replace_word(&out, "Alt", shell.modifier_names.alt);
    }
    out
}

/// `text` with `word` replaced by `with` wherever it stands alone: not
/// next to a letter or a digit.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().next_back().or_else(|| out.chars().next_back());
        let after = rest[at + word.len()..].chars().next();
        let alone = !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric);
        out.push_str(&rest[..at]);
        out.push_str(if alone { with } else { word });
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

/// [`hotkey_parts`] joined: `Cmd+Shift+Q`.
pub fn hotkey_label(hotkey: &Hotkey) -> String {
    hotkey_parts(hotkey).join("+")
}

/// A hotkey field: shows the chord; click, then press a combination.
/// Escape cancels, Backspace clears. Returns `Some(new)` when it changed
/// (`Some(None)` for cleared).
pub fn hotkey_field(ui: &mut Ui, id_salt: &str, current: Option<&Hotkey>, prompt: &str) -> Option<Option<Hotkey>> {
    let id = hotkey_capture_id(ui, id_salt);
    let capturing = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let text =
        if capturing { prompt.to_owned() } else { current.map(|h| hotkey_parts(h).join(" + ")).unwrap_or_default() };
    let resp = ui.add(egui::Button::new(RichText::new(text).italics_if(capturing)).min_size(vec2(170.0, 0.0)));
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(id, !capturing));
    }
    hotkey_capture(ui, id)
}

/// Where a hotkey field keeps whether it is waiting for a combination.
pub fn hotkey_capture_id(ui: &Ui, id_salt: &str) -> egui::Id {
    ui.make_persistent_id(("hotkey", id_salt))
}

/// While the field `id` is waiting ([`hotkey_capture_id`] holds `true`),
/// the combination pressed this frame: `Some(Some(hk))` for a new one,
/// `Some(None)` for cleared (Backspace), and it stops waiting on Escape too.
pub fn hotkey_capture(ui: &mut Ui, id: egui::Id) -> Option<Option<Hotkey>> {
    if !ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        return None;
    }
    let mut out = None;
    ui.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = e {
                // The key itself, not what it types: Shift turns 1 into "!"
                // (and a layout turns Q into "Й"), which no hotkey names, so
                // Ctrl+Shift+1 was never taken. Global hotkeys go by the
                // physical key too.
                let key = physical_key.unwrap_or(*key);
                match key {
                    egui::Key::Escape => out = Some(None::<Option<Hotkey>>),
                    egui::Key::Backspace | egui::Key::Delete if modifiers.is_none() => out = Some(Some(None)),
                    k => {
                        if let Ok(hk) = format!(
                            "{}{}{}{}{}",
                            if modifiers.ctrl || modifiers.command && glossa_platform::shell::SHELL.command_is_ctrl {
                                "Ctrl+"
                            } else {
                                ""
                            },
                            if modifiers.alt { "Alt+" } else { "" },
                            if modifiers.shift { "Shift+" } else { "" },
                            if modifiers.mac_cmd { "Meta+" } else { "" },
                            k.name()
                        )
                        .parse::<Hotkey>()
                        {
                            out = Some(Some(Some(hk)));
                        }
                    }
                }
            }
        }
    });
    match out {
        Some(result) => {
            ui.data_mut(|d| d.insert_temp(id, false));
            result
        }
        None => None,
    }
}

trait ItalicsIf {
    fn italics_if(self, on: bool) -> Self;
}

impl ItalicsIf for RichText {
    fn italics_if(self, on: bool) -> Self {
        if on {
            self.italics()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hotkey field waiting for a combination, given one key press.
    fn capture(key: egui::Key, physical: egui::Key, modifiers: egui::Modifiers) -> Option<Option<Hotkey>> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key,
            physical_key: Some(physical),
            pressed: true,
            repeat: false,
            modifiers,
        });
        let mut out = None;
        let mut frame = ctx.run_ui(input, |ui| {
            let id = hotkey_capture_id(ui, "test");
            ui.data_mut(|d| d.insert_temp(id, true));
            out = hotkey_capture(ui, id);
        });
        frame.textures_delta.clear();
        out
    }

    #[test]
    fn ctrl_is_replaced_only_as_a_word() {
        assert_eq!(replace_word("Translate (Ctrl+Enter)", "Ctrl", "Cmd"), "Translate (Cmd+Enter)");
        assert_eq!(replace_word("Ctrl + wheel, Ctrl+C", "Ctrl", "Cmd"), "Cmd + wheel, Cmd+C");
        assert_eq!(replace_word("с нажатой Ctrl;", "Ctrl", "Cmd"), "с нажатой Cmd;");
        assert_eq!(replace_word("CtrlInsert, LCtrl", "Ctrl", "Cmd"), "CtrlInsert, LCtrl");
        let key = glossa_platform::shell::SHELL.command_key;
        assert_eq!(shortcut_text("Ctrl+N"), format!("{key}+N"));
        let alt = glossa_platform::shell::SHELL.modifier_names.alt;
        assert_eq!(shortcut_text("Go back (Alt+Left Arrow)"), format!("Go back ({alt}+Left Arrow)"));
    }

    #[test]
    fn shift_does_not_change_the_key_a_hotkey_is_taken_from() {
        let ctrl_shift = egui::Modifiers { ctrl: true, shift: true, ..Default::default() };
        // Shift+1 types "!"; the hotkey is still Ctrl+Shift+1.
        let hk = capture(egui::Key::Exclamationmark, egui::Key::Num1, ctrl_shift);
        assert_eq!(hk, Some(Some("Ctrl+Shift+1".parse().unwrap())));
        // A Russian layout types "Й" on Q.
        let hk = capture(egui::Key::Q, egui::Key::Q, ctrl_shift);
        assert_eq!(hk, Some(Some("Ctrl+Shift+Q".parse().unwrap())));
        // Escape still cancels: no change.
        assert_eq!(capture(egui::Key::Escape, egui::Key::Escape, egui::Modifiers::NONE), None);
    }

    #[test]
    fn service_colours_are_stable_and_distinct_for_known_ids() {
        assert_eq!(service_color("google"), service_color("google"));
        assert_ne!(service_color("google"), service_color("yandex"));
        assert_eq!(service_color("qt-some-script"), service_color("qt-some-script"));
    }
}
