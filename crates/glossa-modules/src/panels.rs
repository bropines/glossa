//! `[[ui.panels]]`: windows of a module's own, described as data.
//!
//! ```toml
//! [[ui.panels]]
//! id = "playground"
//! title = "Acme playground"
//! open_from = ["tray", "main_menu"]   # also always: the open_panel hotkey, the control API, the CLI
//! size = [560, 440]
//! layout = ["source", "bar", "result"]
//! persist = ["to"]                    # kept between openings
//! on_open = "refresh"                 # an action run when the panel opens
//!
//! [ui.panels.state]                   # the panel's state, with defaults
//! source = ""
//! to = "ru"
//! result = ""
//!
//! [[ui.panels.items]]
//! id = "source"
//! type = "textarea"
//! bind = "state.source"
//! on_submit = "translate_now"
//! ```
//!
//! This file holds the format: the [`PanelSpec`] as written, the
//! [`PanelPage`] built from it in the interface language (an element tree
//! with compiled conditions), the bounds every value a module puts into a
//! panel's state is held to ([`PanelPage::bound_state`]), and the manifest
//! lint. The live state of an open panel, running its actions and keeping
//! its `persist` keys belong to `glossa-engine::panels`; the desktop app
//! only draws the page.
//!
//! Everything a module puts on a panel is data: text is drawn as text, a
//! link opens only `http` and `https` addresses, an image is decoded by
//! Glossa within size limits, and nothing a module sends is ever run.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::{ActionKind, ActionSpec, ActionView};
use crate::manifest::Manifest;
use crate::provider::{Choice, OptionValues};
use crate::settings::condition::{self, Condition};
use crate::settings::{issue, Conds, Issue, LabelStyle, Level, ModuleStrings, MAX_DEPTH};

/// The longest text a state value may hold, in bytes; longer strings are
/// cut. A panel's text fields and results are drawn every frame.
pub const MAX_TEXT: usize = 256 * 1024;
/// The most rows a list (and the most entries any array) in a panel's
/// state may hold.
pub const MAX_LIST_ROWS: usize = 1000;
/// The largest base64 image a state value may hold (about 6 MB of PNG or
/// JPEG); a larger one is dropped. The decoded size is bounded again when
/// it is drawn ([`MAX_IMAGE_SIDE`]).
pub const MAX_IMAGE_BASE64: usize = 8 * 1024 * 1024;
/// The longest side of an image a panel draws, in pixels; a larger one is
/// not decoded.
pub const MAX_IMAGE_SIDE: u32 = 4096;
/// The most keys a panel's state may have.
pub const MAX_STATE_KEYS: usize = 256;
/// The most elements one panel draws; the rest are left out.
pub const MAX_ELEMENTS: usize = 500;
/// How deep values nest inside a state value before they are cut.
const MAX_VALUE_DEPTH: usize = 16;
/// The most one streamed update may hold, serialized; the largest values
/// are dropped from one that is bigger.
pub const MAX_UPDATE_BYTES: usize = 1024 * 1024;
/// The most a panel's whole state may hold, serialized: room for one
/// image and the text around it. A value that would take it over is not
/// taken.
pub const MAX_STATE_BYTES: usize = 12 * 1024 * 1024;
/// The most a panel keeps between openings, on disk.
pub const MAX_PERSIST_BYTES: usize = 1024 * 1024;
/// The largest picture a panel decodes, in pixels (16 MP: 64 MB of RGBA).
pub const MAX_IMAGE_PIXELS: u64 = 16_000_000;

/// One `[[ui.panels]]` entry, as written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelSpec {
    /// Unique within the module; `module/panel` names it from outside.
    pub id: String,
    /// The window's title after the module's name; the id when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Where Glossa offers to open it: `tray`, `main_menu`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_from: Vec<String>,
    /// The window's first size, in points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f32; 2]>,
    /// The top level, in order: item ids and `@action` ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<String>,
    /// The state's keys and their values when the panel opens.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state: BTreeMap<String, Value>,
    /// State keys kept between openings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub persist: Vec<String>,
    /// An action run when the panel opens.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_open: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<PanelItem>,
}

/// One `[[ui.panels.items]]` entry: any element of the format-1
/// vocabulary. Which keys apply depends on the type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelItem {
    pub id: String,
    /// The element's type, kept as text so one from a newer format still
    /// parses (see [`ElementKind::from_name`]).
    #[serde(rename = "type", default)]
    pub element: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// `state.<key>` (or `options.<key>`, read only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visible_if: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub enabled_if: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_change: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_submit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_click: String,
    /// `"fill"` or points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<WidthSpec>,
    /// `result`, `list`, `image`: the most height it takes, in points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    /// `label`: `normal`, `weak`, `error`, `heading`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub style: String,
    /// `textarea`: visible lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub monospace: bool,
    /// `text`: mask the value.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub password: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// `number`: `"slider"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub widget: String,
    /// `select`, `multiselect`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    /// `lang`: offer "detect".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_auto: bool,
    /// `result`: `translation`, `article`, `text`, `markdown`, `table`, `json`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// `link`: the address, when not bound.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// `file`: extensions offered by the dialog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    /// `button`: the action it runs (`@id` in a layout is the short form).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// `group`: start folded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Containers: item ids and `@action` ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
}

/// `width = "fill"` or `width = 120`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WidthSpec {
    Points(f32),
    Word(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Label,
    Link,
    Separator,
    Text,
    Textarea,
    Number,
    Toggle,
    Select,
    Multiselect,
    Lang,
    File,
    Button,
    Row,
    Group,
    Tabs,
    Result,
    List,
    Image,
    Progress,
    /// From a newer format: drawn as a "needs a newer Glossa" line.
    #[default]
    Unknown,
}

