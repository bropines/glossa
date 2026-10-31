//! The options dialog: the classic pages (Basics, Hotkeys, Internet,
//! Services, Languages, Appearance, Exceptions, Advanced, Updates) plus
//! Text recognition, Modules and Control API.
//!
//! Edits go to a draft copy of the settings; Apply and OK commit it, Cancel
//! drops it. A provider's settings are generated from the options it
//! declares, so a module — JavaScript, Python, native, a classic script or a
//! pot-app plugin — gets a settings form without shipping any UI code.

use egui::{vec2, RichText};
use glossa_core::config::{CopyAction, ExceptionMode, ExceptionRule, LensMode, MouseBehaviour, ProxyMode, ProxyScheme};
use glossa_core::{Action, Capabilities, Config, LangId};
use glossa_modules::{OptionKind, OptionSpec};

use crate::app::GlossaApp;
use crate::widgets::{group, row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Basics,
    Hotkeys,
    Internet,
    Services,
    Languages,
    Appearance,
    Exceptions,
    Advanced,
    Ocr,
    Modules,
    Api,
    Updates,
}

impl Page {
    const ALL: [Page; 12] = [
        Page::Basics,
        Page::Hotkeys,
        Page::Internet,
        Page::Services,
        Page::Languages,
        Page::Appearance,
        Page::Exceptions,
        Page::Advanced,
        Page::Ocr,
        Page::Modules,
        Page::Api,
        Page::Updates,
    ];

    fn key(self) -> &'static str {
        match self {
            Page::Basics => "options.basics",
            Page::Hotkeys => "options.hotkeys",
            Page::Internet => "options.internet",
            Page::Services => "options.services",
            Page::Languages => "options.languages",
            Page::Appearance => "options.appearance",
            Page::Exceptions => "options.exceptions",
            Page::Advanced => "options.advanced",
            Page::Ocr => "options.ocr",
            Page::Modules => "options.modules",
            Page::Api => "options.server",
            Page::Updates => "options.updates",
        }
    }
}

pub struct OptionsState {
    pub draft: Config,
    pub page: Page,
    selected_service: Option<String>,
    service_group: Capabilities,
    exception_edit: Option<(Option<usize>, ExceptionRule)>,
    autostart: bool,
    autostart_initial: bool,
    message: Option<(String, bool)>,
    show_secrets: bool,
    /// The AI endpoint builder: the preset and name for the next endpoint.
    new_ai_preset: String,
    new_ai_name: String,
    /// Models loaded per provider id, for the model picker.
    ai_models: std::collections::HashMap<String, Vec<String>>,
    /// A running "load models" / "test" request, and the last outcome.
    ai_task: Option<std::sync::mpsc::Receiver<AiReply>>,
    ai_status: Option<(String, bool)>,
}

/// What a background request from the AI endpoint settings returned.
enum AiReply {
    Models(String, Result<Vec<String>, String>),
    Test(Result<(String, std::time::Duration), String>),
}

pub fn open(app: &mut GlossaApp) {
    if app.options.is_none() {
        let autostart = glossa_platform::autostart::is_autostart_enabled();
        app.options = Some(OptionsState {
            draft: app.engine.config(),
            page: Page::Basics,
            selected_service: None,
            service_group: Capabilities::TRANSLATE,
            exception_edit: None,
            autostart,
            autostart_initial: autostart,
            message: None,
            show_secrets: false,
            new_ai_preset: "openrouter".into(),
            new_ai_name: String::new(),
            ai_models: Default::default(),
            ai_task: None,
            ai_status: None,
        });
    }
    app.ctx.request_repaint();
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.options.take() else { return };
    let builder = egui::ViewportBuilder::default()
        .with_title(app.t("options.title"))
        .with_inner_size([900.0, 600.0])
        .with_min_inner_size([680.0, 420.0])
        .with_icon(crate::app_icon(32));
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-options"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        open &= body(app, ui, &mut st);
    });
    if open {
        app.options = Some(st);
    }
}

fn apply(app: &mut GlossaApp, st: &mut OptionsState) {
    let draft = st.draft.clone();
    let result = app.engine.update_config(move |c| {
        // The window keeps track of geometry and pane contents itself.
        let windows = std::mem::take(&mut c.windows);
        let contents = std::mem::take(&mut c.contents);
        *c = draft;
        c.windows = windows;
        c.contents = contents;
    });
    if st.autostart != st.autostart_initial {
        let exe = std::env::current_exe().unwrap_or_default();
        match glossa_platform::autostart::set_autostart(st.autostart, &exe, &["--hidden".to_owned()]) {
            Ok(()) => st.autostart_initial = st.autostart,
            Err(e) => st.message = Some((e.to_string(), true)),
        }
    }
    match result {
        Ok(()) => st.draft = app.engine.config(),
        Err(e) => st.message = Some((e.to_string(), true)),
    }
}

/// Returns false when the dialog should close.
fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) -> bool {
    let mut keep = true;
    let look = app.look.clone();
    egui::Panel::bottom("opt-buttons").show(ui, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some((m, err)) = &st.message {
                ui.label(RichText::new(m).color(if *err { look.error() } else { look.weak_text() }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let w = vec2(100.0, 28.0);
                if ui.add(egui::Button::new(app.t("options.apply")).min_size(w)).clicked() {
                    apply(app, st);
                    st.message = Some(("Saved.".into(), false));
                }
                if ui.add(egui::Button::new(app.t("options.cancel")).min_size(w)).clicked() {
                    keep = false;
                }
                if ui.add(egui::Button::new(app.t("options.ok")).min_size(w)).clicked() {
                    apply(app, st);
                    keep = false;
                }
            });
        });
        ui.add_space(6.0);
    });
    egui::Panel::left("opt-nav").exact_size(190.0).resizable(false).show(ui, |ui| {
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
            for p in Page::ALL {
                let sel = st.page == p;
                if ui.selectable_label(sel, app.t(p.key())).clicked() {
                    st.page = p;
                }
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(("opt-page", st.page as u8)).show(ui, |ui| {
            ui.set_width(ui.available_width() - 8.0);
            match st.page {
                Page::Basics => basics(app, ui, st),
                Page::Hotkeys => hotkeys(app, ui, st),
                Page::Internet => internet(app, ui, st),
                Page::Services => services(app, ui, st),
                Page::Languages => languages(app, ui, st),
                Page::Appearance => appearance(app, ui, st),
                Page::Exceptions => exceptions(app, ui, st),
                Page::Advanced => advanced(app, ui, st),
                Page::Ocr => ocr(app, ui, st),
                Page::Modules => modules(app, ui, st),
                Page::Api => api(app, ui, st),
                Page::Updates => updates(app, ui, st),
            }
        });
    });
    keep
}

