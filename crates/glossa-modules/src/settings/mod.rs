//! A provider's settings page as data: which fields, in what layout, in
//! which language, which are shown and enabled, and which values are wrong.
//!
//! The UI only draws a [`SettingsPage`]; everything that decides something
//! lives here, so `glossa-cli module check`, the control API and a future
//! front end see the same rules. The pieces:
//!
//! * [`condition`]: the `visible_if` / `enabled_if` expressions.
//! * [`validate`]: the declarative checks on one value.
//! * [`strings`]: a module's `locales/<lang>.toml`.
//! * this file: the optional `[ui.settings]` layout, building the page, and
//!   the manifest lint for all of the above.
//!
//! ```toml
//! [ui]
//! format = 1
//!
//! [ui.settings]
//! layout = ["api_key", "model", "advanced"]   # top level, in order
//!
//! [[ui.settings.items]]
//! id = "advanced"
//! type = "group"                               # group | row | tabs | label | separator
//! label = "Advanced"
//! collapsed = true
//! children = ["temperature", "tone"]
//! ```
//!
//! Without `[ui.settings]` the options are listed in declaration order, with
//! those that name a `group` gathered into it where the first of them stands.
//! An option the layout does not mention is never lost: it is appended at
//! the end (or to its `group`).

pub mod condition;
pub mod strings;
pub mod validate;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use condition::Condition;
pub use strings::ModuleStrings;
pub use validate::FieldError;

use crate::actions::ActionView;
use crate::manifest::Manifest;
use crate::provider::{resolve_options, OptionKind, OptionSpec, OptionValues};

/// The newest `[ui] format` this build understands.
pub const UI_FORMAT: u32 = 1;

/// `[ui]` in `module.toml`: the settings layout and the module's panels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiSection {
    /// The version of everything under `[ui]`. A newer one is drawn as far
    /// as this build understands it.
    #[serde(default = "ui_format")]
    pub format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SettingsLayout>,
    /// `[[ui.panels]]`: windows of the module's own; see [`crate::panels`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<crate::panels::PanelSpec>,
}

fn ui_format() -> u32 {
    UI_FORMAT
}

impl Default for UiSection {
    fn default() -> Self {
        Self { format: UI_FORMAT, settings: None, panels: Vec::new() }
    }
}

impl UiSection {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `[ui.settings]`: how the settings page is laid out.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsLayout {
    /// The top level, in order: option keys, item ids, and `@action` ids
    /// (an action's button, from phase 2 on). Empty: the options in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<LayoutItem>,
}

/// One `[[ui.settings.items]]` entry: a container or a decoration. Options
/// are not items; containers refer to them by key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutItem {
    pub id: String,
    /// `group`, `row`, `tabs`, `label` or `separator`. Kept as text so a
    /// type from a newer format still parses (see [`LayoutItem::kind`]).
    #[serde(rename = "type", default)]
    pub kind: String,
    /// A group's title, a tab's name, a label's text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// `group`: start folded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Containers: option keys, item ids and `@action` ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    /// `label`: `normal`, `weak`, `error` or `heading`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub style: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visible_if: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub enabled_if: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Group,
    Row,
    Tabs,
    Label,
    Separator,
    /// From a newer format: drawn as a "needs a newer Glossa" line.
    Unknown,
}