impl ElementKind {
    pub fn from_name(name: &str) -> ElementKind {
        match name {
            "label" => ElementKind::Label,
            "link" => ElementKind::Link,
            "separator" => ElementKind::Separator,
            "text" => ElementKind::Text,
            "textarea" => ElementKind::Textarea,
            "number" => ElementKind::Number,
            "toggle" => ElementKind::Toggle,
            "select" => ElementKind::Select,
            "multiselect" => ElementKind::Multiselect,
            "lang" => ElementKind::Lang,
            "file" => ElementKind::File,
            "button" => ElementKind::Button,
            "row" => ElementKind::Row,
            "group" => ElementKind::Group,
            "tabs" => ElementKind::Tabs,
            "result" => ElementKind::Result,
            "list" => ElementKind::List,
            "image" => ElementKind::Image,
            "progress" => ElementKind::Progress,
            _ => ElementKind::Unknown,
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, ElementKind::Row | ElementKind::Group | ElementKind::Tabs)
    }

    /// Whether the element edits its bound value.
    pub fn is_input(self) -> bool {
        matches!(
            self,
            ElementKind::Text
                | ElementKind::Textarea
                | ElementKind::Number
                | ElementKind::Toggle
                | ElementKind::Select
                | ElementKind::Multiselect
                | ElementKind::Lang
                | ElementKind::File
        )
    }

    /// Whether the element shows a bound value without editing it.
    fn shows_value(self) -> bool {
        matches!(
            self,
            ElementKind::Label
                | ElementKind::Link
                | ElementKind::Result
                | ElementKind::List
                | ElementKind::Image
                | ElementKind::Progress
        )
    }
}

/// How a `result` element draws its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    /// Like the popup's translation, with copy and listen buttons.
    #[default]
    Translation,
    /// A dictionary article (Glossa's article structure), like the
    /// dictionary window.
    Article,
    Text,
    /// A small subset: paragraphs, headings, lists, bold, italics, code.
    Markdown,
    /// Rows of cells: a list of lists, or a list of objects.
    Table,
    /// Any value, pretty-printed.
    Json,
}

impl ResultKind {
    fn from_name(name: &str) -> Option<ResultKind> {
        Some(match name {
            "" | "translation" => ResultKind::Translation,
            "article" => ResultKind::Article,
            "text" => ResultKind::Text,
            "markdown" => ResultKind::Markdown,
            "table" => ResultKind::Table,
            "json" => ResultKind::Json,
            _ => return None,
        })
    }
}

/// Where an element's value lives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Bind {
    #[default]
    None,
    State(String),
    /// A setting: shown, never edited from a panel.
    Options(String),
}

impl Bind {
    pub fn parse(src: &str) -> Bind {
        let src = src.trim();
        if let Some(k) = src.strip_prefix("state.").filter(|k| !k.is_empty()) {
            Bind::State(k.to_owned())
        } else if let Some(k) = src.strip_prefix("options.").filter(|k| !k.is_empty()) {
            Bind::Options(k.to_owned())
        } else {
            Bind::None
        }
    }