fn lang_row(app: &GlossaApp, ui: &mut egui::Ui, label: &str, salt: &str, value: &mut LangId, allow_auto: bool) {
    let auto = app.t("main.auto_detect");
    row(ui, label, |ui| {
        if let Some(l) = crate::widgets::lang_combo(ui, salt, *value, allow_auto, &auto, &[], 240.0) {
            *value = l;
        }
    });
}

fn basics(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.basics.general_group"), |ui| {
        ui.checkbox(&mut st.autostart, app.t("options.basics.start_with_system"));
        row(ui, &app.t("options.basics.ui_language"), |ui| {
            let locales = app.engine.locales();
            let current = locales
                .iter()
                .find(|l| {
                    l.code.eq_ignore_ascii_case(&d.general.ui_language)
                        || l.folder.eq_ignore_ascii_case(&d.general.ui_language)
                        || l.name.eq_ignore_ascii_case(&d.general.ui_language)
                })
                .map(|l| format!("{} - {}", l.name, l.native_name))
                .unwrap_or_else(|| "System".into());
            egui::ComboBox::from_id_salt("ui-lang").width(240.0).selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(d.general.ui_language.is_empty(), "System").clicked() {
                    d.general.ui_language.clear();
                }
                for l in &locales {
                    if ui
                        .selectable_label(l.code == d.general.ui_language, format!("{} - {}", l.name, l.native_name))
                        .clicked()
                    {
                        d.general.ui_language = l.code.clone();
                    }
                }
            });
        });
        row(ui, &app.t("options.basics.font"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut d.appearance.font_family)
                    .hint_text("--- Default ---")
                    .desired_width(240.0),
            );
        });
        row(ui, &app.t("options.basics.text_size"), |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.text_size, 7.0..=28.0).step_by(0.5));
        });
    });
    group(ui, &look, &app.t("options.basics.auto_detect_group"), |ui| {
        lang_row(app, ui, &app.t("options.basics.first_language"), "first", &mut d.general.first_language, false);
        lang_row(app, ui, &app.t("options.basics.second_language"), "second", &mut d.general.second_language, false);
        lang_row(app, ui, &app.t("options.basics.speech_input"), "speech", &mut d.general.speech_language, false);
        ui.checkbox(
            &mut d.general.smart_direction,
            "Smart direction: the first language goes to the second, everything else to the first",
        );
    });
    group(ui, &look, &app.t("options.basics.history_group"), |ui| {
        ui.checkbox(&mut d.history.enabled, app.t("options.basics.enable_history"));
        ui.checkbox(&mut d.history.clear_on_exit, app.t("options.basics.clear_history_on_exit"));
        ui.checkbox(&mut d.history.expand_items, app.t("options.basics.expand_items"));
    });
}

fn hotkeys(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let d = &mut st.draft;
    ui.checkbox(&mut d.hotkeys.enabled, app.t("options.hotkeys.enable"));
    if app.session.is_wayland() || app.hotkeys.is_none() {
        let token = &d.server.token;
        let cmd = format!("curl -s 'http://{}/v1/action/show_popup?token={token}'", d.server.bind);
        ui.label(RichText::new("Global hotkeys are not available in this session (Wayland). Bind a shortcut in your desktop's keyboard settings to:").color(app.look.weak_text()));
        ui.horizontal(|ui| {
            ui.code(&cmd);
            if crate::icons::sized_button(ui, &app.look, crate::icons::Icon::Copy, "Copy", 18.0, true).clicked() {
                ui.ctx().copy_text(cmd.clone());
            }
        });
    }
    for e in &app.hotkey_errors {
        ui.label(RichText::new(e).color(app.look.error()));
    }
    ui.add_space(6.0);
    let conflicts = d.hotkeys.conflicts();
    egui::Grid::new("hotkeys").num_columns(2).striped(true).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
        ui.label(RichText::new(app.t("options.hotkeys.action")).strong());
        ui.label(RichText::new(app.t("options.hotkeys.hotkey")).strong());
        ui.end_row();
        for action in Action::ALL {
            if action == Action::Quit {
                continue;
            }
            ui.label(app.t(&format!("action.{}", action.id())));
            let current = d.hotkeys.bindings.get(&action).cloned();
            ui.horizontal(|ui| {
                if let Some(change) = crate::widgets::hotkey_field(
                    ui,
                    action.id(),
                    current.as_ref(),
                    &app.t("options.hotkeys.press_keys"),
                ) {
                    match change {
                        Some(hk) => {
                            d.hotkeys.bindings.insert(action, hk);
                        }
                        None => {
                            d.hotkeys.bindings.remove(&action);
                        }
                    }
                }
                if let Some(hk) = &current {
                    if let Some((_, others)) = conflicts.iter().find(|(k, _)| k == hk) {
                        let names: Vec<String> =
                            others.iter().filter(|a| **a != action).map(|a| a.label().to_owned()).collect();
                        ui.label(RichText::new("!").strong().color(app.look.error()))
                            .on_hover_text(app.locale.fmt("options.hotkeys.conflict", &names.join(", ")));
                    }
                }
            });
            ui.end_row();
        }
    });
}

fn internet(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let n = &mut st.draft.network;
    group(ui, &look, &app.t("options.internet.connection"), |ui| {
        row(ui, &app.t("options.internet.timeout"), |ui| {
            ui.add(egui::DragValue::new(&mut n.timeout_secs).range(1..=300));
        });
        row(ui, "Retries:", |ui| {
            ui.add(egui::DragValue::new(&mut n.max_retries).range(0..=5));
        });
    });
    group(ui, &look, &app.t("options.internet.proxy"), |ui| {
        let p = &mut n.proxy;
        let names = [
            (ProxyMode::System, app.t("options.internet.proxy_system")),
            (ProxyMode::None, app.t("options.internet.proxy_none")),
            (ProxyMode::Manual, app.t("options.internet.proxy_manual")),
            (ProxyMode::Auto, app.t("options.internet.proxy_auto")),
        ];
        let current = names.iter().find(|(m, _)| *m == p.mode).map(|(_, n)| n.clone()).unwrap_or_default();
        egui::ComboBox::from_id_salt("proxy-mode").width(ui.available_width()).selected_text(current).show_ui(
            ui,
            |ui| {
                for (m, name) in &names {
                    ui.selectable_value(&mut p.mode, *m, name);
                }
            },
        );
        ui.add_enabled_ui(p.mode == ProxyMode::Manual, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("proxy-scheme")
                    .width(100.0)
                    .selected_text(p.scheme.as_str().to_uppercase())
                    .show_ui(ui, |ui| {
                        for s in [ProxyScheme::Http, ProxyScheme::Https, ProxyScheme::Socks4, ProxyScheme::Socks5] {
                            ui.selectable_value(&mut p.scheme, s, s.as_str().to_uppercase());
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut p.host)
                        .hint_text(app.t("options.internet.host"))
                        .desired_width(240.0),
                );
                ui.add(egui::DragValue::new(&mut p.port).range(1..=65535));
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut p.username)
                        .hint_text(app.t("options.internet.username"))
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut p.password)
                        .hint_text(app.t("options.internet.password"))
                        .password(true)
                        .desired_width(180.0),
                );
            });
            ui.label(
                RichText::new("The password is stored in the settings file in plain text.")
                    .small()
                    .color(look.weak_text()),
            );
        });
    });
}

