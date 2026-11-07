//! A code editor: egui's `TextEdit` with syntax colours, a line-number
//! gutter, error marks, find and go to line. The module editor uses it for
//! `module.toml`, scripts and test fixtures.
//!
//! Highlighting is a hand-written tokenizer per language (JavaScript, TOML,
//! JSON, Python) turned into a `LayoutJob` with colours from the theme
//! ([`Syntax::from_look`]). The job is rebuilt only when the text, the marks,
//! the search or the colours change (a hash of them is kept), and egui caches
//! the laid-out galley, so an idle frame costs nothing. What `TextEdit` has
//! no slot for (Tab inserting spaces, auto-indent, toggling comments) is
//! done by taking those keys before the `TextEdit` sees them.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{Color32, FontId, Key, KeyboardShortcut, Modifiers, Stroke, Ui};

use crate::style::Look;

/// Spaces a Tab inserts.
const INDENT: &str = "  ";

// -- languages and tokens ----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    JavaScript,
    Toml,
    Json,
    Python,
    Plain,
}

impl Lang {
    /// The language of a file, by its extension.
    pub fn for_path(path: &str) -> Lang {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "js" | "mjs" | "cjs" | "ts" | "java" | "c" | "h" | "rs" | "kt" => Lang::JavaScript,
            "toml" => Lang::Toml,
            "json" => Lang::Json,
            "py" => Lang::Python,
            _ => Lang::Plain,
        }
    }

    /// What starts a line comment, for toggling comments.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::JavaScript => Some("//"),
            Lang::Toml | Lang::Python => Some("#"),
            Lang::Json | Lang::Plain => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tok {
    Keyword,
    /// `true`, `false`, `null`, `undefined`, `None`…
    Literal,
    String,
    Number,
    Comment,
    Regex,
    /// A TOML key, a JSON object key, a Python decorator.
    Key,
    /// A TOML `[table]` header.
    Table,
}

/// A coloured span: a byte range of the text and what it is. Spans are in
/// order and do not overlap; what lies between them is plain text.
pub type Span = (Range<usize>, Tok);

pub fn tokenize(lang: Lang, text: &str) -> Vec<Span> {
    match lang {
        Lang::JavaScript => js(text),
        Lang::Toml => toml(text),
        Lang::Json => json(text),
        Lang::Python => python(text),
        Lang::Plain => Vec::new(),
    }
}

const JS_KEYWORDS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
];
const JS_LITERALS: &[&str] = &["true", "false", "null", "undefined", "NaN", "Infinity"];
/// Keywords after which a `/` starts a regular expression, not a division.
const JS_BEFORE_REGEX: &[&str] =
    &["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield"];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// A cursor over the text by bytes, stepping over whole characters.
struct Scan<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Scan<'a> {
    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }
    fn peek_at(&self, n: usize) -> Option<char> {
        self.s[self.i..].chars().nth(n)
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }
    fn starts(&self, p: &str) -> bool {
        self.s[self.i..].starts_with(p)
    }
    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&f) {
            self.bump();
        }
    }
    fn skip_line(&mut self) {
        self.eat_while(|c| c != '\n');
    }
    /// A quoted string ending at `quote` (escapes with `\`), or at the end
    /// of the line when `single_line` and it is not closed.
    fn quoted(&mut self, quote: char, single_line: bool) {
        while let Some(c) = self.peek() {
            if single_line && c == '\n' {
                return;
            }
            self.bump();
            if c == '\\' {
                self.bump();
            } else if c == quote {
                return;
            }
        }
    }
    /// Up to and including `end`, or to the end of the text.
    fn until(&mut self, end: &str) {
        match self.s[self.i..].find(end) {
            Some(at) => self.i += at + end.len(),
            None => self.i = self.s.len(),
        }
    }
    fn number(&mut self) {
        if self.starts("0x") || self.starts("0X") || self.starts("0b") || self.starts("0o") {
            self.i += 2;
            self.eat_while(|c| c.is_ascii_hexdigit() || c == '_');
        } else {
            self.eat_while(|c| c.is_ascii_digit() || c == '_' || c == '.');
            if matches!(self.peek(), Some('e' | 'E'))
                && self.peek_at(1).is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+')
            {
                self.bump();
                self.bump();
                self.eat_while(|c| c.is_ascii_digit());
            }
        }
        if self.peek() == Some('n') {
            self.bump();
        }
    }
}

