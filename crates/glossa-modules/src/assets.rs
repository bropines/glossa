//! Files a module needs besides its own folder: ML models, dictionaries,
//! helper binaries.
//!
//! A module lists them in `module.toml`:
//!
//! ```toml
//! [[assets]]
//! name = "det-model"                  # file (or folder, when unpacked) name
//! url = ["https://a.example/det.onnx", "https://mirror.example/det.onnx"]
//! sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
//! size = 4718592                      # bytes, of the download
//! unpack = "none"                     # none | zip | tar.gz
//! os = ["windows"]                    # optional: windows, macos, linux, unix
//! arch = ["x86_64"]                   # optional: x86_64, aarch64, …
//! ```
//!
//! Nothing is fetched when Glossa starts. [`ModuleAssets::download`] runs on
//! a worker thread when the user asks for it (the module's options, the
//! CLI, the control API) or on the module's first call when automatic
//! downloads are switched on. A download resumes a partial file, is checked
//! against its SHA-256 before anything can use it, and lands in
//! `<models>/<module-id>/<name>`. A marker in `.glossa/` beside it records
//! the checksum it was verified against, so knowing an asset is ready costs a
//! `stat` and a small read, never a re-hash of a large model.
//!
//! A module whose assets are not ready is listed but not loaded: the
//! registry puts an [`AssetGate`] in its place, which starts the real runtime
//! only once the files are there.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use glossa_core::{
    Article, Audio, Detection, DictRequest, Error, OcrRequest, OcrResult, ProviderInfo, Result, SpeakRequest,
    TranslateRequest, Translation,
};
use glossa_net::{DownloadEvent, Http, HttpRequest};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::Manifest;
use crate::provider::{CancelToken, Host, OptionSpec, Provider};

/// How long one asset's transfer may take in all. Generous, because a model
/// can be large and a connection slow; the server must still start
/// answering within the network timeout.
const TRANSFER_BUDGET: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// The folder beside the assets holding partial downloads and markers.
const STATE_DIR: &str = ".glossa";

/// How many times in a row one mirror may fail without the partial file
/// growing before the next mirror gets its turn. An attempt that brought
/// more bytes starts the count over: a flaky connection that keeps making
/// headway finishes a large model however often it drops.
const ATTEMPTS_PER_MIRROR: u32 = 5;

/// The most requests one mirror gets for one file, headway or not, so a
/// server that sends a few bytes and hangs up every time cannot keep a
/// download going forever.
const MAX_REQUESTS_PER_MIRROR: u32 = 200;

/// The longest pause between two attempts, whether it comes from the
/// doubling or from the server's `Retry-After`.
const MAX_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(120);

/// The pause before retry number `attempt` (1 for the first): one second
/// doubling to thirty, scaled by a random 50-100 % so several clients do not
/// retry in step, or what the server asked for when that is longer.
fn retry_pause(attempt: u32, retry_after_secs: Option<u64>) -> std::time::Duration {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(attempt);
    let jitter = 0.5 + 0.5 * (h.finish() % 1000) as f64 / 1000.0;
    let doubled = 2f64.powi(attempt.saturating_sub(1).min(5) as i32).min(30.0);
    let own = std::time::Duration::from_secs_f64(doubled * jitter);
    let asked = std::time::Duration::from_secs(retry_after_secs.unwrap_or(0));
    own.max(asked).min(MAX_RETRY_PAUSE)
}

/// Whether asking the same mirror again could work: the network failed, the
/// server was busy or told us to slow down. A 404 or a 403 will not change.
fn worth_retrying(e: &Error) -> bool {
    match e {
        Error::Network(_) | Error::Timeout(_) | Error::RateLimited { .. } => true,
        Error::HttpStatus { status, .. } => matches!(status, 408 | 425 | 429) || (500..600).contains(status),
        _ => false,
    }
}

/// Wait `pause`, waking early when the download is cancelled.
fn cancellable_pause(pause: std::time::Duration, cancel: &CancelToken) -> Result<()> {
    let until = std::time::Instant::now() + pause;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(std::time::Duration::from_millis(100)));
    }
}

// -- the manifest side ----------------------------------------------------------

/// One `[[assets]]` entry of `module.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSpec {
    /// The file name the asset is saved under (the folder name, when it is
    /// unpacked). Letters, digits, `.`, `-` and `_`.
    pub name: String,
    /// Where to get it: one URL, or several tried in order (mirrors).
    pub url: Urls,
    /// The SHA-256 of the downloaded file, in hex.
    pub sha256: String,
    /// The size of the downloaded file in bytes.
    pub size: u64,
    #[serde(default)]
    pub unpack: Unpack,
    /// Only on these systems (`windows`, `macos`, `linux`, `unix` for every
    /// system but Windows); empty for all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    /// Only on these processors (`std::env::consts::ARCH` values:
    /// `x86_64`, `aarch64`, `x86`, `arm`…); empty for all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arch: Vec<String>,
    /// Not needed to start the module: fetched only when asked for by name
    /// (one of many language models, voices or recognisers). The module
    /// sees which are there through the `assets` host call.
    #[serde(default)]
    pub optional: bool,
    /// Make the file executable after download (Unix), for a helper program
    /// a process module's `command` names through `${ASSETS_DIR}`.
    #[serde(default)]
    pub executable: bool,
}

/// `url = "…"` or `url = ["…", "…"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Urls {
    One(String),
    Many(Vec<String>),
}

impl Urls {
    pub fn list(&self) -> Vec<&str> {
        match self {
            Urls::One(u) => vec![u.as_str()],
            Urls::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// What to do with the file once it is verified.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unpack {
    /// Keep the file as it is.
    #[default]
    #[serde(rename = "none")]
    None,
    /// Unpack a `.zip` into a folder named after the asset.
    #[serde(rename = "zip")]
    Zip,
    /// Unpack a gzipped tar into a folder named after the asset.
    #[serde(rename = "tar.gz", alias = "tgz")]
    TarGz,
    /// Decompress a single gzipped file, saved under the asset's name.
    #[serde(rename = "gz")]
    Gz,
}

impl AssetSpec {
    /// Whether this entry applies to `os` / `arch` (`std::env::consts`
    /// values). The OS names are those `command.windows` and friends use.
    pub fn applies_to(&self, os: &str, arch: &str) -> bool {
        let os_ok = self.os.is_empty()
            || self.os.iter().any(|o| {
                let o = o.trim().to_ascii_lowercase();
                o == os || (o == "unix" && os != "windows")
            });
        let arch_ok = self.arch.is_empty() || self.arch.iter().any(|a| a.trim().eq_ignore_ascii_case(arch));
        os_ok && arch_ok
    }

    /// Check one entry; the manifest calls this for each.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let n = &self.name;
        if !valid_name(n) {
            return Err(format!("asset name {n:?} must be letters, digits, '.', '-' or '_', and not start with '.'"));
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("asset {n}: sha256 must be 64 hex digits"));
        }
        // Nothing hashes to all zeros: it is a placeholder left for a file
        // that did not exist yet. Refusing it here stops a manifest from
        // shipping with one (`module check` and `module test` report it).
        if self.sha256.bytes().all(|b| b == b'0') {
            return Err(format!(
                "asset {n}: sha256 is all zeros, a placeholder; put the SHA-256 of the file at its url here"
            ));
        }
        if self.size == 0 {
            return Err(format!("asset {n}: size must be the file's size in bytes"));
        }
        let urls = self.url.list();
        if urls.is_empty() {
            return Err(format!("asset {n}: url is empty"));
        }
        for u in urls {
            if !(u.starts_with("https://") || u.starts_with("http://")) {
                return Err(format!("asset {n}: {u:?} is not an http(s) URL"));
            }
        }
        for o in &self.os {
            if !matches!(o.trim().to_ascii_lowercase().as_str(), "windows" | "macos" | "linux" | "unix") {
                return Err(format!("asset {n}: unknown os {o:?} (windows, macos, linux or unix)"));
            }
        }
        Ok(())
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with('.')
        && n.len() <= 128
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// The entries of `specs` that apply to `os` / `arch`. Two entries with the
/// same name may both apply only if one is more specific, which is a
/// manifest error, so the first one wins and the clash is reported.
pub fn select<'a>(specs: &'a [AssetSpec], os: &str, arch: &str) -> std::result::Result<Vec<&'a AssetSpec>, String> {
    let mut out: Vec<&AssetSpec> = Vec::new();
    for s in specs.iter().filter(|s| s.applies_to(os, arch)) {
        if out.iter().any(|o| o.name == s.name) {
            return Err(format!("two assets named {:?} apply to {os}/{arch}; narrow them with os or arch", s.name));
        }
        out.push(s);
    }
    Ok(out)
}

/// The environment variable naming one asset's path:
/// `GLOSSA_ASSET_DET_MODEL` for `det-model`.
pub fn env_name(asset: &str) -> String {
    let mut s = String::from("GLOSSA_ASSET_");
    s.extend(asset.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }));
    s
}

// -- state ----------------------------------------------------------------------

/// Where one asset stands on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum AssetState {
    /// Downloaded and verified.
    Ready,
    /// Not there.
    Missing,
    /// Part of it was downloaded; the next download resumes.
    Partial { have: u64 },
    /// A file is there, but not the one the manifest names now (the module
    /// was updated, or the file was changed by hand).
    Outdated,
    /// A full check found it damaged; it was deleted.
    Corrupt,
}

/// One asset and where it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetStatus {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    /// Not needed to start the module; see [`AssetSpec::optional`].
    #[serde(default)]
    pub optional: bool,
    #[serde(flatten)]
    pub state: AssetState,
}

/// What the marker beside a ready asset records.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    sha256: String,
    size: u64,
    /// For an unpacked asset: each file below its folder and its size, for
    /// [`ModuleAssets::verify`] (the archive itself is not kept).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<(String, u64)>,
}

