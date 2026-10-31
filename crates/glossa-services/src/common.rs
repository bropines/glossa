//! Helpers every built-in service shares: language-list plumbing, response
//! checking, and splitting long text for endpoints with a short limit.

use glossa_core::{Error, LangId, ProviderInfo, Result};
use glossa_net::HttpResponse;
use serde_json::Value;

/// Resolve a list of service codes to table ids, dropping the ones the table
/// does not know and any duplicates (two codes may land on one slot, as
/// `zh`/`zh-Hans` do). Building the list from the service's own codes keeps it
/// next to the code mapping, where a reviewer can check both at once.
pub(crate) fn langs_from_codes(codes: &[&str]) -> Vec<LangId> {
    let mut out: Vec<LangId> = Vec::with_capacity(codes.len());
    for code in codes {
        if let Some(id) = LangId::from_code(code) {
            if (id.is_language() || id == LangId::AUTO) && !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// `AUTO` followed by every language in the table, for services that cover
/// the whole table and detect on their own.
pub(crate) fn all_langs_with_auto() -> Vec<LangId> {
    std::iter::once(LangId::AUTO).chain(LangId::all()).collect()
}

/// Check a direction against the provider's language list before spending a
/// request on it. `UNKNOWN` as a source is treated like `AUTO`. A service that
/// cannot detect on its own has no `AUTO` in its list, so it reports the
/// source as unsupported and the engine detects first.
pub(crate) fn check_pair(info: &ProviderInfo, from: LangId, to: LangId) -> Result<()> {
    let from = if from == LangId::UNKNOWN { LangId::AUTO } else { from };
    let listed = |l: LangId| info.languages.is_empty() || info.languages.contains(&l);
    if !to.is_language() || !listed(to) {
        return Err(Error::TargetLanguageUnsupported { provider: info.name.clone(), lang: to });
    }
    let from_ok = if from == LangId::AUTO {
        info.languages.is_empty() || info.languages.contains(&LangId::AUTO)
    } else {
        from.is_language() && listed(from)
    };
    if !from_ok {
        return Err(Error::SourceLanguageUnsupported { provider: info.name.clone(), lang: from });
    }
    Ok(())
}

/// Whether the request should let the service detect the source.
pub(crate) fn is_auto(lang: LangId) -> bool {
    !lang.is_language()
}

/// A code the service reported, as a real table language. `None` for
/// anything the table does not know, and for `auto`/empty.
pub(crate) fn detected(code: &str) -> Option<LangId> {
    LangId::from_code(code).filter(|l| l.is_language())
}

/// Turn a non-2xx response into an error, putting the API's own message in
/// it rather than a raw JSON blob when the body carries one in a familiar
/// shape (`{"error":{"message"}}`, `{"error":"..."}`, `{"message":"..."}`).
pub(crate) fn check_status(resp: HttpResponse, provider: &str) -> Result<HttpResponse> {
    if resp.is_success() {
        return Ok(resp);
    }
    let status = resp.status;
    match resp.error_for_status(provider) {
        Err(Error::HttpStatus { provider, body, .. }) => {
            let body = api_message(&body).unwrap_or(body);
            Err(Error::HttpStatus { provider, status, body })
        }
        other => other,
    }
}

/// The human-readable message inside a JSON error body, if there is one.
pub(crate) fn api_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let candidates = [&v["error"]["message"], &v["error"], &v["message"], &v["detail"], &v["responseDetails"]];
    candidates.iter().find_map(|c| c.as_str()).filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

/// Check the status, then parse the body as JSON. Malformed JSON is a
/// [`Error::BadResponse`], never a panic.
pub(crate) fn json_body(resp: HttpResponse, provider: &str) -> Result<Value> {
    let resp = check_status(resp, provider)?;
    parse_json(&resp, provider)
}

/// Parse a body as JSON without looking at the status, for APIs that report
/// errors inside a 200.
pub(crate) fn parse_json(resp: &HttpResponse, provider: &str) -> Result<Value> {
    resp.json::<Value>().map_err(|e| Error::bad_response(provider, format!("invalid JSON: {e}")))
}

/// The string at `v`, or a [`Error::BadResponse`] naming what was missing.
pub(crate) fn require_str<'a>(v: &'a Value, what: &str, provider: &str) -> Result<&'a str> {
    v.as_str().ok_or_else(|| Error::bad_response(provider, format!("missing {what}")))
}

/// A base URL from the options, without a trailing slash, defaulting when the
/// user cleared the field.
pub(crate) fn base_url(value: &str, default: &str) -> String {
    let v = value.trim();
    let v = if v.is_empty() { default } else { v };
    v.trim_end_matches('/').to_owned()
}

fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '。' | '、' | '，' | '！' | '？')
}

/// Split `text` into pieces of at most `max` characters, breaking after
/// whitespace or punctuation where possible so a speech engine does not cut a
/// word in half. A run with no break point (a long URL, CJK text) is cut at
/// the limit. Pieces are trimmed and empty ones dropped.
pub(crate) fn split_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let mut cut = rest.len();
        let mut last_break: Option<usize> = None;
        for (count, (i, c)) in rest.char_indices().enumerate() {
            if count == max {
                cut = last_break.unwrap_or(i);
                break;
            }
            if is_break(c) {
                last_break = Some(i + c.len_utf8());
            }
        }
        let piece = rest[..cut].trim();
        if !piece.is_empty() {
            out.push(piece.to_owned());
        }
        rest = rest[cut..].trim_start();
    }
    out
}

