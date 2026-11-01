//! Google Lens: Glossa's own client (`glossa-lens`, the `native` mode), or
//! the user's chrome-lens-py.
//!
//! The built-in client needs nothing installed. chrome-lens-py, when the user
//! has it, can be reached three ways, fastest first:
//!
//! 1. **A running daemon.** It is a small HTTP server on loopback (TCP on
//!    Windows, a Unix socket on macOS and Linux) that has already paid for
//!    its imports and holds a warm HTTP/2 connection to Google: a request
//!    costs a few hundred milliseconds. A running daemon announces itself in
//!    `daemon.json` in chrome-lens-py's state directory
//!    (`%LOCALAPPDATA%\chrome-lens-py\` on Windows,
//!    `~/Library/Caches/chrome-lens-py/` on macOS, `$XDG_RUNTIME_DIR/chrome-lens-py/`
//!    or `~/.cache/chrome-lens-py/` on Linux, `$LENS_STATE_DIR` if set):
//!    `{version, pid, transport: "tcp"|"unix", address, token, started_at, managed}`,
//!    where `address` is `127.0.0.1:<port>` or the socket path.
//! 2. **The login service**, installed by `lens_scan service install`: the
//!    same daemon on a fixed port (8765 by default) whose port and token are
//!    kept in `service.json` (`{port, token}`) in chrome-lens-py's data
//!    directory (the state directory, except on Linux where it is
//!    `~/.cache/chrome-lens-py/`). It normally writes `daemon.json` too; the
//!    service file is the fallback for when that is missing.
//! 3. **The CLI**, `lens_scan <image> [lang] -t <to> --format json`, which
//!    costs a Python start-up per image (seconds) but needs nothing running.
//!
//! [`LensConfig::mode`] picks among them. In `auto` an explicit endpoint
//! from the config comes first, then the discovery file, then the service
//! file; if no daemon answers its `/health`, the built-in client does the
//! work (a daemon that comes up later is noticed and preferred: its
//! connection to Google is already warm). `native` uses only the built-in
//! client, `daemon` only the first three, `cli` only the last. The endpoint found is cached; when it stops answering it is dropped
//! and looked up again, once, within the same request.
//!
//! # Proxies
//!
//! The daemon is on loopback, so a loopback endpoint is reached with a
//! client of our own that never uses a proxy: the user's proxy settings (and
//! a system proxy picked up from the environment) are meant for the internet,
//! and a proxy would either refuse `127.0.0.1` or try to reach its *own*
//! loopback. chrome-lens-py applies its own proxy settings on the way to
//! Google. A non-loopback endpoint set by hand goes through the host's
//! client like any other request.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use glossa_core::config::{LensConfig, LensMode};
use glossa_core::ocr::{OcrBlock, OcrGranularity, OcrLevel, Rect};
use glossa_core::{Capabilities, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, Provider};
use glossa_net::{Http, HttpRequest, HttpResponse, Method};
use serde::Deserialize;
use serde_json::Value;

use crate::process;
use crate::tempimage::TempImage;

pub const LENS_ID: &str = "lens";
const NAME: &str = "Google Lens";
const APP_DIR: &str = "chrome-lens-py";
/// The service daemon's port when `service.json` does not say.
pub const DEFAULT_SERVICE_PORT: u16 = 8765;

/// A Lens round trip is normally under a second; the margin is for a large
/// image on a slow link, and chrome-lens-py's own retries.
const OCR_TIMEOUT: Duration = Duration::from_secs(90);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const CLI_TIMEOUT: Duration = Duration::from_secs(120);

/// Where chrome-lens-py keeps its files, and where its CLI is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locator {
    /// `daemon.json`.
    pub daemon_file: Option<PathBuf>,
    /// `service.json`.
    pub service_file: Option<PathBuf>,
    /// The `lens_scan` executable.
    pub cli: Option<PathBuf>,
}

impl Locator {
    /// The real locations on this machine.
    pub fn system(cli_path: &str) -> Locator {
        let (state, data) = lens_dirs(&|k| std::env::var_os(k), dirs::home_dir(), Os::current());
        Locator {
            daemon_file: state.map(|d| d.join("daemon.json")),
            service_file: data.map(|d| d.join("service.json")),
            cli: find_cli(cli_path, std::env::var_os("PATH"), &extra_cli_dirs()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// chrome-lens-py's state directory (`daemon.json`) and data directory
/// (`service.json`), by the rules of its `runtime.py`.
pub fn lens_dirs(
    env: &dyn Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
    os: Os,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = var("LENS_STATE_DIR") {
        return (Some(dir.clone()), Some(dir));
    }
    match os {
        Os::Windows => {
            let base = var("LOCALAPPDATA").or_else(|| home.map(|h| h.join("AppData").join("Local")));
            let dir = base.map(|b| b.join(APP_DIR));
            (dir.clone(), dir)
        }
        Os::Mac => {
            let dir = home.map(|h| h.join("Library").join("Caches").join(APP_DIR));
            (dir.clone(), dir)
        }
        Os::Linux => {
            let cache = home.map(|h| h.join(".cache").join(APP_DIR));
            let state =
                var("XDG_RUNTIME_DIR").filter(|p| p.is_dir()).map(|r| r.join(APP_DIR)).or_else(|| cache.clone());
            (state, cache)
        }
    }
}

/// The `lens_scan` executable: `explicit` if given, else the first match on
/// `PATH`, else in `extra` (where installers put it without always fixing
/// `PATH` for GUI programs).
pub fn find_cli(explicit: &str, path_var: Option<OsString>, extra: &[PathBuf]) -> Option<PathBuf> {
    let explicit = explicit.trim().trim_matches('"');
    let names: &[&str] = if cfg!(windows) { &["lens_scan.exe"] } else { &["lens_scan"] };
    let mut dirs: Vec<PathBuf> = path_var.map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    dirs.extend(extra.iter().cloned());
    if !explicit.is_empty() {
        let p = PathBuf::from(explicit);
        // A path is taken as given, so a typo surfaces as "cannot find" with
        // the user's own path in it; a bare name is looked up like the default.
        if p.components().count() > 1 {
            return Some(p);
        }
        return dirs.iter().map(|d| d.join(&p)).find(|c| c.is_file());
    }
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|c| c.is_file())
}

fn extra_cli_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = dirs::home_dir();
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
            out.push(PathBuf::from(appdata).join("uv").join("tools").join(APP_DIR).join("Scripts"));
        }
        if let Some(h) = &home {
            out.push(h.join(".local").join("bin")); // uv tool's default bin dir
            out.push(h.join("scoop").join("shims"));
        }
    } else {
        if let Some(h) = &home {
            out.push(h.join(".local").join("bin"));
        }
        out.push(PathBuf::from("/opt/homebrew/bin"));
        out.push(PathBuf::from("/usr/local/bin"));
    }
    out
}

/// How a request reaches chrome-lens-py.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Endpoint {
    /// A daemon over HTTP; `base` has no trailing slash.
    Http { base: String, token: String, from: &'static str },
    /// A daemon on a Unix socket (macOS and Linux).
    Unix { path: PathBuf, token: String },
    /// The CLI, per image.
    Cli(PathBuf),
    /// Glossa's own client (`glossa-lens`), straight to Google.
    Native,
}

impl Endpoint {
    fn describe(&self) -> String {
        match self {
            Endpoint::Http { base, from, .. } => format!("{base} ({from})"),
            Endpoint::Unix { path, .. } => format!("unix:{} (daemon.json)", path.display()),
            Endpoint::Cli(p) => format!("CLI {}", p.display()),
            Endpoint::Native => "the built-in client".into(),
        }
    }
}

#[derive(Deserialize)]
struct DaemonFile {
    transport: String,
    address: String,
    token: String,
}

/// `daemon.json`, as chrome-lens-py's `runtime.read_daemon` accepts it.
fn parse_daemon_file(text: &str) -> Option<Endpoint> {
    let d: DaemonFile = serde_json::from_str(text).ok()?;
    match d.transport.as_str() {
        "tcp" if !d.address.is_empty() => {
            Some(Endpoint::Http { base: format!("http://{}", d.address), token: d.token, from: "daemon.json" })
        }
        "unix" if !d.address.is_empty() => Some(Endpoint::Unix { path: PathBuf::from(d.address), token: d.token }),
        _ => None,
    }
}

