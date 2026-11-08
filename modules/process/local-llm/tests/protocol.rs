//! The module protocol as Glossa speaks it, replayed against the built
//! binary over stdin and stdout, with the model-free stand-in for
//! llama-server (`LOCAL_LLM_FAKE_SERVER=1`, src/fake.rs). The whole path
//! runs: the `assets` host call, starting the server and waiting for it,
//! the chat request, the answer, and stopping the server. Nothing here needs
//! a model or the network. `live_translation` (ignored) uses a real one.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_local-llm");

/// A scratch folder with an empty "model" file, and the fake server's log.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("local-llm-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.gguf"), b"GGUF").unwrap();
        Scratch { dir }
    }
    fn model(&self) -> String {
        self.dir.join("model.gguf").display().to_string()
    }
    fn log(&self) -> PathBuf {
        self.dir.join("fake.log")
    }
    /// The fake server's log: its start lines and the requests it got.
    fn entries(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn starts(&self) -> Vec<Value> {
        self.entries().into_iter().filter(|e| e.get("pid").is_some()).collect()
    }
    fn requests(&self) -> Vec<Value> {
        self.entries().into_iter().filter(|e| e.get("messages").is_some()).collect()
    }
    /// The answer to the `assets` host call: the default model downloaded.
    fn assets(&self, id: &str) -> Value {
        json!({"id": id, "result": {"dir": self.dir, "files": {"llama": self.dir, "hy-mt2-1.8b-q4_k_m.gguf": self.model()}, "missing": []}})
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The module, talked to one line at a time.
struct Module {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Module {
    fn start(s: &Scratch, env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(BIN);
        cmd.env("LOCAL_LLM_FAKE_SERVER", "1")
            .env_remove("LOCAL_LLM_SERVER")
            .env_remove("GLOSSA_ASSETS_DIR")
            .env("GLOSSA_MODULE_ID", "local-llm")
            .env("FAKE_LLM_LOG", s.log())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("start local-llm");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Module { child, stdin, stdout }
    }
    fn send(&mut self, v: &Value) {
        let w = self.stdin.as_mut().unwrap();
        writeln!(w, "{v}").unwrap();
        w.flush().unwrap();
    }
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
    /// A request, answering the `assets` host call it makes on the way.
    fn call(&mut self, s: &Scratch, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"id": id, "method": method, "params": params}));
        loop {
            let msg = self.recv();
            if msg["method"] == "assets" {
                let hid = msg["id"].as_str().unwrap().to_owned();
                self.send(&s.assets(&hid));
                continue;
            }
            assert_eq!(msg["id"], id, "{msg}");
            return msg;
        }
    }
    /// Close stdin as Glossa does when the module is idle, and wait.
    fn close(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success(), "{status:?}");
    }
}

fn translate(text: &str, from: &str, to: &str, options: Value) -> Value {
    json!({"text": text, "from": from, "to": to, "options": options})
}

fn error_kind(v: &Value) -> &str {
    v["error"]["kind"].as_str().unwrap_or("")
}

