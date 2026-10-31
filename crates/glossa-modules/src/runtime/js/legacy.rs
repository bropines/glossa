//! Classic QTranslate service scripts.
//!
//! A classic script is driven, not called: it hands back a description of an
//! HTTP request (`RequestData`), the host performs it, and the script turns
//! the response body into a result (`ResponseData`). A response may name a
//! follow-up request handler, for services that need several round trips.
//! This module plays the host's side of that conversation, so a `Service.js`
//! from a classic installation works without edits.
//!
//! Languages reach a classic script as positions in its `SupportedLanguages`
//! array, numbered exactly as Glossa's [`LangId`] — that numbering is why the
//! table in `glossa-core` is append-only.

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use boa_engine::{Context, JsValue};
use glossa_core::{Capabilities, LangId};
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};
use glossa_net::Codepage;

use super::engine::{self, Bridge};
use crate::manifest::{Manifest, ModuleSection, Permissions, Runtime};
use crate::protocol::Transport;
use crate::provider::{OptionKind, OptionSpec};

/// Our implementation of the classic helper library, used when the
/// installation does not provide its own `Common.js`.
pub const COMPAT: &str = include_str!("qtranslate_compat.js");

/// Emulations of the Windows Script Host objects classic scripts used
/// (`ActiveXObject` HTTP components).
pub const LEGACY_HOST: &str = include_str!("legacy_host.js");

/// The most request/response hops one call may take before we give up on a
/// script that keeps chaining.
const MAX_HOPS: usize = 8;

pub struct LegacyScript {
    /// The installation's own `Common.js`, when there is one.
    pub common: Option<String>,
    pub service: String,
    /// Folder name, for ids and messages.
    pub folder: String,
}

/// What the script says about itself, read once at load time.
#[derive(Debug, Clone)]
pub struct LegacyMeta {
    pub legacy_id: i64,
    pub name: String,
    pub info: String,
    pub capabilities: u32,
    /// `SupportedLanguages`, by slot: the script's code, or `None` for `-1`.
    pub codes: Vec<Option<String>>,
    pub declared_options: Vec<(String, serde_json::Value)>,
}

