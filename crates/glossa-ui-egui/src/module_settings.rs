//! Draws a provider's settings page ([`glossa_modules::SettingsPage`]): the
//! fields, the `[ui.settings]` groups, rows and tabs, the conditions and the
//! messages under fields that fail their checks, and the buttons of the
//! module's `[[actions]]` with their notices.
//!
//! Everything that decides something (what is shown, enabled or wrong, in
//! which language) is decided by the page in `glossa-modules`; this only
//! draws it and writes edits into the values it is handed. [`show`] is the
//! one entry point, used by the Services page and meant for the module
//! editor's preview as well.

use std::collections::BTreeSet;

use egui::RichText;
use glossa_core::locale::Locale;
use glossa_core::LangId;
use glossa_modules::settings::{Container, FieldState, LabelStyle, Node};
use glossa_modules::{OptionKind, OptionSpec, OptionValues, SettingsPage};

use crate::module_actions::{self, Level, Runner};
use crate::style::Look;

/// Width of a field's control in a full-width row.
const CONTROL_WIDTH: f32 = 320.0;
/// Width of a control inside a `row`, where several share the line.
const COMPACT_WIDTH: f32 = 140.0;

/// Draw `page`, editing `values` (the Options draft, or any scratch map).
/// `id_salt` keeps two pages on one screen apart (the provider id will do);
/// `show_secrets` unmasks `secret` fields. Returns whether a value changed.
/// Action buttons are drawn but cannot be pressed; see [`show_with`]. Kept
/// for previews (the module editor), which have no module to run.
#[allow(dead_code)]
pub fn show(
    ui: &mut egui::Ui,
    look: &Look,
    locale: &Locale,
    page: &SettingsPage,
    values: &mut OptionValues,
    show_secrets: bool,
    id_salt: &str,
) -> bool {
    show_with(ui, look, locale, page, values, show_secrets, id_salt, None)
}

/// [`show`], with `runner` starting the module's actions when their
/// buttons are pressed (`None`: a preview, where they cannot be).
#[allow(clippy::too_many_arguments)]
pub fn show_with(
    ui: &mut egui::Ui,
    look: &Look,
    locale: &Locale,
    page: &SettingsPage,
    values: &mut OptionValues,
    show_secrets: bool,
    id_salt: &str,
    runner: Option<Runner>,
) -> bool {
    let runs = module_actions::for_page(ui.ctx(), id_salt);
    // A reply that came back since the last frame lands in the draft first,
    // so this frame draws it.
    let mut changed = runs.lock().poll(&page.options, values, locale.get("module_action.done"));
    let states = page.states(Some(values));
    let placed = page.placed_actions().into_iter().map(str::to_owned).collect();
    let mut cx = Cx {
        look,
        locale,
        page,
        states: &states,
        values,
        show_secrets,
        salt: id_salt,
        changed: false,
        runs,
        runner,
        placed,
    };
    cx.nodes(ui, &page.nodes, false);
    changed |= cx.changed;
    changed
}

/// Choose tab `index` of the `tabs` item `tabs` on the page drawn with
/// `id_salt`. For the module editor's preview, and `--screenshot`
/// (`GLOSSA_SCREENSHOT_TAB`).
pub fn select_tab(ctx: &egui::Context, id_salt: &str, tabs: &str, index: usize) {
    ctx.data_mut(|d| d.insert_temp(tabs_id(id_salt, tabs), index));
}

/// Where the chosen tab of a `tabs` item is remembered: not tied to the
/// widget path, so [`select_tab`] can reach it.
fn tabs_id(salt: &str, tabs: &str) -> egui::Id {
    egui::Id::unique(("module-tabs", salt, tabs))
}

/// A file dialog's answer on its way back from the worker: `None` while it
/// is open, then `Some(None)` for cancelled or `Some(Some(path))`.
type PickSlot = std::sync::Arc<parking_lot::Mutex<Option<Option<String>>>>;

struct Cx<'a> {
    look: &'a Look,
    locale: &'a Locale,
    page: &'a SettingsPage,
    states: &'a [FieldState],
    values: &'a mut OptionValues,
    show_secrets: bool,
    salt: &'a str,
    changed: bool,
    runs: module_actions::Shared,
    runner: Option<Runner<'a>>,
    /// Actions the layout places with `@id`; a `choices_from` action that is
    /// not among them gets a small button beside its field instead.
    placed: BTreeSet<String>,
}

