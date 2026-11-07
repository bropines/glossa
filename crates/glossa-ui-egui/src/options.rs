//! The options dialog: the classic pages (Basics, Hotkeys, Internet,
//! Services, Languages, Appearance, Exceptions, Advanced, Updates) plus
//! Text recognition, Modules and Control API.
//!
//! Edits go to a draft copy of the settings; Apply and OK commit it, Cancel
//! drops it. A provider's settings are generated from the options it
//! declares, so a module — JavaScript, Python, native, a classic script or a
//! pot-app plugin — gets a settings form without shipping any UI code.

use egui::{vec2, Color32, RichText};
use glossa_core::config::{
    CopyAction, ExceptionMode, ExceptionRule, LensMode, MouseBehaviour, ProxyMode, ProxyScheme, TrayAction, TrayConfig,
    VisionLevel,
};
use glossa_core::{Action, Capabilities, Config, LangId};
use glossa_platform::shell::SHELL;

use crate::app::GlossaApp;
use crate::icons::Icon;
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
    Permissions,
    Updates,
}

/// The groups the pages are listed in, under a small heading each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    General,
    Translation,
    Recognition,
    System,
}

impl Group {
    const ALL: [Group; 4] = [Group::General, Group::Translation, Group::Recognition, Group::System];

    fn key(self) -> &'static str {
        match self {
            Group::General => "options.group.general",
            Group::Translation => "options.group.translation",
            Group::Recognition => "options.group.recognition",
            Group::System => "options.group.system",
        }
    }
}

impl Page {
    /// Every page, in the order the list shows them (by group).
    const ALL: [Page; 13] = [
        Page::Basics,
        Page::Appearance,
        Page::Hotkeys,
        Page::Services,
        Page::Languages,
        Page::Exceptions,
        Page::Ocr,
        Page::Internet,
        Page::Permissions,
        Page::Modules,
        Page::Api,
        Page::Advanced,
        Page::Updates,
    ];

    fn group(self) -> Group {
        match self {
            Page::Basics | Page::Appearance | Page::Hotkeys => Group::General,
            Page::Services | Page::Languages | Page::Exceptions => Group::Translation,
            Page::Ocr => Group::Recognition,
            Page::Internet | Page::Permissions | Page::Modules | Page::Api | Page::Advanced | Page::Updates => {
                Group::System
            }
        }
    }

    /// The page's icon, drawn white on its colour.
    fn icon(self) -> Icon {
        match self {
            Page::Basics => Icon::Gear,
            Page::Appearance => Icon::Palette,
            Page::Hotkeys => Icon::Keyboard,
            Page::Services => Icon::Translate,
            Page::Languages => Icon::Globe,
            Page::Exceptions => Icon::Block,
            Page::Ocr => Icon::Camera,
            Page::Internet => Icon::Wifi,
            Page::Permissions => Icon::Lock,
            Page::Modules => Icon::Boxes,
            Page::Api => Icon::Code,
            Page::Advanced => Icon::Sliders,
            Page::Updates => Icon::Refresh,
        }
    }

    /// A colour of its own, so a page is found by colour as much as by
    /// name (the system colours macOS uses for its settings).
    fn tint(self) -> Color32 {
        let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        match self {
            Page::Basics => rgb(0x8e8e93),
            Page::Appearance => rgb(0xbf5af2),
            Page::Hotkeys => rgb(0x5e5ce6),
            Page::Services => rgb(0x0a84ff),
            Page::Languages => rgb(0x30b0c7),
            Page::Exceptions => rgb(0xff453a),
            Page::Ocr => rgb(0xff9f0a),
            Page::Internet => rgb(0x34c759),
            Page::Permissions => rgb(0x64748b),
            Page::Modules => rgb(0xac8e68),
            Page::Api => rgb(0x5856d6),
            Page::Advanced => rgb(0x8e8e93),
            Page::Updates => rgb(0x32ade6),
        }
    }

    /// Whether this system has the page: only macOS asks for permissions.
    fn shown(self) -> bool {
        self != Page::Permissions || !glossa_platform::permissions::Permission::all().is_empty()
    }

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
            Page::Permissions => "options.permissions",
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
    /// What the last change to starting at login did, shown next to the
    /// checkbox: turned on or off, or the error.
    autostart_done: Option<Result<bool, String>>,
    /// Whether the system lets the entry run (macOS Login Items), asked on a
    /// worker when Options opens with it on and after turning it on.
    login_status: Option<glossa_platform::autostart::LoginItemStatus>,
    login_status_rx: Option<std::sync::mpsc::Receiver<glossa_platform::autostart::LoginItemStatus>>,
    message: Option<(String, bool)>,
    show_secrets: bool,
    /// A setting that applies at start-up changed: offer to restart.
    ask_restart: bool,
    /// The AI endpoint builder: the preset and name for the next endpoint.
    new_ai_preset: String,
    new_ai_name: String,
    /// Models loaded per provider id, for the model picker.
    ai_models: std::collections::HashMap<String, Vec<String>>,
    /// A running "load models" / "test" request, and the last outcome.
    ai_task: Option<std::sync::mpsc::Receiver<AiReply>>,
    ai_status: Option<(String, bool)>,
    /// A running check of a module's files, and the last outcome.
    assets_check: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    assets_note: Option<(String, bool)>,
    /// The installed font families, for the font picker.
    fonts: FontList,
    /// The Presets tab of the Services page is showing, and its state.
    presets_tab: bool,
    pub presets: crate::presets::PresetsPage,
}

/// The installed font families, listed on a worker the first time the font
/// picker opens.
#[derive(Default)]
struct FontList {
    names: Option<Vec<String>>,
    pending: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Open the picker at the next frame (`--screenshot options:font`).
    open_now: bool,
}

impl FontList {
    fn load(&mut self, ctx: &egui::Context) {
        if self.names.is_some() || self.pending.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(glossa_platform::fonts::families());
            ctx.request_repaint();
        });
        self.pending = Some(rx);
    }

    fn poll(&mut self) {
        if let Some(names) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.names = Some(names);
            self.pending = None;
        }
    }
}

/// The interface font: a button opening a searchable list of the installed
/// families, "Default" (the theme's font) first. Enter takes the first
/// match, or the typed name when nothing matches.
fn font_picker(app: &GlossaApp, ui: &mut egui::Ui, list: &mut FontList, value: &mut String) {
    use crate::dropdown;
    let id = ui.make_persistent_id("font-picker");
    let search_id = id.with("search");
    let default = app.t("options.basics.font_default");
    let shown = if value.trim().is_empty() { default.clone() } else { value.clone() };
    let button = dropdown::combo_button(ui, &shown, 240.0, dropdown::is_open(ui.ctx(), id));
    let for_screenshot = std::mem::take(&mut list.open_now);
    if button.clicked() || for_screenshot {
        if for_screenshot {
            // A screenshot's window may never get the focus.
            dropdown::toggle_unfocused(ui.ctx(), id);
        } else {
            dropdown::toggle(ui.ctx(), id);
        }
        ui.ctx().data_mut(|d| d.remove::<String>(search_id));
        list.load(ui.ctx());
    }
    list.poll();
    dropdown::show(ui, id, button.rect, 240.0, 460.0, |ui, settling| {
        let mut search: String = ui.ctx().data(|d| d.get_temp(search_id)).unwrap_or_default();
        let r = ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text(app.t("lang.search"))
                .desired_width(f32::INFINITY)
                .align(egui::Align2::LEFT_CENTER),
        );
        if !r.has_focus() {
            r.request_focus();
        }
        let needle = search.trim().to_lowercase();
        let mut picked: Option<String> = None;
        if needle.is_empty() && ui.selectable_label(value.trim().is_empty(), &default).clicked() {
            picked = Some(String::new());
        }
        // The list fills the dropdown under the search box, which stays put.
        let room = dropdown::list_room(ui);
        match &list.names {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label("…");
                });
            }
            Some(names) => {
                let matches: Vec<&String> =
                    names.iter().filter(|n| needle.is_empty() || n.to_lowercase().contains(&needle)).collect();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !needle.is_empty() {
                    picked = Some(matches.first().map(|n| (*n).clone()).unwrap_or_else(|| search.trim().to_owned()));
                }
                egui::ScrollArea::vertical().id_salt("font-list").max_height(room).show(ui, |ui| {
                    for n in matches {
                        let current = n.eq_ignore_ascii_case(value.trim());
                        let resp = ui.selectable_label(current, n.as_str());
                        if current && settling {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                        if resp.clicked() {
                            picked = Some(n.clone());
                        }
                    }
                });
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(search_id, search));
        if let Some(p) = picked {
            *value = p;
            ui.close();
        }
    });
}

/// What a background request from the AI endpoint settings returned.
enum AiReply {
    Models(String, Result<Vec<String>, String>),
    Test(Result<(String, std::time::Duration), String>),
}

/// Every page's short name (`basics`, `appearance`, `server`…), as
/// `--screenshot options:NAME` takes it, in the list's order.
pub fn page_names() -> Vec<&'static str> {
    Page::ALL.into_iter().map(|p| p.key().trim_start_matches("options.")).collect()
}

/// Open the options on the page whose key ends in `name` (`appearance`,
/// `ocr`…), for `--screenshot options:NAME`.
pub fn open_at(app: &mut GlossaApp, name: &str) {
    open(app);
    // Development aids for `--screenshot`: the font list open, or the note
    // shown once starting at login has been turned on (shown, not done).
    if let Some(st) = app.options.as_mut() {
        match name {
            "font" => st.fonts.open_now = true,
            "autostart" => {
                st.autostart = true;
                st.autostart_initial = true;
                st.autostart_done = Some(Ok(true));
            }
            "presets" => {
                st.page = Page::Services;
                st.presets_tab = true;
                return;
            }
            _ => {}
        }
    }
    let page = Page::ALL.into_iter().find(|p| p.key().ends_with(name));
    if let (Some(st), Some(page)) = (app.options.as_mut(), page) {
        st.page = page;
    }
}

/// Show provider `id` on the Services page, in the first group it belongs
/// to. A development aid for `--screenshot` (`GLOSSA_SCREENSHOT_SERVICE`).
pub fn select_service(app: &mut GlossaApp, id: &str) {
    let Some(info) = app.engine.registry().get(id).map(|p| p.info().clone()) else { return };
    let Some(st) = app.options.as_mut() else { return };
    let group = [Capabilities::TRANSLATE, Capabilities::DICTIONARY, Capabilities::LISTEN, Capabilities::OCR]
        .into_iter()
        .find(|c| info.capabilities.contains(*c));
    if let Some(group) = group {
        st.page = Page::Services;
        st.service_group = group;
        st.selected_service = Some(id.to_owned());
    }
}