fn js(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut s = Scan { s: text, i: 0 };
    // Open template literals: the brace depth at which each `${` started.
    let mut templates: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    // Whether a `/` here would start a regular expression.
    let mut regex_ok = true;
    // Inside a template literal's text, which started at this byte (its
    // backtick, or the `}` that closed a `${…}`).
    let mut in_template: Option<usize> = None;
    loop {
        if let Some(start) = in_template.take() {
            let mut opens_code = false;
            while let Some(c) = s.peek() {
                if c == '\\' {
                    s.bump();
                    s.bump();
                } else if c == '`' {
                    s.bump();
                    break;
                } else if s.starts("${") {
                    s.i += 2;
                    opens_code = true;
                    break;
                } else {
                    s.bump();
                }
            }
            out.push((start..s.i, Tok::String));
            if opens_code {
                templates.push(depth);
                depth += 1;
            }
            regex_ok = opens_code;
            continue;
        }
        let Some(c) = s.peek() else { break };
        let start = s.i;
        if c.is_whitespace() {
            s.bump();
            continue;
        }
        if s.starts("//") {
            s.skip_line();
            out.push((start..s.i, Tok::Comment));
            continue;
        }
        if s.starts("/*") {
            s.i += 2;
            s.until("*/");
            out.push((start..s.i, Tok::Comment));
            continue;
        }
        match c {
            '"' | '\'' => {
                s.bump();
                s.quoted(c, true);
                out.push((start..s.i, Tok::String));
                regex_ok = false;
            }
            '`' => {
                s.bump();
                in_template = Some(start);
            }
            '/' => {
                s.bump();
                if regex_ok {
                    // A regular expression: up to an unescaped `/` outside
                    // a character class, then its flags.
                    let mut class = false;
                    while let Some(d) = s.peek() {
                        if d == '\n' {
                            break;
                        }
                        s.bump();
                        match d {
                            '\\' => {
                                s.bump();
                            }
                            '[' => class = true,
                            ']' => class = false,
                            '/' if !class => break,
                            _ => {}
                        }
                    }
                    s.eat_while(|c| c.is_ascii_alphabetic());
                    out.push((start..s.i, Tok::Regex));
                    regex_ok = false;
                } else {
                    regex_ok = true;
                }
            }
            '0'..='9' => {
                s.number();
                out.push((start..s.i, Tok::Number));
                regex_ok = false;
            }
            '.' if s.peek_at(1).is_some_and(|d| d.is_ascii_digit()) => {
                s.bump();
                s.number();
                out.push((start..s.i, Tok::Number));
                regex_ok = false;
            }
            c if is_ident_start(c) => {
                s.eat_while(is_ident);
                let word = &text[start..s.i];
                let after_dot = text[..start].trim_end().ends_with('.');
                if !after_dot && JS_KEYWORDS.contains(&word) {
                    out.push((start..s.i, Tok::Keyword));
                    regex_ok = JS_BEFORE_REGEX.contains(&word);
                } else if !after_dot && JS_LITERALS.contains(&word) {
                    out.push((start..s.i, Tok::Literal));
                    regex_ok = false;
                } else {
                    regex_ok = false;
                }
            }
            '{' => {
                s.bump();
                depth += 1;
                regex_ok = true;
            }
            '}' => {
                s.bump();
                depth = depth.saturating_sub(1);
                if templates.last() == Some(&depth) {
                    templates.pop();
                    in_template = Some(start);
                }
                regex_ok = true;
            }
            ')' | ']' => {
                s.bump();
                regex_ok = false;
            }
            _ => {
                s.bump();
                regex_ok = true;
            }
        }
    }
    out
}

fn toml(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut s = Scan { s: text, i: 0 };
    // At the start of a line (only whitespace before): a key or a table
    // header may begin here.
    let mut line_start = true;
    // After `=` on this line: a value, not a key.
    let mut in_value = false;
    // Inside `[...]` arrays or `{...}` inline tables of a value.
    let mut nesting = 0usize;
    while let Some(c) = s.peek() {
        let start = s.i;
        match c {
            '\n' => {
                s.bump();
                if nesting == 0 {
                    line_start = true;
                    in_value = false;
                }
            }
            c if c.is_whitespace() => {
                s.bump();
            }
            '#' => {
                s.skip_line();
                out.push((start..s.i, Tok::Comment));
            }
            '[' if line_start && !in_value => {
                s.skip_line();
                // The header ends at its last `]`; a comment may follow.
                let line = &text[start..s.i];
                let end = line.rfind(']').map(|e| start + e + 1).unwrap_or(s.i);
                out.push((start..end, Tok::Table));
                s.i = end;
                line_start = false;
            }
            '"' | '\'' => {
                let triple: String = std::iter::repeat_n(c, 3).collect();
                if s.starts(&triple) {
                    s.i += 3;
                    s.until(&triple);
                } else {
                    s.bump();
                    if c == '"' {
                        s.quoted('"', true);
                    } else {
                        s.eat_while(|d| d != '\'' && d != '\n');
                        if s.peek() == Some('\'') {
                            s.bump();
                        }
                    }
                }
                let kind = if in_value || nesting > 0 { Tok::String } else { Tok::Key };
                out.push((start..s.i, kind));
                line_start = false;
            }
            '=' => {
                s.bump();
                in_value = true;
                line_start = false;
            }
            '[' | '{' => {
                s.bump();
                nesting += 1;
                if c == '{' {
                    // Keys come next inside an inline table.
                    in_value = false;
                }
            }
            ']' | '}' => {
                s.bump();
                nesting = nesting.saturating_sub(1);
                if nesting > 0 || c == '}' {
                    in_value = true;
                }
            }
            ',' => {
                s.bump();
                // In an inline table a key follows a comma.
                if nesting > 0 && text[..start].rfind('{') > text[..start].rfind('[') {
                    in_value = false;
                }
            }
            _ if !in_value && nesting == 0 || (!in_value && nesting > 0 && (is_ident(c) || c == '-')) => {
                // A bare key (dotted keys included).
                s.eat_while(|d| is_ident(d) || d == '-' || d == '.');
                if s.i == start {
                    s.bump();
                }
                out.push((start..s.i, Tok::Key));
                line_start = false;
            }
            c if c.is_ascii_digit() || ((c == '-' || c == '+') && s.peek_at(1).is_some_and(|d| d.is_ascii_digit())) => {
                s.bump();
                // Numbers, dates and times: digits with their punctuation.
                s.eat_while(|d| d.is_ascii_alphanumeric() || matches!(d, '_' | '.' | ':' | '-' | '+'));
                out.push((start..s.i, Tok::Number));
            }
            c if c.is_ascii_alphabetic() => {
                s.eat_while(|d| d.is_ascii_alphanumeric());
                let word = &text[start..s.i];
                if matches!(word, "true" | "false" | "inf" | "nan") {
                    out.push((start..s.i, Tok::Literal));
                }
            }
            _ => {
                s.bump();
            }
        }
    }
    out
}

