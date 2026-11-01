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
    glossa-cli [--json] [--home DIR] [-v] <command> [options] [text…]
    glossa-cli --version              the version and the commit it was built from

COMMANDS
    translate  [-f FROM] [-t TO] [-s SERVICE] [--back] [--save-history] [--full] [TEXT…]
                                      stdin if no text; piped output is the translation alone
                                      unless --full
    detect     [TEXT…]
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
    history    [--search Q] [--favorites] [--export FILE.{txt,csv,html,json}]
    import-classic DIR                settings, history, and use its services/themes/locales
    theme      list | show NAME | convert FILE    (classic JSON -> native TOML)
    locale     list | convert FILE                (classic lang.json -> native JSON)
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
];

/// Flags every command takes.
const GLOBAL: &[&str] = &["--json", "-v", "--verbose", "-h", "--help", "-V", "--version", "--home"];

/// The flags each command takes besides the global ones, or `None` for a
/// command that does not exist (that error is reported instead).
fn command_flags(command: &str) -> Option<&'static [&'static str]> {
    Some(match command {
        "translate" | "tr" => &["-f", "--from", "-t", "--to", "-s", "--service", "--back", "--save-history", "--full"],
        "dict" | "dictionary" => &["-f", "--from", "-t", "--to", "-s", "--service"],
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
        "speak" => &["-o", "--out", "-l", "--lang", "--slow"],
        "services" => &["-c", "--capability"],
        "history" => &["--search", "--favorites", "--export"],
        "detect" | "modules" | "import-classic" | "theme" | "locale" | "config" | "serve" | "action" | "doctor"
        | "" | "help" | "version" => &[],
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

    #[test]
    fn long_lines_are_shortened_for_listing() {
        assert_eq!(one_line("a\n  b"), "a b");
        assert_eq!(one_line(&"x".repeat(100)).chars().count(), 60);
    }
}
