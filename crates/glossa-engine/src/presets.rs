//! Presets for every front end: list, save, apply, rename, delete,
//! duplicate, export and import (see [`glossa_core::preset`] for what a
//! preset holds).
//!
//! The engine decides what counts as a secret here, because only it knows
//! the modules' declared option kinds; a preset never leaves with one. An
//! imported preset is held to more: it keeps only options the installed
//! module declares, and never an address (a URL, endpoint or host), since
//! applying it would send the keys already stored for that service there.
//!
//! Every change emits [`Event::PresetsChanged`], so a window holding an
//! unsaved copy of the settings (Options) can take it in.

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
    /// Options an import left out, as `module.key`: addresses (URLs,
    /// endpoints, hosts), and options no installed module declares.
    #[serde(default)]
    pub dropped_options: Vec<String>,
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
        self.presets_changed(None);
        Ok(PresetReport { name, dropped_secrets: dropped, ..PresetReport::default() })
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
        let name = p.name.clone();
        self.presets_changed(Some(p));
        Ok(PresetReport { name, missing: applied.missing, ..PresetReport::default() })
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
        Ok(p.to_file())
    }

    /// Export a preset to a `.toml` file.
    pub fn export_preset(&self, name: &str, path: &Path) -> Result<()> {
        let text = self.export_preset_text(name)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Add a preset from an exported file's text. A name already taken
    /// gets a number. Of the module options, only those the installed
    /// module declares are kept, and of those no credential and no address
    /// (both listed in the report). Services it names that are not
    /// installed are listed; it is stored anyway, so it works once they are.
    pub fn import_preset_text(&self, text: &str) -> Result<PresetReport> {
        if text.len() as u64 > MAX_PRESET_FILE {
            return Err(Error::Config(format!("a preset file is at most {} KB", MAX_PRESET_FILE / 1024)));
        }
        let mut p = Preset::from_file(text)?;
        let name = check_name(&p.name)?;
        let registry = self.registry();
        let mut dropped_secrets = Vec::new();
        let mut dropped_options = Vec::new();
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
                if spec.kind == OptionKind::Url || preset::looks_like_address(key, value) {
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
        if !dropped_options.is_empty() {
            tracing::info!(preset = %name, dropped = ?dropped_options, "import left options out");
        }
        self.update_config(|c| c.presets.push(p))?;
        self.presets_changed(None);
        Ok(PresetReport { name, missing, dropped_secrets, dropped_options })
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
