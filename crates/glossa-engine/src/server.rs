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
//! POST /v1/translate   {"text","from"?,"to"?,"service"?,"back"?,"history"?}
//! GET  /v1/translate?text=…&to=…
//! POST /v1/detect      {"text"}
//! POST /v1/dictionary  {"text","from"?,"to"?,"service"?}
//! POST /v1/ocr         image bytes (image/*), or {"image_base64","lang"?,"translate_to"?,
//!                      "engine"?,"format"?,"in_place"?}; format text|json|hocr|alto returns
//!                      that document (json: the full layout) instead of the summary;
//!                      in_place translates block by block, each with its box, under "blocks"
//! POST /v1/speak       {"text","lang"?,"slow"?}            -> audio bytes
//! GET|POST /v1/action/<action>?text=…   ask the UI to act (show_popup, text_recognition…)
//! ```
//!
//! Parameters are checked as strictly as the command line checks them: an
//! unknown language, capability or service id is a 400, never quietly
//! replaced by a default. A known path asked with the wrong method is a 405
//! with an `Allow` header.
//!
//! pot-app's endpoints are answered too (`/selection_translate`,
//! `/input_translate`, `/ocr_recognize`, `/ocr_translate`, `POST /translate`
//! with a text body), so key bindings made for it keep working.
//!
//! Security: loopback by default; every request must carry the token
//! (`Authorization: Bearer …` or `?token=…`), which is never empty (the
//! engine replaces an empty one). The one exception is pot-app's routes,
//! which pot-app's bindings call without a token: with
//! `[server] pot_compat_without_token` on, they are let through from a
//! loopback peer only. They can only open windows, never return data. Any
//! request carrying a header only browsers send (`Origin`, `Sec-Fetch-*`,
//! `Referer`) is refused, so a web page cannot reach the API from the user's
//! browser, not even with an `<img>`; and the `Host` must be an address or
//! `localhost`, which defeats DNS rebinding.

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
    let addr = checked_bind(&cfg.bind, &cfg.token)?;
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
                        let _ = std::thread::Builder::new()
                            .name("glossa-api-req".into())
                            .spawn(move || handle(&engine, req));
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

/// The address to listen on. The engine never leaves the token empty, but
/// listening beyond loopback without one is refused here too, so no
/// mistake elsewhere can expose an open API to the network.
fn checked_bind(bind: &str, token: &str) -> Result<SocketAddr, Error> {
    let addr: SocketAddr = bind.parse().map_err(|e| Error::Config(format!("server.bind {bind:?}: {e}")))?;
    if !addr.ip().is_loopback() && token.is_empty() {
        return Err(Error::Config("the control API may listen beyond loopback only with a token".into()));
    }
    Ok(addr)
}

/// Where `glossa-cli action` (or anything else on this machine) should
/// connect to reach a server listening on `bind`: the same port, but on
/// loopback when the server listens on every address (`0.0.0.0`, `::`),
/// which is not an address one can connect to everywhere.
pub fn connect_addr(bind: &str) -> String {
    match bind.parse::<SocketAddr>() {
        Ok(SocketAddr::V4(a)) if a.ip().is_unspecified() => format!("127.0.0.1:{}", a.port()),
        Ok(SocketAddr::V6(a)) if a.ip().is_unspecified() => format!("[::1]:{}", a.port()),
        _ => bind.to_owned(),
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
    /// The `Allow` header of a 405.
    allow: Option<&'static str>,
}

impl Reply {
    fn json(status: u16, v: Value) -> Reply {
        Reply { status, body: v.to_string().into_bytes(), content_type: "application/json; charset=utf-8", allow: None }
    }

    fn bytes(status: u16, body: Vec<u8>, content_type: &'static str) -> Reply {
        Reply { status, body, content_type, allow: None }
    }

    fn error(status: u16, message: impl std::fmt::Display) -> Reply {
        Reply::json(status, json!({ "error": message.to_string() }))
    }

    fn from_error(e: &Error) -> Reply {
        let status = match e {
            Error::EmptyInput | Error::SameLanguage | Error::Config(_) => 400,
            Error::NoProvider(_) | Error::NoSuchModule(_) => 404,
            Error::NoTextRecognised | Error::LanguageNotDetected => 422,
            Error::MissingCredential { .. } | Error::AssetsMissing { .. } | Error::RuntimeMissing { .. } => 412,
            Error::RateLimited { .. } => 429,
            Error::Timeout(_) => 504,
            Error::Internal(_) => 500,
            _ => 502,
        };
        Reply::error(status, e)
    }

    /// OCR errors: an engine that is not installed or not set up (Lens
    /// without chrome-lens-py, no engine at all) is the server's problem,
    /// not the request's, so it is a 503 rather than a 400 or 404.
    fn from_ocr_error(e: &Error) -> Reply {
        if e.is_configuration_fault() || matches!(e, Error::NoProvider(_)) {
            Reply::error(503, e)
        } else {
            Reply::from_error(e)
        }
    }
}

fn handle(engine: &Engine, mut req: Request) {
    // A bug behind one request answers 500 instead of ending the program
    // (release builds unwind; the panic hook has logged the panic).
    let reply = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| route(engine, &mut req))) {
        Ok(reply) => reply,
        Err(payload) => {
            let msg = glossa_core::error::panic_message(payload.as_ref());
            tracing::error!("control API request panicked: {msg}");
            Reply::error(500, format!("internal error: {msg}"))
        }
    };
    let mut resp = Response::from_data(reply.body).with_status_code(reply.status);
    if let Ok(h) = Header::from_bytes("Content-Type", reply.content_type) {
        resp = resp.with_header(h);
    }
    if let Some(allow) = reply.allow {
        if let Ok(h) = Header::from_bytes("Allow", allow) {
            resp = resp.with_header(h);
        }
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
    // The engine never leaves the token empty; if it somehow is, nothing
    // gets in rather than everything.
    if token.is_empty() {
        return false;
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

/// pot-app's routes, which its key bindings call without a token.
const POT_ROUTES: &[&str] =
    &["/selection_translate", "/input_translate", "/ocr_recognize", "/ocr_translate", "/translate"];

/// Whether a pot-app route may skip the token: the setting is on and the
/// peer is on this machine. (A request with an `Origin` header never gets
/// this far.)
fn pot_route_open(engine: &Engine, req: &Request, path: &str) -> bool {
    POT_ROUTES.contains(&path)
        && engine.config().server.pot_compat_without_token
        && req.remote_addr().is_some_and(|a| a.ip().is_loopback())
}

/// The methods a known path answers, for a 405 and its `Allow` header.
fn allowed_methods(path: &str) -> Option<&'static str> {
    match path {
        "/v1/services" => Some("GET"),
        "/v1/translate" | "/v1/dictionary" => Some("GET, POST"),
        "/v1/detect" | "/v1/ocr" | "/v1/speak" | "/translate" => Some("POST"),
        p if p.starts_with("/v1/action/") => Some("GET, POST"),
        "/v1/assets" => Some("GET"),
        p if p.starts_with("/v1/assets/") => Some(if p.matches('/').count() == 3 { "GET" } else { "POST" }),
        "/v1/python" => Some("GET"),
        "/v1/presets" | "/v1/presets/export" => Some("GET"),
        p if p.starts_with("/v1/presets/") => Some("POST"),
        "/v1/python/install" | "/v1/python/cancel" => Some("POST"),
        p if p.starts_with("/v1/python/") => Some(if p.matches('/').count() == 3 { "GET" } else { "POST" }),
        _ => None,
    }
}

fn method_allowed(method: &Method, allow: &str) -> bool {
    allow.split(", ").any(|m| m == method.as_str())
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

/// A language parameter: absent or empty is `default`, an unknown code is a
/// 400, as on the command line.
fn lang_param(body: &Value, url: &str, key: &str, default: LangId) -> Result<LangId, Reply> {
    match field(body, url, key).filter(|c| !c.trim().is_empty()) {
        None => Ok(default),
        Some(code) => LangId::from_code(code.trim()).ok_or_else(|| {
            Reply::error(400, format!("unknown language {code:?} for {key} (codes like en, ru, zh-CN, auto)"))
        }),
    }
}

/// A boolean parameter, from the JSON body (`true`, or a string) or the
/// query string (`back=1`, `slow=true`).
fn bool_param(body: &Value, url: &str, key: &str) -> Result<Option<bool>, Reply> {
    let text = match body.get(key) {
        Some(Value::Bool(b)) => return Ok(Some(*b)),
        Some(Value::Null) | None => query_param(url, key),
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };
    match text.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        None => Ok(None),
        // `?back` alone means yes, as a flag would.
        Some(s) if matches!(s.as_str(), "" | "1" | "true" | "yes" | "on") => Ok(Some(true)),
        Some(s) if matches!(s.as_str(), "0" | "false" | "no" | "off") => Ok(Some(false)),
        Some(s) => Err(Reply::error(400, format!("{key} must be true or false, not {s:?}"))),
    }
}

/// A service id the request names, checked like the command line's `-s`.
fn service_param(
    engine: &Engine,
    body: &Value,
    url: &str,
    keys: &[&str],
    cap: Capabilities,
) -> Result<Option<String>, Reply> {
    let Some(id) = keys.iter().find_map(|k| field(body, url, k)).filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    engine.require_provider(id.trim(), cap).map_err(|msg| Reply::error(400, msg))?;
    Ok(Some(id.trim().to_owned()))
}

/// `?` for handlers that build a [`Reply`] either way.
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(reply) => return reply,
        }
    };
}