pub fn open(app: &mut GlossaApp) {
    if app.options.is_some() {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(crate::chrome::OPTIONS));
    } else {
        let autostart = glossa_platform::autostart::is_autostart_enabled();
        app.options = Some(OptionsState {
            draft: app.engine.config(),
            page: Page::Basics,
            selected_service: None,
            service_group: Capabilities::TRANSLATE,
            exception_edit: None,
            autostart,
            autostart_initial: autostart,
            autostart_done: None,
            login_status: None,
            login_status_rx: autostart.then(|| ask_login_status(&app.ctx)).flatten(),
            message: None,
            show_secrets: false,
            ask_restart: false,
            new_ai_preset: "openrouter".into(),
            new_ai_name: String::new(),
            ai_models: Default::default(),
            ai_task: None,
            ai_status: None,
            assets_check: None,
            assets_note: None,
            fonts: FontList::default(),
            presets_tab: false,
            presets: Default::default(),
        });
    }
    app.ctx.request_repaint();
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.options.take() else { return };
    let title = app.t("options.title");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([900.0, 600.0]).with_min_inner_size([680.0, 420.0]);
    let mut open = true;
    let custom = crate::chrome::custom(&app.cfg);
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::OPTIONS), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        let _trace = crate::frametrace::begin(ui.ctx(), "options");
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        open &= body(app, ui, &mut st);
    });
    app.child_backdrop(&title, false);
    if open {
        app.options = Some(st);
    }
}

/// Save the draft and, when it changed, the autostart entry, and say how it
/// went at the bottom of the window. Returns true when the window should
/// stay open to show the result: starting at login was turned on (the note
/// under its checkbox says what the system may still ask) or failed.
fn apply(app: &mut GlossaApp, st: &mut OptionsState) -> bool {
    // The checkbox shows the system's own autostart entry; the setting
    // follows it, so the program can keep the entry pointing at itself.
    st.draft.general.start_with_system = st.autostart;
    let draft = st.draft.clone();
    let current = app.engine.config();
    if draft.appearance.renderer != current.appearance.renderer
        || crate::style::needs_transparent_root(&draft) != crate::style::needs_transparent_root(&current)
    {
        st.ask_restart = true;
    }
    let result = app.engine.update_config(move |c| {
        // The window keeps track of geometry and pane contents itself.
        let windows = std::mem::take(&mut c.windows);
        let contents = std::mem::take(&mut c.contents);
        *c = draft;
        c.windows = windows;
        c.contents = contents;
    });
    let mut attention = false;
    let mut autostart_error = None;
    if st.autostart != st.autostart_initial {
        let exe = std::env::current_exe().unwrap_or_default();
        match glossa_platform::autostart::set_autostart(st.autostart, &exe, &["--hidden".to_owned()]) {
            Ok(()) => {
                st.autostart_initial = st.autostart;
                st.autostart_done = Some(Ok(st.autostart));
                attention = st.autostart;
                st.login_status = None;
                st.login_status_rx = if st.autostart { ask_login_status(&app.ctx) } else { None };
            }
            Err(e) => {
                autostart_error = Some(app.locale.fmt("options.basics.autostart_failed", &e.to_string()));
                st.autostart_done = Some(Err(e.to_string()));
                attention = true;
            }
        }
    }
    st.message = Some(match result {
        Ok(()) => {
            st.draft = app.engine.config();
            match autostart_error {
                Some(e) => (e, true),
                None => (app.t("options.saved"), false),
            }
        }
        Err(e) => (e.to_string(), true),
    });
    if attention {
        // The note is under the checkbox on the first page.
        st.page = Page::Basics;
    }
    attention
}

/// Returns false when the dialog should close.
fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) -> bool {
    let mut keep = true;
    let look = app.look.clone();
    egui::Panel::bottom("opt-buttons").show(ui, |ui| {
        ui.add_space(6.0);
        if st.ask_restart {
            ui.horizontal(|ui| {
                ui.label(RichText::new(app.t("options.restart_needed")).strong());
                if ui.button(app.t("options.restart_now")).clicked() {
                    app.restart();
                }
                if ui.button(app.t("options.restart_later")).clicked() {
                    st.ask_restart = false;
                }
            });
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            if let Some((m, err)) = &st.message {
                ui.label(RichText::new(m).color(if *err { look.error() } else { look.weak_text() }));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let w = vec2(100.0, 28.0);
                if ui.add(egui::Button::new(app.t("options.apply")).min_size(w)).clicked() {
                    apply(app, st);
                }
                if ui.add(egui::Button::new(app.t("options.cancel")).min_size(w)).clicked() {
                    keep = false;
                }
                if ui.add(egui::Button::new(app.t("options.ok")).min_size(w)).clicked() {
                    // Stays open when starting at login changed in a way the
                    // user has to see (the note and its button, or an error);
                    // a second OK closes it.
                    keep = apply(app, st);
                }
            });
        });
        ui.add_space(6.0);
    });
    egui::Panel::left("opt-nav").exact_size(214.0).resizable(false).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("opt-nav-scroll").show(ui, |ui| {
            ui.add_space(4.0);
            for g in Group::ALL {
                let pages: Vec<Page> = Page::ALL.into_iter().filter(|p| p.group() == g && p.shown()).collect();
                if pages.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(RichText::new(app.t(g.key())).size(11.0).strong().color(look.weak_text()));
                for p in pages {
                    if nav_row(ui, &look, p, &app.t(p.key()), st.page == p).clicked() {
                        st.page = p;
                    }
                }
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        page_header(ui, &look, st.page, &app.t(st.page.key()));
        if st.page == Page::Services {
            // Two panes with their own scrolling.
            services(app, ui, st);
            return;
        }
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
                Page::Permissions => permissions(app, ui),
                Page::Updates => updates(app, ui, st),
            }
        });
    });
    keep
}

/// A page's icon: white on a rounded square of the page's colour.
fn page_icon(ui: &mut egui::Ui, page: Page, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same((size * 0.26) as u8), page.tint());
    let glyph = egui::Rect::from_center_size(rect.center(), vec2(size * 0.62, size * 0.62));
    crate::icons::paint(ui.painter(), glyph, page.icon(), Color32::WHITE);
}

/// One page in the list: its icon and name. The chosen one is drawn as the
/// theme's system draws it: on the accent with Mac-style controls (System
/// Settings), as a faint plate with an accent bar at its left otherwise
/// (WinUI's NavigationView), where a light accent under light text would
/// not read.
fn nav_row(ui: &mut egui::Ui, look: &crate::style::Look, page: Page, name: &str, chosen: bool) -> egui::Response {
    let height = 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::click());
    let radius = egui::CornerRadius::same(7);
    let text = look.text();
    let veil = |a: u8| Color32::from_rgba_unmultiplied(text.r(), text.g(), text.b(), a);
    let mac = look.theme.metrics.controls == glossa_core::theme::ControlStyle::Mac;
    let mut colour = text;
    if chosen && mac {
        ui.painter().rect_filled(rect, radius, look.accent());
        colour = crate::style::text_on(look.accent());
    } else if chosen {
        ui.painter().rect_filled(rect, radius, veil(22));
        let bar = egui::Rect::from_center_size(egui::pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 16.0));
        ui.painter().rect_filled(bar, egui::CornerRadius::same(2), look.accent());
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, radius, veil(14));
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    page_icon(&mut child, page, 20.0);
    child.add_space(4.0);
    let label = RichText::new(name).color(colour);
    let label = if chosen && !mac { label.strong() } else { label };
    child.add(egui::Label::new(label).selectable(false).truncate());
    resp
}

/// The page's icon and name over its content.
fn page_header(ui: &mut egui::Ui, look: &crate::style::Look, page: Page, name: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        page_icon(ui, page, 30.0);
        ui.add_space(4.0);
        ui.label(RichText::new(name).size(20.0).strong().color(look.text()));
    });
    ui.add_space(10.0);
}

/// A list of the installed themes for `value`.
fn theme_picker(app: &GlossaApp, ui: &mut egui::Ui, salt: &str, value: &mut String) {
    let entries = app.engine.theme_entries();
    crate::dropdown::ComboBox::from_id_salt(salt).width(260.0).selected_text(value.as_str()).show_ui(ui, |ui| {
        for t in &entries {
            let label =
                if t.classic { app.locale.fmt("options.appearance.classic_theme", &t.name) } else { t.name.clone() };
            if ui.selectable_label(t.name == *value, label).clicked() {
                *value = t.name.clone();
            }
        }
    });
}

/// How the draft will look, above the appearance settings.
fn appearance_preview(app: &GlossaApp, ui: &mut egui::Ui, look: &crate::style::Look, draft: &Config) {
    let system_dark = app.engine.system_dark();
    let theme = app.engine.theme(draft.appearance.effective_theme(system_dark));
    let preview = crate::style::Look::new(theme, draft);
    let to = crate::langs::name(draft.general.first_language);
    let from = crate::langs::name(draft.general.second_language);
    let labels = crate::preview::Labels {
        source: app.t("options.appearance.preview_source"),
        translation: app.t("options.appearance.preview_translation"),
        popup_title: format!("{from} \u{2192} {to} (Google)"),
        from,
        to,
        translate: app.t("main.translate"),
    };
    group(ui, look, &app.t("options.appearance.preview"), |ui| {
        crate::preview::show(ui, &preview, draft, 230.0, &labels);
    });
    ui.add_space(6.0);
}

fn lang_row(app: &GlossaApp, ui: &mut egui::Ui, label: &str, salt: &str, value: &mut LangId, allow_auto: bool) {
    let auto = app.t("main.auto_detect");
    row(ui, label, |ui| {
        if let Some(l) =
            crate::widgets::lang_combo(ui, salt, *value, allow_auto, &auto, &app.t("lang.search"), &[], 240.0)
        {
            *value = l;
        }
    });
}

/// The interface language `setting` names (a code, a classic folder name or
/// an English name), or `None` for an empty one: follow the system.
/// Built-in packs have no folder name, so an empty setting must not be
/// compared with it.
fn chosen_locale<'a>(
    locales: &'a [glossa_engine::catalog::LocaleEntry],
    setting: &str,
) -> Option<&'a glossa_engine::catalog::LocaleEntry> {
    let setting = setting.trim();
    if setting.is_empty() {
        return None;
    }
    locales.iter().find(|l| {
        l.code.eq_ignore_ascii_case(setting)
            || (!l.folder.is_empty() && l.folder.eq_ignore_ascii_case(setting))
            || l.name.eq_ignore_ascii_case(setting)
    })
}

/// The user's language folder, a rescan (so a pack dropped there, or a
/// string fixed in one, shows without a restart), and what was wrong in the
/// files.
fn language_files(app: &GlossaApp, ui: &mut egui::Ui, look: &crate::style::Look) {
    ui.horizontal(|ui| {
        if ui.button(app.t("options.basics.ui_language_folder")).clicked() {
            let dir = app.engine.paths().user_locale_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = crate::app::open_with_system(&dir.to_string_lossy());
        }
        if ui.button(app.t("options.basics.ui_language_reload")).clicked() {
            // The rescan reads every theme and locale folder, so it runs off
            // the UI thread; the engine's ConfigChanged brings the result.
            let engine = app.engine.clone();
            let _ = std::thread::Builder::new().name("glossa-rescan".into()).spawn(move || engine.refresh_catalogs());
        }
    });
    let issues = app.engine.locale_issues();
    if issues.is_empty() {
        return;
    }
    ui.label(RichText::new(app.t("options.basics.ui_language_problems")).color(look.weak_text()));
    for issue in &issues {
        let colour = match issue.severity() {
            glossa_core::locale::Severity::Error => look.error(),
            glossa_core::locale::Severity::Warning => look.weak_text(),
        };
        // The file's name is enough here; the log has the full path.
        let text = match issue.source(&app.locale) {
            Some(f) => format!("{f}: {}", issue.message(&app.locale)),
            None => issue.message(&app.locale),
        };
        ui.label(RichText::new(text).small().color(colour));
    }
}

