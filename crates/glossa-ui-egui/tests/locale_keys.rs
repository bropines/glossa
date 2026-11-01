//! Every locale key the interface asks for exists in `glossa_core::locale`.
//!
//! A key missing from `STRINGS` shows up on screen as the bare key, in every
//! language, so this reads the UI sources and checks each key they name: the
//! argument of `t("…")`, `locale.get("…")`, `locale.fmt("…")` and
//! `locale.fmt_args("…")`, and any string literal shaped like a key of a
//! known group (`"options.ai.add"`), which also catches keys kept in tables.
//! Keys built at run time (`action.{id}`, `capability.{name}`) are checked
//! by tests in `glossa-core`.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use glossa_core::locale::STRINGS;

fn sources() -> Vec<(PathBuf, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                out.push((p, text));
            }
        }
    }
    assert!(out.len() > 5, "found no UI sources");
    out
}

/// The text of the string literal at the start of `s` (after whitespace).
fn literal_at(s: &str) -> Option<&str> {
    let s = s.trim_start().strip_prefix('"')?;
    let end = s.find('"')?;
    Some(&s[..end])
}

/// Keys passed directly to the lookup functions.
fn called_keys(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for call in [".t(", "locale.get(", "locale.fmt(", "locale.fmt_args("] {
        for (at, _) in text.match_indices(call) {
            if let Some(key) = literal_at(&text[at + call.len()..]) {
                out.push(key.to_owned());
            }
        }
    }
    out
}

fn is_key_shaped(s: &str) -> bool {
    let mut parts = s.split('.');
    let first = parts.next().unwrap_or_default();
    !first.is_empty()
        && first.chars().all(|c| c.is_ascii_lowercase())
        && s.contains('.')
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

/// String literals shaped like a key whose group (`options`, `msg`…) exists.
fn key_shaped_literals(text: &str, groups: &HashSet<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        let candidate = &after[..end];
        if is_key_shaped(candidate) && groups.contains(candidate.split('.').next().unwrap_or_default()) {
            out.push(candidate.to_owned());
        }
        // Step past the opening quote only: the closing one may open the
        // next literal.
        rest = after;
    }
    out
}

#[test]
fn every_key_the_ui_uses_is_defined() {
    let known: HashSet<&str> = STRINGS.iter().map(|(k, _, _)| *k).collect();
    let groups: HashSet<&str> = known.iter().filter_map(|k| k.split('.').next()).collect();
    let mut missing = BTreeSet::new();
    let mut used = HashSet::new();
    for (path, text) in sources() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for key in called_keys(&text) {
            if !known.contains(key.as_str()) {
                missing.insert(format!("{name}: {key}"));
            }
            used.insert(key);
        }
        for key in key_shaped_literals(&text, &groups) {
            if !known.contains(key.as_str()) {
                missing.insert(format!("{name}: {key}"));
            }
            used.insert(key);
        }
    }
    assert!(missing.is_empty(), "keys missing from glossa_core::locale::STRINGS:\n{missing:#?}");
    // Guard against the scanner silently finding nothing.
    assert!(used.len() > 250, "only {} keys found in the UI sources", used.len());
}

#[test]
fn the_scanner_finds_what_it_should() {
    let groups: HashSet<&str> = ["options", "msg"].into_iter().collect();
    let text = r#"app.t("options.ok"); app.locale.fmt_args(
        "msg.ms", &[]); let x = ("options.remove", "remove"); s.get("model"); "a b" "c.d""#;
    assert_eq!(called_keys(text), ["options.ok", "msg.ms"]);
    assert_eq!(key_shaped_literals(text, &groups), ["options.ok", "msg.ms", "options.remove"]);
}