impl LayoutItem {
    pub fn kind(&self) -> ItemKind {
        match self.kind.as_str() {
            "group" => ItemKind::Group,
            "row" => ItemKind::Row,
            "tabs" => ItemKind::Tabs,
            "label" => ItemKind::Label,
            "separator" => ItemKind::Separator,
            _ => ItemKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelStyle {
    #[default]
    Normal,
    Weak,
    Error,
    Heading,
}

/// A field's or a container's conditions, parsed. An unparsable condition
/// is dropped (the field stays shown and enabled) and reported by [`lint`].
#[derive(Debug, Clone, Default)]
pub struct Conds {
    pub visible: Option<Condition>,
    pub enabled: Option<Condition>,
}

impl Conds {
    pub fn parse(visible: &str, enabled: &str) -> Self {
        let p = |s: &str| (!s.trim().is_empty()).then(|| Condition::parse(s).ok()).flatten();
        Conds { visible: p(visible), enabled: p(enabled) }
    }

    pub fn visible(&self, cx: &condition::Context) -> bool {
        self.visible.as_ref().is_none_or(|c| c.eval(cx))
    }

    pub fn enabled(&self, cx: &condition::Context) -> bool {
        self.enabled.as_ref().is_none_or(|c| c.eval(cx))
    }
}

/// A container: a group box, a row, or a tab strip.
#[derive(Debug, Clone, Default)]
pub struct Container {
    pub id: String,
    /// Translated.
    pub label: String,
    pub help: String,
    pub collapsed: bool,
    pub children: Vec<Node>,
    pub conds: Conds,
}

/// One thing on the page, in drawing order.
#[derive(Debug, Clone)]
pub enum Node {
    /// `SettingsPage::options[i]`.
    Option(usize),
    /// An action's button (`@id`), naming one of [`SettingsPage::actions`];
    /// nothing is drawn for an id the module does not declare.
    Action(String),
    Group(Container),
    Row(Container),
    /// Every child is a [`Node::Group`]: one tab each.
    Tabs(Container),
    Label {
        id: String,
        text: String,
        style: LabelStyle,
        conds: Conds,
    },
    Separator,
    /// An item type this build does not know.
    Unknown {
        id: String,
        kind: String,
    },
}

impl Node {
    /// The options under this node, at any depth.
    pub fn option_indices(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<usize>) {
        match self {
            Node::Option(i) => out.push(*i),
            Node::Group(c) | Node::Row(c) | Node::Tabs(c) => c.children.iter().for_each(|n| n.collect(out)),
            _ => {}
        }
    }

    fn find_group_mut(&mut self, id: &str) -> Option<&mut Container> {
        let is_group = matches!(self, Node::Group(_));
        match self {
            Node::Group(c) | Node::Row(c) | Node::Tabs(c) => {
                if is_group && c.id == id {
                    return Some(c);
                }
                c.children.iter_mut().find_map(|n| n.find_group_mut(id))
            }
            _ => None,
        }
    }
}

/// A settings page ready to draw: the options with their text in the
/// interface language, the layout tree, and the compiled conditions and
/// patterns.
#[derive(Debug, Clone, Default)]
pub struct SettingsPage {
    /// The module's name and description in the interface language, when
    /// its locale file translates them.
    pub name: Option<String>,
    pub description: Option<String>,
    /// The provider's options, translated; [`Node::Option`] indexes this.
    pub options: Vec<OptionSpec>,
    pub nodes: Vec<Node>,
    /// The module's `[[actions]]`, translated; [`Node::Action`] names one.
    pub actions: Vec<ActionView>,
    conds: Vec<Conds>,
    patterns: Vec<Option<validate::Pattern>>,
}

/// Where each field stands, for drawing.
#[derive(Debug, Clone, Default)]
pub struct FieldState {
    pub visible: bool,
    pub enabled: bool,
    pub error: Option<FieldError>,
}

impl SettingsPage {
    /// The page for `specs` with no layout and no translations: what every
    /// built-in provider gets.
    pub fn plain(specs: &[OptionSpec]) -> Self {
        Self::build(specs, None, &ModuleStrings::default(), "en")
    }

    /// The page for a module: its options, `[ui.settings]` and locale files.
    pub fn for_manifest(m: &Manifest, lang: &str) -> Self {
        let mut page = Self::build(&m.options, m.ui.settings.as_ref(), &m.strings, lang);
        page.name = m.strings.get(lang, "module.name").map(str::to_owned);
        page.description = m.strings.get(lang, "module.description").map(str::to_owned);
        page.actions = crate::actions::views(m, lang);
        page
    }

    /// The action `id` declares, if the module has one by that id.
    pub fn action(&self, id: &str) -> Option<&ActionView> {
        self.actions.iter().find(|a| a.id == id)
    }

    /// The options `action` needs that are empty or fail their checks for
    /// `user`'s values, as indices into [`SettingsPage::options`]. Empty:
    /// the action may run.
    ///
    /// A needed field that is hidden (`visible_if`) or greyed out
    /// (`enabled_if`) does not block: the user could not fill it in, which
    /// is the rule the page's own checks follow, and the condition that
    /// hides it says the module does not need it in that state.
    pub fn action_blockers(&self, action: &ActionView, user: Option<&OptionValues>) -> Vec<usize> {
        let values = self.values(user);
        let states = self.states(user);
        action
            .needs
            .iter()
            .filter_map(|key| self.options.iter().position(|o| o.key == *key))
            .filter(|&i| states[i].visible && states[i].enabled)
            .filter(|&i| {
                let v = values.get(&self.options[i].key).unwrap_or(&serde_json::Value::Null);
                validate::is_empty(v) || self.check(i, &values).is_some()
            })
            .collect()
    }

    /// This page with the choices `choices_from` actions loaded, by option
    /// key, in place of the declared ones; from then on the checks use the
    /// loaded list. A loaded value the manifest also declares keeps the
    /// declared label, which the module's locale file may translate.
    pub fn with_loaded_choices(&self, loaded: &BTreeMap<String, Vec<crate::provider::Choice>>) -> SettingsPage {
        let mut page = self.clone();
        for o in &mut page.options {
            let Some(list) = loaded.get(&o.key).filter(|_| !o.choices_from.is_empty()) else { continue };
            o.choices = list
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    if let Some(d) = o.choices.iter().find(|d| d.value == c.value && !d.label.is_empty()) {
                        c.label = d.label.clone();
                    }
                    c
                })
                .collect();
            o.choices_loaded = true;
        }
        page
    }

    /// The actions the layout places as buttons (`@id`), at any depth.
    pub fn placed_actions(&self) -> BTreeSet<&str> {
        fn walk<'a>(nodes: &'a [Node], out: &mut BTreeSet<&'a str>) {
            for n in nodes {
                match n {
                    Node::Action(a) => {
                        out.insert(a.as_str());
                    }
                    Node::Group(c) | Node::Row(c) | Node::Tabs(c) => walk(&c.children, out),
                    _ => {}
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&self.nodes, &mut out);
        out
    }

    /// Build a page. `lang` is Glossa's interface language code (`en`,
    /// `ru`, a classic pack's code); `strings` supplies the translations.
    pub fn build(specs: &[OptionSpec], layout: Option<&SettingsLayout>, strings: &ModuleStrings, lang: &str) -> Self {
        let tr =
            |key: String, fallback: &str| strings.get(lang, &key).map(str::to_owned).unwrap_or(fallback.to_owned());
        let options: Vec<OptionSpec> = specs
            .iter()
            .map(|s| {
                let k = &s.key;
                let mut s = s.clone();
                s.label = tr(format!("options.{k}.label"), &s.label);
                s.help = tr(format!("options.{k}.help"), &s.help);
                s.placeholder = tr(format!("options.{k}.placeholder"), &s.placeholder);
                s.pattern_message = tr(format!("options.{k}.pattern_message"), &s.pattern_message);
                for c in &mut s.choices {
                    c.label = tr(format!("options.{k}.choices.{}", c.value), &c.label);
                }
                s
            })
            .collect();
        let conds = options.iter().map(|o| Conds::parse(&o.visible_if, &o.enabled_if)).collect();
        let patterns = options
            .iter()
            .map(|o| (!o.pattern.is_empty()).then(|| validate::Pattern::new(&o.pattern).ok()).flatten())
            .collect();

        let empty = SettingsLayout::default();
        let layout = layout.unwrap_or(&empty);
        let mut b = Builder {
            options: &options,
            items: &layout.items,
            tr: &|key: String, fallback: &str| tr(key, fallback),
            used: BTreeSet::new(),
            placed: BTreeSet::new(),
            depth: 0,
        };
        let mut nodes: Vec<Node> = layout.layout.iter().filter_map(|id| b.resolve(id)).collect();
        for (i, option) in options.iter().enumerate() {
            if b.used.contains(&i) {
                continue;
            }
            let group = option.group.trim();
            if group.is_empty() {
                b.used.insert(i);
                nodes.push(Node::Option(i));
                continue;
            }
            if nodes.iter_mut().all(|n| n.find_group_mut(group).is_none()) {
                // A declared group nobody placed, or one the option invents.
                let declared = layout.items.iter().any(|it| it.id == group && it.kind() == ItemKind::Group);
                let node = if declared {
                    b.resolve(group)
                } else {
                    Some(Node::Group(Container {
                        id: group.to_owned(),
                        label: tr(format!("settings.items.{group}.label"), group),
                        ..Default::default()
                    }))
                };
                nodes.extend(node);
            }
            if b.used.insert(i) {
                if let Some(c) = nodes.iter_mut().find_map(|n| n.find_group_mut(group)) {
                    c.children.push(Node::Option(i));
                }
            }
        }
        SettingsPage { name: None, description: None, options, nodes, actions: Vec::new(), conds, patterns }
    }

    /// The values conditions and checks see: the defaults with `user`'s
    /// values over them.
    pub fn values(&self, user: Option<&OptionValues>) -> OptionValues {
        resolve_options(&self.options, user)
    }

    /// Where every option stands for `user`'s values (the draft, on the
    /// settings page), indexed like [`SettingsPage::options`]. A field inside
    /// a hidden or disabled container is hidden or disabled too. Hidden and
    /// disabled fields are not checked: the user could not fix them.
    pub fn states(&self, user: Option<&OptionValues>) -> Vec<FieldState> {
        let values = self.values(user);
        let cx = condition::Context { options: &values, state: None };
        let mut out = vec![FieldState::default(); self.options.len()];
        fn walk(
            page: &SettingsPage,
            nodes: &[Node],
            cx: &condition::Context,
            shown: bool,
            on: bool,
            out: &mut [FieldState],
        ) {
            for n in nodes {
                match n {
                    Node::Option(i) => {
                        let c = &page.conds[*i];
                        let visible = shown && c.visible(cx);
                        let enabled = on && c.enabled(cx);
                        let error = (visible && enabled).then(|| page.check(*i, cx.options)).flatten();
                        out[*i] = FieldState { visible, enabled, error };
                    }
                    Node::Group(c) | Node::Row(c) | Node::Tabs(c) => {
                        walk(page, &c.children, cx, shown && c.conds.visible(cx), on && c.conds.enabled(cx), out);
                    }
                    _ => {}
                }
            }
        }
        walk(self, &self.nodes, &cx, true, true, &mut out);
        out
    }

    /// Check option `i` against `values` (already resolved).
    pub fn check(&self, i: usize, values: &OptionValues) -> Option<FieldError> {
        let spec = &self.options[i];
        let v = values.get(&spec.key).unwrap_or(&serde_json::Value::Null);
        validate::check(spec, self.patterns[i].as_ref(), v)
    }

    /// The fields that fail their checks, by key.
    pub fn errors(&self, user: Option<&OptionValues>) -> BTreeMap<String, FieldError> {
        self.states(user)
            .into_iter()
            .enumerate()
            .filter_map(|(i, s)| s.error.map(|e| (self.options[i].key.clone(), e)))
            .collect()
    }

    /// Whether a container or a label is shown and enabled.
    pub fn container_state(&self, conds: &Conds, user: Option<&OptionValues>) -> (bool, bool) {
        let values = self.values(user);
        let cx = condition::Context { options: &values, state: None };
        (conds.visible(&cx), conds.enabled(&cx))
    }
}

/// The last page built for a provider, by language. The options page asks
/// for the page every frame; building compiles patterns and conditions, so a
/// provider keeps the result until the interface language changes.
#[derive(Debug, Default)]
pub struct PageCache(parking_lot::Mutex<Option<(String, std::sync::Arc<SettingsPage>)>>);

impl PageCache {
    pub fn get(&self, lang: &str, build: impl FnOnce() -> SettingsPage) -> std::sync::Arc<SettingsPage> {
        let mut slot = self.0.lock();
        if let Some((l, page)) = slot.as_ref() {
            if l == lang {
                return page.clone();
            }
        }
        let page = std::sync::Arc::new(build());
        *slot = Some((lang.to_owned(), page.clone()));
        page
    }
}

struct Builder<'a> {
    options: &'a [OptionSpec],
    items: &'a [LayoutItem],
    tr: &'a dyn Fn(String, &str) -> String,
    used: BTreeSet<usize>,
    /// Items and actions already placed. Each is drawn once: a second
    /// reference would clash with the first's egui ids, a container inside
    /// itself would never end, and items that each list the next twice
    /// would otherwise build 2^N nodes.
    placed: BTreeSet<String>,
    /// How deep the item being resolved is nested.
    depth: usize,
}

/// Containers nested deeper than this are left out: the page is drawn and
/// walked recursively, every frame, on the UI thread.
pub const MAX_DEPTH: usize = 16;

impl Builder<'_> {
    fn resolve(&mut self, id: &str) -> Option<Node> {
        let id = id.trim();
        if let Some(action) = id.strip_prefix('@') {
            return self.placed.insert(id.to_owned()).then(|| Node::Action(action.to_owned()));
        }
        if let Some(i) = self.options.iter().position(|o| o.key == id) {
            // Listed twice: the second would clash with the first's widget.
            return self.used.insert(i).then_some(Node::Option(i));
        }
        let item = self.items.iter().find(|it| it.id == id)?;
        if self.depth >= MAX_DEPTH || !self.placed.insert(id.to_owned()) {
            return None;
        }
        self.depth += 1;
        let label = (self.tr)(format!("settings.items.{id}.label"), &item.label);
        let help = (self.tr)(format!("settings.items.{id}.help"), &item.help);
        let conds = Conds::parse(&item.visible_if, &item.enabled_if);
        let node = match item.kind() {
            ItemKind::Group | ItemKind::Row | ItemKind::Tabs => {
                let mut children: Vec<Node> = item.children.iter().filter_map(|c| self.resolve(c)).collect();
                if item.kind() == ItemKind::Tabs {
                    // A tab is a group; anything else gets a tab of its own.
                    children = children
                        .into_iter()
                        .map(|n| match n {
                            Node::Group(_) => n,
                            other => Node::Group(Container {
                                id: String::new(),
                                label: String::new(),
                                children: vec![other],
                                ..Default::default()
                            }),
                        })
                        .collect();
                }
                let c = Container { id: id.to_owned(), label, help, collapsed: item.collapsed, children, conds };
                match item.kind() {
                    ItemKind::Group => Node::Group(c),
                    ItemKind::Row => Node::Row(c),
                    _ => Node::Tabs(c),
                }
            }
            ItemKind::Label => Node::Label {
                id: id.to_owned(),
                text: label,
                style: match item.style.as_str() {
                    "weak" => LabelStyle::Weak,
                    "error" => LabelStyle::Error,
                    "heading" => LabelStyle::Heading,
                    _ => LabelStyle::Normal,
                },
                conds,
            },
            ItemKind::Separator => Node::Separator,
            ItemKind::Unknown => Node::Unknown { id: id.to_owned(), kind: item.kind.clone() },
        };
        self.depth -= 1;
        Some(node)
    }
}

