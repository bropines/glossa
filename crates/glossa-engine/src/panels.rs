//! Module panels from the engine's side: which panels the loaded modules
//! declare, running a panel's actions (module handlers, `call`s of the
//! module's own capabilities, and Glossa's built-ins), and the live state
//! of an open panel ([`Panel`]) with its streamed updates and the keys it
//! keeps between openings.
//!
//! The desktop app draws a [`Panel`] and forwards what the user does to
//! it; `glossa-cli module panel` and the control API reach the same
//! functions. The format itself (elements, bounds, lint) is in
//! `glossa_modules::panels`.
//!
//! A panel costs nothing until it is opened: the list is read from the
//! manifests when asked for, and an open panel's state lives in its
//! [`Panel`], dropped when it closes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::{Audio, Error, LangId, LangPair, Result};
use glossa_modules::actions::ActionKind;
use glossa_modules::panels::{safe_link, value_text, Bind, Element, PanelPage};
use glossa_modules::wire::{ActionEvent, ActionParams, ActionReply};
use glossa_modules::{CancelToken, OptionValues};
use serde::Serialize;
use serde_json::Value;

use crate::{Engine, TranslateOptions};

/// How long an input must stay unchanged before its `on_change` action runs.
pub const CHANGE_DEBOUNCE: Duration = Duration::from_millis(400);

/// A panel's name from outside: `module/panel`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PanelId {
    pub module: String,
    pub panel: String,
}

impl PanelId {
    pub fn new(module: impl Into<String>, panel: impl Into<String>) -> Self {
        Self { module: module.into(), panel: panel.into() }
    }

    /// `module/panel`; the error says what was expected.
    pub fn parse(s: &str) -> Result<PanelId> {
        match s.trim().split_once('/') {
            Some((m, p)) if !m.trim().is_empty() && !p.trim().is_empty() && !p.contains('/') => {
                Ok(PanelId::new(m.trim(), p.trim()))
            }
            _ => Err(Error::Config(format!("{s:?} is not a panel name; use MODULE/PANEL"))),
        }
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.module, self.panel)
    }
}

/// A panel some loaded module declares.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PanelInfo {
    /// `module/panel`.
    pub id: String,
    pub module: String,
    pub panel: String,
    /// "Module name: panel title", in the interface language.
    pub title: String,
    /// Where it is offered: `tray`, `main_menu`.
    pub open_from: Vec<String>,
}

/// Something an action asks the front end to do, beyond changing the
/// panel's state.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Put this text on the clipboard.
    Copy(String),
    /// Open this address in the browser (always `http` or `https`).
    OpenUrl(String),
    /// Play this audio.
    Play(Arc<Audio>),
}

/// What running one of a panel's actions came to.
#[derive(Debug, Clone, Default)]
pub struct PanelOutcome {
    /// The reply, already held to the panel's bounds: its `state` is what
    /// to merge into the panel; `options` is always empty (a panel cannot
    /// change settings).
    pub reply: ActionReply,
    pub effects: Vec<Effect>,
}

/// What set an action off, and the panel's state at that moment.
#[derive(Debug, Clone, Default)]
pub struct PanelRequest {
    pub state: OptionValues,
    pub event: ActionEvent,
}

impl Engine {
    /// The panels the loaded modules declare, in module order, with their
    /// titles in the interface language.
    pub fn panels(&self) -> Vec<PanelInfo> {
        let lang = self.locale().code.clone();
        let registry = self.registry();
        let mut out = Vec::new();
        for entry in registry.entries() {
            let Some(m) = entry.provider.manifest() else { continue };
            for page in glossa_modules::panels::pages(m, &lang) {
                out.push(PanelInfo {
                    id: format!("{}/{}", page.module, page.id),
                    module: page.module.clone(),
                    panel: page.id.clone(),
                    title: page.window_title.clone(),
                    open_from: page.open_from.clone(),
                });
            }
        }
        out
    }

    /// The page of panel `id`, in the interface language.
    pub fn panel_page(&self, id: &PanelId) -> Result<Arc<PanelPage>> {
        let provider = self.registry().get(&id.module).ok_or_else(|| Error::NoSuchModule(id.module.clone()))?;
        let m = provider.manifest().ok_or_else(|| Error::Config(format!("module {} has no panels", id.module)))?;
        let spec = m.ui.panels.iter().find(|p| p.id == id.panel).ok_or_else(|| {
            let known: Vec<&str> = m.ui.panels.iter().map(|p| p.id.as_str()).collect();
            Error::Config(if known.is_empty() {
                format!("module {} has no panels", id.module)
            } else {
                format!("module {} has no panel {:?} (it has {})", id.module, id.panel, known.join(", "))
            })
        })?;
        Ok(Arc::new(PanelPage::build(m, spec, &self.locale().code)))
    }

