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
//! ```
//!
//! Classic QTranslate scripts and pot-app plugins carry no `module.toml`; the
//! runtimes synthesise a [`Manifest`] for them from what the script itself
//! declares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glossa_core::{Capabilities, LangId, ProviderInfo};
use serde::{Deserialize, Serialize};

use crate::provider::OptionSpec;

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
    /// Longest input the service accepts, in characters.
    #[serde(default)]
    pub max_input_chars: Option<usize>,
    /// For a module running as a process (Python, Java, `process`): how long
    /// it may sit idle before its process is stopped, in seconds. Keeps
    /// resident memory down.
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
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
        Manifest::parse(&text).map_err(|message| ManifestError::Invalid { path: path.to_owned(), message })
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
        if self.module.runtime == Runtime::Process && self.module.command.is_none() {
            return Err("runtime = \"process\" needs a command, e.g. command = [\"node\", \"main.mjs\"]".into());
        }
        Ok(())
    }

    pub fn capabilities(&self) -> Capabilities {
        self.capabilities.iter().filter_map(|c| Capabilities::from_name(c)).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
