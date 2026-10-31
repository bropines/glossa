//! XDXF dictionaries (`.xdxf`, optionally gzipped), visual format.
//!
//! ```xml
//! <xdxf lang_from="ENG" lang_to="RUS" format="visual">
//!   <full_name>Tiny</full_name>
//!   <ar><k>cat</k>
//!   <tr>kæt</tr>
//!   <pos><abr>n.</abr></pos> <dtrn>кошка</dtrn>
//!   <ex>a black cat — чёрная кошка</ex>
//!   see <kref>kitten</kref></ar>
//! </xdxf>
//! ```
//!
//! The file is decoded once and indexed by the `<k>` headwords of each
//! `<ar>`; an article is converted only when looked up. The same converter
//! reads the XDXF fragments StarDict stores in `x` fields. In the visual
//! format line breaks are significant, so they become `<br>` in the HTML.
//! `<opt>` inside a headword marks an optional part: the article is indexed
//! both with and without it.

use std::io;
use std::path::Path;

use glossa_core::dict::{Example, Explanation, Pronunciation};
use glossa_core::{Article, LangId};

use crate::html::{decode_entities, escape, Token, Tokenizer};
use crate::source::{
    decode_text, guess_lang_pair, invalid, lang_from_any, read_maybe_gzip, split_example, strip_numbering, tidy, Body,
    CardIndex, DictionarySource, RawEntry,
};

/// An opened XDXF dictionary, held in memory.
pub struct Xdxf {
    name: String,
    lang_pair: Option<(LangId, LangId)>,
    text: String,
    cards: CardIndex,
}

impl Xdxf {
    pub fn open(path: &Path) -> io::Result<Xdxf> {
        let bytes = read_maybe_gzip(path)?;
        let file_name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = file_name.split('.').next().unwrap_or("").to_owned();
        Xdxf::parse(&bytes, &stem).map_err(|detail| invalid(path, detail))
    }

    pub fn parse(bytes: &[u8], fallback_name: &str) -> Result<Xdxf, String> {
        let text = decode_text(bytes, false);
        if text.len() > u32::MAX as usize {
            return Err("file larger than 4 GiB".into());
        }
        let first_ar = find_ar(&text, 0).map_or(text.len(), |(tag, _)| tag);
        let head = &text[..first_ar];
        let mut name = String::new();
        let (mut from, mut to) = (None, None);
        let mut saw_root = false;
        let mut in_name = false;
        for tok in Tokenizer::xml(head) {
            match tok {
                Token::Start { name: tag, attrs, .. } => match tag.as_str() {
                    "xdxf" => {
                        saw_root = true;
                        for (k, v) in &attrs {
                            match k.as_str() {
                                "lang_from" => from = lang_from_any(v),
                                "lang_to" => to = lang_from_any(v),
                                _ => {}
                            }
                        }
                    }
                    "full_name" | "full_title" if name.is_empty() => in_name = true,
                    _ => {}
                },
                Token::End { name: tag } if tag == "full_name" || tag == "full_title" => in_name = false,
                Token::Text(t) if in_name => name.push_str(&decode_entities(t)),
                _ => {}
            }
        }
        if !saw_root {
            return Err("not an XDXF file (no <xdxf> element)".into());
        }

        let mut cards = CardIndex::default();
        let mut pos = 0;
        while let Some((_, content)) = find_ar(&text, pos) {
            let end = text[content..].find("</ar>").map_or(text.len(), |p| content + p);
            for (display, keys) in headwords(&text[content..end]) {
                cards.push(display, &keys, content, end);
            }
            pos = end.max(content + 1);
        }
        cards.finish();

        let mut name = tidy(&name);
        if name.is_empty() {
            name = fallback_name.to_owned();
        }
        let lang_pair = match (from, to) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => guess_lang_pair(&name).or_else(|| guess_lang_pair(fallback_name)),
        };
        Ok(Xdxf { name, lang_pair, text, cards })
    }
}

/// The next `<ar>` at or after `from`: (offset of `<`, offset after `>`).
fn find_ar(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut at = from;
    while let Some(p) = text.get(at..)?.find("<ar") {
        let tag = at + p;
        match bytes.get(tag + 3) {
            Some(b'>') => return Some((tag, tag + 4)),
            Some(b) if b.is_ascii_whitespace() => {
                let close = text[tag..].find('>').map_or(text.len(), |c| tag + c + 1);
                return Some((tag, close));
            }
            None => return None,
            _ => at = tag + 3,
        }
    }
    None
}

