//! HTML → rich-text blocks, for showing dictionary articles without a web
//! engine.
//!
//! Classic dictionary services (and StarDict `h` fields, and the HTML we
//! generate from DSL and XDXF) answer with an HTML fragment. The GUI is
//! immediate-mode and has no browser, so it needs the article reduced to what
//! it can actually draw: paragraphs of styled spans, headings, list items,
//! rules, quotes and simple tables. That reduction is this module.
//!
//! The input is whatever a scraper caught, so the parser is deliberately a
//! tag-soup reader rather than a conforming HTML5 parser: unclosed and
//! misnested tags, unquoted attributes, stray `<`, unterminated comments and
//! half a document are all normal. The one hard rule is that no input, however
//! broken, may panic — every slice is cut at an ASCII delimiter, so it always
//! falls on a character boundary.
//!
//! Whitespace collapses the way a browser collapses it (runs become one space,
//! leading and trailing space in a block disappears) except inside `<pre>`.
//! A `<br>` becomes a `'\n'` inside the current block's text rather than a new
//! block, because dictionary HTML uses `<br>` as a line break within one sense
//! and a block boundary would add paragraph spacing between every line.

use std::borrow::Cow;

use glossa_core::Color;

/// One block of rich text.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `<h1>`…`<h6>`; `level` is 1–6.
    Heading { level: u8, spans: Vec<Span> },
    Paragraph(Vec<Span>),
    /// A `<li>`. `depth` is 0 for a top-level list; `ordered_index` is the
    /// item's number in an `<ol>`, `None` for a bullet.
    ListItem { depth: u32, ordered_index: Option<u32>, spans: Vec<Span> },
    /// `<hr>`.
    Rule,
    /// Text inside `<blockquote>`.
    Quote(Vec<Span>),
    /// Rows of cells of spans. Nested tables are flattened into their cell.
    Table(Vec<Vec<Vec<Span>>>),
}

/// A run of text in one style. `text` may contain `'\n'` (from `<br>`, or a
/// line of `<pre>`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub monospace: bool,
    pub link: Option<String>,
    pub color: Option<Color>,
    pub small: bool,
    pub sup: bool,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Span { text: text.into(), ..Default::default() }
    }
}

/// Reduce an HTML fragment (or a whole document) to blocks.
pub fn to_blocks(html: &str) -> Vec<Block> {
    let mut b = Builder::default();
    for token in Tokenizer::html(html) {
        match token {
            Token::Text(raw) => {
                let text = decode_entities(raw);
                b.text(&text);
            }
            Token::Start { name, attrs, .. } => b.start(&name, &attrs),
            Token::End { name } => b.end(&name),
        }
    }
    b.finish()
}

/// The text of an HTML fragment, one block per line: what goes on the
/// clipboard when the user copies a dictionary article.
pub fn to_plain_text(html: &str) -> String {
    blocks_to_plain_text(&to_blocks(html))
}

/// Plain text of already-parsed blocks.
pub fn blocks_to_plain_text(blocks: &[Block]) -> String {
    fn spans(s: &[Span]) -> String {
        s.iter().map(|s| s.text.as_str()).collect::<String>().replace('\u{a0}', " ")
    }
    let mut lines = Vec::new();
    for block in blocks {
        match block {
            Block::Heading { spans: s, .. } | Block::Paragraph(s) | Block::Quote(s) => lines.push(spans(s)),
            Block::ListItem { depth, ordered_index, spans: s } => {
                let indent = "  ".repeat(*depth as usize);
                let marker = match ordered_index {
                    Some(n) => format!("{n}. "),
                    None => "• ".to_owned(),
                };
                lines.push(format!("{indent}{marker}{}", spans(s)));
            }
            Block::Rule => lines.push("────────".to_owned()),
            Block::Table(rows) => {
                for row in rows {
                    let cells: Vec<String> = row.iter().map(|c| spans(c).replace('\n', " ")).collect();
                    lines.push(cells.join("\t"));
                }
            }
        }
    }
    lines.join("\n")
}

/// Escape text for inclusion in HTML (element content or a quoted attribute).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One piece of tag soup. Text is raw (entities not yet decoded); tag and
/// attribute names are lowercased, attribute values decoded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token<'a> {
    Text(&'a str),
    Start { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    End { name: String },
}

/// A forgiving tokenizer shared by the HTML reader and the XDXF parser (XDXF
/// is XML, which this reads well enough; it just never switches to raw-text
/// mode there).
pub(crate) struct Tokenizer<'a> {
    src: &'a str,
    pos: usize,
    /// In HTML mode the content of `<script>`, `<style>` and friends is
    /// skipped wholesale, the way a browser treats it as raw text.
    html: bool,
    raw_until: Option<&'static str>,
}

impl<'a> Tokenizer<'a> {
    pub(crate) fn html(src: &'a str) -> Self {
        Tokenizer { src, pos: 0, html: true, raw_until: None }
    }

    pub(crate) fn xml(src: &'a str) -> Self {
        Tokenizer { src, pos: 0, html: false, raw_until: None }
    }

