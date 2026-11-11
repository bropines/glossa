//! Presets for every front end: list, save, apply, rename, delete,
//! duplicate, export and import (see [`glossa_core::preset`] for what a
//! preset holds).
//!
//! The engine decides what counts as a secret here, because only it knows
//! the modules' declared option kinds; a preset never leaves with one. Nor
//! does it carry a command (an option whose value is run as a program):
//! none is saved, exported, imported, or applied from a preset already in
//! the settings.
//!
//! An imported preset is someone else's file, and is held to more:
//!
//! - it keeps only options the installed module declares;
//! - never an address (a URL, endpoint or host), since applying it would
//!   send the keys already stored for that service there;
//! - never a path or anything named like a program, an interpreter or a
//!   script, and never raw request headers or body fields;
//! - it cannot put a provider that runs a command (the external OCR
//!   command) into a role unless the user set that command up already;
//! - it can make language detection more private (`detect = "offline"`),
//!   never less;
//! - anything else in the file (a `[server]`, `[modules]` or `[network]`
//!   table, say) is not a preset's to set, and is reported and ignored.
//!
//! [`PresetReport`] lists what was left out and what applying the preset
//! will change, for the front end to show.
//!
//! Every change emits [`Event::PresetsChanged`], so a window holding an
//! unsaved copy of the settings (Options) can take it in.

use std::path::Path;
use std::sync::Arc;

use glossa_core::config::DetectMode;
use glossa_core::preset::{self, Preset};
use glossa_core::{Config, Error, LangPair, Result};
use glossa_modules::OptionKind;
use serde::{Deserialize, Serialize};

use crate::{Engine, Event};

/// A preset as lists show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetEntry {
    pub name: String,
    /// Comes with Glossa: cannot be renamed, changed or deleted, only
    /// duplicated.
    pub builtin: bool,
    /// The preset applied last.
    pub current: bool,
    /// The services it switches on, in order.
    pub enabled: Vec<String>,
    pub active: String,
    pub dictionary_enabled: Vec<String>,
    pub pair: Option<LangPair>,
    /// Services and modules it names that are not installed here.
    pub missing: Vec<String>,
}

/// What saving, applying or importing a preset did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetReport {
    /// The preset's name (for an import, the name it was stored under).
    pub name: String,
    /// Services and modules it names that are not installed: skipped when
    /// applying, listed when importing.
    pub missing: Vec<String>,
    /// Credentials left out, as `module.key` (saving, importing a file
    /// someone put a key into).
    pub dropped_secrets: Vec<String>,
    /// Options left out, as `module.key`. Always: commands (options whose
    /// value is run as a program). From an import also: addresses (URLs,
    /// endpoints, hosts), paths, raw request headers and fields, and
    /// options no installed module declares.
    #[serde(default)]
    pub dropped_options: Vec<String>,
    /// What an imported file asked for and was refused, as `key = value`:
    /// a role for a provider that runs a command the user has not set up
    /// (`ocr = command-ocr`), less private language detection
    /// (`detect = service`), and settings that are not a preset's
    /// (`server`, `modules`, `network`…).
    #[serde(default)]
    pub blocked: Vec<String>,
    /// What applying the preset changes, compared with the settings now
    /// (for an import: what it will change once applied).
    #[serde(default)]
    pub changes: Vec<PresetChange>,
}

/// One setting a preset changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetChange {
    /// `active`, `enabled`, `dictionary_active`, `dictionary_enabled`,
    /// `tts`, `ocr`, `pair`, `detect`, or `options.MODULE.KEY`.
    pub what: String,
    pub from: String,
    pub to: String,
    /// The new value brings in a provider that sends text, pictures or
    /// speech off this computer and was not in that role before.
    #[serde(default)]
    pub online: bool,
}

/// The largest preset file read, in bytes. Real ones are a few hundred.
pub const MAX_PRESET_FILE: u64 = 256 * 1024;

impl Engine {
    /// Built-in presets first, then the user's, in their order.
    pub fn presets(&self) -> Vec<PresetEntry> {
        let cfg = self.config();
        let registry = self.registry();
        let available = |id: &str| registry.get(id).is_some();
        let entry = |p: &Preset, builtin: bool| PresetEntry {
            name: p.name.clone(),
            builtin,
            current: preset::same_name(&p.name, &cfg.services.preset),
            enabled: p.enabled.clone(),
            active: p.active.clone(),
            dictionary_enabled: p.dictionary_enabled.clone(),
            pair: p.pair,
            missing: p.missing(&available),
        };
        let mut out: Vec<PresetEntry> = preset::builtin().iter().map(|p| entry(p, true)).collect();
        out.extend(cfg.presets.iter().map(|p| entry(p, false)));
        out
    }

    /// The preset called `name` (ignoring case), and whether it is built in.
    pub fn preset(&self, name: &str) -> Option<(Preset, bool)> {
        if let Some(p) = self.config().presets.into_iter().find(|p| preset::same_name(&p.name, name)) {
            return Some((p, false));
        }
        preset::builtin().into_iter().find(|p| preset::same_name(&p.name, name)).map(|p| (p, true))
    }