fn basics(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.basics.general_group"), |ui| {
        ui.checkbox(&mut st.autostart, app.t(glossa_platform::shell::SHELL.autostart_label));
        if let Some(status) = st.login_status_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            st.login_status = Some(status);
            st.login_status_rx = None;
        }
        autostart_feedback(
            app,
            ui,
            &look,
            st.autostart != st.autostart_initial,
            st.autostart_done.as_ref(),
            st.login_status,
        );
        row(ui, &app.t("options.basics.ui_language"), |ui| {
            let locales = app.engine.locales();
            let current = chosen_locale(&locales, &d.general.ui_language)
                .map(|l| format!("{} - {}", l.name, l.native_name))
                .unwrap_or_else(|| app.t("options.basics.ui_language_system"));
            crate::dropdown::ComboBox::from_id_salt("ui-lang").width(240.0).selected_text(current).show_ui(ui, |ui| {
                if ui
                    .selectable_label(d.general.ui_language.is_empty(), app.t("options.basics.ui_language_system"))
                    .clicked()
                {
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
        language_files(app, ui, &look);
        row(ui, &app.t("options.basics.font"), |ui| {
            font_picker(app, ui, &mut st.fonts, &mut d.appearance.font_family);
        });
        row(ui, &app.t("options.basics.text_size"), |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.text_size, 7.0..=28.0).step_by(0.5));
        });
    });
    group(ui, &look, &app.t("options.basics.privacy_group"), |ui| {
        use glossa_core::secrets::SecretStorage;
        let keychain = app.engine.secret_store_name();
        ui.label(app.t("options.basics.secret_storage"));
        ui.radio_value(&mut d.general.secret_storage, SecretStorage::File, app.t("options.basics.secrets_file"));
        // Kept selectable when already chosen, so a missing keychain can be
        // seen and switched away from.
        ui.add_enabled_ui(keychain.is_some() || d.general.secret_storage == SecretStorage::System, |ui| {
            ui.radio_value(
                &mut d.general.secret_storage,
                SecretStorage::System,
                app.locale.fmt("options.basics.secrets_system", glossa_platform::secrets::system_store_name()),
            );
        });
        let note = if keychain.is_some() { "options.basics.secrets_note" } else { "options.basics.secrets_none" };
        ui.label(RichText::new(app.t(note)).small().color(look.weak_text()));
        ui.add_space(4.0);
        if ui.button(app.t("options.basics.show_welcome")).clicked() {
            crate::welcome::open(app);
        }
    });
    group(ui, &look, &app.t("options.basics.auto_detect_group"), |ui| {
        lang_row(app, ui, &app.t("options.basics.first_language"), "first", &mut d.general.first_language, false);
        lang_row(app, ui, &app.t("options.basics.second_language"), "second", &mut d.general.second_language, false);

        ui.checkbox(&mut d.general.smart_direction, app.t("options.basics.smart_direction"));
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
    // On Wayland the desktop portal may bind the hotkeys; without it, point
    // at a desktop shortcut instead.
    let portal = app.hotkeys.as_ref().is_some_and(|h| h.uses_portal());
    if app.hotkeys.is_none() || (app.session.is_wayland() && !portal) {
        let token = &d.server.token;
        let cmd = format!("curl -s 'http://{}/v1/action/show_popup?token={token}'", d.server.bind);
        ui.label(RichText::new(app.t("options.hotkeys.wayland")).color(app.look.weak_text()));
        ui.horizontal(|ui| {
            ui.code(&cmd);
            if crate::icons::sized_button(ui, &app.look, crate::icons::Icon::Copy, &app.t("edit.copy"), 18.0, true)
                .clicked()
            {
                ui.ctx().copy_text(cmd.clone());
            }
        });
    }
    for (action, e) in &app.hotkey_errors {
        let text = crate::hotkey_error_text(&app.locale, *action, e);
        ui.label(RichText::new(text).color(app.look.error()));
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
                        let names: Vec<String> = others
                            .iter()
                            .filter(|a| **a != action)
                            .map(|a| app.t(&format!("action.{}", a.id())))
                            .collect();
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
        row(ui, &app.t("options.internet.retries"), |ui| {
            ui.add(egui::DragValue::new(&mut n.max_retries).range(0..=5));
        });
    });
    group(ui, &look, &app.t("options.internet.proxy"), |ui| {
        let p = &mut n.proxy;
        let names = [
            (ProxyMode::System, app.t("options.internet.proxy_system")),
            (ProxyMode::None, app.t("options.internet.proxy_none")),
            (ProxyMode::Manual, app.t("options.internet.proxy_manual")),
        ];
        // "Automatic" (the classic choice) finds the proxy the way "System" does,
        // so it is shown as that.
        let shown = if p.mode == ProxyMode::Auto { ProxyMode::System } else { p.mode };
        let current = names.iter().find(|(m, _)| *m == shown).map(|(_, n)| n.clone()).unwrap_or_default();
        crate::dropdown::ComboBox::from_id_salt("proxy-mode")
            .width(ui.available_width())
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (m, name) in &names {
                    ui.selectable_value(&mut p.mode, *m, name);
                }
            });
        ui.add_enabled_ui(p.mode == ProxyMode::Manual, |ui| {
            ui.horizontal(|ui| {
                crate::dropdown::ComboBox::from_id_salt("proxy-scheme")
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
            ui.label(RichText::new(app.t("options.internet.password_plain")).small().color(look.weak_text()));
        });
    });
}

fn services(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    ui.horizontal(|ui| {
        for (cap, key) in [
            (Capabilities::TRANSLATE, "options.services.translation"),
            (Capabilities::DICTIONARY, "options.services.dictionaries"),
            (Capabilities::LISTEN, "options.services.speech"),
            (Capabilities::OCR, "options.services.ocr"),
        ] {
            if ui.selectable_label(!st.presets_tab && st.service_group == cap, app.t(key)).clicked() {
                st.service_group = cap;
                st.selected_service = None;
                st.presets_tab = false;
            }
        }
        if ui.selectable_label(st.presets_tab, app.t("options.services.presets")).clicked() {
            st.presets_tab = true;
        }
    });
    ui.separator();
    if st.presets_tab {
        crate::presets::page(app, ui, &mut st.draft, &mut st.presets);
        return;
    }
    poll_ai_task(app, st);
    let providers = app.engine.providers(st.service_group);
    // Two panes that use the whole window: the list on the left (its width
    // dragged by the divider), the selected service's settings on the right.
    egui::Panel::left("svc-list-pane").resizable(true).default_size(320.0).show(ui, |ui| {
        service_list(app, ui, st, &providers);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(("svc-detail", st.service_group.0)).show(ui, |ui| {
            ui.set_width(ui.available_width() - 8.0);
            service_details(app, ui, st, &providers);
        });
    });
}

/// The providers with the current capability: switch them on, reorder them,
/// pick one to configure.
fn service_list(
    app: &mut GlossaApp,
    ui: &mut egui::Ui,
    st: &mut OptionsState,
    providers: &[glossa_engine::ProviderSummary],
) {
    let look = app.look.clone();
    let cap = st.service_group;
    let d = &mut st.draft;
    let (order, enabled): (&mut Vec<String>, &mut Vec<String>) = if cap == Capabilities::DICTIONARY {
        (&mut d.services.dictionary_order, &mut d.services.dictionary_enabled)
    } else {
        (&mut d.services.order, &mut d.services.enabled)
    };
    // Show providers in the draft's order.
    let mut list: Vec<_> = providers.to_vec();
    list.sort_by_key(|p| order.iter().position(|o| *o == p.info.id).unwrap_or(usize::MAX));
    let selectable_tabs = cap == Capabilities::TRANSLATE || cap == Capabilities::DICTIONARY;

    // Buttons and switches stay at the bottom; the list takes the rest.
    egui::Panel::bottom("svc-list-buttons").show(ui, |ui| {
        if !selectable_tabs {
            return;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let sel = st.selected_service.clone();
            let up = crate::icons::sized_button(ui, &look, crate::icons::Icon::Up, "", 22.0, sel.is_some()).clicked();
            let down =
                crate::icons::sized_button(ui, &look, crate::icons::Icon::Down, "", 22.0, sel.is_some()).clicked();
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
                        .add_filter(
                            app.t("options.services.dictionaries"),
                            &["ifo", "dsl", "dz", "xdxf", "tsv", "txt", "csv"],
                        )
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
        if cap == Capabilities::DICTIONARY {
            for f in &d.dictionary.offline {
                ui.label(RichText::new(f.display().to_string()).small().color(look.weak_text()));
            }
        }
        if cap == Capabilities::TRANSLATE {
            ui.checkbox(&mut d.services.fail_over, app.t("options.services.fail_over"));
        }
        ui.add_space(2.0);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(("svc-list", cap.0)).auto_shrink([false, false]).show(ui, |ui| {
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
                    ui.label(RichText::new(origin_label(app, &p.origin)).small().color(look.weak_text()));
                    let user = d.services.options.get(&p.info.id);
                    let has = user.is_some_and(|o| o.values().any(|v| v.as_str().is_some_and(|s| !s.is_empty())));
                    if p.info.needs_credentials && !has {
                        ui.label(RichText::new(app.t("options.services.needs_key")).small().color(look.error()));
                    } else if user.is_some_and(|u| !u.is_empty()) && !p.page.errors(user).is_empty() {
                        // A field fails its checks: saved anyway (the user
                        // may be half-way through), but marked.
                        ui.label(RichText::new(app.t("options.services.check_settings")).small().color(look.error()));
                    }
                    if cap == Capabilities::LISTEN && ui.radio(d.services.tts == p.info.id, "").clicked() {
                        d.services.tts = p.info.id.clone();
                    }
                    if cap == Capabilities::OCR && ui.radio(d.ocr.provider == p.info.id, "").clicked() {
                        d.ocr.provider = p.info.id.clone();
                    }
                });
            }
        });
    });
}