/// How one request for a file ended, when it did not fail.
enum Transfer {
    /// The response brought the file to its full size, continuing a partial
    /// file (`resumed`) or sending all of it.
    Whole { resumed: bool },
    /// The partial file was whole already; nothing was asked for.
    AlreadyWhole,
}

/// Download progress over all of a module's assets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The asset being fetched now.
    pub asset: String,
    /// Bytes present so far, counting assets already done and resumed parts.
    pub done: u64,
    /// Bytes in all.
    pub total: u64,
}

// -- one module's assets ----------------------------------------------------------

/// One module's assets on this system.
pub struct ModuleAssets {
    pub module: String,
    /// `<models>/<module-id>`.
    pub root: PathBuf,
    /// `<models>`: nothing is ever written or deleted outside `root`, which
    /// sits directly below it.
    models_dir: PathBuf,
    pub specs: Vec<AssetSpec>,
    /// How a download waits between two attempts ([`cancellable_pause`]);
    /// the tests put a recorder here so they need not sleep.
    pause: fn(std::time::Duration, &CancelToken) -> Result<()>,
}

/// One writer at a time per module folder, across every [`ModuleAssets`]
/// made for it in this process (each reload of the modules makes new ones):
/// the UI's download, a first call's automatic one, a check and a delete
/// would otherwise touch the same files. Another process (`glossa-cli`
/// next to the desktop app) is kept out by a file lock as well; see
/// [`ModuleAssets::exclusive`].
fn folder_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: std::sync::OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>> =
        std::sync::OnceLock::new();
    LOCKS.get_or_init(Default::default).lock().entry(root.to_owned()).or_default().clone()
}

/// Held while a download, a check or a delete runs.
pub struct Exclusive {
    _file: Option<File>,
    _guard: parking_lot::ArcMutexGuard<parking_lot::RawMutex, ()>,
}

/// Take folder `name` below `parent` for writing: an in-process lock, then
/// `<parent>/.locks/<name>.lock` against another Glossa process. Blocks while
/// either is held elsewhere. Module assets, the managed Python and the
/// modules' Python environments all go through this.
pub fn lock_folder(parent: &Path, name: &str) -> io::Result<Exclusive> {
    let guard = folder_lock(&parent.join(name)).lock_arc();
    let locks = parent.join(".locks");
    let file = fs::create_dir_all(&locks).and_then(|()| {
        fs::OpenOptions::new().create(true).truncate(false).write(true).open(locks.join(format!("{name}.lock")))
    });
    let file = match file {
        Ok(f) => {
            if f.try_lock().is_err() {
                tracing::info!(folder = %name, "another Glossa is working on these files; waiting");
                f.lock()?;
            }
            Some(f)
        }
        // A read-only folder fails later with a clearer error; the
        // in-process lock still holds.
        Err(e) => {
            tracing::debug!(folder = %name, "no lock file: {e}");
            None
        }
    };
    Ok(Exclusive { _file: file, _guard: guard })
}

/// Whether folder `name` below `parent` is taken by [`lock_folder`] in this
/// process now.
pub fn folder_busy(parent: &Path, name: &str) -> bool {
    folder_lock(&parent.join(name)).try_lock().is_none()
}

/// The most an archive may unpack to: generous for compressed text
/// (dictionaries shrink tenfold or more), but bounded, so a small archive
/// cannot fill the disk.
pub fn unpack_limit(download_size: u64) -> u64 {
    const FLOOR: u64 = 64 << 20;
    const CEILING: u64 = 64 << 30;
    download_size.saturating_mul(32).clamp(FLOOR, CEILING)
}

/// The most a tar extended-header record (a long name, pax attributes) may
/// take; real ones are a few hundred bytes.
const MAX_TAR_RECORD: u64 = 1 << 20;