    fn require_preset(&self, name: &str) -> Result<(Preset, bool)> {
        self.preset(name).ok_or_else(|| Error::Config(format!("no preset called {name:?}")))
    }

    /// Whether an option holds a credential: declared as a secret by its
    /// module, of a type this build does not know (it may be a newer kind
    /// of secret), or named like one.
    fn option_is_secret(&self, module: &str, key: &str) -> bool {
        let declared = self
            .registry()
            .get(module)
            .is_some_and(|p| p.options().iter().any(|s| s.key == key && possibly_secret(&s.kind)));
        declared || preset::looks_secret(key)
    }

    /// Whether an option's value is run as a program: declared as a
    /// command, or named like one (a declared path is a path: kept in the
    /// user's own presets, left out of imported ones).
    fn option_is_command(&self, module: &str, key: &str) -> bool {
        let registry = self.registry();
        let kind = registry.get(module).and_then(|p| p.options().iter().find(|s| s.key == key).map(|s| s.kind.clone()));
        match kind {
            Some(OptionKind::Command) => true,
            Some(OptionKind::File) => false,
            _ => preset::looks_like_command(key),
        }
    }

    /// Remove every command from a preset's options; returns what was
    /// removed, as `module.key`.
    fn strip_commands(&self, p: &mut Preset) -> Vec<String> {
        let mut dropped = Vec::new();
        for (module, values) in p.options.iter_mut() {
            values.retain(|key, _| {
                let command = self.option_is_command(module, key);
                if command {
                    dropped.push(format!("{module}.{key}"));
                }
                !command
            });
        }
        p.options.retain(|_, v| !v.is_empty());
        dropped
    }

    /// Whether provider `id` runs a program named in its settings, and the
    /// user has not named one: the provider is not in use in any role and
    /// none of its command settings is filled in.
    fn runs_a_command_not_set_up(&self, id: &str, cfg: &Config) -> bool {
        let registry = self.registry();
        let Some(provider) = registry.get(id) else { return false };
        let commands: Vec<&str> =
            provider.options().iter().filter(|s| s.kind == OptionKind::Command).map(|s| s.key.as_str()).collect();
        if commands.is_empty() {
            return false;
        }
        let set_up = commands
            .iter()
            .any(|key| cfg.services.option(id, key).and_then(|v| v.as_str()).is_some_and(|v| !v.trim().is_empty()));
        !set_up
    }