/// The right pane: the AI endpoint builder (for translators) and the
/// selected provider's settings, generated from what it declares.
fn service_details(
    app: &mut GlossaApp,
    ui: &mut egui::Ui,
    st: &mut OptionsState,
    providers: &[glossa_engine::ProviderSummary],
) {
    let look = app.look.clone();
    if st.service_group == Capabilities::TRANSLATE {
        ai_endpoints(app, ui, st);
    }
    let Some(id) = st.selected_service.clone() else {
        ui.add_space(8.0);
        ui.label(RichText::new(app.t("options.services.pick_one")).color(look.weak_text()));
        return;
    };
    let Some(p) = providers.iter().find(|p| p.info.id == id) else { return };
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
            RichText::new(format!(
                "{} {}",
                app.t("options.services.capabilities"),
                p.info
                    .capabilities
                    .names()
                    .iter()
                    .map(|n| app.t(&format!("capability.{n}")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .small()
            .color(look.weak_text()),
        );
        module_assets(app, ui, st, &id);
        module_python(app, ui, &id);
        if p.options.is_empty() {
            ui.label(RichText::new(app.t("options.services.no_settings")).color(look.weak_text()));
        } else {
            ui.checkbox(&mut st.show_secrets, app.t("options.services.show_keys"));
            if is_ai_endpoint(&id) {
                ai_tools(app, ui, &id, &mut st.draft, &st.ai_models, &mut st.ai_task, &st.ai_status);
            }
            let values = st.draft.services.options.entry(id.clone()).or_default();
            crate::module_settings::show(ui, &look, &app.locale, &p.page, values, st.show_secrets, &id);
        }
    });
}

/// The files a module declares (models, dictionaries): where each stands,
/// the download with its progress, and checking or deleting them. The work
/// runs in the engine on worker threads; this only draws and dispatches.
fn module_assets(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState, id: &str) {
    let Ok(report) = app.engine.module_assets(id) else { return };
    if let Some(rx) = &st.assets_check {
        if let Ok(r) = rx.try_recv() {
            st.assets_note = Some(match r {
                Ok(m) => (m, false),
                Err(m) => (m, true),
            });
            st.assets_check = None;
        }
    }
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.label(RichText::new(app.t("options.assets.title")).strong());
    ui.label(RichText::new(app.t("options.assets.hint")).small().color(look.weak_text()));
    let running = report.job.as_ref().is_some_and(|j| j.running);
    let mut fetch_one = None;
    for a in &report.assets {
        let (state, bad) = match &a.state {
            glossa_engine::AssetState::Ready => (app.t("options.assets.ready"), false),
            glossa_engine::AssetState::Missing => (app.t("options.assets.missing"), !a.optional),
            glossa_engine::AssetState::Partial { have } => {
                (app.locale.fmt("options.assets.partial", &human_size(*have)), !a.optional)
            }
            glossa_engine::AssetState::Outdated => (app.t("options.assets.outdated"), true),
            glossa_engine::AssetState::Corrupt => (app.t("options.assets.corrupt"), true),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&a.name).small());
            ui.label(RichText::new(human_size(a.size)).small().color(look.weak_text()));
            if a.optional {
                ui.label(RichText::new(app.t("options.assets.optional")).small().color(look.weak_text()));
            }
            ui.label(RichText::new(state).small().color(if bad { look.error() } else { look.weak_text() }));
            // Optional files (one model of many) are fetched one by one.
            if a.optional
                && !report.busy
                && a.state != glossa_engine::AssetState::Ready
                && ui.small_button(app.t("options.assets.get")).clicked()
            {
                fetch_one = Some(a.name.clone());
            }
        });
    }
    if let Some(name) = fetch_one {
        st.assets_note = None;
        if let Err(e) = app.engine.start_asset_download(id, &[name]) {
            st.assets_note = Some((e.to_string(), true));
        }
    }
    if let Some(job) = &report.job {
        if job.running {
            let p = &job.progress;
            let fraction = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
            let text = app.locale.fmt_args("options.assets.progress", &[&human_size(p.done), &human_size(p.total)]);
            ui.add(egui::ProgressBar::new(fraction).text(text).desired_width(320.0));
        } else if !job.error.is_empty() {
            ui.label(RichText::new(&job.error).small().color(look.error()));
        }
    }
    ui.horizontal_wrapped(|ui| {
        // A delete runs as a job too, with nothing to count and nothing
        // to cancel.
        let downloading = running && report.job.as_ref().is_some_and(|j| j.progress.total > 0);
        if downloading {
            if ui.button(app.t("options.assets.cancel")).clicked() {
                app.engine.cancel_asset_download(id);
            }
        } else if report.busy {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new(app.t("options.assets.busy")).small().color(look.weak_text()));
        } else if !report.ready {
            let label = app.locale.fmt("options.assets.download", &human_size(report.total_size));
            if ui.button(label).clicked() {
                st.assets_note = None;
                if let Err(e) = app.engine.start_asset_download(id, &[]) {
                    st.assets_note = Some((e.to_string(), true));
                }
            }
        }
        let checking = st.assets_check.is_some();
        if report.ready
            && !report.busy
            && ui.add_enabled(!checking, egui::Button::new(app.t("options.assets.verify"))).clicked()
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let (engine, ctx, id, ok) =
                (app.engine.clone(), app.ctx.clone(), id.to_owned(), app.t("options.assets.verified"));
            std::thread::spawn(move || {
                let r = engine.verify_module_assets(&id).map_err(|e| e.to_string()).and_then(|r| {
                    if r.assets.iter().all(|a| a.state == glossa_engine::AssetState::Ready) {
                        Ok(ok)
                    } else {
                        let bad: Vec<&str> = r
                            .assets
                            .iter()
                            .filter(|a| a.state != glossa_engine::AssetState::Ready)
                            .map(|a| a.name.as_str())
                            .collect();
                        Err(bad.join(", "))
                    }
                });
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            st.assets_check = Some(rx);
        }
        if checking {
            ui.add(egui::Spinner::new().size(14.0));
        }
        if ui.button(app.t("options.assets.open_folder")).clicked() {
            let _ = std::fs::create_dir_all(&report.dir);
            let _ = crate::app::open_with_system(&report.dir.to_string_lossy());
        }
        let any_on_disk = report.assets.iter().any(|a| a.state != glossa_engine::AssetState::Missing);
        if any_on_disk && ui.add_enabled(!report.busy, egui::Button::new(app.t("options.assets.remove"))).clicked() {
            st.assets_note = None;
            if let Err(e) = app.engine.start_asset_removal(id) {
                st.assets_note = Some((e.to_string(), true));
            }
        }
    });
    if let Some((text, error)) = &st.assets_note {
        let color = if *error { look.error() } else { look.weak_text() };
        ui.label(RichText::new(text).small().color(color));
    }
    ui.add_space(4.0);
}

/// A Python module's interpreter and packages when something is missing:
/// what, the download with its progress, and deleting its environment. The
/// work runs in the engine on worker threads; this only draws and
/// dispatches.
fn module_python(app: &mut GlossaApp, ui: &mut egui::Ui, id: &str) {
    // Cached in the engine and refreshed when it reports a change, so
    // drawing this every frame reads nothing from disk.
    let Some(m) = app.engine.module_python(id) else { return };
    let look = app.look.clone();
    let failed = m.job.as_ref().is_some_and(|j| !j.running && !j.error.is_empty());
    let has_venv = m.venv_exists;
    if m.state == "ready" && !failed && !has_venv && !m.busy {
        return;
    }
    if m.state == "unknown" {
        // The search for a system Python runs; the page redraws when it ends.
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new(app.t("options.python.title")).strong());
    if let Some(req) = &m.requirements {
        ui.label(RichText::new(app.locale.fmt("options.python.requirements", req)).small().color(look.weak_text()));
    }
    match m.state {
        "ready" => {
            ui.label(RichText::new(app.t("options.python.ready")).small().color(look.weak_text()));
        }
        "impossible" => {
            ui.label(RichText::new(m.missing.join("\n")).small().color(look.error()));
        }
        _ => {
            ui.label(RichText::new(app.t("options.python.hint")).small().color(look.weak_text()));
            for line in &m.missing {
                ui.label(RichText::new(line).small().color(look.error()));
            }
        }
    }
    if let Some(job) = &m.job {
        if job.running {
            let p = &job.progress;
            if p.total > 0 {
                let fraction = p.done as f32 / p.total as f32;
                let text = app.locale.fmt_args("options.assets.progress", &[&human_size(p.done), &human_size(p.total)]);
                ui.add(egui::ProgressBar::new(fraction).text(text).desired_width(320.0));
            } else {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(app.t("options.python.installing")).small().color(look.weak_text()));
                });
            }
        } else if !job.error.is_empty() {
            ui.label(RichText::new(&job.error).small().color(look.error()));
        }
    }
    ui.horizontal_wrapped(|ui| {
        let running = m.job.as_ref().is_some_and(|j| j.running);
        if running {
            if ui.button(app.t("options.assets.cancel")).clicked() {
                app.engine.cancel_python_install(id);
            }
        } else if m.busy {
            ui.add(egui::Spinner::new().size(14.0));
        } else if m.state == "missing" {
            let label = if m.download_size > 0 {
                app.locale.fmt("options.assets.download", &human_size(m.download_size))
            } else {
                app.t("options.python.install")
            };
            if ui.button(label).clicked() {
                if let Err(e) = app.engine.start_python_install(Some(id)) {
                    app.notify(e.to_string(), true);
                }
            }
        }
        if has_venv && !m.busy && ui.button(app.t("options.python.remove")).clicked() {
            let (engine, id) = (app.engine.clone(), id.to_owned());
            std::thread::spawn(move || {
                if let Err(e) = engine.remove_module_python(&id) {
                    tracing::warn!(module = %id, "could not delete the module's Python environment: {e}");
                }
            });
        }
    });
    ui.add_space(4.0);
}

/// A byte count for people: 512 B, 4.5 MB, 1.2 GB.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for u in &UNITS[1..] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    if v < 10.0 {
        format!("{v:.1} {unit}")
    } else {
        format!("{v:.0} {unit}")
    }
}

/// A backdrop picker, greyed out where backdrops cannot show.
fn backdrop_row(
    app: &GlossaApp,
    ui: &mut egui::Ui,
    label: &str,
    salt: &str,
    value: &mut glossa_core::config::Backdrop,
) {
    use glossa_core::config::Backdrop;
    let supported = crate::style::backdrops_supported(&app.engine.config());
    let keys = glossa_platform::shell::SHELL.backdrop_names;
    let names = [
        (Backdrop::None, app.t("options.backdrop.none")),
        (Backdrop::Mica, app.t(keys.mica)),
        (Backdrop::MicaAlt, app.t(keys.mica_alt)),
        (Backdrop::Acrylic, app.t(keys.acrylic)),
        (Backdrop::Blur, app.t(keys.blur)),
    ];
    row(ui, label, |ui| {
        ui.add_enabled_ui(supported, |ui| {
            let cur = names.iter().find(|(b, _)| b == value).map(|(_, n)| n.clone()).unwrap_or_default();
            crate::dropdown::ComboBox::from_id_salt(salt).width(200.0).selected_text(cur).show_ui(ui, |ui| {
                for (b, n) in &names {
                    ui.selectable_value(value, *b, n);
                }
            });
        })
        .response
        .on_disabled_hover_text(app.t("options.backdrop.needs_opengl"));
    });
}