impl ModuleAssets {
    /// The assets of `manifest` that apply here, under `models_dir`. `None`
    /// when the module declares none for this system.
    pub fn for_manifest(manifest: &Manifest, models_dir: &Path) -> std::result::Result<Option<Self>, String> {
        Self::for_platform(manifest, models_dir, std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn for_platform(
        manifest: &Manifest,
        models_dir: &Path,
        os: &str,
        arch: &str,
    ) -> std::result::Result<Option<Self>, String> {
        let specs: Vec<AssetSpec> = select(&manifest.assets, os, arch)?.into_iter().cloned().collect();
        if specs.is_empty() {
            return Ok(None);
        }
        let id = &manifest.module.id;
        // The id becomes a folder name: exactly one ordinary component, so
        // `.`, `..` or a separator can never point the folder elsewhere.
        let one_component = matches!(Path::new(id).components().collect::<Vec<_>>().as_slice(), [Component::Normal(c)] if *c == std::ffi::OsStr::new(id));
        if !one_component || id.chars().all(|c| c == '.') {
            return Err(format!("module id {id:?} cannot name a folder for its files"));
        }
        Ok(Some(Self {
            module: id.clone(),
            root: models_dir.join(id),
            models_dir: models_dir.to_owned(),
            specs,
            pause: cancellable_pause,
        }))
    }

    /// Files that belong to no module (the managed Python, `uv`): `specs`
    /// under `<parent>/<name>`, with the same checks, locking and unpacking.
    /// `specs` should already be the ones for this system.
    pub fn new(name: &str, parent: &Path, specs: Vec<AssetSpec>) -> Self {
        Self {
            module: name.to_owned(),
            root: parent.join(name),
            models_dir: parent.to_owned(),
            specs,
            pause: cancellable_pause,
        }
    }

    /// Where `name` is (or will be) once ready: the file, or the folder it
    /// was unpacked into.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// What the `assets` host call answers: only assets verified against
    /// the hash the manifest names now are handed out; the rest (optional
    /// ones not downloaded, files left from an older version) are listed
    /// as missing.
    pub fn host_info(&self) -> glossa_module_sdk::wire::AssetsInfo {
        let mut info =
            glossa_module_sdk::wire::AssetsInfo { dir: self.root.to_string_lossy().into_owned(), ..Default::default() };
        for status in self.status() {
            if status.state == AssetState::Ready {
                info.files.insert(status.name, status.path.to_string_lossy().into_owned());
            } else {
                info.missing.push(status.name);
            }
        }
        info
    }

    /// Tell a module with assets where they are: `${ASSETS_DIR}` in its command
    /// line, arguments, environment and working folder, `GLOSSA_ASSETS_DIR` and
    /// one `GLOSSA_ASSET_<NAME>` per required asset in its environment
    /// (unless the manifest sets them itself), and the `assets` host call.
    /// The registry does this for every module it loads; tools that resolve
    /// a module's command line themselves (the module tester) do it too.
    pub fn apply_to(&self, manifest: &mut Manifest) {
        let assets = self;
        let root = assets.root.to_string_lossy().into_owned();
        let expand = |s: &mut String| {
            if s.contains("${ASSETS_DIR}") {
                *s = s.replace("${ASSETS_DIR}", &root);
            }
        };
        let m = &mut manifest.module;
        match &mut m.command {
            Some(crate::manifest::CommandSpec::Line(line)) => line.iter_mut().for_each(expand),
            Some(crate::manifest::CommandSpec::Program(p)) => expand(p),
            Some(crate::manifest::CommandSpec::PerPlatform(p)) => {
                for line in
                    [&mut p.windows, &mut p.macos, &mut p.linux, &mut p.unix, &mut p.default].into_iter().flatten()
                {
                    line.iter_mut().for_each(expand);
                }
            }
            None => {}
        }
        m.args.iter_mut().for_each(expand);
        m.env.values_mut().for_each(expand);
        m.classpath.iter_mut().for_each(expand);
        m.jvm_args.iter_mut().for_each(expand);
        expand(&mut m.cwd);
        m.env.entry("GLOSSA_ASSETS_DIR".into()).or_insert_with(|| root.clone());
        // Required assets only: the module starts once those are verified.
        // Optional ones may be missing or outdated, so a module learns about
        // them from the `assets` host call, which checks.
        for spec in assets.specs.iter().filter(|s| !s.optional) {
            let path = assets.path(&spec.name);
            m.env.entry(env_name(&spec.name)).or_insert_with(|| path.to_string_lossy().into_owned());
        }
        manifest.assets_dir = Some(assets.root.clone());
    }

    /// Every asset's path, by name.
    pub fn paths(&self) -> BTreeMap<String, PathBuf> {
        self.specs.iter().map(|s| (s.name.clone(), self.path(&s.name))).collect()
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    /// The partial download of `name`. Its name carries the start of the
    /// expected hash, so a part left over from an older version of the
    /// file is never resumed into the new one.
    fn part_path(&self, name: &str) -> PathBuf {
        let sha = self.specs.iter().find(|s| s.name == name).map(|s| s.sha256.to_ascii_lowercase()).unwrap_or_default();
        self.state_dir().join(format!("{name}.{}.part", &sha[..sha.len().min(16)]))
    }

    /// Delete partial downloads of `name` other than the current one.
    fn drop_stale_parts(&self, name: &str) {
        let current = self.part_path(name);
        let Ok(dir) = fs::read_dir(self.state_dir()) else { return };
        for e in dir.flatten() {
            let file = e.file_name().to_string_lossy().into_owned();
            let ours = file
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(|rest| rest.ends_with(".part") && !rest[..rest.len() - 5].contains('.'));
            if ours && e.path() != current {
                let _ = fs::remove_file(e.path());
            }
        }
    }

    /// Whether `path` lies inside this module's folder, which lies directly
    /// in the models folder. Checked before anything is written or deleted.
    fn contained(&self, path: &Path) -> io::Result<()> {
        let root_ok = self.root.parent() == Some(self.models_dir.as_path())
            && self.root.file_name().is_some_and(|n| n == std::ffi::OsStr::new(&self.module));
        let inside =
            path.strip_prefix(&self.root).is_ok_and(|rel| rel.components().all(|c| matches!(c, Component::Normal(_))));
        if root_ok && inside {
            Ok(())
        } else {
            Err(io::Error::other(format!("refusing to touch {} outside {}", path.display(), self.root.display())))
        }
    }

    fn remove_inside(&self, path: &Path) -> io::Result<()> {
        self.contained(path)?;
        remove_any(path)
    }

    /// Whether a download, a check or a delete runs for this module now, in
    /// this process (the UI greys out Delete meanwhile).
    pub fn is_busy(&self) -> bool {
        folder_lock(&self.root).try_lock().is_none()
    }

    /// Take the module's folder for writing: the in-process lock, then a
    /// lock file beside the folder against another Glossa process. Blocks
    /// while either is held elsewhere.
    pub fn exclusive(&self) -> io::Result<Exclusive> {
        lock_folder(&self.models_dir, &self.module)
    }

    fn marker_path(&self, name: &str) -> PathBuf {
        self.state_dir().join(format!("{name}.ok"))
    }

    fn read_marker(&self, name: &str) -> Option<Marker> {
        serde_json::from_slice(&fs::read(self.marker_path(name)).ok()?).ok()
    }

    /// Where each asset stands. Cheap: `stat`s and the small markers.
    pub fn status(&self) -> Vec<AssetStatus> {
        self.specs.iter().map(|s| self.status_of(s)).collect()
    }

    fn status_of(&self, spec: &AssetSpec) -> AssetStatus {
        let path = self.path(&spec.name);
        let state = self.state_of(spec, &path);
        AssetStatus {
            name: spec.name.clone(),
            path,
            size: spec.size,
            sha256: spec.sha256.clone(),
            optional: spec.optional,
            state,
        }
    }

    fn state_of(&self, spec: &AssetSpec, path: &Path) -> AssetState {
        let present = match spec.unpack {
            Unpack::None => fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == spec.size),
            Unpack::Gz => path.is_file(),
            Unpack::Zip | Unpack::TarGz => path.is_dir(),
        };
        let marker = self.read_marker(&spec.name);
        if present && marker.as_ref().is_some_and(|m| m.sha256.eq_ignore_ascii_case(&spec.sha256)) {
            return AssetState::Ready;
        }
        if path.exists() {
            return AssetState::Outdated;
        }
        match fs::metadata(self.part_path(&spec.name)) {
            Ok(m) if m.len() > 0 && m.len() <= spec.size => AssetState::Partial { have: m.len() },
            _ => AssetState::Missing,
        }
    }

    /// Whether every required (not `optional`) asset is ready: what the
    /// module needs to start.
    pub fn ready(&self) -> bool {
        self.required().all(|s| self.state_of(s, &self.path(&s.name)) == AssetState::Ready)
    }

    fn required(&self) -> impl Iterator<Item = &AssetSpec> {
        self.specs.iter().filter(|s| !s.optional)
    }

    /// The required assets that are not ready.
    pub fn missing(&self) -> Vec<String> {
        self.required()
            .filter(|s| self.state_of(s, &self.path(&s.name)) != AssetState::Ready)
            .map(|s| s.name.clone())
            .collect()
    }

    /// Bytes of the required assets.
    pub fn total_size(&self) -> u64 {
        self.required().map(|s| s.size).sum()
    }

    /// The assets `names` stands for: those named, or every required one
    /// when it is empty.
    pub fn chosen(&self, names: &[String]) -> Result<Vec<&AssetSpec>> {
        if names.is_empty() {
            return Ok(self.required().collect());
        }
        names
            .iter()
            .map(|n| {
                self.specs
                    .iter()
                    .find(|s| s.name == *n)
                    .ok_or_else(|| Error::Config(format!("module {} has no file {n:?} for this system", self.module)))
            })
            .collect()
    }

    /// Fetch every required asset that is not ready; see [`Self::download_some`].
    pub fn download(&self, http: &dyn Http, cancel: &CancelToken, progress: &dyn Fn(&Progress)) -> Result<()> {
        self.download_some(&[], http, cancel, progress)
    }

    /// Fetch the assets `names` names (every required one when empty) that
    /// are not ready, resuming partial files, and verify each against its
    /// SHA-256 before it is put in place. Blocks; run it on a worker thread.
    /// `progress` is called as bytes arrive.
    pub fn download_some(
        &self,
        names: &[String],
        http: &dyn Http,
        cancel: &CancelToken,
        progress: &dyn Fn(&Progress),
    ) -> Result<()> {
        let chosen = self.chosen(names)?;
        let _busy = self.exclusive()?;
        let total = chosen.iter().map(|s| s.size).sum();
        let mut base = 0u64;
        for spec in chosen {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if self.state_of(spec, &self.path(&spec.name)) != AssetState::Ready {
                self.fetch_one(spec, http, cancel, &|have| {
                    progress(&Progress { asset: spec.name.clone(), done: base + have, total })
                })?;
            }
            base += spec.size;
            progress(&Progress { asset: spec.name.clone(), done: base, total });
        }
        Ok(())
    }

    fn fetch_one(&self, spec: &AssetSpec, http: &dyn Http, cancel: &CancelToken, progress: &dyn Fn(u64)) -> Result<()> {
        fs::create_dir_all(self.state_dir())?;
        self.drop_stale_parts(&spec.name);
        let part = self.part_path(&spec.name);
        // What the user sees never goes backwards: when a server cannot
        // resume and the file starts over, the bar waits where it was until
        // the new copy has caught up.
        let shown = std::cell::Cell::new(None::<u64>);
        let progress = |have: u64| {
            if shown.get().is_none_or(|s| have > s) {
                shown.set(Some(have));
                progress(have);
            }
        };
        // A file put together from several responses that fails its check
        // gets one more go from nothing: a mirror that changed the file
        // between two pieces is not a damaged download.
        let mut fresh_start_left = true;
        loop {
            let pieces = self.fetch_from_mirrors(spec, &part, http, cancel, &progress)?;
            let actual = sha256_file(&part)?;
            if actual.eq_ignore_ascii_case(&spec.sha256) {
                return self.install(spec, &part);
            }
            // Never keep, resume or install a file with the wrong hash.
            let _ = fs::remove_file(&part);
            if pieces > 1 && fresh_start_left {
                fresh_start_left = false;
                tracing::warn!(module = %self.module, asset = %spec.name, "a resumed download failed its check; downloading it again from the start");
                continue;
            }
            return Err(Error::AssetCorrupt {
                module: self.module.clone(),
                asset: spec.name.clone(),
                expected: spec.sha256.to_ascii_lowercase(),
                actual,
            });
        }
    }

    /// Bring `part` to its full size from the first mirror that manages it,
    /// retrying each one and resuming where the file ends; see
    /// [`ATTEMPTS_PER_MIRROR`]. Returns how many pieces the file was put
    /// together from (1 for a download in one go, more when it was resumed,
    /// in this run or after an earlier one).
    fn fetch_from_mirrors(
        &self,
        spec: &AssetSpec,
        part: &Path,
        http: &dyn Http,
        cancel: &CancelToken,
        progress: &dyn Fn(u64),
    ) -> Result<u32> {
        let size_of = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let mut pieces = u32::from(size_of(part) > 0);
        let mut last_err = None;
        for url in spec.url.list() {
            let mut stalled = 0;
            for request in 1..=MAX_REQUESTS_PER_MIRROR {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let before = size_of(part);
                let outcome = self.transfer(spec, url, part, http, cancel, progress);
                let after = size_of(part);
                let e = match outcome {
                    Ok(Transfer::Whole { resumed: true }) => return Ok(pieces + 1),
                    Ok(Transfer::Whole { resumed: false }) => return Ok(1),
                    Ok(Transfer::AlreadyWhole) => return Ok(pieces.max(1)),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => e,
                };
                if after > before {
                    // Headway: this mirror works, however shakily.
                    pieces += 1;
                    stalled = 0;
                } else if after < before {
                    // The server sent the file from its start (or the part
                    // was dropped): what was there before is gone.
                    pieces = u32::from(after > 0);
                }
                stalled += 1;
                let again = worth_retrying(&e) && stalled < ATTEMPTS_PER_MIRROR && request < MAX_REQUESTS_PER_MIRROR;
                tracing::warn!(
                    module = %self.module, asset = %spec.name, url = %glossa_net::redact(url), have = after,
                    "asset download failed{}: {e}", if again { "; trying again" } else { "" }
                );
                if !again {
                    last_err = Some(e);
                    break;
                }
                let asked = match &e {
                    Error::RateLimited { retry_after_secs, .. } => *retry_after_secs,
                    _ => None,
                };
                (self.pause)(retry_pause(stalled, asked), cancel)?;
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Network(format!("{} has no download address", spec.name))))
    }

    /// One attempt from one URL, appending to `part` when the server can
    /// resume and starting over when it cannot.
    fn transfer(
        &self,
        spec: &AssetSpec,
        url: &str,
        part: &Path,
        http: &dyn Http,
        cancel: &CancelToken,
        progress: &dyn Fn(u64),
    ) -> Result<Transfer> {
        let mut have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if have > spec.size {
            fs::remove_file(part)?;
            have = 0;
        }
        if have == spec.size {
            return Ok(Transfer::AlreadyWhole);
        }
        let provider = format!("{} assets", self.module);
        let mut request = HttpRequest::get(url).provider(provider.clone()).timeout(TRANSFER_BUDGET);
        if have > 0 {
            request = request.header("Range", format!("bytes={have}-"));
        }
        let mut file: Option<File> = None;
        let mut written = have;
        let mut resumed = false;
        let mut failure: Option<Error> = None;
        let response = http.send_download(request, &mut |event| match event {
            DownloadEvent::Status(status) => {
                // 206: the server resumes where the partial file ends.
                // Anything else: it sends the whole file, so start over.
                let opened = if status == 206 && have > 0 {
                    resumed = true;
                    fs::OpenOptions::new().append(true).open(part)
                } else {
                    written = 0;
                    File::create(part)
                };
                match opened {
                    Ok(f) => {
                        file = Some(f);
                        progress(written);
                        true
                    }
                    Err(e) => {
                        failure = Some(e.into());
                        false
                    }
                }
            }
            DownloadEvent::Chunk(chunk) => {
                if cancel.is_cancelled() {
                    failure = Some(Error::Cancelled);
                    return false;
                }
                let Some(f) = file.as_mut() else { return false };
                if written + chunk.len() as u64 > spec.size {
                    failure = Some(Error::bad_response(
                        &provider,
                        format!("{} is larger than the {} bytes the manifest gives", spec.name, spec.size),
                    ));
                    return false;
                }
                if let Err(e) = f.write_all(chunk) {
                    failure = Some(e.into());
                    return false;
                }
                written += chunk.len() as u64;
                progress(written);
                true
            }
        });
        if let Some(mut f) = file.take() {
            f.flush()?;
        }
        if let Some(e) = failure {
            if matches!(e, Error::BadResponse { .. }) {
                // Too large: what we have cannot be the right file.
                let _ = fs::remove_file(part);
            }
            return Err(e);
        }
        let response = response?;
        if response.status == 416 && have > 0 {
            // The server will not continue from where the partial file
            // ends (it is not the file we began, or it never was): drop
            // the part, and the next attempt asks for the whole file.
            let _ = fs::remove_file(part);
            return Err(Error::Network(format!(
                "the server cannot resume {} from byte {have}; downloading it again from the start",
                spec.name
            )));
        }
        if !response.is_success() {
            return match response.error_for_status(&provider) {
                Err(e) => Err(e),
                Ok(r) => Err(Error::HttpStatus { provider, status: r.status, body: String::new() }),
            };
        }
        if written < spec.size {
            // The connection closed early. The partial file stays, and the
            // next attempt (this mirror again, another one, or the next
            // download) resumes.
            return Err(Error::Network(format!(
                "the download of {} stopped after {written} of {} bytes; downloading again resumes it",
                spec.name, spec.size
            )));
        }
        Ok(Transfer::Whole { resumed })
    }

    /// Put a verified download in place and write its marker.
    fn install(&self, spec: &AssetSpec, part: &Path) -> Result<()> {
        let dest = self.path(&spec.name);
        self.remove_inside(&dest)?;
        let mut files = Vec::new();
        match spec.unpack {
            Unpack::None => {
                fs::rename(part, &dest)?;
                #[cfg(unix)]
                if spec.executable {
                    set_executable(&dest, 0o755);
                }
            }
            Unpack::Gz => {
                let staging = self.state_dir().join(format!("{}.unpacking", spec.name));
                let unpacked = gunzip(part, &staging, &mut Budget::new(spec.size));
                if let Err(e) = unpacked {
                    let _ = fs::remove_file(&staging);
                    let _ = fs::remove_file(part);
                    return Err(Error::Module {
                        module: self.module.clone(),
                        detail: format!("cannot unpack {}: {e}", spec.name),
                    });
                }
                #[cfg(unix)]
                if spec.executable {
                    set_executable(&staging, 0o755);
                }
                files.push((String::new(), fs::metadata(&staging)?.len()));
                fs::rename(&staging, &dest)?;
                fs::remove_file(part)?;
            }
            kind => {
                let staging = self.state_dir().join(format!("{}.unpacking", spec.name));
                self.remove_inside(&staging)?;
                fs::create_dir_all(&staging)?;
                let unpacked = match kind {
                    Unpack::Zip => unzip(part, &staging, &mut Budget::new(spec.size)),
                    _ => untar_gz(part, &staging, &mut Budget::new(spec.size)),
                };
                if let Err(e) = unpacked {
                    let _ = fs::remove_dir_all(&staging);
                    let _ = fs::remove_file(part);
                    return Err(Error::Module {
                        module: self.module.clone(),
                        detail: format!("cannot unpack {}: {e}", spec.name),
                    });
                }
                files = list_files(&staging)?;
                fs::rename(&staging, &dest)?;
                fs::remove_file(part)?;
            }
        }
        let marker = Marker { sha256: spec.sha256.to_ascii_lowercase(), size: spec.size, files };
        fs::write(self.marker_path(&spec.name), serde_json::to_vec(&marker)?)?;
        tracing::info!(module = %self.module, asset = %spec.name, "module asset ready");
        Ok(())
    }

    /// A full check: re-hash every plain file and check every unpacked file
    /// is there with its size. Anything damaged is deleted, so the next
    /// download fetches it again. Blocks.
    pub fn verify(&self) -> Result<Vec<AssetStatus>> {
        let _busy = self.exclusive()?;
        let mut out = Vec::new();
        for spec in &self.specs {
            let mut status = self.status_of(spec);
            if status.state == AssetState::Ready && !self.intact(spec)? {
                self.remove_inside(&status.path)?;
                let _ = fs::remove_file(self.marker_path(&spec.name));
                status.state = AssetState::Corrupt;
            }
            out.push(status);
        }
        Ok(out)
    }

    fn intact(&self, spec: &AssetSpec) -> Result<bool> {
        let path = self.path(&spec.name);
        match spec.unpack {
            Unpack::None => Ok(sha256_file(&path)?.eq_ignore_ascii_case(&spec.sha256)),
            Unpack::Gz => {
                let Some(marker) = self.read_marker(&spec.name) else { return Ok(false) };
                let expected = marker.files.first().map(|(_, size)| *size);
                Ok(fs::metadata(&path).is_ok_and(|m| m.is_file() && Some(m.len()) == expected))
            }
            Unpack::Zip | Unpack::TarGz => {
                let Some(marker) = self.read_marker(&spec.name) else { return Ok(false) };
                Ok(marker
                    .files
                    .iter()
                    .all(|(rel, size)| fs::metadata(path.join(rel)).is_ok_and(|m| m.is_file() && m.len() == *size)))
            }
        }
    }

    /// Delete every downloaded file of this module, partial ones included.
    pub fn remove(&self) -> Result<()> {
        let _busy = self.exclusive()?;
        self.contained(&self.root.join(STATE_DIR))?;
        remove_any(&self.root)?;
        Ok(())
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The SHA-256 of a file, in lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every file below `root`, relative, with its size.
fn list_files(root: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let ty = e.file_type()?;
            if ty.is_dir() {
                stack.push(e.path());
            } else if ty.is_file() {
                let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/");
                out.push((rel, e.metadata()?.len()));
            }
        }
    }
    out.sort();
    Ok(out)
}

// -- unpacking ------------------------------------------------------------------

/// `name` from an archive as a path below `root`, or `None` for anything
/// that would land outside it (`../x`, `/etc/x`, `C:\x`).
fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name);
    let mut out = root.to_owned();
    let mut any = false;
    for c in rel.components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                any = true;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    any.then_some(out)
}

