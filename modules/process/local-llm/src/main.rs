//! `local-llm`: offline translation with a small language model, as a
//! Glossa process module.
//!
//! Glossa starts it on the first translation and talks JSON lines over
//! stdin and stdout (docs/modules/process-modules.md). On the first
//! request it starts `llama-server` from the llama.cpp release (a module
//! asset) on a free port on 127.0.0.1, with the chosen model, and turns
//! each `translate` and `detect` request into a call to llama-server's
//! OpenAI-style chat API. When Glossa closes stdin (it does after
//! `idle_timeout_secs`) or the module dies, llama-server is stopped, so an
//! idle Glossa holds no model in memory.
//!
//! Other modes, for tests and debugging:
//!
//! ```text
//! local-llm --version
//! local-llm --supervise PROGRAM ARGS…          (used internally; see server.rs)
//! local-llm --fake-llama-server ARGS…          (a model-free llama-server for tests; see fake.rs)
//! ```

mod fake;
mod http;
mod lang;
mod prompt;
mod server;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use prompt::Family;
use server::{Config, Running};

/// The models offered on the options page: option value, asset name,
/// display name, how it is prompted, and the languages it was trained on
/// (`None`: every language this module knows).
struct Model {
    id: &'static str,
    asset: &'static str,
    name: &'static str,
    family: Family,
    languages: Option<&'static [&'static str]>,
}

const MODELS: &[Model] = &[
    Model {
        id: "hy-mt2",
        asset: "hy-mt2-1.8b-q4_k_m.gguf",
        name: "Hy-MT2 1.8B",
        family: Family::HyMt,
        languages: Some(lang::HY_MT2),
    },
    Model { id: "qwen3", asset: "qwen3-1.7b-q4_k_m.gguf", name: "Qwen3 1.7B", family: Family::Chat, languages: None },
];

/// The llama.cpp release, unpacked (a required asset).
const LLAMA_ASSET: &str = "llama";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => println!("local-llm {}", env!("CARGO_PKG_VERSION")),
        Some("--help" | "-h") => println!("{}", include_str!("usage.txt")),
        Some("--supervise") => std::process::exit(server::supervise(&args[1..])),
        Some("--fake-llama-server") => std::process::exit(fake::run(&args[1..])),
        _ => serve(),
    }
}

// -- the module protocol ----------------------------------------------------------

/// A module error: `kind` is one of the protocol's (docs/modules/protocol.md).
#[derive(Debug)]
struct Fail {
    kind: &'static str,
    message: String,
}

impl Fail {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

struct Io {
    input: io::StdinLock<'static>,
    out: io::Stdout,
    next_host_id: u64,
}

impl Io {
    /// The next line, `None` at the end of input. A line that is not UTF-8
    /// is decoded lossily: it can only be a broken request, answered below.
    fn next_line(&mut self) -> Option<String> {
        let mut buf = Vec::new();
        match self.input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).to_owned()),
        }
    }

    fn send(&mut self, v: &Value) {
        let mut out = self.out.lock();
        // A closed stdout means Glossa is gone; the read loop ends next.
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    /// A host call: ask, then read until the answer with our id comes.
    /// Glossa sends nothing else while a request is open.
    fn host(&mut self, method: &str, params: Value) -> Result<Value, Fail> {
        self.next_host_id += 1;
        let id = format!("h{}", self.next_host_id);
        self.send(&json!({"id": id, "method": method, "params": params}));
        while let Some(line) = self.next_line() {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                let text = err.get("message").and_then(Value::as_str).unwrap_or("host call failed");
                return Err(Fail::new("other", format!("{method}: {text}")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(Fail::new("other", "stdin closed during a host call"))
    }
}

fn serve() {
    let mut io = Io { input: io::stdin().lock(), out: io::stdout(), next_host_id: 0 };
    let mut server: Option<Running> = None;
    while let Some(line) = io.next_line() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            eprintln!("local-llm: not JSON: {line}");
            continue;
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let answer = match method {
            "translate" => translate(&mut io, &mut server, &params),
            "detect" => detect(&mut io, &mut server, &params),
            other => Err(Fail::new("unsupported", format!("local-llm translates text; it does not do {other:?}"))),
        };
        let reply = match answer {
            Ok(result) => json!({"id": id, "result": result}),
            Err(f) => json!({"id": id, "error": {"kind": f.kind, "message": f.message}}),
        };
        io.send(&reply);
    }
    // End of input: Glossa is done with us. Dropping the server stops it.
    drop(server);
}

// -- requests ---------------------------------------------------------------------

/// The settings every request carries, read from `options`.
struct Settings {
    model: String,
    custom_model: String,
    context: u32,
    threads: u32,
    gpu: bool,
}

impl Settings {
    fn from_options(o: &Value) -> Self {
        let num = |key: &str, default: f64, min: f64, max: f64| {
            let v = &o[key];
            let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(default);
            n.clamp(min, max) as u32
        };
        Settings {
            model: o["model"].as_str().filter(|s| !s.is_empty()).unwrap_or("hy-mt2").to_owned(),
            custom_model: o["custom_model"].as_str().unwrap_or("").trim().to_owned(),
            context: num("context", 4096.0, 512.0, 131_072.0),
            threads: num("threads", 0.0, 0.0, 256.0),
            gpu: o["gpu"].as_bool().unwrap_or(true),
        }
    }
}

/// What a request will run on: the model's file and how it is prompted.
struct Chosen {
    path: PathBuf,
    name: String,
    family: Family,
    languages: Option<&'static [&'static str]>,
}

fn module_id() -> String {
    std::env::var("GLOSSA_MODULE_ID").unwrap_or_else(|_| "local-llm".into())
}

/// The module's files by name, from the `assets` host call.
fn asset_files(io: &mut Io) -> Result<Vec<(String, PathBuf)>, Fail> {
    match io.host("assets", Value::Null) {
        Ok(v) => Ok(v
            .get("files")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), PathBuf::from(v.as_str()?)))).collect())
            .unwrap_or_default()),
        // A Glossa without the call: whatever is in the assets folder.
        Err(_) => Ok(std::env::var_os("GLOSSA_ASSETS_DIR")
            .map(|d| {
                let d = PathBuf::from(d);
                let mut names = vec![LLAMA_ASSET.to_owned()];
                names.extend(MODELS.iter().map(|m| m.asset.to_owned()));
                names.into_iter().map(|n| (n.clone(), d.join(n))).filter(|(_, p)| p.exists()).collect()
            })
            .unwrap_or_default()),
    }
}

