//! Checks that need no call: the manifest, the files beside it, and whether
//! what the module needs to run (an interpreter, a built library) is here.

use std::path::{Path, PathBuf};

use glossa_core::LangId;
use glossa_modules::manifest::{Manifest, Runtime};
use glossa_modules::{OptionKind, RuntimeContext};

use crate::{Finding, Severity};

/// What [`check`] found.
pub struct Lint {
    /// The manifest, when it parsed.
    pub manifest: Option<Manifest>,
    pub findings: Vec<Finding>,
    /// Something the module needs that is not here (an interpreter, a
    /// library or jar that has not been built): the module cannot be run,
    /// through no fault of its own.
    pub missing: Option<String>,
    /// Bytes in the module's folder, build folders excluded.
    pub size: u64,
}

/// Folders that are build output or tooling, not part of what users install.
const IGNORED_DIRS: &[&str] = &["target", ".git", "__pycache__", "node_modules", ".venv", "build", ".gradle"];

/// A folder this large is worth a warning: modules are downloaded, copied
/// and loaded, and Glossa's own footprint is single megabytes.
const LARGE: u64 = 20 * 1024 * 1024;

pub fn check(dir: &Path, ctx: &RuntimeContext) -> Lint {
    let mut lint = Lint { manifest: None, findings: Vec::new(), missing: None, size: 0 };
    let path = dir.join("module.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            lint.findings.push(Finding::error(format!("cannot read {}: {e}", path.display())));
            return lint;
        }
    };

    // Unknown keys: serde skips them, so a misspelt `entyr` or `languages`
    // under the wrong table would otherwise do nothing, silently.
    let mut unknown = Vec::new();
    let parsed: Result<Manifest, String> = toml::Deserializer::parse(&text)
        .map_err(|e| e.to_string())
        .and_then(|de| serde_ignored::deserialize(de, |p| unknown.push(p.to_string())).map_err(|e| e.to_string()));
    for key in unknown {
        lint.findings.push(Finding::error(format!(
            "module.toml: unknown key `{key}` (misspelt, or in the wrong table?); Glossa ignores it"
        )));
    }
    let manifest = match parsed {
        Ok(m) => m,
        Err(e) => {
            lint.findings.push(Finding::error(format!("module.toml: {}", e.trim())));
            return lint;
        }
    };
    if let Err(e) = manifest.validate() {
        lint.findings.push(Finding::error(format!("module.toml: {e}")));
    }

    check_metadata(&manifest, &mut lint.findings);
    check_languages(&manifest, &mut lint.findings);
    check_options(&manifest, &mut lint.findings);
    check_permissions(&manifest, &mut lint.findings);
    lint.missing = check_entry(dir, &manifest, ctx, &mut lint.findings);

    let mut files = Vec::new();
    walk(dir, &mut files);
    lint.size = files.iter().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum();
    if lint.size > LARGE {
        lint.findings.push(Finding::warning(format!(
            "the folder holds {:.1} MB; download large models or data on first use instead of shipping them",
            lint.size as f64 / 1_048_576.0
        )));
    }
    for file in &files {
        scan_for_secrets(dir, file, &mut lint.findings);
    }

    lint.manifest = Some(manifest);
    lint
}

fn check_metadata(m: &Manifest, out: &mut Vec<Finding>) {
    let s = &m.module;
    if s.name.trim().is_empty() {
        out.push(Finding::error("module.toml: `name` is empty; it is what the user sees".into()));
    }
    if s.description.trim().is_empty() {
        out.push(Finding::note("module.toml: no `description`; the services list shows it".into()));
    }
    if !s.homepage.is_empty() && !s.homepage.starts_with("https://") && !s.homepage.starts_with("http://") {
        out.push(Finding::warning(format!("module.toml: homepage {:?} is not a web address", s.homepage)));
    }
    if s.version == "0.0.0" {
        out.push(Finding::note("module.toml: no `version`; set one so users can tell releases apart".into()));
    }
    let translates = m.capabilities.iter().any(|c| c == "translate" || c == "dictionary");
    if translates && s.max_input_chars.is_none() {
        out.push(Finding::note(
            "module.toml: no `max_input_chars`; say how much text the service takes, and Glossa cuts longer text at a word boundary"
                .into(),
        ));
    }
    let required = m.options.iter().any(|o| o.required);
    if s.ready_to_use && required {
        out.push(Finding::warning(
            "module.toml: `ready_to_use = true` has no effect while an option is required".into(),
        ));
    }
    let process = matches!(s.runtime, Runtime::Python | Runtime::Java | Runtime::Process);
    if !process && (s.idle_timeout_secs.is_some() || !s.args.is_empty() || !s.env.is_empty() || !s.cwd.is_empty()) {
        out.push(Finding::warning(format!(
            "module.toml: idle_timeout_secs, args, env and cwd apply to process modules only, not runtime = {:?}",
            s.runtime.label()
        )));
    }
}

