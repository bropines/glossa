//! `[[actions]]`: named things a module's settings page (and, later, its
//! panels) can trigger, such as "Load models" or "Test connection".
//!
//! ```toml
//! [[actions]]
//! id = "list_models"
//! label = "Load models"           # the handler defaults to the id: list_models(ev, host)
//!
//! [[actions]]
//! id = "test"
//! label = "Test connection"
//! handler = "testKey"             # another function name
//! needs = ["api_key"]             # disabled, with a hint, until these are filled in and valid
//!
//! [[options]]
//! key = "model"
//! type = "select"
//! choices_from = "list_models"    # the action's reply may replace the choices
//!
//! [ui.settings]
//! layout = ["api_key", "model", "@test"]   # `@id` places an action's button
//! ```
//!
//! An action with a handler is sent to the module as the protocol's
//! `action` method ([`glossa_module_sdk::wire::ActionParams`]); its reply
//! ([`ActionReply`]) may set options in the draft, replace a select's
//! choices, put messages under fields and show a notice. `call` and
//! `builtin` actions run without module code, in panels; `glossa-engine`
//! runs them, and [`crate::panels::lint`] checks what they say.
//!
//! What the reply does to the values being edited is decided here
//! ([`apply_options`], [`choices_from_reply`]), so every front end treats it
//! the same.

use std::collections::BTreeMap;

use glossa_module_sdk::wire::ActionReply;
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::provider::{Choice, OptionKind, OptionSpec, OptionValues};
use crate::settings::{issue, Issue, Level, ModuleStrings};

/// One `[[actions]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Unique within the module; `@id` in a layout places the button.
    pub id: String,
    /// The button's text; the id when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The button's tooltip.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// The module function that runs; the id when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handler: String,
    /// Option keys that must be filled in and pass their checks first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Panels: run this module's own capability instead of a handler.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub call: String,
    /// Panels: the request fields of a `call`, as bound values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
    /// Panels: where a `call`'s result goes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub into: String,
    /// Panels: one of Glossa's own actions (`copy`, `open_url`, `speak`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub builtin: String,
    /// Panels: a key combination that runs the action while the panel has focus.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shortcut: String,
}

/// How an action runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Module code, through the `action` method.
    Handler,
    /// `call = "translate"` and the like (panels).
    Call,
    /// `builtin = "copy"` and the like (panels).
    Builtin,
}

impl ActionSpec {
    pub fn handler_name(&self) -> &str {
        if self.handler.trim().is_empty() {
            &self.id
        } else {
            self.handler.trim()
        }
    }

    pub fn kind(&self) -> ActionKind {
        if !self.builtin.trim().is_empty() {
            ActionKind::Builtin
        } else if !self.call.trim().is_empty() {
            ActionKind::Call
        } else {
            ActionKind::Handler
        }
    }
}

/// An action as a page shows it: its text in the interface language.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionView {
    pub id: String,
    /// Translated; the id when the manifest gives no label.
    pub label: String,
    pub help: String,
    pub handler: String,
    pub needs: Vec<String>,
    pub kind: ActionKind,
}

/// The module's actions with their text in `lang`.
pub fn views(m: &Manifest, lang: &str) -> Vec<ActionView> {
    let tr = |key: String, fallback: &str| m.strings.get(lang, &key).unwrap_or(fallback).to_owned();
    m.actions
        .iter()
        .map(|a| {
            let label = tr(format!("actions.{}.label", a.id), &a.label);
            ActionView {
                id: a.id.clone(),
                label: if label.trim().is_empty() { a.id.clone() } else { label },
                help: tr(format!("actions.{}.help", a.id), &a.help),
                handler: a.handler_name().to_owned(),
                needs: a.needs.iter().map(|n| n.trim().to_owned()).collect(),
                kind: a.kind(),
            }
        })
        .collect()
}

/// The module's `[strings]` for code (`host.t`) in `lang`: English
/// (`locales/en.toml`) first, then the language's base, then the language
/// itself, so a key missing from a translation still has its English text.
pub fn code_strings(strings: &ModuleStrings, lang: &str) -> BTreeMap<String, String> {
    let lang = lang.trim().replace('_', "-").to_ascii_lowercase();
    let base = lang.split('-').next().unwrap_or("").to_owned();
    let mut out = BTreeMap::new();
    let mut tables = vec!["en".to_owned()];
    for l in [base, lang] {
        if !l.is_empty() && !tables.contains(&l) {
            tables.push(l);
        }
    }
    for l in &tables {
        if let Some(t) = strings.table(l) {
            for (k, v) in t {
                if let Some(key) = k.strip_prefix("strings.") {
                    if !v.is_empty() {
                        out.insert(key.to_owned(), v.clone());
                    }
                }
            }
        }
    }
    out
}

