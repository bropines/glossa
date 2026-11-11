//! Approving what a module installed by hand may do.
//!
//! A module dropped into a modules folder, installed from a zip or a
//! `.potext`, or updated in place does not run until the user has approved
//! what it asks for: the hosts it reaches, whether it runs a program with
//! the user's rights (and which), the files it downloads, the Python
//! packages it installs, its actions, and whether it is a native library
//! that runs inside Glossa. That request is a [`TrustRequest`], built from
//! the manifest alone, before anything of the module runs.
//!
//! What the user approved is a [`ModuleGrant`] in `config.toml`
//! (`[modules.granted.<id>]`), keyed by module id and carrying a hash of the
//! approved request and a hash of the module's files ([`content_hash`]). A
//! module whose request is [`within`](TrustRequest::within) its grant and
//! whose files are the approved ones runs. A wider request (a new host, a
//! new download, a different program) or a changed file waits for approval
//! again: the registry lists it behind a [`PendingApproval`] provider whose
//! every call fails with [`Error::NeedsApproval`].
//!
//! Modules shipped with Glossa (compiled in, or in the folders beside the
//! program) and the services of a classic installation the user pointed
//! Glossa at are trusted and never asked about ([`TrustPolicy::trusted_dirs`]).
//! Nothing else is: a module found in a folder the user can write to waits
//! however empty the list of approvals is, so a first start approves
//! nothing by itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glossa_core::config::ModuleGrant;
use glossa_core::{Error, ProviderInfo, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::manifest::{url_host, Manifest, Runtime};
use crate::provider::{Host, OptionSpec, Provider};

/// How far outside the sandbox a module reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// JavaScript: no files, no programs, only the network it declares.
    Sandboxed,
    /// Python, Java or any program: runs with the user's rights, so it can
    /// read and change the user's files.
    Program,
    /// A shared library loaded into Glossa itself: full trust.
    Native,
}

/// One file a module declares for download, as the prompt shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetRequest {
    pub name: String,
    pub size: u64,
    /// The hosts it is downloaded from (several for mirrors).
    pub hosts: Vec<String>,
    pub sha256: String,
    pub optional: bool,
}

impl AssetRequest {
    /// The form kept in a grant: everything that changes what is fetched.
    fn key(&self) -> String {
        format!("{} {} bytes from {} sha256:{}", self.name, self.size, self.hosts.join(","), self.sha256)
    }
}

/// What a module asks to be allowed to do: the permission-relevant parts of
/// its manifest, normalised (sorted, deduplicated, lowercased hosts) so
/// equal requests hash equally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustRequest {
    pub runtime: Runtime,
    pub reach: Reach,
    /// The program a process module runs, with its arguments and
    /// environment (`KEY=VALUE` entries); empty for sandboxed modules.
    pub program: Vec<String>,
    pub network: Vec<String>,
    pub assets: Vec<AssetRequest>,
    /// Python requirement lines, then `index <url>` for where they come from.
    pub packages: Vec<String>,
    /// Action ids.
    pub actions: Vec<String>,
    /// Permission lines other features contribute (`"panel main"`, `"tray
    /// Open Acme"`, `"hotkey …"`): the extension point for what later
    /// manifest sections add, without changing the hash of modules that do
    /// not use them. See [`extra_lines`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
}

/// The permission lines a manifest contributes beyond the fixed kinds
/// above: each feature that gives a module a new way to reach the user or
/// the system (panels, tray and menu entries, hotkeys) adds its lines here
/// as `"<kind> <what>"`. They are hashed only when present, so modules
/// without them keep their hash, and a new line asks again like a new host.
pub fn extra_lines(manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    // Panels: a window of the module's own, and where Glossa offers it.
    // (Global hotkeys for panels are the user's own, in `[hotkeys.panels]`;
    // a module cannot declare one.)
    for p in &manifest.ui.panels {
        out.push(format!("panel {}", p.id));
        for place in &p.open_from {
            match place.as_str() {
                "tray" => out.push(format!("tray {}", p.id)),
                "main_menu" => out.push(format!("menu {}", p.id)),
                _ => {}
            }
        }
    }
    out
}

