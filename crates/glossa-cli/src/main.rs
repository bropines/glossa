//! `glossa-cli` — Glossa without a window.
//!
//! Everything the desktop program can do that makes sense in a terminal:
//! translate (text as arguments or on stdin), detect, look a word up,
//! recognise an image, speak, list and inspect providers and modules, import
//! a classic installation, convert themes and locales, and run the control
//! API in the foreground.
//!
//! Results go to stdout and everything else to stderr, so
//! `glossa-cli translate -t ru < in.txt > out.txt` holds only the translation.
//! Exit codes: 0 success, 1 the operation failed, 64 bad command line.

mod doctor;

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use glossa_core::locale::Severity;
use glossa_core::{Capabilities, LangId, LangPair, Paths};
use std::sync::{Arc, Mutex};

use glossa_engine::{Engine, ExportFormat, TranslateOptions};
use serde_json::json;

/// `println!` that exits quietly when stdout is closed (`glossa-cli … | head`)
/// instead of panicking.
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = writeln!(std::io::stdout(), $($t)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        }
    }};
}

/// `print!`, likewise.
macro_rules! outp {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = write!(std::io::stdout(), $($t)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        }
    }};
}

const USAGE: &str = "\
glossa-cli — translate, look up and recognise text from the command line

USAGE
    glossa-cli [--json] [--home DIR] [--offline] [-v] <command> [options] [text…]
    glossa-cli --version              the version and the commit it was built from

    --offline                         for this run, nothing leaves this computer: only services
                                      running here are used ([network] offline_only makes it permanent)

