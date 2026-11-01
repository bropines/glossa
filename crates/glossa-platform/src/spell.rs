//! Spell checking with the system's own checker.
//!
//! | | Windows | macOS | Linux and the BSDs |
//! |---|---|---|---|
//! | checker | Spell Checking API (`ISpellChecker`, Windows 8+) | `NSSpellChecker` | Hunspell dictionaries, read by `spellbook` (pure Rust) |
//! | languages | the installed language packs | the system's dictionaries | `*.dic` + `*.aff` in `/usr/share/hunspell`, `/usr/share/myspell`, `~/.local/share/hunspell`, `$DICPATH` |
//! | add a word | the user's Windows dictionary | the user's macOS dictionary | `~/.hunspell_<lang>` (Hunspell's personal dictionary) |
//! | thread | any | main thread only | any |
//!
//! One file per system: `spell/win.rs`, `spell/mac.rs`, `spell/linux.rs`.
//! A language without a dictionary is not an error: nothing is reported
//! for it. Checking can take tens of milliseconds the first time a language
//! loads, so callers should run it off the UI thread unless
//! [`MAIN_THREAD_ONLY`] says otherwise.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use glossa_core::LangId;

#[cfg(windows)]
#[path = "spell/win.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "spell/mac.rs"]
mod imp;
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "spell/linux.rs"]
mod imp;

/// A word the checker does not know.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Misspelling {
    /// Where the word is, as a byte range of the checked text.
    pub range: Range<usize>,
    pub word: String,
}

/// Whether the checker must be called on the main thread (macOS). Elsewhere
/// any thread will do, and a worker thread is the right one.
pub const MAIN_THREAD_ONLY: bool = imp::MAIN_THREAD_ONLY;

/// Whether [`add`] can put a word in the user's dictionary.
pub const CAN_ADD: bool = imp::CAN_ADD;

/// At most this many suggestions are given for a word.
pub const MAX_SUGGESTIONS: usize = 8;

/// Texts longer than this (in bytes) are not checked.
pub const MAX_TEXT: usize = 100_000;

/// The languages there are dictionaries for, as BCP 47 tags (`en-US`,
/// `ru-RU`, `de`). Read once per process.
pub fn languages() -> Vec<String> {
    static LANGS: OnceLock<Vec<String>> = OnceLock::new();
    if MAIN_THREAD_ONLY {
        // The macOS list can only be read on the main thread; do not cache
        // an empty answer from elsewhere.
        if let Some(l) = LANGS.get() {
            return l.clone();
        }
        let l = imp::languages();
        if !l.is_empty() {
            let _ = LANGS.set(l.clone());
        }
        return l;
    }
    LANGS.get_or_init(imp::languages).clone()
}

/// The dictionary to check `lang` with, if there is one.
pub fn locale_for(lang: LangId) -> Option<String> {
    if !lang.is_language() || lang.is_scriptio_continua() {
        return None;
    }
    static PICKED: OnceLock<Mutex<HashMap<LangId, Option<String>>>> = OnceLock::new();
    let picked = PICKED.get_or_init(Default::default);
    if let Some(p) = picked.lock().ok().and_then(|m| m.get(&lang).cloned()) {
        return p;
    }
    let available = languages();
    if available.is_empty() {
        // Nothing to remember: on macOS this may be the wrong thread.
        return None;
    }
    let p = pick_locale(lang.bcp47(), &available);
    if let Ok(mut m) = picked.lock() {
        m.insert(lang, p.clone());
    }
    p
}

/// The words in `text` that the dictionary for `lang` does not know, in
/// order. Empty when there is no dictionary for `lang`.
pub fn check(text: &str, lang: LangId) -> Vec<Misspelling> {
    if text.trim().is_empty() || text.len() > MAX_TEXT {
        return Vec::new();
    }
    let Some(tag) = locale_for(lang) else { return Vec::new() };
    let mut found = imp::check(text, &tag);
    found.retain(|m| text.get(m.range.clone()) == Some(m.word.as_str()) && !m.word.trim().is_empty());
    found.sort_by_key(|m| m.range.start);
    found
}

/// What `word` might have been meant to be, best first.
pub fn suggest(word: &str, lang: LangId) -> Vec<String> {
    let Some(tag) = locale_for(lang) else { return Vec::new() };
    let mut list = imp::suggest(word, &tag);
    let mut seen = std::collections::HashSet::new();
    list.retain(|s| !s.is_empty() && s != word && seen.insert(s.clone()));
    list.truncate(MAX_SUGGESTIONS);
    list
}

/// Put `word` in the user's dictionary for `lang`, for good. Returns
/// whether it worked; always false where [`CAN_ADD`] is false.
pub fn add(word: &str, lang: LangId) -> bool {
    if !CAN_ADD || word.trim().is_empty() {
        return false;
    }
    let Some(tag) = locale_for(lang) else { return false };
    imp::add(word, &tag)
}

/// The best of `available` for a language whose BCP 47 tag is `lang`:
/// the tag itself, then the usual region (`en` → `en-US`, `ru` → `ru-RU`),
/// then any region of the same language. Tags compare without regard to
/// case or to `-` and `_`.
pub fn pick_locale(lang: &str, available: &[String]) -> Option<String> {
    let norm = |s: &str| s.replace('_', "-").to_ascii_lowercase();
    let want = norm(lang);
    let find = |tag: &str| available.iter().find(|a| norm(a) == tag).cloned();
    if let Some(a) = find(&want) {
        return Some(a);
    }
    let primary = want.split('-').next().unwrap_or_default().to_owned();
    // Norwegian goes by both names.
    let primaries: Vec<String> = match primary.as_str() {
        "nb" | "no" => vec!["nb".into(), "no".into()],
        "he" | "iw" => vec!["he".into(), "iw".into()],
        _ => vec![primary.clone()],
    };
    for p in &primaries {
        let region = usual_region(p).map(str::to_owned).unwrap_or_else(|| p.clone());
        if let Some(a) = find(&format!("{p}-{region}")).or_else(|| find(p)) {
            return Some(a);
        }
    }
    let mut same: Vec<&String> = available
        .iter()
        .filter(|a| {
            let n = norm(a);
            primaries.iter().any(|p| n.split('-').next() == Some(p.as_str()))
        })
        .collect();
    same.sort();
    same.first().map(|s| (*s).clone())
}

