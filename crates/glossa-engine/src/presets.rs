//! Presets for every front end: list, save, apply, rename, delete,
//! duplicate, export and import (see [`glossa_core::preset`] for what a
//! preset holds).
//!
//! The engine decides what counts as a secret here, because only it knows
//! the modules' declared option kinds; a preset never leaves with one.

use std::path::Path;
use std::sync::Arc;

use glossa_core::preset::{self, Preset};
use glossa_core::{Error, LangPair, Result};
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
}

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
    /// module, or (for a module not loaded, whose declarations are unknown)
    /// named like one.
    fn option_is_secret(&self, module: &str, key: &str) -> bool {
        let declared = self
            .registry()
            .get(module)
            .is_some_and(|p| p.options().iter().any(|s| s.key == key && s.kind == OptionKind::Secret));
        declared || preset::looks_secret(key)
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
        self.update_config(|c| {
            match c.presets.iter_mut().find(|q| preset::same_name(&q.name, &name)) {
                Some(q) => *q = p.clone(),
                None => c.presets.push(p.clone()),
            }
            c.services.preset = p.name.clone();
        })?;
        Ok(PresetReport { name, missing: Vec::new(), dropped_secrets: dropped })
    }

    /// Apply a preset. Services it names that are not installed are skipped
    /// and listed in the report. Writes the settings file, so front ends
    /// with a UI call [`Engine::apply_preset_async`].
    pub fn apply_preset(&self, name: &str) -> Result<PresetReport> {
        let (p, _) = self.require_preset(name)?;
        let registry = self.registry();
        let available = |id: &str| registry.get(id).is_some();
        let mut applied = Default::default();
        self.update_config(|c| applied = p.apply_to(c, &available))?;
        if !applied.missing.is_empty() {
            tracing::info!(preset = %p.name, missing = ?applied.missing, "preset applied without some services");
        }
        Ok(PresetReport { name: p.name, missing: applied.missing, dropped_secrets: Vec::new() })
    }

    /// [`Engine::apply_preset`] on a worker thread; the outcome comes back
    /// as [`Event::PresetApplied`].
    pub fn apply_preset_async(&self, name: String) {
        let engine = self.clone();
        self.spawn("preset", move || {
            let result = engine.apply_preset(&name).map_err(Arc::new);
            engine.emit(Event::PresetApplied { result });
        });
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
        })
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
        })
    }

    /// Copy a preset (a built-in one included) under `new_name`, or under
    /// "NAME (2)" when none is given. Returns the copy's name.
    pub fn duplicate_preset(&self, name: &str, new_name: Option<&str>) -> Result<String> {
        let (mut p, _) = self.require_preset(name)?;
        let taken = |n: &str| self.preset(n).is_some();
        p.name = match new_name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) if taken(n) => return Err(Error::Config(format!("there is already a preset called {n:?}"))),
            Some(n) => n.to_owned(),
            None => preset::unique_name(&p.name, &taken),
        };
        let copy = p.name.clone();
        self.update_config(|c| c.presets.push(p))?;
        Ok(copy)
    }

    /// A preset as the text of an exported file, secrets excluded.
    pub fn export_preset_text(&self, name: &str) -> Result<String> {
        let (mut p, _) = self.require_preset(name)?;
        // Saved presets hold no secrets already; this catches one a user
        // typed into config.toml by hand.
        p.strip_secrets(&|m, k| self.option_is_secret(m, k));
        Ok(p.to_file())
    }

    /// Export a preset to a `.toml` file.
    pub fn export_preset(&self, name: &str, path: &Path) -> Result<()> {
        let text = self.export_preset_text(name)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Add a preset from an exported file's text. A name already taken
    /// gets a number; credentials in the file are dropped; services it
    /// names that are not installed are listed (it is stored anyway, so it
    /// works once they are).
    pub fn import_preset_text(&self, text: &str) -> Result<PresetReport> {
        let mut p = Preset::from_file(text)?;
        let dropped = p.strip_secrets(&|m, k| self.option_is_secret(m, k));
        p.name = preset::unique_name(&p.name, &|n| self.preset(n).is_some());
        let registry = self.registry();
        let missing = p.missing(&|id| registry.get(id).is_some());
        let name = p.name.clone();
        self.update_config(|c| c.presets.push(p))?;
        Ok(PresetReport { name, missing, dropped_secrets: dropped })
    }

    /// Import a preset from a file.
    pub fn import_preset(&self, path: &Path) -> Result<PresetReport> {
        let text =
            std::fs::read_to_string(path).map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        self.import_preset_text(&text)
    }
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
                "glossa_preset = 1\nname = \"Saved\"\n[options.someone]\ntoken = \"t-fake\"\nmodel = \"x\"\n",
            )
            .unwrap();
        assert_eq!(report.name, "Saved (2)");
        assert_eq!(report.dropped_secrets, vec!["someone.token".to_owned()]);
        assert_eq!(report.missing, vec!["someone".to_owned()]);
        let (p, _) = e.preset("Saved (2)").unwrap();
        assert!(!p.options["someone"].contains_key("token"));
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
