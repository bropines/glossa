//! What every dictionary format has in common.
//!
//! A [`DictionarySource`] is one opened dictionary file. It answers lookups
//! with [`RawEntry`]s — the headword plus the article body in the format's own
//! markup — and [`RawEntry::to_article`] turns that into the structured
//! [`Article`] the rest of Glossa speaks. Keeping the raw body until the last
//! moment means a lookup costs only the entries actually shown, and the HTML
//! we generate for [`Article::html`] can be built from the original markup
//! rather than from a lossy intermediate.
//!
//! The shared helpers here — text decoding, gzip sniffing, the sorted key
//! index and language guessing — exist because DSL, XDXF and tab-separated
//! files all need the same things and differ only in how they find their
//! entries.

use std::io::{self, Read};
use std::path::Path;

use glossa_core::dict::{Explanation, Pronunciation};
use glossa_core::{Article, LangId};

use crate::html;

/// How many entries a non-exact lookup returns at most. Each one is read and
/// converted, and a popup has room for a handful anyway.
pub const MAX_PREFIX_RESULTS: usize = 12;

/// One opened dictionary.
pub trait DictionarySource: Send + Sync {
    /// The dictionary's own title (`bookname`, `#NAME`, `<full_name>`), or
    /// the file name when it has none.
    fn name(&self) -> &str;

    /// Source and target language, when the file says or its name gives it
    /// away. `None` means "unknown": the dictionary is consulted for every
    /// language pair.
    fn lang_pair(&self) -> Option<(LangId, LangId)>;

    /// Number of headwords (synonyms not counted).
    fn entry_count(&self) -> usize;

    /// Look `word` up. `exact` matches the headword case-insensitively;
    /// otherwise headwords that start with `word` are included too, exact
    /// matches first, then shorter headwords, at most
    /// [`MAX_PREFIX_RESULTS`]. A read error on one entry drops that entry
    /// (and is logged) rather than failing the lookup.
    fn lookup(&self, word: &str, exact: bool) -> Vec<RawEntry>;

    /// Headwords starting with `prefix`, in index order, for the dictionary
    /// window's completion list.
    fn suggest(&self, prefix: &str, limit: usize) -> Vec<String>;
}

/// An article as the dictionary stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub headword: String,
    pub body: Body,
}

/// An article body in its original markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Plain text; line breaks separate senses.
    Plain(String),
    /// An HTML fragment (StarDict `h`, and Pango markup from `g`).
    Html(String),
    /// The body lines of an ABBYY Lingvo DSL card, markup intact.
    Dsl(String),
    /// An XDXF article fragment (the inside of `<ar>`, or a StarDict `x`
    /// field).
    Xdxf(String),
    /// A transcription (StarDict `t` / `y` fields).
    Phonetic(String),
    /// Several fields of one StarDict entry, in order.
    Parts(Vec<Body>),
}

impl RawEntry {
    pub fn new(headword: impl Into<String>, body: Body) -> Self {
        RawEntry { headword: headword.into(), body }
    }

    /// Convert to an [`Article`]. The structured fields are best-effort; the
    /// `html` field always carries the full article, since that is what
    /// keeps the original formatting.
    pub fn to_article(&self) -> Article {
        body_to_article(&self.headword, &self.body)
    }
}

fn body_to_article(headword: &str, body: &Body) -> Article {
    match body {
        Body::Plain(text) => plain_article(headword, text),
        Body::Html(fragment) => html_article(headword, fragment),
        Body::Dsl(text) => crate::dsl::to_article(headword, text),
        Body::Xdxf(fragment) => crate::xdxf::to_article(headword, fragment),
        Body::Phonetic(symbol) => {
            let symbol = symbol.trim();
            Article {
                headword: headword.to_owned(),
                pronunciations: if symbol.is_empty() {
                    Vec::new()
                } else {
                    vec![Pronunciation { symbol: symbol.to_owned(), ..Default::default() }]
                },
                html: Some(format!("<p>[{}]</p>", html::escape(symbol))),
                ..Default::default()
            }
        }
        Body::Parts(parts) => {
            let mut out = Article { headword: headword.to_owned(), ..Default::default() };
            let mut html_parts = String::new();
            for part in parts {
                let a = body_to_article(headword, part);
                merge_into(&mut out, a, &mut html_parts);
            }
            out.html = (!html_parts.is_empty()).then_some(html_parts);
            out
        }
    }
}