/// Counts what an archive unpacks to against [`unpack_limit`].
struct Budget {
    left: u64,
}

impl Budget {
    fn new(download_size: u64) -> Self {
        Self { left: unpack_limit(download_size) }
    }

    /// Copy at most what is left of the budget, failing past it.
    fn copy(&mut self, from: &mut impl Read, to: &mut impl Write, what: &str) -> std::result::Result<u64, String> {
        let copied = io::copy(&mut from.take(self.left + 1), to).map_err(|e| format!("{what}: {e}"))?;
        if copied > self.left {
            return Err(format!(
                "{what}: the archive unpacks to more than {} bytes, far beyond its download size",
                self.left
            ));
        }
        self.left -= copied;
        Ok(copied)
    }
}

fn unzip(archive: &Path, dest: &Path, budget: &mut Budget) -> std::result::Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");
        let Some(path) = safe_join(dest, &name) else {
            return Err(format!("{name:?} points outside the archive's folder"));
        };
        if entry.is_dir() {
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        budget.copy(&mut entry, &mut out, &name)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            set_executable(&path, mode);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o111 != 0 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
}

fn gunzip(archive: &Path, dest: &Path, budget: &mut Budget) -> std::result::Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut r = flate2::read::GzDecoder::new(io::BufReader::new(file));
    let mut out = File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    budget.copy(&mut r, &mut out, "gz")?;
    Ok(())
}

