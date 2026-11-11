//! `module.toml`: what a module is and what it may do.
//!
//! ```toml
//! [module]
//! id = "my-translator"          # lowercase, hyphenated, unique
//! name = "My Translator"
//! version = "1.0.0"
//! api = 1                       # module protocol major version
//! runtime = "js"                # js | python | java | process | native
//! entry = "main.js"             # main.js | main.py | module.jar | the library file stem
//! command = ["node", "main.mjs"] # runtime = "process" only; or command.windows = […] etc.
//! description = "…"
//! homepage = "https://…"
//! author = "…"
//! license = "MIT"
//! requirements = ["requests>=2.31"]  # runtime = "python" only: packages, see `python_env`
//! min_glossa = "0.2.0"          # refuse to load in older builds
//!
//! capabilities = ["translate", "detect_language"]
//! languages = ["en", "ru", "de"]   # omit for "all"
//!
//! [language_map]                 # optional: our code -> the service's
//! "zh-Hans" = "zh"
//!
//! [permissions]
//! network = ["api.example.com"]  # hosts the module may reach; "*" for any
//!
//! [[options]]
//! key = "api_key"
//! type = "secret"
//! label = "API key"
//! required = true
//!
//! [[options]]                    # richer settings; see `settings`
//! key = "model"
//! type = "select"
//! choices = [{ value = "small" }, { value = "large" }]
//! enabled_if = "options.api_key"
//! group = "advanced"
//! choices_from = "list_models"   # an action may replace the choices
//!
//! [[actions]]                    # buttons; see `actions`
//! id = "list_models"
//! label = "Load models"
//!
//! [ui.settings]                  # optional layout; see `settings`
//! layout = ["api_key", "advanced", "@list_models"]
//!
//! [[assets]]                     # files fetched on first use; see `assets`
//! name = "model"
//! url = "https://example.com/model.onnx"
//! sha256 = "…64 hex digits…"
//! size = 4718592
//! ```
//!
//! Classic QTranslate scripts and pot-app plugins carry no `module.toml`; the
//! runtimes synthesise a [`Manifest`] for them from what the script itself
//! declares.
//!
//! `locales/<lang>.toml` beside the manifest translates its text
//! ([`crate::settings::strings`]); [`Manifest::load`] reads the files into
//! [`Manifest::strings`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glossa_core::{Capabilities, LangId, ProviderInfo};
use serde::{Deserialize, Serialize};

use crate::actions::ActionSpec;
use crate::assets::AssetSpec;
use crate::provider::OptionSpec;
use crate::settings::{ModuleStrings, SettingsPage, UiSection};

