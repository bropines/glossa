//! Interface translations.
//!
//! Strings are looked up by a dotted key (`main.translate`,
//! `options.hotkeys.enable`). English is compiled in; a locale file supplies
//! the rest and anything it lacks falls back to English, so a half-finished
//! translation is still usable.
//!
//! Two file formats load:
//!
//! * **Native** — `{ "code": "ru", "name": "Russian", "native_name": "Русский",
//!   "strings": { "main.translate": "Перевести", ... },
//!   "languages": { "de": "Немецкий", ... } }`. `languages` names the
//!   languages of the language table in this language, by BCP 47 tag; a
//!   language it lacks keeps its English name.
//! * **Classic** — the numeric-id `lang.json` files of the original program,
//!   37 of them. Every string here that has a counterpart in the classic UI
//!   carries the classic address in [`STRINGS`], so those files translate this
//!   interface without being converted.
//!
//! Some languages also ship compiled in ([`BUILTIN_LOCALES`], native files in
//! `crates/glossa-core/locales/`) and cover every key. Loading a language
//! layers English, then the built-in pack, then a classic file, then the
//! user's own file (see `glossa_engine::LocaleCatalog::load`), so a classic
//! file written for the old program no longer leaves Glossa's newer strings
//! in English.
//!
//! Each file is checked as it is laid on ([`Locale::validate`]): unknown keys
//! are dropped with a warning, and a string whose `%s` count differs from
//! English is dropped as an error, so the layer below shows instead.
//! [`Locale::template`] writes the starting point for a new translation.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;

use crate::jsonc;

/// A loaded interface language.
#[derive(Debug, Clone, Default)]
pub struct Locale {
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub author: String,
    /// Read from a classic numeric-id `lang.json`.
    pub classic: bool,
    strings: HashMap<String, String>,
    /// Language names in this language, by BCP 47 tag.
    languages: HashMap<String, String>,
}

/// A language compiled into the program.
pub struct BuiltinLocale {
    pub code: &'static str,
    pub name: &'static str,
    pub native_name: &'static str,
    /// The pack in the native format.
    pub source: &'static str,
}

/// The languages that ship inside the executable. English needs no pack: it
/// is the second column of [`STRINGS`]. The packs live with this crate, not
/// under `assets/locales`, which a development build searches for user locale
/// files: there a pack would be laid over a classic file as if the user wrote it.
pub static BUILTIN_LOCALES: &[BuiltinLocale] = &[BuiltinLocale {
    code: "ru",
    name: "Russian",
    native_name: "Русский",
    source: include_str!("../locales/ru.json"),
}];

impl Locale {
    /// English: every key, nothing loaded.
    pub fn english() -> Self {
        Locale {
            code: "en".into(),
            name: "English".into(),
            native_name: "English".into(),
            author: String::new(),
            classic: false,
            strings: HashMap::new(),
            languages: HashMap::new(),
        }
    }

    /// The compiled-in pack for `code` (`ru`, `ru-RU`), if there is one.
    pub fn builtin(code: &str) -> Option<Locale> {
        let base = code.split(['-', '_']).next().unwrap_or(code);
        let pack = BUILTIN_LOCALES.iter().find(|b| b.code.eq_ignore_ascii_case(base))?;
        match Locale::parse(pack.source) {
            Ok(l) => Some(l),
            Err(e) => {
                // Only a broken build gets here; the tests parse every pack.
                tracing::error!("built-in locale {}: {e}", pack.code);
                None
            }
        }
    }

    /// Lay `over` on top of this locale: its strings win, and its name and
    /// author replace these where it has them.
    pub fn merge(&mut self, over: Locale) {
        for (field, value) in
            [(&mut self.name, over.name), (&mut self.native_name, over.native_name), (&mut self.author, over.author)]
        {
            if !value.is_empty() {
                *field = value;
            }
        }
        self.strings.extend(over.strings);
        self.languages.extend(over.languages);
    }

    /// The name of `lang` in this language, or its English name where this
    /// locale has none.
    pub fn language_name(&self, lang: crate::LangId) -> &str {
        self.languages.get(lang.bcp47()).map(String::as_str).unwrap_or_else(|| lang.english_name())
    }

    /// Whether this locale names `lang` itself.
    pub fn names_language(&self, lang: crate::LangId) -> bool {
        self.languages.contains_key(lang.bcp47())
    }

    /// Whether this locale itself translates `key` (rather than falling back
    /// to English).
    pub fn has(&self, key: &str) -> bool {
        self.strings.contains_key(key)
    }

    /// The text for `key`, in this language if it has it, else in English,
    /// else the key itself (so a missing string is visible, not blank).
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(s) = self.strings.get(key) {
            return s;
        }
        english(key).unwrap_or_else(|| {
            note_missing(key);
            key
        })
    }

    /// `get`, with `%s` replaced by `arg` — the classic placeholder.
    pub fn fmt(&self, key: &str, arg: &str) -> String {
        self.get(key).replacen("%s", arg, 1)
    }

    /// `get`, with each `%s` replaced by the next of `args`, in order. An
    /// argument that itself contains `%s` is left alone.
    pub fn fmt_args(&self, key: &str, args: &[&str]) -> String {
        let mut parts = self.get(key).split("%s");
        let mut out = parts.next().unwrap_or_default().to_owned();
        let mut args = args.iter();
        for part in parts {
            out.push_str(args.next().copied().unwrap_or("%s"));
            out.push_str(part);
        }
        out
    }

    /// How many of the known keys this locale actually translates.
    pub fn coverage(&self) -> (usize, usize) {
        let have = STRINGS.iter().filter(|(k, _, _)| self.strings.contains_key(*k)).count();
        (have, STRINGS.len())
    }

    /// Read a locale file of either format.
    pub fn parse(text: &str) -> Result<Locale, String> {
        let value: Value = serde_json::from_str(&jsonc::strip(text)).map_err(|e| e.to_string())?;
        if value.get("Strings").is_some() || value.get("Windows").is_some() {
            Ok(Locale::from_classic(&value))
        } else {
            #[derive(Deserialize)]
            struct Native {
                code: String,
                #[serde(default)]
                name: String,
                #[serde(default)]
                native_name: String,
                #[serde(default)]
                author: String,
                #[serde(default)]
                strings: HashMap<String, String>,
                #[serde(default)]
                languages: HashMap<String, String>,
            }
            let mut n: Native = serde_json::from_value(value).map_err(|e| e.to_string())?;
            // An empty string is a gap in a template, not a blank label.
            n.strings.retain(|_, v| !v.is_empty());
            n.languages.retain(|_, v| !v.trim().is_empty());
            Ok(Locale {
                code: n.code,
                name: n.name,
                native_name: n.native_name,
                author: n.author,
                classic: false,
                strings: n.strings,
                languages: n.languages,
            })
        }
    }

    fn from_classic(v: &Value) -> Locale {
        let flat = flatten_classic(v);
        let mut strings = HashMap::new();
        for (key, _, addr) in STRINGS {
            if let Some(addr) = addr {
                if let Some(text) = flat.get(*addr) {
                    if !text.is_empty() {
                        strings.insert((*key).to_owned(), unmnemonic(text));
                    }
                }
            }
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut native_name = s("LanguageNativeName");
        if native_name.is_empty() {
            native_name = s("LanguageName");
        }
        Locale {
            code: s("LanguageCode"),
            name: s("LanguageName"),
            native_name,
            author: s("Author"),
            classic: true,
            strings,
            languages: HashMap::new(),
        }
    }

    /// Serialise in the native format, e.g. to convert a classic file.
    pub fn to_native_json(&self) -> String {
        let mut strings: Vec<(&String, &String)> = self.strings.iter().collect();
        strings.sort();
        let map: serde_json::Map<String, Value> =
            strings.into_iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let mut doc = serde_json::json!({
            "code": self.code,
            "name": self.name,
            "native_name": self.native_name,
            "author": self.author,
            "strings": map,
        });
        if !self.languages.is_empty() {
            let mut names: Vec<(&String, &String)> = self.languages.iter().collect();
            names.sort();
            let names: serde_json::Map<String, Value> =
                names.into_iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
            doc["languages"] = Value::Object(names);
        }
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }
}

/// What the language list needs from a locale file, read without keeping
/// its strings: discovery reads only this, and the full file loads when the
/// language is chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocaleHeader {
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub classic: bool,
}

/// How serious a problem in a locale file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Harmless: the entry is ignored and everything else applies.
    Warning,
    /// The entry would break the interface, so it is skipped and the
    /// layer below (or English) shows instead.
    Error,
}

/// What is wrong with an entry of a locale file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    /// A string key the program does not know (a typo, or a string that was
    /// removed).
    UnknownKey,
    /// A language name for a tag that is not in the language table.
    UnknownLanguage,
    /// A different number of `%s` than the English text.
    Placeholders { expected: usize, found: usize },
    /// The file could not be read or parsed at all.
    Unreadable(String),
}

/// A problem found in a locale file, for the log, Options and
/// `glossa-cli locale check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleIssue {
    /// The file, when the issue came from one on disk.
    pub file: Option<std::path::PathBuf>,
    /// The string key or language tag; empty for a whole-file problem.
    pub key: String,
    pub kind: IssueKind,
    /// The file is a classic QTranslate `lang.json`, read from the user's
    /// installation: nothing the user is expected to fix.
    pub classic: bool,
}

impl LocaleIssue {
    /// A problem with the file itself.
    pub fn unreadable(file: &std::path::Path, why: String, classic: bool) -> Self {
        LocaleIssue { file: Some(file.to_owned()), key: String::new(), kind: IssueKind::Unreadable(why), classic }
    }

    /// Classic files are warnings whatever is wrong with them: the strings
    /// are skipped all the same, but the file belongs to the old program
    /// and is not the user's to fix.
    pub fn severity(&self) -> Severity {
        match self.kind {
            _ if self.classic => Severity::Warning,
            IssueKind::UnknownKey | IssueKind::UnknownLanguage => Severity::Warning,
            IssueKind::Placeholders { .. } | IssueKind::Unreadable(_) => Severity::Error,
        }
    }

    /// Where the issue is, shortly: the file's name, with its folder and a
    /// note for a classic file (every classic file is `lang.json`).
    pub fn source(&self, locale: &Locale) -> Option<String> {
        let file = self.file.as_ref()?;
        let name = file.file_name()?.to_string_lossy().into_owned();
        if !self.classic {
            return Some(name);
        }
        let folder = file.parent().and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned());
        let shown = match folder {
            Some(f) => format!("{f}/{name}"),
            None => name,
        };
        Some(locale.fmt("locale.issue.classic_file", &shown))
    }

    /// The issue in `locale`'s language, without the file name.
    pub fn message(&self, locale: &Locale) -> String {
        match &self.kind {
            IssueKind::UnknownKey => locale.fmt("locale.issue.unknown_key", &self.key),
            IssueKind::UnknownLanguage => locale.fmt("locale.issue.unknown_language", &self.key),
            IssueKind::Placeholders { expected, found } => {
                locale.fmt_args("locale.issue.placeholders", &[&self.key, &found.to_string(), &expected.to_string()])
            }
            IssueKind::Unreadable(why) => locale.fmt("locale.issue.unreadable", why),
        }
    }

    /// The message after the file's full path, when there is one.
    pub fn describe(&self, locale: &Locale) -> String {
        match &self.file {
            Some(f) if self.classic => {
                format!(
                    "{}: {}",
                    locale.fmt("locale.issue.classic_file", &f.display().to_string()),
                    self.message(locale)
                )
            }
            Some(f) => format!("{}: {}", f.display(), self.message(locale)),
            None => self.message(locale),
        }
    }
}

impl Locale {
    /// Read only the header of a locale file of either format.
    ///
    /// The strings are skipped as the text is read, not built: listing the
    /// languages costs a pass over each file and nothing more.
    pub fn parse_header(text: &str) -> Result<LocaleHeader, String> {
        use serde::de::IgnoredAny;
        // Both formats' header fields; everything else (the strings) is
        // passed over without being kept.
        #[derive(Deserialize)]
        #[allow(non_snake_case)]
        struct Header {
            code: Option<String>,
            #[serde(default)]
            name: String,
            #[serde(default)]
            native_name: String,
            #[serde(default)]
            LanguageCode: String,
            #[serde(default)]
            LanguageName: String,
            #[serde(default)]
            LanguageNativeName: String,
            Strings: Option<IgnoredAny>,
            Windows: Option<IgnoredAny>,
        }
        let h: Header = serde_json::from_str(&jsonc::strip(text)).map_err(|e| e.to_string())?;
        if h.Strings.is_some() || h.Windows.is_some() {
            let native_name =
                if h.LanguageNativeName.is_empty() { h.LanguageName.clone() } else { h.LanguageNativeName };
            Ok(LocaleHeader { code: h.LanguageCode, name: h.LanguageName, native_name, classic: true })
        } else {
            let code = h.code.ok_or("missing field `code`")?;
            Ok(LocaleHeader { code, name: h.name, native_name: h.native_name, classic: false })
        }
    }

