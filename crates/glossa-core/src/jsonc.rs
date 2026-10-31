//! JSON with comments and trailing commas.
//!
//! Classic theme and locale files carry `//` comments and the occasional
//! trailing comma, neither of which strict JSON allows. [`strip`] removes both
//! without touching string contents, so the result feeds a standard parser.
//! It also drops a leading byte-order mark, which the classic files have.

/// Return `text` with comments, trailing commas and a BOM removed.
pub fn strip(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;

    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            // Copy whole UTF-8 sequences; only ASCII bytes are ever special.
            let ch_len = utf8_len(c);
            out.push_str(&text[i..i + ch_len]);
            if c == b'\\' && i + 1 < bytes.len() {
                let next_len = utf8_len(bytes[i + 1]);
                out.push_str(&text[i + 1..i + 1 + next_len]);
                i += 1 + next_len;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
            i += ch_len;
            continue;
        }
        match c {
            b'"' => {
                in_string = true;
                out.push('"');
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b',' => {
                // A comma followed only by whitespace/comments and then a
                // closing bracket is a trailing comma: drop it.
                if next_significant(bytes, i + 1).is_some_and(|b| b == b'}' || b == b']') {
                    i += 1;
                } else {
                    out.push(',');
                    i += 1;
                }
            }
            _ => {
                let ch_len = utf8_len(c);
                out.push_str(&text[i..i + ch_len]);
                i += ch_len;
            }
        }
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// The next byte that is not whitespace and not inside a comment.
fn next_significant(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b => return Some(b),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parses(text: &str) -> serde_json::Value {
        serde_json::from_str(&strip(text)).expect("stripped text should be valid JSON")
    }

    #[test]
    fn line_and_block_comments_go() {
        let v = parses("{ // hi\n \"a\": 1, /* there */ \"b\": 2 }");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn comment_markers_inside_strings_stay() {
        let v = parses(r#"{ "url": "https://example.com/a//b", "c": "/* not a comment */" }"#);
        assert_eq!(v["url"], "https://example.com/a//b");
        assert_eq!(v["c"], "/* not a comment */");
    }

    #[test]
    fn escaped_quotes_do_not_end_the_string() {
        let v = parses(r#"{ "q": "say \"hi\" // ok" }"#);
        assert_eq!(v["q"], "say \"hi\" // ok");
    }

    #[test]
    fn trailing_commas_go_even_before_a_comment() {
        let v = parses("{ \"a\": [1, 2, ], \"b\": 3, // last\n }");
        assert_eq!(v["a"], serde_json::json!([1, 2]));
        assert_eq!(v["b"], 3);
    }

    #[test]
    fn bom_and_non_ascii_survive() {
        let v = parses("\u{feff}{ \"name\": \"Русский\", \"x\": \"日本語\" }");
        assert_eq!(v["name"], "Русский");
        assert_eq!(v["x"], "日本語");
    }
}
