//! The history window: searchable, with favourites, export and per-entry
//! actions.

use egui::RichText;
use glossa_core::history::HistoryEntry;
use glossa_engine::ExportFormat;

use crate::app::GlossaApp;
use crate::icons::{self, Icon};

pub struct HistoryState {
    pub search: String,
    pub favorites_only: bool,
    pub entries: Vec<HistoryEntry>,
    pub dirty: bool,
    pub confirm_clear: bool,
}

pub fn open(app: &mut GlossaApp) {
    if app.history.is_none() {
        app.history = Some(HistoryState {
            search: String::new(),
            favorites_only: false,
            entries: Vec::new(),
            dirty: true,
            confirm_clear: false,
        });
    }
    app.ctx.request_repaint();
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.history.take() else { return };
    if st.dirty {
        st.entries = app.engine.search_history(&st.search, st.favorites_only);
        st.dirty = false;
    }
    let title = app.t("history.title");
    let builder = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_transparent(app.look.window_backdrop != glossa_core::config::Backdrop::None)
        .with_inner_size([760.0, 520.0])
        .with_min_inner_size([420.0, 260.0])
        .with_icon(crate::app_icon(32));
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-history"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        app.zoom_input(ui);
        body(app, ui, &mut st, &mut open);
    });
    app.child_backdrop(&title, false);
    if open {
        app.history = Some(st);
    }
}

fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut HistoryState, open: &mut bool) {
    let look = app.look.clone();
    let expand = app.cfg.history.expand_items;
    egui::Panel::top("hist-top").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text(app.t("history.search"))
                    .desired_width(ui.available_width() - 200.0),
            );
            if r.changed() {
                st.dirty = true;
            }
            let star = if st.favorites_only { Icon::StarFilled } else { Icon::Star };
            if icons::sized_button(ui, &look, star, &app.t("history.favorites_only"), 24.0, true).clicked() {
                st.favorites_only = !st.favorites_only;
                st.dirty = true;
            }
            ui.label(RichText::new(format!("{}", st.entries.len())).small().color(look.weak_text()));
        });
        ui.add_space(4.0);
    });
    egui::Panel::bottom("hist-bottom").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(app.t("history.save_as")).clicked() {
                let mut dialog = rfd::FileDialog::new().set_file_name("glossa-history.html");
                for f in ExportFormat::ALL {
                    dialog = dialog.add_filter(f.description(), &[f.extension()]);
                }
                if let Some(path) = dialog.save_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_owned();
                    let fmt = ExportFormat::from_extension(&ext).unwrap_or(ExportFormat::Text);
                    match app.engine.export_history(&path, fmt, st.favorites_only) {
                        Ok(n) => app.notify(format!("{n} → {}", path.display()), false),
                        Err(e) => app.notify(e.to_string(), true),
                    }
                }
            }
            if st.confirm_clear {
                ui.label(RichText::new("Clear all non-favourite entries?").color(look.error()));
                if ui.button("Yes").clicked() {
                    app.engine.clear_history(false);
                    st.confirm_clear = false;
                    st.dirty = true;
                }
                if ui.button("No").clicked() {
                    st.confirm_clear = false;
                }
            } else if ui.button(app.t("history.clear")).clicked() {
                st.confirm_clear = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(app.t("options.ok")).clicked() {
                    *open = false;
                }
            });
        });
        ui.add_space(4.0);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        let mut action: Option<(u64, &'static str)> = None;
        egui::ScrollArea::vertical().id_salt("hist-list").show(ui, |ui| {
            ui.set_width(ui.available_width());
            for e in &st.entries {
                let tr = e.latest().map(|r| r.text.clone()).unwrap_or_default();
                app.fonts.ensure(&app.ctx, &e.source);
                app.fonts.ensure(&app.ctx, &tr);
                let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).corner_radius(2);
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let star = if e.favorite { Icon::StarFilled } else { Icon::Star };
                            if icons::sized_button(ui, &look, star, "", 18.0, true).clicked() {
                                action = Some((e.id, "fav"));
                            }
                            let src = one_line(&e.source, if expand { 400 } else { 90 });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if icons::sized_button(ui, &look, Icon::Trash, &app.t("history.delete"), 18.0, true)
                                    .clicked()
                                {
                                    action = Some((e.id, "delete"));
                                }
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    ui.add(egui::Label::new(RichText::new(src).strong()).truncate());
                                });
                            });
                        });
                        let shown = if expand { tr.clone() } else { one_line(&tr, 110) };
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            ui.add(egui::Label::new(RichText::new(shown).color(look.weak_text())).wrap());
                        });
                        if let Some(r) = e.latest() {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {} → {}",
                                        r.provider_id,
                                        r.from.english_name(),
                                        r.to.english_name()
                                    ))
                                    .small()
                                    .color(look.weak_text()),
                                );
                            });
                        }
                    })
                    .response
                    .interact(egui::Sense::click());
                if resp.hovered() {
                    ui.painter().rect_stroke(
                        resp.rect,
                        2.0,
                        egui::Stroke::new(1.0, look.border()),
                        egui::StrokeKind::Inside,
                    );
                }
                if resp.double_clicked() {
                    action = Some((e.id, "open"));
                }
                resp.context_menu(|ui| {
                    for (key, what) in [
                        ("history.open", "open"),
                        ("history.copy_text", "copy_text"),
                        ("history.copy_translation", "copy_tr"),
                        ("history.listen", "listen"),
                        ("history.delete", "delete"),
                    ] {
                        if ui.button(app.t(key)).clicked() {
                            action = Some((e.id, what));
                            ui.close();
                        }
                    }
                });
                ui.separator();
            }
        });
        if let Some((id, what)) = action {
            let Some(entry) = st.entries.iter().find(|e| e.id == id).cloned() else { return };
            let tr = entry.latest().cloned();
            match what {
                "fav" => {
                    app.engine.set_favorite(id, !entry.favorite);
                    st.dirty = true;
                }
                "open" => {
                    app.main.source = entry.source.clone();
                    app.main.translated_source.clear();
                    app.translate_main();
                    app.show_main();
                }
                "copy_text" => app.copy_text(&entry.source),
                "copy_tr" => {
                    if let Some(r) = tr {
                        app.copy_text(&r.text);
                    }
                }
                "listen" => app.speak(&entry.source, tr.map(|r| r.from).unwrap_or(glossa_core::LangId::AUTO)),
                "delete" => {
                    app.engine.remove_history(id);
                    st.dirty = true;
                }
                _ => {}
            }
        }
    });
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        flat.chars().take(max - 1).chain(std::iter::once('…')).collect()
    } else {
        flat
    }
}