    /// The state key, if the value lives in the state.
    pub fn state_key(&self) -> Option<&str> {
        match self {
            Bind::State(k) => Some(k),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Width {
    /// The control's own width.
    #[default]
    Natural,
    Fill,
    Points(f32),
}

/// One element ready to draw: its text in the interface language, its
/// conditions compiled, its children resolved.
#[derive(Debug, Clone, Default)]
pub struct Element {
    pub id: String,
    pub kind: ElementKind,
    /// The type as written, for an unknown one.
    pub type_name: String,
    pub label: String,
    pub help: String,
    pub placeholder: String,
    pub bind: Bind,
    pub conds: Conds,
    pub on_change: String,
    pub on_submit: String,
    pub on_click: String,
    pub width: Width,
    pub height: Option<f32>,
    pub style: LabelStyle,
    pub rows: usize,
    pub monospace: bool,
    pub password: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub slider: bool,
    pub choices: Vec<Choice>,
    pub allow_auto: bool,
    pub result: ResultKind,
    pub url: String,
    pub filters: Vec<String>,
    /// `button`: the action.
    pub action: String,
    pub collapsed: bool,
    pub children: Vec<Element>,
}

/// A panel ready to draw and to run.
#[derive(Debug, Clone, Default)]
pub struct PanelPage {
    pub module: String,
    /// The module's name in the interface language.
    pub module_name: String,
    pub id: String,
    /// The panel's own title, translated.
    pub title: String,
    /// "Module name: panel title": every panel's window is named after its
    /// module, so it cannot pass for one of Glossa's.
    pub window_title: String,
    pub size: [f32; 2],
    pub open_from: Vec<String>,
    pub elements: Vec<Element>,
    /// The module's actions, translated.
    pub actions: Vec<ActionView>,
    /// The same actions as written, for what `call` and `builtin` actions do.
    pub specs: Vec<ActionSpec>,
    /// The state as the panel opens.
    pub defaults: OptionValues,
    pub persist: Vec<String>,
    pub on_open: Option<String>,
    /// State keys bound to `image` elements, for the bounds.
    image_keys: BTreeSet<String>,
}

/// The default window size when a panel names none.
pub const DEFAULT_SIZE: [f32; 2] = [520.0, 420.0];

impl PanelPage {
    /// The page for panel `spec` of manifest `m`, in `lang`.
    pub fn build(m: &Manifest, spec: &PanelSpec, lang: &str) -> PanelPage {
        let strings = &m.strings;
        let pid = &spec.id;
        let tr = |key: String, fallback: &str| strings.get(lang, &key).unwrap_or(fallback).to_owned();
        let module_name = tr("module.name".into(), &m.module.name);
        let title = tr(format!("panels.{pid}.title"), &spec.title);
        let title = if title.trim().is_empty() { pid.clone() } else { title };
        let mut b = Builder { spec, strings, lang, placed: BTreeSet::new(), depth: 0, count: 0 };
        let elements: Vec<Element> = spec.layout.iter().filter_map(|id| b.resolve(id)).collect();
        let mut image_keys = BTreeSet::new();
        fn walk(els: &[Element], images: &mut BTreeSet<String>) {
            for e in els {
                if let (ElementKind::Image, Some(k)) = (e.kind, e.bind.state_key()) {
                    images.insert(k.to_owned());
                }
                walk(&e.children, images);
            }
        }
        walk(&elements, &mut image_keys);
        let size = spec
            .size
            .filter(|[w, h]| w.is_finite() && h.is_finite())
            .map(|[w, h]| [w.clamp(240.0, 2400.0), h.clamp(160.0, 1600.0)])
            .unwrap_or(DEFAULT_SIZE);
        let mut page = PanelPage {
            module: m.module.id.clone(),
            window_title: format!("{module_name}: {title}"),
            module_name,
            id: pid.clone(),
            title,
            size,
            open_from: spec.open_from.clone(),
            elements,
            actions: crate::actions::views(m, lang),
            specs: m.actions.clone(),
            defaults: OptionValues::new(),
            persist: spec.persist.iter().map(|k| k.trim().to_owned()).filter(|k| !k.is_empty()).collect(),
            on_open: Some(spec.on_open.trim().to_owned()).filter(|a| !a.is_empty()),
            image_keys,
        };
        let mut defaults: OptionValues = spec.state.clone().into_iter().collect();
        page.bound_values(&mut defaults, true);
        page.defaults = defaults;
        page
    }

    /// The action `id` declares, translated.
    pub fn action(&self, id: &str) -> Option<&ActionView> {
        self.actions.iter().find(|a| a.id == id)
    }

    /// The action `id` as written.
    pub fn spec(&self, id: &str) -> Option<&ActionSpec> {
        self.specs.iter().find(|a| a.id == id)
    }

    /// Whether `key` is one of the panel's state keys: declared in
    /// `[ui.panels.state]` or bound by an element.
    pub fn knows_key(&self, key: &str) -> bool {
        if self.defaults.contains_key(key) {
            return true;
        }
        fn any(els: &[Element], key: &str) -> bool {
            els.iter().any(|e| e.bind.state_key() == Some(key) || any(&e.children, key))
        }
        any(&self.elements, key)
    }

    /// Hold `values` (a reply's state, a streamed update, the persisted
    /// keys) to the panel's bounds, in place: keys the panel does not know
    /// are dropped, as are images over [`MAX_IMAGE_BASE64`]; arrays are cut
    /// to [`MAX_LIST_ROWS`], text to [`MAX_TEXT`], nesting to a fixed depth,
    /// and the keys to [`MAX_STATE_KEYS`].
    pub fn bound_state(&self, values: &mut OptionValues) {
        self.bound_values(values, false);
    }

    /// [`Self::bound_state`]; `keep_unknown` for the declared defaults
    /// themselves, while the page is being built.
    fn bound_values(&self, values: &mut OptionValues, keep_unknown: bool) {
        values.retain(|k, _| keep_unknown || self.knows_key(k));
        while values.len() > MAX_STATE_KEYS {
            values.pop_last();
        }
        for (k, v) in values.iter_mut() {
            if self.image_keys.contains(k) {
                if !matches!(v, Value::String(s) if s.len() <= MAX_IMAGE_BASE64) {
                    *v = Value::Null;
                }
                continue;
            }
            bound_value(v, 0);
        }
    }

    /// Where every element stands for `state` and `options`: visible and
    /// enabled, by element id. A container's conditions apply to what it
    /// holds.
    pub fn context<'a>(&self, state: &'a OptionValues, options: &'a OptionValues) -> condition::Context<'a> {
        condition::Context { options, state: Some(state) }
    }

    /// The state keys this panel keeps between openings, with their values.
    pub fn persisted(&self, state: &OptionValues) -> OptionValues {
        self.persist.iter().filter_map(|k| state.get(k).map(|v| (k.clone(), v.clone()))).collect()
    }

    /// Every element, depth first.
    pub fn walk(&self) -> Vec<&Element> {
        fn go<'a>(els: &'a [Element], out: &mut Vec<&'a Element>) {
            for e in els {
                out.push(e);
                go(&e.children, out);
            }
        }
        let mut out = Vec::new();
        go(&self.elements, &mut out);
        out
    }

    /// The element `id`, at any depth.
    pub fn element(&self, id: &str) -> Option<&Element> {
        self.walk().into_iter().find(|e| e.id == id)
    }
}

impl Element {
    pub fn visible(&self, cx: &condition::Context) -> bool {
        self.conds.visible(cx)
    }

    pub fn enabled(&self, cx: &condition::Context) -> bool {
        self.conds.enabled(cx)
    }
}

/// Cut strings, arrays and nesting in `v` to the bounds.
fn bound_value(v: &mut Value, depth: usize) {
    if depth > MAX_VALUE_DEPTH {
        *v = Value::Null;
        return;
    }
    match v {
        Value::String(s) => clip(s, MAX_TEXT),
        Value::Array(a) => {
            a.truncate(MAX_LIST_ROWS);
            a.iter_mut().for_each(|x| bound_value(x, depth + 1));
        }
        Value::Object(o) => {
            if o.len() > MAX_STATE_KEYS {
                let keep: Vec<String> = o.keys().take(MAX_STATE_KEYS).cloned().collect();
                o.retain(|k, _| keep.contains(k));
            }
            o.values_mut().for_each(|x| bound_value(x, depth + 1));
        }
        _ => {}
    }
}

/// How many bytes `v` takes as JSON, without building the text.
pub fn json_size(v: &Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len();
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Count(0);
    match serde_json::to_writer(&mut c, v) {
        Ok(()) => c.0,
        Err(_) => usize::MAX,
    }
}

/// The size of one entry of a state map as JSON (key, value, punctuation).
fn entry_size(k: &str, v: &Value) -> usize {
    k.len() + 4 + json_size(v)
}

/// Drop the largest values of `values` until all of it, as JSON, takes at
/// most `cap` bytes. Returns the keys dropped.
pub fn fit_size(values: &mut OptionValues, cap: usize) -> Vec<String> {
    let mut sizes: Vec<(usize, String)> = values.iter().map(|(k, v)| (entry_size(k, v), k.clone())).collect();
    let mut total: usize = sizes.iter().map(|(s, _)| *s).sum::<usize>() + 2;
    sizes.sort();
    let mut dropped = Vec::new();
    while total > cap {
        let Some((s, k)) = sizes.pop() else { break };
        values.remove(&k);
        total -= s;
        dropped.push(k);
    }
    dropped
}

/// Drop the largest of `incoming` until `state` with them merged in takes
/// at most `cap` bytes as JSON; what is dropped keeps its old value.
/// Returns the keys dropped.
pub fn fit_merge(state: &OptionValues, incoming: &mut OptionValues, cap: usize) -> Vec<String> {
    let kept: usize = state.iter().filter(|(k, _)| !incoming.contains_key(*k)).map(|(k, v)| entry_size(k, v)).sum();
    fit_size(incoming, cap.saturating_sub(kept))
}

/// Cut `s` to at most `max` bytes at a character boundary, marking the cut.
pub fn clip(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
}

/// A value as the text a label or a text field shows.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// One row of a `list`: `{ title, subtitle, actions }`, or a plain string
/// as its title.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListRow {
    pub title: String,
    pub subtitle: String,
    /// Action ids drawn as small buttons on the row.
    pub actions: Vec<String>,
}

impl ListRow {
    pub fn from_value(v: &Value) -> ListRow {
        match v {
            Value::Object(o) => ListRow {
                title: o.get("title").map(value_text).unwrap_or_default(),
                subtitle: o.get("subtitle").map(value_text).unwrap_or_default(),
                actions: o
                    .get("actions")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).take(8).collect())
                    .unwrap_or_default(),
            },
            other => ListRow { title: value_text(other), ..Default::default() },
        }
    }
}

