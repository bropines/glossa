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
use glossa_core::{Capabilities, LangId};
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};
use glossa_net::Codepage;
use rquickjs::{Ctx, Value};

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
    pub common: Option<Arc<engine::Script>>,
    pub service: engine::Script,
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
            .map(|p| shared_common(&p))
            .transpose()?;
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "service".into());
        let service = engine::Script::new(format!("{folder}/Service.js"), service);
        Ok(LegacyScript { common, service, folder })
    }

    /// Evaluate the helper library and the service in a fresh context, with
    /// `Options` set.
    fn setup(&self, ctx: &Ctx<'_>, options: &serde_json::Value) -> Result<(), String> {
        engine::shared::LEGACY_HOST.eval(ctx)?;
        match &self.common {
            Some(common) => common.eval(ctx)?,
            None => engine::shared::COMPAT.eval(ctx)?,
        };
        engine::set_global_json(ctx, "__hostOptions", options)?;
        // Merge rather than replace, so options a script declared at load
        // time with addOption keep their defaults.
        engine::eval(
            ctx,
            "if (typeof Options !== 'object' || Options === null) { Options = {}; } \
             for (var __k in __hostOptions) { Options[__k] = __hostOptions[__k]; }",
            "options",
        )?;
        self.service.eval(ctx)?;
        Ok(())
    }

    /// Whether the script reads the Bing session values the classic host
    /// scraped for it.
    fn needs_bing_session(&self) -> bool {
        ["Options.BingToken", "Options.BingKey", "Options.BingCookie", "Options.IG"]
            .iter()
            .any(|k| self.service.source.contains(k))
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
        engine::with_context(|ctx| {
            self.setup(ctx, &serde_json::json!({ "LanguageCode": "en" }))?;
            let header = engine::call_global(ctx, "serviceHeader", &[])?;
            let header = engine::to_json(ctx, &header)?;
            let codes_value = engine::get_global(ctx, "SupportedLanguages");
            let codes_json = engine::to_json(ctx, &codes_value).unwrap_or_default();
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
            let declared = engine::get_global(ctx, "__declaredOptions");
            let declared = engine::to_json(ctx, &declared).unwrap_or_default();
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
        })?
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
                // A classic install brings dozens; the imported settings or
                // the user decide which to show.
                ready_to_use: false,
                ..Default::default()
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
                    ..OptionSpec::default()
                })
                .collect(),
            assets: Vec::new(),
            assets_dir: None,
            ui: Default::default(),
            actions: Vec::new(),
            strings: Default::default(),
        }
    }
}

pub struct LegacyTransport {
    pub script: Arc<LegacyScript>,
    pub ui_language_code: String,
    pub preferred_domain: String,
}

/// The classic `Capability` values. The host passes one as the first argument
/// of `serviceHost`, so a script serving several capabilities can name a
/// different host for each (`serviceHost(Capability.DICTIONARY, from, to)` is
/// how the scripts call it themselves).
mod cap {
    pub const TRANSLATE: i32 = 1;
    pub const DETECT_LANGUAGE: i32 = 2;
    pub const LISTEN: i32 = 4;
    pub const DICTIONARY: i32 = 8;
}

/// One performed request, as the driver needs it.
struct Fetched {
    text: String,
    bytes: Vec<u8>,
    status: u16,
    url: String,
}

impl Fetched {
    /// Nothing was requested (`HttpMethod.UNDEFINED`).
    fn none() -> Fetched {
        Fetched { text: String::new(), bytes: Vec::new(), status: 200, url: String::new() }
    }