/// Whether a process is still running (Unix: `kill -0`).
#[cfg(unix)]
fn running(pid: u64) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(unix)]
fn wait_gone(pid: u64) -> bool {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while Instant::now() < deadline {
        if !running(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[test]
fn translates_through_the_server_and_starts_it_once() {
    let s = Scratch::new("once");
    let mut m = Module::start(&s, &[("FAKE_LLM_REPLY", "Привет,\nмир!"), ("FAKE_LLM_LOAD_MS", "300")]);
    let out = m.call(&s, 1, "translate", translate("Hello,\nworld!", "en", "ru", json!({})));
    assert_eq!(out["result"]["text"], "Привет,\nмир!", "{out}");
    let out = m.call(&s, 2, "translate", translate("Again", "auto", "ru", json!({})));
    assert_eq!(out["result"]["text"], "Привет,\nмир!", "{out}");
    m.close();

    let starts = s.starts();
    assert_eq!(starts.len(), 1, "one server for both requests: {starts:?}");
    let args: Vec<&str> = starts[0]["args"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let joined = args.join(" ");
    assert!(joined.contains(&format!("--model {}", s.model())), "{joined}");
    assert!(joined.contains("--host 127.0.0.1") && joined.contains("--ctx-size 4096"), "{joined}");
    assert!(joined.contains("--n-gpu-layers all") && joined.contains("--parallel 1"), "{joined}");

    // Hy-MT2 gets its model card's prompt and sampling.
    let req = &s.requests()[0];
    let content = req["messages"][0]["content"].as_str().unwrap();
    assert!(content.starts_with("Translate the following text into Russian."), "{content}");
    assert!(content.ends_with("\n\nHello,\nworld!"), "{content}");
    assert_eq!(req["top_p"], 0.6);
    assert_eq!(req["stream"], false);
}

#[test]
fn qwen3_gets_a_system_prompt_and_its_thinking_is_dropped() {
    let s = Scratch::new("qwen");
    let mut m = Module::start(&s, &[("FAKE_LLM_REPLY", "<think>\n\n</think>\n\nGuten Tag")]);
    // The default model's file is the only one "downloaded".
    let out = m.call(&s, 1, "translate", translate("Good day", "en", "de", json!({"model": "qwen3"})));
    assert_eq!(error_kind(&out), "other");
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("glossa-cli assets download local-llm qwen3-1.7b-q4_k_m.gguf"));
    let custom = json!({"model": "custom", "custom_model": s.model(), "context": 2048, "gpu": false, "threads": 3});
    let out = m.call(&s, 2, "translate", translate("Good day", "en", "de", custom));
    assert_eq!(out["result"]["text"], "Guten Tag", "{out}");
    m.close();

    let req = &s.requests()[0];
    assert_eq!(req["messages"][0]["role"], "system");
    assert!(req["messages"][0]["content"].as_str().unwrap().contains("from English to German"));
    assert_eq!(req["messages"][1]["content"], "Good day");
    let joined: String =
        s.starts()[0]["args"].as_array().unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
    assert!(
        joined.contains("--ctx-size 2048") && joined.contains("--n-gpu-layers 0") && joined.contains("--threads 3"),
        "{joined}"
    );
}

#[test]
fn changed_settings_restart_the_server() {
    let s = Scratch::new("restart");
    let mut m = Module::start(&s, &[]);
    let out = m.call(&s, 1, "translate", translate("a", "en", "ru", json!({})));
    assert_eq!(out["result"]["text"], format!("[{}]", s.requests()[0]["messages"][0]["content"].as_str().unwrap()));
    m.call(&s, 2, "translate", translate("b", "en", "ru", json!({"context": 8192})));
    m.close();
    assert_eq!(s.starts().len(), 2);
}

#[test]
fn requests_the_model_cannot_do_are_refused_before_starting_it() {
    let s = Scratch::new("refuse");
    let mut m = Module::start(&s, &[]);
    let out = m.call(&s, 1, "translate", translate("Hello", "en", "sw", json!({})));
    assert_eq!(error_kind(&out), "target_language", "{out}");
    let out = m.call(&s, 2, "translate", translate("Habari", "sw", "en", json!({})));
    assert_eq!(error_kind(&out), "source_language", "{out}");
    let out =
        m.call(&s, 3, "translate", translate("Hello", "en", "ru", json!({"model": "custom", "custom_model": ""})));
    assert_eq!((error_kind(&out), out["error"]["message"].as_str()), ("missing_credential", Some("custom_model")));
    // An empty text never reaches the host call.
    m.send(&json!({"id": 4, "method": "translate", "params": translate("  ", "en", "ru", json!({}))}));
    assert_eq!(error_kind(&m.recv()), "bad_request");
    m.send(&json!({"id": 5, "method": "speak", "params": {"text": "hi"}}));
    assert_eq!(error_kind(&m.recv()), "unsupported");
    m.close();
    assert!(s.starts().is_empty(), "no server for refused requests");
}

#[test]
fn a_text_too_long_for_the_context_is_a_bad_request() {
    let s = Scratch::new("ctx");
    let mut m = Module::start(&s, &[("FAKE_LLM_STATUS", "400")]);
    let out = m.call(&s, 1, "translate", translate("long", "en", "ru", json!({"context": 1024})));
    assert_eq!(error_kind(&out), "bad_request", "{out}");
    assert!(out["error"]["message"].as_str().unwrap().contains("1024"), "{out}");
    m.close();
}

#[test]
fn detect_maps_the_named_language_to_a_code() {
    let s = Scratch::new("detect");
    let mut m = Module::start(&s, &[("FAKE_LLM_REPLY", "Russian")]);
    // Hy-MT2, the default, does not detect: it misnames languages.
    let out = m.call(&s, 1, "detect", json!({"text": "Привет, мир", "options": {}}));
    assert_eq!(error_kind(&out), "unsupported", "{out}");
    let custom = json!({"model": "custom", "custom_model": s.model()});
    let out = m.call(&s, 2, "detect", json!({"text": "Привет, мир", "options": custom}));
    assert_eq!(out["result"]["lang"], "ru", "{out}");
    m.close();
    let req = &s.requests()[0];
    assert!(req["messages"][0]["content"].as_str().unwrap().ends_with("Привет, мир"));
}

#[cfg(unix)]
#[test]
fn closing_stdin_stops_the_server() {
    let s = Scratch::new("eof");
    let mut m = Module::start(&s, &[]);
    m.call(&s, 1, "translate", translate("a", "en", "ru", json!({})));
    let pid = s.starts()[0]["pid"].as_u64().unwrap();
    assert!(running(pid));
    m.close();
    assert!(wait_gone(pid), "llama-server still running after stdin closed");
}

#[cfg(unix)]
#[test]
fn a_killed_module_does_not_leave_the_server_behind() {
    // Glossa kills a module that overruns its call timeout: the supervisor
    // sees its stdin close and stops llama-server.
    let s = Scratch::new("kill");
    let mut m = Module::start(&s, &[]);
    m.call(&s, 1, "translate", translate("a", "en", "ru", json!({})));
    let pid = s.starts()[0]["pid"].as_u64().unwrap();
    m.child.kill().unwrap();
    let _ = m.child.wait();
    assert!(wait_gone(pid), "llama-server outlived a killed module");
}

/// A real translation. Needs a llama-server and a model:
/// `LOCAL_LLM_SERVER=<llama-server> LOCAL_LLM_MODEL=<model.gguf> cargo test --release -- --ignored`.
/// `LOCAL_LLM_MODEL_ID` is `hy-mt2` (default) or `qwen3`, for the prompt.
#[test]
#[ignore]
fn live_translation() {
    let server = std::env::var("LOCAL_LLM_SERVER").expect("LOCAL_LLM_SERVER");
    let model = std::env::var("LOCAL_LLM_MODEL").expect("LOCAL_LLM_MODEL");
    let model_id = std::env::var("LOCAL_LLM_MODEL_ID").unwrap_or_else(|_| "hy-mt2".into());
    let asset = if model_id == "qwen3" { "qwen3-1.7b-q4_k_m.gguf" } else { "hy-mt2-1.8b-q4_k_m.gguf" };
    let mut child = Command::new(BIN)
        .env("LOCAL_LLM_SERVER", &server)
        .env_remove("LOCAL_LLM_FAKE_SERVER")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let files = json!({"files": {asset: model}});
    let mut ask = |id: u64, text: &str, from: &str, to: &str| -> String {
        let t = Instant::now();
        writeln!(
            stdin,
            "{}",
            json!({"id": id, "method": "translate", "params": translate(text, from, to, json!({"model": model_id}))})
        )
        .unwrap();
        loop {
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["method"] == "assets" {
                writeln!(stdin, "{}", json!({"id": msg["id"], "result": files})).unwrap();
                continue;
            }
            eprintln!("{from}->{to} in {} ms: {msg}", t.elapsed().as_millis());
            return msg["result"]["text"].as_str().unwrap_or_else(|| panic!("{msg}")).to_owned();
        }
    };
    let ru = ask(1, "The weather is nice today, so let's go for a walk in the park.", "en", "ru");
    assert!(ru.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap())), "{ru}");
    let en = ask(2, "Сегодня хорошая погода.", "ru", "en");
    assert!(en.to_lowercase().contains("weather"), "{en}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
