use super::*;
use glossa_core::{Config, Paths};
use serde_json::json;

const MANIFEST: &str = r#"
    capabilities = ["translate"]
    [module]
    id = "gloss"
    name = "Gloss"
    runtime = "js"

    [[actions]]
    id = "explain"
    label = "Explain"

    [[actions]]
    id = "translate_now"
    call = "translate"
    args = { text = "state.source", to = "state.to" }
    into = "state.result"

    [[actions]]
    id = "copy_row"
    builtin = "copy"
    args = { text = "item.title" }

    [[actions]]
    id = "open"
    builtin = "open_url"
    args = { url = "state.link" }

    [[actions]]
    id = "load"

    [[ui.panels]]
    id = "main"
    title = "Glossary"
    open_from = ["tray"]
    layout = ["source", "to", "result", "words"]
    persist = ["to"]
    on_open = "load"

    [ui.panels.state]
    source = ""
    to = "de"
    result = ""
    words = []
    link = ""

    [[ui.panels.items]]
    id = "source"
    type = "textarea"
    bind = "state.source"
    on_change = "explain"

    [[ui.panels.items]]
    id = "to"
    type = "lang"
    bind = "state.to"

    [[ui.panels.items]]
    id = "result"
    type = "result"
    bind = "state.result"

    [[ui.panels.items]]
    id = "words"
    type = "list"
    bind = "state.words"
    on_click = "copy_row"
"#;

const SCRIPT: &str = r#"
    function translate(req) { return req.text.toUpperCase() + " (" + req.to + ")"; }
    function load(ev) { return { state: { words: [{ title: "Haus", subtitle: "house" }], planted: 1 } }; }
    function explain(ev, host) {
        host.ui.update({ result: "thinking" });
        host.ui.update({ result: "x".repeat(300000) });
        return { state: { result: "explained " + ev.state.source }, options: { secret: "no" }, notice: ev.event.type };
    }
"#;