/// The longest choice value or label a reply may carry, in bytes. A longer
/// value is dropped (cutting it would change it); a longer label is cut.
pub const MAX_CHOICE_TEXT: usize = 1024;
/// The longest notice or field message a reply may carry, in bytes; longer
/// ones are cut.
pub const MAX_MESSAGE: usize = 4096;

/// Whether `v` is a value an option of `kind` can hold.
pub fn value_fits(kind: &OptionKind, v: &serde_json::Value) -> bool {
    match kind {
        OptionKind::Bool => v.is_boolean(),
        OptionKind::Number => v.is_number(),
        OptionKind::Multiselect => v.as_array().is_some_and(|a| a.iter().all(|x| x.is_string())),
        // A type from a newer format: whatever the module sends.
        OptionKind::Unknown(_) => true,
        _ => v.is_string(),
    }
}

/// Check a reply before anything uses it: every value in `options` must
/// fit its option's type (a mismatch is an error naming the key; keys that
/// are not options are dropped), and its text is bounded: choice values
/// and labels to [`MAX_CHOICE_TEXT`], the notice and field messages to
/// [`MAX_MESSAGE`], the choices of one option to [`MAX_CHOICES`].
pub fn check_reply(specs: &[OptionSpec], reply: &mut ActionReply) -> Result<(), String> {
    reply.options.retain(|k, _| specs.iter().any(|s| s.key == *k));
    for (k, v) in &reply.options {
        let spec = specs.iter().find(|s| s.key == *k).expect("kept above");
        if !value_fits(&spec.kind, v) {
            return Err(format!("options.{k}: a {} option cannot be set to {v}", spec.kind.name()));
        }
    }
    for list in reply.choices.values_mut() {
        list.retain(|c| c.value.len() <= MAX_CHOICE_TEXT);
        list.truncate(MAX_CHOICES);
        for c in list.iter_mut() {
            clip(&mut c.label, MAX_CHOICE_TEXT);
        }
    }
    if let Some(n) = reply.notice.as_mut() {
        clip(n, MAX_MESSAGE);
    }
    for msg in reply.errors.values_mut() {
        clip(msg, MAX_MESSAGE);
    }
    Ok(())
}

/// Cut `s` to at most `max` bytes at a character boundary, marking the cut.
fn clip(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
}

/// Merge a reply's `options` into `values` (the settings draft). Only the
/// module's own option keys are taken, with values that fit their type: a
/// reply cannot plant settings the page does not show. Returns whether
/// anything changed.
pub fn apply_options(specs: &[OptionSpec], reply: &ActionReply, values: &mut OptionValues) -> bool {
    let mut changed = false;
    for (k, v) in &reply.options {
        let fits = specs.iter().any(|s| s.key == *k && value_fits(&s.kind, v));
        if fits && values.get(k) != Some(v) {
            values.insert(k.clone(), v.clone());
            changed = true;
        }
    }
    changed
}

/// The choices a reply gives for `select` and `multiselect` options, by
/// key, as option choices. Keys that are not such options are dropped, as
/// are empty values.
pub fn choices_from_reply(specs: &[OptionSpec], reply: &ActionReply) -> BTreeMap<String, Vec<Choice>> {
    reply
        .choices
        .iter()
        .filter(|(k, _)| {
            specs.iter().any(|s| s.key == **k && matches!(s.kind, OptionKind::Select | OptionKind::Multiselect))
        })
        .map(|(k, list)| {
            let list = list
                .iter()
                .filter(|c| !c.value.is_empty())
                .take(MAX_CHOICES)
                .map(|c| Choice { value: c.value.clone(), label: c.label.clone() })
                .collect();
            (k.clone(), list)
        })
        .collect()
}

/// The most choices taken from one reply: a dropdown with more is
/// unusable, and the list is drawn every frame while open.
pub const MAX_CHOICES: usize = 2000;

