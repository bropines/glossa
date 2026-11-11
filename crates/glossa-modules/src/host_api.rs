//! The host side of the module protocol: what a module may ask of us.
//!
//! Modules of every runtime call back with `http`, `log`, `cache_get`, `t`,
//! `cache_set`, `assets` and the `image.*` operations. [`HostApi`] answers those for
//! one module during one call, applying its network permissions — a module
//! asking for a host outside its allowlist gets an error, not a request —
//! and holding that call's images. The dispatch is by method name only, so
//! any runtime whose transport forwards host calls gets all of them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use glossa_core::Error;
use glossa_module_sdk::wire::{self, ErrorKind, ModuleError, Reply};
use glossa_net::{Http, HttpRequest, HttpResponse, Method};
use parking_lot::Mutex;

use crate::manifest::Permissions;

/// An [`Http`] that refuses URLs outside a module's allowlist.
pub struct ScopedHttp<'a> {
    pub inner: &'a dyn Http,
    pub permissions: &'a Permissions,
    pub module_id: &'a str,
}

impl Http for ScopedHttp<'_> {
    fn send(&self, mut request: HttpRequest) -> Result<HttpResponse, Error> {
        if !self.permissions.allows_url(&request.url) {
            return Err(Error::Module {
                module: self.module_id.to_owned(),
                detail: format!(
                    "network access to {} is not in the module's permissions",
                    crate::manifest::url_host(&request.url).unwrap_or_else(|| request.url.clone())
                ),
            });
        }
        if request.provider.is_empty() {
            request.provider = self.module_id.to_owned();
        }
        self.inner.send(request)
    }
}

/// A small key-value cache shared by all modules, partitioned by module id.
/// In memory only: it exists to keep tokens and session ids between calls, not
/// to persist data.
#[derive(Default)]
pub struct ModuleCache {
    entries: Mutex<HashMap<CacheKey, CacheEntry>>,
    /// Which modules reported a rate limit and until when they are left
    /// alone, by module id. Kept here because the cache is the one thing
    /// that lives across reloads of the modules: applying the settings
    /// must not make a refusing service fair game again.
    pub limits: crate::backoff::Backoff,
}

/// (module id, key).
type CacheKey = (String, String);
/// The value and when it expires, if ever.
type CacheEntry = (serde_json::Value, Option<Instant>);

impl ModuleCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn get(&self, module: &str, key: &str) -> Option<serde_json::Value> {
        let mut map = self.entries.lock();
        let k = (module.to_owned(), key.to_owned());
        match map.get(&k) {
            Some((_, Some(expiry))) if *expiry <= Instant::now() => {
                map.remove(&k);
                None
            }
            Some((v, _)) => Some(v.clone()),
            None => None,
        }
    }

    pub fn set(&self, module: &str, key: &str, value: serde_json::Value, ttl: Option<Duration>) {
        let mut map = self.entries.lock();
        // Bound the cache: a module cannot grow it without limit.
        if map.len() > 4096 {
            let now = Instant::now();
            map.retain(|_, (_, exp)| exp.is_none_or(|e| e > now));
            if map.len() > 4096 {
                map.clear();
            }
        }
        map.insert((module.to_owned(), key.to_owned()), (value, ttl.map(|t| Instant::now() + t)));
    }
}

/// Answers one module's host calls during one call into it.
pub struct HostApi<'a> {
    pub http: ScopedHttp<'a>,
    pub cache: &'a ModuleCache,
    /// The images of this call (`image.*` host calls); dropped with it.
    #[cfg(feature = "image")]
    pub images: Mutex<crate::image_ops::ImageStore>,
    /// The module's assets, for the `assets` host call.
    pub assets: Option<&'a crate::assets::ModuleAssets>,
    /// The module's `[strings]` in the interface language, for the `t` host
    /// call during an action.
    pub strings: Option<&'a std::collections::BTreeMap<String, String>>,
    /// Where `ui_update` host calls go during a panel's action: partial
    /// state, applied to the panel at once. `None` outside one, where the
    /// call is accepted and dropped.
    pub ui_update: Option<&'a (dyn Fn(crate::provider::OptionValues) + Sync)>,
    /// The call's cancel token: once it is set, host calls are refused, so
    /// a module still working for a superseded request stops sending
    /// requests on its behalf and comes back sooner.
    pub cancel: Option<&'a crate::provider::CancelToken>,
}

impl<'a> HostApi<'a> {
    pub fn new(http: ScopedHttp<'a>, cache: &'a ModuleCache) -> Self {
        Self {
            http,
            cache,
            #[cfg(feature = "image")]
            images: Mutex::new(crate::image_ops::ImageStore::new()),
            assets: None,
            strings: None,
            ui_update: None,
            cancel: None,
        }
    }