impl TrustRequest {
    /// What the manifest in `dir` asks for. Only the assets for this system
    /// count, since only those can be downloaded here. Reading the Python
    /// requirements may read `requirements.txt` or `pyproject.toml` beside
    /// the manifest; one that cannot be read asks for nothing here, and the
    /// module fails to load for that reason instead.
    pub fn for_manifest(manifest: &Manifest, dir: &Path) -> TrustRequest {
        let m = &manifest.module;
        let runtime = m.runtime;
        let reach = reach_of(runtime);
        let mut program = Vec::new();
        match runtime {
            Runtime::Process => {
                program.extend(m.command.as_ref().and_then(|c| c.for_this_platform()).unwrap_or_default());
                program.extend(m.args.iter().cloned());
            }
            Runtime::Python => {
                program.push("python".into());
                program.push(or(&m.entry, "main.py"));
                program.extend(m.args.iter().cloned());
            }
            Runtime::Java => {
                program.push("java".into());
                program.extend(m.jvm_args.iter().cloned());
                if m.main_class.trim().is_empty() {
                    program.push("-jar".into());
                    program.push(or(&m.entry, "module.jar"));
                } else {
                    program.push("-cp".into());
                    program.push(m.classpath.join(if cfg!(windows) { ";" } else { ":" }));
                    program.push(m.main_class.trim().to_owned());
                }
                program.extend(m.args.iter().cloned());
            }
            Runtime::Native => program.push(or(&m.entry, &m.id)),
            Runtime::Js | Runtime::Qtranslate | Runtime::Pot => {}
        }
        if reach != Reach::Sandboxed {
            program.extend(m.env.iter().map(|(k, v)| format!("{k}={v}")));
            if !m.cwd.trim().is_empty() {
                program.push(format!("cwd={}", m.cwd.trim()));
            }
        }
        let assets = manifest
            .assets
            .iter()
            .filter(|a| a.applies_to(std::env::consts::OS, std::env::consts::ARCH))
            .map(|a| {
                let mut hosts: Vec<String> = a.url.list().iter().filter_map(|u| url_host(u)).collect();
                hosts.sort();
                hosts.dedup();
                AssetRequest {
                    name: a.name.clone(),
                    size: a.size,
                    hosts,
                    sha256: a.sha256.to_ascii_lowercase(),
                    optional: a.optional,
                }
            })
            .collect();
        let mut packages = Vec::new();
        if runtime == Runtime::Python {
            if let Ok(Some(r)) = crate::python_env::Requirements::find(dir, manifest) {
                packages.extend(r.lines);
                packages.push(format!("index {}", r.index));
            }
        }
        TrustRequest {
            runtime,
            reach,
            program,
            network: manifest.permissions.network.clone(),
            assets,
            packages,
            actions: manifest.actions.iter().map(|a| a.id.clone()).collect(),
            extra: extra_lines(manifest),
        }
        .normalised()
    }

    /// The request of a module without a manifest of its own (a classic
    /// script, a pot-app plugin): those always had any website.
    pub fn unrestricted_script(runtime: Runtime) -> TrustRequest {
        TrustRequest {
            runtime,
            reach: reach_of(runtime),
            program: Vec::new(),
            network: vec!["*".into()],
            assets: Vec::new(),
            packages: Vec::new(),
            actions: Vec::new(),
            extra: Vec::new(),
        }
    }

