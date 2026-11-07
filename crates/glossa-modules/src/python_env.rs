//! The Python a Python module runs with: a system interpreter, or a Python
//! build Glossa downloads itself, and for a module with dependencies an
//! environment of its own that `uv` fills.
//!
//! * The **managed Python** is a python-build-standalone build
//!   (`install_only_stripped`), pinned below by version, URL and SHA-256 for
//!   each supported system, unpacked into `<runtimes>/python-<version>/`.
//!   One copy serves every module.
//! * **`uv`**, pinned the same way, lands in `<runtimes>/uv-<version>/`. It is
//!   fetched only for a module that declares dependencies.
//! * A module declares dependencies with `requirements = [...]` in its
//!   `[module]` section, or a `requirements.txt` or `pyproject.toml` beside
//!   `module.toml`. It then runs in `<venvs>/<module-id>/`, made by
//!   `uv venv` and filled by `uv pip install`. A hash of the requirements
//!   (and of the interpreter and `uv` version) is kept in the environment;
//!   when it changes, the environment is made again.
//!
//! Downloads go through [`ModuleAssets`], so they resume, are checked
//! against their SHA-256 before use, unpack with the same caps and stay
//! below their folder. Nothing here runs when Glossa starts: the module's
//! first call finds out what is missing ([`PythonEnv::ready`]) and either
//! fails with [`Error::RuntimeMissing`] for the front end to offer the
//! download, or downloads it on that call's worker when automatic downloads
//! are on.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use glossa_core::{Error, Result};
use glossa_net::Http;
use sha2::{Digest, Sha256};

use crate::assets::{self, AssetSpec, AssetStatus, ModuleAssets, Progress, Unpack, Urls};
use crate::manifest::Manifest;
use crate::provider::CancelToken;

/// The managed Python's version.
pub const PYTHON_VERSION: &str = "3.13.18";
/// The python-build-standalone release it comes from.
const PBS_RELEASE: &str = "20261026";
/// The `uv` release used for dependencies.
pub const UV_VERSION: &str = "0.13.7";

/// How long making one module's environment may take: `uv` downloads and
/// unpacks wheels, which on a slow line takes a while, but must not hang.
const INSTALL_BUDGET: Duration = Duration::from_secs(20 * 60);

/// The environment variables `uv` keeps from Glossa's; everything else is
/// cleared. On Windows a program needs `SYSTEMROOT` and friends to start at
/// all; `SSL_CERT_*` point at certificates a company network may require.
const KEPT_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// The marker inside a module's environment recording what it was made
/// from.
const VENV_MARKER: &str = ".glossa-requirements";

/// One supported system: the target triple both projects name their
/// builds after, and the pinned downloads.
struct Pin {
    os: &'static str,
    arch: &'static str,
    triple: &'static str,
    python_sha256: &'static str,
    python_size: u64,
    uv_sha256: &'static str,
    uv_size: u64,
}

/// The pinned builds. To update: pick a python-build-standalone release and
/// a uv release, and take each file's size and SHA-256 from the release
/// (`gh api repos/astral-sh/python-build-standalone/releases/tags/<tag>`
/// lists both as `size` and `digest`).
const PINS: &[Pin] = &[
    Pin {
        os: "macos",
        arch: "aarch64",
        triple: "aarch64-apple-darwin",
        python_sha256: "8392b6ea51d94391befb2c31ebe75463f045d0b69dfcf5e1bb3234e1f5fa3405",
        python_size: 25_265_774,
        uv_sha256: "81b811b57e4624df7ed3aac148e0359100d9962f3d9dce00aec533dc2e086731",
        uv_size: 14_347_867,
    },
    Pin {
        os: "macos",
        arch: "x86_64",
        triple: "x86_64-apple-darwin",
        python_sha256: "7412c19d896c99431b74a18a5643e110449a7f9590fbab179c3e5ab27b9aa9e3",
        python_size: 25_055_263,
        uv_sha256: "20c47e4c1f630b8abaff8c291fa9b9aa300fef92c4fbff95adf198b55287cda7",
        uv_size: 17_026_382,
    },
    Pin {
        os: "windows",
        arch: "x86_64",
        triple: "x86_64-pc-windows-msvc",
        python_sha256: "7369f7a35f261222a6ac8b3a87c1c88eeebee8af945d85346202591a64038dd3",
        python_size: 22_023_022,
        uv_sha256: "0cdc5b38fe5a8168ed6a25814f5ba43c21d313dd3187314fccb37b7cec46e021",
        uv_size: 15_236_317,
    },
    Pin {
        os: "windows",
        arch: "aarch64",
        triple: "aarch64-pc-windows-msvc",
        python_sha256: "bd3315abb8abb8ce5b19c2c2b709d318f983e0b8753d0830831df07c46474699",
        python_size: 21_036_090,
        uv_sha256: "505d9e65e7692bac3ddf98c66754aa26c5050c409c184af30a881a0a72cc85e3",
        uv_size: 15_538_425,
    },
    Pin {
        os: "linux",
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        python_sha256: "df15fe60d3ff55bf9dcf25824b5572756bb8816ff93865a273d3301d7066a8b7",
        python_size: 35_395_906,
        uv_sha256: "c5221ff41b7892bfc223543883600125f6a47fe845093e0f5bdff4f359196d83",
        uv_size: 16_671_812,
    },
    Pin {
        os: "linux",
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        python_sha256: "578d9da1983463ffb0c8c98ab0da2f18177f44069328eb0af847860c1eb05c3e",
        python_size: 29_492_455,
        uv_sha256: "71279f8d3c67241e807b9d8db872d1bf541579fb9f08b7d1eb1682196d1e57dc",
        uv_size: 15_896_996,
    },
];