fn json(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut s = Scan { s: text, i: 0 };
    while let Some(c) = s.peek() {
        let start = s.i;
        match c {
            '"' => {
                s.bump();
                s.quoted('"', true);
                let end = s.i;
                let is_key = text[end..].trim_start().starts_with(':');
                out.push((start..end, if is_key { Tok::Key } else { Tok::String }));
            }
            '-' | '0'..='9' => {
                s.bump();
                s.eat_while(|d| d.is_ascii_digit() || matches!(d, '.' | 'e' | 'E' | '+' | '-'));
                out.push((start..s.i, Tok::Number));
            }
            c if c.is_ascii_alphabetic() => {
                s.eat_while(|d| d.is_ascii_alphabetic());
                if matches!(&text[start..s.i], "true" | "false" | "null") {
                    out.push((start..s.i, Tok::Literal));
                }
            }
            // JSONC comments, which some fixtures use.
            '/' if s.starts("//") => {
                s.skip_line();
                out.push((start..s.i, Tok::Comment));
            }
            '/' if s.starts("/*") => {
                s.i += 2;
                s.until("*/");
                out.push((start..s.i, Tok::Comment));
            }
            _ => {
                s.bump();
            }
        }
    }
    out
}

const PY_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield", "match", "case", "self",
];

fn python(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut s = Scan { s: text, i: 0 };
    while let Some(c) = s.peek() {
        let start = s.i;
        // String prefixes: r, b, f, u and their pairs.
        let prefix = {
            let mut n = 0;
            while n < 2 && s.peek_at(n).is_some_and(|p| "rRbBfFuU".contains(p)) {
                n += 1;
            }
            if n > 0 && matches!(s.peek_at(n), Some('"' | '\'')) {
                let before = text[..start].chars().next_back();
                if before.is_some_and(is_ident) {
                    0
                } else {
                    n
                }
            } else {
                0
            }
        };
        if prefix > 0 {
            for _ in 0..prefix {
                s.bump();
            }
        }
        let Some(c2) = s.peek() else { break };
        if prefix > 0 || c2 == '"' || c2 == '\'' {
            let q = c2;
            let triple: String = std::iter::repeat_n(q, 3).collect();
            if s.starts(&triple) {
                s.i += 3;
                s.until(&triple);
            } else {
                s.bump();
                s.quoted(q, true);
            }
            out.push((start..s.i, Tok::String));
            continue;
        }
        match c {
            '#' => {
                s.skip_line();
                out.push((start..s.i, Tok::Comment));
            }
            '@' if text[..start].rsplit('\n').next().is_some_and(|l| l.trim().is_empty()) => {
                s.bump();
                s.eat_while(|d| is_ident(d) || d == '.');
                out.push((start..s.i, Tok::Key));
            }
            '0'..='9' => {
                s.number();
                s.eat_while(|d| d == 'j');
                out.push((start..s.i, Tok::Number));
            }
            c if is_ident_start(c) => {
                s.eat_while(is_ident);
                let word = &text[start..s.i];
                if PY_KEYWORDS.contains(&word) {
                    out.push((start..s.i, Tok::Keyword));
                } else if matches!(word, "True" | "False" | "None") {
                    out.push((start..s.i, Tok::Literal));
                }
            }
            _ => {
                s.bump();
            }
        }
    }
    out
}

// -- colours ----------------------------------------------------------------

/// Syntax colours, made from the theme so the editor follows it: keywords
/// in the accent, comments in the quiet text colour, the rest the accent's
/// hue turned around the colour wheel, at a lightness that reads on the
/// edit field's background.
#[derive(Clone, Copy, PartialEq, Hash)]
pub struct Syntax {
    pub text: Color32,
    pub keyword: Color32,
    pub literal: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub key: Color32,
    pub error: Color32,
    pub found: Color32,
    pub back: Color32,
    pub gutter: Color32,
}

impl Syntax {
    pub fn from_look(look: &Look) -> Syntax {
        let back = look.edit_back();
        let dark = luminance(crate::style::c(look.theme.edit.normal.back)) < 0.5;
        let accent = look.accent();
        let turn = |degrees: f32| {
            let mut h = egui::ecolor::Hsva::from(accent);
            h.h = (h.h + degrees / 360.0).rem_euclid(1.0);
            h.s = h.s.clamp(0.45, 0.8);
            h.v = if dark { 0.88 } else { 0.55 };
            Color32::from(h)
        };
        let readable = |c: Color32| {
            // A theme accent too close to the background (a pale accent on
            // white) is darkened or lightened until it reads.
            let mut h = egui::ecolor::Hsva::from(c);
            if dark {
                h.v = h.v.max(0.75);
            } else {
                h.v = h.v.min(0.6);
            }
            Color32::from(h)
        };
        Syntax {
            text: crate::style::c(look.theme.edit.normal.text),
            keyword: readable(accent),
            literal: turn(-60.0),
            string: turn(140.0),
            number: turn(-100.0),
            comment: look.weak_text(),
            key: turn(60.0),
            error: look.error(),
            found: accent.gamma_multiply(0.35),
            back,
            gutter: look.weak_text(),
        }
    }