/// The Glossa version `min_glossa` is compared with. Every crate shares the
/// workspace version.
pub const GLOSSA_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub module: ModuleSection,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Supported languages as codes; empty for all.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Our BCP 47 code -> the code the service uses.
    #[serde(default)]
    pub language_map: BTreeMap<String, String>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub options: Vec<OptionSpec>,
    /// `[ui]`: the settings-page layout (and, in later formats, panels).
    /// Older builds ignore the table and load the module as a plain service.
    #[serde(default, skip_serializing_if = "UiSection::is_default")]
    pub ui: UiSection,
    /// `[[actions]]`: buttons the settings page can show, run by the module
    /// through the protocol's `action` method; see [`crate::actions`].
    /// Older builds ignore them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionSpec>,
    /// Files the module needs that are downloaded on demand (models,
    /// dictionaries, helper programs); see [`crate::assets`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetSpec>,
    /// Set by the registry: the folder this module's assets live in, which
    /// the `assets` host call reports. `None` for a module without assets.
    #[serde(skip)]
    pub assets_dir: Option<PathBuf>,
    /// Translations of the manifest's text from `locales/<lang>.toml`, read
    /// by [`Manifest::load`]. Empty for a manifest parsed from text.
    #[serde(skip)]
    pub strings: ModuleStrings,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleSection {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_api")]
    pub api: u32,
    pub runtime: Runtime,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    /// The oldest Glossa that can run the module (`"0.2.0"`). Older builds
    /// list it among the failed modules with "needs Glossa x.y.z or newer".
    /// Empty: any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub min_glossa: String,
    /// Longest input the service accepts, in characters.
    #[serde(default)]
    pub max_input_chars: Option<usize>,
    /// For a module running as a process (Python, Java, `process`): how long
    /// it may sit idle before its process is stopped, in seconds. Keeps
    /// resident memory down.
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// How long one call into the module may take, in seconds (1 to 600),
    /// instead of the program's `[modules] call_timeout_ms`: for a module
    /// whose work is slow by nature (a local language model, OCR of a full
    /// page). Shown to the user when the module is approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
    /// The module works with no setup, so Glossa may switch it on when it
    /// first appears. Leave it off for anything that needs a key or a server.
    #[serde(default)]
    pub ready_to_use: bool,

    // -- process modules (runtime = "process", "java", "python") --------------
    /// `runtime = "process"`: the program and its arguments, either one list
    /// or one per platform (`command.windows`, `command.macos`,
    /// `command.linux`, `command.unix`, `command.default`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    /// Extra arguments after the command (after the script or jar for the
    /// Python and Java runtimes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables set for the module's process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The process's working directory, relative to the module's directory
    /// (the default).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    /// `runtime = "java"`: run this class from `classpath` instead of
    /// `java -jar <entry>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub main_class: String,
    /// `runtime = "java"` with `main_class`: classpath entries relative to
    /// the module's directory (`lib/*` wildcards are Java's own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classpath: Vec<String>,
    /// `runtime = "java"`: options for the JVM, before `-jar` / the class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jvm_args: Vec<String>,
    /// `runtime = "python"`: packages the module needs, as pip requirement
    /// lines (`"requests>=2.31"`). Glossa installs them with `uv` into an
    /// environment of the module's own; see [`crate::python_env`]. A
    /// `requirements.txt` or `pyproject.toml` beside the manifest does the
    /// same when this is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<String>,
    /// `runtime = "python"`: the package index `requirements` come from,
    /// instead of PyPI (`https://…/simple`). Its host must be in
    /// `[permissions] network`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub python_index: String,
}

/// A process module's command line: one for every platform, or one per
/// platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    /// `command = ["node", "main.mjs"]`.
    Line(Vec<String>),
    /// `command = "./my-module"`: a program with no arguments.
    Program(String),
    /// `command.windows = ["my-module.exe"]` and so on.
    PerPlatform(PlatformCommands),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlatformCommands {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<Vec<String>>,
    /// Linux, macOS and the BSDs, when the specific key is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix: Option<Vec<String>>,
    /// Any platform without its own entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Vec<String>>,
}

impl CommandSpec {
    /// The command line for the platform this build runs on, if any.
    pub fn for_this_platform(&self) -> Option<Vec<String>> {
        self.for_platform(std::env::consts::OS)
    }

    /// The command line for `os` (a `std::env::consts::OS` value).
    pub fn for_platform(&self, os: &str) -> Option<Vec<String>> {
        let line = match self {
            CommandSpec::Line(l) => Some(l.clone()),
            CommandSpec::Program(p) => Some(vec![p.clone()]),
            CommandSpec::PerPlatform(p) => {
                let specific = match os {
                    "windows" => p.windows.clone(),
                    "macos" => p.macos.clone(),
                    "linux" => p.linux.clone(),
                    _ => None,
                };
                let unix = (os != "windows").then(|| p.unix.clone()).flatten();
                specific.or(unix).or_else(|| p.default.clone())
            }
        };
        line.filter(|l| l.first().is_some_and(|p| !p.trim().is_empty()))
    }
}

fn default_version() -> String {
    "0.0.0".into()
}

fn default_api() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    /// A Glossa JavaScript module.
    #[default]
    Js,
    /// A Python module speaking the module protocol over stdio.
    Python,
    /// A shared library exporting the C ABI.
    Native,
    /// Any program speaking the module protocol over stdio (`command`).
    Process,
    /// A Java program speaking the module protocol over stdio: `java -jar`
    /// on `entry`, or `main_class` on `classpath`.
    Java,
    /// A classic QTranslate `Service.js`.
    Qtranslate,
    /// A pot-app plugin (`info.json` + `main.js`, or a `.potext` archive).
    Pot,
}

impl Runtime {
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Js => "JavaScript",
            Runtime::Python => "Python",
            Runtime::Native => "Native",
            Runtime::Process => "Process",
            Runtime::Java => "Java",
            Runtime::Qtranslate => "QTranslate script",
            Runtime::Pot => "pot-app plugin",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    /// Hosts the module may contact. A leading `*.` matches subdomains; a
    /// bare `*` allows anything. Empty means no network at all.
    #[serde(default)]
    pub network: Vec<String>,
}

