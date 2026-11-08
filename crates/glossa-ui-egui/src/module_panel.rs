//! Module panels: the windows ([`PanelWindow`]) and the renderer
//! ([`draw`]) the module editor's preview uses too.
//!
//! Everything that decides something lives in `glossa_engine::panels`:
//! the state, which action runs, the streamed updates, the kept keys. This
//! draws a [`Panel`]'s elements with Options' widgets and `style::Look`'s
//! colours, hands edits and clicks back to it, and carries out what an
//! action asks for (copy, open a web address, play audio).
//!
//! What a module puts on a panel is drawn as data: text as text (never as
//! markup that could reach anything), links only for `http`/`https` with
//! the address on hover, images decoded here within size limits.

use std::sync::Arc;

use egui::RichText;
use glossa_core::locale::Locale;
use glossa_core::LangId;
use glossa_engine::panels::NoticeLevel;
use glossa_engine::{Effect, Panel, PanelId};
use glossa_modules::panels::{self as fmt, Element, ElementKind, ResultKind, Width};
use glossa_modules::settings::LabelStyle;
use serde_json::Value;

use crate::app::GlossaApp;
use crate::style::Look;

/// An open panel's window.
pub struct PanelWindow {
    pub panel: Panel,
    viewport: egui::ViewportId,
    /// The state revision whose text was last checked for fonts.
    fonts_for: Option<u64>,
    /// An action to run once the panel has settled (`--screenshot`).
    pressed: Option<String>,
}

impl PanelWindow {
    /// Run `action` once the panel is ready, as if its button were pressed.
    pub fn press(&mut self, action: &str) {
        self.pressed = Some(action.to_owned());
    }
}

/// Something the user asked for that the app carries out.
pub enum Out {
    OpenUrl(String),
    Speak(String),
    Copy(String),
}

pub fn viewport_of(id: &PanelId) -> egui::ViewportId {
    crate::chrome::viewport(&format!("glossa-panel-{id}"))
}

/// Open panel `name` (`module/panel`), or raise it when it is open.
pub fn open_named(app: &mut GlossaApp, name: &str) {
    match PanelId::parse(name) {
        Ok(id) => open(app, &id),
        Err(e) => {
            let m = app.describe(&e);
            app.notify(m, true);
        }
    }
}

pub fn open(app: &mut GlossaApp, id: &PanelId) {
    if let Some(w) = app.panel_windows.iter().find(|w| w.panel.id == *id) {
        crate::chrome::raise(&app.ctx, w.viewport);
        return;
    }
    let ctx = app.ctx.clone();
    match app.engine.open_panel(id, Arc::new(move || ctx.request_repaint())) {
        Ok(panel) => {
            app.panel_windows.push(PanelWindow { panel, viewport: viewport_of(id), fonts_for: None, pressed: None });
            app.ctx.request_repaint();
        }
        Err(e) => {
            let m = app.describe(&e);
            app.notify(m, true);
        }
    }
}

/// Close every panel (the GUI unloading, or quitting): each stops its
/// action and keeps its `persist` keys.
pub fn close_all(app: &mut GlossaApp) {
    for mut w in app.panel_windows.drain(..) {
        w.panel.close();
    }
}

/// Draw the open panels' windows.
pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let mut windows = std::mem::take(&mut app.panel_windows);
    windows.retain_mut(|w| show_one(app, ctx, w));
    // A panel opened while these were drawn (from a panel's own action, a
    // hotkey) is kept too.
    windows.append(&mut app.panel_windows);
    app.panel_windows = windows;
}

/// One window; false once it is closed.
fn show_one(app: &mut GlossaApp, ctx: &egui::Context, w: &mut PanelWindow) -> bool {
    if let Some(wait) = w.panel.poll() {
        ctx.request_repaint_after(wait);
    }
    if w.panel.running().is_none() {
        if let Some(a) = w.pressed.take() {
            if !w.panel.trigger(&a, "click", Some(&a), None) {
                w.pressed = Some(a);
            }
        }
    }
    if w.fonts_for != Some(w.panel.revision()) {
        w.fonts_for = Some(w.panel.revision());
        ensure_fonts(app, ctx, &w.panel);
    }
    let page = w.panel.page().clone();
    let title = page.window_title.clone();
    let builder = crate::chrome::builder(app, &title).with_inner_size(page.size).with_min_inner_size([240.0, 160.0]);
    let custom = crate::chrome::custom(&app.cfg);
    let mut open = true;
    let mut outs = Vec::new();
    let salt = format!("panel-{}", w.panel.id);
    ctx.show_viewport_immediate(w.viewport, builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("panel-scroll", &salt)).auto_shrink([false, false]).show(ui, |ui| {
                outs = draw(ui, &app.look, &app.locale, &mut w.panel, &salt);
            });
        });
    });
    app.child_backdrop(&title, false);
    carry_out(app, outs, w.panel.take_effects());
    if !open {
        w.panel.close();
    }
    open
}

