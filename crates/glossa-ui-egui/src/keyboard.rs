//! The on-screen keyboard window.

use egui::vec2;
use glossa_core::LangId;

use crate::app::GlossaApp;

/// The on-screen keyboard: types into the main window's source pane, in the
/// first language's layout or English.
pub fn show_keyboard(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.keyboard_open {
        return;
    }
    let builder = egui::ViewportBuilder::default()
        .with_title(app.t("msg.keyboard"))
        .with_inner_size([620.0, 230.0])
        .with_icon(crate::app_icon(32));
    let mut open = true;
    let id = egui::Id::unique("keyboard-layout");
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-keyboard"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let mut russian: bool =
                ui.data(|d| d.get_temp(id)).unwrap_or(app.cfg.general.first_language == LangId::RUSSIAN);
            let mut shift: bool = ui.data(|d| d.get_temp(id.with("shift"))).unwrap_or(false);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut russian, false, "EN");
                ui.selectable_value(&mut russian, true, "RU");
                ui.toggle_value(&mut shift, app.t("keyboard.shift"));
            });
            let rows_en = ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"];
            let rows_ru = ["ё1234567890-=", "йцукенгшщзхъ\\", "фывапролджэ", "ячсмитьбю."];
            let rows = if russian { rows_ru } else { rows_en };
            for row in rows {
                ui.horizontal(|ui| {
                    for ch in row.chars() {
                        let label = if shift { ch.to_uppercase().to_string() } else { ch.to_string() };
                        if ui.add(egui::Button::new(&label).min_size(vec2(34.0, 30.0))).clicked() {
                            app.main.source.push_str(&label);
                            app.main.edited_at = Some(std::time::Instant::now());
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(app.t("keyboard.space")).min_size(vec2(260.0, 30.0))).clicked() {
                    app.main.source.push(' ');
                }
                if ui.add(egui::Button::new(app.t("keyboard.backspace")).min_size(vec2(60.0, 30.0))).clicked() {
                    app.main.source.pop();
                }
                if ui.add(egui::Button::new(app.t("keyboard.enter")).min_size(vec2(60.0, 30.0))).clicked() {
                    app.main.source.push('\n');
                }
            });
            ui.data_mut(|d| {
                d.insert_temp(id, russian);
                d.insert_temp(id.with("shift"), shift);
            });
        });
    });
    app.keyboard_open = open;
}