impl Permissions {
    pub fn allow_all() -> Self {
        Self { network: vec!["*".into()] }
    }

    /// Whether `url` falls inside the allowlist.
    pub fn allows_url(&self, url: &str) -> bool {
        let Some(host) = url_host(url) else { return false };
        self.network.iter().any(|pattern| {
            let p = pattern.trim().to_ascii_lowercase();
            if p == "*" {
                return true;
            }
            if let Some(suffix) = p.strip_prefix("*.") {
                return host == suffix || host.ends_with(&format!(".{suffix}"));
            }
            host == p
        })
    }
}

/// The host part of a URL, lowercased, without port or credentials.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next()? // IPv6 literal
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_owned(), e))?;
        let mut m =
            Manifest::parse(&text).map_err(|message| ManifestError::Invalid { path: path.to_owned(), message })?;
        if let Some(dir) = path.parent() {
            // A broken translation never stops a module: its text falls
            // back to the manifest's, and `module check` names the file.
            let (strings, problems) = ModuleStrings::load(dir);
            for p in problems {
                tracing::warn!(module = %m.module.id, "{p}");
            }
            m.strings = strings;
        }
        Ok(m)
    }

    /// The settings page in the interface language `lang`.
    pub fn settings_page(&self, lang: &str) -> SettingsPage {
        SettingsPage::for_manifest(self, lang)
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), String> {
        let id = &self.module.id;
        if id.is_empty()
            || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!("module id {id:?} must be lowercase letters, digits, '-', '_' or '.'"));
        }
        if id.chars().all(|c| c == '.') {
            // It names folders (the module's files go to `models/<id>`).
            return Err(format!("module id {id:?} must contain a letter or a digit"));
        }
        check_min_glossa(&self.module.min_glossa, GLOSSA_VERSION)?;
        if self.module.api != crate::PROTOCOL_VERSION {
            return Err(format!(
                "module speaks protocol {}, this build speaks {}",
                self.module.api,
                crate::PROTOCOL_VERSION
            ));
        }
        for cap in &self.capabilities {
            if Capabilities::from_name(cap).is_none() {
                return Err(format!("unknown capability {cap:?}"));
            }
        }
        if self.capabilities.is_empty() {
            return Err("the manifest lists no capabilities".into());
        }
        if let Some(secs) = self.module.call_timeout_secs {
            let max = crate::protocol::MAX_CALL_TIMEOUT_SECS;
            if secs == 0 || secs > max {
                return Err(format!("call_timeout_secs = {secs} must be between 1 and {max}"));
            }
        }
        if self.module.runtime == Runtime::Process && self.module.command.is_none() {
            return Err("runtime = \"process\" needs a command, e.g. command = [\"node\", \"main.mjs\"]".into());
        }
        if !self.module.requirements.is_empty() && self.module.runtime != Runtime::Python {
            return Err("requirements are for runtime = \"python\" only".into());
        }
        for r in &self.module.requirements {
            crate::python_env::check_requirement(r)?;
        }
        let index = self.module.python_index.trim();
        if !index.is_empty() {
            if self.module.runtime != Runtime::Python {
                return Err("python_index is for runtime = \"python\" only".into());
            }
            if !index.starts_with("https://") {
                return Err(format!("python_index {index:?} must be an https:// URL"));
            }
            // Like asset downloads: the permissions list says every host the
            // module may get anything from.
            if !self.permissions.allows_url(index) {
                let host = url_host(index).unwrap_or_else(|| index.to_owned());
                return Err(format!(
                    "python_index: {host} is not in [permissions] network; add it to install from there"
                ));
            }
        }
        for asset in &self.assets {
            asset.validate()?;
            // Downloads stay within the hosts the module declares, like its
            // own requests, so the permissions list says everything the
            // module may reach.
            for url in asset.url.list() {
                if !self.permissions.allows_url(url) {
                    let host = url_host(url).unwrap_or_else(|| url.to_owned());
                    return Err(format!(
                        "asset {}: {host} is not in [permissions] network; add it to download from there",
                        asset.name
                    ));
                }
            }
        }
        Ok(())
    }

    /// The time limit the manifest asks for one call, if it does.
    pub fn call_timeout(&self) -> Option<std::time::Duration> {
        self.module
            .call_timeout_secs
            .map(|s| std::time::Duration::from_secs(s.clamp(1, crate::protocol::MAX_CALL_TIMEOUT_SECS)))
    }

    /// What the manifest says the module can do. `offline` is its word
    /// that nothing leaves this computer, and a permission to contact any
    /// host at all contradicts it: such a module is not counted as offline.
    /// Named hosts are left to it (an offline engine names where its models
    /// are downloaded from, a local server is `127.0.0.1`).
    pub fn capabilities(&self) -> Capabilities {
        let caps: Capabilities = self.capabilities.iter().filter_map(|c| Capabilities::from_name(c)).collect();
        if self.permissions.network.iter().any(|h| h.trim() == "*") {
            return caps.without(Capabilities::OFFLINE);
        }
        caps
    }

    pub fn provider_info(&self) -> ProviderInfo {
        let languages = self.languages.iter().filter_map(|c| LangId::from_code(c)).collect();
        ProviderInfo {
            id: self.module.id.clone(),
            name: self.module.name.clone(),
            description: self.module.description.clone(),
            homepage: self.module.homepage.clone(),
            capabilities: self.capabilities(),
            languages,
            max_input_chars: self.module.max_input_chars,
            needs_credentials: self.options.iter().any(|o| o.required),
            ready_to_use: self.module.ready_to_use && !self.options.iter().any(|o| o.required),
        }
    }

    /// The code to send the module for `lang`.
    pub fn code_for(&self, lang: LangId) -> String {
        if lang == LangId::AUTO {
            return self.language_map.get("auto").cloned().unwrap_or_else(|| "auto".into());
        }
        let bcp = lang.bcp47();
        self.language_map
            .get(bcp)
            .or_else(|| self.language_map.get(lang.legacy_code()))
            .cloned()
            .unwrap_or_else(|| bcp.to_owned())
    }

    /// Our language for a code the module sent back.
    pub fn lang_for(&self, code: &str) -> Option<LangId> {
        self.language_map
            .iter()
            .find(|(_, theirs)| theirs.eq_ignore_ascii_case(code))
            .and_then(|(ours, _)| LangId::from_code(ours))
            .or_else(|| LangId::from_code(code))
    }
}