fn check_languages(m: &Manifest, out: &mut Vec<Finding>) {
    for code in &m.languages {
        if !LangId::from_code(code).is_some_and(|l| l.is_language()) {
            out.push(Finding::error(format!(
                "module.toml: unknown language code {code:?} in `languages` (BCP 47: en, zh-Hans, pt-BR…)"
            )));
        }
    }
    for (ours, theirs) in &m.language_map {
        if ours != "auto" && !LangId::from_code(ours).is_some_and(|l| l.is_language()) {
            out.push(Finding::error(format!(
                "module.toml: [language_map] key {ours:?} is not a language Glossa knows; keys are Glossa's codes, values the service's"
            )));
        }
        if theirs.trim().is_empty() {
            out.push(Finding::error(format!("module.toml: [language_map] {ours:?} maps to an empty code")));
        }
    }
    let mut seen = Vec::new();
    for code in &m.languages {
        if seen.contains(&code) {
            out.push(Finding::warning(format!("module.toml: {code:?} is listed twice in `languages`")));
        }
        seen.push(code);
    }
}

fn check_options(m: &Manifest, out: &mut Vec<Finding>) {
    let mut keys = Vec::new();
    for o in &m.options {
        if o.key.trim().is_empty() {
            out.push(Finding::error("module.toml: an [[options]] entry has no `key`".into()));
            continue;
        }
        if keys.contains(&&o.key) {
            out.push(Finding::error(format!("module.toml: option {:?} is declared twice", o.key)));
        }
        keys.push(&o.key);
        if o.label.trim().is_empty() {
            out.push(Finding::note(format!(
                "module.toml: option {:?} has no `label`; the key is shown instead",
                o.key
            )));
        }
        let secretish = ["key", "token", "secret", "password"].iter().any(|w| o.key.to_ascii_lowercase().contains(w));
        if o.kind == OptionKind::Secret && !o.default.is_null() && o.default != serde_json::json!("") {
            out.push(Finding::error(format!(
                "module.toml: secret option {:?} has a default; a default ships the value to every user",
                o.key
            )));
        } else if secretish && o.kind == OptionKind::Text {
            out.push(Finding::warning(format!(
                "module.toml: option {:?} looks like a credential; give it type = \"secret\" so it is masked and kept in the keychain",
                o.key
            )));
        }
        match o.kind {
            OptionKind::Select if o.choices.is_empty() => {
                out.push(Finding::error(format!("module.toml: select option {:?} has no `choices`", o.key)));
            }
            OptionKind::Select => {
                if let Some(d) = o.default.as_str() {
                    if !o.choices.iter().any(|c| c.value == d) {
                        out.push(Finding::error(format!(
                            "module.toml: option {:?} defaults to {d:?}, which is not one of its choices",
                            o.key
                        )));
                    }
                }
            }
            OptionKind::Number => {
                if let Some(d) = o.default.as_f64() {
                    if o.min.is_some_and(|min| d < min) || o.max.is_some_and(|max| d > max) {
                        out.push(Finding::error(format!(
                            "module.toml: option {:?} defaults to {d}, outside its min/max",
                            o.key
                        )));
                    }
                } else if !o.default.is_null() {
                    out.push(Finding::error(format!(
                        "module.toml: number option {:?} has a non-number default",
                        o.key
                    )));
                }
            }
            OptionKind::Bool if !o.default.is_null() && !o.default.is_boolean() => {
                out.push(Finding::error(format!("module.toml: bool option {:?} has a non-boolean default", o.key)));
            }
            _ => {}
        }
    }
}

