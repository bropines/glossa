//! Finding themes and interface languages on disk.

use std::path::{Path, PathBuf};

use glossa_core::locale::{Locale, BUILTIN_LOCALES};
use glossa_core::theme::{builtin, Theme};

/// A theme the picker can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    pub name: String,
    /// `None` for a built-in theme.
    pub path: Option<PathBuf>,
    /// A theme from a classic installation.
    pub classic: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ThemeCatalog {
    entries: Vec<ThemeEntry>,
}

impl ThemeCatalog {
    /// Built-ins first, then every `.json`/`.toml` file in `dirs` (later
    /// directories override earlier ones of the same name).
    pub fn discover(dirs: &[PathBuf], classic_dirs: &[PathBuf]) -> ThemeCatalog {
        let mut entries: Vec<ThemeEntry> =
            builtin::NAMES.iter().map(|n| ThemeEntry { name: (*n).to_owned(), path: None, classic: false }).collect();
        let mut add = |dir: &Path, classic: bool| {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("toml"))
                })
                .collect();
            files.sort();
            for path in files {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let entry = ThemeEntry { name: name.clone(), path: Some(path), classic };
                match entries.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&name)) {
                    Some(existing) => *existing = entry,
                    None => entries.push(entry),
                }
            }
        };
        for d in classic_dirs {
            add(d, true);
        }
        for d in dirs {
            add(d, false);
        }
        ThemeCatalog { entries }
    }

    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    fn file_text(&self, name: &str) -> Option<String> {
        let entry = self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))?;
        std::fs::read_to_string(entry.path.as_ref()?).ok()
    }

    /// Load a theme by name. An unknown or broken theme falls back to the
    /// default, with the reason logged — a bad theme file must never stop the
    /// program from starting.
    pub fn load(&self, name: &str) -> Theme {
        match self.try_load(name) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("theme {name:?}: {e}; using the default");
                Theme::default()
            }
        }
    }

    pub fn try_load(&self, name: &str) -> Result<Theme, String> {
        let Some(entry) = self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)) else {
            return Err("no such theme".into());
        };
        match &entry.path {
            None => builtin::by_name(&entry.name).ok_or_else(|| "no such built-in theme".into()),
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let resolver = |parent: &str| self.file_text(parent);
                let mut theme = Theme::load(&text, &path.to_string_lossy(), &resolver).map_err(|e| e.to_string())?;
                if theme.name.is_empty() {
                    theme.name = entry.name.clone();
                }
                Ok(theme)
            }
        }
    }
}

/// An interface language the picker can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleEntry {
    pub code: String,
    pub name: String,
    pub native_name: String,
    /// The folder name, for classic `Locales/<Name>/lang.json` entries.
    pub folder: String,
    /// A pack for this language is compiled in.
    pub builtin: bool,
    /// Files laid over the built-in strings, lowest priority first: classic
    /// `lang.json` files, then native locale files.
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct LocaleCatalog {
    entries: Vec<LocaleEntry>,
}