    fn succeeded(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The error to report when a script got nothing out of a failed
    /// response: the HTTP status says more than the script's own complaint.
    fn http_error(&self, detail: Option<&str>) -> ModuleError {
        let kind = if self.status == 429 { ErrorKind::RateLimited } else { ErrorKind::Http };
        let mut message = format!("HTTP {} from {}", self.status, glossa_net::redact(&self.url));
        if let Some(d) = detail {
            message.push_str(&format!("; the script could not read the reply ({d})"));
        }
        ModuleError::new(kind, message)
    }
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
            // Without these values the script would only fail later with a
            // confusing TypeError (`Options.BingKey` is undefined), so the
            // real cause is reported instead.
            let session = bing_session(host).map_err(|e| {
                ModuleError::new(e.kind, format!("could not start a Bing Translator session: {}", e.message))
            })?;
            user.extend(session);
        }
        let options = self.host_options(&user);
        engine::with_context(|ctx| -> Result<serde_json::Value, ModuleError> {
            self.script.setup(ctx, &options).map_err(engine::script_error)?;
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let slot = |key: &str| -> i32 {
                let code = params.get(key).and_then(|v| v.as_str()).unwrap_or("auto");
                LangId::from_code(code).map(|l| l.0 as i32).unwrap_or(0)
            };

            match method {
                "translate" => {
                    let (from, to) = (slot("from"), slot("to"));
                    let args = [engine::str(ctx, &text), engine::int(ctx, from), engine::int(ctx, to)];
                    let resp = self.flow(
                        ctx,
                        host,
                        cap::TRANSLATE,
                        "serviceTranslateRequest",
                        "serviceTranslateResponse",
                        &args,
                    )?;
                    let translation =
                        resp.get("translation").and_then(|v| v.as_str()).unwrap_or("").replace("\r\n", "\n");
                    if translation.trim().is_empty() {
                        // Some scripts explain a failure in the `data` slot.
                        let reason = resp
                            .get("data")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty() && !has_scheme(s))
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default();
                        return Err(ModuleError::new(
                            ErrorKind::BadResponse,
                            format!("the script returned no translation{reason}"),
                        ));
                    }
                    let detected = resp
                        .get("sourceLanguage")
                        .and_then(|v| v.as_i64())
                        .filter(|n| *n > 1)
                        .map(|n| LangId(n as u16));
                    Ok(serde_json::json!({
                        "text": translation.trim_end(),
                        "detected_from": detected.filter(|l| l.is_language()).map(|l| l.bcp47()),
                    }))
                }
                "dictionary" => {
                    let (from, to) = (slot("from"), slot("to"));
                    let args = [engine::str(ctx, &text), engine::int(ctx, from), engine::int(ctx, to)];
                    let resp = self.flow(
                        ctx,
                        host,
                        cap::DICTIONARY,
                        "serviceDictionaryRequest",
                        "serviceDictionaryResponse",
                        &args,
                    )?;
                    let html = resp.get("translation").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                    if html.trim().is_empty() {
                        return Err(ModuleError::new(ErrorKind::NotFound, format!("no article for {text:?}")));
                    }
                    let link =
                        resp.get("data").and_then(|v| v.as_str()).filter(|s| s.starts_with("http")).map(str::to_owned);
                    Ok(serde_json::json!({ "headword": text, "html": html, "link": link }))
                }
                "detect" => {
                    let rd = engine::call_global(ctx, "serviceDetectLanguageRequest", &[engine::str(ctx, &text)])
                        .map_err(engine::script_error)?;
                    let rd = read_request(ctx, &rd)?;
                    let fetched = self.perform(ctx, host, &rd, cap::DETECT_LANGUAGE, 1, 0, false)?;
                    let handler = if rd.response_handler.is_empty() {
                        "serviceDetectLanguageResponse"
                    } else {
                        &rd.response_handler
                    };
                    let lang = engine::call_global(ctx, handler, &[engine::str(ctx, &fetched.text)])
                        .map_err(|e| script_failure(&fetched, e))?;
                    let n = engine::to_json(ctx, &lang).ok().and_then(|v| v.as_i64()).unwrap_or(0);
                    let lang = LangId(n.max(0) as u16);
                    if !lang.is_language() && !fetched.succeeded() {
                        return Err(fetched.http_error(None));
                    }
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
                        ctx,
                        "serviceListenRequest",
                        &[engine::str(ctx, &text), engine::int(ctx, lang.0 as i32), engine::bool(ctx, slow)],
                    )
                    .map_err(engine::script_error)?;
                    let rd = read_request(ctx, &rd)?;
                    let fetched = self.perform(ctx, host, &rd, cap::LISTEN, lang.0 as i32, lang.0 as i32, true)?;
                    if !fetched.succeeded() {
                        return Err(fetched.http_error(None));
                    }
                    let bytes = fetched.bytes;
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
        })
        .map_err(ModuleError::other)?
    }

    /// request -> HTTP -> response handler, following `nextRequestHandler`.
    ///
    /// A chained response *adds to* what came before rather than replacing
    /// it: the Microsoft script, for one, translates first and then chains a
    /// dictionary lookup whose response holds only the dictionary block
    /// (starting with a blank line, to sit under the translation). So the
    /// pieces are concatenated, the languages a response settled on carry
    /// over into the next hop, and a `nextRequestHandler` that names no
    /// function ends the chain with what has been collected — some scripts
    /// put other values in that slot.
    fn flow<'js>(
        &self,
        ctx: &Ctx<'js>,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        capability: i32,
        request_fn: &str,
        response_fn: &str,
        args: &[Value<'js>; 3],
    ) -> Result<serde_json::Value, ModuleError> {
        if !engine::has_function(ctx, request_fn) {
            return Err(ModuleError::unsupported(request_fn));
        }
        let mut from = engine::to_json(ctx, &args[1]).ok().and_then(|v| v.as_i64()).unwrap_or(1) as i32;
        let mut to = engine::to_json(ctx, &args[2]).ok().and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let mut collected = String::new();

        let mut rd_value = engine::call_global(ctx, request_fn, args).map_err(engine::script_error)?;
        for _ in 0..MAX_HOPS {
            let rd = read_request(ctx, &rd_value)?;
            let fetched = self.perform(ctx, host, &rd, capability, from, to, false)?;
            let handler =
                if rd.response_handler.is_empty() { response_fn.to_owned() } else { rd.response_handler.clone() };
            let resp_value = engine::call_global(
                ctx,
                &handler,
                &[args[0].clone(), engine::str(ctx, &fetched.text), engine::int(ctx, from), engine::int(ctx, to)],
            )
            .map_err(|e| script_failure(&fetched, e))?;
            let mut resp = engine::to_json(ctx, &resp_value).map_err(ModuleError::other)?;
            if let Some(piece) = resp.get("translation").and_then(|v| v.as_str()) {
                collected.push_str(piece);
            }
            let language = |key: &str| resp.get(key).and_then(|v| v.as_i64()).filter(|n| *n > 1).map(|n| n as i32);
            from = language("sourceLanguage").unwrap_or(from);
            to = language("translationLanguage").unwrap_or(to);

            let next = resp.get("nextRequestHandler").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            if next.is_empty() || !engine::has_function(ctx, &next) {
                if !next.is_empty() {
                    tracing::debug!("nextRequestHandler {next:?} is not a function; the response is final");
                }
                if collected.trim().is_empty() && !fetched.succeeded() {
                    return Err(fetched.http_error(None));
                }
                if let Some(obj) = resp.as_object_mut() {
                    obj.insert("translation".into(), collected.into());
                    obj.insert("sourceLanguage".into(), from.into());
                    obj.insert("translationLanguage".into(), to.into());
                }
                return Ok(resp);
            }
            rd_value = engine::call_global(
                ctx,
                &next,
                &[args[0].clone(), engine::int(ctx, from), engine::int(ctx, to), resp_value],
            )
            .map_err(engine::script_error)?;
        }
        Err(ModuleError::new(ErrorKind::BadResponse, "the script chained too many requests"))
    }

    /// Perform one classic request through the host.
    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        ctx: &Ctx<'_>,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        rd: &RequestData,
        capability: i32,
        from: i32,
        to: i32,
        binary: bool,
    ) -> Result<Fetched, ModuleError> {
        // HttpMethod.UNDEFINED: the script needs no request; its response
        // handler works from the text alone.
        if rd.method == 0 {
            return Ok(Fetched::none());
        }
        let url = if has_scheme(&rd.uri) {
            rd.uri.clone()
        } else {
            let base = engine::call_global(
                ctx,
                "serviceHost",
                &[engine::int(ctx, capability), engine::int(ctx, from), engine::int(ctx, to)],
            )
            .map(|v| engine::to_string(ctx, &v))
            .map_err(engine::script_error)?;
            // A few scripts name a bare host ("dict.example.com"); the
            // classic host connected to it over HTTP(S) all the same.
            let base = if has_scheme(&base) { base } else { format!("https://{}", base.trim_start_matches('/')) };
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
        // A POST built with the GET header set carries no Content-Type. Form
        // encoding is what such a body always is (and what a browser or curl
        // would declare); some endpoints ignore an undeclared body — Bing's
        // dictionary lookup answers "status 205" without it.
        if method == "POST" && !rd.data.is_empty() && !headers.keys().any(|k| k.eq_ignore_ascii_case("content-type")) {
            headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
        }
        let mut req = serde_json::json!({ "method": method, "url": url, "headers": headers, "binary": true });
        if method == "POST" {
            let body = codepage.encode(&rd.data);
            req["body_base64"] = base64::engine::general_purpose::STANDARD.encode(body).into();
        }
        let resp = host("http", req).into_result()?;
        let status = resp.get("status").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
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
        let text = if binary { String::new() } else { declared.unwrap_or(codepage).decode(&bytes) };
        let fetched = Fetched { text, bytes, status, url };
        // Some services report errors in a body the script knows how to read;
        // hand those over. Fail at once on an empty body, and on a server
        // error page (a proxy's "502 Bad Gateway"), which a scraping script
        // would otherwise present as the article.
        let error_page = fetched.status >= 500 && fetched.text.trim_start().starts_with('<');
        if !fetched.succeeded() && !binary && (fetched.text.trim().is_empty() || error_page) {
            return Err(fetched.http_error(None));
        }
        Ok(fetched)
    }
}

