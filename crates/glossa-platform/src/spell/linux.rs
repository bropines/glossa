//! Linux and the BSDs: the Hunspell dictionaries the system has installed
//! (`hunspell-en-us`, `hunspell-ru`…), read by `spellbook`, a Hunspell
//! reader in pure Rust. Words added go to Hunspell's personal dictionary,
//! `~/.hunspell_<name>` (`~/.hunspell_en_US`), which the `hunspell` command
//! and other programs on Hunspell read too.
//!
//! A dictionary loads on first use and stays loaded for as long as the
//! thread that loaded it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use spellbook::Dictionary;
use unicode_segmentation::UnicodeSegmentation;

use super::Misspelling;

pub const MAIN_THREAD_ONLY: bool = false;
pub const CAN_ADD: bool = true;

/// Where Hunspell dictionaries are looked for, first match wins.
fn search_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("DICPATH") {
        out.extend(std::env::split_paths(&p));
    }
    if let Some(d) = dirs::data_dir() {
        out.push(d.join("hunspell"));
    }
    for d in [
        "/usr/share/hunspell",
        "/usr/share/myspell",
        "/usr/share/myspell/dicts",
        "/usr/local/share/hunspell",
        "/usr/local/share/myspell",
    ] {
        out.push(PathBuf::from(d));
    }
    out
}

/// Each dictionary as (tag with `-`, path without the extension).
fn installed() -> &'static [(String, PathBuf)] {
    static FOUND: OnceLock<Vec<(String, PathBuf)>> = OnceLock::new();
    FOUND.get_or_init(|| {
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        for dir in search_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut here: Vec<(String, PathBuf)> = entries
                .flatten()
                .filter_map(|e| {
                    let path = e.path();
                    if path.extension()? != "dic" {
                        return None;
                    }
                    let base = path.with_extension("");
                    let name = base.file_name()?.to_str()?.to_owned();
                    // Hyphenation and thesaurus files are .dic too, without
                    // an .aff beside them.
                    base.with_extension("aff").is_file().then(|| (name.replace('_', "-"), base))
                })
                .collect();
            here.sort();
            for (tag, base) in here {
                if !found.iter().any(|(t, _)| *t == tag) {
                    found.push((tag, base));
                }
            }
        }
        found
    })
}

pub fn languages() -> Vec<String> {
    installed().iter().map(|(t, _)| t.clone()).collect()
}

thread_local! {
    /// Loaded dictionaries, per thread: they go when the thread that used
    /// them ends (the app's spelling worker ends with its window state), so
    /// a hidden app does not keep megabytes of word lists.
    static LOADED: RefCell<HashMap<String, Option<Rc<RefCell<Dictionary>>>>> = RefCell::new(HashMap::new());
}

/// Run `f` with the dictionary for `tag`, loaded on first use. `None` when
/// it is not installed or does not parse.
fn with_dictionary<R>(tag: &str, f: impl FnOnce(&mut Dictionary) -> R) -> Option<R> {
    let dict = LOADED.with(|loaded| {
        if let Some(d) = loaded.borrow().get(tag) {
            return d.clone();
        }
        let d = installed()
            .iter()
            .find(|(t, _)| t == tag)
            .and_then(|(_, base)| load(base))
            .map(|d| Rc::new(RefCell::new(d)));
        loaded.borrow_mut().insert(tag.to_owned(), d.clone());
        d
    })?;
    let mut d = dict.try_borrow_mut().ok()?;
    Some(f(&mut d))
}

fn load(base: &Path) -> Option<Dictionary> {
    let aff = std::fs::read(base.with_extension("aff")).ok()?;
    let dic = std::fs::read(base.with_extension("dic")).ok()?;
    let label = encoding_of(&aff);
    let started = std::time::Instant::now();
    let mut dict = match Dictionary::new(&decode(&aff, &label), &decode(&dic, &label)) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("spell: cannot read {}: {e}", base.display());
            return None;
        }
    };
    if let Some(path) = personal_file(base) {
        if let Ok(words) = std::fs::read_to_string(path) {
            for line in words.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('*')) {
                let _ = dict.add(line);
            }
        }
    }
    tracing::debug!("spell: loaded {} in {:?}", base.display(), started.elapsed());
    Some(dict)
}