    /// Run `action` of panel `id` with `req`, blocking. Handlers get the
    /// state and the event; their `ui_update` calls reach `updates`, held
    /// to the panel's bounds, as do a streaming translation's partial
    /// texts for a `call = "translate"`. Fails before the module is called
    /// when a field the action `needs` is empty or invalid.
    pub fn panel_action(
        &self,
        id: &PanelId,
        action: &str,
        req: PanelRequest,
        cancel: &CancelToken,
        updates: &(dyn Fn(OptionValues) + Sync),
    ) -> Result<PanelOutcome> {
        let page = self.panel_page(id)?;
        self.panel_action_on(&page, action, req, cancel, updates)
    }

    fn panel_action_on(
        &self,
        page: &PanelPage,
        action: &str,
        req: PanelRequest,
        cancel: &CancelToken,
        updates: &(dyn Fn(OptionValues) + Sync),
    ) -> Result<PanelOutcome> {
        let module = page.module.as_str();
        let provider = self.registry().get(module).ok_or_else(|| Error::NoSuchModule(module.to_owned()))?;
        let spec = page.spec(action).ok_or_else(|| {
            Error::Config(format!("module {module} has no action {action:?} for panel {:?}", page.id))
        })?;
        let lang = self.locale().code.clone();
        let settings = provider.settings_page(&lang);
        let started = Instant::now();
        let bounded = |mut u: OptionValues| {
            page.bound_state(&mut u);
            if !u.is_empty() {
                updates(u);
            }
        };
        let result = self.with_pipeline(cancel, |p| -> Result<PanelOutcome> {
            let options = p.options_for(provider.as_ref());
            if let Some(view) = settings.action(action) {
                if let Some(&i) = settings.action_blockers(view, Some(&options)).first() {
                    return Err(Error::MissingCredential {
                        provider: provider.info().name.clone(),
                        what: settings.options[i].key.clone(),
                    });
                }
            }
            let arg = |name: &str| -> Value {
                spec.args
                    .get(name)
                    .map(|src| resolve_arg(src, &req.state, &options, req.event.item.as_ref()))
                    .unwrap_or(Value::Null)
            };
            let into = Bind::parse(&spec.into).state_key().map(str::to_owned);
            let mut out = PanelOutcome::default();
            match spec.kind() {
                ActionKind::Handler => {
                    let params = ActionParams {
                        action: action.to_owned(),
                        handler: spec.handler_name().to_owned(),
                        panel: Some(page.id.clone()),
                        event: Some(req.event.clone()),
                        state: req.state.clone(),
                        options: options.clone(),
                        ui_lang: lang.clone(),
                        strings: Default::default(),
                    };
                    out.reply = provider.action_with_updates(&p.host(&options), params, &bounded)?;
                }
                ActionKind::Call => {
                    let text = value_text(&arg("text"));
                    let from = lang_arg(&arg("from")).unwrap_or(LangId::AUTO);
                    let cfg = self.config();
                    let value = match spec.call.trim() {
                        "translate" => {
                            let to = lang_arg(&arg("to")).unwrap_or(cfg.general.first_language);
                            let (tx, rx) = mpsc::channel::<String>();
                            let opts = TranslateOptions {
                                provider: Some(module.to_owned()),
                                fail_over: Some(false),
                                progress: Some(crate::Progress::new(move |t| {
                                    let _ = tx.send(t.to_owned());
                                })),
                                ..Default::default()
                            };
                            // A streaming service reports the text so far
                            // on this thread; a helper forwards it into the
                            // panel while the translation goes on.
                            let (into_ref, sink) = (&into, &bounded);
                            let outcome = std::thread::scope(|s| {
                                s.spawn(move || {
                                    for t in rx {
                                        if let Some(k) = into_ref {
                                            sink([(k.clone(), Value::String(t))].into());
                                        }
                                    }
                                });
                                let r = p.translate(&text, LangPair::new(from, to), &opts);
                                drop(opts);
                                r
                            })?;
                            Value::String(outcome.translation.text)
                        }
                        "dictionary" => {
                            let to = lang_arg(&arg("to")).unwrap_or(LangId::AUTO);
                            let article = self.lookup(&text, LangPair::new(from, to), Some(module), cancel)?;
                            article_value(&article)?
                        }
                        "detect" => {
                            let d = provider.detect(&p.host(&options), &text)?;
                            Value::String(d.lang.bcp47().to_owned())
                        }
                        "speak" => {
                            let lang = lang_arg(&arg("lang")).unwrap_or(from);
                            let req = glossa_core::SpeakRequest { text: text.clone(), lang, slow: false };
                            let audio = provider.speak(&p.host(&options), &req)?;
                            out.effects.push(Effect::Play(Arc::new(audio)));
                            Value::Null
                        }
                        other => {
                            return Err(Error::Config(format!(
                                "action {action:?}: call {other:?} is not one Glossa makes"
                            )))
                        }
                    };
                    if let (Some(k), false) = (&into, value.is_null()) {
                        out.reply.state.insert(k.clone(), value);
                    }
                }
                ActionKind::Builtin => match spec.builtin.trim() {
                    "copy" => out.effects.push(Effect::Copy(value_text(&arg("text")))),
                    "open_url" => {
                        let url = value_text(&arg("url"));
                        if !safe_link(&url) {
                            return Err(Error::Config(format!("{url:?} is not an http or https address")));
                        }
                        out.effects.push(Effect::OpenUrl(url.trim().to_owned()));
                    }
                    "speak" => {
                        let lang = lang_arg(&arg("lang")).unwrap_or(LangId::AUTO);
                        let audio = self.speak_with(&value_text(&arg("text")), lang, None, cancel)?;
                        out.effects.push(Effect::Play(Arc::new(audio)));
                    }
                    other => return Err(Error::Config(format!("action {action:?}: builtin {other:?} is unknown"))),
                },
            }
            Ok(out)
        });
        tracing::debug!(
            module,
            panel = %page.id,
            action,
            ok = result.is_ok(),
            ms = started.elapsed().as_millis() as u64,
            "panel action"
        );
        let mut out = result?;
        // A panel cannot change settings behind the user's back.
        out.reply.options.clear();
        out.reply.choices.clear();
        page.bound_state(&mut out.reply.state);
        Ok(out)
    }

