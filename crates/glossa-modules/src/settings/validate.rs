//! Declarative checks on one option's value: `required`, the type itself (a
//! `url` must be a web address, a `number` a number), `pattern`,
//! `min`/`max` and `min_length`/`max_length`. They run natively, every frame
//! on the settings page, with no module call; checks only the service can
//! make (is the key accepted?) are a module action (phase 2).

use glossa_core::locale::Locale;
use glossa_core::LangId;
use serde_json::Value;

use crate::provider::{OptionKind, OptionSpec};

/// Why a value is not acceptable. The text comes from Glossa's locale
/// (`key` with `args` for its `%s`), unless the module gave its own
/// `pattern_message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub key: &'static str,
    pub args: Vec<String>,
    /// The module's own message, already in the interface language.
    pub message: Option<String>,
}

impl FieldError {
    fn new(key: &'static str, args: Vec<String>) -> Self {
        Self { key, args, message: None }
    }

    /// The message in `locale`'s language.
    pub fn describe(&self, locale: &Locale) -> String {
        if let Some(m) = &self.message {
            return m.clone();
        }
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        locale.fmt_args(self.key, &args)
    }
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe(&Locale::english()))
    }
}

/// Whether `v` counts as "not filled in".
pub fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Longest `pattern` accepted, in bytes.
const MAX_PATTERN_LEN: usize = 1000;
/// Memory a compiled pattern may take (NFA, DFA and determinizing each).
/// Ordinary patterns need a few kilobytes; this stops one like `\w{1000}`
/// from eating memory on the UI thread.
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// A compiled `pattern`.
///
/// It runs on `regex-automata`'s dense DFA, which Glossa already links for
/// its log filter, without Unicode tables: classes such as `\d`, `\w` and
/// `[a-z]` are ASCII, case-insensitive matching is ASCII, and `.` matches one
/// byte. Literal non-ASCII text (`^ключ-`) matches as written.
#[derive(Debug, Clone)]
pub struct Pattern(regex_automata::dfa::regex::Regex);

impl Pattern {
    pub fn new(src: &str) -> Result<Pattern, String> {
        use regex_automata::dfa::{dense, regex::Regex};
        use regex_automata::nfa::thompson;
        use regex_automata::util::syntax;
        if src.len() > MAX_PATTERN_LEN {
            return Err(format!("longer than {MAX_PATTERN_LEN} characters"));
        }
        Regex::builder()
            .syntax(syntax::Config::new().unicode(false).utf8(false))
            .thompson(thompson::Config::new().utf8(false).nfa_size_limit(Some(PATTERN_SIZE_LIMIT)))
            .dense(
                dense::Config::new()
                    .dfa_size_limit(Some(PATTERN_SIZE_LIMIT))
                    .determinize_size_limit(Some(PATTERN_SIZE_LIMIT)),
            )
            .build(src)
            .map(Pattern)
            .map_err(|e| e.to_string())
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }
}

