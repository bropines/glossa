//! Writing modules inside Glossa: what the module editor needs, kept here so
//! `glossa-cli` and the control API can use it too.
//!
//! * [`Engine::module_sources`] — the modules there are and whether each can
//!   be edited where it is, copied, or neither;
//! * [`Engine::new_module`] / [`Engine::copy_module_to_user`] — a module in
//!   the user's folder to edit, from a template or from a bundled or
//!   installed one (the copy replaces the original by id);
//! * [`module_files`] / [`save_module_file`] — the folder's files, and
//!   saving one atomically;
//! * [`Engine::check_module`] — the manifest's TOML with positions, its
//!   meaning (the test harness's checks), the script's syntax, the
//!   fixtures' syntax, all as [`Problem`]s with a file, line and column;
//! * [`Engine::reload_module`] — load one folder again and replace that one
//!   provider, without rescanning every folder;
//! * [`Engine::test_module`] — the test harness, as `glossa-cli module test`
//!   runs it;
//! * [`Engine::module_log`] — what the module logs, as it logs it.
//!
//! Everything here touches the disk or runs module code: call it from a
//! worker thread, never from a UI thread.

use std::path::{Component, Path, PathBuf};

use glossa_core::{Error, Result};
use glossa_modules::log_tap::LogTap;
use glossa_modules::manifest::{Manifest, Runtime};
use glossa_modules::{Origin, Registry};

pub use glossa_module_test::{CaseReport, ModuleReport, Severity, Status as TestStatus};

use crate::{Engine, Event, ModuleStatus};

/// Something wrong in a module's files, as precisely as it can be placed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Problem {
    /// Relative to the module's folder, with `/`; empty when it concerns the
    /// module as a whole.
    pub file: String,
    /// 1-based; 0 when the line is not known.
    pub line: u32,
    /// 1-based; 0 when the column is not known.
    pub column: u32,
    pub severity: Severity,
    pub message: String,
}

impl Problem {
    fn new(file: &str, line: u32, column: u32, severity: Severity, message: impl Into<String>) -> Problem {
        Problem { file: file.to_owned(), line, column, severity, message: message.into() }
    }

    /// `file:line:col`, as far as it is known.
    pub fn place(&self) -> String {
        match (self.line, self.column) {
            (0, _) => self.file.clone(),
            (l, 0) => format!("{}:{l}", self.file),
            (l, c) => format!("{}:{l}:{c}", self.file),
        }
    }
}

/// Where a module is and what can be done with it in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleSource {
    /// In the user's modules folder: edited in place.
    User(PathBuf),
    /// In another folder (beside the program, system-wide, a classic
    /// installation, an extra folder from the settings): copied to the
    /// user's folder first, where the copy replaces it by id.
    Installed(PathBuf),
    /// Compiled into the program: copied out first, likewise.
    Bundled,
    /// Rust code: nothing to edit.
    Builtin,
}

impl ModuleSource {
    pub fn dir(&self) -> Option<&Path> {
        match self {
            ModuleSource::User(d) | ModuleSource::Installed(d) => Some(d),
            _ => None,
        }
    }
}

/// One module, as the editor lists it.
#[derive(Debug, Clone)]
pub struct ModuleListing {
    /// The module's id; for a folder that failed to load, the folder's name.
    pub id: String,
    pub name: String,
    pub source: ModuleSource,
    /// "JavaScript", "Python", "Bundled"…
    pub kind: String,
    /// Why it did not load, for a folder that failed.
    pub failed: Option<String>,
}

/// A file in a module's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFile {
    /// Relative to the folder, with `/`.
    pub path: String,
    pub size: u64,
    /// Text the editor can open; binaries (a `.jar`, a library, a picture)
    /// are listed but not opened.
    pub text: bool,
}

/// What reloading one module did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReloadOutcome {
    /// The new code is in use. A process module's old process was stopped;
    /// the next call starts the new one.
    Reloaded { id: String },
    /// A native library cannot be unloaded safely: the new one loads when
    /// Glossa starts again.
    NeedsRestart { id: String },
    /// The module is switched off in the settings, so nothing was loaded.
    Disabled { id: String },
}