/// The region a language's dictionary is most often for, where it is not
/// the language's own code (`de-DE`, `ru-RU` need no entry).
fn usual_region(primary: &str) -> Option<&'static str> {
    Some(match primary {
        "en" => "us",
        "pt" => "br",
        "zh" => "cn",
        "sv" => "se",
        "da" => "dk",
        "cs" => "cz",
        "el" => "gr",
        "uk" => "ua",
        "he" | "iw" => "il",
        "ja" => "jp",
        "ko" => "kr",
        "sr" => "rs",
        "sl" => "si",
        "et" => "ee",
        "ca" => "es",
        "nb" | "nn" | "no" => "no",
        "fa" => "ir",
        "hi" => "in",
        "vi" => "vn",
        "ka" => "ge",
        "hy" => "am",
        "kk" => "kz",
        "sq" => "al",
        "be" => "by",
        "ga" => "ie",
        "cy" => "gb",
        "ms" => "my",
        "ar" => "sa",
        "ur" => "pk",
        "sw" => "ke",
        "af" => "za",
        "eu" => "es",
        "gl" => "es",
        _ => return None,
    })
}

/// Byte offsets of UTF-16 offsets in `text`: entry `i` is where the
/// character holding UTF-16 unit `i` starts; the last entry is
/// `text.len()`. The Windows and macOS checkers count in UTF-16.
#[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]
pub(crate) fn utf16_offsets(text: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        for _ in 0..c.len_utf16() {
            map.push(i);
        }
    }
    map.push(text.len());
    map
}

/// The misspelling at UTF-16 `start` and `len` in `text`, with `map` from
/// [`utf16_offsets`]; `None` when the range is outside the text.
#[cfg_attr(all(unix, not(target_os = "macos")), allow(dead_code))]
pub(crate) fn from_utf16(text: &str, map: &[usize], start: usize, len: usize) -> Option<Misspelling> {
    let a = *map.get(start)?;
    let b = *map.get(start.checked_add(len)?)?;
    (a < b).then(|| Misspelling { range: a..b, word: text[a..b].to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_language_finds_its_usual_dictionary() {
        let have = tags(&["en-AU", "en-GB", "en-US", "ru-RU", "de-CH", "pt-PT", "nb-NO"]);
        assert_eq!(pick_locale("en", &have).as_deref(), Some("en-US"));
        assert_eq!(pick_locale("ru", &have).as_deref(), Some("ru-RU"));
        // No de-DE: any German will do.
        assert_eq!(pick_locale("de", &have).as_deref(), Some("de-CH"));
        assert_eq!(pick_locale("pt", &have).as_deref(), Some("pt-PT"));
        assert_eq!(pick_locale("no", &have).as_deref(), Some("nb-NO"));
        assert_eq!(pick_locale("fr", &have), None);
        // An exact tag wins.
        assert_eq!(pick_locale("en-GB", &have).as_deref(), Some("en-GB"));
    }

    #[test]
    fn hunspell_names_and_bare_languages_match_too() {
        let have = tags(&["en_GB", "ru_RU", "uk"]);
        assert_eq!(pick_locale("en", &have).as_deref(), Some("en_GB"));
        assert_eq!(pick_locale("ru", &have).as_deref(), Some("ru_RU"));
        assert_eq!(pick_locale("uk", &have).as_deref(), Some("uk"));
        assert_eq!(pick_locale("he", &tags(&["iw-IL"])).as_deref(), Some("iw-IL"));
    }

    #[test]
    fn utf16_offsets_land_on_characters() {
        // "П" is 2 bytes and 1 unit; "😀" is 4 bytes and 2 units.
        let text = "Пр 😀 ok";
        let map = utf16_offsets(text);
        assert_eq!(map.len(), text.encode_utf16().count() + 1);
        assert_eq!(from_utf16(text, &map, 0, 2).unwrap().word, "Пр");
        assert_eq!(from_utf16(text, &map, 3, 2).unwrap().word, "😀");
        let ok = from_utf16(text, &map, 6, 2).unwrap();
        assert_eq!((ok.word.as_str(), ok.range), ("ok", 10..12));
        assert_eq!(from_utf16(text, &map, 7, 5), None);
    }

    /// The system checker on a real misspelling:
    /// `cargo test -p glossa-platform system_checker -- --ignored`.
    /// Needs an English dictionary (on Linux, `hunspell-en-us`).
    #[test]
    #[ignore = "uses the system's spell checker and its dictionaries"]
    fn system_checker_knows_hello() {
        eprintln!("languages: {:?}", languages());
        let lang = LangId::ENGLISH;
        eprintln!("English: {:?}", locale_for(lang));
        let found = check("Helo wrld, this is fine.", lang);
        eprintln!("found: {found:?}");
        let words: Vec<&str> = found.iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, ["Helo", "wrld"]);
        assert_eq!(found[0].range, 0..4);
        let guesses = suggest("Helo", lang);
        eprintln!("suggestions: {guesses:?}");
        assert!(guesses.iter().any(|g| g == "Hello"), "{guesses:?}");
    }
}