    fn normalised(mut self) -> Self {
        for h in &mut self.network {
            *h = h.trim().to_ascii_lowercase();
        }
        self.network.retain(|h| !h.is_empty());
        self.network.sort();
        self.network.dedup();
        self.assets.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.sha256.cmp(&b.sha256)));
        self.assets.dedup();
        self.packages.sort();
        self.packages.dedup();
        self.actions.sort();
        self.actions.dedup();
        self.extra.sort();
        self.extra.dedup();
        self
    }

    /// SHA-256 of the request in a fixed text form, in hex. Stable across
    /// versions of Glossa as long as the form below is: changing it would
    /// make every module ask again, so add new lines only for new kinds of
    /// permission, and only when present.
    pub fn hash(&self) -> String {
        let mut text = format!("runtime {}\n", runtime_name(self.runtime));
        for p in &self.program {
            text.push_str(&format!("program {p}\n"));
        }
        for h in &self.network {
            text.push_str(&format!("network {h}\n"));
        }
        for a in &self.assets {
            text.push_str(&format!("asset {}\n", a.key()));
        }
        for p in &self.packages {
            text.push_str(&format!("package {p}\n"));
        }
        for a in &self.actions {
            text.push_str(&format!("action {a}\n"));
        }
        for e in &self.extra {
            text.push_str(&format!("extra {e}\n"));
        }
        hex(&Sha256::digest(text.as_bytes()))
    }

    /// The grant that approves exactly this request for the module in
    /// `dir` (see [`canonical`]) with the files it holds now
    /// ([`content_hash`]).
    pub fn to_grant(&self, dir: &Path) -> ModuleGrant {
        ModuleGrant {
            hash: self.hash(),
            runtime: runtime_name(self.runtime).into(),
            program: self.program.clone(),
            network: self.network.clone(),
            assets: self.assets.iter().map(AssetRequest::key).collect(),
            packages: self.packages.clone(),
            actions: self.actions.clone(),
            extra: self.extra.clone(),
            dir: canonical(dir).to_string_lossy().into_owned(),
            content: content_hash(dir),
        }
    }

    /// Whether `grant` already covers everything this asks for: the same
    /// hash, or a narrower request (fewer hosts, downloads, packages or
    /// actions, with the same runtime and program).
    pub fn within(&self, grant: &ModuleGrant) -> bool {
        if grant.hash == self.hash() {
            return true;
        }
        grant.runtime == runtime_name(self.runtime)
            && grant.program == self.program
            && self.network.iter().all(|h| host_covered(h, &grant.network))
            && self.assets.iter().all(|a| grant.assets.contains(&a.key()))
            && self.packages.iter().all(|p| grant.packages.contains(p))
            && self.actions.iter().all(|a| grant.actions.contains(a))
            && self.extra.iter().all(|e| grant.extra.contains(e))
    }

    /// What this asks for beyond `grant`, for the prompt to point out on an
    /// update (all of it when there is no grant).
    pub fn beyond(&self, grant: Option<&ModuleGrant>) -> TrustRequest {
        let Some(g) = grant else { return self.clone() };
        let same_program = g.runtime == runtime_name(self.runtime) && g.program == self.program;
        TrustRequest {
            runtime: self.runtime,
            reach: self.reach,
            program: if same_program { Vec::new() } else { self.program.clone() },
            network: self.network.iter().filter(|h| !host_covered(h, &g.network)).cloned().collect(),
            assets: self.assets.iter().filter(|a| !g.assets.contains(&a.key())).cloned().collect(),
            packages: self.packages.iter().filter(|p| !g.packages.contains(p)).cloned().collect(),
            actions: self.actions.iter().filter(|a| !g.actions.contains(a)).cloned().collect(),
            extra: self.extra.iter().filter(|e| !g.extra.contains(e)).cloned().collect(),
        }
    }
}

fn or(value: &str, default: &str) -> String {
    match value.trim() {
        "" => default.to_owned(),
        v => v.to_owned(),
    }
}

fn reach_of(runtime: Runtime) -> Reach {
    match runtime {
        Runtime::Js | Runtime::Qtranslate | Runtime::Pot => Reach::Sandboxed,
        Runtime::Python | Runtime::Java | Runtime::Process => Reach::Program,
        Runtime::Native => Reach::Native,
    }
}