/// The backdrop opacity slider under a backdrop choice, with what it does.
fn backdrop_opacity_row(app: &GlossaApp, ui: &mut egui::Ui, look: &crate::style::Look, enabled: bool, value: &mut u8) {
    ui.add_enabled_ui(enabled, |ui| {
        row(ui, &app.t("options.appearance.backdrop_opacity"), |ui| {
            ui.add(egui::Slider::new(value, 0..=100).suffix(" %"));
        });
        hint(ui, look, &app.t(glossa_platform::shell::SHELL.backdrop_names.opacity_hint));
    });
}

/// Under the "start at login" checkbox: that the change waits for OK or
/// Apply, then what it did, in a box that stands out from the hints. On
/// macOS the system may want Glossa allowed in Login Items, so there is a
/// button that opens them.
fn autostart_feedback(
    app: &GlossaApp,
    ui: &mut egui::Ui,
    look: &crate::style::Look,
    pending: bool,
    done: Option<&Result<bool, String>>,
    status: Option<glossa_platform::autostart::LoginItemStatus>,
) {
    use glossa_platform::autostart::LoginItemStatus;
    let shell = glossa_platform::shell::SHELL;
    if pending {
        hint(ui, look, &app.t("options.basics.autostart_pending"));
        return;
    }
    let needs_approval = status == Some(LoginItemStatus::RequiresApproval);
    let (text, colour, login_items) = match done {
        Some(Err(e)) => (app.locale.fmt("options.basics.autostart_failed", e), look.error(), shell.login_items_url),
        Some(Ok(false)) => (app.t("options.basics.autostart_off"), look.accent(), None),
        // On, but the system keeps it from running: said whether or not it
        // was turned on just now, since the checkbox alone looks fine.
        _ if needs_approval => (app.t("options.basics.autostart_needs_approval"), look.error(), shell.login_items_url),
        Some(Ok(true)) if status == Some(LoginItemStatus::Enabled) => {
            (app.t("options.basics.autostart_allowed_macos"), look.accent(), shell.login_items_url)
        }
        Some(Ok(true)) => (app.t(shell.autostart_note), look.accent(), shell.login_items_url),
        None => return,
    };
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, colour.gamma_multiply(0.6)))
        .corner_radius(look.theme.metrics.button_radius.clamp(2.0, 8.0) as u8)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(560.0));
            ui.label(RichText::new(text).color(look.text()));
            if let Some(url) = login_items {
                ui.add_space(4.0);
                if ui.button(app.t("options.basics.open_login_items")).clicked() {
                    if let Err(e) = glossa_platform::shell::open(url) {
                        tracing::warn!("opening Login Items: {e}");
                    }
                }
            }
        });
}

/// Ask on a worker whether the system lets the autostart entry run, where it
/// can tell (macOS: Login Items). A new launch agent takes macOS a moment to
/// notice, so it asks a few times before settling for what it heard.
fn ask_login_status(
    ctx: &egui::Context,
) -> Option<std::sync::mpsc::Receiver<glossa_platform::autostart::LoginItemStatus>> {
    use glossa_platform::autostart::{login_item_status, LoginItemStatus};
    glossa_platform::shell::SHELL.login_items_url?;
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let mut status = LoginItemStatus::Unknown;
        for attempt in 0..6 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            status = login_item_status();
            if !matches!(status, LoginItemStatus::NotRegistered) {
                break;
            }
        }
        let _ = tx.send(status);
        ctx.request_repaint();
    });
    Some(rx)
}

/// A line of quiet text explaining the setting above it.
fn hint(ui: &mut egui::Ui, look: &crate::style::Look, text: &str) {
    ui.label(RichText::new(text).small().color(look.weak_text()));
}

/// Where a provider comes from, in the interface language. Module runtime
/// names ("JavaScript", "Python") are left as they are.
fn origin_label(app: &GlossaApp, origin: &str) -> String {
    match origin {
        "Built-in" => app.t("options.services.origin_builtin"),
        "Bundled" => app.t("options.services.origin_bundled"),
        other => other.to_owned(),
    }
}

fn is_ai_endpoint(id: &str) -> bool {
    glossa_engine::ai::is_endpoint(id)
}