    /// Open panel `id`: its state from the declared defaults and the keys
    /// it keeps, read on a worker. `waker` is called from workers whenever
    /// the panel has news for [`Panel::poll`] (a repaint request).
    pub fn open_panel(&self, id: &PanelId, waker: Arc<dyn Fn() + Send + Sync>) -> Result<Panel> {
        Ok(self.open_panel_page(self.panel_page(id)?, waker))
    }

    /// [`Engine::open_panel`] for a page built elsewhere.
    pub fn open_panel_page(&self, page: Arc<PanelPage>, waker: Arc<dyn Fn() + Send + Sync>) -> Panel {
        self.open_panel_with(page, waker, true)
    }

    /// A panel for the module editor's preview, from the manifest as last
    /// checked: its actions run in the module as loaded (and fail politely
    /// when it is not), and it neither reads nor writes the kept keys.
    pub fn preview_panel(&self, page: Arc<PanelPage>, waker: Arc<dyn Fn() + Send + Sync>) -> Panel {
        self.open_panel_with(page, waker, false)
    }

    fn open_panel_with(&self, page: Arc<PanelPage>, waker: Arc<dyn Fn() + Send + Sync>, keeps: bool) -> Panel {
        let id = PanelId::new(page.module.clone(), page.id.clone());
        let options = self.registry().get(&id.module).map(|p| self.options_of(p.as_ref())).unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let mut panel = Panel {
            engine: self.clone(),
            revision: 0,
            keeps,
            id: id.clone(),
            state: page.defaults.clone(),
            page,
            options,
            tx,
            rx,
            waker,
            running: None,
            generation: 0,
            notice: None,
            errors: BTreeMap::new(),
            effects: Vec::new(),
            touched: BTreeSet::new(),
            restored: false,
            pending_change: None,
            closed: false,
        };
        if panel.page.persist.is_empty() || !keeps {
            panel.restored(OptionValues::new());
        } else {
            let (path, tx, waker) = (self.panel_state_file(&id), panel.tx.clone(), panel.waker.clone());
            self.spawn("panel state", move || {
                let saved = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<OptionValues>(&b).ok())
                    .unwrap_or_default();
                let _ = tx.send(Msg::Restored(saved));
                waker();
            });
        }
        panel
    }

    fn options_of(&self, provider: &dyn glossa_modules::Provider) -> OptionValues {
        glossa_modules::provider::resolve_options(
            provider.options(),
            self.config().services.options.get(&provider.info().id),
        )
    }

    /// Where panel `id` keeps its `persist` keys.
    pub fn panel_state_file(&self, id: &PanelId) -> PathBuf {
        let safe = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect::<String>()
        };
        self.paths().cache_dir.join("panels").join(format!("{}.{}.json", safe(&id.module), safe(&id.panel)))
    }
}