/// The runtime as `module.toml` spells it.
pub fn runtime_name(r: Runtime) -> &'static str {
    match r {
        Runtime::Js => "js",
        Runtime::Python => "python",
        Runtime::Native => "native",
        Runtime::Process => "process",
        Runtime::Java => "java",
        Runtime::Qtranslate => "qtranslate",
        Runtime::Pot => "pot",
    }
}

/// `path` with links and `..` resolved, so the same folder compares equal
/// however it was reached; as given when it does not exist.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// Files the system writes into a folder the user merely looked at; never
/// read by a module runtime, so they do not count as the module's content.
const IGNORED_FILES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

/// SHA-256, in hex, over every file in the module folder `dir`: each
/// file's path relative to the folder, its length and its bytes, in path
/// order, through sub-folders. A link counts as where it points, without
/// being followed, and a file that cannot be read as that fact, so
/// swapping either changes the hash. Left out are only the files in
/// [`IGNORED_FILES`] and a `.git` folder, which no runtime executes.
///
/// This is what binds an approval to what was approved: the manifest says
/// what a module may do, the files are what does it.
pub fn content_hash(dir: &Path) -> String {
    let mut files = Vec::new();
    collect_files(dir, "", &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    for (rel, path) in files {
        let meta = std::fs::symlink_metadata(&path);
        if meta.as_ref().is_ok_and(|m| m.file_type().is_symlink()) {
            let target = std::fs::read_link(&path).unwrap_or_default();
            hasher.update(format!("link {rel}\0{}\n", target.display()).as_bytes());
            continue;
        }
        let opened = std::fs::File::open(&path).and_then(|f| Ok((f.metadata()?.len(), f)));
        let Ok((len, mut file)) = opened else {
            hasher.update(format!("unreadable {rel}\0\n").as_bytes());
            continue;
        };
        hasher.update(format!("file {rel}\0{len}\n").as_bytes());
        loop {
            match std::io::Read::read(&mut file, &mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(_) => {
                    hasher.update(b"\0unreadable\n");
                    break;
                }
            }
        }
    }
    hex(&hasher.finalize())
}

/// Every file under `dir` as (path relative to the module folder with `/`
/// between its parts, full path). Links are listed, not followed.
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let is_dir = item.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            if !(prefix.is_empty() && name == ".git") {
                collect_files(&item.path(), &rel, out);
            }
        } else if !IGNORED_FILES.contains(&name.as_str()) {
            out.push((rel, item.path()));
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the host pattern `wanted` (`api.x.com`, `*.x.com`, `*`) asks for
/// nothing the `granted` patterns do not already allow.
fn host_covered(wanted: &str, granted: &[String]) -> bool {
    granted.iter().any(|g| {
        let g = g.trim().to_ascii_lowercase();
        if g == "*" || g == wanted {
            return true;
        }
        match g.strip_prefix("*.") {
            Some(suffix) if wanted != "*" => {
                // `*.x.com` allows `x.com` itself too, as the network check does.
                let host = wanted.strip_prefix("*.").unwrap_or(wanted);
                host == suffix || host.ends_with(&format!(".{suffix}"))
            }
            _ => false,
        }
    })
}

/// How the registry decides whether a module may run.
#[derive(Clone, Default)]
pub struct TrustPolicy {
    /// What the user approved, by module id. `None` approves everything:
    /// only for tools that test one module the developer pointed them at.
    /// An empty map approves nothing.
    pub granted: Option<Arc<BTreeMap<String, ModuleGrant>>>,
    /// Ids of modules known to have run before approvals existed, for the
    /// one-time upgrade: a module with one of these ids and no grant loads
    /// once more, and the program then records its grant. Empty otherwise,
    /// and always on a fresh install.
    pub legacy: Vec<String>,
    /// Module folders whose modules need no approval: the ones shipped with
    /// Glossa. Compared canonically.
    pub trusted_dirs: Vec<PathBuf>,
    /// Folders whose classic `Service.js` scripts need no approval (a
    /// classic installation the user pointed Glossa at). A module with a
    /// `module.toml` found there is asked about as anywhere else.
    pub trusted_script_dirs: Vec<PathBuf>,
}

impl TrustPolicy {
    /// Whether the module at `path` (a folder or archive inside a modules
    /// folder) comes from a folder shipped with Glossa.
    pub fn is_trusted(&self, path: &Path) -> bool {
        in_any(path, &self.trusted_dirs)
    }

    /// Whether the classic script at `path` needs no approval.
    pub fn is_trusted_script(&self, path: &Path) -> bool {
        self.is_trusted(path) || in_any(path, &self.trusted_script_dirs)
    }

    /// Whether module `id` in `dir` asking for `request` may run: its grant
    /// covers the request, was given to this folder, and the folder holds
    /// the files it held then.
    pub fn allows(&self, id: &str, dir: &Path, request: &TrustRequest) -> bool {
        self.verdict(id, dir, request) == Verdict::Allowed
    }

    /// [`Self::allows`], with why not. The files are only hashed for a
    /// module whose grant otherwise covers it.
    pub fn verdict(&self, id: &str, dir: &Path, request: &TrustRequest) -> Verdict {
        let Some(granted) = &self.granted else { return Verdict::Allowed };
        match granted.get(id) {
            Some(grant) => {
                let same_folder = grant.dir.is_empty() || Path::new(&grant.dir) == canonical(dir);
                if !(same_folder && request.within(grant)) {
                    Verdict::Unapproved
                } else if grant.content.is_empty() || grant.content == content_hash(dir) {
                    // An empty hash: a grant from before files were hashed,
                    // kept working; the engine binds it on this load.
                    Verdict::Allowed
                } else {
                    Verdict::Changed
                }
            }
            None if self.legacy.iter().any(|l| l == id) => Verdict::Allowed,
            None => Verdict::Unapproved,
        }
    }
}

/// What [`TrustPolicy::verdict`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    /// No grant, a grant for another folder, or a request beyond the grant.
    Unapproved,
    /// The grant covers the request, but the module's files are not the
    /// ones approved.
    Changed,
}