    /// Refuse host calls once `cancel` is set.
    pub fn with_cancel(mut self, cancel: Option<&'a crate::provider::CancelToken>) -> Self {
        self.cancel = cancel;
        self
    }

    /// Answer the `assets` host call from `assets`.
    pub fn with_assets(mut self, assets: Option<&'a crate::assets::ModuleAssets>) -> Self {
        self.assets = assets;
        self
    }

    /// Answer the `t` host call from `strings`.
    pub fn with_strings(mut self, strings: Option<&'a std::collections::BTreeMap<String, String>>) -> Self {
        self.strings = strings;
        self
    }

    /// Send `ui_update` host calls to `sink`.
    pub fn with_ui_update(mut self, sink: Option<&'a (dyn Fn(crate::provider::OptionValues) + Sync)>) -> Self {
        self.ui_update = sink;
        self
    }

    /// Register encoded image bytes with this call's image API, decoded on
    /// first use. `None` when the image API is not built in.
    pub fn register_image(&self, bytes: Vec<u8>) -> Option<u32> {
        #[cfg(feature = "image")]
        {
            Some(self.images.lock().insert_encoded(bytes))
        }
        #[cfg(not(feature = "image"))]
        {
            let _ = bytes;
            None
        }
    }

    #[cfg(feature = "image")]
    fn image_op(&self, op: &str, params: serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let info = self.images.lock().call(op, &params)?;
        serde_json::to_value(info).map_err(|e| ModuleError::other(e.to_string()))
    }

    #[cfg(not(feature = "image"))]
    fn image_op(&self, _op: &str, _params: serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        Err(ModuleError::new(ErrorKind::Unsupported, "this Glossa was built without the image API"))
    }

    pub fn handle(&self, method: &str, params: serde_json::Value) -> Reply {
        if self.cancel.is_some_and(|c| c.is_cancelled()) {
            return Reply::err(cancelled_error());
        }
        match self.dispatch(method, params) {
            Ok(v) => Reply::ok(v),
            Err(e) => Reply::err(e),
        }
    }

    fn dispatch(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let module = self.http.module_id;
        match method {
            "http" => {
                let req: wire::HttpRequest = serde_json::from_value(params)
                    .map_err(|e| ModuleError::new(ErrorKind::BadRequest, format!("http: {e}")))?;
                let binary = req.binary;
                let resp = self.http.send(to_net_request(req)?).map_err(to_module_error)?;
                Ok(serde_json::to_value(from_net_response(resp, binary)).expect("response serialises"))
            }
            "log" => {
                let level = params.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
                match level {
                    "trace" => tracing::trace!(module, "{message}"),
                    "debug" => tracing::debug!(module, "{message}"),
                    "warn" => tracing::warn!(module, "{message}"),
                    "error" => tracing::error!(module, "{message}"),
                    _ => tracing::info!(module, "{message}"),
                }
                crate::log_tap::emit(module, level, message);
                Ok(serde_json::Value::Null)
            }
            "cache_get" => {
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                Ok(self.cache.get(module, key).unwrap_or(serde_json::Value::Null))
            }
            "cache_set" => {
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let value = params.get("value").cloned().unwrap_or(serde_json::Value::Null);
                let ttl = params.get("ttl_secs").and_then(|v| v.as_u64()).filter(|t| *t > 0).map(Duration::from_secs);
                self.cache.set(module, key, value, ttl);
                Ok(serde_json::Value::Null)
            }
            "assets" => {
                let info = self.assets.map(|a| a.host_info()).unwrap_or_default();
                Ok(serde_json::to_value(info).expect("assets info serialises"))
            }
            // `{"key": "bad_key", "args": ["x"]}`: a module string, for
            // runtimes whose SDK does not read the table sent with the call.
            "t" => {
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let args: Vec<String> = params
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect())
                    .unwrap_or_default();
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let template = self.strings.and_then(|s| s.get(key)).map(String::as_str).unwrap_or(key);
                Ok(wire::format_string(template, &args).into())
            }
            // `{"result": "…so far"}`: partial state for the panel the
            // running action belongs to. The engine bounds it like a reply.
            "ui_update" => {
                let state: crate::provider::OptionValues = match params {
                    serde_json::Value::Object(o) => o.into_iter().collect(),
                    serde_json::Value::Null => Default::default(),
                    _ => {
                        return Err(ModuleError::new(ErrorKind::BadRequest, "ui_update takes an object of state keys"))
                    }
                };
                if let Some(sink) = self.ui_update {
                    if !state.is_empty() {
                        sink(state);
                    }
                }
                Ok(serde_json::Value::Null)
            }
            op if op.starts_with("image.") => self.image_op(&op["image.".len()..], params),
            other => Err(ModuleError::new(ErrorKind::Unsupported, format!("unknown host call {other:?}"))),
        }
    }
}

