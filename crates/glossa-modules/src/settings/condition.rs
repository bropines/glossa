//! The tiny expression language of `visible_if` and `enabled_if`.
//!
//! ```text
//! options.api_key                       truthy
//! !state.result                         falsy
//! options.tone == 'casual'              equality with a string, number or bool
//! options.temperature > 0.5             < <= > >= on numbers
//! options.mode == 'custom' && options.url
//! ```
//!
//! A path (`options.<key>` or `state.<key>`, with further `.field` steps into
//! objects), a literal (`'text'`, `"text"`, a number, `true`, `false`,
//! `null`), `== != < <= > >=`, `!`, `&&`, `||` and parentheses. No calls and
//! no arithmetic: conditions are evaluated natively every frame, so they must
//! stay cheap and cannot run module code.

use serde_json::Value;

/// Where a path looks its value up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The module's settings (the draft, on the settings page).
    Options,
    /// A panel's state (phase 3). On the settings page every `state.*` path
    /// is `null`.
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A parsed condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Path(Scope, Vec<String>),
    Literal(Value),
    Not(Box<Condition>),
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Cmp(CmpOp, Box<Condition>, Box<Condition>),
}

/// The values a condition reads.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub options: &'a crate::provider::OptionValues,
    pub state: Option<&'a serde_json::Map<String, Value>>,
}

impl Condition {
    /// Parse `src`. The error says what was expected and where.
    pub fn parse(src: &str) -> Result<Condition, String> {
        let tokens = tokenize(src)?;
        let mut p = Parser { tokens, at: 0 };
        let c = p.or()?;
        match p.peek() {
            None => Ok(c),
            Some(t) => Err(format!("unexpected {} at character {}", t.kind.describe(), t.pos + 1)),
        }
    }

    /// Whether the condition holds.
    pub fn eval(&self, cx: &Context) -> bool {
        truthy(&self.value(cx))
    }

    fn value(&self, cx: &Context) -> Value {
        match self {
            Condition::Path(scope, steps) => lookup(cx, *scope, steps).cloned().unwrap_or(Value::Null),
            Condition::Literal(v) => v.clone(),
            Condition::Not(c) => Value::Bool(!c.eval(cx)),
            Condition::And(a, b) => Value::Bool(a.eval(cx) && b.eval(cx)),
            Condition::Or(a, b) => Value::Bool(a.eval(cx) || b.eval(cx)),
            Condition::Cmp(op, a, b) => Value::Bool(compare(*op, &a.value(cx), &b.value(cx))),
        }
    }

    /// Every path the condition reads, for the manifest lint.
    pub fn paths(&self) -> Vec<(Scope, &[String])> {
        let mut out = Vec::new();
        self.collect_paths(&mut out);
        out
    }

    fn collect_paths<'a>(&'a self, out: &mut Vec<(Scope, &'a [String])>) {
        match self {
            Condition::Path(s, steps) => out.push((*s, steps)),
            Condition::Literal(_) => {}
            Condition::Not(c) => c.collect_paths(out),
            Condition::And(a, b) | Condition::Or(a, b) | Condition::Cmp(_, a, b) => {
                a.collect_paths(out);
                b.collect_paths(out);
            }
        }
    }
}

fn lookup<'a>(cx: &Context<'a>, scope: Scope, steps: &[String]) -> Option<&'a Value> {
    let (first, rest) = steps.split_first()?;
    let mut v = match scope {
        Scope::Options => cx.options.get(first)?,
        Scope::State => cx.state?.get(first)?,
    };
    for step in rest {
        v = v.as_object()?.get(step)?;
    }
    Some(v)
}

/// JavaScript-like truthiness, minus its surprises: `null`, `false`, `0`,
/// an empty or blank string and an empty list or object are false.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A number, or a string holding one: text fields store numbers as text.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn compare(op: CmpOp, a: &Value, b: &Value) -> bool {
    let equal = || match (a, b) {
        (Value::Number(_), _) | (_, Value::Number(_)) => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        // An unset option equals `null` and `false` but not `''`, which is
        // a value the user typed and cleared.
        (Value::Null, Value::Bool(false)) | (Value::Bool(false), Value::Null) => true,
        _ => a == b,
    };
    match op {
        CmpOp::Eq => equal(),
        CmpOp::Ne => !equal(),
        _ => {
            let (Some(x), Some(y)) = (number(a), number(b)) else { return false };
            match op {
                CmpOp::Lt => x < y,
                CmpOp::Le => x <= y,
                CmpOp::Gt => x > y,
                CmpOp::Ge => x >= y,
                CmpOp::Eq | CmpOp::Ne => unreachable!(),
            }
        }
    }
}