fn in_any(path: &Path, dirs: &[PathBuf]) -> bool {
    let Some(parent) = path.parent() else { return false };
    let parent = canonical(parent);
    dirs.iter().any(|d| canonical(d) == parent)
}

/// Where a pending module's settings page comes from.
enum PageSource {
    Manifest(Box<Manifest>, crate::settings::PageCache),
    /// A classic script or pot-app plugin, already set up but never called.
    Provider(Arc<dyn Provider>),
}

/// Stands in for a module waiting for approval: listed with its name,
/// capabilities and settings, but every call fails with
/// [`Error::NeedsApproval`] and nothing of the module runs.
pub struct PendingApproval {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
    page: PageSource,
}

impl PendingApproval {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            info: manifest.provider_info(),
            options: manifest.options.clone(),
            page: PageSource::Manifest(Box::new(manifest.clone()), Default::default()),
        }
    }

    /// For a sandboxed script that was set up to learn its id and settings.
    pub fn wrapping(provider: Arc<dyn Provider>) -> Self {
        provider.suspend();
        Self {
            info: provider.info().clone(),
            options: provider.options().to_vec(),
            page: PageSource::Provider(provider),
        }
    }

    fn refuse(&self) -> Error {
        Error::NeedsApproval { module: self.info.id.clone() }
    }
}

impl Provider for PendingApproval {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn settings_page(&self, lang: &str) -> Arc<crate::settings::SettingsPage> {
        match &self.page {
            PageSource::Manifest(m, cache) => cache.get(lang, || m.settings_page(lang)),
            PageSource::Provider(p) => p.settings_page(lang),
        }
    }

    fn translate(&self, _: &Host, _: &glossa_core::TranslateRequest) -> Result<glossa_core::Translation> {
        Err(self.refuse())
    }

    fn list_models(&self, _: &Host) -> Result<Vec<String>> {
        Err(self.refuse())
    }

    fn detect(&self, _: &Host, _: &str) -> Result<glossa_core::Detection> {
        Err(self.refuse())
    }

