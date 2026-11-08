//! Module actions from the engine's side: running one of a module's
//! `[[actions]]` (a "Load models" or "Test" button) with the settings being
//! edited. The desktop app's settings page, `glossa-cli module action` and
//! the module test harness all come through here or through the same
//! [`Provider::action`](glossa_modules::Provider::action).
//!
//! The call blocks: front ends run it on a worker thread.

use glossa_core::{Error, Result};
use glossa_modules::actions::ActionKind;
use glossa_modules::wire::{ActionEvent, ActionParams, ActionReply};
use glossa_modules::{CancelToken, OptionValues};

use crate::Engine;

/// What a front end passes with an action besides its name.
#[derive(Debug, Clone, Default)]
pub struct ActionRequest {
    /// The settings to run with, over the declared defaults: the unsaved
    /// draft on the settings page. `None` uses the saved settings.
    pub options: Option<OptionValues>,
    /// A panel's state; empty on the settings page.
    pub state: OptionValues,
    /// The panel the action comes from; `None` on the settings page.
    pub panel: Option<String>,
}

impl Engine {
    /// Run `action` of module `module` and return its reply. Fails with
    /// [`Error::MissingCredential`] naming the first field in `needs` that is
    /// empty or invalid, before the module is called; with
    /// [`Error::Unsupported`] for `call` and `builtin` actions without a
    /// panel. With `req.panel` the action runs as that panel runs it
    /// ([`Engine::panel_action`]), and only the reply is returned. A
    /// handler's reply is returned as the module sent it, after the network
    /// permissions of the module applied to everything it fetched.
    pub fn module_action(&self, module: &str, action: &str, req: ActionRequest) -> Result<ActionReply> {
        self.module_action_with(module, action, req, &CancelToken::new())
    }

    /// [`Engine::module_action`] with a cancel flag, so a front end can stop
    /// waiting for it (the settings page closing).
    pub fn module_action_with(
        &self,
        module: &str,
        action: &str,
        req: ActionRequest,
        cancel: &CancelToken,
    ) -> Result<ActionReply> {
        if let Some(panel) = &req.panel {
            // From a panel: every kind of action runs, as the panel runs it.
            let id = crate::PanelId::new(module, panel.clone());
            let event = ActionEvent { kind: "click".into(), element: Some(action.to_owned()), item: None };
            let preq = crate::PanelRequest { state: req.state.clone(), event };
            return self.panel_action(&id, action, preq, cancel, &|_| {}).map(|o| o.reply);
        }
        let provider = self.registry().get(module).ok_or_else(|| Error::NoSuchModule(module.to_owned()))?;
        let lang = self.locale().code.clone();
        let page = provider.settings_page(&lang);
        let view = page.action(action).ok_or_else(|| {
            let known: Vec<&str> = page.actions.iter().map(|a| a.id.as_str()).collect();
            Error::Config(if known.is_empty() {
                format!("module {module} declares no actions")
            } else {
                format!("module {module} has no action {action:?} (it has {})", known.join(", "))
            })
        })?;
        if view.kind != ActionKind::Handler {
            return Err(Error::Unsupported {
                provider: provider.info().name.clone(),
                capability: "call and built-in actions outside a panel (name the panel)",
            });
        }
        let started = std::time::Instant::now();
        let reply = self.with_pipeline(cancel, |p| {
            let options = match &req.options {
                Some(o) => glossa_modules::provider::resolve_options(provider.options(), Some(o)),
                None => p.options_for(provider.as_ref()),
            };
            if let Some(&i) = page.action_blockers(view, Some(&options)).first() {
                return Err(Error::MissingCredential {
                    provider: provider.info().name.clone(),
                    what: page.options[i].key.clone(),
                });
            }
            let params = ActionParams {
                action: action.to_owned(),
                handler: view.handler.clone(),
                panel: req.panel.clone(),
                event: Some(ActionEvent { kind: "click".into(), element: Some(action.to_owned()), item: None }),
                state: req.state.clone(),
                options: options.clone(),
                ui_lang: lang.clone(),
                strings: Default::default(),
            };
            provider.action(&p.host(&options), params)
        });
        // The options may hold keys; only the outcome is logged.
        tracing::debug!(module, action, ok = reply.is_ok(), ms = started.elapsed().as_millis() as u64, "module action");
        reply
    }
}

#[cfg(all(test, feature = "js"))]
mod tests {
    use super::*;
    use glossa_core::{Config, Paths};
    use std::path::PathBuf;

    fn engine(tag: &str) -> (Engine, PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-engine-actions-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let module = dir.join("home").join("modules").join("acts");
        std::fs::create_dir_all(module.join("locales")).unwrap();
        std::fs::write(
            module.join("module.toml"),
            r#"
            capabilities = ["translate"]
            [module]
            id = "acts"
            name = "Acts"
            runtime = "js"
            [[options]]
            key = "api_key"
            type = "secret"
            pattern = '^ak-'
            [[options]]
            key = "model"
            type = "select"
            default = "small"
            choices = [{ value = "small" }]
            choices_from = "list_models"
            [[actions]]
            id = "list_models"
            [[actions]]
            id = "test"
            needs = ["api_key"]
            [[actions]]
            id = "copy"
            builtin = "copy"
            "#,
        )
        .unwrap();
        std::fs::write(module.join("locales").join("en.toml"), "[strings]\nok = \"Key {0} works\"\n").unwrap();
        std::fs::write(
            module.join("main.js"),
            r#"
            function translate(req) { return req.text; }
            function list_models(ev) { return { choices: { model: ["small", ev.options.model + "-xl"] } }; }
            function test(ev, host) { return host.t("ok", ev.options.api_key) + " in " + ev.ui_lang; }
            "#,
        )
        .unwrap();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        (Engine::with_config(paths, Config::default()).unwrap(), dir)
    }

    #[test]
    fn actions_run_with_the_draft_and_are_gated_by_needs() {
        let (e, dir) = engine("run");
        let r = e.module_action("acts", "list_models", ActionRequest::default()).unwrap();
        assert_eq!(r.choices["model"][1].value, "small-xl", "the saved settings and defaults");

        // `needs` stops the call before the module sees it.
        let err = e.module_action("acts", "test", ActionRequest::default()).unwrap_err();
        assert!(matches!(err, Error::MissingCredential { ref what, .. } if what == "api_key"), "{err:?}");
        let draft = |key: &str| ActionRequest {
            options: Some([("api_key".to_owned(), serde_json::json!(key))].into()),
            ..Default::default()
        };
        assert!(matches!(e.module_action("acts", "test", draft("wrong")), Err(Error::MissingCredential { .. })));
        let r = e.module_action("acts", "test", draft("ak-1")).unwrap();
        assert_eq!(r.notice.as_deref(), Some("Key ak-1 works in en"), "the draft, and host.t");

        assert!(matches!(e.module_action("acts", "copy", ActionRequest::default()), Err(Error::Unsupported { .. })));
        let err = e.module_action("acts", "nope", ActionRequest::default()).unwrap_err();
        assert!(err.to_string().contains("list_models, test, copy"), "{err}");
        assert!(matches!(e.module_action("ghost", "x", ActionRequest::default()), Err(Error::NoSuchModule(_))));
        std::fs::remove_dir_all(&dir).ok();
    }
}