// -- lint -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
}

/// One problem [`lint`] found, worded for a module author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    pub message: String,
}

pub(crate) fn issue(level: Level, message: String) -> Issue {
    Issue { level, message }
}

/// Check everything phase 1 added: option keys, conditions, patterns, the
/// `[ui.settings]` layout and the locale files in `strings` (with the
/// problems [`ModuleStrings::load`] reported). Used by `glossa-cli module
/// check`; the program itself never refuses a module for these.
pub fn lint(m: &Manifest, strings: &ModuleStrings, load_problems: &[String]) -> Vec<Issue> {
    let mut out = Vec::new();
    let keys: BTreeSet<&str> = m.options.iter().map(|o| o.key.as_str()).collect();
    let layout = m.ui.settings.clone().unwrap_or_default();
    let item_ids: BTreeSet<&str> = layout.items.iter().map(|i| i.id.as_str()).collect();

    if m.ui.format > UI_FORMAT {
        out.push(issue(
            Level::Warning,
            format!("[ui] format = {}: this Glossa knows format {UI_FORMAT}; newer parts are skipped", m.ui.format),
        ));
    }

    let check_cond = |what: &str, field: &str, src: &str, out: &mut Vec<Issue>| {
        if src.trim().is_empty() {
            return;
        }
        match Condition::parse(src) {
            Err(e) => out.push(issue(Level::Error, format!("{what}: {field} {src:?}: {e}"))),
            Ok(c) => {
                for (scope, path) in c.paths() {
                    if scope == condition::Scope::Options && !keys.contains(path[0].as_str()) {
                        out.push(issue(
                            Level::Warning,
                            format!("{what}: {field} reads options.{}, which is not an option", path[0]),
                        ));
                    }
                    if scope == condition::Scope::State {
                        out.push(issue(
                            Level::Note,
                            format!(
                                "{what}: {field} reads state.{}, which is always empty on the settings page",
                                path[0]
                            ),
                        ));
                    }
                }
            }
        }
    };

    for o in &m.options {
        let what = format!("option {:?}", o.key);
        if let OptionKind::Unknown(name) = &o.kind {
            out.push(issue(
                Level::Error,
                format!("{what}: unknown type {name:?} (text, secret, url, bool, number, select, multiselect, multiline, lang, file); older and newer builds may differ"),
            ));
        }
        check_cond(&what, "visible_if", &o.visible_if, &mut out);
        check_cond(&what, "enabled_if", &o.enabled_if, &mut out);
        let mut pattern = None;
        if !o.pattern.is_empty() {
            match validate::Pattern::new(&o.pattern) {
                Ok(re) => pattern = Some(re),
                Err(e) => out.push(issue(
                    Level::Error,
                    format!("{what}: pattern {:?} is not a valid regular expression: {e}", o.pattern),
                )),
            }
            if !o.kind.is_text() {
                out.push(issue(Level::Warning, format!("{what}: `pattern` applies to text fields only")));
            }
        } else if !o.pattern_message.is_empty() {
            out.push(issue(Level::Warning, format!("{what}: `pattern_message` without a `pattern`")));
        }
        if let (Some(a), Some(b)) = (o.min_length, o.max_length) {
            if a > b {
                out.push(issue(Level::Error, format!("{what}: min_length {a} is more than max_length {b}")));
            }
        }
        if (o.min_length.is_some() || o.max_length.is_some())
            && !(o.kind.is_text() || o.kind == OptionKind::Multiselect)
        {
            out.push(issue(
                Level::Warning,
                format!("{what}: min_length/max_length apply to text and multiselect fields"),
            ));
        }
        if let (Some(a), Some(b)) = (o.min, o.max) {
            if a > b {
                out.push(issue(Level::Error, format!("{what}: min {a} is more than max {b}")));
            }
        }
        if o.step.is_some_and(|s| s <= 0.0) {
            out.push(issue(Level::Error, format!("{what}: step must be more than 0")));
        }
        if o.step.is_some() && o.kind != OptionKind::Number {
            out.push(issue(Level::Warning, format!("{what}: `step` applies to numbers only")));
        }
        match o.widget.as_str() {
            "" => {}
            "slider" if o.kind == OptionKind::Number => {
                if o.min.is_none() || o.max.is_none() {
                    out.push(issue(Level::Error, format!("{what}: widget = \"slider\" needs both min and max")));
                }
            }
            w => out.push(issue(
                Level::Warning,
                format!("{what}: unknown widget {w:?} for this type; the usual control is drawn"),
            )),
        }
        if o.allow_auto && o.kind != OptionKind::Lang {
            out.push(issue(Level::Warning, format!("{what}: `allow_auto` applies to lang fields only")));
        }
        if !o.filters.is_empty() && o.kind != OptionKind::File {
            out.push(issue(Level::Warning, format!("{what}: `filters` applies to file fields only")));
        }
        if o.kind == OptionKind::Multiselect && o.choices.is_empty() {
            out.push(issue(Level::Error, format!("{what}: multiselect option has no `choices`")));
        }
        // A default that fails its own checks greets every user with an
        // error. "Required" is fine: that is a field to fill in. Number, select
        // and bool defaults are checked by the older option lint in
        // glossa-module-test, as errors.
        let checked_elsewhere = matches!(o.kind, OptionKind::Number | OptionKind::Select | OptionKind::Bool);
        if !o.default.is_null() && !matches!(o.kind, OptionKind::Unknown(_)) && !checked_elsewhere {
            if let Some(e) = validate::check(o, pattern.as_ref(), &o.default) {
                if e.key != "field.required" {
                    out.push(issue(
                        Level::Warning,
                        format!("{what}: the default {} fails its own check: {e}", o.default),
                    ));
                }
            }
        }
        if !o.group.is_empty() {
            if let Some(it) = layout.items.iter().find(|i| i.id == o.group) {
                if it.kind() != ItemKind::Group {
                    out.push(issue(Level::Error, format!("{what}: group {:?} is a {}, not a group", o.group, it.kind)));
                }
            }
        }
    }

    // The layout.
    let mut seen_items = BTreeSet::new();
    for it in &layout.items {
        let what = format!("[ui.settings] item {:?}", it.id);
        if it.id.trim().is_empty() {
            out.push(issue(Level::Error, "[ui.settings] an item has no `id`".into()));
            continue;
        }
        if !seen_items.insert(it.id.as_str()) {
            out.push(issue(Level::Error, format!("{what} is declared twice")));
        }
        if keys.contains(it.id.as_str()) {
            out.push(issue(Level::Error, format!("{what} has the same id as an option")));
        }
        match it.kind() {
            ItemKind::Unknown => out.push(issue(
                Level::Warning,
                format!("{what}: unknown type {:?} (group, row, tabs, label, separator); it shows as needing a newer Glossa", it.kind),
            )),
            ItemKind::Group | ItemKind::Row | ItemKind::Tabs if it.children.is_empty() => {
                let grouped = m.options.iter().any(|o| o.group == it.id);
                if !grouped {
                    out.push(issue(Level::Warning, format!("{what} has no children")));
                }
            }
            ItemKind::Label if it.label.is_empty() => out.push(issue(Level::Warning, format!("{what}: a label needs `label`"))),
            _ => {}
        }
        if it.kind() == ItemKind::Tabs {
            for c in &it.children {
                let is_group = layout.items.iter().any(|x| x.id == *c && x.kind() == ItemKind::Group);
                if !is_group {
                    out.push(issue(
                        Level::Warning,
                        format!("{what}: child {c:?} is not a group; each tab should be one"),
                    ));
                }
            }
        }
        if it.kind() == ItemKind::Label
            && !it.style.is_empty()
            && !["normal", "weak", "error", "heading"].contains(&it.style.as_str())
        {
            out.push(issue(Level::Warning, format!("{what}: unknown style {:?}", it.style)));
        }
        check_cond(&what, "visible_if", &it.visible_if, &mut out);
        check_cond(&what, "enabled_if", &it.enabled_if, &mut out);
    }
    let mut referenced: BTreeMap<&str, usize> = BTreeMap::new();
    let lists = std::iter::once(("[ui.settings] layout".to_owned(), &layout.layout))
        .chain(layout.items.iter().map(|it| (format!("[ui.settings] item {:?}", it.id), &it.children)));
    for (what, list) in lists {
        for id in list {
            let id = id.trim();
            if let Some(action) = id.strip_prefix('@') {
                if !m.actions.iter().any(|a| a.id == action) {
                    out.push(issue(
                        Level::Error,
                        format!(
                            "{what}: `@{action}` names an action that [[actions]] does not declare; no button is drawn"
                        ),
                    ));
                }
                *referenced.entry(id).or_default() += 1;
                continue;
            }
            if !keys.contains(id) && !item_ids.contains(id) {
                out.push(issue(Level::Error, format!("{what}: {id:?} is neither an option nor an item")));
            }
            *referenced.entry(id).or_default() += 1;
        }
    }
    for (id, n) in &referenced {
        if *n > 1 {
            out.push(issue(
                Level::Warning,
                format!("[ui.settings]: {id:?} is placed {n} times; only the first is drawn"),
            ));
        }
    }
    if m.ui.settings.is_some() {
        for it in &layout.items {
            let targeted = m.options.iter().any(|o| o.group == it.id);
            if !referenced.contains_key(it.id.as_str()) && !targeted && !layout.layout.is_empty() {
                out.push(issue(Level::Warning, format!("[ui.settings] item {:?} is not placed anywhere", it.id)));
            }
        }
        for o in &m.options {
            if !o.group.is_empty() && referenced.contains_key(o.key.as_str()) {
                out.push(issue(
                    Level::Note,
                    format!("option {:?}: `group` is ignored because the layout places the option itself", o.key),
                ));
            }
        }
        // A cycle: a container that contains itself.
        // Each item is visited once per search, so a layout that lists the
        // same items many times costs no more than one that does not.
        let children = |id: &str| -> &[String] {
            layout.items.iter().find(|x| x.id == id).map(|x| x.children.as_slice()).unwrap_or(&[])
        };
        for it in &layout.items {
            let mut seen = BTreeSet::new();
            let mut todo: Vec<&str> = children(&it.id).iter().map(|c| c.trim()).collect();
            let mut cyclic = false;
            while let Some(id) = todo.pop() {
                if id == it.id {
                    cyclic = true;
                    break;
                }
                if seen.insert(id) {
                    todo.extend(children(id).iter().map(|c| c.trim()));
                }
            }
            if cyclic {
                out.push(issue(Level::Error, format!("[ui.settings] item {:?} contains itself", it.id)));
            }
        }
        // Nesting depth, longest chain first; a cycle counts as a dead end.
        fn depth<'a>(
            id: &'a str,
            children: &dyn Fn(&str) -> &'a [String],
            memo: &mut BTreeMap<&'a str, usize>,
            open: &mut BTreeSet<&'a str>,
        ) -> usize {
            if let Some(d) = memo.get(id) {
                return *d;
            }
            if open.len() > MAX_DEPTH {
                // Too deep already; no need to recurse further to say so.
                return MAX_DEPTH + 1;
            }
            if !open.insert(id) {
                return 0;
            }
            let d = 1 + children(id).iter().map(|c| depth(c.trim(), children, memo, open)).max().unwrap_or(0);
            open.remove(id);
            memo.insert(id, d);
            d
        }
        let mut memo = BTreeMap::new();
        let deepest = layout.items.iter().map(|it| depth(&it.id, &children, &mut memo, &mut BTreeSet::new())).max();
        if deepest.is_some_and(|d| d > MAX_DEPTH) {
            out.push(issue(
                Level::Error,
                format!("[ui.settings]: containers nest more than {MAX_DEPTH} deep; the deeper ones are left out"),
            ));
        }
    }

    out.extend(crate::actions::lint(m));
    out.extend(crate::panels::lint(m));

    // Locale files.
    for p in load_problems {
        out.push(issue(Level::Error, p.clone()));
    }
    for lang in strings.languages() {
        let Some(table) = strings.table(lang) else { continue };
        for key in table.keys() {
            if let Some(why) = unknown_string_key(m, &keys, &item_ids, key) {
                out.push(issue(Level::Warning, format!("locales/{lang}.toml: `{key}` {why}; it is never shown")));
            }
        }
    }
    out
}