/// Refuse a module that needs a newer Glossa than `current`.
pub fn check_min_glossa(min: &str, current: &str) -> Result<(), String> {
    let min = min.trim();
    if min.is_empty() {
        return Ok(());
    }
    let want = Version::parse(min).ok_or_else(|| format!("min_glossa {min:?} is not a version like \"0.2.0\""))?;
    let have = Version::parse(current).ok_or_else(|| format!("this build's version {current:?} does not parse"))?;
    if have < want {
        return Err(format!("needs Glossa {min} or newer (this is {current})"));
    }
    Ok(())
}

/// `major.minor.patch[-pre]`, ordered as semantic versioning orders them:
/// `0.2.0-beta.2 < 0.2.0-beta.10 < 0.2.0`. Build metadata is ignored.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    nums: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches('v');
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_owned).collect()),
            None => (s, Vec::new()),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() > 3 {
            return None;
        }
        let mut nums = [0u64; 3];
        for (i, p) in parts.iter().enumerate() {
            nums[i] = p.parse().ok()?;
        }
        Some(Version { nums, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.nums.cmp(&other.nums).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let o = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        _ => a.cmp(b),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_glossa_compares_versions_with_prereleases() {
        assert!(check_min_glossa("", "0.1.0").is_ok());
        assert!(check_min_glossa("0.1.0", "0.1.0").is_ok());
        assert!(check_min_glossa("0.1", "0.1.0").is_ok());
        assert!(check_min_glossa("0.1.0-beta.2", "0.1.0-beta.10").is_ok());
        assert!(check_min_glossa("0.1.0-beta.2", "0.1.0").is_ok());
        assert!(check_min_glossa("0.1.0-alpha", "0.1.0-beta").is_ok());
        let e = check_min_glossa("0.1.0", "0.1.0-beta.2").unwrap_err();
        assert_eq!(e, "needs Glossa 0.1.0 or newer (this is 0.1.0-beta.2)");
        assert!(check_min_glossa("0.10.0", "0.9.9").is_err());
        assert!(check_min_glossa("1.0.0", "0.99.0").is_err());
        assert!(check_min_glossa("soon", "0.1.0").unwrap_err().contains("not a version"));
        // This build loads a module asking for its own version and refuses
        // one asking for a far-off release.
        let own = SAMPLE.replace("[module]", &format!("[module]\nmin_glossa = \"{GLOSSA_VERSION}\""));
        assert!(Manifest::parse(&own).is_ok());
        let future = SAMPLE.replace("[module]", "[module]\nmin_glossa = \"99.0.0\"");
        assert!(Manifest::parse(&future).unwrap_err().contains("needs Glossa 99.0.0 or newer"));
    }

    #[test]
    fn offline_is_not_taken_on_its_word_with_the_whole_network_allowed() {
        let offline = SAMPLE.replacen("capabilities = [", "capabilities = [\"offline\", ", 1);
        let mut m = Manifest::parse(&offline).unwrap();
        m.permissions.network = vec!["127.0.0.1".into(), "models.test".into()];
        assert!(m.capabilities().contains(Capabilities::OFFLINE));
        assert!(m.provider_info().capabilities.contains(Capabilities::OFFLINE));
        m.permissions.network.push(" * ".into());
        assert!(!m.capabilities().contains(Capabilities::OFFLINE));
        assert!(!m.provider_info().capabilities.contains(Capabilities::OFFLINE));
    }

    #[test]
    fn old_manifests_parse_as_before() {
        // The sample predates richer settings: nothing new appears, and it
        // serialises without any of the new keys.
        let m = Manifest::parse(SAMPLE).unwrap();
        assert!(m.ui.is_default() && m.module.min_glossa.is_empty() && m.strings.is_empty());
        let o = &m.options[0];
        assert!(o.pattern.is_empty() && o.visible_if.is_empty() && o.group.is_empty() && o.step.is_none());
        let back = serde_json::to_string(&m).unwrap();
        for new in ["min_glossa", "[ui", "pattern", "visible_if", "widget", "group", "allow_auto"] {
            assert!(!back.contains(new), "{new} in {back}");
        }
        // A type from a newer format still loads the module, and is written
        // back as it was, not as "unknown".
        let newer = SAMPLE.replace("type = \"secret\"", "type = \"colour\"");
        let m = Manifest::parse(&newer).unwrap();
        assert_eq!(m.options[0].kind, crate::OptionKind::Unknown("colour".into()));
        let back: serde_json::Value = serde_json::to_value(&m.options[0]).unwrap();
        assert_eq!(back["type"], "colour");
        let spec: crate::OptionSpec = toml::from_str("key = \"k\"\ntype = \"multiselect\"").unwrap();
        assert_eq!(serde_json::to_value(&spec).unwrap()["type"], "multiselect");
        assert_eq!(crate::OptionSpec::default().kind.name(), "text");
    }

    #[test]
    fn load_reads_the_locale_files() {
        let dir = std::env::temp_dir().join(format!("glossa-manifest-locales-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("locales")).unwrap();
        std::fs::write(dir.join("module.toml"), SAMPLE).unwrap();
        std::fs::write(dir.join("locales/ru.toml"), "[options.api_key]\nlabel = 'Ключ'\n").unwrap();
        let m = Manifest::load(&dir.join("module.toml")).unwrap();
        assert_eq!(m.settings_page("ru").options[0].label, "Ключ");
        assert_eq!(m.settings_page("en").options[0].label, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    const SAMPLE: &str = r#"
        capabilities = ["translate", "detect_language"]
        languages = ["en", "ru", "zh-Hans"]

        [module]
        id = "my-translator"
        name = "My Translator"
        runtime = "js"
        entry = "main.js"

        [language_map]
        "zh-Hans" = "ZH"
        auto = "AUTO"

        [permissions]
        network = ["api.example.com", "*.cdn.example.org"]

        [[options]]
        key = "api_key"
        type = "secret"
        required = true
    "#;

    #[test]
    fn parses_and_derives_provider_info() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.module.runtime, Runtime::Js);
        assert_eq!(m.module.api, 1);
        let info = m.provider_info();
        assert!(info.capabilities.contains(Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE));
        assert_eq!(info.languages.len(), 3);
        assert!(info.needs_credentials);
    }

    #[test]
    fn language_codes_map_both_ways() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.code_for(LangId::from_code("zh-CN").unwrap()), "ZH");
        assert_eq!(m.code_for(LangId::RUSSIAN), "ru");
        assert_eq!(m.code_for(LangId::AUTO), "AUTO");
        assert_eq!(m.lang_for("ZH"), LangId::from_code("zh-Hans"));
        assert_eq!(m.lang_for("ru"), Some(LangId::RUSSIAN));
        // Hebrew goes out as its modern tag when unmapped.
        assert_eq!(m.code_for(LangId::from_code("iw").unwrap()), "he");
    }

    #[test]
    fn invalid_manifests_are_rejected_with_a_reason() {
        let bad_id = SAMPLE.replace("my-translator", "My Translator");
        assert!(Manifest::parse(&bad_id).unwrap_err().contains("lowercase"));
        let bad_cap = SAMPLE.replace("\"detect_language\"", "\"teleport\"");
        assert!(Manifest::parse(&bad_cap).unwrap_err().contains("teleport"));
        let future = SAMPLE.replace("entry = \"main.js\"", "entry = \"main.js\"\napi = 9");
        assert!(Manifest::parse(&future).unwrap_err().contains("protocol 9"));
    }

    #[test]
    fn network_permissions_match_hosts_and_subdomains_only() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let p = &m.permissions;
        assert!(p.allows_url("https://api.example.com/v1/translate?x=1"));
        assert!(p.allows_url("https://API.EXAMPLE.COM:8443/"));
        assert!(!p.allows_url("https://evil.com/?https://api.example.com"));
        assert!(!p.allows_url("https://api.example.com.evil.com/"));
        assert!(p.allows_url("https://a.cdn.example.org/x"));
        assert!(p.allows_url("https://cdn.example.org/x"));
        assert!(!p.allows_url("https://user:pw@other.org/"));
        assert!(!Permissions::default().allows_url("https://api.example.com/"));
        assert!(Permissions::allow_all().allows_url("http://anything/"));
    }

    #[test]
    fn process_commands_parse_in_every_form() {
        let head = "capabilities = [\"translate\"]\n[module]\nid = \"p\"\nname = \"P\"\nruntime = \"process\"\n";
        let m = Manifest::parse(&format!(
            "{head}command = [\"node\", \"main.mjs\"]\nargs = [\"-q\"]\nenv = {{ A = \"1\" }}"
        ))
        .unwrap();
        assert_eq!(m.module.runtime, Runtime::Process);
        assert_eq!(m.module.command.unwrap().for_platform("linux").unwrap(), ["node", "main.mjs"]);
        assert_eq!((m.module.args, m.module.env["A"].as_str()), (vec!["-q".to_owned()], "1"));

        let m = Manifest::parse(&format!("{head}command = \"./tool\"")).unwrap();
        assert_eq!(m.module.command.unwrap().for_platform("windows").unwrap(), ["./tool"]);

        let m = Manifest::parse(&format!(
            "{head}command.windows = [\"tool.exe\"]\ncommand.unix = [\"./tool\"]\ncommand.macos = [\"./tool-mac\"]\ncommand.default = [\"java\", \"-jar\", \"t.jar\"]"
        ))
        .unwrap();
        let c = m.module.command.unwrap();
        assert_eq!(c.for_platform("windows").unwrap(), ["tool.exe"]);
        assert_eq!(c.for_platform("macos").unwrap(), ["./tool-mac"]);
        assert_eq!(c.for_platform("linux").unwrap(), ["./tool"]);
        assert_eq!(c.for_platform("freebsd").unwrap(), ["./tool"]);
        let only_windows =
            CommandSpec::PerPlatform(PlatformCommands { windows: Some(vec!["x.exe".into()]), ..Default::default() });
        assert_eq!(only_windows.for_platform("linux"), None);
        assert_eq!(CommandSpec::Line(vec![]).for_platform("linux"), None);

        assert!(Manifest::parse(head).unwrap_err().contains("needs a command"));
        let java = head.replace("\"process\"", "\"java\"") + "entry = \"m.jar\"\njvm_args = [\"-Xmx64m\"]";
        assert_eq!(Manifest::parse(&java).unwrap().module.runtime, Runtime::Java);
    }

    #[test]
    fn url_host_edge_cases() {
        assert_eq!(url_host("https://user:pw@Host.com:80/p").as_deref(), Some("host.com"));
        assert_eq!(url_host("http://[::1]:8080/").as_deref(), Some("::1"));
        assert_eq!(url_host("not a url"), None);
        assert_eq!(url_host("https:///path"), None);
    }
}