COMMANDS
    translate  [-f FROM] [-t TO] [-s SERVICE] [--back] [--save-history] [--full]
               [--detect offline|service|offline_then_service] [TEXT…]
                                      stdin if no text; piped output is the translation alone
                                      unless --full; text longer than the service takes goes in pieces
    detect     [--detect offline|service|offline_then_service] [TEXT…]
                                      offline: the detector built into Glossa, nothing is sent
    dict       [-f FROM] [-t TO] [-s SERVICE] WORD
    ocr        IMAGE [-l LANG] [-t TO] [-e ENGINE] [-F text|json|hocr|alto]
               [--in-place] [--join-lines] [--translate-with ocr|translator]
                                      ENGINE: lens, windows-ocr, oneocr, macos-vision, tesseract, command-ocr…;
                                      -F json/hocr/alto print the layout (blocks, lines, words with boxes);
                                      --in-place translates block by block (each block with its box under
                                      --json); --join-lines joins lines into paragraphs; --translate-with ocr
                                      lets the OCR engine translate when it can (Google Lens), translator
                                      always uses the current translator
    speak      [-l LANG] [--slow] -o OUT.mp3 TEXT…
    services   [-c CAPABILITY]        translate | dictionary | listen | ocr | detect_language
    modules                           what loaded, what failed and why
    module test [--live|--record] [--case NAME] [--set-from-env KEY=VAR] [--set KEY=VALUE]
                [--timeout SECS] [--strict] [--require-runtimes] [--ignore-recordings] [--download] DIR…
                                      check a module folder (or every module below DIR), load it
                                      and run its fixtures (tests/*.toml) with recorded HTTP;
                                      --record writes tests/recorded/, which replays then prefer
                                      over the fixtures' own answers (--ignore-recordings: not);
                                      a module whose files are not downloaded is skipped, unless
                                      --download fetches them first
    module check [--strict] DIR…      the same checks without loading or running anything
    module pending                    modules installed by hand that wait for approval, and what
                                      each asks for (hosts, programs, downloads, packages, actions)
    module approve|revoke|decline ID [--local] [--force]
                                      approve what a module asks for, withdraw the approval, or
                                      switch it off; through a running Glossa when it answers
    module action ID ACTION [--option KEY=VALUE] [--set-from-env KEY=VAR] [--state KEY=VALUE]
                                      run one of an installed module's [[actions]] (a settings-page
                                      button) with the saved settings, KEY=VALUE over them, and print
                                      its reply (notice, choices, errors, options); with --panel NAME
                                      any action runs as that panel runs it (call, builtin)
    module panel [ID/PANEL [--action ACTION] [--state KEY=VALUE] [--item JSON] [--open]]
                                      module panels: without a name, list them; with one, show its
                                      elements and state; --action runs one of its actions and prints
                                      the streamed updates, the state after and what Glossa would do
                                      (copy, open, play); --open asks a running Glossa to open it
    assets     [list | status ID | download ID [NAME…] | verify ID | remove ID]
                                      files modules declare (models, dictionaries): their state,
                                      downloading (resumes, checks SHA-256; NAMEs pick optional
                                      files, none fetches the required ones), re-checking,
                                      deleting; also `module assets …`
    preset     [list | apply NAME | next | save NAME [--languages] | export NAME [-o FILE]
               | import FILE | rename NAME NEW | duplicate NAME [NEW] | delete NAME] [--local] [--force]
                                      named sets of services, their order and options (never keys);
                                      changes go to a running Glossa through the control API when
                                      one answers, else to the settings file (--local: always the file);
                                      a running Glossa that does not answer is not written behind
                                      unless --force
    python     [status | install [MODULE] | remove [MODULE]]
                                      the Python modules run with: which one each module uses,
                                      downloading the managed Python and uv (with MODULE: also
                                      installing that module's packages), deleting them all
                                      (with MODULE: only that module's environment)
    history    [--search Q] [--favorites] [--export FILE.{txt,csv,html,json}]
    import-classic DIR                settings, history, and use its services/themes/locales
    theme      list | show NAME | convert FILE    (classic JSON -> native TOML)
    locale     list | convert FILE                (classic lang.json -> native JSON)
               check FILE | template [LANG]       (validate a pack; a pack to translate from)
    config     path | show
    serve                             run the control API (and on Linux the D-Bus service) until Ctrl+C
    action     ACTION [TEXT]          ask a running Glossa to act (show_popup, text_recognition…)
    doctor                            what works here: session, hotkeys, selection, OCR engines,
                                      modules, permissions, files, proxy, control API

Languages are codes (en, ru, zh-CN, he, auto). SERVICE is a provider id from `services`.

Support the project: https://boosty.to/pinus
";

struct Args {
    json: bool,
    home: Option<PathBuf>,
    verbose: bool,
    command: String,
    flags: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

impl Args {
    fn flag(&self, names: &[&str]) -> Option<&str> {
        self.flags.iter().rev().find(|(k, _)| names.contains(&k.as_str())).and_then(|(_, v)| v.as_deref())
    }

    fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|(k, _)| names.contains(&k.as_str()))
    }
}

/// Flags that take a value.
const VALUED: &[&str] = &[
    "-f",
    "--from",
    "-t",
    "--to",
    "-s",
    "--service",
    "-e",
    "--engine",
    "-l",
    "--lang",
    "-o",
    "--out",
    "-c",
    "--capability",
    "--search",
    "--export",
    "--home",
    "-F",
    "--format",
    "--translate-with",
    "--case",
    "--set",
    "--set-from-env",
    "--timeout",
    "--option",
    "--state",
    "--panel",
    "--action",
    "--item",
    "--detect",
];

/// Flags that take no value.
const SWITCHES: &[&str] = &[
    "--json",
    "-v",
    "--verbose",
    "-h",
    "--help",
    "-V",
    "--version",
    "--back",
    "--slow",
    "--favorites",
    "--save-history",
    "--full",
    "--in-place",
    "--join-lines",
    "--download",
    "--live",
    "--record",
    "--strict",
    "--require-runtimes",
    "--ignore-recordings",
    "--languages",
    "--local",
    "--force",
    "--open",
    "--offline",
];

/// Flags every command takes.
const GLOBAL: &[&str] = &["--json", "-v", "--verbose", "-h", "--help", "-V", "--version", "--home", "--offline"];

/// The flags each command takes besides the global ones, or `None` for a
/// command that does not exist (that error is reported instead).
fn command_flags(command: &str) -> Option<&'static [&'static str]> {
    Some(match command {
        "translate" | "tr" => {
            &["-f", "--from", "-t", "--to", "-s", "--service", "--back", "--save-history", "--full", "--detect"]
        }
        "dict" | "dictionary" => &["-f", "--from", "-t", "--to", "-s", "--service", "--detect"],
        "detect" => &["--detect"],
        "ocr" => &[
            "-l",
            "--lang",
            "-t",
            "--to",
            "-e",
            "--engine",
            "-s",
            "--service",
            "-F",
            "--format",
            "--in-place",
            "--join-lines",
            "--translate-with",
        ],
        "speak" => &["-o", "--out", "-l", "--lang", "--slow", "--detect"],
        "preset" | "presets" => &["-o", "--out", "--languages", "--local", "--force"],
        "services" => &["-c", "--capability"],
        "history" => &["--search", "--favorites", "--export"],
        "module" => &[
            "--live",
            "--record",
            "--case",
            "--set",
            "--set-from-env",
            "--timeout",
            "--strict",
            "--require-runtimes",
            "--ignore-recordings",
            "--download",
            "--option",
            "--state",
            "--panel",
            "--local",
            "--force",
            "--action",
            "--item",
            "--open",
        ],
        "modules" | "assets" | "python" | "import-classic" | "theme" | "locale" | "config" | "serve" | "action"
        | "doctor" | "" | "help" | "version" => &[],
        _ => return None,
    })
}

fn parse(argv: Vec<String>) -> Result<Args, String> {
    let mut a = Args { json: false, home: None, verbose: false, command: String::new(), flags: vec![], rest: vec![] };
    let mut it = argv.into_iter().peekable();
    let mut only_text = false;
    while let Some(arg) = it.next() {
        if only_text {
            a.rest.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => only_text = true,
            flag if flag.starts_with('-')
                && flag.len() > 1
                && !flag.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) =>
            {
                let (name, inline) = match flag.split_once('=') {
                    Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                    None => (flag.to_owned(), None),
                };
                let value = if VALUED.contains(&name.as_str()) {
                    Some(match inline {
                        Some(v) => v,
                        None => it.next().ok_or_else(|| format!("{name} needs a value"))?,
                    })
                } else if SWITCHES.contains(&name.as_str()) {
                    if inline.is_some() {
                        return Err(format!("{name} takes no value"));
                    }
                    None
                } else {
                    // Unknown: kept so the check after parsing can name it
                    // with the closest real flag for the command.
                    inline
                };
                match name.as_str() {
                    "--json" => a.json = true,
                    "-v" | "--verbose" => a.verbose = true,
                    "-h" | "--help" => a.command = "help".into(),
                    "-V" | "--version" => a.command = "version".into(),
                    _ => {}
                }
                a.flags.push((name, value));
            }
            _ if a.command.is_empty() => a.command = arg,
            _ => a.rest.push(arg),
        }
    }
    if let Some(home) = a.flag(&["--home"]) {
        if home.trim().is_empty() {
            return Err("--home needs a directory".into());
        }
        a.home = Some(home.into());
    }
    Ok(a)
}

/// Refuse flags the command does not take, naming the closest one it does:
/// a misspelt `--slwo` must not quietly do nothing.
fn check_flags(a: &Args) -> Result<(), String> {
    if a.command == "help" {
        return Ok(());
    }
    let Some(own) = command_flags(&a.command) else { return Ok(()) };
    let takes = |name: &str| GLOBAL.contains(&name) || own.contains(&name);
    for (name, _) in &a.flags {
        if takes(name) {
            continue;
        }
        let shown = if a.command.is_empty() { "glossa-cli".to_owned() } else { a.command.clone() };
        let known = VALUED.contains(&name.as_str()) || SWITCHES.contains(&name.as_str());
        let near = glossa_core::text::closest(name, GLOBAL.iter().chain(own).copied(), 2);
        let hint = match near.as_slice() {
            [] => String::new(),
            [one] => format!("; did you mean {one}?"),
            [first, second, ..] => format!("; did you mean {first} or {second}?"),
        };
        return Err(if known {
            format!("{shown} does not take {name}{hint}")
        } else {
            format!("unknown flag {name}{hint}")
        });
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1).collect()).and_then(|a| check_flags(&a).map(|()| a)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("glossa-cli: {e}\n\n{USAGE}");
            return ExitCode::from(64);
        }
    };
    let filter = if args.verbose { "debug" } else { "warn" };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("GLOSSA_LOG").unwrap_or_else(|_| filter.into()))
        .with_writer(std::io::stderr)
        .try_init();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Usage(msg)) => {
            eprintln!("glossa-cli: {msg}\n\n{USAGE}");
            ExitCode::from(64)
        }
        Err(Fail::Op(msg)) => {
            if args.json {
                out!("{}", json!({ "error": msg }));
            } else {
                eprintln!("glossa-cli: {msg}");
            }
            ExitCode::from(1)
        }
    }
}

enum Fail {
    Usage(String),
    Op(String),
}

impl From<glossa_core::Error> for Fail {
    fn from(e: glossa_core::Error) -> Self {
        Fail::Op(e.to_string())
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Op(e.to_string())
    }
}

fn paths(args: &Args) -> Paths {
    match &args.home {
        Some(home) => Paths::for_exe_dir(
            std::env::current_exe().ok().and_then(|p| p.parent().map(Into::into)).unwrap_or_default(),
            Some(home.clone()),
        ),
        None => Paths::discover(),
    }
}

fn engine(args: &Args) -> Result<Engine, Fail> {
    let e = Engine::start(paths(args))?;
    // For this run only; neither is written to the settings file.
    let detect = match args.flag(&["--detect"]) {
        None => None,
        Some(name) => Some(glossa_core::config::DetectMode::from_name(name).ok_or_else(|| {
            Fail::Usage(format!("--detect is offline, service or offline_then_service, not {name:?}"))
        })?),
    };
    let overrides = glossa_engine::SessionOverrides { offline_only: args.has(&["--offline"]), detect };
    if overrides != glossa_engine::SessionOverrides::default() {
        e.set_session_overrides(overrides)?;
    }
    e.attach_secret_store(glossa_platform::secrets::system_store());
    for w in e.startup_warnings() {
        eprintln!("glossa-cli: {}", w.message(&glossa_core::Locale::english()));
    }
    Ok(e)
}

fn lang(code: Option<&str>, default: LangId) -> Result<LangId, Fail> {
    match code {
        None => Ok(default),
        Some(c) => LangId::from_code(c).ok_or_else(|| Fail::Usage(format!("unknown language {c:?}"))),
    }
}

/// The provider id given with one of `names`, checked: an id that does not
/// exist (or cannot do `cap`) is a usage error naming similar ids, instead
/// of being skipped while another service answers.
fn service(e: &Engine, args: &Args, names: &[&str], cap: Capabilities) -> Result<Option<String>, Fail> {
    match args.flag(names) {
        None => Ok(None),
        Some(id) => {
            e.require_provider(id, cap).map_err(Fail::Usage)?;
            Ok(Some(id.to_owned()))
        }
    }
}

/// The text argument, or stdin when there is none and stdin is not a terminal.
fn text(args: &Args) -> Result<String, Fail> {
    if !args.rest.is_empty() {
        return Ok(args.rest.join(" "));
    }
    if std::io::stdin().is_terminal() {
        return Err(Fail::Usage("no text given".into()));
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s)?;
    Ok(s)
}

/// `KEY=VALUE` from the command line: a number or a boolean as one, anything
/// else as text.
fn key_value(flag: &str, pair: &str) -> Result<(String, serde_json::Value), Fail> {
    let (k, v) = pair.split_once('=').ok_or_else(|| Fail::Usage(format!("{flag} {pair:?}: use KEY=VALUE")))?;
    let value = serde_json::from_str::<serde_json::Value>(v)
        .ok()
        .filter(|j| j.is_number() || j.is_boolean())
        .unwrap_or(json!(v));
    Ok((k.trim().to_owned(), value))
}

/// `module action ID ACTION`: run one of an installed module's actions, as
/// its button on the settings page does, and print the reply.
fn module_action(args: &Args) -> Result<(), Fail> {
    let (Some(id), Some(action)) = (args.rest.get(1), args.rest.get(2)) else {
        return Err(Fail::Usage("module action ID ACTION [--option KEY=VALUE]… [--state KEY=VALUE]…".into()));
    };
    let mut overrides = glossa_modules::OptionValues::new();
    let mut state = glossa_modules::OptionValues::new();
    for (name, value) in &args.flags {
        let pair = value.as_deref().unwrap_or_default();
        match name.as_str() {
            "--option" | "--set" => {
                let (k, v) = key_value(name, pair)?;
                overrides.insert(k, v);
            }
            // A key from the environment stays out of the shell's history.
            "--set-from-env" => {
                let (k, var) = pair
                    .split_once('=')
                    .ok_or_else(|| Fail::Usage(format!("--set-from-env {pair:?}: use KEY=VARIABLE")))?;
                let v = std::env::var(var.trim())
                    .map_err(|_| Fail::Usage(format!("--set-from-env: the variable {var} is not set")))?;
                overrides.insert(k.trim().to_owned(), json!(v));
            }
            "--state" => {
                let (k, v) = key_value(name, pair)?;
                state.insert(k, v);
            }
            _ => {}
        }
    }
    let e = engine(args)?;
    // The saved settings, with what was given on the command line over them.
    let options = (!overrides.is_empty()).then(|| {
        let mut o = e.config().services.options.get(id.as_str()).cloned().unwrap_or_default();
        o.extend(overrides);
        o
    });
    let req = glossa_engine::ActionRequest { options, state, panel: args.flag(&["--panel"]).map(str::to_owned) };
    let reply = e.module_action(id, action, req)?;
    if args.json {
        out!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
        return Ok(());
    }
    let mut said = false;
    if let Some(n) = &reply.notice {
        match reply.notice_level.as_deref() {
            Some(level @ ("warning" | "error")) => out!("{level}: {n}"),
            _ => out!("{n}"),
        }
        said = true;
    }
    for (key, msg) in &reply.errors {
        out!("{key}: {msg}");
        said = true;
    }
    for (key, list) in &reply.choices {
        out!("choices for {key} ({}):", list.len());
        for c in list {
            if c.label.is_empty() || c.label == c.value {
                out!("  {}", c.value);
            } else {
                out!("  {}  {}", c.value, c.label);
            }
        }
        said = true;
    }
    for (k, v) in &reply.options {
        out!("option {k} = {v}");
        said = true;
    }
    for (k, v) in &reply.state {
        out!("state {k} = {v}");
        said = true;
    }
    if !said {
        out!("done");
    }
    if !reply.errors.is_empty() || reply.notice_level.as_deref() == Some("error") {
        // A refused key is a failure for scripts, though the call worked.
        std::process::exit(1);
    }
    Ok(())
}

/// `module pending|approve|revoke|decline`: approving modules installed by
/// hand. Through a running Glossa's control API when there is one, since it
/// keeps its settings in memory; otherwise on the settings file.
fn module_trust(args: &Args, op: &str) -> Result<(), Fail> {
    let id = args.rest.get(1).map(String::as_str);
    if op != "pending" && id.is_none() {
        return Err(Fail::Usage(format!("module {op} needs a module id (see `glossa-cli module pending`)")));
    }
    let local = args.has(&["--local"]);
    let remote = if local { None } else { running_glossa(args) };
    if remote.is_none() && op != "pending" {
        refuse_behind_a_running_glossa(args, local)?;
    }
    let local_engine = match remote {
        Some(_) => None,
        None => Some(engine(args)?),
    };
    let list = || -> Result<Vec<serde_json::Value>, Fail> {
        match (&remote, &local_engine) {
            (Some((addr, token)), _) => {
                let v = parse_answer(&control_api(addr, token, false, "/v1/trust", vec![])?)?;
                Ok(v.as_array().cloned().unwrap_or_default())
            }
            (None, Some(e)) => Ok(e.module_trust().iter().map(|m| json!(m)).collect()),
            (None, None) => Ok(Vec::new()),
        }
    };
    let call = |what: &str, body: serde_json::Value| -> Result<(), Fail> {
        let id = id.unwrap_or_default();
        match (&remote, &local_engine) {
            (Some((addr, token)), _) => {
                control_api(addr, token, true, &format!("/v1/trust/{id}/{what}"), body.to_string().into_bytes())?;
                Ok(())
            }
            (None, Some(e)) => Ok(match what {
                "approve" => e.approve_module(id, body["hash"].as_str()),
                "revoke" => e.revoke_module(id),
                _ => e.decline_module(id),
            }?),
            (None, None) => Ok(()),
        }
    };
    match op {
        "pending" => {
            let waiting: Vec<_> = list()?.into_iter().filter(|m| m["pending"] == json!(true)).collect();
            if args.json {
                out!("{}", serde_json::to_string_pretty(&waiting).unwrap_or_default());
            } else if waiting.is_empty() {
                out!("no module is waiting for approval");
            } else {
                for m in &waiting {
                    let why = if m["first_time"] == json!(true) { "new" } else { "asks for more since approved" };
                    out!(
                        "{} ({}): {why}",
                        m["id"].as_str().unwrap_or_default(),
                        m["name"].as_str().unwrap_or_default()
                    );
                    for line in describe_request(&m["request"]) {
                        out!("  {line}");
                    }
                }
                out!(
                    "\napprove with `glossa-cli module approve ID`, or switch off with `glossa-cli module decline ID`"
                );
            }
            Ok(())
        }
        "approve" => {
            let all = list()?;
            let id = id.unwrap_or_default();
            let Some(m) = all.iter().find(|m| m["id"] == json!(id)) else {
                return Err(Fail::Op(format!(
                    "no module {id:?} that needs approval (shipped modules need none; a switched-off one is not loaded)"
                )));
            };
            // The hash of what is printed here, so a manifest changed in
            // between is not approved unseen.
            call("approve", json!({ "hash": m["hash"] }))?;
            if !args.json {
                out!("approved {id}:");
                for line in describe_request(&m["request"]) {
                    out!("  {line}");
                }
            }
            Ok(())
        }
        other => {
            call(other, json!({}))?;
            if !args.json {
                let id = id.unwrap_or_default();
                out!(
                    "{}",
                    if other == "revoke" {
                        format!("{id} now waits for approval")
                    } else {
                        format!("switched {id} off")
                    }
                );
            }
            Ok(())
        }
    }
}

/// What a module asks for (a `TrustRequest` as JSON), in plain words.
fn describe_request(r: &serde_json::Value) -> Vec<String> {
    let list = |key: &str| -> Vec<String> {
        r[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
    };
    let mut out = Vec::new();
    let program = list("program").join(" ");
    out.push(match r["reach"].as_str() {
        Some("native") => format!("loads a native library into Glossa ({program}): full trust, it runs inside Glossa"),
        Some("program") => {
            format!("runs a program on your computer with your rights, so it can read and change your files: {program}")
        }
        _ => "runs sandboxed: no access to your files or programs".to_owned(),
    });
    let hosts = list("network");
    out.push(match hosts.as_slice() {
        [] => "no network access".to_owned(),
        h if h.iter().any(|x| x == "*") => "can reach any website".to_owned(),
        h => format!("can reach: {}", h.join(", ")),
    });
    for a in r["assets"].as_array().into_iter().flatten() {
        let hosts: Vec<&str> = a["hosts"].as_array().into_iter().flatten().filter_map(|h| h.as_str()).collect();
        let mb = a["size"].as_u64().unwrap_or(0) as f64 / 1_048_576.0;
        let optional = if a["optional"] == json!(true) { ", when asked for" } else { "" };
        out.push(format!(
            "downloads {}, {mb:.1} MB, from {}{optional}",
            a["name"].as_str().unwrap_or("?"),
            hosts.join(", ")
        ));
    }
    let packages = list("packages");
    let (index, packages): (Vec<String>, Vec<String>) = packages.into_iter().partition(|p| p.starts_with("index "));
    if !packages.is_empty() {
        let from = index.first().map(|i| format!(" from {}", &i["index ".len()..])).unwrap_or_default();
        out.push(format!("installs Python packages{from}: {}", packages.join(", ")));
    }
    let actions = list("actions");
    if !actions.is_empty() {
        out.push(format!("has actions: {}", actions.join(", ")));
    }
    for e in list("extra") {
        out.push(format!("also: {e}"));
    }
    out
}

/// `module panel [ID/PANEL]`: list the panels, show one, run one of its
/// actions, or ask a running Glossa to open it.
fn module_panel(args: &Args) -> Result<(), Fail> {
    let Some(name) = args.rest.get(1) else {
        let e = engine(args)?;
        let list = e.panels();
        if args.json {
            out!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
        } else if list.is_empty() {
            out!("no module declares a panel");
        } else {
            for p in list {
                let from =
                    if p.open_from.is_empty() { String::new() } else { format!("  ({})", p.open_from.join(", ")) };
                out!("{}  {}{from}", p.id, p.title);
            }
        }
        return Ok(());
    };
    let id = glossa_engine::PanelId::parse(name)?;
    if args.flags.iter().any(|(n, _)| n == "--open") {
        let cfg = glossa_core::Config::load(&paths(args).config_file())?;
        let addr = glossa_engine::server::connect_addr(&cfg.server.bind);
        let url =
            format!("http://{addr}/v1/panel?id={}&token={}", glossa_net::url_encode(&id.to_string()), cfg.server.token);
        let resp = glossa_net::Http::send(&glossa_net::HttpClient::plain(), glossa_net::HttpRequest::post(url))?;
        if !resp.is_success() {
            return Err(Fail::Op(format!("Glossa answered {}: {}", resp.status, resp.text())));
        }
        return Ok(());
    }
    let e = engine(args)?;
    let page = e.panel_page(&id)?;
    let Some(action) = args.flag(&["--action"]) else {
        // The panel as written, and its state as it opens.
        let m = e.registry().get(&id.module).and_then(|p| p.manifest().cloned());
        let spec = m.as_ref().and_then(|m| m.ui.panels.iter().find(|p| p.id == id.panel).cloned());
        if args.json {
            out!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({ "title": page.window_title, "panel": spec, "state": page.defaults })
                )
                .unwrap_or_default()
            );
            return Ok(());
        }
        out!("{}", page.window_title);
        fn walk(els: &[glossa_modules::panels::Element], depth: usize) {
            for el in els {
                let bind = match &el.bind {
                    glossa_modules::panels::Bind::State(k) => format!("  state.{k}"),
                    glossa_modules::panels::Bind::Options(k) => format!("  options.{k}"),
                    glossa_modules::panels::Bind::None => String::new(),
                };
                let what = if el.action.is_empty() { el.type_name.clone() } else { format!("button @{}", el.action) };
                out!("{}{}  {what}{bind}", "  ".repeat(depth + 1), el.id);
                walk(&el.children, depth + 1);
            }
        }
        walk(&page.elements, 0);
        for (k, v) in &page.defaults {
            out!("state {k} = {v}");
        }
        return Ok(());
    };
    let mut state = page.defaults.clone();
    for (name, value) in &args.flags {
        if name == "--state" {
            let pair = value.as_deref().unwrap_or_default();
            let (k, mut v) = key_value(name, pair)?;
            // Lists and objects too: a panel's state holds them.
            if let Some(text) = v.as_str().filter(|t| t.starts_with('[') || t.starts_with('{')) {
                if let Ok(j) = serde_json::from_str(text) {
                    v = j;
                }
            }
            state.insert(k, v);
        }
    }
    let item = match args.flag(&["--item"]) {
        None => None,
        Some(t) => {
            Some(serde_json::from_str::<serde_json::Value>(t).map_err(|e| Fail::Usage(format!("--item {t:?}: {e}")))?)
        }
    };
    let req = glossa_engine::PanelRequest {
        state: state.clone(),
        event: glossa_modules::wire::ActionEvent { kind: "click".into(), element: Some(action.to_owned()), item },
    };
    let json = args.json;
    let printed = |u: glossa_modules::OptionValues| {
        if !json {
            for (k, v) in u {
                eprintln!("update {k} = {v}");
            }
        }
    };
    let out = e.panel_action(&id, action, req, &glossa_modules::CancelToken::new(), &printed)?;
    state.extend(out.reply.state.clone());
    let effects: Vec<serde_json::Value> = out
        .effects
        .iter()
        .map(|f| match f {
            glossa_engine::Effect::Copy(t) => json!({ "copy": t }),
            glossa_engine::Effect::OpenUrl(u) => json!({ "open_url": u }),
            glossa_engine::Effect::Play(a) => json!({ "play": a.mime, "bytes": a.bytes.len() }),
        })
        .collect();
    if json {
        out!(
            "{}",
            serde_json::to_string_pretty(&json!({ "reply": out.reply, "state": state, "effects": effects }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    if let Some(n) = &out.reply.notice {
        out!("{n}");
    }
    for (k, msg) in &out.reply.errors {
        out!("{k}: {msg}");
    }
    for f in &effects {
        out!("would {f}");
    }
    for (k, v) in &state {
        out!("state {k} = {v}");
    }
    Ok(())
}

/// `module test` / `module check`: the module test harness
/// (`glossa-module-test`). Needs no data folder, so it runs the same on a
/// developer's machine and in CI.
fn module_command(args: &Args) -> Result<(), Fail> {
    use glossa_module_test::{Network, Settings, Status};
    let sub = args.rest.first().map(String::as_str).unwrap_or("");
    let check_only = match sub {
        "test" => false,
        "check" => true,
        "action" => return module_action(args),
        "pending" | "approve" | "revoke" | "decline" => return module_trust(args, sub),
        "panel" | "panels" => return module_panel(args),
        "" => {
            return Err(Fail::Usage(
                "module test DIR…, module check DIR…, module action ID ACTION, module panel [ID/PANEL], \
                 module pending, module approve ID, module revoke ID or module decline ID"
                    .into(),
            ))
        }
        other => {
            return Err(Fail::Usage(format!(
                "unknown module command {other:?}; use test, check, action, panel, pending, approve, revoke or decline"
            )))
        }
    };
    let roots = &args.rest[1..];
    if roots.is_empty() {
        return Err(Fail::Usage(format!("module {sub} needs a module folder")));
    }
    let network = match (args.has(&["--live"]), args.has(&["--record"])) {
        (true, true) => return Err(Fail::Usage("--live and --record do not go together".into())),
        (true, false) => Network::Live,
        (false, true) => Network::Record,
        (false, false) => Network::Replay,
    };
    let mut settings = Settings {
        network,
        check_only,
        strict: args.has(&["--strict"]),
        require_runtimes: args.has(&["--require-runtimes"]),
        case_filter: args.flag(&["--case"]).map(str::to_owned),
        ignore_recordings: args.has(&["--ignore-recordings"]),
        models_dir: Some(paths(args).models_dir()),
        download_assets: args.has(&["--download"]),
        python_dirs: Some(glossa_engine::python_dirs(&paths(args))),
        ..Settings::default()
    };
    if let Some(t) = args.flag(&["--timeout"]) {
        let secs: u64 = t.parse().map_err(|_| Fail::Usage(format!("--timeout {t:?}: seconds, e.g. 30")))?;
        settings.timeout = std::time::Duration::from_secs(secs.max(1));
    }
    for (name, value) in &args.flags {
        let pair = value.as_deref().unwrap_or_default();
        let (k, v) = match name.as_str() {
            "--set" => {
                let (k, v) =
                    pair.split_once('=').ok_or_else(|| Fail::Usage(format!("--set {pair:?}: use KEY=VALUE")))?;
                (k, v.to_owned())
            }
            // A key from the environment stays out of the shell's history.
            "--set-from-env" => {
                let (k, var) = pair
                    .split_once('=')
                    .ok_or_else(|| Fail::Usage(format!("--set-from-env {pair:?}: use KEY=VARIABLE")))?;
                let v = std::env::var(var.trim())
                    .map_err(|_| Fail::Usage(format!("--set-from-env: the variable {var} is not set")))?;
                (k, v)
            }
            _ => continue,
        };
        // A number or a boolean is passed as one, anything else as text.
        let value = serde_json::from_str::<serde_json::Value>(&v)
            .ok()
            .filter(|j| j.is_number() || j.is_boolean())
            .unwrap_or_else(|| json!(v));
        settings.options.insert(k.trim().to_owned(), value);
    }

    let mut dirs = Vec::new();
    for root in roots {
        let found = glossa_module_test::find_modules(std::path::Path::new(root));
        if found.is_empty() {
            return Err(Fail::Op(format!("no module.toml in {root} or below it")));
        }
        dirs.extend(found);
    }
    let reports: Vec<_> = dirs.iter().map(|d| glossa_module_test::test_module(d, &settings)).collect();
    let count = |s: Status| reports.iter().filter(|r| r.status == s).count();
    let (pass, fail, skip) = (count(Status::Pass), count(Status::Fail), count(Status::Skip));
    if args.json {
        out!("{}", json!({ "modules": reports, "passed": pass, "failed": fail, "skipped": skip }));
    } else {
        for r in &reports {
            outp!("{}", r.render());
        }
        if reports.len() > 1 {
            out!("\n{} modules: {pass} passed, {fail} failed, {skip} skipped", reports.len());
        }
    }
    if fail > 0 {
        // The report already says what failed; exit 1 without repeating it.
        std::process::exit(1);
    }
    Ok(())
}

fn run(args: &Args) -> Result<(), Fail> {
    match args.command.as_str() {
        "" | "help" => {
            outp!("{USAGE}");
            Ok(())
        }
        "version" => {
            if args.json {
                out!(
                    "{}",
                    json!({ "name": "glossa-cli", "version": glossa_core::VERSION, "commit": glossa_core::COMMIT })
                );
            } else {
                out!("glossa-cli {}", glossa_core::long_version());
            }
            Ok(())
        }
        "translate" | "tr" => {
            let e = engine(args)?;
            let pair = LangPair::new(
                lang(args.flag(&["-f", "--from"]), LangId::AUTO)?,
                lang(args.flag(&["-t", "--to"]), LangId::AUTO)?,
            );
            // On a terminal, a streaming service's answer is printed as it
            // is written; piped output gets the finished text only.
            use std::io::IsTerminal as _;
            let printed = Arc::new(Mutex::new(String::new()));
            let progress = (!args.json && std::io::stdout().is_terminal()).then(|| {
                let printed = printed.clone();
                glossa_engine::Progress::new(move |text| {
                    let mut done = printed.lock().unwrap_or_else(|e| e.into_inner());
                    match text.strip_prefix(done.as_str()) {
                        Some(new) => outp!("{new}"),
                        // Another service took over after a failure: start again.
                        None => outp!("\n{text}"),
                    }
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    *done = text.to_owned();
                })
            });
            let provider = service(&e, args, &["-s", "--service"], Capabilities::TRANSLATE)?;
            let opts =
                TranslateOptions { provider, back_translate: args.has(&["--back"]), progress, ..Default::default() };
            let out = e.translate(&text(args)?, pair, &opts)?;
            if args.has(&["--save-history"]) && e.record_translation(&out) {
                if let Err(err) = e.save_history() {
                    eprintln!("glossa-cli: history not saved: {err}");
                }
            }
            let streamed = std::mem::take(&mut *printed.lock().unwrap_or_else(|e| e.into_inner()));
            if args.json {
                out!("{}", serde_json::to_string(&out.report()).unwrap_or_default());
            } else {
                // Piped, stdout is the translation and nothing else, so
                // `< in.txt > out.txt` works; on a terminal (or with
                // --full) the romanisation and dictionary senses follow.
                let rich = std::io::stdout().is_terminal() || args.has(&["--full"]);
                let block = if rich { out.translation.as_display_block() } else { out.translation.text.clone() };
                match block.strip_prefix(streamed.as_str()) {
                    Some(rest) if !streamed.is_empty() => out!("{rest}"),
                    _ if !streamed.is_empty() => out!("\n{block}"),
                    _ => out!("{block}"),
                }
                if let Some(back) = out.back {
                    eprintln!("\n↩ {}", back.text);
                }
                for (id, why) in &out.failed {
                    eprintln!("[{id} did not answer: {why}]");
                }
                if out.truncated {
                    eprintln!(
                        "[the text is too long for {}, even in pieces: its end was NOT translated]",
                        out.translation.provider_id
                    );
                }
                eprintln!(
                    "[{} · {} → {} · {} ms]",
                    out.translation.provider_id,
                    out.pair.from.english_name(),
                    out.pair.to.english_name(),
                    out.translation.elapsed_ms
                );
            }
            Ok(())
        }
        "detect" => {
            let e = engine(args)?;
            let d = e.detect(&text(args)?)?;
            if args.json {
                out!(
                    "{}",
                    json!({ "lang": d.lang.bcp47(), "name": d.lang.english_name(), "confidence": d.confidence })
                );
            } else {
                out!("{} ({})", d.lang.english_name(), d.lang.bcp47());
            }
            Ok(())
        }
        "dict" | "dictionary" => {
            let e = engine(args)?;
            let pair = LangPair::new(
                lang(args.flag(&["-f", "--from"]), LangId::AUTO)?,
                lang(args.flag(&["-t", "--to"]), LangId::AUTO)?,
            );
            let provider = service(&e, args, &["-s", "--service"], Capabilities::DICTIONARY)?;
            let a = e.dictionary(&text(args)?, pair, provider.as_deref())?;
            if args.json {
                out!("{}", serde_json::to_string_pretty(&a).unwrap_or_default());
            } else if !a.explanations.is_empty() || !a.examples.is_empty() {
                out!("{}", a.to_plain_text());
            } else if let Some(html) = &a.html {
                out!("{}", glossa_dict::to_plain_text(html));
            }
            if !args.json {
                if let Some(link) = &a.link {
                    eprintln!("{link}");
                }
                eprintln!("[{} · {} ms]", a.provider_id, a.elapsed_ms);
            }
            Ok(())
        }
        "ocr" => {
            let path = args.rest.first().ok_or_else(|| Fail::Usage("ocr needs an image file".into()))?;
            let image = std::fs::read(path)?;
            let e = engine(args)?;
            let to = args.flag(&["-t", "--to"]).map(|c| lang(Some(c), LangId::ENGLISH)).transpose()?;
            let from = lang(args.flag(&["-l", "--lang"]), LangId::AUTO)?;
            let engine_id = service(&e, args, &["-e", "--engine", "-s", "--service"], Capabilities::OCR)?;
            let engine_id = engine_id.as_deref();
            if let Some(name) = args.flag(&["-F", "--format"]) {
                use glossa_core::ocr::export::{self, OcrFormat};
                use glossa_core::ocr::OcrGranularity;
                let format = OcrFormat::from_name(name)
                    .ok_or_else(|| Fail::Usage(format!("unknown format {name:?}: text, json, hocr or alto")))?;
                let granularity =
                    if format.wants_structure() { OcrGranularity::Words } else { OcrGranularity::FullText };
                let out = e.recognize_detailed(image, from, to, engine_id, granularity)?;
                let mut ocr = out.ocr.clone();
                if let Some(t) = out.translated_text() {
                    ocr.translation = Some(t.to_owned());
                }
                match format {
                    OcrFormat::Text => out!("{}", ocr.translation.as_deref().unwrap_or(&export::text(&ocr))),
                    other => outp!("{}", export::render(&ocr, other)),
                }
                eprintln!("[{} · {} ms]", ocr.provider_id, ocr.elapsed_ms);
                return Ok(());
            }
            let in_place = args.has(&["--in-place"]);
            let join = args.has(&["--join-lines"]);
            let translate_with = match args.flag(&["--translate-with"]) {
                None => None,
                Some("ocr" | "engine" | "ocr_engine") => Some(glossa_core::config::OcrTranslateWith::OcrEngine),
                Some("translator") => Some(glossa_core::config::OcrTranslateWith::Translator),
                Some(other) => {
                    return Err(Fail::Usage(format!("--translate-with takes ocr or translator, not {other}")));
                }
            };
            let opts = glossa_engine::RecognizeOptions {
                lang: from,
                translate_to: if in_place { Some(to.unwrap_or(e.config().general.first_language)) } else { to },
                provider: engine_id.map(str::to_owned),
                in_place,
                layout: join,
                translate_with,
                ..Default::default()
            };
            let out = e.recognize_with_options(image, &opts)?;
            let text = glossa_engine::ocr::copy_text(&out.ocr, join);
            if args.json {
                let mut summary = glossa_engine::server::ocr_summary(&out);
                summary["text"] = text.into();
                out!("{summary}");
            } else if !out.blocks.is_empty() {
                // One line per block: what was read there, a tab, its translation.
                for b in &out.blocks {
                    out!("{}\t{}", b.source.replace('\n', " "), b.translation.replace('\n', " "));
                }
                eprintln!("[{} · {} ms]", out.ocr.provider_id, out.ocr.elapsed_ms);
            } else {
                out!("{}", out.translated_text().unwrap_or(&text));
                eprintln!("[{} · {} ms]", out.ocr.provider_id, out.ocr.elapsed_ms);
            }
            Ok(())
        }
        "speak" => {
            let out_path = args.flag(&["-o", "--out"]).ok_or_else(|| Fail::Usage("speak needs -o OUT.mp3".into()))?;
            let e = engine(args)?;
            let audio =
                e.speak(&text(args)?, lang(args.flag(&["-l", "--lang"]), LangId::AUTO)?, Some(args.has(&["--slow"])))?;
            std::fs::write(out_path, &audio.bytes)?;
            eprintln!("{} bytes of {} written to {out_path}", audio.bytes.len(), audio.mime);
            Ok(())
        }
        "services" => {
            let e = engine(args)?;
            let cap = match args.flag(&["-c", "--capability"]) {
                None => Capabilities::TRANSLATE,
                Some(c) => {
                    Capabilities::from_name(c).ok_or_else(|| Fail::Usage(format!("unknown capability {c:?}")))?
                }
            };
            let list = e.providers(cap);
            if args.json {
                let v: Vec<_> = list
                    .iter()
                    .map(|p| json!({ "id": p.info.id, "name": p.info.name, "origin": p.origin, "enabled": p.enabled,
                                     "capabilities": p.info.capabilities.names(), "needs_credentials": p.info.needs_credentials }))
                    .collect();
                out!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                for p in list {
                    let mark = if p.enabled { '*' } else { ' ' };
                    let key = if p.info.needs_credentials { " (needs a key)" } else { "" };
                    out!("{mark} {:28} {:32} [{}]{key}", p.info.id, p.info.name, p.origin);
                }
            }
            Ok(())
        }
        "modules" | "module" if args.rest.first().is_some_and(|w| w == "assets") => assets(args, &args.rest[1..]),
        "assets" => assets(args, &args.rest),
        "python" => python(args, &args.rest),
        "preset" | "presets" => preset(args, &args.rest),
        "modules" => {
            let e = engine(args)?;
            let s = e.module_status();
            if args.json {
                out!(
                    "{}",
                    json!({
                        "loaded": s.loaded,
                        "failed": s.failed.iter().map(|(p, w)| json!({"path": p, "error": w})).collect::<Vec<_>>(),
                        "skipped": s.skipped.iter().map(|(p, w)| json!({"path": p, "reason": w})).collect::<Vec<_>>(),
                    })
                );
            } else {
                out!("loaded ({}):", s.loaded.len());
                let reg = e.registry();
                for id in &s.loaded {
                    match reg.entry(id).map(|entry| &entry.origin) {
                        Some(glossa_modules::Origin::Module { runtime, .. }) => {
                            out!("  {id:32} [{}]", runtime.label())
                        }
                        _ => out!("  {id}"),
                    }
                }
                if !s.failed.is_empty() {
                    out!("failed ({}):", s.failed.len());
                    for (p, w) in &s.failed {
                        out!("  {}: {w}", p.display());
                    }
                }
                if !s.skipped.is_empty() {
                    out!("skipped ({}):", s.skipped.len());
                    for (p, w) in &s.skipped {
                        out!("  {}: {w}", p.display());
                    }
                }
            }
            Ok(())
        }
        "history" => {
            let e = engine(args)?;
            let favorites = args.has(&["--favorites"]);
            if let Some(path) = args.flag(&["--export"]) {
                let ext = std::path::Path::new(path).extension().and_then(|x| x.to_str()).unwrap_or("txt");
                let format = ExportFormat::from_extension(ext)
                    .ok_or_else(|| Fail::Usage(format!("unknown export format .{ext}")))?;
                let n = e.export_history(std::path::Path::new(path), format, favorites)?;
                eprintln!("{n} entries written to {path}");
                return Ok(());
            }
            let entries = e.search_history(args.flag(&["--search"]).unwrap_or(""), favorites);
            if args.json {
                out!("{}", serde_json::to_string_pretty(&entries).unwrap_or_default());
            } else {
                for h in entries.iter().take(50) {
                    let star = if h.favorite { "★ " } else { "" };
                    let t = h.latest().map(|r| r.text.as_str()).unwrap_or("");
                    out!("{star}{} → {}", one_line(&h.source), one_line(t));
                }
            }
            Ok(())
        }
        "import-classic" => {
            let dir = PathBuf::from(
                args.rest.first().ok_or_else(|| Fail::Usage("import-classic needs the classic folder".into()))?,
            );
            if !glossa_core::config::ClassicInstall::looks_like_install(&dir) {
                return Err(Fail::Op(format!(
                    "{} does not look like a classic installation (no Services/Data)",
                    dir.display()
                )));
            }
            let e = engine(args)?;
            let r = e.import_classic(&dir)?;
            let status = e.module_status();
            out!(
                "settings: {}\nhistory entries: {}\nproviders now: {} (modules loaded: {}, failed: {})",
                if r.options_imported { "imported" } else { "not found" },
                r.history_entries,
                e.registry().entries().len(),
                status.loaded.len(),
                status.failed.len()
            );
            for n in r.notes {
                out!("note: {n}");
            }
            e.shutdown();
            Ok(())
        }
        "module" => module_command(args),
        "theme" => {
            let sub = args.rest.first().map(String::as_str).unwrap_or("list");
            match sub {
                "list" => {
                    let e = engine(args)?;
                    for t in e.theme_entries() {
                        let origin = match (&t.path, t.classic) {
                            (None, _) => "built-in".to_owned(),
                            (Some(p), true) => format!("classic: {}", p.display()),
                            (Some(p), false) => p.display().to_string(),
                        };
                        out!("{:24} {origin}", t.name);
                    }
                    Ok(())
                }
                "show" => {
                    let e = engine(args)?;
                    let name = args.rest.get(1).ok_or_else(|| Fail::Usage("theme show NAME".into()))?;
                    outp!("{}", e.theme(name).to_toml());
                    Ok(())
                }
                "convert" => {
                    let file = args.rest.get(1).ok_or_else(|| Fail::Usage("theme convert FILE".into()))?;
                    let text = std::fs::read_to_string(file)?;
                    let theme =
                        glossa_core::Theme::load(&text, file, &|_| None).map_err(|e| Fail::Op(e.to_string()))?;
                    outp!("{}", theme.to_toml());
                    Ok(())
                }
                other => Err(Fail::Usage(format!("unknown theme command {other:?}"))),
            }
        }
        "locale" => {
            let sub = args.rest.first().map(String::as_str).unwrap_or("list");
            match sub {
                "list" => {
                    let e = engine(args)?;
                    for l in e.locales() {
                        out!("{:8} {:24} {}", l.code, l.name, l.native_name);
                    }
                    Ok(())
                }
                "convert" => {
                    let file = args.rest.get(1).ok_or_else(|| Fail::Usage("locale convert FILE".into()))?;
                    let locale = glossa_core::Locale::parse(&std::fs::read_to_string(file)?).map_err(Fail::Op)?;
                    let (have, total) = locale.coverage();
                    eprintln!("{} ({}): {have} of {total} strings", locale.name, locale.code);
                    out!("{}", locale.to_native_json());
                    Ok(())
                }
                "check" => {
                    let file = args.rest.get(1).ok_or_else(|| Fail::Usage("locale check FILE".into()))?;
                    let mut locale = glossa_core::Locale::parse(&std::fs::read_to_string(file)?)
                        .map_err(|e| Fail::Op(format!("{file}: {e}")))?;
                    let issues = locale.validate();
                    let (have, total) = locale.coverage();
                    let missing = locale.untranslated_keys();
                    let en = glossa_core::Locale::english();
                    let errors = issues.iter().filter(|i| i.severity() == Severity::Error).count();
                    if args.json {
                        let list: Vec<serde_json::Value> = issues
                            .iter()
                            .map(|i| {
                                json!({
                                    "key": i.key,
                                    "severity": if i.severity() == Severity::Error { "error" } else { "warning" },
                                    "message": i.message(&en),
                                })
                            })
                            .collect();
                        out!(
                            "{}",
                            json!({
                                "code": locale.code, "name": locale.name, "native_name": locale.native_name,
                                "translated": have, "total": total, "missing": missing, "issues": list,
                            })
                        );
                    } else {
                        out!("{} ({}): {have} of {total} strings", locale.native_name, locale.code);
                        for i in &issues {
                            let level = if i.severity() == Severity::Error { "error" } else { "warning" };
                            out!("{level}: {}", i.message(&en));
                        }
                        if args.verbose {
                            for k in &missing {
                                out!("missing: {k}");
                            }
                        } else if !missing.is_empty() {
                            out!("{} strings fall back to English (-v lists them)", missing.len());
                        }
                    }
                    if errors > 0 {
                        if args.json {
                            // The report above is the output; the exit code says it failed.
                            std::process::exit(1);
                        }
                        return Err(Fail::Op(format!("{errors} strings skipped")));
                    }
                    Ok(())
                }
                "template" => {
                    let base = match args.rest.get(1) {
                        Some(lang) => {
                            let e = engine(args)?;
                            let Some(entry) = e
                                .locales()
                                .into_iter()
                                .find(|l| l.code.eq_ignore_ascii_case(lang) || l.name.eq_ignore_ascii_case(lang))
                            else {
                                return Err(Fail::Usage(format!("no interface language {lang:?} (see locale list)")));
                            };
                            Some(e.load_locale(&entry.code))
                        }
                        None => None,
                    };
                    outp!("{}", glossa_core::Locale::template(base.as_ref()));
                    Ok(())
                }
                other => Err(Fail::Usage(format!("unknown locale command {other:?}"))),
            }
        }
        "config" => {
            let p = paths(args);
            match args.rest.first().map(String::as_str).unwrap_or("path") {
                "path" => {
                    out!("{}", p.config_file().display());
                    eprintln!("data: {}  (portable: {})", p.data_dir.display(), p.portable);
                }
                "show" => outp!("{}", glossa_core::Config::load(&p.config_file())?.to_toml()),
                other => return Err(Fail::Usage(format!("unknown config command {other:?}"))),
            }
            Ok(())
        }
        "serve" => {
            let e = engine(args)?;
            if !e.config().server.enabled {
                // The command is an explicit request to run the API, so the
                // setting (which governs the desktop program) does not stop it.
                eprintln!(
                    "note: [server] enabled is false; that switches the API off in the desktop program, \
                     serve runs it anyway"
                );
            }
            let handle = glossa_engine::server::start(e.clone())?;
            let token = e.config().server.token;
            eprintln!("control API on http://{}  (token: {token})", handle.addr);
            eprintln!("try: curl -s 'http://{}/v1/translate?text=hello&to=ru&token={token}'", handle.addr);
            // Linux and the BSDs: the D-Bus service too, unless switched off.
            let _dbus = if glossa_engine::dbus::SUPPORTED && e.config().server.dbus {
                match glossa_engine::dbus::start(e.clone()) {
                    Ok(h) => {
                        use glossa_engine::dbus::{BUS_NAME, INTERFACE, OBJECT_PATH};
                        eprintln!("D-Bus: {BUS_NAME} on the session bus");
                        eprintln!(
                            "try: gdbus call --session --dest {BUS_NAME} --object-path {OBJECT_PATH} \
                             --method {INTERFACE}.Translate hello '' ru ''"
                        );
                        Some(h)
                    }
                    Err(err) => {
                        eprintln!("note: no D-Bus service: {err}");
                        None
                    }
                }
            } else {
                None
            };
            let rx = e.subscribe();
            loop {
                match rx.recv() {
                    Ok(glossa_engine::Event::Action { action, text }) => {
                        eprintln!(
                            "action requested: {}{}",
                            action.id(),
                            text.map(|t| format!(" ({t})")).unwrap_or_default()
                        );
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            Ok(())
        }
        "action" => {
            let id = args.rest.first().ok_or_else(|| Fail::Usage("action needs an action id".into()))?;
            let cfg = glossa_core::Config::load(&paths(args).config_file())?;
            let addr = glossa_engine::server::connect_addr(&cfg.server.bind);
            let mut url = format!("http://{addr}/v1/action/{id}?token={}", cfg.server.token);
            if let Some(t) = args.rest.get(1..).filter(|r| !r.is_empty()) {
                url.push_str("&text=");
                url.push_str(&glossa_net::url_encode(&t.join(" ")));
            }
            let http = glossa_net::HttpClient::plain();
            let resp = glossa_net::Http::send(&http, glossa_net::HttpRequest::get(url))?;
            if !resp.is_success() {
                return Err(Fail::Op(format!("Glossa answered {}: {}", resp.status, resp.text())));
            }
            Ok(())
        }
        "doctor" => {
            let report = doctor::report(&paths(args));
            if args.json {
                out!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            } else {
                outp!("{}", doctor::print(&report));
            }
            Ok(())
        }
        other => Err(Fail::Usage(format!("unknown command {other:?}"))),
    }
}

fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 60 {
        flat.chars().take(59).chain(std::iter::once('…')).collect()
    } else {
        flat
    }
}

/// `glossa-cli assets …`: the files modules declare.
fn assets(args: &Args, words: &[String]) -> Result<(), Fail> {
    let e = engine(args)?;
    let op = words.first().map(String::as_str).unwrap_or("list");
    let id = || words.get(1).map(String::as_str).ok_or_else(|| Fail::Usage(format!("assets {op} needs a module id")));
    let print = |r: &glossa_engine::AssetReport| {
        if args.json {
            out!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
            return;
        }
        out!("{} — {} ({})", r.module, if r.ready { "ready" } else { "needs download" }, r.dir.display());
        for a in &r.assets {
            let state = match &a.state {
                glossa_engine::AssetState::Ready => "ready".to_owned(),
                glossa_engine::AssetState::Missing => "missing".to_owned(),
                glossa_engine::AssetState::Partial { have } => format!("partial, {have} of {} bytes", a.size),
                glossa_engine::AssetState::Outdated => "out of date".to_owned(),
                glossa_engine::AssetState::Corrupt => "damaged, deleted".to_owned(),
            };
            let optional = if a.optional { ", optional" } else { "" };
            out!("  {:24} {:>12} bytes  {state}{optional}", a.name, a.size);
        }
    };
    match op {
        "list" => {
            let all = e.modules_with_assets();
            if args.json {
                out!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
            } else if all.is_empty() {
                out!("no module declares files to download");
            }
            if !args.json {
                all.iter().for_each(print);
            }
            Ok(())
        }
        "status" => {
            print(&e.module_assets(id()?).map_err(Fail::from)?);
            Ok(())
        }
        "download" => {
            let id = id()?;
            let cancel = glossa_modules::CancelToken::new();
            let last = Mutex::new((String::new(), 0u64));
            let quiet = args.json || !std::io::stderr().is_terminal();
            let r = e
                .download_module_assets(id, &words[2..], &cancel, &|p| {
                    if quiet {
                        return;
                    }
                    // One line per asset, rewritten in place at each whole percent.
                    let pct = (p.done * 100).checked_div(p.total).unwrap_or(0);
                    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                    if last.0 != p.asset || last.1 != pct {
                        *last = (p.asset.clone(), pct);
                        eprint!("\r{}: {pct}% ({} of {} bytes)   ", p.asset, p.done, p.total);
                    }
                })
                .map_err(Fail::from);
            if !quiet {
                eprintln!();
            }
            print(&r?);
            Ok(())
        }
        "verify" => {
            let r = e.verify_module_assets(id()?).map_err(Fail::from)?;
            print(&r);
            if r.assets.iter().all(|a| a.state == glossa_engine::AssetState::Ready) {
                Ok(())
            } else {
                Err(Fail::Op("some files are missing or damaged; `glossa-cli assets download` fetches them".into()))
            }
        }
        "remove" => {
            let id = id()?;
            e.remove_module_assets(id).map_err(Fail::from)?;
            if !args.json {
                out!("deleted the files of {id}");
            }
            Ok(())
        }
        other => Err(Fail::Usage(format!("unknown assets command {other:?} (list, status, download, verify, remove)"))),
    }
}

/// Progress on stderr, one line rewritten in place at each whole percent.
fn progress_line(quiet: bool) -> impl Fn(&glossa_engine::AssetProgress) {
    let last = Mutex::new((String::new(), u64::MAX));
    move |p| {
        if quiet {
            return;
        }
        let pct = (p.done * 100).checked_div(p.total).unwrap_or(0);
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if last.0 != p.asset || last.1 != pct {
            *last = (p.asset.clone(), pct);
            if p.total == 0 {
                eprint!("\r{}…                              ", p.asset);
            } else {
                eprint!("\r{}: {pct}% ({} of {} bytes)   ", p.asset, p.done, p.total);
            }
        }
    }
}

/// `glossa-cli python …`: the Python modules run with.
fn python(args: &Args, words: &[String]) -> Result<(), Fail> {
    let e = engine(args)?;
    let op = words.first().map(String::as_str).unwrap_or("status");
    let module = words.get(1).map(String::as_str);
    let print = |r: &glossa_engine::PythonReport, system: Option<&std::path::Path>| {
        if args.json {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            v["system_python"] = json!(system.map(|p| p.display().to_string()));
            out!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            return;
        }
        out!("setting        [modules] python = {:?}", r.setting);
        match system {
            Some(p) => out!("system Python  {}", p.display()),
            None => out!("system Python  none found (3.8 or newer)"),
        }
        let state = |s: &Option<glossa_engine::AssetStatus>| match s.as_ref().map(|s| &s.state) {
            Some(glossa_engine::AssetState::Ready) => "ready".to_owned(),
            Some(glossa_engine::AssetState::Partial { have }) => format!("partly downloaded ({have} bytes)"),
            Some(_) => format!("not downloaded ({} bytes)", s.as_ref().map_or(0, |s| s.size)),
            None => "not available for this system".to_owned(),
        };
        out!("managed Python {} — {}", r.python_version, state(&r.python));
        if let Some(exe) = &r.python_exe {
            out!("               {}", exe.display());
        }
        out!("uv             {} — {}", r.uv_version, state(&r.uv));
        out!("environments   {}", r.venvs_dir.display());
        if r.modules.is_empty() {
            out!("no Python modules are loaded");
        }
        for m in &r.modules {
            let what = m.requirements.as_deref().map(|q| format!(", needs {q}")).unwrap_or_default();
            out!("  {:24} {}{what}", m.module, m.state);
            for line in &m.missing {
                out!("  {:24}   - {line}", "");
            }
        }
    };
    match op {
        "status" => {
            let system = e.system_python();
            print(&e.python_status(), system.as_deref());
            Ok(())
        }
        "install" => {
            let quiet = args.json || !std::io::stderr().is_terminal();
            let r = e
                .install_python(module, &glossa_modules::CancelToken::new(), &progress_line(quiet))
                .map_err(Fail::from);
            if !quiet {
                eprintln!();
            }
            let system = e.system_python();
            print(&r?, system.as_deref());
            Ok(())
        }
        "remove" => {
            match module {
                Some(id) => e.remove_module_python(id).map_err(Fail::from)?,
                None => e.remove_python().map_err(Fail::from)?,
            }
            if !args.json {
                match module {
                    Some(id) => out!("deleted the Python environment of {id}"),
                    None => out!("deleted the managed Python, uv and every module's environment"),
                }
            }
            Ok(())
        }
        other => Err(Fail::Usage(format!("unknown python command {other:?} (status, install, remove)"))),
    }
}

/// A running Glossa's control API, when one answers: its address and token.
fn running_glossa(args: &Args) -> Option<(String, String)> {
    let cfg = glossa_core::Config::load(&paths(args).config_file()).ok()?;
    if !cfg.server.enabled || cfg.server.token.is_empty() {
        return None;
    }
    let addr = glossa_engine::server::connect_addr(&cfg.server.bind);
    let probe = glossa_net::HttpRequest::get(format!("http://{addr}/health")).timeout(Duration::from_secs(2));
    let resp = glossa_net::Http::send(&glossa_net::HttpClient::plain(), probe).ok()?;
    (resp.is_success() && resp.text().contains("\"glossa\"")).then_some((addr, cfg.server.token))
}

/// A Glossa holding this data folder that could not be reached through
/// its control API keeps its settings in memory and writes them back on its
/// next save, undoing a change made to the file now. Refuse, unless
/// `--force`; with it, warn.
fn refuse_behind_a_running_glossa(args: &Args, local: bool) -> Result<(), Fail> {
    let paths = paths(args);
    if !glossa_platform::instance::is_running(&paths) {
        return Ok(());
    }
    let why = if local { "--local was given" } else { "its control API is off or does not answer" };
    let problem = format!(
        "Glossa is running with this data folder ({}), and {why}: it would write its own settings over a change \
         made to config.toml now",
        paths.config_file().display()
    );
    if args.has(&["--force"]) {
        eprintln!("glossa-cli: warning: {problem}; changing the file anyway (--force)");
        return Ok(());
    }
    Err(Fail::Op(format!(
        "{problem}. Turn the control API on ([server] enabled = true, Options > Control API) so the change goes \
         through it, quit Glossa first, or pass --force to change the file anyway"
    )))
}

/// One call to `/v1/presets…` on a running Glossa; the body of its answer.
fn preset_api(addr: &str, token: &str, post: bool, path: &str, body: Vec<u8>) -> Result<String, Fail> {
    control_api(addr, token, post, &format!("/v1/presets{path}"), body)
}

/// One call to the control API of a running Glossa; the body of its answer.
fn control_api(addr: &str, token: &str, post: bool, path: &str, body: Vec<u8>) -> Result<String, Fail> {
    let url = format!("http://{addr}{path}");
    let req = if post { glossa_net::HttpRequest::post(url).body(body) } else { glossa_net::HttpRequest::get(url) };
    let req = req.header("Authorization", format!("Bearer {token}")).timeout(Duration::from_secs(30));
    let resp = glossa_net::Http::send(&glossa_net::HttpClient::plain(), req)?;
    let text = resp.text();
    if !resp.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or(text);
        return Err(Fail::Op(msg));
    }
    Ok(text)
}

fn parse_answer(text: &str) -> Result<serde_json::Value, Fail> {
    serde_json::from_str(text).map_err(|e| Fail::Op(format!("unexpected answer from Glossa: {e}")))
}

fn parse_report(text: &str) -> Result<glossa_engine::PresetReport, Fail> {
    serde_json::from_str(text).map_err(|e| Fail::Op(format!("unexpected answer from Glossa: {e}")))
}

/// `glossa-cli preset …`: list, apply, save, export, import and manage
/// presets.
fn preset(args: &Args, words: &[String]) -> Result<(), Fail> {
    let op = words.first().map(String::as_str).unwrap_or("list");
    let arg = |i: usize, what: &str| {
        words.get(i).map(String::as_str).ok_or_else(|| Fail::Usage(format!("preset {op} needs {what}")))
    };
    // The running program keeps its settings in memory and would write over
    // a change made to the file behind its back, so changes go through it.
    let local = args.has(&["--local"]);
    let remote = if local { None } else { running_glossa(args) };
    if remote.is_none() && !matches!(op, "list" | "export") {
        refuse_behind_a_running_glossa(args, local)?;
    }
    let report = |r: &glossa_engine::PresetReport, verb: &str| {
        if args.json {
            out!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
            return;
        }
        out!("{verb} preset {:?}", r.name);
        if !r.missing.is_empty() {
            out!("not installed here, skipped: {}", r.missing.join(", "));
        }
        if !r.dropped_secrets.is_empty() {
            out!("left out (a preset never keeps keys or tokens): {}", r.dropped_secrets.join(", "));
        }
        if !r.dropped_options.is_empty() {
            out!(
                "left out (commands, paths, addresses, and settings no installed module declares): {}",
                r.dropped_options.join(", ")
            );
        }
        if !r.blocked.is_empty() {
            out!("refused (not for a shared preset to set): {}", r.blocked.join("; "));
        }
        if !r.changes.is_empty() {
            out!("{}", if verb == "imported" { "applying it will change:" } else { "changed:" });
            for c in &r.changes {
                let from = if c.from.is_empty() { "(not set)" } else { c.from.as_str() };
                let online = if c.online { "   [online: leaves this computer]" } else { "" };
                out!("    {}: {from} -> {}{online}", c.what, c.to);
            }
        }
    };
    let named = |name: &str, new_name: Option<&str>| {
        let mut v = json!({ "name": name });
        if let Some(n) = new_name {
            v["new_name"] = json!(n);
        }
        serde_json::to_vec(&v).unwrap_or_default()
    };
    match op {
        "list" => {
            let entries: Vec<glossa_engine::PresetEntry> = match &remote {
                Some((addr, token)) => {
                    let v = parse_answer(&preset_api(addr, token, false, "", vec![])?)?;
                    serde_json::from_value(v["presets"].clone()).map_err(|e| Fail::Op(e.to_string()))?
                }
                None => engine(args)?.presets(),
            };
            if args.json {
                out!("{}", serde_json::to_string_pretty(&entries).unwrap_or_default());
                return Ok(());
            }
            for p in &entries {
                let mark = if p.current { "*" } else { " " };
                let kind = if p.builtin { " (built-in)" } else { "" };
                out!("{mark} {}{kind}", p.name);
                let services = if p.enabled.is_empty() { "unchanged".to_owned() } else { p.enabled.join(", ") };
                out!("    services: {services}");
                if !p.missing.is_empty() {
                    out!("    not installed: {}", p.missing.join(", "));
                }
            }
            Ok(())
        }
        "apply" | "next" => {
            let r = match (&remote, op) {
                (Some((addr, token)), "next") => parse_report(&preset_api(addr, token, true, "/next", vec![])?)?,
                (Some((addr, token)), _) => {
                    parse_report(&preset_api(addr, token, true, "/apply", named(arg(1, "a name")?, None))?)?
                }
                (None, "next") => engine(args)?.next_preset()?,
                (None, _) => engine(args)?.apply_preset(arg(1, "a name")?)?,
            };
            report(&r, "applied");
            Ok(())
        }
        "save" => {
            let name = arg(1, "a name")?;
            let languages = args.has(&["--languages"]);
            let r = match &remote {
                Some((addr, token)) => {
                    let body = serde_json::to_vec(&json!({ "name": name, "languages": languages })).unwrap_or_default();
                    parse_report(&preset_api(addr, token, true, "/save", body)?)?
                }
                None => engine(args)?.save_preset(name, languages)?,
            };
            report(&r, "saved");
            Ok(())
        }
        "export" => {
            let name = arg(1, "a name")?;
            let text = match &remote {
                Some((addr, token)) => {
                    let path = format!("/export?name={}", glossa_net::url_encode(name));
                    preset_api(addr, token, false, &path, vec![])?
                }
                None => engine(args)?.export_preset_text(name)?,
            };
            match args.flag(&["-o", "--out"]) {
                Some(file) => {
                    std::fs::write(file, text)?;
                    if !args.json {
                        eprintln!("wrote {file}");
                    }
                }
                None => outp!("{text}"),
            }
            Ok(())
        }
        "import" => {
            let file = arg(1, "a file")?;
            let r = match &remote {
                Some((addr, token)) => parse_report(&preset_api(addr, token, true, "/import", std::fs::read(file)?)?)?,
                None => engine(args)?.import_preset(std::path::Path::new(file))?,
            };
            report(&r, "imported");
            Ok(())
        }
        "rename" | "duplicate" | "delete" => {
            let name = arg(1, "a name")?;
            let new_name = if op == "rename" { Some(arg(2, "a new name")?) } else { words.get(2).map(String::as_str) };
            let result = match &remote {
                Some((addr, token)) => {
                    let v = parse_answer(&preset_api(addr, token, true, &format!("/{op}"), named(name, new_name))?)?;
                    v["name"].as_str().or(v["deleted"].as_str()).unwrap_or(name).to_owned()
                }
                None => {
                    let e = engine(args)?;
                    match (op, new_name) {
                        ("rename", Some(new)) => e.rename_preset(name, new).map(|()| new.trim().to_owned())?,
                        ("duplicate", new) => e.duplicate_preset(name, new)?,
                        _ => e.delete_preset(name).map(|()| name.to_owned())?,
                    }
                }
            };
            if args.json {
                out!("{}", json!({ "name": result }));
            } else {
                match op {
                    "rename" => out!("renamed {name:?} to {result:?}"),
                    "duplicate" => out!("copied {name:?} as {result:?}"),
                    _ => out!("deleted {name:?}"),
                }
            }
            Ok(())
        }
        other => Err(Fail::Usage(format!(
            "unknown preset command {other:?} (list, apply, next, save, export, import, rename, duplicate, delete)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_help_names_the_support_page() {
        assert!(USAGE.contains(glossa_core::SUPPORT_URL));
    }

    fn p(args: &[&str]) -> Args {
        parse(args.iter().map(|s| s.to_string()).collect()).unwrap()
    }

    #[test]
    fn flags_values_and_text_are_separated() {
        let a = p(&["--json", "translate", "-t", "ru", "--from=en", "hello", "world"]);
        assert!(a.json);
        assert_eq!(a.command, "translate");
        assert_eq!(a.flag(&["-t", "--to"]), Some("ru"));
        assert_eq!(a.flag(&["-f", "--from"]), Some("en"));
        assert_eq!(a.rest, ["hello", "world"]);
    }

    #[test]
    fn double_dash_ends_flags() {
        let a = p(&["translate", "--", "-t", "is text"]);
        assert_eq!(a.rest, ["-t", "is text"]);
        assert_eq!(a.flag(&["-t"]), None);
    }

    #[test]
    fn negative_numbers_are_text_not_flags() {
        let a = p(&["translate", "-5", "degrees"]);
        assert_eq!(a.rest, ["-5", "degrees"]);
    }

    #[test]
    fn a_valued_flag_without_a_value_is_an_error() {
        assert!(parse(vec!["translate".into(), "-t".into()]).is_err());
    }

    fn check(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|s| s.to_string()).collect()).and_then(|a| check_flags(&a).map(|()| a))
    }

    #[test]
    fn every_valued_flag_takes_an_equals_sign_home_included() {
        let a = check(&["--home=/tmp/g", "translate", "--to=ru", "-s=google", "hi"]).unwrap();
        assert_eq!(a.home.as_deref(), Some(std::path::Path::new("/tmp/g")));
        assert_eq!(a.flag(&["-t", "--to"]), Some("ru"));
        assert_eq!(a.flag(&["-s", "--service"]), Some("google"));
        let a = check(&["translate", "--home", "/tmp/h", "hi"]).unwrap();
        assert_eq!(a.home.as_deref(), Some(std::path::Path::new("/tmp/h")));
        assert_eq!(a.rest, ["hi"]);
        assert!(check(&["--home=", "translate"]).is_err());
        let a = check(&["--json", "-v", "history", "--export=out.csv"]).unwrap();
        assert!(a.json && a.verbose);
        assert_eq!(a.flag(&["--export"]), Some("out.csv"));
    }

    #[test]
    fn unknown_flags_are_errors_with_a_suggestion() {
        let err = check(&["speak", "--slwo", "-o", "x.mp3", "hi"]).err().unwrap();
        assert!(err.contains("unknown flag --slwo") && err.contains("did you mean --slow"), "{err}");
        let err = check(&["translate", "--form", "en", "hi"]).err().unwrap();
        assert!(err.contains("--from"), "{err}");
        // A real flag on the wrong command says so.
        let err = check(&["detect", "--back", "hi"]).err().unwrap();
        assert!(err.contains("detect does not take --back"), "{err}");
        // Switches take no value.
        assert!(check(&["translate", "--back=yes", "hi"]).is_err());
        assert!(check(&["--json=1", "detect", "hi"]).is_err());
        // Help anywhere still wins, and an unknown command is reported as such.
        assert!(check(&["translate", "--nonsense", "--help"]).is_ok());
        assert!(check(&["frobnicate", "--nonsense"]).is_ok(), "run() reports the unknown command");
        assert!(check(&["--nope"]).is_err());
    }

    #[test]
    fn each_command_accepts_its_own_flags() {
        for argv in [
            &["translate", "-f", "en", "-t", "ru", "-s", "google", "--back", "--save-history", "--full", "hi"][..],
            &["dict", "--from", "en", "--to", "ru", "--service", "wiktionary", "word"],
            &["ocr", "x.png", "-l", "ja", "-t", "en", "-e", "lens", "-F", "hocr"],
            &["speak", "-l", "en", "--slow", "-o", "x.mp3", "hi"],
            &["services", "-c", "ocr"],
            &["history", "--search", "x", "--favorites", "--export", "x.csv"],
            &["--json", "--home", "/tmp/x", "-v", "modules"],
        ] {
            assert!(check(argv).is_ok(), "{argv:?}: {:?}", check(argv).err());
        }
    }

    /// With a Glossa holding the data folder and no control API to reach,
    /// a change to the file would be written over: refused without
    /// `--force`, with or without `--local`. Reading is still allowed.
    #[test]
    fn presets_are_not_changed_behind_a_running_glossa() {
        let home = std::env::temp_dir().join(format!("glossa-cli-presets-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        let h = home.to_string_lossy().into_owned();
        let args = |rest: &[&str]| {
            let mut v = vec!["--home", h.as_str(), "--json", "preset"];
            v.extend_from_slice(rest);
            p(&v)
        };
        let run = |rest: &[&str]| {
            let a = args(rest);
            preset(&a, &a.rest)
        };
        let guard = glossa_platform::instance::acquire(&paths(&args(&[])));
        assert!(matches!(guard, glossa_platform::instance::Instance::Primary(_)));
        let refused = run(&["save", "A"]);
        assert!(matches!(&refused, Err(Fail::Op(m)) if m.contains("--force")), "refused without --force");
        assert!(matches!(run(&["--local", "apply", "Private"]), Err(Fail::Op(_))), "--local too");
        assert!(run(&["list"]).is_ok(), "reading is fine");
        assert!(run(&["--force", "save", "A"]).is_ok());
        drop(guard);
        assert!(run(&["--local", "apply", "A"]).is_ok(), "nobody running");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn modules_are_approved_and_revoked_from_the_command_line() {
        let home = std::env::temp_dir().join(format!("glossa-cli-trust-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let h = home.to_string_lossy().into_owned();
        let args = |rest: &[&str]| {
            let mut v = vec!["--home", h.as_str(), "--json", "module"];
            v.extend_from_slice(rest);
            p(&v)
        };
        let m = paths(&args(&[])).user_module_dir().join("hand-made");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"hand-made\"\nname = \"Hand made\"\nruntime = \"js\"\n\
             [permissions]\nnetwork = [\"api.test\"]\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return req.text; }").unwrap();
        let config = paths(&args(&[])).config_file();
        std::fs::write(&config, "[server]\nenabled = false\n[modules]\ntrust_initialised = true\n").unwrap();
        let granted = || glossa_core::Config::load(&config).unwrap().modules.granted;

        assert!(module_trust(&args(&["pending"]), "pending").is_ok());
        assert!(matches!(module_trust(&args(&["approve"]), "approve"), Err(Fail::Usage(_))), "needs an id");
        assert!(module_trust(&args(&["approve", "nope", "--local"]), "approve").is_err());
        assert!(module_trust(&args(&["approve", "hand-made", "--local"]), "approve").is_ok());
        assert_eq!(granted()["hand-made"].network, ["api.test"]);
        assert!(module_trust(&args(&["revoke", "hand-made", "--local"]), "revoke").is_ok());
        assert!(granted().is_empty());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn long_lines_are_shortened_for_listing() {
        assert_eq!(one_line("a\n  b"), "a b");
        assert_eq!(one_line(&"x".repeat(100)).chars().count(), 60);
    }
}
