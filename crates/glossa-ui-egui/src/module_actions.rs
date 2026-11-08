//! Where a settings page's module actions stand between frames: the one
//! running (its reply on the way back from a worker), the notice each
//! action left, the messages a reply put under fields, and the choices a
//! `choices_from` action loaded.
//!
//! What a reply does to the draft is decided in `glossa_modules::actions`;
//! this keeps the results and hands them to `module_settings` to draw.
//!
//! The state is kept by page id (the provider id; the editor's preview has
//! its own), outside any window, so the settings page needs no field for
//! it. A run belongs to the draft it was started from: [`reset_pages`]
//! cancels the runs and forgets what they left when Options opens or
//! closes and when modules are loaded again, and a reply from an earlier
//! generation is dropped rather than written into a newer draft.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, LazyLock};

use glossa_modules::provider::Choice;
use glossa_modules::wire::ActionReply;
use glossa_modules::{CancelToken, OptionSpec, OptionValues};
use parking_lot::Mutex;

/// What a run sends back: the reply, or the error already worded for the
/// user.
pub type Outcome = Result<ActionReply, String>;

/// Starts an action on a worker: `(action id, the values on the page, a
/// flag that stops waiting for it)`.
pub type Runner<'a> = &'a dyn Fn(&str, OptionValues, CancelToken) -> Receiver<Outcome>;