/// Each `<k>` of an article: its displayed text and index keys (with and
/// without `<opt>` parts).
fn headwords(article: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut depth_k = 0usize;
    let mut depth_opt = 0usize;
    let (mut full, mut short) = (String::new(), String::new());
    for tok in Tokenizer::xml(article) {
        match tok {
            Token::Start { name, self_closing: false, .. } if name == "k" => {
                depth_k += 1;
                full.clear();
                short.clear();
            }
            Token::Start { name, self_closing: false, .. } if name == "opt" && depth_k > 0 => depth_opt += 1,
            Token::End { name } if name == "opt" => depth_opt = depth_opt.saturating_sub(1),
            Token::End { name } if name == "k" && depth_k > 0 => {
                depth_k -= 1;
                let (f, s) = (tidy(&full), tidy(&short));
                if !f.is_empty() {
                    let mut keys = vec![f.clone()];
                    if s != f && !s.is_empty() {
                        keys.push(s);
                    }
                    out.push((f, keys));
                }
            }
            Token::Text(t) if depth_k > 0 => {
                let t = decode_entities(t);
                full.push_str(&t);
                if depth_opt == 0 {
                    short.push_str(&t);
                }
            }
            _ => {}
        }
    }
    out
}

impl DictionarySource for Xdxf {
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
                Some(RawEntry { headword: card.headword.to_string(), body: Body::Xdxf(body.to_owned()) })
            })
            .collect()
    }

    fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        self.cards.suggest(prefix, limit)
    }
}

// ---------------------------------------------------------------------------
// Articles
// ---------------------------------------------------------------------------

/// Colour names from `<c c="…">` go into an attribute; keep them inert.
fn sanitize_color(value: &str) -> String {
    let c: String = value.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '#').take(32).collect();
    if c.is_empty() {
        "green".into()
    } else {
        c
    }
}

struct Open {
    name: String,
    close: &'static str,
    link: Option<(bool, String)>,
}

/// Render an XDXF article fragment as HTML.
pub fn to_html(headword: &str, fragment: &str) -> String {
    to_article(headword, fragment).html.unwrap_or_default()
}