impl Pin {
    fn find(os: &str, arch: &str) -> Option<&'static Pin> {
        PINS.iter().find(|p| p.os == os && p.arch == arch)
    }

    fn python(&self) -> Tool {
        let url = format!(
            "https://github.com/astral-sh/python-build-standalone/releases/download/{PBS_RELEASE}/cpython-{PYTHON_VERSION}%2B{PBS_RELEASE}-{}-install_only_stripped.tar.gz",
            self.triple
        );
        // The archive holds `python/`. Its `bin/python3` is a symbolic link,
        // which the unpacker skips, so the real file is named.
        let minor = PYTHON_VERSION.rsplit_once('.').map_or(PYTHON_VERSION, |(m, _)| m);
        let exe = if self.os == "windows" {
            PathBuf::from("python").join("python.exe")
        } else {
            PathBuf::from("python").join("bin").join(format!("python{minor}"))
        };
        Tool {
            folder: format!("python-{PYTHON_VERSION}"),
            spec: spec(self.triple, url, self.python_sha256, self.python_size, Unpack::TarGz),
            exe,
        }
    }

    fn uv(&self) -> Tool {
        let windows = self.os == "windows";
        let ext = if windows { "zip" } else { "tar.gz" };
        let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-{}.{ext}", self.triple);
        let exe =
            if windows { PathBuf::from("uv.exe") } else { PathBuf::from(format!("uv-{}", self.triple)).join("uv") };
        let unpack = if windows { Unpack::Zip } else { Unpack::TarGz };
        Tool {
            folder: format!("uv-{UV_VERSION}"),
            spec: spec(self.triple, url, self.uv_sha256, self.uv_size, unpack),
            exe,
        }
    }
}

fn spec(name: &str, url: String, sha256: &str, size: u64, unpack: Unpack) -> AssetSpec {
    AssetSpec {
        name: name.to_owned(),
        url: Urls::One(url),
        sha256: sha256.to_owned(),
        size,
        unpack,
        os: Vec::new(),
        arch: Vec::new(),
        optional: false,
        executable: false,
    }
}

/// One downloaded program: its archive and where the executable is inside.
#[derive(Debug, Clone)]
pub struct Tool {
    /// The folder below the runtimes folder (`python-3.13.18`).
    pub folder: String,
    pub spec: AssetSpec,
    /// The executable, relative to the unpacked archive.
    pub exe: PathBuf,
}

/// Where the managed Python, `uv`, the modules' environments and `uv`'s
/// cache go. The program sets these from its data folders.
#[derive(Debug, Clone)]
pub struct PythonDirs {
    /// `runtimes/`: `python-<version>/`, `uv-<version>/`.
    pub runtimes: PathBuf,
    /// `venvs/`: one environment per module id.
    pub venvs: PathBuf,
    /// `uv`'s download cache.
    pub uv_cache: PathBuf,
    /// The proxy `uv` goes through, from Glossa's network settings (a URL,
    /// credentials included); `None` for a direct connection.
    pub proxy: Option<String>,
}

impl Default for PythonDirs {
    fn default() -> Self {
        let tmp = std::env::temp_dir().join("glossa-python");
        Self { runtimes: tmp.join("runtimes"), venvs: tmp.join("venvs"), uv_cache: tmp.join("uv-cache"), proxy: None }
    }
}

// -- the managed Python and uv ----------------------------------------------------

/// The managed Python and `uv` for this system.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub dirs: PythonDirs,
    python: Option<Tool>,
    uv: Option<Tool>,
}