/// The rows of a list bound value.
pub fn list_rows(v: &Value) -> Vec<ListRow> {
    v.as_array().map(|a| a.iter().take(MAX_LIST_ROWS).map(ListRow::from_value).collect()).unwrap_or_default()
}

/// A `table` result's cells: a list of lists, or a list of objects (the
/// first row's keys are the header). The header row comes first when
/// there is one.
pub fn table_cells(v: &Value) -> (Vec<String>, Vec<Vec<String>>) {
    let Some(rows) = v.as_array() else { return (Vec::new(), Vec::new()) };
    let rows: Vec<&Value> = rows.iter().take(MAX_LIST_ROWS).collect();
    if let Some(Value::Object(first)) = rows.first() {
        let header: Vec<String> = first.keys().take(32).cloned().collect();
        let body = rows
            .iter()
            .map(|r| header.iter().map(|k| r.get(k).map(value_text).unwrap_or_default()).collect())
            .collect();
        return (header, body);
    }
    let body = rows
        .iter()
        .map(|r| match r {
            Value::Array(cells) => cells.iter().take(32).map(value_text).collect(),
            other => vec![value_text(other)],
        })
        .collect();
    (Vec::new(), body)
}

/// A `progress` value: a fraction from 0 to 1, or `None` to spin. Numbers
/// above 1 are read as percent.
pub fn progress_fraction(v: &Value) -> Option<f32> {
    let n = v.as_f64()?;
    let f = if n > 1.0 { n / 100.0 } else { n };
    Some(f.clamp(0.0, 1.0) as f32)
}

/// Whether `url` is one a panel's link may open: `http` or `https` only.
pub fn safe_link(url: &str) -> bool {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && u.len() > "http://".len()
        && !u.chars().any(|c| c.is_control() || c.is_whitespace())
}

struct Builder<'a> {
    spec: &'a PanelSpec,
    strings: &'a ModuleStrings,
    lang: &'a str,
    /// Items and actions already placed; each is drawn once (see the
    /// settings page's builder for why).
    placed: BTreeSet<String>,
    depth: usize,
    count: usize,
}

impl Builder<'_> {
    fn tr(&self, key: String, fallback: &str) -> String {
        self.strings.get(self.lang, &key).unwrap_or(fallback).to_owned()
    }

    fn resolve(&mut self, id: &str) -> Option<Element> {
        let id = id.trim();
        if self.count >= MAX_ELEMENTS {
            return None;
        }
        if let Some(action) = id.strip_prefix('@') {
            if !self.placed.insert(id.to_owned()) {
                return None;
            }
            self.count += 1;
            return Some(Element {
                id: id.to_owned(),
                kind: ElementKind::Button,
                type_name: "button".into(),
                action: action.to_owned(),
                ..Default::default()
            });
        }
        let item = self.spec.items.iter().find(|it| it.id == id)?;
        if self.depth >= MAX_DEPTH || !self.placed.insert(id.to_owned()) {
            return None;
        }
        self.count += 1;
        self.depth += 1;
        let pid = &self.spec.id;
        let key = |field: &str| format!("panels.{pid}.items.{id}.{field}");
        let kind = ElementKind::from_name(item.element.trim());
        let children = if kind.is_container() {
            let mut kids: Vec<Element> = item.children.iter().filter_map(|c| self.resolve(c)).collect();
            if kind == ElementKind::Tabs {
                // A tab is a group; anything else gets a tab of its own.
                kids = kids
                    .into_iter()
                    .map(|e| {
                        if e.kind == ElementKind::Group {
                            e
                        } else {
                            Element { kind: ElementKind::Group, children: vec![e], ..Default::default() }
                        }
                    })
                    .collect();
            }
            kids
        } else {
            Vec::new()
        };
        let choices = item
            .choices
            .iter()
            .map(|c| Choice { value: c.value.clone(), label: self.tr(key(&format!("choices.{}", c.value)), &c.label) })
            .collect();
        let el = Element {
            id: id.to_owned(),
            kind,
            type_name: item.element.clone(),
            label: self.tr(key("label"), &item.label),
            help: self.tr(key("help"), &item.help),
            placeholder: self.tr(key("placeholder"), &item.placeholder),
            bind: Bind::parse(&item.bind),
            conds: Conds::parse(&item.visible_if, &item.enabled_if),
            on_change: item.on_change.trim().to_owned(),
            on_submit: item.on_submit.trim().to_owned(),
            on_click: item.on_click.trim().to_owned(),
            width: match &item.width {
                None => Width::Natural,
                Some(WidthSpec::Word(w)) if w == "fill" => Width::Fill,
                Some(WidthSpec::Word(_)) => Width::Natural,
                Some(WidthSpec::Points(p)) if p.is_finite() && *p > 0.0 => Width::Points(p.min(2400.0)),
                Some(WidthSpec::Points(_)) => Width::Natural,
            },
            height: item.height.filter(|h| h.is_finite() && *h > 0.0).map(|h| h.min(2000.0)),
            style: match item.style.as_str() {
                "weak" => LabelStyle::Weak,
                "error" => LabelStyle::Error,
                "heading" => LabelStyle::Heading,
                _ => LabelStyle::Normal,
            },
            rows: item.rows.unwrap_or(4).clamp(1, 40) as usize,
            monospace: item.monospace,
            password: item.password,
            min: item.min,
            max: item.max,
            step: item.step,
            slider: item.widget == "slider",
            choices,
            allow_auto: item.allow_auto,
            result: ResultKind::from_name(item.kind.trim()).unwrap_or(ResultKind::Text),
            url: item.url.trim().to_owned(),
            filters: item.filters.clone(),
            action: item.action.trim().to_owned(),
            collapsed: item.collapsed,
            children,
        };
        self.depth -= 1;
        Some(el)
    }
}