/// A [`Runner`] for module `module`: each action runs in the engine on a
/// worker thread, with the values on the page over the declared defaults,
/// and its reply comes back through the channel with a repaint.
pub fn engine_runner(
    app: &crate::app::GlossaApp,
    module: &str,
) -> impl Fn(&str, OptionValues, CancelToken) -> Receiver<Outcome> {
    let (engine, ctx, locale, module) = (app.engine.clone(), app.ctx.clone(), app.locale.clone(), module.to_owned());
    move |action: &str, values: OptionValues, cancel: CancelToken| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (engine, ctx, locale, module, action) =
            (engine.clone(), ctx.clone(), locale.clone(), module.clone(), action.to_owned());
        std::thread::spawn(move || {
            let req = glossa_engine::ActionRequest { options: Some(values), ..Default::default() };
            let r = engine.module_action_with(&module, &action, req, &cancel);
            if !cancel.is_cancelled() {
                let _ = tx.send(r.map_err(|e| crate::app::describe_error(&locale, &e)));
                ctx.request_repaint();
            }
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

/// The action running on a page.
struct Running {
    action: String,
    /// The page's generation when it started.
    generation: u64,
    rx: Receiver<Outcome>,
    cancel: CancelToken,
}

/// One page's actions.
#[derive(Default)]
pub struct PageRuns {
    running: Option<Running>,
    /// Bumped by [`PageRuns::reset`]: replies from before it are dropped.
    generation: u64,
    notices: HashMap<String, Notice>,
    /// Key -> (message, the value it was about). Shown while the field
    /// still holds that value.
    field_errors: BTreeMap<String, (String, serde_json::Value)>,
    choices: BTreeMap<String, Vec<Choice>>,
    /// An action to start as if its button were pressed (`--screenshot`).
    pressed: Option<String>,
}

pub type Shared = Arc<Mutex<PageRuns>>;

static PAGES: LazyLock<Mutex<HashMap<String, Shared>>> = LazyLock::new(Default::default);

/// The state of the page drawn with `id_salt`.
pub fn for_page(id_salt: &str) -> Shared {
    PAGES.lock().entry(id_salt.to_owned()).or_default().clone()
}

/// Cancel the runs of every page whose id `matches`, and forget their
/// notices, field messages and loaded choices: the draft they were about
/// is gone (Options closed or opened afresh), or the module was loaded
/// again.
pub fn reset_pages(matches: impl Fn(&str) -> bool) {
    for (id, page) in PAGES.lock().iter() {
        if matches(id) {
            page.lock().reset();
        }
    }
}

/// Press `action`'s button on the page drawn with `id_salt` the next time
/// it is drawn, for `--screenshot` (`GLOSSA_SCREENSHOT_ACTION`).
pub fn press(id_salt: &str, action: &str) {
    for_page(id_salt).lock().pressed = Some(action.to_owned());
}

impl PageRuns {
    /// Cancel the running action, drop its reply when it comes, and forget
    /// what earlier runs left.
    pub fn reset(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancel.cancel();
        }
        self.generation += 1;
        self.notices.clear();
        self.field_errors.clear();
        self.choices.clear();
    }

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
        self.running.as_ref().map(|r| r.action.as_str())
    }

    /// Start `action` with `run`, tied to this page's current generation.
    pub fn start(&mut self, action: &str, run: Runner, values: OptionValues) {
        let cancel = CancelToken::new();
        let rx = run(action, values, cancel.clone());
        self.notices.remove(action);
        self.running = Some(Running { action: action.to_owned(), generation: self.generation, rx, cancel });
    }

    /// Take a finished reply and apply it to `values` (the draft). Returns
    /// whether a value changed.
    pub fn poll(&mut self, specs: &[OptionSpec], values: &mut OptionValues, done: &str) -> bool {
        let Some(run) = &self.running else { return false };
        let result = match run.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the action stopped without an answer".into()),
        };
        let Some(run) = self.running.take() else { return false };
        if run.generation != self.generation {
            // Started for a draft that is gone.
            return false;
        }
        let action = run.action;
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

    /// The choices actions loaded, by option key.
    pub fn loaded_choices(&self) -> &BTreeMap<String, Vec<Choice>> {
        &self.choices
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
        let rx = std::cell::RefCell::new(Some(rx));
        let run = |_: &str, _: OptionValues, _: CancelToken| rx.borrow_mut().take().unwrap();
        runs.start("test", &run, OptionValues::new());
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
        assert_eq!(runs.loaded_choices()["model"].len(), 2);
        assert!(runs.notice("test").is_none(), "something visible happened: no \"done\"");
        assert_eq!(runs.field_error(&specs, &values, "api_key").as_deref(), Some("refused"));
        values.insert("api_key".into(), json!("new"));
        assert_eq!(runs.field_error(&specs, &values, "api_key"), None, "an edit clears it");

        let (tx, rx2) = std::sync::mpsc::channel();
        *rx.borrow_mut() = Some(rx2);
        runs.start("test", &run, OptionValues::new());
        tx.send(Ok(ActionReply::default())).unwrap();
        runs.poll(&specs, &mut values, "Done.");
        assert_eq!(runs.notice("test").unwrap().text, "Done.");
        let (tx, rx2) = std::sync::mpsc::channel();
        *rx.borrow_mut() = Some(rx2);
        runs.start("test", &run, OptionValues::new());
        tx.send(Err("no connection".into())).unwrap();
        runs.poll(&specs, &mut values, "Done.");
        assert_eq!(runs.notice("test").unwrap().level, Level::Error);
    }

    #[test]
    fn a_reset_cancels_the_run_and_drops_its_late_reply() {
        let specs = vec![OptionSpec::select("model", "Model", "small", &[("small", "Small")])];
        let mut runs = PageRuns::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let rx = std::cell::RefCell::new(Some(rx));
        let token = std::cell::RefCell::new(None);
        let run = |_: &str, _: OptionValues, cancel: CancelToken| {
            *token.borrow_mut() = Some(cancel);
            rx.borrow_mut().take().unwrap()
        };
        runs.start("list_models", &run, OptionValues::new());
        // Options closed and opened again: a fresh draft.
        runs.reset();
        assert!(token.borrow().as_ref().unwrap().is_cancelled(), "the worker is told to stop");
        assert_eq!(runs.running(), None);
        let _ = tx.send(Ok(serde_json::from_value(json!({ "options": { "model": "large" } })).unwrap()));
        let mut fresh = OptionValues::new();
        assert!(!runs.poll(&specs, &mut fresh, "Done."), "the late reply is dropped");
        assert!(fresh.is_empty() && runs.notice("list_models").is_none());

        // A run whose generation no longer matches is dropped too.
        let (tx, rx2) = std::sync::mpsc::channel();
        *rx.borrow_mut() = Some(rx2);
        runs.start("list_models", &run, OptionValues::new());
        runs.generation += 1;
        tx.send(Ok(serde_json::from_value(json!({ "options": { "model": "large" } })).unwrap())).unwrap();
        assert!(!runs.poll(&specs, &mut fresh, "Done.") && fresh.is_empty());
    }
}