/// Why `key` in a locale file names nothing, if it does not.
fn unknown_string_key(m: &Manifest, keys: &BTreeSet<&str>, items: &BTreeSet<&str>, key: &str) -> Option<String> {
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["module", "name" | "description"] => None,
        // Strings for code are free-form.
        ["strings", ..] => None,
        ["panels", ..] => crate::panels::unknown_string_key(m, &parts[1..]),
        ["actions", id, rest @ ..] => {
            if !m.actions.iter().any(|a| a.id == *id) {
                return Some(format!("names action {id:?}, which does not exist"));
            }
            (!matches!(rest, ["label" | "help"])).then(|| "is not label or help".into())
        }
        ["options", k, rest @ ..] => {
            if !keys.contains(k) {
                return Some(format!("names option {k:?}, which does not exist"));
            }
            match rest {
                ["label" | "help" | "placeholder" | "pattern_message"] => None,
                ["choices", v] => {
                    let o = m.options.iter().find(|o| o.key == *k)?;
                    // Loaded choices are not in the manifest; any value may come.
                    (o.choices_from.is_empty() && !o.choices.iter().any(|c| c.value == *v))
                        .then(|| format!("names choice {v:?}, which option {k:?} lacks"))
                }
                _ => Some("is not label, help, placeholder, pattern_message or choices.<value>".into()),
            }
        }
        ["settings", "items", id, "label" | "help"] => {
            // A group an option invents with `group` has no item but can
            // still be titled.
            let invented = m.options.iter().any(|o| o.group == *id);
            (!items.contains(id) && !invented).then(|| format!("names item {id:?}, which does not exist"))
        }
        _ => Some(
            "is not a key Glossa reads (module.*, options.*, actions.*, settings.items.*, panels.*, strings.*)".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEAD: &str = "capabilities = [\"translate\"]\n[module]\nid = \"t\"\nname = \"T\"\nruntime = \"js\"\n";

    fn manifest(rest: &str) -> Manifest {
        Manifest::parse(&format!("{HEAD}{rest}")).unwrap()
    }

    /// The tree as text: options by key, containers with their children.
    fn shape(page: &SettingsPage) -> String {
        fn one(page: &SettingsPage, n: &Node) -> String {
            let kids = |c: &Container| c.children.iter().map(|n| one(page, n)).collect::<Vec<_>>().join(" ");
            match n {
                Node::Option(i) => page.options[*i].key.clone(),
                Node::Action(a) => format!("@{a}"),
                Node::Group(c) => format!("group:{}[{}]", c.id, kids(c)),
                Node::Row(c) => format!("row:{}[{}]", c.id, kids(c)),
                Node::Tabs(c) => format!("tabs:{}[{}]", c.id, kids(c)),
                Node::Label { id, .. } => format!("label:{id}"),
                Node::Separator => "---".into(),
                Node::Unknown { id, kind } => format!("?{kind}:{id}"),
            }
        }
        page.nodes.iter().map(|n| one(page, n)).collect::<Vec<_>>().join(" ")
    }

    const OPTIONS: &str = r#"
        [[options]]
        key = "a"
        [[options]]
        key = "b"
        group = "more"
        [[options]]
        key = "c"
        [[options]]
        key = "d"
        group = "more"
    "#;

    #[test]
    fn without_a_layout_options_stay_in_order_and_groups_gather() {
        let m = manifest(OPTIONS);
        assert_eq!(shape(&SettingsPage::for_manifest(&m, "en")), "a group:more[b d] c");
        let plain = SettingsPage::plain(&manifest("[[options]]\nkey = \"x\"\n[[options]]\nkey = \"y\"").options);
        assert_eq!(shape(&plain), "x y");
    }

    #[test]
    fn a_layout_orders_nests_and_never_loses_an_option() {
        let m = manifest(&format!(
            r#"{OPTIONS}
            [[options]]
            key = "e"
            [ui.settings]
            layout = ["c", "box", "@test", "nope", "c"]
            [[ui.settings.items]]
            id = "box"
            type = "tabs"
            children = ["first", "second", "a"]
            [[ui.settings.items]]
            id = "first"
            type = "group"
            label = "First"
            children = ["line", "pair"]
            [[ui.settings.items]]
            id = "second"
            type = "group"
            children = ["sep", "future"]
            [[ui.settings.items]]
            id = "pair"
            type = "row"
            children = ["b", "pair"]
            [[ui.settings.items]]
            id = "line"
            type = "label"
            label = "Hello"
            [[ui.settings.items]]
            id = "sep"
            type = "separator"
            [[ui.settings.items]]
            id = "future"
            type = "carousel"
            [[ui.settings.items]]
            id = "more"
            type = "group"
            "#
        ));
        let page = SettingsPage::for_manifest(&m, "en");
        // `c` twice: drawn once. `nope`: dropped. `pair` inside itself: the
        // cycle is cut. `a` in tabs: wrapped into a tab. `d` names the
        // declared but unplaced group `more`; `e` goes to the end.
        assert_eq!(
            shape(&page),
            "c tabs:box[group:first[label:line row:pair[b]] group:second[--- ?carousel:future] group:[a]] @test group:more[d] e"
        );
    }

    #[test]
    fn repeated_and_deep_layouts_stay_small_and_bounded() {
        // Forty groups that each list the next twice: 2^40 nodes if items
        // were not placed once.
        let n = 40;
        let mut text = String::from("[[options]]\nkey = \"x\"\n[ui.settings]\nlayout = [\"g0\", \"g0\"]\n");
        for i in 0..n {
            let next = if i + 1 < n { format!("\"g{}\", \"g{}\"", i + 1, i + 1) } else { "\"x\"".into() };
            text += &format!("[[ui.settings.items]]\nid = \"g{i}\"\ntype = \"group\"\nchildren = [{next}]\n");
        }
        let m = manifest(&text);
        let start = std::time::Instant::now();
        let page = SettingsPage::for_manifest(&m, "en");
        fn count(n: &Node) -> usize {
            match n {
                Node::Group(c) | Node::Row(c) | Node::Tabs(c) => 1 + c.children.iter().map(count).sum::<usize>(),
                _ => 1,
            }
        }
        let total: usize = page.nodes.iter().map(count).sum();
        // Nested no deeper than MAX_DEPTH; `x`, past the cut, goes to the end.
        assert_eq!(total, MAX_DEPTH + 1, "{}", shape(&page));
        assert!(matches!(page.nodes.last(), Some(Node::Option(0))));
        let issues = lint(&m, &ModuleStrings::default(), &[]);
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(issues.iter().any(|i| i.message.contains("nest more than")), "{issues:?}");
        assert!(issues.iter().any(|i| i.message.contains("placed 2 times")));
        assert!(!issues.iter().any(|i| i.message.contains("contains itself")));
    }

    #[test]
    fn conditions_hide_and_disable_fields_and_their_containers() {
        let m = manifest(
            r#"
            [[options]]
            key = "mode"
            type = "select"
            default = "simple"
            choices = [{ value = "simple" }, { value = "custom" }]
            [[options]]
            key = "url"
            type = "url"
            required = true
            visible_if = "options.mode == 'custom'"
            [[options]]
            key = "key"
            type = "secret"
            [[options]]
            key = "model"
            enabled_if = "options.key"
            required = true
            [[options]]
            key = "deep"
            group = "adv"
            required = true
            [ui.settings]
            [[ui.settings.items]]
            id = "adv"
            type = "group"
            visible_if = "options.mode == 'custom'"
            "#,
        );
        let page = SettingsPage::for_manifest(&m, "en");
        let st = page.states(None);
        assert!(st[0].visible && !st[1].visible, "url hidden by default");
        assert!(st[2].enabled && !st[3].enabled, "model disabled without a key");
        assert!(!st[4].visible, "the group's condition hides its fields");
        assert!(page.errors(None).is_empty(), "hidden and disabled fields are not checked");

        let user: OptionValues =
            serde_json::from_value(json!({ "mode": "custom", "key": "k", "url": "nope" })).unwrap();
        let st = page.states(Some(&user));
        assert!(st[1].visible && st[3].enabled && st[4].visible);
        let errors = page.errors(Some(&user));
        assert_eq!(errors.keys().collect::<Vec<_>>(), ["deep", "model", "url"]);
        assert_eq!(errors["url"].key, "field.not_url");
        assert_eq!(errors["model"].key, "field.required");
    }

    #[test]
    fn translations_follow_the_language_and_fall_back() {
        let mut m = manifest(
            r#"
            [[options]]
            key = "tone"
            type = "select"
            label = "Tone"
            help = "How it sounds"
            choices = [{ value = "casual", label = "Casual" }, { value = "formal" }]
            [[options]]
            key = "x"
            label = "X"
            group = "adv"
            "#,
        );
        let (ru, _) = strings::parse(
            "[module]\nname = 'Т'\n[options.tone]\nlabel = 'Тон'\n[options.tone.choices]\nformal = 'Официальный'\n[settings.items.adv]\nlabel = 'Ещё'\n",
        )
        .unwrap();
        m.strings.insert("ru", ru);
        let page = SettingsPage::for_manifest(&m, "ru");
        assert_eq!(page.name.as_deref(), Some("Т"));
        let tone = &page.options[0];
        assert_eq!((tone.label.as_str(), tone.help.as_str()), ("Тон", "How it sounds"));
        assert_eq!((tone.choices[0].label.as_str(), tone.choices[1].label.as_str()), ("Casual", "Официальный"));
        let Node::Group(g) = &page.nodes[1] else { panic!() };
        assert_eq!(g.label, "Ещё");
        let en = SettingsPage::for_manifest(&m, "en");
        assert_eq!((en.options[0].label.as_str(), en.name.as_deref()), ("Tone", None));
        let Node::Group(g) = &en.nodes[1] else { panic!() };
        assert_eq!(g.label, "adv", "an invented group is titled by its id");
        // A classic language pack with no module translation: English.
        assert_eq!(SettingsPage::for_manifest(&m, "uk").options[0].label, "Tone");
    }

    #[test]
    fn the_lint_finds_mistakes_in_every_part() {
        let m = manifest(
            r#"
            [[options]]
            key = "k"
            pattern = "([a-"
            visible_if = "options.missing =="
            [[options]]
            key = "short"
            min_length = 5
            default = "abc"
            [[options]]
            key = "n"
            type = "number"
            widget = "slider"
            step = 0.0
            default = 5
            max = 1
            [[options]]
            key = "t"
            type = "teleport"
            enabled_if = "options.ghost"
            [[options]]
            key = "m"
            type = "multiselect"
            [ui]
            format = 2
            [ui.settings]
            layout = ["k", "g", "ghost", "@act", "k"]
            [[ui.settings.items]]
            id = "g"
            type = "tabs"
            children = ["n", "g"]
            [[ui.settings.items]]
            id = "lonely"
            type = "group"
            children = ["t"]
            "#,
        );
        let mut s = ModuleStrings::default();
        s.insert(
            "ru",
            [("options.k.label", "К"), ("options.zz.label", "?"), ("options.k.color", "?"), ("whatever", "?")]
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .collect(),
        );
        let issues = lint(&m, &s, &["locales/de.toml: broken".into()]);
        let text = issues.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join("\n");
        for want in [
            "format = 2",
            "not a valid regular expression",
            "visible_if \"options.missing ==\"",
            "slider\" needs both min and max",
            "step must be more than 0",
            "fails its own check",
            "unknown type",
            "reads options.ghost",
            "multiselect option has no `choices`",
            "\"ghost\" is neither an option nor an item",
            "`@act` names an action",
            "\"k\" is placed 2 times",
            "child \"n\" is not a group",
            "item \"g\" contains itself",
            "item \"lonely\" is not placed anywhere",
            "locales/de.toml: broken",
            "`options.zz.label` names option \"zz\"",
            "`options.k.color` is not label",
            "`whatever` is not a key Glossa reads",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        assert!(!text.contains("options.k.label`"), "a good key is not reported");
    }

    #[test]
    fn an_old_manifest_has_a_clean_lint_and_a_plain_page() {
        let m = manifest(
            "[[options]]\nkey = \"api_key\"\ntype = \"secret\"\nrequired = true\n[[options]]\nkey = \"n\"\ntype = \"number\"\nmin = 0.0\nmax = 2.0\ndefault = 1.0\n",
        );
        assert!(lint(&m, &ModuleStrings::default(), &[]).is_empty());
        assert!(m.ui.is_default());
        let page = SettingsPage::for_manifest(&m, "ru");
        assert_eq!(shape(&page), "api_key n");
        assert_eq!(page.errors(None).keys().collect::<Vec<_>>(), ["api_key"]);
    }

    #[test]
    fn actions_are_placed_and_gated_by_what_they_need() {
        let m = manifest(
            r#"
            [[options]]
            key = "api_key"
            type = "secret"
            pattern = '^ak-[a-z]{4}$'
            [[options]]
            key = "url"
            type = "url"
            [[options]]
            key = "model"
            type = "select"
            default = "small"
            choices = [{ value = "small" }]
            choices_from = "list_models"
            [[actions]]
            id = "list_models"
            label = "Load models"
            [[actions]]
            id = "test"
            label = "Test"
            needs = ["api_key", "url"]
            [ui.settings]
            layout = ["api_key", "url", "model", "@test", "@ghost"]
            "#,
        );
        let page = SettingsPage::for_manifest(&m, "en");
        assert_eq!(shape(&page), "api_key url model @test @ghost");
        assert_eq!(page.placed_actions().into_iter().collect::<Vec<_>>(), ["ghost", "test"]);
        assert!(page.action("ghost").is_none(), "an undeclared id draws nothing");
        let test = page.action("test").unwrap();
        let keys = |user: &OptionValues| {
            page.action_blockers(test, Some(user)).iter().map(|i| page.options[*i].key.clone()).collect::<Vec<_>>()
        };
        // Empty and invalid fields block; valid ones do not, required or not.
        assert_eq!(keys(&OptionValues::new()), ["api_key", "url"]);
        let mut user: OptionValues = [("api_key".to_owned(), json!("nope"))].into();
        assert_eq!(keys(&user), ["api_key", "url"]);
        user.insert("api_key".into(), json!("ak-abcd"));
        user.insert("url".into(), json!("https://x.test"));
        assert!(keys(&user).is_empty());
        assert!(page.action_blockers(page.action("list_models").unwrap(), None).is_empty());
        // Until a list is loaded, any model passes; then the list counts.
        user.insert("model".into(), json!("acme-xl"));
        assert!(page.errors(Some(&user)).is_empty(), "{:?}", page.errors(Some(&user)));
        let loaded: BTreeMap<String, Vec<crate::provider::Choice>> = [(
            "model".to_owned(),
            vec![
                crate::provider::Choice { value: "small".into(), label: "S".into() },
                crate::provider::Choice { value: "large".into(), label: String::new() },
            ],
        )]
        .into();
        let with = page.with_loaded_choices(&loaded);
        assert_eq!(with.options[2].choices.len(), 2);
        assert_eq!(with.errors(Some(&user)).keys().collect::<Vec<_>>(), ["model"], "acme-xl is not in the loaded list");
        user.insert("model".into(), json!("large"));
        assert!(with.errors(Some(&user)).is_empty());

        // A needed field that is hidden or greyed out does not block.
        let m2 = manifest(
            r#"
            [[options]]
            key = "mode"
            type = "select"
            default = "free"
            choices = [{ value = "free" }, { value = "pro" }]
            [[options]]
            key = "key"
            type = "secret"
            visible_if = "options.mode == 'pro'"
            [[options]]
            key = "url"
            type = "url"
            enabled_if = "options.mode == 'pro'"
            [[actions]]
            id = "go"
            needs = ["key", "url"]
            "#,
        );
        let p2 = SettingsPage::for_manifest(&m2, "en");
        let go = p2.action("go").unwrap();
        assert!(p2.action_blockers(go, None).is_empty(), "both needed fields are out of reach");
        let pro: OptionValues = [("mode".to_owned(), json!("pro"))].into();
        assert_eq!(p2.action_blockers(go, Some(&pro)).len(), 2, "in reach and empty: they block");
        let issues = lint(&m, &ModuleStrings::default(), &[]);
        assert!(issues
            .iter()
            .any(|i| i.message.contains("`@ghost` names an action that [[actions]] does not declare")));
        assert!(!issues.iter().any(|i| i.message.contains("@test")), "{issues:?}");
    }
}