fn engine(tag: &str) -> (Engine, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("glossa-engine-panels-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let module = dir.join("home").join("modules").join("gloss");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("module.toml"), MANIFEST).unwrap();
    std::fs::write(module.join("main.js"), SCRIPT).unwrap();
    let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
    (Engine::with_config(paths, Config::default()).unwrap(), dir)
}

/// Poll until the kept keys are read and no action runs.
fn settle(p: &mut Panel) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        p.poll();
        if p.running().is_none() && p.restored {
            break;
        }
        assert!(Instant::now() < until, "the panel did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn req(state: &OptionValues, item: Option<Value>) -> PanelRequest {
    PanelRequest { state: state.clone(), event: ActionEvent { kind: "click".into(), element: None, item } }
}

#[test]
fn panels_are_listed_and_named() {
    let (e, dir) = engine("list");
    let list = e.panels();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id.as_str(), list[0].title.as_str()), ("gloss/main", "Gloss: Glossary"));
    assert_eq!(list[0].open_from, ["tray"]);
    assert_eq!(PanelId::parse("gloss/main").unwrap(), PanelId::new("gloss", "main"));
    for bad in ["gloss", "/main", "gloss/", "a/b/c"] {
        assert!(PanelId::parse(bad).is_err(), "{bad}");
    }
    assert!(e.panel_page(&PanelId::new("gloss", "nope")).unwrap_err().to_string().contains("it has main"));
    assert!(matches!(e.panel_page(&PanelId::new("ghost", "x")), Err(Error::NoSuchModule(_))));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn dispatch_runs_handlers_calls_and_builtins() {
    let (e, dir) = engine("dispatch");
    let id = PanelId::new("gloss", "main");
    let cancel = CancelToken::new();
    let state: OptionValues = [("source".to_owned(), json!("hund")), ("to".to_owned(), json!("en"))].into();

    // A call of the module's own capability, into the named key.
    let out = e.panel_action(&id, "translate_now", req(&state, None), &cancel, &|_| {}).unwrap();
    assert_eq!(out.reply.state["result"], "HUND (en)");

    // A handler: streamed updates and its reply, both bounded; settings
    // and keys the panel does not know are dropped.
    let seen = std::sync::Mutex::new(Vec::new());
    let out = e.panel_action(&id, "explain", req(&state, None), &cancel, &|u| seen.lock().unwrap().push(u)).unwrap();
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0]["result"], "thinking");
    let streamed = seen[1]["result"].as_str().unwrap().len();
    assert!(streamed <= glossa_modules::panels::MAX_TEXT + 3, "updates are bounded");
    assert_eq!(out.reply.state["result"], "explained hund");
    assert!(out.reply.options.is_empty(), "a panel cannot change settings");
    let out = e.panel_action(&id, "load", req(&state, None), &cancel, &|_| {}).unwrap();
    assert!(!out.reply.state.contains_key("planted"), "undeclared keys are dropped");

    // Built-ins: copy the clicked row, open only web addresses.
    let row = Some(json!({ "title": "Haus" }));
    let out = e.panel_action(&id, "copy_row", req(&state, row), &cancel, &|_| {}).unwrap();
    assert!(matches!(out.effects.as_slice(), [Effect::Copy(t)] if t == "Haus"));
    let mut s = state.clone();
    s.insert("link".into(), json!("https://example.com/haus"));
    let out = e.panel_action(&id, "open", req(&s, None), &cancel, &|_| {}).unwrap();
    assert!(matches!(out.effects.as_slice(), [Effect::OpenUrl(u)] if u == "https://example.com/haus"));
    s.insert("link".into(), json!("file:///etc/passwd"));
    assert!(e.panel_action(&id, "open", req(&s, None), &cancel, &|_| {}).is_err());

    // The settings page's entry point reaches panels too.
    let r = e
        .module_action(
            "gloss",
            "translate_now",
            crate::ActionRequest { state: state.clone(), panel: Some("main".into()), ..Default::default() },
        )
        .unwrap();
    assert_eq!(r.state["result"], "HUND (en)");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_open_panel_streams_debounces_and_keeps_its_keys() {
    let (e, dir) = engine("live");
    let id = PanelId::new("gloss", "main");
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = wakes.clone();
    let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let mut p = e.open_panel(&id, waker.clone()).unwrap();
    assert_eq!(p.state()["to"], "de", "the declared default");
    settle(&mut p);
    assert_eq!(p.state()["words"][0]["title"], "Haus", "on_open ran once the kept keys were read");
    assert!(wakes.load(std::sync::atomic::Ordering::SeqCst) > 0, "workers wake the front end");

    // An edit with on_change waits out the debounce, then runs.
    let source = p.page().element("source").unwrap().clone();
    p.edit(&source, json!("katze"));
    let wait = p.poll().expect("a change is pending");
    assert!(wait <= CHANGE_DEBOUNCE);
    assert!(p.running().is_none(), "not before the pause");
    std::thread::sleep(wait + Duration::from_millis(20));
    p.poll();
    assert_eq!(p.running(), Some("explain"));
    assert!(!p.trigger("translate_now", "click", None, None), "one action at a time");
    settle(&mut p);
    assert_eq!(p.state()["result"], "explained katze");
    assert_eq!(p.notice().unwrap().text.as_deref(), Some("change"), "the event's type reaches the handler");

    // A row click hands the front end something to do.
    assert!(p.trigger("copy_row", "click", Some("words"), Some(json!({ "title": "Haus" }))));
    settle(&mut p);
    assert!(matches!(p.take_effects().as_slice(), [Effect::Copy(t)] if t == "Haus"));
    assert!(p.take_effects().is_empty());

    // Closing keeps `to`, and only `to`; a new opening reads it back.
    p.set("to", json!("fr"));
    p.set("source", json!("gone"));
    p.close();
    let file = e.panel_state_file(&id);
    let until = Instant::now() + Duration::from_secs(5);
    while !file.exists() {
        assert!(Instant::now() < until, "the kept keys were not written");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    let mut again = e.open_panel(&id, waker).unwrap();
    settle(&mut again);
    assert_eq!(again.state()["to"], "fr");
    assert_eq!(again.state()["source"], "", "only persist keys are kept");
    drop(again);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_closed_panel_drops_its_late_reply() {
    let (e, dir) = engine("late");
    let mut p = e.open_panel(&PanelId::new("gloss", "main"), Arc::new(|| {})).unwrap();
    settle(&mut p);
    assert!(p.trigger("explain", "click", None, None));
    p.close();
    assert!(p.running().is_none() && p.is_closed());
    std::thread::sleep(Duration::from_millis(300));
    p.poll();
    assert_eq!(p.state()["result"], "", "nothing lands after closing");
    assert!(!p.trigger("explain", "click", None, None), "a closed panel runs nothing");
    std::fs::remove_dir_all(&dir).ok();
}