fn services(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    ui.horizontal(|ui| {
        for (cap, key) in [
            (Capabilities::TRANSLATE, "options.services.translation"),
            (Capabilities::DICTIONARY, "options.services.dictionaries"),
            (Capabilities::LISTEN, "options.services.speech"),
            (Capabilities::OCR, "options.services.ocr"),
        ] {
            if ui.selectable_label(st.service_group == cap, app.t(key)).clicked() {
                st.service_group = cap;
                st.selected_service = None;
            }
        }
    });
    ui.separator();
    let cap = st.service_group;
    let providers = app.engine.providers(cap);
    let d = &mut st.draft;
    let (order, enabled): (&mut Vec<String>, &mut Vec<String>) = if cap == Capabilities::DICTIONARY {
        (&mut d.services.dictionary_order, &mut d.services.dictionary_enabled)
    } else {
        (&mut d.services.order, &mut d.services.enabled)
    };
    // Show providers in the draft's order.
    let mut list: Vec<_> = providers.clone();
    list.sort_by_key(|p| order.iter().position(|o| *o == p.info.id).unwrap_or(usize::MAX));
    let selectable_tabs = cap == Capabilities::TRANSLATE || cap == Capabilities::DICTIONARY;
    egui::ScrollArea::vertical().id_salt("svc-list").max_height(250.0).show(ui, |ui| {
        for p in &list {
            ui.horizontal(|ui| {
                if selectable_tabs {
                    let mut on = enabled.contains(&p.info.id);
                    if ui.checkbox(&mut on, "").changed() {
                        if on {
                            enabled.push(p.info.id.clone());
                        } else {
                            enabled.retain(|e| *e != p.info.id);
                        }
                    }
                }
                crate::widgets::service_badge(ui, &p.info.id, &p.info.name, 18.0);
                let sel = st.selected_service.as_deref() == Some(p.info.id.as_str());
                if ui.selectable_label(sel, &p.info.name).clicked() {
                    st.selected_service = Some(p.info.id.clone());
                }
                ui.label(RichText::new(&p.origin).small().color(look.weak_text()));
                if p.info.needs_credentials {
                    let has = d
                        .services
                        .options
                        .get(&p.info.id)
                        .is_some_and(|o| o.values().any(|v| v.as_str().is_some_and(|s| !s.is_empty())));
                    if !has {
                        ui.label(RichText::new("needs a key").small().color(look.error()));
                    }
                }
                if cap == Capabilities::LISTEN {
                    let mut tts = d.services.tts == p.info.id;
                    if ui.radio(tts, "").on_hover_text("Use for listening").clicked() {
                        tts = true;
                        d.services.tts = p.info.id.clone();
                    }
                    let _ = tts;
                }
                if cap == Capabilities::OCR && ui.radio(d.ocr.provider == p.info.id, "").clicked() {
                    d.ocr.provider = p.info.id.clone();
                }
            });
        }
    });
    if selectable_tabs {
        ui.horizontal(|ui| {
            let sel = st.selected_service.clone();
            let up = crate::icons::sized_button(ui, &look, crate::icons::Icon::Up, "", 26.0, sel.is_some()).clicked();
            let down =
                crate::icons::sized_button(ui, &look, crate::icons::Icon::Down, "", 26.0, sel.is_some()).clicked();
            if let (Some(id), true) = (&sel, up || down) {
                // Materialise the full order first, so unlisted providers
                // keep their place.
                let mut full: Vec<String> = list.iter().map(|p| p.info.id.clone()).collect();
                if let Some(i) = full.iter().position(|x| x == id) {
                    let j = if up { i.saturating_sub(1) } else { (i + 1).min(full.len() - 1) };
                    full.swap(i, j);
                }
                *order = full;
            }
            if cap == Capabilities::DICTIONARY {
                if ui.button(app.t("options.services.add_dictionary")).clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("Dictionaries", &["ifo", "dsl", "dz", "xdxf", "tsv", "txt", "csv"])
                        .pick_files()
                    {
                        for f in files {
                            if !d.dictionary.offline.contains(&f) {
                                d.dictionary.offline.push(f);
                            }
                        }
                        if !enabled.contains(&"offline".to_owned()) {
                            enabled.push("offline".into());
                        }
                    }
                }
                let removable = !d.dictionary.offline.is_empty();
                if ui.add_enabled(removable, egui::Button::new(app.t("options.services.remove_dictionary"))).clicked() {
                    d.dictionary.offline.pop();
                }
            }
        });
        if cap == Capabilities::DICTIONARY && !d.dictionary.offline.is_empty() {
            for f in &d.dictionary.offline {
                ui.label(RichText::new(f.display().to_string()).small().color(look.weak_text()));
            }
        }
        if cap == Capabilities::TRANSLATE {
            ui.checkbox(&mut d.services.fail_over, app.t("options.services.fail_over"));
        }
    }
    if cap == Capabilities::TRANSLATE {
        ai_endpoints(app, ui, st);
    }
    poll_ai_task(app, st);

    // The selected provider's settings, generated from what it declares.
    if let Some(id) = st.selected_service.clone() {
        if let Some(p) = providers.iter().find(|p| p.info.id == id) {
            ui.add_space(6.0);
            group(ui, &look, &format!("{} — {}", p.info.name, app.t("options.services.settings")), |ui| {
                if !p.info.description.is_empty() {
                    ui.label(RichText::new(&p.info.description).small());
                }
                if !p.info.homepage.is_empty() && ui.link(RichText::new(&p.info.homepage).small()).clicked() {
                    app.open_url(&p.info.homepage);
                }
                if let Some(dir) = &p.dir {
                    ui.label(RichText::new(dir.display().to_string()).small().color(look.weak_text()));
                }
                ui.label(
                    RichText::new(format!("Capabilities: {}", p.info.capabilities.names().join(", ")))
                        .small()
                        .color(look.weak_text()),
                );
                if p.options.is_empty() {
                    ui.label(RichText::new("No settings.").color(look.weak_text()));
                } else {
                    ui.checkbox(&mut st.show_secrets, "Show keys");
                    if is_ai_endpoint(&id) {
                        ai_tools(app, ui, &id, &mut st.draft, &st.ai_models, &mut st.ai_task, &st.ai_status);
                    }
                    let values = st.draft.services.options.entry(id.clone()).or_default();
                    for spec in &p.options {
                        option_field(ui, spec, values, st.show_secrets);
                    }
                }
            });
        }
    }
}