/// An action argument: `state.<key>`, `options.<key>`, `item` or
/// `item.<field>` (the clicked list row), or else the text itself.
fn resolve_arg(src: &str, state: &OptionValues, options: &OptionValues, item: Option<&Value>) -> Value {
    let src = src.trim();
    // Further `.field` steps reach into objects: `state.article.headword`.
    let walk = |root: Option<&Value>, rest: &str| -> Value {
        rest.split('.').try_fold(root, |v, step| Some(v?.get(step))).flatten().cloned().unwrap_or(Value::Null)
    };
    let split = |s: &str| -> (String, String) {
        let (head, rest) = s.split_once('.').unwrap_or((s, ""));
        (head.to_owned(), rest.to_owned())
    };
    if let Some(path) = src.strip_prefix("state.") {
        let (k, rest) = split(path);
        return if rest.is_empty() {
            state.get(&k).cloned().unwrap_or(Value::Null)
        } else {
            walk(state.get(&k), &rest)
        };
    }
    if let Some(path) = src.strip_prefix("options.") {
        let (k, rest) = split(path);
        return if rest.is_empty() {
            options.get(&k).cloned().unwrap_or(Value::Null)
        } else {
            walk(options.get(&k), &rest)
        };
    }
    if src == "item" {
        return item.cloned().unwrap_or(Value::Null);
    }
    if let Some(path) = src.strip_prefix("item.") {
        return walk(item, path);
    }
    Value::String(src.to_owned())
}

/// An article as a panel's state holds it: in the protocol's words
/// (`part_of_speech`, `definitions`), as a module would write one, so the
/// module reads back what it knows. Glossa's renderer takes both spellings.
fn article_value(a: &glossa_core::Article) -> Result<Value> {
    let mut v = serde_json::to_value(a)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("provider_id");
        o.remove("elapsed_ms");
        for e in o.get_mut("explanations").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(e) = e.as_object_mut() {
                for (from, to) in [("trait", "part_of_speech"), ("explains", "definitions")] {
                    if let Some(x) = e.remove(from) {
                        e.insert(to.to_owned(), x);
                    }
                }
            }
        }
    }
    Ok(v)
}

fn lang_arg(v: &Value) -> Option<LangId> {
    v.as_str().filter(|s| !s.trim().is_empty()).and_then(LangId::from_code)
}

enum Msg {
    /// The kept keys, read from disk.
    Restored(OptionValues),
    /// Partial state from the run of this generation.
    Update(u64, OptionValues),
    /// The run of this generation finished.
    Done(u64, String, Result<PanelOutcome>),
}

/// The action running in a panel.
struct Run {
    action: String,
    generation: u64,
    cancel: CancelToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// What the last action said, shown at the foot of the panel.
#[derive(Debug, Clone)]
pub struct PanelNotice {
    pub action: String,
    pub level: NoticeLevel,
    /// The module's own text.
    pub text: Option<String>,
    /// A failure, for the front end to word.
    pub error: Option<Arc<Error>>,
}

/// An open panel: its state, the action running, what the last one said,
/// and what it asked the front end to do. Lives on the UI thread; every
/// call returns at once, and work runs on workers that report back
/// through a channel drained by [`Panel::poll`].
pub struct Panel {
    engine: Engine,
    /// Bumped whenever the state changes, so a front end can tell.
    revision: u64,
    /// Reads and writes its `persist` keys (not a preview).
    keeps: bool,
    pub id: PanelId,
    page: Arc<PanelPage>,
    state: OptionValues,
    /// The module's saved settings, for conditions and read-only fields.
    options: OptionValues,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    waker: Arc<dyn Fn() + Send + Sync>,
    running: Option<Run>,
    /// Bumped by each run and by closing: news from an earlier run is dropped.
    generation: u64,
    notice: Option<PanelNotice>,
    /// State key -> (message, the value it was about), shown under the
    /// field bound to it while it still holds that value.
    errors: BTreeMap<String, (String, Value)>,
    effects: Vec<Effect>,
    /// Keys the user changed before the kept ones were read: theirs wins.
    touched: BTreeSet<String>,
    restored: bool,
    /// An `on_change` waiting out the debounce: (action, element, due).
    pending_change: Option<(String, String, Instant)>,
    closed: bool,
}

impl Panel {
    pub fn page(&self) -> &Arc<PanelPage> {
        &self.page
    }