/// Check `[[actions]]`, `needs`, `choices_from` and the `@id`s in the
/// settings layout. Part of [`crate::settings::lint`].
pub fn lint(m: &Manifest) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for a in &m.actions {
        let what = format!("action {:?}", a.id);
        let id_ok = !a.id.is_empty()
            && a.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !a.id.starts_with(|c: char| c.is_ascii_digit());
        if !id_ok {
            out.push(issue(
                Level::Error,
                format!("{what}: an action id is letters, digits, '_' and '-', not starting with a digit"),
            ));
        }
        if !seen.insert(a.id.as_str()) {
            out.push(issue(Level::Error, format!("{what} is declared twice")));
        }
        if a.label.trim().is_empty() {
            out.push(issue(Level::Note, format!("{what} has no `label`; the button shows its id")));
        }
        let handler = a.handler_name();
        let js_ident = !handler.is_empty()
            && handler.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
            && !handler.starts_with(|c: char| c.is_ascii_digit());
        if a.kind() == ActionKind::Handler && !js_ident && m.module.runtime == crate::Runtime::Js {
            out.push(issue(
                Level::Error,
                format!("{what}: handler {handler:?} is not a JavaScript function name; set `handler`"),
            ));
        }
        if !a.call.is_empty() && !a.builtin.is_empty() {
            out.push(issue(Level::Error, format!("{what}: `call` and `builtin` do not go together")));
        }
        for n in &a.needs {
            if !m.options.iter().any(|o| o.key == n.trim()) {
                out.push(issue(Level::Error, format!("{what}: needs {n:?}, which is not an option")));
            }
        }
    }
    for o in &m.options {
        let from = o.choices_from.trim();
        if from.is_empty() {
            continue;
        }
        let what = format!("option {:?}", o.key);
        if !matches!(o.kind, OptionKind::Select | OptionKind::Multiselect) {
            out.push(issue(Level::Warning, format!("{what}: `choices_from` applies to select and multiselect fields")));
        }
        match m.actions.iter().find(|a| a.id == from) {
            None => out.push(issue(Level::Error, format!("{what}: choices_from {from:?} is not an action"))),
            Some(a) if a.kind() != ActionKind::Handler => {
                out.push(issue(Level::Error, format!("{what}: choices_from {from:?} must be an action with a handler")))
            }
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_module_sdk::wire::ActionChoice;
    use serde_json::json;

    const HEAD: &str = "capabilities = [\"translate\"]\n[module]\nid = \"t\"\nname = \"T\"\nruntime = \"js\"\n";

    fn manifest(rest: &str) -> Manifest {
        Manifest::parse(&format!("{HEAD}{rest}")).unwrap()
    }

    const ACME: &str = r#"
        [[options]]
        key = "api_key"
        type = "secret"
        pattern = '^ak-[a-z]{4}$'
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
        label = "Test connection"
        handler = "testKey"
        needs = ["api_key"]

        [[actions]]
        id = "translate_now"
        call = "translate"
        args = { text = "state.source" }
        into = "state.result"
        shortcut = "Ctrl+Enter"

        [ui.settings]
        layout = ["api_key", "model", "@test"]
    "#;

    #[test]
    fn actions_parse_beside_options_and_round_trip() {
        let m = manifest(ACME);
        assert_eq!(m.actions.len(), 3);
        let test = &m.actions[1];
        assert_eq!((test.handler_name(), test.kind()), ("testKey", ActionKind::Handler));
        assert_eq!(test.needs, ["api_key"]);
        assert_eq!(m.actions[0].handler_name(), "list_models", "the handler defaults to the id");
        assert_eq!(m.actions[2].kind(), ActionKind::Call);
        assert_eq!(m.actions[2].args["text"], "state.source");
        assert_eq!(m.options[1].choices_from, "list_models");
        // The module editor rewrites manifests: nothing is lost on the way.
        // (Through JSON: an option without a default is a null, which TOML
        // cannot write.)
        let again: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(again.actions, m.actions);
        assert_eq!(again.options, m.options);
        #[derive(Serialize, Deserialize)]
        struct Only {
            actions: Vec<ActionSpec>,
        }
        let text = toml::to_string(&Only { actions: m.actions.clone() }).unwrap();
        assert_eq!(toml::from_str::<Only>(&text).unwrap().actions, m.actions);
        // A manifest without actions writes none.
        assert!(serde_json::to_value(manifest("")).unwrap().get("actions").is_none());
    }

    #[test]
    fn views_are_translated_and_fall_back_to_the_id() {
        let mut m = manifest(ACME);
        m.strings.insert("ru", [("actions.test.label".to_owned(), "Проверить".to_owned())].into());
        let v = views(&m, "ru");
        assert_eq!(v[1].label, "Проверить");
        assert_eq!(v[0].label, "Load models");
        assert_eq!(v[2].label, "translate_now", "no label: the id");
        assert_eq!(views(&m, "en")[1].label, "Test connection");
    }

    #[test]
    fn code_strings_fall_back_to_english_key_by_key() {
        let mut s = ModuleStrings::default();
        s.insert(
            "en",
            [("strings.ok", "Connected"), ("strings.bad", "Bad key"), ("options.x.label", "X")]
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .collect(),
        );
        s.insert("ru", [("strings.ok".to_owned(), "Подключено".to_owned())].into());
        let ru = code_strings(&s, "ru_RU");
        assert_eq!(ru["ok"], "Подключено");
        assert_eq!(ru["bad"], "Bad key");
        assert!(!ru.contains_key("x.label") && ru.len() == 2, "only [strings]");
        assert_eq!(code_strings(&s, "de")["ok"], "Connected");
    }

    #[test]
    fn replies_only_touch_the_modules_own_fields() {
        let m = manifest(ACME);
        let reply: ActionReply = serde_json::from_value(json!({
            "options": { "model": "large", "intruder": 1 },
            "choices": { "model": ["large", { "value": "xl", "label": "XL" }, ""], "api_key": ["x"], "nope": ["y"] }
        }))
        .unwrap();
        let mut values = OptionValues::new();
        assert!(apply_options(&m.options, &reply, &mut values));
        assert_eq!(values, [("model".to_owned(), json!("large"))].into());
        assert!(!apply_options(&m.options, &reply, &mut values), "applying twice changes nothing");
        let choices = choices_from_reply(&m.options, &reply);
        assert_eq!(choices.keys().collect::<Vec<_>>(), ["model"], "only selects take choices");
        assert_eq!(
            choices["model"],
            [Choice { value: "large".into(), label: String::new() }, Choice { value: "xl".into(), label: "XL".into() }]
        );
        let many = ActionReply {
            choices: [("model".to_owned(), vec![ActionChoice { value: "v".into(), label: String::new() }; 5000])]
                .into(),
            ..Default::default()
        };
        assert_eq!(choices_from_reply(&m.options, &many)["model"].len(), MAX_CHOICES);
    }

    #[test]
    fn replies_are_type_checked_and_bounded() {
        let m = manifest(
            r#"
            [[options]]
            key = "n"
            type = "number"
            [[options]]
            key = "on"
            type = "bool"
            [[options]]
            key = "tags"
            type = "multiselect"
            choices = [{ value = "a" }]
            [[options]]
            key = "model"
            type = "select"
            choices = [{ value = "small" }]
            choices_from = "x"
            [[actions]]
            id = "x"
            "#,
        );
        let ok = |v: serde_json::Value| -> Result<ActionReply, String> {
            let mut r: ActionReply = serde_json::from_value(v).unwrap();
            check_reply(&m.options, &mut r).map(|_| r)
        };
        let r = ok(json!({ "options": { "n": 2.5, "on": true, "tags": ["a"], "model": "xl", "ghost": 1 } })).unwrap();
        assert_eq!(r.options.len(), 4, "keys that are not options are dropped");
        for bad in [json!({ "n": "2" }), json!({ "on": 1 }), json!({ "tags": "a" }), json!({ "model": 5 })] {
            let e = ok(json!({ "options": bad })).unwrap_err();
            assert!(e.contains("cannot be set to"), "{e}");
        }
        // apply_options never writes a misfit either, whoever calls it.
        let raw: ActionReply = serde_json::from_value(json!({ "options": { "n": "two", "on": false } })).unwrap();
        let mut values = OptionValues::new();
        apply_options(&m.options, &raw, &mut values);
        assert_eq!(values, [("on".to_owned(), json!(false))].into());

        let long = "я".repeat(5000);
        let r = ok(json!({
            "notice": long, "errors": { "n": long },
            "choices": { "model": [{ "value": "v".repeat(MAX_CHOICE_TEXT + 1) }, { "value": "ok", "label": long }] }
        }))
        .unwrap();
        assert!(r.notice.as_ref().unwrap().len() <= MAX_MESSAGE + '…'.len_utf8());
        assert!(r.notice.as_ref().unwrap().ends_with('…'));
        assert!(r.errors["n"].len() <= MAX_MESSAGE + '…'.len_utf8());
        assert_eq!(r.choices["model"].len(), 1, "an over-long value is dropped");
        assert!(r.choices["model"][0].label.len() <= MAX_CHOICE_TEXT + '…'.len_utf8());
    }

    #[test]
    fn the_lint_names_broken_actions() {
        let m = manifest(
            r#"
            [[options]]
            key = "n"
            type = "number"
            choices_from = "ghost"
            [[options]]
            key = "s"
            type = "select"
            choices_from = "copy"
            [[actions]]
            id = "2bad"
            [[actions]]
            id = "a"
            label = "A"
            handler = "do-it"
            needs = ["missing"]
            [[actions]]
            id = "a"
            label = "A again"
            [[actions]]
            id = "copy"
            label = "Copy"
            builtin = "copy"
            "#,
        );
        let text = lint(&m).iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("\n");
        for want in [
            "action \"2bad\": an action id is",
            "action \"2bad\" has no `label`",
            "handler \"do-it\" is not a JavaScript function name",
            "needs \"missing\", which is not an option",
            "action \"a\" is declared twice",
            "option \"n\": `choices_from` applies to select",
            "choices_from \"ghost\" is not an action",
            "choices_from \"copy\" must be an action with a handler",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        assert_eq!(
            lint(&manifest(ACME)).iter().filter(|i| i.level == Level::Error).count(),
            0,
            "a good manifest has no errors"
        );
    }
}
