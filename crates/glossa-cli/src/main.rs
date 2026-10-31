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

COMMANDS
    translate  [-f FROM] [-t TO] [-s SERVICE] [--back] [TEXT…]   (stdin if no text)
    detect     [TEXT…]
    dict       [-f FROM] [-t TO] [-s SERVICE] WORD
    ocr        IMAGE [-l LANG] [-t TO]
    speak      [-l LANG] [--slow] -o OUT.mp3 TEXT…
    services   [-c CAPABILITY]        translate | dictionary | listen | ocr | detect_language
    modules                           what loaded, what failed and why
    history    [--search Q] [--favorites] [--export FILE.{txt,csv,html,json}]
    import-classic DIR                settings, history, and use its services/themes/locales
    theme      list | show NAME | convert FILE    (classic JSON -> native TOML)
    locale     list | convert FILE                (classic lang.json -> native JSON)
    config     path | show
    serve                             run the control API until Ctrl+C
    action     ACTION [TEXT]          ask a running Glossa to act (show_popup, text_recognition…)

Languages are codes (en, ru, zh-CN, he, auto). SERVICE is a provider id from `services`.
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
    "-l",
    "--lang",
    "-o",
    "--out",
    "-c",
    "--capability",
    "--search",
    "--export",
    "--home",
];

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
            "--json" => a.json = true,
            "-v" | "--verbose" => a.verbose = true,
            "-h" | "--help" => a.command = "help".into(),
            "--home" => a.home = Some(it.next().ok_or("--home needs a directory")?.into()),
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
                } else {
                    inline
                };
                a.flags.push((name, value));
            }
            _ if a.command.is_empty() => a.command = arg,
            _ => a.rest.push(arg),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1).collect()) {
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
    Ok(Engine::start(paths(args))?)
}

fn lang(code: Option<&str>, default: LangId) -> Result<LangId, Fail> {
    match code {
        None => Ok(default),
        Some(c) => LangId::from_code(c).ok_or_else(|| Fail::Usage(format!("unknown language {c:?}"))),
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
            let opts = TranslateOptions {
                provider: args.flag(&["-s", "--service"]).map(str::to_owned),
                back_translate: args.has(&["--back"]),
                progress,
                ..Default::default()
            };
            let out = e.translate(&text(args)?, pair, &opts)?;
            let streamed = std::mem::take(&mut *printed.lock().unwrap_or_else(|e| e.into_inner()));
            if args.json {
                out!(
                    "{}",
                    json!({
                        "text": out.translation.text,
                        "from": out.pair.from.bcp47(),
                        "to": out.pair.to.bcp47(),
                        "service": out.translation.provider_id,
                        "romanization": out.translation.romanisation,
                        "back": out.back.as_ref().map(|b| &b.text),
                        "failed": out.failed,
                        "elapsed_ms": out.translation.elapsed_ms,
                    })
                );
            } else {
                let block = out.translation.as_display_block();
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
            let a = e.dictionary(&text(args)?, pair, args.flag(&["-s", "--service"]))?;
            if args.json {
                out!("{}", serde_json::to_string_pretty(&a).unwrap_or_default());
            } else if !a.explanations.is_empty() || !a.examples.is_empty() {
                out!("{}", a.to_plain_text());
            } else if let Some(html) = &a.html {
                out!("{}", glossa_dict::to_plain_text(html));
            }
            if let Some(link) = &a.link {
                eprintln!("{link}");
            }
            Ok(())
        }
        "ocr" => {
            let path = args.rest.first().ok_or_else(|| Fail::Usage("ocr needs an image file".into()))?;
            let image = std::fs::read(path)?;
            let e = engine(args)?;
            let to = args.flag(&["-t", "--to"]).map(|c| lang(Some(c), LangId::ENGLISH)).transpose()?;
            let out = e.recognize(image, lang(args.flag(&["-l", "--lang"]), LangId::AUTO)?, to)?;
            if args.json {
                out!(
                    "{}",
                    json!({ "text": out.ocr.text, "translation": out.translated_text(),
                            "detected": out.ocr.detected_lang.map(|l| l.bcp47()), "service": out.ocr.provider_id,
                            "elapsed_ms": out.ocr.elapsed_ms })
                );
            } else {
                out!("{}", out.translated_text().unwrap_or(&out.ocr.text));
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
                for id in &s.loaded {
                    out!("  {id}");
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
            let handle = glossa_engine::server::start(e.clone())?;
            let token = e.config().server.token;
            eprintln!("control API on http://{}  (token: {token})", handle.addr);
            eprintln!("try: curl -s 'http://{}/v1/translate?text=hello&to=ru&token={token}'", handle.addr);
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
            let mut url = format!("http://{}/v1/action/{id}?token={}", cfg.server.bind, cfg.server.token);
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

    #[test]
    fn long_lines_are_shortened_for_listing() {
        assert_eq!(one_line("a\n  b"), "a b");
        assert_eq!(one_line(&"x".repeat(100)).chars().count(), 60);
    }
}
