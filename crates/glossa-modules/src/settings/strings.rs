//! A module's translations of its own text: `locales/<lang>.toml` in the
//! module folder.
//!
//! ```toml
//! # locales/ru.toml
//! [module]
//! name = "Акме Переводчик"
//!
//! [options.api_key]
//! label = "Ключ API"
//! pattern_message = "Ключ Acme: ak- и 32 буквы или цифры."
//!
//! [options.tone.choices]
//! casual = "Разговорный"
//!
//! [settings.items.advanced]
//! label = "Дополнительно"
//! ```
//!
//! Tables flatten to dotted keys (`options.api_key.label`); dotted keys
//! written directly work the same. The manifest's text is the English
//! default, and a missing key falls back to it. Which language is used is
//! Glossa's interface language, so a module follows a switch of it at once.

use std::collections::BTreeMap;
use std::path::Path;

/// The largest locale file read, in bytes. Translations of a settings page
/// are a few kilobytes; every module's are read when modules load.
const MAX_FILE: u64 = 256 * 1024;

/// Every locale file of one module, flattened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleStrings {
    /// Lowercased language code (the file stem) -> dotted key -> text.
    by_lang: BTreeMap<String, BTreeMap<String, String>>,
}

impl ModuleStrings {
    /// Read `<dir>/locales/*.toml`. A file that does not parse is skipped
    /// and named in the second value; a module without the folder has no
    /// translations and no problems.
    pub fn load(dir: &Path) -> (ModuleStrings, Vec<String>) {
        let mut out = ModuleStrings::default();
        let mut problems = Vec::new();
        let folder = dir.join("locales");
        let Ok(read) = std::fs::read_dir(&folder) else { return (out, problems) };
        // Links may point anywhere; a module's translations stay inside its
        // own folder.
        let Some(root) = folder.canonicalize().ok().filter(|f| dir.canonicalize().is_ok_and(|d| f.starts_with(d)))
        else {
            problems.push("locales/ points outside the module's folder; it is not read".into());
            return (out, problems);
        };
        let mut files: Vec<_> = read.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("toml")) {
                continue;
            }
            let Some(lang) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let shown = format!("locales/{}", path.file_name().unwrap_or_default().to_string_lossy());
            if !path.canonicalize().is_ok_and(|p| p.starts_with(&root)) {
                problems.push(format!("{shown}: a link to outside the module's folder; not read"));
                continue;
            }
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
                problems.push(format!("{shown}: larger than {} KB; not read", MAX_FILE / 1024));
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse(&text) {
                    Ok((map, bad)) => {
                        problems.extend(bad.into_iter().map(|k| format!("{shown}: `{k}` is not a string")));
                        out.insert(lang, map);
                    }
                    Err(e) => problems.push(format!("{shown}: {}", e.trim())),
                },
                Err(e) => problems.push(format!("{shown}: {e}")),
            }
        }
        (out, problems)
    }

    /// Add (or replace) one language's strings.
    pub fn insert(&mut self, lang: &str, strings: BTreeMap<String, String>) {
        self.by_lang.insert(normalise(lang), strings);
    }

    pub fn is_empty(&self) -> bool {
        self.by_lang.is_empty()
    }

    /// The languages that have a file.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.by_lang.keys().map(String::as_str)
    }

    /// One language's strings, for the lint and the editor.
    pub fn table(&self, lang: &str) -> Option<&BTreeMap<String, String>> {
        self.by_lang.get(&normalise(lang))
    }

    /// The text for `key` in `lang`: the exact language (`pt-br`), then its
    /// base (`pt`). `None` means "use the manifest's text".
    pub fn get(&self, lang: &str, key: &str) -> Option<&str> {
        let lang = normalise(lang);
        let found = self.by_lang.get(&lang).and_then(|m| m.get(key));
        let found = found.or_else(|| {
            let base = lang.split('-').next().filter(|b| *b != lang)?;
            self.by_lang.get(base)?.get(key)
        });
        found.map(String::as_str).filter(|s| !s.is_empty())
    }
}

/// `pt_BR` and `pt-BR` name the same file.
fn normalise(lang: &str) -> String {
    lang.trim().replace('_', "-").to_ascii_lowercase()
}

