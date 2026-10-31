//! ABBYY Lingvo DSL dictionaries (`.dsl`, `.dsl.dz`).
//!
//! A DSL file is text — usually UTF-16LE with a BOM, sometimes UTF-8 or
//! UTF-16BE, and in old Russian dictionaries Windows-1251. It starts with
//! `#NAME`, `#INDEX_LANGUAGE`, `#CONTENTS_LANGUAGE` header lines, then cards:
//! one or more headword lines at column 0 followed by body lines indented by
//! a tab or spaces (shown as spaces below).
//!
//! ```text
//! #NAME "Tiny En-Ru"
//! colo(u)r
//! hue
//!     [m1][p]n[/p] [trn]цвет[/trn][/m]
//!     [m2][ex][lang id=1033]bright ~[/lang] — яркий цвет[/ex][/m]
//! ```
//!
//! The decoded text is stored once; the index maps each headword to the
//! byte range of its card body, and a body is parsed only when it is looked
//! up. DSL markup becomes both a structured [`Article`] (best-effort:
//! `[p]` → part of speech, `[trn]` or numbered `[mN]` lines → definitions,
//! `[ex]` → examples, `[t]` → pronunciation, `[ref]` → associations) and a
//! small, well-formed HTML rendering for [`Article::html`]. Media (`[s]`) is
//! dropped. Links to other headwords use the `dict://` scheme.

use std::io;
use std::path::Path;

use glossa_core::dict::{Explanation, Pronunciation};
use glossa_core::{Article, LangId};

use crate::html::escape;
use crate::source::{
    decode_text, guess_lang_pair, invalid, lang_from_any, read_maybe_gzip, split_example, strip_numbering, tidy, Body,
    CardIndex, DictionarySource, RawEntry,
};

/// An opened DSL dictionary, held in memory.
pub struct Dsl {
    name: String,
    lang_pair: Option<(LangId, LangId)>,
    text: String,
    cards: CardIndex,
}

impl Dsl {
    pub fn open(path: &Path) -> io::Result<Dsl> {
        let bytes = read_maybe_gzip(path)?;
        let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = file_name.split('.').next().unwrap_or("").to_owned();
        Dsl::parse(&bytes, &stem).map_err(|detail| invalid(path, detail))
    }

