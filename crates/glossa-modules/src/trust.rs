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
//! approved request. A module whose request is [`within`](TrustRequest::within)
//! its grant runs: an unchanged manifest (same hash) or a narrower one. A
//! wider one (a new host, a new download, a different program) waits for
//! approval again: the registry lists it behind a [`PendingApproval`]
//! provider whose every call fails with [`Error::NeedsApproval`].
//!
//! Modules shipped with Glossa (compiled in, or in the folders beside the
//! program) and the services of a classic installation the user pointed
//! Glossa at are trusted and never asked about ([`TrustPolicy::trusted_dirs`]).

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
        hex(&Sha256::digest(text.as_bytes()))
    }

    /// The grant that approves exactly this request.
    pub fn to_grant(&self) -> ModuleGrant {
        ModuleGrant {
            hash: self.hash(),
            runtime: runtime_name(self.runtime).into(),
            program: self.program.clone(),
            network: self.network.clone(),
            assets: self.assets.iter().map(AssetRequest::key).collect(),
            packages: self.packages.clone(),
            actions: self.actions.clone(),
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
    /// for tools that test one module, and for the one-time migration that
    /// grants modules installed before approvals existed.
    pub granted: Option<Arc<BTreeMap<String, ModuleGrant>>>,
    /// Module folders whose modules need no approval: the ones shipped with
    /// Glossa and a classic installation's services.
    pub trusted_dirs: Vec<PathBuf>,
}

impl TrustPolicy {
    /// Whether the module at `path` (a folder or archive inside a modules
    /// folder) comes from a trusted folder.
    pub fn is_trusted(&self, path: &Path) -> bool {
        path.parent().is_some_and(|p| self.trusted_dirs.iter().any(|d| d == p))
    }

    /// Whether module `id` asking for `request` may run.
    pub fn allows(&self, id: &str, request: &TrustRequest) -> bool {
        match &self.granted {
            None => true,
            Some(g) => g.get(id).is_some_and(|grant| request.within(grant)),
        }
    }
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
        let granted = request("[permissions]\nnetwork = [\"api.example.com\", \"*.cdn.example\"]").to_grant();
        assert!(request("[permissions]\nnetwork = [\"api.example.com\"]").within(&granted), "fewer hosts");
        assert!(request("[permissions]\nnetwork = [\"a.cdn.example\"]").within(&granted), "inside a wildcard");
        assert!(request("").within(&granted), "no network at all");
        assert!(!request("[permissions]\nnetwork = [\"evil.example\"]").within(&granted), "a new host");
        assert!(!request("[permissions]\nnetwork = [\"*\"]").within(&granted), "any website");
        assert!(!request("[permissions]\nnetwork = [\"*.example.com\"]").within(&granted), "a wider wildcard");

        let with_actions = request("[[actions]]\nid = \"test\"\n").to_grant();
        assert!(!request("[[actions]]\nid = \"test\"\n[[actions]]\nid = \"wipe\"\n").within(&with_actions));
        assert!(request("").within(&with_actions));

        let plain = request("").to_grant();
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
        let granted = request(&asset(a, "dl.test")).to_grant();
        assert!(request(&asset(a, "dl.test")).within(&granted));
        assert!(!request(&asset(b, "dl.test")).within(&granted), "another file");
        let r = request(&asset(b, "dl.test"));
        let extra = r.beyond(Some(&granted));
        assert_eq!(extra.assets.len(), 1);
        assert!(extra.network.is_empty(), "the host was approved before");
        assert_eq!(r.assets[0].hosts, ["dl.test"]);
    }

    #[test]
    fn the_policy_trusts_its_folders_and_its_grants() {
        let r = request("[permissions]\nnetwork = [\"api.example.com\"]");
        let mut granted = BTreeMap::new();
        let policy = TrustPolicy {
            granted: Some(Arc::new(granted.clone())),
            trusted_dirs: vec![PathBuf::from("/opt/glossa/modules")],
        };
        assert!(!policy.allows("m", &r));
        assert!(policy.is_trusted(Path::new("/opt/glossa/modules/m")));
        assert!(!policy.is_trusted(Path::new("/home/u/modules/m")));
        granted.insert("m".to_owned(), r.to_grant());
        let policy = TrustPolicy { granted: Some(Arc::new(granted)), ..policy };
        assert!(policy.allows("m", &r));
        assert!(!policy.allows("other", &r), "grants are per module id");
        assert!(TrustPolicy::default().allows("m", &r), "no policy approves everything");
    }
}