#[derive(Deserialize)]
struct ServiceFile {
    #[serde(default = "default_port")]
    port: u16,
    token: String,
}

fn default_port() -> u16 {
    DEFAULT_SERVICE_PORT
}

/// `service.json`: the login service's fixed port and lasting token.
fn parse_service_file(text: &str) -> Option<Endpoint> {
    let s: ServiceFile = serde_json::from_str(text).ok()?;
    Some(Endpoint::Http { base: format!("http://127.0.0.1:{}", s.port), token: s.token, from: "service.json" })
}

/// Whether a URL points at this machine.
pub fn is_loopback_url(url: &str) -> bool {
    match glossa_modules::manifest::url_host(url) {
        Some(h) => h == "localhost" || h == "::1" || h.starts_with("127.") || h.ends_with(".localhost"),
        None => false,
    }
}

/// The text recogniser backed by chrome-lens-py.
pub struct LensProvider {
    info: ProviderInfo,
    cfg: LensConfig,
    locator: OnceLock<Locator>,
    loopback_http: OnceLock<Arc<dyn Http>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    endpoint: Option<Endpoint>,
}

/// Why one attempt failed: worth looking the endpoint up again, or not.
enum Failure {
    Unreachable(Error),
    Fatal(Error),
}

impl LensProvider {
    pub fn new(cfg: LensConfig) -> Self {
        let mut info = ProviderInfo::new(LENS_ID, NAME, Capabilities::OCR);
        info.description = "Google Lens text recognition and translation: built in, or through a running \
                            chrome-lens-py daemon, which keeps a warm connection to Google."
            .into();
        info.homepage = "https://github.com/bropines/chrome-lens-py".into();
        Self {
            info,
            cfg,
            locator: OnceLock::new(),
            loopback_http: OnceLock::new(),
            state: Mutex::new(State::default()),
        }
    }

    /// Use these locations instead of the real ones (tests, a portable setup).
    pub fn with_locator(cfg: LensConfig, locator: Locator) -> Self {
        let p = Self::new(cfg);
        let _ = p.locator.set(locator);
        p
    }

    /// Use this client for loopback daemons instead of a proxy-less
    /// [`glossa_net::HttpClient`].
    pub fn with_loopback_http(self, http: Arc<dyn Http>) -> Self {
        let _ = self.loopback_http.set(http);
        self
    }

    fn locator(&self) -> &Locator {
        self.locator.get_or_init(|| Locator::system(&self.cfg.cli_path))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn http_for<'a>(&'a self, url: &str, host: &'a Host) -> &'a dyn Http {
        if is_loopback_url(url) {
            self.loopback_http.get_or_init(|| Arc::new(glossa_net::HttpClient::plain())).as_ref()
        } else {
            host.http
        }
    }

    fn send(
        &self,
        ep: &Endpoint,
        host: &Host,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<HttpResponse> {
        match ep {
            Endpoint::Http { base, token, .. } => {
                let mut req = HttpRequest { method, url: format!("{base}{path}"), ..Default::default() }
                    .provider(LENS_ID)
                    .timeout(timeout);
                if !token.is_empty() {
                    req = req.header("Authorization", format!("Bearer {token}"));
                }
                if let Some(b) = body {
                    req = req.json(b);
                }
                self.http_for(base, host).send(req)
            }
            Endpoint::Unix { path: socket, token } => {
                let body = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
                unix_request(socket, method, path, token, body, timeout)
            }
            Endpoint::Cli(_) | Endpoint::Native => Err(Error::Config("not a daemon: no HTTP interface".into())),
        }
    }

    fn healthy(&self, ep: &Endpoint, host: &Host) -> bool {
        match self.send(ep, host, Method::Get, "/health", None, HEALTH_TIMEOUT) {
            Ok(r) if r.status == 200 => true,
            Ok(r) => {
                tracing::debug!(endpoint = %ep.describe(), status = r.status, "chrome-lens health check failed");
                false
            }
            Err(e) => {
                tracing::debug!(endpoint = %ep.describe(), error = %e, "chrome-lens daemon not answering");
                false
            }
        }
    }

    /// Daemons we could talk to, in the order we try them.
    fn daemon_candidates(&self) -> Vec<Endpoint> {
        let mut out = Vec::new();
        let endpoint = self.cfg.endpoint.trim().trim_end_matches('/');
        if !endpoint.is_empty() {
            let base = if endpoint.contains("://") { endpoint.to_owned() } else { format!("http://{endpoint}") };
            out.push(Endpoint::Http { base, token: self.cfg.token.trim().to_owned(), from: "settings" });
        }
        let loc = self.locator();
        let read = |p: &Option<PathBuf>| p.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
        if let Some(ep) = read(&loc.daemon_file).as_deref().and_then(parse_daemon_file) {
            out.push(ep);
        }
        if let Some(ep) = read(&loc.service_file).as_deref().and_then(parse_service_file) {
            let duplicate = out.iter().any(
                |e| matches!((e, &ep), (Endpoint::Http { base: a, .. }, Endpoint::Http { base: b, .. }) if a == b),
            );
            if !duplicate {
                out.push(ep);
            }
        }
        out
    }

    fn first_healthy_daemon(&self, host: &Host) -> Option<Endpoint> {
        self.daemon_candidates().into_iter().find(|ep| self.healthy(ep, host))
    }

    /// The endpoint to use now: the cached one, or a fresh lookup.
    fn endpoint(&self, host: &Host) -> Result<Endpoint> {
        let mut st = self.state();
        match &st.endpoint {
            // A daemon may have come up since: it answers faster than a
            // fresh connection of our own, so look again.
            Some(Endpoint::Native) if self.cfg.mode == LensMode::Auto => {
                if let Some(ep) = self.first_healthy_daemon(host) {
                    tracing::info!(endpoint = %ep.describe(), "chrome-lens daemon found");
                    st.endpoint = Some(ep.clone());
                    return Ok(ep);
                }
            }
            Some(_) => {}
            None => {
                let ep = self.resolve(host, &mut st)?;
                tracing::info!(endpoint = %ep.describe(), "using chrome-lens");
                st.endpoint = Some(ep);
            }
        }
        st.endpoint.clone().ok_or_else(|| Error::Config("no chrome-lens endpoint".into()))
    }

    fn invalidate(&self, ep: &Endpoint) {
        let mut st = self.state();
        if st.endpoint.as_ref() == Some(ep) {
            st.endpoint = None;
        }
    }

    fn not_installed(&self) -> Error {
        if self.cfg.cli_path.trim().is_empty() {
            Error::Config(
                "chrome-lens-py was not found: install it (`uv tool install chrome-lens-py`) or set the path to lens_scan in the OCR settings".into(),
            )
        } else {
            Error::FileNotFound(self.cfg.cli_path.clone())
        }
    }

    fn resolve(&self, host: &Host, _st: &mut State) -> Result<Endpoint> {
        match self.cfg.mode {
            LensMode::Cli => {
                return self.locator().cli.clone().map(Endpoint::Cli).ok_or_else(|| self.not_installed());
            }
            LensMode::Native => return Ok(Endpoint::Native),
            LensMode::Auto | LensMode::Daemon => {}
        }
        let candidates = self.daemon_candidates();
        if let Some(ep) = candidates.iter().find(|ep| self.healthy(ep, host)) {
            return Ok(ep.clone());
        }
        if self.cfg.mode == LensMode::Daemon {
            let tried = if candidates.is_empty() {
                "no daemon.json or service.json found; start one with `lens_scan daemon start`".to_owned()
            } else {
                format!("tried {}", candidates.iter().map(Endpoint::describe).collect::<Vec<_>>().join(", "))
            };
            return Err(Error::Network(format!("{NAME}: no chrome-lens-py daemon is answering ({tried})")));
        }
        // No daemon: the built-in client, with no Python to start.
        Ok(Endpoint::Native)
    }

    fn via_daemon(
        &self,
        ep: &Endpoint,
        bytes: &[u8],
        req: &OcrRequest,
        host: &Host,
    ) -> std::result::Result<OcrResult, Failure> {
        let body = request_body(req, bytes);
        let resp = match self.send(ep, host, Method::Post, "/v1/ocr", Some(&body), OCR_TIMEOUT) {
            Ok(r) => r,
            Err(e @ Error::Network(_)) => return Err(Failure::Unreachable(e)),
            Err(e) => return Err(Failure::Fatal(e)),
        };
        if resp.status == 401 {
            // A daemon restarted with a new token, or a stale file: look again.
            return Err(Failure::Unreachable(Error::HttpStatus {
                provider: NAME.into(),
                status: 401,
                body: "the daemon rejected the token".into(),
            }));
        }
        let resp = resp.error_for_status(NAME).map_err(|e| Failure::Fatal(readable_error(e)))?;
        let value: Value = resp
            .json()
            .map_err(|e| Failure::Fatal(Error::BadResponse { provider: NAME.into(), detail: e.to_string() }))?;
        map_daemon_result(&value, req).map_err(Failure::Fatal)
    }

    fn via_cli(&self, cli: &Path, bytes: &[u8], req: &OcrRequest, host: &Host) -> Result<OcrResult> {
        let image = TempImage::write(bytes)?;
        let mut cmd = Command::new(cli);
        cmd.args(cli_args(image.path(), req));
        // Python writes pipes in the locale's code page on Windows otherwise.
        cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1").env("NO_COLOR", "1");
        let out = process::run(cmd, CLI_TIMEOUT, Some(host.cancel))?;
        match serde_json::from_slice::<Value>(&out.stdout) {
            Ok(v) => map_cli_page(&v, req),
            Err(_) => Err(Error::Module {
                module: LENS_ID.into(),
                detail: format!(
                    "lens_scan exited with {} and no JSON result: {}",
                    out.code.map_or("a signal".to_owned(), |c| format!("code {c}")),
                    out.stderr_tail()
                ),
            }),
        }
    }
}

impl Provider for LensProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        let started = Instant::now();
        let bytes = req.image.to_bytes()?;
        let mut last = None;
        for attempt in 0..2 {
            host.check_cancelled()?;
            let ep = self.endpoint(host)?;
            let outcome = match &ep {
                Endpoint::Cli(cli) => self.via_cli(cli, &bytes, req, host).map_err(Failure::Fatal),
                Endpoint::Native => native::recognize(&bytes, req, host).map_err(Failure::Fatal),
                _ => self.via_daemon(&ep, &bytes, req, host),
            };
            match outcome {
                Ok(mut r) => {
                    r.provider_id = LENS_ID.into();
                    r.elapsed_ms = started.elapsed().as_millis() as u64;
                    return Ok(r);
                }
                Err(Failure::Fatal(e)) => return Err(e),
                Err(Failure::Unreachable(e)) => {
                    tracing::debug!(endpoint = %ep.describe(), attempt, error = %e, "chrome-lens endpoint lost; looking again");
                    self.invalidate(&ep);
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::Network(format!("{NAME}: unreachable"))))
    }

    fn suspend(&self) {
        // Nothing of ours is running; forget the endpoint so a daemon that
        // went away while we idled is not tried first.
        self.state().endpoint = None;
    }
}

// -- the built-in client --------------------------------------------------------

mod native {
    //! Lens without chrome-lens-py: `glossa-lens` builds the request Chromium
    //! sends and reads the reply; the host's client (with the user's proxy
    //! settings) carries it.