fn is_ai_endpoint(id: &str) -> bool {
    id == glossa_engine::ai::BUILTIN_ID || id.starts_with("ai-")
}

/// The AI endpoint builder: the endpoints the user added, and a row to add
/// one from a preset. Adding and removing apply at once (a provider has to
/// exist before its settings can be shown); renaming waits for Apply.
fn ai_endpoints(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    use glossa_engine::ai::{preset, PRESETS};
    let look = app.look.clone();
    ui.add_space(6.0);
    group(ui, &look, "AI endpoints (OpenAI-compatible)", |ui| {
        ui.label(
            RichText::new(
                "Each endpoint is a translator of its own: OpenAI, OpenRouter, DeepSeek, Gemini, a local Ollama… \
                 Select it in the list above for its settings.",
            )
            .small()
            .color(look.weak_text()),
        );
        let mut remove = None;
        for e in st.draft.services.ai_endpoints.iter_mut() {
            ui.horizontal(|ui| {
                crate::widgets::service_badge(ui, &e.id, &e.name, 18.0);
                ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(200.0));
                ui.label(RichText::new(preset(&e.preset).name).small().color(look.weak_text()));
                if ui.button("Settings").clicked() {
                    st.selected_service = Some(e.id.clone());
                }
                if ui.button("Remove").clicked() {
                    remove = Some(e.id.clone());
                }
            });
        }
        if let Some(id) = remove {
            match app.engine.remove_ai_endpoint(&id) {
                Ok(()) => {
                    let s = &mut st.draft.services;
                    s.ai_endpoints.retain(|e| e.id != id);
                    s.enabled.retain(|x| *x != id);
                    s.order.retain(|x| *x != id);
                    s.options.remove(&id);
                    if s.active == id {
                        s.active = s.enabled.first().cloned().unwrap_or_default();
                    }
                    if st.selected_service.as_deref() == Some(id.as_str()) {
                        st.selected_service = None;
                    }
                }
                Err(e) => st.message = Some((e.to_string(), true)),
            }
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("ai-preset")
                .width(190.0)
                .selected_text(preset(&st.new_ai_preset).name)
                .show_ui(ui, |ui| {
                    for p in PRESETS {
                        ui.selectable_value(&mut st.new_ai_preset, p.id.to_owned(), p.name);
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut st.new_ai_name)
                    .hint_text(preset(&st.new_ai_preset).name)
                    .desired_width(180.0),
            );
            if ui.button("Add endpoint").clicked() {
                match app.engine.add_ai_endpoint(&st.new_ai_name, &st.new_ai_preset) {
                    Ok(id) => {
                        // Mirror into the draft, or Apply would drop it again.
                        let cfg = app.engine.config();
                        if let Some(e) = cfg.services.ai_endpoints.iter().find(|e| e.id == id) {
                            st.draft.services.ai_endpoints.push(e.clone());
                        }
                        st.draft.services.enabled.push(id.clone());
                        st.draft.services.order.push(id.clone());
                        st.selected_service = Some(id);
                        st.new_ai_name.clear();
                    }
                    Err(e) => st.message = Some((e.to_string(), true)),
                }
            }
        });
    });
}

/// "Load models" and "Test" for an AI endpoint, with the draft's settings.
fn ai_tools(
    app: &mut GlossaApp,
    ui: &mut egui::Ui,
    id: &str,
    draft: &mut Config,
    models: &std::collections::HashMap<String, Vec<String>>,
    task: &mut Option<std::sync::mpsc::Receiver<AiReply>>,
    status: &Option<(String, bool)>,
) {
    let look = app.look.clone();
    let busy = task.is_some();
    ui.horizontal(|ui| {
        let values = draft.services.options.get(id).cloned().unwrap_or_default();
        if ui.add_enabled(!busy, egui::Button::new("Load models")).clicked() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (engine, ctx, id) = (app.engine.clone(), app.ctx.clone(), id.to_owned());
            let values = values.clone();
            std::thread::spawn(move || {
                let r = engine.list_models(&id, Some(&values)).map_err(|e| e.to_string());
                let _ = tx.send(AiReply::Models(id, r));
                ctx.request_repaint();
            });
            *task = Some(rx);
        }
        if ui.add_enabled(!busy, egui::Button::new("Test")).on_hover_text("Translate a short sample").clicked() {
            let (tx, rx) = std::sync::mpsc::channel();
            let (engine, ctx, id) = (app.engine.clone(), app.ctx.clone(), id.to_owned());
            std::thread::spawn(move || {
                let r = engine.test_provider(&id, Some(&values)).map_err(|e| e.to_string());
                let _ = tx.send(AiReply::Test(r));
                ctx.request_repaint();
            });
            *task = Some(rx);
        }
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        } else if let Some((text, error)) = status {
            let color = if *error { look.error() } else { look.weak_text() };
            ui.add(egui::Label::new(RichText::new(text).small().color(color)).truncate());
        }
    });
    if let Some(list) = models.get(id).filter(|l| !l.is_empty()) {
        let values = draft.services.options.entry(id.to_owned()).or_default();
        let current = values.get("model").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        ui.horizontal(|ui| {
            ui.label(format!("Models ({})", list.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt(("ai-models", id))
                    .width(320.0)
                    .height(360.0)
                    .selected_text(if current.is_empty() { "Pick a model".to_owned() } else { current.clone() })
                    .show_ui(ui, |ui| {
                        for m in list {
                            if ui.selectable_label(*m == current, m).clicked() {
                                values.insert("model".into(), m.clone().into());
                            }
                        }
                    });
            });
        });
    }
}