impl Cx<'_> {
    fn t(&self, key: &str) -> String {
        self.locale.get(key).to_owned()
    }

    fn nodes(&mut self, ui: &mut egui::Ui, nodes: &[Node], compact: bool) {
        for n in nodes {
            self.node(ui, n, compact);
        }
    }

    fn node(&mut self, ui: &mut egui::Ui, node: &Node, compact: bool) {
        match node {
            Node::Option(i) => {
                let st = &self.states[*i];
                if !st.visible {
                    return;
                }
                let (enabled, error) = (st.enabled, st.error.clone());
                let page = self.page;
                let spec = &page.options[*i];
                ui.add_enabled_ui(enabled, |ui| {
                    if compact {
                        ui.vertical(|ui| self.field(ui, spec, true));
                    } else {
                        self.field(ui, spec, false);
                    }
                });
                if let Some(e) = error {
                    ui.label(RichText::new(e.describe(self.locale)).small().color(self.look.error()));
                }
                // What the module said about the field ("the key was refused").
                let said = self.runs.lock().field_error(&page.options, self.values, &spec.key);
                if let Some(msg) = said {
                    ui.label(RichText::new(msg).small().color(self.look.error()));
                }
                // An inline "load" button's failure shows under its field.
                if !spec.choices_from.is_empty() && !self.placed.contains(&spec.choices_from) {
                    let notice = self.runs.lock().notice(&spec.choices_from).cloned();
                    if let Some(n) = notice.filter(|n| matches!(n.level, Level::Error | Level::Warning)) {
                        ui.label(RichText::new(n.text).small().color(self.look.error()));
                    }
                }
            }
            Node::Action(id) => {
                ui.horizontal_wrapped(|ui| self.action_button(ui, id, false));
            }
            Node::Group(c) => {
                let Some(enabled) = self.container(c) else { return };
                ui.add_enabled_ui(enabled, |ui| {
                    if c.collapsed {
                        let title = if c.label.is_empty() { c.id.clone() } else { c.label.clone() };
                        egui::CollapsingHeader::new(RichText::new(title).color(self.look.text()))
                            .id_salt(("module-group", self.salt, &c.id))
                            .default_open(false)
                            .show(ui, |ui| self.nodes(ui, &c.children, false));
                    } else if c.label.is_empty() {
                        self.nodes(ui, &c.children, false);
                    } else {
                        let look = self.look;
                        crate::widgets::group(ui, look, &c.label, |ui| self.nodes(ui, &c.children, false));
                    }
                });
            }
            Node::Row(c) => {
                let Some(enabled) = self.container(c) else { return };
                ui.add_enabled_ui(enabled, |ui| {
                    ui.horizontal_wrapped(|ui| self.nodes(ui, &c.children, true));
                });
            }
            Node::Tabs(c) => {
                let Some(enabled) = self.container(c) else { return };
                ui.add_enabled_ui(enabled, |ui| self.tabs(ui, c));
            }
            Node::Label { text, style, conds, .. } => {
                let (visible, enabled) = self.page.container_state(conds, Some(self.values));
                if !visible {
                    return;
                }
                let rich = RichText::new(text);
                let rich = match style {
                    LabelStyle::Normal => rich.color(self.look.text()),
                    LabelStyle::Weak => rich.color(self.look.weak_text()),
                    LabelStyle::Error => rich.color(self.look.error()),
                    LabelStyle::Heading => rich.strong().color(self.look.text()),
                };
                ui.add_enabled(enabled, egui::Label::new(rich).wrap());
            }
            Node::Separator => {
                ui.separator();
            }
            Node::Unknown { .. } => {
                ui.label(RichText::new(self.t("field.needs_newer")).small().color(self.look.weak_text()));
            }
        }
    }

    /// An action's button, with a spinner while it runs and its notice
    /// after. `small` for the inline button beside a `choices_from` field,
    /// which leaves its notice to the field.
    fn action_button(&mut self, ui: &mut egui::Ui, id: &str, small: bool) {
        let page = self.page;
        let Some(action) = page.action(id) else { return };
        let blockers = page.action_blockers(action, Some(self.values));
        let running = self.runs.lock().running().map(str::to_owned);
        let enabled = self.runner.is_some() && blockers.is_empty() && running.is_none();
        let button = if small {
            egui::Button::new(RichText::new(&action.label).small())
        } else {
            egui::Button::new(&action.label)
        };
        let mut resp = ui.add_enabled(enabled, button);
        if !blockers.is_empty() {
            let names: Vec<String> = blockers
                .iter()
                .map(|i| {
                    let o = &page.options[*i];
                    if o.label.is_empty() {
                        o.key.clone()
                    } else {
                        o.label.clone()
                    }
                })
                .collect();
            resp = resp.on_disabled_hover_text(self.locale.fmt("module_action.needs", &names.join(", ")));
        } else if !action.help.is_empty() {
            resp = resp.on_hover_text(&action.help).on_disabled_hover_text(&action.help);
        }
        let pressed = enabled && self.runs.lock().take_press(id);
        if resp.clicked() || pressed {
            if let Some(run) = self.runner {
                let rx = run(id, self.values.clone());
                self.runs.lock().start(id, rx);
            }
        }
        if running.as_deref() == Some(id) {
            ui.add(egui::Spinner::new().size(14.0));
        } else if !small {
            let notice = self.runs.lock().notice(id).cloned();
            if let Some(n) = notice {
                let color = match n.level {
                    Level::Error | Level::Warning => self.look.error(),
                    Level::Success => self.look.text(),
                    Level::Info => self.look.weak_text(),
                };
                ui.add(egui::Label::new(RichText::new(n.text).small().color(color)).wrap());
            }
        }
    }

    /// `None` when the container is hidden; else whether it is enabled.
    fn container(&self, c: &Container) -> Option<bool> {
        let (visible, enabled) = self.page.container_state(&c.conds, Some(self.values));
        visible.then_some(enabled)
    }

    /// A strip of tabs, one per child group, and the chosen tab's fields. A
    /// tab whose fields have an error is titled in the error colour, so an
    /// error on a tab out of sight is not missed.
    fn tabs(&mut self, ui: &mut egui::Ui, c: &Container) {
        let id = tabs_id(self.salt, &c.id);
        let tabs: Vec<&Container> = c
            .children
            .iter()
            .filter_map(|n| match n {
                Node::Group(g) if self.container(g).is_some() => Some(g),
                _ => None,
            })
            .collect();
        if tabs.is_empty() {
            return;
        }
        let mut current = ui.data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(tabs.len() - 1);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (i, tab) in tabs.iter().enumerate() {
                let failing =
                    Node::Group((*tab).clone()).option_indices().iter().any(|j| self.states[*j].error.is_some());
                let title = if tab.label.is_empty() { tab.id.clone() } else { tab.label.clone() };
                let mut text = RichText::new(title);
                if failing {
                    text = text.color(self.look.error());
                }
                if ui.selectable_label(current == i, text).clicked() {
                    current = i;
                    ui.data_mut(|d| d.insert_temp(id, i));
                }
            }
        });
        ui.separator();
        let tab = tabs[current];
        let enabled = self.container(tab).unwrap_or(true);
        ui.add_enabled_ui(enabled, |ui| self.nodes(ui, &tab.children, false));
    }

    /// One field: its label (with ` *` when required, the help on hover) and
    /// its control, side by side; `compact` inside a row.
    fn field(&mut self, ui: &mut egui::Ui, spec: &OptionSpec, compact: bool) {
        let label = if spec.label.is_empty() { spec.key.clone() } else { spec.label.clone() };
        let label = if spec.required { format!("{label} *") } else { label };
        let width = if compact { COMPACT_WIDTH } else { CONTROL_WIDTH };
        if compact {
            let l = ui.label(&label);
            if !spec.help.is_empty() {
                l.on_hover_text(&spec.help);
            }
            self.control(ui, spec, width);
            return;
        }
        ui.horizontal(|ui| {
            let l = ui.label(&label);
            if !spec.help.is_empty() {
                l.on_hover_text(&spec.help);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.control(ui, spec, width));
        });
    }

    /// The choices to offer: those an action loaded, else the declared
    /// ones. A loaded value the manifest also declares keeps the declared
    /// label, which the module's locale file may translate.
    fn choices_for(&self, spec: &OptionSpec) -> Vec<glossa_modules::provider::Choice> {
        let Some(loaded) = self.runs.lock().choices(&spec.key).map(<[_]>::to_vec) else {
            return spec.choices.clone();
        };
        loaded
            .into_iter()
            .map(|mut c| {
                if let Some(d) = spec.choices.iter().find(|d| d.value == c.value && !d.label.is_empty()) {
                    c.label = d.label.clone();
                }
                c
            })
            .collect()
    }

    fn set(&mut self, key: &str, v: serde_json::Value) {
        self.values.insert(key.to_owned(), v);
        self.changed = true;
    }

    fn control(&mut self, ui: &mut egui::Ui, spec: &OptionSpec, width: f32) {
        // A select whose choices an action loads, when the layout does not
        // place that action, gets its button here. The controls are laid
        // out right to left, so it stands right of the list, and the list
        // takes what width is left.
        let inline = matches!(spec.kind, OptionKind::Select | OptionKind::Multiselect)
            && !spec.choices_from.is_empty()
            && !self.placed.contains(&spec.choices_from);
        if inline {
            let from = spec.choices_from.clone();
            self.action_button(ui, &from, true);
        }
        // A narrow pane gets narrower controls rather than a row that runs
        // off its left edge.
        let width = width.min(ui.available_width()).max(60.0);
        let current = self.values.get(&spec.key).cloned().unwrap_or_else(|| spec.default.clone());
        let salt = (self.salt.to_owned(), spec.key.clone());
        match spec.kind {
            OptionKind::Bool => {
                let mut b = current.as_bool().unwrap_or(false);
                if ui.checkbox(&mut b, "").changed() {
                    self.set(&spec.key, b.into());
                }
            }
            OptionKind::Number => {
                let mut n = current.as_f64().or_else(|| current.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
                let changed = match (spec.widget.as_str(), spec.min, spec.max) {
                    ("slider", Some(min), Some(max)) => {
                        // The slider's track plus its number field fill
                        // the control's width, not egui's default track.
                        ui.spacing_mut().slider_width = (width - 70.0).max(40.0);
                        let mut s = egui::Slider::new(&mut n, min..=max);
                        if let Some(step) = spec.step.filter(|s| *s > 0.0) {
                            s = s.step_by(step);
                        }
                        ui.add(s).changed()
                    }
                    _ => {
                        let speed = spec.step.filter(|s| *s > 0.0).unwrap_or(0.05);
                        let range = spec.min.unwrap_or(f64::NEG_INFINITY)..=spec.max.unwrap_or(f64::INFINITY);
                        ui.add(egui::DragValue::new(&mut n).speed(speed).range(range)).changed()
                    }
                };
                if changed {
                    self.set(&spec.key, serde_json::json!(n));
                }
            }
            OptionKind::Select => {
                let choices = self.choices_for(spec);
                let choices = choices.as_slice();
                let cur = current.as_str().unwrap_or("").to_owned();
                let shown = choices
                    .iter()
                    .find(|c| c.value == cur)
                    .map(|c| if c.label.is_empty() { c.value.clone() } else { c.label.clone() })
                    .unwrap_or(cur.clone());
                let mut picked = None;
                crate::dropdown::ComboBox::from_id_salt(("opt", salt))
                    .width(width.min(280.0))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for c in choices {
                            let name = if c.label.is_empty() { c.value.clone() } else { c.label.clone() };
                            if ui.selectable_label(c.value == cur, name).clicked() {
                                picked = Some(c.value.clone());
                            }
                        }
                    });
                if let Some(v) = picked {
                    self.set(&spec.key, v.into());
                }
            }
            OptionKind::Multiselect => self.multiselect(ui, spec, &current, width.min(280.0)),
            OptionKind::Lang => {
                let code = current.as_str().unwrap_or("");
                let lang = LangId::from_code(code).filter(|l| l.is_language() || *l == LangId::AUTO);
                // An unset field shows "None" rather than pretending to be a
                // language.
                let (shown, auto_label) = match lang {
                    Some(l) => (l, self.t("main.auto_detect")),
                    None => (LangId::AUTO, self.t("field.none_selected")),
                };
                let hint = self.t("lang.search");
                let id = format!("{}-{}", self.salt, spec.key);
                if let Some(l) = crate::widgets::lang_combo(
                    ui,
                    &id,
                    shown,
                    spec.allow_auto,
                    &auto_label,
                    &hint,
                    &[],
                    width.min(220.0),
                ) {
                    let code = if l == LangId::AUTO { "auto" } else { l.bcp47() };
                    self.set(&spec.key, code.into());
                }
            }
            OptionKind::File => {
                let mut s = current.as_str().unwrap_or("").to_owned();
                // The dialog runs on a worker so the UI keeps drawing; its
                // answer waits in a slot this field reads on later frames.
                let slot_id = egui::Id::unique(("module-file-pick", self.salt, &spec.key));
                let slot = ui.data(|d| d.get_temp::<PickSlot>(slot_id));
                if let Some(slot) = &slot {
                    if let Some(picked) = slot.lock().take() {
                        ui.data_mut(|d| d.remove::<PickSlot>(slot_id));
                        if let Some(path) = picked {
                            self.set(&spec.key, path.into());
                            return;
                        }
                    }
                }
                // Right to left: the button first, then the path takes the rest.
                let waiting = ui.data(|d| d.get_temp::<PickSlot>(slot_id)).is_some();
                if ui.add_enabled(!waiting, egui::Button::new(self.t("field.browse"))).clicked() {
                    let slot = PickSlot::default();
                    ui.data_mut(|d| d.insert_temp(slot_id, slot.clone()));
                    let title = if spec.label.is_empty() { spec.key.clone() } else { spec.label.clone() };
                    let exts: Vec<String> = spec.filters.iter().map(|f| f.trim_start_matches('.').to_owned()).collect();
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let mut dialog = rfd::FileDialog::new();
                        if !exts.is_empty() {
                            dialog = dialog.add_filter(&title, &exts);
                        }
                        let picked = dialog.pick_file().map(|p| p.display().to_string());
                        *slot.lock() = Some(picked);
                        ctx.request_repaint();
                    });
                }
                let edit = egui::TextEdit::singleline(&mut s)
                    .desired_width((ui.available_width() - 4.0).max(40.0))
                    .hint_text(&spec.placeholder);
                if ui.add(edit).changed() {
                    self.set(&spec.key, s.into());
                }
            }
            OptionKind::Multiline => {
                let mut s = current.as_str().unwrap_or("").to_owned();
                let edit =
                    egui::TextEdit::multiline(&mut s).desired_width(width).desired_rows(4).hint_text(&spec.placeholder);
                if ui.add(edit).changed() {
                    self.set(&spec.key, s.into());
                }
            }
            OptionKind::Text | OptionKind::Secret | OptionKind::Url => {
                let mut s = current.as_str().unwrap_or("").to_owned();
                let edit = egui::TextEdit::singleline(&mut s)
                    .desired_width(width)
                    .hint_text(&spec.placeholder)
                    .password(spec.kind == OptionKind::Secret && !self.show_secrets);
                if ui.add(edit).changed() {
                    self.set(&spec.key, s.into());
                }
            }
            OptionKind::Unknown(_) => {
                ui.label(RichText::new(self.t("field.needs_newer")).small().color(self.look.weak_text()));
            }
        }
    }

    /// Checkboxes in a dropdown that stays open while they are ticked.
    fn multiselect(&mut self, ui: &mut egui::Ui, spec: &OptionSpec, current: &serde_json::Value, width: f32) {
        use crate::dropdown;
        let choices = self.choices_for(spec);
        let choices = choices.as_slice();
        let picked: Vec<String> = current
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let name = |v: &str| {
            choices
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
        let id = ui.make_persistent_id(("multi", self.salt, &spec.key));
        let button = dropdown::combo_button(ui, &shown, width, dropdown::is_open(ui.ctx(), id));
        if button.clicked() {
            dropdown::toggle(ui.ctx(), id);
        }
        let mut next = picked.clone();
        dropdown::show(ui, id, button.rect, button.rect.width(), 400.0, |ui, _| {
            for c in choices {
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
            // Keep the declared order, whatever order they were ticked in.
            let ordered: Vec<serde_json::Value> =
                choices.iter().filter(|c| next.contains(&c.value)).map(|c| c.value.clone().into()).collect();
            self.set(&spec.key, ordered.into());
        }
    }
}