impl Toolchain {
    /// The pinned builds for the system this runs on.
    pub fn new(dirs: PythonDirs) -> Self {
        Self::for_platform(dirs, std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn for_platform(dirs: PythonDirs, os: &str, arch: &str) -> Self {
        let pin = Pin::find(os, arch);
        Self { dirs, python: pin.map(Pin::python), uv: pin.map(Pin::uv) }
    }

    /// Other builds than the pinned ones (tests).
    pub fn with_tools(dirs: PythonDirs, python: Tool, uv: Tool) -> Self {
        Self { dirs, python: Some(python), uv: Some(uv) }
    }

    /// Whether there is a managed Python for this system at all.
    pub fn supported(&self) -> bool {
        self.python.is_some()
    }

    pub fn python_tool(&self) -> Option<&Tool> {
        self.python.as_ref()
    }

    pub fn uv_tool(&self) -> Option<&Tool> {
        self.uv.as_ref()
    }

    fn assets(&self, tool: &Tool) -> ModuleAssets {
        ModuleAssets::new(&tool.folder, &self.dirs.runtimes, vec![tool.spec.clone()])
    }

    pub fn python_assets(&self) -> Option<ModuleAssets> {
        self.python.as_ref().map(|t| self.assets(t))
    }

    pub fn uv_assets(&self) -> Option<ModuleAssets> {
        self.uv.as_ref().map(|t| self.assets(t))
    }

    fn exe(&self, tool: &Tool) -> PathBuf {
        self.dirs.runtimes.join(&tool.folder).join(&tool.spec.name).join(&tool.exe)
    }

    /// The managed interpreter's path, downloaded or not.
    pub fn python_exe(&self) -> Option<PathBuf> {
        self.python.as_ref().map(|t| self.exe(t))
    }

    pub fn uv_exe(&self) -> Option<PathBuf> {
        self.uv.as_ref().map(|t| self.exe(t))
    }

    fn tool_ready(&self, tool: Option<&Tool>) -> bool {
        tool.is_some_and(|t| self.assets(t).ready() && self.exe(t).is_file())
    }

    /// Downloaded and verified. Cheap: a `stat` and a small read.
    pub fn python_ready(&self) -> bool {
        self.tool_ready(self.python.as_ref())
    }

    pub fn uv_ready(&self) -> bool {
        self.tool_ready(self.uv.as_ref())
    }

    pub fn python_status(&self) -> Option<AssetStatus> {
        self.python_assets().and_then(|a| a.status().into_iter().next())
    }

    pub fn uv_status(&self) -> Option<AssetStatus> {
        self.uv_assets().and_then(|a| a.status().into_iter().next())
    }

    /// Fetch what is asked for and not there yet. Blocks: run it on a
    /// worker. `progress` counts bytes over both downloads.
    pub fn download(
        &self,
        python: bool,
        uv: bool,
        http: &dyn Http,
        cancel: &CancelToken,
        progress: &dyn Fn(&Progress),
    ) -> Result<()> {
        let unsupported = || {
            Error::Config(format!(
                "Glossa has no managed Python for {}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        };
        let mut wanted = Vec::new();
        if python {
            wanted.push(self.python.as_ref().ok_or_else(unsupported)?);
        }
        if uv {
            wanted.push(self.uv.as_ref().ok_or_else(unsupported)?);
        }
        let wanted: Vec<&Tool> = wanted.into_iter().filter(|t| !self.tool_ready(Some(t))).collect();
        let total: u64 = wanted.iter().map(|t| t.spec.size).sum();
        let mut base = 0;
        for tool in wanted {
            let assets = self.assets(tool);
            assets.download(http, cancel, &|p| {
                progress(&Progress { asset: tool.folder.clone(), done: base + p.done, total })
            })?;
            if !self.exe(tool).is_file() {
                return Err(Error::Module {
                    module: tool.folder.clone(),
                    detail: format!("the download has no {}", tool.exe.display()),
                });
            }
            base += tool.spec.size;
        }
        Ok(())
    }

    /// Delete the managed Python, `uv` and every module's environment.
    /// `uv`'s cache is in the cache folder and goes with it.
    pub fn remove_all(&self) -> Result<()> {
        for tool in [&self.python, &self.uv].into_iter().flatten() {
            self.assets(tool).remove()?;
        }
        // Older versions left behind by an update, and the environments.
        for dir in [&self.dirs.runtimes, &self.dirs.venvs, &self.dirs.uv_cache] {
            remove_dir(dir)?;
        }
        Ok(())
    }
}

fn remove_dir(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

// -- the setting --------------------------------------------------------------------

/// `[modules] python`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonChoice {
    /// A system Python 3.8+ for a module without dependencies, else the
    /// managed one.
    Auto,
    /// Always a system Python.
    System,
    /// Always the managed Python.
    Managed,
    /// This interpreter: a path, or a command line such as `py -3`.
    Path(String),
}

impl PythonChoice {
    pub fn parse(s: &str) -> Self {
        let s = s.trim().trim_matches('"');
        match s.to_ascii_lowercase().as_str() {
            "" | "auto" => Self::Auto,
            "system" => Self::System,
            "managed" => Self::Managed,
            _ => Self::Path(s.to_owned()),
        }
    }
}

// -- a system Python -----------------------------------------------------------------

/// A configured interpreter: a path to an executable, or a command line
/// such as `py -3`.
pub fn parse_configured(configured: &str) -> Option<Interpreter> {
    let configured = configured.trim().trim_matches('"');
    if configured.is_empty() {
        return None;
    }
    if Path::new(configured).exists() {
        return Some((configured.into(), Vec::new()));
    }
    let mut words = configured.split_whitespace();
    let program = words.next()?;
    Some((program.into(), words.map(str::to_owned).collect()))
}

/// A program and the arguments it needs before the script (`py -3`).
pub type Interpreter = (OsString, Vec<String>);

/// What the search for a system Python found, once it ran.
static FOUND: Mutex<Option<Option<Interpreter>>> = Mutex::new(None);

/// A Python 3.8 or newer on this system, searched for once per process.
/// Runs programs the first time: call it from a worker.
pub fn system_python() -> Option<Interpreter> {
    if let Some(found) = FOUND.lock().clone() {
        return found;
    }
    let found = probe_python();
    *FOUND.lock() = Some(found.clone());
    found
}

/// What [`system_python`] found, without searching: `None` when the search
/// has not run yet.
pub fn system_python_if_known() -> Option<Option<Interpreter>> {
    FOUND.lock().clone()
}

/// Ask each candidate for its version and real executable. Going by the
/// executable it reports skips launchers (`py`) and the Windows Store's
/// `python.exe` alias, which opens the Store instead of running anything.
fn probe_python() -> Option<Interpreter> {
    let mut candidates: Vec<(&str, Vec<String>)> = vec![("python3", vec![]), ("python", vec![])];
    if cfg!(windows) {
        candidates.push(("py", vec!["-3".into()]));
        // Version managers (pyenv-win, conda's activation scripts) put
        // `python.bat` / `python.cmd` shims on PATH, which CreateProcess does
        // not run. cmd resolves them; the probe then reports the real
        // python.exe, so modules themselves start without cmd.
        candidates.push(("cmd", vec!["/d".into(), "/c".into(), "python3".into()]));
        candidates.push(("cmd", vec!["/d".into(), "/c".into(), "python".into()]));
    }
    for (program, args) in candidates {
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .args(["-c", "import sys; print(sys.version_info[0]); print(sys.version_info[1]); print(sys.executable)"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::child::hide_window(&mut cmd);
        let Ok(mut child) = cmd.spawn() else { continue };
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        if !status.is_some_and(|s| s.success()) {
            continue;
        }
        let mut out = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_string(&mut out);
        }
        let mut lines = out.lines();
        if lines.next().map(str::trim) != Some("3") {
            continue;
        }
        // The SDK needs 3.8; an older Python 3 is as good as none.
        if !lines.next().and_then(|m| m.trim().parse::<u32>().ok()).is_some_and(|m| m >= 8) {
            continue;
        }
        let exe = lines.next().map(str::trim).unwrap_or("");
        return Some(if !exe.is_empty() && Path::new(exe).is_file() {
            (exe.into(), Vec::new())
        } else {
            (program.into(), args)
        });
    }
    None
}

// -- requirements -------------------------------------------------------------------

/// Where a module's requirements were written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `requirements = [...]` in `module.toml`.
    Inline,
    /// A `requirements.txt` beside it.
    File,
    /// `[project] dependencies` in a `pyproject.toml` beside it.
    Project,
}

/// What a module declares it needs installed: package requirements only,
/// each checked by [`check_requirement`], whichever file they came from.
/// Glossa writes them to a file of its own for `uv`, so nothing in the
/// module's files (`-r`, `--index-url`, `[tool.uv.sources]`) reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirements {
    pub source: Source,
    pub lines: Vec<String>,
    /// The package index, `https://pypi.org/simple` unless the module names
    /// another with `python_index` (whose host it must declare in
    /// `[permissions] network`).
    pub index: String,
}

/// PyPI's simple index, where packages come from unless a module names
/// another.
pub const DEFAULT_INDEX: &str = "https://pypi.org/simple";

/// Check one requirement: a package from the index by name, with optional
/// extras, version specifiers and environment markers
/// (`"requests[socks]>=2.31; python_version >= '3.9'"`). Refused: pip
/// options (`-r`, `--index-url`, `-e`, `--hash`), direct references
/// (`pkg @ https://…`, `pkg @ file:///…`), URLs and paths, which would
/// fetch or build code from somewhere other than the index.
pub fn check_requirement(r: &str) -> std::result::Result<(), String> {
    let bad = |why: &str| {
        Err(format!("requirement {r:?} {why}; give a package name and version, such as \"requests>=2.31\""))
    };
    if r.contains(['\n', '\r', '\0']) {
        return bad("spans several lines");
    }
    let t = r.trim();
    if t.is_empty() {
        return bad("is empty");
    }
    if t.starts_with('-') {
        return bad("is a pip option, which modules cannot pass");
    }
    // Markers after `;` are evaluated, never fetched.
    let spec = t.split_once(';').map_or(t, |(s, _)| s).trim();
    let name_end =
        spec.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))).unwrap_or(spec.len());
    let (name, rest) = spec.split_at(name_end);
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return bad("does not start with a package name (paths and URLs are not allowed)");
    }
    let rest = rest.trim_start();
    let rest = match rest.strip_prefix('[') {
        Some(extras) => {
            let Some((inside, after)) = extras.split_once(']') else { return bad("has an unclosed [extras]") };
            if !inside.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ',' | ' ')) {
                return bad("has odd extras");
            }
            after
        }
        None => rest,
    };
    // Version specifiers only: no `@` (a direct reference), no `/` or `:`
    // (a URL or a path).
    if !rest.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '<' | '>' | '=' | '!' | '~' | '*' | '.' | ',' | '+' | '(' | ')' | ' ' | '\t')
    }) {
        return bad("is not a plain version requirement (direct URLs and paths are not allowed)");
    }
    Ok(())
}