/// Parse one locale file into dotted keys. The second value lists keys whose
/// value is not a string (ignored).
pub fn parse(text: &str) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    let mut bad = Vec::new();
    flatten("", &toml::Value::Table(table), &mut out, &mut bad);
    Ok((out, bad))
}

fn flatten(prefix: &str, v: &toml::Value, out: &mut BTreeMap<String, String>, bad: &mut Vec<String>) {
    match v {
        toml::Value::Table(t) => {
            for (k, v) in t {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out, bad);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_owned(), s.clone());
        }
        _ => bad.push(prefix.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_dotted_keys_flatten_alike() {
        let (map, bad) = parse(
            "[options.api_key]\nlabel = 'Ключ'\n[options.tone.choices]\ncasual = 'Разговорный'\n\"module.name\" = 'x'\n",
        )
        .unwrap();
        assert_eq!(map["options.api_key.label"], "Ключ");
        assert_eq!(map["options.tone.choices.casual"], "Разговорный");
        assert!(bad.is_empty());
        let (map, bad) = parse("options.api_key.label = 'K'\noptions.n = 3\n").unwrap();
        assert_eq!(map["options.api_key.label"], "K");
        assert_eq!(bad, ["options.n"]);
        assert!(parse("[[x").is_err());
    }

    #[test]
    fn falls_back_from_region_to_base_to_none() {
        let mut s = ModuleStrings::default();
        s.insert("pt", BTreeMap::from([("a".into(), "base".into()), ("b".into(), "base b".into())]));
        s.insert("pt_BR", BTreeMap::from([("a".into(), "brazil".into()), ("e".into(), String::new())]));
        assert_eq!(s.get("pt-BR", "a"), Some("brazil"));
        assert_eq!(s.get("pt-br", "b"), Some("base b"));
        assert_eq!(s.get("pt", "a"), Some("base"));
        assert_eq!(s.get("de", "a"), None);
        assert_eq!(s.get("pt-BR", "e"), None, "an empty translation counts as missing");
    }

    #[test]
    fn loads_a_folder_and_reports_broken_files() {
        let dir = std::env::temp_dir().join(format!("glossa-strings-{}", std::process::id()));
        let locales = dir.join("locales");
        std::fs::create_dir_all(&locales).unwrap();
        std::fs::write(locales.join("ru.toml"), "[options.k]\nlabel = 'Ключ'\n").unwrap();
        std::fs::write(locales.join("de.toml"), "not = [toml").unwrap();
        std::fs::write(locales.join("notes.txt"), "ignored").unwrap();
        let (s, problems) = ModuleStrings::load(&dir);
        assert_eq!(s.get("ru", "options.k.label"), Some("Ключ"));
        assert_eq!(s.languages().collect::<Vec<_>>(), ["ru"]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("locales/de.toml"));
        let (none, problems) = ModuleStrings::load(&dir.join("absent"));
        assert!(none.is_empty() && problems.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stays_inside_the_module_and_skips_huge_files() {
        let base = std::env::temp_dir().join(format!("glossa-strings-guard-{}", std::process::id()));
        let module = base.join("module");
        let locales = module.join("locales");
        std::fs::create_dir_all(&locales).unwrap();
        std::fs::write(locales.join("big.toml"), "x = \"".to_owned() + &"y".repeat(300 * 1024) + "\"\n").unwrap();
        #[cfg(unix)]
        {
            std::fs::write(base.join("outside.toml"), "[options.k]\nlabel = 'leak'\n").unwrap();
            std::os::unix::fs::symlink(base.join("outside.toml"), locales.join("de.toml")).unwrap();
        }
        let (s, problems) = ModuleStrings::load(&module);
        assert!(s.is_empty(), "{s:?}");
        assert!(problems.iter().any(|p| p.contains("big.toml") && p.contains("larger than")), "{problems:?}");
        #[cfg(unix)]
        assert!(problems.iter().any(|p| p.contains("de.toml") && p.contains("outside")), "{problems:?}");
        std::fs::remove_dir_all(&base).ok();
    }
}
