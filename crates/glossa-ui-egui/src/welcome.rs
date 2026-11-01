//! The welcome guide, on the first start (`general.show_welcome`) and
//! whenever Options → Basics asks for it: what Glossa is, the hotkeys
//! (changeable on the spot), how to use it, where to keep API keys and
//! data, and on macOS the permissions it needs. Every choice is applied as
//! it is made; Finish (or closing the window) only stops the guide from
//! coming back.

use egui::{vec2, RichText};
use glossa_core::hotkey::Action;
use glossa_core::secrets::SecretStorage;
use glossa_platform::shell::SHELL;

use crate::app::GlossaApp;
use crate::widgets::{group, row};

/// The actions the guide shows, most used first.
const KEY_ACTIONS: [Action; 6] = [
    Action::ShowPopup,
    Action::ShowMainWindow,
    Action::RecognizeAndTranslate,
    Action::TextRecognition,
    Action::ReplaceSelection,
    Action::CopyTranslation,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Hello,
    Hotkeys,
    Usage,
    Storage,
    Permissions,
    Done,
}

fn pages() -> Vec<Page> {
    let mut v = vec![Page::Hello, Page::Hotkeys, Page::Usage, Page::Storage];
    if !glossa_platform::permissions::Permission::all().is_empty() {
        v.push(Page::Permissions);
    }
    v.push(Page::Done);
    v
}

/// The open guide.
#[derive(Debug, Default)]
pub struct WelcomeState {
    page: usize,
    /// "Keep the data beside the program" ticked, to be done on Finish.
    portable: bool,
    message: Option<(String, bool)>,
}

/// Open the guide (again).
pub fn open(app: &mut GlossaApp) {
    if app.welcome.is_some() {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(crate::chrome::WELCOME));
    } else {
        app.welcome = Some(WelcomeState::default());
    }
    app.ctx.request_repaint();
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.welcome.take() else { return };
    let title = app.t("welcome.title");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([620.0, 500.0]).with_min_inner_size([540.0, 440.0]);
    let custom = crate::chrome::custom(&app.cfg);
    let look = app.look.clone();
    let mut open = true;
    let mut finished = false;
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::WELCOME), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(
            ui,
            &look,
            custom,
            &crate::chrome::Caption { minimize: false, maximize: false, ..crate::chrome::Caption::new(&title) },
        );
        let all = pages();
        st.page = st.page.min(all.len() - 1);
        let page = all[st.page];
        egui::Panel::bottom("welcome-nav").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} / {}", st.page + 1, all.len())).color(look.weak_text()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if st.page + 1 == all.len() {
                        if ui.button(app.t("welcome.finish")).clicked() {
                            finished = true;
                        }
                    } else {
                        if ui.button(app.t("welcome.next")).clicked() {
                            st.page += 1;
                        }
                        if ui.button(app.t("welcome.skip")).clicked() {
                            finished = true;
                        }
                    }
                    if st.page > 0 && ui.button(app.t("welcome.back")).clicked() {
                        st.page -= 1;
                    }
                });
            });
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match page {
                Page::Hello => hello(app, ui),
                Page::Hotkeys => hotkeys(app, ui),
                Page::Usage => usage(app, ui),
                Page::Storage => storage(app, ui, &mut st),
                Page::Permissions => permissions(app, ui),
                Page::Done => done(app, ui, &st),
            });
            if let Some((text, error)) = &st.message {
                ui.add_space(8.0);
                let colour = if *error { look.error() } else { look.weak_text() };
                ui.label(RichText::new(text).color(colour));
            }
        });
    });
    app.child_backdrop(&title, false);
    if finished {
        finish(app, &mut st);
        open = st.message.as_ref().is_some_and(|(_, error)| *error);
    } else if !open {
        // Closed with the window's own button: the same as Skip.
        let _ = app.engine.update_config(|c| c.general.show_welcome = false);
    }
    if open {
        app.welcome = Some(st);
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).heading().strong());
    ui.add_space(6.0);
}