/// The requirements in a `requirements.txt`: comments and blank lines
/// dropped, `\` continuations joined, every line checked.
fn parse_requirements_txt(text: &str, file: &Path) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut pending = String::new();
    for raw in text.lines() {
        // A comment starts at `#` at the start of a line or after a space.
        let line = match raw.find(" #").map(|i| i + 1).or_else(|| raw.trim_start().starts_with('#').then_some(0)) {
            Some(i) => &raw[..i],
            None => raw,
        };
        let line = line.trim_end();
        if let Some(joined) = line.strip_suffix('\\') {
            pending.push_str(joined.trim());
            pending.push(' ');
            continue;
        }
        pending.push_str(line.trim_start());
        let req = std::mem::take(&mut pending).trim().to_owned();
        if req.is_empty() {
            continue;
        }
        check_requirement(&req).map_err(|e| format!("{}: {e}", file.display()))?;
        out.push(req);
    }
    Ok(out)
}

impl Requirements {
    /// What the module in `dir` declares, if anything. A requirement that
    /// [`check_requirement`] refuses fails the module's loading.
    pub fn find(dir: &Path, manifest: &Manifest) -> std::result::Result<Option<Self>, String> {
        let index = match manifest.module.python_index.trim() {
            "" => DEFAULT_INDEX.to_owned(),
            i => i.to_owned(),
        };
        let make =
            |source, lines: Vec<String>| (!lines.is_empty()).then(|| Self { source, lines, index: index.clone() });
        if !manifest.module.requirements.is_empty() {
            for r in &manifest.module.requirements {
                check_requirement(r)?;
            }
            let lines = manifest.module.requirements.iter().map(|r| r.trim().to_owned()).collect();
            return Ok(make(Source::Inline, lines));
        }
        let txt = dir.join("requirements.txt");
        if let Ok(text) = std::fs::read_to_string(&txt) {
            return Ok(make(Source::File, parse_requirements_txt(&text, &txt)?));
        }
        let project = dir.join("pyproject.toml");
        if let Ok(text) = std::fs::read_to_string(&project) {
            let doc: toml::Table = text.parse().map_err(|e| format!("{}: {e}", project.display()))?;
            let deps = doc.get("project").and_then(|p| p.get("dependencies")).and_then(|d| d.as_array());
            let mut lines = Vec::new();
            for d in deps.into_iter().flatten() {
                let r = d.as_str().ok_or_else(|| format!("{}: a dependency is not a string", project.display()))?;
                check_requirement(r).map_err(|e| format!("{}: {e}", project.display()))?;
                lines.push(r.trim().to_owned());
            }
            return Ok(make(Source::Project, lines));
        }
        Ok(None)
    }

    /// For people: the packages.
    pub fn describe(&self) -> String {
        self.lines.join(", ")
    }

    /// The bytes the environment's hash covers: exactly what is installed
    /// and from where.
    fn fingerprint(&self) -> Vec<u8> {
        format!("index {}\n{}", self.index, self.lines.join("\n")).into_bytes()
    }
}

// -- one module's environment -------------------------------------------------------

/// The interpreter a module's environment is built on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Base {
    /// A Python found on the system: the executable and any arguments it
    /// needs (`py -3`).
    System(OsString, Vec<String>),
    /// The managed Python.
    Managed(PathBuf),
    /// The one the settings name.
    Configured(OsString, Vec<String>),
}

impl Base {
    fn program(&self) -> Interpreter {
        match self {
            Base::System(p, a) | Base::Configured(p, a) => (p.clone(), a.clone()),
            Base::Managed(p) => (p.clone().into_os_string(), Vec::new()),
        }
    }

    pub fn is_managed(&self) -> bool {
        matches!(self, Base::Managed(_))
    }

    /// For people and logs.
    pub fn describe(&self) -> String {
        let (p, a) = self.program();
        let mut s = PathBuf::from(p).display().to_string();
        for w in a {
            s.push(' ');
            s.push_str(&w);
        }
        s
    }
}

/// Where a module's Python stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvState {
    /// Everything is there.
    Ready,
    /// These must be downloaded or installed first (for people:
    /// "Python 3.13.18", "uv 0.13.7", "packages: requests").
    Missing(Vec<String>),
    /// Whether a system Python is there is not known yet (finding out runs
    /// a program, so it waits for a worker).
    Unknown,
    /// Nothing can make it work as set up (no system Python with
    /// `python = "system"`, no managed build for this system).
    Impossible(String),
}

/// A Python module's interpreter and, when it has dependencies, its
/// environment.
pub struct PythonEnv {
    pub module: String,
    dir: PathBuf,
    choice: PythonChoice,
    pub requirements: Option<Requirements>,
    pub toolchain: Arc<Toolchain>,
    /// Set once everything was found ready, so later calls skip the checks.
    ready: AtomicBool,
}

impl PythonEnv {
    pub fn new(
        dir: &Path,
        manifest: &Manifest,
        choice: PythonChoice,
        toolchain: Arc<Toolchain>,
    ) -> std::result::Result<Self, String> {
        let id = &manifest.module.id;
        let one_component = matches!(Path::new(id).components().collect::<Vec<_>>().as_slice(), [Component::Normal(c)] if *c == std::ffi::OsStr::new(id));
        if !one_component || id.chars().all(|c| c == '.') {
            return Err(format!("module id {id:?} cannot name a folder for its environment"));
        }
        Ok(Self {
            module: id.clone(),
            dir: dir.to_owned(),
            choice,
            requirements: Requirements::find(dir, manifest)?,
            toolchain,
            ready: AtomicBool::new(false),
        })
    }