fn check_permissions(m: &Manifest, out: &mut Vec<Finding>) {
    for host in &m.permissions.network {
        let h = host.trim();
        if h == "*" {
            out.push(Finding::warning(
                "module.toml: network = [\"*\"] lets the module reach any host; list the service's hosts instead"
                    .into(),
            ));
        } else if h.is_empty() || h.contains("://") || h.contains('/') || h.contains(' ') {
            out.push(Finding::error(format!(
                "module.toml: {host:?} in [permissions] network is not a host name (write api.example.com, not a URL)"
            )));
        } else if h.trim_start_matches("*.").contains('*') {
            out.push(Finding::error(format!(
                "module.toml: {host:?}: only a leading `*.` is supported in [permissions] network"
            )));
        }
    }
}

/// Check the entry file and what it needs. Returns why the module cannot
/// run here, when that is the machine's doing rather than the module's.
fn check_entry(dir: &Path, m: &Manifest, ctx: &RuntimeContext, out: &mut Vec<Finding>) -> Option<String> {
    let s = &m.module;
    let entry = s.entry.trim();
    match s.runtime {
        Runtime::Js => {
            let file = if entry.is_empty() { "main.js" } else { entry };
            if !dir.join(file).is_file() {
                out.push(Finding::error(format!("the entry {file} is missing")));
            }
            None
        }
        Runtime::Python => {
            let file = if entry.is_empty() { "main.py" } else { entry };
            if !dir.join(file).is_file() {
                out.push(Finding::error(format!("the entry {file} is missing")));
            }
            if glossa_modules::runtime::python::find_python(&ctx.python).is_none() {
                return Some("no Python 3 interpreter found (install Python 3.8 or newer)".into());
            }
            None
        }
        Runtime::Process => glossa_modules::runtime::process::process_launch(dir, m, ctx).err(),
        Runtime::Java => {
            if glossa_modules::runtime::process::find_java(&ctx.java).is_none() {
                return Some("no Java runtime found (install Java 17 or newer, or set JAVA_HOME)".into());
            }
            glossa_modules::runtime::process::java_launch(dir, m, ctx).err()
        }
        Runtime::Native => {
            let stem = if entry.is_empty() { s.id.as_str() } else { entry };
            glossa_modules::runtime::native::resolve_library(dir, stem).err().map(|e| format!("{e} (not built yet?)"))
        }
        Runtime::Qtranslate | Runtime::Pot => None,
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !IGNORED_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else if p.is_file() {
            out.push(p);
        }
    }
}

/// Look for things that are keys, tokens or passwords in every text file,
/// recordings included (a recorded answer can echo a key back).
fn scan_for_secrets(dir: &Path, file: &Path, out: &mut Vec<Finding>) {
    let Ok(meta) = file.metadata() else { return };
    if meta.len() > 4 * 1024 * 1024 {
        return;
    }
    let Ok(bytes) = std::fs::read(file) else { return };
    let Ok(text) = std::str::from_utf8(&bytes) else { return };
    let shown = file.strip_prefix(dir).unwrap_or(file).display().to_string();
    for (n, line) in text.lines().enumerate() {
        if let Some(what) = secret_in(line) {
            out.push(Finding::error(format!(
                "{shown}:{}: looks like a hard-coded {what}; take it from an option (type = \"secret\") and use an obvious fake in tests",
                n + 1
            )));
        }
    }
}