    fn dictionary(&self, _: &Host, _: &glossa_core::DictRequest) -> Result<glossa_core::Article> {
        Err(self.refuse())
    }

    fn speak(&self, _: &Host, _: &glossa_core::SpeakRequest) -> Result<glossa_core::Audio> {
        Err(self.refuse())
    }

    fn recognize(&self, _: &Host, _: &glossa_core::OcrRequest) -> Result<glossa_core::OcrResult> {
        Err(self.refuse())
    }

    fn action(
        &self,
        _: &Host,
        _: glossa_module_sdk::wire::ActionParams,
    ) -> Result<glossa_module_sdk::wire::ActionReply> {
        Err(self.refuse())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: &str) -> Manifest {
        Manifest::parse(&format!(
            "capabilities = [\"translate\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"js\"\n{extra}"
        ))
        .unwrap()
    }

    fn request(perms: &str) -> TrustRequest {
        TrustRequest::for_manifest(&manifest(perms), Path::new("/nowhere"))
    }

    #[test]
    fn the_hash_is_stable_and_ignores_order_and_case() {
        let a = request("[permissions]\nnetwork = [\"api.example.com\", \"cdn.example.com\"]");
        let b = request("[permissions]\nnetwork = [\"CDN.example.com\", \"api.example.com\", \"api.example.com\"]");
        assert_eq!(a.hash(), b.hash());
        // Pinned: a change here would make every installed module ask again.
        assert_eq!(a.hash(), "7ddaed6e49bab57e3710cd19f43274ee7472ff8da66b77a8419da9f96b6ab157");
        assert_ne!(a.hash(), request("[permissions]\nnetwork = [\"api.example.com\"]").hash());
    }

    #[test]
    fn widening_asks_again_and_narrowing_does_not() {
        let granted =
            request("[permissions]\nnetwork = [\"api.example.com\", \"*.cdn.example\"]").to_grant(Path::new("/m"));
        assert!(request("[permissions]\nnetwork = [\"api.example.com\"]").within(&granted), "fewer hosts");
        assert!(request("[permissions]\nnetwork = [\"a.cdn.example\"]").within(&granted), "inside a wildcard");
        assert!(request("").within(&granted), "no network at all");
        assert!(!request("[permissions]\nnetwork = [\"evil.example\"]").within(&granted), "a new host");
        assert!(!request("[permissions]\nnetwork = [\"*\"]").within(&granted), "any website");
        assert!(!request("[permissions]\nnetwork = [\"*.example.com\"]").within(&granted), "a wider wildcard");

        let with_actions = request("[[actions]]\nid = \"test\"\n").to_grant(Path::new("/m"));
        assert!(!request("[[actions]]\nid = \"test\"\n[[actions]]\nid = \"wipe\"\n").within(&with_actions));
        assert!(request("").within(&with_actions));

        let plain = request("").to_grant(Path::new("/m"));
        let as_python = TrustRequest::for_manifest(
            &Manifest::parse("capabilities = [\"translate\"]\n[module]\nid = \"m\"\nname = \"M\"\nruntime = \"python\"\nrequirements = [\"requests\"]")
                .unwrap(),
            Path::new("/nowhere"),
        );
        assert_eq!(as_python.reach, Reach::Program);
        assert!(!as_python.within(&plain), "running a program instead of a script");
        assert_eq!(as_python.packages, ["index https://pypi.org/simple", "requests"]);
    }