    fn colour(&self, t: Tok) -> Color32 {
        match t {
            Tok::Keyword => self.keyword,
            Tok::Literal => self.literal,
            Tok::String | Tok::Regex => self.string,
            Tok::Number => self.number,
            Tok::Comment => self.comment,
            Tok::Key | Tok::Table => self.key,
        }
    }
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

// -- the widget ---------------------------------------------------------------

/// A problem to show in the editor: underlined from `column` (1-based; 0
/// for the whole line) to the end of the word there, and marked in the
/// gutter, with `message` on hover.
#[derive(Debug, Clone, PartialEq, Hash)]
pub struct Mark {
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub error: bool,
}

/// What the editor needs from one frame to the next for one buffer.
#[derive(Default)]
pub struct EditorState {
    /// The last layout job and the hash of what it was built from.
    job: Option<(u64, Arc<LayoutJob>)>,
    find: Option<Find>,
    goto: Option<String>,
    /// Move the cursor here (1-based line and column) at the next frame.
    jump: Option<(u32, u32)>,
    /// Select this character range at the next frame.
    select: Option<Range<usize>>,
    /// The cursor's line and column, for the status line.
    pub cursor: (u32, u32),
}

struct Find {
    query: String,
    focus: bool,
    /// The match last selected, as a character index.
    at: usize,
}

/// What happened in a frame.
#[derive(Default)]
pub struct Output {
    pub changed: bool,
}

impl EditorState {
    /// Put the cursor at `line:column` (1-based; a column of 0 is the start
    /// of the line) and scroll to it.
    pub fn jump_to(&mut self, line: u32, column: u32) {
        self.jump = Some((line.max(1), column));
    }

    pub fn open_find(&mut self) {
        let f = self.find.get_or_insert(Find { query: String::new(), focus: true, at: 0 });
        f.focus = true;
        self.goto = None;
    }

    pub fn open_goto(&mut self) {
        self.goto = Some(String::new());
        self.find = None;
    }
}

/// Labels the editor shows, in the interface language.
pub struct Labels {
    pub find: String,
    pub next: String,
    pub previous: String,
    pub go_to_line: String,
    pub matches: String,
    pub no_matches: String,
}

/// Draw the editor for `text` in the rest of `ui`.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    look: &Look,
    labels: &Labels,
    st: &mut EditorState,
    id_salt: &str,
    lang: Lang,
    text: &mut String,
    marks: &[Mark],
) -> Output {
    let mut out = Output::default();
    let id = ui.make_persistent_id(("code-editor", id_salt));
    let edit_id = id.with("text");
    let syntax = Syntax::from_look(look);
    let font = FontId::monospace(look.text_size * 1.1);

    shortcuts(ui, st, edit_id);
    if st.find.is_some() {
        find_bar(ui, look, labels, st, edit_id, text);
    }
    if st.goto.is_some() {
        goto_bar(ui, labels, st);
    }
    if keys(ui, edit_id, lang, text) {
        out.changed = true;
    }
    apply_jump(ui.ctx(), st, edit_id, text);

    let line_count = text.split('\n').count().max(1);
    let digits = line_count.to_string().len().max(2);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let gutter_w = char_w * digits as f32 + 20.0;
    let query = st.find.as_ref().map(|f| f.query.clone()).filter(|q| !q.is_empty());

    let frame = egui::Frame::NONE.fill(syntax.back).corner_radius(4.0);
    frame.show(ui, |ui| {
        egui::ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter, _) =
                    ui.allocate_exact_size(egui::vec2(gutter_w, ui.available_height()), egui::Sense::hover());
                let mut job_cache = st.job.take();
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                    let source = buf.as_str();
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (source, lang, marks, &query, syntax, font.size.to_bits()).hash(&mut h);
                    let key = h.finish();
                    let job = match &job_cache {
                        Some((k, j)) if *k == key => j.clone(),
                        _ => {
                            let j = Arc::new(layout_job(source, lang, marks, query.as_deref(), &syntax, &font));
                            job_cache = Some((key, j.clone()));
                            j
                        }
                    };
                    ui.fonts_mut(|f| f.layout_job((*job).clone()))
                };
                let edit = egui::TextEdit::multiline(text)
                    .id(edit_id)
                    .font(font.clone())
                    .code_editor()
                    .lock_focus(true)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(line_count.max(20))
                    .layouter(&mut layouter);
                let shown = edit.show(ui);
                st.job = job_cache;
                if shown.response.response.changed() {
                    out.changed = true;
                }
                if let Some(range) = shown.cursor_range {
                    let at = range.primary.index.0;
                    st.cursor = char_line_col(text, at);
                }
                paint_gutter(ui, &syntax, &font, gutter, &shown, marks);
                // The place a jump asked for: scroll there now that the
                // galley says where it is.
                if let Some(ch) = ui.ctx().data_mut(|d| d.remove_temp::<usize>(edit_id.with("reveal"))) {
                    let r = shown.galley.pos_from_cursor(CCursor::new(ch)).translate(shown.galley_pos.to_vec2());
                    ui.scroll_to_rect(r.expand(24.0), Some(egui::Align::Center));
                }
            });
        });
    });
    out
}

/// Ctrl/Cmd+F and Ctrl/Cmd+G, while the editor or its bars have the focus.
fn shortcuts(ui: &mut Ui, st: &mut EditorState, edit_id: egui::Id) {
    let focused = ui.memory(|m| m.has_focus(edit_id)) || st.find.is_some() || st.goto.is_some();
    if !focused {
        return;
    }
    if ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        st.open_find();
    }
    if ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::G))) {
        st.open_goto();
    }
    if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        st.find = None;
        st.goto = None;
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
}

