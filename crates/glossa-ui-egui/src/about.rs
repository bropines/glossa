//! The About window.

use egui::{vec2, RichText, Sense};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};

pub fn show_about(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let look = app.look.clone();
    let builder = egui::ViewportBuilder::default()
        .with_title(app.t("msg.about"))
        .with_inner_size([440.0, 330.0])
        .with_resizable(false)
        .with_icon(crate::app_icon(32));
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-about"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Translate, look.accent());
                ui.heading("Glossa");
                ui.label(format!("{}{}", app.t("msg.version"), glossa_core::VERSION));
                ui.add_space(6.0);
                ui.label(app.t("about.tagline"));
                ui.label(RichText::new(app.t("about.summary")).small().color(look.weak_text()));
                ui.add_space(10.0);
                ui.label(RichText::new(app.t("about.ocr_credit")).small());
                ui.label(RichText::new(app.t("about.scripting")).small());
                ui.label(RichText::new(app.t("about.compat")).small().color(look.weak_text()));
                ui.add_space(10.0);
                let s = app.engine.module_status();
                let counts =
                    [app.engine.registry().entries().len(), s.loaded.len(), s.failed.len()].map(|n| n.to_string());
                ui.label(
                    RichText::new(app.locale.fmt_args("about.status", &[&counts[0], &counts[1], &counts[2]]))
                        .small()
                        .color(look.weak_text()),
                );
            });
        });
    });
    app.about_open = open;
}
