//! URL and form encoding.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// What `encodeURIComponent` leaves alone: letters, digits and `-_.!~*'()`.
/// Matching it exactly matters, because some endpoints sign the query string
/// and the classic scripts built theirs with `encodeURIComponent`.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// Percent-encode one URL component, as JavaScript's `encodeURIComponent`
/// does.
pub fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, URI_COMPONENT).to_string()
}

/// `a=1&b=2`, each name and value encoded.
pub fn form_encode(fields: &[(&str, &str)]) -> Vec<u8> {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_encode_uri_component() {
        // Reference values from a JavaScript console.
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a+b=c&d"), "a%2Bb%3Dc%26d");
        assert_eq!(url_encode("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(url_encode("привет"), "%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82");
        assert_eq!(url_encode("/?#"), "%2F%3F%23");
    }

    #[test]
    fn form_bodies() {
        assert_eq!(form_encode(&[("q", "a b"), ("sl", "auto")]), b"q=a%20b&sl=auto");
        assert_eq!(form_encode(&[]), b"");
    }
}