    use glossa_core::ocr::{OcrBlock, OcrGranularity, OcrLevel, Rect};
    use glossa_core::{Error, OcrRequest, OcrResult, Result};
    use glossa_lens::{Bounds, Page};
    use glossa_modules::Host;
    use glossa_net::HttpRequest;

    use super::{translate_code, NAME, OCR_TIMEOUT};

    pub fn recognize(bytes: &[u8], req: &OcrRequest, host: &Host) -> Result<OcrResult> {
        let image = glossa_lens::decode_image(bytes).map_err(|e| Error::Internal(format!("{NAME}: {e}")))?;
        let size = (image.width(), image.height());
        let upload = glossa_lens::encode_image(&image).map_err(|e| Error::Internal(format!("{NAME}: {e}")))?;
        let query = glossa_lens::Query {
            ocr_language: if req.lang.is_language() { req.lang.bcp47().to_owned() } else { String::new() },
            translate_to: req.translate_to.filter(|l| l.is_language()).map(|l| translate_code(l).to_owned()),
            ..glossa_lens::Query::default()
        };
        let body = glossa_lens::objects_request(&upload, &query, glossa_lens::random_uuid());
        let mut request =
            HttpRequest::post(glossa_lens::ENDPOINT).body(body).timeout(OCR_TIMEOUT).provider(NAME.to_owned());
        for (name, value) in glossa_lens::headers(glossa_lens::PUBLIC_API_KEY) {
            request = request.header(name, value);
        }
        host.check_cancelled()?;
        let response = host.http.send(request)?;
        if !(200..300).contains(&response.status) {
            let body = String::from_utf8_lossy(&response.body).chars().take(300).collect();
            return Err(Error::HttpStatus { provider: NAME.into(), status: response.status, body });
        }
        let page = glossa_lens::parse_response(&response.body)
            .map_err(|detail| Error::BadResponse { provider: NAME.into(), detail })?;
        Ok(result(&page, size, req))
    }

    /// The page as our result, its boxes in pixels of the picture as given
    /// (Lens measures the smaller copy it was sent, normalised).
    pub(super) fn result(page: &Page, size: (u32, u32), req: &OcrRequest) -> OcrResult {
        let rect = |b: &Option<Bounds>| -> (Option<Rect>, f32) {
            let Some(b) = b else { return (None, 0.0) };
            let (sx, sy) = if b.normalized { (size.0 as f32, size.1 as f32) } else { (1.0, 1.0) };
            let (w, h) = (b.width * sx, b.height * sy);
            (Some(Rect::new(b.center_x * sx - w / 2.0, b.center_y * sy - h / 2.0, w, h)), b.rotation)
        };
        let node = |level: OcrLevel, text: String, b: &Option<Bounds>, children: Vec<OcrBlock>| {
            let (bounds, angle) = rect(b);
            OcrBlock { bounds, angle, children, ..OcrBlock::new(level, &text) }
        };
        let words = req.granularity == OcrGranularity::Words;
        let mut blocks: Vec<OcrBlock> = match req.granularity {
            OcrGranularity::FullText => Vec::new(),
            OcrGranularity::Lines => page
                .paragraphs
                .iter()
                .flat_map(|p| p.lines.iter().map(|l| node(OcrLevel::Line, l.text(), &l.bounds, Vec::new())))
                .collect(),
            OcrGranularity::Blocks | OcrGranularity::Words => page
                .paragraphs
                .iter()
                .map(|p| {
                    let lines = p
                        .lines
                        .iter()
                        .map(|l| {
                            let kids = if words {
                                l.words
                                    .iter()
                                    .map(|w| node(OcrLevel::Word, w.text.clone(), &w.bounds, Vec::new()))
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            node(OcrLevel::Line, l.text(), &l.bounds, kids)
                        })
                        .collect();
                    node(OcrLevel::Block, p.text(), &p.bounds, lines)
                })
                .collect(),
        };
        // Each paragraph's translation on its block, and each translated line
        // on its line when they pair up.
        for t in &page.translations {
            let Some(block) = blocks.get_mut(t.paragraph).filter(|b| b.level == Some(OcrLevel::Block)) else {
                continue;
            };
            if t.lines.len() == block.children.len() {
                for (i, line) in block.children.iter_mut().enumerate() {
                    line.translation = t.line(i).filter(|s| !s.is_empty());
                }
            }
            block.translation = Some(t.text.trim().to_owned());
        }
        let text = page.text();
        OcrResult {
            text: if req.preserve_line_breaks { text } else { text.split_whitespace().collect::<Vec<_>>().join(" ") },
            blocks,
            detected_lang: page
                .language
                .as_deref()
                .and_then(glossa_core::LangId::from_code)
                .filter(|l| l.is_language()),
            translation: page.translation(),
            image_size: Some(size),
            ..Default::default()
        }
    }
}

// -- requests and replies ---------------------------------------------------------

/// What `output_format` to ask chrome-lens-py for.
fn output_format(g: OcrGranularity) -> &'static str {
    match g {
        OcrGranularity::FullText => "full_text",
        OcrGranularity::Blocks => "blocks",
        OcrGranularity::Lines => "lines",
        OcrGranularity::Words => "detailed",
    }
}

/// The language code for Lens's translation target. Lens takes Google
/// Translate's codes, which spell the two Chinese scripts as regions.
fn translate_code(lang: LangId) -> &'static str {
    match lang.bcp47() {
        "zh-Hans" => "zh-CN",
        "zh-Hant" => "zh-TW",
        other => other,
    }
}