fn choose(settings: &Settings, files: &[(String, PathBuf)]) -> Result<Chosen, Fail> {
    if settings.model == "custom" {
        if settings.custom_model.is_empty() || !Path::new(&settings.custom_model).is_file() {
            // A setup problem: Glossa points the user at the setting.
            return Err(Fail::new("missing_credential", "custom_model"));
        }
        let path = PathBuf::from(&settings.custom_model);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Ok(Chosen { path, name, family: Family::Chat, languages: None });
    }
    let m = MODELS.iter().find(|m| m.id == settings.model).unwrap_or(&MODELS[0]);
    let path = files.iter().find(|(n, _)| n == m.asset).map(|(_, p)| p.clone()).ok_or_else(|| {
        Fail::new(
            "other",
            format!(
                "the {} model is not downloaded: Options → Services → Local LLM → Files, or `glossa-cli assets download {} {}`",
                m.name,
                module_id(),
                m.asset
            ),
        )
    })?;
    Ok(Chosen { path, name: m.name.into(), family: m.family, languages: m.languages })
}

/// The llama-server command: `LOCAL_LLM_SERVER` (your own llama.cpp
/// build), the test server, or the one in the `llama` asset.
fn server_program(files: &[(String, PathBuf)]) -> Result<Vec<String>, Fail> {
    if std::env::var("LOCAL_LLM_FAKE_SERVER").is_ok_and(|v| v == "1") {
        let me = std::env::current_exe().map_err(|e| Fail::new("other", e.to_string()))?;
        return Ok(vec![me.display().to_string(), "--fake-llama-server".into()]);
    }
    if let Some(p) = std::env::var_os("LOCAL_LLM_SERVER").filter(|p| !p.is_empty()) {
        return Ok(vec![PathBuf::from(p).display().to_string()]);
    }
    let dir = files.iter().find(|(n, _)| n == LLAMA_ASSET).map(|(_, p)| p.clone()).ok_or_else(|| {
        Fail::new("other", format!("llama.cpp is not downloaded: `glossa-cli assets download {}`", module_id()))
    })?;
    let exe = server::find_server(&dir).ok_or_else(|| {
        Fail::new("other", format!("no llama-server in {}; download the module's files again", dir.display()))
    })?;
    Ok(vec![exe.display().to_string()])
}