/// Load the fonts the panel's text needs (Cyrillic, CJK…), once per change.
fn ensure_fonts(app: &mut GlossaApp, ctx: &egui::Context, panel: &Panel) {
    let mut text = String::new();
    fn collect(v: &Value, out: &mut String, budget: &mut usize) {
        if *budget == 0 {
            return;
        }
        match v {
            Value::String(s) if !s.is_ascii() => {
                let take: String = s.chars().take(*budget).collect();
                *budget = budget.saturating_sub(take.len());
                out.push_str(&take);
            }
            Value::Array(a) => a.iter().for_each(|x| collect(x, out, budget)),
            Value::Object(o) => o.values().for_each(|x| collect(x, out, budget)),
            _ => {}
        }
    }
    let mut budget = 64 * 1024;
    for v in panel.state().values() {
        collect(v, &mut text, &mut budget);
    }
    let page = panel.page();
    for el in page.walk() {
        text.push_str(&el.label);
        text.push_str(&el.placeholder);
    }
    text.push_str(&page.window_title);
    app.fonts.ensure(ctx, &text);
}

/// Do what the panel asked: open, copy, speak, play.
pub fn carry_out(app: &mut GlossaApp, outs: Vec<Out>, effects: Vec<Effect>) {
    for o in outs {
        match o {
            Out::OpenUrl(u) if fmt::safe_link(&u) => app.open_url(u.trim()),
            Out::OpenUrl(_) => {}
            Out::Speak(t) => app.speak(&t, LangId::AUTO),
            Out::Copy(t) => app.copy_text(&t),
        }
    }
    for e in effects {
        match e {
            Effect::Copy(t) => app.copy_text(&t),
            Effect::OpenUrl(u) if fmt::safe_link(&u) => app.open_url(u.trim()),
            Effect::OpenUrl(_) => {}
            Effect::Play(a) => {
                if let Err(e) = app.audio.play(a.bytes.clone()) {
                    let m = format!("{}: {e}", app.t("msg.cannot_play"));
                    app.notify(m, true);
                }
            }
        }
    }
}

/// Draw `panel`'s elements and its notice; return what the user asked the
/// app to do. `salt` keeps two panels' widgets apart.
pub fn draw(ui: &mut egui::Ui, look: &Look, locale: &Locale, panel: &mut Panel, salt: &str) -> Vec<Out> {
    let page = panel.page().clone();
    let mut cx = Cx { look, locale, salt, out: Vec::new() };
    cx.shortcuts(ui, panel);
    cx.elements(ui, panel, &page.elements, false);
    ui.add_space(6.0);
    if let Some(n) = panel.notice().cloned() {
        let text = match (&n.text, &n.error) {
            (Some(t), _) => t.clone(),
            (None, Some(e)) => crate::app::describe_error(locale, e),
            (None, None) => String::new(),
        };
        if !text.is_empty() {
            let color = match n.level {
                NoticeLevel::Error | NoticeLevel::Warning => look.error(),
                NoticeLevel::Success => look.text(),
                NoticeLevel::Info => look.weak_text(),
            };
            ui.add(egui::Label::new(RichText::new(text).small().color(color)).wrap());
        }
    }
    cx.out
}

struct Cx<'a> {
    look: &'a Look,
    locale: &'a Locale,
    salt: &'a str,
    out: Vec<Out>,
}

/// Where the chosen tab of a `tabs` element is remembered.
fn tabs_id(salt: &str, tabs: &str) -> egui::Id {
    egui::Id::unique(("panel-tabs", salt, tabs))
}

/// A file dialog's answer on its way back from the worker.
type PickSlot = Arc<parking_lot::Mutex<Option<Option<String>>>>;

