//! Just enough XML for hOCR and ALTO: escaping for the writers and a
//! tolerant pull tokenizer for the readers.
//!
//! The tokenizer is not a validating parser. It reads well-formed XML and
//! the HTML that hOCR producers often write instead (unquoted or bare
//! attributes, `<br>` without a slash, unknown entities), skips comments,
//! processing instructions and doctypes, and drops namespace prefixes from
//! names so `alto:String` reads as `String`. Malformed input never panics:
//! the rest of a broken tag is read as text.

use std::borrow::Cow;

/// Escape text for element content and attribute values alike.
pub fn escape(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"', '\'']) && !s.chars().any(is_forbidden) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if is_forbidden(c) => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Control characters XML 1.0 cannot carry at all, even escaped.
fn is_forbidden(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}')
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An opening tag; `empty` for `<x/>`.
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        empty: bool,
    },
    End {
        name: String,
    },
    Text(String),
}

impl Event {
    pub fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Split `input` into events.
pub fn tokenize(input: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut rest = input.trim_start_matches('\u{feff}');
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut out, rest);
            break;
        };
        if lt > 0 {
            push_text(&mut out, &rest[..lt]);
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |i| &r[i + 3..]);
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").unwrap_or(r.len());
            if end > 0 {
                out.push(Event::Text(r[..end].to_owned()));
            }
            rest = r.get(end + 3..).unwrap_or("");
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |i| &rest[i + 1..]);
        } else if let Some((event, used)) = tag(rest) {
            out.push(event);
            rest = &rest[used..];
        } else {
            // A lone `<` that starts no tag: it is text.
            push_text(&mut out, "<");
            rest = &rest[1..];
        }
    }
    out
}

fn push_text(out: &mut Vec<Event>, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let text = unescape(raw);
    if let Some(Event::Text(prev)) = out.last_mut() {
        prev.push_str(&text);
    } else {
        out.push(Event::Text(text.into_owned()));
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_owned()
}

/// One tag at the start of `s` (which begins with `<`), and the bytes it
/// took.
fn tag(s: &str) -> Option<(Event, usize)> {
    let bytes = s.as_bytes();
    let mut i = 1;
    let closing = bytes.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }
    let name = &s[name_start..i];
    if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
        return None;
    }
    if closing {
        let end = s[i..].find('>')? + i;
        return Some((Event::End { name: local(name) }, end + 1));
    }
    let mut attrs = Vec::new();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        match bytes.get(i)? {
            b'>' => return Some((Event::Start { name: local(name), attrs, empty: false }, i + 1)),
            b'/' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'>') {
                    return Some((Event::Start { name: local(name), attrs, empty: true }, i + 1));
                }
                continue;
            }
            _ => {}
        }
        let key_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let key = &s[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            attrs.push((local(key), String::new()));
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = match bytes.get(i)? {
            q @ (b'"' | b'\'') => {
                let end = s[i + 1..].find(*q as char)? + i + 1;
                let v = &s[i + 1..end];
                i = end + 1;
                v
            }
            _ => {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                &s[start..i]
            }
        };
        attrs.push((local(key), unescape(value).into_owned()));
    }
}

/// Decode entities. Unknown named entities are kept as written.
pub fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.char_indices().take(12).find(|(_, c)| *c == ';').map(|(i, _)| i) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
            }
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_round_trips_through_the_tokenizer() {
        let nasty = "a<b>&\"c\" 'd' \u{1}e";
        let doc = format!("<x v=\"{}\">{}</x>", escape(nasty), escape(nasty));
        let ev = tokenize(&doc);
        let Event::Start { attrs, .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(attrs[0].1, "a<b>&\"c\" 'd' e");
        assert_eq!(ev[1], Event::Text("a<b>&\"c\" 'd' e".into()));
        assert_eq!(ev[2], Event::End { name: "x".into() });
    }

    #[test]
    fn html_ish_input_is_tolerated() {
        let ev = tokenize("<?xml version='1.0'?><!DOCTYPE html><!-- c --><p class=a hidden title='t'>x &copy; &#x41;<br></p><alto:String CONTENT=\"y\"/>< 3");
        assert_eq!(
            ev[0],
            Event::Start {
                name: "p".into(),
                attrs: vec![("class".into(), "a".into()), ("hidden".into(), "".into()), ("title".into(), "t".into())],
                empty: false
            }
        );
        assert_eq!(ev[1], Event::Text("x &copy; A".into()));
        assert!(matches!(&ev[2], Event::Start { name, .. } if name == "br"));
        assert!(matches!(&ev[4], Event::Start { name, empty: true, .. } if name == "String"));
        assert_eq!(ev[5], Event::Text("< 3".into()));
    }

    #[test]
    fn broken_input_never_panics() {
        for s in
            ["<", "<a", "<a b='", "<a b=", "</", "<!--", "<![CDATA[x", "&", "&#xZZ;", "<a/", "<é>", "<a \u{1F600}=1>"]
        {
            let _ = tokenize(s);
        }
    }
}