/// Headers only a browser sends. `Origin` comes with scripts' requests and
/// form posts (`null` from sandboxed frames and `file:` pages, which is no
/// less a browser); the `Sec-Fetch-*` metadata comes with every request a
/// current browser makes, an `<img>` or a link included, which is what an
/// `Origin` check alone misses; `Referer` covers older browsers that send
/// neither. Command-line tools and scripts send none of them.
const BROWSER_HEADERS: &[&str] =
    &["Origin", "Sec-Fetch-Site", "Sec-Fetch-Mode", "Sec-Fetch-Dest", "Sec-Fetch-User", "Referer"];

fn from_a_browser(req: &Request) -> bool {
    BROWSER_HEADERS.iter().any(|h| header(req, h).is_some())
}

/// Whether the `Host` header names this machine by address or as
/// `localhost` (with any port). A page that rebinds its own domain name to
/// 127.0.0.1 (DNS rebinding) still sends that domain as the host, so it is
/// refused here even if it slipped past the browser checks. A server
/// listening beyond loopback (`bind = "0.0.0.0:…"`, token required) is
/// reached by its network address, so any IP address is accepted there,
/// but still never a domain name. No `Host` at all is accepted: browsers
/// always send one.
fn host_allowed(host: Option<&str>, listening_beyond_loopback: bool) -> bool {
    let Some(host) = host.map(str::trim) else { return true };
    // Strip the port: `[::1]:60828`, `127.0.0.1:60828`, `localhost:60828`.
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inside, after)) if after.is_empty() || after.starts_with(':') => inside,
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => name,
            Some(_) => return false,
            None => host,
        }
    };
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match name.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback() || listening_beyond_loopback,
        Err(_) => false,
    }
}

