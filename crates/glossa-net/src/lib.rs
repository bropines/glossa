//! HTTP for every provider and module.
//!
//! All network traffic goes through one [`HttpClient`], built from the user's
//! [`Network`](glossa_core::config::Network) settings, so the proxy, the
//! timeout and the retry policy apply everywhere — to built-in services, to
//! JavaScript and Python modules (which reach the network through the host),
//! and to native ones. Providers take `&dyn Http` rather than the concrete
//! client so tests can hand them canned responses.
//!
//! The client is synchronous. Glossa runs each job on a worker thread, and a
//! blocking client keeps the binary and the resident memory small — no async
//! runtime is linked in.

mod codepage;
mod form;

use std::time::{Duration, Instant};

use glossa_core::config::{Network, ProxyMode};
use glossa_core::Error;

pub use codepage::Codepage;
pub use form::{form_encode, url_encode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Head => "HEAD",
        }
    }

    pub fn parse(s: &str) -> Option<Method> {
        Some(match s.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "HEAD" => Method::Head,
            _ => return None,
        })
    }
}

/// One request.
#[derive(Debug, Clone, Default)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// Overrides the client's timeout for this request.
    pub timeout: Option<Duration>,
    /// How to decode the response when it does not say. Classic scripts
    /// asked for Windows-1251 this way.
    pub codepage: Option<Codepage>,
    /// Which provider is asking, for error messages and logs.
    pub provider: String,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: Method::Get, url: url.into(), ..Default::default() }
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self { method: Method::Post, url: url.into(), ..Default::default() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// A JSON body, with the content type set.
    pub fn json(self, value: &serde_json::Value) -> Self {
        self.header("Content-Type", "application/json; charset=utf-8")
            .body(serde_json::to_vec(value).expect("json value serialises"))
    }

    /// A form body, with the content type set.
    pub fn form(self, fields: &[(&str, &str)]) -> Self {
        self.header("Content-Type", "application/x-www-form-urlencoded; charset=utf-8").body(form_encode(fields))
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Parse a header block as the classic scripts wrote it: `Name: value`
    /// lines separated by CRLF.
    pub fn headers_from_block(mut self, block: &str) -> Self {
        for line in block.split(['\r', '\n']) {
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim();
                if !name.is_empty() {
                    self.headers.push((name.to_owned(), value.trim().to_owned()));
                }
            }
        }
        self
    }
}

/// One response. Any status is a response; deciding that 4xx is an error is
/// the provider's business, because some APIs put useful detail in the body.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The decoding hint carried over from the request.
    pub codepage: Option<Codepage>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The body as text: the charset the server named, else the request's
    /// code page hint, else UTF-8 (lossily).
    pub fn text(&self) -> String {
        let declared = self
            .header("content-type")
            .and_then(|ct| ct.split(';').find_map(|p| p.trim().strip_prefix("charset=")))
            .map(|c| c.trim_matches('"'))
            .and_then(Codepage::from_label);
        declared.or(self.codepage).unwrap_or(Codepage::Utf8).decode(&self.body)
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    /// Turn a non-2xx status into the right [`Error`], reading `Retry-After`
    /// for 429 and 503.
    pub fn error_for_status(self, provider: &str) -> Result<HttpResponse, Error> {
        if self.is_success() {
            return Ok(self);
        }
        if self.status == 429 || self.status == 503 {
            let retry = self.header("retry-after").and_then(|v| v.trim().parse().ok());
            if self.status == 429 || retry.is_some() {
                return Err(Error::RateLimited { provider: provider.to_owned(), retry_after_secs: retry });
            }
        }
        Err(Error::HttpStatus { provider: provider.to_owned(), status: self.status, body: self.text() })
    }
}

/// Anything that can perform a request. Implemented by [`HttpClient`] and by
/// test doubles.
pub trait Http: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error>;

    /// Send a request and hand a successful (2xx) response's body to
    /// `on_chunk` as it arrives, for streamed answers (server-sent events).
    /// `on_chunk` returns `false` to stop reading. The returned response has
    /// an empty body when it was streamed; any other status comes back whole,
    /// so error bodies read as usual.
    ///
    /// The default sends normally and delivers the body as one chunk, which
    /// is what test doubles want.
    fn send_streaming(
        &self,
        request: HttpRequest,
        on_chunk: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<HttpResponse, Error> {
        let mut response = self.send(request)?;
        if (200..300).contains(&response.status) {
            let body = std::mem::take(&mut response.body);
            on_chunk(&body);
        }
        Ok(response)
    }
}