/// Remove HTML tags (Google wraps the headword in `<b>` inside examples) and
/// decode the handful of entities those snippets use.
pub(crate) fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg(test)]
pub(crate) mod testing {
    //! Test doubles and small helpers shared by the provider tests.
    //!
    //! [`SeqHttp`] answers requests from a queue, in order — for flows where
    //! the same URL must answer differently the second time (a token refresh
    //! after a 401), which the prefix-routed `MockHttp` cannot do.

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use glossa_core::{Error, Result};
    use glossa_net::{Http, HttpRequest, HttpResponse};

    #[derive(Default)]
    pub struct SeqHttp {
        queue: Mutex<VecDeque<HttpResponse>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl SeqHttp {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn then(self, status: u16, body: &str) -> Self {
            self.queue.lock().unwrap().push_back(HttpResponse {
                status,
                headers: vec![("content-type".into(), "application/json; charset=utf-8".into())],
                body: body.as_bytes().to_vec(),
                codepage: None,
            });
            self
        }

        pub fn requests(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Http for SeqHttp {
        fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            let url = request.url.clone();
            self.seen.lock().unwrap().push(request);
            self.queue
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| Error::Network(format!("no queued response for {url}")))
        }
    }

    /// The value of a request header, case-insensitively.
    pub fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
        req.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// A request body parsed as JSON.
    pub fn body_json(req: &HttpRequest) -> serde_json::Value {
        serde_json::from_slice(req.body.as_deref().unwrap_or_default()).expect("request body is JSON")
    }

    /// A request body as text.
    pub fn body_text(req: &HttpRequest) -> String {
        String::from_utf8_lossy(req.body.as_deref().unwrap_or_default()).into_owned()
    }

    /// Option values from `(key, value)` pairs.
    pub fn opts(pairs: &[(&str, serde_json::Value)]) -> glossa_modules::OptionValues {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::Capabilities;

    #[test]
    fn split_text_breaks_at_word_boundaries() {
        let text = "the quick brown fox jumps over the lazy dog";
        let pieces = split_text(text, 12);
        assert!(pieces.iter().all(|p| p.chars().count() <= 12), "{pieces:?}");
        assert_eq!(pieces.join(" "), text);
        assert_eq!(pieces[0], "the quick");
    }

    #[test]
    fn split_text_cuts_unbreakable_runs_at_the_limit() {
        let pieces = split_text("一二三四五六七八九十", 4);
        assert_eq!(pieces, vec!["一二三四", "五六七八", "九十"]);
    }

    #[test]
    fn split_text_prefers_punctuation_and_drops_empties() {
        assert_eq!(split_text("Hi.There", 5), vec!["Hi.", "There"]);
        assert!(split_text("   ", 10).is_empty());
        assert_eq!(split_text("short", 200), vec!["short"]);
        // Exactly at the limit: one piece.
        assert_eq!(split_text("abcde", 5), vec!["abcde"]);
    }

    #[test]
    fn pairs_are_checked_against_the_language_list() {
        let mut info = ProviderInfo::new("x", "X", Capabilities::TRANSLATE);
        info.languages = langs_from_codes(&["en", "ru"]);
        assert!(check_pair(&info, LangId::ENGLISH, LangId::RUSSIAN).is_ok());
        assert!(matches!(check_pair(&info, LangId::ENGLISH, LangId(33)), Err(Error::TargetLanguageUnsupported { .. })));
        // No `auto` in the list: the engine must detect first.
        assert!(matches!(
            check_pair(&info, LangId::AUTO, LangId::RUSSIAN),
            Err(Error::SourceLanguageUnsupported { .. })
        ));
        info.languages.push(LangId::AUTO);
        assert!(check_pair(&info, LangId::AUTO, LangId::RUSSIAN).is_ok());
        assert!(check_pair(&info, LangId::RUSSIAN, LangId::AUTO).is_err());
    }

    #[test]
    fn error_bodies_yield_the_api_message() {
        assert_eq!(api_message(r#"{"error":{"message":"bad key"}}"#).as_deref(), Some("bad key"));
        assert_eq!(api_message(r#"{"error":"nope"}"#).as_deref(), Some("nope"));
        assert_eq!(api_message(r#"{"message":"m"}"#).as_deref(), Some("m"));
        assert_eq!(api_message("plain text"), None);
    }

    #[test]
    fn non_success_statuses_carry_the_api_message() {
        let resp = HttpResponse {
            status: 400,
            headers: vec![],
            body: br#"{"error":{"message":"bad"}}"#.to_vec(),
            codepage: None,
        };
        match check_status(resp, "P") {
            Err(Error::HttpStatus { status: 400, body, .. }) => assert_eq!(body, "bad"),
            other => panic!("{other:?}"),
        }
        let garbage = HttpResponse { status: 200, headers: vec![], body: b"<html>".to_vec(), codepage: None };
        assert!(matches!(json_body(garbage, "P"), Err(Error::BadResponse { .. })));
    }

    #[test]
    fn tags_and_entities_are_stripped() {
        assert_eq!(strip_tags("say <b>hello</b> &amp; wave"), "say hello & wave");
    }

    #[test]
    fn base_urls_lose_trailing_slashes_and_default_when_empty() {
        assert_eq!(base_url("https://x/", "d"), "https://x");
        assert_eq!(base_url("  ", "https://d/"), "https://d");
    }
}