    fn bytes(&self) -> &'a [u8] {
        self.src.as_bytes()
    }

    /// Byte offset of the next `needle` (ASCII) at or after `from`.
    fn find_at(&self, from: usize, needle: &[u8]) -> Option<usize> {
        let hay = self.bytes();
        if from >= hay.len() || needle.is_empty() {
            return None;
        }
        hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
    }

    /// Like [`Self::find_at`], ignoring ASCII case.
    fn find_ci_at(&self, from: usize, needle: &[u8]) -> Option<usize> {
        let hay = self.bytes();
        if from >= hay.len() || needle.is_empty() {
            return None;
        }
        hay[from..].windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle)).map(|p| p + from)
    }

    fn is_name_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.')
    }

    fn read_name(&mut self) -> String {
        let bytes = self.bytes();
        let start = self.pos;
        while self.pos < bytes.len() && Self::is_name_byte(bytes[self.pos]) {
            self.pos += 1;
        }
        self.src[start..self.pos].to_ascii_lowercase()
    }

    fn skip_ws(&mut self) {
        let bytes = self.bytes();
        while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Parse a start tag; `self.pos` is just past the `<`. `None` if the
    /// input ends inside the tag, which a browser drops too.
    fn start_tag(&mut self) -> Option<Token<'a>> {
        let name = self.read_name();
        let mut attrs = Vec::new();
        let mut self_closing = false;
        let bytes = self.bytes();
        loop {
            self.skip_ws();
            let b = *bytes.get(self.pos)?;
            match b {
                b'>' => {
                    self.pos += 1;
                    break;
                }
                b'/' => {
                    self.pos += 1;
                    self_closing = bytes.get(self.pos) == Some(&b'>');
                }
                _ => {
                    let start = self.pos;
                    while self.pos < bytes.len()
                        && !bytes[self.pos].is_ascii_whitespace()
                        && !matches!(bytes[self.pos], b'=' | b'>' | b'/')
                    {
                        self.pos += 1;
                    }
                    if self.pos == start {
                        // A lone `=` or similar: step over it.
                        self.pos += 1;
                        continue;
                    }
                    let key = self.src[start..self.pos].to_ascii_lowercase();
                    self.skip_ws();
                    let mut value = String::new();
                    if bytes.get(self.pos) == Some(&b'=') {
                        self.pos += 1;
                        self.skip_ws();
                        match bytes.get(self.pos) {
                            Some(&q) if q == b'"' || q == b'\'' => {
                                let vstart = self.pos + 1;
                                let vend = self.find_at(vstart, &[q]).unwrap_or(bytes.len());
                                value = decode_entities(&self.src[vstart.min(vend)..vend]).into_owned();
                                self.pos = (vend + 1).min(bytes.len());
                            }
                            Some(_) => {
                                let vstart = self.pos;
                                while self.pos < bytes.len()
                                    && !bytes[self.pos].is_ascii_whitespace()
                                    && bytes[self.pos] != b'>'
                                {
                                    self.pos += 1;
                                }
                                value = decode_entities(&self.src[vstart..self.pos]).into_owned();
                            }
                            None => return None,
                        }
                    }
                    if attrs.len() < 64 {
                        attrs.push((key, value));
                    }
                }
            }
        }
        if self.html && !self_closing {
            self.raw_until = match name.as_str() {
                "script" => Some("</script"),
                "style" => Some("</style"),
                "title" => Some("</title"),
                "textarea" => Some("</textarea"),
                "noscript" => Some("</noscript"),
                "template" => Some("</template"),
                "svg" => Some("</svg"),
                "math" => Some("</math"),
                _ => None,
            };
        }
        Some(Token::Start { name, attrs, self_closing })
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let bytes = self.bytes();
        loop {
            if self.pos >= bytes.len() {
                return None;
            }
            if let Some(end) = self.raw_until.take() {
                // Skip everything up to the closing tag, which is then read
                // normally.
                self.pos = self.find_ci_at(self.pos, end.as_bytes()).unwrap_or(bytes.len());
                continue;
            }
            if bytes[self.pos] != b'<' {
                let start = self.pos;
                self.pos = self.find_at(self.pos, b"<").unwrap_or(bytes.len());
                return Some(Token::Text(&self.src[start..self.pos]));
            }
            // At a `<`.
            let next = bytes.get(self.pos + 1).copied();
            match next {
                Some(b'!') => {
                    if bytes[self.pos..].starts_with(b"<!--") {
                        self.pos = match self.find_at(self.pos + 4, b"-->") {
                            Some(p) => p + 3,
                            None => bytes.len(),
                        };
                        continue;
                    }
                    if bytes[self.pos..].starts_with(b"<![CDATA[") {
                        let start = self.pos + 9;
                        let end = self.find_at(start, b"]]>").unwrap_or(bytes.len());
                        self.pos = (end + 3).min(bytes.len());
                        return Some(Token::Text(&self.src[start.min(end)..end]));
                    }
                    // <!DOCTYPE …> and other bogus comments.
                    self.pos = self.find_at(self.pos, b">").map_or(bytes.len(), |p| p + 1);
                    continue;
                }
                Some(b'?') => {
                    self.pos = self.find_at(self.pos, b">").map_or(bytes.len(), |p| p + 1);
                    continue;
                }
                Some(b'/') => {
                    let after = bytes.get(self.pos + 2).copied();
                    if after.is_some_and(|b| b.is_ascii_alphabetic()) {
                        self.pos += 2;
                        let name = self.read_name();
                        self.pos = self.find_at(self.pos, b">").map_or(bytes.len(), |p| p + 1);
                        return Some(Token::End { name });
                    }
                    // `</>` or `</ junk>`: a bogus comment.
                    self.pos = self.find_at(self.pos, b">").map_or(bytes.len(), |p| p + 1);
                    continue;
                }
                Some(b) if b.is_ascii_alphabetic() => {
                    self.pos += 1;
                    match self.start_tag() {
                        Some(t) => return Some(t),
                        None => {
                            self.pos = bytes.len();
                            return None;
                        }
                    }
                }
                _ => {
                    // A literal `<`, as in "a < b".
                    let start = self.pos;
                    self.pos += 1;
                    return Some(Token::Text(&self.src[start..self.pos]));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

/// Names for U+00A0…U+00FF, in order.
const LATIN1_NAMES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not",
    "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1",
    "ordm", "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml",
    "Aring", "AElig", "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH",
    "Ntilde", "Ograve", "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc",
    "Uuml", "Yacute", "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig",
    "ccedil", "egrave", "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde",
    "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml",
    "yacute", "thorn", "yuml",
];

/// Greek capitals from U+0391 (U+03A2 is unassigned) and small letters from
/// U+03B1; dictionaries use them in etymologies and maths.
const GREEK_UPPER: [&str; 24] = [
    "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "Iota", "Kappa", "Lambda", "Mu", "Nu",
    "Xi", "Omicron", "Pi", "Rho", "Sigma", "Tau", "Upsilon", "Phi", "Chi", "Psi", "Omega",
];
const GREEK_LOWER: [&str; 25] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu", "nu",
    "xi", "omicron", "pi", "rho", "sigmaf", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega",
];

const OTHER_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("mdash", '—'),
    ("ndash", '–'),
    ("hellip", '…'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("sbquo", '‚'),
    ("bdquo", '„'),
    ("lsaquo", '‹'),
    ("rsaquo", '›'),
    ("bull", '•'),
    ("trade", '™'),
    ("euro", '€'),
    ("dagger", '†'),
    ("Dagger", '‡'),
    ("permil", '‰'),
    ("prime", '′'),
    ("Prime", '″'),
    ("oelig", 'œ'),
    ("OElig", 'Œ'),
    ("scaron", 'š'),
    ("Scaron", 'Š'),
    ("Yuml", 'Ÿ'),
    ("fnof", 'ƒ'),
    ("circ", 'ˆ'),
    ("tilde", '˜'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200c}'),
    ("zwj", '\u{200d}'),
    ("lrm", '\u{200e}'),
    ("rlm", '\u{200f}'),
    ("larr", '←'),
    ("uarr", '↑'),
    ("rarr", '→'),
    ("darr", '↓'),
    ("harr", '↔'),
    ("rArr", '⇒'),
    ("lArr", '⇐'),
    ("minus", '−'),
    ("infin", '∞'),
    ("ne", '≠'),
    ("le", '≤'),
    ("ge", '≥'),
    ("asymp", '≈'),
    ("equiv", '≡'),
    ("sum", '∑'),
    ("prod", '∏'),
    ("radic", '√'),
    ("loz", '◊'),
    ("spades", '♠'),
    ("clubs", '♣'),
    ("hearts", '♥'),
    ("diams", '♦'),
];

fn named_entity(name: &str) -> Option<char> {
    if name.is_empty() {
        return None;
    }
    if let Some(i) = LATIN1_NAMES.iter().position(|n| *n == name) {
        return char::from_u32(0xA0 + i as u32);
    }
    if let Some(&(_, c)) = OTHER_ENTITIES.iter().find(|(n, _)| *n == name) {
        return Some(c);
    }
    if let Some(i) = GREEK_UPPER.iter().position(|n| *n == name) {
        // Skip the hole at U+03A2.
        let cp = 0x391 + i as u32 + u32::from(i >= 17);
        return char::from_u32(cp);
    }
    if let Some(i) = GREEK_LOWER.iter().position(|n| *n == name) {
        return char::from_u32(0x3B1 + i as u32);
    }
    None
}

/// What a browser makes of `&#NNN;`: C1 controls are read as Windows-1252,
/// because that is what pages that write them mean.
fn numeric_entity(cp: u32) -> char {
    let remapped = match cp {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        0 => 0xFFFD,
        other => other,
    };
    char::from_u32(remapped).unwrap_or('\u{fffd}')
}

/// Decode character references. Unknown names are left as written, and a
/// missing `;` is tolerated, as browsers do for the common names.
pub fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            i += 1;
            continue;
        }
        let amp = i;
        let mut j = i + 1;
        let decoded = if bytes.get(j) == Some(&b'#') {
            j += 1;
            let hex = matches!(bytes.get(j), Some(b'x' | b'X'));
            if hex {
                j += 1;
            }
            let digits_start = j;
            let radix = if hex { 16 } else { 10 };
            let mut cp: u32 = 0;
            while j < bytes.len() {
                let Some(d) = (bytes[j] as char).to_digit(radix) else { break };
                // Saturate: anything past U+10FFFF is invalid either way.
                cp = cp.saturating_mul(radix).saturating_add(d).min(0x11_0000);
                j += 1;
            }
            if j == digits_start {
                None
            } else {
                if bytes.get(j) == Some(&b';') {
                    j += 1;
                }
                Some(numeric_entity(cp))
            }
        } else {
            let name_start = j;
            while j < bytes.len() && j - name_start < 32 && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
            match named_entity(&s[name_start..j]) {
                Some(c) => {
                    if bytes.get(j) == Some(&b';') {
                        j += 1;
                    }
                    Some(c)
                }
                None => None,
            }
        };
        match decoded {
            Some(c) => {
                out.push_str(&s[copied..amp]);
                out.push(c);
                i = j;
                copied = j;
            }
            None => i = amp + 1,
        }
    }
    out.push_str(&s[copied..]);
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