fn find_bar(ui: &mut Ui, look: &Look, labels: &Labels, st: &mut EditorState, edit_id: egui::Id, text: &str) {
    let Some(f) = st.find.as_mut() else { return };
    let mut step: Option<bool> = None;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(&labels.find);
        let r = ui.add(egui::TextEdit::singleline(&mut f.query).desired_width(220.0));
        if std::mem::take(&mut f.focus) {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            step = Some(!ui.input(|i| i.modifiers.shift));
            r.request_focus();
        }
        if ui.button(&labels.previous).clicked() {
            step = Some(false);
        }
        if ui.button(&labels.next).clicked() {
            step = Some(true);
        }
        let count = if f.query.is_empty() { 0 } else { find_all(text, &f.query).len() };
        if !f.query.is_empty() {
            let note =
                if count == 0 { labels.no_matches.clone() } else { labels.matches.replace("%s", &count.to_string()) };
            ui.label(egui::RichText::new(note).small());
        }
        if crate::icons::sized_button(ui, look, crate::icons::Icon::Close, "", 16.0, true).clicked() {
            close = true;
        }
    });
    if close {
        st.find = None;
        ui.memory_mut(|m| m.request_focus(edit_id));
        return;
    }
    if let Some(forward) = step {
        let hits = find_all(text, &f.query);
        if hits.is_empty() {
            return;
        }
        let pick = if forward {
            hits.iter().find(|r| r.start > f.at).or(hits.first())
        } else {
            hits.iter().rev().find(|r| r.start < f.at).or(hits.last())
        };
        if let Some(r) = pick.cloned() {
            f.at = r.start;
            st.select = Some(r);
        }
    }
}

fn goto_bar(ui: &mut Ui, labels: &Labels, st: &mut EditorState) {
    let mut go = None;
    let mut close = false;
    if let Some(line) = st.goto.as_mut() {
        ui.horizontal(|ui| {
            ui.label(&labels.go_to_line);
            let r = ui.add(egui::TextEdit::singleline(line).desired_width(80.0));
            if !r.has_focus() && !r.lost_focus() && line.is_empty() {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = line.trim().parse::<u32>().ok();
                close = true;
            }
        });
    }
    if let Some(l) = go {
        st.jump_to(l, 0);
    }
    if close {
        st.goto = None;
    }
}

/// Every place `query` occurs, as character ranges, ignoring case.
pub fn find_all(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    // Lowercasing can change lengths (rare outside a few scripts); fall back
    // to an exact search then, so ranges stay true to the text.
    let (hay, needle) = if hay.len() == text.chars().count() {
        (hay, needle)
    } else {
        (text.chars().collect(), query.chars().collect())
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            out.push(i..i + needle.len());
            i += needle.len().max(1);
        } else {
            i += 1;
        }
    }
    out
}

/// Move the cursor or selection where a jump or a find asked for.
fn apply_jump(ctx: &egui::Context, st: &mut EditorState, edit_id: egui::Id, text: &str) {
    let range = if let Some((line, col)) = st.jump.take() {
        let at = char_index_of(text, line, col);
        Some(at..at)
    } else {
        st.select.take()
    };
    let Some(range) = range else { return };
    let mut state = TextEditState::load(ctx, edit_id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end))));
    state.store(ctx, edit_id);
    ctx.memory_mut(|m| m.request_focus(edit_id));
    ctx.data_mut(|d| d.insert_temp(edit_id.with("reveal"), range.end));
}

/// The character index of 1-based `line` and `column` (0: the line's start),
/// clamped to the text.
pub fn char_index_of(text: &str, line: u32, column: u32) -> usize {
    let mut index = 0;
    for (n, l) in text.split('\n').enumerate() {
        let len = l.chars().count();
        if n + 1 == line as usize {
            return index + (column.saturating_sub(1) as usize).min(len);
        }
        index += len + 1;
    }
    text.chars().count()
}

/// 1-based line and column of character index `at`.
pub fn char_line_col(text: &str, at: usize) -> (u32, u32) {
    let mut line = 1;
    let mut col = 1;
    for c in text.chars().take(at) {
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The keys `TextEdit` does not do the way a code editor should: Tab and
/// Shift+Tab indent with spaces, Enter keeps the indentation, Ctrl/Cmd+/
/// toggles line comments. Returns whether the text changed.
fn keys(ui: &mut Ui, edit_id: egui::Id, lang: Lang, text: &mut String) -> bool {
    if !ui.memory(|m| m.has_focus(edit_id)) {
        return false;
    }
    let Some(mut state) = TextEditState::load(ui.ctx(), edit_id) else { return false };
    let Some(range) = state.cursor.char_range() else { return false };
    let [a, b] = range.sorted_cursors();
    let sel = a.index.0..b.index.0;
    let edit = if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab)) {
        Some(indent_lines(text, sel, false))
    } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
        Some(if text_spans_lines(text, &sel) { indent_lines(text, sel, true) } else { insert(text, sel, INDENT) })
    } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
        let line_start = line_start_of(text, sel.start);
        let current: String = text.chars().skip(line_start).take(sel.start - line_start).collect();
        let mut indent: String = current.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let opens = current.trim_end().ends_with(['{', '[', '('])
            || (lang == Lang::Python && current.trim_end().ends_with(':'));
        if opens {
            indent.push_str(INDENT);
        }
        Some(insert(text, sel, &format!("\n{indent}")))
    } else if ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash))) {
        lang.line_comment().map(|prefix| toggle_comment(text, sel, prefix))
    } else {
        None
    };
    let Some((new_text, new_sel)) = edit else { return false };
    crate::text_menu::remember_for_undo(ui.ctx(), edit_id, text);
    *text = new_text;
    // The undo step above was stored in the state; load it again before
    // moving the cursor so it is kept.
    if let Some(s) = TextEditState::load(ui.ctx(), edit_id) {
        state = s;
    }
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(new_sel.start), CCursor::new(new_sel.end))));
    state.store(ui.ctx(), edit_id);
    true
}

fn text_spans_lines(text: &str, sel: &Range<usize>) -> bool {
    sel.start != sel.end && text.chars().skip(sel.start).take(sel.end - sel.start).any(|c| c == '\n')
}

