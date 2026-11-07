//! Presets: named sets of services the user switches between in one click.
//!
//! A preset records which services are switched on and in what order, the
//! active one, the dictionaries, the voice and the OCR engine, the modules'
//! option values and, if asked, the language pair. It never holds a secret:
//! API keys and tokens stay where they are (the settings file or the system
//! keychain) and a preset that enables a keyed service relies on the key
//! already set there.
//!
//! Presets live in `config.toml` as `[[presets]]`, beside the settings they
//! change. Built-in presets ([`builtin`]) are not written there; they use
//! only services that need no key.
//!
//! A field left empty means "leave this alone": a preset with no dictionary
//! list does not touch the dictionaries.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lang::LangPair;
use crate::secrets::IN_STORE;
use crate::Config;

/// The `glossa_preset` version an exported file carries.
pub const FILE_VERSION: u32 = 1;

/// Module options, keyed by module id, then option key.
pub type PresetOptions = BTreeMap<String, BTreeMap<String, Value>>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// Shown in lists and menus; unique among presets (ignoring case).
    pub name: String,
    /// Translation (and other non-dictionary) services in strip order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// The services switched on. Empty: the current ones stay.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enabled: Vec<String>,
    /// The selected service. Empty or unavailable: the first enabled one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dictionary_order: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dictionary_enabled: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dictionary_active: String,
    /// The voice for "listen".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tts: String,
    /// The OCR engine.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ocr: String,
    /// The language pair, when the preset sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair: Option<LangPair>,
    /// Module option values, secrets excluded. Merged over the current
    /// values: an option the preset does not name keeps its value.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub options: PresetOptions,
}

/// What applying a preset did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// Services and modules the preset names that are not installed (or
    /// cannot do the job), skipped.
    pub missing: Vec<String>,
}

/// Whether an option key looks like it holds a credential. Used for
/// modules whose manifest is not at hand (an imported preset naming a
/// module that is not installed), where the declared kind is unknown.
pub fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let exact = ["key", "auth", "cookie"].iter().any(|w| k == *w || k.ends_with(&format!("_{w}")));
    exact || ["apikey", "api_key", "token", "secret", "password", "passwd", "credential"].iter().any(|w| k.contains(w))
}

impl Preset {
    /// The current setup as a preset named `name`. `is_secret(module, key)`
    /// says which options are credentials; those (and anything held in the
    /// keychain) are left out.
    pub fn capture(name: &str, cfg: &Config, with_pair: bool, is_secret: &dyn Fn(&str, &str) -> bool) -> Preset {
        let s = &cfg.services;
        let mut p = Preset {
            name: name.trim().to_owned(),
            order: s.order.clone(),
            enabled: s.enabled.clone(),
            active: s.active.clone(),
            dictionary_order: s.dictionary_order.clone(),
            dictionary_enabled: s.dictionary_enabled.clone(),
            dictionary_active: s.dictionary_active.clone(),
            tts: s.tts.clone(),
            ocr: cfg.ocr.provider.clone(),
            pair: with_pair.then_some(cfg.general.pair),
            options: s.options.clone(),
        };
        p.strip_secrets(is_secret);
        p
    }

    /// Remove every credential from the options; returns what was removed,
    /// as `module.key`. A value that is the keychain marker counts as one
    /// whatever its key.
    pub fn strip_secrets(&mut self, is_secret: &dyn Fn(&str, &str) -> bool) -> Vec<String> {
        let mut dropped = Vec::new();
        for (module, values) in self.options.iter_mut() {
            values.retain(|key, value| {
                let secret = is_secret(module, key) || value.as_str() == Some(IN_STORE);
                if secret {
                    dropped.push(format!("{module}.{key}"));
                }
                !secret
            });
        }
        self.options.retain(|_, v| !v.is_empty());
        dropped
    }

    /// Every service or module id the preset names, sorted.
    pub fn referenced(&self) -> BTreeSet<&str> {
        let mut ids: BTreeSet<&str> = BTreeSet::new();
        for list in [&self.enabled, &self.dictionary_enabled] {
            ids.extend(list.iter().map(String::as_str));
        }
        for one in [&self.active, &self.dictionary_active, &self.tts, &self.ocr] {
            if !one.is_empty() {
                ids.insert(one);
            }
        }
        ids.extend(self.options.keys().map(String::as_str));
        ids
    }

    /// The ids the preset needs that `available` does not know.
    pub fn missing(&self, available: &dyn Fn(&str) -> bool) -> Vec<String> {
        self.referenced().into_iter().filter(|id| !available(id)).map(str::to_owned).collect()
    }

