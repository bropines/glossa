//! Presets in the interface: the Presets tab of Options → Services, and
//! the preset list in the main window's menu.
//!
//! Every operation is the engine's (`Engine::save_preset` and the rest);
//! this draws and dispatches. Applying runs on a worker
//! (`Engine::apply_preset_async`) and its outcome comes back as
//! `Event::PresetApplied`; export and import touch files the user picked, so
//! they run on a thread of their own and report through a channel.
//!
//! The options window keeps an unsaved copy of the settings; it follows
//! preset changes through `Event::PresetsChanged` (handled in `app.rs`), so
//! nothing here touches that copy.

use std::sync::mpsc::Receiver;

use egui::RichText;
use glossa_engine::{PresetEntry, PresetReport};

use crate::app::GlossaApp;
use crate::widgets::group;

/// The Presets tab's state, kept by the options window.
#[derive(Default)]
pub struct PresetsPage {
    selected: Option<String>,
    /// The name typed for "save current setup as".
    new_name: String,
    with_pair: bool,
    /// The name typed for renaming the selected preset.
    rename: String,
    /// The outcome of the last operation, shown under the buttons.
    note: Option<(String, bool)>,
    /// A running export or import.
    task: Option<Receiver<FileOutcome>>,
    /// The current preset has been chosen for the first look.
    opened: bool,
}

enum FileOutcome {
    Exported(String),
    Imported(PresetReport),
    Failed(String),
    Cancelled,
}

/// A preset's name as shown: a built-in one in the interface language.
pub fn label(app: &GlossaApp, name: &str) -> String {
    match glossa_core::preset::builtin_label_key(name) {
        Some(key) => app.t(&key),
        None => name.to_owned(),
    }
}

/// What applying, saving or importing did, as one line for the user.
pub fn report_text(app: &GlossaApp, verb_key: &str, r: &PresetReport) -> String {
    let mut text = app.locale.fmt(verb_key, &label(app, &r.name));
    if !r.missing.is_empty() {
        text.push_str(". ");
        text.push_str(&app.locale.fmt("preset.skipped", &r.missing.join(", ")));
    }
    if !r.dropped_secrets.is_empty() {
        text.push_str(". ");
        text.push_str(&app.locale.fmt("options.presets.dropped", &r.dropped_secrets.join(", ")));
    }
    if !r.dropped_options.is_empty() {
        text.push_str(". ");
        text.push_str(&app.locale.fmt("options.presets.dropped_options", &r.dropped_options.join(", ")));
    }
    text
}

/// A service id as its name, when it is installed.
fn service_names(app: &GlossaApp, ids: &[String]) -> String {
    if ids.is_empty() {
        return app.t("options.presets.unchanged");
    }
    let reg = app.engine.registry();
    ids.iter()
        .map(|id| reg.get(id).map(|p| p.info().name.clone()).unwrap_or_else(|| id.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Presets tab: the list on the left, the chosen preset and what can be
/// done with it on the right, saving the current setup on top.
pub fn page(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut PresetsPage) {
    let look = app.look.clone();
    poll(app, st);
    let entries = app.engine.presets();
    if !st.opened {
        st.opened = true;
        st.selected = entries.iter().find(|e| e.current).map(|e| e.name.clone());
        st.rename = st.selected.clone().unwrap_or_default();
    }
    if st.selected.as_ref().is_some_and(|s| !entries.iter().any(|e| e.name == *s)) {
        st.selected = None;
    }

    ui.label(RichText::new(app.t("options.presets.hint")).small().color(look.weak_text()));
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(app.t("options.presets.save_as"));
        ui.add(egui::TextEdit::singleline(&mut st.new_name).desired_width(160.0));
        ui.checkbox(&mut st.with_pair, app.t("options.presets.with_pair"));
        let can_save = !st.new_name.trim().is_empty();
        if ui.add_enabled(can_save, egui::Button::new(app.t("options.presets.save"))).clicked() {
            match app.engine.save_preset(&st.new_name, st.with_pair) {
                Ok(r) => {
                    st.note = Some((report_text(app, "options.presets.saved", &r), false));
                    st.selected = Some(r.name);
                    st.new_name.clear();
                }
                Err(e) => st.note = Some((e.to_string(), true)),
            }
        }
        if ui.add_enabled(st.task.is_none(), egui::Button::new(app.t("options.presets.import"))).clicked() {
            start_import(app, st);
        }
    });
    ui.label(RichText::new(app.t("options.presets.unsaved_hint")).small().color(look.weak_text()));
    if let Some((text, error)) = &st.note {
        ui.label(RichText::new(text).small().color(if *error { look.error() } else { look.weak_text() }));
    }
    ui.separator();

    egui::Panel::left("preset-list-pane").resizable(true).default_size(260.0).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("preset-list").auto_shrink([false, false]).show(ui, |ui| {
            for e in &entries {
                ui.horizontal(|ui| {
                    let chosen = st.selected.as_deref() == Some(e.name.as_str());
                    if ui.selectable_label(chosen, label(app, &e.name)).clicked() {
                        st.selected = Some(e.name.clone());
                        st.rename = e.name.clone();
                    }
                    if e.current {
                        ui.label(RichText::new(app.t("options.presets.current")).small().color(look.accent()));
                    }
                    if e.builtin {
                        ui.label(RichText::new(app.t("options.presets.builtin")).small().color(look.weak_text()));
                    }
                });
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("preset-detail").show(ui, |ui| {
            ui.set_width(ui.available_width() - 8.0);
            match st.selected.clone().and_then(|s| entries.iter().find(|e| e.name == s).cloned()) {
                Some(e) => details(app, ui, st, &e),
                None => {
                    ui.add_space(8.0);
                    ui.label(RichText::new(app.t("options.presets.pick_one")).color(look.weak_text()));
                }
            }
        });
    });
}

fn details(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut PresetsPage, e: &PresetEntry) {
    let look = app.look.clone();
    ui.add_space(6.0);
    group(ui, &look, &label(app, &e.name), |ui| {
        ui.label(app.locale.fmt("options.presets.services", &service_names(app, &e.enabled)));
        ui.label(app.locale.fmt("options.presets.dictionaries", &service_names(app, &e.dictionary_enabled)));
        let pair = match e.pair {
            Some(p) => format!("{} - {}", crate::langs::name(p.from), crate::langs::name(p.to)),
            None => app.t("options.presets.unchanged"),
        };
        ui.label(app.locale.fmt("options.presets.pair", &pair));
        if !e.missing.is_empty() {
            ui.label(
                RichText::new(app.locale.fmt("options.presets.missing", &e.missing.join(", "))).color(look.error()),
            );
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(app.t("options.presets.apply")).clicked() {
                app.engine.apply_preset_async(e.name.clone());
            }
            if ui.button(app.t("options.presets.duplicate")).clicked() {
                match app.engine.duplicate_preset(&e.name, None) {
                    Ok(copy) => {
                        st.rename = copy.clone();
                        st.selected = Some(copy);
                        st.note = None;
                    }
                    Err(err) => st.note = Some((err.to_string(), true)),
                }
            }
            if ui.add_enabled(st.task.is_none(), egui::Button::new(app.t("options.presets.export"))).clicked() {
                start_export(app, st, &e.name);
            }
        });
        if e.builtin {
            ui.label(RichText::new(app.t("options.presets.readonly")).small().color(look.weak_text()));
            return;
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.rename).desired_width(160.0));
            let changed = !st.rename.trim().is_empty() && st.rename.trim() != e.name;
            if ui.add_enabled(changed, egui::Button::new(app.t("options.presets.rename"))).clicked() {
                match app.engine.rename_preset(&e.name, &st.rename) {
                    Ok(()) => {
                        st.selected = Some(st.rename.trim().to_owned());
                        st.note = None;
                    }
                    Err(err) => st.note = Some((err.to_string(), true)),
                }
            }
            if ui.button(RichText::new(app.t("options.presets.delete")).color(look.error())).clicked() {
                if let Err(err) = app.engine.delete_preset(&e.name) {
                    st.note = Some((err.to_string(), true));
                }
                st.selected = None;
            }
        });
    });
}