/// Convert an XDXF article fragment to an [`Article`].
pub(crate) fn to_article(headword: &str, fragment: &str) -> Article {
    let uses_dtrn = fragment.contains("<dtrn");
    let mut article = Article { headword: headword.to_owned(), ..Default::default() };
    let mut groups = vec![Explanation::default()];
    let mut html = String::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut line = String::new();
    let (mut dtrn, mut ex, mut ex_orig, mut ex_tran, mut tr, mut pos) =
        (String::new(), String::new(), String::new(), String::new(), String::new(), String::new());
    let has = |stack: &[Open], n: &str| stack.iter().any(|o| o.name == n);

    fn push_definition(groups: &mut [Explanation], text: &str) {
        let d = tidy(text);
        let d = strip_numbering(&d).trim_matches([';', ',']).trim();
        if !d.is_empty() && d.chars().any(char::is_alphanumeric) {
            if let Some(g) = groups.last_mut() {
                g.definitions.push(d.to_owned());
            }
        }
    }

    for tok in Tokenizer::xml(fragment) {
        match tok {
            Token::Start { name, attrs, self_closing } => {
                let attr = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                if name == "br" {
                    html.push_str("<br>");
                    line.push('\n');
                    continue;
                }
                if self_closing || stack.len() >= 64 {
                    continue;
                }
                let mut link = None;
                let (open, close): (String, &'static str) = match name.as_str() {
                    "b" | "i" | "u" | "sup" | "sub" | "big" | "small" => {
                        let close = match name.as_str() {
                            "b" => "</b>",
                            "i" => "</i>",
                            "u" => "</u>",
                            "sup" => "</sup>",
                            "sub" => "</sub>",
                            "big" => "</big>",
                            _ => "</small>",
                        };
                        (format!("<{name}>"), close)
                    }
                    "c" => (format!("<font color=\"{}\">", sanitize_color(attr("c").unwrap_or(""))), "</font>"),
                    "abr" | "pos" | "gr" => ("<i><font color=\"green\">".into(), "</font></i>"),
                    "tr" => ("[".into(), "]"),
                    "ex" => ("<font color=\"slategray\">".into(), "</font>"),
                    "co" => ("<i><font color=\"gray\">".into(), "</font></i>"),
                    "blockquote" => ("<div style=\"margin-left:1em\">".into(), "</div>"),
                    "def" => ("<div>".into(), "</div>"),
                    "kref" => {
                        link = Some((true, String::new()));
                        (String::new(), "")
                    }
                    "iref" => {
                        let href = attr("href").unwrap_or("").to_owned();
                        if href.is_empty() {
                            (String::new(), "")
                        } else {
                            (format!("<a href=\"{}\">", escape(&href)), "</a>")
                        }
                    }
                    _ => (String::new(), ""),
                };
                let suppressed = has(&stack, "k") || has(&stack, "rref") || has(&stack, "kref");
                if !suppressed {
                    html.push_str(&open);
                }
                stack.push(Open { name, close: if suppressed { "" } else { close }, link });
            }
            Token::End { name } => {
                let Some(at) = stack.iter().rposition(|o| o.name == name) else { continue };
                while stack.len() > at {
                    let o = stack.pop().expect("stack is longer than at");
                    match o.name.as_str() {
                        "dtrn" => {
                            if !has(&stack, "dtrn") {
                                push_definition(&mut groups, &dtrn);
                                dtrn.clear();
                            }
                        }
                        "ex" => {
                            let e = if !ex_orig.trim().is_empty() || !ex_tran.trim().is_empty() {
                                Example { source: tidy(&ex_orig), target: tidy(&ex_tran) }
                            } else {
                                split_example(&tidy(&ex))
                            };
                            if !e.source.is_empty() {
                                article.examples.push(e);
                            }
                            ex.clear();
                            ex_orig.clear();
                            ex_tran.clear();
                        }
                        "tr" => {
                            let symbol = tidy(&tr);
                            if !symbol.is_empty() && !article.pronunciations.iter().any(|p| p.symbol == symbol) {
                                article.pronunciations.push(Pronunciation { symbol, ..Default::default() });
                            }
                            tr.clear();
                        }
                        "pos" | "gr" => {
                            let p = tidy(&pos).trim_end_matches(['.', ',', ';']).trim().to_owned();
                            if !p.is_empty() && !has(&stack, "pos") && !has(&stack, "gr") {
                                let current = groups.last_mut().expect("groups is never empty");
                                if current.definitions.is_empty() && current.part_of_speech.is_empty() {
                                    current.part_of_speech = p;
                                } else if current.part_of_speech != p {
                                    groups.push(Explanation { part_of_speech: p, definitions: Vec::new() });
                                }
                                pos.clear();
                            }
                        }
                        _ => {}
                    }
                    if let Some((_, target)) = &o.link {
                        let target = tidy(target);
                        if !target.is_empty() && !has(&stack, "k") {
                            html.push_str(&format!(
                                "<a href=\"{}\">{}</a>",
                                escape(&format!("dict://{target}")),
                                escape(&target)
                            ));
                            if !article.associations.contains(&target) {
                                article.associations.push(target);
                            }
                        }
                    } else if !has(&stack, "k") && !has(&stack, "rref") && !has(&stack, "kref") {
                        html.push_str(o.close);
                    }
                }
            }
            Token::Text(raw) => {
                let text = decode_entities(raw);
                if has(&stack, "k") || has(&stack, "rref") {
                    continue;
                }
                if let Some(link) = stack.iter_mut().rev().find_map(|o| o.link.as_mut()) {
                    link.1.push_str(&text);
                } else {
                    // Line breaks are significant in the visual format; the
                    // indentation of a pretty-printed file is not.
                    let rendered: Vec<String> = text
                        .split('\n')
                        .enumerate()
                        .map(|(i, l)| escape(if i == 0 { l } else { l.trim_start() }))
                        .collect();
                    html.push_str(&rendered.join("<br>"));
                }
                let in_any = |names: &[&str]| names.iter().any(|n| has(&stack, n));
                if has(&stack, "dtrn") {
                    dtrn.push_str(&text);
                }
                if has(&stack, "ex") {
                    if has(&stack, "ex_orig") {
                        ex_orig.push_str(&text);
                    } else if has(&stack, "ex_tran") || has(&stack, "ex_transl") {
                        ex_tran.push_str(&text);
                    }
                    ex.push_str(&text);
                }
                if has(&stack, "tr") {
                    tr.push_str(&text);
                }
                if in_any(&["pos", "gr"]) {
                    pos.push_str(&text);
                }
                if !uses_dtrn && !in_any(&["ex", "tr", "pos", "gr", "co", "abr"]) {
                    for (i, piece) in text.split('\n').enumerate() {
                        if i > 0 {
                            push_definition(&mut groups, &line);
                            line.clear();
                        }
                        line.push_str(piece);
                    }
                }
            }
        }
    }
    if !uses_dtrn {
        push_definition(&mut groups, &line);
    }
    groups.retain(|g| !g.definitions.is_empty());
    article.explanations = groups;
    // Leading breaks come from the newline after `<k>…</k>`.
    let mut html = html.trim().to_owned();
    while let Some(rest) = html.strip_prefix("<br>") {
        html = rest.trim_start().to_owned();
    }
    article.html = Some(format!("<div class=\"xdxf\">{html}</div>"));
    article
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::testutil::{gzip, TempDir};

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!DOCTYPE xdxf SYSTEM "http://xdxf.sourceforge.net/xdxf_lousy.dtd">
<xdxf lang_from="ENG" lang_to="RUS" format="visual">
<full_name>Tiny &amp; Small</full_name>
<description>Test</description>
<ar><k>cat</k><k>pussy<opt>cat</opt></k>
<tr>kæt</tr>
<pos><abr>n.</abr></pos> <dtrn>кошка</dtrn>, <dtrn>кот</dtrn>
<ex>a black cat — чёрная кошка</ex>
<co>informal</co> see <kref>kitten</kref> <rref>cat.wav</rref></ar>
<ar><k>dog</k>
1) собака
2) <c c="red">пёс</c>
<ex><ex_orig>a good dog</ex_orig><ex_tran>хороший пёс</ex_tran></ex></ar>
<ar><k>catalogue</k>каталог</ar>
</xdxf>
"#;

    #[test]
    fn header_and_articles_are_indexed() {
        let d = Xdxf::parse(SAMPLE.as_bytes(), "x").unwrap();
        assert_eq!(d.name(), "Tiny & Small");
        assert_eq!(d.lang_pair(), Some((LangId::ENGLISH, LangId::RUSSIAN)));
        // cat, pussycat, dog, catalogue
        assert_eq!(d.entry_count(), 4);
        assert_eq!(d.lookup("PUSSY", true)[0].headword, "pussycat");
        assert_eq!(d.lookup("pussycat", true).len(), 1);
        let words: Vec<String> = d.lookup("cat", false).into_iter().map(|e| e.headword).collect();
        assert_eq!(words, vec!["cat", "catalogue"]);
        assert_eq!(d.suggest("ca", 5), vec!["cat", "catalogue"]);
    }

    #[test]
    fn article_fields_are_extracted() {
        let d = Xdxf::parse(SAMPLE.as_bytes(), "x").unwrap();
        let a = d.lookup("cat", true)[0].to_article();
        assert_eq!(a.pronunciations[0].symbol, "kæt");
        assert_eq!(a.explanations[0].part_of_speech, "n");
        assert_eq!(a.explanations[0].definitions, vec!["кошка", "кот"]);
        assert_eq!(a.examples[0].source, "a black cat");
        assert_eq!(a.examples[0].target, "чёрная кошка");
        assert_eq!(a.associations, vec!["kitten"]);
        let html = a.html.unwrap();
        assert!(!html.contains("cat.wav"), "{html}");
        assert!(!html.contains("pussy"), "headwords are not repeated in the body: {html}");
        assert!(html.contains("<a href=\"dict://kitten\">kitten</a>"), "{html}");
        let text = crate::html::to_plain_text(&html);
        assert!(text.starts_with("[kæt]"), "{text}");
    }

    #[test]
    fn visual_lines_become_definitions_without_dtrn() {
        let d = Xdxf::parse(SAMPLE.as_bytes(), "x").unwrap();
        let a = d.lookup("dog", true)[0].to_article();
        assert_eq!(a.explanations[0].definitions, vec!["собака", "пёс"]);
        assert_eq!(a.examples[0].source, "a good dog");
        assert_eq!(a.examples[0].target, "хороший пёс");
        let html = a.html.unwrap();
        assert!(html.contains("<font color=\"red\">пёс</font>"), "{html}");
        assert!(html.contains("1) собака<br>2)"), "{html}");
    }

    #[test]
    fn gzipped_file_opens() {
        let dir = TempDir::new("xdxf");
        let p = dir.write("tiny.xdxf.dz", &gzip(SAMPLE.as_bytes()));
        let d = Xdxf::open(&p).unwrap();
        assert_eq!(d.lookup("catalogue", true)[0].to_article().explanations[0].definitions, vec!["каталог"]);
    }

    #[test]
    fn broken_files_do_not_panic() {
        assert!(Xdxf::parse(b"just text", "x").is_err());
        // Truncated mid-article: what is there is still usable.
        let cut = &SAMPLE[..SAMPLE.find("<ex>a black").unwrap()];
        let d = Xdxf::parse(cut.as_bytes(), "x").unwrap();
        let a = d.lookup("cat", true)[0].to_article();
        assert_eq!(a.explanations[0].definitions, vec!["кошка", "кот"]);
        for junk in ["<xdxf><ar", "<xdxf><ar><k>", "<xdxf><ar ><k>a</k><kref>b", "<xdxf><arx><ar\t>"] {
            if let Ok(d) = Xdxf::parse(junk.as_bytes(), "x") {
                for e in d.lookup("a", false) {
                    let _ = e.to_article();
                }
            }
        }
        let _ = to_article("w", "</ex></tr><dtrn><dtrn>x</ex><c c='\"><script>'>y");
    }
}