    /// What applying `p` changes in `cfg`.
    fn changes(&self, p: &Preset, cfg: &Config) -> Vec<PresetChange> {
        let registry = self.registry();
        let mut after = cfg.clone();
        p.apply_to(&mut after, &|id| registry.get(id).is_some());
        let is_online = |id: &str| {
            registry.get(id).is_some_and(|prov| {
                let options = glossa_modules::provider::resolve_options(prov.options(), after.services.options.get(id));
                !prov.is_local(&options)
            })
        };
        let mut out = Vec::new();
        let mut one = |what: &str, from: &str, to: &str, online: bool| {
            if from != to {
                out.push(PresetChange { what: what.to_owned(), from: from.to_owned(), to: to.to_owned(), online });
            }
        };
        let (b, a) = (&cfg.services, &after.services);
        let list = |before: &[String], now: &[String]| now.iter().any(|id| !before.contains(id) && is_online(id));
        one("active", &b.active, &a.active, false);
        one("enabled", &b.enabled.join(", "), &a.enabled.join(", "), list(&b.enabled, &a.enabled));
        one("dictionary_active", &b.dictionary_active, &a.dictionary_active, false);
        one(
            "dictionary_enabled",
            &b.dictionary_enabled.join(", "),
            &a.dictionary_enabled.join(", "),
            list(&b.dictionary_enabled, &a.dictionary_enabled),
        );
        one("tts", &b.tts, &a.tts, is_online(&a.tts));
        one("ocr", &cfg.ocr.provider, &after.ocr.provider, is_online(&after.ocr.provider));
        let pair = |p: LangPair| format!("{} > {}", p.from.bcp47(), p.to.bcp47());
        one("pair", &pair(cfg.general.pair), &pair(after.general.pair), false);
        one(
            "detect",
            cfg.general.detect.name(),
            after.general.detect.name(),
            after.general.detect != DetectMode::Offline,
        );
        for (module, values) in &p.options {
            for (key, value) in values {
                let show = |v: Option<&serde_json::Value>| match v {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                let before = show(b.option(module, key));
                if a.option(module, key) == Some(value) {
                    one(&format!("options.{module}.{key}"), &before, &show(Some(value)), false);
                }
            }
        }
        out
    }

    /// Save the current services as a preset called `name`, replacing a
    /// preset of the user's with that name. `with_pair` records the
    /// language pair too.
    pub fn save_preset(&self, name: &str, with_pair: bool) -> Result<PresetReport> {
        let name = check_name(name)?;
        if preset::builtin().iter().any(|p| preset::same_name(&p.name, &name)) {
            return Err(Error::Config(format!("{name:?} is a built-in preset; choose another name")));
        }
        let cfg = self.config();
        let mut p = Preset::capture(&name, &cfg, with_pair, &|_, _| false);
        let dropped = p.strip_secrets(&|m, k| self.option_is_secret(m, k));
        let dropped_options = self.strip_commands(&mut p);
        self.update_config(|c| {
            match c.presets.iter_mut().find(|q| preset::same_name(&q.name, &name)) {
                Some(q) => *q = p.clone(),
                None => c.presets.push(p.clone()),
            }
            c.services.preset = p.name.clone();
        })?;
        self.presets_changed(None);
        Ok(PresetReport { name, dropped_secrets: dropped, dropped_options, ..PresetReport::default() })
    }

    /// Apply a preset. Services it names that are not installed are skipped
    /// and listed in the report. Writes the settings file, so front ends
    /// with a UI call [`Engine::apply_preset_async`].
    pub fn apply_preset(&self, name: &str) -> Result<PresetReport> {
        let (mut p, _) = self.require_preset(name)?;
        // A preset never sets a command, wherever it came from: one stored
        // by an older version's import, or pasted into the settings file,
        // is applied without it.
        let dropped_options = self.strip_commands(&mut p);
        if !dropped_options.is_empty() {
            tracing::warn!(preset = %p.name, dropped = ?dropped_options, "commands in the preset were not applied");
        }
        let registry = self.registry();
        let available = |id: &str| registry.get(id).is_some();
        let changes = self.changes(&p, &self.config());
        let mut applied = Default::default();
        self.update_config(|c| applied = p.apply_to(c, &available))?;
        if !applied.missing.is_empty() {
            tracing::info!(preset = %p.name, missing = ?applied.missing, "preset applied without some services");
        }
        let name = p.name.clone();
        self.presets_changed(Some(p));
        Ok(PresetReport { name, missing: applied.missing, dropped_options, changes, ..PresetReport::default() })
    }

    /// [`Engine::apply_preset`] on a worker thread; the outcome comes back
    /// as [`Event::PresetApplied`].
    pub fn apply_preset_async(&self, name: String) {
        let engine = self.clone();
        self.spawn_or(
            "preset",
            move || {
                let result = engine.apply_preset(&name).map_err(Arc::new);
                engine.emit(Event::PresetApplied { result });
            },
            |e| self.emit(Event::PresetApplied { result: Err(Arc::new(e)) }),
        );
    }

    /// The preset after the current one (the first when none is current),
    /// in [`Engine::presets`] order.
    pub fn next_preset_name(&self) -> Option<String> {
        let all = self.presets();
        if all.is_empty() {
            return None;
        }
        let next = all.iter().position(|p| p.current).map(|i| (i + 1) % all.len()).unwrap_or(0);
        Some(all[next].name.clone())
    }

    /// Apply the next preset.
    pub fn next_preset(&self) -> Result<PresetReport> {
        let name = self.next_preset_name().ok_or_else(|| Error::Config("there are no presets".into()))?;
        self.apply_preset(&name)
    }

    /// Rename one of the user's presets.
    pub fn rename_preset(&self, old: &str, new: &str) -> Result<()> {
        let new = check_name(new)?;
        let (p, builtin) = self.require_preset(old)?;
        if builtin {
            return Err(Error::Config(format!("{:?} is built in and cannot be renamed; duplicate it", p.name)));
        }
        if !preset::same_name(&p.name, &new) && self.preset(&new).is_some() {
            return Err(Error::Config(format!("there is already a preset called {new:?}")));
        }
        self.update_config(|c| {
            if let Some(q) = c.presets.iter_mut().find(|q| q.name == p.name) {
                q.name = new.clone();
            }
            if preset::same_name(&c.services.preset, &p.name) {
                c.services.preset = new.clone();
            }
        })?;
        self.presets_changed(None);
        Ok(())
    }

    /// Delete one of the user's presets.
    pub fn delete_preset(&self, name: &str) -> Result<()> {
        let (p, builtin) = self.require_preset(name)?;
        if builtin {
            return Err(Error::Config(format!("{:?} is built in and cannot be deleted", p.name)));
        }
        self.update_config(|c| {
            c.presets.retain(|q| q.name != p.name);
            if preset::same_name(&c.services.preset, &p.name) {
                c.services.preset.clear();
            }
        })?;
        self.presets_changed(None);
        Ok(())
    }

    /// Copy a preset (a built-in one included) under `new_name`, or under
    /// "NAME (2)" when none is given. Returns the copy's name.
    pub fn duplicate_preset(&self, name: &str, new_name: Option<&str>) -> Result<String> {
        let (mut p, _) = self.require_preset(name)?;
        let taken = |n: &str| self.preset(n).is_some();
        p.name = match new_name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) if taken(n) => return Err(Error::Config(format!("there is already a preset called {n:?}"))),
            Some(n) => check_name(n)?,
            None => check_name(&preset::unique_name(&p.name, &taken))?,
        };
        let copy = p.name.clone();
        self.update_config(|c| c.presets.push(p))?;
        self.presets_changed(None);
        Ok(copy)
    }

    /// A preset as the text of an exported file, secrets excluded.
    pub fn export_preset_text(&self, name: &str) -> Result<String> {
        let (mut p, _) = self.require_preset(name)?;
        // Saved presets hold no secrets already; this catches one a user
        // typed into config.toml by hand.
        p.strip_secrets(&|m, k| self.option_is_secret(m, k));
        self.strip_commands(&mut p);
        Ok(p.to_file())
    }

    /// Export a preset to a `.toml` file.
    pub fn export_preset(&self, name: &str, path: &Path) -> Result<()> {
        let text = self.export_preset_text(name)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Add a preset from an exported file's text. A name already taken
    /// gets a number. The file is someone else's, so it is cut down to what
    /// a shared preset may decide (the module documentation above lists
    /// what goes); the report says what was left out, what was refused and
    /// what applying it will change. Nothing is applied here. Services it
    /// names that are not installed are listed; it is stored anyway, so it
    /// works once they are.
    pub fn import_preset_text(&self, text: &str) -> Result<PresetReport> {
        if text.len() as u64 > MAX_PRESET_FILE {
            return Err(Error::Config(format!("a preset file is at most {} KB", MAX_PRESET_FILE / 1024)));
        }
        let mut p = Preset::from_file(text)?;
        let name = check_name(&p.name)?;
        let registry = self.registry();
        let cfg = self.config();
        let mut dropped_secrets = Vec::new();
        let mut dropped_options = Vec::new();
        let mut blocked = foreign_keys(text);

        // A provider that runs a command stays out of every role unless the
        // user set that command up: the file must not be what starts it.
        let mut refuse = |role: &str, id: &str| -> bool {
            let refused = self.runs_a_command_not_set_up(id, &cfg);
            if refused {
                blocked.push(format!("{role} = {id}"));
            }
            refused
        };
        for (role, id) in [
            ("ocr", &mut p.ocr),
            ("tts", &mut p.tts),
            ("active", &mut p.active),
            ("dictionary_active", &mut p.dictionary_active),
        ] {
            if !id.is_empty() && refuse(role, id) {
                id.clear();
            }
        }
        for (role, ids) in [
            ("enabled", &mut p.enabled),
            ("order", &mut p.order),
            ("dictionary_enabled", &mut p.dictionary_enabled),
            ("dictionary_order", &mut p.dictionary_order),
        ] {
            ids.retain(|id| !refuse(role, id));
        }
        blocked.dedup();
        // Detection may become more private, never less.
        if let Some(mode) = p.detect.filter(|m| *m != DetectMode::Offline) {
            if mode != cfg.general.detect {
                blocked.push(format!("detect = {}", mode.name()));
            }
            p.detect = None;
        }

        for (module, values) in p.options.iter_mut() {
            let specs = registry.get(module).map(|m| m.options().to_vec()).unwrap_or_default();
            values.retain(|key, value| {
                let label = format!("{module}.{key}");
                let Some(spec) = specs.iter().find(|s| &s.key == key) else {
                    dropped_options.push(label);
                    return false;
                };
                if possibly_secret(&spec.kind)
                    || preset::looks_secret(key)
                    || value.as_str() == Some(glossa_core::secrets::IN_STORE)
                {
                    dropped_secrets.push(label);
                    return false;
                }
                if spec.kind.is_executable()
                    || preset::looks_like_command(key)
                    || preset::looks_like_raw_request(key)
                    || spec.kind == OptionKind::Url
                    || preset::looks_like_address(key, value)
                {
                    dropped_options.push(label);
                    return false;
                }
                true
            });
        }
        p.options.retain(|_, v| !v.is_empty());
        p.name = check_name(&preset::unique_name(&name, &|n| self.preset(n).is_some()))?;
        let missing = p.missing(&|id| registry.get(id).is_some());
        let name = p.name.clone();
        if !dropped_options.is_empty() || !blocked.is_empty() {
            tracing::info!(preset = %name, dropped = ?dropped_options, blocked = ?blocked, "import left settings out");
        }
        let changes = self.changes(&p, &cfg);
        self.update_config(|c| c.presets.push(p))?;
        self.presets_changed(None);
        Ok(PresetReport { name, missing, dropped_secrets, dropped_options, blocked, changes })
    }

    /// Import a preset from a file of at most [`MAX_PRESET_FILE`] bytes.
    pub fn import_preset(&self, path: &Path) -> Result<PresetReport> {
        let cannot = |e: std::io::Error| Error::Config(format!("cannot read {}: {e}", path.display()));
        let size = std::fs::metadata(path).map_err(cannot)?.len();
        if size > MAX_PRESET_FILE {
            return Err(Error::Config(format!(
                "{} is too large for a preset file ({size} bytes; at most {} KB)",
                path.display(),
                MAX_PRESET_FILE / 1024
            )));
        }
        let text = std::fs::read_to_string(path).map_err(cannot)?;
        self.import_preset_text(&text)
    }

    /// Tell front ends the presets or the current one changed.
    fn presets_changed(&self, applied: Option<Preset>) {
        self.emit(Event::PresetsChanged { applied: applied.map(Arc::new) });
    }
}