/// What a new module starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Translator,
    Dictionary,
    Ocr,
}

impl Template {
    pub const ALL: [Template; 3] = [Template::Translator, Template::Dictionary, Template::Ocr];

    /// The name `glossa-cli` and the locale keys use.
    pub fn name(self) -> &'static str {
        match self {
            Template::Translator => "translator",
            Template::Dictionary => "dictionary",
            Template::Ocr => "ocr",
        }
    }

    fn files(self) -> &'static [(&'static str, &'static str)] {
        macro_rules! files {
            ($dir:literal) => {
                &[
                    ("module.toml", include_str!(concat!("templates/", $dir, "/module.toml"))),
                    ("main.js", include_str!(concat!("templates/", $dir, "/main.js"))),
                    ("tests/basic.toml", include_str!(concat!("templates/", $dir, "/tests/basic.toml"))),
                ]
            };
        }
        match self {
            Template::Translator => files!("translator"),
            Template::Dictionary => files!("dictionary"),
            Template::Ocr => files!("ocr"),
        }
    }
}

/// Folders that are build output or tooling, not the module's sources.
const IGNORED_DIRS: &[&str] = &["target", ".git", "__pycache__", "node_modules", ".venv", "build", ".gradle"];
/// Enough for any module's sources; a folder with more is not listed whole.
const MAX_FILES: usize = 500;
/// Files larger than this are not offered for editing.
const MAX_TEXT: u64 = 2 << 20;

/// The files in a module's folder, sorted, build folders left out.
pub fn module_files(dir: &Path) -> Vec<ModuleFile> {
    fn visit(root: &Path, dir: &Path, depth: usize, out: &mut Vec<ModuleFile>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut items: Vec<_> = read.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            if out.len() >= MAX_FILES {
                return;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if depth < 6 && !IGNORED_DIRS.contains(&name.as_str()) {
                    visit(root, &path, depth + 1, out);
                }
                continue;
            }
            let rel = relative(root, &path);
            let text = meta.len() <= MAX_TEXT && looks_like_text(&path);
            out.push(ModuleFile { path: rel, size: meta.len(), text });
        }
    }
    let mut out = Vec::new();
    visit(dir, dir, 0, &mut out);
    out
}

fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Whether a file is text worth opening: by its extension when it is a
/// known one, else by its first bytes (UTF-8 with no NUL).
fn looks_like_text(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    const TEXT: &[&str] = &[
        "js", "mjs", "cjs", "ts", "toml", "json", "py", "java", "kt", "txt", "md", "html", "htm", "css", "xml", "c",
        "h", "rs", "sh", "bat", "cmd", "ps1", "yaml", "yml", "ini", "cfg", "csv", "tsv",
    ];
    const BINARY: &[&str] = &[
        "jar", "class", "dll", "so", "dylib", "exe", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "zip",
        "potext", "pyc", "onnx", "bin", "gz", "mp3", "wav", "ogg",
    ];
    if TEXT.contains(&ext.as_str()) {
        return true;
    }
    if BINARY.contains(&ext.as_str()) {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = [0u8; 4096];
    let n = std::io::Read::read(&mut f, &mut head).unwrap_or(0);
    let head = &head[..n];
    !head.contains(&0) && (std::str::from_utf8(head).is_ok() || n == head.len())
}

/// Resolve `rel` inside `dir`, refusing anything that would leave it.
fn inside(dir: &Path, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    if rel.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::Config(format!("{rel:?} is not a file inside the module's folder")));
    }
    Ok(dir.join(p))
}

/// Read one of a module's text files.
pub fn read_module_file(dir: &Path, rel: &str) -> Result<String> {
    let path = inside(dir, rel)?;
    let text = std::fs::read_to_string(&path)?;
    Ok(text)
}