/// The JSON body of `POST /v1/ocr`.
fn request_body(req: &OcrRequest, bytes: &[u8]) -> Value {
    let mut body = serde_json::json!({
        "image_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "ocr_preserve_line_breaks": req.preserve_line_breaks,
        "output_format": output_format(req.granularity),
    });
    if req.lang.is_language() {
        body["ocr_language"] = req.lang.bcp47().into();
    }
    if let Some(to) = req.translate_to.filter(|l| l.is_language()) {
        body["translate_to"] = translate_code(to).into();
    }
    body
}

/// `lens_scan` arguments for one image.
fn cli_args(image: &Path, req: &OcrRequest) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![image.into()];
    if req.lang.is_language() {
        args.push(req.lang.bcp47().into());
    }
    if let Some(to) = req.translate_to.filter(|l| l.is_language()) {
        args.push("-t".into());
        args.push(translate_code(to).into());
    }
    if !req.preserve_line_breaks {
        args.push("--ocr-single-line".into());
    }
    for a in ["--format", "json", "-q"] {
        args.push(a.into());
    }
    args
}

/// The daemon puts `{"error": "…"}` in error bodies; show that, not the JSON.
fn readable_error(e: Error) -> Error {
    match e {
        Error::HttpStatus { provider, status, body } => {
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or(body);
            Error::HttpStatus { provider, status, body: msg }
        }
        other => other,
    }
}