/// The running server for these settings, started (or restarted) if needed.
fn ensure(server: &mut Option<Running>, config: Config) -> Result<&mut Running, Fail> {
    let reuse = match server.as_mut() {
        Some(s) => s.config == config && s.alive(),
        None => false,
    };
    if !reuse {
        // Stop the old one first: two models at once could exhaust memory.
        *server = None;
        let started = Instant::now();
        let s = Running::start(config).map_err(|e| Fail::new("other", e))?;
        eprintln!("local-llm: started llama-server in {} ms", started.elapsed().as_millis());
        *server = Some(s);
    }
    Ok(server.as_mut().expect("just set"))
}

/// The work a request needs before talking to the model.
fn prepare(io: &mut Io, params: &Value) -> Result<(Settings, Chosen, Config), Fail> {
    let settings = Settings::from_options(params.get("options").unwrap_or(&Value::Null));
    let files = asset_files(io)?;
    let chosen = choose(&settings, &files)?;
    let config = Config {
        program: server_program(&files)?,
        model: chosen.path.clone(),
        context: settings.context,
        threads: settings.threads,
        gpu: settings.gpu,
    };
    Ok((settings, chosen, config))
}

/// One chat completion. A server that died since the last request (it
/// was killed, or crashed) is started again once.
fn complete(server: &mut Option<Running>, config: Config, mut body: Value, family: Family) -> Result<String, Fail> {
    if let (Some(b), Some(s)) = (body.as_object_mut(), prompt::sampling(family).as_object()) {
        b.extend(s.clone());
    }
    body["stream"] = json!(false);
    let text = body.to_string();
    let mut tries = 0;
    loop {
        tries += 1;
        let s = ensure(server, config.clone())?;
        let started = Instant::now();
        match http::request(s.port, "POST", "/v1/chat/completions", Some(&text), Duration::from_secs(600)) {
            Ok(r) if r.status == 200 => {
                eprintln!("local-llm: answered in {} ms", started.elapsed().as_millis());
                return answer_text(&r.body);
            }
            Ok(r) => return Err(server_error(r.status, &r.body, config.context)),
            Err(e) if tries == 1 && !s.alive() => {
                eprintln!("local-llm: llama-server went away ({e}); starting it again");
                *server = None;
            }
            Err(e) => return Err(Fail::new("other", format!("{e}{}", s.last_lines()))),
        }
    }
}

/// The content of an OpenAI-style chat answer.
fn answer_text(body: &str) -> Result<String, Fail> {
    let v: Value =
        serde_json::from_str(body).map_err(|e| Fail::new("bad_response", format!("llama-server sent no JSON: {e}")))?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Fail::new("bad_response", "llama-server's answer has no message content"))
}

/// An error answer from llama-server, as a module error.
fn server_error(status: u16, body: &str, context: u32) -> Fail {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().unwrap_or(body).to_owned();
    if v["error"]["type"] == "exceed_context_size_error" {
        return Fail::new(
            "bad_request",
            format!("the text is too long for a context of {context} tokens; raise Context length or translate less at once"),
        );
    }
    Fail::new(if status >= 500 { "other" } else { "bad_request" }, format!("llama-server: HTTP {status}: {msg}"))
}

/// How many tokens the answer may take: room for a translation a few times
/// longer in tokens than the source, within half the context.
fn max_tokens(text: &str, context: u32) -> u32 {
    let want = text.chars().count() as u32 * 3 + 64;
    want.min(context / 2).max(64)
}

/// A language code as a name, or the protocol error for a language this
/// model cannot do.
fn language(code: &str, chosen: &Chosen, target: bool) -> Result<&'static str, Fail> {
    let kind = if target { "target_language" } else { "source_language" };
    let unknown = || {
        Fail::new(kind, format!("{} does not translate {} {code:?}", chosen.name, if target { "into" } else { "from" }))
    };
    let canonical = lang::canonical(code).ok_or_else(unknown)?;
    if let Some(list) = chosen.languages {
        if !list.contains(&canonical) {
            return Err(unknown());
        }
    }
    lang::name(canonical).ok_or_else(unknown)
}

