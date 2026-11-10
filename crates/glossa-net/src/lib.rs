//! HTTP for every provider and module.
//!
//! All network traffic goes through one [`HttpClient`], built from the user's
//! [`glossa_core::config::Network`] settings, so the proxy, the
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

use std::sync::{Mutex, RwLock};
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
    /// Hand a 429 (or a 503 with `Retry-After`) straight back instead of
    /// retrying: for a provider with somewhere else to go, a retry only
    /// prolongs the block and delays the fall-back.
    pub no_retry_when_limited: bool,
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

    /// See [`HttpRequest::no_retry_when_limited`].
    pub fn no_retry_when_limited(mut self) -> Self {
        self.no_retry_when_limited = true;
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

    /// Like [`Http::send_streaming`], for a large file: no cap on the body
    /// size, and `request.timeout` bounds the whole transfer while waiting
    /// for the response headers keeps the client's usual limit. Module
    /// assets (models of hundreds of megabytes) come through here.
    ///
    /// A successful response is announced with [`DownloadEvent::Status`]
    /// before its body arrives as [`DownloadEvent::Chunk`]s, so a caller
    /// resuming with `Range` can tell a 206 (append) from a 200 (start
    /// over). The callback returns `false` to stop. Any other status comes
    /// back whole, without events.
    ///
    /// The default sends normally and delivers the body as one chunk, which
    /// suits test doubles.
    fn send_download(
        &self,
        request: HttpRequest,
        on_event: &mut dyn FnMut(DownloadEvent<'_>) -> bool,
    ) -> Result<HttpResponse, Error> {
        let mut response = self.send(request)?;
        if (200..300).contains(&response.status) {
            let body = std::mem::take(&mut response.body);
            if on_event(DownloadEvent::Status(response.status)) {
                on_event(DownloadEvent::Chunk(&body));
            }
        }
        Ok(response)
    }
}

/// What [`Http::send_download`] reports as a download goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent<'a> {
    /// The response's (successful) status, before any of its body.
    Status(u16),
    /// The next piece of the body.
    Chunk(&'a [u8]),
}

/// The real client.
pub struct HttpClient {
    agent: RwLock<ureq::Agent>,
    settings: Network,
    /// With the system proxy: the one the agent was built with, and when the
    /// system was last asked. `None` when the settings fix the proxy.
    system: Option<Mutex<(Option<ProxySpec>, Instant)>>,
    timeout: Duration,
    max_retries: u32,
    user_agent: String,
}

/// How often the system proxy is looked up again (a registry read on
/// Windows, environment variables elsewhere).
const PROXY_RECHECK: Duration = Duration::from_secs(5);

/// What we send when nobody asks for anything else: a current desktop
/// browser, because several free endpoints refuse obvious bots.
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

/// Bodies larger than this are refused, so a misbehaving endpoint cannot make
/// us allocate without bound.
const MAX_BODY: u64 = 32 * 1024 * 1024;

/// A request phase taking longer than this is logged (at debug level), so a
/// "Glossa hangs" report can be read from the log.
const SLOW: Duration = Duration::from_secs(2);

impl HttpClient {
    /// A client for `settings`. Fails when the proxy they name cannot be
    /// used (a scheme nothing here speaks, an address that is not one); the
    /// error names the proxy without its credentials.
    pub fn new(settings: &Network) -> Result<HttpClient, Error> {
        match HttpClient::new_or_direct(settings) {
            (client, None) => Ok(client),
            (_, Some(e)) => Err(e),
        }
    }

    /// [`HttpClient::new`] for starting the program, which must not fail
    /// over a proxy: when the proxy cannot be used, the client connects
    /// directly and the error is returned beside it for the user to be
    /// told. A system proxy is still followed, so the client takes it up
    /// once the system names one that works.
    pub fn new_or_direct(settings: &Network) -> (HttpClient, Option<Error>) {
        let spec = proxy_spec(settings);
        let (agent, problem) = match build_agent(settings, spec.clone()) {
            Ok(agent) => (agent, None),
            Err(e) => (build_agent(settings, None).expect("a proxy-less client always builds"), Some(e)),
        };
        let client = HttpClient {
            agent: RwLock::new(agent),
            settings: settings.clone(),
            system: follows_system(settings).then(|| Mutex::new((spec, Instant::now()))),
            timeout: Duration::from_secs(settings.timeout_secs.max(1)),
            max_retries: settings.max_retries,
            user_agent: settings.user_agent.clone(),
        };
        (client, problem)
    }

    /// The agent to use now. Following the system proxy, it is rebuilt when
    /// the system's proxy has changed since it was built: a proxy that was
    /// switched off would otherwise fail every request (or one switched on
    /// be ignored) until the program restarts.
    fn agent(&self) -> ureq::Agent {
        if let Some(system) = &self.system {
            let mut guard = system.lock().unwrap_or_else(|e| e.into_inner());
            let (seen, checked) = &mut *guard;
            if checked.elapsed() >= PROXY_RECHECK {
                *checked = Instant::now();
                let now = proxy_spec(&self.settings);
                if now != *seen {
                    match build_agent(&self.settings, now.clone()) {
                        Ok(agent) => {
                            tracing::info!(
                                from = %describe_proxy(seen.as_ref()),
                                to = %describe_proxy(now.as_ref()),
                                "the system proxy changed"
                            );
                            *self.agent.write().unwrap_or_else(|e| e.into_inner()) = agent;
                            *seen = now;
                        }
                        Err(e) => tracing::warn!("the system proxy changed to one that cannot be used: {e}"),
                    }
                }
            }
        }
        self.agent.read().unwrap_or_else(|e| e.into_inner()).clone()
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
        let started = Instant::now();
        let opened = self.open(request);
        let to_headers = started.elapsed();
        if to_headers >= SLOW {
            // Connecting, TLS, sending and waiting for the status line: the
            // part that stalls on a dead pooled connection, a broken route
            // or a slow proxy.
            tracing::debug!(
                url = %redact(&request.url),
                ms = to_headers.as_millis() as u64,
                ok = opened.is_ok(),
                "slow request: waited this long for the response headers"
            );
        }
        let mut response = opened?;
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
        let reading = started.elapsed() - to_headers;
        if reading >= SLOW {
            tracing::debug!(
                url = %redact(&request.url),
                ms = reading.as_millis() as u64,
                bytes = body.len(),
                "slow request: reading the body took this long"
            );
        }
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
            ureq::Error::Timeout(phase) => {
                let budget = request.timeout.unwrap_or(self.timeout);
                let ms = match phase {
                    ureq::Timeout::Resolve | ureq::Timeout::Connect => budget.min(CONNECT_BUDGET),
                    _ => budget,
                };
                tracing::debug!(url = %redact(&request.url), phase = %phase, "timed out");
                Error::Timeout(ms.as_millis() as u64)
            }
            other => Error::Network(format!("{provider}: {other}")),
        })
    }

    /// Run a request, with its own timeout if it asked for one.
    fn run<S: ureq::AsSendBody>(
        &self,
        req: ureq::http::Request<S>,
        timeout: Option<Duration>,
    ) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let agent = self.agent();
        match timeout {
            Some(t) => {
                let req = agent.configure_request(req).timeout_global(Some(t)).build();
                agent.run(req)
            }
            None => agent.run(req),
        }
    }
}

