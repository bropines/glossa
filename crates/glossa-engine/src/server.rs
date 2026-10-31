//! The local control API.
//!
//! A small HTTP server on loopback that lets anything on the machine drive
//! Glossa: scripts, launchers, a Stream Deck, and — the reason it exists from
//! day one — compositor key bindings on Wayland desktops, where applications
//! cannot register global hotkeys themselves. Bind a key to
//! `curl -s localhost:60828/v1/action/show_popup` and the popup works there
//! too.
//!
//! ```text
//! GET  /health                         {"name":"glossa","version":"…"}
//! GET  /v1/services?capability=translate
//! POST /v1/translate   {"text","from"?,"to"?,"service"?,"back"?}
//! GET  /v1/translate?text=…&to=…
//! POST /v1/detect      {"text"}
//! POST /v1/dictionary  {"text","from"?,"to"?,"service"?}
//! POST /v1/ocr         image bytes (image/*), or {"image_base64","lang"?,"translate_to"?}
//! POST /v1/speak       {"text","lang"?,"slow"?}            -> audio bytes
//! GET|POST /v1/action/<action>?text=…   ask the UI to act (show_popup, text_recognition…)
//! ```
//!
//! pot-app's endpoints are answered too (`/selection_translate`,
//! `/input_translate`, `/ocr_recognize`, `/ocr_translate`, `POST /translate`
//! with a text body), so key bindings made for it keep working.
//!
//! Security: loopback by default; every request must carry the token
//! (`Authorization: Bearer …` or `?token=…`) unless the user cleared it; and a
//! request carrying a browser `Origin` header is refused, so a web page
//! cannot reach the API from the user's browser.

use std::io::Read;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use glossa_core::{Action, Capabilities, Error, LangId, LangPair};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::{Engine, TranslateOptions};