    /// Check this locale against the program's keys and drop what would do
    /// harm: unknown keys and language tags are warnings (they would never
    /// show anyway), a string with a different number of `%s` than English is
    /// an error and is removed, so the layer below shows instead of a
    /// sentence with a hole or a stray placeholder.
    pub fn validate(&mut self) -> Vec<LocaleIssue> {
        let mut issues = Vec::new();
        let classic = self.classic;
        let issue = |key: &str, kind| LocaleIssue { file: None, key: key.to_owned(), kind, classic };
        let mut keys: Vec<String> = self.strings.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let Some(en) = english(&key) else {
                self.strings.remove(&key);
                issues.push(issue(&key, IssueKind::UnknownKey));
                continue;
            };
            let expected = en.matches("%s").count();
            let found = self.strings[&key].matches("%s").count();
            if expected != found {
                self.strings.remove(&key);
                issues.push(issue(&key, IssueKind::Placeholders { expected, found }));
            }
        }
        let tags: std::collections::HashSet<&str> = crate::LangId::all().map(|l| l.bcp47()).collect();
        let mut unknown: Vec<String> = self.languages.keys().filter(|k| !tags.contains(k.as_str())).cloned().collect();
        unknown.sort();
        for tag in unknown {
            self.languages.remove(&tag);
            issues.push(issue(&tag, IssueKind::UnknownLanguage));
        }
        issues
    }

    /// The keys this locale does not translate, in table order.
    pub fn untranslated_keys(&self) -> Vec<&'static str> {
        STRINGS.iter().map(|(k, _, _)| *k).filter(|k| !self.strings.contains_key(*k)).collect()
    }

    /// A pack for translators to start from, in the native format with
    /// comments (which locale files may carry): every key, with its English
    /// text and the classic address (`w:10:1070`, `s:172`…) above it where
    /// the classic program had the string, and the value from `base` or
    /// empty. An empty value counts as untranslated, so a half-filled
    /// template already loads. The language table's names follow, likewise.
    pub fn template(base: Option<&Locale>) -> String {
        let q = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let comment = |s: &str| s.replace(['\r', '\n'], " ");
        let empty = Locale::default();
        let base = base.unwrap_or(&empty);
        let mut out = String::new();
        out.push_str("// A Glossa interface language. Fill in the header and the strings, save it as\n");
        out.push_str("// <code>.json in the data folder's locales folder, and check it with\n");
        out.push_str("// `glossa-cli locale check FILE`. Keep every %s: each stands for a value.\n");
        out.push_str("// Empty strings fall back to English.\n{\n");
        for (field, value) in
            [("code", &base.code), ("name", &base.name), ("native_name", &base.native_name), ("author", &base.author)]
        {
            out.push_str(&format!("  {}: {},\n", q(field), q(value)));
        }
        out.push_str("  \"strings\": {\n");
        for (key, en, addr) in STRINGS {
            out.push_str(&format!("    // {}", comment(en)));
            if let Some(addr) = addr {
                out.push_str(&format!("  [classic {addr}]"));
            }
            out.push('\n');
            let value = base.strings.get(*key).map(String::as_str).unwrap_or("");
            out.push_str(&format!("    {}: {},\n", q(key), q(value)));
        }
        out.push_str("  },\n  \"languages\": {\n");
        for lang in crate::LangId::all() {
            let value = base.languages.get(lang.bcp47()).map(String::as_str).unwrap_or("");
            out.push_str(&format!("    {}: {}, // {}\n", q(lang.bcp47()), q(value), lang.english_name()));
        }
        out.push_str("  }\n}\n");
        out
    }
}