fn non_blank(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn detected(v: Option<&Value>) -> Option<LangId> {
    non_blank(v).and_then(|c| LangId::from_code(&c)).filter(|l| l.is_language())
}

fn size_from(w: Option<&Value>, h: Option<&Value>) -> Option<(u32, u32)> {
    let w = w.and_then(Value::as_u64)?;
    let h = h.and_then(Value::as_u64)?;
    (w > 0 && h > 0).then_some((w as u32, h as u32))
}

/// A node's rectangle and rotation. chrome-lens-py gives Lens's geometry:
/// centre and size normalised to 0..1 (pixels when `coordinate_type` is
/// `IMAGE`), with the rotation in degrees. The CLI's JSON adds a pixel `bbox`
/// (the axis-aligned box around the rotated one), which wins when present.
/// Without an image size, normalised coordinates are passed through as they
/// are.
fn bounds_of(node: &Value, size: Option<(u32, u32)>) -> (Option<Rect>, f32) {
    let geometry = node.get("geometry").filter(|g| g.is_object());
    let angle =
        geometry.and_then(|g| g.get("angle_deg")).and_then(Value::as_f64).map(|d| d.to_radians() as f32).unwrap_or(0.0);
    if let Some(b) = node.get("bbox").filter(|b| b.is_object()) {
        let f = |k: &str| b.get(k).and_then(Value::as_f64);
        if let (Some(x), Some(y), Some(w), Some(h)) = (f("left"), f("top"), f("width"), f("height")) {
            return (Some(Rect::new(x as f32, y as f32, w as f32, h as f32)), angle);
        }
    }
    let Some(g) = geometry else { return (None, angle) };
    let f = |k: &str| g.get(k).and_then(Value::as_f64);
    let (Some(cx), Some(cy), Some(w), Some(h)) = (f("center_x"), f("center_y"), f("width"), f("height")) else {
        return (None, angle);
    };
    let pixels = g.get("coordinate_type").and_then(Value::as_str) == Some("IMAGE");
    let (sx, sy) = match size {
        Some((iw, ih)) if !pixels => (iw as f64, ih as f64),
        _ => (1.0, 1.0),
    };
    let (w, h) = (w * sx, h * sy);
    let rect = Rect::new((cx * sx - w / 2.0) as f32, (cy * sy - h / 2.0) as f32, w as f32, h as f32);
    (Some(rect), angle)
}

/// A node of `level` and its children, `depth` levels deep. Children are
/// under `lines` (strings or nodes), then `words`. Lens paragraphs are
/// blocks, and the levels go block → line → word.
fn block_of(node: &Value, size: Option<(u32, u32)>, level: OcrLevel, depth: usize) -> OcrBlock {
    if let Some(s) = node.as_str() {
        return OcrBlock::new(level, s);
    }
    let (bounds, angle) = bounds_of(node, size);
    let below = if level == OcrLevel::Block { OcrLevel::Line } else { OcrLevel::Word };
    let children = if depth == 0 {
        Vec::new()
    } else {
        node.get("lines")
            .or_else(|| node.get("words"))
            .and_then(Value::as_array)
            .map(|kids| kids.iter().map(|k| block_of(k, size, below, depth - 1)).collect())
            .unwrap_or_default()
    };
    OcrBlock { bounds, angle, children, ..OcrBlock::new(level, node.get("text").and_then(Value::as_str).unwrap_or("")) }
}

/// Hang Lens's per-paragraph translations (`translation_render_data`, one
/// entry per translated paragraph) on the blocks they belong to, and each
/// translated line on its line. An entry borrows its geometry from the
/// source paragraph's lines, so the block is the one holding the centre of
/// its first line; failing that, entries pair with blocks in order when
/// there are as many of each.
fn attach_translations(blocks: &mut [OcrBlock], render: &[Value], size: Option<(u32, u32)>) {
    for (i, entry) in render.iter().enumerate() {
        let Some(translation) = non_blank(entry.get("translation")) else { continue };
        let lines = arr(entry, "lines").map(Vec::as_slice).unwrap_or_default();
        let centre = lines
            .first()
            .and_then(|l| l.get("geometry"))
            .and_then(|g| bounds_of(&serde_json::json!({ "geometry": g }), size).0)
            .map(|r| r.centre());
        let by_place = centre.and_then(|(cx, cy)| {
            blocks.iter().position(|b| {
                b.level == Some(OcrLevel::Block)
                    && b.translation.is_none()
                    && b.bbox().is_some_and(|r| r.expanded(2.0).contains(cx, cy))
            })
        });
        let by_order = (render.len() == blocks.len()).then_some(i);
        let Some(block) = by_place.or(by_order).and_then(|at| blocks.get_mut(at)) else { continue };
        if block.level != Some(OcrLevel::Block) {
            continue;
        }
        // Line ranges index the paragraph's translation in UTF-16 units.
        let units: Vec<u16> = translation.encode_utf16().collect();
        if lines.len() == block.children.len() {
            for (line, rendered) in block.children.iter_mut().zip(lines) {
                let range = rendered.get("range").and_then(Value::as_array);
                let at = |k: usize| range.and_then(|r| r.get(k)).and_then(Value::as_u64).map(|v| v as usize);
                if let (Some(start), Some(end)) = (at(0), at(1)) {
                    let (start, end) = (start.min(units.len()), end.min(units.len()));
                    let text = String::from_utf16_lossy(&units[start..end.max(start)]);
                    if !text.trim().is_empty() {
                        line.translation = Some(text.trim().to_owned());
                    }
                }
            }
        }
        block.translation = Some(translation);
    }
}

fn joined_text(blocks: &[OcrBlock], preserve_line_breaks: bool) -> String {
    let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
    if preserve_line_breaks {
        text
    } else {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

fn arr<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    v.get(key).and_then(Value::as_array)
}

/// The daemon's reply (`LensAPI.process_image` as JSON) as our result. The
/// text sits under `ocr_text`, `text_blocks`, `line_blocks` or
/// `detailed_blocks` depending on the `output_format` asked for.
fn map_daemon_result(v: &Value, req: &OcrRequest) -> Result<OcrResult> {
    if !v.is_object() {
        return Err(Error::BadResponse { provider: NAME.into(), detail: "the reply is not a JSON object".into() });
    }
    let size = v.get("image_size").and_then(Value::as_array).and_then(|a| size_from(a.first(), a.get(1)));
    let mut blocks: Vec<OcrBlock> = if let Some(b) = arr(v, "detailed_blocks") {
        b.iter().map(|n| block_of(n, size, OcrLevel::Block, 2)).collect()
    } else if let Some(b) = arr(v, "text_blocks") {
        b.iter().map(|n| block_of(n, size, OcrLevel::Block, 1)).collect()
    } else if let Some(b) = arr(v, "line_blocks") {
        b.iter().map(|n| block_of(n, size, OcrLevel::Line, 0)).collect()
    } else {
        Vec::new()
    };
    if let Some(render) = arr(v, "translation_render_data") {
        attach_translations(&mut blocks, render, size);
    }
    let text = match v.get("ocr_text").and_then(Value::as_str) {
        Some(t) => t.to_owned(),
        None => joined_text(&blocks, req.preserve_line_breaks),
    };
    Ok(OcrResult {
        text,
        blocks,
        detected_lang: detected(v.get("detected_language")),
        translation: non_blank(v.get("translated_text")),
        image_size: size,
        ..Default::default()
    })
}

/// `lens_scan --format json` output for one image: `{file, width, height,
/// detected_language, ocr_text, translated_text, blocks: [{text, geometry,
/// bbox, lines: [{…, words: […]}]}]}`, or `{file, error}` when it failed.
fn map_cli_page(v: &Value, req: &OcrRequest) -> Result<OcrResult> {
    let page = match v {
        Value::Array(pages) => pages.first().unwrap_or(&Value::Null),
        other => other,
    };
    if !page.is_object() {
        return Err(Error::BadResponse { provider: NAME.into(), detail: "lens_scan printed no result object".into() });
    }
    if let Some(err) = non_blank(page.get("error")) {
        return Err(Error::Module { module: LENS_ID.into(), detail: err });
    }
    let size = size_from(page.get("width"), page.get("height"));
    let tree = arr(page, "blocks").map(Vec::as_slice).unwrap_or_default();
    let blocks: Vec<OcrBlock> = match req.granularity {
        OcrGranularity::FullText => Vec::new(),
        OcrGranularity::Blocks => tree.iter().map(|b| block_of(b, size, OcrLevel::Block, 1)).collect(),
        OcrGranularity::Lines => tree
            .iter()
            .filter_map(|b| arr(b, "lines"))
            .flatten()
            .map(|l| block_of(l, size, OcrLevel::Line, 0))
            .collect(),
        OcrGranularity::Words => tree.iter().map(|b| block_of(b, size, OcrLevel::Block, 2)).collect(),
    };
    Ok(OcrResult {
        text: page.get("ocr_text").and_then(Value::as_str).unwrap_or("").to_owned(),
        blocks,
        detected_lang: detected(page.get("detected_language")),
        translation: non_blank(page.get("translated_text")),
        image_size: size,
        ..Default::default()
    })
}

// -- the Unix-socket daemon -------------------------------------------------------

/// One HTTP/1.1 request over a Unix socket, the way chrome-lens-py's own
/// client talks to its daemon: `Connection: close`, read to EOF.
#[cfg(unix)]
fn unix_request(
    socket: &Path,
    method: Method,
    target: &str,
    token: &str,
    body: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<HttpResponse> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let io_err = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Timeout(timeout.as_millis() as u64),
        _ => Error::Network(format!("{NAME}: {}: {e}", socket.display())),
    };
    let mut stream = UnixStream::connect(socket).map_err(io_err)?;
    stream.set_read_timeout(Some(timeout)).map_err(io_err)?;
    stream.set_write_timeout(Some(timeout)).map_err(io_err)?;
    let mut head = format!("{} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n", method.as_str());
    if !token.is_empty() {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(b) = &body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(io_err)?;
    if let Some(b) = &body {
        stream.write_all(b).map_err(io_err)?;
    }
    let mut buf = Vec::new();
    stream.take(64 * 1024 * 1024).read_to_end(&mut buf).map_err(io_err)?;
    parse_http_response(&buf).map_err(|e| Error::Network(format!("{NAME}: {e}")))
}

#[cfg(not(unix))]
fn unix_request(
    socket: &Path,
    method: Method,
    target: &str,
    token: &str,
    body: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<HttpResponse> {
    let _ = (method, target, token, body, timeout);
    Err(Error::Config(format!("{} is a Unix socket, which this platform cannot reach", socket.display())))
}

/// Split a raw HTTP/1.x response into status, headers and body.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_http_response(buf: &[u8]) -> std::result::Result<HttpResponse, String> {
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("malformed reply")?;
    let head = String::from_utf8_lossy(&buf[..split]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("malformed status line")?;
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect();
    let mut body = buf[split + 4..].to_vec();
    if let Some(len) = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")) {
        let len: usize = len.1.parse().map_err(|_| "malformed content-length")?;
        if body.len() < len {
            return Err("reply cut short".into());
        }
        body.truncate(len);
    }
    Ok(HttpResponse { status, headers, body, codepage: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::ImageSource;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A daemon reply for `output_format: "full_text"`, shaped like
    /// `LensAPI.process_image`'s result after the server's `json_safe`.
    const FULL_TEXT: &str = r#"{
        "translated_text": "Привет, мир",
        "word_data": [
            {"word": "Hello,", "separator": " ", "geometry": {"center_x": 0.3, "center_y": 0.5, "width": 0.2, "height": 0.1, "angle_deg": 0.0, "coordinate_type": "NORMALIZED"}},
            {"word": "world", "separator": "", "geometry": {"center_x": 0.6, "center_y": 0.5, "width": 0.2, "height": 0.1, "angle_deg": 0.0, "coordinate_type": "NORMALIZED"}}
        ],
        "detected_language": "en",
        "objects": [],
        "translation_render_data": [],
        "overlay_path": null,
        "image_size": [800, 600],
        "ocr_text": "Hello, world"
    }"#;

    /// `output_format: "blocks"`: paragraphs whose `lines` are strings.
    const BLOCKS: &str = r#"{
        "translated_text": null,
        "word_data": [],
        "detected_language": "ja",
        "objects": [],
        "translation_render_data": [],
        "overlay_path": null,
        "image_size": [1000, 500],
        "text_blocks": [
            {"text": "第一行\n第二行", "lines": ["第一行", "第二行"],
             "geometry": {"center_x": 0.5, "center_y": 0.2, "width": 0.4, "height": 0.2, "angle_deg": 90.0}},
            {"text": "下", "lines": ["下"],
             "geometry": {"center_x": 0.1, "center_y": 0.9, "width": 0.1, "height": 0.1, "angle_deg": 0.0}}
        ]
    }"#;

    /// `lens_scan --format json` for one image (formats.page_to_dict).
    const CLI_PAGE: &str = r#"{
        "file": "/tmp/x.png",
        "width": 200,
        "height": 100,
        "detected_language": "de",
        "ocr_text": "Guten Tag\nWelt",
        "translated_text": "Good day\nWorld",
        "blocks": [
            {"text": "Guten Tag\nWelt",
             "geometry": {"center_x": 0.5, "center_y": 0.5, "width": 0.5, "height": 0.5, "angle_deg": 0.0},
             "bbox": {"left": 50, "top": 25, "width": 100, "height": 50},
             "lines": [
                {"text": "Guten Tag", "geometry": {"center_x": 0.5, "center_y": 0.4, "width": 0.5, "height": 0.2, "angle_deg": 0.0},
                 "bbox": {"left": 50, "top": 30, "width": 100, "height": 20},
                 "words": [
                    {"text": "Guten", "separator": " ", "geometry": {"center_x": 0.4, "center_y": 0.4, "width": 0.2, "height": 0.2, "angle_deg": 0.0}, "bbox": {"left": 60, "top": 30, "width": 40, "height": 20}},
                    {"text": "Tag", "separator": "", "geometry": null, "bbox": null}
                 ]},
                {"text": "Welt", "geometry": {"center_x": 0.5, "center_y": 0.6, "width": 0.2, "height": 0.2, "angle_deg": 0.0},
                 "bbox": {"left": 80, "top": 50, "width": 40, "height": 20}, "words": []}
             ]}
        ]
    }"#;

    fn req() -> OcrRequest {
        OcrRequest::new(ImageSource::Bytes(vec![0x89, b'P', b'N', b'G', 0, 1, 2]))
    }

    #[test]
    fn the_request_body_follows_the_request() {
        let mut r = req();
        let body = request_body(&r, b"abc");
        assert_eq!(body["image_b64"], "YWJj");
        assert_eq!(body["output_format"], "full_text");
        assert_eq!(body["ocr_preserve_line_breaks"], true);
        assert!(body.get("ocr_language").is_none(), "auto is left to Lens");
        assert!(body.get("translate_to").is_none());

        r.lang = LangId::from_code("zh-CN").unwrap();
        r.translate_to = Some(LangId::from_code("zh-TW").unwrap());
        r.granularity = OcrGranularity::Blocks;
        r.preserve_line_breaks = false;
        let body = request_body(&r, b"abc");
        assert_eq!(body["ocr_language"], "zh-Hans");
        assert_eq!(body["translate_to"], "zh-TW");
        assert_eq!(body["output_format"], "blocks");
        assert_eq!(body["ocr_preserve_line_breaks"], false);
        r.granularity = OcrGranularity::Words;
        assert_eq!(request_body(&r, b"").get("output_format").unwrap(), "detailed");
    }

    #[test]
    fn a_full_text_reply_maps() {
        let r = map_daemon_result(&serde_json::from_str(FULL_TEXT).unwrap(), &req()).unwrap();
        assert_eq!(r.text, "Hello, world");
        assert_eq!(r.translation.as_deref(), Some("Привет, мир"));
        assert_eq!(r.detected_lang, Some(LangId::ENGLISH));
        assert_eq!(r.image_size, Some((800, 600)));
        assert!(r.blocks.is_empty());
    }

    #[test]
    fn block_geometry_becomes_pixels() {
        let mut q = req();
        q.granularity = OcrGranularity::Blocks;
        let r = map_daemon_result(&serde_json::from_str(BLOCKS).unwrap(), &q).unwrap();
        assert_eq!(r.text, "第一行\n第二行\n下", "joined from the blocks when ocr_text is absent");
        assert_eq!(r.translation, None, "null translation");
        assert_eq!(r.detected_lang, LangId::from_code("ja"));
        assert_eq!(r.blocks.len(), 2);
        let b = &r.blocks[0];
        assert_eq!(b.bounds, Some(Rect::new(300.0, 50.0, 400.0, 100.0)));
        assert!((b.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "degrees to radians");
        assert_eq!(b.children.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["第一行", "第二行"]);

        q.preserve_line_breaks = false;
        let flat = map_daemon_result(&serde_json::from_str(BLOCKS).unwrap(), &q).unwrap();
        assert_eq!(flat.text, "第一行 第二行 下");
    }

    #[test]
    fn detailed_and_line_replies_map() {
        let detailed = serde_json::json!({
            "image_size": [100, 100],
            "detected_language": "N/A",
            "detailed_blocks": [{"text": "a b", "geometry": {"center_x": 0.5, "center_y": 0.5, "width": 0.2, "height": 0.2, "angle_deg": 0.0},
                "lines": [{"text": "a b", "geometry": null, "words": [
                    {"text": "a", "separator": " ", "geometry": {"center_x": 10.0, "center_y": 10.0, "width": 4.0, "height": 2.0, "angle_deg": 0.0, "coordinate_type": "IMAGE"}},
                    {"text": "b", "separator": "", "geometry": null}]}]}]
        });
        let r = map_daemon_result(&detailed, &req()).unwrap();
        assert_eq!(r.detected_lang, None, "N/A is not a language");
        let word = &r.blocks[0].children[0].children[0];
        assert_eq!(word.text, "a");
        assert_eq!(word.bounds, Some(Rect::new(8.0, 9.0, 4.0, 2.0)), "IMAGE coordinates are already pixels");
        assert_eq!(r.blocks[0].children[0].children[1].bounds, None);

        let lines = serde_json::json!({"line_blocks": [{"text": "x", "geometry": {"center_x": 0.5, "center_y": 0.5, "width": 1.0, "height": 1.0}}]});
        let r = map_daemon_result(&lines, &req()).unwrap();
        assert_eq!(r.text, "x");
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(0.0, 0.0, 1.0, 1.0)), "no size: normalised as given");
        assert!(map_daemon_result(&serde_json::json!([1]), &req()).is_err());
    }

    #[test]
    fn per_paragraph_translations_hang_on_their_blocks_and_lines() {
        let g = |cy: f64| serde_json::json!({"center_x": 0.5, "center_y": cy, "width": 0.4, "height": 0.1, "angle_deg": 0.0});
        let reply = serde_json::json!({
            "image_size": [100, 100],
            "translated_text": "Привет мир\nПока",
            "detailed_blocks": [
                {"text": "Hello\nworld", "geometry": g(0.2),
                 "lines": [{"text": "Hello", "geometry": g(0.15), "words": []}, {"text": "world", "geometry": g(0.25), "words": []}]},
                {"text": "Bye", "geometry": g(0.8), "lines": [{"text": "Bye", "geometry": g(0.8), "words": []}]}
            ],
            // Listed out of order: matched by where the first line sits.
            "translation_render_data": [
                {"translation": "Пока", "lines": [{"range": [0, 4], "geometry": g(0.8)}]},
                {"translation": "Привет мир", "lines": [
                    {"range": [0, 6], "geometry": g(0.15)}, {"range": [7, 10], "geometry": g(0.25)}]}
            ]
        });
        let r = map_daemon_result(&reply, &req()).unwrap();
        assert_eq!(r.blocks[0].level, Some(OcrLevel::Block));
        assert_eq!(r.blocks[0].children[0].level, Some(OcrLevel::Line));
        assert_eq!(r.blocks[0].translation.as_deref(), Some("Привет мир"));
        assert_eq!(r.blocks[0].children[0].translation.as_deref(), Some("Привет"));
        assert_eq!(r.blocks[0].children[1].translation.as_deref(), Some("мир"));
        assert_eq!(r.blocks[1].translation.as_deref(), Some("Пока"));
        assert_eq!(r.translation.as_deref(), Some("Привет мир\nПока"), "the whole translation stays");
    }

    #[test]
    fn cli_json_maps_at_every_granularity() {
        let v: Value = serde_json::from_str(CLI_PAGE).unwrap();
        let mut q = req();
        let r = map_cli_page(&v, &q).unwrap();
        assert_eq!(r.text, "Guten Tag\nWelt");
        assert_eq!(r.translation.as_deref(), Some("Good day\nWorld"));
        assert_eq!(r.detected_lang, LangId::from_code("de"));
        assert_eq!(r.image_size, Some((200, 100)));
        assert!(r.blocks.is_empty());

        q.granularity = OcrGranularity::Blocks;
        let r = map_cli_page(&v, &q).unwrap();
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(50.0, 25.0, 100.0, 50.0)), "the pixel bbox wins");
        assert_eq!(r.blocks[0].children.len(), 2);
        assert!(r.blocks[0].children[0].children.is_empty(), "no words at block granularity");

        q.granularity = OcrGranularity::Lines;
        let r = map_cli_page(&v, &q).unwrap();
        assert_eq!(r.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Guten Tag", "Welt"]);

        q.granularity = OcrGranularity::Words;
        let r = map_cli_page(&v, &q).unwrap();
        let words = &r.blocks[0].children[0].children;
        assert_eq!(words[0].bounds, Some(Rect::new(60.0, 30.0, 40.0, 20.0)));
        assert_eq!(words[1].text, "Tag");
        assert_eq!(words[1].bounds, None);

        let several = Value::Array(vec![v.clone(), v]);
        assert_eq!(map_cli_page(&several, &req()).unwrap().text, "Guten Tag\nWelt");
        let failed = serde_json::json!({"file": "x.png", "error": "Lens is unreachable"});
        assert!(map_cli_page(&failed, &req()).unwrap_err().to_string().contains("unreachable"));
    }

    #[test]
    fn cli_arguments() {
        let mut q = req();
        let args = |q: &OcrRequest| {
            cli_args(Path::new("/t/i.png"), q).into_iter().map(|a| a.into_string().unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(args(&q), ["/t/i.png", "--format", "json", "-q"]);
        q.lang = LangId::from_code("ja").unwrap();
        q.translate_to = Some(LangId::RUSSIAN);
        q.preserve_line_breaks = false;
        assert_eq!(args(&q), ["/t/i.png", "ja", "-t", "ru", "--ocr-single-line", "--format", "json", "-q"]);
    }

    #[test]
    fn discovery_files_parse_as_chrome_lens_writes_them() {
        let tcp = r#"{"version": "3.8.2", "pid": 18008, "transport": "tcp", "address": "127.0.0.1:8765", "token": "tok", "started_at": 1793100150.52, "managed": "service"}"#;
        assert_eq!(
            parse_daemon_file(tcp),
            Some(Endpoint::Http { base: "http://127.0.0.1:8765".into(), token: "tok".into(), from: "daemon.json" })
        );
        let unix = r#"{"version": "3.8.2", "pid": 1, "transport": "unix", "address": "/run/user/1000/chrome-lens-py/daemon.sock", "token": "t", "started_at": 0, "managed": "auto"}"#;
        assert_eq!(
            parse_daemon_file(unix),
            Some(Endpoint::Unix { path: "/run/user/1000/chrome-lens-py/daemon.sock".into(), token: "t".into() })
        );
        assert_eq!(parse_daemon_file(r#"{"transport": "pigeon", "address": "x", "token": "t"}"#), None);
        assert_eq!(parse_daemon_file(r#"{"transport": "tcp", "address": "x"}"#), None, "no token");
        assert_eq!(parse_daemon_file("not json"), None);
        assert_eq!(
            parse_service_file(r#"{"port": 9000, "token": "s"}"#),
            Some(Endpoint::Http { base: "http://127.0.0.1:9000".into(), token: "s".into(), from: "service.json" })
        );
        assert_eq!(parse_service_file(r#"{"port": 99999, "token": "s"}"#), None);
    }

    #[test]
    fn state_directories_follow_runtime_py() {
        let home = Some(PathBuf::from("/home/u"));
        let none = |_: &str| None;
        assert_eq!(
            lens_dirs(&none, home.clone(), Os::Linux),
            (Some("/home/u/.cache/chrome-lens-py".into()), Some("/home/u/.cache/chrome-lens-py".into()))
        );
        assert_eq!(
            lens_dirs(&none, home.clone(), Os::Mac).0,
            Some(PathBuf::from("/home/u/Library/Caches/chrome-lens-py"))
        );
        let win = |k: &str| (k == "LOCALAPPDATA").then(|| OsString::from("C:/Users/u/AppData/Local"));
        let (state, data) = lens_dirs(&win, home.clone(), Os::Windows);
        assert_eq!(state, Some(PathBuf::from("C:/Users/u/AppData/Local").join("chrome-lens-py")));
        assert_eq!(state, data);
        let runtime_dir = std::env::temp_dir();
        let xdg = |k: &str| (k == "XDG_RUNTIME_DIR").then(|| runtime_dir.clone().into_os_string());
        let (state, data) = lens_dirs(&xdg, home.clone(), Os::Linux);
        assert_eq!(state, Some(runtime_dir.join("chrome-lens-py")), "the runtime dir, when it exists");
        assert_eq!(data, Some(PathBuf::from("/home/u/.cache/chrome-lens-py")), "the service file outlives logout");
        let over = |k: &str| (k == "LENS_STATE_DIR").then(|| OsString::from("/x"));
        assert_eq!(lens_dirs(&over, home, Os::Windows), (Some("/x".into()), Some("/x".into())));
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("glossa-lens-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_cli_is_found_on_path_or_in_extra_dirs() {
        let dir = temp("which");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let exe = if cfg!(windows) { "lens_scan.exe" } else { "lens_scan" };
        std::fs::write(b.join(exe), "").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(find_cli("", Some(path.clone()), &[]), Some(b.join(exe)));
        assert_eq!(find_cli("", None, &[a.clone(), b.clone()]), Some(b.join(exe)));
        assert_eq!(find_cli("", Some(std::env::join_paths([&a]).unwrap()), &[]), None);
        let explicit = dir.join("custom").join("lens");
        assert_eq!(find_cli(explicit.to_str().unwrap(), Some(path), &[]), Some(explicit), "a path is taken as given");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://127.0.0.1:8765"));
        assert!(is_loopback_url("http://localhost:1/x"));
        assert!(is_loopback_url("http://[::1]:8765/"));
        assert!(!is_loopback_url("http://192.168.1.5:8765"));
        assert!(!is_loopback_url("https://lens.example.com"));
    }

    /// A temp directory with chrome-lens-py's files in it.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            Self { dir: temp(name) }
        }

        fn daemon(&self, port: u16, token: &str) {
            let json = serde_json::json!({"version": "3.8.2", "pid": 1, "transport": "tcp", "address": format!("127.0.0.1:{port}"), "token": token, "started_at": 0.0, "managed": "auto"});
            std::fs::write(self.dir.join("daemon.json"), json.to_string()).unwrap();
        }

        fn service(&self, port: u16, token: &str) {
            std::fs::write(
                self.dir.join("service.json"),
                serde_json::json!({"port": port, "token": token}).to_string(),
            )
            .unwrap();
        }

        fn locator(&self, cli: Option<PathBuf>) -> Locator {
            Locator {
                daemon_file: Some(self.dir.join("daemon.json")),
                service_file: Some(self.dir.join("service.json")),
                cli,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn cfg(mode: LensMode) -> LensConfig {
        LensConfig { mode, ..Default::default() }
    }

    fn recognize(p: &LensProvider) -> Result<OcrResult> {
        let host_http = MockHttp::new(); // must never be used for loopback
        let opts = OptionValues::new();
        let r = with_test_host(&host_http, &opts, |host| p.recognize(host, &req()));
        assert!(host_http.requests().is_empty(), "loopback traffic went through the proxied client");
        r
    }

    fn mock_daemon(port: u16) -> MockHttp {
        MockHttp::new()
            .on(
                Method::Get,
                &format!("http://127.0.0.1:{port}/health"),
                200,
                r#"{"status": "ok", "service": "chrome-lens-py", "version": "3.8.2"}"#,
            )
            .on(Method::Post, &format!("http://127.0.0.1:{port}/v1/ocr"), 200, FULL_TEXT)
    }

    #[test]
    fn the_discovery_file_comes_before_the_service_file() {
        let fx = Fixture::new("order");
        fx.daemon(5001, "dtok");
        fx.service(8765, "stok");
        let http = Arc::new(mock_daemon(5001));
        let p = LensProvider::with_locator(cfg(LensMode::Auto), fx.locator(None)).with_loopback_http(http.clone());
        let r = recognize(&p).unwrap();
        assert_eq!(r.text, "Hello, world");
        assert_eq!(r.provider_id, "lens");
        let seen = http.requests();
        assert_eq!(seen[0].url, "http://127.0.0.1:5001/health");
        let post = seen.iter().find(|r| r.method == Method::Post).unwrap();
        assert_eq!(post.url, "http://127.0.0.1:5001/v1/ocr");
        assert!(post.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer dtok"));
        let body: Value = serde_json::from_slice(post.body.as_ref().unwrap()).unwrap();
        assert!(body["image_b64"].as_str().unwrap().starts_with("iVBORw"));

        // Cached: the next call goes straight to the OCR route.
        let before = http.requests().len();
        recognize(&p).unwrap();
        assert_eq!(http.requests().len(), before + 1);
    }

    #[test]
    fn a_dead_discovery_file_falls_back_to_the_service_and_settings_come_first() {
        let fx = Fixture::new("fallback");
        fx.daemon(5002, "dtok"); // nothing answers there
        fx.service(8765, "stok");
        let http = Arc::new(mock_daemon(8765));
        let p = LensProvider::with_locator(cfg(LensMode::Daemon), fx.locator(None)).with_loopback_http(http.clone());
        recognize(&p).unwrap();
        let post = http.requests().into_iter().find(|r| r.method == Method::Post).unwrap();
        assert_eq!(post.url, "http://127.0.0.1:8765/v1/ocr");
        assert!(post.headers.iter().any(|(_, v)| v == "Bearer stok"));

        let configured =
            LensConfig { endpoint: "127.0.0.1:7000/".into(), token: "mine".into(), ..cfg(LensMode::Daemon) };
        let http = Arc::new(mock_daemon(7000).on(Method::Get, "http://127.0.0.1:8765/health", 200, "{}"));
        let p = LensProvider::with_locator(configured, fx.locator(None)).with_loopback_http(http.clone());
        recognize(&p).unwrap();
        let post = http.requests().into_iter().find(|r| r.method == Method::Post).unwrap();
        assert_eq!(post.url, "http://127.0.0.1:7000/v1/ocr");
        assert!(post.headers.iter().any(|(_, v)| v == "Bearer mine"));
    }

    #[test]
    fn daemon_mode_without_a_daemon_says_so() {
        let fx = Fixture::new("nodaemon");
        let p = LensProvider::with_locator(cfg(LensMode::Daemon), fx.locator(None))
            .with_loopback_http(Arc::new(MockHttp::new()));
        let err = recognize(&p).unwrap_err();
        assert!(err.to_string().contains("lens_scan daemon start"), "{err}");
        let p = LensProvider::with_locator(cfg(LensMode::Cli), fx.locator(None));
        assert!(recognize(&p).is_err());
    }

    /// An [`Http`] whose behaviour the test can flip.
    struct Switch {
        a_alive: AtomicBool,
        seen: Mutex<Vec<String>>,
    }

    impl Http for Switch {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(request.url.clone());
            let ok =
                |body: &str| Ok(HttpResponse { status: 200, body: body.as_bytes().to_vec(), ..Default::default() });
            let a = self.a_alive.load(Ordering::SeqCst);
            match request.url.as_str() {
                u if u.starts_with("http://127.0.0.1:5003/") && a => {
                    ok(if u.ends_with("/health") { "{}" } else { FULL_TEXT })
                }
                u if u.starts_with("http://127.0.0.1:8765/") => ok(if u.ends_with("/health") { "{}" } else { BLOCKS }),
                u => Err(Error::Network(format!("connection refused: {u}"))),
            }
        }
    }

    #[test]
    fn a_lost_daemon_is_looked_up_again_within_the_request() {
        let fx = Fixture::new("relookup");
        fx.daemon(5003, "a");
        fx.service(8765, "b");
        let http = Arc::new(Switch { a_alive: AtomicBool::new(true), seen: Mutex::new(Vec::new()) });
        let p = LensProvider::with_locator(cfg(LensMode::Auto), fx.locator(None)).with_loopback_http(http.clone());
        assert_eq!(recognize(&p).unwrap().text, "Hello, world", "daemon A");
        http.a_alive.store(false, Ordering::SeqCst);
        let r = recognize(&p).unwrap();
        assert_eq!(r.text, "第一行\n第二行\n下", "fell over to the service daemon in the same call");
        let seen = http.seen.lock().unwrap().clone();
        assert_eq!(seen.last().map(String::as_str), Some("http://127.0.0.1:8765/v1/ocr"));
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn cli_mode_runs_lens_scan_per_image() {
        let fx = Fixture::new("cli");
        let args_file = fx.dir.join("args");
        let page = fx.dir.join("page.json");
        std::fs::write(&page, CLI_PAGE).unwrap();
        let cli = script(
            &fx.dir,
            "lens_scan",
            &format!("echo \"$@\" > '{}'\ntest -f \"$1\" || exit 9\ncat '{}'", args_file.display(), page.display()),
        );
        let p = LensProvider::with_locator(cfg(LensMode::Cli), fx.locator(Some(cli)));
        let r = recognize(&p).unwrap();
        assert_eq!(r.text, "Guten Tag\nWelt");
        let args = std::fs::read_to_string(&args_file).unwrap();
        assert!(args.contains(".png --format json -q"), "{args}");

        let broken = script(&fx.dir, "broken", "echo 'Traceback: boom' >&2; exit 1");
        let p = LensProvider::with_locator(cfg(LensMode::Cli), fx.locator(Some(broken)));
        let err = recognize(&p).unwrap_err().to_string();
        assert!(err.contains("code 1") && err.contains("boom"), "{err}");
    }

    #[cfg(unix)]
    /// Lens itself, for the built-in client: answers the crupload endpoint
    /// with one paragraph and remembers what it was sent.
    struct FakeLens {
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Http for FakeLens {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            use glossa_lens::proto::*;
            use prost::Message as _;
            self.seen.lock().unwrap().push(request.clone());
            assert_eq!(request.url, glossa_lens::ENDPOINT);
            let word = |t: &str, sep: &str| text_layout::Word {
                plain_text: Some(t.into()),
                text_separator: Some(sep.into()),
                ..Default::default()
            };
            let geometry = Some(Geometry {
                bounding_box: Some(CenterRotatedBox {
                    center_x: Some(0.5),
                    center_y: Some(0.5),
                    width: Some(0.5),
                    height: Some(0.25),
                    rotation_z: Some(0.0),
                    coordinate_type: Some(CoordinateType::Normalized as i32),
                }),
                ..Default::default()
            });
            let line =
                text_layout::Line { words: vec![word("Hello,", " "), word("world", "")], geometry: geometry.clone() };
            let reply = LensOverlayServerResponse {
                objects_response: Some(LensOverlayObjectsResponse {
                    text: Some(Text {
                        text_layout: Some(TextLayout {
                            paragraphs: vec![text_layout::Paragraph {
                                lines: vec![line],
                                geometry,
                                ..Default::default()
                            }],
                        }),
                        content_language: Some("en".into()),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            Ok(HttpResponse { status: 200, body: reply.encode_to_vec(), ..Default::default() })
        }
    }

    #[test]
    fn auto_mode_without_a_daemon_uses_the_built_in_client() {
        let fx = Fixture::new("native");
        // No daemon answers, and no chrome-lens-py is installed either.
        let p = LensProvider::with_locator(cfg(LensMode::Auto), fx.locator(None))
            .with_loopback_http(Arc::new(MockHttp::new()));
        let lens = FakeLens { seen: Mutex::new(Vec::new()) };
        let mut png = Vec::new();
        image::RgbImage::from_pixel(200, 100, image::Rgb([255, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut request = OcrRequest::new(ImageSource::Bytes(png));
        request.granularity = OcrGranularity::Blocks;
        let opts = OptionValues::new();
        let r = with_test_host(&lens, &opts, |host| p.recognize(host, &request)).unwrap();
        assert_eq!(r.text, "Hello, world");
        assert_eq!(r.image_size, Some((200, 100)));
        // The box is in pixels of the picture given: the middle half.
        let b = r.blocks[0].bounds.unwrap();
        assert_eq!((b.x, b.y, b.width, b.height), (50.0, 37.5, 100.0, 25.0));
        let sent = lens.seen.lock().unwrap();
        assert!(sent[0].headers.iter().any(|(k, v)| k == "X-Goog-Api-Key" && !v.is_empty()));
        assert!(sent[0].headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/x-protobuf"));
    }

    #[cfg(unix)]
    #[test]
    fn a_unix_socket_daemon_is_reachable() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;

        let fx = Fixture::new("unix");
        let sock = fx.dir.join("daemon.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let mut auth_ok = true;
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 65536];
                // Read the head, then as much body as Content-Length says.
                loop {
                    let n = s.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..at]).to_string();
                        auth_ok &= head.contains("Authorization: Bearer sock-token");
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .map(|v| v.parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if buf.len() >= at + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let body = if buf.starts_with(b"GET /health") { "{\"status\":\"ok\"}" } else { FULL_TEXT };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(reply.as_bytes()).unwrap();
            }
            auth_ok
        });
        let record = serde_json::json!({"version": "3.8.2", "pid": 1, "transport": "unix", "address": sock, "token": "sock-token", "started_at": 0.0, "managed": "auto"});
        std::fs::write(fx.dir.join("daemon.json"), record.to_string()).unwrap();
        let p = LensProvider::with_locator(cfg(LensMode::Daemon), fx.locator(None))
            .with_loopback_http(Arc::new(MockHttp::new()));
        let r = recognize(&p).unwrap();
        assert_eq!(r.translation.as_deref(), Some("Привет, мир"));
        assert!(server.join().unwrap(), "the token was sent");
    }

    #[test]
    fn raw_http_replies_parse() {
        let r =
            parse_http_response(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\nX: y\r\n\r\nhello-extra").unwrap();
        assert_eq!(r.status, 401);
        assert_eq!(r.body, b"hello");
        assert_eq!(r.header("x"), Some("y"));
        assert!(parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort").is_err());
        assert!(parse_http_response(b"garbage").is_err());
    }

    #[test]
    fn error_bodies_are_unwrapped() {
        let e = readable_error(Error::HttpStatus {
            provider: "p".into(),
            status: 400,
            body: r#"{"error": "'image_b64' is not valid base64", "request_id": "x"}"#.into(),
        });
        assert!(matches!(e, Error::HttpStatus { ref body, .. } if body == "'image_b64' is not valid base64"));
    }
}