/// A running control server. Dropping it stops the server.
pub struct ServerHandle {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    server: Arc<Server>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

const MAX_BODY: u64 = 32 * 1024 * 1024;

/// Start the server per the settings. Refuses to listen beyond loopback
/// without a token.
pub fn start(engine: Engine) -> Result<ServerHandle, Error> {
    let cfg = engine.config().server;
    let addr: SocketAddr = cfg.bind.parse().map_err(|e| Error::Config(format!("server.bind {:?}: {e}", cfg.bind)))?;
    if !addr.ip().is_loopback() && cfg.token.is_empty() {
        return Err(Error::Config("the control API may listen beyond loopback only with a token".into()));
    }
    let server = Arc::new(Server::http(addr).map_err(|e| Error::Platform(format!("control API on {addr}: {e}")))?);
    let bound = server.server_addr().to_ip().unwrap_or(addr);
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (server.clone(), stop.clone());
    let thread = std::thread::Builder::new()
        .name("glossa-api".into())
        .spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match s.recv_timeout(Duration::from_millis(500)) {
                    Ok(Some(req)) => {
                        let engine = engine.clone();
                        // Each request on its own thread: a slow translation
                        // must not hold up a key binding.
                        let _ = std::thread::Builder::new().name("glossa-api-req".into()).spawn(move || handle(&engine, req));
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| Error::Platform(e.to_string()))?;
    tracing::info!("control API listening on http://{bound}");
    Ok(ServerHandle { addr: bound, stop, server, thread: Some(thread) })
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
}

impl Reply {
    fn json(status: u16, v: Value) -> Reply {
        Reply { status, body: v.to_string().into_bytes(), content_type: "application/json; charset=utf-8" }
    }

    fn error(status: u16, message: impl std::fmt::Display) -> Reply {
        Reply::json(status, json!({ "error": message.to_string() }))
    }

    fn from_error(e: &Error) -> Reply {
        let status = match e {
            Error::EmptyInput | Error::SameLanguage | Error::Config(_) => 400,
            Error::NoProvider(_) | Error::NoSuchModule(_) => 404,
            Error::NoTextRecognised | Error::LanguageNotDetected => 422,
            Error::MissingCredential { .. } => 412,
            Error::RateLimited { .. } => 429,
            Error::Timeout(_) => 504,
            _ => 502,
        };
        Reply::error(status, e)
    }
}

fn handle(engine: &Engine, mut req: Request) {
    let reply = route(engine, &mut req);
    let mut resp = Response::from_data(reply.body).with_status_code(reply.status);
    if let Ok(h) = Header::from_bytes("Content-Type", reply.content_type) {
        resp = resp.with_header(h);
    }
    let _ = req.respond(resp);
}

fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn authorised(engine: &Engine, req: &Request) -> bool {
    let token = engine.config().server.token;
    if token.is_empty() {
        return true;
    }
    let bearer = header(req, "Authorization").and_then(|v| v.strip_prefix("Bearer ")).map(str::trim);
    let query = query_param(req.url(), "token");
    constant_time_eq(bearer.unwrap_or(""), &token) || query.is_some_and(|q| constant_time_eq(&q, &token))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn read_body(req: &mut Request) -> Result<Vec<u8>, Reply> {
    let mut body = Vec::new();
    req.as_reader().take(MAX_BODY + 1).read_to_end(&mut body).map_err(|e| Reply::error(400, e))?;
    if body.len() as u64 > MAX_BODY {
        return Err(Reply::error(413, "request body too large"));
    }
    Ok(body)
}

fn json_body(req: &mut Request) -> Result<Value, Reply> {
    let body = read_body(req)?;
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&body).map_err(|e| Reply::error(400, format!("invalid JSON: {e}")))
}

/// A field from the JSON body, else the query string.
fn field(body: &Value, url: &str, key: &str) -> Option<String> {
    body.get(key).and_then(|v| v.as_str()).map(str::to_owned).or_else(|| query_param(url, key))
}

fn lang(s: Option<String>, default: LangId) -> LangId {
    s.as_deref().and_then(LangId::from_code).unwrap_or(default)
}

fn route(engine: &Engine, req: &mut Request) -> Reply {
    if header(req, "Origin").is_some_and(|o| o != "null") {
        return Reply::error(403, "browser requests are not accepted");
    }
    let url = req.url().to_owned();
    let path = url.split('?').next().unwrap_or("/").trim_end_matches('/').to_owned();
    let method = req.method().clone();

    if path.is_empty() || path == "/health" {
        return Reply::json(200, json!({ "name": "glossa", "version": glossa_core::VERSION }));
    }
    if !authorised(engine, req) {
        return Reply::error(401, "missing or wrong token");
    }

    match (method, path.as_str()) {
        (Method::Get, "/v1/services") => {
            let cap = query_param(&url, "capability").and_then(|c| Capabilities::from_name(&c)).unwrap_or(Capabilities::TRANSLATE);
            let list: Vec<Value> = engine
                .providers(cap)
                .into_iter()
                .map(|p| {
                    json!({ "id": p.info.id, "name": p.info.name, "origin": p.origin, "enabled": p.enabled,
                            "capabilities": p.info.capabilities.names() })
                })
                .collect();
            Reply::json(200, Value::Array(list))
        }
        (Method::Get | Method::Post, "/v1/translate") => {
            let body = match json_body(req) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let pair = LangPair::new(lang(field(&body, &url, "from"), LangId::AUTO), lang(field(&body, &url, "to"), LangId::AUTO));
            let opts = TranslateOptions {
                provider: field(&body, &url, "service"),
                back_translate: body.get("back").and_then(|v| v.as_bool()).unwrap_or(false),
                ..Default::default()
            };
            match engine.translate(&text, pair, &opts) {
                Ok(out) => Reply::json(
                    200,
                    json!({
                        "text": out.translation.text,
                        "from": out.pair.from.bcp47(),
                        "to": out.pair.to.bcp47(),
                        "service": out.translation.provider_id,
                        "romanization": out.translation.romanisation,
                        "alternatives": out.translation.alternatives,
                        "back": out.back.map(|b| b.text),
                        "elapsed_ms": out.translation.elapsed_ms,
                    }),
                ),
                Err(e) => Reply::from_error(&e),
            }
        }
        (Method::Post, "/v1/detect") => {
            let body = match json_body(req) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            match engine.detect(&text) {
                Ok(d) => Reply::json(200, json!({ "lang": d.lang.bcp47(), "name": d.lang.english_name(), "confidence": d.confidence })),
                Err(e) => Reply::from_error(&e),
            }
        }
        (Method::Post | Method::Get, "/v1/dictionary") => {
            let body = match json_body(req) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let pair = LangPair::new(lang(field(&body, &url, "from"), LangId::AUTO), lang(field(&body, &url, "to"), LangId::AUTO));
            match engine.dictionary(&text, pair, field(&body, &url, "service").as_deref()) {
                Ok(a) => Reply::json(200, serde_json::to_value(&a).unwrap_or_default()),
                Err(e) => Reply::from_error(&e),
            }
        }
        (Method::Post, "/v1/ocr") => {
            let is_image = header(req, "Content-Type").is_some_and(|c| c.starts_with("image/") || c.starts_with("application/octet-stream"));
            let (image, lang_code, to) = if is_image {
                match read_body(req) {
                    Ok(b) => (b, query_param(&url, "lang"), query_param(&url, "translate_to")),
                    Err(r) => return r,
                }
            } else {
                let body = match json_body(req) {
                    Ok(b) => b,
                    Err(r) => return r,
                };
                let Some(b64) = body.get("image_base64").and_then(|v| v.as_str()) else {
                    return Reply::error(400, "send image bytes or {\"image_base64\": …}");
                };
                match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(bytes) => (bytes, field(&body, &url, "lang"), field(&body, &url, "translate_to")),
                    Err(e) => return Reply::error(400, format!("image_base64: {e}")),
                }
            };
            let to = to.as_deref().and_then(LangId::from_code);
            match engine.recognize(image, lang(lang_code, LangId::AUTO), to) {
                Ok(out) => Reply::json(
                    200,
                    json!({
                        "text": out.ocr.text,
                        "translation": out.translated_text(),
                        "detected": out.ocr.detected_lang.map(|l| l.bcp47()),
                        "service": out.ocr.provider_id,
                    }),
                ),
                Err(e) => Reply::from_error(&e),
            }
        }
        (Method::Post, "/v1/speak") => {
            let body = match json_body(req) {
                Ok(b) => b,
                Err(r) => return r,
            };
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let slow = body.get("slow").and_then(|v| v.as_bool());
            match engine.speak(&text, lang(field(&body, &url, "lang"), LangId::AUTO), slow) {
                Ok(audio) => Reply { status: 200, body: audio.bytes, content_type: if audio.mime == "audio/wav" { "audio/wav" } else { "audio/mpeg" } },
                Err(e) => Reply::from_error(&e),
            }
        }
        (Method::Get | Method::Post, p) if p.starts_with("/v1/action/") => {
            let id = &p["/v1/action/".len()..];
            let Some(action) = Action::from_id(id) else {
                return Reply::error(404, format!("no action {id:?}"));
            };
            let text = query_param(&url, "text").or_else(|| {
                read_body(req).ok().filter(|b| !b.is_empty()).map(|b| String::from_utf8_lossy(&b).into_owned())
            });
            engine.request_action(action, text);
            Reply::json(202, json!({ "accepted": action.id() }))
        }
        // pot-app's control endpoints, for key bindings carried over from it.
        (_, "/selection_translate") => pot_action(engine, Action::ShowPopup, None),
        (_, "/input_translate") => pot_action(engine, Action::ShowMainWindow, None),
        (_, "/ocr_recognize") => pot_action(engine, Action::TextRecognition, None),
        (_, "/ocr_translate") => pot_action(engine, Action::RecognizeAndTranslate, None),
        (Method::Post, "/translate") => match read_body(req) {
            Ok(b) => pot_action(engine, Action::ShowPopup, Some(String::from_utf8_lossy(&b).into_owned())),
            Err(r) => r,
        },
        _ => Reply::error(404, format!("no route {path}")),
    }
}

fn pot_action(engine: &Engine, action: Action, text: Option<String>) -> Reply {
    engine.request_action(action, text);
    Reply::json(202, json!({ "accepted": action.id() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use glossa_core::{Config, Paths};
    use std::io::Write;
    use std::net::TcpStream;

    fn engine() -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-api-{}-{}", std::process::id(), rand_suffix()));
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.server.bind = "127.0.0.1:0".into();
        cfg.server.token = "secret".into();
        (Engine::with_config(paths, cfg).unwrap(), dir)
    }

    fn rand_suffix() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u64
    }

    /// A minimal HTTP/1.1 client, to test against the real socket.
    fn request(addr: SocketAddr, raw: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        let status = out.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
        (status, body)
    }

    #[test]
    fn health_is_open_and_everything_else_needs_the_token() {
        let (e, dir) = engine();
        let server = start(e.clone()).unwrap();
        let (status, body) = request(server.addr, "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("\"glossa\""));

        let (status, _) = request(server.addr, "GET /v1/services HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 401);
        let (status, body) = request(server.addr, "GET /v1/services?token=secret HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("\"google\""));
        let (status, _) = request(
            server.addr,
            "GET /v1/services HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 200);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn browser_requests_are_refused() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let (status, _) = request(
            server.addr,
            "GET /v1/services?token=secret HTTP/1.1\r\nHost: x\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 403);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn actions_reach_the_front_end_with_their_text() {
        let (e, dir) = engine();
        let rx = e.subscribe();
        let server = start(e).unwrap();
        let (status, _) = request(
            server.addr,
            "GET /v1/action/show_popup?token=secret&text=hello%20world HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 202);
        let got = (0..50).find_map(|_| match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Action { action, text }) => Some((action, text)),
            _ => None,
        });
        assert_eq!(got, Some((Action::ShowPopup, Some("hello world".into()))));
        let (status, _) = request(server.addr, "GET /v1/action/self_destruct?token=secret HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 404);
        // pot-app's endpoint maps onto the same action.
        let (status, _) = request(server.addr, "GET /selection_translate?token=secret HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 202);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bad_requests_get_clear_errors() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let body = "{not json";
        let (status, resp) = request(
            server.addr,
            &format!("POST /v1/translate?token=secret HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
        );
        assert_eq!(status, 400);
        assert!(resp.contains("invalid JSON"));
        let (status, _) = request(
            server.addr,
            "POST /v1/translate?token=secret HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        );
        assert_eq!(status, 400, "text is required");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exposing_beyond_loopback_needs_a_token() {
        let dir = std::env::temp_dir().join(format!("glossa-api-open-{}", std::process::id()));
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        let mut cfg = Config::default();
        cfg.server.bind = "0.0.0.0:0".into();
        let e = Engine::with_config(paths, cfg).unwrap();
        e.update_config(|c| c.server.token.clear()).unwrap();
        assert!(matches!(start(e), Err(Error::Config(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_strings_decode() {
        assert_eq!(query_param("/x?text=a%20b+c&to=ru", "text").as_deref(), Some("a b c"));
        assert_eq!(query_param("/x?text=%D0%BF%D1%80", "text").as_deref(), Some("пр"));
        assert_eq!(query_param("/x?a=1", "b"), None);
        assert_eq!(percent_decode("100%"), "100%");
    }
}