/// The classic files mark menu accelerators with `&` and write a literal
/// ampersand as `&&`.
fn unmnemonic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push('&');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Turn a classic locale file into `address -> text`. Addresses:
///
/// * `s:<id>` — a global string
/// * `w:<window>:caption` — a window title
/// * `w:<window>:<control>` — a control
/// * `w:<window>:<control>:<item>` — an item of a list control
/// * `w:<window>:s:<id>` — a string scoped to a window
/// * `m:<menu>:<item>` — a menu item
pub fn flatten_classic(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let pair = |item: &Value| -> Option<(i64, Value)> {
        let arr = item.as_array()?;
        Some((arr.first()?.as_i64()?, arr.get(1)?.clone()))
    };
    if let Some(list) = v.get("Strings").and_then(Value::as_array) {
        for item in list {
            if let Some((id, Value::String(text))) = pair(item) {
                out.insert(format!("s:{id}"), text);
            }
        }
    }
    if let Some(windows) = v.get("Windows").and_then(Value::as_array) {
        for w in windows {
            let Some(wid) = w.get("Id").and_then(Value::as_i64) else { continue };
            if let Some(cap) = w.get("Caption").and_then(Value::as_str) {
                out.insert(format!("w:{wid}:caption"), cap.to_owned());
            }
            for item in w.get("Controls").and_then(Value::as_array).into_iter().flatten() {
                match pair(item) {
                    Some((cid, Value::String(text))) => {
                        out.insert(format!("w:{wid}:{cid}"), text);
                    }
                    Some((cid, Value::Array(sub))) => {
                        for s in &sub {
                            if let Some((sid, Value::String(text))) = pair(s) {
                                out.insert(format!("w:{wid}:{cid}:{sid}"), text);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for item in w.get("Strings").and_then(Value::as_array).into_iter().flatten() {
                if let Some((sid, Value::String(text))) = pair(item) {
                    out.insert(format!("w:{wid}:s:{sid}"), text);
                }
            }
        }
    }
    if let Some(menus) = v.get("Menus").and_then(Value::as_array) {
        for m in menus {
            let Some(mid) = m.get("Id").and_then(Value::as_i64) else { continue };
            for item in m.get("Items").and_then(Value::as_array).into_iter().flatten() {
                if let Some((iid, Value::String(text))) = pair(item) {
                    out.insert(format!("m:{mid}:{iid}"), text);
                }
            }
        }
    }
    out
}

fn english(key: &str) -> Option<&'static str> {
    STRINGS.iter().find(|(k, _, _)| *k == key).map(|(_, en, _)| *en)
}

/// Every interface string: key, English text, classic address (if the
/// classic UI had this string).
pub static STRINGS: &[(&str, &str, Option<&str>)] = &[
    // -- messages ---------------------------------------------------------
    ("msg.clear", "Clear current translation (Ctrl+N)", Some("s:171")),
    ("msg.translate_tip", "Translate (Ctrl+Enter)", Some("s:172")),
    ("msg.swap_tip", "Swap translation direction (Ctrl+I)", Some("s:173")),
    ("msg.listen_tip", "Listen to the text", Some("s:174")),
    ("msg.back_tip", "Go back (Alt+Left Arrow)", Some("s:175")),
    ("msg.forward_tip", "Go forward (Alt+Right Arrow)", Some("s:184")),
    ("msg.cannot_detect", "Cannot detect language. Please choose it manually.", Some("s:176")),
    ("msg.source_unsupported", "Service doesn't support source language.", Some("s:177")),
    ("msg.target_unsupported", "Service doesn't support destination language.", Some("s:198")),
    ("msg.about", "About Glossa", Some("s:178")),
    ("msg.version", "Version ", Some("s:179")),
    ("msg.update_unavailable", "Update server is not available", Some("s:180")),
    ("msg.update_available", "New version (%s) is available", Some("s:181")),
    ("msg.up_to_date", "%s is up to date", Some("s:182")),
    ("msg.checking_updates", "Checking for updates...", Some("s:183")),
    ("msg.update_failed", "Update failed", Some("s:185")),
    ("msg.romanization", "Romanization: ", Some("s:186")),
    ("msg.always_on_top", "Always on top", Some("s:187")),
    ("msg.keyboard", "Keyboard", Some("s:188")),

    ("msg.timeout", "No data returned (timeout while sending data).", Some("s:190")),
    ("msg.no_data", "No data returned.", Some("s:191")),
    ("msg.nothing_to_translate", "No data to translate.", Some("s:192")),
    ("msg.capability_unsupported", "Service doesn't support requested capability.", Some("s:193")),
    ("msg.cannot_play", "Cannot play audio stream.", Some("s:194")),
    ("msg.no_audio", "Service doesn't support audio for this language, or text is too long.", Some("s:195")),
    ("msg.script_error", "Script service error.", Some("s:196")),
    ("msg.unexpected", "Unexpected error.", Some("s:197")),
    ("msg.file_not_found", "Cannot find file: ", Some("s:199")),
    ("msg.downloading_update", "Downloading update...", Some("s:200")),
    ("msg.add_favorite", "Add to favorites", Some("s:201")),
    ("msg.remove_favorite", "Remove from favorites", Some("s:202")),
    ("msg.no_microphone", "Make sure the microphone is connected to your computer.", Some("s:203")),
    ("msg.no_connection", "Unable to establish Internet connection", Some("s:204")),
    ("msg.speech_on", "Turn on speech input (%s)", Some("s:205")),
    ("msg.speech_off", "Turn off speech input", Some("s:206")),
    ("msg.no_text_in_image", "No text found in the selected area.", None),
    ("msg.ocr_unavailable", "OCR is not available: %s", None),
    ("msg.copied", "Copied", None),
    ("msg.translating", "Translating…", None),
    ("msg.recognizing", "Recognising…", None),
    ("msg.yes", "Yes", None),
    ("msg.no", "No", None),
    ("msg.ms", "%s ms", None),
    ("msg.watching", "Watching the region — press the hotkey again to stop.", None),
    ("msg.watch_stopped", "Stopped watching the region.", None),
    ("msg.not_available", "%s — not available in this version.", None),
    // -- main window --------------------------------------------------------
    ("main.title", "Glossa", Some("w:1:caption")),
    ("main.translate", "Translate", Some("w:1:1004")),
    ("main.auto_detect", "Auto-detect", None),
    ("main.auto_detect_as", "Auto-detect (%s)", None),
    ("main.source_placeholder", "Type, paste or drop text here", None),
    ("main.tried_first", "Tried first: %s", None),
    ("lang.search", "Search…", None),
    // -- on-screen keyboard -------------------------------------------------
    ("keyboard.backspace", "Backspace", None),
    ("keyboard.enter", "Enter", None),
    ("keyboard.shift", "Shift", None),
    ("keyboard.tab", "Tab", None),
    ("keyboard.caps", "Caps Lock", None),
    ("keyboard.ctrl", "Ctrl", None),
    ("keyboard.alt", "Alt", None),
    ("keyboard.win", "Win", None),
    ("keyboard.super", "Super", None),
    ("keyboard.mac.delete", "Delete", None),
    ("keyboard.mac.return", "Return", None),
    ("keyboard.mac.fn", "Fn", None),
    ("keyboard.mac.control", "Control", None),
    ("keyboard.mac.option", "Option", None),
    ("keyboard.mac.cmd", "Cmd", None),
    ("keyboard.size_tip", "Ctrl + mouse wheel, or drag the window's edge, to resize the keyboard", None),
    // -- about window -------------------------------------------------------
    (
        "about.description",
        "A fast, small and extensible desktop translator: a clean-room successor to QTranslate.",
        None,
    ),
    ("about.authors", "Co-authors", None),
    ("about.license", "License: %s", None),
    ("about.mit", "MIT License", None),
    ("about.apache", "Apache License 2.0", None),
    ("about.notices", "Third-party notices", None),
    ("about.open_online", "Open on GitHub", None),
    ("about.back", "Back", None),
    ("about.not_affiliated", "Not affiliated with QTranslate or its author.", None),
    ("about.support_hint", "Glossa is free. If it helps you, you can support its development on Boosty.", None),
    // -- popup --------------------------------------------------------------
    ("popup.dictionary", "Dictionary search for", Some("w:6:10:20")),
    ("popup.listen", "Listen to selected text", Some("w:6:10:30")),
    ("popup.copy", "Copy translation to clipboard", Some("w:6:10:40")),
    ("popup.title_direction", "%s to %s (%s)", None),
    ("popup.answered_instead", "From %s: %s did not answer", None),
    ("popup.reset_size", "Back to automatic size", None),
    ("popup.change_target", "Click to translate into another language", None),
    ("popup.target_auto", "Automatic direction", None),
    ("popup.swap", "Swap: translate from %s into %s", None),
    ("mouse.translate", "Translate the selection", None),
    ("mouse.unavailable", "Mouse mode is not available on this system.", None),
    ("mouse.failed", "Mouse mode could not start: %s", None),
    (
        "main.tray_hint",
        "Glossa lives in the tray (the menu bar on macOS) and is ready when you need it. Select text in any program and press %s to translate it.",
        None,
    ),
    (
        "main.tray_hint_no_hotkey",
        "Glossa lives in the tray (the menu bar on macOS). Its icon's menu has text recognition, the dictionary and the settings.",
        None,
    ),
    ("main.hint_dismiss", "Got it", None),
    ("main.startup_warnings", "Problems found at start-up:", None),
    ("main.recent", "Recent", None),
    ("options.appearance.backdrop", "Window backdrop:", None),
    ("options.appearance.popup_backdrop", "Popup backdrop:", None),
    ("options.appearance.popup_shade", "Shade when not blurred (0 - 90):", None),
    (
        "options.appearance.popup_shade_hint",
        "Darkens a see-through popup when nothing blurs what is behind it, so its text stays readable.",
        None,
    ),
    ("options.backdrop.none", "None", None),
    ("options.backdrop.blur", "Blur", None),
    ("options.backdrop.mica", "Mica", None),
    ("options.backdrop.mica_alt", "Mica Alt", None),
    ("options.backdrop.acrylic", "Acrylic", None),
    // macOS: the NSVisualEffectView materials the same choices become.
    ("options.backdrop.mac.window", "Window", None),
    ("options.backdrop.mac.under_window", "Under window", None),
    ("options.backdrop.mac.popover", "Popover", None),
    ("options.backdrop.mac.full_screen", "Full-screen UI", None),
    ("options.backdrop.needs_opengl", "Backdrops need Windows, or macOS with the OpenGL engine.", None),
    ("popup.close", "Close", None),
    ("popup.pin", "Keep it open", None),
    ("popup.unpin", "Let it hide again", None),
    ("options.services.needs_key", "needs a key", None),
    ("options.services.pick_one", "Select a service in the list to see its settings.", None),
    ("options.services.capabilities", "Capabilities:", None),
    ("options.services.no_settings", "No settings.", None),
    ("options.services.show_keys", "Show keys", None),
    ("options.services.check_settings", "check the settings", None),
    ("field.required", "Required.", None),
    ("field.pattern", "Does not match the expected format.", None),
    ("field.too_short", "At least %s characters.", None),
    ("field.too_long", "At most %s characters.", None),
    ("field.too_few", "Pick at least %s.", None),
    ("field.too_many", "Pick at most %s.", None),
    ("field.below_min", "At least %s.", None),
    ("field.above_max", "At most %s.", None),
    ("field.not_url", "Enter a web address starting with http:// or https://.", None),
    ("field.not_choice", "Not one of the choices.", None),
    ("field.not_number", "Not a number.", None),
    ("field.not_language", "Not a language Glossa knows.", None),
    ("field.needs_newer", "This setting needs a newer version of Glossa.", None),
    ("field.browse", "Browse…", None),
    ("field.none_selected", "None", None),
    ("field.n_selected", "%s selected", None),
    ("field.not_in_list", "%s (not in list)", None),
    ("module_action.needs", "Fill in first: %s", None),
    ("module_action.done", "Done.", None),
    ("panel.bad_image", "The picture could not be shown.", None),
    ("options.restart_needed", "Some changes take effect after a restart.", None),
    ("options.restart_now", "Restart now", None),
    ("options.restart_later", "Later", None),
    ("options.appearance.icon_scale", "Icon size:", None),
    ("options.appearance.popup_icon_scale", "Header icon size:", None),
    ("options.appearance.backdrop_opacity", "Backdrop opacity:", None),
    (
        "options.appearance.backdrop_opacity_hint",
        "How much of the theme's colour (and of Acrylic's tint) covers the backdrop; lower shows more of what is behind. Mica tints itself: only the window's own colours thin out there.",
        None,
    ),
    (
        "options.appearance.backdrop_opacity_hint_mac",
        "How much of the theme's colour covers the material; lower shows more of what is behind the window. The material tints itself, light or dark with the theme.",
        None,
    ),
    ("options.appearance.show_border", "Show the popup border", None),
    ("popup.replace", "Replace selected text", Some("w:6:10:50")),
    // -- dictionary ---------------------------------------------------------
    ("dict.title", "Dictionary", Some("w:5:caption")),
    ("dict.no_results", "No results found for '%s'.", Some("w:5:s:10")),
    // -- history ------------------------------------------------------------
    ("history.title", "History", Some("w:3:caption")),
    ("history.save_as", "Save as...", Some("w:3:1160")),
    ("history.clear", "Clear", Some("w:3:1067")),
    ("history.search", "Search history", None),
    ("history.favorites_only", "Favourites only", None),
    ("history.favorite", "Favourite", None),
    ("history.confirm_clear", "Clear all non-favourite entries?", None),
    ("history.format_text", "Text file", None),
    ("history.format_csv", "CSV file", None),
    ("history.format_html", "HTML file", None),
    ("history.format_json", "JSON file", None),
    // -- options frame --------------------------------------------------------
    ("options.title", "Options", Some("w:2:caption")),
    ("options.ok", "OK", Some("w:2:1")),
    ("options.cancel", "Cancel", Some("w:2:2")),
    ("options.apply", "Apply", Some("w:2:1049")),
    ("options.saved", "Saved.", None),
    ("options.remove", "Remove", None),
    // -- options: basics --------------------------------------------------------
    ("options.appearance.preview", "Preview", None),
    ("options.appearance.preview_source", "Hello, everyone! How are you today?", None),
    ("options.appearance.preview_translation", "Всем привет! Как у вас дела?", None),
    ("options.ocr.ai_group", "AI endpoints (vision models)", None),
    ("options.ocr.ai_hint", "%s reads the picture with a vision model, and only when chosen here. Its model, answer format and box coordinates are in its settings.", None),
    ("options.ocr.ai_settings", "Settings…", None),
    ("options.appearance.follow_system", "Follow the system's light and dark mode", None),
    ("options.appearance.theme_light", "Light theme:", None),
    ("options.appearance.theme_dark", "Dark theme:", None),
    ("options.appearance.follow_system_unknown", "This system does not say whether it is light or dark, so this theme is used:", None),
    ("options.group.general", "General", None),
    ("options.group.translation", "Translation", None),
    ("options.group.recognition", "Recognition", None),
    ("options.group.system", "System", None),
    ("options.basics", "Basics", Some("w:10:caption")),
    ("options.basics.start_with_system", "Start with Windows", Some("w:10:1045")),
    ("options.basics.ui_language", "Interface language:", Some("w:10:1070")),
    ("options.basics.font", "Font name:", Some("w:10:1075")),
    ("options.basics.text_size", "Text size:", Some("w:10:1074")),
    ("options.basics.first_language", "First language:", Some("w:10:1068")),
    ("options.basics.second_language", "Second language:", Some("w:10:1069")),
    ("options.basics.speech_input", "Speech input:", Some("w:10:1071")),
    ("options.basics.auto_detect_group", "Auto-detect languages", Some("w:10:1063")),
    ("options.basics.general_group", "General", Some("w:10:1046")),
    ("options.basics.privacy_group", "Keys and privacy", None),
    ("options.basics.secret_storage", "Keep API keys, the proxy password and the Lens token:", None),
    ("options.basics.secrets_file", "In the settings file (plain text, readable by your account only)", None),
    ("options.basics.secrets_system", "In the system keychain: %s", None),
    ("options.basics.secrets_note", "In the keychain the settings file keeps only a placeholder, so it can be shared or synced without the keys. A portable copy on a USB stick is better off with the file: the keychain stays on this computer.", None),
    ("options.basics.secrets_none", "No keychain answers on this system, so keys stay in the settings file.", None),
    ("options.basics.show_welcome", "Show the welcome guide", None),
    ("welcome.title", "Welcome to Glossa", None),
    ("welcome.step", "Step %s of %s", None),
    ("welcome.hello.look", "Make it yours", None),
    ("welcome.hotkeys.none", "Not set", None),
    ("welcome.usage.replace_unset", "Give “Replace selected text” a hotkey on the previous page, and the translation goes right where you type.", None),
    ("welcome.storage.file_title", "Settings file", None),
    ("welcome.storage.file_text", "Plain text, readable by your account only. Simple, and it travels with a portable copy.", None),
    ("welcome.storage.system_text", "The settings file keeps only a placeholder, so it can be shared or synced without the keys.", None),
    ("welcome.done.try", "Try it now", None),
    ("welcome.done.select", "Select a word in any program and press", None),
    ("welcome.done.later", "Everything here can be changed later in Options; this guide is in Options → Basics.", None),
    ("welcome.next", "Next", None),
    ("welcome.back", "Back", None),
    ("welcome.skip", "Skip", None),
    ("welcome.finish", "Finish", None),
    ("welcome.hello.title", "A translator that is always at hand", None),
    ("welcome.hello.what", "Glossa translates text anywhere on your computer: select it in any program and press a hotkey. It also reads text off the screen (OCR), looks words up in dictionaries, reads aloud, and lets you pick among many translation services, AI models included.", None),
    ("welcome.hello.tray", "It lives in the tray, next to the clock, and stays out of the way until you call it. Closing a window does not quit it: use the tray icon's menu for that.", None),
    ("welcome.hello.menu_bar", "It lives in the menu bar and stays out of the way until you call it. Closing a window does not quit it: use the menu-bar icon or ⌘Q.", None),
    ("welcome.hotkeys.title", "Hotkeys", None),
    ("welcome.hotkeys.intro", "These work in every program. Click a key to change it, press the new combination, Esc to cancel, Backspace to clear it.", None),
    ("welcome.hotkeys.unavailable", "Global hotkeys are not available in this session; Options → Hotkeys explains what to do instead.", None),
    ("welcome.hotkeys.more", "There are more actions (the dictionary, reading aloud, the keyboard layout fix…) in Options → Hotkeys.", None),
    ("welcome.usage.title", "How to use it", None),
    ("welcome.usage.select", "Select text in any program and press %s: the translation pops up next to it. Press it again to close the popup.", None),
    ("welcome.usage.main", "%s opens the main window, for typing, pasting or dropping text and files.", None),
    ("welcome.usage.ocr", "%s lets you draw a box around text on the screen (a picture, a game, a video) and translates it.", None),
    ("welcome.usage.replace", "%s replaces the selected text with its translation, right where you type.", None),
    ("welcome.usage.zoom", "%s and the mouse wheel over a text area make its text bigger or smaller.", None),
    ("welcome.usage.services", "The service strip under the translation switches between services; Options → Services chooses which are shown and takes API keys.", None),
    ("welcome.storage.title", "Keys and data", None),
    ("welcome.storage.data", "Settings and history", None),
    ("welcome.storage.profile_now", "They are kept in your user profile: %s", None),
    ("welcome.storage.home_now", "They are kept in the folder given with --home or GLOSSA_HOME: %s", None),
    ("welcome.storage.portable_now", "This is a portable copy: they are kept beside the program, in %s", None),
    ("welcome.storage.make_portable", "Keep them beside the program instead (a portable copy, for a USB stick)", None),
    ("welcome.storage.portable_note", "They are copied to a data folder in %s and Glossa restarts when you finish. The copy in your profile is left as it is.", None),
    ("welcome.storage.portable_failed", "Could not make the copy portable: %s", None),
    ("welcome.done.title", "All set", None),
    ("welcome.done.text", "Try it now: select a word anywhere and press %s. Everything here can be changed later in Options, and this guide is in Options → Basics.", None),
    ("welcome.done.restart", "Glossa restarts once to move its data beside the program.", None),
    ("options.basics.history_group", "History", Some("w:10:1064")),
    ("options.basics.enable_history", "Enable history", Some("w:10:1122")),
    ("options.basics.clear_history_on_exit", "Clear history on exit", Some("w:10:1124")),
    ("options.basics.expand_items", "Expand items", Some("w:10:1123")),
    ("options.basics.ui_language_system", "System", None),
    ("options.basics.ui_language_folder", "Language folder", None),
    ("options.basics.ui_language_reload", "Reload languages", None),
    ("options.basics.ui_language_problems", "Problems in the language files:", None),
    ("locale.issue.unknown_key", "%s: unknown key, ignored", None),
    ("locale.issue.unknown_language", "%s: unknown language tag, ignored", None),
    ("locale.issue.placeholders", "%s: %s placeholders where English has %s; the string is skipped", None),
    ("locale.issue.unreadable", "cannot be read: %s", None),
    ("locale.issue.classic_file", "%s (classic QTranslate file)", None),
    ("options.basics.font_default", "--- Default ---", None),
    (
        "options.basics.smart_direction",
        "Smart direction: the first language goes to the second, everything else to the first",
        None,
    ),
    // -- options: internet ------------------------------------------------------
    ("options.internet", "Internet", Some("w:11:caption")),
    ("options.internet.connection", "Connection", Some("w:11:1049")),
    ("options.internet.timeout", "Timeout, sec:", Some("w:11:1084")),
    ("options.internet.proxy", "Proxy", Some("w:11:1048")),
    ("options.internet.host", "Host", Some("w:11:1034")),
    ("options.internet.port", "Port", Some("w:11:1035")),
    ("options.internet.username", "Username", Some("w:11:1036")),
    ("options.internet.password", "Password", Some("w:11:1037")),
    ("options.internet.proxy_system", "Use system proxy settings", Some("w:11:1135:0")),
    ("options.internet.proxy_none", "No proxy", Some("w:11:1135:1")),
    ("options.internet.proxy_manual", "Manual proxy configuration", Some("w:11:1135:2")),
    ("options.internet.proxy_auto", "Auto-detect proxy settings", Some("w:11:1135:3")),
    ("options.internet.retries", "Retries:", None),
    ("options.internet.password_plain", "The password is stored in the settings file in plain text.", None),
    // -- options: services --------------------------------------------------------
    ("options.services", "Services", Some("w:12:caption")),
    ("options.services.add_dictionary", "Add dictionary...", Some("w:12:1177")),
    ("options.services.remove_dictionary", "Remove dictionary", Some("w:12:1180")),
    ("options.services.translation", "Translation", None),
    ("options.services.dictionaries", "Dictionaries", None),
    ("options.services.speech", "Speech", None),
    ("options.services.ocr", "Text recognition", None),
    ("options.services.settings", "Settings", None),
    ("options.services.fail_over", "If a service fails, try the next one", None),
    ("options.services.origin_builtin", "Built-in", None),
    ("options.services.origin_bundled", "Bundled", None),
    ("capability.translate", "translation", None),
    ("capability.detect_language", "language detection", None),
    ("capability.listen", "speech", None),
    ("capability.dictionary", "dictionary", None),
    ("capability.ocr", "text recognition", None),
    ("capability.speech_input", "speech input", None),
    ("capability.batch", "batches", None),
    ("capability.alternatives", "alternatives", None),
    ("capability.promptable", "custom prompt", None),
    ("capability.offline", "offline", None),
    // -- options: AI endpoints (on the Services page) ---------------------------
    ("options.ai.title", "AI endpoints (OpenAI-compatible)", None),
    (
        "options.ai.hint",
        "Each endpoint is a translator of its own: OpenAI, OpenRouter, DeepSeek, Gemini, a local Ollama… \
         Select it in the list above for its settings.",
        None,
    ),
    ("options.ai.add", "Add endpoint", None),
    ("options.ai.load_models", "Load models", None),
    ("options.ai.test", "Test", None),
    ("options.ai.test_tip", "Translate a short sample", None),
    ("options.ai.test_ok", "OK, %s ms: %s", None),
    ("options.ai.models", "Models (%s)", None),
    ("options.ai.models_loaded", "%s models", None),
    ("options.ai.pick_model", "Pick a model", None),
    // -- options: languages --------------------------------------------------------
    ("options.languages", "Languages", Some("w:13:caption")),
    ("options.languages.toggle_all", "Check / Uncheck all", Some("w:13:1099")),
    // -- options: appearance ---------------------------------------------------------
    ("options.appearance", "Appearance", Some("w:14:caption")),
    ("options.appearance.general", "General", Some("w:14:1076")),
    ("options.appearance.window_style", "Enable window style", Some("w:14:1123")),
    ("options.appearance.theme", "Theme:", Some("w:14:1169")),
    ("options.appearance.auto_size", "Enable auto size", Some("w:14:1158")),
    ("options.appearance.auto_position", "Enable auto position", Some("w:14:1160")),
    ("options.appearance.activate", "Activate when opened", Some("w:14:1159")),
    ("options.appearance.pin_when_dragging", "Pin when dragging", Some("w:14:1161")),
    ("options.appearance.popup_group", "Popup window", Some("w:14:1124")),
    ("options.appearance.back_color", "Background color:", Some("w:14:1092")),
    ("options.appearance.text_color", "Text color:", Some("w:14:1091")),
    ("options.appearance.frame_color", "Frame color:", Some("w:14:1090")),
    ("options.appearance.transparency", "Transparency (0 - 90):", Some("w:14:1145")),
    ("options.appearance.frame_thickness", "Frame thickness:", Some("w:14:1146")),
    ("options.appearance.auto_hide", "Auto-hide delay:", Some("w:14:1147")),
    ("options.appearance.seconds", "seconds", Some("w:14:1194")),
    ("options.appearance.icons", "Icons:", Some("w:14:1137")),
    ("options.appearance.icon_dictionary", "Show open dictionary icon", Some("w:14:1138")),
    ("options.appearance.icon_listen", "Show listen to selected text icon", Some("w:14:1139")),
    ("options.appearance.icon_copy", "Show copy text to clipboard icon", Some("w:14:1140")),
    ("options.appearance.icon_replace", "Show replace selected text icon", Some("w:14:1141")),
    ("options.appearance.ui_scale", "Interface scale:", None),
    ("options.appearance.edit_theme", "Edit theme file", None),
    ("options.appearance.reload_themes", "Reload themes", None),
    ("options.appearance.classic_theme", "%s  (classic)", None),
    ("options.appearance.window_transparency", "Window transparency (0 - 90):", None),
    ("options.appearance.renderer", "Graphics engine (after restart):", None),
    ("options.appearance.renderer_opengl", "OpenGL — lightest", None),
    ("options.appearance.renderer_gpu", "GPU — Direct3D 12 / Vulkan / Metal", None),
    ("options.appearance.renderer_software", "Software — no GPU (WARP)", None),
    (
        "options.appearance.renderer_hint",
        "How windows are drawn. OpenGL uses the least memory; try another if windows stay black or flicker.",
        None,
    ),
    ("options.appearance.unload_after", "Unload graphics after hiding, s (0 = never):", None),
    (
        "options.appearance.unload_hint",
        "While everything is hidden in the tray, Glossa frees its graphics and keeps only the tray, hotkeys \
         and engine — a few MB. The first window afterwards takes a moment longer to appear.",
        None,
    ),
    ("options.appearance.popup_width", "Width:", None),
    ("options.appearance.width_max", "max ", None),
    ("options.appearance.width_min", "min ", None),
    // -- options: exceptions -----------------------------------------------------------
    ("options.exceptions", "Exceptions", Some("w:15:caption")),
    ("options.exceptions.enabled_for", "Enabled for these programs:", Some("w:15:1162:0")),
    ("options.exceptions.disabled_for", "Disabled for these programs:", Some("w:15:1162:1")),
    ("options.exceptions.add", "Add", Some("w:15:1101")),
    ("options.exceptions.modify", "Modify", Some("w:15:1103")),
    ("options.exceptions.delete", "Delete", Some("w:15:1102")),
    ("options.exceptions.smart", "Enable smart detection", Some("w:15:1113")),
    ("options.exceptions.program", "Program name", None),
    ("options.exceptions.window_class", "Window class name", None),
    ("options.exceptions.any_program", "* All Programs *", None),
    ("options.exceptions.any_window", "* All Windows *", None),
    ("options.exceptions.from_active", "From the active window", None),
    ("options.exceptions.from_active_tip", "Fill from the window that was active before this one", None),
    ("options.exceptions.empty", "No programs yet. Add one by its name, or take it from the active window.", None),
    // -- options: hotkeys ----------------------------------------------------------------
    ("options.hotkeys", "Hotkeys", Some("w:16:caption")),
    ("options.hotkeys.enable", "Enable hotkeys", Some("w:16:1118")),
    ("options.hotkeys.action", "Action", None),
    ("options.hotkeys.hotkey", "Hotkey", None),
    ("options.hotkeys.press_keys", "Press a key combination… (Esc cancels, Backspace clears)", None),
    ("options.hotkeys.conflict", "Also bound to: %s", None),
    ("options.hotkeys.taken", "%s is already in use by another program", None),
    ("options.hotkeys.unknown_key", "The key \"%s\" cannot be used in a global hotkey", None),
    ("options.hotkeys.unsupported", "Global hotkeys are not available: %s", None),
    ("options.hotkeys.failed", "Cannot register %s: %s", None),
    (
        "options.hotkeys.wayland",
        "Global hotkeys are not available in this session (Wayland). Bind a shortcut in your desktop's \
         keyboard settings to:",
        None,
    ),
    ("action.show_main_window", "Show main window", Some("w:16:1165:0")),
    // The classic "Show popup window" (w:16:1165:1) read as if it only
    // opened a window; it translates the selection there, so it is not
    // mapped and the classic files' wording does not win over this one.
    ("action.show_popup", "Translate selection", None),
    ("action.show_dictionary", "Show dictionary window", Some("w:16:1165:2")),
    ("action.show_history", "Show history window", Some("w:16:1165:3")),
    ("action.show_keyboard", "Show keyboard window", Some("w:16:1165:4")),
    ("action.listen_selection", "Listen to selected text", Some("w:16:1165:5")),
    ("action.copy_translation", "Copy translation to clipboard", Some("w:16:1165:6")),
    ("action.replace_selection", "Replace selected text", Some("w:16:1165:7")),
    ("action.toggle_mouse_mode", "Switch mouse mode", Some("w:16:1165:8")),
    ("action.translate_clipboard", "Translate clipboard content", Some("w:16:1165:9")),
    ("action.translate_clipboard_in_main", "Translate clipboard in main window", Some("w:16:1165:10")),
    ("action.translate_clipboard_in_popup", "Translate clipboard in popup window", Some("w:16:1165:11")),
    ("action.dictionary_from_clipboard", "Dictionary from clipboard", Some("w:16:1165:12")),
    ("action.convert_layout", "Change keyboard layout and selected text", Some("w:16:1165:13")),
    ("action.listen_translation", "Listen to selected text translation", Some("w:16:1165:14")),
    ("action.text_recognition", "Text recognition", Some("w:16:1165:15")),
    ("action.speech_input", "Speech input", Some("w:16:1165:16")),
    ("action.recognize_and_translate", "Recognise and translate a screen region", None),
    ("action.repeat_last_recognition", "Repeat the last recognised region", None),
    ("action.recognize_to_clipboard", "Recognise a screen region to the clipboard", None),
    ("action.watch_region", "Watch a screen region and translate changes", None),
    ("action.translate_region_in_place", "Translate a screen region in place", None),
    ("action.translate_screen_in_place", "Translate the whole screen in place", None),
    ("action.swap_languages", "Swap translation direction", None),
    ("action.next_service", "Switch to the next service", None),
    ("action.next_preset", "Switch to the next preset", None),
    ("tray.next_preset", "Next preset", None),
    ("tray.modules", "Modules", None),
    ("menu.presets", "Presets", None),
    ("preset.applied", "Preset: %s", None),
    ("preset.skipped", "not installed, skipped: %s", None),
    ("preset.none", "There are no presets", None),
    ("preset.builtin.everyday", "Everyday", None),
    ("preset.builtin.private", "Private", None),
    ("preset.builtin.word_study", "Word study", None),
    ("options.services.presets", "Presets", None),
    ("options.presets.hint", "A preset switches the services, their order, the active one and module settings in one click. Keys and tokens are never stored in a preset: it uses the ones already set.", None),
    ("options.presets.unsaved_hint", "Presets work with the applied settings: apply changes made in this window first.", None),
    ("options.presets.save_as", "Save current setup as:", None),
    ("options.presets.with_pair", "With the language pair", None),
    ("options.presets.save", "Save", None),
    ("options.presets.import", "Import", None),
    ("options.presets.export", "Export", None),
    ("options.presets.apply", "Apply", None),
    ("options.presets.duplicate", "Duplicate", None),
    ("options.presets.rename", "Rename", None),
    ("options.presets.delete", "Delete", None),
    ("options.presets.current", "current", None),
    ("options.presets.builtin", "built-in", None),
    ("options.presets.services", "Services: %s", None),
    ("options.presets.dictionaries", "Dictionaries: %s", None),
    ("options.presets.pair", "Languages: %s", None),
    ("options.presets.unchanged", "unchanged", None),
    ("options.presets.missing", "Not installed here, skipped when applied: %s", None),
    ("options.presets.pick_one", "Choose a preset to see what it sets.", None),
    ("options.presets.readonly", "Built-in presets cannot be changed; duplicate one to make your own.", None),
    ("options.presets.saved", "Saved preset %s", None),
    ("options.presets.imported", "Imported preset %s", None),
    ("options.presets.exported", "Exported to %s", None),
    ("options.presets.dropped", "keys and tokens left out: %s", None),
    ("options.presets.file_filter", "Glossa preset", None),
    (
        "options.presets.dropped_options",
        "left out (addresses and settings no installed module has): %s",
        None,
    ),
    ("options.presets.too_large", "too large for a preset file", None),
    ("action.show_options", "Show options", None),
    ("action.quit", "Exit", None),
    ("action.open_panel", "Open a module panel", None),
    ("options.hotkeys.panels", "Module panels", None),
    // -- options: advanced ---------------------------------------------------------------
    ("options.advanced", "Advanced", Some("w:17:caption")),
    ("options.advanced.group", "Advanced", Some("w:17:1146")),
    ("options.advanced.browser", "Default browser:", Some("w:17:1151")),
    ("options.advanced.copy_action", "Copy action:", Some("w:17:1147")),
    ("options.advanced.layout_indicator", "Keyboard layout indicator:", Some("w:17:1149")),
    ("options.advanced.preferred_domain", "Preferred domain:", Some("w:17:1157")),
    ("options.advanced.gui_translation", "Enable GUI translation", Some("w:17:1154")),
    ("options.advanced.slower_listen", "Enable listening to the same text at a slower speed", Some("w:17:1159")),
    ("options.advanced.remove_line_breaks", "Remove line breaks", Some("w:17:1160")),
    ("options.advanced.mouse_mode_group", "Mouse mode", Some("w:17:1186")),
    ("options.advanced.mouse_ctrl_only", "Enable mouse mode only when Ctrl is pressed", Some("w:17:1155")),
    (
        "options.advanced.mouse_ctrl_only_hint",
        "Only a selection made with Ctrl held down is translated; ordinary selections are left alone.",
        None,
    ),
    ("options.advanced.api_keys", "API keys", Some("w:17:1158")),
    ("options.advanced.ocr_key", "OCR API key:", Some("w:17:1195")),
    ("options.advanced.browser_default", "system default", None),
    ("options.advanced.copy_insert_no_key", "Ctrl + Insert (no Insert key here: sends %s + C)", None),
    ("options.advanced.copy_accessibility", "Accessibility only", None),
    ("options.advanced.copy_wait", "Selection copy wait, ms:", None),
    (
        "options.advanced.browser_hint",
        "Opens a service's website (the service name in the main window) and links. Empty uses the system's default browser; otherwise the path of a browser's program.",
        None,
    ),
    (
        "options.advanced.copy_action_hint",
        "How the selection is taken from programs that cannot be read directly: by pressing these keys for them (the clipboard is put back afterwards), or not at all.",
        None,
    ),
    (
        "options.advanced.preferred_domain_hint",
        "The country site used by services that have several, such as translate.google.ru for \"ru\".",
        None,
    ),
    (
        "options.advanced.copy_wait_hint",
        "How long to wait for a program to copy the selection. Raise it if a slow program's selection is sometimes missed.",
        None,
    ),
    ("options.advanced.slower_listen_hint", "Listening to the same text again reads it more slowly.", None),
    (
        "options.advanced.remove_line_breaks_hint",
        "Joins lines broken in the middle of a sentence (text copied from PDFs and e-mails) before translating.",
        None,
    ),
    (
        "options.advanced.mouse_mode_note",
        "Select text with the mouse to translate it, no hotkey needed. The text is read straight from the program (UI Automation on Windows, Accessibility on macOS), so the clipboard is left alone. A click on the tray icon can turn the mode on and off (see Tray icon).",
        None,
    ),
    ("options.advanced.tray_group", "Tray icon", None),
    ("options.advanced.tray_group_menu_bar", "Menu bar icon", None),
    ("options.tray.click", "Click:", None),
    ("options.tray.double_click", "Double click:", None),
    ("options.tray.middle_click", "Middle click:", None),
    ("options.tray.menu", "Open the menu", None),
    ("options.tray.nothing", "Nothing", None),
    (
        "options.tray.hint",
        "Actions on the selection read it from the program you were in before the click. While a double click does something, a single click waits out the double-click time before it acts.",
        None,
    ),
    ("options.tray.menu_hint", "While a click opens the menu, a double click cannot be told from it.", None),
    ("options.tray.unsupported", "This desktop's tray does not report clicks: any click opens the menu.", None),
    // -- options: updates ------------------------------------------------------------------
    ("options.basics.start_with_system_macos", "Open at login", None),
    ("options.basics.start_with_system_linux", "Start at login", None),
    ("options.basics.autostart_pending", "Takes effect when you press OK or Apply.", None),
    ("options.basics.autostart_note", "Done: Glossa will start with Windows, in the tray. Task Manager's Startup apps list can switch it off.", None),
    (
        "options.basics.autostart_note_macos",
        "Done: Glossa will open at login, in the menu bar. macOS may show a \"Background Items Added\" notice about it. If Glossa is switched off in System Settings > General > Login Items (Login Items & Extensions on macOS 15), it will not open: allow it there.",
        None,
    ),
    (
        "options.basics.autostart_allowed_macos",
        "Done: Glossa will open at login, in the menu bar, and macOS allows it: it is listed in System Settings > General > Login Items. macOS may show a \"Background Items Added\" notice about it.",
        None,
    ),
    (
        "options.basics.autostart_needs_approval",
        "Glossa is set to open at login, but macOS has it switched off. Allow it in System Settings > General > Login Items (Login Items & Extensions on macOS 15).",
        None,
    ),
    ("options.basics.autostart_note_linux", "Done: Glossa will start at login, in the tray (an entry in ~/.config/autostart). Your desktop's startup settings can switch it off.", None),
    ("options.basics.autostart_off", "Done: Glossa will no longer start at login.", None),
    ("options.basics.autostart_failed", "Could not change starting at login: %s", None),
    ("options.basics.open_login_items", "Open Login Items", None),
    ("options.permissions", "Permissions", None),
    ("options.permissions.intro", "macOS asks you before a program may read other programs' text or the screen. Grant these once; Glossa notices without a restart, except where noted.", None),
    ("options.permissions.accessibility", "Accessibility", None),
    ("options.permissions.accessibility_why", "Reads the selected text in other programs, and copies or replaces it. Needed by the translation hotkeys.", None),
    ("options.permissions.screen", "Screen Recording", None),
    ("options.permissions.screen_why", "Captures a region of the screen for text recognition. macOS applies it after Glossa restarts.", None),
    ("options.permissions.input", "Input Monitoring", None),
    ("options.permissions.input_why", "Lets mouse mode notice selections made with the mouse in other programs. Accessibility already covers it; allow this one if mouse mode does not react while Accessibility is off. Turn mouse mode off and on again afterwards.", None),
    ("options.permissions.granted", "Allowed", None),
    ("options.permissions.missing", "Not allowed", None),
    ("options.permissions.request", "Ask", None),
    ("options.permissions.open_settings", "Open System Settings", None),
    ("options.permissions.rebuild_note", "macOS ties a permission to the program's signature: after installing a new build, allow it again (remove the old Glossa entry from the list first).", None),
    // -- macOS: the app menu (shown while a full window is open) ----------------------------
    ("macmenu.settings", "Settings…", None),
    ("macmenu.hide", "Hide Glossa", None),
    ("macmenu.hide_others", "Hide Others", None),
    ("macmenu.show_all", "Show All", None),
    ("macmenu.quit", "Quit Glossa", None),
    ("macmenu.edit", "Edit", None),
    ("macmenu.redo", "Redo", None),
    ("macmenu.window", "Window", None),
    ("macmenu.minimize", "Minimize", None),
    ("macmenu.close", "Close Window", None),
    ("options.updates", "Updates", Some("w:18:caption")),
    ("options.updates.auto", "Automatically check for updates", Some("w:18:1188")),
    ("options.updates.check_now", "Check now", Some("w:18:1190")),
    ("options.updates.install", "Install", Some("w:18:1191")),
    (
        "options.updates.no_server",
        "Glossa has no update server yet; new versions are published as releases of the project.",
        None,
    ),
    // -- options: new pages ---------------------------------------------------------------
    ("options.ocr", "Text recognition", None),
    ("options.modules", "Modules", None),
    ("options.server", "Control API", None),
    // -- options: text recognition ----------------------------------------------------------
    ("options.ocr.provider", "Provider:", None),
    ("options.ocr.translate_after", "Translate the recognised text", None),
    ("options.ocr.keep_line_breaks", "Keep line breaks", None),
    ("options.ocr.watch_interval", "Watched region check, ms:", None),
    ("options.ocr.save_dir", "Save captured regions to:", None),
    ("options.ocr.after_select", "After selecting a region:", None),
    ("options.ocr.after_show", "Show the text and its translation", None),
    ("options.ocr.after_copy", "Copy the text to the clipboard", None),
    ("options.ocr.after_in_place", "Translate it in place", None),
    ("options.ocr.after_ask", "Ask, with a toolbar", None),
    ("options.ocr.translate_with", "Translate with:", None),
    ("options.ocr.translate_with_engine", "The OCR engine itself when it can (Google Lens, AI endpoints)", None),
    ("options.ocr.translate_with_translator", "The current translator", None),
    (
        "options.ocr.copy_joined",
        "Join lines when copying text (prose); off keeps the line breaks (code, lists)",
        None,
    ),
    ("options.ocr.lens_auto", "Auto", None),
    ("options.ocr.lens_native", "Built in", None),
    ("options.ocr.lens_daemon", "Daemon only", None),
    ("options.ocr.lens_cli", "CLI only", None),
    ("options.ocr.lens_endpoint", "Daemon URL:", None),
    ("options.ocr.lens_endpoint_hint", "discovered automatically", None),
    ("options.ocr.lens_cli_path", "lens_scan path:", None),
    ("options.ocr.lens_cli_hint", "found on PATH", None),
    (
        "options.ocr.lens_install",
        "Lens is built in and needs nothing installed. The daemon and CLI fields are only for chrome-lens-py, if you run it yourself.",
        None,
    ),
    ("options.ocr.windows_title", "Windows OCR", None),
    (
        "options.ocr.windows_note",
        "Built into Windows, offline. Reads the languages whose OCR pack is installed (Settings → Time & language → \
         Language & region → a language's options). With the language on auto it uses your Windows display languages.",
        None,
    ),
    ("options.ocr.oneocr_title", "OneOCR (Snipping Tool)", None),
    ("options.ocr.oneocr_folder", "Engine folder:", None),
    ("options.ocr.oneocr_folder_hint", "found in the Snipping Tool automatically", None),
    (
        "options.ocr.oneocr_note",
        "The Windows 11 Snipping Tool's engine: offline, many scripts at once, no language to choose. Glossa copies \
         its two libraries (about 55 MB) into its cache folder once; nothing is downloaded. A folder set here must \
         hold oneocr.dll, onnxruntime.dll and oneocr.onemodel.",
        None,
    ),
    ("options.ocr.default", "Default", None),
    ("options.ocr.auto_langs", "Languages when automatic:", None),
    ("options.ocr.tesseract_title", "Tesseract", None),
    ("options.ocr.tesseract_path", "Program:", None),
    ("options.ocr.tesseract_path_hint", "found on PATH or in the usual folders", None),
    ("options.ocr.tesseract_tessdata", "Language data folder:", None),
    ("options.ocr.tesseract_tessdata_hint", "Tesseract's own", None),
    ("options.ocr.tesseract_auto_hint", "eng (e.g. eng+rus)", None),
    ("options.ocr.tesseract_psm", "Page layout (--psm):", None),
    ("options.ocr.tesseract_oem", "Engine (--oem):", None),
    ("options.ocr.tesseract_extra", "More arguments:", None),
    ("options.ocr.psm_3", "3 — automatic", None),
    ("options.ocr.psm_4", "4 — one column", None),
    ("options.ocr.psm_6", "6 — one block of text", None),
    ("options.ocr.psm_7", "7 — one line", None),
    ("options.ocr.psm_11", "11 — scattered text", None),
    ("options.ocr.psm_13", "13 — one raw line", None),
    ("options.ocr.oem_0", "0 — legacy", None),
    ("options.ocr.oem_1", "1 — neural network", None),
    ("options.ocr.oem_2", "2 — both", None),
    (
        "options.ocr.tesseract_note",
        "Open-source, offline, on every system; reads the languages whose data is installed. Install it on Linux \
         from your distribution (tesseract-ocr and tesseract-ocr-eng, -rus…), on macOS with \
         brew install tesseract tesseract-lang, on Windows with the UB Mannheim installer.",
        None,
    ),
    ("options.ocr.vision_title", "Apple Vision", None),
    ("options.ocr.vision_accurate", "Accurate", None),
    ("options.ocr.vision_fast", "Fast", None),
    ("options.ocr.vision_correction", "Correct words with the language model", None),
    ("options.ocr.vision_auto_hint", "detected by macOS 13 and later", None),
    (
        "options.ocr.vision_note",
        "Built into macOS (Live Text), offline. The languages grow with the system: Russian and Ukrainian need \
         macOS 13 or later. Languages are given as Vision names such as en-US, ru-RU.",
        None,
    ),
    // -- options: modules ----------------------------------------------------------------------
    ("options.modules.runtimes", "Runtimes", None),
    ("options.modules.legacy_scripts", "Load classic QTranslate service scripts", None),
    ("options.modules.native", "Load native modules (they run with Glossa's full privileges)", None),
    ("options.modules.python", "Python interpreter:", None),
    ("options.modules.python_hint", "a path, or python3 / py -3", None),
    ("options.modules.python_auto", "Automatic", None),
    ("options.modules.python_system", "Installed on this computer", None),
    ("options.modules.python_managed", "Downloaded by Glossa", None),
    ("options.modules.python_path", "This interpreter…", None),
    ("options.modules.python_note", "Automatic uses an installed Python 3.8 or newer for modules without dependencies, and otherwise a Python that Glossa downloads once (about 25 MB) and keeps in its data folder.", None),
    ("options.python.title", "Python", None),
    ("options.python.hint", "This module needs a Python that Glossa downloads once, and shares with other modules.", None),
    ("options.python.requirements", "Packages: %s", None),
    ("options.python.ready", "Python and the module's packages are ready.", None),
    ("options.python.install", "Install", None),
    ("options.python.installing", "Installing the module's packages…", None),
    ("options.python.remove", "Delete its packages", None),
    ("options.modules.java", "Java (for Java modules):", None),
    ("options.modules.java_hint", "JAVA_HOME, then java from PATH", None),
    ("options.modules.call_timeout", "Call timeout, ms:", None),
    ("msg.needs_approval", "This module is waiting for your approval. Look at what it asks for in Options, Modules.", None),
    ("msg.assets_missing", "This service needs files that have not been downloaded yet. Download them in its settings (Options, Services).", None),
    ("assets.cancelled", "Download cancelled.", None),
    ("options.assets.title", "Files", None),
    ("options.assets.hint", "This module needs extra files (models, dictionaries). They are downloaded once and checked before use.", None),
    ("options.assets.ready", "ready", None),
    ("options.assets.missing", "not downloaded", None),
    ("options.assets.partial", "partly downloaded (%s)", None),
    ("options.assets.outdated", "out of date", None),
    ("options.assets.corrupt", "damaged, deleted", None),
    ("options.assets.download", "Download (%s)", None),
    ("options.assets.cancel", "Cancel", None),
    ("options.assets.busy", "Downloading or checking files…", None),
    ("options.assets.optional", "optional", None),
    ("options.assets.get", "Download", None),
    ("options.assets.verify", "Check files", None),
    ("options.assets.remove", "Delete files", None),
    ("options.assets.open_folder", "Open folder", None),
    ("options.assets.progress", "%s of %s", None),
    ("options.assets.verified", "All files are intact.", None),
    ("options.modules.auto_download", "Download module files (models, dictionaries) on first use without asking", None),
    ("options.modules.classic", "Classic QTranslate installation", None),
    ("options.modules.choose", "Choose…", None),
    ("options.modules.use_services", "services", None),
    ("options.modules.use_themes", "themes", None),
    ("options.modules.use_locales", "interface languages", None),
    ("options.modules.import", "Import its settings and history", None),
    ("options.modules.imported", "Imported: settings %s, %s history entries", None),
    ("options.modules.folders", "Module folders", None),
    ("options.modules.open_folder", "Open my modules folder", None),
    ("options.modules.install", "Install module…", None),
    ("options.modules.install_tip", "A module folder, or a pot-app .potext file", None),
    ("options.modules.potext_filter", "pot-app plugin", None),
    ("options.modules.installed", "Installed. Enable it on the Services page.", None),
    ("options.modules.not_a_module", "That folder has no module.toml, Service.js or info.json.", None),
    ("options.modules.reload", "Reload modules", None),
    ("options.modules.add_folder", "Add a folder…", None),
    ("options.modules.status", "Status: %s loaded, %s failed, %s skipped", None),
    ("options.modules.failed_item", "failed  %s: %s", None),
    ("options.modules.editor", "Module editor", None),
    (
        "options.modules.editor_hint",
        "Your own modules are edited where they are. Bundled and installed ones are copied to your modules folder first; the copy replaces the original.",
        None,
    ),
    ("options.modules.new", "New module:", None),
    ("options.modules.new_name", "Name", None),
    ("options.modules.create", "Create and edit", None),
    ("options.modules.template.translator", "Translator", None),
    ("options.modules.template.dictionary", "Dictionary", None),
    ("options.modules.template.ocr", "Text recognition", None),
    ("options.modules.edit", "Edit", None),
    ("options.modules.copy_edit", "Copy to my modules and edit", None),
    ("options.modules.mine", "yours", None),
    ("options.modules.failed_short", "did not load", None),
    ("options.modules.needs_approval", "needs approval", None),
    ("options.modules.trust.title", "Waiting for your approval", None),
    ("options.modules.trust.hint", "These modules were installed by hand. They do not run until you approve what they ask for.", None),
    ("options.modules.trust.new", "new module", None),
    ("options.modules.trust.widened", "asks for more than you approved", None),
    ("options.modules.trust.sandboxed", "Runs sandboxed: no access to your files or programs", None),
    ("options.modules.trust.program", "Runs a program on your computer with your rights, so it can read and change your files: %s", None),
    ("options.modules.trust.native", "Loads a native library into Glossa: full trust, it runs inside Glossa (%s)", None),
    ("options.modules.trust.no_network", "No network access", None),
    ("options.modules.trust.any_site", "Can reach any website", None),
    ("options.modules.trust.hosts", "Can reach: %s", None),
    ("options.modules.trust.asset", "Downloads %s, %s, from %s", None),
    ("options.modules.trust.asset_optional", "Downloads %s, %s, from %s, when asked for", None),
    ("options.modules.trust.packages", "Installs Python packages from %s: %s", None),
    ("options.modules.trust.actions", "Has actions (buttons): %s", None),
    ("options.modules.trust.approve", "Approve", None),
    ("options.modules.trust.edit_first", "Approve this module first, at the top of this page.", None),
    ("options.modules.trust.extra", "Also: %s", None),
    ("options.modules.trust.decline", "Decline", None),
    ("options.modules.trust.decline_tip", "Switch the module off. Switch it back on to be asked again.", None),
    // -- the module editor ---------------------------------------------------------------------
    ("editor.title", "Module editor", None),
    ("editor.title_named", "Module editor: %s", None),
    ("editor.save", "Save", None),
    ("editor.save_tip", "Save, check and reload the module (%s)", None),
    ("editor.check", "Check", None),
    ("editor.reload", "Reload", None),
    ("editor.test", "Test", None),
    ("editor.test_tip", "Run the cases in tests/ (F5)", None),
    ("editor.open_folder", "Open folder", None),
    ("editor.working", "Working…", None),
    ("editor.saved", "Saved.", None),
    ("editor.reloaded", "Reloaded %s.", None),
    ("editor.restart_needed", "Saved. Restart Glossa to load the new library.", None),
    ("editor.disabled", "%s is switched off in Options, so it was not loaded.", None),
    ("editor.not_reloaded", "Not reloaded: fix the errors first.", None),
    ("editor.files", "Files", None),
    ("editor.binary", "Not a text file", None),
    ("editor.close_tab", "Close", None),
    ("editor.line_col", "Line %s, column %s", None),
    (
        "editor.keys_hint",
        "Ctrl+S save   F5 test   Ctrl+F find   Ctrl+G go to line   Ctrl+/ comment",
        None,
    ),
    ("editor.find", "Find:", None),
    ("editor.next", "Next", None),
    ("editor.previous", "Previous", None),
    ("editor.go_to_line", "Go to line:", None),
    ("editor.matches", "%s found", None),
    ("editor.no_matches", "Not found", None),
    ("editor.problems", "Problems (%s)", None),
    ("editor.log", "Log", None),
    ("editor.tests", "Test results", None),
    ("editor.clear", "Clear", None),
    ("editor.no_problems", "No problems found.", None),
    ("editor.error", "error", None),
    ("editor.warning", "warning", None),
    ("editor.note", "note", None),
    (
        "editor.log_empty",
        "Nothing logged yet. What the module logs (host.log, console.*, a process's stderr) while it runs or is tested shows here.",
        None,
    ),
    ("editor.tests_none", "Press Test (F5) to run the cases in the tests folder.", None),
    ("editor.tests_summary", "%s passed, %s failed, %s skipped", None),
    ("editor.pass", "pass", None),
    ("editor.fail", "FAIL", None),
    ("editor.skip", "skipped", None),
    ("editor.preview", "Preview", None),
    ("editor.preview_settings", "settings page", None),
    ("editor.preview_note", "As the Services page will show it. Values typed here are not saved.", None),
    ("editor.preview_none", "This module has no settings.", None),
    ("editor.preview_panel", "panel", None),
    ("editor.preview_panel_note", "Its actions run in the module as last reloaded. Nothing typed here is kept.", None),
    ("editor.unsaved", "Some files have changes that are not saved.", None),
    ("editor.save_all", "Save all", None),
    ("editor.discard", "Discard and close", None),
    ("editor.keep_editing", "Keep editing", None),
    ("editor.read_only", "Read only: the file is larger than 512 KB", None),
    // -- options: control API ------------------------------------------------------------------
    ("options.server.enable", "Enable the local control API (restart to apply)", None),
    ("options.server.listen_on", "Listen on:", None),
    ("options.server.token", "Token:", None),
    ("options.server.regenerate", "Regenerate", None),
    ("options.server.show_token", "Show token", None),
    ("options.server.running", "Running.", None),
    ("options.server.not_running", "Not running: %s", None),
    ("options.server.stopped", "Not running. Turn it on above and restart Glossa to start it.", None),
    (
        "options.server.bind_hint",
        "Bind any of these to a key in your desktop or launcher (essential on Wayland):",
        None,
    ),
    ("options.server.cmd_popup", "Translate the selection in the popup", None),
    ("options.server.cmd_ocr", "Recognise a screen region", None),
    ("options.server.cmd_ocr_translate", "Recognise and translate", None),
    ("options.server.cmd_main", "Show the main window", None),
    // -- main menu (the "⋮" button) -------------------------------------------------------
    ("menu.spell_checking", "Spell checking", Some("m:1:10")),
    ("menu.instant_translation", "Instant translation", Some("m:1:20")),
    ("menu.back_translation", "Backward translation", Some("m:1:30")),
    ("menu.extended", "Extended", Some("m:1:40")),
    ("menu.read_phonetically", "Read phonetically", Some("m:1:50")),
    ("menu.clear_on_drop", "Clear input on Drag & Drop", Some("m:1:60")),
    ("menu.auto_cleanup", "Auto-cleanup of translation", Some("m:1:61")),
    ("menu.save_on_exit", "Save contents on exit", Some("m:1:62")),
    ("menu.show_top_pane", "Show top pane", Some("m:1:65")),
    ("menu.show_middle_pane", "Show middle pane", Some("m:1:70")),
    ("menu.show_services_pane", "Show services pane", Some("m:1:80")),
    ("menu.tray_on_minimize", "Minimize to system tray on minimize", Some("m:1:81")),
    ("menu.tray_on_close", "Minimize to system tray on close", Some("m:1:82")),
    ("menu.on_startup", "On startup", Some("m:1:85")),
    ("menu.startup_show", "Show window", Some("m:1:86")),
    ("menu.startup_tray", "Minimize to tray", Some("m:1:87")),
    ("menu.startup_restore", "Restore previous state", Some("m:1:88")),
    ("menu.dictionary", "Dictionary", Some("m:1:90")),
    ("menu.history", "History", Some("m:1:100")),
    ("menu.keyboard", "Keyboard", Some("m:1:110")),
    ("menu.options", "Options", Some("m:1:120")),
    ("menu.about", "About Glossa", Some("m:1:125")),
    ("menu.help", "Help", Some("m:1:130")),
    ("menu.support", "Support the project", None),
    ("menu.exit", "Exit", Some("m:1:140")),
    ("menu.stacked_results", "Show all services at once", None),
    // -- text field context menu -------------------------------------------------------------
    ("edit.undo", "Undo", Some("m:2:10")),
    ("edit.redo", "Redo", None),
    ("edit.cut", "Cut", Some("m:2:20")),
    ("edit.cut_all", "Cut all", Some("m:2:30")),
    ("edit.copy", "Copy", Some("m:2:40")),
    ("edit.copy_all", "Copy all", Some("m:2:50")),
    ("edit.paste", "Paste", Some("m:2:60")),
    ("edit.select_all", "Select all", Some("m:2:70")),
    ("edit.translate", "Translate", Some("m:2:75")),
    ("edit.dictionary", "Dictionary search for", Some("m:2:80")),
    ("edit.listen", "Listen", Some("m:2:90")),
    ("edit.no_suggestions", "No suggestions", None),
    ("edit.ignore_word", "Ignore", None),
    ("edit.add_to_dictionary", "Add to dictionary", None),
    // -- history item menu ---------------------------------------------------------------------
    ("history.open", "Open", Some("m:3:10")),
    ("history.copy_text", "Copy text", Some("m:3:15")),
    ("history.copy_translation", "Copy translation", Some("m:3:20")),
    ("history.delete", "Delete", Some("m:3:30")),
    ("history.listen", "Listen", Some("m:3:40")),
    // -- language menu ---------------------------------------------------------------------------
    ("lang.reset", "Reset", Some("m:4:10")),

    ("lang.always_detect", "Always detect language", Some("m:4:30")),
    // -- tray menu --------------------------------------------------------------------------------
    ("tray.text_recognition", "Text recognition", Some("m:5:5")),
    ("tray.dictionary", "Dictionary", Some("m:5:10")),
    ("tray.history", "History", Some("m:5:20")),
    ("tray.keyboard", "Keyboard", Some("m:5:30")),
    ("tray.hotkeys", "Enable global hotkeys", Some("m:5:36")),
    ("tray.mouse_mode", "Mouse mode", Some("m:5:40")),
    ("tray.mouse_enabled", "Enabled", Some("m:5:50")),
    ("tray.mouse_icon", "Show icon", Some("m:5:60")),
    ("tray.mouse_translation", "Show translation", Some("m:5:70")),
    ("tray.mouse_translation_read", "Show translation and read", Some("m:5:71")),
    ("tray.options", "Options", Some("m:5:80")),
    ("tray.about", "About Glossa", Some("m:5:90")),
    ("tray.support", "Support the project", None),
    ("tray.exit", "Exit", Some("m:5:100")),
    ("tray.show", "Show Glossa", None),
    // -- dictionary article menu --------------------------------------------------------------------
    ("article.copy", "Copy", Some("m:6:20")),
    ("article.translate", "Translate", Some("m:6:40")),
    ("article.search", "Search", Some("m:6:50")),
    ("article.listen", "Listen", Some("m:6:60")),
    ("history.show_full", "Show Full History", Some("m:7:10")),
    ("dict.show_info", "Show info", Some("m:8:10")),
    ("dict.remove", "Remove dictionary", Some("m:8:20")),
    ("dict.clear_history", "Clear history", Some("m:9:10")),
    ("dict.show_services", "Show services pane", Some("m:9:20")),
    ("dict.exact_search", "Exact search", Some("m:9:30")),
    ("dict.options", "Options", Some("m:9:40")),
    // -- OCR window ---------------------------------------------------------------------------------
    ("ocr.language", "OCR language", Some("m:10:110")),
    ("ocr.translate", "Translate", Some("m:10:120")),
    ("ocr.copy_image", "Copy image", Some("m:10:130")),
    ("ocr.copy_text", "Copy text", Some("m:10:140")),
    ("ocr.save_image", "Save image", Some("m:10:150")),
    ("ocr.edit_image", "Edit image", Some("m:10:160")),
    ("ocr.select_all", "Select all", Some("m:10:170")),
    ("ocr.clear_selection", "Clear selection", Some("m:10:180")),
    ("ocr.close", "Close", Some("m:10:190")),
    ("ocr.hint", "Drag to select an area. Esc or a right click cancels.", None),
    ("ocr.menu.recognize", "Recognise text", None),
    ("ocr.menu.recognize_translate", "Recognise and translate", None),
    ("ocr.menu.copy", "Copy text from the screen", None),
    ("ocr.menu.in_place", "Translate the screen in place", None),
    ("ocr.menu.screen_in_place", "Translate the whole screen in place", None),
    ("ocr.menu.watch", "Watch a region", None),
    ("ocr.menu.repeat", "Repeat the last region", None),
    ("ocr.menu.more", "More recognition", None),
    ("ocr.copied_chars", "Copied %s characters", None),
    ("ocr.in_place", "Translate in place", None),
    ("ocr.cancel", "Cancel", None),
    ("ocr.toolbar_keys", "Enter: %s   C: copy text   T: translate   I: in place   Esc: cancel", None),
    ("ocr.original", "Original", None),
    ("ocr.translation", "Translation", None),
    ("ocr.in_place_keys", "Space: original   C: copy translation   Esc: close", None),
    ("ocr.nothing_to_paint", "No text with a position to translate in place", None),
    ("ocr.result.title", "Recognised text", None),
    ("ocr.result.larger", "Larger picture", None),
    ("ocr.result.smaller", "Smaller picture", None),
    ("ocr.result.boxes", "Show where text was found", None),
    ("ocr.result.counts", "%s characters · %s lines", None),
    ("ocr.result.failed", "Nothing was recognised", None),
    ("ocr.result.select_again", "Select again", None),
    ("ocr.result.translator", "Translator", None),
    ("ocr.result.lens_note", "Google Lens translates the picture itself, without a second request", None),
    ("ocr.result.no_lens_translation", "Google Lens did not translate this picture", None),
    ("ocr.result.translating", "Translating…", None),
    ("ocr.result.translate_hint", "Not translated yet. Translate: %s", None),
    ("ocr.result.stale", "The text has changed since it was translated", None),
    ("ocr.result.to_main", "To main window", None),
    ("ocr.result.read_aloud", "Read aloud", None),
    ("ocr.result.no_clipboard", "The clipboard is not available", None),
    // -- start-up warnings and platform limitations (glossa-cli doctor) --
    ("startup.no_keychain", "The settings keep API keys in the system keychain, but no keychain answered. Services that need a key will not work until it does; Options → Basics can keep them in the settings file instead.", None),
    ("startup.config_broken", "config.toml could not be read, so Glossa started with the default settings. The file was moved to %s. The problem: %s", None),
    ("limitation.windows.elevated", "Programs running as administrator cannot be read from or typed into unless Glossa also runs as administrator.", None),
    ("limitation.macos.accessibility", "Reading the selection and pasting translations need the Accessibility permission (System Settings → Privacy & Security → Accessibility).", None),
    ("limitation.macos.screen_recording", "Screen text recognition needs the Screen Recording permission.", None),
    ("limitation.x11.primary", "The selection is read from the PRIMARY selection: the text highlighted most recently, in any window.", None),
    ("limitation.wayland.hotkeys", "This compositor offers no global shortcut portal, so Glossa cannot register hotkeys. Bind a shortcut in its settings that runs Glossa's D-Bus action (gdbus call --session --dest io.github.bropines.Glossa … Action show_popup '') or calls the local HTTP API with curl.", None),
    ("limitation.wayland.hotkeys_portal", "Global hotkeys go through the desktop's shortcut portal: the desktop asks once to confirm them, and you change them in its own settings. Without the portal (GNOME before 48), bind a desktop shortcut to Glossa's D-Bus action or HTTP API instead.", None),
    ("limitation.wayland.capture", "Screen capture goes through the desktop portal (or grim on wlroots compositors); the first time, the desktop may ask whether Glossa may take screenshots.", None),
    ("limitation.wayland.typing", "Wayland does not allow typing into other programs: \"replace selection\" leaves the translation on the clipboard for you to paste.", None),
    ("limitation.wayland.typing_portal", "Copying from and pasting into other programs goes through the desktop's remote desktop portal: the first time, the desktop asks whether Glossa may use the keyboard.", None),
    ("limitation.wayland.no_primary", "%s does not let Glossa read the highlighted text; copy it with Ctrl+C first and Glossa will use the clipboard.", None),
    ("limitation.wayland.no_primary_portal", "%s does not let Glossa read the highlighted text directly; Glossa copies it with Ctrl+C through the desktop's remote desktop portal.", None),
    ("limitation.wayland.no_primary_unknown", "This compositor does not let Glossa read the highlighted text; copy it with Ctrl+C first and Glossa will use the clipboard.", None),
    ("limitation.wayland.popup_position", "The popup cannot be placed at the text cursor; it opens near the mouse or in the centre of the screen.", None),
    ("limitation.no_display", "No graphical session was found (neither WAYLAND_DISPLAY nor DISPLAY is set): clipboard, hotkeys and screen capture are unavailable.", None),
];

/// Keys asked for that no locale, English included, has. A lookup that
/// misses shows the bare key; the screenshot smoke test reads this list to
/// fail on such a string even where no one would spot it in a picture.
static MISSING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn note_missing(key: &str) {
    let mut missing = MISSING.lock().unwrap_or_else(|e| e.into_inner());
    if !missing.contains(key) {
        missing.insert(key.to_owned());
    }
}

/// Every key looked up so far that had no text in any language, sorted.
pub fn missing_keys() -> Vec<String> {
    MISSING.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSIC_RU: &str = r#"{
        "Author": "someone",
        "LanguageName": "Russian",
        "LanguageNativeName": "Русский",
        "LanguageCode": "ru",
        "Strings": [ [172, "Перевести (Ctrl+Enter)"], [181, "Доступна новая версия (%s)"] ],
        "Windows": [
            { "Id": 1, "Caption": "QTranslate", "Controls": [ [1004, "Перевести"] ] },
            { "Id": 16, "Caption": "Горячие клавиши", "Controls": [
                [1118, "Использовать горячие клавиши"],
                [1165, [ [0, "Показать главное окно"], [1, "Показать всплывающее окно"] ]]
            ]},
            { "Id": 5, "Caption": "Словарь", "Controls": [], "Strings": [ [10, "Нет результатов для '%s'."] ] },
        ],
        "Menus": [ { "Id": 1, "Items": [ [60, "Очищать при Drag && Drop"], [120, "&Настройки"] ] } ]
    }"#;

    #[test]
    fn english_is_built_in_and_missing_keys_show_themselves() {
        let en = Locale::english();
        assert_eq!(en.get("main.translate"), "Translate");
        assert_eq!(en.get("no.such.key"), "no.such.key");
    }

    #[test]
    fn classic_locale_files_translate_this_interface() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.code, "ru");
        assert_eq!(ru.native_name, "Русский");
        assert_eq!(ru.get("main.translate"), "Перевести");
        assert_eq!(ru.get("msg.translate_tip"), "Перевести (Ctrl+Enter)");
        assert_eq!(ru.get("options.hotkeys"), "Горячие клавиши");
        assert_eq!(ru.get("options.hotkeys.enable"), "Использовать горячие клавиши");
        // Deliberately not mapped: Glossa's own wording wins.
        assert_eq!(ru.get("action.show_popup"), "Translate selection");
        assert_eq!(ru.get("dict.title"), "Словарь");
        // Keys the file lacks fall back to English.
        assert_eq!(ru.get("history.title"), "History");
    }

    #[test]
    fn mnemonics_are_stripped_and_double_ampersands_kept() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.get("menu.options"), "Настройки");
        assert_eq!(ru.get("menu.clear_on_drop"), "Очищать при Drag & Drop");
    }

    #[test]
    fn placeholders_format() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        assert_eq!(ru.fmt("msg.update_available", "2.0"), "Доступна новая версия (2.0)");
        assert_eq!(ru.fmt("dict.no_results", "xyz"), "Нет результатов для 'xyz'.");
    }

    #[test]
    fn native_round_trip() {
        let ru = Locale::parse(CLASSIC_RU).unwrap();
        let native = ru.to_native_json();
        let back = Locale::parse(&native).unwrap();
        assert_eq!(back.code, "ru");
        assert_eq!(back.get("main.translate"), "Перевести");
        assert_eq!(back.coverage(), ru.coverage());
    }

    #[test]
    fn keys_and_addresses_are_unique() {
        let mut keys: Vec<&str> = STRINGS.iter().map(|(k, _, _)| *k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate key in STRINGS");

        let mut addrs: Vec<&str> = STRINGS.iter().filter_map(|(_, _, a)| *a).collect();
        addrs.sort_unstable();
        let n = addrs.len();
        addrs.dedup();
        assert_eq!(addrs.len(), n, "two keys claim the same classic address");
    }

    #[test]
    fn every_action_has_a_label_key() {
        for a in crate::hotkey::Action::ALL {
            let key = format!("action.{}", a.id());
            assert!(english(&key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn every_capability_has_a_label_key() {
        for name in crate::Capabilities(u32::MAX).names() {
            let key = format!("capability.{name}");
            assert!(english(&key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn language_names_come_from_the_locale_or_fall_back_to_english() {
        use crate::LangId;
        let ru = Locale::builtin("ru").unwrap();
        assert_eq!(ru.language_name(LangId::ENGLISH), "Английский");
        assert_eq!(Locale::english().language_name(LangId::ENGLISH), "English");
        // A user's pack that names one language keeps the rest from below.
        let mut merged = ru.clone();
        merged.merge(Locale::parse(r#"{"code":"ru","languages":{"en":"Англ."}}"#).unwrap());
        assert_eq!(merged.language_name(LangId::ENGLISH), "Англ.");
        assert_eq!(merged.language_name(LangId::RUSSIAN), "Русский");
        // They survive a round trip through the native format.
        let back = Locale::parse(&ru.to_native_json()).unwrap();
        assert_eq!(back.language_name(LangId::RUSSIAN), "Русский");
    }

    /// Every built-in pack names every language of the table, and names
    /// only languages that exist.
    #[test]
    fn built_in_packs_name_every_language() {
        use crate::LangId;
        let tags: std::collections::HashSet<&str> = LangId::all().map(|l| l.bcp47()).collect();
        for pack in BUILTIN_LOCALES {
            let l = Locale::parse(pack.source).unwrap();
            let missing: Vec<&str> = LangId::all().filter(|id| !l.names_language(*id)).map(|id| id.bcp47()).collect();
            assert!(missing.is_empty(), "{}: no name for {missing:?}", pack.code);
            let unknown: Vec<&String> = l.languages.keys().filter(|k| !tags.contains(k.as_str())).collect();
            assert!(unknown.is_empty(), "{}: names for unknown tags {unknown:?}", pack.code);
            assert!(l.languages.values().all(|n| !n.trim().is_empty()), "{}: a blank language name", pack.code);
        }
    }

    #[test]
    fn several_placeholders_fill_in_order() {
        let en = Locale::english();
        assert_eq!(en.fmt_args("options.ai.test_ok", &["12", "Hallo"]), "OK, 12 ms: Hallo");
        // Too few arguments leave the rest visible; an argument holding a
        // placeholder is not expanded again.
        assert_eq!(en.fmt_args("options.ai.test_ok", &["%s"]), "OK, %s ms: %s");
        assert_eq!(en.fmt_args("msg.ms", &["5", "extra"]), "5 ms");
    }

    #[test]
    fn merging_lays_strings_over_and_keeps_the_rest() {
        let mut ru = Locale::builtin("ru").unwrap();
        let built_in = ru.get("options.modules").to_owned();
        ru.merge(Locale::parse(CLASSIC_RU).unwrap());
        assert_eq!(ru.get("options.hotkeys"), "Горячие клавиши", "the classic file wins");
        assert_eq!(ru.get("main.translate"), "Перевести");
        assert_eq!(ru.get("options.modules"), built_in, "what the classic file lacks stays from the pack");
        assert_eq!(ru.author, "someone");
    }

    /// Every compiled-in pack parses, translates every key and nothing else,
    /// and keeps the English placeholders.
    #[test]
    fn built_in_packs_cover_every_key() {
        let known: std::collections::HashSet<&str> = STRINGS.iter().map(|(k, _, _)| *k).collect();
        for pack in BUILTIN_LOCALES {
            let l = Locale::parse(pack.source).unwrap_or_else(|e| panic!("{}: {e}", pack.code));
            assert_eq!(l.code, pack.code);
            assert_eq!(l.native_name, pack.native_name);
            let missing: Vec<&str> = STRINGS.iter().map(|(k, _, _)| *k).filter(|k| !l.has(k)).collect();
            assert!(missing.is_empty(), "{}: untranslated keys {missing:?}", pack.code);
            let mut stale: Vec<&String> = l.strings.keys().filter(|k| !known.contains(k.as_str())).collect();
            stale.sort();
            assert!(stale.is_empty(), "{}: keys that no longer exist {stale:?}", pack.code);
            for (key, en, _) in STRINGS {
                let text = l.get(key);
                assert!(!text.trim().is_empty(), "{}: {key} is blank", pack.code);
                assert_eq!(
                    text.matches("%s").count(),
                    en.matches("%s").count(),
                    "{}: {key} has a different number of %s than English",
                    pack.code
                );
            }
        }
        assert!(Locale::builtin("ru-RU").is_some(), "a regional code finds its base pack");
        assert!(Locale::builtin("xx").is_none());
    }

    #[test]
    fn a_key_no_language_has_is_recorded_as_missing() {
        // The list is process-wide and other tests look keys up in
        // parallel, so these keys are unique to this test.
        let key = "test.missing_keys.nowhere";
        assert_eq!(Locale::english().get(key), key, "shown bare");
        let ru = Locale::builtin("ru").unwrap();
        assert_eq!(ru.get("test.missing_keys.nowhere_ru"), "test.missing_keys.nowhere_ru");
        let missing = missing_keys();
        assert!(missing.iter().any(|k| k == key), "{missing:?}");
        assert!(missing.iter().any(|k| k == "test.missing_keys.nowhere_ru"), "missed through a pack too");
        // Asked twice, listed once.
        Locale::english().get(key);
        assert_eq!(missing_keys().iter().filter(|k| *k == key).count(), 1);
        // Through the formatting helpers as well.
        Locale::english().fmt("test.missing_keys.fmt", "x");
        Locale::english().fmt_args("test.missing_keys.fmt_args", &["x"]);
        let missing = missing_keys();
        assert!(missing.iter().any(|k| k == "test.missing_keys.fmt"));
        assert!(missing.iter().any(|k| k == "test.missing_keys.fmt_args"));
        let mut sorted = missing.clone();
        sorted.sort();
        assert_eq!(missing, sorted, "sorted");
    }

    #[test]
    fn keys_english_or_the_locale_has_are_not_recorded() {
        Locale::english().get("main.translate");
        // A key only a user's pack has (no English text) is found, so it is
        // not missing either.
        let own = Locale::parse(r#"{"code":"xx","strings":{"test.missing_keys.own":"mine"}}"#).unwrap();
        assert_eq!(own.get("test.missing_keys.own"), "mine");
        let missing = missing_keys();
        assert!(!missing.iter().any(|k| k == "main.translate"), "{missing:?}");
        assert!(!missing.iter().any(|k| k == "test.missing_keys.own"));
        // `has` is a question, not a lookup to record.
        assert!(!Locale::english().has("test.missing_keys.has"));
        assert!(!missing_keys().iter().any(|k| k == "test.missing_keys.has"));
    }

    #[test]
    fn fmt_fills_only_the_first_placeholder_and_leaves_text_without_one_alone() {
        let l = Locale::parse(r#"{"code":"xx","strings":{"a":"%s and %s","b":"plain","c":"100%"}}"#).unwrap();
        assert_eq!(l.fmt("a", "x"), "x and %s");
        assert_eq!(l.fmt("b", "x"), "plain");
        assert_eq!(l.fmt("c", "x"), "100%", "a lone percent sign is not a placeholder");
        // The argument is inserted verbatim, never expanded.
        assert_eq!(l.fmt("a", "%s"), "%s and %s");
        assert_eq!(l.fmt_args("a", &[]), "%s and %s", "no arguments: placeholders stay visible");
        assert_eq!(l.fmt_args("b", &["x"]), "plain");
        assert_eq!(l.fmt_args("a", &["", "y"]), " and y", "an empty argument still uses up its slot");
    }

    #[test]
    fn placeholders_at_the_edges_are_filled() {
        let l = Locale::parse(r#"{"code":"xx","strings":{"e":"%s%s","s":"%s!"}}"#).unwrap();
        assert_eq!(l.fmt_args("e", &["a", "b"]), "ab");
        assert_eq!(l.fmt("s", "hi"), "hi!");
    }

    #[test]
    fn merging_an_unnamed_pack_keeps_the_names_below() {
        let mut ru = Locale::builtin("ru").unwrap();
        ru.merge(Locale::parse(r#"{"code":"ru","strings":{"main.translate":"Перевод!"}}"#).unwrap());
        assert_eq!(ru.native_name, "Русский", "an empty name does not blank it");
        assert_eq!(ru.get("main.translate"), "Перевод!");
        // The last pack laid on wins.
        ru.merge(Locale::parse(r#"{"code":"ru","name":"Mine","strings":{"main.translate":"Второй"}}"#).unwrap());
        assert_eq!(ru.get("main.translate"), "Второй");
        assert_eq!(ru.name, "Mine");
        assert_eq!(ru.code, "ru", "the code is not replaced by a merge");
    }

    #[test]
    fn classic_files_are_recognised_by_windows_alone_and_name_falls_back() {
        let l = Locale::parse(
            r#"{"LanguageName":"Deutsch","LanguageCode":"de",
                "Windows":[{"Id":1,"Caption":"x","Controls":[[1004,"Übersetzen"]]}]}"#,
        )
        .unwrap();
        assert!(l.classic);
        assert_eq!(l.native_name, "Deutsch", "no native name: the plain one");
        assert_eq!(l.get("main.translate"), "Übersetzen");
        // A native file is not classic.
        assert!(!Locale::parse(r#"{"code":"de"}"#).unwrap().classic);
    }

    #[test]
    fn empty_classic_strings_fall_back_to_english() {
        let l = Locale::parse(r#"{"LanguageCode":"xx","Windows":[{"Id":1,"Controls":[[1004,""]]}]}"#).unwrap();
        assert!(!l.has("main.translate"), "an empty translation is no translation");
        assert_eq!(l.get("main.translate"), "Translate");
    }

    #[test]
    fn locale_files_tolerate_comments_and_report_real_errors() {
        let l = Locale::parse(
            "// a user's notes\n{ \"code\": \"xx\", /* inline */ \"strings\": { \"main.translate\": \"T\" } }",
        )
        .unwrap();
        assert_eq!(l.get("main.translate"), "T");
        assert!(Locale::parse("{ not json").is_err());
        assert!(Locale::parse(r#"{"name":"no code"}"#).is_err(), "a native file must name its code");
        assert!(Locale::parse(r#"{"code":"xx","strings":{"a":1}}"#).is_err(), "strings are strings");
    }

    #[test]
    fn classic_addresses_cover_every_kind_of_entry() {
        let flat = flatten_classic(&serde_json::from_str::<Value>(&jsonc::strip(CLASSIC_RU)).unwrap());
        assert_eq!(flat["s:172"], "Перевести (Ctrl+Enter)");
        assert_eq!(flat["w:1:caption"], "QTranslate");
        assert_eq!(flat["w:1:1004"], "Перевести");
        assert_eq!(flat["w:16:1165:1"], "Показать всплывающее окно");
        assert_eq!(flat["w:5:s:10"], "Нет результатов для '%s'.");
        assert_eq!(flat["m:1:120"], "&Настройки", "flattening keeps the mnemonic; loading strips it");
        // Malformed entries are skipped, not fatal.
        let odd = serde_json::json!({
            "Strings": [[1], ["x", "y"], [2, 3], [4, "ok"]],
            "Windows": [{"Caption": "no id"}, {"Id": 9, "Controls": [[1, [[0]]]]}],
            "Menus": [{"Items": [[1, "no id"]]}]
        });
        let flat = flatten_classic(&odd);
        assert_eq!(flat.len(), 1, "{flat:?}");
        assert_eq!(flat["s:4"], "ok");
    }

    #[test]
    fn mnemonic_stripping_handles_the_edges() {
        assert_eq!(unmnemonic("&File"), "File");
        assert_eq!(unmnemonic("Save &As…"), "Save As…");
        assert_eq!(unmnemonic("trailing&"), "trailing");
        assert_eq!(unmnemonic("&&&x"), "&x");
        assert_eq!(unmnemonic("A && B && C"), "A & B & C");
    }

    #[test]
    fn built_in_packs_are_found_by_any_spelling_of_their_code() {
        for code in ["ru", "RU", "ru_RU", "ru-Latn-RU"] {
            assert_eq!(Locale::builtin(code).map(|l| l.code), Some("ru".to_owned()), "{code}");
        }
        assert!(Locale::builtin("").is_none());
        assert!(Locale::builtin("en").is_none(), "English needs no pack");
    }

    #[test]
    fn coverage_counts_only_known_keys() {
        let (have, total) = Locale::english().coverage();
        assert_eq!(have, 0, "English is the table, not a pack");
        assert_eq!(total, STRINGS.len());
        let l = Locale::parse(r#"{"code":"xx","strings":{"main.translate":"T","not.a.key":"N"}}"#).unwrap();
        assert_eq!(l.coverage().0, 1);
    }

    #[test]
    fn validation_skips_broken_placeholders_and_warns_about_unknown_keys() {
        let mut l = Locale::parse(
            r#"{ "code": "xx", "strings": {
                "main.translate": "Tr",
                "msg.ms": "ms only",
                "options.ai.test_ok": "OK %s %s",
                "no.such.key": "?",
                "main.source_placeholder": ""
            }, "languages": { "de": "Deu", "zz-Fake": "?" } }"#,
        )
        .unwrap();
        let issues = l.validate();
        let kinds: Vec<(&str, &IssueKind)> = issues.iter().map(|i| (i.key.as_str(), &i.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("msg.ms", &IssueKind::Placeholders { expected: 1, found: 0 }),
                ("no.such.key", &IssueKind::UnknownKey),
                ("zz-Fake", &IssueKind::UnknownLanguage),
            ]
        );
        assert_eq!(issues[0].severity(), Severity::Error);
        assert_eq!(issues[1].severity(), Severity::Warning);
        assert_eq!(l.get("main.translate"), "Tr");
        assert_eq!(l.get("options.ai.test_ok"), "OK %s %s", "matching placeholders stay");
        assert_eq!(l.get("msg.ms"), "%s ms", "a skipped string falls back to English");
        assert_eq!(l.get("main.source_placeholder"), english("main.source_placeholder").unwrap(), "empty is a gap");
        assert_eq!(
            issues[0].message(&Locale::english()),
            "msg.ms: 0 placeholders where English has 1; the string is skipped"
        );
        let ru = Locale::builtin("ru").unwrap();
        assert_eq!(issues[1].message(&ru), "no.such.key: неизвестный ключ, пропущен");
    }

    #[test]
    fn headers_read_without_the_strings() {
        let h = Locale::parse_header(r#"{ "code": "de", "native_name": "Deutsch", "strings": { "a": "b" } }"#).unwrap();
        assert_eq!(
            h,
            LocaleHeader { code: "de".into(), name: "".into(), native_name: "Deutsch".into(), classic: false }
        );
        let c = Locale::parse_header(CLASSIC_RU).unwrap();
        assert!(c.classic && c.code == "ru" && c.native_name == "Русский");
        assert!(Locale::parse_header(r#"{ "strings": {} }"#).is_err(), "a native file needs its code");
        assert!(Locale::parse_header("not json").is_err());
    }

    #[test]
    fn the_template_lists_every_key_and_loads_back() {
        let t = Locale::template(None);
        for (key, _, _) in STRINGS {
            assert!(t.contains(&format!("\"{key}\": \"\"")), "{key} missing from the template");
        }
        assert!(t.contains("// Interface language:  [classic w:10:1070]"), "English and the classic address");
        assert!(t.contains("\"de\": \"\", // German"));
        // A blank template is a valid, empty pack.
        let mut blank = Locale::parse(&t.replacen("\"code\": \"\"", "\"code\": \"xx\"", 1)).unwrap();
        assert!(blank.validate().is_empty());
        assert_eq!(blank.coverage().0, 0);
        assert_eq!(blank.get("main.translate"), "Translate");
        // Starting from a language keeps its strings, so a translator can
        // update a pack from it.
        let ru = Locale::builtin("ru").unwrap();
        let mut back = Locale::parse(&Locale::template(Some(&ru))).unwrap();
        assert!(back.validate().is_empty());
        assert_eq!(back.code, "ru");
        assert_eq!(back.coverage(), ru.coverage());
        assert_eq!(back.get("options.server"), ru.get("options.server"));
        assert_eq!(
            back.language_name(crate::LangId::from_code("de").unwrap()),
            ru.language_name(crate::LangId::from_code("de").unwrap())
        );
    }
}