impl LocaleCatalog {
    /// English and the compiled-in packs, then native `*.json` locale files
    /// and classic `<Folder>/lang.json` folders. Files for a language that is
    /// already known join it as layers instead of replacing it.
    pub fn discover(dirs: &[PathBuf]) -> LocaleCatalog {
        let mut entries = vec![LocaleEntry {
            code: "en".into(),
            name: "English".into(),
            native_name: "English".into(),
            folder: "English".into(),
            builtin: true,
            files: Vec::new(),
        }];
        entries.extend(BUILTIN_LOCALES.iter().map(|b| LocaleEntry {
            code: b.code.into(),
            name: b.name.into(),
            native_name: b.native_name.into(),
            folder: String::new(),
            builtin: true,
            files: Vec::new(),
        }));
        // Which files are classic, to order the layers once all are found.
        let mut classic_files: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            items.sort();
            for item in items {
                let (file, folder) = if item.is_dir() {
                    (
                        item.join("lang.json"),
                        item.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    )
                } else if item.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
                    (item.clone(), String::new())
                } else {
                    continue;
                };
                let Ok(text) = std::fs::read_to_string(&file) else { continue };
                let Ok(locale) = Locale::parse(&text) else {
                    tracing::debug!("not a locale file: {}", file.display());
                    continue;
                };
                if locale.code.is_empty() {
                    continue;
                }
                // English is built in: a classic English file only repeats it
                // (with the old program's name in it). A native one may
                // still reword single strings.
                if locale.classic && locale.code.eq_ignore_ascii_case("en") {
                    continue;
                }
                if locale.classic {
                    classic_files.push(file.clone());
                }
                match entries.iter_mut().find(|e| e.code.eq_ignore_ascii_case(&locale.code)) {
                    Some(existing) => {
                        if !locale.name.is_empty() && existing.name.is_empty() {
                            existing.name = locale.name.clone();
                        }
                        if !locale.native_name.is_empty() && existing.native_name.is_empty() {
                            existing.native_name = locale.native_name.clone();
                        }
                        if !folder.is_empty() {
                            existing.folder = folder;
                        }
                        existing.files.push(file);
                    }
                    None => entries.push(LocaleEntry {
                        code: locale.code.clone(),
                        name: if locale.name.is_empty() { folder.clone() } else { locale.name.clone() },
                        native_name: locale.native_name.clone(),
                        folder,
                        builtin: false,
                        files: vec![file],
                    }),
                }
            }
        }
        for e in &mut entries {
            // Stable: discovery order holds within each kind.
            e.files.sort_by_key(|f| !classic_files.contains(f));
        }
        LocaleCatalog { entries }
    }

    pub fn entries(&self) -> &[LocaleEntry] {
        &self.entries
    }

    /// Find a locale by code (`ru`), name (`Russian`), classic folder name or
    /// native name. Empty means "the system's language".
    pub fn find(&self, wanted: &str) -> Option<&LocaleEntry> {
        let wanted = wanted.trim();
        let wanted = if wanted.is_empty() { system_language() } else { wanted.to_owned() };
        let base = wanted.split(['-', '_']).next().unwrap_or(&wanted).to_owned();
        self.entries
            .iter()
            .find(|e| {
                e.code.eq_ignore_ascii_case(&wanted)
                    || e.name.eq_ignore_ascii_case(&wanted)
                    || e.folder.eq_ignore_ascii_case(&wanted)
                    || (!e.native_name.is_empty() && e.native_name.eq_ignore_ascii_case(&wanted))
            })
            .or_else(|| self.entries.iter().find(|e| e.code.eq_ignore_ascii_case(&base)))
    }

    /// Load a language as layers: English, the built-in pack, classic files,
    /// the user's native files. A layer that fails to load is skipped, with
    /// the reason logged, and the rest still apply.
    pub fn load(&self, wanted: &str) -> Locale {
        let Some(entry) = self.find(wanted) else { return Locale::english() };
        let mut out = Locale::english();
        if entry.builtin {
            if let Some(pack) = Locale::builtin(&entry.code) {
                out.merge(pack);
            }
        }
        for path in &entry.files {
            match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| Locale::parse(&t)) {
                Ok(l) => out.merge(l),
                Err(e) => tracing::warn!("locale {}: {e}; skipped", path.display()),
            }
        }
        out.code = entry.code.clone();
        out.name = entry.name.clone();
        out.native_name = entry.native_name.clone();
        tracing::debug!(wanted, code = %out.code, builtin = entry.builtin, files = ?entry.files, "interface language");
        out
    }
}

/// The user's language, as a BCP 47-ish code (`ru-RU`).
///
/// `GLOSSA_LANG` overrides everything. On Windows the display language comes
/// next: `LANG` and friends there are usually left behind by Unix-like
/// toolkits rather than chosen by the user.
pub fn system_language() -> String {
    let from_env = |var: &str| {
        let v = std::env::var(var).ok()?;
        let v = v.split(['.', '@']).next().unwrap_or("").trim().to_owned();
        (!v.is_empty() && v != "C" && v != "POSIX").then(|| v.replace('_', "-"))
    };
    from_env("GLOSSA_LANG")
        .or_else(windows_language)
        .or_else(|| ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(from_env))
        .or_else(macos_language)
        .unwrap_or_else(|| "en".into())
}