/// The panels of manifest `m`, built in `lang`.
pub fn pages(m: &Manifest, lang: &str) -> Vec<PanelPage> {
    m.ui.panels.iter().map(|p| PanelPage::build(m, p, lang)).collect()
}

// -- lint -----------------------------------------------------------------------

/// The `call`s a panel action may make.
pub const CALLS: [&str; 4] = ["translate", "dictionary", "speak", "detect"];
/// The built-in actions.
pub const BUILTINS: [&str; 3] = ["copy", "open_url", "speak"];
/// Where a panel can be offered.
pub const OPEN_FROM: [&str; 2] = ["tray", "main_menu"];

/// Check `[[ui.panels]]`, and what `call` and `builtin` actions say. Part
/// of [`crate::settings::lint`].
pub fn lint(m: &Manifest) -> Vec<Issue> {
    let mut out = Vec::new();
    let action_ids: BTreeSet<&str> = m.actions.iter().map(|a| a.id.as_str()).collect();
    let option_keys: BTreeSet<&str> = m.options.iter().map(|o| o.key.as_str()).collect();

    for a in &m.actions {
        let what = format!("action {:?}", a.id);
        match a.kind() {
            ActionKind::Handler => {
                if !a.into.is_empty() || !a.args.is_empty() {
                    out.push(issue(
                        Level::Warning,
                        format!("{what}: `args` and `into` apply to `call` and `builtin` actions"),
                    ));
                }
            }
            ActionKind::Call => {
                if !CALLS.contains(&a.call.trim()) {
                    out.push(issue(
                        Level::Error,
                        format!("{what}: call {:?} is not one of {}", a.call, CALLS.join(", ")),
                    ));
                }
                if a.call.trim() != "speak" && Bind::parse(&a.into).state_key().is_none() {
                    out.push(issue(Level::Error, format!("{what}: a `call` needs `into = \"state.<key>\"`")));
                }
                if !a.args.contains_key("text") {
                    out.push(issue(Level::Error, format!("{what}: a `call` needs `args.text`")));
                }
            }
            ActionKind::Builtin => {
                let b = a.builtin.trim();
                if !BUILTINS.contains(&b) {
                    out.push(issue(
                        Level::Error,
                        format!("{what}: builtin {:?} is not one of {}", a.builtin, BUILTINS.join(", ")),
                    ));
                }
                let needed = if b == "open_url" { "url" } else { "text" };
                if BUILTINS.contains(&b) && !a.args.contains_key(needed) {
                    out.push(issue(Level::Error, format!("{what}: builtin {b:?} needs `args.{needed}`")));
                }
            }
        }
        for (k, v) in &a.args {
            let v = v.trim();
            if let Some(key) = v.strip_prefix("options.") {
                if !option_keys.contains(key) {
                    out.push(issue(
                        Level::Warning,
                        format!("{what}: args.{k} reads options.{key}, which is not an option"),
                    ));
                }
            }
        }
        if !a.shortcut.is_empty() && a.shortcut.parse::<glossa_core::Hotkey>().is_err() {
            out.push(issue(Level::Error, format!("{what}: shortcut {:?} is not a key combination", a.shortcut)));
        }
    }

    let mut seen = BTreeSet::new();
    for p in &m.ui.panels {
        let what = format!("panel {:?}", p.id);
        let id_ok = !p.id.is_empty() && p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !id_ok {
            out.push(issue(Level::Error, format!("{what}: a panel id is letters, digits, '_' and '-'")));
        }
        if !seen.insert(p.id.as_str()) {
            out.push(issue(Level::Error, format!("{what} is declared twice")));
        }
        if p.title.trim().is_empty() {
            out.push(issue(Level::Note, format!("{what} has no `title`; the window shows its id")));
        }
        for o in &p.open_from {
            if !OPEN_FROM.contains(&o.as_str()) {
                out.push(issue(
                    Level::Warning,
                    format!("{what}: open_from {o:?} is not one of {}; it is ignored", OPEN_FROM.join(", ")),
                ));
            }
        }
        if p.layout.is_empty() {
            out.push(issue(Level::Error, format!("{what} has an empty `layout`; nothing would be drawn")));
        }
        if p.items.len() > MAX_ELEMENTS {
            out.push(issue(Level::Error, format!("{what} has more than {MAX_ELEMENTS} items; the rest are left out")));
        }
        if p.state.len() > MAX_STATE_KEYS {
            out.push(issue(Level::Error, format!("{what}: more than {MAX_STATE_KEYS} state keys")));
        }
        for k in &p.persist {
            if !p.state.contains_key(k.trim()) {
                out.push(issue(Level::Warning, format!("{what}: persist names {k:?}, which [ui.panels.state] lacks")));
            }
        }
        let on_open = p.on_open.trim();
        if !on_open.is_empty() && !action_ids.contains(on_open) {
            out.push(issue(Level::Error, format!("{what}: on_open {on_open:?} is not an action")));
        }
        // Copying and opening a page need the user's own click.
        let needs_gesture =
            |id: &str| m.actions.iter().any(|a| a.id == id && matches!(a.builtin.trim(), "copy" | "open_url"));
        if needs_gesture(on_open) {
            out.push(issue(
                Level::Error,
                format!("{what}: on_open {on_open:?} copies or opens a page, which runs only on a click"),
            ));
        }
        for it in &p.items {
            if needs_gesture(it.on_change.trim()) {
                out.push(issue(
                    Level::Error,
                    format!(
                        "{what} item {:?}: on_change {:?} copies or opens a page, which runs only on a click",
                        it.id, it.on_change
                    ),
                ));
            }
        }

        let item_ids: BTreeSet<&str> = p.items.iter().map(|i| i.id.as_str()).collect();
        let mut seen_items = BTreeSet::new();
        let check_cond = |what: &str, field: &str, src: &str, out: &mut Vec<Issue>| {
            if src.trim().is_empty() {
                return;
            }
            match Condition::parse(src) {
                Err(e) => out.push(issue(Level::Error, format!("{what}: {field} {src:?}: {e}"))),
                Ok(c) => {
                    for (scope, path) in c.paths() {
                        let missing = match scope {
                            condition::Scope::Options => !option_keys.contains(path[0].as_str()),
                            condition::Scope::State => !p.state.contains_key(&path[0]),
                        };
                        if missing {
                            let where_ = if scope == condition::Scope::State { "state" } else { "options" };
                            out.push(issue(
                                Level::Warning,
                                format!("{what}: {field} reads {where_}.{}, which the panel does not declare", path[0]),
                            ));
                        }
                    }
                }
            }
        };
        for it in &p.items {
            let what = format!("{what} item {:?}", it.id);
            if it.id.trim().is_empty() {
                out.push(issue(Level::Error, format!("panel {:?}: an item has no `id`", p.id)));
                continue;
            }
            if !seen_items.insert(it.id.as_str()) {
                out.push(issue(Level::Error, format!("{what} is declared twice")));
            }
            let kind = ElementKind::from_name(it.element.trim());
            if kind == ElementKind::Unknown {
                out.push(issue(
                    Level::Warning,
                    format!(
                        "{what}: unknown type {:?}; it shows as needing a newer Glossa (label, link, separator, text, \
                         textarea, number, toggle, select, multiselect, lang, file, button, row, group, tabs, result, \
                         list, image, progress)",
                        it.element
                    ),
                ));
            }
            let bind = Bind::parse(&it.bind);
            if !it.bind.trim().is_empty() && bind == Bind::None {
                out.push(issue(
                    Level::Error,
                    format!("{what}: bind {:?} is not state.<key> or options.<key>", it.bind),
                ));
            }
            match &bind {
                Bind::State(k) if !p.state.contains_key(k) => out.push(issue(
                    Level::Warning,
                    format!("{what}: binds state.{k}, which [ui.panels.state] does not declare"),
                )),
                Bind::Options(k) if !option_keys.contains(k.as_str()) => {
                    out.push(issue(Level::Error, format!("{what}: binds options.{k}, which is not an option")))
                }
                Bind::Options(_) if kind.is_input() => out.push(issue(
                    Level::Warning,
                    format!("{what}: a panel cannot change settings; the field bound to options is read only"),
                )),
                _ => {}
            }
            if (kind.is_input() || kind.shows_value())
                && bind == Bind::None
                && kind != ElementKind::Label
                && kind != ElementKind::Link
            {
                out.push(issue(Level::Error, format!("{what}: a {} needs `bind`", it.element)));
            }
            if kind == ElementKind::Label && bind == Bind::None && it.label.is_empty() {
                out.push(issue(Level::Warning, format!("{what}: a label needs `label` or `bind`")));
            }
            if kind == ElementKind::Link && bind == Bind::None {
                if it.url.is_empty() {
                    out.push(issue(Level::Error, format!("{what}: a link needs `url` or `bind`")));
                } else if !safe_link(&it.url) {
                    out.push(issue(Level::Error, format!("{what}: url {:?} is not an http or https address", it.url)));
                }
            }
            if kind == ElementKind::Result && ResultKind::from_name(it.kind.trim()).is_none() {
                out.push(issue(
                    Level::Warning,
                    format!(
                        "{what}: result kind {:?} is not translation, article, text, markdown, table or json; drawn as text",
                        it.kind
                    ),
                ));
            }
            if kind == ElementKind::Button {
                if it.action.is_empty() {
                    out.push(issue(Level::Error, format!("{what}: a button needs `action`")));
                } else if !action_ids.contains(it.action.trim()) {
                    out.push(issue(Level::Error, format!("{what}: action {:?} is not declared", it.action)));
                }
            }
            if kind.is_container() && it.children.is_empty() {
                out.push(issue(Level::Warning, format!("{what} has no children")));
            }
            if !kind.is_container() && !it.children.is_empty() {
                out.push(issue(Level::Warning, format!("{what}: `children` apply to row, group and tabs")));
            }
            if matches!(kind, ElementKind::Select | ElementKind::Multiselect) && it.choices.is_empty() {
                out.push(issue(Level::Error, format!("{what}: a {} needs `choices`", it.element)));
            }
            if kind == ElementKind::Number && it.widget == "slider" && (it.min.is_none() || it.max.is_none()) {
                out.push(issue(Level::Error, format!("{what}: widget = \"slider\" needs both min and max")));
            }
            for (field, ev) in [("on_change", &it.on_change), ("on_submit", &it.on_submit), ("on_click", &it.on_click)]
            {
                let ev = ev.trim();
                if !ev.is_empty() && !action_ids.contains(ev) {
                    out.push(issue(Level::Error, format!("{what}: {field} {ev:?} is not an action")));
                }
            }
            if !it.on_submit.is_empty() && !matches!(kind, ElementKind::Text | ElementKind::Textarea) {
                out.push(issue(Level::Warning, format!("{what}: `on_submit` applies to text and textarea")));
            }
            if !it.on_click.is_empty() && !matches!(kind, ElementKind::Button | ElementKind::List) {
                out.push(issue(Level::Warning, format!("{what}: `on_click` applies to buttons and lists")));
            }
            if !it.on_change.is_empty() && !kind.is_input() {
                out.push(issue(Level::Warning, format!("{what}: `on_change` applies to inputs")));
            }
            check_cond(&what, "visible_if", &it.visible_if, &mut out);
            check_cond(&what, "enabled_if", &it.enabled_if, &mut out);
        }
        // References: every id in a layout or children names an item or an
        // action, and is placed once.
        let mut referenced: BTreeMap<&str, usize> = BTreeMap::new();
        let lists = std::iter::once((format!("{what} layout"), &p.layout))
            .chain(p.items.iter().map(|it| (format!("{what} item {:?}", it.id), &it.children)));
        for (where_, list) in lists {
            for id in list {
                let id = id.trim();
                if let Some(action) = id.strip_prefix('@') {
                    if !action_ids.contains(action) {
                        out.push(issue(Level::Error, format!("{where_}: `@{action}` names an undeclared action")));
                    }
                } else if option_keys.contains(id) && !item_ids.contains(id) {
                    out.push(issue(
                        Level::Error,
                        format!("{where_}: {id:?} is an option; panels show their own items (bind = \"options.{id}\" to show a setting)"),
                    ));
                } else if !item_ids.contains(id) {
                    out.push(issue(Level::Error, format!("{where_}: {id:?} is not an item")));
                }
                *referenced.entry(id).or_default() += 1;
            }
        }
        for (id, n) in &referenced {
            if *n > 1 {
                out.push(issue(Level::Warning, format!("{what}: {id:?} is placed {n} times; only the first is drawn")));
            }
        }
        for it in &p.items {
            if !referenced.contains_key(it.id.as_str()) {
                out.push(issue(Level::Warning, format!("{what} item {:?} is not placed anywhere", it.id)));
            }
        }
    }
    out
}