/// The top-level keys of a preset file that are not a preset's: tables
/// lifted from `config.toml` (`server`, `modules`, `network`, `classic`…),
/// or anything a newer or a made-up format adds. They are never read; they
/// are reported so that a file asking for more than a preset may is seen
/// to have asked.
fn foreign_keys(text: &str) -> Vec<String> {
    const KNOWN: [&str; 13] = [
        "glossa_preset",
        "name",
        "order",
        "enabled",
        "active",
        "dictionary_order",
        "dictionary_enabled",
        "dictionary_active",
        "tts",
        "ocr",
        "pair",
        "detect",
        "options",
    ];
    let Ok(table) = toml::from_str::<toml::Table>(text) else { return Vec::new() };
    table.keys().filter(|k| !KNOWN.contains(&k.as_str())).cloned().collect()
}

/// A secret, or a type from a newer format whose nature is unknown: never
/// put in a preset, so never exported.
fn possibly_secret(kind: &OptionKind) -> bool {
    matches!(kind, OptionKind::Secret | OptionKind::Unknown(_))
}

fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Config("a preset needs a name".into()));
    }
    if name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(Error::Config("a preset name is one line of at most 80 characters".into()));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::{Config, Paths};
    use serde_json::json;

    fn engine(name: &str) -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("glossa-presets-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let paths = Paths::for_exe_dir(dir.clone(), Some(dir.join("home")));
        (Engine::with_config(paths, Config::default()).unwrap(), dir)
    }

    #[test]
    fn save_switch_and_come_back() {
        let (e, dir) = engine("round-trip");
        e.update_config(|c| {
            c.services.enabled = vec!["google".into(), "bing".into()];
            c.services.active = "bing".into();
            c.services.set_option("deepl", "api_key", json!("dk-fake-key"));
            c.services.set_option("google", "domain", json!("com"));
        })
        .unwrap();
        let saved = e.save_preset("Work", false).unwrap();
        assert!(saved.dropped_secrets.contains(&"deepl.api_key".to_owned()), "{saved:?}");

        e.apply_preset("Private").unwrap();
        assert_eq!(e.config().services.active, "lingva");
        assert_eq!(e.config().services.preset, "Private");

        let back = e.apply_preset("work").unwrap();
        assert!(back.missing.is_empty(), "{back:?}");
        let cfg = e.config();
        assert_eq!(cfg.services.enabled, vec!["google".to_owned(), "bing".to_owned()]);
        assert_eq!(cfg.services.active, "bing");
        // The key was never touched.
        assert_eq!(cfg.services.option("deepl", "api_key"), Some(&json!("dk-fake-key")));
        // It survives a restart.
        let reloaded = Config::load(&e.paths().config_file()).unwrap();
        assert_eq!(reloaded.presets.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn secrets_are_never_exported() {
        let (e, dir) = engine("export");
        e.update_config(|c| {
            c.services.set_option("deepl", "api_key", json!("dk-fake-key"));
            c.services.set_option("deepl", "formality", json!("more"));
            // Typed into config.toml by hand, past the save.
            c.presets.push(Preset {
                name: "Hand".into(),
                options: [(
                    "deepl".to_owned(),
                    [("api_key".to_owned(), json!("dk-fake-key")), ("formality".to_owned(), json!("less"))]
                        .into_iter()
                        .collect(),
                )]
                .into_iter()
                .collect(),
                ..Preset::default()
            });
        })
        .unwrap();
        e.save_preset("Saved", true).unwrap();
        for name in ["Saved", "Hand"] {
            let file = dir.join(format!("{name}.toml"));
            e.export_preset(name, &file).unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(!text.contains("dk-fake-key"), "{name}: {text}");
            assert!(text.contains("formality"), "{name}: {text}");
        }
        // An imported file carrying a key loses it.
        let report = e
            .import_preset_text(
                "glossa_preset = 1\nname = \"Saved\"\n[options.deepl]\napi_key = \"t-fake\"\nformality = \"less\"\n",
            )
            .unwrap();
        assert_eq!(report.name, "Saved (2)");
        assert_eq!(report.dropped_secrets, vec!["deepl.api_key".to_owned()]);
        let (p, _) = e.preset("Saved (2)").unwrap();
        assert!(!p.options["deepl"].contains_key("api_key"));
        assert_eq!(p.options["deepl"].get("formality"), Some(&json!("less")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The attack: a shared preset that points a service the user has a
    /// key for at another server. Applying it would send that key there.
    #[test]
    fn an_imported_preset_cannot_redirect_stored_keys() {
        let (e, dir) = engine("redirect");
        let endpoint = e.add_ai_endpoint("Mine", "openai").unwrap();
        e.update_config(|c| {
            c.services.set_option("deeplx", "token", json!("dx-fake"));
            c.services.set_option("libretranslate", "api_key", json!("lt-fake"));
            c.services.set_option(&endpoint, "api_key", json!("ai-fake"));
        })
        .unwrap();
        let file = format!(
            "glossa_preset = 1\nname = \"Shared\"\nenabled = [\"deeplx\", \"libretranslate\"]\n\
             [options.deeplx]\nurl = \"https://evil.example/translate\"\n\
             [options.libretranslate]\nurl = \"https://evil.example\"\n\
             [options.{endpoint}]\nbase_url = \"https://evil.example/v1\"\nmodel = \"https://evil.example/m\"\n\
             [options.not-installed]\nwhatever = \"x\"\n\
             [options.deepl]\nnot_declared = \"x\"\n"
        );
        let report = e.import_preset_text(&file).unwrap();
        for dropped in ["deeplx.url", "libretranslate.url", "not-installed.whatever", "deepl.not_declared"] {
            assert!(report.dropped_options.contains(&dropped.to_owned()), "{dropped}: {report:?}");
        }
        assert!(report.dropped_options.contains(&format!("{endpoint}.model")), "{report:?}");
        let (p, _) = e.preset("Shared").unwrap();
        assert!(p.options.is_empty(), "{:?}", p.options);
        let before = e.config().services.options.clone();
        e.apply_preset("Shared").unwrap();
        assert_eq!(e.config().services.options, before, "no option changed");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The attack the audit reproduced: a shared preset that sets the
    /// external OCR command and switches OCR to it, so the next text
    /// recognition runs a program of the file's choosing.
    #[cfg(unix)]
    #[test]
    fn an_imported_preset_cannot_run_a_program() {
        let (e, dir) = engine("command");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("pwned");
        let before = e.config();
        let file = format!(
            "glossa_preset = 1\nname = \"Fast OCR\"\nocr = \"command-ocr\"\n\
             [options.command-ocr]\ncommand = \"sh -c 'touch {}' {{image}}\"\ntimeout_secs = 5\nauto_lang3 = \"deu\"\n",
            marker.display()
        );
        let report = e.import_preset_text(&file).unwrap();
        assert_eq!(report.dropped_options, vec!["command-ocr.command".to_owned()], "{report:?}");
        assert_eq!(report.blocked, vec!["ocr = command-ocr".to_owned()], "{report:?}");
        assert!(report.changes.iter().all(|c| c.what != "ocr"), "{report:?}");
        let (stored, _) = e.preset("Fast OCR").unwrap();
        assert!(stored.ocr.is_empty());
        assert!(!stored.options.get("command-ocr").is_some_and(|o| o.contains_key("command")));

        e.apply_preset("Fast OCR").unwrap();
        let cfg = e.config();
        assert_eq!(cfg.ocr.provider, before.ocr.provider, "OCR was not switched");
        assert_eq!(cfg.services.option("command-ocr", "command"), None);
        // Harmless settings of the same module still come through.
        assert_eq!(cfg.services.option("command-ocr", "auto_lang3"), Some(&json!("deu")));
        let png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let _ = e.recognize_using("command-ocr", png, glossa_core::LangId::ENGLISH, None);
        assert!(!marker.exists(), "the preset's program ran");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A preset already in the settings (imported by an older version, or
    /// pasted into config.toml) does not get its command applied either,
    /// and the user's own command survives every preset.
    #[test]
    fn no_preset_sets_or_carries_a_command() {
        let (e, dir) = engine("command-apply");
        let options = |command: &str| -> preset::PresetOptions {
            [("command-ocr".to_owned(), [("command".to_owned(), json!(command))].into_iter().collect())]
                .into_iter()
                .collect()
        };
        e.update_config(|c| {
            c.services.set_option("command-ocr", "command", json!("tesseract {image} -"));
            c.presets.push(Preset {
                name: "Old import".into(),
                ocr: "command-ocr".into(),
                options: options("evil {image}"),
                ..Preset::default()
            });
        })
        .unwrap();
        let report = e.apply_preset("Old import").unwrap();
        assert_eq!(report.dropped_options, vec!["command-ocr.command".to_owned()]);
        let cfg = e.config();
        assert_eq!(cfg.services.option("command-ocr", "command"), Some(&json!("tesseract {image} -")));
        // The user set the command up, so their own preset may choose it.
        assert_eq!(cfg.ocr.provider, "command-ocr");

        let saved = e.save_preset("Mine", false).unwrap();
        assert!(saved.dropped_options.contains(&"command-ocr.command".to_owned()), "{saved:?}");
        let text = e.export_preset_text("Mine").unwrap();
        assert!(!text.contains("tesseract {image}"), "{text}");
        assert!(text.contains("ocr = \"command-ocr\""), "{text}");

        // With the command set up, a shared preset may select the engine
        // (it runs the user's command), but still not replace the command.
        let report = e
            .import_preset_text("name = \"Shared\"\nocr = \"command-ocr\"\n[options.command-ocr]\ncommand = \"evil\"\n")
            .unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert_eq!(report.dropped_options, vec!["command-ocr.command".to_owned()]);
        assert_eq!(e.preset("Shared").unwrap().0.ocr, "command-ocr");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A module's own program-like settings: declared as `command` or
    /// `file`, or only named like one.
    #[test]
    fn an_imported_preset_sets_no_path_interpreter_or_raw_request_of_a_module() {
        let (e, dir) = engine("module-commands");
        let m = e.paths().user_module_dir().join("runner");
        std::fs::create_dir_all(&m).unwrap();
        let option =
            |key: &str, kind: &str| format!("[[options]]\nkey = \"{key}\"\ntype = \"{kind}\"\nlabel = \"{key}\"\n");
        let manifest = format!(
            "capabilities = [\"translate\"]\n[module]\nid = \"runner\"\nname = \"Runner\"\nruntime = \"js\"\n{}{}{}{}{}{}{}",
            option("launcher", "command"),
            option("model", "file"),
            option("python_path", "text"),
            option("player_cmd", "text"),
            option("browser", "text"),
            option("extra_headers", "multiline"),
            option("style", "text"),
        );
        std::fs::write(m.join("module.toml"), manifest).unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return req.text; }").unwrap();
        e.reload_modules();
        assert!(e.registry().get("runner").is_some(), "{:?}", e.module_status().failed);
        let report = e
            .import_preset_text(
                "name = \"R\"\nenabled = [\"runner\", \"google\"]\nactive = \"runner\"\n[options.runner]\nlauncher = \"/tmp/x.sh\"\n\
                 model = \"/tmp/evil.bin\"\npython_path = \"/tmp/python\"\nplayer_cmd = \"sh -c x\"\nbrowser = \"/tmp/b\"\n\
                 extra_headers = \"X-Forwarded-For: 1\"\nstyle = \"formal\"\n",
            )
            .unwrap();
        for key in ["launcher", "model", "python_path", "player_cmd", "browser", "extra_headers"] {
            assert!(report.dropped_options.contains(&format!("runner.{key}")), "{key}: {report:?}");
        }
        let (p, _) = e.preset("R").unwrap();
        assert_eq!(p.options["runner"].keys().collect::<Vec<_>>(), vec!["style"]);
        // It declares a command the user never filled in: no role for it.
        assert!(report.blocked.contains(&"active = runner".to_owned()), "{report:?}");
        assert!(report.blocked.contains(&"enabled = runner".to_owned()), "{report:?}");
        assert_eq!(p.enabled, vec!["google".to_owned()]);
        assert!(p.active.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// What else a file might ask for that is not a preset's to give.
    #[test]
    fn an_imported_preset_changes_nothing_outside_a_presets_remit() {
        let (e, dir) = engine("foreign");
        let before = e.config();
        let report = e
            .import_preset_text(
                "glossa_preset = 1\nname = \"Sneaky\"\ndetect = \"service\"\n\
                 [server]\nenabled = true\nbind = \"0.0.0.0\"\ntoken = \"known\"\n\
                 [modules]\nallow_native = true\nallow_legacy_scripts = true\ntrusted = [\"evil\"]\ndirs = [\"/tmp/evil\"]\n\
                 [network]\noffline_only = false\n[network.proxy]\nmode = \"manual\"\nhost = \"evil.example\"\n\
                 [classic]\npath = \"/tmp/evil\"\n[general]\nsecret_storage = \"file\"\n",
            )
            .unwrap();
        for key in ["server", "modules", "network", "classic", "general"] {
            assert!(report.blocked.contains(&key.to_owned()), "{key}: {report:?}");
        }
        e.apply_preset("Sneaky").unwrap();
        let after = e.config();
        assert_eq!(after.server, before.server);
        assert_eq!(after.modules, before.modules);
        assert_eq!(after.network, before.network);
        assert_eq!(after.classic, before.classic);
        assert_eq!(after.ocr, before.ocr);
        assert_eq!(after.general.secret_storage, before.general.secret_storage);
        assert_eq!(after.general.detect, before.general.detect, "detection did not become less private");

        // `ocr` as a table is not a preset: the file is refused whole.
        assert!(e.import_preset_text("name = \"T\"\n[ocr]\nprovider = \"lens\"\n").is_err());

        // More private detection is a preset's to ask for.
        e.update_config(|c| c.general.detect = DetectMode::Service).unwrap();
        let report = e.import_preset_text("name = \"Quiet\"\ndetect = \"offline\"\n").unwrap();
        assert!(report.blocked.is_empty(), "{report:?}");
        assert!(report.changes.iter().any(|c| c.what == "detect" && c.to == "offline" && !c.online), "{report:?}");
        let report = e.import_preset_text("name = \"Loud\"\ndetect = \"offline_then_service\"\n").unwrap();
        assert_eq!(report.blocked, vec!["detect = offline_then_service".to_owned()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_import_says_what_applying_it_will_change() {
        let (e, dir) = engine("changes");
        e.update_config(|c| {
            c.ocr.provider = "tesseract".into();
            c.services.tts = "system-tts".into();
            c.services.enabled = vec!["lingva".into()];
            c.services.active = "lingva".into();
            c.services.set_option("deepl", "formality", json!("more"));
        })
        .unwrap();
        let report = e
            .import_preset_text(
                "name = \"Shared\"\nenabled = [\"lingva\", \"google\"]\nactive = \"google\"\nocr = \"lens\"\ntts = \"google\"\n\
                 [options.deepl]\nformality = \"less\"\n",
            )
            .unwrap();
        let change =
            |what: &str| report.changes.iter().find(|c| c.what == what).unwrap_or_else(|| panic!("{what}: {report:?}"));
        assert_eq!((change("ocr").from.as_str(), change("ocr").to.as_str()), ("tesseract", "lens"));
        assert!(change("ocr").online, "pictures would start going to Google");
        assert!(change("tts").online);
        assert_eq!(change("active").to, "google");
        assert!(change("enabled").online && change("enabled").to.contains("google"));
        assert_eq!(
            (change("options.deepl.formality").from.as_str(), change("options.deepl.formality").to.as_str()),
            ("more", "less")
        );
        // Nothing was applied by importing.
        assert_eq!(e.config().ocr.provider, "tesseract");
        // Applying reports the same.
        let applied = e.apply_preset("Shared").unwrap();
        assert!(applied.changes.iter().any(|c| c.what == "ocr" && c.to == "lens"), "{applied:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An option of a type this build does not know may be a newer kind of
    /// secret: it is never saved into a preset, exported or imported.
    #[test]
    fn options_of_unknown_types_are_treated_as_secrets() {
        let (e, dir) = engine("unknown-kind");
        let m = e.paths().user_module_dir().join("future");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(
            m.join("module.toml"),
            "capabilities = [\"translate\"]\n[module]\nid = \"future\"\nname = \"Future\"\nruntime = \"js\"\n\
             [[options]]\nkey = \"passkey_blob\"\ntype = \"webauthn\"\nlabel = \"X\"\n\
             [[options]]\nkey = \"style\"\ntype = \"text\"\nlabel = \"Style\"\n",
        )
        .unwrap();
        std::fs::write(m.join("main.js"), "function translate(req) { return req.text; }").unwrap();
        e.reload_modules();
        assert!(e.registry().get("future").is_some());
        e.update_config(|c| {
            c.services.set_option("future", "passkey_blob", json!("blob-fake"));
            c.services.set_option("future", "style", json!("plain"));
        })
        .unwrap();
        let saved = e.save_preset("F", false).unwrap();
        assert!(saved.dropped_secrets.contains(&"future.passkey_blob".to_owned()), "{saved:?}");
        let text = e.export_preset_text("F").unwrap();
        assert!(!text.contains("blob-fake") && text.contains("plain"), "{text}");
        let report =
            e.import_preset_text("name = \"G\"\n[options.future]\npasskey_blob = \"x\"\nstyle = \"bold\"\n").unwrap();
        assert_eq!(report.dropped_secrets, vec!["future.passkey_blob".to_owned()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imported_and_copied_names_are_checked_and_files_capped() {
        let (e, dir) = engine("names");
        assert!(e.import_preset_text("name = \"a\\nb\"\n").is_err(), "a newline");
        let long = "x".repeat(81);
        assert!(e.import_preset_text(&format!("name = \"{long}\"\n")).is_err(), "too long");
        let huge = format!("name = \"big\"\n# {}\n", "x".repeat(MAX_PRESET_FILE as usize));
        assert!(e.import_preset_text(&huge).is_err());
        let path = dir.join("huge.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, &huge).unwrap();
        assert!(e.import_preset(&path).is_err());
        assert!(e.duplicate_preset("Everyday", Some("two\nlines")).is_err());
        assert!(e.duplicate_preset("Everyday", Some(&long)).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_change_is_announced() {
        let (e, dir) = engine("events");
        let rx = e.subscribe();
        let changes = |rx: &std::sync::mpsc::Receiver<Event>| {
            rx.try_iter()
                .filter_map(|ev| match ev {
                    Event::PresetsChanged { applied } => Some(applied.map(|p| p.name.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        e.save_preset("A", false).unwrap();
        e.apply_preset("Private").unwrap();
        e.rename_preset("A", "B").unwrap();
        let copy = e.duplicate_preset("B", None).unwrap();
        e.delete_preset(&copy).unwrap();
        e.import_preset_text("name = \"C\"\n").unwrap();
        assert_eq!(changes(&rx), vec![None, Some("Private".to_owned()), None, None, None, None]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_module_is_skipped_with_a_note() {
        let (e, dir) = engine("missing");
        e.import_preset_text(
            "name = \"Japanese reading\"\nenabled = [\"not-installed\", \"google\"]\norder = [\"not-installed\", \"google\"]\nactive = \"not-installed\"\n",
        )
        .unwrap();
        let report = e.apply_preset("Japanese reading").unwrap();
        assert_eq!(report.missing, vec!["not-installed".to_owned()]);
        let cfg = e.config();
        assert_eq!(cfg.services.enabled, vec!["google".to_owned()]);
        assert_eq!(cfg.services.active, "google");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn built_ins_are_read_only_and_cycle_with_the_rest() {
        let (e, dir) = engine("builtin");
        assert!(e.delete_preset("Everyday").is_err());
        assert!(e.rename_preset("Everyday", "Mine").is_err());
        assert!(e.save_preset("everyday", false).is_err());
        let copy = e.duplicate_preset("Everyday", None).unwrap();
        assert_eq!(copy, "Everyday (2)");
        e.rename_preset(&copy, "Mine").unwrap();
        let names: Vec<String> = e.presets().into_iter().map(|p| p.name).collect();
        assert_eq!(names.last().map(String::as_str), Some("Mine"));
        let mut seen = Vec::new();
        for _ in 0..names.len() {
            seen.push(e.next_preset().unwrap().name);
        }
        assert_eq!(seen, names);
        e.delete_preset("Mine").unwrap();
        assert!(e.config().services.preset.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