/// The most that looking up a host, or connecting to it (TCP, any proxy
/// negotiation and the TLS handshake), may take, within the overall timeout.
/// These take a fraction of a second when they work; a server that answers
/// slowly is given the rest of the budget, a host that cannot be reached is
/// given up on early so the next service gets its turn.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

fn build_agent(settings: &Network, proxy: Option<ProxySpec>) -> Result<ureq::Agent, Error> {
    let timeout = Duration::from_secs(settings.timeout_secs.max(1));
    let cfg = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .timeout_resolve(Some(timeout.min(CONNECT_BUDGET)))
        .timeout_connect(Some(timeout.min(CONNECT_BUDGET)))
        .max_redirects(8)
        .user_agent(if settings.user_agent.is_empty() {
            DEFAULT_USER_AGENT.to_owned()
        } else {
            settings.user_agent.clone()
        })
        .proxy(make_proxy(proxy)?)
        .tls_config(tls_config());
    Ok(cfg.build().into())
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

    fn send_download(
        &self,
        request: HttpRequest,
        on_event: &mut dyn FnMut(DownloadEvent<'_>) -> bool,
    ) -> Result<HttpResponse, Error> {
        use std::io::Read as _;
        let provider = if request.provider.is_empty() { "http" } else { request.provider.as_str() };
        let whole = request.timeout.unwrap_or(self.timeout);
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
        for (name, value) in &request.headers {
            if name.eq_ignore_ascii_case("accept-encoding") || name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            builder = builder.header(name.as_str(), value.as_str());
        }
        let req = builder.body(()).map_err(|e| Error::network(format!("bad request: {e}")))?;
        let agent = self.agent();
        // The whole transfer may take as long as the request says; the
        // server still has to start answering within the usual timeout, so
        // a dead host is given up on early.
        let req =
            agent.configure_request(req).timeout_global(Some(whole)).timeout_recv_response(Some(self.timeout)).build();
        let mut response = agent.run(req).map_err(|e| match e {
            ureq::Error::Timeout(_) => Error::Timeout(self.timeout.as_millis() as u64),
            other => Error::Network(format!("{provider}: {other}")),
        })?;
        let (status, headers) = status_and_headers(&response);
        if !(200..300).contains(&status) {
            let body = response.body_mut().with_config().limit(64 * 1024).read_to_vec().unwrap_or_default();
            return Ok(HttpResponse { status, headers, body, codepage: None });
        }
        if !on_event(DownloadEvent::Status(status)) {
            return Ok(HttpResponse { status, headers, body: Vec::new(), codepage: None });
        }
        let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if !on_event(DownloadEvent::Chunk(&buf[..n])) {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(Error::Timeout(whole.as_millis() as u64));
                }
                Err(e) => return Err(Error::Network(format!("{provider}: reading body: {e}"))),
            }
        }
        Ok(HttpResponse { status, headers, body: Vec::new(), codepage: None })
    }

    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            let result = self.send_once(&request);
            let retry = match &result {
                // A timeout has used the whole budget already; waiting for it
                // again doubles the time before the engine moves on to the
                // next service. A host the network black-holes (TLS to some
                // CDNs is dropped by DPI on some networks) cost 20 s that way.
                Err(Error::Timeout(_)) => false,
                Err(e) if e.is_transient() => true,
                Ok(r) if request.no_retry_when_limited && is_rate_limit(r) => false,
                Ok(r) => matches!(r.status, 429 | 500 | 502 | 503 | 504),
                Err(_) => false,
            };
            if !retry || attempt >= self.max_retries {
                if let Err(e) = &result {
                    tracing::debug!(
                        url = %redact(&request.url),
                        error = %e,
                        attempts = attempt + 1,
                        ms = started.elapsed().as_millis() as u64,
                        "request failed"
                    );
                } else if started.elapsed() >= SLOW {
                    tracing::debug!(
                        url = %redact(&request.url),
                        attempts = attempt + 1,
                        ms = started.elapsed().as_millis() as u64,
                        "slow request done"
                    );
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
            let why = match &result {
                Err(e) => e.to_string(),
                Ok(r) => format!("status {}", r.status),
            };
            tracing::debug!(url = %redact(&request.url), attempt, error = %why, "retrying in {backoff:?}");
            std::thread::sleep(backoff);
        }
    }
}