/// Check `value` against `spec`. `pattern` is the compiled `spec.pattern`
/// (`None` when absent or invalid: the lint reports an invalid one, and the
/// page does not punish the user for the author's mistake).
pub fn check(spec: &OptionSpec, pattern: Option<&Pattern>, value: &Value) -> Option<FieldError> {
    if is_empty(value) {
        if spec.required {
            return Some(FieldError::new("field.required", vec![]));
        }
        // Picking nothing is a choice a minimum can rule out; an empty text
        // field is just "not set".
        if spec.kind == OptionKind::Multiselect {
            if let Some(min) = spec.min_length.filter(|m| *m > 0) {
                return Some(FieldError::new("field.too_few", vec![min.to_string()]));
            }
        }
        return None;
    }
    let fmt_num = |n: f64| {
        if n.fract() == 0.0 && n.abs() < 1e15 {
            format!("{}", n as i64)
        } else {
            format!("{n}")
        }
    };
    match spec.kind {
        OptionKind::Number => {
            let n = match value {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse().ok(),
                _ => None,
            };
            let Some(n) = n else { return Some(FieldError::new("field.not_number", vec![])) };
            if let Some(min) = spec.min.filter(|m| n < *m) {
                return Some(FieldError::new("field.below_min", vec![fmt_num(min)]));
            }
            if let Some(max) = spec.max.filter(|m| n > *m) {
                return Some(FieldError::new("field.above_max", vec![fmt_num(max)]));
            }
            None
        }
        OptionKind::Select => {
            let v = value.as_str().unwrap_or_default();
            (!spec.choices.is_empty() && !spec.choices.iter().any(|c| c.value == v))
                .then(|| FieldError::new("field.not_choice", vec![]))
        }
        OptionKind::Multiselect => {
            let Some(list) = value.as_array() else {
                return Some(FieldError::new("field.not_choice", vec![]));
            };
            let known = |v: &Value| v.as_str().is_some_and(|s| spec.choices.iter().any(|c| c.value == s));
            if !spec.choices.is_empty() && !list.iter().all(known) {
                return Some(FieldError::new("field.not_choice", vec![]));
            }
            if let Some(min) = spec.min_length.filter(|m| list.len() < *m) {
                return Some(FieldError::new("field.too_few", vec![min.to_string()]));
            }
            if let Some(max) = spec.max_length.filter(|m| list.len() > *m) {
                return Some(FieldError::new("field.too_many", vec![max.to_string()]));
            }
            None
        }
        OptionKind::Lang => {
            let code = value.as_str().unwrap_or_default();
            let ok = match LangId::from_code(code) {
                Some(l) if l == LangId::AUTO => spec.allow_auto,
                Some(l) => l.is_language(),
                None => false,
            };
            (!ok).then(|| FieldError::new("field.not_language", vec![]))
        }
        OptionKind::Bool | OptionKind::Unknown(_) => None,
        OptionKind::Text | OptionKind::Secret | OptionKind::Url | OptionKind::Multiline | OptionKind::File => {
            let s = value.as_str()?;
            let len = s.chars().count();
            if let Some(min) = spec.min_length.filter(|m| len < *m) {
                return Some(FieldError::new("field.too_short", vec![min.to_string()]));
            }
            if let Some(max) = spec.max_length.filter(|m| len > *m) {
                return Some(FieldError::new("field.too_long", vec![max.to_string()]));
            }
            if spec.kind == OptionKind::Url && !is_web_address(s.trim()) {
                return Some(FieldError::new("field.not_url", vec![]));
            }
            if let Some(re) = pattern {
                if !re.is_match(s) {
                    let mut e = FieldError::new("field.pattern", vec![]);
                    e.message = (!spec.pattern_message.is_empty()).then(|| spec.pattern_message.clone());
                    return Some(e);
                }
            }
            None
        }
    }
}