/// What kind of secret `line` seems to hold, if any.
pub fn secret_in(line: &str) -> Option<&'static str> {
    if line.contains("PRIVATE KEY-----") {
        return Some("private key");
    }
    let prefixed: &[(&str, usize, &str)] = &[
        ("AIza", 35, "Google API key"),
        ("sk-ant-", 20, "Anthropic key"),
        ("sk-", 32, "API key"),
        ("ghp_", 30, "GitHub token"),
        ("github_pat_", 30, "GitHub token"),
        ("xoxb-", 20, "Slack token"),
        ("xoxp-", 20, "Slack token"),
        ("AKIA", 16, "AWS access key"),
    ];
    for (prefix, min, what) in prefixed {
        let mut from = 0;
        while let Some(i) = line[from..].find(prefix) {
            let start = from + i;
            let before_ok = line[..start].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
            let run = token_run(&line[start + prefix.len()..]);
            if before_ok && run.len() >= *min && !looks_fake(run) {
                return Some(what);
            }
            from = start + prefix.len();
        }
    }
    // `api_key = "…"`, `"token": "…"`, `password='…'` with a long literal.
    let lower = line.to_ascii_lowercase();
    for name in ["api_key", "apikey", "api-key", "access_key", "secret", "token", "password", "bearer "] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(name) {
            let after = from + i + name.len();
            let rest = line[after..].trim_start_matches(['"', '\'', ' ']);
            let rest =
                if name == "bearer " { Some(rest) } else { rest.strip_prefix([':', '=']).map(|r| r.trim_start()) };
            if let Some(rest) = rest {
                if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    let inner = &rest[1..];
                    if let Some(end) = inner.find(quote) {
                        let value = &inner[..end];
                        if value.len() >= 20 && token_run(value).len() == value.len() && !looks_fake(value) {
                            return Some("secret");
                        }
                    }
                } else if name == "bearer " {
                    let run = token_run(rest);
                    if run.len() >= 20 && !looks_fake(run) {
                        return Some("bearer token");
                    }
                }
            }
            from = after;
        }
    }
    None
}

fn token_run(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || "-_.+/=".contains(c))).unwrap_or(s.len());
    &s[..end]
}

/// Placeholders and test values are fine.
fn looks_fake(value: &str) -> bool {
    let v = value.to_ascii_lowercase();
    ["fake", "test", "example", "dummy", "placeholder", "xxxx", "your", "redacted", "changeme"]
        .iter()
        .any(|w| v.contains(w))
        || value.chars().all(|c| c == value.chars().next().unwrap_or('x'))
}

impl Finding {
    pub(crate) fn error(message: String) -> Self {
        Finding { severity: Severity::Error, message }
    }

    pub(crate) fn warning(message: String) -> Self {
        Finding { severity: Severity::Warning, message }
    }

    pub(crate) fn note(message: String) -> Self {
        Finding { severity: Severity::Note, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_coded_keys_are_found_and_fakes_are_not() {
        // The look-alike values are put together at run time, so this file
        // does not itself trip secret scanners such as GitGuardian.
        let token = "q8Zr2Lm4Np6V".repeat(3);
        let google = format!("var key = \"{}za{}{}\";", "AI", "Sy", &token[..33]);
        assert_eq!(secret_in(&google), Some("Google API key"));
        let bearer = format!("headers = {{\"Authorization\": \"{} {token}\"}}", "Bearer");
        assert_eq!(secret_in(&bearer), Some("bearer token"));
        assert_eq!(secret_in(&format!("API_KEY = \"{}\"", &token[..24])), Some("secret"));
        assert_eq!(secret_in(&format!("\"token\": \"{token}\"")), Some("secret"));
        assert_eq!(secret_in(&format!("-----BEGIN RSA {} KEY-----", "PRIVATE")), Some("private key"));
        // Code that reads a key, and fakes, are fine.
        assert_eq!(secret_in(r#"var key = req.options.api_key;"#), None);
        assert_eq!(secret_in(r#"headers["Authorization"] = "Bearer " + token;"#), None);
        assert_eq!(secret_in(r#"options = { api_key = "test-key-not-real-1234567890" }"#), None);
        assert_eq!(secret_in(r#"key = "api_key""#), None);
        assert_eq!(secret_in("desk-top-environment-with-long-words-here-and-more"), None);
        assert_eq!(secret_in(r#"api_key = "xxxxxxxxxxxxxxxxxxxxxxxx""#), None);
    }
}