    #[test]
    fn new_or_changed_downloads_ask_again() {
        let asset = |sha: &str, host: &str| {
            format!("[permissions]\nnetwork = [\"{host}\"]\n[[assets]]\nname = \"model.bin\"\nurl = \"https://{host}/m\"\nsha256 = \"{sha}\"\nsize = 11\n")
        };
        let a = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let b = "a94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let granted = request(&asset(a, "dl.test")).to_grant(Path::new("/m"));
        assert!(request(&asset(a, "dl.test")).within(&granted));
        assert!(!request(&asset(b, "dl.test")).within(&granted), "another file");
        let r = request(&asset(b, "dl.test"));
        let extra = r.beyond(Some(&granted));
        assert_eq!(extra.assets.len(), 1);
        assert!(extra.network.is_empty(), "the host was approved before");
        assert_eq!(r.assets[0].hosts, ["dl.test"]);

        // Mirrors are sorted, so reordering them changes nothing.
        let mirrors = |urls: &str| {
            request(&format!(
                "[permissions]\nnetwork = [\"a.test\", \"b.test\"]\n[[assets]]\nname = \"m\"\nurl = [{urls}]\nsha256 = \"{a}\"\nsize = 1\n"
            ))
        };
        let one = mirrors("\"https://b.test/m\", \"https://a.test/m\", \"https://b.test/n\"");
        let two = mirrors("\"https://a.test/m\", \"https://b.test/m\"");
        assert_eq!(one.assets[0].hosts, ["a.test", "b.test"]);
        assert_eq!(one.hash(), two.hash());
    }

    #[test]
    fn panels_and_their_menu_entries_are_asked_for() {
        let plain = request("[permissions]\nnetwork = [\"api.example.com\"]");
        let panel = "\n[[ui.panels]]\nid = \"main\"\nopen_from = [\"tray\", \"main_menu\"]\nlayout = [\"a\"]\n";
        let with = request(&format!("[permissions]\nnetwork = [\"api.example.com\"]{panel}"));
        assert_eq!(with.extra, ["menu main", "panel main", "tray main"]);
        // An approved module that adds a panel asks again, for the panel.
        let grant = plain.to_grant(Path::new("/m"));
        assert!(!with.within(&grant));
        assert_eq!(with.beyond(Some(&grant)).extra.len(), 3);
        assert_ne!(with.hash(), plain.hash());
        assert!(plain.extra.is_empty(), "no panel, no lines, the hash unchanged");
    }

    #[test]
    fn extra_permission_lines_count_only_when_present() {
        let plain = request("[permissions]\nnetwork = [\"api.example.com\"]");
        let mut with_panel = plain.clone();
        with_panel.extra = vec!["panel main".into()];
        assert_eq!(plain.hash(), request("[permissions]\nnetwork = [\"api.example.com\"]").hash());
        assert_ne!(plain.hash(), with_panel.hash());
        assert!(!with_panel.within(&plain.to_grant(Path::new("/m"))), "a new line asks again");
        assert!(plain.within(&with_panel.to_grant(Path::new("/m"))), "dropping one does not");
        assert_eq!(with_panel.beyond(Some(&plain.to_grant(Path::new("/m")))).extra, ["panel main"]);
    }

    #[test]
    fn the_policy_trusts_its_folders_and_its_grants() {
        let r = request("[permissions]\nnetwork = [\"api.example.com\"]");
        let mut granted = BTreeMap::new();
        let policy = TrustPolicy {
            granted: Some(Arc::new(granted.clone())),
            trusted_dirs: vec![PathBuf::from("/opt/glossa/modules")],
            trusted_script_dirs: vec![PathBuf::from("/classic/Services")],
            legacy: Vec::new(),
        };
        assert!(!policy.allows("m", Path::new("/m"), &r));
        assert!(policy.is_trusted(Path::new("/opt/glossa/modules/m")));
        assert!(!policy.is_trusted(Path::new("/home/u/modules/m")));
        granted.insert("m".to_owned(), r.to_grant(Path::new("/m")));
        let policy = TrustPolicy { granted: Some(Arc::new(granted)), ..policy };
        assert!(policy.allows("m", Path::new("/m"), &r));
        assert!(!policy.allows("other", Path::new("/m"), &r), "grants are per module id");
        assert!(!policy.allows("m", Path::new("/elsewhere/m"), &r), "and per folder");
        assert!(policy.is_trusted_script(Path::new("/classic/Services/Bing")));
        assert!(!policy.is_trusted(Path::new("/classic/Services/acme")), "manifest modules there are asked about");
        assert!(TrustPolicy::default().allows("m", Path::new("/m"), &r), "no policy approves everything");
    }