fn poll_ai_task(app: &GlossaApp, st: &mut OptionsState) {
    let Some(rx) = &st.ai_task else { return };
    match rx.try_recv() {
        Ok(AiReply::Models(id, Ok(list))) => {
            st.ai_status = Some((format!("{} models", list.len()), false));
            st.ai_models.insert(id, list);
            st.ai_task = None;
        }
        Ok(AiReply::Test(Ok((text, took)))) => {
            st.ai_status = Some((format!("OK, {} ms: {text}", took.as_millis()), false));
            st.ai_task = None;
        }
        Ok(AiReply::Models(_, Err(e)) | AiReply::Test(Err(e))) => {
            st.ai_status = Some((e, true));
            st.ai_task = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            app.ctx.request_repaint_after(std::time::Duration::from_millis(100))
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => st.ai_task = None,
    }
}

/// One generated settings field.
fn option_field(
    ui: &mut egui::Ui,
    spec: &OptionSpec,
    values: &mut std::collections::BTreeMap<String, serde_json::Value>,
    show_secrets: bool,
) {
    let label = if spec.label.is_empty() { spec.key.clone() } else { spec.label.clone() };
    let label = if spec.required { format!("{label} *") } else { label };
    let current = values.get(&spec.key).cloned().unwrap_or_else(|| spec.default.clone());
    ui.horizontal(|ui| {
        let l = ui.label(&label);
        if !spec.help.is_empty() {
            l.on_hover_text(&spec.help);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match spec.kind {
            OptionKind::Bool => {
                let mut b = current.as_bool().unwrap_or(false);
                if ui.checkbox(&mut b, "").changed() {
                    values.insert(spec.key.clone(), b.into());
                }
            }
            OptionKind::Number => {
                let mut n = current.as_f64().unwrap_or(0.0);
                let mut dv = egui::DragValue::new(&mut n).speed(0.05);
                if let (Some(min), Some(max)) = (spec.min, spec.max) {
                    dv = dv.range(min..=max);
                }
                if ui.add(dv).changed() {
                    values.insert(spec.key.clone(), serde_json::json!(n));
                }
            }
            OptionKind::Select => {
                let cur = current.as_str().unwrap_or("").to_owned();
                let shown = spec
                    .choices
                    .iter()
                    .find(|c| c.value == cur)
                    .map(|c| if c.label.is_empty() { c.value.clone() } else { c.label.clone() })
                    .unwrap_or(cur.clone());
                egui::ComboBox::from_id_salt(("opt", &spec.key)).width(280.0).selected_text(shown).show_ui(ui, |ui| {
                    for c in &spec.choices {
                        let name = if c.label.is_empty() { c.value.clone() } else { c.label.clone() };
                        if ui.selectable_label(c.value == cur, name).clicked() {
                            values.insert(spec.key.clone(), c.value.clone().into());
                        }
                    }
                });
            }
            OptionKind::Multiline => {
                let mut s = current.as_str().unwrap_or("").to_owned();
                if ui.add(egui::TextEdit::multiline(&mut s).desired_width(320.0).desired_rows(4)).changed() {
                    values.insert(spec.key.clone(), s.into());
                }
            }
            OptionKind::Text | OptionKind::Secret | OptionKind::Url => {
                let mut s = current.as_str().unwrap_or("").to_owned();
                let edit = egui::TextEdit::singleline(&mut s)
                    .desired_width(320.0)
                    .hint_text(&spec.placeholder)
                    .password(spec.kind == OptionKind::Secret && !show_secrets);
                if ui.add(edit).changed() {
                    values.insert(spec.key.clone(), s.into());
                }
            }
        });
    });
}

fn languages(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let d = &mut st.draft.languages;
    let mut langs: Vec<LangId> = LangId::all().collect();
    langs.sort_by_key(|l| l.english_name());
    let per_col = langs.len().div_ceil(4);
    egui::Grid::new("langs").num_columns(4).spacing(vec2(18.0, 2.0)).show(ui, |ui| {
        for r in 0..per_col {
            for col in 0..4 {
                if let Some(l) = langs.get(col * per_col + r) {
                    let mut on = !d.disabled.contains(l);
                    if ui.checkbox(&mut on, l.english_name()).on_hover_text(l.native_name()).changed() {
                        if on {
                            d.disabled.retain(|x| x != l);
                        } else {
                            d.disabled.push(*l);
                        }
                    }
                } else {
                    ui.label("");
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    if ui
        .add_sized(vec2(ui.available_width(), 26.0), egui::Button::new(app.t("options.languages.toggle_all")))
        .clicked()
    {
        if d.disabled.is_empty() {
            d.disabled = LangId::all().collect();
        } else {
            d.disabled.clear();
        }
    }
}

fn appearance(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.appearance.general"), |ui| {
        ui.checkbox(&mut d.appearance.custom_frame, app.t("options.appearance.window_style"));
        row(ui, &app.t("options.appearance.theme"), |ui| {
            let entries = app.engine.theme_entries();
            egui::ComboBox::from_id_salt("theme").width(260.0).selected_text(&d.appearance.theme).show_ui(ui, |ui| {
                for t in &entries {
                    let label = if t.classic { format!("{}  (classic)", t.name) } else { t.name.clone() };
                    if ui.selectable_label(t.name == d.appearance.theme, label).clicked() {
                        d.appearance.theme = t.name.clone();
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            if ui.button(app.t("options.appearance.edit_theme")).clicked() {
                // Built-in themes are exported to the user's theme folder as
                // an editable TOML file that extends the original.
                let entries = app.engine.theme_entries();
                let path = match entries.iter().find(|t| t.name == d.appearance.theme).and_then(|t| t.path.clone()) {
                    Some(p) => p,
                    None => {
                        let dir = app.engine.paths().user_theme_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let p = dir.join(format!("{} (custom).toml", d.appearance.theme));
                        if !p.exists() {
                            let text = format!("# A theme of your own. Anything not set here comes from \"{0}\".\nextends = \"{0}\"\nname = \"{0} (custom)\"\n\n# accent = \"#2a8de4\"\n# [popup]\n# opacity = 0.9\n", d.appearance.theme);
                            let _ = std::fs::write(&p, text);
                        }
                        d.appearance.theme = format!("{} (custom)", d.appearance.theme);
                        p
                    }
                };
                let _ = crate::app::open_with_system(&path.to_string_lossy());
                app.engine.refresh_catalogs();
            }
            if ui.button(app.t("options.appearance.reload_themes")).clicked() {
                app.engine.refresh_catalogs();
            }
        });
        row(ui, &app.t("options.appearance.ui_scale"), |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.ui_scale, 0.75..=2.0).step_by(0.05));
        });
        row(ui, "Window transparency (0 - 90):", |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.window_transparency, 0..=90));
        });
        row(ui, "Graphics engine (after restart):", |ui| {
            use glossa_core::config::Renderer;
            let names = [
                (Renderer::OpenGl, "OpenGL — lightest"),
                (Renderer::Gpu, "GPU — Direct3D 12 / Vulkan / Metal"),
                (Renderer::Software, "Software — no GPU (WARP)"),
            ];
            let cur = names.iter().find(|(r, _)| *r == d.appearance.renderer).map(|(_, n)| *n).unwrap_or("");
            egui::ComboBox::from_id_salt("renderer").width(280.0).selected_text(cur).show_ui(ui, |ui| {
                for (r, n) in names {
                    ui.selectable_value(&mut d.appearance.renderer, r, n);
                }
            });
        });
        row(ui, "Unload graphics after hiding, s (0 = never):", |ui| {
            ui.add(egui::DragValue::new(&mut d.appearance.unload_graphics_after_secs).range(0..=3600));
        });
        ui.label(RichText::new("While everything is hidden in the tray, Glossa frees its graphics and keeps only the tray, hotkeys and engine — a few MB. The first window afterwards takes a moment longer to appear.").small().color(look.weak_text()));
    });
    let p = &mut d.appearance.popup;
    group(ui, &look, &app.t("options.appearance.popup_group"), |ui| {
        ui.checkbox(&mut p.auto_size, app.t("options.appearance.auto_size"));
        ui.checkbox(&mut p.auto_position, app.t("options.appearance.auto_position"));
        ui.checkbox(&mut p.activate_on_open, app.t("options.appearance.activate"));
        ui.checkbox(&mut p.pin_when_dragging, app.t("options.appearance.pin_when_dragging"));
        row(ui, &app.t("options.appearance.auto_hide"), |ui| {
            ui.label(app.t("options.appearance.seconds"));
            ui.add(egui::DragValue::new(&mut p.auto_hide_secs).range(0..=600));
        });
        row(ui, &app.t("options.appearance.transparency"), |ui| {
            ui.add(egui::Slider::new(&mut p.transparency, 0..=90));
        });
        row(ui, &app.t("options.appearance.frame_thickness"), |ui| {
            ui.add(egui::Slider::new(&mut p.frame_thickness, 0.0..=8.0).step_by(1.0));
        });
        row(ui, "Width:", |ui| {
            ui.add(egui::DragValue::new(&mut p.max_width).range(200.0..=1600.0).prefix("max "));
            ui.add(egui::DragValue::new(&mut p.min_width).range(120.0..=1200.0).prefix("min "));
        });
        row(ui, &app.t("options.appearance.icons"), |ui| {
            use crate::icons::Icon;
            crate::widgets::icon_toggle(
                ui,
                &look,
                Icon::Replace,
                &mut p.icons.replace,
                &app.t("options.appearance.icon_replace"),
            );
            crate::widgets::icon_toggle(
                ui,
                &look,
                Icon::Copy,
                &mut p.icons.copy,
                &app.t("options.appearance.icon_copy"),
            );
            crate::widgets::icon_toggle(
                ui,
                &look,
                Icon::Listen,
                &mut p.icons.listen,
                &app.t("options.appearance.icon_listen"),
            );
            crate::widgets::icon_toggle(
                ui,
                &look,
                Icon::Dictionary,
                &mut p.icons.dictionary,
                &app.t("options.appearance.icon_dictionary"),
            );
        });
        // As in the classic page: own colours only without window style.
        ui.add_enabled_ui(!d.appearance.custom_frame, |ui| {
            ui.horizontal(|ui| {
                for (key, slot, fallback) in [
                    ("options.appearance.back_color", &mut p.back, look.theme.popup.back),
                    ("options.appearance.text_color", &mut p.text, look.theme.popup.text),
                    ("options.appearance.frame_color", &mut p.frame, look.theme.popup.frame),
                ] {
                    ui.label(app.t(key));
                    let mut col = slot.unwrap_or(fallback);
                    if crate::widgets::color_edit(ui, &mut col) {
                        *slot = Some(col);
                    }
                    ui.add_space(10.0);
                }
            });
        });
    });
}

fn exceptions(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let any_prog = app.t("options.exceptions.any_program");
    let any_win = app.t("options.exceptions.any_window");
    let e = &mut st.draft.exceptions;
    let modes = [
        (ExceptionMode::EnabledFor, app.t("options.exceptions.enabled_for")),
        (ExceptionMode::DisabledFor, app.t("options.exceptions.disabled_for")),
    ];
    let current = modes.iter().find(|(m, _)| *m == e.mode).map(|(_, s)| s.clone()).unwrap_or_default();
    egui::ComboBox::from_id_salt("exc-mode").width(ui.available_width()).selected_text(current).show_ui(ui, |ui| {
        for (m, s) in &modes {
            ui.selectable_value(&mut e.mode, *m, s);
        }
    });
    let mut selected: Option<usize> = ui.data(|dd| dd.get_temp(egui::Id::unique("exc-sel")));
    egui::Grid::new("exc").num_columns(2).striped(true).min_col_width(260.0).show(ui, |ui| {
        ui.label(RichText::new(app.t("options.exceptions.program")).strong());
        ui.label(RichText::new(app.t("options.exceptions.window_class")).strong());
        ui.end_row();
        for (i, r) in e.rules.iter().enumerate() {
            let p = if r.program.is_empty() { any_prog.clone() } else { r.program.clone() };
            let w = if r.window_class.is_empty() { any_win.clone() } else { r.window_class.clone() };
            if ui.selectable_label(selected == Some(i), p).clicked() {
                selected = Some(i);
            }
            ui.label(w);
            ui.end_row();
        }
    });
    ui.data_mut(|dd| dd.insert_temp(egui::Id::unique("exc-sel"), selected));
    ui.horizontal(|ui| {
        if ui.button(app.t("options.exceptions.add")).clicked() {
            st.exception_edit = Some((None, ExceptionRule { program: String::new(), window_class: String::new() }));
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new(app.t("options.exceptions.modify"))).clicked() {
            if let Some(i) = selected {
                st.exception_edit = Some((Some(i), e.rules[i].clone()));
            }
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new(app.t("options.exceptions.delete"))).clicked() {
            if let Some(i) = selected.filter(|i| *i < e.rules.len()) {
                e.rules.remove(i);
            }
        }
    });
    if let Some((index, mut rule)) = st.exception_edit.take() {
        let mut done = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut rule.program).hint_text("program.exe").desired_width(200.0));
            ui.add(egui::TextEdit::singleline(&mut rule.window_class).hint_text("WindowClass").desired_width(200.0));
            if ui.button("OK").clicked() {
                done = true;
            }
            if ui.button(app.t("options.cancel")).clicked() {
                cancel = true;
            }
            if ui
                .button("From the active window")
                .on_hover_text("Fill from the window that was active before this one")
                .clicked()
            {
                if let Some(w) = glossa_platform::window::foreground_window() {
                    rule.program = w.process_name;
                    rule.window_class = w.class_name;
                }
            }
        });
        if done {
            match index {
                Some(i) if i < e.rules.len() => e.rules[i] = rule,
                _ => e.rules.push(rule),
            }
        } else if !cancel {
            st.exception_edit = Some((index, rule));
        }
    }
    ui.checkbox(&mut e.smart_detection, app.t("options.exceptions.smart"));
}