/// The AI endpoint builder: the endpoints the user added, and a row to add
/// one from a preset. Adding and removing apply at once (a provider has to
/// exist before its settings can be shown); renaming waits for Apply.
fn ai_endpoints(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    use glossa_engine::ai::{preset, PRESETS};
    let look = app.look.clone();
    ui.add_space(6.0);
    group(ui, &look, &app.t("options.ai.title"), |ui| {
        ui.label(RichText::new(app.t("options.ai.hint")).small().color(look.weak_text()));
        let mut remove = None;
        for e in st.draft.services.ai_endpoints.iter_mut() {
            ui.horizontal_wrapped(|ui| {
                crate::widgets::service_badge(ui, &e.id, &e.name, 18.0);
                ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(200.0));
                ui.label(RichText::new(preset(&e.preset).name).small().color(look.weak_text()));
                if ui.button(app.t("options.services.settings")).clicked() {
                    st.selected_service = Some(e.id.clone());
                }
                if ui.button(app.t("options.remove")).clicked() {
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
        // Wrapped: the pane can be narrower than the row, and a row wider
        // than the pane would widen the whole settings column.
        ui.horizontal_wrapped(|ui| {
            crate::dropdown::ComboBox::from_id_salt("ai-preset")
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
            if ui.button(app.t("options.ai.add")).clicked() {
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
        if ui.add_enabled(!busy, egui::Button::new(app.t("options.ai.load_models"))).clicked() {
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
        if ui
            .add_enabled(!busy, egui::Button::new(app.t("options.ai.test")))
            .on_hover_text(app.t("options.ai.test_tip"))
            .clicked()
        {
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
        // The saved value, else the preset's default shown in the field below.
        let default = app
            .engine
            .registry()
            .get(id)
            .and_then(|p| {
                p.options().iter().find(|o| o.key == "model").and_then(|o| o.default.as_str().map(str::to_owned))
            })
            .unwrap_or_default();
        let current = values.get("model").and_then(|v| v.as_str()).map(str::to_owned).unwrap_or(default);
        ui.horizontal(|ui| {
            ui.label(app.locale.fmt("options.ai.models", &list.len().to_string()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::dropdown::ComboBox::from_id_salt(("ai-models", id))
                    .width(320.0)
                    .height(360.0)
                    .selected_text(if current.is_empty() { app.t("options.ai.pick_model") } else { current.clone() })
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
            st.ai_status = Some((app.locale.fmt("options.ai.models_loaded", &list.len().to_string()), false));
            st.ai_models.insert(id, list);
            st.ai_task = None;
        }
        Ok(AiReply::Test(Ok((text, took)))) => {
            let ms = took.as_millis().to_string();
            st.ai_status = Some((app.locale.fmt_args("options.ai.test_ok", &[&ms, &text]), false));
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

fn languages(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let d = &mut st.draft.languages;
    let mut langs: Vec<LangId> = LangId::all().collect();
    crate::langs::sort(&mut langs);
    let per_col = langs.len().div_ceil(4);
    egui::Grid::new("langs").num_columns(4).spacing(vec2(18.0, 2.0)).show(ui, |ui| {
        for r in 0..per_col {
            for col in 0..4 {
                if let Some(l) = langs.get(col * per_col + r) {
                    let mut on = !d.disabled.contains(l);
                    if ui.checkbox(&mut on, crate::langs::name(*l)).on_hover_text(l.native_name()).changed() {
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
    appearance_preview(app, ui, &look, &st.draft);
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.appearance.general"), |ui| {
        ui.checkbox(&mut d.appearance.custom_frame, app.t("options.appearance.window_style"));
        // Light and dark themes for a system that switches, or one theme.
        let system_dark = app.engine.system_dark();
        ui.checkbox(&mut d.appearance.follow_system_theme, app.t("options.appearance.follow_system"));
        if d.appearance.follow_system_theme {
            row(ui, &app.t("options.appearance.theme_light"), |ui| {
                theme_picker(app, ui, "theme-light", &mut d.appearance.theme_light);
            });
            row(ui, &app.t("options.appearance.theme_dark"), |ui| {
                theme_picker(app, ui, "theme-dark", &mut d.appearance.theme_dark);
            });
            // Without a mode to follow, the one theme is used: show it.
            if system_dark.is_none() {
                hint(ui, &look, &app.t("options.appearance.follow_system_unknown"));
                row(ui, &app.t("options.appearance.theme"), |ui| {
                    theme_picker(app, ui, "theme", &mut d.appearance.theme);
                });
            }
        } else {
            row(ui, &app.t("options.appearance.theme"), |ui| {
                theme_picker(app, ui, "theme", &mut d.appearance.theme);
            });
        }
        ui.horizontal(|ui| {
            if ui.button(app.t("options.appearance.edit_theme")).clicked() {
                // The theme in effect now is the one edited. Built-in themes
                // are exported to the user's theme folder as an editable
                // TOML file that extends the original.
                let a = &mut d.appearance;
                let slot = match (a.follow_system_theme, system_dark) {
                    (true, Some(true)) => &mut a.theme_dark,
                    (true, Some(false)) => &mut a.theme_light,
                    _ => &mut a.theme,
                };
                let entries = app.engine.theme_entries();
                let path = match entries.iter().find(|t| t.name == *slot).and_then(|t| t.path.clone()) {
                    Some(p) => p,
                    None => {
                        let dir = app.engine.paths().user_theme_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let p = dir.join(format!("{} (custom).toml", slot));
                        if !p.exists() {
                            let text = format!("# A theme of your own. Anything not set here comes from \"{0}\".\nextends = \"{0}\"\nname = \"{0} (custom)\"\n\n# accent = \"#2a8de4\"\n# [popup]\n# opacity = 0.9\n", slot);
                            let _ = std::fs::write(&p, text);
                        }
                        *slot = format!("{} (custom)", slot);
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
        row(ui, &app.t("options.appearance.window_transparency"), |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.window_transparency, 0..=90));
        });
        backdrop_row(app, ui, &app.t("options.appearance.backdrop"), "backdrop", &mut d.appearance.backdrop);
        let has_backdrop = d.appearance.backdrop != glossa_core::config::Backdrop::None;
        backdrop_opacity_row(app, ui, &look, has_backdrop, &mut d.appearance.backdrop_opacity);
        row(ui, &app.t("options.appearance.icon_scale"), |ui| {
            ui.add(egui::Slider::new(&mut d.appearance.icon_scale, 0.75..=2.5).step_by(0.05));
        });
        row(ui, &app.t("options.appearance.renderer"), |ui| {
            use glossa_core::config::Renderer;
            let names = [
                (Renderer::OpenGl, app.t("options.appearance.renderer_opengl")),
                (Renderer::Gpu, app.t("options.appearance.renderer_gpu")),
                (Renderer::Software, app.t("options.appearance.renderer_software")),
            ];
            let cur =
                names.iter().find(|(r, _)| *r == d.appearance.renderer).map(|(_, n)| n.clone()).unwrap_or_default();
            crate::dropdown::ComboBox::from_id_salt("renderer").width(280.0).selected_text(cur).show_ui(ui, |ui| {
                for (r, n) in names {
                    ui.selectable_value(&mut d.appearance.renderer, r, n);
                }
            });
        });
        hint(ui, &look, &app.t("options.appearance.renderer_hint"));
        row(ui, &app.t("options.appearance.unload_after"), |ui| {
            ui.add(egui::DragValue::new(&mut d.appearance.unload_graphics_after_secs).range(0..=3600));
        });
        ui.label(RichText::new(app.t("options.appearance.unload_hint")).small().color(look.weak_text()));
    });
    let p = &mut d.appearance.popup;
    group(ui, &look, &app.t("options.appearance.popup_group"), |ui| {
        ui.checkbox(&mut p.auto_size, app.t("options.appearance.auto_size"));
        ui.checkbox(&mut p.auto_position, app.t("options.appearance.auto_position"));
        ui.checkbox(&mut p.activate_on_open, app.t("options.appearance.activate"));
        ui.checkbox(&mut p.pin_when_dragging, app.t("options.appearance.pin_when_dragging"));
        ui.checkbox(&mut p.show_border, app.t("options.appearance.show_border"));
        backdrop_row(app, ui, &app.t("options.appearance.popup_backdrop"), "popup-backdrop", &mut p.backdrop);
        let has_backdrop = p.backdrop != glossa_core::config::Backdrop::None;
        backdrop_opacity_row(app, ui, &look, has_backdrop, &mut p.backdrop_opacity);
        row(ui, &app.t("options.appearance.popup_icon_scale"), |ui| {
            ui.add(egui::Slider::new(&mut p.icon_scale, 0.75..=2.5).step_by(0.05));
        });
        row(ui, &app.t("options.appearance.popup_shade"), |ui| {
            ui.add(egui::Slider::new(&mut p.shade, 0..=90));
        });
        hint(ui, &look, &app.t("options.appearance.popup_shade_hint"));
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
        row(ui, &app.t("options.appearance.popup_width"), |ui| {
            let (max, min) = (app.t("options.appearance.width_max"), app.t("options.appearance.width_min"));
            ui.add(egui::DragValue::new(&mut p.max_width).range(200.0..=1600.0).prefix(max));
            ui.add(egui::DragValue::new(&mut p.min_width).range(120.0..=1200.0).prefix(min));
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
    crate::dropdown::ComboBox::from_id_salt("exc-mode").width(ui.available_width()).selected_text(current).show_ui(
        ui,
        |ui| {
            for (m, s) in &modes {
                ui.selectable_value(&mut e.mode, *m, s);
            }
        },
    );
    let mut selected: Option<usize> = ui.data(|dd| dd.get_temp(egui::Id::unique("exc-sel")));
    // Where the system names no window classes (macOS) a rule is just a
    // program, so the class column, its field and smart detection (which
    // looks at classes) are left out.
    let classes = glossa_platform::shell::SHELL.window_classes;
    let columns = if classes { 2 } else { 1 };
    if e.rules.is_empty() {
        ui.label(RichText::new(app.t("options.exceptions.empty")).weak());
    } else {
        egui::Grid::new("exc").num_columns(columns).striped(true).min_col_width(260.0).show(ui, |ui| {
            ui.label(RichText::new(app.t("options.exceptions.program")).strong());
            if classes {
                ui.label(RichText::new(app.t("options.exceptions.window_class")).strong());
            }
            ui.end_row();
            for (i, r) in e.rules.iter().enumerate() {
                let p = if r.program.is_empty() { any_prog.clone() } else { r.program.clone() };
                if ui.selectable_label(selected == Some(i), p).clicked() {
                    selected = Some(i);
                }
                if classes {
                    ui.label(if r.window_class.is_empty() { any_win.clone() } else { r.window_class.clone() });
                }
                ui.end_row();
            }
        });
    }
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
            ui.add(
                egui::TextEdit::singleline(&mut rule.program)
                    .hint_text(glossa_platform::shell::SHELL.program_example)
                    .desired_width(200.0),
            );
            if classes {
                ui.add(
                    egui::TextEdit::singleline(&mut rule.window_class).hint_text("WindowClass").desired_width(200.0),
                );
            }
            if ui.button(app.t("options.ok")).clicked() {
                done = true;
            }
            if ui.button(app.t("options.cancel")).clicked() {
                cancel = true;
            }
            if ui
                .button(app.t("options.exceptions.from_active"))
                .on_hover_text(app.t("options.exceptions.from_active_tip"))
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
    if classes {
        ui.checkbox(&mut e.smart_detection, app.t("options.exceptions.smart"));
    }
}

fn advanced(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.advanced.group"), |ui| {
        row(ui, &app.t("options.advanced.browser"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut d.advanced.browser)
                    .hint_text(app.t("options.advanced.browser_default"))
                    .desired_width(260.0),
            );
        });
        hint(ui, &look, &app.t("options.advanced.browser_hint"));
        row(ui, &app.t("options.advanced.copy_action"), |ui| {
            // Cmd+C on macOS, which has no Insert key: Ctrl+Insert is offered
            // only where the command key is Ctrl (or when already chosen).
            let shell = glossa_platform::shell::SHELL;
            let names: Vec<(CopyAction, String)> = [
                (CopyAction::CtrlC, format!("{} + C", shell.command_key)),
                (
                    CopyAction::CtrlInsert,
                    if shell.command_is_ctrl {
                        "Ctrl + Insert".to_owned()
                    } else {
                        // What it does there: Cmd+C (see `input::send_copy`).
                        app.locale.fmt("options.advanced.copy_insert_no_key", shell.command_key)
                    },
                ),
                (CopyAction::AccessibilityOnly, app.t("options.advanced.copy_accessibility")),
            ]
            .into_iter()
            .filter(|(a, _)| {
                *a != CopyAction::CtrlInsert
                    || shell.command_is_ctrl
                    || d.advanced.copy_action == CopyAction::CtrlInsert
            })
            .collect();
            let cur =
                names.iter().find(|(a, _)| *a == d.advanced.copy_action).map(|(_, n)| n.clone()).unwrap_or_default();
            crate::dropdown::ComboBox::from_id_salt("copy-action").width(260.0).selected_text(cur).show_ui(ui, |ui| {
                for (a, n) in names {
                    ui.selectable_value(&mut d.advanced.copy_action, a, n);
                }
            });
        });
        hint(ui, &look, &app.t("options.advanced.copy_action_hint"));
        row(ui, &app.t("options.advanced.preferred_domain"), |ui| {
            crate::dropdown::ComboBox::from_id_salt("domain")
                .width(260.0)
                .selected_text(&d.advanced.preferred_domain)
                .show_ui(ui, |ui| {
                    for dom in [
                        "com", "ru", "com.ua", "by", "kz", "de", "fr", "es", "it", "co.uk", "co.jp", "com.br", "pl",
                        "com.tr", "cn",
                    ] {
                        ui.selectable_value(&mut d.advanced.preferred_domain, dom.to_owned(), dom);
                    }
                });
        });
        hint(ui, &look, &app.t("options.advanced.preferred_domain_hint"));
        row(ui, &app.t("options.advanced.copy_wait"), |ui| {
            ui.add(egui::DragValue::new(&mut d.advanced.copy_wait_ms).range(20..=3000));
        });
        hint(ui, &look, &app.t("options.advanced.copy_wait_hint"));
        ui.checkbox(&mut d.advanced.slower_listen_repeat, app.t("options.advanced.slower_listen"));
        hint(ui, &look, &app.t("options.advanced.slower_listen_hint"));
        ui.checkbox(&mut d.advanced.remove_line_breaks, app.t("options.advanced.remove_line_breaks"));
        hint(ui, &look, &app.t("options.advanced.remove_line_breaks_hint"));
    });
    let supported = crate::mouse::MouseHub::supported();
    let mouse_error = app.mouse_error().filter(|_| d.mouse_mode.enabled);
    group(ui, &look, &app.t("options.advanced.mouse_mode_group"), |ui| {
        if !supported {
            ui.label(RichText::new(app.t("mouse.unavailable")).color(look.weak_text()));
        } else if let Some(e) = &mouse_error {
            ui.label(RichText::new(app.locale.fmt("mouse.failed", e)).color(look.error()));
        }
        ui.add_enabled_ui(supported, |ui| mouse_mode_options(app, ui, &look, d));
    });
    let tray_title =
        if SHELL.app_menu { "options.advanced.tray_group_menu_bar" } else { "options.advanced.tray_group" };
    group(ui, &look, &app.t(tray_title), |ui| tray_click_options(app, ui, &look, &mut d.tray));
}

/// What tray clicks can be set to, in the order the list shows them. A file
/// may name any action; one not listed here is shown too while chosen.
const TRAY_CHOICES: [TrayAction; 13] = [
    TrayAction::Run(Action::ShowMainWindow),
    TrayAction::Run(Action::ShowPopup),
    TrayAction::Run(Action::TranslateClipboardInPopup),
    TrayAction::Run(Action::TextRecognition),
    TrayAction::Run(Action::RecognizeAndTranslate),
    TrayAction::Run(Action::TranslateRegionInPlace),
    TrayAction::Run(Action::ShowDictionary),
    TrayAction::Run(Action::ShowHistory),
    TrayAction::Run(Action::ToggleMouseMode),
    TrayAction::Run(Action::NextPreset),
    TrayAction::Run(Action::ShowOptions),
    TrayAction::Menu,
    TrayAction::Nothing,
];

fn tray_action_label(app: &GlossaApp, a: TrayAction) -> String {
    match a {
        TrayAction::Nothing => app.t("options.tray.nothing"),
        TrayAction::Menu => app.t("options.tray.menu"),
        TrayAction::Run(action) => app.t(&format!("action.{}", action.id())),
    }
}

/// The tray click settings, greyed out where the tray reports no clicks.
fn tray_click_options(app: &GlossaApp, ui: &mut egui::Ui, look: &crate::style::Look, d: &mut TrayConfig) {
    if !SHELL.tray_clicks {
        ui.label(RichText::new(app.t("options.tray.unsupported")).color(look.weak_text()));
    }
    ui.add_enabled_ui(SHELL.tray_clicks, |ui| {
        let rows = [
            ("tray-click", "options.tray.click", &mut d.click),
            ("tray-double-click", "options.tray.double_click", &mut d.double_click),
            ("tray-middle-click", "options.tray.middle_click", &mut d.middle_click),
        ];
        for (salt, label, value) in rows {
            row(ui, &app.t(label), |ui| {
                crate::dropdown::ComboBox::from_id_salt(salt)
                    .width(280.0)
                    .selected_text(tray_action_label(app, *value))
                    .show_ui(ui, |ui| {
                        let current = *value;
                        let extra = (!TRAY_CHOICES.contains(&current)).then_some(current);
                        for choice in TRAY_CHOICES.into_iter().chain(extra) {
                            ui.selectable_value(value, choice, tray_action_label(app, choice));
                        }
                    });
            });
        }
        if d.click == TrayAction::Menu && d.double_click != TrayAction::Nothing {
            hint(ui, look, &app.t("options.tray.menu_hint"));
        }
        hint(ui, look, &app.t("options.tray.hint"));
    });
}

/// The mouse mode switches, greyed out by the caller where it cannot work.
fn mouse_mode_options(app: &GlossaApp, ui: &mut egui::Ui, look: &crate::style::Look, d: &mut Config) {
    ui.checkbox(&mut d.mouse_mode.enabled, app.t("tray.mouse_enabled"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mouse_mode.behaviour, MouseBehaviour::ShowIcon, app.t("tray.mouse_icon"));
        ui.radio_value(&mut d.mouse_mode.behaviour, MouseBehaviour::ShowTranslation, app.t("tray.mouse_translation"));
        ui.radio_value(
            &mut d.mouse_mode.behaviour,
            MouseBehaviour::ShowTranslationAndRead,
            app.t("tray.mouse_translation_read"),
        );
    });
    // On macOS the key is Cmd (or Control): see `mouse::selection_task`.
    let ctrl_only = crate::widgets::shortcut_text(&app.t("options.advanced.mouse_ctrl_only"));
    ui.checkbox(&mut d.mouse_mode.ctrl_only, ctrl_only);
    hint(ui, look, &crate::widgets::shortcut_text(&app.t("options.advanced.mouse_ctrl_only_hint")));
    ui.label(RichText::new(app.t("options.advanced.mouse_mode_note")).small().color(look.weak_text()));
}

fn ocr(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let d = &mut st.draft.ocr;
    group(ui, &look, &app.t("options.ocr"), |ui| {
        let providers = app.engine.providers(Capabilities::OCR);
        row(ui, &app.t("options.ocr.provider"), |ui| {
            let cur = providers
                .iter()
                .find(|p| p.info.id == d.provider)
                .map(|p| p.info.name.clone())
                .unwrap_or(d.provider.clone());
            // Recognition engines first, then the AI endpoints that can
            // read pictures with a vision model.
            let (ai, engines): (Vec<_>, Vec<_>) = providers.iter().partition(|p| is_ai_endpoint(&p.info.id));
            crate::dropdown::ComboBox::from_id_salt("ocr-provider").width(280.0).selected_text(cur).show_ui(ui, |ui| {
                for p in &engines {
                    ui.selectable_value(&mut d.provider, p.info.id.clone(), &p.info.name);
                }
                if !ai.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(app.t("options.ocr.ai_group")).small().color(look.weak_text()));
                    for p in &ai {
                        ui.selectable_value(&mut d.provider, p.info.id.clone(), &p.info.name);
                    }
                }
            });
        });
        if is_ai_endpoint(&d.provider) {
            let name =
                providers.iter().find(|p| p.info.id == d.provider).map(|p| p.info.name.clone()).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(app.locale.fmt("options.ocr.ai_hint", &name)).small().color(look.weak_text()));
                if ui.small_button(app.t("options.ocr.ai_settings")).clicked() {
                    st.selected_service = Some(d.provider.clone());
                    st.page = Page::Services;
                }
            });
        }
        let auto = app.t("main.auto_detect");
        row(ui, &app.t("ocr.language"), |ui| {
            if let Some(l) = crate::widgets::lang_combo(
                ui,
                "ocr-lang-opt",
                d.language,
                true,
                &auto,
                &app.t("lang.search"),
                &[],
                280.0,
            ) {
                d.language = l;
            }
        });
        ui.checkbox(&mut d.translate_after, app.t("options.ocr.translate_after"));
        ui.checkbox(&mut d.preserve_line_breaks, app.t("options.ocr.keep_line_breaks"));
        ui.checkbox(&mut d.copy_joined_lines, app.t("options.ocr.copy_joined"));
        {
            use glossa_core::config::{OcrAfterSelect as A, OcrTranslateWith as T};
            ui.label(app.t("options.ocr.after_select"));
            ui.indent("ocr-after-select", |ui| {
                ui.radio_value(&mut d.after_select, A::Show, app.t("options.ocr.after_show"));
                ui.radio_value(&mut d.after_select, A::Copy, app.t("options.ocr.after_copy"));
                ui.radio_value(&mut d.after_select, A::InPlace, app.t("options.ocr.after_in_place"));
                ui.radio_value(&mut d.after_select, A::Ask, app.t("options.ocr.after_ask"));
            });
            ui.label(app.t("options.ocr.translate_with"));
            ui.indent("ocr-translate-with", |ui| {
                ui.radio_value(&mut d.translate_with, T::OcrEngine, app.t("options.ocr.translate_with_engine"));
                ui.radio_value(&mut d.translate_with, T::Translator, app.t("options.ocr.translate_with_translator"));
            });
        }
        row(ui, &app.t("options.ocr.watch_interval"), |ui| {
            ui.add(egui::DragValue::new(&mut d.watch_interval_ms).range(200..=60000));
        });
        ui.horizontal(|ui| {
            ui.label(app.t("options.ocr.save_dir"));
            let shown = d.save_image_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into());
            ui.label(RichText::new(shown).small());
            if ui.button("…").clicked() {
                d.save_image_dir = rfd::FileDialog::new().pick_folder();
            }
            if d.save_image_dir.is_some() && ui.button(app.t("history.clear")).clicked() {
                d.save_image_dir = None;
            }
        });
    });
    group(ui, &look, "Google Lens", |ui| {
        let l = &mut d.lens;
        ui.horizontal(|ui| {
            ui.radio_value(&mut l.mode, LensMode::Auto, app.t("options.ocr.lens_auto"));
            ui.radio_value(&mut l.mode, LensMode::Native, app.t("options.ocr.lens_native"));
            ui.radio_value(&mut l.mode, LensMode::Daemon, app.t("options.ocr.lens_daemon"));
            ui.radio_value(&mut l.mode, LensMode::Cli, app.t("options.ocr.lens_cli"));
        });
        row(ui, &app.t("options.ocr.lens_endpoint"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut l.endpoint)
                    .hint_text(app.t("options.ocr.lens_endpoint_hint"))
                    .desired_width(280.0),
            );
        });
        row(ui, &app.t("options.server.token"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut l.token).password(true).desired_width(280.0));
        });
        row(ui, &app.t("options.ocr.lens_cli_path"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut l.cli_path)
                    .hint_text(app.t("options.ocr.lens_cli_hint"))
                    .desired_width(280.0),
            );
        });
        ui.label(RichText::new(app.t("options.ocr.lens_install")).small().color(look.weak_text()));
    });
    if cfg!(windows) {
        group(ui, &look, &app.t("options.ocr.windows_title"), |ui| {
            ui.label(RichText::new(app.t("options.ocr.windows_note")).small().color(look.weak_text()));
        });
        group(ui, &look, &app.t("options.ocr.oneocr_title"), |ui| {
            let o = &mut d.oneocr;
            row(ui, &app.t("options.ocr.oneocr_folder"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut o.folder)
                        .hint_text(app.t("options.ocr.oneocr_folder_hint"))
                        .desired_width(250.0),
                );
                if ui.button("…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        o.folder = dir.display().to_string();
                    }
                }
            });
            ui.label(RichText::new(app.t("options.ocr.oneocr_note")).small().color(look.weak_text()));
        });
    }
    if cfg!(target_os = "macos") {
        group(ui, &look, &app.t("options.ocr.vision_title"), |ui| {
            let v = &mut d.vision;
            ui.horizontal(|ui| {
                ui.radio_value(&mut v.level, VisionLevel::Accurate, app.t("options.ocr.vision_accurate"));
                ui.radio_value(&mut v.level, VisionLevel::Fast, app.t("options.ocr.vision_fast"));
            });
            ui.checkbox(&mut v.language_correction, app.t("options.ocr.vision_correction"));
            row(ui, &app.t("options.ocr.auto_langs"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut v.auto_languages)
                        .hint_text(app.t("options.ocr.vision_auto_hint"))
                        .desired_width(250.0),
                );
            });
            ui.label(RichText::new(app.t("options.ocr.vision_note")).small().color(look.weak_text()));
        });
    }
    group(ui, &look, &app.t("options.ocr.tesseract_title"), |ui| {
        let t = &mut d.tesseract;
        row(ui, &app.t("options.ocr.tesseract_path"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut t.path)
                    .hint_text(app.t("options.ocr.tesseract_path_hint"))
                    .desired_width(250.0),
            );
            if ui.button("…").clicked() {
                if let Some(file) = rfd::FileDialog::new().pick_file() {
                    t.path = file.display().to_string();
                }
            }
        });
        row(ui, &app.t("options.ocr.tesseract_tessdata"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut t.tessdata_dir)
                    .hint_text(app.t("options.ocr.tesseract_tessdata_hint"))
                    .desired_width(250.0),
            );
            if ui.button("…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    t.tessdata_dir = dir.display().to_string();
                }
            }
        });
        row(ui, &app.t("options.ocr.auto_langs"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut t.auto_languages)
                    .hint_text(app.t("options.ocr.tesseract_auto_hint"))
                    .desired_width(250.0),
            );
        });
        let default = app.t("options.ocr.default");
        let psm: Vec<(u8, String)> =
            [3u8, 4, 6, 7, 11, 13].into_iter().map(|n| (n, app.t(&format!("options.ocr.psm_{n}")))).collect();
        row(ui, &app.t("options.ocr.tesseract_psm"), |ui| {
            let shown = t
                .psm
                .map(|n| psm.iter().find(|(m, _)| *m == n).map_or(n.to_string(), |(_, l)| l.clone()))
                .unwrap_or(default.clone());
            egui::ComboBox::from_id_salt("tesseract-psm").width(250.0).selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut t.psm, None, &default);
                for (n, label) in &psm {
                    ui.selectable_value(&mut t.psm, Some(*n), label);
                }
            });
        });
        let oem: Vec<(u8, String)> =
            [0u8, 1, 2].into_iter().map(|n| (n, app.t(&format!("options.ocr.oem_{n}")))).collect();
        row(ui, &app.t("options.ocr.tesseract_oem"), |ui| {
            let shown = t
                .oem
                .map(|n| oem.iter().find(|(m, _)| *m == n).map_or(n.to_string(), |(_, l)| l.clone()))
                .unwrap_or(default.clone());
            egui::ComboBox::from_id_salt("tesseract-oem").width(250.0).selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut t.oem, None, &default);
                for (n, label) in &oem {
                    ui.selectable_value(&mut t.oem, Some(*n), label);
                }
            });
        });
        row(ui, &app.t("options.ocr.tesseract_extra"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut t.extra_args)
                    .hint_text("-c preserve_interword_spaces=1")
                    .desired_width(250.0),
            );
        });
        ui.label(RichText::new(app.t("options.ocr.tesseract_note")).small().color(look.weak_text()));
    });
}