    #[test]
    fn an_empty_list_of_approvals_approves_nothing() {
        let r = request("");
        let empty = TrustPolicy { granted: Some(Arc::new(BTreeMap::new())), ..Default::default() };
        assert_eq!(empty.verdict("m", Path::new("/m"), &r), Verdict::Unapproved);
        // Only an id known from before approvals existed loads without one.
        let upgrading = TrustPolicy { legacy: vec!["m".into()], ..empty };
        assert!(upgrading.allows("m", Path::new("/m"), &r));
        assert!(!upgrading.allows("other", Path::new("/m"), &r));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glossa-content-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("module.toml"), "x").unwrap();
        std::fs::write(dir.join("main.js"), "function translate() {}").unwrap();
        std::fs::write(dir.join("lib").join("util.js"), "1").unwrap();
        dir
    }

    #[test]
    fn the_content_hash_covers_every_file_and_nothing_the_system_writes() {
        let dir = scratch("hash");
        let before = content_hash(&dir);
        assert_eq!(before.len(), 64);
        assert_eq!(before, content_hash(&dir), "stable");
        std::fs::write(dir.join(".DS_Store"), "finder").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git").join("HEAD"), "ref").unwrap();
        assert_eq!(before, content_hash(&dir), "looking at the folder changes nothing");

        std::fs::write(dir.join("lib").join("util.js"), "2").unwrap();
        let edited = content_hash(&dir);
        assert_ne!(before, edited, "a changed file in a sub-folder");
        std::fs::write(dir.join("lib").join("util.js"), "1").unwrap();
        assert_eq!(before, content_hash(&dir), "and back");
        std::fs::write(dir.join("extra.js"), "").unwrap();
        assert_ne!(before, content_hash(&dir), "a new file, even an empty one");
        std::fs::remove_file(dir.join("extra.js")).unwrap();
        std::fs::rename(dir.join("main.js"), dir.join("main2.js")).unwrap();
        assert_ne!(before, content_hash(&dir), "a renamed file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_boundaries_cannot_be_shifted() {
        // Moving bytes from the end of one file to the start of the next
        // must not hash the same.
        let a = scratch("shift-a");
        let b = scratch("shift-b");
        std::fs::write(a.join("a.js"), "one").unwrap();
        std::fs::write(a.join("b.js"), "two").unwrap();
        std::fs::write(b.join("a.js"), "on").unwrap();
        std::fs::write(b.join("b.js"), "etwo").unwrap();
        assert_ne!(content_hash(&a), content_hash(&b));
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn changed_files_ask_again_and_older_grants_keep_working() {
        let dir = scratch("verdict");
        let r = request("[permissions]\nnetwork = [\"api.example.com\"]");
        let grant = r.to_grant(&dir);
        assert_eq!(grant.content, content_hash(&dir));
        let policy = |g: &ModuleGrant| TrustPolicy {
            granted: Some(Arc::new(BTreeMap::from([("m".to_owned(), g.clone())]))),
            ..Default::default()
        };
        assert_eq!(policy(&grant).verdict("m", &dir, &r), Verdict::Allowed);

        // The manifest asks for nothing new, but the code is another.
        std::fs::write(dir.join("main.js"), "function translate() { steal(); }").unwrap();
        assert_eq!(policy(&grant).verdict("m", &dir, &r), Verdict::Changed);
        assert!(!policy(&grant).allows("m", &dir, &r));

        // A grant written before files were hashed has no content hash.
        let old = ModuleGrant { content: String::new(), ..grant };
        assert_eq!(policy(&old).verdict("m", &dir, &r), Verdict::Allowed);
        let text = toml::to_string(&old).unwrap();
        assert!(!text.contains("content ="), "{text}");
        let back: ModuleGrant = toml::from_str(&text).unwrap();
        assert_eq!(back, old);
        std::fs::remove_dir_all(&dir).ok();
    }
}