/// `/v1/assets/ID` (GET: the state) and `/v1/assets/ID/download`
/// (`?names=a,b` for optional files), `/cancel`, `/verify`, `/remove`
/// (POST). A download runs in the
/// background: the reply is the state as it starts, and polling the GET
/// shows its progress.
fn assets_route(engine: &Engine, rest: &str, url: &str) -> Reply {
    let (id, op) = rest.split_once('/').unwrap_or((rest, ""));
    // `?names=a,b` picks optional files; without it, the required ones.
    let names: Vec<String> = query_param(url, "names")
        .map(|n| n.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    let result = match op {
        "" => engine.module_assets(id),
        "download" => engine.start_asset_download(id, &names).and_then(|()| engine.module_assets(id)),
        "cancel" => {
            engine.cancel_asset_download(id);
            engine.module_assets(id)
        }
        "verify" => engine.verify_module_assets(id),
        "remove" => engine.remove_module_assets(id).and_then(|()| engine.module_assets(id)),
        other => return Reply::error(404, format!("no assets operation {other:?} (download, cancel, verify, remove)")),
    };
    match result {
        Ok(report) => Reply::json(if op == "download" { 202 } else { 200 }, json!(report)),
        Err(e) => Reply::from_error(&e),
    }
}

/// `/v1/python/install` and `/cancel` (POST) for the managed Python and `uv`
/// themselves; `/v1/python/ID` (GET: a module's state), `/v1/python/ID/install`
/// and `/cancel` (POST). An install runs in the background: the reply is
/// the state as it starts, and polling the GET shows its progress.
fn python_route(engine: &Engine, rest: &str) -> Reply {
    let shared = |op: &str| -> Reply {
        let started = match op {
            "install" => engine.start_python_install(None),
            _ => {
                engine.cancel_python_install(crate::PYTHON_JOB);
                Ok(())
            }
        };
        match started {
            Ok(()) => Reply::json(if op == "install" { 202 } else { 200 }, json!(engine.python_status())),
            Err(e) => Reply::from_error(&e),
        }
    };
    let (id, op) = match rest.split_once('/') {
        None if rest == "install" || rest == "cancel" => return shared(rest),
        None => (rest, ""),
        Some(pair) => pair,
    };
    let started = match op {
        "" => Ok(()),
        "install" => engine.start_python_install(Some(id)),
        "cancel" => {
            engine.cancel_python_install(id);
            Ok(())
        }
        other => return Reply::error(404, format!("no python operation {other:?} (install, cancel)")),
    };
    if let Err(e) = started {
        return Reply::from_error(&e);
    }
    match engine.module_python(id) {
        Some(m) => Reply::json(if op == "install" { 202 } else { 200 }, json!(m)),
        None => Reply::error(404, format!("no Python module {id:?}")),
    }
}

fn route(engine: &Engine, req: &mut Request) -> Reply {
    if from_a_browser(req) {
        return Reply::error(403, "browser requests are not accepted");
    }
    let beyond_loopback = engine.config().server.bind.parse::<SocketAddr>().is_ok_and(|a| !a.ip().is_loopback());
    if !host_allowed(header(req, "Host"), beyond_loopback) {
        return Reply::error(403, "the Host header must be 127.0.0.1, localhost or [::1]");
    }
    let url = req.url().to_owned();
    let path = url.split('?').next().unwrap_or("/").trim_end_matches('/').to_owned();
    let method = req.method().clone();

    if path.is_empty() || path == "/health" {
        return Reply::json(
            200,
            json!({ "name": "glossa", "version": glossa_core::VERSION, "commit": glossa_core::COMMIT }),
        );
    }
    if !authorised(engine, req) && !pot_route_open(engine, req, &path) {
        return Reply::error(401, "missing or wrong token");
    }
    if let Some(allow) = allowed_methods(&path) {
        if !method_allowed(&method, allow) {
            let mut reply = Reply::error(405, format!("{path} answers {allow}, not {}", method.as_str()));
            reply.allow = Some(allow);
            return reply;
        }
    }

    match path.as_str() {
        "/v1/services" => {
            let cap = match query_param(&url, "capability").filter(|c| !c.trim().is_empty()) {
                None => Capabilities::TRANSLATE,
                Some(c) => match Capabilities::from_name(c.trim()) {
                    Some(cap) => cap,
                    None => {
                        return Reply::error(
                            400,
                            format!(
                                "unknown capability {c:?}: translate, dictionary, listen, ocr, detect_language, \
                                 speech_input, batch, alternatives, promptable, offline"
                            ),
                        )
                    }
                },
            };
            let list: Vec<Value> = engine
                .providers(cap)
                .into_iter()
                .map(|p| {
                    json!({ "id": p.info.id, "name": p.info.name, "origin": p.origin, "enabled": p.enabled,
                            "capabilities": p.info.capabilities.names(),
                            "needs_credentials": p.info.needs_credentials })
                })
                .collect();
            Reply::json(200, Value::Array(list))
        }
        "/v1/translate" => {
            let body = tri!(json_body(req));
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let pair = LangPair::new(
                tri!(lang_param(&body, &url, "from", LangId::AUTO)),
                tri!(lang_param(&body, &url, "to", LangId::AUTO)),
            );
            let opts = TranslateOptions {
                provider: tri!(service_param(engine, &body, &url, &["service"], Capabilities::TRANSLATE)),
                back_translate: tri!(bool_param(&body, &url, "back")).unwrap_or(false),
                ..Default::default()
            };
            let save = tri!(bool_param(&body, &url, "history")).unwrap_or(false);
            match engine.translate(&text, pair, &opts) {
                Ok(out) => {
                    if save && engine.record_translation(&out) {
                        if let Err(e) = engine.save_history() {
                            tracing::warn!("history not saved: {e}");
                        }
                    }
                    Reply::json(200, serde_json::to_value(out.report()).unwrap_or_default())
                }
                Err(e) => Reply::from_error(&e),
            }
        }
        "/v1/detect" => {
            let body = tri!(json_body(req));
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            match engine.detect(&text) {
                Ok(d) => Reply::json(
                    200,
                    json!({ "lang": d.lang.bcp47(), "name": d.lang.english_name(), "confidence": d.confidence }),
                ),
                Err(e) => Reply::from_error(&e),
            }
        }
        "/v1/dictionary" => {
            let body = tri!(json_body(req));
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let pair = LangPair::new(
                tri!(lang_param(&body, &url, "from", LangId::AUTO)),
                tri!(lang_param(&body, &url, "to", LangId::AUTO)),
            );
            let service = tri!(service_param(engine, &body, &url, &["service"], Capabilities::DICTIONARY));
            match engine.dictionary(&text, pair, service.as_deref()) {
                Ok(a) => Reply::json(200, serde_json::to_value(&a).unwrap_or_default()),
                Err(e) => Reply::from_error(&e),
            }
        }
        "/v1/ocr" => {
            let is_image = header(req, "Content-Type")
                .is_some_and(|c| c.starts_with("image/") || c.starts_with("application/octet-stream"));
            let body = if is_image { json!({}) } else { tri!(json_body(req)) };
            let image = if is_image {
                tri!(read_body(req))
            } else {
                let Some(b64) = body.get("image_base64").and_then(|v| v.as_str()) else {
                    return Reply::error(400, "send image bytes or {\"image_base64\": …}");
                };
                match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(bytes) => bytes,
                    Err(e) => return Reply::error(400, format!("image_base64: {e}")),
                }
            };
            let lang = tri!(lang_param(&body, &url, "lang", LangId::AUTO));
            let to = match field(&body, &url, "translate_to").filter(|c| !c.trim().is_empty()) {
                None => None,
                Some(_) => Some(tri!(lang_param(&body, &url, "translate_to", LangId::AUTO))),
            };
            // `engine` as on the command line; `service` is the older name.
            let service = tri!(service_param(engine, &body, &url, &["engine", "service"], Capabilities::OCR));
            if let Some(name) = field(&body, &url, "format") {
                use glossa_core::ocr::export::{self, OcrFormat};
                let Some(format) = OcrFormat::from_name(&name) else {
                    return Reply::error(400, format!("unknown format {name:?}: text, json, hocr or alto"));
                };
                let granularity = if format.wants_structure() {
                    glossa_core::ocr::OcrGranularity::Words
                } else {
                    glossa_core::ocr::OcrGranularity::FullText
                };
                return match engine.recognize_detailed(image, lang, to, service.as_deref(), granularity) {
                    Ok(out) => {
                        let mut ocr = out.ocr.clone();
                        if let Some(t) = out.translated_text() {
                            ocr.translation = Some(t.to_owned());
                        }
                        let body = match format {
                            OcrFormat::Text => ocr.translation.clone().unwrap_or_else(|| export::text(&ocr)),
                            OcrFormat::Json => export::json(&ocr).to_string(),
                            other => export::render(&ocr, other),
                        };
                        Reply::bytes(200, body.into_bytes(), format.mime())
                    }
                    Err(e) => Reply::from_ocr_error(&e),
                };
            }
            let in_place = tri!(bool_param(&body, &url, "in_place")).unwrap_or(false);
            let result = match (in_place, service.as_deref()) {
                (true, service) => {
                    let to = to.unwrap_or(engine.config().general.first_language);
                    engine.recognize_in_place(image, lang, to, service)
                }
                (false, Some(id)) => engine.recognize_using(id, image, lang, to),
                (false, None) => engine.recognize(image, lang, to),
            };
            match result {
                Ok(out) => Reply::json(200, ocr_summary(&out)),
                Err(e) => Reply::from_ocr_error(&e),
            }
        }
        "/v1/speak" => {
            let body = tri!(json_body(req));
            let Some(text) = field(&body, &url, "text") else { return Reply::error(400, "text is required") };
            let slow = tri!(bool_param(&body, &url, "slow"));
            let lang = tri!(lang_param(&body, &url, "lang", LangId::AUTO));
            match engine.speak(&text, lang, slow) {
                Ok(audio) => {
                    Reply::bytes(200, audio.bytes, if audio.mime == "audio/wav" { "audio/wav" } else { "audio/mpeg" })
                }
                Err(e) => Reply::from_error(&e),
            }
        }
        p if p.starts_with("/v1/action/") => {
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
        "/selection_translate" => pot_action(engine, Action::ShowPopup, None),
        "/input_translate" => pot_action(engine, Action::ShowMainWindow, None),
        "/ocr_recognize" => pot_action(engine, Action::TextRecognition, None),
        "/ocr_translate" => pot_action(engine, Action::RecognizeAndTranslate, None),
        "/translate" => match read_body(req) {
            Ok(b) => pot_action(engine, Action::ShowPopup, Some(String::from_utf8_lossy(&b).into_owned())),
            Err(r) => r,
        },
        "/v1/assets" => Reply::json(200, json!(engine.modules_with_assets())),
        p if p.starts_with("/v1/assets/") => assets_route(engine, &p["/v1/assets/".len()..], &url),
        "/v1/python" => Reply::json(200, json!(engine.python_status())),
        p if p.starts_with("/v1/python/") => python_route(engine, &p["/v1/python/".len()..]),
        "/v1/presets" => Reply::json(200, presets_list(engine)),
        p if p.starts_with("/v1/presets/") => presets_route(engine, req, &p["/v1/presets/".len()..], &url),
        _ => Reply::error(404, format!("no route {path}")),
    }
}

/// `GET /v1/presets`: every preset and the current one's name.
pub fn presets_list(engine: &Engine) -> Value {
    let presets = engine.presets();
    let current = presets.iter().find(|p| p.current).map(|p| p.name.clone());
    json!({ "current": current, "presets": presets })
}

/// `/v1/presets/…`: export (GET), and apply, next, save, import, rename,
/// duplicate, delete (POST). Names come in the JSON body or the query.
fn presets_route(engine: &Engine, req: &mut Request, op: &str, url: &str) -> Reply {
    if op == "import" {
        let body = tri!(read_body(req));
        return match engine.import_preset_text(&String::from_utf8_lossy(&body)) {
            Ok(report) => Reply::json(200, json!(report)),
            Err(e) => Reply::from_error(&e),
        };
    }
    let body = tri!(json_body(req));
    if op == "next" {
        return match engine.next_preset() {
            Ok(report) => Reply::json(200, json!(report)),
            Err(e) => Reply::from_error(&e),
        };
    }
    let Some(name) = field(&body, url, "name").filter(|n| !n.trim().is_empty()) else {
        return Reply::error(400, "name is required");
    };
    if op != "save" && engine.preset(&name).is_none() {
        return Reply::error(404, format!("no preset called {name:?}"));
    }
    let new_name = field(&body, url, "new_name");
    let result = match op {
        "export" => {
            return match engine.export_preset_text(&name) {
                Ok(text) => Reply::bytes(200, text.into_bytes(), "application/toml; charset=utf-8"),
                Err(e) => Reply::from_error(&e),
            }
        }
        "apply" => engine.apply_preset(&name).map(|r| json!(r)),
        "save" => {
            let with_pair = tri!(bool_param(&body, url, "languages")).unwrap_or(false);
            engine.save_preset(&name, with_pair).map(|r| json!(r))
        }
        "rename" => match new_name {
            Some(n) => engine.rename_preset(&name, &n).map(|()| json!({ "name": n.trim() })),
            None => return Reply::error(400, "new_name is required"),
        },
        "duplicate" => engine.duplicate_preset(&name, new_name.as_deref()).map(|n| json!({ "name": n })),
        "delete" => engine.delete_preset(&name).map(|()| json!({ "deleted": name })),
        other => {
            return Reply::error(
                404,
                format!(
                    "no presets operation {other:?} (apply, next, save, import, export, rename, duplicate, delete)"
                ),
            )
        }
    };
    match result {
        Ok(v) => Reply::json(200, v),
        Err(e) => Reply::from_error(&e),
    }
}

/// `/v1/ocr`'s JSON answer, the same shape as `glossa-cli --json ocr`.
pub fn ocr_summary(out: &crate::OcrOutcome) -> Value {
    json!({
        "text": out.ocr.text,
        "translation": out.translated_text(),
        "detected": out.ocr.detected_lang.map(|l| l.bcp47()),
        "service": out.ocr.provider_id,
        "elapsed_ms": out.ocr.elapsed_ms,
        // Who translated: `lens` when Google Lens translated the picture itself.
        "translator": out.translation.as_ref().map(|t| t.translation.provider_id.clone()),
        "image_size": out.ocr.image_size,
        // Filled for an in-place translation: each block with its place.
        "blocks": out.blocks,
    })
}

fn pot_action(engine: &Engine, action: Action, text: Option<String>) -> Reply {
    engine.request_action(action, text);
    Reply::json(202, json!({ "accepted": action.id() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use glossa_core::{Config, History, Paths};
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
    fn module_files_are_listed_and_unknown_modules_are_404() {
        let (e, dir) = engine();
        let server = start(e.clone()).unwrap();
        let get = |path: &str, method: &str| {
            request(
                server.addr,
                &format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            )
        };
        assert_eq!(get("/v1/assets", "GET"), (200, "[]".into()));
        assert_eq!(get("/v1/assets/nope", "GET").0, 404);
        assert_eq!(get("/v1/assets/nope/download", "POST").0, 404);
        assert_eq!(get("/v1/assets/nope/download", "GET").0, 405);
        assert_eq!(get("/v1/assets/nope/teleport", "POST").0, 404);
        assert_eq!(get("/v1/python", "GET").0, 200);
        assert_eq!(get("/v1/python/nope", "GET").0, 404);
        assert_eq!(get("/v1/python/nope/install", "POST").0, 404);
        assert_eq!(get("/v1/python/nope/install", "GET").0, 405);
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn presets_need_the_token_and_can_be_saved_applied_and_exported() {
        let (e, dir) = engine();
        let server = start(e.clone()).unwrap();
        let call = |method: &str, path: &str, auth: bool, body: &str| {
            let auth = if auth { "Authorization: Bearer secret\r\n" } else { "" };
            request(
                server.addr,
                &format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
        };
        assert_eq!(call("GET", "/v1/presets", false, "").0, 401);
        assert_eq!(call("POST", "/v1/presets/apply", false, r#"{"name":"Private"}"#).0, 401);
        let (status, body) = call("GET", "/v1/presets", true, "");
        assert_eq!(status, 200);
        assert!(body.contains("\"Everyday\""), "{body}");
        assert_eq!(call("POST", "/v1/presets/apply", true, r#"{"name":"Private"}"#).0, 200);
        assert_eq!(e.config().services.preset, "Private");
        assert_eq!(call("POST", "/v1/presets/apply", true, r#"{"name":"Nope"}"#).0, 404);
        assert_eq!(call("POST", "/v1/presets/save", true, r#"{"name":"Mine","languages":true}"#).0, 200);
        let (status, text) = call("GET", "/v1/presets/export?name=Mine", true, "");
        assert_eq!(status, 200);
        assert!(text.contains("glossa_preset = 1") && text.contains("name = \"Mine\""), "{text}");
        assert_eq!(call("GET", "/v1/presets/apply", true, "").0, 405);
        let (status, body) = call("POST", "/v1/presets/import", true, &text);
        assert_eq!(status, 200);
        assert!(body.contains("Mine (2)"), "{body}");
        server.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn health_is_open_and_everything_else_needs_the_token() {
        let (e, dir) = engine();
        let server = start(e.clone()).unwrap();
        let (status, body) =
            request(server.addr, "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("\"glossa\""));
        assert!(body.contains("\"commit\""), "{body}");

        let (status, _) =
            request(server.addr, "GET /v1/services HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert_eq!(status, 401);
        let (status, body) = request(
            server.addr,
            "GET /v1/services?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 200);
        assert!(body.contains("\"google\""));
        let (status, _) = request(
            server.addr,
            "GET /v1/services HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n",
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
            "GET /v1/services?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
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
            "GET /v1/action/show_popup?token=secret&text=hello%20world HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 202);
        let got = (0..50).find_map(|_| match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Action { action, text }) => Some((action, text)),
            _ => None,
        });
        assert_eq!(got, Some((Action::ShowPopup, Some("hello world".into()))));
        let (status, _) = request(
            server.addr,
            "GET /v1/action/self_destruct?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 404);
        // pot-app's endpoint maps onto the same action.
        let (status, _) = request(
            server.addr,
            "GET /selection_translate?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
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
            &format!("POST /v1/translate?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
        );
        assert_eq!(status, 400);
        assert!(resp.contains("invalid JSON"));
        let (status, _) = request(
            server.addr,
            "POST /v1/translate?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        );
        assert_eq!(status, 400, "text is required");
        let body = r#"{"image_base64":"iVBORw0KGgo=","format":"pdf"}"#;
        let (status, resp) = request(
            server.addr,
            &format!("POST /v1/ocr?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
        );
        assert_eq!(status, 400);
        assert!(resp.contains("unknown format"), "{resp}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole answer, headers included.
    fn raw(addr: SocketAddr, raw: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    /// A request with a JSON body (or none).
    fn call(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
        request(
            addr,
            &format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn exposing_beyond_loopback_needs_a_token() {
        assert!(matches!(checked_bind("0.0.0.0:60828", ""), Err(Error::Config(_))));
        assert!(checked_bind("0.0.0.0:60828", "t").is_ok());
        assert!(checked_bind("127.0.0.1:60828", "").is_ok());
        assert!(matches!(checked_bind("localhost", "t"), Err(Error::Config(_))));
    }

    #[test]
    fn clients_connect_to_loopback_when_the_server_listens_everywhere() {
        assert_eq!(connect_addr("0.0.0.0:60828"), "127.0.0.1:60828");
        assert_eq!(connect_addr("[::]:60828"), "[::1]:60828");
        assert_eq!(connect_addr("127.0.0.1:60828"), "127.0.0.1:60828");
        assert_eq!(connect_addr("192.168.1.5:9000"), "192.168.1.5:9000");
    }

    #[test]
    fn null_origins_are_browsers_too() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let (status, _) = request(
            server.addr,
            "GET /v1/services?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: null\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 403);
        // Not even pot-app's open routes take a browser request.
        let (status, _) = request(
            server.addr,
            "GET /selection_translate HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: null\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 403);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anything_a_browser_sends_is_refused_even_without_an_origin() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        // What an `<img src=http://127.0.0.1:60828/selection_translate>` on
        // a page sends: no Origin, but fetch metadata.
        for extra in [
            "Sec-Fetch-Site: cross-site\r\n",
            "Sec-Fetch-Mode: no-cors\r\n",
            "Sec-Fetch-Dest: image\r\n",
            "Sec-Fetch-User: ?1\r\n",
            "Referer: https://evil.example/page\r\n",
        ] {
            let (status, _) = request(
                server.addr,
                &format!("GET /selection_translate HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Connection: close\r\n\r\n"),
            );
            assert_eq!(status, 403, "{extra}");
            // The token does not make a browser request acceptable either.
            let (status, _) = request(
                server.addr,
                &format!(
                    "GET /v1/services?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Connection: close\r\n\r\n"
                ),
            );
            assert_eq!(status, 403, "{extra}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_host_must_name_this_machine() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let port = server.addr.port();
        for host in [
            "127.0.0.1".to_owned(),
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            "LOCALHOST".into(),
            format!("[::1]:{port}"),
        ] {
            let (status, _) = request(
                server.addr,
                &format!("GET /v1/services?token=secret HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
            );
            assert_eq!(status, 200, "{host}");
        }
        // A rebinding page sends its own name; health included.
        for path in ["/health", "/selection_translate", "/v1/services?token=secret"] {
            let (status, _) = request(
                server.addr,
                &format!("GET {path} HTTP/1.1\r\nHost: evil.example:{port}\r\nConnection: close\r\n\r\n"),
            );
            assert_eq!(status, 403, "{path}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn host_names_are_parsed_strictly() {
        for ok in ["127.0.0.1", "127.0.0.1:60828", "localhost", "localhost:1", "[::1]", "[::1]:60828", "127.5.6.7"] {
            assert!(host_allowed(Some(ok), false), "{ok}");
        }
        for bad in [
            "evil.example",
            "localhost.evil.example",
            "127.0.0.1.nip.io",
            "192.168.1.5:60828",
            "[::1]x",
            "127.0.0.1:port",
            "::1",
            "",
        ] {
            assert!(!host_allowed(Some(bad), false), "{bad}");
        }
        assert!(host_allowed(None, false), "no Host: not a browser");
        // Listening on the network: its address is fine, a name still is not.
        assert!(host_allowed(Some("192.168.1.5:60828"), true));
        assert!(host_allowed(Some("[fe80::1]:60828"), true));
        assert!(!host_allowed(Some("glossa.lan:60828"), true));
    }

    #[test]
    fn pot_app_routes_work_without_the_token_from_this_machine_only_when_allowed() {
        let (e, dir) = engine();
        let server = start(e.clone()).unwrap();
        for path in ["/selection_translate", "/input_translate", "/ocr_recognize", "/ocr_translate"] {
            assert_eq!(call(server.addr, "GET", path, "").0, 202, "{path}");
        }
        assert_eq!(call(server.addr, "POST", "/translate", "hello").0, 202);
        // Only those routes.
        assert_eq!(call(server.addr, "GET", "/v1/action/show_popup", "").0, 401);
        assert_eq!(call(server.addr, "GET", "/v1/services", "").0, 401);

        e.update_config(|c| c.server.pot_compat_without_token = false).unwrap();
        assert_eq!(call(server.addr, "GET", "/selection_translate", "").0, 401);
        assert_eq!(call(server.addr, "GET", "/selection_translate?token=secret", "").0, 202);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_wrong_method_is_405_with_allow() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let out = raw(
            server.addr,
            "DELETE /v1/translate?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(out.starts_with("HTTP/1.1 405"), "{out}");
        assert!(out.lines().any(|l| l.eq_ignore_ascii_case("allow: GET, POST")), "{out}");
        let out =
            raw(server.addr, "GET /v1/speak?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 405") && out.contains("Allow: POST"), "{out}");
        let out =
            raw(server.addr, "GET /translate?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 405"), "{out}");
        assert_eq!(call(server.addr, "GET", "/v1/nothing?token=secret", "").0, 404);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_languages_capabilities_and_services_are_refused() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let (status, body) = call(server.addr, "GET", "/v1/translate?token=secret&text=hi&to=klingon", "");
        assert_eq!(status, 400);
        assert!(body.contains("unknown language") && body.contains("klingon"), "{body}");
        let (status, _) = call(server.addr, "POST", "/v1/translate?token=secret", r#"{"text":"hi","from":"xx-YY"}"#);
        assert_eq!(status, 400);
        let (status, body) = call(server.addr, "GET", "/v1/services?token=secret&capability=teleport", "");
        assert_eq!(status, 400);
        assert!(body.contains("unknown capability"), "{body}");
        let (status, body) =
            call(server.addr, "POST", "/v1/translate?token=secret", r#"{"text":"hi","service":"gogle"}"#);
        assert_eq!(status, 400);
        assert!(body.contains("unknown service") && body.contains("google"), "{body}");
        let (status, _) = call(server.addr, "GET", "/v1/dictionary?token=secret&text=hi&service=google", "");
        assert_eq!(status, 400, "google is a translator, not a dictionary");
        let (status, body) = call(server.addr, "POST", "/v1/translate?token=secret&back=maybe", r#"{"text":"hi"}"#);
        assert_eq!(status, 400);
        assert!(body.contains("back must be true or false"), "{body}");
        let body = r#"{"image_base64":"iVBORw0KGgo=","engine":"nope"}"#;
        assert_eq!(call(server.addr, "POST", "/v1/ocr?token=secret", body).0, 400);
        let body = r#"{"image_base64":"iVBORw0KGgo=","translate_to":"elvish"}"#;
        assert_eq!(call(server.addr, "POST", "/v1/ocr?token=secret", body).0, 400);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn services_say_which_need_credentials() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let (status, body) = call(server.addr, "GET", "/v1/services?token=secret", "");
        assert_eq!(status, 200);
        let list: Value = serde_json::from_str(&body).unwrap();
        let deepl = list.as_array().unwrap().iter().find(|p| p["id"] == "deepl").unwrap();
        assert_eq!(deepl["needs_credentials"], true);
        let google = list.as_array().unwrap().iter().find(|p| p["id"] == "google").unwrap();
        assert_eq!(google["needs_credentials"], false);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ocr_engine_that_is_not_set_up_is_a_503() {
        let (e, dir) = engine();
        e.update_config(|c| c.ocr.tesseract.path = dir.join("no-tesseract").to_string_lossy().into_owned()).unwrap();
        let server = start(e).unwrap();
        let body = r#"{"image_base64":"iVBORw0KGgo=","engine":"tesseract"}"#;
        let (status, resp) = call(server.addr, "POST", "/v1/ocr?token=secret", body);
        assert_eq!(status, 503, "{resp}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn translations_have_the_shared_shape_and_can_be_recorded() {
        let (e, dir) = engine();
        // A translator that needs no network: echoes its input.
        let m = e.paths().user_module_dir().join("echo");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"echo\"\nname = \"Echo\"\nruntime = \"js\"\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return 'echo: ' + req.text; }").unwrap();
        e.reload_modules();
        let server = start(e.clone()).unwrap();

        let (status, body) =
            call(server.addr, "GET", "/v1/translate?token=secret&text=hi&from=en&to=ru&service=echo&back=1", "");
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        for key in [
            "text",
            "from",
            "to",
            "service",
            "romanization",
            "source_romanization",
            "alternatives",
            "back",
            "failed",
            "elapsed_ms",
        ] {
            assert!(v.get(key).is_some(), "{key} missing from {body}");
        }
        assert_eq!(v["text"], "echo: hi");
        assert_eq!(v["back"], "echo: echo: hi", "back accepted from the query string");
        assert!(e.history().is_empty(), "not recorded unless asked");

        let (status, _) = call(
            server.addr,
            "POST",
            "/v1/translate?token=secret",
            r#"{"text":"hi","from":"en","to":"ru","service":"echo","history":true}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(e.history().len(), 1);
        assert_eq!(History::load(&e.paths().history_file(), 100).unwrap().len(), 1, "saved at once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn booleans_read_from_body_or_query() {
        let body = json!({ "back": true, "slow": "no" });
        assert_eq!(bool_param(&body, "/x", "back").ok().flatten(), Some(true));
        assert_eq!(bool_param(&body, "/x", "slow").ok().flatten(), Some(false));
        assert_eq!(bool_param(&json!({}), "/x?slow=1", "slow").ok().flatten(), Some(true));
        assert_eq!(bool_param(&json!({}), "/x?slow=false", "slow").ok().flatten(), Some(false));
        assert_eq!(bool_param(&json!({}), "/x?back", "back").ok().flatten(), Some(true));
        assert_eq!(bool_param(&json!({}), "/x", "back").ok().flatten(), None);
        assert!(bool_param(&json!({}), "/x?back=perhaps", "back").is_err());
    }

    #[test]
    fn query_strings_decode() {
        assert_eq!(query_param("/x?text=a%20b+c&to=ru", "text").as_deref(), Some("a b c"));
        assert_eq!(query_param("/x?text=%D0%BF%D1%80", "text").as_deref(), Some("пр"));
        assert_eq!(query_param("/x?a=1", "b"), None);
        assert_eq!(percent_decode("100%"), "100%");
    }

    /// The next action event the front end would get, if one comes.
    fn next_action(rx: &std::sync::mpsc::Receiver<Event>) -> Option<(Action, Option<String>)> {
        (0..50).find_map(|_| match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Action { action, text }) => Some((action, text)),
            _ => None,
        })
    }

    #[test]
    fn a_wrong_or_malformed_token_is_refused() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        for (auth, query) in [
            ("Authorization: Bearer wrong\r\n", ""),
            ("Authorization: Bearer secretx\r\n", ""),
            ("Authorization: Bearer secre\r\n", ""),
            ("Authorization: Bearer SECRET\r\n", ""),
            ("Authorization: Basic secret\r\n", ""),
            ("Authorization: secret\r\n", ""),
            ("Authorization: Bearer \r\n", ""),
            ("", "?token="),
            ("", "?token=wrong"),
            ("", "?tokens=secret"),
            ("", "?xtoken=secret"),
        ] {
            let (status, body) = request(
                server.addr,
                &format!("GET /v1/services{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n"),
            );
            assert_eq!(status, 401, "{auth}{query}");
            assert!(body.contains("token"), "{body}");
        }
        // A wrong header does not spoil a right query string, nor the other
        // way round.
        let (status, _) = request(
            server.addr,
            "GET /v1/services?token=secret HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer wrong\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 200);
        let (status, _) = request(
            server.addr,
            "GET /v1/services?token=wrong HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(status, 200);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_token_is_checked_before_the_method_and_the_route() {
        // Without the token nothing about the API is revealed: not which
        // paths exist, nor which methods they take.
        let (e, dir) = engine();
        let server = start(e).unwrap();
        assert_eq!(call(server.addr, "DELETE", "/v1/translate", "").0, 401);
        assert_eq!(call(server.addr, "GET", "/v1/nothing", "").0, 401);
        assert_eq!(call(server.addr, "POST", "/v1/translate", "{not json").0, 401);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn health_answers_at_the_root_and_with_a_trailing_slash() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        for path in ["/", "/health", "/health/", "/health?x=1"] {
            let (status, body) = call(server.addr, "GET", path, "");
            assert_eq!(status, 200, "{path}");
            let v: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["name"], "glossa");
            assert_eq!(v["version"], glossa_core::VERSION);
        }
        // A trailing slash reaches the real route too.
        assert_eq!(call(server.addr, "GET", "/v1/services/?token=secret", "").0, 200);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_endpoint_that_needs_text_says_so() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        for path in ["/v1/translate", "/v1/detect", "/v1/dictionary", "/v1/speak"] {
            let (status, body) = call(server.addr, "POST", &format!("{path}?token=secret"), "{}");
            assert_eq!(status, 400, "{path}");
            assert!(body.contains("text is required"), "{path}: {body}");
            // A body that is not JSON is told apart from a missing field.
            let (status, body) = call(server.addr, "POST", &format!("{path}?token=secret"), "[1,");
            assert_eq!(status, 400, "{path}");
            assert!(body.contains("invalid JSON"), "{path}: {body}");
        }
        // The answer is JSON with an `error` field, which clients parse.
        let (_, body) = call(server.addr, "POST", "/v1/detect?token=secret", "{}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].is_string(), "{body}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ocr_requests_without_a_usable_image_are_400() {
        let (e, dir) = engine();
        let server = start(e).unwrap();
        let (status, body) = call(server.addr, "POST", "/v1/ocr?token=secret", "{}");
        assert_eq!(status, 400);
        assert!(body.contains("image_base64"), "{body}");
        let (status, body) = call(server.addr, "POST", "/v1/ocr?token=secret", r#"{"image_base64":"%%%"}"#);
        assert_eq!(status, 400);
        assert!(body.contains("image_base64"), "{body}");
        let (status, _) = call(server.addr, "POST", "/v1/ocr?token=secret", r#"{"image_base64":7}"#);
        assert_eq!(status, 400, "not a string");
        let (status, _) = call(server.addr, "POST", "/v1/ocr?token=secret", "nope");
        assert_eq!(status, 400, "neither an image type nor JSON");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn actions_take_their_text_from_a_post_body() {
        let (e, dir) = engine();
        let rx = e.subscribe();
        let server = start(e).unwrap();
        let (status, body) = call(server.addr, "POST", "/v1/action/show_popup?token=secret", "из тела");
        assert_eq!(status, 202);
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["accepted"], "show_popup");
        assert_eq!(next_action(&rx), Some((Action::ShowPopup, Some("из тела".into()))));
        // The query string wins over the body; no text at all is `None`.
        call(server.addr, "POST", "/v1/action/show_popup?token=secret&text=query", "body");
        assert_eq!(next_action(&rx), Some((Action::ShowPopup, Some("query".into()))));
        call(server.addr, "GET", "/v1/action/show_main_window?token=secret", "");
        assert_eq!(next_action(&rx), Some((Action::ShowMainWindow, None)));
        // A wrong method on an action is a 405, not an unknown action.
        assert_eq!(call(server.addr, "PUT", "/v1/action/show_popup?token=secret", "").0, 405);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pot_app_routes_map_onto_their_actions() {
        let (e, dir) = engine();
        let rx = e.subscribe();
        let server = start(e).unwrap();
        for (path, action) in [
            ("/selection_translate", Action::ShowPopup),
            ("/input_translate", Action::ShowMainWindow),
            ("/ocr_recognize", Action::TextRecognition),
            ("/ocr_translate", Action::RecognizeAndTranslate),
        ] {
            assert_eq!(call(server.addr, "GET", path, "").0, 202, "{path}");
            assert_eq!(next_action(&rx), Some((action, None)), "{path}");
        }
        // pot-app's `/translate` carries the text as the plain body.
        assert_eq!(call(server.addr, "POST", "/translate", "plain text body").0, 202);
        assert_eq!(next_action(&rx), Some((Action::ShowPopup, Some("plain text body".into()))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn method_lists_match_whole_names() {
        assert!(method_allowed(&Method::Get, "GET, POST"));
        assert!(method_allowed(&Method::Post, "GET, POST"));
        assert!(!method_allowed(&Method::Put, "GET, POST"));
        assert!(!method_allowed(&Method::Get, "POST"));
        assert_eq!(allowed_methods("/v1/action/anything"), Some("GET, POST"));
        assert_eq!(allowed_methods("/health"), None, "open to every method");
        assert_eq!(allowed_methods("/selection_translate"), None, "pot-app's bindings use any method");
        assert_eq!(allowed_methods("/translate"), Some("POST"));
    }

    #[test]
    fn engine_errors_map_onto_the_right_status() {
        let p = || "p".to_owned();
        for (e, status) in [
            (Error::EmptyInput, 400),
            (Error::SameLanguage, 400),
            (Error::Config("x".into()), 400),
            (Error::NoProvider("translate"), 404),
            (Error::NoSuchModule("m".into()), 404),
            (Error::NoTextRecognised, 422),
            (Error::LanguageNotDetected, 422),
            (Error::MissingCredential { provider: p(), what: "key".into() }, 412),
            (Error::RateLimited { provider: p(), retry_after_secs: Some(3) }, 429),
            (Error::Timeout(10), 504),
            (Error::Internal("bug".into()), 500),
            (Error::Network("down".into()), 502),
            (Error::HttpStatus { provider: p(), status: 500, body: String::new() }, 502),
            (Error::BadResponse { provider: p(), detail: "x".into() }, 502),
        ] {
            let reply = Reply::from_error(&e);
            assert_eq!(reply.status, status, "{e:?}");
            let v: Value = serde_json::from_slice(&reply.body).unwrap();
            assert_eq!(v["error"], e.to_string(), "the message reaches the client");
        }
    }

    #[test]
    fn ocr_errors_blame_the_server_only_for_its_own_set_up() {
        assert_eq!(Reply::from_ocr_error(&Error::NoProvider("ocr")).status, 503);
        assert_eq!(Reply::from_ocr_error(&Error::Config("no lens".into())).status, 503);
        assert_eq!(Reply::from_ocr_error(&Error::FileNotFound("tesseract".into())).status, 503);
        let missing = Error::MissingCredential { provider: "p".into(), what: "key".into() };
        assert_eq!(Reply::from_ocr_error(&missing).status, 503);
        // What is wrong with the picture stays the request's fault.
        assert_eq!(Reply::from_ocr_error(&Error::NoTextRecognised).status, 422);
        assert_eq!(Reply::from_ocr_error(&Error::Timeout(5)).status, 504);
    }

    #[test]
    fn tokens_compare_whole_and_exact() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret "));
        assert!(!constant_time_eq("", "secret"));
        assert!(constant_time_eq("", ""), "the caller refuses an empty token itself");
    }

    #[test]
    fn languages_are_optional_but_never_guessed() {
        let body = json!({ "from": "  en ", "to": "" });
        assert_eq!(lang_param(&body, "/x", "from", LangId::AUTO).ok(), Some(LangId::ENGLISH), "trimmed");
        assert_eq!(lang_param(&body, "/x", "to", LangId::RUSSIAN).ok(), Some(LangId::RUSSIAN), "empty: default");
        assert_eq!(lang_param(&json!({}), "/x?to=ru", "to", LangId::AUTO).ok(), Some(LangId::RUSSIAN));
        assert_eq!(lang_param(&json!({}), "/x?to=auto", "to", LangId::ENGLISH).ok(), Some(LangId::AUTO));
        let err = lang_param(&json!({}), "/x?to=qq", "to", LangId::AUTO).err().unwrap();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn the_json_body_wins_over_the_query_string() {
        let body = json!({ "text": "from body", "n": 5 });
        assert_eq!(field(&body, "/x?text=from+query", "text").as_deref(), Some("from body"));
        assert_eq!(field(&json!({}), "/x?text=from+query", "text").as_deref(), Some("from query"));
        // A non-string field falls through to the query string rather than
        // being turned into text.
        assert_eq!(field(&body, "/x", "n"), None);
        assert_eq!(field(&body, "/x?n=6", "n").as_deref(), Some("6"));
    }

    #[test]
    fn booleans_from_json_numbers_and_nulls() {
        assert_eq!(bool_param(&json!({ "b": 1 }), "/x", "b").ok().flatten(), Some(true));
        assert_eq!(bool_param(&json!({ "b": 0 }), "/x", "b").ok().flatten(), Some(false));
        assert!(bool_param(&json!({ "b": 2 }), "/x", "b").is_err());
        assert!(bool_param(&json!({ "b": [true] }), "/x", "b").is_err());
        // JSON null is absent: the query string decides.
        assert_eq!(bool_param(&json!({ "b": null }), "/x?b=on", "b").ok().flatten(), Some(true));
        assert_eq!(bool_param(&json!({ "b": " YES " }), "/x", "b").ok().flatten(), Some(true));
        assert_eq!(bool_param(&json!({}), "/x?b=OFF", "b").ok().flatten(), Some(false));
    }

    #[test]
    fn query_parameters_match_whole_keys_and_tolerate_bad_escapes() {
        assert_eq!(query_param("/x?text", "text").as_deref(), Some(""), "a bare key is an empty value");
        assert_eq!(query_param("/x?subtext=a&text=b", "text").as_deref(), Some("b"));
        assert_eq!(query_param("/x?text=a&text=b", "text").as_deref(), Some("a"), "the first wins");
        assert_eq!(query_param("/x", "text"), None);
        assert_eq!(query_param("/x?a=1%3D2", "a").as_deref(), Some("1=2"));
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%41%42"), "AB");
        assert_eq!(percent_decode("%FF"), "\u{fffd}", "invalid UTF-8 is replaced, not a panic");
        assert_eq!(percent_decode("a%2Bb+c"), "a+b c", "an escaped plus stays a plus");
    }

    #[test]
    fn binding_needs_a_full_socket_address() {
        assert!(matches!(checked_bind("127.0.0.1", "t"), Err(Error::Config(_))), "a port is required");
        assert!(matches!(checked_bind("[::]:60828", ""), Err(Error::Config(_))), "every address, no token");
        assert!(checked_bind("[::1]:60828", "").is_ok());
        assert_eq!(connect_addr("not an address"), "not an address", "left for the caller to report");
    }
}
