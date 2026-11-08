//! Where a settings page's module actions stand between frames: the one
//! running (its reply on the way back from a worker), the notice each
//! action left, the messages a reply put under fields, and the choices a
//! `choices_from` action loaded.
//!
//! What a reply does to the draft is decided in `glossa_modules::actions`;
//! this keeps the results and hands them to `module_settings` to draw. The
//! state lives in egui's memory under the page's id, so it survives the
//! Options window closing and opening again while the app runs, and the
//! settings page needs no field of its own for it.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use glossa_modules::provider::Choice;
use glossa_modules::wire::ActionReply;
use glossa_modules::{OptionSpec, OptionValues};
use parking_lot::Mutex;

/// Starts an action on a worker: `(action id, the values on the page)`.
/// The receiver gets the reply, or the error already worded for the user.
pub type Runner<'a> = &'a dyn Fn(&str, OptionValues) -> Receiver<Result<ActionReply, String>>;

/// A [`Runner`] for module `module`: each action runs in the engine on a
/// worker thread, with the values on the page over the declared defaults,
/// and its reply comes back through the channel with a repaint.
pub fn engine_runner(
    app: &crate::app::GlossaApp,
    module: &str,
) -> impl Fn(&str, OptionValues) -> Receiver<Result<ActionReply, String>> {
    let (engine, ctx, locale, module) = (app.engine.clone(), app.ctx.clone(), app.locale.clone(), module.to_owned());
    move |action: &str, values: OptionValues| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (engine, ctx, locale, module, action) =
            (engine.clone(), ctx.clone(), locale.clone(), module.clone(), action.to_owned());
        std::thread::spawn(move || {
            let req = glossa_engine::ActionRequest { options: Some(values), ..Default::default() };
            let r = engine.module_action(&module, &action, req);
            let _ = tx.send(r.map_err(|e| crate::app::describe_error(&locale, &e)));
            ctx.request_repaint();
        });
        rx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub level: Level,
}

/// One page's actions.
#[derive(Default)]
pub struct PageRuns {
    running: Option<(String, Receiver<Result<ActionReply, String>>)>,
    notices: HashMap<String, Notice>,
    /// Key -> (message, the value it was about). Shown while the field
    /// still holds that value.
    field_errors: BTreeMap<String, (String, serde_json::Value)>,
    choices: BTreeMap<String, Vec<Choice>>,
    /// An action to start as if its button were pressed (`--screenshot`).
    pressed: Option<String>,
}

pub type Shared = Arc<Mutex<PageRuns>>;

/// The state of the page drawn with `id_salt`.
pub fn for_page(ctx: &egui::Context, id_salt: &str) -> Shared {
    let id = egui::Id::unique(("module-actions", id_salt));
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(id).clone())
}

/// Press `action`'s button on the page drawn with `id_salt` the next time
/// it is drawn, for `--screenshot` (`GLOSSA_SCREENSHOT_ACTION`).
pub fn press(ctx: &egui::Context, id_salt: &str, action: &str) {
    for_page(ctx, id_salt).lock().pressed = Some(action.to_owned());
}

impl PageRuns {
    /// Whether `action` was pressed from outside; true once.
    pub fn take_press(&mut self, action: &str) -> bool {
        if self.pressed.as_deref() == Some(action) {
            self.pressed = None;
            return true;
        }
        false
    }

    /// The action running now, if any.
    pub fn running(&self) -> Option<&str> {
        self.running.as_ref().map(|(a, _)| a.as_str())
    }

    pub fn start(&mut self, action: &str, rx: Receiver<Result<ActionReply, String>>) {
        self.notices.remove(action);
        self.running = Some((action.to_owned(), rx));
    }