fn advanced(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.advanced.group"), |ui| {
        row(ui, &app.t("options.advanced.browser"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut d.advanced.browser).hint_text("system default").desired_width(260.0),
            );
        });
        row(ui, &app.t("options.advanced.copy_action"), |ui| {
            let names = [
                (CopyAction::CtrlC, "Ctrl + C"),
                (CopyAction::CtrlInsert, "Ctrl + Insert"),
                (CopyAction::AccessibilityOnly, "Accessibility only"),
            ];
            let cur = names.iter().find(|(a, _)| *a == d.advanced.copy_action).map(|(_, n)| *n).unwrap_or("");
            egui::ComboBox::from_id_salt("copy-action").width(260.0).selected_text(cur).show_ui(ui, |ui| {
                for (a, n) in names {
                    ui.selectable_value(&mut d.advanced.copy_action, a, n);
                }
            });
        });
        row(ui, &app.t("options.advanced.preferred_domain"), |ui| {
            egui::ComboBox::from_id_salt("domain").width(260.0).selected_text(&d.advanced.preferred_domain).show_ui(
                ui,
                |ui| {
                    for dom in [
                        "com", "ru", "com.ua", "by", "kz", "de", "fr", "es", "it", "co.uk", "co.jp", "com.br", "pl",
                        "com.tr", "cn",
                    ] {
                        ui.selectable_value(&mut d.advanced.preferred_domain, dom.to_owned(), dom);
                    }
                },
            );
        });
        row(ui, "Selection copy wait, ms:", |ui| {
            ui.add(egui::DragValue::new(&mut d.advanced.copy_wait_ms).range(20..=3000));
        });
        ui.checkbox(&mut d.advanced.slower_listen_repeat, app.t("options.advanced.slower_listen"));
        ui.checkbox(&mut d.advanced.remove_line_breaks, app.t("options.advanced.remove_line_breaks"));
    });
    group(ui, &look, &app.t("options.advanced.mouse_mode_group"), |ui| {
        ui.checkbox(&mut d.mouse_mode.enabled, app.t("tray.mouse_enabled"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut d.mouse_mode.behaviour, MouseBehaviour::ShowIcon, app.t("tray.mouse_icon"));
            ui.radio_value(
                &mut d.mouse_mode.behaviour,
                MouseBehaviour::ShowTranslation,
                app.t("tray.mouse_translation"),
            );
            ui.radio_value(
                &mut d.mouse_mode.behaviour,
                MouseBehaviour::ShowTranslationAndRead,
                app.t("tray.mouse_translation_read"),
            );
        });
        ui.checkbox(&mut d.mouse_mode.ctrl_only, app.t("options.advanced.mouse_ctrl_only"));
        ui.checkbox(&mut d.mouse_mode.tray_click_toggles, app.t("options.advanced.tray_toggles_mouse"));
        ui.label(
            RichText::new(
                "Mouse mode (translate on mouse selection, without a hotkey) is saved but not active in this version.",
            )
            .small()
            .color(look.weak_text()),
        );
    });
}