/// Fold one article's fields into another, collecting HTML separately.
pub(crate) fn merge_into(out: &mut Article, a: Article, html_out: &mut String) {
    for p in a.pronunciations {
        if !out.pronunciations.contains(&p) {
            out.pronunciations.push(p);
        }
    }
    for e in a.explanations {
        match out.explanations.iter_mut().find(|x| x.part_of_speech == e.part_of_speech) {
            Some(existing) => existing.definitions.extend(e.definitions),
            None => out.explanations.push(e),
        }
    }
    out.examples.extend(a.examples);
    for s in a.associations {
        if !out.associations.contains(&s) {
            out.associations.push(s);
        }
    }
    if let Some(h) = a.html {
        html_out.push_str(&h);
    }
}

fn plain_article(headword: &str, text: &str) -> Article {
    let definitions: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    let html = text.trim().lines().map(html::escape).collect::<Vec<_>>().join("<br>");
    Article {
        headword: headword.to_owned(),
        explanations: if definitions.is_empty() {
            Vec::new()
        } else {
            vec![Explanation { part_of_speech: String::new(), definitions }]
        },
        html: Some(format!("<div>{html}</div>")),
        ..Default::default()
    }
}

fn html_article(headword: &str, fragment: &str) -> Article {
    let text = html::to_plain_text(fragment);
    let definitions: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    Article {
        headword: headword.to_owned(),
        explanations: if definitions.is_empty() {
            Vec::new()
        } else {
            vec![Explanation { part_of_speech: String::new(), definitions }]
        },
        html: Some(fragment.to_owned()),
        ..Default::default()
    }
}

/// Split an example line written "source — translation".
pub(crate) fn split_example(text: &str) -> glossa_core::dict::Example {
    let text = text.trim();
    for sep in [" — ", " – ", " - ", " ― "] {
        if let Some((src, tgt)) = text.split_once(sep) {
            return glossa_core::dict::Example { source: src.trim().to_owned(), target: tgt.trim().to_owned() };
        }
    }
    glossa_core::dict::Example { source: text.to_owned(), target: String::new() }
}

// ---------------------------------------------------------------------------
// Reading files
// ---------------------------------------------------------------------------

/// Read a whole file, transparently un-gzipping it (`.dsl.dz`, `.xdxf.dz`,
/// `.idx.gz`). Dictzip is gzip, so a plain decoder reads it front to back.
pub(crate) fn read_maybe_gzip(path: &Path) -> io::Result<Vec<u8>> {
    let raw = std::fs::read(path)?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(&raw[..]).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(raw)
    }
}

pub(crate) fn invalid(path: &Path, detail: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {detail}", path.display()))
}

/// Windows-1251 upper half, for DSL files from before Lingvo went Unicode.
const CP1251_HIGH: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C,
    0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0xFFFD, 0x2122, 0x0459, 0x203A,
    0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9,
    0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407, 0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
    0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457, 0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415,
    0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423,
    0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F, 0x0430, 0x0431,
    0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D,
    0x044E, 0x044F,
];

