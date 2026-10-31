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
    let initial = name.trim_start_matches("qt-").chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(size * 0.62), Color32::WHITE);
}

/// One tab of the service strip: badge + name, the name truncating when
/// narrow.
pub fn service_tab(ui: &mut Ui, look: &Look, id: &str, name: &str, selected: bool, width: f32) -> Response {
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
            p.line_segment([rect.left_bottom() + vec2(2.0, -1.0), rect.right_bottom() + vec2(-2.0, -1.0)], egui::Stroke::new(2.0, look.accent()));
        }
        let badge = h - 10.0;
        let badge_rect = egui::Rect::from_min_size(rect.min + vec2(5.0, 5.0), vec2(badge, badge));
        p.rect_filled(badge_rect, badge * 0.2, service_color(id));
        let initial = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
        p.text(badge_rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(badge * 0.62), Color32::WHITE);
        let text_left = badge_rect.right() + 6.0;
        let room = rect.right() - text_left - 4.0;
        if room > 8.0 {
            let galley = ui.painter().layout(
                name.to_owned(),
                egui::FontId::proportional(look.theme.fonts.size * 1.25),
                c(b.text),
                f32::INFINITY,
            );
            let clip = egui::Rect::from_min_max(egui::pos2(text_left, rect.top()), egui::pos2(rect.right() - 2.0, rect.bottom()));
            ui.painter().with_clip_rect(clip).galley(egui::pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, c(b.text));
        }
    }
    resp.on_hover_text(name)
}

/// A language picker: a button showing the language, opening a searchable
/// list (English and native names).
pub fn lang_combo(
    ui: &mut Ui,
    id_salt: &str,
    current: LangId,
    allow_auto: bool,
    auto_label: &str,
    disabled: &[LangId],
    width: f32,
) -> Option<LangId> {
    let mut picked = None;
    let label = if current == LangId::AUTO { auto_label.to_owned() } else { current.english_name().to_owned() };
    let search_id = ui.make_persistent_id((id_salt, "search"));
    egui::ComboBox::from_id_salt(id_salt).width(width).selected_text(label).height(420.0).show_ui(ui, |ui| {
        let mut search: String = ui.data_mut(|d| d.get_temp::<String>(search_id).unwrap_or_default());
        let r = ui.add(egui::TextEdit::singleline(&mut search).hint_text("Search…").desired_width(f32::INFINITY));
        if !r.has_focus() && search.is_empty() {
            r.request_focus();
        }
        ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
        let needle = search.to_lowercase();
        if allow_auto && (needle.is_empty() || auto_label.to_lowercase().contains(&needle)) && ui.selectable_label(current == LangId::AUTO, auto_label).clicked() {
            picked = Some(LangId::AUTO);
        }
        let mut langs: Vec<LangId> = LangId::all().filter(|l| !disabled.contains(l)).collect();
        langs.sort_by_key(|l| l.english_name());
        for l in langs {
            let name = l.english_name();
            let native = l.native_name();
            if !needle.is_empty()
                && !name.to_lowercase().contains(&needle)
                && !native.to_lowercase().contains(&needle)
                && !l.bcp47().eq_ignore_ascii_case(&needle)
            {
                continue;
            }
            let text = if native.is_empty() || native == name { name.to_owned() } else { format!("{name}  ·  {native}") };
            if ui.selectable_label(current == l, text).clicked() {
                picked = Some(l);
            }
        }
    });
    if picked.is_some() {
        ui.data_mut(|d| d.remove::<String>(search_id));
    }
    picked
}

/// An icon that toggles a setting: drawn pressed while on.
pub fn icon_toggle(ui: &mut Ui, look: &Look, icon: crate::icons::Icon, value: &mut bool, tooltip: &str) {
    let size = 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let state = if *value { WidgetState::Pressed } else if resp.hovered() { WidgetState::Hover } else { WidgetState::Normal };
    let b = look.theme.button.get(state);
    ui.painter().rect_filled(rect, look.theme.metrics.button_radius, c(b.back()));
    if *value {
        ui.painter().rect_stroke(rect, look.theme.metrics.button_radius, egui::Stroke::new(1.5, look.accent()), egui::StrokeKind::Inside);
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
    let galley = ui.painter().layout_no_wrap(title.to_owned(), egui::FontId::proportional(look.theme.fonts.size * 1.3), look.text());
    let pos = rect.left_top() + vec2(10.0, -galley.size().y / 2.0);
    ui.painter().rect_filled(egui::Rect::from_min_size(pos - vec2(4.0, 0.0), galley.size() + vec2(8.0, 0.0)), 0.0, c(look.theme.window.back));
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
    let changed = egui::color_picker::color_edit_button_srgba(ui, &mut c32, egui::color_picker::Alpha::Opaque).changed();
    if changed {
        *color = crate::style::from32(c32);
    }
    changed
}

/// A hotkey field: shows the chord; click, then press a combination.
/// Escape cancels, Backspace clears. Returns `Some(new)` when it changed
/// (`Some(None)` for cleared).
pub fn hotkey_field(ui: &mut Ui, id_salt: &str, current: Option<&Hotkey>, prompt: &str) -> Option<Option<Hotkey>> {
    let id = ui.make_persistent_id(("hotkey", id_salt));
    let capturing = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let text = if capturing { prompt.to_owned() } else { current.map(|h| h.to_string().replace('+', " + ")).unwrap_or_default() };
    let resp = ui.add(egui::Button::new(RichText::new(text).italics_if(capturing)).min_size(vec2(170.0, 0.0)));
    if resp.clicked() {
        ui.data_mut(|d| d.insert_temp(id, !capturing));
    }
    if !capturing {
        return None;
    }
    let mut out = None;
    ui.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                match key {
                    egui::Key::Escape => out = Some(None::<Option<Hotkey>>),
                    egui::Key::Backspace | egui::Key::Delete if modifiers.is_none() => out = Some(Some(None)),
                    k => {
                        if let Ok(hk) = format!(
                            "{}{}{}{}{}",
                            if modifiers.ctrl || modifiers.command && !cfg!(target_os = "macos") { "Ctrl+" } else { "" },
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

    #[test]
    fn service_colours_are_stable_and_distinct_for_known_ids() {
        assert_eq!(service_color("google"), service_color("google"));
        assert_ne!(service_color("google"), service_color("yandex"));
        assert_eq!(service_color("qt-some-script"), service_color("qt-some-script"));
    }
}