// -- tokens ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Path(Scope, Vec<String>),
    Literal(Value),
    Op(CmpOp),
    Not,
    And,
    Or,
    Open,
    Close,
}

impl Kind {
    fn describe(&self) -> String {
        match self {
            Kind::Path(..) => "a path".into(),
            Kind::Literal(v) => format!("the value {v}"),
            Kind::Op(_) => "a comparison".into(),
            Kind::Not => "`!`".into(),
            Kind::And => "`&&`".into(),
            Kind::Or => "`||`".into(),
            Kind::Open => "`(`".into(),
            Kind::Close => "`)`".into(),
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    pos: usize,
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        let next = chars.get(i + 1).map(|&(_, c)| c);
        let two = |k: Kind, out: &mut Vec<Token>| out.push(Token { kind: k, pos });
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                two(Kind::Open, &mut out);
                i += 1;
            }
            ')' => {
                two(Kind::Close, &mut out);
                i += 1;
            }
            '&' | '|' if next == Some(c) => {
                two(if c == '&' { Kind::And } else { Kind::Or }, &mut out);
                i += 2;
            }
            '=' if next == Some('=') => {
                two(Kind::Op(CmpOp::Eq), &mut out);
                i += 2;
            }
            '!' if next == Some('=') => {
                two(Kind::Op(CmpOp::Ne), &mut out);
                i += 2;
            }
            '!' => {
                two(Kind::Not, &mut out);
                i += 1;
            }
            '<' | '>' => {
                let eq = next == Some('=');
                let op = match (c, eq) {
                    ('<', false) => CmpOp::Lt,
                    ('<', true) => CmpOp::Le,
                    ('>', false) => CmpOp::Gt,
                    _ => CmpOp::Ge,
                };
                two(Kind::Op(op), &mut out);
                i += if eq { 2 } else { 1 };
            }
            '\'' | '"' => {
                let mut s = String::new();
                let mut j = i + 1;
                loop {
                    let Some(&(_, ch)) = chars.get(j) else {
                        return Err(format!("the string starting at character {} is not closed", pos + 1));
                    };
                    j += 1;
                    if ch == c {
                        break;
                    }
                    if ch == '\\' {
                        if let Some(&(_, esc)) = chars.get(j) {
                            s.push(esc);
                            j += 1;
                            continue;
                        }
                    }
                    s.push(ch);
                }
                out.push(Token { kind: Kind::Literal(Value::String(s)), pos });
                i = j;
            }
            c if c.is_ascii_digit() || (c == '-' && next.is_some_and(|n| n.is_ascii_digit())) => {
                let mut j = i + 1;
                while chars.get(j).is_some_and(|&(_, ch)| ch.is_ascii_digit() || ch == '.') {
                    j += 1;
                }
                let end = chars.get(j).map_or(src.len(), |&(p, _)| p);
                let text = &src[pos..end];
                let n: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
                out.push(Token { kind: Kind::Literal(serde_json::json!(n)), pos });
                i = j;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut j = i + 1;
                while chars.get(j).is_some_and(|&(_, ch)| ch.is_ascii_alphanumeric() || "_-.".contains(ch)) {
                    j += 1;
                }
                let end = chars.get(j).map_or(src.len(), |&(p, _)| p);
                let word = &src[pos..end];
                let kind = match word {
                    "true" => Kind::Literal(Value::Bool(true)),
                    "false" => Kind::Literal(Value::Bool(false)),
                    "null" => Kind::Literal(Value::Null),
                    _ => {
                        let mut steps = word.split('.');
                        let scope = match steps.next() {
                            Some("options") => Scope::Options,
                            Some("state") => Scope::State,
                            _ => {
                                return Err(format!(
                                    "{word:?} at character {}: a path starts with `options.` or `state.`",
                                    pos + 1
                                ))
                            }
                        };
                        let steps: Vec<String> = steps.map(str::to_owned).collect();
                        if steps.is_empty() || steps.iter().any(String::is_empty) {
                            return Err(format!(
                                "{word:?} at character {}: name a key, as in options.api_key",
                                pos + 1
                            ));
                        }
                        Kind::Path(scope, steps)
                    }
                };
                out.push(Token { kind, pos });
                i = j;
            }
            _ => return Err(format!("unexpected {c:?} at character {}", pos + 1)),
        }
    }
    Ok(out)
}