const NAMED_COLORS: &[(&str, u32)] = &[
    ("black", 0x000000),
    ("white", 0xffffff),
    ("red", 0xff0000),
    ("green", 0x008000),
    ("blue", 0x0000ff),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("silver", 0xc0c0c0),
    ("maroon", 0x800000),
    ("purple", 0x800080),
    ("fuchsia", 0xff00ff),
    ("magenta", 0xff00ff),
    ("lime", 0x00ff00),
    ("olive", 0x808000),
    ("navy", 0x000080),
    ("teal", 0x008080),
    ("aqua", 0x00ffff),
    ("cyan", 0x00ffff),
    ("orange", 0xffa500),
    ("yellow", 0xffff00),
    ("brown", 0xa52a2a),
    ("pink", 0xffc0cb),
    ("violet", 0xee82ee),
    ("indigo", 0x4b0082),
    ("gold", 0xffd700),
    ("crimson", 0xdc143c),
    ("darkred", 0x8b0000),
    ("darkgreen", 0x006400),
    ("darkblue", 0x00008b),
    ("darkgray", 0xa9a9a9),
    ("darkgrey", 0xa9a9a9),
    ("darkcyan", 0x008b8b),
    ("darkmagenta", 0x8b008b),
    ("darkorange", 0xff8c00),
    ("darkviolet", 0x9400d3),
    ("darkkhaki", 0xbdb76b),
    ("darkolivegreen", 0x556b2f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("lightgray", 0xd3d3d3),
    ("lightgrey", 0xd3d3d3),
    ("lightblue", 0xadd8e6),
    ("lightgreen", 0x90ee90),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("slateblue", 0x6a5acd),
    ("steelblue", 0x4682b4),
    ("royalblue", 0x4169e1),
    ("dodgerblue", 0x1e90ff),
    ("midnightblue", 0x191970),
    ("mediumblue", 0x0000cd),
    ("cadetblue", 0x5f9ea0),
    ("skyblue", 0x87ceeb),
    ("blueviolet", 0x8a2be2),
    ("seagreen", 0x2e8b57),
    ("forestgreen", 0x228b22),
    ("olivedrab", 0x6b8e23),
    ("saddlebrown", 0x8b4513),
    ("sienna", 0xa0522d),
    ("chocolate", 0xd2691e),
    ("tomato", 0xff6347),
    ("orangered", 0xff4500),
    ("firebrick", 0xb22222),
    ("indianred", 0xcd5c5c),
    ("deeppink", 0xff1493),
    ("hotpink", 0xff69b4),
    ("khaki", 0xf0e68c),
    ("tan", 0xd2b48c),
    ("wheat", 0xf5deb3),
];

/// Parse a CSS / HTML colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, bare
/// six-digit hex (as legacy `<font color=ff0000>` writes it), `rgb()` /
/// `rgba()`, or a common name. `None` for anything else, including `inherit`
/// and friends.
pub fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim().trim_end_matches("!important").trim();
    if v.is_empty() {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    if let Some(args) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
        let args = args.trim_end_matches(')');
        let parts: Vec<&str> = args.split([',', ' ', '/']).filter(|p| !p.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        let channel = |p: &str| -> Option<u8> {
            if let Some(pct) = p.strip_suffix('%') {
                let f: f32 = pct.parse().ok()?;
                Some((f.clamp(0.0, 100.0) * 2.55).round() as u8)
            } else {
                let f: f32 = p.parse().ok()?;
                Some(f.clamp(0.0, 255.0).round() as u8)
            }
        };
        let alpha = match parts.get(3) {
            Some(a) => {
                if let Some(pct) = a.strip_suffix('%') {
                    (pct.parse::<f32>().ok()?.clamp(0.0, 100.0) * 2.55).round() as u8
                } else {
                    (a.parse::<f32>().ok()?.clamp(0.0, 1.0) * 255.0).round() as u8
                }
            }
            None => 255,
        };
        return Some(Color::rgba(channel(parts[0])?, channel(parts[1])?, channel(parts[2])?, alpha));
    }
    if let Some(&(_, rgb)) = NAMED_COLORS.iter().find(|(n, _)| *n == lower) {
        return Some(Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
    }
    let hashed = lower.starts_with('#');
    let hex = lower.strip_prefix('#').unwrap_or(&lower);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // Bare hex without `#` only in the unambiguous six-digit form; the
    // classic theme shorthand ("b" = grey) is not a CSS thing.
    if hex.len() != 6 && !hashed {
        return None;
    }
    match hex.len() {
        3 | 6 | 8 => Color::parse_legacy(hex).ok().flatten(),
        4 => {
            let d = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 17);
            Some(Color::rgba(d(0)?, d(1)?, d(2)?, d(3)?))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// The inline style in force. `hidden` is ours: content under
/// `display:none` or `hidden` is dropped, as a browser would not show it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    monospace: bool,
    link: Option<String>,
    color: Option<Color>,
    small: bool,
    sup: bool,
    hidden: bool,
}

impl Style {
    fn span_matches(&self, span: &Span) -> bool {
        span.bold == self.bold
            && span.italic == self.italic
            && span.underline == self.underline
            && span.monospace == self.monospace
            && span.link == self.link
            && span.color == self.color
            && span.small == self.small
            && span.sup == self.sup
    }

    fn span(&self, text: String) -> Span {
        Span {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            monospace: self.monospace,
            link: self.link.clone(),
            color: self.color,
            small: self.small,
            sup: self.sup,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Paragraph,
    Heading(u8),
    ListItem { depth: u32, ordered_index: Option<u32> },
}

struct ListCtx {
    ordered: bool,
    next: u32,
}

#[derive(Default)]
struct TableState {
    rows: Vec<Vec<Vec<Span>>>,
    row: Option<Vec<Vec<Span>>>,
    cell: Option<Vec<Span>>,
}

/// How deep the open-element stack may grow; garbage with a million
/// unclosed tags must not cost a million style copies.
const MAX_STACK: usize = 256;

const VOID: &[&str] = &[
    "br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "param", "source", "track",
    "wbr", "keygen", "frame", "basefont",
];

const BLOCK: &[&str] = &[
    "p", "div", "section", "article", "header", "footer", "nav", "aside", "main", "dl", "dt", "dd", "center",
    "address", "figure", "figcaption", "form", "fieldset", "details", "summary", "caption", "body", "html",
    "legend", "menu", "dir", "hgroup", "frameset", "noframes", "option", "select",
];

struct Builder {
    blocks: Vec<Block>,
    cur: Vec<Span>,
    kind: Kind,
    styles: Vec<(String, Style)>,
    pre: usize,
    skip_newline: bool,
    pending_space: bool,
    /// The style of the whitespace that set `pending_space`.
    pending_style: Style,
    lists: Vec<ListCtx>,
    quote: usize,
    table: Option<TableState>,
    table_depth: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Builder {
            blocks: Vec::new(),
            cur: Vec::new(),
            kind: Kind::Paragraph,
            styles: Vec::new(),
            pre: 0,
            skip_newline: false,
            pending_space: false,
            pending_style: Style::default(),
            lists: Vec::new(),
            quote: 0,
            table: None,
            table_depth: 0,
        }
    }
}

impl Builder {
    fn style(&self) -> Style {
        self.styles.last().map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// Where inline content goes: the open table cell, or the current block.
    fn target(&mut self) -> &mut Vec<Span> {
        match self.table.as_mut() {
            Some(t) => t.cell.get_or_insert_with(Vec::new),
            None => &mut self.cur,
        }
    }

    fn target_ref(&self) -> Option<&Vec<Span>> {
        match &self.table {
            Some(t) => t.cell.as_ref(),
            None => Some(&self.cur),
        }
    }

    /// Whether the target is at the start of a line, where collapsed space
    /// is dropped.
    fn at_line_start(&self) -> bool {
        let Some(spans) = self.target_ref() else { return true };
        match spans.iter().rev().find(|s| !s.text.is_empty()) {
            Some(s) => s.text.ends_with('\n') || s.text.ends_with(' '),
            None => true,
        }
    }

    fn push_str(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        let target = self.target();
        match target.last_mut() {
            Some(last) if style.span_matches(last) => last.text.push_str(text),
            _ => target.push(style.span(text.to_owned())),
        }
    }

    fn text(&mut self, text: &str) {
        let style = self.style();
        if style.hidden {
            return;
        }
        if self.pre > 0 {
            let mut t = text.replace("\r\n", "\n").replace('\r', "\n");
            if self.skip_newline {
                if t.starts_with('\n') {
                    t.remove(0);
                }
                if !t.is_empty() {
                    self.skip_newline = false;
                }
            }
            self.push_str(&t, &style);
            return;
        }
        let mut buf = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if !self.pending_space {
                    self.pending_space = true;
                    self.pending_style = style.clone();
                }
                continue;
            }
            if self.pending_space {
                self.pending_space = false;
                if !buf.is_empty() {
                    buf.push(' ');
                } else if !self.at_line_start() {
                    // The collapsed space keeps the style of the text it came
                    // from, as in a browser: in "<b>bold </b><i>x</i>" the
                    // space is bold, not italic.
                    let style = std::mem::take(&mut self.pending_style);
                    self.push_str(" ", &style);
                }
            }
            buf.push(c);
        }
        self.push_str(&buf, &style);
    }

    fn line_break(&mut self) {
        self.pending_space = false;
        let style = self.style();
        if style.hidden {
            return;
        }
        let empty = self.target_ref().is_none_or(|c| c.iter().all(|s| s.text.is_empty()));
        if empty {
            return;
        }
        // A space before the break is invisible in a browser.
        if let Some(last) = self.target().last_mut() {
            let trimmed = last.text.trim_end_matches(' ').len();
            last.text.truncate(trimmed);
        }
        self.push_str("\n", &style);
    }

    /// End the current block (or, inside a table, the current line of the
    /// cell).
    fn flush(&mut self) {
        self.pending_space = false;
        if self.table.is_some() {
            self.line_break();
            return;
        }
        let spans = trim_spans(std::mem::take(&mut self.cur));
        if spans.is_empty() {
            return;
        }
        let block = match self.kind {
            Kind::Heading(level) => Block::Heading { level, spans },
            Kind::ListItem { depth, ordered_index } => Block::ListItem { depth, ordered_index, spans },
            Kind::Paragraph if self.quote > 0 => Block::Quote(spans),
            Kind::Paragraph => Block::Paragraph(spans),
        };
        self.blocks.push(block);
        self.kind = Kind::Paragraph;
    }

    fn finish_cell(&mut self) {
        self.pending_space = false;
        if let Some(t) = self.table.as_mut() {
            if let Some(cell) = t.cell.take() {
                t.row.get_or_insert_with(Vec::new).push(trim_spans(cell));
            }
        }
    }

    fn finish_row(&mut self) {
        self.finish_cell();
        if let Some(t) = self.table.as_mut() {
            if let Some(row) = t.row.take() {
                if !row.is_empty() {
                    t.rows.push(row);
                }
            }
        }
    }

    fn finish_table(&mut self) {
        self.finish_row();
        if let Some(t) = self.table.take() {
            let has_content = t.rows.iter().flatten().any(|c| !c.is_empty());
            if has_content {
                self.blocks.push(Block::Table(t.rows));
            }
        }
    }

    fn start(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "br" => return self.line_break(),
            "hr" => {
                self.flush();
                if self.table.is_none() {
                    self.blocks.push(Block::Rule);
                }
                return;
            }
            "img" => {
                // An image with alt text reads as that text; without it, nothing.
                if let Some((_, alt)) = attrs.iter().find(|(k, _)| k == "alt") {
                    if !alt.trim().is_empty() {
                        let alt = alt.clone();
                        self.text(&alt);
                    }
                }
                return;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                if self.table.is_none() {
                    self.kind = Kind::Heading(name.as_bytes()[1] - b'0');
                }
            }
            "ul" | "ol" => {
                self.flush();
                let start =
                    attrs.iter().find(|(k, _)| k == "start").and_then(|(_, v)| v.trim().parse().ok()).unwrap_or(1);
                if self.lists.len() < MAX_STACK {
                    self.lists.push(ListCtx { ordered: name == "ol", next: start });
                }
            }
            "li" => {
                self.flush();
                if self.table.is_none() {
                    let depth = self.lists.len().saturating_sub(1) as u32;
                    let value = attrs.iter().find(|(k, _)| k == "value").and_then(|(_, v)| v.trim().parse().ok());
                    let ordered_index = match self.lists.last_mut() {
                        Some(l) if l.ordered => {
                            if let Some(v) = value {
                                l.next = v;
                            }
                            let n = l.next;
                            l.next = l.next.saturating_add(1);
                            Some(n)
                        }
                        _ => None,
                    };
                    self.kind = Kind::ListItem { depth, ordered_index };
                }
            }
            "blockquote" => {
                self.flush();
                self.quote += 1;
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.flush();
                self.pre += 1;
                self.skip_newline = true;
            }
            "table" => {
                self.flush();
                self.table_depth += 1;
                if self.table_depth == 1 {
                    self.table = Some(TableState::default());
                }
            }
            "tr" => {
                if self.table_depth == 1 {
                    self.finish_row();
                } else {
                    self.flush();
                }
            }
            "td" | "th" => {
                if self.table_depth == 1 {
                    self.finish_cell();
                    // Open the cell now so an empty one still counts.
                    if let Some(t) = self.table.as_mut() {
                        t.cell = Some(Vec::new());
                    }
                } else if self.table_depth > 1 {
                    self.pending_space = true;
                    self.pending_style = self.style();
                }
            }
            n if BLOCK.contains(&n) => self.flush(),
            _ => {}
        }
        if VOID.contains(&name) || self.styles.len() >= MAX_STACK {
            return;
        }
        let mut style = self.style();
        apply_tag_style(name, attrs, &mut style);
        self.styles.push((name.to_owned(), style));
    }

    fn end(&mut self, name: &str) {
        if let Some(i) = self.styles.iter().rposition(|(n, _)| n == name) {
            self.styles.truncate(i);
        }
        match name {
            "br" => self.line_break(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                if self.table.is_none() {
                    self.kind = Kind::Paragraph;
                }
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
            }
            "li" => self.flush(),
            "blockquote" => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.flush();
                self.pre = self.pre.saturating_sub(1);
            }
            "table" => {
                if self.table_depth == 1 {
                    self.finish_table();
                } else {
                    self.flush();
                }
                self.table_depth = self.table_depth.saturating_sub(1);
            }
            "tr" => {
                if self.table_depth == 1 {
                    self.finish_row();
                }
            }
            "td" | "th" => {
                if self.table_depth == 1 {
                    self.finish_cell();
                }
            }
            n if BLOCK.contains(&n) => self.flush(),
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        if self.table.is_some() {
            self.finish_table();
        }
        self.flush();
        self.blocks
    }
}

/// Drop leading/trailing blank text and empty spans.
fn trim_spans(mut spans: Vec<Span>) -> Vec<Span> {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end_matches([' ', '\n', '\t']).len();
        last.text.truncate(trimmed);
        if last.text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
    while let Some(first) = spans.first_mut() {
        // Only line breaks: leading spaces can only come from `<pre>`,
        // where they matter.
        let lead = first.text.len() - first.text.trim_start_matches('\n').len();
        first.text.drain(..lead);
        if first.text.is_empty() {
            spans.remove(0);
        } else {
            break;
        }
    }
    spans.retain(|s| !s.text.is_empty());
    spans
}

/// The style an element adds on top of its parent's.
fn apply_tag_style(name: &str, attrs: &[(String, String)], style: &mut Style) {
    match name {
        "b" | "strong" | "th" => style.bold = true,
        "i" | "em" | "cite" | "dfn" | "var" | "address" => style.italic = true,
        "u" | "ins" => style.underline = true,
        "code" | "tt" | "kbd" | "samp" | "pre" | "listing" | "xmp" | "plaintext" => style.monospace = true,
        "small" | "sub" => style.small = true,
        "sup" => style.sup = true,
        "big" => style.small = false,
        "a" => {
            if let Some((_, href)) = attrs.iter().find(|(k, _)| k == "href") {
                let href = href.trim();
                if !href.is_empty() {
                    style.link = Some(href.to_owned());
                }
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => style.bold = true,
        _ => {}
    }
    for (key, value) in attrs {
        match key.as_str() {
            // `<font color>`, and Pango's `<span foreground>` (StarDict `g`
            // fields are Pango markup).
            "color" | "foreground" | "fgcolor" if matches!(name, "font" | "span") => {
                if let Some(c) = parse_color(value) {
                    style.color = Some(c);
                }
            }
            "weight" if name == "span" => {
                style.bold =
                    matches!(value.trim(), "bold" | "heavy" | "ultrabold" | "600" | "700" | "800" | "900");
            }
            "underline" if name == "span" => style.underline = value.trim() != "none",
            "font_family" | "face" if value.to_ascii_lowercase().contains("mono") => style.monospace = true,
            "hidden" => style.hidden = true,
            "style" => apply_css(value, name, style),
            _ => {}
        }
    }
}

fn apply_css(css: &str, tag: &str, style: &mut Style) {
    for decl in css.split(';') {
        let Some((prop, value)) = decl.split_once(':') else {
            // Pango's `style="italic"` is an attribute value, not CSS.
            if tag == "span" && decl.trim() == "italic" {
                style.italic = true;
            }
            continue;
        };
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let value = value.trim_end_matches("!important").trim();
        match prop.as_str() {
            "color" => {
                if let Some(c) = parse_color(value) {
                    style.color = Some(c);
                }
            }
            "font-weight" => {
                style.bold = match value {
                    "bold" | "bolder" => true,
                    v => v.parse::<u32>().map_or(style.bold && v != "normal", |w| w >= 600),
                }
            }
            "font-style" => style.italic = value.starts_with("italic") || value.starts_with("oblique"),
            "text-decoration" | "text-decoration-line" => {
                if value.contains("underline") {
                    style.underline = true;
                } else if value == "none" {
                    style.underline = false;
                }
            }
            "font-family" if value.contains("mono") || value.contains("courier") => style.monospace = true,
            "font-size" if matches!(value, "small" | "smaller" | "x-small" | "xx-small") => style.small = true,
            "vertical-align" if value == "super" => style.sup = true,
            "vertical-align" if value == "sub" => style.small = true,
            "display" if value == "none" => style.hidden = true,
            "visibility" if value == "hidden" => style.hidden = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    fn para(block: &Block) -> &[Span] {
        match block {
            Block::Paragraph(s) => s,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn nested_formatting_produces_styled_spans() {
        let blocks = to_blocks("<p>plain <b>bold <i>both</i></b> <u>under</u></p>");
        assert_eq!(blocks.len(), 1);
        let spans = para(&blocks[0]);
        assert_eq!(text_of(spans), "plain bold both under");
        let both = spans.iter().find(|s| s.text == "both").unwrap();
        assert!(both.bold && both.italic);
        let bold = spans.iter().find(|s| s.text.starts_with("bold")).unwrap();
        assert!(bold.bold && !bold.italic);
        assert!(spans.iter().find(|s| s.text == "under").unwrap().underline);
    }

    #[test]
    fn links_carry_their_target() {
        let blocks =
            to_blocks(r#"see <a href="dict://run">run</a> and <a href=http://x.org/a?b=1&amp;c=2>this</a>"#);
        let spans = para(&blocks[0]);
        assert_eq!(spans.iter().find(|s| s.text == "run").unwrap().link.as_deref(), Some("dict://run"));
        assert_eq!(
            spans.iter().find(|s| s.text == "this").unwrap().link.as_deref(),
            Some("http://x.org/a?b=1&c=2")
        );
        assert_eq!(spans[0].link, None);
    }

    #[test]
    fn colours_from_font_and_css() {
        let blocks = to_blocks(
            r##"<font color="#ff0000">red</font><span style="color: rgb(0, 128, 0)">green</span><font color=navy>navy</font><span style="COLOR:#abc">short</span><font color="ff8800">bare</font>"##,
        );
        let spans = para(&blocks[0]);
        let color = |t: &str| spans.iter().find(|s| s.text == t).unwrap().color;
        assert_eq!(color("red"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(color("green"), Some(Color::rgb(0, 128, 0)));
        assert_eq!(color("navy"), Some(Color::rgb(0, 0, 128)));
        assert_eq!(color("short"), Some(Color::rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(color("bare"), Some(Color::rgb(0xff, 0x88, 0)));
        assert_eq!(parse_color("inherit"), None);
        assert_eq!(parse_color("b"), None);
        assert_eq!(parse_color("rgba(10,20,30,0.5)"), Some(Color::rgba(10, 20, 30, 128)));
    }

    #[test]
    fn lists_nest_and_number() {
        let blocks =
            to_blocks("<ol start=3><li>three<li>four<ul><li>bullet</li></ul></li><li>five</ol><p>after</p>");
        assert_eq!(blocks[0], Block::ListItem { depth: 0, ordered_index: Some(3), spans: vec![Span::plain("three")] });
        assert_eq!(blocks[1], Block::ListItem { depth: 0, ordered_index: Some(4), spans: vec![Span::plain("four")] });
        assert_eq!(blocks[2], Block::ListItem { depth: 1, ordered_index: None, spans: vec![Span::plain("bullet")] });
        assert_eq!(blocks[3], Block::ListItem { depth: 0, ordered_index: Some(5), spans: vec![Span::plain("five")] });
        assert_eq!(blocks[4], Block::Paragraph(vec![Span::plain("after")]));
        assert_eq!(to_plain_text("<ul><li>a<ul><li>b</ul></ul>"), "• a\n  • b");
    }

    #[test]
    fn tables_become_rows_of_cells() {
        let blocks =
            to_blocks("<table><tr><th>word</th><td>слово</td></tr><tr><td>a<br>b</td><td></td></tr></table>");
        match &blocks[0] {
            Block::Table(rows) => {
                assert_eq!(rows.len(), 2);
                assert_eq!(text_of(&rows[0][0]), "word");
                assert!(rows[0][0][0].bold);
                assert_eq!(text_of(&rows[0][1]), "слово");
                assert_eq!(text_of(&rows[1][0]), "a\nb");
                assert!(rows[1][1].is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(to_plain_text("<table><tr><td>a</td><td>b</td></tr></table>"), "a\tb");
    }

    #[test]
    fn nested_tables_flatten_into_the_outer_cell() {
        let blocks =
            to_blocks("<table><tr><td>x<table><tr><td>in1</td><td>in2</td></tr></table></td></tr></table>");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Table(rows) => {
                assert_eq!(rows.len(), 1);
                let t = text_of(&rows[0][0]);
                assert!(t.contains("in1") && t.contains("in2") && t.starts_with('x'), "{t:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities(
                "&lt;a&gt; &amp; &quot;q&quot; &apos;s&apos; &mdash; &ndash; &hellip; &laquo;x&raquo; &copy; 5&deg; caf&eacute;"
            ),
            "<a> & \"q\" 's' — – … «x» © 5° café"
        );
        assert_eq!(decode_entities("&#65;&#x42;&#X43;&#150;&#0;"), "ABC–\u{fffd}");
        assert_eq!(decode_entities("&nbsp;&alpha;&Omega;&Sigma;&sigmaf;"), "\u{a0}αΩΣς");
        // Unknown and malformed references stay as written.
        assert_eq!(decode_entities("&bogus; & &#; &#x; a&b"), "&bogus; & &#; &#x; a&b");
        // A missing semicolon is tolerated for known names.
        assert_eq!(decode_entities("&amp &lt"), "& <");
        assert_eq!(decode_entities("&#99999999999;"), "\u{fffd}");
    }

    #[test]
    fn broken_markup_is_tolerated() {
        let blocks = to_blocks("<b>unclosed <i>deep <p>next para</b> tail <a href='x>broken");
        let text = blocks_to_plain_text(&blocks);
        assert!(text.contains("unclosed deep"), "{text:?}");
        assert!(text.contains("next para tail"), "{text:?}");
        // Stray angle brackets are text.
        assert_eq!(to_plain_text("a < b > c"), "a < b > c");
        assert_eq!(to_plain_text("x <!-- never closed"), "x");
        assert_eq!(to_plain_text("</>x</ junk>y<"), "xy<");
        assert_eq!(to_plain_text("<div><span>a</div>b</span>"), "a\nb");
    }

    #[test]
    fn scripts_styles_and_hidden_content_are_dropped() {
        let html = r#"<html><head><title>T</title><style>p{color:red}</style><script>var a = "<p>no</p>";</script></head>
            <body><p>yes</p><SCRIPT type="x">alert(1)</SCRIPT><div style="display:none">secret</div><span hidden>also</span>ok</body></html>"#;
        assert_eq!(to_plain_text(html), "yes\nok");
        assert_eq!(to_plain_text("<script>never closed <b>x</b>"), "");
    }

    #[test]
    fn pre_preserves_whitespace() {
        let blocks = to_blocks("<p>a   b\n c</p><pre>\n  line 1\n    line 2  </pre>");
        assert_eq!(text_of(para(&blocks[0])), "a b c");
        let pre = para(&blocks[1]);
        assert_eq!(text_of(pre), "  line 1\n    line 2");
        assert!(pre.iter().all(|s| s.monospace));
    }

    #[test]
    fn whitespace_collapses_across_tags() {
        assert_eq!(to_plain_text("  <b> hello </b>  <i>world</i>  "), "hello world");
        assert_eq!(to_plain_text("one<br>two<br/><br>three"), "one\ntwo\n\nthree");
        assert_eq!(to_plain_text("<br><br>start"), "start");
        assert_eq!(to_plain_text("a&nbsp;&nbsp;b"), "a  b");
        assert_eq!(to_plain_text("a <br> b"), "a\nb");
    }

    #[test]
    fn headings_rules_quotes_sup_small_code() {
        let blocks =
            to_blocks("<h2>Title</h2><hr><blockquote>quoted</blockquote>x<sup>2</sup><small>s</small><code>c</code>");
        assert_eq!(blocks[0], Block::Heading { level: 2, spans: vec![Span { bold: true, ..Span::plain("Title") }] });
        assert_eq!(blocks[1], Block::Rule);
        assert_eq!(blocks[2], Block::Quote(vec![Span::plain("quoted")]));
        let spans = para(&blocks[3]);
        assert!(spans.iter().find(|s| s.text == "2").unwrap().sup);
        assert!(spans.iter().find(|s| s.text == "s").unwrap().small);
        assert!(spans.iter().find(|s| s.text == "c").unwrap().monospace);
    }

    #[test]
    fn css_font_weight_and_style() {
        let blocks = to_blocks(
            r#"<span style="font-weight:700;font-style:italic">x</span><span style="text-decoration: underline">y</span>"#,
        );
        let spans = para(&blocks[0]);
        assert!(spans[0].bold && spans[0].italic);
        assert!(spans[1].underline);
    }

    #[test]
    fn pango_markup_reads_like_html() {
        let blocks = to_blocks(r#"<span foreground="blue" weight="bold">word</span> <i>n.</i>"#);
        let spans = para(&blocks[0]);
        assert_eq!(spans[0].color, Some(Color::rgb(0, 0, 255)));
        assert!(spans[0].bold);
    }

    #[test]
    fn tokenizer_reads_attributes_in_every_spelling() {
        let toks: Vec<Token> = Tokenizer::html(r#"<a HREF="x" title='y' data-z=1 checked/>"#).collect();
        assert_eq!(
            toks[0],
            Token::Start {
                name: "a".into(),
                attrs: vec![
                    ("href".into(), "x".into()),
                    ("title".into(), "y".into()),
                    ("data-z".into(), "1".into()),
                    ("checked".into(), String::new())
                ],
                self_closing: true
            }
        );
        let toks: Vec<Token> = Tokenizer::xml("<k>a<![CDATA[<b>]]></k>").collect();
        assert_eq!(toks[2], Token::Text("<b>"));
    }

    #[test]
    fn escape_round_trips_through_the_reader() {
        let s = r#"<tag> & "quote" ünïcödé"#;
        assert_eq!(to_plain_text(&escape(s)), s);
    }

    /// Deterministic garbage: tag fragments, entities, quotes and multi-byte
    /// text in random order. Nothing may panic, and no empty span may leak.
    #[test]
    fn random_tag_soup_never_panics() {
        const PIECES: &[&str] = &[
            "<", ">", "</", "/>", "<b>", "</b>", "<i", "<p>", "</p>", "<table>", "<tr>", "<td>", "</td>", "</table>",
            "<ul>", "<ol>", "<li>", "</li>", "</ul>", "<pre>", "</pre>", "<br>", "<hr>", "<!--", "-->", "<![CDATA[",
            "]]>", "<script>", "</script>", "&", "&amp", "&#", "&#x", "&#1234567;", ";", "\"", "'", "=", " ", "\n",
            "\t", "a", "слово", "日本", "é", "🦀", "<a href=", "<font color=", "#zzz", "style=\"color:", "<h1>",
            "</h7>", "<blockquote>", "<!DOCTYPE", "<?xml", "<sup>", "<span hidden>", "&nbsp;", "\u{a0}",
            "<img alt=x>", "</tr>", "<th>",
        ];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..3000 {
            let len = (next() % 60) as usize;
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(PIECES[(next() % PIECES.len() as u64) as usize]);
            }
            let blocks = to_blocks(&s);
            let _ = blocks_to_plain_text(&blocks);
            for b in &blocks {
                if let Block::Paragraph(spans) | Block::Quote(spans) = b {
                    assert!(!spans.is_empty() && spans.iter().all(|s| !s.text.is_empty()), "empty span from {s:?}");
                }
            }
        }
    }

    #[test]
    fn random_bytes_never_panic() {
        let mut state: u32 = 12345;
        for len in 0..400 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (state >> 16) as u8
                })
                .collect();
            let s = String::from_utf8_lossy(&bytes);
            let _ = to_blocks(&s);
            let _ = decode_entities(&s);
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let html = "<b>".repeat(100_000) + "x";
        let blocks = to_blocks(&html);
        assert_eq!(blocks_to_plain_text(&blocks), "x");
    }
}