/// Decode a text dictionary file: UTF-8 or UTF-16 (either byte order) with or
/// without a BOM. Text that is neither is read as Windows-1251 when
/// `cyrillic_fallback` is set (old Russian DSL files), else as Latin-1 —
/// never an error, because a slightly mis-decoded dictionary beats none.
pub(crate) fn decode_text(bytes: &[u8], cyrillic_fallback: bool) -> String {
    fn utf16(bytes: &[u8], le: bool) -> String {
        let units = bytes.chunks_exact(2).map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect()
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    // No BOM: UTF-16 shows itself through zero bytes in ASCII-heavy text.
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.len() >= 4 {
        let even_zeros = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd_zeros = sample.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let half = sample.len() / 2;
        if odd_zeros * 3 > half && even_zeros * 10 < half {
            return utf16(bytes, true);
        }
        if even_zeros * 3 > half && odd_zeros * 10 < half {
            return utf16(bytes, false);
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) if cyrillic_fallback => bytes
            .iter()
            .map(|&b| {
                if b < 0x80 {
                    b as char
                } else {
                    char::from_u32(CP1251_HIGH[(b - 0x80) as usize] as u32).unwrap_or('\u{fffd}')
                }
            })
            .collect(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

// ---------------------------------------------------------------------------
// The sorted key index
// ---------------------------------------------------------------------------

/// Normalise a headword for matching: Unicode lowercase, stress marks
/// removed (Russian dictionaries mark stress with a combining acute, and
/// nobody types it), whitespace collapsed.
pub(crate) fn normalize_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.trim().chars() {
        if matches!(c, '\u{0301}' | '\u{0300}' | '\u{00AD}') {
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.extend(c.to_lowercase());
    }
    out
}

/// Normalised key → entry id, sorted, for the formats that are read into
/// memory (DSL, XDXF, tab-separated). Lookups are a binary search.
#[derive(Debug, Default)]
pub(crate) struct KeyIndex {
    keys: Vec<(Box<str>, u32)>,
}

impl KeyIndex {
    pub(crate) fn push(&mut self, headword: &str, id: u32) {
        let key = normalize_key(headword);
        if !key.is_empty() {
            self.keys.push((key.into_boxed_str(), id));
        }
    }

    pub(crate) fn finish(&mut self) {
        self.keys.sort();
        self.keys.dedup();
        self.keys.shrink_to_fit();
    }

    fn lower_bound(&self, key: &str) -> usize {
        self.keys.partition_point(|(k, _)| &**k < key)
    }

    /// Entry ids for `word`: exact matches, then (unless `exact`) prefix
    /// matches ranked by headword length. Each id appears once.
    pub(crate) fn lookup(&self, word: &str, exact: bool, cap: usize) -> Vec<u32> {
        let key = normalize_key(word);
        if key.is_empty() {
            return Vec::new();
        }
        let mut exacts = Vec::new();
        let mut prefixed: Vec<(usize, u32)> = Vec::new();
        for (k, id) in &self.keys[self.lower_bound(&key)..] {
            if !k.starts_with(key.as_str()) {
                break;
            }
            if **k == *key {
                exacts.push(*id);
            } else if exact {
                break;
            } else {
                prefixed.push((k.chars().count(), *id));
                // The ranking needs some slack past the cap, but scanning
                // every word under a one-letter prefix is pointless.
                if prefixed.len() >= cap * 8 {
                    break;
                }
            }
        }
        prefixed.sort_by_key(|(len, _)| *len);
        let mut out: Vec<u32> = Vec::new();
        for id in exacts.into_iter().chain(prefixed.into_iter().map(|(_, id)| id)) {
            if !out.contains(&id) {
                out.push(id);
                if out.len() >= cap {
                    break;
                }
            }
        }
        out
    }

    /// Entry ids whose key starts with `prefix`, in key order.
    pub(crate) fn prefixed(&self, prefix: &str, limit: usize) -> Vec<u32> {
        let key = normalize_key(prefix);
        let mut out = Vec::new();
        for (k, id) in &self.keys[self.lower_bound(&key)..] {
            if out.len() >= limit || !k.starts_with(key.as_str()) {
                break;
            }
            if !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }
}

/// One headword of an in-memory dictionary and the byte range of its
/// article in the decoded text.
pub(crate) struct Card {
    pub(crate) headword: Box<str>,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

/// Cards plus their key index: the shared shape of DSL and XDXF, where the
/// whole file is decoded once and articles are slices of it.
#[derive(Default)]
pub(crate) struct CardIndex {
    pub(crate) cards: Vec<Card>,
    index: KeyIndex,
}

impl CardIndex {
    /// Add a card findable under each of `keys`.
    pub(crate) fn push(&mut self, headword: String, keys: &[String], start: usize, end: usize) {
        let id = self.cards.len() as u32;
        for k in keys {
            self.index.push(k, id);
        }
        self.cards.push(Card { headword: headword.into_boxed_str(), start: start as u32, end: end as u32 });
    }

    pub(crate) fn finish(&mut self) {
        self.index.finish();
        self.cards.shrink_to_fit();
    }

    pub(crate) fn len(&self) -> usize {
        self.cards.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Matching cards, best first. Several headwords of one article point
    /// at the same range; it is returned once.
    pub(crate) fn lookup(&self, word: &str, exact: bool) -> Vec<&Card> {
        let mut seen: Vec<(u32, u32)> = Vec::new();
        let mut out = Vec::new();
        for id in self.index.lookup(word, exact, MAX_PREFIX_RESULTS * 2) {
            let card = &self.cards[id as usize];
            if seen.contains(&(card.start, card.end)) {
                continue;
            }
            seen.push((card.start, card.end));
            out.push(card);
            if out.len() >= MAX_PREFIX_RESULTS {
                break;
            }
        }
        out
    }

    pub(crate) fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for id in self.index.prefixed(prefix, limit.saturating_mul(2)) {
            let h = self.cards[id as usize].headword.to_string();
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out.truncate(limit);
        out
    }
}

/// "1) foo" → "foo", "2. bar" → "bar": the UI numbers senses itself.
pub(crate) fn strip_numbering(s: &str) -> &str {
    let t = s.trim();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits < 4 {
        if let Some(rest) = t[digits..].strip_prefix([')', '.']) {
            return rest.trim();
        }
    }
    t
}

/// Collapse whitespace runs to single spaces.
pub(crate) fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Languages
// ---------------------------------------------------------------------------

/// ISO 639-2 codes (XDXF and file names use them) for the languages
/// dictionaries are commonly made for.
const ISO639_2: &[(&str, &str)] = &[
    ("eng", "en"),
    ("rus", "ru"),
    ("deu", "de"),
    ("ger", "de"),
    ("fra", "fr"),
    ("fre", "fr"),
    ("spa", "es"),
    ("ita", "it"),
    ("por", "pt"),
    ("ukr", "uk"),
    ("pol", "pl"),
    ("jpn", "ja"),
    ("chi", "zh-CN"),
    ("zho", "zh-CN"),
    ("kor", "ko"),
    ("nld", "nl"),
    ("dut", "nl"),
    ("swe", "sv"),
    ("fin", "fi"),
    ("ces", "cs"),
    ("cze", "cs"),
    ("tur", "tr"),
    ("ara", "ar"),
    ("heb", "he"),
    ("lat", "la"),
    ("ell", "el"),
    ("gre", "el"),
    ("hun", "hu"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("bul", "bg"),
    ("srp", "sr"),
    ("hrv", "hr"),
    ("slk", "sk"),
    ("slo", "sk"),
    ("slv", "sl"),
    ("dan", "da"),
    ("nor", "no"),
    ("nob", "no"),
    ("est", "et"),
    ("lav", "lv"),
    ("lit", "lt"),
    ("bel", "be"),
    ("kaz", "kk"),
    ("tat", "tt"),
    ("hin", "hi"),
    ("vie", "vi"),
    ("tha", "th"),
    ("ind", "id"),
    ("fas", "fa"),
    ("per", "fa"),
    ("epo", "eo"),
    ("cat", "ca"),
    ("eus", "eu"),
    ("baq", "eu"),
    ("glg", "gl"),
    ("isl", "is"),
    ("ice", "is"),
    ("gle", "ga"),
    ("cym", "cy"),
    ("wel", "cy"),
    ("sqi", "sq"),
    ("alb", "sq"),
    ("mkd", "mk"),
    ("mac", "mk"),
    ("hye", "hy"),
    ("arm", "hy"),
    ("kat", "ka"),
    ("geo", "ka"),
    ("aze", "az"),
    ("uzb", "uz"),
    ("afr", "af"),
    ("mlt", "mt"),
    ("msa", "ms"),
    ("may", "ms"),
    ("swa", "sw"),
    ("urd", "ur"),
    ("yid", "yi"),
    ("mon", "mn"),
    ("tgk", "tg"),
];

/// A language from whatever a dictionary file wrote: a two-letter code, an
/// ISO 639-2 code, or an English name the way ABBYY spells it ("English",
/// "GermanNewSpelling").
pub(crate) fn lang_from_any(s: &str) -> Option<LangId> {
    let s = s.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    if s.len() == 3 {
        if let Some((_, two)) = ISO639_2.iter().find(|(three, _)| three.eq_ignore_ascii_case(s)) {
            return LangId::from_code(two).filter(|l| l.is_language());
        }
    }
    if s.len() <= 3 || (s.len() <= 7 && s.contains(['-', '_'])) {
        if let Some(l) = LangId::from_code(s).filter(|l| l.is_language()) {
            return Some(l);
        }
    }
    let lower = s.to_ascii_lowercase();
    // Exact English name, then the longest name the text starts with
    // ("GermanNewSpelling" → German), then a name that starts with the text
    // ("Chinese" → Chinese (Simplified)).
    if let Some(l) = LangId::all().find(|l| l.english_name().eq_ignore_ascii_case(s)) {
        return Some(l);
    }
    if let Some(l) = LangId::all()
        .filter(|l| lower.starts_with(&l.english_name().to_ascii_lowercase()))
        .max_by_key(|l| l.english_name().len())
    {
        return Some(l);
    }
    if lower.len() >= 4 {
        return LangId::all().find(|l| l.english_name().to_ascii_lowercase().starts_with(&lower));
    }
    None
}

/// Find a language pair spelled into a dictionary's name or file name:
/// "Mueller (En-Ru)", "eng-rus", "de_en", "en2ru".
pub(crate) fn guess_lang_pair(text: &str) -> Option<(LangId, LangId)> {
    let chars: Vec<char> = text.chars().collect();
    let letters = |from: usize| chars[from..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if !(boundary && chars[i].is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let a_len = letters(i);
        let sep = i + a_len;
        if (2..=3).contains(&a_len) && sep + 1 < chars.len() && matches!(chars[sep], '-' | '_' | '2' | '>') {
            let b_start = sep + 1;
            let b_len = letters(b_start);
            let end = b_start + b_len;
            let ends_cleanly = end == chars.len() || !chars[end].is_ascii_alphanumeric();
            if (2..=3).contains(&b_len) && ends_cleanly {
                let a: String = chars[i..sep].iter().collect();
                let b: String = chars[b_start..end].iter().collect();
                if let (Some(x), Some(y)) = (code_only(&a), code_only(&b)) {
                    if x != y {
                        return Some((x, y));
                    }
                }
            }
        }
        i += a_len.max(1);
    }
    None
}

/// A language from a bare 2/3-letter code, without the English-name
/// fallback (which would read "the" or "big" as something).
fn code_only(code: &str) -> Option<LangId> {
    if code.len() == 3 {
        return ISO639_2
            .iter()
            .find(|(three, _)| three.eq_ignore_ascii_case(code))
            .and_then(|(_, two)| LangId::from_code(two))
            .filter(|l| l.is_language());
    }
    LangId::from_code(code).filter(|l| l.is_language())
}

/// Temporary directories for the format tests.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct TempDir(PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "glossa-dict-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }

        pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, bytes).expect("write fixture");
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Gzip `data` (plain, not dictzip).
    pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    pub(crate) fn utf16le_with_bom(s: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_decoding_handles_boms_and_bomless_utf16() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi", false), "hi");
        assert_eq!(decode_text(&testutil::utf16le_with_bom("слово"), false), "слово");
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("ab".encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
        assert_eq!(decode_text(&be, false), "ab");
        let bomless: Vec<u8> = "#NAME x".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_text(&bomless, false), "#NAME x");
        // "Привет" in Windows-1251.
        assert_eq!(decode_text(&[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2], true), "Привет");
        assert_eq!(decode_text(&[0xE9], false), "é");
        // Odd-length UTF-16 must not panic.
        let _ = decode_text(&[0xFF, 0xFE, 0x41], false);
    }

    #[test]
    fn keys_ignore_case_stress_and_spacing() {
        assert_eq!(normalize_key("  Приве\u{301}т   Мир "), "привет мир");
        assert_eq!(normalize_key("HELLO"), "hello");
    }

    #[test]
    fn key_index_ranks_exact_then_shorter_prefixes() {
        let mut idx = KeyIndex::default();
        for (i, w) in ["running", "run", "Run", "runner", "rune", "apple"].iter().enumerate() {
            idx.push(w, i as u32);
        }
        idx.finish();
        let hits = idx.lookup("RUN", true, 10);
        assert_eq!(hits, vec![1, 2]);
        let hits = idx.lookup("run", false, 10);
        assert_eq!(&hits[..2], &[1, 2]);
        assert_eq!(hits[2], 4); // "rune" before "runner" before "running"
        assert_eq!(hits.len(), 5);
        assert_eq!(idx.lookup("run", false, 3).len(), 3);
        assert!(idx.lookup("zzz", false, 3).is_empty());
        assert!(idx.lookup("", false, 3).is_empty());
        assert_eq!(idx.prefixed("ru", 2).len(), 2);
    }

    #[test]
    fn language_pairs_are_found_in_names() {
        let en_ru = Some((LangId::ENGLISH, LangId::RUSSIAN));
        assert_eq!(guess_lang_pair("Mueller (En-Ru)"), en_ru);
        assert_eq!(guess_lang_pair("mueller-eng-rus"), en_ru);
        assert_eq!(guess_lang_pair("LingvoUniversal_en_ru.dsl"), en_ru);
        assert_eq!(guess_lang_pair("en2ru"), en_ru);
        assert_eq!(guess_lang_pair("Collins English Dictionary"), None);
        assert_eq!(guess_lang_pair("my-dictionary"), None);
        assert_eq!(lang_from_any("English"), Some(LangId::ENGLISH));
        assert_eq!(lang_from_any("\"Russian\""), Some(LangId::RUSSIAN));
        assert_eq!(lang_from_any("GermanNewSpelling"), LangId::from_code("de"));
        assert_eq!(lang_from_any("RUS"), Some(LangId::RUSSIAN));
        assert_eq!(lang_from_any("Chinese"), LangId::from_code("zh-CN"));
        assert_eq!(lang_from_any("Klingon"), None);
    }

    #[test]
    fn multi_part_bodies_merge() {
        let e = RawEntry::new("cat", Body::Parts(vec![Body::Phonetic("kæt".into()), Body::Plain("кошка\nкот".into())]));
        let a = e.to_article();
        assert_eq!(a.pronunciations[0].symbol, "kæt");
        assert_eq!(a.explanations[0].definitions, vec!["кошка", "кот"]);
        let html = a.html.unwrap();
        assert!(html.contains("[kæt]") && html.contains("кошка<br>кот"), "{html}");
    }

    #[test]
    fn html_bodies_keep_their_markup() {
        let a = RawEntry::new("x", Body::Html("<b>one</b><br>two &amp; three".into())).to_article();
        assert_eq!(a.explanations[0].definitions, vec!["one", "two & three"]);
        assert_eq!(a.html.as_deref(), Some("<b>one</b><br>two &amp; three"));
    }

    #[test]
    fn examples_split_on_dashes() {
        let e = split_example("I run — я бегу");
        assert_eq!((e.source.as_str(), e.target.as_str()), ("I run", "я бегу"));
        assert_eq!(split_example("no translation").target, "");
    }
}