// -- parser ---------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, kind: &Kind) -> bool {
        if self.peek().is_some_and(|t| &t.kind == kind) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Condition, String> {
        let mut left = self.and()?;
        while self.eat(&Kind::Or) {
            left = Condition::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Condition, String> {
        let mut left = self.cmp()?;
        while self.eat(&Kind::And) {
            left = Condition::And(Box::new(left), Box::new(self.cmp()?));
        }
        Ok(left)
    }

    fn cmp(&mut self) -> Result<Condition, String> {
        let left = self.unary()?;
        if let Some(Token { kind: Kind::Op(op), .. }) = self.peek() {
            let op = *op;
            self.at += 1;
            let right = self.unary()?;
            if let Some(Token { kind: Kind::Op(_), pos }) = self.peek() {
                return Err(format!("comparisons cannot be chained (character {}); use &&", pos + 1));
            }
            return Ok(Condition::Cmp(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Condition, String> {
        if self.eat(&Kind::Not) {
            return Ok(Condition::Not(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Condition, String> {
        let Some(t) = self.tokens.get(self.at).cloned() else {
            return Err("the condition ends too early".into());
        };
        self.at += 1;
        match t.kind {
            Kind::Path(s, steps) => Ok(Condition::Path(s, steps)),
            Kind::Literal(v) => Ok(Condition::Literal(v)),
            Kind::Open => {
                let inner = self.or()?;
                if !self.eat(&Kind::Close) {
                    return Err(format!("the `(` at character {} is not closed", t.pos + 1));
                }
                Ok(inner)
            }
            other => Err(format!("unexpected {} at character {}", other.describe(), t.pos + 1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(src: &str, options: Value) -> bool {
        let options: crate::provider::OptionValues = serde_json::from_value(options).unwrap();
        let state = serde_json::Map::from_iter([("result".to_owned(), json!("done"))]);
        Condition::parse(src).unwrap().eval(&Context { options: &options, state: Some(&state) })
    }

    #[test]
    fn evaluates_a_table_of_cases() {
        let o = json!({
            "api_key": "ak-1", "empty": "", "blank": "  ", "tone": "casual", "temperature": 0.7,
            "count": "3", "on": true, "off": false, "list": [], "nested": { "a": { "b": 2 } }
        });
        let cases: &[(&str, bool)] = &[
            ("options.api_key", true),
            ("options.empty", false),
            ("options.blank", false),
            ("options.missing", false),
            ("!options.missing", true),
            ("!!options.api_key", true),
            ("options.tone == 'casual'", true),
            ("options.tone == \"casual\"", true),
            ("options.tone != 'casual'", false),
            ("options.temperature > 0.5", true),
            ("options.temperature >= 0.7", true),
            ("options.temperature < 0.5", false),
            ("options.temperature <= 0.7", true),
            ("options.count == 3", true),
            ("options.count > 2", true),
            ("options.tone > 2", false),
            ("options.on == true", true),
            ("options.off == false", true),
            ("options.missing == false", true),
            ("options.missing == null", true),
            ("options.empty == null", false),
            ("options.list", false),
            ("options.nested.a.b == 2", true),
            ("options.nested.x.y", false),
            ("options.tone == 'casual' && options.api_key", true),
            ("options.tone == 'formal' || options.on", true),
            ("options.off || options.empty", false),
            ("!(options.on && options.off)", true),
            ("!options.on || options.on && options.off", false),
            ("state.result", true),
            ("state.result == 'done'", true),
            ("-1 < 0", true),
            ("options.tone == 'it\\'s'", false),
        ];
        for (src, want) in cases {
            assert_eq!(eval(src, o.clone()), *want, "{src}");
        }
    }

    #[test]
    fn state_paths_are_null_without_a_panel() {
        let options = Default::default();
        let c = Condition::parse("state.result").unwrap();
        assert!(!c.eval(&Context { options: &options, state: None }));
        assert!(Condition::parse("!state.result").unwrap().eval(&Context { options: &options, state: None }));
    }

    #[test]
    fn bad_conditions_say_what_is_wrong() {
        let cases: &[(&str, &str)] = &[
            ("", "ends too early"),
            ("options.", "name a key"),
            ("api_key", "starts with `options.`"),
            ("options.a ==", "ends too early"),
            ("options.a = 'x'", "unexpected '='"),
            ("options.a == 'x", "not closed"),
            ("(options.a", "not closed"),
            ("options.a options.b", "unexpected a path"),
            ("1 < options.a < 3", "chained"),
            ("options.a + 1", "unexpected '+'"),
            ("len(options.a)", "starts with"),
        ];
        for (src, want) in cases {
            let err = Condition::parse(src).unwrap_err();
            assert!(err.contains(want), "{src}: {err}");
        }
    }

    #[test]
    fn lists_the_paths_it_reads() {
        let c = Condition::parse("options.a == 'x' && (!state.b || options.c.d)").unwrap();
        let paths: Vec<_> = c.paths().into_iter().map(|(s, p)| (s, p.join("."))).collect();
        assert_eq!(
            paths,
            vec![(Scope::Options, "a".into()), (Scope::State, "b".into()), (Scope::Options, "c.d".into())]
        );
    }
}