impl Cx<'_> {
    fn t(&self, key: &str) -> String {
        self.locale.get(key).to_owned()
    }

    /// Actions with a `shortcut` run when it is pressed in the panel.
    fn shortcuts(&mut self, ui: &mut egui::Ui, panel: &mut Panel) {
        let page = panel.page().clone();
        for spec in page.specs.iter().filter(|s| !s.shortcut.is_empty()) {
            let Ok(hk) = spec.shortcut.parse::<glossa_core::Hotkey>() else { continue };
            let Some(key) = egui::Key::from_name(&hk.key.0) else { continue };
            // Ctrl is the platform's command key (Cmd on macOS), as in
            // every other Glossa window.
            let mut mods = egui::Modifiers::NONE;
            if hk.mods.ctrl || hk.mods.meta {
                mods |= egui::Modifiers::COMMAND;
            }
            if hk.mods.alt {
                mods |= egui::Modifiers::ALT;
            }
            if hk.mods.shift {
                mods |= egui::Modifiers::SHIFT;
            }
            if ui.input_mut(|i| i.consume_key(mods, key)) && panel.blockers(&spec.id).is_empty() {
                panel.trigger(&spec.id, "shortcut", None, None);
            }
        }
    }

    fn elements(&mut self, ui: &mut egui::Ui, panel: &mut Panel, els: &[Element], compact: bool) {
        for el in els {
            self.element(ui, panel, el, compact);
        }
    }

    fn element(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element, compact: bool) {
        let (visible, enabled) = panel.element_state(el);
        if !visible {
            return;
        }
        // A field bound to a setting is shown, never edited, from a panel.
        let read_only = matches!(el.bind, fmt::Bind::Options(_));
        ui.add_enabled_ui(enabled, |ui| match el.kind {
            ElementKind::Label => self.label(ui, panel, el),
            ElementKind::Link => self.link(ui, panel, el),
            ElementKind::Separator => {
                ui.separator();
            }
            ElementKind::Button => {
                self.button(ui, panel, &el.action, &el.id, false);
            }
            ElementKind::Row => {
                ui.horizontal_wrapped(|ui| self.elements(ui, panel, &el.children, true));
            }
            ElementKind::Group => {
                let title = if el.label.is_empty() { el.id.clone() } else { el.label.clone() };
                if el.collapsed {
                    egui::CollapsingHeader::new(RichText::new(title).color(self.look.text()))
                        .id_salt(("panel-group", self.salt, &el.id))
                        .default_open(false)
                        .show(ui, |ui| self.elements(ui, panel, &el.children, false));
                } else if el.label.is_empty() {
                    self.elements(ui, panel, &el.children, false);
                } else {
                    let look = self.look;
                    crate::widgets::group(ui, look, &title, |ui| self.elements(ui, panel, &el.children, false));
                }
            }
            ElementKind::Tabs => self.tabs(ui, panel, el),
            ElementKind::Result => self.result(ui, panel, el),
            ElementKind::List => self.list(ui, panel, el),
            ElementKind::Image => self.image(ui, panel, el),
            ElementKind::Progress => {
                let v = panel.value(&el.bind);
                match fmt::progress_fraction(&v) {
                    Some(f) => {
                        let bar = egui::ProgressBar::new(f).show_percentage().fill(self.look.accent());
                        ui.add(bar);
                    }
                    None => {
                        ui.add(egui::Spinner::new().size(16.0));
                    }
                }
            }
            ElementKind::Unknown => {
                ui.label(RichText::new(self.t("field.needs_newer")).small().color(self.look.weak_text()));
            }
            _ => {
                ui.add_enabled_ui(!read_only, |ui| self.input(ui, panel, el, compact));
            }
        });
        // What the module said about the field ("not a word").
        if let Some(key) = el.bind.state_key() {
            if let Some(msg) = panel.field_error(key) {
                ui.label(RichText::new(msg).small().color(self.look.error()));
            }
        }
    }

    fn label(&mut self, ui: &mut egui::Ui, panel: &Panel, el: &Element) {
        let text = match &el.bind {
            fmt::Bind::None => el.label.clone(),
            bind => fmt::value_text(&panel.value(bind)),
        };
        if text.is_empty() {
            return;
        }
        let rich = RichText::new(text);
        let rich = match el.style {
            LabelStyle::Normal => rich.color(self.look.text()),
            LabelStyle::Weak => rich.color(self.look.weak_text()),
            LabelStyle::Error => rich.color(self.look.error()),
            LabelStyle::Heading => rich.strong().size(self.look.text_font().size * 1.2).color(self.look.text()),
        };
        let label = crate::bidi::auto(ui, rich);
        let r = ui.add(egui::Label::new(label).wrap().selectable(true));
        if !el.help.is_empty() {
            r.on_hover_text(&el.help);
        }
    }

    fn link(&mut self, ui: &mut egui::Ui, panel: &Panel, el: &Element) {
        let url = match &el.bind {
            fmt::Bind::None => el.url.clone(),
            bind => fmt::value_text(&panel.value(bind)),
        };
        let text = if el.label.is_empty() { url.clone() } else { el.label.clone() };
        if text.is_empty() {
            return;
        }
        if fmt::safe_link(&url) {
            // The address always shows on hover: a link's text cannot hide
            // where it goes.
            if ui.link(RichText::new(text).color(self.look.accent())).on_hover_text(url.trim()).clicked() {
                self.out.push(Out::OpenUrl(url));
            }
        } else {
            ui.label(RichText::new(text).color(self.look.weak_text()));
        }
    }

    /// An action's button, with a spinner while it runs. `small` for a
    /// list row's buttons.
    fn button(&mut self, ui: &mut egui::Ui, panel: &mut Panel, action: &str, element: &str, small: bool) -> bool {
        self.button_with(ui, panel, action, element, None, small)
    }

    fn button_with(
        &mut self,
        ui: &mut egui::Ui,
        panel: &mut Panel,
        action: &str,
        element: &str,
        item: Option<&Value>,
        small: bool,
    ) -> bool {
        let page = panel.page().clone();
        let Some(view) = page.action(action) else {
            ui.label(RichText::new(action).small().color(self.look.weak_text()));
            return false;
        };
        let blockers = panel.blockers(action);
        let running = panel.running().map(str::to_owned);
        let enabled = blockers.is_empty() && running.is_none();
        let text = if small { RichText::new(&view.label).small() } else { RichText::new(&view.label) };
        let mut resp = ui.add_enabled(enabled, egui::Button::new(text));
        if !blockers.is_empty() {
            resp = resp.on_disabled_hover_text(self.locale.fmt("module_action.needs", &blockers.join(", ")));
        } else if !view.help.is_empty() {
            resp = resp.on_hover_text(&view.help).on_disabled_hover_text(&view.help);
        }
        let clicked = resp.clicked();
        if clicked {
            panel.trigger(action, "click", Some(element), item.cloned());
        }
        if running.as_deref() == Some(action) && !small {
            ui.add(egui::Spinner::new().size(14.0));
        }
        clicked
    }

    fn tabs(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element) {
        let id = tabs_id(self.salt, &el.id);
        let tabs: Vec<&Element> = el.children.iter().filter(|t| panel.element_state(t).0).collect();
        if tabs.is_empty() {
            return;
        }
        let mut current = ui.data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(tabs.len() - 1);
        ui.horizontal_wrapped(|ui| {
            for (i, tab) in tabs.iter().enumerate() {
                let title = if tab.label.is_empty() { tab.id.clone() } else { tab.label.clone() };
                if ui.selectable_label(current == i, title).clicked() {
                    current = i;
                    ui.data_mut(|d| d.insert_temp(id, i));
                }
            }
        });
        ui.separator();
        let tab = tabs[current];
        self.elements(ui, panel, &tab.children, false);
    }

    /// The width a control takes in its row.
    fn width(&self, ui: &egui::Ui, el: &Element, natural: f32) -> f32 {
        match el.width {
            Width::Natural => natural.min(ui.available_width()),
            Width::Fill => ui.available_width(),
            Width::Points(p) => p.min(ui.available_width()),
        }
        .max(40.0)
    }

    fn input(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element, compact: bool) {
        let current = panel.value(&el.bind);
        let label = el.label.clone();
        let show_label = !label.is_empty() && !matches!(el.kind, ElementKind::Toggle);
        let draw = |cx: &mut Self, ui: &mut egui::Ui, panel: &mut Panel| match el.kind {
            ElementKind::Text => {
                let mut s = fmt::value_text(&current);
                let w = cx.width(ui, el, if compact { 160.0 } else { ui.available_width() });
                let mut edit = egui::TextEdit::singleline(&mut s).desired_width(w).password(el.password);
                if !el.placeholder.is_empty() {
                    edit = edit.hint_text(&el.placeholder);
                }
                if el.monospace {
                    edit = edit.font(egui::TextStyle::Monospace);
                }
                let r = ui.add(edit);
                if r.changed() {
                    panel.edit(el, s.into());
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !el.on_submit.is_empty() {
                    panel.trigger(&el.on_submit, "submit", Some(&el.id), None);
                }
            }
            ElementKind::Textarea => {
                let mut s = fmt::value_text(&current);
                let w = cx.width(ui, el, ui.available_width());
                let mut edit = egui::TextEdit::multiline(&mut s).desired_width(w).desired_rows(el.rows);
                if !el.placeholder.is_empty() {
                    edit = edit.hint_text(&el.placeholder);
                }
                if el.monospace {
                    edit = edit.font(egui::TextStyle::Monospace);
                }
                let r = ui.add(edit);
                if r.changed() {
                    panel.edit(el, s.into());
                }
                let submit =
                    r.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
                if submit && !el.on_submit.is_empty() {
                    panel.trigger(&el.on_submit, "submit", Some(&el.id), None);
                }
            }
            ElementKind::Number => {
                let mut n = current.as_f64().or_else(|| current.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
                let changed = match (el.slider, el.min, el.max) {
                    (true, Some(min), Some(max)) => {
                        ui.spacing_mut().slider_width = (cx.width(ui, el, 220.0) - 70.0).max(40.0);
                        let mut s = egui::Slider::new(&mut n, min..=max);
                        if let Some(step) = el.step.filter(|s| *s > 0.0) {
                            s = s.step_by(step);
                        }
                        ui.add(s).changed()
                    }
                    _ => {
                        let speed = el.step.filter(|s| *s > 0.0).unwrap_or(0.05);
                        let range = el.min.unwrap_or(f64::NEG_INFINITY)..=el.max.unwrap_or(f64::INFINITY);
                        ui.add(egui::DragValue::new(&mut n).speed(speed).range(range)).changed()
                    }
                };
                if changed {
                    panel.edit(el, serde_json::json!(n));
                }
            }
            ElementKind::Toggle => {
                let mut b = current.as_bool().unwrap_or(false);
                if ui.checkbox(&mut b, &label).changed() {
                    panel.edit(el, b.into());
                }
            }
            ElementKind::Select => {
                let cur = fmt::value_text(&current);
                let name = |c: &glossa_modules::provider::Choice| {
                    if c.label.is_empty() {
                        c.value.clone()
                    } else {
                        c.label.clone()
                    }
                };
                let shown = el.choices.iter().find(|c| c.value == cur).map(name).unwrap_or(cur.clone());
                let mut picked = None;
                crate::dropdown::ComboBox::from_id_salt(("panel-select", cx.salt, &el.id))
                    .width(cx.width(ui, el, 200.0))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for c in &el.choices {
                            if ui.selectable_label(c.value == cur, name(c)).clicked() {
                                picked = Some(c.value.clone());
                            }
                        }
                    });
                if let Some(v) = picked {
                    panel.edit(el, v.into());
                }
            }
            ElementKind::Multiselect => cx.multiselect(ui, panel, el, &current),
            ElementKind::Lang => {
                let code = current.as_str().unwrap_or("");
                let lang = LangId::from_code(code).filter(|l| l.is_language() || *l == LangId::AUTO);
                let (shown, auto_label) = match lang {
                    Some(l) => (l, cx.t("main.auto_detect")),
                    None => (LangId::AUTO, cx.t("field.none_selected")),
                };
                let hint = cx.t("lang.search");
                let id = format!("{}-{}", cx.salt, el.id);
                let w = cx.width(ui, el, 200.0);
                if let Some(l) = crate::widgets::lang_combo(ui, &id, shown, el.allow_auto, &auto_label, &hint, &[], w) {
                    let code = if l == LangId::AUTO { "auto" } else { l.bcp47() };
                    panel.edit(el, code.into());
                }
            }
            ElementKind::File => cx.file(ui, panel, el, &current),
            _ => {}
        };
        if compact || !show_label {
            ui.horizontal(|ui| {
                if show_label {
                    let l = ui.label(&label);
                    if !el.help.is_empty() {
                        l.on_hover_text(&el.help);
                    }
                }
                draw(self, ui, panel);
            });
        } else {
            let l = ui.label(&label);
            if !el.help.is_empty() {
                l.on_hover_text(&el.help);
            }
            draw(self, ui, panel);
        }
    }

    fn multiselect(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element, current: &Value) {
        use crate::dropdown;
        let picked: Vec<String> = current
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let name = |v: &str| {
            el.choices
                .iter()
                .find(|c| c.value == v)
                .map(|c| if c.label.is_empty() { c.value.clone() } else { c.label.clone() })
                .unwrap_or_else(|| v.to_owned())
        };
        let shown = match picked.len() {
            0 => self.t("field.none_selected"),
            1 | 2 => picked.iter().map(|v| name(v)).collect::<Vec<_>>().join(", "),
            n => self.locale.fmt("field.n_selected", &n.to_string()),
        };
        let id = ui.make_persistent_id(("panel-multi", self.salt, &el.id));
        let width = self.width(ui, el, 200.0);
        let button = dropdown::combo_button(ui, &shown, width, dropdown::is_open(ui.ctx(), id));
        if button.clicked() {
            dropdown::toggle(ui.ctx(), id);
        }
        let mut next = picked.clone();
        dropdown::show(ui, id, button.rect, button.rect.width(), 400.0, |ui, _| {
            for c in &el.choices {
                let mut on = next.contains(&c.value);
                let label = if c.label.is_empty() { c.value.clone() } else { c.label.clone() };
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        next.push(c.value.clone());
                    } else {
                        next.retain(|v| *v != c.value);
                    }
                }
            }
        });
        if next != picked {
            // In the list's order, whatever order they were ticked in.
            let ordered: Vec<Value> =
                el.choices.iter().filter(|c| next.contains(&c.value)).map(|c| c.value.clone().into()).collect();
            panel.edit(el, ordered.into());
        }
    }

    fn file(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element, current: &Value) {
        let slot_id = egui::Id::unique(("panel-file-pick", self.salt, &el.id));
        if let Some(slot) = ui.data(|d| d.get_temp::<PickSlot>(slot_id)) {
            if let Some(picked) = slot.lock().take() {
                ui.data_mut(|d| d.remove::<PickSlot>(slot_id));
                if let Some(path) = picked {
                    panel.edit(el, path.into());
                }
            }
        }
        let mut s = fmt::value_text(current);
        let waiting = ui.data(|d| d.get_temp::<PickSlot>(slot_id)).is_some();
        let w = (self.width(ui, el, ui.available_width()) - 90.0).max(60.0);
        let mut edit = egui::TextEdit::singleline(&mut s).desired_width(w);
        if !el.placeholder.is_empty() {
            edit = edit.hint_text(&el.placeholder);
        }
        if ui.add(edit).changed() {
            panel.edit(el, s.into());
        }
        // The dialog runs on a worker so the panel keeps drawing.
        if ui.add_enabled(!waiting, egui::Button::new(self.t("field.browse"))).clicked() {
            let slot = PickSlot::default();
            ui.data_mut(|d| d.insert_temp(slot_id, slot.clone()));
            let title = if el.label.is_empty() { el.id.clone() } else { el.label.clone() };
            let exts: Vec<String> = el.filters.iter().map(|f| f.trim_start_matches('.').to_owned()).collect();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let mut dialog = rfd::FileDialog::new();
                if !exts.is_empty() {
                    dialog = dialog.add_filter(&title, &exts);
                }
                *slot.lock() = Some(dialog.pick_file().map(|p| p.display().to_string()));
                ctx.request_repaint();
            });
        }
    }

    fn result(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element) {
        let v = panel.value(&el.bind);
        if v.is_null() || v.as_str().is_some_and(|s| s.is_empty()) {
            return;
        }
        if !el.label.is_empty() {
            ui.label(RichText::new(&el.label).small().color(self.look.weak_text()));
        }
        let height = el.height;
        let body = |cx: &mut Self, ui: &mut egui::Ui| match el.result {
            ResultKind::Translation => cx.translation(ui, &fmt::value_text(&v)),
            ResultKind::Article => match serde_json::from_value::<glossa_core::Article>(v.clone()) {
                Ok(a) => match crate::rich::article(ui, cx.look, &a, 1.0) {
                    Some(crate::rich::ArticleClick::Open(u)) => cx.out.push(Out::OpenUrl(u)),
                    Some(crate::rich::ArticleClick::Listen(t)) => cx.out.push(Out::Speak(t)),
                    Some(crate::rich::ArticleClick::Lookup(_)) | None => {}
                },
                // Not an article after all: show what came.
                Err(_) => cx.plain(ui, &fmt::value_text(&v), false),
            },
            ResultKind::Text => cx.plain(ui, &fmt::value_text(&v), el.monospace),
            ResultKind::Markdown => markdown(ui, cx.look, &fmt::value_text(&v)),
            ResultKind::Table => cx.table(ui, el, &v),
            ResultKind::Json => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                cx.plain(ui, &text, true);
            }
        };
        match height {
            Some(h) => {
                egui::ScrollArea::vertical()
                    .id_salt(("panel-result", self.salt, &el.id))
                    .max_height(h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| body(self, ui));
            }
            None => body(self, ui),
        }
    }

    /// A translation as the popup shows one: large, selectable, with copy
    /// and listen buttons.
    fn translation(&mut self, ui: &mut egui::Ui, text: &str) {
        let size = self.look.text_font().size * 1.15;
        let rich = crate::bidi::auto(ui, RichText::new(text).size(size).color(self.look.text()));
        ui.add(egui::Label::new(rich).wrap().selectable(true));
        ui.horizontal(|ui| {
            let look = self.look;
            if crate::icons::sized_button(ui, look, crate::icons::Icon::Copy, &self.t("edit.copy"), 22.0, true)
                .clicked()
            {
                self.out.push(Out::Copy(text.to_owned()));
            }
            if crate::icons::sized_button(ui, look, crate::icons::Icon::Listen, &self.t("edit.listen"), 22.0, true)
                .clicked()
            {
                self.out.push(Out::Speak(text.to_owned()));
            }
        });
    }

    fn plain(&mut self, ui: &mut egui::Ui, text: &str, mono: bool) {
        let mut rich = RichText::new(text).color(self.look.text());
        if mono {
            rich = rich.monospace();
        }
        let rich = crate::bidi::auto(ui, rich);
        ui.add(egui::Label::new(rich).wrap().selectable(true));
    }

    fn table(&mut self, ui: &mut egui::Ui, el: &Element, v: &Value) {
        let (header, rows) = fmt::table_cells(v);
        egui::Grid::new(("panel-table", self.salt, &el.id)).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            if !header.is_empty() {
                for h in &header {
                    ui.label(RichText::new(h).strong().color(self.look.text()));
                }
                ui.end_row();
            }
            for row in &rows {
                for cell in row {
                    ui.add(egui::Label::new(RichText::new(cell).color(self.look.text())).selectable(true));
                }
                ui.end_row();
            }
        });
    }

    /// Rows of `{title, subtitle, actions}`; clicking a row runs the list's
    /// `on_click` with the row as the event's item.
    fn list(&mut self, ui: &mut egui::Ui, panel: &mut Panel, el: &Element) {
        let v = panel.value(&el.bind);
        let raw: Vec<Value> = v.as_array().cloned().unwrap_or_default();
        let rows = fmt::list_rows(&v);
        if rows.is_empty() {
            return;
        }
        if !el.label.is_empty() {
            ui.label(RichText::new(&el.label).small().color(self.look.weak_text()));
        }
        let title_h = ui.text_style_height(&egui::TextStyle::Body);
        let sub_h = ui.text_style_height(&egui::TextStyle::Small);
        let row_h = title_h + if rows.iter().any(|r| !r.subtitle.is_empty()) { sub_h + 2.0 } else { 0.0 } + 8.0;
        let max_h = el.height.unwrap_or(280.0);
        let running = panel.running().is_some();
        egui::ScrollArea::vertical()
            .id_salt(("panel-list", self.salt, &el.id))
            .max_height(max_h)
            .auto_shrink([false, true])
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for i in range {
                    let row = &rows[i];
                    let item = raw.get(i).cloned().unwrap_or(Value::Null);
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::click());
                    let clickable = !el.on_click.is_empty() && !running;
                    if resp.hovered() && clickable {
                        ui.painter().rect_filled(rect, 3.0, self.look.edit_back());
                    }
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(6.0, 4.0))));
                    // The row's buttons first, at the right; the text takes
                    // what is left, one line each, so every row has one
                    // height and only those in view are drawn.
                    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for a in row.actions.iter().rev() {
                            self.button_with(ui, panel, a, &el.id, Some(&item), true);
                        }
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(RichText::new(&row.title).color(self.look.text())).truncate());
                            if !row.subtitle.is_empty() {
                                let sub = RichText::new(&row.subtitle).small().color(self.look.weak_text());
                                ui.add(egui::Label::new(sub).truncate()).on_hover_text(&row.subtitle);
                            }
                        });
                    });
                    if resp.clicked() && clickable {
                        panel.trigger(&el.on_click, "click", Some(&el.id), Some(item));
                    }
                }
            });
    }

    /// A base64 PNG or JPEG, decoded once per value into a texture kept in
    /// the context's memory; one too large is not decoded.
    fn image(&mut self, ui: &mut egui::Ui, panel: &Panel, el: &Element) {
        let v = panel.value(&el.bind);
        let Some(b64) = v.as_str().filter(|s| !s.is_empty()) else { return };
        let key = egui::Id::unique(("panel-image", self.salt, &el.id, fxhash(b64)));
        type Slot = Option<egui::TextureHandle>;
        let cached: Option<Slot> = ui.data(|d| d.get_temp::<Slot>(key));
        let tex = match cached {
            Some(t) => t,
            None => {
                let t = decode_image(b64).map(|img| {
                    ui.ctx().load_texture(format!("panel-image-{}", el.id), img, egui::TextureOptions::LINEAR)
                });
                ui.data_mut(|d| d.insert_temp::<Slot>(key, t.clone()));
                t
            }
        };
        match tex {
            Some(t) => {
                let size = t.size_vec2();
                let max_w = self.width(ui, el, ui.available_width());
                let max_h = el.height.unwrap_or(400.0);
                let scale = (max_w / size.x).min(max_h / size.y).min(1.0);
                ui.add(egui::Image::new(&t).fit_to_exact_size(size * scale));
            }
            None => {
                ui.label(RichText::new(self.t("panel.bad_image")).small().color(self.look.weak_text()));
            }
        }
    }
}