/// The aff file's `SET` line: the encoding of both files.
fn encoding_of(aff: &[u8]) -> String {
    aff.split(|&b| b == b'\n')
        .take(200)
        .find_map(|line| {
            let line = String::from_utf8_lossy(line);
            let rest = line.trim().strip_prefix("SET")?;
            rest.starts_with(char::is_whitespace).then(|| rest.trim().to_owned())
        })
        .unwrap_or_else(|| "UTF-8".to_owned())
}

/// Hunspell's names for encodings, read as the web's names.
fn decode(bytes: &[u8], label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    let label = match lower.strip_prefix("microsoft-cp") {
        Some(n) => format!("windows-{n}"),
        None => lower,
    };
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// `~/.hunspell_en_US` for `/usr/share/hunspell/en_US`.
fn personal_file(base: &Path) -> Option<PathBuf> {
    let name = base.file_name()?.to_str()?;
    Some(dirs::home_dir()?.join(format!(".hunspell_{name}")))
}

/// The words of `text` worth checking, as byte ranges: no numbers, no
/// identifiers, no single letters, nothing from a link or an address.
pub(crate) fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut chunk_start = 0;
    for chunk in text.split_inclusive(char::is_whitespace) {
        let skip = chunk.contains("://") || chunk.contains('@') || chunk.starts_with("www.");
        if !skip {
            for (i, w) in chunk.split_word_bound_indices() {
                let mut chars = w.chars();
                let worth = w.chars().any(char::is_alphabetic)
                    && !w.chars().any(|c| c.is_numeric() || c == '_')
                    && chars.next().is_some()
                    && chars.next().is_some();
                if worth {
                    out.push(chunk_start + i..chunk_start + i + w.len());
                }
            }
        }
        chunk_start += chunk.len();
    }
    out
}

fn known(dict: &Dictionary, word: &str) -> bool {
    dict.check(word) || (word.contains('’') && dict.check(&word.replace('’', "'")))
}

pub fn check(text: &str, tag: &str) -> Vec<Misspelling> {
    with_dictionary(tag, |dict| check_with(dict, text)).unwrap_or_default()
}

fn check_with(dict: &Dictionary, text: &str) -> Vec<Misspelling> {
    words(text)
        .into_iter()
        .filter(|r| !known(dict, &text[r.clone()]))
        .map(|r| Misspelling { word: text[r.clone()].to_owned(), range: r })
        .collect()
}

pub fn suggest(word: &str, tag: &str) -> Vec<String> {
    let Some(out) = with_dictionary(tag, |dict| {
        let mut out = Vec::new();
        dict.suggest(word, &mut out);
        out
    }) else {
        return Vec::new();
    };
    rank(word, out)
}

/// spellbook gives suggestions in the order it found them, not by how
/// likely they are (Hunspell ranks by n-grams). Put the likely ones first:
/// whole words before split ones, then the fewest edits, then a doubled or
/// undoubled letter ("Helo" → "Hello"), then swapped letters ("teh" →
/// "the"), then letters left out.
fn rank(word: &str, list: Vec<String>) -> Vec<String> {
    let w: Vec<char> = word.to_lowercase().chars().collect();
    let mut keyed: Vec<((bool, usize, u8), usize, String)> = list
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let c: Vec<char> = s.to_lowercase().chars().collect();
            let split = c.iter().any(|&ch| ch == ' ' || ch == '-') && !w.iter().any(|&ch| ch == ' ' || ch == '-');
            let kind = if undouble(&c) == undouble(&w) {
                0
            } else if sorted(&c) == sorted(&w) {
                1
            } else if is_subsequence(&w, &c) {
                2
            } else {
                3
            };
            ((split, distance(&w, &c), kind), i, s)
        })
        .collect();
    keyed.sort_by_key(|a| (a.0, a.1));
    keyed.into_iter().map(|(_, _, s)| s).collect()
}

fn undouble(s: &[char]) -> Vec<char> {
    let mut out = s.to_vec();
    out.dedup();
    out
}

fn sorted(s: &[char]) -> Vec<char> {
    let mut out = s.to_vec();
    out.sort_unstable();
    out
}