fn para(ui: &mut egui::Ui, text: &str) {
    ui.label(text);
    ui.add_space(6.0);
}

fn hello(app: &mut GlossaApp, ui: &mut egui::Ui) {
    heading(ui, &app.t("welcome.hello.title"));
    para(ui, &app.t("welcome.hello.what"));
    let tray = if SHELL.app_menu { app.t("welcome.hello.menu_bar") } else { app.t("welcome.hello.tray") };
    para(ui, &tray);
    ui.add_space(6.0);
    let cfg = app.engine.config();
    row(ui, &app.t("options.basics.ui_language"), |ui| {
        let locales = app.engine.locales();
        let current = locales
            .iter()
            .find(|l| l.code.eq_ignore_ascii_case(&cfg.general.ui_language))
            .map(|l| format!("{} - {}", l.name, l.native_name))
            .unwrap_or_else(|| app.t("options.basics.ui_language_system"));
        let mut pick = None;
        crate::dropdown::ComboBox::from_id_salt("welcome-ui-lang").width(240.0).selected_text(current).show_ui(
            ui,
            |ui| {
                if ui
                    .selectable_label(cfg.general.ui_language.is_empty(), app.t("options.basics.ui_language_system"))
                    .clicked()
                {
                    pick = Some(String::new());
                }
                for l in &locales {
                    let label = format!("{} - {}", l.name, l.native_name);
                    if ui.selectable_label(l.code == cfg.general.ui_language, label).clicked() {
                        pick = Some(l.code.clone());
                    }
                }
            },
        );
        if let Some(code) = pick {
            let _ = app.engine.update_config(|c| c.general.ui_language = code);
        }
    });
    row(ui, &app.t("options.appearance.theme"), |ui| {
        let entries = app.engine.theme_entries();
        let mut pick = None;
        crate::dropdown::ComboBox::from_id_salt("welcome-theme")
            .width(240.0)
            .selected_text(&cfg.appearance.theme)
            .show_ui(ui, |ui| {
                for t in entries.iter().filter(|t| t.path.is_none()) {
                    if ui.selectable_label(t.name == cfg.appearance.theme, &t.name).clicked() {
                        pick = Some(t.name.clone());
                    }
                }
            });
        if let Some(name) = pick {
            let _ = app.engine.update_config(|c| c.appearance.theme = name);
        }
    });
}

fn hotkeys(app: &mut GlossaApp, ui: &mut egui::Ui) {
    heading(ui, &app.t("welcome.hotkeys.title"));
    para(ui, &app.t("welcome.hotkeys.intro"));
    let cfg = app.engine.config();
    if !cfg.hotkeys.enabled || app.hotkeys.is_none() {
        ui.label(RichText::new(app.t("welcome.hotkeys.unavailable")).color(app.look.weak_text()));
        ui.add_space(6.0);
    }
    let mut change = None;
    egui::Grid::new("welcome-hotkeys").num_columns(2).spacing(vec2(24.0, 8.0)).show(ui, |ui| {
        for action in KEY_ACTIONS {
            ui.label(app.t(&format!("action.{}", action.id())));
            let current = cfg.hotkeys.bindings.get(&action).cloned();
            if let Some(c) = crate::widgets::hotkey_field(
                ui,
                &format!("welcome-{}", action.id()),
                current.as_ref(),
                &app.t("options.hotkeys.press_keys"),
            ) {
                change = Some((action, c));
            }
            ui.end_row();
        }
    });
    if let Some((action, hk)) = change {
        let _ = app.engine.update_config(|c| match hk {
            Some(hk) => {
                c.hotkeys.bindings.insert(action, hk);
            }
            None => {
                c.hotkeys.bindings.remove(&action);
            }
        });
    }
    ui.add_space(6.0);
    ui.label(RichText::new(app.t("welcome.hotkeys.more")).small().color(app.look.weak_text()));
}

