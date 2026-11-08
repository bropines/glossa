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
use glossa_modules::panels::{
    fit_merge, fit_size, safe_link, value_text, Bind, Element, PanelPage, MAX_PERSIST_BYTES, MAX_STATE_BYTES,
    MAX_UPDATE_BYTES,
};
use glossa_modules::wire::{ActionEvent, ActionParams, ActionReply};
use glossa_modules::{CancelToken, OptionValues};
use serde::Serialize;
use serde_json::Value;

use crate::{Engine, TranslateOptions};

/// How long an input must stay unchanged before its `on_change` action runs.
pub const CHANGE_DEBOUNCE: Duration = Duration::from_millis(400);

/// The events that are the user's own doing. Effects that reach beyond the
/// panel (the clipboard, the browser) run only for these, never for
/// `open` or `change`, which happen without a deliberate gesture.
pub const GESTURES: [&str; 3] = ["click", "submit", "shortcut"];

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
            let dropped = fit_size(&mut u, MAX_UPDATE_BYTES);
            if !dropped.is_empty() {
                tracing::debug!(
                    module,
                    ?dropped,
                    "a streamed update over {MAX_UPDATE_BYTES} bytes lost its largest values"
                );
            }
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
                            // A streaming service reports the whole text
                            // so far each time, on this thread; only the
                            // latest matters. A helper forwards it into the
                            // panel a few times a second while the
                            // translation goes on, instead of queueing every
                            // step.
                            let latest = Arc::new(parking_lot::Mutex::new(None::<String>));
                            let done = std::sync::atomic::AtomicBool::new(false);
                            let slot = latest.clone();
                            let opts = TranslateOptions {
                                provider: Some(module.to_owned()),
                                fail_over: Some(false),
                                progress: Some(crate::Progress::new(move |t| {
                                    *slot.lock() = Some(t.to_owned());
                                })),
                                ..Default::default()
                            };
                            let (into_ref, sink, done_ref, latest_ref) = (&into, &bounded, &done, &latest);
                            let outcome = std::thread::scope(|s| {
                                s.spawn(move || loop {
                                    let finished = done_ref.load(std::sync::atomic::Ordering::SeqCst);
                                    if let (Some(t), Some(k)) = (latest_ref.lock().take(), into_ref) {
                                        sink([(k.clone(), Value::String(t))].into());
                                    }
                                    if finished {
                                        break;
                                    }
                                    std::thread::sleep(Duration::from_millis(80));
                                });
                                let r = p.translate(&text, LangPair::new(from, to), &opts);
                                done.store(true, std::sync::atomic::Ordering::SeqCst);
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
                    b @ ("copy" | "open_url") if !GESTURES.contains(&req.event.kind.as_str()) => {
                        return Err(Error::Config(format!(
                            "action {action:?}: {b} runs only when the user clicks, presses Enter or a shortcut, \
                             not on {:?}",
                            req.event.kind
                        )))
                    }
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
        fit_size(&mut out.reply.state, MAX_STATE_BYTES);
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
            changed: BTreeMap::new(),
            streamed: Arc::new(parking_lot::Mutex::new(None)),
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
            pending_changes: BTreeMap::new(),
            closed: false,
        };
        if panel.page.persist.is_empty() || !keeps {
            panel.restored(OptionValues::new());
        } else {
            let (path, tx, waker) = (self.panel_state_file(&id), panel.tx.clone(), panel.waker.clone());
            self.spawn("panel state", move || {
                // A file grown past the limit (edited by hand, say) is
                // ignored rather than read whole.
                let small = std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_PERSIST_BYTES as u64);
                let mut saved = small
                    .then(|| std::fs::read(&path).ok())
                    .flatten()
                    .and_then(|b| serde_json::from_slice::<OptionValues>(&b).ok())
                    .unwrap_or_default();
                fit_size(&mut saved, MAX_PERSIST_BYTES);
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
    /// Partial state from the run of this generation is waiting in
    /// [`Panel::streamed`].
    Streamed,
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
    /// The revision at which each state key last changed.
    changed: BTreeMap<String, u64>,
    /// Streamed updates not yet applied, coalesced (the latest value of
    /// each key), with the generation of the run they came from. Workers
    /// write here and [`Panel::poll`] takes it, so a module that streams
    /// faster than the window draws queues nothing.
    streamed: Arc<parking_lot::Mutex<Option<(u64, OptionValues)>>>,
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
    /// `on_change` actions waiting out the debounce, one per element:
    /// element -> (action, due).
    pending_changes: BTreeMap<String, (String, Instant)>,
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

    /// The [`Panel::revision`] at which state `key` last changed (0: never
    /// since the panel opened), so a front end redoes work for a value (an
    /// image to decode) only when that value changed.
    pub fn changed_at(&self, key: &str) -> u64 {
        self.changed.get(key).copied().unwrap_or(0)
    }

    /// Merge `incoming` into the state, within the total limit; a value that
    /// would take the state over it keeps its old value.
    fn merge(&mut self, mut incoming: OptionValues) {
        let dropped = fit_merge(&self.state, &mut incoming, MAX_STATE_BYTES);
        if !dropped.is_empty() {
            tracing::debug!(panel = %self.id, ?dropped, "values over the panel's {MAX_STATE_BYTES} bytes were not taken");
        }
        if incoming.is_empty() {
            return;
        }
        self.revision += 1;
        for (k, v) in incoming {
            self.changed.insert(k.clone(), self.revision);
            self.state.insert(k, v);
        }
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
        if let Some(k) = one.keys().next().cloned() {
            self.touched.insert(k);
            self.merge(one);
        }
    }

    /// The user changed `el`'s value: store it, and schedule the element's
    /// `on_change` once the value has stayed put for [`CHANGE_DEBOUNCE`].
    pub fn edit(&mut self, el: &Element, value: Value) {
        let Some(key) = el.bind.state_key() else { return };
        self.set(key, value);
        if !el.on_change.is_empty() {
            // One wait per element: a change in another field does not
            // replace this one's.
            self.pending_changes.insert(el.id.clone(), (el.on_change.clone(), Instant::now() + CHANGE_DEBOUNCE));
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
        *self.streamed.lock() = None;
        let (engine, page, tx, waker, action, streamed) = (
            self.engine.clone(),
            self.page.clone(),
            self.tx.clone(),
            self.waker.clone(),
            action.to_owned(),
            self.streamed.clone(),
        );
        self.engine.spawn("panel action", move || {
            let updates = {
                let (tx, waker, cancel) = (std::sync::Mutex::new(tx.clone()), waker.clone(), cancel.clone());
                move |u: OptionValues| {
                    if cancel.is_cancelled() {
                        return;
                    }
                    // Coalesce: the latest value of each key, within the
                    // size of one update; announce only the first.
                    let first = {
                        let mut slot = streamed.lock();
                        let first = slot.as_ref().is_none_or(|(g, _)| *g != generation);
                        if first {
                            *slot = Some((generation, OptionValues::new()));
                        }
                        if let Some((_, pending)) = slot.as_mut() {
                            pending.extend(u);
                            fit_size(pending, MAX_UPDATE_BYTES);
                        }
                        first
                    };
                    if first {
                        if let Ok(tx) = tx.lock() {
                            let _ = tx.send(Msg::Streamed);
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
                    self.merge(saved);
                    self.restored(OptionValues::new());
                }
                Msg::Streamed => self.take_streamed(),
                Msg::Done(generation, action, result) => {
                    if !self.running.as_ref().is_some_and(|r| r.generation == generation) {
                        continue;
                    }
                    // What was streamed lands before the final reply.
                    self.take_streamed();
                    self.running = None;
                    self.finish(action, result);
                }
            }
        }
        // `on_change` actions whose wait is over, earliest first, one at a
        // time.
        let now = Instant::now();
        let next_due = self.pending_changes.values().map(|(_, due)| *due).min()?;
        if next_due > now {
            return Some(next_due - now);
        }
        if self.running.is_some() {
            // Wait for the running action; its end wakes the panel.
            return Some(Duration::from_millis(50));
        }
        let element = self
            .pending_changes
            .iter()
            .filter(|(_, (_, due))| *due <= now)
            .min_by_key(|(_, (_, due))| *due)
            .map(|(el, _)| el.clone())?;
        if let Some((action, _)) = self.pending_changes.remove(&element) {
            self.trigger(&action, "change", Some(&element), None);
        }
        (!self.pending_changes.is_empty()).then_some(Duration::from_millis(50))
    }

    /// Apply the streamed updates of the running action.
    fn take_streamed(&mut self) {
        let taken = self.streamed.lock().take();
        if let Some((generation, u)) = taken {
            if self.running.as_ref().is_some_and(|r| r.generation == generation) {
                self.merge(u);
            }
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
                self.merge(r.state);
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
        let mut keep = self.page.persisted(&self.state);
        let dropped = fit_size(&mut keep, MAX_PERSIST_BYTES);
        if !dropped.is_empty() {
            tracing::debug!(panel = %self.id, ?dropped, "kept keys over {MAX_PERSIST_BYTES} bytes are not written");
        }
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