    pub fn state(&self) -> &OptionValues {
        &self.state
    }

    /// A number that changes whenever the state does.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn options(&self) -> &OptionValues {
        &self.options
    }

    /// The value an element shows: its state key, or the setting it reads.
    pub fn value(&self, bind: &Bind) -> Value {
        match bind {
            Bind::State(k) => self.state.get(k).cloned().unwrap_or(Value::Null),
            Bind::Options(k) => self.options.get(k).cloned().unwrap_or(Value::Null),
            Bind::None => Value::Null,
        }
    }

    /// Whether `el` is shown and enabled now.
    pub fn element_state(&self, el: &Element) -> (bool, bool) {
        let cx = self.page.context(&self.state, &self.options);
        (el.visible(&cx), el.enabled(&cx))
    }

    /// The action running now, if any.
    pub fn running(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.action.as_str())
    }

    pub fn notice(&self) -> Option<&PanelNotice> {
        self.notice.as_ref()
    }

    /// The message a reply put under `key`, while it still holds the value
    /// the message was about.
    pub fn field_error(&self, key: &str) -> Option<&str> {
        let (msg, about) = self.errors.get(key)?;
        (self.state.get(key).unwrap_or(&Value::Null) == about).then_some(msg.as_str())
    }

    /// What actions asked the front end to do since the last call.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The labels of the settings `action` needs that are empty or wrong;
    /// empty when it may run.
    pub fn blockers(&self, action: &str) -> Vec<String> {
        let Some(provider) = self.engine.registry().get(&self.id.module) else { return Vec::new() };
        let page = provider.settings_page(&self.engine.locale().code);
        let Some(view) = page.action(action) else { return Vec::new() };
        page.action_blockers(view, Some(&self.options))
            .into_iter()
            .map(|i| {
                let o = &page.options[i];
                if o.label.is_empty() {
                    o.key.clone()
                } else {
                    o.label.clone()
                }
            })
            .collect()
    }

    /// Set state `key` (a field edited), within the bounds.
    pub fn set(&mut self, key: &str, value: Value) {
        let mut one: OptionValues = [(key.to_owned(), value)].into();
        self.page.bound_state(&mut one);
        if let Some((k, v)) = one.into_iter().next() {
            self.touched.insert(k.clone());
            self.state.insert(k, v);
            self.revision += 1;
        }
    }

    /// The user changed `el`'s value: store it, and schedule the element's
    /// `on_change` once the value has stayed put for [`CHANGE_DEBOUNCE`].
    pub fn edit(&mut self, el: &Element, value: Value) {
        let Some(key) = el.bind.state_key() else { return };
        self.set(key, value);
        if !el.on_change.is_empty() {
            self.pending_change = Some((el.on_change.clone(), el.id.clone(), Instant::now() + CHANGE_DEBOUNCE));
        }
    }

    /// Run `action` for an event of `kind` (`click`, `submit`, `change`,
    /// `open`, `shortcut`) on `element`, with `item` for a list row. Returns
    /// false, doing nothing, while another action runs (one at a time) or
    /// when the panel is closed.
    pub fn trigger(&mut self, action: &str, kind: &str, element: Option<&str>, item: Option<Value>) -> bool {
        if self.running.is_some() || self.closed || action.is_empty() {
            return false;
        }
        if self.page.spec(action).is_none() {
            self.notice = Some(PanelNotice {
                action: action.to_owned(),
                level: NoticeLevel::Error,
                text: None,
                error: Some(Arc::new(Error::Config(format!("the module declares no action {action:?}")))),
            });
            return false;
        }
        self.generation += 1;
        let generation = self.generation;
        let cancel = CancelToken::new();
        self.running = Some(Run { action: action.to_owned(), generation, cancel: cancel.clone() });
        self.notice = None;
        let req = PanelRequest {
            state: self.state.clone(),
            event: ActionEvent { kind: kind.to_owned(), element: element.map(str::to_owned), item },
        };
        let (engine, page, tx, waker, action) =
            (self.engine.clone(), self.page.clone(), self.tx.clone(), self.waker.clone(), action.to_owned());
        self.engine.spawn("panel action", move || {
            let updates = {
                let (tx, waker, cancel) = (std::sync::Mutex::new(tx.clone()), waker.clone(), cancel.clone());
                move |u: OptionValues| {
                    if !cancel.is_cancelled() {
                        if let Ok(tx) = tx.lock() {
                            let _ = tx.send(Msg::Update(generation, u));
                        }
                        waker();
                    }
                }
            };
            let r = engine.panel_action_on(&page, &action, req, &cancel, &updates);
            if !cancel.is_cancelled() {
                let _ = tx.send(Msg::Done(generation, action, r));
                waker();
            }
        });
        true
    }

    /// Apply what workers sent since the last call, and start an
    /// `on_change` whose wait is over. Returns how long until the next one
    /// is due, for a timed repaint.
    pub fn poll(&mut self) -> Option<Duration> {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Restored(mut saved) => {
                    saved.retain(|k, _| self.page.persist.contains(k) && !self.touched.contains(k));
                    self.page.bound_state(&mut saved);
                    self.state.extend(saved);
                    self.revision += 1;
                    self.restored(OptionValues::new());
                }
                Msg::Update(generation, u) => {
                    if self.running.as_ref().is_some_and(|r| r.generation == generation) {
                        self.state.extend(u);
                        self.revision += 1;
                    }
                }
                Msg::Done(generation, action, result) => {
                    if !self.running.as_ref().is_some_and(|r| r.generation == generation) {
                        continue;
                    }
                    self.running = None;
                    self.finish(action, result);
                }
            }
        }
        let (action, element, due) = self.pending_change.clone()?;
        let now = Instant::now();
        if due > now {
            return Some(due - now);
        }
        if self.running.is_none() {
            self.pending_change = None;
            self.trigger(&action, "change", Some(&element), None);
            None
        } else {
            // Wait for the running action; ask again shortly.
            Some(CHANGE_DEBOUNCE)
        }
    }

    fn restored(&mut self, _: OptionValues) {
        if self.restored {
            return;
        }
        self.restored = true;
        if let Some(a) = self.page.on_open.clone() {
            self.trigger(&a, "open", None, None);
        }
    }

    fn finish(&mut self, action: String, result: Result<PanelOutcome>) {
        match result {
            Err(e) => {
                self.notice =
                    Some(PanelNotice { action, level: NoticeLevel::Error, text: None, error: Some(Arc::new(e)) });
            }
            Ok(out) => {
                let r = out.reply;
                self.state.extend(r.state);
                self.revision += 1;
                for (key, msg) in r.errors {
                    let about = self.state.get(&key).cloned().unwrap_or(Value::Null);
                    self.errors.insert(key, (msg, about));
                }
                let level = match r.notice_level.as_deref() {
                    Some("success") => NoticeLevel::Success,
                    Some("warning") => NoticeLevel::Warning,
                    Some("error") => NoticeLevel::Error,
                    _ => NoticeLevel::Info,
                };
                self.notice = r.notice.map(|t| PanelNotice { action, level, text: Some(t), error: None });
                self.effects.extend(out.effects);
            }
        }
    }

    /// Close the panel: stop the running action (its reply is dropped) and
    /// write the kept keys on a worker. Also done when the panel is dropped.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.generation += 1;
        if let Some(r) = self.running.take() {
            r.cancel.cancel();
        }
        if self.page.persist.is_empty() || !self.restored || !self.keeps {
            // Nothing to keep, or the kept keys were never read: writing
            // now would replace them with the defaults.
            return;
        }
        let keep = self.page.persisted(&self.state);
        let path = self.engine.panel_state_file(&self.id);
        self.engine.spawn("panel state", move || {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match serde_json::to_vec_pretty(&keep) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&path, bytes) {
                        tracing::warn!("could not keep a panel's state in {}: {e}", path.display());
                    }
                }
                Err(e) => tracing::warn!("could not keep a panel's state: {e}"),
            }
        });
    }

    /// Whether [`Panel::close`] ran.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(all(test, feature = "js"))]
mod tests;