/// Write one of a module's files: to a temporary name beside it, then
/// renamed over the old one, so a crash or a full disk never leaves half a
/// file behind and a running Glossa never loads one.
pub fn save_module_file(dir: &Path, rel: &str, text: &str) -> Result<()> {
    let path = inside(dir, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(Error::from)
}

/// Write `template`'s files into `dir` for a module `id` named `name`,
/// without loading it (see [`Engine::new_module`] for the usual way).
pub fn write_template(template: Template, dir: &Path, id: &str, name: &str) -> Result<()> {
    let display = if name.trim().is_empty() { id.to_owned() } else { name.trim().to_owned() };
    // The name goes into a TOML string and JavaScript comments.
    let toml_name = display.replace('\\', "\\\\").replace('"', "\\\"");
    for (rel, body) in template.files() {
        let body =
            body.replace("{{id}}", id).replace("{{name}}", if rel.ends_with(".toml") { &toml_name } else { &display });
        save_module_file(dir, rel, &body)?;
    }
    Ok(())
}

/// A folder name for a new module: its name lowercased, with `-` for
/// anything that is not a letter or digit.
pub fn module_id_for(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    let id = id.trim_end_matches('-').to_owned();
    if id.is_empty() {
        "my-module".into()
    } else {
        id
    }
}

/// `file:line[:col]` mentioned anywhere in a message ("main.js:14:9",
/// "(at main.js:3)"), for errors that reach us as text: a runtime's error,
/// a test case's failure.
pub fn find_place(message: &str) -> Option<(String, u32, u32)> {
    for word in message.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | '"' | '\'')) {
        let word = word.trim_end_matches(['.', ':']);
        let mut parts = word.split(':');
        let (Some(file), Some(line)) = (parts.next(), parts.next()) else { continue };
        let Ok(line) = line.parse::<u32>() else { continue };
        let column = parts.next().and_then(|c| c.parse::<u32>().ok()).unwrap_or(0);
        let has_ext = file.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
        if has_ext && line > 0 {
            return Some((file.to_owned(), line, column));
        }
    }
    None
}

/// Line and column (1-based, in characters) of byte `offset` in `text`.
pub fn line_col(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let before = &text[..text.floor_char_boundary(offset)];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) as u32 + 1;
    (line, col)
}

/// The line where `key` is set in a TOML file (`key =` at the start of a
/// line, or `[key]` / `[[key]]`), for messages that name a key but carry no
/// position.
fn line_of_key(text: &str, key: &str) -> u32 {
    let last = key.rsplit('.').next().unwrap_or(key);
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        let table = t.trim_start_matches('[').trim_end().trim_end_matches(']');
        let set = t.strip_prefix(last).is_some_and(|rest| rest.trim_start().starts_with('='));
        if set || table == key || table == last {
            return i as u32 + 1;
        }
    }
    0
}

/// The TOML syntax of `text`, as at most one problem in `file`.
fn toml_problem<T: serde::de::DeserializeOwned>(file: &str, text: &str) -> Option<Problem> {
    let err = toml::from_str::<T>(text).err()?;
    let (line, col) = err.span().map(|s| line_col(text, s.start)).unwrap_or((0, 0));
    Some(Problem::new(file, line, col, Severity::Error, err.message().trim().to_owned()))
}

fn json_problem(file: &str, text: &str) -> Option<Problem> {
    let err = serde_json::from_str::<serde_json::Value>(text).err()?;
    let message = err.to_string();
    // serde_json appends " at line L column C"; the place has its own field.
    let message = message.split(" at line ").next().unwrap_or(&message).to_owned();
    Some(Problem::new(file, err.line() as u32, err.column() as u32, Severity::Error, message))
}