/// A response handler threw. After a failed request the HTTP status is the
/// real story (an error page the script cannot parse); otherwise it is the
/// script's own error.
fn script_failure(fetched: &Fetched, message: String) -> ModuleError {
    if fetched.succeeded() {
        engine::script_error(message)
    } else {
        fetched.http_error(Some(&message))
    }
}

fn has_scheme(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Transport for LegacyTransport {
    fn call_with(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
        control: &crate::protocol::CallControl,
    ) -> Result<serde_json::Value, crate::protocol::CallError> {
        let bridge = Bridge::install_with(host, control);
        bridge.finish(self.run(method, &params, host))
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

fn read_request<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<RequestData, ModuleError> {
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

/// The installation's `Common.js`, one copy (and, once compiled, one
/// bytecode) for all of its services rather than one per service. The table
/// holds weak references, so the script goes when the last service using it
/// is unloaded; a changed file (a reload after editing) gets a fresh entry.
fn shared_common(path: &Path) -> Result<Arc<engine::Script>, String> {
    static TABLE: parking_lot::Mutex<Vec<(std::path::PathBuf, std::sync::Weak<engine::Script>)>> =
        parking_lot::Mutex::new(Vec::new());
    let text = read_text(path)?;
    let key = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let mut table = TABLE.lock();
    table.retain(|(_, weak)| weak.strong_count() > 0);
    if let Some(script) = table.iter().filter(|(p, _)| *p == key).find_map(|(_, weak)| weak.upgrade()) {
        if script.source == text {
            return Ok(script);
        }
    }
    let script = Arc::new(engine::Script::new("Common.js", text));
    table.push((key, Arc::downgrade(&script)));
    Ok(script)
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
        let script = Arc::new(LegacyScript {
            common: None,
            service: engine::Script::new("Test Service/Service.js", service),
            folder: "Test Service".into(),
        });
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
    fn services_of_one_installation_share_its_common_js() {
        let root = std::env::temp_dir().join(format!("glossa-classic-common-{}", std::process::id()));
        let service = |name: &str| {
            let dir = root.join("Services").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("Service.js"), "function serviceHeader() { return {}; }").unwrap();
            dir
        };
        let (a, b) = (service("A"), service("B"));
        std::fs::write(root.join("Common.js"), "var shared = 1;").unwrap();
        let sa = LegacyScript::from_dir(&a).unwrap();
        let sb = LegacyScript::from_dir(&b).unwrap();
        assert!(Arc::ptr_eq(sa.common.as_ref().unwrap(), sb.common.as_ref().unwrap()));
        // Edited on disk: a reload reads the new text instead of reusing.
        std::fs::write(root.join("Common.js"), "var shared = 2;").unwrap();
        let sc = LegacyScript::from_dir(&a).unwrap();
        assert_eq!(sc.common.as_ref().unwrap().source, "var shared = 2;");
        assert!(!Arc::ptr_eq(sa.common.as_ref().unwrap(), sc.common.as_ref().unwrap()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn compat_helpers_behave_like_the_classic_ones() {
        engine::with_context(compat_helpers).unwrap();
    }

    fn check(ctx: &Ctx<'_>, expr: &str) -> serde_json::Value {
        let v = engine::eval(ctx, expr, "t").unwrap();
        engine::to_json(ctx, &v).unwrap()
    }

    fn compat_helpers(ctx: &Ctx<'_>) {
        engine::eval(ctx, COMPAT, "compat").unwrap();
        assert_eq!(check(ctx, "format('{0}-{1}-{0}', 'a', 'b')"), "a-b-a");
        assert_eq!(check(ctx, "stringFindSub('xx<b>bold</b>yy', '<b>', false, '</b>', false)"), "bold");
        assert_eq!(check(ctx, "stringFindSub('xx<b>bold</b>yy', '<b>', true, '</b>', true)"), "<b>bold</b>");
        assert_eq!(check(ctx, "stripHtml('<p>a&nbsp;<b>b</b></p><br/><script>x</script>c')"), "a b\r\nc");
        assert_eq!(check(ctx, "trimString('  x  ')"), "x");
        assert_eq!(check(ctx, "unquoteHtml('&lt;a&gt; &amp; &quot;q&quot; &#39;')"), "<a> & \"q\" '");
        assert_eq!(check(ctx, "removeEmptyLines('a\\n\\n\\n\\nb\\n')"), "a\r\n\r\nb");
        assert_eq!(
            check(ctx, "updateHtmlLinks('<a href=\"/x\"><img src=\"//cdn/y\">', 'https://site.org')"),
            "<a href=\"https://site.org/x\"><img src=\"https://cdn/y\">"
        );
        // A long query is cut at the last encoded space.
        let cut = check(ctx, "encodeGetParam(new Array(400).join('word ')).length");
        assert!(cut.as_u64().unwrap() <= 1800);
        assert_eq!(check(ctx, "new RequestData(HttpMethod.GET, '/x').headers.indexOf('Accept: */*') === 0"), true);
        assert_eq!(check(ctx, "parseJSON(\"{a: 'single quotes'}\").a"), "single quotes");
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

    /// The header and language table every test script below shares.
    const HEAD: &str = r#"
        function serviceHeader() { return new ServiceHeader(902, "Hops", "", Capability.TRANSLATE | Capability.DICTIONARY); }
        SupportedLanguages = [-1, "auto"]; SupportedLanguages[17] = "en"; SupportedLanguages[46] = "ru";
    "#;

    fn translate_with(
        p: &ProtocolProvider<LegacyTransport>,
        http: &MockHttp,
        text: &str,
    ) -> glossa_core::Result<String> {
        let opts = OptionValues::new();
        with_test_host(http, &opts, |host| {
            p.translate(host, &TranslateRequest::new(text, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
                .map(|t| t.text)
        })
    }

    #[test]
    fn service_host_gets_the_capability_and_bare_hosts_get_a_scheme() {
        let service = format!(
            r#"{HEAD}
            function serviceHost(a, b, c) {{ return a == Capability.DICTIONARY ? "https://" + codeFromLanguage(b) + ".dict.test" : "tr.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/t?q=" + encodeGetParam(a)); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(b, c, d); }}
            function serviceDictionaryRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/w/" + a); }}
            function serviceDictionaryResponse(a, b, c, d) {{ return new ResponseData(b, c, d, serviceHost(Capability.DICTIONARY, c, d) + "/w/" + a); }}"#
        );
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Get, "https://tr.test/t?q=hi", 200, "привет").on(
            Method::Get,
            "https://en.dict.test/w/run",
            200,
            "<b>run</b>",
        );
        assert_eq!(translate_with(&p, &http, "hi").unwrap(), "привет");
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let req = DictRequest { text: "run".into(), from: LangId::ENGLISH, to: LangId::RUSSIAN, exact: true };
            let a = p.dictionary(host, &req).unwrap();
            assert_eq!(a.link.as_deref(), Some("https://en.dict.test/w/run"));
        });
    }

    #[test]
    fn chained_responses_add_to_the_translation() {
        // Translate, then chain a lookup whose response holds only the extra
        // block, the way the Microsoft script appends its dictionary.
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://svc.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/t"); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(b, c, d, null, "lookupRequest"); }}
            function lookupRequest(a, b, c, prev) {{
                return new RequestData(HttpMethod.GET, "/l?from=" + codeFromLanguage(b) + "&had=" + encodeGetParam(prev.translation), "", "", null, "lookupResponse");
            }}
            function lookupResponse(a, b, c, d) {{ return new ResponseData(Const.NL2 + "Nouns: " + b, c, d); }}"#
        );
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 200, "дом").on(
            Method::Get,
            "https://svc.test/l?from=en&had=%D0%B4%D0%BE%D0%BC",
            200,
            "дом, здание",
        );
        assert_eq!(translate_with(&p, &http, "house").unwrap(), "дом\n\nNouns: дом, здание");

        // An empty extra block leaves the translation alone.
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 200, "дом").on(
            Method::Get,
            "https://svc.test/l",
            200,
            "",
        );
        let p = provider(&service.replace("Const.NL2 + \"Nouns: \" + b", "\"\""));
        assert_eq!(translate_with(&p, &http, "house").unwrap(), "дом");
    }

    #[test]
    fn a_post_without_a_content_type_is_sent_as_a_form() {
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://svc.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.POST, "/t", "text=" + encodePostParam(a), getHeader()); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(b, c, d); }}"#
        );
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Post, "https://svc.test/t", 200, "дом");
        assert_eq!(translate_with(&p, &http, "house").unwrap(), "дом");
        let req = &http.requests()[0];
        assert!(req.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/x-www-form-urlencoded"));
    }

    #[test]
    fn a_next_handler_that_is_not_a_function_ends_the_chain() {
        // Seen in the wild: the source text passed as the fifth argument.
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://svc.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/t"); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(b, c, d, null, a); }}"#
        );
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 200, "Погода хорошая.");
        assert_eq!(translate_with(&p, &http, "The weather is nice.").unwrap(), "Погода хорошая.");
    }

    #[test]
    fn a_failed_request_is_reported_by_its_status() {
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://svc.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/t?q=" + a); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(parseJSON(b).response.text, c, d); }}"#
        );
        let p = provider(&service);
        // The script chokes on the error body: the status is what is reported.
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 404, r#"{"detail":"Not Found"}"#);
        let msg = translate_with(&p, &http, "x").unwrap_err().to_string();
        assert!(msg.contains("HTTP 404 from https://svc.test/t") && msg.contains("TypeError"), "{msg}");
        assert!(!msg.contains("q=x"), "the query (the user's text) stays out of the message: {msg}");

        // A script that swallows the problem and returns nothing gets the same.
        let quiet = service.replace("parseJSON(b).response.text", "''");
        let p = provider(&quiet);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 502, "<html>Bad Gateway</html>");
        let msg = translate_with(&p, &http, "x").unwrap_err().to_string();
        assert!(msg.contains("HTTP 502"), "{msg}");
    }

    #[test]
    fn a_reason_in_the_data_slot_explains_an_empty_translation() {
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://svc.test"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.GET, "/t"); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData("", 0, d, "Translation error: quota"); }}"#
        );
        let p = provider(&service);
        let http = MockHttp::new().on(Method::Get, "https://svc.test/t", 200, "{}");
        let msg = translate_with(&p, &http, "x").unwrap_err().to_string();
        assert!(msg.contains("Translation error: quota"), "{msg}");
    }

    #[test]
    fn host_utils_and_the_htmlfile_document() {
        engine::with_context(host_utils).unwrap();
    }

    fn host_utils(ctx: &Ctx<'_>) {
        engine::eval(ctx, LEGACY_HOST, "host").unwrap();
        engine::eval(ctx, COMPAT, "compat").unwrap();
        assert_eq!(check(ctx, "Utils.md5('')"), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(check(ctx, "Utils.md5('привет')"), "608333adc72f545078ede3aad71bfe74");

        let page = r#"<html><body><div id="header"><div>logo</div><div>menu</div></div>
            <div id="content" class="c"><p>article <img src="x.png"> text</p></div>
            <div id='page'><textarea id="out">a &amp; b</textarea></div><div id="footer">foot</div></body></html>"#;
        engine::set_global_json(ctx, "page", &serde_json::json!(page)).unwrap();
        let out = check(
            ctx,
            r#"var doc = new ActiveXObject("htmlfile"); doc.write(page);
               var h = doc.getElementById("header"); if (h) h.parentNode.removeChild(h);
               var f = doc.getElementById("footer"); f.parentNode.removeChild(f);
               doc.getElementById("content").style.margin = "0";
               var p = doc.getElementById("page"); p.style.padding = "0"; p.style.width = "100%";
               [doc.getElementById("missing") === null, doc.getElementById("out").value,
                doc.getElementById("content").getAttribute("class"), doc.documentElement.outerHTML]"#,
        );
        assert_eq!(out[0], true);
        assert_eq!(out[1], "a & b");
        assert_eq!(out[2], "c");
        let html = out[3].as_str().unwrap();
        assert!(!html.contains("logo") && !html.contains("menu") && !html.contains("foot"), "{html}");
        assert!(html.contains(r#"<div style="margin:0" id="content" class="c">"#), "{html}");
        assert!(html.contains(r#"<div style="padding:0;width:100%" id='page'>"#), "{html}");
        assert!(html.contains("article <img src=\"x.png\"> text"), "{html}");
    }

    #[test]
    fn a_missing_bing_session_is_reported_instead_of_a_script_error() {
        let service = format!(
            r#"{HEAD}
            function serviceHost() {{ return "https://www.bing.com"; }}
            function serviceTranslateRequest(a, b, c) {{ return new RequestData(HttpMethod.POST, "/t", "key=" + Options.BingKey.match(/[0-9]+$/g)); }}
            function serviceTranslateResponse(a, b, c, d) {{ return new ResponseData(b, c, d); }}"#
        );
        let p = provider(&service);
        let http =
            MockHttp::new().on(Method::Get, "https://www.bing.com/translator", 200, "<html>no session here</html>");
        let msg = translate_with(&p, &http, "x").unwrap_err().to_string();
        assert!(msg.contains("Bing Translator session") && msg.contains("IG"), "{msg}");
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