fn modules(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let status = app.engine.module_status();
    let mut reload_draft = false;
    let d = &mut st.draft;
    group(ui, &look, &app.t("options.modules.runtimes"), |ui| {
        ui.checkbox(&mut d.modules.allow_legacy_scripts, app.t("options.modules.legacy_scripts"));
        ui.checkbox(&mut d.modules.allow_native, app.t("options.modules.native"));
        row(ui, &app.t("options.modules.python"), |ui| {
            use glossa_modules::python_env::PythonChoice;
            let choice = PythonChoice::parse(&d.modules.python);
            let names = [
                ("auto", app.t("options.modules.python_auto")),
                ("system", app.t("options.modules.python_system")),
                ("managed", app.t("options.modules.python_managed")),
                ("path", app.t("options.modules.python_path")),
            ];
            let current = match choice {
                PythonChoice::Auto => 0,
                PythonChoice::System => 1,
                PythonChoice::Managed => 2,
                PythonChoice::Path(_) => 3,
            };
            let mut picked = current;
            crate::dropdown::ComboBox::from_id_salt("python-choice")
                .width(200.0)
                .selected_text(names[current].1.clone())
                .show_ui(ui, |ui| {
                    for (i, (_, name)) in names.iter().enumerate() {
                        ui.selectable_value(&mut picked, i, name);
                    }
                });
            if picked != current {
                // A path starts as the usual command, for the user to edit.
                d.modules.python = if picked == 3 { "python3".into() } else { names[picked].0.into() };
            }
            if picked == 3 {
                ui.add(
                    egui::TextEdit::singleline(&mut d.modules.python)
                        .hint_text(app.t("options.modules.python_hint"))
                        .desired_width(200.0),
                );
            }
        });
        ui.label(RichText::new(app.t("options.modules.python_note")).small().color(look.weak_text()));
        row(ui, &app.t("options.modules.java"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut d.modules.java)
                    .hint_text(app.t("options.modules.java_hint"))
                    .desired_width(280.0),
            );
        });
        row(ui, &app.t("options.modules.call_timeout"), |ui| {
            ui.add(egui::DragValue::new(&mut d.modules.call_timeout_ms).range(1000..=600000));
        });
        ui.checkbox(&mut d.modules.auto_download_assets, app.t("options.modules.auto_download"));
    });
    group(ui, &look, &app.t("options.modules.classic"), |ui| {
        ui.horizontal(|ui| {
            let shown = d.classic.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "—".into());
            ui.label(shown);
            if ui.button(app.t("options.modules.choose")).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    d.classic.path = Some(dir);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.classic.use_services, app.t("options.modules.use_services"));
            ui.checkbox(&mut d.classic.use_themes, app.t("options.modules.use_themes"));
            ui.checkbox(&mut d.classic.use_locales, app.t("options.modules.use_locales"));
        });
        if let Some(dir) = d.classic.path.clone() {
            if ui.button(app.t("options.modules.import")).clicked() {
                match app.engine.import_classic(&dir) {
                    Ok(r) => {
                        let settings = app.t(if r.options_imported { "msg.yes" } else { "msg.no" }).to_lowercase();
                        let entries = r.history_entries.to_string();
                        st.message =
                            Some((app.locale.fmt_args("options.modules.imported", &[&settings, &entries]), false));
                        reload_draft = true;
                    }
                    Err(e) => st.message = Some((e.to_string(), true)),
                }
            }
        }
    });
    group(ui, &look, &app.t("options.modules.folders"), |ui| {
        let user_dir = app.engine.paths().user_module_dir();
        ui.horizontal(|ui| {
            if ui.button(app.t("options.modules.open_folder")).clicked() {
                let _ = std::fs::create_dir_all(&user_dir);
                let _ = crate::app::open_with_system(&user_dir.to_string_lossy());
            }
            if ui.button(app.t("options.modules.install")).on_hover_text(app.t("options.modules.install_tip")).clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(app.t("options.modules.potext_filter"), &["potext"])
                    .pick_file()
                    .or_else(|| rfd::FileDialog::new().pick_folder())
                {
                    match install(&path, &user_dir, &app.t("options.modules.not_a_module")) {
                        Ok(()) => {
                            app.engine.reload_modules();
                            st.message = Some((app.t("options.modules.installed"), false));
                        }
                        Err(e) => st.message = Some((e, true)),
                    }
                }
            }
            if ui.button(app.t("options.modules.reload")).clicked() {
                app.engine.reload_modules();
            }
        });
        for dir in d.modules.dirs.clone() {
            ui.horizontal(|ui| {
                ui.label(dir.display().to_string());
                if ui.small_button(app.t("options.remove")).clicked() {
                    d.modules.dirs.retain(|x| *x != dir);
                }
            });
        }
        if ui.button(app.t("options.modules.add_folder")).clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                d.modules.dirs.push(dir);
            }
        }
    });
    group(
        ui,
        &look,
        &app.locale.fmt_args(
            "options.modules.status",
            &[&status.loaded.len().to_string(), &status.failed.len().to_string(), &status.skipped.len().to_string()],
        ),
        |ui| {
            for (path, why) in &status.failed {
                let line = app.locale.fmt_args("options.modules.failed_item", &[&path.display().to_string(), why]);
                ui.label(RichText::new(line).color(look.error()).small());
            }
            for (path, why) in &status.skipped {
                ui.label(RichText::new(format!("– {}: {why}", path.display())).color(look.weak_text()).small());
            }
            ui.label(RichText::new(status.loaded.join(", ")).small().color(look.weak_text()));
        },
    );
    if reload_draft {
        st.draft = app.engine.config();
        // The classic program's "start with Windows" comes in with its
        // settings: offered as the checkbox, written to the system on Apply.
        st.autostart = st.draft.general.start_with_system;
    }
}