    pub fn choice(&self) -> &PythonChoice {
        &self.choice
    }

    /// The module's own environment, when it has dependencies.
    pub fn venv_dir(&self) -> Option<PathBuf> {
        self.requirements.as_ref().map(|_| self.toolchain.dirs.venvs.join(&self.module))
    }

    fn venv_python(venv: &Path) -> PathBuf {
        if cfg!(windows) {
            venv.join("Scripts").join("python.exe")
        } else {
            venv.join("bin").join("python")
        }
    }

    /// The interpreter to build on. `system` decides whether a system Python
    /// is there: `Some(found)` after a search, `None` when the search has
    /// not run (only `auto` without dependencies and `system` need it).
    fn base_with(&self, system: Option<Option<Interpreter>>) -> std::result::Result<Option<Base>, String> {
        let managed = || {
            self.toolchain.python_exe().map(Base::Managed).ok_or_else(|| {
                format!(
                    "Glossa has no Python build for {}/{}; install Python 3.8 or newer and set [modules] python = \"system\"",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                )
            })
        };
        match &self.choice {
            PythonChoice::Managed => managed().map(Some),
            PythonChoice::Path(p) => {
                let (program, args) = parse_configured(p)
                    .ok_or_else(|| format!("[modules] python = {p:?} names no program"))?;
                Ok(Some(Base::Configured(program, args)))
            }
            PythonChoice::Auto if self.requirements.is_some() => managed().map(Some),
            PythonChoice::Auto => match system {
                None => Ok(None),
                Some(Some((p, a))) => Ok(Some(Base::System(p, a))),
                Some(None) => managed().map(Some),
            },
            PythonChoice::System => match system {
                None => Ok(None),
                Some(Some((p, a))) => Ok(Some(Base::System(p, a))),
                Some(None) => Err("no Python 3.8 or newer found on this system; install one, or set [modules] python = \"managed\" to let Glossa download it".into()),
            },
        }
    }

    /// The interpreter to build on, searching for a system Python when the
    /// setting needs one. May run a program: call it from a worker.
    pub fn base(&self) -> std::result::Result<Base, String> {
        let system = system_python();
        Ok(self.base_with(Some(system))?.expect("the search ran"))
    }

    /// The hash an up-to-date environment carries.
    fn venv_hash(&self, base: &Base, req: &Requirements) -> Result<String> {
        let mut h = Sha256::new();
        h.update(b"glossa-venv 1\n");
        h.update(base.describe().as_bytes());
        h.update(format!("\nuv {UV_VERSION}\n").as_bytes());
        h.update(req.fingerprint());
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    fn venv_current(&self, base: &Base) -> bool {
        let (Some(req), Some(venv)) = (&self.requirements, self.venv_dir()) else { return true };
        let Ok(want) = self.venv_hash(base, req) else { return false };
        Self::venv_python(&venv).is_file()
            && std::fs::read_to_string(venv.join(VENV_MARKER)).is_ok_and(|have| have.trim() == want)
    }

    fn state_with(&self, system: Option<Option<Interpreter>>) -> EnvState {
        if self.ready.load(Ordering::Relaxed) {
            return EnvState::Ready;
        }
        let base = match self.base_with(system) {
            Err(why) => return EnvState::Impossible(why),
            Ok(None) => return EnvState::Unknown,
            Ok(Some(b)) => b,
        };
        let mut missing = Vec::new();
        if base.is_managed() && !self.toolchain.python_ready() {
            missing.push(format!("Python {PYTHON_VERSION}"));
        }
        if let Some(req) = &self.requirements {
            if !self.toolchain.uv_ready() {
                missing.push(format!("uv {UV_VERSION}"));
            }
            if !self.venv_current(&base) {
                missing.push(format!("packages ({})", req.describe()));
            }
        }
        if missing.is_empty() {
            self.ready.store(true, Ordering::Relaxed);
            EnvState::Ready
        } else {
            EnvState::Missing(missing)
        }
    }

    /// Where things stand, searching for a system Python if need be. May run
    /// a program: call it from a worker.
    pub fn state(&self) -> EnvState {
        self.state_with(Some(system_python()))
    }

    /// Where things stand without running anything: [`EnvState::Unknown`]
    /// when that would need a search for a system Python that has not run.
    pub fn state_now(&self) -> EnvState {
        self.state_with(system_python_if_known())
    }

    /// Bytes still to download for the managed Python and `uv` (packages
    /// not counted: their size is not known in advance).
    pub fn download_size(&self) -> u64 {
        let Ok(Some(base)) = self.base_with(system_python_if_known()) else { return 0 };
        let mut size = 0;
        if base.is_managed() && !self.toolchain.python_ready() {
            size += self.toolchain.python.as_ref().map_or(0, |t| t.spec.size);
        }
        if self.requirements.is_some() && !self.toolchain.uv_ready() {
            size += self.toolchain.uv.as_ref().map_or(0, |t| t.spec.size);
        }
        size
    }

    /// Make everything the module needs: download the managed Python and
    /// `uv` if it uses them, then make its environment. Blocks for as long
    /// as that takes: call it from a worker.
    pub fn install(&self, http: &dyn Http, cancel: &CancelToken, progress: &dyn Fn(&Progress)) -> Result<()> {
        let base = self.base().map_err(|e| Error::module(&self.module, e))?;
        self.toolchain.download(base.is_managed(), self.requirements.is_some(), http, cancel, progress)?;
        if let Some(req) = &self.requirements {
            progress(&Progress { asset: "packages".into(), done: 0, total: 0 });
            self.make_venv(&base, req, cancel)?;
        }
        self.ready.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// `uv venv` and `uv pip install`, with no shell, into the module's own
    /// folder below the environments folder.
    fn make_venv(&self, base: &Base, req: &Requirements, cancel: &CancelToken) -> Result<()> {
        let venvs = &self.toolchain.dirs.venvs;
        let _busy = assets::lock_folder(venvs, &self.module)?;
        self.ready.store(false, Ordering::Relaxed);
        if self.venv_current(base) {
            return Ok(());
        }
        let venv = venvs.join(&self.module);
        // `new` made sure the id is one plain folder name.
        debug_assert!(venv.parent() == Some(venvs.as_path()));
        remove_dir(&venv)?;
        let uv = self
            .toolchain
            .uv_exe()
            .filter(|p| p.is_file())
            .ok_or_else(|| Error::RuntimeMissing { module: self.module.clone(), what: format!("uv {UV_VERSION}") })?;
        let (program, args) = base.program();
        if !args.is_empty() {
            return Err(Error::module(
                &self.module,
                format!(
                    "a module with dependencies needs a Python given by its path, not the command line {:?}",
                    base.describe()
                ),
            ));
        }
        tracing::info!(module = %self.module, python = %base.describe(), "making the module's Python environment");
        let started = std::time::Instant::now();
        let mut venv_cmd = self.uv_command(&uv);
        venv_cmd.arg("venv").arg("--python").arg(&program).arg(&venv);
        self.run_uv(venv_cmd, cancel)?;

        // Only the checked lines, from a file of Glossa's own, from one index,
        // ignoring `[tool.uv.sources]`, as wheels.
        let file = venv.join("glossa-requirements.txt");
        std::fs::write(&file, req.lines.join("\n") + "\n")?;
        let mut install = self.uv_command(&uv);
        install
            .arg("pip")
            .arg("install")
            .arg("--python")
            .arg(Self::venv_python(&venv))
            .arg("--index-url")
            .arg(&req.index)
            .arg("--no-sources")
            // Wheels only: building a source distribution runs its build
            // script on the user's machine, which may happen unasked on a
            // first call with automatic downloads.
            .arg("--no-build")
            .arg("-r")
            .arg(file);
        if let Err(e) = self.run_uv(install, cancel) {
            let _ = std::fs::remove_dir_all(&venv);
            return Err(e);
        }
        std::fs::write(venv.join(VENV_MARKER), self.venv_hash(base, req)?)?;
        tracing::info!(module = %self.module, secs = started.elapsed().as_secs_f32(), "the module's Python environment is ready");
        Ok(())
    }

    fn uv_command(&self, uv: &Path) -> Command {
        let mut cmd = Command::new(uv);
        // No user or project configuration, no Python downloads of uv's own
        // (Glossa pins the interpreter), the cache in Glossa's cache folder.
        cmd.args(["--no-config", "--no-python-downloads", "--quiet", "--color", "never"]).current_dir(&self.dir);
        // A cleared environment: `UV_INDEX_URL`, `PIP_FIND_LINKS`,
        // `UV_INSECURE_HOST` and the like in the user's shell must not steer
        // where packages come from. Only what finding programs, the home and
        // temporary folders, and certificates need is kept.
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            if KEPT_ENV.iter().any(|keep| k.to_str().is_some_and(|k| k.eq_ignore_ascii_case(keep))) {
                cmd.env(k, v);
            }
        }
        cmd.env("UV_CACHE_DIR", &self.toolchain.dirs.uv_cache).env("UV_NO_PROGRESS", "1");
        // Glossa's own proxy setting, as the proxy variables uv reads.
        if let Some(proxy) = &self.toolchain.dirs.proxy {
            for var in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
                cmd.env(var, proxy);
            }
            cmd.env("NO_PROXY", "localhost,127.0.0.1,::1");
        }
        cmd
    }