/// A minimal tar reader: regular files and folders, with GNU long names and
/// pax `path` records. Links, devices and the like are skipped; an asset has
/// no business carrying them.
fn untar_gz(archive: &Path, dest: &Path, budget: &mut Budget) -> std::result::Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut r = flate2::read::GzDecoder::new(io::BufReader::new(file));
    let mut header = [0u8; 512];
    let mut long_name: Option<String> = None;
    loop {
        if !read_block(&mut r, &mut header)? {
            break;
        }
        if header.iter().all(|b| *b == 0) {
            break; // the end-of-archive blocks
        }
        let size = parse_octal(&header[124..136]).ok_or("a tar header has a bad size")?;
        let kind = header[156];
        let mut name = long_name.take().unwrap_or_else(|| {
            let base = cstr(&header[0..100]);
            let prefix = if &header[257..262] == b"ustar" { cstr(&header[345..500]) } else { String::new() };
            if prefix.is_empty() {
                base
            } else {
                format!("{prefix}/{base}")
            }
        });
        let padded = size.div_ceil(512) * 512;
        match kind {
            b'L' | b'x' => {
                if size > MAX_TAR_RECORD {
                    return Err(format!("a tar header record of {size} bytes is not a file name"));
                }
                let mut data = vec![0u8; size as usize];
                r.read_exact(&mut data).map_err(|e| e.to_string())?;
                skip(&mut r, padded - size)?;
                long_name = if kind == b'L' { Some(cstr(&data)) } else { pax_path(&data) };
                continue;
            }
            b'g' => {
                skip(&mut r, padded)?;
                continue;
            }
            _ => {}
        }
        name = name.replace('\\', "/");
        let is_dir = kind == b'5' || (kind == 0 || kind == b'0') && name.ends_with('/');
        let is_file = kind == 0 || kind == b'0' || kind == b'7';
        if !(is_dir || is_file) {
            skip(&mut r, padded)?;
            continue;
        }
        let Some(path) = safe_join(dest, name.trim_end_matches('/')) else {
            if name.trim_matches(['.', '/']).is_empty() {
                skip(&mut r, padded)?;
                continue;
            }
            return Err(format!("{name:?} points outside the archive's folder"));
        };
        if is_dir {
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            skip(&mut r, padded)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let copied = budget.copy(&mut (&mut r).take(size), &mut out, &name)?;
        if copied != size {
            return Err(format!("{name}: the archive ends early"));
        }
        skip(&mut r, padded - size)?;
        #[cfg(unix)]
        if let Some(mode) = parse_octal(&header[100..108]) {
            set_executable(&path, mode as u32);
        }
    }
    Ok(())
}

/// Read one 512-byte block; `false` at a clean end of the stream.
fn read_block(r: &mut impl Read, buf: &mut [u8; 512]) -> std::result::Result<bool, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err("the archive ends inside a header".into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}

fn skip(r: &mut impl Read, n: u64) -> std::result::Result<(), String> {
    let skipped = io::copy(&mut r.take(n), &mut io::sink()).map_err(|e| e.to_string())?;
    if skipped == n {
        Ok(())
    } else {
        Err("the archive ends early".into())
    }
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn parse_octal(field: &[u8]) -> Option<u64> {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        // GNU base-256 for sizes past 8 GiB.
        let mut v: u64 = 0;
        for b in &field[1..] {
            v = v.checked_mul(256)?.checked_add(*b as u64)?;
        }
        return Some(v);
    }
    let s = cstr(field);
    let s = s.trim_matches(|c: char| c == ' ' || c == '\0');
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

/// The `path` record of a pax extended header (`"<len> path=<value>\n"`).
fn pax_path(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once(' ')?;
        rest.strip_prefix("path=").map(str::to_owned)
    })
}

// -- the gate -------------------------------------------------------------------

/// Builds the module's real provider once its assets are ready.
pub type MakeProvider = Box<dyn Fn() -> std::result::Result<Arc<dyn Provider>, String> + Send + Sync>;

/// Stands in for a module whose assets are not ready. It answers for the
/// module (name, options, capabilities) without starting anything; the first
/// call starts the real runtime once the files are there, or fails with
/// [`Error::AssetsMissing`] for the front end to offer the download (or
/// downloads them itself when automatic downloads are on).
pub struct AssetGate {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
    /// For the settings page: the layout and translations travel with the
    /// options, so the page looks the same before the files arrive.
    manifest: Manifest,
    page: crate::settings::PageCache,
    assets: Arc<ModuleAssets>,
    auto_download: bool,
    make: MakeProvider,
    inner: Mutex<Option<Arc<dyn Provider>>>,
}

impl AssetGate {
    pub fn new(manifest: &Manifest, assets: Arc<ModuleAssets>, auto_download: bool, make: MakeProvider) -> Self {
        Self {
            info: manifest.provider_info(),
            options: manifest.options.clone(),
            manifest: manifest.clone(),
            page: Default::default(),
            assets,
            auto_download,
            make,
            inner: Mutex::new(None),
        }
    }

    /// Whether the real runtime has been started.
    pub fn is_loaded(&self) -> bool {
        self.inner.lock().is_some()
    }

    fn provider(&self, host: &Host) -> Result<Arc<dyn Provider>> {
        let mut inner = self.inner.lock();
        if let Some(p) = inner.as_ref() {
            return Ok(p.clone());
        }
        if !self.assets.ready() {
            if !self.auto_download {
                return Err(Error::AssetsMissing { module: self.info.id.clone(), missing: self.assets.missing() });
            }
            tracing::info!(module = %self.info.id, "downloading the module's files on its first use");
            self.assets.download(host.http, host.cancel, &|_| {})?;
        }
        let p = (self.make)().map_err(|e| Error::module(&self.info.id, e))?;
        *inner = Some(p.clone());
        Ok(p)
    }
}

impl Provider for AssetGate {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn settings_page(&self, lang: &str) -> Arc<crate::settings::SettingsPage> {
        self.page.get(lang, || self.manifest.settings_page(lang))
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        self.provider(host)?.translate(host, req)
    }

    fn translate_stream(&self, host: &Host, req: &TranslateRequest, progress: &dyn Fn(&str)) -> Result<Translation> {
        self.provider(host)?.translate_stream(host, req, progress)
    }