fn ocr(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft.ocr;
    group(ui, &look, &app.t("options.ocr"), |ui| {
        row(ui, "Provider:", |ui| {
            let providers = app.engine.providers(Capabilities::OCR);
            let cur = providers
                .iter()
                .find(|p| p.info.id == d.provider)
                .map(|p| p.info.name.clone())
                .unwrap_or(d.provider.clone());
            egui::ComboBox::from_id_salt("ocr-provider").width(280.0).selected_text(cur).show_ui(ui, |ui| {
                for p in &providers {
                    ui.selectable_value(&mut d.provider, p.info.id.clone(), &p.info.name);
                }
            });
        });
        let auto = app.t("main.auto_detect");
        row(ui, &app.t("ocr.language"), |ui| {
            if let Some(l) = crate::widgets::lang_combo(ui, "ocr-lang-opt", d.language, true, &auto, &[], 280.0) {
                d.language = l;
            }
        });
        ui.checkbox(&mut d.translate_after, "Translate the recognised text");
        ui.checkbox(&mut d.preserve_line_breaks, "Keep line breaks");
        row(ui, "Watched region check, ms:", |ui| {
            ui.add(egui::DragValue::new(&mut d.watch_interval_ms).range(200..=60000));
        });
        ui.horizontal(|ui| {
            ui.label("Save captured regions to:");
            let shown = d.save_image_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into());
            ui.label(RichText::new(shown).small());
            if ui.button("…").clicked() {
                d.save_image_dir = rfd::FileDialog::new().pick_folder();
            }
            if d.save_image_dir.is_some() && ui.button("Clear").clicked() {
                d.save_image_dir = None;
            }
        });
    });
    group(ui, &look, "Google Lens (chrome-lens-py)", |ui| {
        let l = &mut d.lens;
        ui.horizontal(|ui| {
            ui.radio_value(&mut l.mode, LensMode::Auto, "Auto");
            ui.radio_value(&mut l.mode, LensMode::Daemon, "Daemon only");
            ui.radio_value(&mut l.mode, LensMode::Cli, "CLI only");
        });
        row(ui, "Daemon URL:", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut l.endpoint).hint_text("discovered automatically").desired_width(280.0),
            );
        });
        row(ui, "Token:", |ui| {
            ui.add(egui::TextEdit::singleline(&mut l.token).password(true).desired_width(280.0));
        });
        row(ui, "lens_scan path:", |ui| {
            ui.add(egui::TextEdit::singleline(&mut l.cli_path).hint_text("found on PATH").desired_width(280.0));
        });
        ui.label(
            RichText::new(
                "Install: pip install \"chrome-lens-py[clipboard]\" — the warm daemon makes a scan take ~0.4 s.",
            )
            .small()
            .color(look.weak_text()),
        );
    });
}