#[cfg(windows)]
fn windows_language() -> Option<String> {
    // Read the registry directly: running `reg.exe` from a GUI process opens
    // a console window, which steals the focus from the program whose
    // selection we are about to read.
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let first = |path: &str, value: &str| {
        let list = hkcu.open_subkey(path).ok()?.get_value::<Vec<String>, _>(value).ok()?;
        list.into_iter().find(|s| !s.trim().is_empty())
    };
    // The display language the user picked, then the one Windows was
    // installed with, then the regional format.
    first(r"Control Panel\Desktop", "PreferredUILanguages")
        .or_else(|| first(r"Control Panel\Desktop\MuiCached", "MachinePreferredUILanguages"))
        .or_else(|| {
            let key = hkcu.open_subkey(r"Control Panel\International").ok()?;
            key.get_value::<String, _>("LocaleName").ok().filter(|s| !s.is_empty())
        })
}

#[cfg(not(windows))]
fn windows_language() -> Option<String> {
    None
}

/// macOS: the first of the user's preferred languages. A program started
/// from Finder or at login gets no `LANG`, so the environment says nothing
/// there; `defaults` reads the same list System Settings shows.
#[cfg(target_os = "macos")]
fn macos_language() -> Option<String> {
    let out = std::process::Command::new("/usr/bin/defaults").args(["read", "-g", "AppleLanguages"]).output().ok()?;
    first_plist_string(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "macos"))]
fn macos_language() -> Option<String> {
    None
}