/// Problems with the module's files that need nothing run: the TOML and
/// JSON files' syntax and the script's. `manifest_text` is `module.toml`.
fn syntax_problems(dir: &Path, files: &[ModuleFile], manifest: Option<&Manifest>) -> Vec<Problem> {
    let mut out = Vec::new();
    for f in files.iter().filter(|f| f.text && f.path != "module.toml") {
        let lower = f.path.to_ascii_lowercase();
        let Ok(text) = std::fs::read_to_string(dir.join(&f.path)) else { continue };
        if lower.ends_with(".toml") {
            out.extend(toml_problem::<toml::Value>(&f.path, &text));
        } else if lower.ends_with(".json") {
            out.extend(json_problem(&f.path, text.trim_start_matches('\u{feff}')));
        }
    }
    #[cfg(feature = "js")]
    if let Some(m) = manifest.filter(|m| m.module.runtime == Runtime::Js) {
        let entry = if m.module.entry.is_empty() { "main.js" } else { m.module.entry.as_str() };
        if let Ok(text) = std::fs::read_to_string(dir.join(entry)) {
            let name = Path::new(entry).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Err(e) =
                glossa_modules::runtime::js::engine::check_syntax(text.trim_start_matches('\u{feff}'), &name)
            {
                let (bare, loc) = glossa_modules::runtime::js::engine::split_location(&e);
                let message = bare.strip_prefix(&format!("{name}: ")).unwrap_or(bare).to_owned();
                let (line, column) = loc.map(|l| (l.line, l.column)).unwrap_or((0, 0));
                out.push(Problem::new(entry, line, column, Severity::Error, message));
            }
        }
    }
    #[cfg(not(feature = "js"))]
    let _ = manifest;
    out
}

/// A harness finding as a problem: placed in the file its message names,
/// at the line of the key it quotes when it names no line.
fn finding_problem(dir: &Path, files: &[ModuleFile], severity: Severity, message: &str) -> Problem {
    let manifest_text = || std::fs::read_to_string(dir.join("module.toml")).unwrap_or_default();
    if let Some((file, line, col)) = find_place(message) {
        if let Some(f) = files.iter().find(|f| f.path == file || f.path.ends_with(&format!("/{file}"))) {
            return Problem::new(&f.path, line, col, severity, message);
        }
    }
    let (file, rest) = match message.split_once(": ") {
        Some((f, rest)) if files.iter().any(|x| x.path == f) => (f.to_owned(), rest),
        _ => (String::new(), message),
    };
    let mut line = 0;
    if file == "module.toml" {
        let text = manifest_text();
        if let Some(key) = rest.split('`').nth(1) {
            line = line_of_key(&text, key);
        }
        if line == 0 {
            // "module id …", "unknown capability …", "runtime = …".
            for (word, key) in [
                ("module id", "id"),
                ("capabilit", "capabilities"),
                ("runtime", "runtime"),
                ("protocol", "api"),
                ("option", "options"),
                ("language", "languages"),
                ("network", "network"),
            ] {
                if rest.contains(word) {
                    line = line_of_key(&text, key);
                    if line > 0 {
                        break;
                    }
                }
            }
        }
    }
    Problem::new(&file, line, 0, severity, rest)
}

impl Engine {
    /// Every module the editor can show: the user's own (editable), the
    /// installed and bundled ones (copied before editing), and folders in
    /// the user's modules folder that failed to load (to be fixed).
    pub fn module_sources(&self) -> Vec<ModuleListing> {
        let user_dir = self.inner.paths.user_module_dir();
        let registry = self.registry();
        let mut out: Vec<ModuleListing> = registry
            .entries()
            .iter()
            .filter_map(|e| {
                let (source, kind) = match &e.origin {
                    Origin::Builtin => return None,
                    Origin::Bundled => (ModuleSource::Bundled, "Bundled".to_owned()),
                    Origin::Module { dir, runtime, .. } if dir.starts_with(&user_dir) => {
                        (ModuleSource::User(dir.clone()), runtime.label().to_owned())
                    }
                    Origin::Module { dir, runtime, .. } => {
                        (ModuleSource::Installed(dir.clone()), runtime.label().to_owned())
                    }
                };
                Some(ModuleListing { id: e.info().id.clone(), name: e.info().name.clone(), source, kind, failed: None })
            })
            .collect();
        for (path, why) in &self.module_status().failed {
            // A folder whose reload failed still has its old provider
            // running: one row, saying why the new code did not load.
            if let Some(m) = out.iter_mut().find(|m| m.source.dir() == Some(path.as_path())) {
                m.failed = Some(why.clone());
                continue;
            }
            if path.starts_with(&user_dir) && path.is_dir() {
                let id = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(ModuleListing {
                    name: id.clone(),
                    id,
                    source: ModuleSource::User(path.clone()),
                    kind: String::new(),
                    failed: Some(why.clone()),
                });
            }
        }
        out.sort_by_key(|m| (!matches!(m.source, ModuleSource::User(_)), m.name.to_lowercase()));
        out
    }