    /// Parse a DSL file's bytes. `fallback_name` names the dictionary when
    /// it has no `#NAME`.
    pub fn parse(bytes: &[u8], fallback_name: &str) -> Result<Dsl, String> {
        // The header is ASCII in every 8-bit encoding, so it can be sniffed
        // before decoding.
        let cyrillic = contains(bytes, b"Russian") || contains(bytes, b"Cyrillic") || contains(bytes, b"Ukrainian");
        let text = decode_text(bytes, cyrillic);
        if text.len() > u32::MAX as usize {
            return Err("file larger than 4 GiB".into());
        }
        let mut name = String::new();
        let (mut from, mut to) = (None, None);
        let mut saw_header = false;
        let mut cards = CardIndex::default();
        let mut pending: Vec<String> = Vec::new();
        let mut body: Option<(usize, usize)> = None;
        let mut in_header = true;

        let flush = |pending: &mut Vec<String>, body: &mut Option<(usize, usize)>, cards: &mut CardIndex| {
            if let Some((start, end)) = body.take() {
                for line in pending.drain(..) {
                    let (display, keys) = headword_forms(&line);
                    cards.push(display, &keys, start, end);
                }
            }
        };

        let mut offset = 0;
        for raw_line in text.split_inclusive('\n') {
            let start = offset;
            offset += raw_line.len();
            let line = raw_line.trim_end_matches(['\n', '\r']);
            if in_header && line.starts_with('#') {
                saw_header = true;
                let (key, value) = line.split_once([' ', '\t']).unwrap_or((line, ""));
                let value = value.trim().trim_matches('"').trim();
                match key.to_ascii_uppercase().as_str() {
                    "#NAME" => name = value.to_owned(),
                    "#INDEX_LANGUAGE" => from = lang_from_any(value),
                    "#CONTENTS_LANGUAGE" => to = lang_from_any(value),
                    _ => {}
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            in_header = false;
            let first = line.chars().next().unwrap_or(' ');
            if first == ' ' || first == '\t' {
                if pending.is_empty() {
                    continue; // a body with no headword: nothing can find it
                }
                let end = start + line.len();
                body = Some(match body {
                    Some((s, _)) => (s, end),
                    None => (start, end),
                });
            } else {
                if line.trim_start().starts_with("{{") && line.trim_end().ends_with("}}") {
                    continue; // a comment line
                }
                if body.is_some() {
                    flush(&mut pending, &mut body, &mut cards);
                }
                let (display, _) = headword_forms(line);
                if !display.is_empty() {
                    pending.push(line.to_owned());
                }
            }
        }
        flush(&mut pending, &mut body, &mut cards);

        if cards.is_empty() && !saw_header {
            return Err("not a DSL dictionary (no header and no cards)".into());
        }
        cards.finish();
        if name.is_empty() {
            name = fallback_name.to_owned();
        }
        let lang_pair = match (from, to) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => guess_lang_pair(&name).or_else(|| guess_lang_pair(fallback_name)),
        };
        Ok(Dsl { name, lang_pair, text, cards })
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The displayed headword and its index keys. `{…}` is an unsorted part
/// (shown, not indexed), `(…)` an optional part (indexed with and without),
/// `\x` an escaped character.
fn headword_forms(line: &str) -> (String, Vec<String>) {
    let mut display = String::new();
    let mut with = String::new();
    let mut without = String::new();
    let (mut in_brace, mut in_paren, mut in_tag) = (false, false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        let literal = match c {
            '\\' => chars.next(),
            '{' if !in_brace => {
                in_brace = true;
                None
            }
            '}' if in_brace => {
                in_brace = false;
                in_tag = false;
                None
            }
            '[' if in_brace => {
                in_tag = true;
                None
            }
            ']' if in_tag => {
                in_tag = false;
                None
            }
            '(' if !in_brace && !in_paren => {
                in_paren = true;
                display.push('(');
                None
            }
            ')' if in_paren => {
                in_paren = false;
                display.push(')');
                None
            }
            c => Some(c),
        };
        let Some(c) = literal else { continue };
        if in_tag {
            continue;
        }
        display.push(c);
        if !in_brace {
            with.push(c);
            if !in_paren {
                without.push(c);
            }
        }
    }
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut keys = vec![collapse(&with)];
    let without = collapse(&without);
    if without != keys[0] && !without.is_empty() {
        keys.push(without);
    }
    keys.retain(|k| !k.is_empty());
    (collapse(&display), keys)
}

impl DictionarySource for Dsl {
    fn name(&self) -> &str {
        &self.name
    }

    fn lang_pair(&self) -> Option<(LangId, LangId)> {
        self.lang_pair
    }

    fn entry_count(&self) -> usize {
        self.cards.len()
    }

    fn lookup(&self, word: &str, exact: bool) -> Vec<RawEntry> {
        self.cards
            .lookup(word, exact)
            .into_iter()
            .filter_map(|card| {
                let body = self.text.get(card.start as usize..card.end as usize)?;
                Some(RawEntry { headword: card.headword.to_string(), body: Body::Dsl(body.to_owned()) })
            })
            .collect()
    }

    fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.cards.suggest(prefix, limit)
    }
}

// ---------------------------------------------------------------------------
// Card bodies
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Tok {
    Text(String),
    Open(String, String),
    Close(String),
}

/// Split one body line into text and tags. `~` stands for the headword;
/// `{{…}}` is a comment; `<<word>>` is shorthand for `[ref]word[/ref]`.
fn tokenize(line: &str, headword: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<Tok>| {
        if !text.is_empty() {
            out.push(Tok::Text(std::mem::take(text)));
        }
    };
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < line.len() {
        let rest = &line[i..];
        let c = rest.chars().next().unwrap_or(' ');
        match c {
            '\\' => {
                let next = rest[1..].chars().next();
                match next {
                    Some(n) => {
                        text.push(n);
                        i += 1 + n.len_utf8();
                    }
                    None => {
                        text.push('\\');
                        i += 1;
                    }
                }
                continue;
            }
            '[' => {
                if let Some(close) = find_unescaped(rest, b']') {
                    let inner = &rest[1..close];
                    if !inner.is_empty() && inner.len() < 64 && !inner.contains('[') {
                        flush(&mut text, &mut out);
                        if let Some(name) = inner.strip_prefix('/') {
                            out.push(Tok::Close(name.trim().to_ascii_lowercase()));
                        } else {
                            let (name, attr) = inner.split_once([' ', '\t']).unwrap_or((inner, ""));
                            out.push(Tok::Open(name.trim().to_ascii_lowercase(), attr.trim().to_owned()));
                        }
                        i += close + 1;
                        continue;
                    }
                }
            }
            '{' if bytes.get(i + 1) == Some(&b'{') => {
                i = match rest.find("}}") {
                    Some(p) => i + p + 2,
                    None => line.len(),
                };
                continue;
            }
            '<' if bytes.get(i + 1) == Some(&b'<') => {
                if let Some(p) = rest.find(">>") {
                    flush(&mut text, &mut out);
                    out.push(Tok::Open("ref".into(), String::new()));
                    out.push(Tok::Text(rest[2..p].to_owned()));
                    out.push(Tok::Close("ref".into()));
                    i += p + 2;
                    continue;
                }
            }
            '~' => {
                text.push_str(headword);
                i += 1;
                continue;
            }
            _ => {}
        }
        text.push(c);
        i += c.len_utf8();
    }
    flush(&mut text, &mut out);
    out
}

/// Byte offset of `target` in `s`, skipping `\`-escaped characters.
fn find_unescaped(s: &str, target: u8) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Colour names in `[c …]` go straight into a `color` attribute, so only
/// characters that cannot break out of it survive.
fn sanitize_color(attr: &str) -> String {
    let c: String = attr.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '#').take(32).collect();
    if c.is_empty() {
        "green".into()
    } else {
        c
    }
}

/// What one body line yields.
#[derive(Default)]
struct Line {
    html: String,
    margin: u8,
    /// Text outside `[p]`, `[ex]`, `[t]`, `[com]` and media: the definition
    /// when the dictionary does not use `[trn]`.
    rest: String,
    trn: String,
    ex: String,
    pos: String,
    transcription: String,
    refs: Vec<String>,
}

struct Open {
    name: String,
    close: &'static str,
    link: String,
}

fn render_line(line: &str, headword: &str) -> Line {
    let mut out = Line::default();
    let mut stack: Vec<Open> = Vec::new();
    let has = |stack: &[Open], n: &str| stack.iter().any(|o| o.name == n);

    fn close(o: Open, out: &mut Line) {
        match o.name.as_str() {
            "ref" | "url" => {
                let target = o.link.trim().to_owned();
                if !target.is_empty() {
                    let href = if o.name == "ref" { format!("dict://{target}") } else { target.clone() };
                    out.html.push_str(&format!("<a href=\"{}\">{}</a>", escape(&href), escape(&target)));
                    if o.name == "ref" && !out.refs.contains(&target) {
                        out.refs.push(target);
                    }
                }
            }
            _ => out.html.push_str(o.close),
        }
    }

    for tok in tokenize(line, headword) {
        match tok {
            Tok::Open(name, attr) => {
                let margin = name
                    .strip_prefix('m')
                    .filter(|d| d.chars().all(|c| c.is_ascii_digit()))
                    .map(|d| d.parse::<u8>().unwrap_or(1).min(9));
                if let Some(m) = margin {
                    out.margin = if name == "m" { 1 } else { m };
                    continue;
                }
                if name == "br" {
                    out.html.push_str("<br>");
                    continue;
                }
                if stack.len() >= 32 {
                    continue;
                }
                let (open, close): (String, &'static str) = match name.as_str() {
                    "b" => ("<b>".into(), "</b>"),
                    "i" => ("<i>".into(), "</i>"),
                    "u" | "'" => ("<u>".into(), "</u>"),
                    "sup" => ("<sup>".into(), "</sup>"),
                    "sub" => ("<sub>".into(), "</sub>"),
                    "c" => (format!("<font color=\"{}\">", sanitize_color(&attr)), "</font>"),
                    "p" => ("<i><font color=\"green\">".into(), "</font></i>"),
                    "ex" => ("<font color=\"slategray\">".into(), "</font>"),
                    "com" => ("<i><font color=\"gray\">".into(), "</font></i>"),
                    "trn" => {
                        if !out.trn.is_empty() {
                            out.trn.push_str("; ");
                        }
                        (String::new(), "")
                    }
                    _ => (String::new(), ""),
                };
                let in_link = has(&stack, "ref") || has(&stack, "url");
                if !in_link && !has(&stack, "s") {
                    out.html.push_str(&open);
                }
                stack.push(Open { name, close: if in_link { "" } else { close }, link: String::new() });
            }
            Tok::Close(name) => {
                if name.starts_with('m') && name[1..].chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if let Some(at) = stack.iter().rposition(|o| o.name == name) {
                    while stack.len() > at {
                        let o = stack.pop().expect("stack is longer than at");
                        if !has(&stack, "s") {
                            close(o, &mut out);
                        }
                    }
                }
            }
            Tok::Text(t) => {
                if has(&stack, "s") || has(&stack, "video") {
                    continue;
                }
                if let Some(link) = stack.iter_mut().rev().find(|o| o.name == "ref" || o.name == "url") {
                    link.link.push_str(&t);
                } else {
                    out.html.push_str(&escape(&t));
                }
                let (p, ex, tr, com) = (has(&stack, "p"), has(&stack, "ex"), has(&stack, "t"), has(&stack, "com"));
                if has(&stack, "trn") {
                    out.trn.push_str(&t);
                }
                if ex {
                    out.ex.push_str(&t);
                }
                if p {
                    out.pos.push_str(&t);
                }
                if tr {
                    out.transcription.push_str(&t);
                }
                if !(p || ex || tr || com) {
                    out.rest.push_str(&t);
                }
            }
        }
    }
    while let Some(o) = stack.pop() {
        if !has(&stack, "s") {
            close(o, &mut out);
        }
    }
    out
}

/// Render a card body as HTML.
pub fn to_html(headword: &str, body: &str) -> String {
    to_article(headword, body).html.unwrap_or_default()
}

/// Convert a card body to an [`Article`].
pub(crate) fn to_article(headword: &str, body: &str) -> Article {
    let uses_trn = body.contains("[trn]");
    let mut article = Article { headword: headword.to_owned(), ..Default::default() };
    let mut groups = vec![Explanation::default()];
    let mut html = String::from("<div class=\"dsl\">");
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(sub) = line.strip_prefix('@') {
            // A sub-entry headword inside the card.
            let sub = sub.trim();
            if !sub.is_empty() {
                html.push_str(&format!("<div><b>{}</b></div>", escape(&tidy(sub))));
            }
            continue;
        }
        let l = render_line(line, headword);
        if !l.html.trim().is_empty() {
            if l.margin > 0 {
                html.push_str(&format!("<div style=\"margin-left:{}em\">{}</div>", l.margin, l.html));
            } else {
                html.push_str(&format!("<div>{}</div>", l.html));
            }
        }
        let pos = tidy(&l.pos);
        if !pos.is_empty() {
            let current = groups.last_mut().expect("groups is never empty");
            if current.definitions.is_empty() && current.part_of_speech.is_empty() {
                current.part_of_speech = pos;
            } else if current.part_of_speech != pos {
                groups.push(Explanation { part_of_speech: pos, definitions: Vec::new() });
            }
        }
        let transcription = tidy(&l.transcription);
        let transcription = transcription.trim_matches(['[', ']', '/']).trim();
        if !transcription.is_empty() && !article.pronunciations.iter().any(|p| p.symbol == transcription) {
            article.pronunciations.push(Pronunciation { symbol: transcription.to_owned(), ..Default::default() });
        }
        let ex = tidy(&l.ex);
        if !ex.is_empty() {
            article.examples.push(split_example(&ex));
        }
        let definition = if uses_trn { tidy(&l.trn) } else { tidy(&l.rest) };
        let definition = strip_numbering(&definition).trim_matches([';', ',']).trim();
        if !definition.is_empty() && definition.chars().any(char::is_alphanumeric) {
            groups.last_mut().expect("groups is never empty").definitions.push(definition.to_owned());
        }
        for r in l.refs {
            if !article.associations.contains(&r) {
                article.associations.push(r);
            }
        }
    }
    html.push_str("</div>");
    groups.retain(|g| !g.definitions.is_empty());
    article.explanations = groups;
    article.html = Some(html);
    article
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::testutil::{gzip, utf16le_with_bom, TempDir};