/// Whether [`HttpResponse::error_for_status`] would call this a rate limit.
fn is_rate_limit(r: &HttpResponse) -> bool {
    r.status == 429 || r.status == 503 && r.header("retry-after").is_some()
}

/// Which certificates a server must chain up to.
///
/// On Windows and macOS the operating system decides, as it does for the
/// browser and as WinHTTP did for the classic program: the system store
/// (with corporate and user-installed roots), revocation, and fetching an
/// intermediate certificate a misconfigured server forgot to send — a
/// bundled root list fails those servers with "UnknownIssuer". On Linux and
/// the BSDs the platform verifier checks against the system's CA store
/// (`/etc/ssl/certs`, `SSL_CERT_FILE`, `SSL_CERT_DIR`), so a corporate CA
/// installed there works too; a system without any CA certificates (a bare
/// container) falls back to the bundled Mozilla roots instead of failing
/// every connection.
fn tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder().root_certs(roots(system_store_has_roots())).build()
}

/// The roots to trust, given whether the system's CA store has any.
fn roots(system_store_has_roots: bool) -> ureq::tls::RootCerts {
    if system_store_has_roots {
        ureq::tls::RootCerts::PlatformVerifier
    } else {
        ureq::tls::RootCerts::WebPki
    }
}

/// Whether the system has CA certificates of its own. Always on Windows
/// and macOS; on other Unix systems the store is read once per process.
fn system_store_has_roots() -> bool {
    #[cfg(any(windows, target_vendor = "apple"))]
    {
        true
    }
    #[cfg(not(any(windows, target_vendor = "apple")))]
    {
        static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *PRESENT.get_or_init(|| {
            let found = rustls_native_certs::load_native_certs();
            if found.certs.is_empty() {
                let why: Vec<String> = found.errors.iter().map(|e| e.to_string()).collect();
                tracing::warn!(
                    "no CA certificates in the system store ({}); using the bundled Mozilla roots",
                    if why.is_empty() { "none installed".to_owned() } else { why.join("; ") }
                );
                false
            } else {
                tracing::debug!("{} CA certificates in the system store", found.certs.len());
                true
            }
        })
    }
}