fn is_web_address(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !s.contains(char::is_whitespace)
        && crate::manifest::url_host(s).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(toml_text: &str) -> OptionSpec {
        toml::from_str(&format!("key = \"k\"\n{toml_text}")).unwrap()
    }

    fn err(s: &OptionSpec, v: Value) -> Option<String> {
        let re = (!s.pattern.is_empty()).then(|| Pattern::new(&s.pattern).unwrap());
        check(s, re.as_ref(), &v).map(|e| e.to_string())
    }

    #[test]
    fn required_and_empty_values() {
        let s = spec("required = true");
        assert_eq!(err(&s, json!(null)).as_deref(), Some("Required."));
        assert_eq!(err(&s, json!("  ")).as_deref(), Some("Required."));
        assert_eq!(err(&s, json!("x")), None);
        // An optional empty field passes every other check.
        let s = spec("pattern = '^a$'\nmin_length = 3");
        assert_eq!(err(&s, json!("")), None);
    }

    #[test]
    fn text_lengths_and_patterns() {
        let s = spec("min_length = 2\nmax_length = 4");
        assert_eq!(err(&s, json!("a")).as_deref(), Some("At least 2 characters."));
        assert_eq!(err(&s, json!("abcde")).as_deref(), Some("At most 4 characters."));
        assert_eq!(err(&s, json!("абвг")), None, "characters, not bytes");
        let s = spec("pattern = '^ak-[a-z0-9]{4}$'");
        assert_eq!(err(&s, json!("ak-12ab")), None);
        assert_eq!(err(&s, json!("nope")).as_deref(), Some("Does not match the expected format."));
        let s = spec("pattern = '^ak-'\npattern_message = 'Starts with ak-.'");
        assert_eq!(err(&s, json!("x")).as_deref(), Some("Starts with ak-."));
    }

    #[test]
    fn patterns_are_ascii_and_bounded() {
        let p = Pattern::new(r"^ak-[A-Za-z0-9_\-]{4}$").unwrap();
        assert!(p.is_match("ak-ab_1") && !p.is_match("ak-abc"));
        assert!(Pattern::new("^ключ-").unwrap().is_match("ключ-1"), "literal non-ASCII text matches");
        assert!(Pattern::new(r"\d+").unwrap().is_match("x42"), "unanchored patterns match anywhere");
        assert!(Pattern::new("([a-").is_err());
        assert!(Pattern::new(&"a".repeat(1001)).unwrap_err().contains("longer than"));
        // A pattern whose automaton would be huge is refused, not built.
        assert!(Pattern::new("(?:a{1000}){1000}").is_err());
    }

    #[test]
    fn typed_values() {
        let url = spec("type = 'url'");
        assert_eq!(err(&url, json!("https://example.com/x")), None);
        assert!(err(&url, json!("example.com")).is_some());
        assert!(err(&url, json!("https://")).is_some());
        let num = spec("type = 'number'\nmin = 0.0\nmax = 1.5");
        assert_eq!(err(&num, json!(1)), None);
        assert_eq!(err(&num, json!("0.5")), None);
        assert_eq!(err(&num, json!(-1)).as_deref(), Some("At least 0."));
        assert_eq!(err(&num, json!(2)).as_deref(), Some("At most 1.5."));
        assert_eq!(err(&num, json!("many")).as_deref(), Some("Not a number."));
        let sel = spec("type = 'select'\nchoices = [{ value = 'a' }, { value = 'b' }]");
        assert_eq!(err(&sel, json!("a")), None);
        assert!(err(&sel, json!("c")).is_some());
        let multi = spec("type = 'multiselect'\nchoices = [{ value = 'a' }, { value = 'b' }]\nmax_length = 1");
        assert_eq!(err(&multi, json!(["a"])), None);
        assert!(err(&multi, json!(["a", "z"])).is_some());
        assert_eq!(err(&multi, json!(["a", "b"])).as_deref(), Some("Pick at most 1."));
        let at_least = spec("type = 'multiselect'\nchoices = [{ value = 'a' }]\nmin_length = 1");
        assert_eq!(err(&at_least, json!([])).as_deref(), Some("Pick at least 1."));
        assert_eq!(err(&at_least, json!(null)).as_deref(), Some("Pick at least 1."));
        let lang = spec("type = 'lang'");
        assert_eq!(err(&lang, json!("de")), None);
        assert!(err(&lang, json!("auto")).is_some());
        assert!(err(&lang, json!("klingon-ish")).is_some());
        assert_eq!(err(&spec("type = 'lang'\nallow_auto = true"), json!("auto")), None);
    }

    #[test]
    fn messages_follow_the_locale() {
        let ru = Locale::builtin("ru").unwrap();
        let e = check(&spec("min_length = 5"), None, &json!("abc")).unwrap();
        assert_ne!(e.describe(&ru), e.to_string());
        assert!(e.describe(&ru).contains('5'));
    }
}