fn modules(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let status = app.engine.module_status();
    let mut reload_draft = false;
    let d = &mut st.draft;
    group(ui, &look, "Runtimes", |ui| {
        ui.checkbox(&mut d.modules.allow_legacy_scripts, "Load classic QTranslate service scripts");
        ui.checkbox(&mut d.modules.allow_native, "Load native modules (they run with Glossa's full privileges)");
        row(ui, "Python interpreter:", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut d.modules.python)
                    .hint_text("python3 / py -3 from PATH")
                    .desired_width(280.0),
            );
        });
        row(ui, "Call timeout, ms:", |ui| {
            ui.add(egui::DragValue::new(&mut d.modules.call_timeout_ms).range(1000..=600000));
        });
    });
    group(ui, &look, "Classic QTranslate installation", |ui| {
        ui.horizontal(|ui| {
            let shown = d.classic.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into());
            ui.label(shown);
            if ui.button("Choose…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    d.classic.path = Some(dir);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.classic.use_services, "services");
            ui.checkbox(&mut d.classic.use_themes, "themes");
            ui.checkbox(&mut d.classic.use_locales, "interface languages");
        });
        if let Some(dir) = d.classic.path.clone() {
            if ui.button("Import its settings and history").clicked() {
                match app.engine.import_classic(&dir) {
                    Ok(r) => {
                        st.message = Some((
                            format!(
                                "Imported: settings {}, {} history entries",
                                if r.options_imported { "yes" } else { "no" },
                                r.history_entries
                            ),
                            false,
                        ));
                        reload_draft = true;
                    }
                    Err(e) => st.message = Some((e.to_string(), true)),
                }
            }
        }
    });
    group(ui, &look, "Module folders", |ui| {
        let user_dir = app.engine.paths().user_module_dir();
        ui.horizontal(|ui| {
            if ui.button("Open my modules folder").clicked() {
                let _ = std::fs::create_dir_all(&user_dir);
                let _ = crate::app::open_with_system(&user_dir.to_string_lossy());
            }
            if ui.button("Install module…").on_hover_text("A module folder, or a pot-app .potext file").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("pot-app plugin", &["potext"])
                    .pick_file()
                    .or_else(|| rfd::FileDialog::new().pick_folder())
                {
                    match install(&path, &user_dir) {
                        Ok(()) => {
                            app.engine.reload_modules();
                            st.message = Some(("Installed. Enable it on the Services page.".into(), false));
                        }
                        Err(e) => st.message = Some((e, true)),
                    }
                }
            }
            if ui.button("Reload modules").clicked() {
                app.engine.reload_modules();
            }
        });
        for dir in d.modules.dirs.clone() {
            ui.horizontal(|ui| {
                ui.label(dir.display().to_string());
                if ui.small_button("Remove").clicked() {
                    d.modules.dirs.retain(|x| *x != dir);
                }
            });
        }
        if ui.button("Add a folder…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                d.modules.dirs.push(dir);
            }
        }
    });
    group(
        ui,
        &look,
        &format!(
            "Status: {} loaded, {} failed, {} skipped",
            status.loaded.len(),
            status.failed.len(),
            status.skipped.len()
        ),
        |ui| {
            for (path, why) in &status.failed {
                ui.label(RichText::new(format!("failed  {}: {why}", path.display())).color(look.error()).small());
            }
            for (path, why) in &status.skipped {
                ui.label(RichText::new(format!("– {}: {why}", path.display())).color(look.weak_text()).small());
            }
            ui.label(RichText::new(status.loaded.join(", ")).small().color(look.weak_text()));
        },
    );
    if reload_draft {
        st.draft = app.engine.config();
    }
}

/// Copy a module folder or a `.potext` archive into the user's modules.
fn install(path: &std::path::Path, user_dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(user_dir).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or("no file name")?;
    let target = user_dir.join(name);
    if path.is_file() {
        std::fs::copy(path, &target).map_err(|e| e.to_string())?;
        return Ok(());
    }
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let p = e.path();
            if p.is_dir() {
                copy_dir(&p, &to.join(e.file_name()))?;
            } else {
                std::fs::copy(&p, to.join(e.file_name()))?;
            }
        }
        Ok(())
    }
    if !(path.join("module.toml").is_file() || path.join("Service.js").is_file() || path.join("info.json").is_file()) {
        return Err("that folder has no module.toml, Service.js or info.json".into());
    }
    copy_dir(path, &target).map_err(|e| e.to_string())
}

fn api(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let s = &mut st.draft.server;
    ui.checkbox(&mut s.enabled, "Enable the local control API (restart to apply)");
    row(ui, "Listen on:", |ui| {
        ui.add(egui::TextEdit::singleline(&mut s.bind).desired_width(220.0));
    });
    row(ui, "Token:", |ui| {
        if ui.button("Regenerate").clicked() {
            s.token = glossa_engine::random_token();
        }
        ui.add(egui::TextEdit::singleline(&mut s.token).password(!st.show_secrets).desired_width(320.0));
    });
    ui.checkbox(&mut st.show_secrets, "Show token");
    match &app.server_error {
        Some(e) => {
            ui.label(RichText::new(format!("Not running: {e}")).color(look.error()));
        }
        None => {
            ui.label(RichText::new("Running.").color(look.weak_text()));
        }
    }
    ui.add_space(8.0);
    ui.label("Bind any of these to a key in your desktop or launcher (essential on Wayland):");
    for (what, path) in [
        ("Translate the selection in the popup", "/v1/action/show_popup"),
        ("Recognise a screen region", "/v1/action/text_recognition"),
        ("Recognise and translate", "/v1/action/recognize_and_translate"),
        ("Show the main window", "/v1/action/show_main_window"),
    ] {
        let cmd = format!("curl -s 'http://{}{path}?token={}'", s.bind, s.token);
        ui.horizontal(|ui| {
            if crate::icons::sized_button(ui, &app.look, crate::icons::Icon::Copy, "Copy", 18.0, true).clicked() {
                ui.ctx().copy_text(cmd.clone());
            }
            ui.label(RichText::new(what).small());
        });
        ui.code(if st.show_secrets { cmd } else { cmd.replace(&s.token, "<token>") });
    }
}

fn updates(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    ui.checkbox(&mut st.draft.updates.check_automatically, app.t("options.updates.auto"));
    ui.add_space(8.0);
    ui.label(format!("{}{}", app.t("msg.version"), glossa_core::VERSION));
    ui.label(
        RichText::new("Glossa has no update server yet; new versions are published as releases of the project.")
            .color(app.look.weak_text()),
    );
}