    /// Put the preset into `cfg`. Services `available` does not know are
    /// skipped and listed in the result; the rest of each list keeps the
    /// order it had.
    pub fn apply_to(&self, cfg: &mut Config, available: &dyn Fn(&str) -> bool) -> Applied {
        let missing = self.missing(available);
        let s = &mut cfg.services;
        apply_list(&self.order, &self.enabled, &self.active, &mut s.order, &mut s.enabled, &mut s.active, available);
        apply_list(
            &self.dictionary_order,
            &self.dictionary_enabled,
            &self.dictionary_active,
            &mut s.dictionary_order,
            &mut s.dictionary_enabled,
            &mut s.dictionary_active,
            available,
        );
        if !self.tts.is_empty() && available(&self.tts) {
            s.tts = self.tts.clone();
        }
        for (module, values) in &self.options {
            if !available(module) {
                continue;
            }
            let into = s.options.entry(module.clone()).or_default();
            for (k, v) in values {
                into.insert(k.clone(), v.clone());
            }
        }
        s.preset = self.name.clone();
        if !self.ocr.is_empty() && available(&self.ocr) {
            cfg.ocr.provider = self.ocr.clone();
        }
        if let Some(pair) = self.pair {
            cfg.general.pair = pair;
        }
        Applied { missing }
    }

    /// The preset as an exported file: a short header, then the fields.
    pub fn to_file(&self) -> String {
        let body = toml::to_string(self).unwrap_or_default();
        format!(
            "# A Glossa preset. Import it in Options > Services > Presets or with\n\
             # `glossa-cli preset import FILE`. It holds no API keys or tokens.\n\
             glossa_preset = {FILE_VERSION}\n{body}"
        )
    }

    /// Read an exported file. A file from a newer format version is refused
    /// rather than half-read.
    pub fn from_file(text: &str) -> crate::Result<Preset> {
        let value: toml::Table = toml::from_str(text).map_err(|e| crate::Error::Config(format!("preset file: {e}")))?;
        if let Some(v) = value.get("glossa_preset") {
            let version = v.as_integer().unwrap_or(i64::MAX);
            if version > FILE_VERSION as i64 {
                return Err(crate::Error::Config(format!(
                    "preset file format {version} is newer than this Glossa reads ({FILE_VERSION})"
                )));
            }
        }
        let mut p: Preset = toml::from_str(text).map_err(|e| crate::Error::Config(format!("preset file: {e}")))?;
        p.name = p.name.trim().to_owned();
        if p.name.is_empty() {
            return Err(crate::Error::Config("preset file: it has no name".into()));
        }
        Ok(p)
    }
}

/// Apply one service list: the preset's own order first (the services it
/// knows), then the rest in their old order; the enabled set replaced by
/// the available ones the preset names; the active one kept in it.
fn apply_list(
    p_order: &[String],
    p_enabled: &[String],
    p_active: &str,
    order: &mut Vec<String>,
    enabled: &mut Vec<String>,
    active: &mut String,
    available: &dyn Fn(&str) -> bool,
) {
    if !p_order.is_empty() {
        let mut out: Vec<String> = p_order.iter().filter(|id| available(id)).cloned().collect();
        for id in order.iter() {
            if !out.contains(id) {
                out.push(id.clone());
            }
        }
        *order = out;
    }
    if p_enabled.is_empty() {
        return;
    }
    let on: Vec<String> = p_enabled.iter().filter(|id| available(id)).cloned().collect();
    if on.is_empty() {
        // Nothing the preset wants is here: keep what works rather than
        // leave an empty strip.
        return;
    }
    // Enabled ones follow the strip order.
    let mut on_sorted = on.clone();
    on_sorted.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    *enabled = on_sorted;
    *active = if !p_active.is_empty() && enabled.iter().any(|e| e == p_active) {
        p_active.to_owned()
    } else if enabled.contains(active) {
        active.clone()
    } else {
        enabled[0].clone()
    };
}

/// The presets that come with Glossa. They use services that need no key,
/// and are not stored in the settings: duplicating one gives an editable
/// copy. Their names are English; front ends may show a translated label
/// through [`builtin_label_key`].
pub fn builtin() -> Vec<Preset> {
    let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    vec![
        Preset {
            name: "Everyday".into(),
            order: ids(&["google", "bing", "yandex-web"]),
            enabled: ids(&["google", "bing", "yandex-web"]),
            active: "google".into(),
            dictionary_order: ids(&["google-dictionary", "wiktionary"]),
            dictionary_enabled: ids(&["google-dictionary", "wiktionary"]),
            dictionary_active: "google-dictionary".into(),
            ..Preset::default()
        },
        Preset {
            name: "Private".into(),
            order: ids(&["lingva", "mymemory"]),
            enabled: ids(&["lingva", "mymemory"]),
            active: "lingva".into(),
            dictionary_order: ids(&["wiktionary", "wikipedia"]),
            dictionary_enabled: ids(&["wiktionary", "wikipedia"]),
            dictionary_active: "wiktionary".into(),
            tts: "system-tts".into(),
            ..Preset::default()
        },
        Preset {
            name: "Word study".into(),
            order: ids(&["google", "deepl-web"]),
            enabled: ids(&["google", "deepl-web"]),
            active: "google".into(),
            dictionary_order: ids(&["wiktionary", "oxford-learners", "google-dictionary", "multitran"]),
            dictionary_enabled: ids(&["wiktionary", "oxford-learners", "google-dictionary", "multitran"]),
            dictionary_active: "wiktionary".into(),
            ..Preset::default()
        },
    ]
}