impl LegacyScript {
    pub fn from_dir(dir: &Path) -> Result<LegacyScript, String> {
        let service = read_text(&dir.join("Service.js"))?;
        let common = [dir.join("Common.js"), dir.join("..").join("Common.js"), dir.join("../..").join("Common.js")]
            .into_iter()
            .find(|p| p.is_file())
            .map(|p| read_text(&p))
            .transpose()?;
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "service".into());
        Ok(LegacyScript { common, service, folder })
    }

    /// A context with the helper library and the service evaluated, and
    /// `Options` set.
    fn context(&self, options: &serde_json::Value) -> Result<Context, String> {
        let mut ctx = engine::new_context()?;
        engine::eval(&mut ctx, LEGACY_HOST, "host objects")?;
        engine::eval(&mut ctx, self.common.as_deref().unwrap_or(COMPAT), "Common.js")?;
        engine::set_global_json(&mut ctx, "__hostOptions", options)?;
        // Merge rather than replace, so options a script declared at load
        // time with addOption keep their defaults.
        engine::eval(
            &mut ctx,
            "if (typeof Options !== 'object' || Options === null) { Options = {}; } \
             for (var __k in __hostOptions) { Options[__k] = __hostOptions[__k]; }",
            "options",
        )?;
        engine::eval(&mut ctx, &self.service, &format!("{}/Service.js", self.folder))?;
        Ok(ctx)
    }

    /// Whether the script reads the Bing session values the classic host
    /// scraped for it.
    fn needs_bing_session(&self) -> bool {
        ["Options.BingToken", "Options.BingKey", "Options.BingCookie", "Options.IG"]
            .iter()
            .any(|k| self.service.contains(k))
    }

    pub fn probe(&self) -> Result<LegacyMeta, String> {
        // Some scripts make requests at the top level (to fetch a cookie).
        // Probing only needs the header and the language table, so answer
        // those requests with an empty page instead of touching the network.
        let fake = |method: &str, _: serde_json::Value| match method {
            "http" => Reply::ok(serde_json::json!({ "status": 200, "headers": {}, "body": "" })),
            _ => Reply::ok(serde_json::Value::Null),
        };
        let _bridge = Bridge::install(&fake);
        let mut ctx = self.context(&serde_json::json!({ "LanguageCode": "en" }))?;
        let header = engine::call_global(&mut ctx, "serviceHeader", &[])?;
        let header = engine::to_json(&mut ctx, &header)?;
        let codes_value = engine::get_global(&mut ctx, "SupportedLanguages");
        let codes_json = engine::to_json(&mut ctx, &codes_value).unwrap_or_default();
        let codes = codes_json
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| match v {
                        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                        // Some scripts (Multitran) use numeric codes.
                        serde_json::Value::Number(n) if n.as_i64().is_some_and(|x| x >= 0) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let declared = engine::get_global(&mut ctx, "__declaredOptions");
        let declared = engine::to_json(&mut ctx, &declared).unwrap_or_default();
        let declared_options = declared
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|pair| {
                        let p = pair.as_array()?;
                        Some((p.first()?.as_str()?.to_owned(), p.get(1).cloned().unwrap_or_default()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(LegacyMeta {
            legacy_id: header.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
            name: header.get("name").and_then(|v| v.as_str()).unwrap_or(&self.folder).to_owned(),
            info: header.get("info").and_then(|v| v.as_str()).unwrap_or("").replace("\r\n", "\n"),
            capabilities: header.get("capabilities").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            codes,
            declared_options,
        })
    }
}

impl LegacyMeta {
    /// A manifest the registry and the protocol provider can work with.
    pub fn manifest(&self, folder: &str) -> Manifest {
        let caps = Capabilities(self.capabilities & 0xf);
        let mut languages: Vec<String> = Vec::new();
        if self.codes.get(1).is_some_and(Option::is_some) {
            languages.push("auto".into());
        }
        for (slot, code) in self.codes.iter().enumerate().skip(2) {
            if code.is_some() && LangId(slot as u16).is_language() {
                languages.push(LangId(slot as u16).bcp47().to_owned());
            }
        }
        let mut description = self.info.clone();
        if description.len() > 600 {
            description.truncate(description.char_indices().nth(600).map(|(i, _)| i).unwrap_or(description.len()));
        }
        Manifest {
            module: ModuleSection {
                id: format!("qt-{}", slug(folder)),
                name: self.name.clone(),
                version: String::new(),
                api: crate::PROTOCOL_VERSION,
                runtime: Runtime::Qtranslate,
                entry: "Service.js".into(),
                description,
                homepage: String::new(),
                author: String::new(),
                license: String::new(),
                max_input_chars: Some(5000),
                idle_timeout_secs: None,
            },
            capabilities: caps.names().into_iter().map(String::from).collect(),
            languages,
            language_map: Default::default(),
            // Classic scripts always had unrestricted network access, and
            // their hosts are spread across many domains.
            permissions: Permissions::allow_all(),
            options: self
                .declared_options
                .iter()
                .map(|(key, default)| OptionSpec {
                    key: key.clone(),
                    label: key.clone(),
                    kind: if key.to_ascii_lowercase().contains("key") || key.to_ascii_lowercase().contains("token") {
                        OptionKind::Secret
                    } else {
                        OptionKind::Text
                    },
                    default: default.clone(),
                    help: String::new(),
                    required: false,
                    choices: Vec::new(),
                    min: None,
                    max: None,
                    placeholder: String::new(),
                })
                .collect(),
        }
    }
}

pub struct LegacyTransport {
    pub script: Arc<LegacyScript>,
    pub ui_language_code: String,
    pub preferred_domain: String,
}

/// The classic request description, read out of a `RequestData` object.
struct RequestData {
    method: i64,
    uri: String,
    data: String,
    headers: String,
    codepage: i64,
    response_handler: String,
}

impl LegacyTransport {
    fn host_options(&self, user: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("LanguageCode".into(), self.ui_language_code.clone().into());
        o.insert("PreferredDomain".into(), self.preferred_domain.clone().into());
        o.insert("GoogleDomain".into(), self.preferred_domain.clone().into());
        // The classic host scraped a Google token into this; the keyless
        // endpoints the scripts use accept the neutral value.
        o.insert("GoogleTkk".into(), "0.0".into());
        for (k, v) in user {
            o.insert(k.clone(), v.clone());
        }
        serde_json::Value::Object(o)
    }

    fn run(
        &self,
        method: &str,
        params: &serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
    ) -> Result<serde_json::Value, ModuleError> {
        let mut user = params.get("options").and_then(|o| o.as_object()).cloned().unwrap_or_default();
        if self.script.needs_bing_session() {
            match bing_session(host) {
                Ok(session) => user.extend(session),
                Err(e) => tracing::debug!("could not fetch the Bing session values: {}", e.message),
            }
        }
        let mut ctx = self.script.context(&self.host_options(&user)).map_err(engine::script_error)?;
        let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let slot = |key: &str| -> i32 {
            let code = params.get(key).and_then(|v| v.as_str()).unwrap_or("auto");
            LangId::from_code(code).map(|l| l.0 as i32).unwrap_or(0)
        };

        match method {
            "translate" => {
                let (from, to) = (slot("from"), slot("to"));
                let args = [JsValue::from(js(&text)), JsValue::from(from), JsValue::from(to)];
                let resp = self.flow(&mut ctx, host, "serviceTranslateRequest", "serviceTranslateResponse", &args)?;
                let translation = resp.get("translation").and_then(|v| v.as_str()).unwrap_or("").replace("\r\n", "\n");
                if translation.trim().is_empty() {
                    return Err(ModuleError::new(ErrorKind::BadResponse, "the script returned no translation"));
                }
                let detected =
                    resp.get("sourceLanguage").and_then(|v| v.as_i64()).filter(|n| *n > 1).map(|n| LangId(n as u16));
                Ok(serde_json::json!({
                    "text": translation.trim_end(),
                    "detected_from": detected.filter(|l| l.is_language()).map(|l| l.bcp47()),
                }))
            }
            "dictionary" => {
                let (from, to) = (slot("from"), slot("to"));
                let args = [JsValue::from(js(&text)), JsValue::from(from), JsValue::from(to)];
                let resp = self.flow(&mut ctx, host, "serviceDictionaryRequest", "serviceDictionaryResponse", &args)?;
                let html = resp.get("translation").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                if html.trim().is_empty() {
                    return Err(ModuleError::new(ErrorKind::NotFound, format!("no article for {text:?}")));
                }
                let link =
                    resp.get("data").and_then(|v| v.as_str()).filter(|s| s.starts_with("http")).map(str::to_owned);
                Ok(serde_json::json!({ "headword": text, "html": html, "link": link }))
            }
            "detect" => {
                let rd = engine::call_global(&mut ctx, "serviceDetectLanguageRequest", &[JsValue::from(js(&text))])
                    .map_err(engine::script_error)?;
                let rd = read_request(&mut ctx, &rd)?;
                let body = self.perform(&mut ctx, host, &rd, &text, 1, 0, false)?;
                let handler =
                    if rd.response_handler.is_empty() { "serviceDetectLanguageResponse" } else { &rd.response_handler };
                let lang = engine::call_global(&mut ctx, handler, &[JsValue::from(js(&body.0))])
                    .map_err(engine::script_error)?;
                let n = engine::to_json(&mut ctx, &lang).ok().and_then(|v| v.as_i64()).unwrap_or(0);
                let lang = LangId(n.max(0) as u16);
                if !lang.is_language() {
                    return Err(ModuleError::new(ErrorKind::NotFound, "the script could not detect the language"));
                }
                Ok(serde_json::json!({ "lang": lang.bcp47(), "confidence": 1.0 }))
            }
            "speak" => {
                let lang = LangId::from_code(params.get("lang").and_then(|v| v.as_str()).unwrap_or("en"))
                    .unwrap_or(LangId::ENGLISH);
                let slow = params.get("slow").and_then(|v| v.as_bool()).unwrap_or(false);
                let rd = engine::call_global(
                    &mut ctx,
                    "serviceListenRequest",
                    &[JsValue::from(js(&text)), JsValue::from(lang.0 as i32), JsValue::from(slow)],
                )
                .map_err(engine::script_error)?;
                let rd = read_request(&mut ctx, &rd)?;
                let (_, bytes) = self.perform(&mut ctx, host, &rd, &text, lang.0 as i32, lang.0 as i32, true)?;
                if bytes.is_empty() {
                    return Err(ModuleError::new(ErrorKind::BadResponse, "the service returned no audio"));
                }
                Ok(serde_json::json!({
                    "audio_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    "mime": sniff_audio(&bytes),
                }))
            }
            other => Err(ModuleError::unsupported(other)),
        }
    }

    /// request -> HTTP -> response handler, following `nextRequestHandler`.
    fn flow(
        &self,
        ctx: &mut Context,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        request_fn: &str,
        response_fn: &str,
        args: &[JsValue; 3],
    ) -> Result<serde_json::Value, ModuleError> {
        if !engine::has_function(ctx, request_fn) {
            return Err(ModuleError::unsupported(request_fn));
        }
        let text = engine::to_string(ctx, &args[0]);
        let from = engine::to_json(ctx, &args[1]).ok().and_then(|v| v.as_i64()).unwrap_or(1) as i32;
        let to = engine::to_json(ctx, &args[2]).ok().and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let mut rd_value = engine::call_global(ctx, request_fn, args).map_err(engine::script_error)?;
        for _ in 0..MAX_HOPS {
            let rd = read_request(ctx, &rd_value)?;
            let (body, _) = self.perform(ctx, host, &rd, &text, from, to, false)?;
            let handler =
                if rd.response_handler.is_empty() { response_fn.to_owned() } else { rd.response_handler.clone() };
            let resp_value = engine::call_global(
                ctx,
                &handler,
                &[args[0].clone(), JsValue::from(js(&body)), args[1].clone(), args[2].clone()],
            )
            .map_err(engine::script_error)?;
            let resp = engine::to_json(ctx, &resp_value).map_err(ModuleError::other)?;
            let next = resp.get("nextRequestHandler").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            if next.is_empty() {
                return Ok(resp);
            }
            rd_value =
                engine::call_global(ctx, &next, &[args[0].clone(), args[1].clone(), args[2].clone(), resp_value])
                    .map_err(engine::script_error)?;
        }
        Err(ModuleError::new(ErrorKind::BadResponse, "the script chained too many requests"))
    }

    /// Perform one classic request through the host. Returns the body decoded
    /// as text and the raw bytes.
    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        ctx: &mut Context,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        rd: &RequestData,
        text: &str,
        from: i32,
        to: i32,
        binary: bool,
    ) -> Result<(String, Vec<u8>), ModuleError> {
        // HttpMethod.UNDEFINED: the script needs no request; its response
        // handler works from the text alone.
        if rd.method == 0 {
            return Ok((String::new(), Vec::new()));
        }
        let url = if rd.uri.starts_with("http://") || rd.uri.starts_with("https://") {
            rd.uri.clone()
        } else {
            let base = engine::call_global(
                ctx,
                "serviceHost",
                &[JsValue::from(js(text)), JsValue::from(from), JsValue::from(to)],
            )
            .map(|v| engine::to_string(ctx, &v))
            .map_err(engine::script_error)?;
            join_url(&base, &rd.uri)
        };
        let codepage = Codepage::from_number(rd.codepage as u32).unwrap_or(Codepage::Utf8);
        let mut headers = serde_json::Map::new();
        for line in rd.headers.split(['\r', '\n']) {
            if let Some((k, v)) = line.split_once(':') {
                if !k.trim().is_empty() {
                    headers.insert(k.trim().to_owned(), v.trim().to_owned().into());
                }
            }
        }
        let method = if rd.method == 2 { "POST" } else { "GET" };
        let mut req = serde_json::json!({ "method": method, "url": url, "headers": headers, "binary": true });
        if method == "POST" {
            let body = codepage.encode(&rd.data);
            req["body_base64"] = base64::engine::general_purpose::STANDARD.encode(body).into();
        }
        let resp = host("http", req).into_result()?;
        let status = resp.get("status").and_then(|v| v.as_u64()).unwrap_or(0);
        let bytes = resp
            .get("body_base64")
            .and_then(|v| v.as_str())
            .map(|b| base64::engine::general_purpose::STANDARD.decode(b).unwrap_or_default())
            .unwrap_or_default();
        let declared = resp
            .get("headers")
            .and_then(|h| h.get("content-type"))
            .and_then(|v| v.as_str())
            .and_then(|ct| ct.split(';').find_map(|p| p.trim().strip_prefix("charset=")))
            .and_then(|c| Codepage::from_label(c.trim_matches('"')));
        let body_text = if binary { String::new() } else { declared.unwrap_or(codepage).decode(&bytes) };
        if !(200..300).contains(&status) {
            // Some services report errors in a body the script knows how to
            // read; hand those over, and fail only on an empty one.
            if body_text.trim().is_empty() {
                let kind = if status == 429 { ErrorKind::RateLimited } else { ErrorKind::Http };
                return Err(ModuleError::new(kind, format!("HTTP {status} from {}", glossa_net::redact(&url))));
            }
        }
        Ok((body_text, bytes))
    }
}

impl Transport for LegacyTransport {
    fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
    ) -> Result<serde_json::Value, ModuleError> {
        let _bridge = Bridge::install(host);
        self.run(method, &params, host)
    }
}

/// What the classic host scraped from bing.com/translator for the Microsoft
/// script: the page's `IG` id, the anti-abuse key and token, and the session
/// cookies. Cached for ten minutes.
fn bing_session(
    host: &dyn Fn(&str, serde_json::Value) -> Reply,
) -> Result<serde_json::Map<String, serde_json::Value>, ModuleError> {
    const CACHE_KEY: &str = "bing_session";
    if let Ok(cached) = host("cache_get", serde_json::json!({ "key": CACHE_KEY })).into_result() {
        if let Some(map) = cached.as_object() {
            return Ok(map.clone());
        }
    }
    let resp = host("http", serde_json::json!({ "url": "https://www.bing.com/translator" })).into_result()?;
    let body = resp.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let ig = between(body, "IG:\"", "\"")
        .ok_or_else(|| ModuleError::new(ErrorKind::BadResponse, "no IG on the Bing page"))?;
    let helper = between(body, "params_AbusePreventionHelper", "]")
        .ok_or_else(|| ModuleError::new(ErrorKind::BadResponse, "no anti-abuse parameters on the Bing page"))?;
    let helper = helper.trim_start_matches([' ', '=', '[']);
    let mut parts = helper.splitn(3, ',');
    let key = parts.next().unwrap_or("").trim().to_owned();
    let token = parts.next().unwrap_or("").trim().trim_matches('"').to_owned();
    let cookies = resp
        .get("headers")
        .and_then(|h| h.get("set-cookie"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .lines()
        .filter_map(|c| c.split(';').next())
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let mut map = serde_json::Map::new();
    map.insert("IG".into(), ig.into());
    map.insert("BingKey".into(), key.into());
    map.insert("BingToken".into(), token.into());
    map.insert("BingCookie".into(), cookies.into());
    let _ = host("cache_set", serde_json::json!({ "key": CACHE_KEY, "value": map, "ttl_secs": 600 }));
    Ok(map)
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let len = s[from..].find(end)?;
    Some(&s[from..from + len])
}

fn read_request(ctx: &mut Context, value: &JsValue) -> Result<RequestData, ModuleError> {
    let v = engine::to_json(ctx, value).map_err(ModuleError::other)?;
    if !v.is_object() {
        return Err(ModuleError::new(ErrorKind::BadResponse, "the script did not return a RequestData"));
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
    Ok(RequestData {
        method: v.get("method").and_then(|x| x.as_i64()).unwrap_or(1),
        uri: s("uri"),
        data: s("data"),
        headers: s("headers"),
        codepage: v.get("codepage").and_then(|x| x.as_i64()).unwrap_or(65001),
        response_handler: s("responseHandler"),
    })
}

fn join_url(base: &str, path: &str) -> String {
    if path.is_empty() {
        return base.to_owned();
    }
    match (base.ends_with('/'), path.starts_with('/')) {
        (true, true) => format!("{}{}", base, &path[1..]),
        (false, false) if !path.starts_with('?') => format!("{base}/{path}"),
        _ => format!("{base}{path}"),
    }
}

fn js(s: &str) -> boa_engine::JsString {
    boa_engine::JsString::from(s)
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "service".into()
    } else {
        out
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // Classic scripts are usually UTF-8, sometimes with a BOM, occasionally
    // Windows-1251.
    match String::from_utf8(bytes) {
        Ok(s) => Ok(s.trim_start_matches('\u{feff}').to_owned()),
        Err(e) => Ok(Codepage::Windows1251.decode(e.as_bytes())),
    }
}

fn sniff_audio(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"RIFF") {
        "audio/wav"
    } else if bytes.starts_with(b"OggS") {
        "audio/ogg"
    } else {
        "audio/mpeg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_api::ModuleCache;
    use crate::protocol::ProtocolProvider;
    use crate::provider::{with_test_host, OptionValues, Provider};
    use glossa_core::{DictRequest, LangPair, TranslateRequest};
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    /// A script in the classic style, written for these tests: a GET request
    /// to a JSON endpoint, a typical response parser, a dictionary that scrapes
    /// HTML, and detection.
    const SERVICE: &str = r#"
        function serviceHeader() {
            return new ServiceHeader(901, "Test Service", "A test." + Const.NL2 + "https://svc.test",
                Capability.TRANSLATE | Capability.DETECT_LANGUAGE | Capability.DICTIONARY);
        }
        function serviceHost(a, b, c) { return "https://svc.test"; }
        function serviceLink(a, b, c) { return serviceHost(); }
        SupportedLanguages = [-1, "auto", "af", -1, -1, "ar", -1, -1, -1, -1, -1, "zh-CN", -1, -1, -1, -1, -1, "en",
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, "ru"];
        addOption("ApiKey", "");
        function serviceTranslateRequest(a, b, c) {
            return new RequestData(HttpMethod.GET, format("/t?sl={0}&tl={1}&q={2}&k={3}",
                codeFromLanguage(b), codeFromLanguage(c), encodeGetParam(a), Options.ApiKey));
        }
        function serviceTranslateResponse(a, b, c, d) {
            var j = parseJSON(b);
            var src = isLanguage(c) ? c : languageFromCode(j.src);
            return new ResponseData(j.text + Const.NL + "line2", src, d);
        }
        function serviceDetectLanguageRequest(a) {
            return new RequestData(HttpMethod.POST, "/detect", "q=" + encodePostParam(a));
        }
        function serviceDetectLanguageResponse(a) { return languageFromCode(parseJSON(a).lang); }
        function serviceDictionaryRequest(a, b, c) { return new RequestData(HttpMethod.GET, "https://dict.test/w/" + encodeGetParam(a)); }
        function serviceDictionaryResponse(a, b, c, d) {
            var body = stringFindSub(b, "<article>", false, "</article>", false);
            return new ResponseData(body, c, d, "https://dict.test/w/" + a);
        }
    "#;

    fn provider(service: &str) -> ProtocolProvider<LegacyTransport> {
        let script = Arc::new(LegacyScript { common: None, service: service.into(), folder: "Test Service".into() });
        let meta = script.probe().unwrap();
        let manifest = meta.manifest(&script.folder);
        ProtocolProvider::new(
            manifest,
            LegacyTransport { script, ui_language_code: "ru".into(), preferred_domain: "com".into() },
            ModuleCache::new(),
        )
    }

    #[test]
    fn metadata_comes_from_the_script() {
        let p = provider(SERVICE);
        let info = p.info();
        assert_eq!(info.id, "qt-test-service");
        assert_eq!(info.name, "Test Service");
        assert!(info
            .capabilities
            .contains(Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE | Capabilities::DICTIONARY));
        assert!(!info.capabilities.contains(Capabilities::LISTEN));
        // Slots 2 (af), 5 (ar), 11 (zh-CN), 17 (en), 46 (ru), plus auto.
        assert!(info.supports_language(LangId::ENGLISH));
        assert!(info.supports_language(LangId::RUSSIAN));
        assert!(info.supports_language(LangId::from_code("zh-CN").unwrap()));
        assert!(!info.supports_language(LangId::from_code("de").unwrap()));
        assert!(info.supports_language(LangId::AUTO));
        // addOption shows up as a setting, masked because it looks like a key.
        assert_eq!(p.options()[0].key, "ApiKey");
        assert_eq!(p.options()[0].kind, OptionKind::Secret);
    }

    #[test]
    fn translation_runs_the_request_response_cycle() {
        let p = provider(SERVICE);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t?", 200, r#"{"text":"hello","src":"ru"}"#);
        let mut opts = OptionValues::new();
        opts.insert("ApiKey".into(), serde_json::json!("K"));
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("привет мир", LangPair::new(LangId::AUTO, LangId::ENGLISH)))
                .unwrap();
            assert_eq!(t.text, "hello\nline2", "CRLF from the script is normalised");
            assert_eq!(t.detected_from, Some(LangId::RUSSIAN));
        });
        let req = &http.requests()[0];
        assert_eq!(
            req.url,
            "https://svc.test/t?sl=auto&tl=en&q=%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82%20%D0%BC%D0%B8%D1%80&k=K"
        );
        // The classic default headers went out, and the user's option reached
        // the script through Options.
        assert!(req.headers.iter().any(|(k, v)| k == "Accept-Language" && v.starts_with("ru;")));
    }

    #[test]
    fn detection_and_post_bodies() {
        let p = provider(SERVICE);
        let http = MockHttp::new().on(Method::Post, "https://svc.test/detect", 200, r#"{"lang":"ar"}"#);
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let d = p.detect(host, "مرحبا").unwrap();
            assert_eq!(d.lang, LangId::from_code("ar").unwrap());
        });
        let req = &http.requests()[0];
        assert_eq!(req.body.as_deref(), Some("q=%D9%85%D8%B1%D8%AD%D8%A8%D8%A7".as_bytes()));
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Content-Type" && v.starts_with("application/x-www-form-urlencoded")));
    }

    #[test]
    fn dictionary_articles_carry_html_and_link() {
        let p = provider(SERVICE);
        let http = MockHttp::new().on(
            Method::Get,
            "https://dict.test/w/",
            200,
            "<html><article><b>run</b> — бежать</article></html>",
        );
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let a = p
                .dictionary(
                    host,
                    &DictRequest { text: "run".into(), from: LangId::ENGLISH, to: LangId::RUSSIAN, exact: true },
                )
                .unwrap();
            assert_eq!(a.html.as_deref(), Some("<b>run</b> — бежать"));
            assert_eq!(a.link.as_deref(), Some("https://dict.test/w/run"));
        });
    }

    #[test]
    fn script_errors_are_reported_with_the_function_name() {
        let broken = SERVICE.replace("var j = parseJSON(b);", "var j = parseJSON(b); undefinedFunction();");
        let p = provider(&broken);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/", 200, "{}");
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let err = p
                .translate(host, &TranslateRequest::new("x", LangPair::new(LangId::RUSSIAN, LangId::ENGLISH)))
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("serviceTranslateResponse") && msg.contains("undefinedFunction"), "{msg}");
        });
    }

    #[test]
    fn compat_helpers_behave_like_the_classic_ones() {
        let mut ctx = engine::new_context().unwrap();
        engine::eval(&mut ctx, COMPAT, "compat").unwrap();
        let check = |ctx: &mut Context, expr: &str| -> serde_json::Value {
            let v = engine::eval(ctx, expr, "t").unwrap();
            engine::to_json(ctx, &v).unwrap()
        };
        assert_eq!(check(&mut ctx, "format('{0}-{1}-{0}', 'a', 'b')"), "a-b-a");
        assert_eq!(check(&mut ctx, "stringFindSub('xx<b>bold</b>yy', '<b>', false, '</b>', false)"), "bold");
        assert_eq!(check(&mut ctx, "stringFindSub('xx<b>bold</b>yy', '<b>', true, '</b>', true)"), "<b>bold</b>");
        assert_eq!(check(&mut ctx, "stripHtml('<p>a&nbsp;<b>b</b></p><br/><script>x</script>c')"), "a b\r\nc");
        assert_eq!(check(&mut ctx, "trimString('  x  ')"), "x");
        assert_eq!(check(&mut ctx, "unquoteHtml('&lt;a&gt; &amp; &quot;q&quot; &#39;')"), "<a> & \"q\" '");
        assert_eq!(check(&mut ctx, "removeEmptyLines('a\\n\\n\\n\\nb\\n')"), "a\r\n\r\nb");
        assert_eq!(
            check(&mut ctx, "updateHtmlLinks('<a href=\"/x\"><img src=\"//cdn/y\">', 'https://site.org')"),
            "<a href=\"https://site.org/x\"><img src=\"https://cdn/y\">"
        );
        // A long query is cut at the last encoded space.
        let cut = check(&mut ctx, "encodeGetParam(new Array(400).join('word ')).length");
        assert!(cut.as_u64().unwrap() <= 1800);
        assert_eq!(check(&mut ctx, "new RequestData(HttpMethod.GET, '/x').headers.indexOf('Accept: */*') === 0"), true);
        assert_eq!(check(&mut ctx, "parseJSON(\"{a: 'single quotes'}\").a"), "single quotes");
    }

    #[test]
    fn top_level_activex_requests_work_at_call_time_and_are_faked_while_probing() {
        let service = format!(
            "var x = new ActiveXObject('WinHttp.WinHttpRequest.5.1'); x.open('GET', 'https://cookie.test/', false); x.send(null);\n\
             var cookie = x.getResponseHeader('Content-Type');\n{}",
            SERVICE.replace(
                "return new ResponseData(j.text + Const.NL + \"line2\", src, d);",
                "return new ResponseData(j.text + '/' + cookie, src, d);"
            )
        );
        // Probing does not touch the network (there is no host to ask).
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Get, "https://cookie.test/", 200, "").on(
            Method::Get,
            "https://svc.test/t?",
            200,
            r#"{"text":"hi","src":"ru"}"#,
        );
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("x", LangPair::new(LangId::RUSSIAN, LangId::ENGLISH)))
                .unwrap();
            assert_eq!(t.text, "hi/application/json; charset=utf-8");
        });
        assert_eq!(http.requests()[0].url, "https://cookie.test/");
    }

    #[test]
    fn bing_session_values_are_scraped_and_cached() {
        let page = r#"<script>var x = {IG:"ABC123DEF",x:1}; var params_AbusePreventionHelper = [1712345678901,"tok-en_9",3600000];</script>"#;
        let calls = std::cell::RefCell::new(0);
        let cache = std::cell::RefCell::new(None::<serde_json::Value>);
        let host = |m: &str, p: serde_json::Value| -> Reply {
            match m {
                "http" => {
                    *calls.borrow_mut() += 1;
                    Reply::ok(serde_json::json!({ "status": 200, "body": page,
                        "headers": { "set-cookie": "MUID=1; path=/\n_EDGE_S=F=1; HttpOnly" } }))
                }
                "cache_get" => Reply::ok(cache.borrow().clone().unwrap_or(serde_json::Value::Null)),
                "cache_set" => {
                    *cache.borrow_mut() = p.get("value").cloned();
                    Reply::ok(serde_json::Value::Null)
                }
                _ => Reply::ok(serde_json::Value::Null),
            }
        };
        let s = bing_session(&host).unwrap();
        assert_eq!(s["IG"], "ABC123DEF");
        assert_eq!(s["BingKey"], "1712345678901");
        assert_eq!(s["BingToken"], "tok-en_9");
        assert_eq!(s["BingCookie"], "MUID=1; _EDGE_S=F=1");
        bing_session(&host).unwrap();
        assert_eq!(*calls.borrow(), 1, "second call is served from the cache");
    }

    #[test]
    fn urls_join_without_double_slashes() {
        assert_eq!(join_url("https://a.test", "/x"), "https://a.test/x");
        assert_eq!(join_url("https://a.test/", "/x"), "https://a.test/x");
        assert_eq!(join_url("https://a.test", "x"), "https://a.test/x");
        assert_eq!(join_url("https://a.test/p", "?q=1"), "https://a.test/p?q=1");
    }

    #[test]
    fn folder_names_become_ids() {
        assert_eq!(slug("Google Translate"), "google-translate");
        assert_eq!(slug("Dic Academic"), "dic-academic");
        assert_eq!(slug("ABBYY Lingvo Live"), "abbyy-lingvo-live");
        assert_eq!(slug("***"), "service");
    }
}