fn start_export(app: &GlossaApp, st: &mut PresetsPage, name: &str) {
    let text = match app.engine.export_preset_text(name) {
        Ok(t) => t,
        Err(e) => {
            st.note = Some((e.to_string(), true));
            return;
        }
    };
    let file_name = format!("{}.toml", name.replace(['/', '\\', ':'], "-"));
    let filter = app.t("options.presets.file_filter");
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        let outcome = match rfd::FileDialog::new().add_filter(filter, &["toml"]).set_file_name(file_name).save_file() {
            None => FileOutcome::Cancelled,
            Some(path) => match std::fs::write(&path, text) {
                Ok(()) => FileOutcome::Exported(path.display().to_string()),
                Err(e) => FileOutcome::Failed(format!("{}: {e}", path.display())),
            },
        };
        let _ = tx.send(outcome);
        ctx.request_repaint();
    });
    st.task = Some(rx);
}

fn start_import(app: &GlossaApp, st: &mut PresetsPage) {
    let filter = app.t("options.presets.file_filter");
    let too_large = app.t("options.presets.too_large");
    let engine = app.engine.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = app.ctx.clone();
    std::thread::spawn(move || {
        let outcome = match rfd::FileDialog::new().add_filter(filter, &["toml"]).pick_file() {
            None => FileOutcome::Cancelled,
            // Checked here too, before anything is read: a large file picked
            // by mistake (a log, a disk image) is refused at once.
            Some(path) if std::fs::metadata(&path).is_ok_and(|m| m.len() > glossa_engine::presets::MAX_PRESET_FILE) => {
                FileOutcome::Failed(format!("{}: {too_large}", path.display()))
            }
            Some(path) => match engine.import_preset(&path) {
                Ok(r) => FileOutcome::Imported(r),
                Err(e) => FileOutcome::Failed(e.to_string()),
            },
        };
        let _ = tx.send(outcome);
        ctx.request_repaint();
    });
    st.task = Some(rx);
}

fn poll(app: &GlossaApp, st: &mut PresetsPage) {
    let Some(outcome) = st.task.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
    st.task = None;
    st.note = match outcome {
        FileOutcome::Cancelled => None,
        FileOutcome::Exported(path) => Some((app.locale.fmt("options.presets.exported", &path), false)),
        FileOutcome::Failed(e) => Some((e, true)),
        FileOutcome::Imported(r) => {
            let text = report_text(app, "options.presets.imported", &r);
            st.selected = Some(r.name);
            Some((text, false))
        }
    };
}

/// The presets in the main window's menu, in line (a submenu would be cut
/// off at the menu window's edge): one radio item each, the current one
/// marked.
pub fn menu_section(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let entries = app.engine.presets();
    if entries.is_empty() {
        return;
    }
    ui.label(RichText::new(app.t("menu.presets")).color(app.look.weak_text()));
    let mut chosen = None;
    ui.indent("presets-menu", |ui| {
        for e in &entries {
            if ui.radio(e.current, label(app, &e.name)).clicked() {
                chosen = Some(e.name.clone());
            }
        }
    });
    if let Some(name) = chosen {
        app.engine.apply_preset_async(name);
        ui.close();
    }
}