/// The real client.
pub struct HttpClient {
    agent: ureq::Agent,
    timeout: Duration,
    max_retries: u32,
    user_agent: String,
}

/// What we send when nobody asks for anything else: a current desktop
/// browser, because several free endpoints refuse obvious bots.
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Bodies larger than this are refused, so a misbehaving endpoint cannot make
/// us allocate without bound.
const MAX_BODY: u64 = 32 * 1024 * 1024;

impl HttpClient {
    pub fn new(settings: &Network) -> Result<HttpClient, Error> {
        let timeout = Duration::from_secs(settings.timeout_secs.max(1));
        let proxy = resolve_proxy(settings)?;
        let mut cfg = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .max_redirects(8)
            .user_agent(if settings.user_agent.is_empty() {
                DEFAULT_USER_AGENT.to_owned()
            } else {
                settings.user_agent.clone()
            });
        cfg = cfg.proxy(proxy).tls_config(tls_config());
        Ok(HttpClient {
            agent: cfg.build().into(),
            timeout,
            max_retries: settings.max_retries,
            user_agent: settings.user_agent.clone(),
        })
    }

    /// A client with default settings and no proxy, for tools and tests.
    pub fn plain() -> HttpClient {
        let settings = Network {
            proxy: glossa_core::config::Proxy { mode: ProxyMode::None, ..Default::default() },
            ..Default::default()
        };
        HttpClient::new(&settings).expect("a proxy-less client always builds")
    }

    fn send_once(&self, request: &HttpRequest) -> Result<HttpResponse, Error> {
        let provider = if request.provider.is_empty() { "http" } else { &request.provider };
        let mut response = self.open(request)?;
        let (status, headers) = status_and_headers(&response);
        let body = if request.method == Method::Head {
            Vec::new()
        } else {
            response
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()
                .map_err(|e| Error::Network(format!("{provider}: reading body: {e}")))?
        };
        Ok(HttpResponse { status, headers, body, codepage: request.codepage })
    }

    /// Make the request and return the response with its body unread.
    fn open(&self, request: &HttpRequest) -> Result<ureq::http::Response<ureq::Body>, Error> {
        let provider = if request.provider.is_empty() { "http" } else { &request.provider };
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
        for (name, value) in &request.headers {
            // ureq negotiates compression itself and only undoes what it
            // asked for; a caller-supplied Accept-Encoding (the classic
            // scripts all send one) could get us a body we cannot read.
            if name.eq_ignore_ascii_case("accept-encoding") || name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            if name.eq_ignore_ascii_case("user-agent") && !self.user_agent.is_empty() {
                continue; // the user's explicit setting wins
            }
            builder = builder.header(name.as_str(), value.as_str());
        }

        let timeout = request.timeout.filter(|t| *t != self.timeout);
        let call = match &request.body {
            Some(body) => {
                let req = builder.body(body.clone()).map_err(|e| Error::network(format!("bad request: {e}")))?;
                self.run(req, timeout)
            }
            None => {
                let req = builder.body(()).map_err(|e| Error::network(format!("bad request: {e}")))?;
                self.run(req, timeout)
            }
        };

        call.map_err(|e| match e {
            ureq::Error::Timeout(_) => Error::Timeout(request.timeout.unwrap_or(self.timeout).as_millis() as u64),
            other => Error::Network(format!("{provider}: {other}")),
        })
    }

    /// Run a request, with its own timeout if it asked for one.
    fn run<S: ureq::AsSendBody>(
        &self,
        req: ureq::http::Request<S>,
        timeout: Option<Duration>,
    ) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        match timeout {
            Some(t) => {
                let req = self.agent.configure_request(req).timeout_global(Some(t)).build();
                self.agent.run(req)
            }
            None => self.agent.run(req),
        }
    }
}

