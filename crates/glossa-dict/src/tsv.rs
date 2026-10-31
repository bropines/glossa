//! Tab-separated word lists (`.tsv`, `.txt`, `.csv`).
//!
//! The simplest dictionary there is, and the one people make themselves: one
//! `word<TAB>translation` per line. Lines without a tab fall back to `;` (and,
//! for `.csv` files only, `,` — in a `.txt` a comma is far more likely to be
//! part of the translation). `#` starts a comment line. A headword that
//! appears on several lines collects all their translations, in file order.
//! A literal `\n` in a translation is a line break.

use std::io;
use std::path::Path;

use glossa_core::LangId;

use crate::source::{
    decode_text, guess_lang_pair, invalid, read_maybe_gzip, Body, DictionarySource, KeyIndex, RawEntry,
    MAX_PREFIX_RESULTS,
};

struct Entry {
    headword: Box<str>,
    translations: Vec<Box<str>>,
}

/// An opened word list, held in memory.
pub struct Tsv {
    name: String,
    lang_pair: Option<(LangId, LangId)>,
    entries: Vec<Entry>,
    index: KeyIndex,
}

impl Tsv {
    pub fn open(path: &Path) -> io::Result<Tsv> {
        let bytes = read_maybe_gzip(path)?;
        let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let allow_comma = file_name.to_ascii_lowercase().ends_with(".csv");
        let stem = file_name.split('.').next().unwrap_or("").to_owned();
        Tsv::parse(&bytes, &stem, allow_comma).map_err(|detail| invalid(path, detail))
    }

    pub fn parse(bytes: &[u8], name: &str, allow_comma: bool) -> Result<Tsv, String> {
        let text = decode_text(bytes, false);
        if text.contains('\0') {
            return Err("binary data, not a word list".into());
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let split = line
                .split_once('\t')
                .or_else(|| line.split_once(';'))
                .or_else(|| if allow_comma { line.split_once(',') } else { None });
            let Some((word, translation)) = split else { continue };
            let word = unquote(word);
            let translation = unquote(translation).replace("\\n", "\n").replace('\t', "; ");
            if word.is_empty() || translation.trim().is_empty() {
                continue;
            }
            let key = crate::source::normalize_key(&word);
            match by_key.get(&key) {
                Some(&i) => entries[i].translations.push(translation.into_boxed_str()),
                None => {
                    by_key.insert(key, entries.len());
                    entries.push(Entry {
                        headword: word.into_boxed_str(),
                        translations: vec![translation.into_boxed_str()],
                    });
                }
            }
        }
        let mut index = KeyIndex::default();
        for (i, e) in entries.iter().enumerate() {
            index.push(&e.headword, i as u32);
        }
        index.finish();
        entries.shrink_to_fit();
        Ok(Tsv { name: name.to_owned(), lang_pair: guess_lang_pair(name), entries, index })
    }

    fn raw(&self, id: u32) -> RawEntry {
        let e = &self.entries[id as usize];
        let body = e.translations.iter().map(|t| t.trim()).collect::<Vec<_>>().join("\n");
        RawEntry { headword: e.headword.to_string(), body: Body::Plain(body) }
    }
}

/// Trim, and strip one pair of surrounding double quotes (spreadsheet
/// exports quote fields), undoubling `""` inside.
fn unquote(s: &str) -> String {
    let t = s.trim();
    match t.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => t.to_owned(),
    }
}

impl DictionarySource for Tsv {
    fn name(&self) -> &str {
        &self.name
    }

    fn lang_pair(&self) -> Option<(LangId, LangId)> {
        self.lang_pair
    }

    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn lookup(&self, word: &str, exact: bool) -> Vec<RawEntry> {
        self.index.lookup(word, exact, MAX_PREFIX_RESULTS).into_iter().map(|id| self.raw(id)).collect()
    }

    fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.index.prefixed(prefix, limit).into_iter().map(|id| self.entries[id as usize].headword.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::testutil::TempDir;

    const SAMPLE: &str = "# my words en-ru\nhello\tпривет\nworld\tмир\nHello\tздравствуйте\nsemi;colon\n\"quoted\";\"a \"\"b\"\"\"\nmulti\tline\\nbreak\nno separator here\ncomma,separated\n\n";

    #[test]
    fn lines_parse_and_merge() {
        let t = Tsv::parse(SAMPLE.as_bytes(), "words_en-ru", false).unwrap();
        assert_eq!(t.lang_pair(), Some((LangId::ENGLISH, LangId::RUSSIAN)));
        assert_eq!(t.entry_count(), 5);
        let hits = t.lookup("HELLO", true);
        assert_eq!(hits, vec![RawEntry::new("hello", Body::Plain("привет\nздравствуйте".into()))]);
        let a = hits[0].to_article();
        assert_eq!(a.explanations[0].definitions, vec!["привет", "здравствуйте"]);
        assert_eq!(t.lookup("semi", true)[0].body, Body::Plain("colon".into()));
        assert_eq!(t.lookup("quoted", true)[0].body, Body::Plain("a \"b\"".into()));
        assert_eq!(t.lookup("multi", true)[0].body, Body::Plain("line\nbreak".into()));
        assert!(t.lookup("comma", true).is_empty());
        assert_eq!(t.suggest("w", 5), vec!["world"]);
        assert_eq!(t.lookup("wor", false).len(), 1);
    }

    #[test]
    fn csv_files_also_split_on_commas() {
        let dir = TempDir::new("tsv");
        let p = dir.write("list.csv", "comma,separated\n".as_bytes());
        let t = Tsv::open(&p).unwrap();
        assert_eq!(t.lookup("comma", true)[0].body, Body::Plain("separated".into()));
        assert_eq!(t.name(), "list");
    }

    #[test]
    fn binary_is_rejected() {
        assert!(Tsv::parse(b"\x00\x01\x02a\tb", "x", false).is_err());
        let t = Tsv::parse(b"", "x", false).unwrap();
        assert_eq!(t.entry_count(), 0);
        assert!(t.lookup("a", false).is_empty());
    }
}