/// What a host call made for a cancelled request is answered with.
pub fn cancelled_error() -> ModuleError {
    ModuleError::other("the request was cancelled")
}

pub fn to_net_request(req: wire::HttpRequest) -> Result<HttpRequest, ModuleError> {
    let method = Method::parse(&req.method)
        .ok_or_else(|| ModuleError::new(ErrorKind::BadRequest, format!("unknown method {:?}", req.method)))?;
    let body = match (req.body_base64, req.body) {
        (Some(b64), _) => Some(
            base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| ModuleError::new(ErrorKind::BadRequest, format!("body_base64: {e}")))?,
        ),
        (None, Some(text)) => Some(text.into_bytes()),
        (None, None) => None,
    };
    Ok(HttpRequest {
        method,
        url: req.url,
        headers: req.headers.into_iter().collect(),
        body,
        timeout: req.timeout_ms.map(Duration::from_millis),
        codepage: None,
        // A 429 goes straight back to the module, which knows what its
        // service means by it (and can report `rate_limited`); a silent
        // second request from here would only prolong the block.
        no_retry_when_limited: true,
        ..Default::default()
    })
}

pub fn from_net_response(resp: HttpResponse, binary: bool) -> wire::HttpResponse {
    // Repeated headers (Set-Cookie above all) are joined with newlines rather
    // than letting the last one win.
    let mut headers: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (k, v) in &resp.headers {
        headers
            .entry(k.to_ascii_lowercase())
            .and_modify(|existing| {
                existing.push('\n');
                existing.push_str(v);
            })
            .or_insert_with(|| v.clone());
    }
    // `Retry-After` in seconds, read for the module (an HTTP date, which
    // few services send, is left to the header).
    let retry_after_secs = matches!(resp.status, 429 | 503)
        .then(|| headers.get("retry-after").and_then(|v| v.lines().next()?.trim().parse::<u64>().ok()))
        .flatten();
    if binary {
        wire::HttpResponse {
            status: resp.status,
            headers,
            body: String::new(),
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(&resp.body)),
            retry_after_secs,
        }
    } else {
        wire::HttpResponse { status: resp.status, headers, body: resp.text(), body_base64: None, retry_after_secs }
    }
}

/// Our error, as a module sees it.
pub fn to_module_error(e: Error) -> ModuleError {
    let kind = match &e {
        Error::Network(_) | Error::Timeout(_) => ErrorKind::Network,
        Error::RateLimited { .. } => ErrorKind::RateLimited,
        Error::HttpStatus { .. } => ErrorKind::Http,
        Error::MissingCredential { .. } => ErrorKind::MissingCredential,
        _ => ErrorKind::Other,
    };
    let retry_after_secs = match &e {
        Error::RateLimited { retry_after_secs, .. } => *retry_after_secs,
        _ => None,
    };
    // A refusal of ours already names the module ("module x failed: …");
    // the module's own failure will say so again, so only the reason
    // travels.
    let message = match e {
        Error::Module { detail, .. } | Error::ModuleAt { detail, .. } => detail,
        other => other.to_string(),
    };
    ModuleError { kind, message, retry_after_secs, location: None }
}