fn status_and_headers(response: &ureq::http::Response<ureq::Body>) -> (u16, Vec<(String, String)>) {
    let headers = response
        .headers()
        .iter()
        .map(|(n, v)| (n.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    (response.status().as_u16(), headers)
}

impl Http for HttpClient {
    fn send_streaming(
        &self,
        request: HttpRequest,
        on_chunk: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<HttpResponse, Error> {
        use std::io::Read as _;
        let provider = if request.provider.is_empty() { "http" } else { request.provider.as_str() };
        // No retries: once part of an answer has been shown, repeating the
        // request would show it twice. A failure to connect is reported as
        // is, and the engine's fail-over takes it from there.
        let mut response = self.open(&request)?;
        let (status, headers) = status_and_headers(&response);
        if !(200..300).contains(&status) {
            let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec().unwrap_or_default();
            return Ok(HttpResponse { status, headers, body, codepage: request.codepage });
        }
        let mut reader = response.body_mut().with_config().limit(MAX_BODY).reader();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if !on_chunk(&buf[..n]) {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(Error::Timeout(request.timeout.unwrap_or(self.timeout).as_millis() as u64));
                }
                Err(e) => return Err(Error::Network(format!("{provider}: reading body: {e}"))),
            }
        }
        Ok(HttpResponse { status, headers, body: Vec::new(), codepage: request.codepage })
    }

    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            let result = self.send_once(&request);
            let retry = match &result {
                Err(e) if e.is_transient() => true,
                Ok(r) => matches!(r.status, 429 | 500 | 502 | 503 | 504),
                Err(_) => false,
            };
            if !retry || attempt >= self.max_retries {
                if let Err(e) = &result {
                    tracing::debug!(url = %redact(&request.url), error = %e, "request failed");
                } else {
                    tracing::trace!(
                        url = %redact(&request.url),
                        ms = started.elapsed().as_millis() as u64,
                        "request done"
                    );
                }
                return result;
            }
            attempt += 1;
            let backoff = Duration::from_millis(300 * (1 << attempt.min(4)));
            tracing::debug!(url = %redact(&request.url), attempt, "retrying in {backoff:?}");
            std::thread::sleep(backoff);
        }
    }
}

/// Which certificates a server must chain up to.
///
/// On Windows and macOS the operating system decides, as it does for the
/// browser and as WinHTTP did for the classic program: the system store
/// (with corporate and user-installed roots), revocation, and fetching an
/// intermediate certificate a misconfigured server forgot to send — a
/// bundled root list fails those servers with "UnknownIssuer". Linux keeps
/// the bundled Mozilla roots, which work without any system setup.
fn tls_config() -> ureq::tls::TlsConfig {
    let roots = if cfg!(any(windows, target_os = "macos")) {
        ureq::tls::RootCerts::PlatformVerifier
    } else {
        ureq::tls::RootCerts::WebPki
    };
    ureq::tls::TlsConfig::builder().root_certs(roots).build()
}

/// Strip the query string from a URL before it reaches a log, since keys and
/// the user's text travel there.
pub fn redact(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Hosts that never go through a proxy: a local server (a self-hosted DeepLX
/// or LibreTranslate, say) is unreachable from the proxy's side.
const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];

fn resolve_proxy(settings: &Network) -> Result<Option<ureq::Proxy>, Error> {
    let (url, bypass) = match settings.proxy.mode {
        ProxyMode::None => return Ok(None),
        ProxyMode::Manual => (settings.proxy.url(), Vec::new()),
        ProxyMode::System | ProxyMode::Auto => match system_proxy() {
            Some((url, bypass)) => (Some(url), bypass),
            None => (None, Vec::new()),
        },
    };
    let Some(u) = url else { return Ok(None) };
    let bad = |e: ureq::Error| Error::Config(format!("proxy {u:?}: {e}"));
    // Parsed once for its parts, then rebuilt with the bypass list, which
    // only the builder takes.
    let parsed = ureq::Proxy::new(&u).map_err(bad)?;
    let mut builder = ureq::Proxy::builder(parsed.protocol()).host(parsed.host()).port(parsed.port());
    if let Some(user) = parsed.username() {
        builder = builder.username(user);
        if let Some(password) = parsed.password() {
            builder = builder.password(password);
        }
    }
    for host in LOOPBACK.iter().copied().chain(bypass.iter().map(String::as_str)) {
        builder = builder.no_proxy(host);
    }
    builder.build().map(Some).map_err(bad)
}

/// The system proxy and the hosts that bypass it: environment variables
/// first (the convention on Unix and what most tools honour, with
/// `NO_PROXY`), then on Windows the per-user WinINet setting (with its
/// `ProxyOverride` list).
fn system_proxy() -> Option<(String, Vec<String>)> {
    for var in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                let bypass = ["NO_PROXY", "no_proxy"].iter().find_map(|k| std::env::var(k).ok()).unwrap_or_default();
                return Some((v, split_bypass(&bypass, ',')));
            }
        }
    }
    windows_proxy()
}