fn is_subsequence(short: &[char], long: &[char]) -> bool {
    let mut it = long.iter();
    short.iter().all(|c| it.any(|l| l == c))
}

/// Edit distance, with a swap of neighbours as one edit.
fn distance(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

pub fn add(word: &str, tag: &str) -> bool {
    // A slash would start Hunspell flags.
    if word.contains(['/', '\n']) {
        return false;
    }
    if with_dictionary(tag, |dict| dict.add(word).is_ok()) != Some(true) {
        return false;
    }
    let base = installed().iter().find(|(t, _)| t == tag).map(|(_, b)| b.clone());
    let Some(path) = base.as_deref().and_then(personal_file) else { return true };
    let saved =
        std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| writeln!(f, "{word}"));
    if let Err(e) = saved {
        tracing::warn!("spell: cannot save to {}: {e}", path.display());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words_of(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn words_skip_numbers_links_and_single_letters() {
        assert_eq!(words_of("Helo wrld, a test!"), ["Helo", "wrld", "test"]);
        assert_eq!(words_of("Привет, мир 😀 ok"), ["Привет", "мир", "ok"]);
        assert_eq!(words_of("don't stop"), ["don't", "stop"]);
        assert_eq!(words_of("see https://exmple.com or me@exmple.com now"), ["see", "or", "now"]);
        assert_eq!(words_of("abc123 snake_case 42 x"), Vec::<&str>::new());
        // Byte ranges, past multi-byte characters.
        let text = "мир helo";
        assert_eq!(words(text), [0..6, 7..11]);
    }

    #[test]
    fn a_tiny_dictionary_checks_and_suggests() {
        let aff = "SET UTF-8\nTRY abcdefghijklmnopqrstuvwxyz\n";
        let dic = "3\nhello\nworld\nмир\n";
        let mut dict = Dictionary::new(aff, dic).unwrap();
        let found = check_with(&dict, "hello wrld, мир мирр");
        let words: Vec<&str> = found.iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, ["wrld", "мирр"]);
        assert_eq!(found[1].range, 19..27);
        let mut out = Vec::new();
        dict.suggest("wrld", &mut out);
        assert!(out.iter().any(|s| s == "world"), "{out:?}");
        dict.add("wrld").unwrap();
        assert_eq!(check_with(&dict, "hello wrld").len(), 0);
    }

    #[test]
    fn likely_suggestions_come_first() {
        let list = ["Hel", "Helot", "Hero", "He lo", "Hole", "Hello"].map(String::from).to_vec();
        assert_eq!(rank("Helo", list), ["Hello", "Helot", "Hel", "Hero", "Hole", "He lo"]);
        assert_eq!(distance(&['a', 'b'], &['b', 'a']), 1);
        assert_eq!(rank("teh", vec!["ten".into(), "the".into()]), ["the", "ten"]);
    }

    #[test]
    fn hunspell_encodings_are_understood() {
        assert_eq!(encoding_of(b"# comment\nSET KOI8-R\nTRY x\n"), "KOI8-R");
        assert_eq!(encoding_of(b"TRY x\n"), "UTF-8");
        // "мир" in KOI8-R and in Windows-1251.
        assert_eq!(decode(&[0xcd, 0xc9, 0xd2], "KOI8-R"), "мир");
        assert_eq!(decode(&[0xec, 0xe8, 0xf0], "microsoft-cp1251"), "мир");
        assert_eq!(decode("été".as_bytes(), "UTF-8"), "été");
    }

    /// The system's en_US dictionary, where it is installed
    /// (`hunspell-en-us`); passes quietly where it is not.
    #[test]
    fn the_system_english_dictionary_when_installed() {
        let Some((_, base)) = installed().iter().find(|(t, _)| t == "en-US") else { return };
        let dict = load(base).expect("en_US loads");
        let found = check_with(&dict, "Helo wrld, this is fine.");
        let words: Vec<&str> = found.iter().map(|m| m.word.as_str()).collect();
        assert_eq!(words, ["Helo", "wrld"]);
        let mut out = Vec::new();
        dict.suggest("Helo", &mut out);
        let ranked = rank("Helo", out);
        assert_eq!(ranked.first().map(String::as_str), Some("Hello"), "{ranked:?}");
    }
}