    fn list_models(&self, host: &Host) -> Result<Vec<String>> {
        self.provider(host)?.list_models(host)
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        self.provider(host)?.detect(host, text)
    }

    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        self.provider(host)?.dictionary(host, req)
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        self.provider(host)?.speak(host, req)
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        self.provider(host)?.recognize(host, req)
    }

    fn action(
        &self,
        host: &Host,
        params: glossa_module_sdk::wire::ActionParams,
    ) -> Result<glossa_module_sdk::wire::ActionReply> {
        self.provider(host)?.action(host, params)
    }

    fn suspend(&self) {
        if let Some(p) = self.inner.lock().as_ref() {
            p.suspend();
        }
    }

    fn warm_up(&self) {
        // Only a runtime that is already there; warming up must not start
        // one whose files may be missing.
        let p = self.inner.lock().clone();
        if let Some(p) = p {
            p.warm_up();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    const HELLO: &str = "hello world";
    const HELLO_SHA: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glossa-assets-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn manifest(assets: &str) -> Manifest {
        Manifest::parse(&format!(
            "capabilities = [\"translate\"]\n[permissions]\nnetwork = [\"*.test\", \"x\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"js\"\n{assets}"
        ))
        .unwrap()
    }

    fn one(url: &str, sha: &str, size: usize, extra: &str) -> String {
        format!("[[assets]]\nname = \"data.bin\"\nurl = {url}\nsha256 = \"{sha}\"\nsize = {size}\n{extra}\n")
    }

    fn assets(tmp: &Temp, toml: &str) -> ModuleAssets {
        let mut a = ModuleAssets::for_manifest(&manifest(toml), &tmp.0).unwrap().expect("assets apply");
        a.pause = record_pause;
        a
    }

    thread_local! {
        /// The pauses the download on this thread asked for.
        static PAUSES: std::cell::RefCell<Vec<std::time::Duration>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Stands in for [`cancellable_pause`]: notes the pause instead of
    /// sleeping through it.
    fn record_pause(pause: std::time::Duration, cancel: &CancelToken) -> Result<()> {
        PAUSES.with(|p| p.borrow_mut().push(pause));
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    fn pauses() -> Vec<std::time::Duration> {
        PAUSES.with(|p| std::mem::take(&mut *p.borrow_mut()))
    }

    /// What a [`Scripted`] server does with one request.
    enum Step {
        /// Answer with this status, these headers and this body.
        Reply(u16, &'static [(&'static str, &'static str)], &'static str),
        /// Start a 200 or 206 answer, send this much of the body, and drop
        /// the connection.
        Cut(u16, &'static str),
        /// Never answer: the connection fails.
        Refuse,
    }

    /// An `Http` playing a script, one step per request, for what
    /// `MockHttp` cannot do: a connection that dies in the middle of a
    /// body, and response headers.
    struct Scripted {
        steps: Mutex<std::collections::VecDeque<Step>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Self {
            Self { steps: Mutex::new(steps.into()), seen: Mutex::new(Vec::new()) }
        }

        fn ranges(&self) -> Vec<Option<String>> {
            self.seen
                .lock()
                .iter()
                .map(|r| r.headers.iter().find(|(n, _)| n == "Range").map(|(_, v)| v.clone()))
                .collect()
        }
    }

    impl Http for Scripted {
        fn send(&self, _: HttpRequest) -> Result<glossa_net::HttpResponse> {
            unreachable!("assets are fetched with send_download")
        }

        fn send_download(
            &self,
            request: HttpRequest,
            on_event: &mut dyn FnMut(DownloadEvent<'_>) -> bool,
        ) -> Result<glossa_net::HttpResponse> {
            self.seen.lock().push(request);
            let step = self.steps.lock().pop_front().expect("a request the script does not expect");
            match step {
                Step::Refuse => Err(Error::Network("connection refused".into())),
                Step::Cut(status, sent) => {
                    if on_event(DownloadEvent::Status(status)) {
                        on_event(DownloadEvent::Chunk(sent.as_bytes()));
                    }
                    Err(Error::Network("reading body: connection reset by peer".into()))
                }
                Step::Reply(status, headers, body) => {
                    let headers = headers.iter().map(|(n, v)| ((*n).to_owned(), (*v).to_owned())).collect();
                    if (200..300).contains(&status) {
                        if on_event(DownloadEvent::Status(status)) {
                            on_event(DownloadEvent::Chunk(body.as_bytes()));
                        }
                        return Ok(glossa_net::HttpResponse { status, headers, ..Default::default() });
                    }
                    Ok(glossa_net::HttpResponse { status, headers, body: body.as_bytes().to_vec(), codepage: None })
                }
            }
        }
    }

    fn hello(tmp: &Temp, urls: &str) -> ModuleAssets {
        assets(tmp, &one(urls, HELLO_SHA, HELLO.len(), ""))
    }

    #[test]
    fn a_connection_cut_in_the_middle_of_the_body_is_resumed_in_the_same_download() {
        let tmp = Temp::new("cut");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        let http = Scripted::new(vec![Step::Cut(200, "hello "), Step::Refuse, Step::Reply(206, &[], "world")]);
        let seen = Mutex::new(Vec::new());
        pauses();
        a.download(&http, &CancelToken::new(), &|p| seen.lock().push(p.done)).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
        assert_eq!(http.ranges(), [None, Some("bytes=6-".into()), Some("bytes=6-".into())]);
        assert_eq!(pauses().len(), 2, "one pause before each new attempt");
        let seen = seen.lock();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "progress never goes back: {seen:?}");
        assert_eq!(seen.last(), Some(&(HELLO.len() as u64)));
    }

    #[test]
    fn a_server_without_range_support_starts_over_and_the_progress_does_not_go_back() {
        let tmp = Temp::new("norange-retry");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        // Six bytes, a dropped connection, and then the whole file again
        // with a 200 although a range was asked for.
        let http = Scripted::new(vec![Step::Cut(200, "hello "), Step::Cut(200, "hel"), Step::Reply(200, &[], HELLO)]);
        let seen = Mutex::new(Vec::new());
        a.download(&http, &CancelToken::new(), &|p| seen.lock().push(p.done)).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO, "no piece of the first try is left in it");
        assert_eq!(http.ranges(), [None, Some("bytes=6-".into()), Some("bytes=3-".into())]);
        let seen = seen.lock();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "progress never goes back: {seen:?}");
        assert_eq!(seen.last(), Some(&(HELLO.len() as u64)));
    }

    #[test]
    fn a_rate_limit_waits_as_long_as_the_server_asks_and_then_goes_on() {
        let tmp = Temp::new("limited");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        let http = Scripted::new(vec![
            Step::Reply(429, &[("Retry-After", "7")], "slow down"),
            Step::Reply(503, &[], "busy"),
            Step::Reply(200, &[], HELLO),
        ]);
        pauses();
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert!(a.ready());
        let waited = pauses();
        assert_eq!(waited.len(), 2);
        assert!(waited[0] >= std::time::Duration::from_secs(7), "Retry-After is honoured: {waited:?}");
        assert!(waited[1] <= std::time::Duration::from_secs(2), "a plain failure backs off from a second: {waited:?}");
    }

    #[test]
    fn pauses_double_with_jitter_and_stay_bounded() {
        for attempt in 1..=40 {
            let p = retry_pause(attempt, None).as_secs_f64();
            let nominal = 2f64.powi((attempt - 1).min(5) as i32).min(30.0);
            assert!(p >= nominal * 0.5 && p <= nominal, "attempt {attempt}: {p}");
        }
        assert_eq!(retry_pause(1, Some(40)), std::time::Duration::from_secs(40));
        assert_eq!(retry_pause(1, Some(86_400)), MAX_RETRY_PAUSE, "a server cannot park a download for a day");
    }

    #[test]
    fn a_wrong_hash_after_a_resume_is_downloaded_afresh_once_and_never_accepted() {
        // The two pieces do not make the file the manifest names (the
        // mirror changed it in between); a clean third request does.
        let tmp = Temp::new("resume-hash");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        let http =
            Scripted::new(vec![Step::Cut(200, "HELLO "), Step::Reply(206, &[], "world"), Step::Reply(200, &[], HELLO)]);
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
        assert_eq!(http.ranges(), [None, Some("bytes=6-".into()), None], "the last request asks for the whole file");

        // Still wrong when fetched in one go: refused, deleted, not retried again.
        let tmp = Temp::new("resume-hash-bad");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        let http = Scripted::new(vec![
            Step::Cut(200, "HELLO "),
            Step::Reply(206, &[], "world"),
            Step::Reply(200, &[], "HELLO WORLD"),
        ]);
        let err = a.download(&http, &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(matches!(err, Error::AssetCorrupt { ref actual, .. } if *actual == sha_hex(b"HELLO WORLD")), "{err:?}");
        assert!(!a.path("data.bin").exists() && !a.part_path("data.bin").exists());
        assert_eq!(a.status()[0].state, AssetState::Missing);
        assert!(http.steps.lock().is_empty());
    }

    #[test]
    fn a_mirror_that_keeps_failing_gives_way_to_the_next_and_a_refusal_is_not_retried() {
        let tmp = Temp::new("giveup");
        let a = hello(&tmp, "[\"https://one.test/d\", \"https://two.test/d\"]");
        let mut steps: Vec<Step> = (0..ATTEMPTS_PER_MIRROR).map(|_| Step::Refuse).collect();
        steps.push(Step::Reply(200, &[], HELLO));
        let http = Scripted::new(steps);
        pauses();
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        let urls: Vec<String> = http.seen.lock().iter().map(|r| r.url.clone()).collect();
        assert_eq!(urls.iter().filter(|u| u.contains("one.test")).count(), ATTEMPTS_PER_MIRROR as usize);
        assert_eq!(urls.last().map(String::as_str), Some("https://two.test/d"));
        assert_eq!(pauses().len(), ATTEMPTS_PER_MIRROR as usize - 1);

        // 404 and 403 will not change by asking again.
        let tmp = Temp::new("giveup-404");
        let a = hello(&tmp, "\"https://one.test/d\"");
        let http = Scripted::new(vec![Step::Reply(404, &[], "gone")]);
        let err = a.download(&http, &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(matches!(err, Error::HttpStatus { status: 404, .. }), "{err:?}");
        assert!(pauses().is_empty());
    }

    #[test]
    fn a_server_that_cannot_continue_the_part_gets_asked_for_the_whole_file() {
        let tmp = Temp::new("416");
        let a = hello(&tmp, "\"https://dl.test/data.bin\"");
        fs::create_dir_all(a.state_dir()).unwrap();
        fs::write(a.part_path("data.bin"), "junk!").unwrap();
        let http = Scripted::new(vec![Step::Reply(416, &[], ""), Step::Reply(200, &[], HELLO)]);
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
        assert_eq!(http.ranges(), [Some("bytes=5-".into()), None]);
    }

    #[test]
    fn cancelling_while_waiting_to_retry_stops_the_download_and_keeps_the_part() {
        let tmp = Temp::new("cancel-wait");
        let mut a = hello(&tmp, "\"https://dl.test/data.bin\"");
        a.pause = cancellable_pause;
        let cancel = CancelToken::new();
        let http = Scripted::new(vec![Step::Cut(200, "hello ")]);
        let started = std::time::Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(40));
                cancel.cancel();
            });
            assert!(matches!(a.download(&http, &cancel, &|_| {}), Err(Error::Cancelled)));
        });
        assert!(started.elapsed() < std::time::Duration::from_millis(450), "the pause ended early");
        assert_eq!(a.status()[0].state, AssetState::Partial { have: 6 });
    }

    /// An `Http` serving binary bodies, which `MockHttp` (text only) cannot.
    struct Bytes(Vec<u8>);

    impl Http for Bytes {
        fn send(&self, _: HttpRequest) -> Result<glossa_net::HttpResponse> {
            Ok(glossa_net::HttpResponse { status: 200, body: self.0.clone(), ..Default::default() })
        }
    }

    fn sha_hex(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn a_good_download_is_verified_and_put_in_place() {
        let tmp = Temp::new("good");
        let a = assets(&tmp, &one("\"https://dl.test/data.bin\"", HELLO_SHA, HELLO.len(), ""));
        assert_eq!(a.status()[0].state, AssetState::Missing);
        assert!(!a.ready());
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        let seen = Mutex::new(Vec::new());
        a.download(&http, &CancelToken::new(), &|p| seen.lock().push(p.done)).unwrap();
        assert!(a.ready());
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
        assert_eq!(seen.lock().last(), Some(&(HELLO.len() as u64)));
        assert!(!a.part_path("data.bin").exists(), "the partial file is gone");
        // A second download has nothing to do.
        a.download(&MockHttp::new(), &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(a.verify().unwrap()[0].state, AssetState::Ready);
    }

    #[test]
    fn a_bad_hash_deletes_the_file_and_says_so() {
        let tmp = Temp::new("bad");
        let a = assets(&tmp, &one("\"https://dl.test/data.bin\"", HELLO_SHA, HELLO.len(), ""));
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, "HELLO WORLD");
        match a.download(&http, &CancelToken::new(), &|_| {}).unwrap_err() {
            Error::AssetCorrupt { module, asset, expected, actual } => {
                assert_eq!((module.as_str(), asset.as_str(), expected.as_str()), ("m", "data.bin", HELLO_SHA));
                assert_eq!(actual, sha_hex(b"HELLO WORLD"));
            }
            other => panic!("expected AssetCorrupt, got {other:?}"),
        }
        assert_eq!(a.status()[0].state, AssetState::Missing);
        assert!(!a.path("data.bin").exists() && !a.part_path("data.bin").exists());
    }

    #[test]
    fn an_interrupted_download_resumes_with_a_range_request() {
        let tmp = Temp::new("resume");
        let a = assets(&tmp, &one("\"https://dl.test/data.bin\"", HELLO_SHA, HELLO.len(), ""));
        // The connection drops after six bytes, then the server resumes.
        let http = MockHttp::new().on_once(Method::Get, "https://dl.test/", 200, "hello ").on_once(
            Method::Get,
            "https://dl.test/",
            206,
            "world",
        );
        // One download now sees it through: the short answer is retried.
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
        let requests = http.requests();
        assert!(requests[0].headers.iter().all(|(n, _)| n != "Range"));
        assert!(requests[1].headers.contains(&("Range".into(), "bytes=6-".into())), "{:?}", requests[1].headers);
    }

    #[test]
    fn a_server_ignoring_the_range_starts_the_file_over() {
        let tmp = Temp::new("norange");
        let a = assets(&tmp, &one("\"https://dl.test/data.bin\"", HELLO_SHA, HELLO.len(), ""));
        fs::create_dir_all(a.state_dir()).unwrap();
        fs::write(a.part_path("data.bin"), "junk!").unwrap();
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(fs::read_to_string(a.path("data.bin")).unwrap(), HELLO);
    }

    #[test]
    fn mirrors_are_tried_in_order_and_oversized_files_refused() {
        let tmp = Temp::new("mirrors");
        let urls = "[\"https://one.test/d\", \"https://two.test/d\"]";
        let a = assets(&tmp, &one(urls, HELLO_SHA, HELLO.len(), ""));
        let http = MockHttp::new().on(Method::Get, "https://one.test/", 404, "gone").on(
            Method::Get,
            "https://two.test/",
            200,
            HELLO,
        );
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert!(a.ready());

        let tmp = Temp::new("toobig");
        let a = assets(&tmp, &one("\"https://one.test/d\"", HELLO_SHA, 5, ""));
        let http = MockHttp::new().on(Method::Get, "https://one.test/", 200, HELLO);
        let err = a.download(&http, &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(err.to_string().contains("larger than"), "{err}");
        assert!(!a.part_path("data.bin").exists());
    }

    #[test]
    fn a_cancelled_download_stops_and_keeps_its_part() {
        let tmp = Temp::new("cancel");
        let a = assets(&tmp, &one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), ""));
        let cancel = CancelToken::new();
        cancel.cancel();
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        assert!(matches!(a.download(&http, &cancel, &|_| {}), Err(Error::Cancelled)));
        assert!(!a.ready());
    }

    #[test]
    fn os_and_arch_selectors_pick_the_entry_for_this_system() {
        let toml = format!(
            "{}{}{}",
            one("\"https://dl.test/w\"", HELLO_SHA, 11, "os = [\"windows\"]\narch = [\"x86_64\"]"),
            one("\"https://dl.test/u\"", HELLO_SHA, 11, "os = [\"unix\"]"),
            "[[assets]]\nname = \"shared\"\nurl = \"https://dl.test/s\"\nsha256 = \"".to_owned()
                + HELLO_SHA
                + "\"\nsize = 11\n",
        );
        let m = manifest(&toml);
        let names = |os, arch| -> Vec<String> {
            select(&m.assets, os, arch)
                .unwrap()
                .iter()
                .map(|a| format!("{}@{}", a.name, a.url.list()[0].rsplit('/').next().unwrap()))
                .collect()
        };
        assert_eq!(names("windows", "x86_64"), ["data.bin@w", "shared@s"]);
        assert_eq!(names("windows", "aarch64"), ["shared@s"]);
        assert_eq!(names("linux", "aarch64"), ["data.bin@u", "shared@s"]);
        assert_eq!(names("macos", "x86_64"), ["data.bin@u", "shared@s"]);
        let tmp = Temp::new("select");
        let a = ModuleAssets::for_platform(&m, &tmp.0, "windows", "aarch64").unwrap().unwrap();
        assert_eq!(a.specs.len(), 1);
        let only_windows = manifest(&one("\"https://dl.test/w\"", HELLO_SHA, 11, "os = [\"windows\"]"));
        assert!(ModuleAssets::for_platform(&only_windows, &tmp.0, "linux", "x86_64").unwrap().is_none());

        // Two entries both applying is a manifest error.
        let clash = manifest(&format!(
            "{}{}",
            one("\"https://dl.test/a\"", HELLO_SHA, 11, ""),
            one("\"https://dl.test/b\"", HELLO_SHA, 11, "os = [\"linux\"]")
        ));
        assert!(select(&clash.assets, "linux", "x86_64").unwrap_err().contains("two assets"));
        assert_eq!(select(&clash.assets, "windows", "x86_64").unwrap().len(), 1);
        assert_eq!(env_name("det-model.v2"), "GLOSSA_ASSET_DET_MODEL_V2");
    }

    #[test]
    fn bad_asset_entries_are_rejected_with_a_reason() {
        let parse = |toml: &str| {
            Manifest::parse(&format!(
                "capabilities = [\"translate\"]\n[permissions]\nnetwork = [\"x\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"js\"\n{toml}"
            ))
        };
        assert!(parse(&one("\"https://x/\"", "abc", 11, "")).unwrap_err().contains("64 hex"));
        let placeholder = parse(&one("\"https://x/\"", &"0".repeat(64), 11, "")).unwrap_err();
        assert!(placeholder.contains("placeholder"), "{placeholder}");
        assert!(parse(&one("\"ftp://x/\"", HELLO_SHA, 11, "")).unwrap_err().contains("http"));
        assert!(parse(&one("\"https://x/\"", HELLO_SHA, 0, "")).unwrap_err().contains("size"));
        assert!(parse(&one("\"https://x/\"", HELLO_SHA, 11, "os = [\"amiga\"]")).unwrap_err().contains("amiga"));
        let traversal = one("\"https://x/\"", HELLO_SHA, 11, "").replace("data.bin", "../evil");
        assert!(parse(&traversal).unwrap_err().contains("asset name"));
        let ok = parse(&one("[\"https://x/\"]", HELLO_SHA, 11, "unpack = \"tar.gz\"")).unwrap();
        assert_eq!(ok.assets[0].unpack, Unpack::TarGz);
    }

    fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut out);
            for (name, data) in files {
                let opts =
                    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        out.into_inner()
    }

    fn tar_gz_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, data) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            h[156] = b'0';
            h[257..262].copy_from_slice(b"ustar");
            h[148..156].fill(b' ');
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            tar.extend_from_slice(&h);
            tar.extend_from_slice(data);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn archives_unpack_into_a_folder_and_verify_by_their_files() {
        for (kind, bytes) in [
            ("zip", zip_bytes(&[("model/a.txt", b"alpha"), ("b.txt", b"beta")])),
            ("tar.gz", tar_gz_bytes(&[("model/a.txt", b"alpha"), ("b.txt", b"beta")])),
        ] {
            let tmp = Temp::new(&format!("unpack-{}", kind.replace('.', "")));
            let toml = one("\"https://dl.test/a\"", &sha_hex(&bytes), bytes.len(), &format!("unpack = \"{kind}\""));
            let a = assets(&tmp, &toml);
            a.download(&Bytes(bytes.clone()), &CancelToken::new(), &|_| {}).unwrap();
            let dir = a.path("data.bin");
            assert!(a.ready(), "{kind}");
            assert_eq!(fs::read_to_string(dir.join("model/a.txt")).unwrap(), "alpha");
            assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "beta");
            assert!(!a.part_path("data.bin").exists(), "{kind}: the archive is not kept");
            assert_eq!(a.verify().unwrap()[0].state, AssetState::Ready);
            fs::remove_file(dir.join("b.txt")).unwrap();
            assert_eq!(a.verify().unwrap()[0].state, AssetState::Corrupt, "{kind}");
            assert_eq!(a.status()[0].state, AssetState::Missing);
        }
    }

    #[test]
    fn archives_cannot_write_outside_their_folder() {
        for (kind, bytes) in
            [("zip", zip_bytes(&[("../escape.txt", b"x")])), ("tar.gz", tar_gz_bytes(&[("../escape.txt", b"x")]))]
        {
            let tmp = Temp::new(&format!("escape-{}", kind.replace('.', "")));
            let toml = one("\"https://dl.test/a\"", &sha_hex(&bytes), bytes.len(), &format!("unpack = \"{kind}\""));
            let a = assets(&tmp, &toml);
            let err = a.download(&Bytes(bytes), &CancelToken::new(), &|_| {}).unwrap_err();
            assert!(err.to_string().contains("outside"), "{kind}: {err}");
            assert!(!tmp.0.join("m").join("escape.txt").exists() && !tmp.0.join("escape.txt").exists());
            assert!(!a.ready());
        }
    }

    #[test]
    fn verify_catches_a_changed_file_and_remove_clears_everything() {
        let tmp = Temp::new("verify");
        let a = assets(&tmp, &one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), ""));
        a.download(&MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO), &CancelToken::new(), &|_| {})
            .unwrap();
        // Same size, other bytes: the cheap check cannot tell, the full one can.
        fs::write(a.path("data.bin"), "HELLO WORLD").unwrap();
        assert!(a.ready());
        assert_eq!(a.verify().unwrap()[0].state, AssetState::Corrupt);
        assert!(!a.path("data.bin").exists());
        a.remove().unwrap();
        assert!(!a.root.exists());
    }

    fn gate(tmp: &Temp, auto: bool, made: Arc<std::sync::atomic::AtomicUsize>) -> AssetGate {
        let m = manifest(&one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), ""));
        let a = Arc::new(ModuleAssets::for_manifest(&m, &tmp.0).unwrap().unwrap());
        let make: MakeProvider = Box::new(move || {
            made.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Arc::new(crate::provider::FnProvider::translator("m", |_, r| {
                Ok(Translation::plain(r.text.to_uppercase()))
            })) as Arc<dyn Provider>)
        });
        AssetGate::new(&m, a, auto, make)
    }

    fn translate(p: &dyn Provider, http: &dyn Http) -> Result<Translation> {
        let req = TranslateRequest::new(
            "hi",
            glossa_core::LangPair::new(glossa_core::LangId::AUTO, glossa_core::LangId::ENGLISH),
        );
        crate::provider::with_test_host(http, &Default::default(), |h| p.translate(h, &req))
    }

    #[test]
    fn the_gate_starts_nothing_until_the_files_are_there() {
        let tmp = Temp::new("gate");
        let made = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let g = gate(&tmp, false, made.clone());
        assert_eq!(g.info().id, "m");
        match translate(&g, &MockHttp::new()).unwrap_err() {
            Error::AssetsMissing { module, missing } => {
                assert_eq!((module.as_str(), missing), ("m", vec!["data.bin".into()]))
            }
            other => panic!("expected AssetsMissing, got {other:?}"),
        }
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 0);
        g.warm_up();
        assert!(!g.is_loaded(), "warming up starts nothing");

        // With automatic downloads on, the first call fetches and starts it.
        let made = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let g = gate(&tmp, true, made.clone());
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        assert_eq!(translate(&g, &http).unwrap().text, "HI");
        assert_eq!(translate(&g, &http).unwrap().text, "HI");
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 1, "started once");
        assert_eq!(http.requests().len(), 1);
    }

    #[test]
    fn optional_files_do_not_hold_the_module_back_and_come_by_name() {
        let tmp = Temp::new("optional");
        let toml = format!(
            "{}{}",
            one("\"https://dl.test/req\"", HELLO_SHA, HELLO.len(), ""),
            one("\"https://dl.test/opt\"", HELLO_SHA, HELLO.len(), "optional = true").replace("data.bin", "ru-en")
        );
        let a = assets(&tmp, &toml);
        assert_eq!(a.missing(), ["data.bin"]);
        assert_eq!(a.total_size(), HELLO.len() as u64);
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        a.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert!(a.ready(), "the optional file is not needed");
        assert_eq!(http.requests().len(), 1);
        assert_eq!(a.status()[1].state, AssetState::Missing);
        assert!(a.status()[1].optional);
        a.download_some(&["ru-en".into()], &http, &CancelToken::new(), &|_| {}).unwrap();
        assert_eq!(a.status()[1].state, AssetState::Ready);
        assert_eq!(http.requests().last().unwrap().url, "https://dl.test/opt");
        assert!(a.download_some(&["nope".into()], &http, &CancelToken::new(), &|_| {}).is_err());
    }

    #[test]
    fn a_gzipped_file_is_unpacked_in_place() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(b"#!/bin/sh\necho model\n").unwrap();
        let bytes = gz.finish().unwrap();
        let tmp = Temp::new("gz");
        let toml = one("\"https://dl.test/a\"", &sha_hex(&bytes), bytes.len(), "unpack = \"gz\"\nexecutable = true");
        let a = assets(&tmp, &toml);
        a.download(&Bytes(bytes), &CancelToken::new(), &|_| {}).unwrap();
        assert!(a.ready());
        let path = a.path("data.bin");
        assert_eq!(fs::read_to_string(&path).unwrap(), "#!/bin/sh\necho model\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0, "executable");
        }
        assert_eq!(a.verify().unwrap()[0].state, AssetState::Ready);
        fs::write(&path, "shorter").unwrap();
        assert_eq!(a.verify().unwrap()[0].state, AssetState::Corrupt);
    }

    #[test]
    fn module_ids_cannot_point_the_folder_outside_the_models_folder() {
        for bad in [".", "..", "..."] {
            let err = Manifest::parse(&format!(
                "capabilities = [\"translate\"]\n[module]\nid = \"{bad}\"\nname = \"M\"\nruntime = \"js\"\n"
            ))
            .unwrap_err();
            assert!(err.contains("letter or a digit"), "{bad}: {err}");
            // Even a manifest that skipped validation gets no folder.
            let mut m = manifest(&one("\"https://dl.test/d\"", HELLO_SHA, 11, ""));
            m.module.id = bad.into();
            assert!(ModuleAssets::for_manifest(&m, Path::new("/tmp/models")).is_err(), "{bad}");
        }
        // Deleting a module's files leaves its neighbours alone.
        let tmp = Temp::new("contained");
        let a = assets(&tmp, &one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), ""));
        fs::write(tmp.0.join("neighbour.txt"), "keep").unwrap();
        fs::create_dir_all(a.state_dir()).unwrap();
        a.remove().unwrap();
        assert!(tmp.0.join("neighbour.txt").exists() && !a.root.exists());
        assert!(a.contained(&tmp.0.join("neighbour.txt")).is_err());
        assert!(a.contained(&a.root.join("../neighbour.txt")).is_err());
        assert!(a.contained(&a.root.join("data.bin")).is_ok());
    }

    #[test]
    fn asset_hosts_must_be_in_the_permissions() {
        let err = Manifest::parse(&format!(
            "capabilities = [\"translate\"]\n[permissions]\nnetwork = [\"api.example.com\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"js\"\n{}",
            one("[\"https://api.example.com/a\", \"https://cdn.other.org/a\"]", HELLO_SHA, 11, "")
        ))
        .unwrap_err();
        assert!(err.contains("cdn.other.org") && err.contains("[permissions]"), "{err}");
    }

    #[test]
    fn archives_that_unpack_far_beyond_their_size_are_refused() {
        assert_eq!(unpack_limit(1), 64 << 20);
        assert_eq!(unpack_limit(u64::MAX), 64 << 30);
        // 80 MiB of zeros gzip to a few dozen kilobytes.
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let zeros = vec![0u8; 1 << 20];
        for _ in 0..80 {
            gz.write_all(&zeros).unwrap();
        }
        let bomb = gz.finish().unwrap();
        let tmp = Temp::new("bomb");
        let a = assets(&tmp, &one("\"https://dl.test/a\"", &sha_hex(&bomb), bomb.len(), "unpack = \"gz\""));
        let err = a.download(&Bytes(bomb), &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(err.to_string().contains("unpacks to more than"), "{err}");
        assert!(!a.path("data.bin").exists() && !a.ready());

        // A tar "long name" record claiming megabytes is refused before
        // anything is allocated for it.
        let mut h = [0u8; 512];
        h[..13].copy_from_slice(b"././@LongLink");
        h[124..135].copy_from_slice(format!("{:011o}", 2u64 << 20).as_bytes());
        h[156] = b'L';
        let mut tar = h.to_vec();
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        let bytes = gz.finish().unwrap();
        let tmp = Temp::new("longname");
        let a = assets(&tmp, &one("\"https://dl.test/a\"", &sha_hex(&bytes), bytes.len(), "unpack = \"tar.gz\""));
        let err = a.download(&Bytes(bytes), &CancelToken::new(), &|_| {}).unwrap_err();
        assert!(err.to_string().contains("header record"), "{err}");
    }

    #[test]
    fn writers_to_one_folder_take_turns_across_reloads_and_processes() {
        let tmp = Temp::new("turns");
        let toml = one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), "");
        // Two instances for one folder, as after a reload of the modules.
        let a = assets(&tmp, &toml);
        let b = Arc::new(assets(&tmp, &toml));
        let held = a.exclusive().unwrap();
        assert!(b.is_busy(), "the lock is shared by folder, not by instance");
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = {
            let (b, done) = (b.clone(), done.clone());
            std::thread::spawn(move || {
                let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
                b.download(&http, &CancelToken::new(), &|_| {}).unwrap();
                done.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst), "the second writer waits");
        drop(held);
        worker.join().unwrap();
        assert!(b.ready() && !b.is_busy());

        // Another process holding the lock file keeps a writer waiting too.
        let lock = fs::OpenOptions::new().write(true).open(tmp.0.join(".locks").join("m.lock")).unwrap();
        lock.lock().unwrap();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = {
            let (b, done) = (b.clone(), done.clone());
            std::thread::spawn(move || {
                b.verify().unwrap();
                done.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!done.load(std::sync::atomic::Ordering::SeqCst), "the file lock holds it back");
        lock.unlock().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn a_part_left_from_an_older_version_is_not_resumed() {
        const NEW: &str = "HELLO WORLD";
        let tmp = Temp::new("stale");
        let old = assets(&tmp, &one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), ""));
        let http = MockHttp::new().on_once(Method::Get, "https://dl.test/", 200, "hello ");
        assert!(old.download(&http, &CancelToken::new(), &|_| {}).is_err());
        assert_eq!(old.status()[0].state, AssetState::Partial { have: 6 });

        // The module is updated: same name and size, another file.
        let new = assets(&tmp, &one("\"https://dl.test/d\"", &sha_hex(NEW.as_bytes()), NEW.len(), ""));
        assert_eq!(new.status()[0].state, AssetState::Missing, "the old part is not this file's");
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, NEW);
        new.download(&http, &CancelToken::new(), &|_| {}).unwrap();
        assert!(http.requests()[0].headers.iter().all(|(n, _)| n != "Range"), "nothing resumed");
        assert_eq!(fs::read_to_string(new.path("data.bin")).unwrap(), NEW);
        assert!(!old.part_path("data.bin").exists(), "the stale part was cleared");
    }

    #[test]
    fn the_host_call_hands_out_only_files_verified_for_this_manifest() {
        let tmp = Temp::new("hostinfo");
        let toml = one("\"https://dl.test/d\"", HELLO_SHA, HELLO.len(), "optional = true");
        let a = assets(&tmp, &toml);
        assert_eq!(a.host_info().missing, ["data.bin"]);
        let http = MockHttp::new().on(Method::Get, "https://dl.test/", 200, HELLO);
        a.download_some(&["data.bin".into()], &http, &CancelToken::new(), &|_| {}).unwrap();
        let info = a.host_info();
        assert!(info.missing.is_empty() && info.files.contains_key("data.bin"));
        // The manifest now names another file: the old one is not handed out.
        let changed = assets(&tmp, &toml.replace(HELLO_SHA, &sha_hex(b"HELLO WORLD")));
        let info = changed.host_info();
        assert_eq!((info.files.len(), info.missing), (0, vec!["data.bin".to_owned()]));
    }
}