/// Strip the query string and any `user:password@` from a URL before it
/// reaches a log or an error message, since keys, passwords and the user's
/// text travel there.
pub fn redact(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    // The credentials end at the last `@` before the path.
    let authority_len = rest.find('/').unwrap_or(rest.len());
    let rest = match rest[..authority_len].rfind('@') {
        Some(at) => &rest[at + 1..],
        None => rest,
    };
    match scheme {
        Some(scheme) => format!("{scheme}://{rest}"),
        None => rest.to_owned(),
    }
}

/// Whether the proxy `settings` call for can be used, without building a
/// client: for checking settings before they are saved. The error names
/// the proxy without its credentials.
pub fn check_proxy(settings: &Network) -> Result<(), Error> {
    make_proxy(proxy_spec(settings)).map(drop)
}

/// Whether the proxy is the user's own entry in the settings (rather than
/// the system's, which the settings cannot correct).
pub fn proxy_is_manual(settings: &Network) -> bool {
    settings.proxy.mode == ProxyMode::Manual
}

/// Hosts that never go through a proxy: a local server (a self-hosted DeepLX
/// or LibreTranslate, say) is unreachable from the proxy's side.
const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];

/// A proxy URL and the hosts that bypass it.
type ProxySpec = (String, Vec<String>);

/// Whether the proxy comes from the system, and so can change while Glossa
/// runs (a VPN client switching the system proxy on and off).
fn follows_system(settings: &Network) -> bool {
    matches!(settings.proxy.mode, ProxyMode::System | ProxyMode::Auto)
}

/// The proxy the settings call for right now.
fn proxy_spec(settings: &Network) -> Option<ProxySpec> {
    match settings.proxy.mode {
        ProxyMode::None => None,
        ProxyMode::Manual => settings.proxy.url().map(|u| (u, Vec::new())),
        ProxyMode::System | ProxyMode::Auto => system_proxy(),
    }
}

/// The proxy the settings call for right now, credentials included, for a
/// program Glossa starts that should go the same way (`uv`); `None` for a
/// direct connection.
pub fn proxy_url(settings: &Network) -> Option<String> {
    proxy_spec(settings).map(|(url, _)| url)
}

/// The proxy requests go through right now, without credentials (`none`,
/// `http://proxy.example:3128`), for diagnostics such as `glossa-cli
/// doctor`. With the system mode this is what the system says at this
/// moment.
pub fn proxy_in_use(settings: &Network) -> String {
    describe_proxy(proxy_spec(settings).as_ref())
}

/// Whether certificates are checked against the system's store (else the
/// bundled Mozilla roots), for diagnostics.
pub fn uses_system_certificates() -> bool {
    system_store_has_roots()
}