    fn run_uv(&self, cmd: Command, cancel: &CancelToken) -> Result<()> {
        let out = crate::child::run(cmd, INSTALL_BUDGET, Some(cancel))?;
        if out.success() {
            Ok(())
        } else {
            Err(Error::module(&self.module, format!("uv failed: {}", out.stderr_tail())))
        }
    }

    /// The program to start the module with: its environment's interpreter
    /// when it has dependencies, else the base one. Fails politely when
    /// something is missing (the gate normally catches that first).
    pub fn interpreter(&self) -> std::result::Result<Interpreter, String> {
        let base = self.base()?;
        if let Some(venv) = self.venv_dir() {
            let python = Self::venv_python(&venv);
            if !python.is_file() {
                return Err(format!("the module's Python environment is not made yet ({})", venv.display()));
            }
            return Ok((python.into_os_string(), Vec::new()));
        }
        if let Base::Managed(p) = &base {
            if !p.is_file() {
                return Err(format!("the managed Python {PYTHON_VERSION} is not downloaded yet"));
            }
        }
        Ok(base.program())
    }

    /// Delete the module's environment (the shared Python stays).
    pub fn remove_venv(&self) -> Result<()> {
        let Some(venv) = self.venv_dir() else { return Ok(()) };
        let _busy = assets::lock_folder(&self.toolchain.dirs.venvs, &self.module)?;
        self.ready.store(false, Ordering::Relaxed);
        remove_dir(&venv)
    }