    const SAMPLE: &str =
        "#NAME\t\"Tiny En-Ru\"\r\n#INDEX_LANGUAGE\t\"English\"\r\n#CONTENTS_LANGUAGE\t\"Russian\"\r\n\r\n\
colo(u)r\r\n\
hue\r\n\
\t[m0][b]colour[/b] \\[[t]ˈkʌlə[/t]\\][/m]\r\n\
\t[m1][p]n[/p] [trn]цвет[/trn]; [trn]оттенок[/trn][/m]\r\n\
\t[m2][ex][lang id=1033]bright ~[/lang] — яркий цвет[/ex][/m]\r\n\
\t[m1][p]v[/p] [trn]красить[/trn] [com](разг.)[/com][/m]\r\n\
\t[m1]см. [ref]tint[/ref] [s]colour.wav[/s][/m]\r\n\
\r\n\
go{ (somewhere)}\r\n\
\t[m1]1) идти[/m]\r\n\
\t[m1]2) [c red]ехать[/c][/m]\r\n\
\t{{a comment}}\r\n\
\t[m1][ex]go home — идти домой[/ex][/m]\r\n\
run\r\n\
\t[m1]бежать <<sprint>>[/m]\r\n";

    fn sample() -> Dsl {
        Dsl::parse(&utf16le_with_bom(SAMPLE), "tiny").unwrap()
    }