    /// Take a finished reply and apply it to `values` (the draft). Returns
    /// whether a value changed.
    pub fn poll(&mut self, specs: &[OptionSpec], values: &mut OptionValues, done: &str) -> bool {
        let Some((action, rx)) = &self.running else { return false };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the action stopped without an answer".into()),
        };
        let action = action.clone();
        self.running = None;
        match result {
            Err(text) => {
                self.notices.insert(action, Notice { text, level: Level::Error });
                false
            }
            Ok(reply) => {
                let changed = glossa_modules::actions::apply_options(specs, &reply, values);
                self.choices.extend(glossa_modules::actions::choices_from_reply(specs, &reply));
                for (key, msg) in &reply.errors {
                    let now = current(specs, values, key);
                    self.field_errors.insert(key.clone(), (msg.clone(), now));
                }
                let level = match reply.notice_level.as_deref() {
                    Some("success") => Level::Success,
                    Some("warning") => Level::Warning,
                    Some("error") => Level::Error,
                    _ => Level::Info,
                };
                // A reply with nothing visible still says it ran.
                let text = match &reply.notice {
                    Some(n) => n.clone(),
                    None if reply.errors.is_empty() && reply.choices.is_empty() && !changed => done.to_owned(),
                    None => String::new(),
                };
                if !text.is_empty() {
                    self.notices.insert(action, Notice { text, level });
                }
                changed
            }
        }
    }

    pub fn notice(&self, action: &str) -> Option<&Notice> {
        self.notices.get(action)
    }

    /// The message a reply put under `key`, while the field still holds the
    /// value it was about.
    pub fn field_error(&mut self, specs: &[OptionSpec], values: &OptionValues, key: &str) -> Option<String> {
        let (msg, about) = self.field_errors.get(key)?;
        if *about == current(specs, values, key) {
            return Some(msg.clone());
        }
        self.field_errors.remove(key);
        None
    }

    /// The choices an action loaded for `key`, if one has.
    pub fn choices(&self, key: &str) -> Option<&[Choice]> {
        self.choices.get(key).map(Vec::as_slice)
    }
}

fn current(specs: &[OptionSpec], values: &OptionValues, key: &str) -> serde_json::Value {
    values
        .get(key)
        .cloned()
        .or_else(|| specs.iter().find(|s| s.key == key).map(|s| s.default.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replies_land_in_the_draft_and_field_errors_last_until_an_edit() {
        let specs: Vec<OptionSpec> = vec![
            OptionSpec::secret("api_key", "Key"),
            OptionSpec::select("model", "Model", "small", &[("small", "Small")]),
        ];
        let mut runs = PageRuns::default();
        let mut values = OptionValues::new();
        let (tx, rx) = std::sync::mpsc::channel();
        runs.start("test", rx);
        assert_eq!(runs.running(), Some("test"));
        assert!(!runs.poll(&specs, &mut values, "Done."), "nothing yet");
        tx.send(Ok(serde_json::from_value(json!({
            "options": { "model": "large" },
            "choices": { "model": ["small", "large"] },
            "errors": { "api_key": "refused" }
        }))
        .unwrap()))
            .unwrap();
        assert!(runs.poll(&specs, &mut values, "Done."));
        assert_eq!(runs.running(), None);
        assert_eq!(values["model"], "large");
        assert_eq!(runs.choices("model").unwrap().len(), 2);
        assert!(runs.notice("test").is_none(), "something visible happened: no \"done\"");
        assert_eq!(runs.field_error(&specs, &values, "api_key").as_deref(), Some("refused"));
        values.insert("api_key".into(), json!("new"));
        assert_eq!(runs.field_error(&specs, &values, "api_key"), None, "an edit clears it");

        let (tx, rx) = std::sync::mpsc::channel();
        runs.start("test", rx);
        tx.send(Ok(ActionReply::default())).unwrap();
        runs.poll(&specs, &mut values, "Done.");
        assert_eq!(runs.notice("test").unwrap().text, "Done.");
        let (tx, rx) = std::sync::mpsc::channel();
        runs.start("test", rx);
        tx.send(Err("no connection".into())).unwrap();
        runs.poll(&specs, &mut values, "Done.");
        assert_eq!(runs.notice("test").unwrap().level, Level::Error);
    }
}