/// The character index where the line holding `at` starts.
fn line_start_of(text: &str, at: usize) -> usize {
    let mut start = 0;
    for (i, c) in text.chars().enumerate().take(at) {
        if c == '\n' {
            start = i + 1;
        }
    }
    start
}

fn byte_of(text: &str, ch: usize) -> usize {
    text.char_indices().nth(ch).map_or(text.len(), |(i, _)| i)
}

/// `with` in place of the selection; the cursor after it.
pub fn insert(text: &str, sel: Range<usize>, with: &str) -> (String, Range<usize>) {
    let (a, b) = (byte_of(text, sel.start), byte_of(text, sel.end));
    let out = format!("{}{with}{}", &text[..a], &text[b..]);
    let at = sel.start + with.chars().count();
    (out, at..at)
}

/// The lines the selection touches, as 0-based line numbers.
fn selected_lines(text: &str, sel: &Range<usize>) -> Range<usize> {
    let first = text.chars().take(sel.start).filter(|c| *c == '\n').count();
    // A selection ending at the very start of a line leaves that line out.
    let mut end = sel.end;
    if end > sel.start && text.chars().nth(end - 1) == Some('\n') {
        end -= 1;
    }
    let last = text.chars().take(end).filter(|c| *c == '\n').count();
    first..last + 1
}

/// Indent (or dedent) every line the selection touches; the whole lines
/// stay selected.
pub fn indent_lines(text: &str, sel: Range<usize>, indent: bool) -> (String, Range<usize>) {
    let lines = selected_lines(text, &sel);
    let mut out = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        if !lines.contains(&n) {
            out.push(line.to_owned());
        } else if indent {
            out.push(format!("{INDENT}{line}"));
        } else {
            let strip = line.chars().take(INDENT.len()).take_while(|c| *c == ' ').count();
            let strip = if strip == 0 && line.starts_with('\t') { 1 } else { strip };
            out.push(line[strip..].to_owned());
        }
    }
    let joined = out.join("\n");
    let range = whole_lines(&joined, &lines);
    (joined, range)
}

/// The character range covering lines `lines` (0-based) of `text`.
fn whole_lines(text: &str, lines: &Range<usize>) -> Range<usize> {
    let mut start = 0;
    let mut index = 0;
    for (n, l) in text.split('\n').enumerate() {
        let len = l.chars().count();
        if n == lines.start {
            start = index;
        }
        if n + 1 == lines.end {
            return start..index + len;
        }
        index += len + 1;
    }
    start..text.chars().count()
}

/// Comment out the lines the selection touches with `prefix`, or remove it
/// when every non-empty one already has it.
pub fn toggle_comment(text: &str, sel: Range<usize>, prefix: &str) -> (String, Range<usize>) {
    let lines = selected_lines(text, &sel);
    let all: Vec<&str> = text.split('\n').collect();
    let chosen: Vec<&&str> = all.iter().enumerate().filter(|(n, _)| lines.contains(n)).map(|(_, l)| l).collect();
    let uncomment = chosen.iter().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with(prefix))
        && !chosen.is_empty();
    // Comments go at the selection's smallest indentation, so the code
    // stays aligned.
    let indent =
        chosen.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let out: Vec<String> = all
        .iter()
        .enumerate()
        .map(|(n, l)| {
            if !lines.contains(&n) || l.trim().is_empty() {
                return (*l).to_owned();
            }
            if uncomment {
                let at = l.find(prefix).unwrap_or(0);
                let mut rest = &l[at + prefix.len()..];
                if rest.starts_with(' ') {
                    rest = &rest[1..];
                }
                format!("{}{rest}", &l[..at])
            } else {
                let cut = indent.min(l.len());
                format!("{}{prefix} {}", &l[..cut], &l[cut..])
            }
        })
        .collect();
    let joined = out.join("\n");
    let range = whole_lines(&joined, &lines);
    (joined, range)
}

/// The coloured layout of `text`.
fn layout_job(
    text: &str,
    lang: Lang,
    marks: &[Mark],
    query: Option<&str>,
    syntax: &Syntax,
    font: &FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let spans = tokenize(lang, text);

    // Per-byte decorations: underlined ranges and found ranges (bytes).
    let underlines: Vec<Range<usize>> = marks.iter().filter_map(|m| mark_range(text, m)).collect();
    let found: Vec<Range<usize>> = query
        .map(|q| find_all(text, q).into_iter().map(|r| byte_of(text, r.start)..byte_of(text, r.end)).collect())
        .unwrap_or_default();

    // Cut the text at every boundary of a span, an underline or a match.
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for (r, _) in &spans {
        cuts.extend([r.start, r.end]);
    }
    for r in underlines.iter().chain(&found) {
        cuts.extend([r.start, r.end]);
    }
    cuts.retain(|c| *c <= text.len() && text.is_char_boundary(*c));
    cuts.sort_unstable();
    cuts.dedup();

    let mut span_i = 0;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        while span_i < spans.len() && spans[span_i].0.end <= a {
            span_i += 1;
        }
        let tok = spans.get(span_i).filter(|(r, _)| r.start <= a && b <= r.end).map(|(_, t)| *t);
        let mut fmt = TextFormat::simple(font.clone(), tok.map_or(syntax.text, |t| syntax.colour(t)));
        if tok == Some(Tok::Comment) {
            fmt.italics = true;
        }
        if underlines.iter().any(|r| r.start <= a && b <= r.end) {
            fmt.underline = Stroke::new(1.5, syntax.error);
        }
        if found.iter().any(|r| r.start <= a && b <= r.end) {
            fmt.background = syntax.found;
        }
        job.append(&text[a..b], 0.0, fmt);
    }
    if text.is_empty() {
        job.append("", 0.0, TextFormat::simple(font.clone(), syntax.text));
    }
    job
}