    #[test]
    fn header_and_cards_are_read_from_utf16() {
        let d = sample();
        assert_eq!(d.name(), "Tiny En-Ru");
        assert_eq!(d.lang_pair(), Some((LangId::ENGLISH, LangId::RUSSIAN)));
        // colo(u)r, hue, go, run
        assert_eq!(d.entry_count(), 4);
    }

    #[test]
    fn several_headwords_share_one_body() {
        let d = sample();
        let a = d.lookup("hue", true);
        let b = d.lookup("COLOR", true);
        let c = d.lookup("colour", true);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].body, b[0].body);
        assert_eq!(b[0].body, c[0].body);
        assert_eq!(b[0].headword, "colo(u)r");
        assert_eq!(a[0].headword, "hue");
    }

    #[test]
    fn unsorted_parts_are_shown_but_not_indexed() {
        let d = sample();
        let hits = d.lookup("go", true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].headword, "go (somewhere)");
        assert!(d.lookup("go (somewhere)", true).is_empty());
    }

    #[test]
    fn prefix_lookup_and_suggestions() {
        let d = sample();
        // "colo(u)r" and "hue" share a body, so "colo" yields it once.
        assert_eq!(d.lookup("colo", false).len(), 1);
        assert_eq!(d.suggest("r", 5), vec!["run"]);
        assert!(d.lookup("xyz", false).is_empty());
    }

    #[test]
    fn markup_becomes_an_article() {
        let d = sample();
        let a = d.lookup("colour", true)[0].to_article();
        assert_eq!(a.headword, "colo(u)r");
        assert_eq!(a.pronunciations[0].symbol, "ˈkʌlə");
        assert_eq!(a.explanations.len(), 2);
        assert_eq!(a.explanations[0].part_of_speech, "n");
        assert_eq!(a.explanations[0].definitions, vec!["цвет; оттенок"]);
        assert_eq!(a.explanations[1].part_of_speech, "v");
        assert_eq!(a.explanations[1].definitions, vec!["красить"]);
        assert_eq!(a.examples[0].source, "bright colo(u)r");
        assert_eq!(a.examples[0].target, "яркий цвет");
        assert_eq!(a.associations, vec!["tint"]);
        let html = a.html.unwrap();
        assert!(html.contains("<b>colour</b>"), "{html}");
        assert!(html.contains("<a href=\"dict://tint\">tint</a>"), "{html}");
        assert!(!html.contains("wav"), "media must be dropped: {html}");
        assert!(html.contains("margin-left:2em"), "{html}");
        // The HTML reads back cleanly.
        let text = crate::html::to_plain_text(&html);
        assert!(text.contains("[ˈkʌlə]"), "{text}");
        assert!(text.contains("bright colo(u)r — яркий цвет"), "{text}");
    }

    #[test]
    fn numbered_lines_without_trn_are_definitions() {
        let d = sample();
        let a = d.lookup("go", true)[0].to_article();
        assert_eq!(a.explanations[0].definitions, vec!["идти", "ехать"]);
        assert_eq!(a.examples.len(), 1);
        let html = a.html.unwrap();
        assert!(html.contains("<font color=\"red\">ехать</font>"), "{html}");
        assert!(!html.contains("comment"), "{html}");
        let run = d.lookup("run", true)[0].to_article();
        assert_eq!(run.associations, vec!["sprint"]);
    }

    #[test]
    fn utf8_gzip_and_cp1251_files_open() {
        let dir = TempDir::new("dsl");
        let utf8 = "\u{feff}#NAME \"U\"\nслово\n\tword\n";
        let p = dir.write("u.dsl.dz", &gzip(utf8.as_bytes()));
        let d = Dsl::open(&p).unwrap();
        assert_eq!(d.lookup("СЛОВО", true)[0].body, Body::Dsl("\tword".into()));

        let mut cp = b"#NAME \"C\"\n#INDEX_LANGUAGE \"Russian\"\n".to_vec();
        cp.extend_from_slice(&[0xF1, 0xEB, 0xEE, 0xE2, 0xEE, b'\n', b'\t', b'w', b'\n']); // "слово"
        let d = Dsl::parse(&cp, "c").unwrap();
        assert_eq!(d.lookup("слово", true).len(), 1);
    }

    #[test]
    fn garbage_is_an_error_or_empty_not_a_panic() {
        assert!(Dsl::parse(b"", "x").is_err());
        assert!(Dsl::parse(b"\t\tbody without headword\n", "x").is_err());
        let mut state = 99u32;
        for len in [1usize, 5, 50, 500, 5000] {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (state >> 16) as u8
                })
                .collect();
            if let Ok(d) = Dsl::parse(&junk, "x") {
                for e in d.lookup("a", false) {
                    let _ = e.to_article();
                }
            }
        }
        // Broken markup in a body.
        let a = to_article("w", "\t[b]unclosed [c [ref]x [/i][/ref]] \\ [s]a\n\t[[[]]] ~ {{ <<a");
        assert!(a.html.is_some());
    }

    #[test]
    fn tokenizer_handles_escapes_and_shorthands() {
        assert_eq!(
            tokenize("a\\[b\\] ~ [i]x[/i]", "hw"),
            vec![
                Tok::Text("a[b] hw ".into()),
                Tok::Open("i".into(), String::new()),
                Tok::Text("x".into()),
                Tok::Close("i".into())
            ]
        );
        assert_eq!(tokenize("{{c}}x", ""), vec![Tok::Text("x".into())]);
        assert_eq!(tokenize("[c  darkred]", ""), vec![Tok::Open("c".into(), "darkred".into())]);
        assert_eq!(sanitize_color("red\" onclick=\"x"), "redonclickx");
    }
}