/// A module's error, as the engine sees it.
pub fn from_module_error(module: &str, name: &str, e: ModuleError) -> Error {
    match e.kind {
        ErrorKind::MissingCredential => Error::MissingCredential { provider: name.to_owned(), what: e.message },
        ErrorKind::Unsupported => Error::Unsupported { provider: name.to_owned(), capability: "this operation" },
        ErrorKind::RateLimited => {
            Error::RateLimited { provider: name.to_owned(), retry_after_secs: e.retry_after_secs }
        }
        ErrorKind::Network => Error::Network(format!("{name}: {}", e.message)),
        ErrorKind::BadResponse => Error::BadResponse { provider: name.to_owned(), detail: e.message },
        ErrorKind::NotFound => Error::NoTextRecognised,
        // The language is not known here; `ProtocolProvider` puts the
        // request's in. What the module said about it is kept: often the
        // one useful part ("download rec-ch.onnx for Chinese").
        ErrorKind::SourceLanguage => Error::SourceLanguageUnsupported {
            provider: name.to_owned(),
            lang: glossa_core::LangId::UNKNOWN,
            detail: e.message,
        },
        ErrorKind::TargetLanguage => Error::TargetLanguageUnsupported {
            provider: name.to_owned(),
            lang: glossa_core::LangId::UNKNOWN,
            detail: e.message,
        },
        _ => match e.location {
            Some(l) if !l.file.is_empty() => Error::ModuleAt {
                module: module.to_owned(),
                detail: e.message,
                place: glossa_core::error::SourcePlace { file: l.file, line: l.line, column: l.column },
            },
            _ => Error::Module { module: module.to_owned(), detail: e.message },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;

    fn api<'a>(http: &'a MockHttp, perms: &'a Permissions, cache: &'a ModuleCache) -> HostApi<'a> {
        HostApi::new(ScopedHttp { inner: http, permissions: perms, module_id: "m" }, cache)
    }

    #[cfg(feature = "image")]
    #[test]
    fn image_calls_reach_the_image_api_and_live_for_the_call() {
        let http = MockHttp::new();
        let perms = Permissions::default();
        let cache = ModuleCache::default();
        let api = api(&http, &perms, &cache);
        let made = api.handle("image.new", serde_json::json!({"width": 4, "height": 3})).into_result().unwrap();
        assert_eq!(made["width"], 4);
        let h = made["handle"].clone();
        let png = api.handle("image.encode", serde_json::json!({"handle": h})).into_result().unwrap();
        assert_eq!(png["mime"], "image/png");
        let err = api.handle("image.crop", serde_json::json!({"handle": 77})).into_result().unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadRequest);
        let second = HostApi::new(ScopedHttp { inner: &http, permissions: &perms, module_id: "m" }, &cache);
        assert!(
            second.handle("image.info", serde_json::json!({"handle": h})).into_result().is_err(),
            "a new call starts empty"
        );
    }

    #[test]
    fn http_calls_respect_the_allowlist() {
        let http = MockHttp::new().on(Method::Get, "https://ok.example/", 200, "{\"a\":1}");
        let perms = Permissions { network: vec!["ok.example".into()] };
        let cache = ModuleCache::default();
        let api = api(&http, &perms, &cache);

        let reply = api.handle("http", serde_json::json!({"url": "https://ok.example/x"}));
        let v = reply.into_result().unwrap();
        assert_eq!(v["status"], 200);
        assert_eq!(v["body"], "{\"a\":1}");

        let denied = api.handle("http", serde_json::json!({"url": "https://evil.example/x"}));
        let err = denied.into_result().unwrap_err();
        assert!(err.message.contains("not in the module's permissions"), "{err:?}");
        assert_eq!(http.requests().len(), 1, "the denied request never went out");
    }

    #[test]
    fn binary_responses_come_back_base64() {
        let http = MockHttp::new().on(Method::Get, "https://ok.example/", 200, "\u{1}\u{2}");
        let perms = Permissions::allow_all();
        let cache = ModuleCache::default();
        let v = api(&http, &perms, &cache)
            .handle("http", serde_json::json!({"url": "https://ok.example/a.mp3", "binary": true}))
            .into_result()
            .unwrap();
        assert_eq!(v["body_base64"], "AQI=");
    }

    #[test]
    fn cache_is_partitioned_and_expires() {
        let cache = ModuleCache::default();
        cache.set("a", "k", serde_json::json!(1), None);
        cache.set("b", "k", serde_json::json!(2), Some(Duration::from_millis(0)));
        assert_eq!(cache.get("a", "k"), Some(serde_json::json!(1)));
        assert_eq!(cache.get("b", "k"), None, "expired immediately");
        assert_eq!(cache.get("c", "k"), None);

        let http = MockHttp::new();
        let perms = Permissions::default();
        let api = api(&http, &perms, &cache);
        api.handle("cache_set", serde_json::json!({"key": "t", "value": "tok"}));
        assert_eq!(api.handle("cache_get", serde_json::json!({"key": "t"})).into_result().unwrap(), "tok");
    }

    #[test]
    fn unknown_host_calls_are_errors() {
        let http = MockHttp::new();
        let perms = Permissions::default();
        let cache = ModuleCache::default();
        assert!(api(&http, &perms, &cache).handle("format_disk", serde_json::json!({})).into_result().is_err());
    }

    #[test]
    fn errors_map_across_the_boundary() {
        let e = from_module_error("m", "My", ModuleError::missing_option("api_key"));
        assert!(matches!(e, Error::MissingCredential { ref what, .. } if what == "api_key"));
        let back = to_module_error(Error::Timeout(5));
        assert_eq!(back.kind, ErrorKind::Network);
    }
}