/// Why `panels.<rest>` in a locale file names nothing, if it does not.
pub(crate) fn unknown_string_key(m: &Manifest, rest: &[&str]) -> Option<String> {
    let Some((pid, rest)) = rest.split_first() else { return Some("names no panel".into()) };
    let Some(p) = m.ui.panels.iter().find(|p| p.id == *pid) else {
        return Some(format!("names panel {pid:?}, which does not exist"));
    };
    match rest {
        ["title"] => None,
        ["items", id, field @ ..] => {
            let Some(it) = p.items.iter().find(|i| i.id == *id) else {
                return Some(format!("names item {id:?}, which panel {pid:?} lacks"));
            };
            match field {
                ["label" | "help" | "placeholder"] => None,
                ["choices", v] => (!it.choices.iter().any(|c| c.value == *v))
                    .then(|| format!("names choice {v:?}, which item {id:?} lacks")),
                _ => Some("is not label, help, placeholder or choices.<value>".into()),
            }
        }
        _ => Some("is not title or items.<id>.<field>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEAD: &str = "capabilities = [\"translate\"]\n[module]\nid = \"acme\"\nname = \"Acme\"\nruntime = \"js\"\n";

    pub(crate) const PLAYGROUND: &str = r#"
        [[actions]]
        id = "translate_now"
        label = "Translate"
        call = "translate"
        args = { text = "state.source", to = "state.to" }
        into = "state.result"
        shortcut = "Ctrl+Enter"

        [[actions]]
        id = "explain"
        label = "Explain"

        [[actions]]
        id = "copy"
        builtin = "copy"
        args = { text = "item.title" }

        [[ui.panels]]
        id = "playground"
        title = "Playground"
        open_from = ["tray"]
        size = [560, 440]
        layout = ["source", "bar", "result", "words", "pic", "busy", "more"]
        persist = ["to"]

        [ui.panels.state]
        source = ""
        to = "ru"
        result = ""
        words = []
        pic = ""
        busy = 0.5

        [[ui.panels.items]]
        id = "source"
        type = "textarea"
        bind = "state.source"
        placeholder = "Text"
        rows = 6
        on_submit = "translate_now"

        [[ui.panels.items]]
        id = "bar"
        type = "row"
        children = ["to", "@translate_now", "@explain"]

        [[ui.panels.items]]
        id = "to"
        type = "lang"
        bind = "state.to"

        [[ui.panels.items]]
        id = "result"
        type = "result"
        kind = "translation"
        bind = "state.result"
        visible_if = "state.result"

        [[ui.panels.items]]
        id = "words"
        type = "list"
        bind = "state.words"
        on_click = "copy"

        [[ui.panels.items]]
        id = "pic"
        type = "image"
        bind = "state.pic"

        [[ui.panels.items]]
        id = "busy"
        type = "progress"
        bind = "state.busy"

        [[ui.panels.items]]
        id = "more"
        type = "hologram"
    "#;

    fn manifest(rest: &str) -> Manifest {
        Manifest::parse(&format!("{HEAD}{rest}")).unwrap()
    }

    #[test]
    fn panels_parse_build_and_round_trip() {
        let mut m = manifest(PLAYGROUND);
        m.strings.insert(
            "ru",
            [
                ("panels.playground.title".to_owned(), "Песочница".to_owned()),
                ("panels.playground.items.source.placeholder".to_owned(), "Текст".to_owned()),
            ]
            .into(),
        );
        assert_eq!(m.ui.panels.len(), 1);
        let page = PanelPage::build(&m, &m.ui.panels[0], "ru");
        assert_eq!(page.window_title, "Acme: Песочница", "titled after the module");
        assert_eq!(page.size, [560.0, 440.0]);
        let kinds: Vec<ElementKind> = page.elements.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                ElementKind::Textarea,
                ElementKind::Row,
                ElementKind::Result,
                ElementKind::List,
                ElementKind::Image,
                ElementKind::Progress,
                ElementKind::Unknown
            ]
        );
        assert_eq!(page.elements[0].placeholder, "Текст");
        assert_eq!(page.elements[0].rows, 6);
        let row: Vec<&str> = page.elements[1].children.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(row, ["to", "@translate_now", "@explain"]);
        assert_eq!(page.elements[1].children[1].action, "translate_now");
        assert_eq!(page.defaults["to"], "ru");
        assert_eq!(page.persisted(&page.defaults), [("to".to_owned(), json!("ru"))].into());

        // Conditions read the panel's state.
        let options = OptionValues::new();
        let result = page.element("result").unwrap();
        assert!(!result.visible(&page.context(&page.defaults, &options)));
        let mut state = page.defaults.clone();
        state.insert("result".into(), json!("Привет"));
        assert!(result.visible(&page.context(&state, &options)));

        // The editor rewrites manifests: nothing is lost on the way.
        let again: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(again.ui.panels, m.ui.panels);
        #[derive(Serialize, Deserialize)]
        struct Only {
            panels: Vec<PanelSpec>,
        }
        let text = toml::to_string(&Only { panels: m.ui.panels.clone() }).unwrap();
        assert_eq!(toml::from_str::<Only>(&text).unwrap().panels, m.ui.panels);
    }

    #[test]
    fn state_values_are_bounded() {
        let m = manifest(PLAYGROUND);
        let page = PanelPage::build(&m, &m.ui.panels[0], "en");
        let mut v: OptionValues = [
            ("source".to_owned(), json!("я".repeat(MAX_TEXT))),
            ("words".to_owned(), json!(vec![json!({ "title": "w" }); MAX_LIST_ROWS + 50])),
            ("pic".to_owned(), json!("A".repeat(MAX_IMAGE_BASE64 + 1))),
            ("intruder".to_owned(), json!(1)),
            ("result".to_owned(), json!({ "deep": [[[[[[[[[[[[[[[[[[[["x"]]]]]]]]]]]]]]]]]]]] })),
        ]
        .into();
        page.bound_state(&mut v);
        assert!(v["source"].as_str().unwrap().len() <= MAX_TEXT + '…'.len_utf8());
        assert_eq!(v["words"].as_array().unwrap().len(), MAX_LIST_ROWS);
        assert!(v["pic"].is_null(), "an oversized image is dropped, not cut");
        assert!(!v.contains_key("intruder"), "keys the panel does not know are dropped");
        assert!(v["result"].to_string().contains("null"), "deep nesting is cut");
        let mut ok: OptionValues = [("pic".to_owned(), json!("iVBORw0KGgo="))].into();
        page.bound_state(&mut ok);
        assert_eq!(ok["pic"], "iVBORw0KGgo=");
    }

    #[test]
    fn sizes_are_capped_in_total_dropping_the_largest() {
        let mut v: OptionValues = [
            ("small".to_owned(), json!("a")),
            ("big".to_owned(), json!("x".repeat(600))),
            ("mid".to_owned(), json!("y".repeat(300))),
        ]
        .into();
        assert_eq!(json_size(&json!("ab")), 4);
        assert_eq!(fit_size(&mut v, 400), ["big"]);
        assert!(v.contains_key("mid") && v.contains_key("small"));
        let state: OptionValues = [("held".to_owned(), json!("z".repeat(300))), ("mid".to_owned(), json!(1))].into();
        let mut incoming = v.clone();
        assert_eq!(fit_merge(&state, &mut incoming, 400), ["mid"], "the state already takes most of the room");
        assert!(incoming.contains_key("small"));
        let mut all = v.clone();
        assert!(fit_size(&mut all, 10_000).is_empty());
    }

    #[test]
    fn rows_tables_progress_and_links_read_module_values_as_data() {
        let rows = list_rows(&json!(["plain", { "title": "t", "subtitle": 2, "actions": ["copy", 5] }]));
        assert_eq!(rows[0].title, "plain");
        assert_eq!((rows[1].subtitle.as_str(), rows[1].actions.as_slice()), ("2", &["copy".to_owned()][..]));
        let (h, b) = table_cells(&json!([{ "a": 1, "b": "x" }, { "a": 2 }]));
        assert_eq!(h, ["a", "b"]);
        assert_eq!(b[1], ["2", ""]);
        let (h, b) = table_cells(&json!([["1", "2"], "3"]));
        assert!(h.is_empty() && b[1] == ["3"]);
        assert_eq!(progress_fraction(&json!(50)), Some(0.5));
        assert_eq!(progress_fraction(&json!(0.25)), Some(0.25));
        assert_eq!(progress_fraction(&Value::Null), None, "null spins");
        assert!(safe_link("https://example.com/a?b"));
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "http://", "https://a b", "ftp://x"] {
            assert!(!safe_link(bad), "{bad}");
        }
    }

    #[test]
    fn the_lint_names_broken_panels() {
        let m = manifest(
            r#"
            [[options]]
            key = "api_key"
            type = "secret"
            [[actions]]
            id = "go"
            label = "Go"
            call = "teleport"
            [[actions]]
            id = "cp"
            label = "Copy"
            builtin = "copy"
            [[ui.panels]]
            id = "p"
            layout = ["api_key", "ghost", "x", "x", "@nope"]
            persist = ["missing"]
            on_open = "nothing"
            [ui.panels.state]
            a = ""
            [[ui.panels.items]]
            id = "x"
            type = "text"
            bind = "state.b"
            on_submit = "zzz"
            visible_if = "state.c"
            [[ui.panels.items]]
            id = "y"
            type = "result"
            kind = "hologram"
            [[ui.panels.items]]
            id = "l"
            type = "link"
            url = "javascript:alert(1)"
            [[ui.panels.items]]
            id = "t"
            type = "text"
            bind = "state.a"
            on_change = "cp"
            "#,
        );
        let text = lint(&m).iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("\n");
        for want in [
            "call \"teleport\" is not one of",
            "a `call` needs `into",
            "builtin \"copy\" needs `args.text`",
            "\"api_key\" is an option; panels show their own items",
            "\"ghost\" is not an item",
            "\"x\" is placed 2 times",
            "`@nope` names an undeclared action",
            "persist names \"missing\"",
            "on_open \"nothing\" is not an action",
            "binds state.b, which [ui.panels.state] does not declare",
            "on_submit \"zzz\" is not an action",
            "reads state.c, which the panel does not declare",
            "result kind \"hologram\"",
            "a result needs `bind`",
            "is not an http or https address",
            "item \"y\" is not placed anywhere",
            "on_change \"cp\" copies or opens a page",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        let good = manifest(PLAYGROUND);
        let errors: Vec<_> = lint(&good).into_iter().filter(|i| i.level == Level::Error).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }
}