/// A cheap hash of a (possibly long) string, to key a decoded image.
fn fxhash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.len().hash(&mut h);
    s.hash(&mut h);
    h.finish()
}

/// Decode a base64 PNG or JPEG (a `data:` prefix is allowed) within
/// [`fmt::MAX_IMAGE_SIDE`].
fn decode_image(b64: &str) -> Option<egui::ColorImage> {
    let data = b64.split_once("base64,").map(|(_, d)| d).unwrap_or(b64);
    let bytes = base64_decode(data.trim())?;
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > fmt::MAX_IMAGE_SIDE || h > fmt::MAX_IMAGE_SIDE {
        return None;
    }
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// Standard base64, without a dependency: the UI crate has none for it.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// A small Markdown subset as egui text: `#` headings, `-`/`*`/`1.` list
/// items, fenced code, and inline `**bold**`, `*italics*` and `` `code` ``.
/// Links stay text: a result is never a way to open something.
fn markdown(ui: &mut egui::Ui, look: &Look, src: &str) {
    let base = look.text_font().size;
    let mut in_code = false;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            ui.label(RichText::new(line).monospace().color(look.text()));
            continue;
        }
        if trimmed.is_empty() {
            ui.add_space(base * 0.4);
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && trimmed[level..].starts_with(' ') {
            let size = base * (1.0 + 0.5 / level as f32);
            ui.label(RichText::new(trimmed[level..].trim()).strong().size(size).color(look.text()));
            continue;
        }
        let (bullet, rest) = if let Some(r) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            (Some("•".to_owned()), r)
        } else if let Some((n, r)) =
            trimmed.split_once(". ").filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
        {
            (Some(format!("{n}.")), r)
        } else {
            (None, trimmed)
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if let Some(b) = bullet {
                ui.label(RichText::new(format!("{b} ")).color(look.weak_text()));
            }
            for (text, style) in inline_spans(rest) {
                let mut r = RichText::new(text).size(base).color(look.text());
                match style {
                    Span::Bold => r = r.strong(),
                    Span::Italic => r = r.italics(),
                    Span::Code => r = r.monospace(),
                    Span::Plain => {}
                }
                ui.add(egui::Label::new(r).selectable(true));
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Plain,
    Bold,
    Italic,
    Code,
}

/// Split a line into runs of plain, bold, italic and code text. Unclosed
/// markers stay as written.
fn inline_spans(s: &str) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    let mut rest = s;
    let mut plain = String::new();
    while !rest.is_empty() {
        let found = [("**", Span::Bold), ("`", Span::Code), ("*", Span::Italic), ("_", Span::Italic)]
            .into_iter()
            .find_map(|(m, span)| {
                let body = rest.strip_prefix(m)?;
                let end = body.find(m)?;
                (end > 0).then(|| (body[..end].to_owned(), span, m.len() * 2 + end))
            });
        match found {
            Some((text, span, used)) => {
                if !plain.is_empty() {
                    out.push((std::mem::take(&mut plain), Span::Plain));
                }
                out.push((text, span));
                rest = &rest[used..];
            }
            None => {
                let c = rest.chars().next().expect("not empty");
                plain.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !plain.is_empty() {
        out.push((plain, Span::Plain));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_spans_split_and_leave_unclosed_markers() {
        let spans = inline_spans("a **b** c *d* `e` f_");
        assert_eq!(
            spans,
            [
                ("a ".to_owned(), Span::Plain),
                ("b".to_owned(), Span::Bold),
                (" c ".to_owned(), Span::Plain),
                ("d".to_owned(), Span::Italic),
                (" ".to_owned(), Span::Plain),
                ("e".to_owned(), Span::Code),
                (" f_".to_owned(), Span::Plain),
            ]
        );
        assert_eq!(inline_spans("**"), [("**".to_owned(), Span::Plain)]);
        assert_eq!(inline_spans("привет *мир*")[1], ("мир".to_owned(), Span::Italic));
    }

    #[test]
    fn images_decode_within_bounds() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert!(base64_decode("not base64!").is_none());
        // A 1x1 PNG.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let img = decode_image(png).unwrap();
        assert_eq!(img.size, [1, 1]);
        assert!(decode_image(&format!("data:image/png;base64,{png}")).is_some());
        assert!(decode_image("aGVsbG8=").is_none(), "not a picture");
    }
}