/// A bypass list as single host patterns. WinINet's `<local>` (hosts
/// without a dot) has no equivalent; loopback is always bypassed anyway.
fn split_bypass(list: &str, separator: char) -> Vec<String> {
    list.split(separator).map(str::trim).filter(|s| !s.is_empty() && *s != "<local>").map(str::to_owned).collect()
}

#[cfg(windows)]
fn windows_proxy() -> Option<(String, Vec<String>)> {
    use winreg::enums::HKEY_CURRENT_USER;
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
    Some((parse_wininet_proxy(&server)?, split_bypass(&bypass, ';')))
}

#[cfg(not(windows))]
fn windows_proxy() -> Option<(String, Vec<String>)> {
    None
}

/// WinINet writes either `host:port` or `http=h:p;https=h:p;socks=h:p`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_wininet_proxy(server: &str) -> Option<String> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    if !server.contains('=') {
        return Some(if server.contains("://") { server.to_owned() } else { format!("http://{server}") });
    }
    let mut map = std::collections::HashMap::new();
    for part in server.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            map.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    if let Some(v) = map.get("https").or_else(|| map.get("http")) {
        return Some(format!("http://{v}"));
    }
    map.get("socks").map(|v| format!("socks5://{v}"))
}

/// A canned-response [`Http`] for tests: each request is matched against the
/// routes in order, by method and URL prefix.
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    type Route = (Method, String, HttpResponse);

    #[derive(Default)]
    pub struct MockHttp {
        routes: Mutex<Vec<Route>>,
        pub seen: Mutex<Vec<HttpRequest>>,
    }

    impl MockHttp {
        pub fn new() -> Self {
            Self::default()
        }

        /// Answer `method url_prefix…` with `status` and `body`.
        pub fn on(self, method: Method, url_prefix: &str, status: u16, body: &str) -> Self {
            self.routes.lock().unwrap().push((
                method,
                url_prefix.to_owned(),
                HttpResponse {
                    status,
                    headers: vec![("content-type".into(), "application/json; charset=utf-8".into())],
                    body: body.as_bytes().to_vec(),
                    codepage: None,
                },
            ));
            self
        }

        pub fn requests(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Http for MockHttp {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
            self.seen.lock().unwrap().push(request.clone());
            let routes = self.routes.lock().unwrap();
            routes
                .iter()
                .find(|(m, prefix, _)| *m == request.method && request.url.starts_with(prefix.as_str()))
                .map(|(_, _, r)| HttpResponse { codepage: request.codepage, ..r.clone() })
                .ok_or_else(|| Error::Network(format!("no mock route for {} {}", request.method.as_str(), request.url)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_blocks_parse_like_the_classic_scripts_wrote_them() {
        let req = HttpRequest::get("https://x")
            .headers_from_block("Accept: */*\r\nAccept-Language: ru;q=0.8\r\n\r\nX-Odd:   spaced  ");
        assert_eq!(
            req.headers,
            vec![
                ("Accept".into(), "*/*".into()),
                ("Accept-Language".into(), "ru;q=0.8".into()),
                ("X-Odd".into(), "spaced".into()),
            ]
        );
    }

    #[test]
    fn response_text_prefers_the_declared_charset() {
        // "привет" in Windows-1251.
        let cp1251 = vec![0xef, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2];
        let declared = HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html; charset=windows-1251".into())],
            body: cp1251.clone(),
            codepage: None,
        };
        assert_eq!(declared.text(), "привет");

        // No declaration: the request's hint applies.
        let hinted = HttpResponse { status: 200, headers: vec![], body: cp1251, codepage: Some(Codepage::Windows1251) };
        assert_eq!(hinted.text(), "привет");

        // Neither: UTF-8.
        let utf8 =
            HttpResponse { status: 200, headers: vec![], body: "привет".as_bytes().to_vec(), codepage: None };
        assert_eq!(utf8.text(), "привет");
    }

    #[test]
    fn statuses_map_to_the_right_errors() {
        let r = |status: u16, headers: Vec<(String, String)>| HttpResponse {
            status,
            headers,
            body: b"nope".to_vec(),
            codepage: None,
        };
        assert!(r(200, vec![]).error_for_status("p").is_ok());
        match r(429, vec![("Retry-After".into(), "7".into())]).error_for_status("p") {
            Err(Error::RateLimited { retry_after_secs: Some(7), .. }) => {}
            other => panic!("{other:?}"),
        }
        match r(403, vec![]).error_for_status("p") {
            Err(Error::HttpStatus { status: 403, body, .. }) => assert_eq!(body, "nope"),
            other => panic!("{other:?}"),
        }
        // 503 without Retry-After is a plain status error (transient anyway).
        assert!(matches!(r(503, vec![]).error_for_status("p"), Err(Error::HttpStatus { status: 503, .. })));
    }

    #[test]
    fn loopback_and_listed_hosts_bypass_the_proxy() {
        assert_eq!(split_bypass("<local>;*.google.com; 10.* ;", ';'), ["*.google.com", "10.*"]);
        assert_eq!(split_bypass("localhost,.corp.test", ','), ["localhost", ".corp.test"]);

        let settings = Network {
            proxy: glossa_core::config::Proxy {
                mode: ProxyMode::Manual,
                host: "proxy.test".into(),
                port: 3128,
                username: "u ser".into(),
                password: "p@ss".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy = resolve_proxy(&settings).unwrap().unwrap();
        assert_eq!((proxy.host(), proxy.port()), ("proxy.test", 3128));
        assert_eq!((proxy.username(), proxy.password()), (Some("u ser"), Some("p@ss")));
        let uri = |s: &str| s.parse::<ureq::http::Uri>().unwrap();
        assert!(proxy.is_no_proxy(&uri("http://127.0.0.1:1188/translate")));
        assert!(proxy.is_no_proxy(&uri("http://localhost:5000/")));
        assert!(!proxy.is_no_proxy(&uri("https://translate.example.com/")));
    }

    #[test]
    fn wininet_proxy_strings() {
        assert_eq!(parse_wininet_proxy("127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(parse_wininet_proxy("http=h:1;https=h:2;socks=s:3").as_deref(), Some("http://h:2"));
        assert_eq!(parse_wininet_proxy("socks=s:1080").as_deref(), Some("socks5://s:1080"));
        assert_eq!(parse_wininet_proxy("").as_deref(), None);
    }

    #[test]
    fn urls_are_redacted_for_logs() {
        assert_eq!(redact("https://api.x/v1?key=secret&q=hi"), "https://api.x/v1");
        assert_eq!(redact("https://api.x/v1#frag"), "https://api.x/v1");
    }

    #[test]
    fn mock_matches_by_method_and_prefix() {
        let m = mock::MockHttp::new().on(Method::Get, "https://a/", 200, "{\"ok\":1}");
        let r = m.send(HttpRequest::get("https://a/b?c")).unwrap();
        assert_eq!(r.json::<serde_json::Value>().unwrap()["ok"], 1);
        assert!(m.send(HttpRequest::post("https://a/b")).is_err());
        assert_eq!(m.requests().len(), 2);
    }

    #[test]
    fn clients_build_for_every_proxy_mode() {
        use glossa_core::config::{Proxy, ProxyScheme};
        for mode in [ProxyMode::None, ProxyMode::System, ProxyMode::Manual, ProxyMode::Auto] {
            let settings = Network {
                proxy: Proxy {
                    mode,
                    scheme: ProxyScheme::Socks5,
                    host: "127.0.0.1".into(),
                    port: 9,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(HttpClient::new(&settings).is_ok(), "{mode:?}");
        }
    }
}