/// A proxy's address without any credentials in it, for the log and for
/// error messages.
fn describe_proxy(spec: Option<&ProxySpec>) -> String {
    match spec {
        None => "none".to_owned(),
        Some((url, _)) => match url.split_once("://") {
            Some((scheme, rest)) => format!("{scheme}://{}", rest.rsplit('@').next().unwrap_or(rest)),
            None => url.rsplit('@').next().unwrap_or(url).to_owned(),
        },
    }
}

/// `text` with the proxy's credentials taken out, however a library chose
/// to quote the address in its error.
fn without_credentials(text: &str, proxy_url: &str) -> String {
    let safe = describe_proxy(Some(&(proxy_url.to_owned(), Vec::new())));
    let mut out = text.replace(proxy_url, &safe);
    let rest = proxy_url.split_once("://").map_or(proxy_url, |(_, rest)| rest);
    if let Some((userinfo, _)) = rest.rsplit_once('@') {
        out = out.replace(userinfo, "***");
        if let Some((_, password)) = userinfo.split_once(':') {
            if !password.is_empty() {
                out = out.replace(password, "***");
            }
        }
    }
    out
}

#[cfg(test)]
fn resolve_proxy(settings: &Network) -> Result<Option<ureq::Proxy>, Error> {
    make_proxy(proxy_spec(settings))
}