/// The byte range a mark underlines: from its column to the end of the word
/// there (at least one character), or the whole line without a column.
fn mark_range(text: &str, m: &Mark) -> Option<Range<usize>> {
    let line_start = text.split('\n').take(m.line.saturating_sub(1) as usize).map(|l| l.len() + 1).sum::<usize>();
    let line = text.get(line_start..)?.split('\n').next()?;
    if line.trim().is_empty() {
        return None;
    }
    if m.column == 0 {
        let lead = line.len() - line.trim_start().len();
        return Some(line_start + lead..line_start + line.trim_end().len());
    }
    let start = line.char_indices().nth(m.column as usize - 1).map_or(line.len(), |(i, _)| i);
    let rest = &line[start..];
    let word = rest.char_indices().find(|(_, c)| !is_ident(*c)).map_or(rest.len(), |(i, _)| i);
    let len = if word == 0 { rest.chars().next().map_or(0, char::len_utf8) } else { word };
    if len == 0 {
        // At the end of the line: underline the last character.
        let back = line.char_indices().next_back().map_or(0, |(i, _)| i);
        return Some(line_start + back..line_start + line.len());
    }
    Some(line_start + start..line_start + start + len)
}

/// Line numbers beside the text, and a mark on lines with a problem.
fn paint_gutter(
    ui: &Ui,
    syntax: &Syntax,
    font: &FontId,
    gutter: egui::Rect,
    shown: &egui::widgets::text_edit::TextEditOutput,
    marks: &[Mark],
) {
    let painter = ui.painter_at(gutter.expand2(egui::vec2(0.0, 4000.0)).intersect(ui.clip_rect()));
    let origin = shown.galley_pos;
    let clip = ui.clip_rect();
    let small = FontId::monospace(font.size * 0.9);
    let pointer = ui.ctx().pointer_hover_pos();
    for (n, row) in shown.galley.rows.iter().enumerate() {
        let r = row.rect().translate(origin.to_vec2());
        if r.bottom() < clip.top() || r.top() > clip.bottom() {
            continue;
        }
        let line = n as u32 + 1;
        let marked: Vec<&Mark> = marks.iter().filter(|m| m.line == line).collect();
        let colour = if marked.is_empty() { syntax.gutter } else { syntax.error };
        painter.text(
            egui::pos2(gutter.right() - 12.0, r.center().y),
            egui::Align2::RIGHT_CENTER,
            line.to_string(),
            small.clone(),
            colour,
        );
        if let Some(first) = marked.first() {
            let dot = egui::pos2(gutter.right() - 4.0, r.center().y);
            let colour = if first.error { syntax.error } else { syntax.gutter };
            painter.circle_filled(dot, 2.5, colour);
            let row_rect = egui::Rect::from_x_y_ranges(gutter.x_range(), r.y_range());
            if pointer.is_some_and(|p| row_rect.contains(p)) {
                let text: Vec<&str> = marked.iter().map(|m| m.message.as_str()).collect();
                ui.interact(row_rect, ui.scope_id().with(("gutter-mark", line)), egui::Sense::hover())
                    .on_hover_text(text.join("\n"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Lang, text: &str) -> Vec<(String, Tok)> {
        tokenize(lang, text).into_iter().map(|(r, t)| (text[r].to_owned(), t)).collect()
    }

    fn has(spans: &[(String, Tok)], text: &str, tok: Tok) -> bool {
        spans.iter().any(|(s, t)| s == text && *t == tok)
    }

    #[test]
    fn javascript_tokens() {
        let src = "// note\nfunction f(a) {\n  var s = 'it\\'s' + \"x\";\n  return /a[/]b/gi.test(s) ? 0x1F : 3.5e-2 / 2;\n}\n/* end */";
        let k = kinds(Lang::JavaScript, src);
        assert!(has(&k, "// note", Tok::Comment));
        assert!(has(&k, "function", Tok::Keyword));
        assert!(has(&k, "var", Tok::Keyword));
        assert!(has(&k, "'it\\'s'", Tok::String));
        assert!(has(&k, "\"x\"", Tok::String));
        assert!(has(&k, "/a[/]b/gi", Tok::Regex), "{k:?}");
        assert!(has(&k, "0x1F", Tok::Number));
        assert!(has(&k, "3.5e-2", Tok::Number));
        assert!(has(&k, "2", Tok::Number), "a division is not a regex: {k:?}");
        assert!(has(&k, "/* end */", Tok::Comment));
        assert!(!k.iter().any(|(s, _)| s == "test"), "a method name is plain");
    }

    #[test]
    fn javascript_template_literals_hold_code() {
        let src = "const t = `a ${b + `in ${c}`} z`; if (x) {}";
        let k = kinds(Lang::JavaScript, src);
        assert!(has(&k, "`a ${", Tok::String), "{k:?}");
        assert!(has(&k, "} z`", Tok::String), "{k:?}");
        assert!(has(&k, "`in ${", Tok::String), "{k:?}");
        assert!(has(&k, "}`", Tok::String), "{k:?}");
        assert!(has(&k, "if", Tok::Keyword), "the code after the template is code again: {k:?}");
        assert!(!k.iter().any(|(s, t)| s.contains('b') && *t == Tok::String && s.contains('+')), "{k:?}");
    }

    #[test]
    fn javascript_regex_or_division() {
        let k = kinds(Lang::JavaScript, "a = b / c / d; e = (x) / 2; g = a[1] / 3;");
        assert!(!k.iter().any(|(_, t)| *t == Tok::Regex), "{k:?}");
        let k = kinds(Lang::JavaScript, "return /x+/.test(s); y = obj.return / 2;");
        assert!(has(&k, "/x+/", Tok::Regex), "{k:?}");
        let k = kinds(Lang::JavaScript, "f = [/re/];");
        assert!(has(&k, "/re/", Tok::Regex), "{k:?}");
        // An unterminated string stops at the end of its line.
        let k = kinds(Lang::JavaScript, "var s = 'open\nvar t;");
        assert!(has(&k, "'open", Tok::String) && has(&k, "var", Tok::Keyword), "{k:?}");
    }

    #[test]
    fn toml_tokens() {
        let src = "# top\ncapabilities = [\"translate\"]\n\n[module]\nid = \"x\" # trailing\nmax = 5\non = true\nwhen = 1979-05-27\n\n[[options]]\n\"quoted key\" = 'lit'\ninline = { a = 1, b = \"two\" }\ntext = \"\"\"\nmany\nlines\"\"\"\n";
        let k = kinds(Lang::Toml, src);
        assert!(has(&k, "# top", Tok::Comment));
        assert!(has(&k, "capabilities", Tok::Key));
        assert!(has(&k, "\"translate\"", Tok::String));
        assert!(has(&k, "[module]", Tok::Table));
        assert!(has(&k, "[[options]]", Tok::Table));
        assert!(has(&k, "\"x\"", Tok::String));
        assert!(has(&k, "# trailing", Tok::Comment));
        assert!(has(&k, "5", Tok::Number));
        assert!(has(&k, "true", Tok::Literal));
        assert!(has(&k, "1979-05-27", Tok::Number));
        assert!(has(&k, "\"quoted key\"", Tok::Key), "{k:?}");
        assert!(has(&k, "'lit'", Tok::String));
        assert!(has(&k, "a", Tok::Key) && has(&k, "b", Tok::Key), "inline table keys: {k:?}");
        assert!(has(&k, "\"two\"", Tok::String));
        assert!(has(&k, "\"\"\"\nmany\nlines\"\"\"", Tok::String), "{k:?}");
    }

    #[test]
    fn json_tokens() {
        let k = kinds(Lang::Json, "{\"a\": [1, -2.5e3, true, null], \"b\" : \"s\\\"q\"}");
        assert!(has(&k, "\"a\"", Tok::Key));
        assert!(has(&k, "\"b\"", Tok::Key));
        assert!(has(&k, "\"s\\\"q\"", Tok::String));
        assert!(has(&k, "-2.5e3", Tok::Number));
        assert!(has(&k, "true", Tok::Literal) && has(&k, "null", Tok::Literal));
    }

    #[test]
    fn python_tokens() {
        let src = "@mod.translate\ndef translate(req, host):\n    s = f'x{y}' + r\"\\d\" # c\n    return None if not s else \"\"\"doc\"\"\"\n";
        let k = kinds(Lang::Python, src);
        assert!(has(&k, "@mod.translate", Tok::Key));
        assert!(has(&k, "def", Tok::Keyword));
        assert!(has(&k, "f'x{y}'", Tok::String), "{k:?}");
        assert!(has(&k, "r\"\\d\"", Tok::String), "{k:?}");
        assert!(has(&k, "# c", Tok::Comment));
        assert!(has(&k, "None", Tok::Literal));
        assert!(has(&k, "\"\"\"doc\"\"\"", Tok::String));
    }

    #[test]
    fn spans_are_ordered_and_on_char_boundaries() {
        for (lang, src) in [
            (Lang::JavaScript, "var ж = 'привет'; // ünïcode\n`${ж}`"),
            (Lang::Toml, "ключ = \"значение\" # é"),
            (Lang::Json, "{\"ключ\": \"é\"}"),
            (Lang::Python, "s = 'ж' # ü"),
        ] {
            let spans = tokenize(lang, src);
            let mut end = 0;
            for (r, _) in &spans {
                assert!(r.start >= end && r.start < r.end, "{lang:?}: {spans:?}");
                assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end));
                end = r.end;
            }
        }
    }

    #[test]
    fn editing_helpers() {
        let (t, sel) = indent_lines("a\nb\nc", 0..3, true);
        assert_eq!(t, "  a\n  b\nc");
        assert_eq!(sel, 0..7);
        let (t, _) = indent_lines(&t, 0..7, false);
        assert_eq!(t, "a\nb\nc");
        let (t, _) = toggle_comment("  x = 1\n  y = 2\nz", 0..9, "//");
        assert_eq!(t, "  // x = 1\n  // y = 2\nz");
        let (t, _) = toggle_comment(&t, 0..12, "//");
        assert_eq!(t, "  x = 1\n  y = 2\nz");
        assert_eq!(insert("ab", 1..1, "--").0, "a--b");
        assert_eq!(char_index_of("ab\ncd\nef", 2, 2), 4);
        assert_eq!(char_index_of("ab\ncd", 9, 1), 5);
        assert_eq!(char_line_col("ab\ncd", 4), (2, 2));
        assert_eq!(find_all("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
    }

    #[test]
    fn marks_underline_the_word_at_their_column() {
        let text = "var a = 1;\n  return optons.x;\n";
        let m = Mark { line: 2, column: 10, message: String::new(), error: true };
        assert_eq!(&text[mark_range(text, &m).unwrap()], "optons");
        let whole = Mark { column: 0, ..m.clone() };
        assert_eq!(&text[mark_range(text, &whole).unwrap()], "return optons.x;");
        let past = Mark { column: 99, ..m };
        assert_eq!(&text[mark_range(text, &past).unwrap()], ";");
    }
}