/// Copy a module folder or a `.potext` archive into the user's modules.
/// `not_a_module` is the message for a folder that holds no module.
fn install(path: &std::path::Path, user_dir: &std::path::Path, not_a_module: &str) -> Result<(), String> {
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
        return Err(not_a_module.to_owned());
    }
    copy_dir(path, &target).map_err(|e| e.to_string())
}

fn api(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut OptionsState) {
    let look = app.look.clone();
    let s = &mut st.draft.server;
    ui.checkbox(&mut s.enabled, app.t("options.server.enable"));
    row(ui, &app.t("options.server.listen_on"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut s.bind).desired_width(220.0));
    });
    row(ui, &app.t("options.server.token"), |ui| {
        if ui.button(app.t("options.server.regenerate")).clicked() {
            s.token = glossa_engine::random_token();
        }
        ui.add(egui::TextEdit::singleline(&mut s.token).password(!st.show_secrets).desired_width(320.0));
    });
    ui.checkbox(&mut st.show_secrets, app.t("options.server.show_token"));
    // What is running now; turning it on or off takes a restart.
    match (&app.server_error, app.server_running()) {
        (Some(e), _) => {
            ui.label(RichText::new(app.locale.fmt("options.server.not_running", e)).color(look.error()));
        }
        (None, true) => {
            ui.label(RichText::new(app.t("options.server.running")).color(look.weak_text()));
        }
        (None, false) => {
            ui.label(RichText::new(app.t("options.server.stopped")).color(look.weak_text()));
        }
    }
    ui.add_space(8.0);
    ui.label(app.t("options.server.bind_hint"));
    for (what, path) in [
        ("options.server.cmd_popup", "/v1/action/show_popup"),
        ("options.server.cmd_ocr", "/v1/action/text_recognition"),
        ("options.server.cmd_ocr_translate", "/v1/action/recognize_and_translate"),
        ("options.server.cmd_main", "/v1/action/show_main_window"),
    ] {
        let cmd = format!("curl -s 'http://{}{path}?token={}'", s.bind, s.token);
        ui.horizontal(|ui| {
            if crate::icons::sized_button(ui, &app.look, crate::icons::Icon::Copy, &app.t("edit.copy"), 18.0, true)
                .clicked()
            {
                ui.ctx().copy_text(cmd.clone());
            }
            ui.label(RichText::new(app.t(what)).small());
        });
        ui.code(if st.show_secrets { cmd } else { cmd.replace(&s.token, "<token>") });
    }
}

/// What the system lets Glossa do, with a way to ask for what is missing.
pub(crate) fn permissions(app: &mut GlossaApp, ui: &mut egui::Ui) {
    use glossa_platform::permissions::{self as perm, Permission, Status};
    let look = app.look.clone();
    // Grants happen in System Settings, outside this window: look again now
    // and then.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    ui.label(RichText::new(app.t("options.permissions.intro")).color(look.weak_text()));
    for &p in Permission::all() {
        let (name, why) = match p {
            Permission::Accessibility => ("options.permissions.accessibility", "options.permissions.accessibility_why"),
            Permission::ScreenRecording => ("options.permissions.screen", "options.permissions.screen_why"),
            Permission::InputMonitoring => ("options.permissions.input", "options.permissions.input_why"),
        };
        group(ui, &look, &app.t(name), |ui| {
            ui.label(RichText::new(app.t(why)).color(look.weak_text()));
            ui.horizontal(|ui| {
                let granted = perm::status(p) == Status::Granted;
                let (text, colour) = if granted {
                    (app.t("options.permissions.granted"), egui::Color32::from_rgb(0x3f, 0xb9, 0x50))
                } else {
                    (app.t("options.permissions.missing"), look.error())
                };
                ui.label(RichText::new(text).color(colour).strong());
                if !granted && ui.button(app.t("options.permissions.request")).clicked() {
                    perm::request(p);
                }
                if ui.button(app.t("options.permissions.open_settings")).clicked() {
                    if let Err(e) = perm::open_settings(p) {
                        app.notify(e.to_string(), true);
                    }
                }
            });
        });
    }
    ui.add_space(6.0);
    ui.label(RichText::new(app.t("options.permissions.rebuild_note")).small().color(look.weak_text()));
}

fn updates(app: &mut GlossaApp, ui: &mut egui::Ui, _st: &mut OptionsState) {
    // There is no update server yet, so no "check automatically" switch:
    // `updates.check_automatically` is kept in the settings, unused.
    ui.label(format!("{}{}", app.t("msg.version"), glossa_core::VERSION));
    ui.label(RichText::new(app.t("options.updates.no_server")).color(app.look.weak_text()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(code: &str, folder: &str) -> glossa_engine::catalog::LocaleEntry {
        glossa_engine::catalog::LocaleEntry {
            code: code.into(),
            name: code.into(),
            native_name: code.into(),
            folder: folder.into(),
            builtin: folder.is_empty(),
            files: Vec::new(),
            classic_files: 0,
        }
    }

    #[test]
    fn an_empty_interface_language_is_the_system_one() {
        let locales = [entry("en", "English"), entry("ru", "")];
        assert!(chosen_locale(&locales, "").is_none(), "not the built-in pack with no folder");
        assert_eq!(chosen_locale(&locales, "ru").map(|l| l.code.as_str()), Some("ru"));
        assert_eq!(chosen_locale(&locales, "english").map(|l| l.code.as_str()), Some("en"));
    }
}