    /// Whether an install for this module runs in this process now.
    pub fn is_busy(&self) -> bool {
        assets::folder_busy(&self.toolchain.dirs.venvs, &self.module)
            || self.toolchain.python_assets().is_some_and(|a| a.is_busy())
            || self.toolchain.uv_assets().is_some_and(|a| a.is_busy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;
    use std::io::Write;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glossa-pyenv-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }

        fn dirs(&self) -> PythonDirs {
            PythonDirs {
                runtimes: self.0.join("runtimes"),
                venvs: self.0.join("venvs"),
                uv_cache: self.0.join("uv"),
                proxy: None,
            }
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(extra: &str) -> Manifest {
        Manifest::parse(&format!(
            "capabilities = [\"translate\"]\n[module]\nid = \"py-mod\"\nname = \"M\"\nruntime = \"python\"\n{extra}"
        ))
        .unwrap()
    }

    /// A gzipped tar holding `files` (path, contents, mode).
    fn tar_gz(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, body, mode) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(format!("{mode:07o}").as_bytes());
            h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
            h[156] = b'0';
            h[257..262].copy_from_slice(b"ustar");
            h[148..156].fill(b' ');
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            tar.extend_from_slice(&h);
            tar.extend_from_slice(body);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn tool(folder: &str, url: &str, archive: &[u8], exe: &str) -> Tool {
        Tool {
            folder: folder.into(),
            spec: spec("test-target", url.into(), &sha(archive), archive.len() as u64, Unpack::TarGz),
            exe: PathBuf::from(exe),
        }
    }

    #[test]
    fn every_pinned_build_is_well_formed() {
        let tmp = Temp::new("pins");
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let t = Toolchain::for_platform(tmp.dirs(), os, arch);
            assert!(t.supported(), "{os}/{arch}");
            for tool in [t.python_tool().unwrap(), t.uv_tool().unwrap()] {
                tool.spec.validate().unwrap();
                let url = tool.spec.url.list()[0];
                assert!(url.starts_with("https://github.com/astral-sh/"), "{url}");
                assert!(url.contains(&tool.spec.name), "{url} is for {}", tool.spec.name);
            }
            let py = t.python_exe().unwrap();
            assert!(py.starts_with(tmp.0.join("runtimes").join(format!("python-{PYTHON_VERSION}"))), "{py:?}");
        }
        let shas: std::collections::HashSet<_> = PINS.iter().flat_map(|p| [p.python_sha256, p.uv_sha256]).collect();
        assert_eq!(shas.len(), PINS.len() * 2, "every pinned file has its own hash");
        assert!(!Toolchain::for_platform(tmp.dirs(), "freebsd", "x86_64").supported());
    }

    #[test]
    fn the_setting_parses() {
        assert_eq!(PythonChoice::parse(""), PythonChoice::Auto);
        assert_eq!(PythonChoice::parse(" Auto "), PythonChoice::Auto);
        assert_eq!(PythonChoice::parse("system"), PythonChoice::System);
        assert_eq!(PythonChoice::parse("managed"), PythonChoice::Managed);
        assert_eq!(PythonChoice::parse("/usr/bin/python3"), PythonChoice::Path("/usr/bin/python3".into()));
        assert_eq!(PythonChoice::parse("py -3"), PythonChoice::Path("py -3".into()));
    }

    #[test]
    fn requirements_come_from_the_manifest_or_a_file_beside_it() {
        let tmp = Temp::new("req");
        let dir = &tmp.0;
        let find = |extra: &str| Requirements::find(dir, &manifest(extra));
        assert_eq!(find("").unwrap(), None);
        let r = find("requirements = [\"requests>=2\"]").unwrap().unwrap();
        assert_eq!(
            (r.source, r.lines.clone(), r.index.as_str()),
            (Source::Inline, vec!["requests>=2".into()], DEFAULT_INDEX)
        );

        std::fs::write(dir.join("pyproject.toml"), "[project]\nname = \"x\"\ndependencies = []\n").unwrap();
        assert_eq!(find("").unwrap(), None, "no dependencies, no environment");
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\ndependencies = [\"idna\"]\n[tool.uv.sources]\nidna = { url = \"https://evil.test/x.whl\" }\n",
        )
        .unwrap();
        let r = find("").unwrap().unwrap();
        assert_eq!((r.source, r.lines), (Source::Project, vec!["idna".to_owned()]), "only the dependencies are read");
        std::fs::write(dir.join("pyproject.toml"), "[project]\ndependencies = [\"x @ https://evil.test/x.whl\"]\n")
            .unwrap();
        assert!(find("").is_err());

        std::fs::write(dir.join("requirements.txt"), "# nothing\n\n").unwrap();
        assert_eq!(find("").unwrap(), None);
        std::fs::write(
            dir.join("requirements.txt"),
            "idna==3.10  # pinned\nrequests[socks] \\\n  >=2.31 ; python_version >= '3.8'\n",
        )
        .unwrap();
        let r = find("").unwrap().unwrap();
        assert_eq!(r.source, Source::File);
        assert_eq!(r.lines, ["idna==3.10", "requests[socks] >=2.31 ; python_version >= '3.8'"]);
        for bad in ["--index-url https://evil.test/simple", "-r ../other.txt", "-e .", "x @ file:///etc"] {
            std::fs::write(dir.join("requirements.txt"), format!("idna\n{bad}\n")).unwrap();
            assert!(find("").is_err(), "{bad:?}");
        }

        for good in
            ["requests", "requests>=2.31", "a.b-c_d[x,y]~=1.0", "numpy==2.*; sys_platform == 'darwin'", "x (>=1)"]
        {
            assert!(check_requirement(good).is_ok(), "{good:?}");
        }
        for bad in [
            "",
            "--index-url=https://evil.test",
            "-r other.txt",
            "a\nb",
            "pkg @ https://evil.test/pkg.whl",
            "pkg@file:///tmp/pkg",
            "./local",
            "../up",
            "/abs/path",
            "https://evil.test/pkg.tar.gz",
            "C:\\pkg",
            "pkg --hash=sha256:00",
        ] {
            assert!(check_requirement(bad).is_err(), "{bad:?}");
        }
        let parse = |extra: &str| {
            Manifest::parse(&format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"python\"\n{extra}\n[permissions]\nnetwork = [\"pypi.example.org\"]"
            ))
        };
        assert!(parse("requirements = [\"-e .\"]").unwrap_err().to_string().contains("pip option"));
        let err = Manifest::parse(
            "capabilities = [\"translate\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"js\"\nrequirements = [\"x\"]",
        )
        .unwrap_err();
        assert!(err.to_string().contains("python"), "{err}");

        // Another index only from a host the module declares.
        let m = parse("requirements = [\"x\"]\npython_index = \"https://pypi.example.org/simple\"").unwrap();
        assert_eq!(Requirements::find(dir, &m).unwrap().unwrap().index, "https://pypi.example.org/simple");
        assert!(parse("python_index = \"https://elsewhere.test/simple\"")
            .unwrap_err()
            .to_string()
            .contains("permissions"));
        assert!(parse("python_index = \"http://pypi.example.org/simple\"").unwrap_err().to_string().contains("https"));
    }

    #[test]
    fn uv_runs_with_a_cleared_environment() {
        let tmp = Temp::new("uvenv");
        let mut dirs = tmp.dirs();
        dirs.proxy = Some("socks5://127.0.0.1:9050".into());
        let tc = Arc::new(Toolchain::new(dirs));
        let env = PythonEnv::new(&tmp.0, &manifest("requirements = [\"idna\"]"), PythonChoice::Managed, tc).unwrap();
        let cmd = env.uv_command(Path::new("uv"));
        let vars: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
            .collect();
        assert_eq!(vars.get("UV_CACHE_DIR").cloned().flatten(), Some(tmp.0.join("uv").to_string_lossy().into_owned()));
        assert_eq!(vars.get("HTTPS_PROXY").cloned().flatten().as_deref(), Some("socks5://127.0.0.1:9050"));
        for k in vars.keys() {
            let k = k.to_ascii_uppercase();
            assert!(
                !(k.starts_with("PIP_") || (k.starts_with("UV_") && k != "UV_CACHE_DIR" && k != "UV_NO_PROGRESS")),
                "{k} leaks into uv"
            );
            assert!(
                KEPT_ENV.contains(&k.as_str())
                    || ["UV_CACHE_DIR", "UV_NO_PROGRESS", "HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"]
                        .contains(&k.as_str()),
                "{k}"
            );
        }
    }