fn translate(io: &mut Io, server: &mut Option<Running>, params: &Value) -> Result<Value, Fail> {
    let text = params["text"].as_str().unwrap_or("");
    if text.trim().is_empty() {
        return Err(Fail::new("bad_request", "nothing to translate"));
    }
    let (settings, chosen, config) = prepare(io, params)?;
    let to = language(params["to"].as_str().unwrap_or(""), &chosen, true)?;
    let from = match params["from"].as_str().unwrap_or("auto") {
        "" | "auto" => None,
        code => Some(language(code, &chosen, false)?),
    };
    let glossary: Vec<(String, String)> = params["glossary"]
        .as_array()
        .map(|g| {
            g.iter()
                .filter_map(|e| {
                    Some((e["source"].as_str()?.trim().to_owned(), e["target"].as_str()?.trim().to_owned()))
                })
                .filter(|(s, t)| !s.is_empty() && !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let style = params["style"].as_str().map(str::trim).filter(|s| !s.is_empty() && *s != "default");
    let ask = prompt::Ask { text, from, to, style, glossary: &glossary };
    let body = json!({
        "messages": prompt::translate_messages(chosen.family, &ask),
        "max_tokens": max_tokens(text, settings.context),
    });
    let answer = complete(server, config, body, chosen.family)?;
    let translation = prompt::clean(&answer, text);
    if translation.is_empty() {
        return Err(Fail::new("bad_response", format!("{} answered with nothing", chosen.name)));
    }
    Ok(json!({"text": translation}))
}

fn detect(io: &mut Io, server: &mut Option<Running>, params: &Value) -> Result<Value, Fail> {
    let text = params["text"].as_str().unwrap_or("");
    if text.trim().is_empty() {
        return Err(Fail::new("bad_request", "no text to detect the language of"));
    }
    let (_, chosen, config) = prepare(io, params)?;
    if chosen.family == Family::HyMt {
        // Asked to name a language, Hy-MT2 called Russian "English" in
        // tests: a translation model, not a classifier.
        return Err(Fail::new(
            "unsupported",
            format!("{} does not detect languages; Qwen3 or a custom model does", chosen.name),
        ));
    }
    // A few hundred characters are plenty to tell a language.
    let sample: String = text.chars().take(400).collect();
    let body = json!({"messages": prompt::detect_messages(&sample), "max_tokens": 16});
    let answer = complete(server, config, body, Family::Chat)?;
    let answer = prompt::clean(&answer, "");
    let code = lang::from_name(&answer)
        .ok_or_else(|| Fail::new("bad_response", format!("{} named no language it knows: {answer:?}", chosen.name)))?;
    Ok(json!({"lang": code, "confidence": 0.9}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_have_defaults_and_limits() {
        let s = Settings::from_options(&Value::Null);
        assert_eq!((s.model.as_str(), s.context, s.threads, s.gpu), ("hy-mt2", 4096, 0, true));
        let s = Settings::from_options(&json!({"model": "qwen3", "context": "100", "threads": 8, "gpu": false}));
        assert_eq!((s.model.as_str(), s.context, s.threads, s.gpu), ("qwen3", 512, 8, false));
    }

    #[test]
    fn a_missing_model_names_its_download() {
        let s = Settings::from_options(&json!({"model": "qwen3"}));
        let err = choose(&s, &[]).err().unwrap();
        assert!(
            err.message.contains("assets download") && err.message.contains("qwen3-1.7b-q4_k_m.gguf"),
            "{}",
            err.message
        );
        let s = Settings::from_options(&json!({"model": "custom", "custom_model": "/nowhere/x.gguf"}));
        let err = choose(&s, &[]).err().unwrap();
        assert_eq!((err.kind, err.message.as_str()), ("missing_credential", "custom_model"));
    }

    #[test]
    fn languages_follow_the_model() {
        let hy = Chosen {
            path: PathBuf::new(),
            name: "Hy-MT2 1.8B".into(),
            family: Family::HyMt,
            languages: Some(lang::HY_MT2),
        };
        assert_eq!(language("ru", &hy, true).unwrap(), "Russian");
        assert_eq!(language("zh-CN", &hy, true).unwrap(), "Chinese");
        assert_eq!(language("sw", &hy, true).unwrap_err().kind, "target_language");
        assert_eq!(language("sw", &hy, false).unwrap_err().kind, "source_language");
        let qwen = Chosen { languages: None, family: Family::Chat, ..hy };
        assert_eq!(language("sw", &qwen, true).unwrap(), "Swahili");
        assert_eq!(language("xx", &qwen, true).unwrap_err().kind, "target_language");
    }

    #[test]
    fn answer_room_grows_with_the_text_within_the_context() {
        assert_eq!(max_tokens("hi", 4096), 70);
        assert_eq!(max_tokens(&"x".repeat(5000), 4096), 2048);
    }

    #[test]
    fn server_errors_become_module_errors() {
        let f =
            server_error(400, r#"{"error":{"code":400,"message":"too big","type":"exceed_context_size_error"}}"#, 2048);
        assert_eq!(f.kind, "bad_request");
        assert!(f.message.contains("2048"));
        assert_eq!(server_error(500, "boom", 1).kind, "other");
    }
}