    /// Where module `id` comes from, if it is loaded.
    pub fn module_source(&self, id: &str) -> Option<ModuleSource> {
        self.module_sources().into_iter().find(|m| m.id == id).map(|m| m.source)
    }

    /// Make a new module in the user's modules folder from `template`,
    /// named `name`, and load it. Returns its folder.
    pub fn new_module(&self, template: Template, name: &str) -> Result<PathBuf> {
        let user_dir = self.inner.paths.user_module_dir();
        let base = module_id_for(name);
        let taken = |id: &str| user_dir.join(id).exists() || self.registry().get(id).is_some();
        let id = (1..1000)
            .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
            .find(|id| !taken(id))
            .ok_or_else(|| Error::Config(format!("no free folder name for {base:?}")))?;
        let dir = user_dir.join(&id);
        write_template(template, &dir, &id, name)?;
        // Loading it is a convenience: a template that cannot load here
        // (no JavaScript in this build) is still there to edit.
        if let Err(e) = self.reload_module(&dir) {
            tracing::warn!(module = %id, "the new module did not load: {e}");
        }
        Ok(dir)
    }

    /// Copy a bundled or installed module into the user's modules folder,
    /// where it replaces the original by id, and load the copy. Returns the
    /// copy's folder. A user module is already where it can be edited.
    pub fn copy_module_to_user(&self, id: &str) -> Result<PathBuf> {
        let user_dir = self.inner.paths.user_module_dir();
        let source = self.module_source(id).ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
        let target = match &source {
            ModuleSource::User(dir) => return Ok(dir.clone()),
            ModuleSource::Builtin => {
                return Err(Error::Unsupported { provider: id.to_owned(), capability: "editing" });
            }
            ModuleSource::Bundled => {
                let b = crate::bundled::BUNDLED
                    .iter()
                    .find(|b| b.id == id)
                    .ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
                let target = user_dir.join(b.id);
                if target.exists() {
                    return Err(Error::Config(format!("{} already exists", target.display())));
                }
                save_module_file(&target, "module.toml", b.manifest)?;
                save_module_file(&target, "main.js", b.script)?;
                target
            }
            ModuleSource::Installed(dir) => {
                let name = dir.file_name().ok_or_else(|| Error::NoSuchModule(id.to_owned()))?;
                let target = user_dir.join(name);
                if target.exists() {
                    return Err(Error::Config(format!("{} already exists", target.display())));
                }
                copy_dir(dir, &target)?;
                target
            }
        };
        self.reload_module(&target)?;
        Ok(target)
    }