    #[test]
    fn the_interpreter_follows_the_setting() {
        let tmp = Temp::new("choice");
        let tc = Arc::new(Toolchain::for_platform(tmp.dirs(), "macos", "aarch64"));
        let managed = Base::Managed(tc.python_exe().unwrap());
        let sys = Some(Some((OsString::from("/usr/bin/python3"), Vec::new())));
        let env = |choice, extra: &str| PythonEnv::new(&tmp.0, &manifest(extra), choice, tc.clone()).unwrap();

        let plain = env(PythonChoice::Auto, "");
        assert_eq!(plain.base_with(sys.clone()).unwrap(), Some(Base::System("/usr/bin/python3".into(), vec![])));
        assert_eq!(plain.base_with(Some(None)).unwrap(), Some(managed.clone()), "no system Python: the managed one");
        assert_eq!(plain.base_with(None).unwrap(), None, "unknown until searched");
        assert_eq!(plain.state_with(None), EnvState::Unknown);

        let deps = env(PythonChoice::Auto, "requirements = [\"idna\"]");
        assert_eq!(deps.base_with(sys.clone()).unwrap(), Some(managed.clone()), "dependencies: always managed");
        assert_eq!(deps.venv_dir(), Some(tmp.0.join("venvs").join("py-mod")));
        match deps.state_with(None) {
            EnvState::Missing(m) => {
                assert_eq!(m.len(), 3, "{m:?}");
                assert!(m[2].contains("idna"), "{m:?}");
            }
            other => panic!("{other:?}"),
        }

        assert!(env(PythonChoice::System, "").base_with(Some(None)).is_err());
        assert_eq!(env(PythonChoice::Managed, "").base_with(sys.clone()).unwrap(), Some(managed));
        assert_eq!(
            env(PythonChoice::Path("py -3".into()), "").base_with(None).unwrap(),
            Some(Base::Configured("py".into(), vec!["-3".into()]))
        );

        let none = Arc::new(Toolchain::for_platform(tmp.dirs(), "freebsd", "x86_64"));
        let e = PythonEnv::new(&tmp.0, &manifest("requirements = [\"idna\"]"), PythonChoice::Auto, none).unwrap();
        assert!(matches!(e.state_with(None), EnvState::Impossible(_)));
    }

    #[test]
    fn the_managed_python_and_uv_are_downloaded_verified_and_removed() {
        let tmp = Temp::new("download");
        let py = tar_gz(&[("python/bin/python3.13", b"#!/bin/sh\n", 0o755), ("python/lib/os.py", b"x", 0o644)]);
        let uv = tar_gz(&[("uv-test/uv", b"#!/bin/sh\n", 0o755)]);
        let tc = Toolchain::with_tools(
            tmp.dirs(),
            tool("python-9.9", "https://dl.test/py.tgz", &py, "python/bin/python3.13"),
            tool("uv-9.9", "https://dl.test/uv.tgz", &uv, "uv-test/uv"),
        );
        assert!(!tc.python_ready() && !tc.uv_ready());
        let http = MockHttp::new().on_bytes(Method::Get, "https://dl.test/py.tgz", 200, &py).on_bytes(
            Method::Get,
            "https://dl.test/uv.tgz",
            200,
            &uv,
        );
        let seen = parking_lot::Mutex::new(Vec::new());
        tc.download(true, true, &http, &CancelToken::new(), &|p| seen.lock().push(p.clone())).unwrap();
        assert!(tc.python_ready() && tc.uv_ready());
        let exe = tc.python_exe().unwrap();
        assert_eq!(exe, tmp.0.join("runtimes/python-9.9/test-target/python/bin/python3.13"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111, 0, "executable");
        }
        let last = seen.lock().last().cloned().unwrap();
        assert_eq!((last.done, last.total), ((py.len() + uv.len()) as u64, (py.len() + uv.len()) as u64));
        // A second call has nothing to fetch.
        tc.download(true, true, &MockHttp::new(), &CancelToken::new(), &|_| {}).unwrap();

        tc.remove_all().unwrap();
        assert!(!tmp.0.join("runtimes").exists() && !tc.python_ready());
    }

    #[test]
    fn a_tampered_python_download_is_refused() {
        let tmp = Temp::new("tampered");
        let py = tar_gz(&[("python/bin/python3.13", b"#!/bin/sh\n", 0o755)]);
        let tc = Toolchain::with_tools(
            tmp.dirs(),
            tool("python-9.9", "https://dl.test/py.tgz", &py, "python/bin/python3.13"),
            tool("uv-9.9", "https://dl.test/uv.tgz", &py, "uv"),
        );
        let mut evil = py.clone();
        let n = evil.len();
        evil[n - 9] ^= 0xff;
        let http = MockHttp::new().on_bytes(Method::Get, "https://dl.test/py.tgz", 200, &evil);
        let err = tc.download(true, false, &http, &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(matches!(err, Error::AssetCorrupt { .. }), "{err:?}");
        assert!(!tc.python_ready());
        assert!(!tc.python_exe().unwrap().exists());
    }

    #[test]
    fn a_module_id_cannot_point_its_environment_elsewhere() {
        let tmp = Temp::new("id");
        let tc = Arc::new(Toolchain::new(tmp.dirs()));
        let mut m = manifest("requirements = [\"idna\"]");
        for bad in ["..", "a/b", "/abs"] {
            m.module.id = bad.into();
            assert!(PythonEnv::new(&tmp.0, &m, PythonChoice::Auto, tc.clone()).is_err(), "{bad}");
        }
    }

    /// The real thing: downloads the pinned Python and uv (about 40 MB) and
    /// installs a small package. `cargo test -p glossa-modules live_managed -- --ignored`
    #[test]
    #[ignore]
    fn live_managed_python_with_a_dependency() {
        let tmp = Temp::new("live");
        let dir = tmp.0.join("module");
        std::fs::create_dir_all(&dir).unwrap();
        let tc = Arc::new(Toolchain::new(tmp.dirs()));
        let env =
            PythonEnv::new(&dir, &manifest("requirements = [\"idna==3.10\"]"), PythonChoice::Auto, tc.clone()).unwrap();
        let http = glossa_net::HttpClient::plain();
        env.install(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(env.state(), EnvState::Ready);
        let (python, _) = env.interpreter().unwrap();
        let out = Command::new(python)
            .args(["-c", "import idna, sys; print(idna.encode('bücher.example').decode(), sys.version_info[:2])"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("xn--bcher-kva.example"), "{text} {}", String::from_utf8_lossy(&out.stderr));
    }
}