/// The locale key of a built-in preset's label (`preset.builtin.everyday`),
/// or `None` for a name that is not a built-in's.
pub fn builtin_label_key(name: &str) -> Option<String> {
    builtin()
        .iter()
        .any(|p| p.name == name)
        .then(|| format!("preset.builtin.{}", name.to_ascii_lowercase().replace(' ', "_")))
}

/// Whether two preset names are the same one.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A name not taken by `taken`: `base`, else `base (2)`, `base (3)`…
pub fn unique_name(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = base.trim();
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|n| format!("{base} ({n})")).find(|n| !taken(n)).expect("unbounded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.services.order = vec!["google".into(), "deepl".into(), "bing".into()];
        c.services.enabled = vec!["google".into(), "deepl".into()];
        c.services.active = "deepl".into();
        c.services.set_option("deepl", "api_key", json!("dk-fake"));
        c.services.set_option("deepl", "formality", json!("more"));
        c.services.set_option("ai-x", "key", json!(IN_STORE));
        c.services.set_option("ai-x", "model", json!("m1"));
        c
    }

    fn secret(_: &str, key: &str) -> bool {
        key == "api_key"
    }

    #[test]
    fn capture_leaves_secrets_out() {
        let p = Preset::capture("Work", &cfg(), false, &secret);
        assert_eq!(p.options["deepl"].get("formality"), Some(&json!("more")));
        assert!(!p.options["deepl"].contains_key("api_key"));
        // The keychain marker is never carried, whatever the key is called.
        assert!(!p.options["ai-x"].contains_key("key"));
        assert!(p.pair.is_none());
        assert!(!p.to_file().contains("dk-fake"));
    }

    #[test]
    fn save_and_apply_round_trip() {
        let original = cfg();
        let p = Preset::capture("Work", &original, true, &secret);
        let mut changed = original.clone();
        changed.services.enabled = vec!["bing".into()];
        changed.services.active = "bing".into();
        changed.services.set_option("deepl", "formality", json!("less"));
        let applied = p.apply_to(&mut changed, &|_| true);
        assert!(applied.missing.is_empty());
        assert_eq!(changed.services.enabled, original.services.enabled);
        assert_eq!(changed.services.active, "deepl");
        assert_eq!(changed.services.option("deepl", "formality"), Some(&json!("more")));
        // The key stays the one already set.
        assert_eq!(changed.services.option("deepl", "api_key"), Some(&json!("dk-fake")));
        assert_eq!(changed.services.preset, "Work");
    }

    #[test]
    fn missing_services_are_skipped_and_listed() {
        let p = Preset {
            name: "X".into(),
            order: vec!["gone".into(), "bing".into()],
            enabled: vec!["gone".into(), "bing".into()],
            active: "gone".into(),
            ..Preset::default()
        };
        let mut c = cfg();
        let applied = p.apply_to(&mut c, &|id| id != "gone");
        assert_eq!(applied.missing, vec!["gone".to_owned()]);
        assert_eq!(c.services.enabled, vec!["bing".to_owned()]);
        assert_eq!(c.services.active, "bing");
        assert!(!c.services.order.contains(&"gone".to_owned()));
        assert_eq!(c.services.order[0], "bing");
    }

    #[test]
    fn a_preset_with_nothing_available_leaves_the_strip_alone() {
        let p = Preset { name: "X".into(), enabled: vec!["gone".into()], ..Preset::default() };
        let mut c = cfg();
        p.apply_to(&mut c, &|id| id != "gone");
        assert_eq!(c.services.enabled, cfg().services.enabled);
    }

    #[test]
    fn files_round_trip_and_newer_ones_are_refused() {
        let p = Preset::capture("Japanese reading", &cfg(), true, &secret);
        let back = Preset::from_file(&p.to_file()).unwrap();
        assert_eq!(back, p);
        assert!(Preset::from_file("glossa_preset = 99\nname = \"x\"").is_err());
        assert!(Preset::from_file("glossa_preset = 1").is_err(), "a preset needs a name");
    }

    #[test]
    fn secret_looking_keys() {
        for k in ["api_key", "apiKey", "token", "auth_token", "password", "client_secret", "key"] {
            assert!(looks_secret(k), "{k}");
        }
        for k in ["model", "formality", "url", "keyboard_layout", "temperature"] {
            assert!(!looks_secret(k), "{k}");
        }
    }

    #[test]
    fn old_configs_without_presets_load() {
        let c = Config::from_toml("[services]\nactive = \"google\"\n").unwrap();
        assert!(c.presets.is_empty());
        assert!(c.services.preset.is_empty());
        let mut with = Config::default();
        with.presets.push(Preset::capture("A", &cfg(), false, &secret));
        let back = Config::from_toml(&with.to_toml()).unwrap();
        assert_eq!(back.presets, with.presets);
    }

    #[test]
    fn builtin_presets_need_no_keys_and_have_labels() {
        for p in builtin() {
            assert!(p.options.is_empty());
            assert!(builtin_label_key(&p.name).is_some());
        }
        assert_eq!(unique_name("A", &|n| n == "A" || n == "A (2)"), "A (3)");
    }
}