fn make_proxy(spec: Option<ProxySpec>) -> Result<Option<ureq::Proxy>, Error> {
    let Some((u, bypass)) = spec else { return Ok(None) };
    // The address goes into the message without its credentials: this
    // error is printed, logged and shown in a dialog.
    let bad = |e: ureq::Error| {
        let shown = describe_proxy(Some(&(u.clone(), Vec::new())));
        Error::Config(format!("the proxy {shown} cannot be used: {}", without_credentials(&e.to_string(), &u)))
    };
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

    /// Method, URL prefix, the answer, and whether it is used only once.
    type Route = (Method, String, HttpResponse, bool);

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
            self.route(method, url_prefix, status, body, false)
        }

        /// The same, for the first matching request only: the routes after
        /// it answer the next ones (a refusal, then a success).
        pub fn on_once(self, method: Method, url_prefix: &str, status: u16, body: &str) -> Self {
            self.route(method, url_prefix, status, body, true)
        }

        /// Answer with a binary body (an archive, an image).
        pub fn on_bytes(self, method: Method, url_prefix: &str, status: u16, body: &[u8]) -> Self {
            self.route(method, url_prefix, status, body, false)
        }

        fn route(self, method: Method, url_prefix: &str, status: u16, body: impl AsRef<[u8]>, once: bool) -> Self {
            self.routes.lock().unwrap().push((
                method,
                url_prefix.to_owned(),
                HttpResponse {
                    status,
                    headers: vec![("content-type".into(), "application/json; charset=utf-8".into())],
                    body: body.as_ref().to_vec(),
                    codepage: None,
                },
                once,
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
            let mut routes = self.routes.lock().unwrap();
            let i = routes
                .iter()
                .position(|(m, prefix, _, _)| *m == request.method && request.url.starts_with(prefix.as_str()))
                .ok_or_else(|| {
                    Error::Network(format!("no mock route for {} {}", request.method.as_str(), request.url))
                })?;
            let response = HttpResponse { codepage: request.codepage, ..routes[i].2.clone() };
            if routes[i].3 {
                routes.remove(i);
            }
            Ok(response)
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
        // Credentials in the address itself go too, wherever an `@` is.
        assert_eq!(redact("https://user:pw@api.x/v1?key=secret"), "https://api.x/v1");
        assert_eq!(redact("http://user:p@ss@host.test:8080/a@b"), "http://host.test:8080/a@b");
        assert_eq!(redact("https://api.x/users/me@example.com"), "https://api.x/users/me@example.com");
        assert_eq!(redact("user:pw@host.test/path"), "host.test/path");
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

    /// The audit's case: `HTTPS_PROXY="ftp://x:secretpw@proxy:1"` printed
    /// the password and stopped the program from starting.
    #[test]
    fn an_unusable_proxy_is_an_error_without_its_password_and_never_stops_a_start() {
        let spec = Some(("ftp://x:secretpw@proxy.test:1".to_owned(), Vec::new()));
        let message = make_proxy(spec.clone()).unwrap_err().to_string();
        assert!(message.contains("ftp://proxy.test:1"), "{message}");
        assert!(!message.contains("secretpw") && !message.contains("x:"), "{message}");
        let message = build_agent(&Network::default(), spec).unwrap_err().to_string();
        assert!(!message.contains("secretpw"), "{message}");
        // The same through the settings: a manual proxy whose host is not one.
        use glossa_core::config::{Proxy, ProxyScheme};
        let settings = Network {
            proxy: Proxy {
                mode: ProxyMode::Manual,
                scheme: ProxyScheme::Http,
                host: "not a host".into(),
                port: 8080,
                username: "x".into(),
                password: "secretpw".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let strict = HttpClient::new(&settings).map(drop).unwrap_err().to_string();
        assert!(!strict.contains("secretpw"), "{strict}");
        assert!(check_proxy(&settings).is_err() && proxy_is_manual(&settings));
        // At start: a client that connects directly, and the problem to show.
        let (client, problem) = HttpClient::new_or_direct(&settings);
        let problem = problem.expect("reported").to_string();
        assert!(!problem.contains("secretpw"), "{problem}");
        assert!(client.system.is_none());
        // A proxy that works is no problem.
        let fine = Network {
            proxy: Proxy { host: "127.0.0.1".into(), ..settings.proxy.clone() },
            ..Default::default()
        };
        assert!(check_proxy(&fine).is_ok());
        assert!(HttpClient::new_or_direct(&fine).1.is_none());
    }

    #[test]
    fn whatever_a_library_quotes_the_credentials_are_taken_out() {
        let url = "socks5://user:p%40ss@proxy.test:1080";
        for text in [
            "bad uri socks5://user:p%40ss@proxy.test:1080",
            "cannot parse \"user:p%40ss@proxy.test\"",
            "password p%40ss rejected",
        ] {
            let out = without_credentials(text, url);
            assert!(!out.contains("p%40ss"), "{out}");
        }
        assert_eq!(without_credentials("no such host proxy.test", url), "no such host proxy.test");
    }

    #[test]
    fn proxies_are_logged_without_credentials() {
        let spec = |u: &str| Some((u.to_owned(), Vec::new()));
        assert_eq!(describe_proxy(None), "none");
        assert_eq!(describe_proxy(spec("http://user:secret@proxy.test:3128").as_ref()), "http://proxy.test:3128");
        assert_eq!(describe_proxy(spec("127.0.0.1:2080").as_ref()), "127.0.0.1:2080");
    }

    #[test]
    fn tls_trusts_the_system_store_with_the_bundled_roots_as_fallback() {
        assert!(matches!(roots(true), ureq::tls::RootCerts::PlatformVerifier));
        assert!(matches!(roots(false), ureq::tls::RootCerts::WebPki));
        if cfg!(any(windows, target_os = "macos")) {
            assert!(system_store_has_roots());
        }
        // Whatever the store holds, an agent builds.
        assert!(build_agent(&Network::default(), None).is_ok());
    }

    #[test]
    fn a_changed_system_proxy_rebuilds_the_agent() {
        let settings = Network {
            proxy: glossa_core::config::Proxy { mode: ProxyMode::System, ..Default::default() },
            ..Default::default()
        };
        let http = HttpClient::new(&settings).unwrap();
        let system = http.system.as_ref().expect("the system proxy is followed");
        // Pretend the agent was built for another proxy, a while ago.
        *system.lock().unwrap() =
            (Some(("http://gone.test:1".to_owned(), Vec::new())), Instant::now() - PROXY_RECHECK * 2);
        let _ = http.agent();
        let (seen, checked) = &*system.lock().unwrap();
        assert_eq!(*seen, proxy_spec(&settings), "rebuilt for the proxy the system names now");
        assert!(checked.elapsed() < PROXY_RECHECK);

        let fixed = Network {
            proxy: glossa_core::config::Proxy { mode: ProxyMode::None, ..Default::default() },
            ..Default::default()
        };
        assert!(HttpClient::new(&fixed).unwrap().system.is_none(), "a fixed proxy is never looked up again");
    }

    /// A local server that accepts connections and does whatever `serve`
    /// says with each, on a thread of its own. Returns its address.
    fn stalling_server(serve: fn(std::net::TcpStream)) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || serve(stream));
            }
        });
        format!("http://{addr}/")
    }

    fn client(timeout_secs: u64, max_retries: u32) -> HttpClient {
        let settings = Network {
            timeout_secs,
            max_retries,
            proxy: glossa_core::config::Proxy { mode: ProxyMode::None, ..Default::default() },
            ..Default::default()
        };
        HttpClient::new(&settings).unwrap()
    }

    #[test]
    fn a_rate_limit_is_retried_unless_the_request_says_not_to() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static HITS: AtomicUsize = AtomicUsize::new(0);
        let url = stalling_server(|mut s| {
            use std::io::{Read as _, Write as _};
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            HITS.fetch_add(1, Ordering::SeqCst);
            let _ = s.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        let r = client(5, 1).send(HttpRequest::get(&url).no_retry_when_limited()).unwrap();
        assert_eq!((r.status, HITS.load(Ordering::SeqCst)), (429, 1), "asked once");
        let r = client(5, 1).send(HttpRequest::get(&url)).unwrap();
        assert_eq!((r.status, HITS.load(Ordering::SeqCst)), (429, 3), "asked again by default");
    }

    #[test]
    fn a_server_that_never_answers_costs_the_timeout_not_more() {
        // Accepts, reads the request, then says nothing: what a dead pooled
        // connection or a black-holed proxy looks like from here.
        let url = stalling_server(|mut s| {
            use std::io::Read as _;
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            std::thread::sleep(Duration::from_secs(30));
        });
        let started = Instant::now();
        let err = client(1, 0).send(HttpRequest::get(&url)).unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_millis(2500), "took {:?}", started.elapsed());

        // Retries are allowed, but a timeout is not repeated: the engine
        // moves on to the next service instead.
        let started = Instant::now();
        assert!(client(1, 2).send(HttpRequest::get(&url)).is_err());
        assert!(started.elapsed() < Duration::from_millis(2500), "took {:?}", started.elapsed());
    }

    #[test]
    fn a_handshake_that_never_finishes_is_given_up_within_the_connect_budget() {
        // Accepts TCP but never answers the TLS ClientHello: what a
        // black-holed CDN looks like through DPI.
        let url = stalling_server(|mut s| {
            use std::io::Read as _;
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            std::thread::sleep(Duration::from_secs(30));
        });
        let https = url.replacen("http://", "https://", 1);
        let started = Instant::now();
        let err = client(30, 2).send(HttpRequest::get(&https)).unwrap_err();
        let took = started.elapsed();
        assert!(matches!(err, Error::Timeout(ms) if ms == CONNECT_BUDGET.as_millis() as u64), "{err:?}");
        assert!(took >= CONNECT_BUDGET && took < CONNECT_BUDGET + Duration::from_secs(2), "took {took:?}");
    }

    #[test]
    fn a_body_that_stops_halfway_times_out() {
        let url = stalling_server(|mut s| {
            use std::io::{Read as _, Write as _};
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhalf");
            let _ = s.flush();
            std::thread::sleep(Duration::from_secs(30));
        });
        let started = Instant::now();
        let err = client(1, 0).send(HttpRequest::get(&url)).unwrap_err();
        assert!(matches!(err, Error::Timeout(_) | Error::Network(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_millis(2500), "took {:?}", started.elapsed());
    }

    #[test]
    fn a_kept_alive_connection_the_server_closed_is_replaced() {
        // The server answers one request per connection and closes it while
        // the client keeps it in its pool; the next request must not stall
        // on the dead connection.
        let url = stalling_server(|mut s| {
            use std::io::{Read as _, Write as _};
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok");
            let _ = s.flush();
            std::thread::sleep(Duration::from_millis(50));
        });
        let http = client(5, 0);
        assert_eq!(http.send(HttpRequest::get(&url)).unwrap().body, b"ok");
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        assert_eq!(http.send(HttpRequest::get(&url)).unwrap().body, b"ok");
        assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    }
}