/// The first entry of the old-style plist array `defaults` prints, one
/// quoted or bare entry per line between parentheses.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn first_plist_string(text: &str) -> Option<String> {
    text.lines()
        .map(|l| l.trim().trim_end_matches(',').trim_matches('"').trim())
        .find(|l| !l.is_empty() && *l != "(" && *l != ")")
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_preferred_language_from_defaults() {
        assert_eq!(
            first_plist_string(
                "(
    \"ru-RU\",
    \"en-US\"
)
"
            )
            .as_deref(),
            Some("ru-RU")
        );
        assert_eq!(
            first_plist_string(
                "(
    en
)
"
            )
            .as_deref(),
            Some("en")
        );
        assert_eq!(first_plist_string("").as_deref(), None);
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("glossa-cat-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&p).ok();
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn themes_from_disk_join_the_builtins_and_user_files_win() {
        let classic = temp("classic-themes");
        let user = temp("user-themes");
        std::fs::write(
            classic.join("Flat Dark.json"),
            r#"{ "Common": {}, "Window": { "Normal": { "Back": "20", "Text": "b" } }, "Button": {} }"#,
        )
        .unwrap();
        std::fs::write(user.join("Mine.toml"), "extends = \"Flat Dark\"\naccent = \"#ff0000\"\n").unwrap();
        let cat = ThemeCatalog::discover(std::slice::from_ref(&user), std::slice::from_ref(&classic));
        let names = cat.names();
        assert!(names.contains(&"Glossa Dark".to_string()));
        assert!(names.contains(&"Flat Dark".to_string()));
        assert!(names.contains(&"Mine".to_string()));
        let mine = cat.try_load("mine").unwrap();
        assert_eq!(mine.accent.to_string(), "#ff0000");
        assert_eq!(mine.window.back.to_string(), "#202020", "inherited from the classic file");
        // Broken or unknown themes fall back rather than failing.
        assert_eq!(cat.load("nope").name, Theme::default().name);
        std::fs::remove_dir_all(&classic).ok();
        std::fs::remove_dir_all(&user).ok();
    }

    #[test]
    fn classic_and_native_locales_are_found_by_code_name_or_folder() {
        let dir = temp("locales");
        std::fs::create_dir_all(dir.join("Russian")).unwrap();
        std::fs::write(
            dir.join("Russian").join("lang.json"),
            r#"{ "LanguageName": "Russian", "LanguageNativeName": "Русский", "LanguageCode": "ru", "Strings": [[172, "Перевести! (Ctrl+Enter)"]] }"#,
        )
        .unwrap();
        std::fs::write(dir.join("de.json"), r#"{ "code": "de", "name": "German", "native_name": "Deutsch", "strings": { "main.translate": "Übersetzen" } }"#).unwrap();
        let cat = LocaleCatalog::discover(std::slice::from_ref(&dir));
        assert_eq!(cat.entries().len(), 3, "English, Russian (built in, the classic file joins it), German");
        assert_eq!(cat.find("ru").unwrap().native_name, "Русский");
        assert_eq!(cat.find("Russian").unwrap().code, "ru", "the classic options store the folder name");
        assert_eq!(cat.find("ru-RU").unwrap().code, "ru", "regional variants fall back to the base");
        assert_eq!(cat.load("de").get("main.translate"), "Übersetzen");
        assert_eq!(cat.load("ru").get("msg.translate_tip"), "Перевести! (Ctrl+Enter)");
        assert_eq!(cat.load("xx").code, "en");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn built_in_languages_are_offered_without_any_files() {
        let cat = LocaleCatalog::discover(&[]);
        let ru = cat.find("ru-RU").expect("Russian is compiled in");
        assert!(ru.builtin && ru.files.is_empty());
        assert_eq!(ru.native_name, "Русский");
        assert_eq!(cat.find("Russian").unwrap().code, "ru");
        let l = cat.load("ru-RU");
        assert_eq!(l.code, "ru");
        assert_eq!(l.get("options.server"), "API управления");
        assert_eq!(l.get("options.services.fail_over"), "Если сервис не ответил, пробовать следующий");
        assert_eq!(cat.load("en").get("options.server"), "Control API");
    }

    /// English, then the built-in pack, then a classic file, then the user's
    /// own file, whatever order the folders come in.
    #[test]
    fn layers_apply_in_order() {
        let classic = temp("layers-classic");
        let user = temp("layers-user");
        std::fs::create_dir_all(classic.join("Russian")).unwrap();
        std::fs::write(
            classic.join("Russian").join("lang.json"),
            r#"{ "LanguageName": "Russian", "LanguageCode": "ru",
                 "Windows": [ { "Id": 1, "Controls": [ [1004, "Перевести!"] ] },
                              { "Id": 16, "Caption": "Хоткеи", "Controls": [] } ] }"#,
        )
        .unwrap();
        std::fs::write(
            user.join("ru.json"),
            r#"{ "code": "ru", "strings": { "options.hotkeys": "Сочетания клавиш", "options.ocr": "OCR" } }"#,
        )
        .unwrap();
        // A classic English file is ignored; a native one rewords strings.
        std::fs::create_dir_all(classic.join("English")).unwrap();
        std::fs::write(
            classic.join("English").join("lang.json"),
            r#"{ "LanguageName": "English", "LanguageCode": "en", "Windows": [ { "Id": 1, "Caption": "QTranslate" } ] }"#,
        )
        .unwrap();
        std::fs::write(user.join("en.json"), r#"{ "code": "en", "strings": { "main.translate": "Go" } }"#).unwrap();

        for dirs in [[classic.clone(), user.clone()], [user.clone(), classic.clone()]] {
            let cat = LocaleCatalog::discover(&dirs);
            let ru = cat.load("ru");
            assert_eq!(ru.get("main.translate"), "Перевести!", "the classic file over the pack");
            assert_eq!(ru.get("options.hotkeys"), "Сочетания клавиш", "the user's file over the classic one");
            assert_eq!(ru.get("options.ocr"), "OCR");
            assert_eq!(ru.get("options.modules"), "Модули", "the pack fills what the files lack");
            assert_eq!(cat.find("Russian").unwrap().folder, "Russian");
            let en = cat.load("en");
            assert_eq!(en.get("main.translate"), "Go");
            assert_eq!(en.get("main.title"), "Glossa");
        }
        std::fs::remove_dir_all(&classic).ok();
        std::fs::remove_dir_all(&user).ok();
    }
}