/// A hotkey's text for the tips, or a dash when it has none.
fn key(app: &GlossaApp, action: Action) -> String {
    app.engine.config().hotkeys.bindings.get(&action).map(|h| h.to_string()).unwrap_or_else(|| "—".into())
}

fn usage(app: &mut GlossaApp, ui: &mut egui::Ui) {
    heading(ui, &app.t("welcome.usage.title"));
    let tips = [
        app.locale.fmt("welcome.usage.select", &key(app, Action::ShowPopup)),
        app.locale.fmt("welcome.usage.main", &key(app, Action::ShowMainWindow)),
        app.locale.fmt("welcome.usage.ocr", &key(app, Action::RecognizeAndTranslate)),
        app.locale.fmt("welcome.usage.replace", &key(app, Action::ReplaceSelection)),
        app.locale.fmt("welcome.usage.zoom", SHELL.command_key),
        app.t("welcome.usage.services"),
    ];
    for tip in tips {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("•").color(app.look.accent()));
            ui.label(tip);
        });
        ui.add_space(4.0);
    }
}

fn storage(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut WelcomeState) {
    heading(ui, &app.t("welcome.storage.title"));
    let look = app.look.clone();
    let cfg = app.engine.config();
    group(ui, &look, &app.t("options.basics.secret_storage"), |ui| {
        let keychain = app.engine.secret_store_name();
        let mut storage = cfg.general.secret_storage;
        ui.radio_value(&mut storage, SecretStorage::File, app.t("options.basics.secrets_file"));
        ui.add_enabled_ui(keychain.is_some(), |ui| {
            ui.radio_value(
                &mut storage,
                SecretStorage::System,
                app.locale.fmt("options.basics.secrets_system", glossa_platform::secrets::system_store_name()),
            );
        });
        let note = if keychain.is_some() { "options.basics.secrets_note" } else { "options.basics.secrets_none" };
        ui.label(RichText::new(app.t(note)).small().color(look.weak_text()));
        if storage != cfg.general.secret_storage {
            let _ = app.engine.update_config(|c| c.general.secret_storage = storage);
        }
    });
    group(ui, &look, &app.t("welcome.storage.data"), |ui| {
        let paths = app.engine.paths().clone();
        if paths.portable {
            ui.label(app.locale.fmt("welcome.storage.portable_now", &paths.data_dir.display().to_string()));
        } else {
            ui.label(app.locale.fmt("welcome.storage.profile_now", &paths.data_dir.display().to_string()));
            if SHELL.portable_beside_exe && paths.exe_dir_writable() {
                ui.checkbox(&mut st.portable, app.t("welcome.storage.make_portable"));
                ui.label(
                    RichText::new(
                        app.locale.fmt("welcome.storage.portable_note", &paths.exe_dir.display().to_string()),
                    )
                    .small()
                    .color(look.weak_text()),
                );
            }
        }
    });
}

fn permissions(app: &mut GlossaApp, ui: &mut egui::Ui) {
    heading(ui, &app.t("options.permissions"));
    crate::options::permissions(app, ui);
}

fn done(app: &mut GlossaApp, ui: &mut egui::Ui, st: &WelcomeState) {
    heading(ui, &app.t("welcome.done.title"));
    para(ui, &app.locale.fmt("welcome.done.text", &key(app, Action::ShowPopup)));
    if st.portable {
        para(ui, &app.t("welcome.done.restart"));
    }
}

fn finish(app: &mut GlossaApp, st: &mut WelcomeState) {
    let _ = app.engine.update_config(|c| {
        c.general.show_welcome = false;
        // The guide said where Glossa lives; the main window's note would
        // say it again.
        c.general.show_tray_hint = false;
    });
    app.cfg.general.show_tray_hint = false;
    if st.portable {
        match app.engine.paths().make_portable() {
            Ok(_) => app.restart(),
            Err(e) => st.message = Some((app.locale.fmt("welcome.storage.portable_failed", &e.to_string()), true)),
        }
    }
}