    /// Check a module's files without running it: the manifest's TOML (with
    /// the place of a syntax error), what the manifest says (the test
    /// harness's checks: unknown keys, capabilities, options, permissions,
    /// the entry file), the script's syntax, and the syntax of its other TOML
    /// and JSON files (fixtures, recordings).
    pub fn check_module(&self, dir: &Path) -> Vec<Problem> {
        let files = module_files(dir);
        let mut out = Vec::new();
        let manifest_path = dir.join("module.toml");
        let mut manifest = None;
        if let Ok(text) = std::fs::read_to_string(&manifest_path) {
            match toml_problem::<Manifest>("module.toml", &text) {
                Some(p) => out.push(p),
                None => {
                    let ctx = self.runtime_context(&self.config());
                    let lint = glossa_module_test::lint::check(dir, &ctx);
                    for f in &lint.findings {
                        out.push(finding_problem(dir, &files, f.severity, &f.message));
                    }
                    if let Some(why) = lint.missing {
                        out.push(Problem::new("", 0, 0, Severity::Note, why));
                    }
                    manifest = lint.manifest;
                }
            }
        }
        out.extend(syntax_problems(dir, &files, manifest.as_ref()));
        out.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| (&a.file, a.line).cmp(&(&b.file, b.line))));
        out.dedup();
        out
    }

    /// Load the module in `dir` again and put it in place of the provider
    /// with its id (and of one loaded earlier from the same folder), leaving
    /// every other provider as it is. On failure the old provider keeps
    /// running, and the folder is listed as failed with the reason.
    pub fn reload_module(&self, dir: &Path) -> Result<ReloadOutcome> {
        let cfg = self.config();
        let ctx = self.runtime_context(&cfg);
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // A native library stays loaded for the life of the process, so one
        // already loaded from this folder is not loaded again: decided from
        // the manifest alone, before anything is loaded.
        if let Ok(m) = Manifest::load(&dir.join("module.toml")) {
            if m.module.runtime == Runtime::Native && self.registry().entry_from_dir(&dir).is_some() {
                return Ok(ReloadOutcome::NeedsRestart { id: m.module.id });
            }
        }
        let entry = match Registry::load_module(&dir, &ctx) {
            Ok(entry) => entry,
            Err(why) => {
                self.set_module_status(&dir, None, Some(why.clone()));
                // The status changed (the folder is listed as failed now).
                self.emit(Event::ModulesReloaded);
                return Err(Error::Module { module: folder, detail: why });
            }
        };
        let id = entry.info().id.clone();
        if cfg.modules.disabled.contains(&id) {
            return Ok(ReloadOutcome::Disabled { id });
        }
        {
            let mut registry = self.inner.registry.write();
            let next = registry.with_replaced(entry);
            *registry = std::sync::Arc::new(next);
        }
        self.set_module_status(&dir, Some(&id), None);
        self.adopt_providers();
        self.emit(Event::ModulesReloaded);
        tracing::info!(module = %id, dir = %dir.display(), "module reloaded");
        Ok(ReloadOutcome::Reloaded { id })
    }

    fn set_module_status(&self, dir: &Path, loaded: Option<&str>, failed: Option<String>) {
        let mut status = self.inner.modules.write();
        let ModuleStatus { loaded: ids, failed: bad, skipped } = &mut *status;
        bad.retain(|(p, _)| p != dir);
        skipped.retain(|(p, _)| p != dir);
        if let Some(id) = loaded {
            if !ids.iter().any(|x| x == id) {
                ids.push(id.to_owned());
            }
        }
        if let Some(why) = failed {
            bad.push((dir.to_owned(), why));
        }
    }

    /// Run the module's tests (its `tests/` fixtures, offline, against the
    /// recorded answers) the way `glossa-cli module test` does, with this
    /// program's interpreters, call budget and downloaded module files.
    pub fn test_module(&self, dir: &Path) -> ModuleReport {
        let cfg = self.config();
        let settings = glossa_module_test::Settings {
            timeout: std::time::Duration::from_millis(cfg.modules.call_timeout_ms),
            python: cfg.modules.python.clone(),
            java: cfg.modules.java.clone(),
            models_dir: Some(self.inner.paths.models_dir()),
            python_dirs: Some(self.python_dirs()),
            ..Default::default()
        };
        glossa_module_test::test_module(dir, &settings)
    }

    /// Keep what module `id` logs from now on (`host.log`, `console.*`, a
    /// process module's stderr), for as long as the tap lives. The front
    /// end's waker is called after each line.
    pub fn module_log(&self, id: &str) -> LogTap {
        let waker = self.inner.waker.read().clone();
        LogTap::open(id, waker)
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        let p = e.path();
        if p.is_dir() {
            if !IGNORED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                copy_dir(&p, &to.join(&name))?;
            }
        } else {
            std::fs::copy(&p, to.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
