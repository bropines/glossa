//! A stand-in for llama-server, for tests: `local-llm --fake-llama-server
//! <llama-server's arguments>`. It answers `/health` and
//! `/v1/chat/completions` the way llama-server does, without a model, so
//! the whole path (start, wait, request, answer, stop) runs offline.
//! The module uses it instead of the real one when `LOCAL_LLM_FAKE_SERVER=1`.
//!
//! Set by the tests:
//! - `FAKE_LLM_LOG`: a file; one JSON line with the arguments and process
//!   id at start, then every request body.
//! - `FAKE_LLM_REPLY`: the answer's content (default: `[<last message>]`).
//! - `FAKE_LLM_LOAD_MS`: how long `/health` answers 503 (loading).
//! - `FAKE_LLM_STATUS`: the status of chat answers (default 200).
//! - `FAKE_LLM_NO_AUTH=1`: accept any key, like a stranger on the port.
//!
//! Like llama-server, it requires `LLAMA_API_KEY` as a bearer token on
//! everything but `/health`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub fn run(args: &[String]) -> i32 {
    let port: u16 =
        args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(0);
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake llama-server: binding port {port}: {e}");
            return 1;
        }
    };
    log(&json!({"args": args, "pid": std::process::id()}));
    eprintln!("fake llama-server: listening on {port}");
    let started = Instant::now();
    let load = Duration::from_millis(env("FAKE_LLM_LOAD_MS").and_then(|v| v.parse().ok()).unwrap_or(0));
    for stream in listener.incoming().filter_map(Result::ok) {
        let _ = handle(stream, started.elapsed() >= load);
    }
    0
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn log(v: &Value) {
    if let Some(path) = env("FAKE_LLM_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{v}");
        }
    }
}

fn handle(stream: TcpStream, loaded: bool) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut len = 0usize;
    let mut bearer = String::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            } else if k.trim().eq_ignore_ascii_case("authorization") {
                bearer = v.trim().strip_prefix("Bearer ").unwrap_or("").to_owned();
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let key = env("LLAMA_API_KEY").unwrap_or_default();
    let authorised = env("FAKE_LLM_NO_AUTH").is_some() || (!key.is_empty() && bearer == key);
    let (status, answer) = match path.as_str() {
        "/health" if loaded => (200, json!({"status": "ok"})),
        "/health" => (503, json!({"error": {"code": 503, "message": "Loading model"}})),
        _ if !authorised => {
            (401, json!({"error": {"code": 401, "message": "Invalid API Key", "type": "authentication_error"}}))
        }
        "/props" => (200, json!({"total_slots": 1})),
        "/v1/chat/completions" => {
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            log(&req);
            let last =
                req["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["content"].as_str()).unwrap_or("");
            let content = env("FAKE_LLM_REPLY").unwrap_or_else(|| format!("[{last}]"));
            let status = env("FAKE_LLM_STATUS").and_then(|s| s.parse().ok()).unwrap_or(200);
            if status == 200 {
                (
                    200,
                    json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]}),
                )
            } else {
                (
                    status,
                    json!({"error": {"code": status, "message": "the context is too small", "type": "exceed_context_size_error"}}),
                )
            }
        }
        _ => (404, json!({"error": {"code": 404, "message": "File Not Found"}})),
    };
    let body = answer.to_string();
    let mut out = stream;
    write!(out, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    out.flush()
}
