//! Google Translate, through the keyless endpoints the browser extensions
//! use, plus the `translate_tts` speech endpoint.
//!
//! Three addresses answer the same `translate_a/single` request in the same
//! format: `clients5.google.com` (`client=dict-chrome-ex`, the dictionary
//! extension's), `translate.googleapis.com` (`client=gtx`) and
//! `translate.google.<domain>` (`client=gtx`). Each keeps its own count of
//! how hard it is being asked, so when one starts refusing (HTTP 429, or a
//! redirect to the `google.com/sorry` captcha) the next one usually still
//! answers; the next one is also asked when the first cannot be reached. A refusing address is left alone for a cooling-off period that
//! doubles with each refusal (see [`crate::backoff`]); when all of them are
//! cooling off the provider says so at once, with the wait, and the engine
//! moves on to the next service. `docs/research/google-translate.md` has
//! what was observed and how other clients deal with it.
//!
//! The endpoint answers with positional JSON arrays rather than objects, and
//! which slots are filled depends on the `dt=` flags sent. The layout this
//! module relies on (`dt=` flag → top-level slot):
//!
//! | slot | flag | contents                                                     |
//! |------|------|--------------------------------------------------------------|
//! | 0    | t/rm | segments `[translated, original, …]`; with `rm`, a final segment `[null, null, target_translit, source_translit]` |
//! | 1    | bd   | `[pos, [words], [[word, [back translations], …], …], …]`     |
//! | 2    | —    | detected (or given) source code                              |
//! | 6    | ld   | detection confidence                                         |
//! | 8    | ld   | `[[codes], null, [confidences], [codes]]`                    |
//! | 11   | ss   | synonyms `[pos, [[[syn, …], id], …], word]`                  |
//! | 12   | md   | definitions `[pos, [[definition, id, example?], …], word]`   |
//! | 13   | ex   | examples `[[[html, …], …]]`                                  |
//!
//! Every access goes through `serde_json::Value` indexing, which yields
//! `Null` for a missing slot, so a reshuffled response degrades to "less
//! detail" rather than a panic.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use glossa_core::dict::{Example, Explanation, Pronunciation};
use glossa_core::translate::{PartOfSpeechEntry, Sense};
use glossa_core::{
    Article, Audio, Capabilities, Detection, DictRequest, Error, LangId, LangPair, ProviderInfo, Result, SpeakRequest,
    TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest, HttpResponse};
use serde_json::Value;

use crate::backoff::{ceil_secs, Backoff};
use crate::common::{
    all_langs_with_auto, check_pair, check_status, detected, is_auto, parse_json, split_text, strip_tags,
};

/// The first translation endpoint, which the tests answer for.
#[cfg(test)]
const API: &str = "https://clients5.google.com/translate_a/single";
#[cfg(test)]
const TTS: &str = "https://translate.google.com/translate_tts";
/// Past this many bytes of encoded text the query string gets long enough for
/// Google to answer 413/400, so the text moves into a form body.
const GET_LIMIT: usize = 1800;
/// The TTS endpoint refuses longer input.
const TTS_CHUNK: usize = 200;
/// Characters per translation request. The endpoint takes far more in one
/// form body (100 000 characters were answered in testing), but an answer
/// that large is slow to arrive in one piece; a few requests this size keep
/// each one quick without turning a long text into a stream of small ones.
const CHUNK: usize = 10_000;
/// The most text one translation takes: five requests at most.
const MAX_INPUT: usize = 5 * CHUNK;

/// A keyless address of Google's web translator.
#[derive(Debug, Clone, Copy)]
struct Endpoint {
    /// Its name in the cooling-off table and in the log.
    key: &'static str,
    /// `None`: `translate.google.<the preferred domain>`.
    host: Option<&'static str>,
    client: &'static str,
}

impl Endpoint {
    fn url(&self, host: &Host, path: &str) -> String {
        match self.host {
            Some(h) => format!("https://{h}{path}?client={}", self.client),
            None => {
                format!("https://translate.google.{}{path}?client={}", sane_domain(host.preferred_domain), self.client)
            }
        }
    }
}

/// Where `translate_a/single` is asked, in order of preference. The
/// dictionary extension's address comes first because it was the one that
/// kept answering Glossa's client while both `client=gtx` addresses showed
/// it the captcha (see the research notes); the user-facing site comes last,
/// so that a block on it, which the user's browser shares, is the last
/// thing we add to.
const TRANSLATE: [Endpoint; 3] = [
    Endpoint { key: "clients5.google.com", host: Some("clients5.google.com"), client: "dict-chrome-ex" },
    Endpoint { key: "translate.googleapis.com", host: Some("translate.googleapis.com"), client: "gtx" },
    Endpoint { key: "translate.google", host: None, client: "gtx" },
];

/// Where `translate_tts` is asked. Speech has its own limits, separate from
/// translation's, so it has its own entries in the table.
const SPEECH: [Endpoint; 3] = [
    Endpoint { key: "tts translate.google", host: None, client: "tw-ob" },
    Endpoint { key: "tts translate.googleapis.com", host: Some("translate.googleapis.com"), client: "gtx" },
    Endpoint { key: "tts clients5.google.com", host: Some("clients5.google.com"), client: "dict-chrome-ex" },
];

/// Google's codes are the legacy ones (`iw`, `zh-CN`, `tl`, `no`).
fn code(lang: LangId) -> &'static str {
    if is_auto(lang) {
        "auto"
    } else {
        lang.legacy_code()
    }
}

/// `hl=` localises the part-of-speech labels; fall back to English when the
/// interface language is not a real one.
fn hl(host: &Host) -> &'static str {
    if host.ui_language.is_language() {
        host.ui_language.legacy_code()
    } else {
        "en"
    }
}

/// The domain the user prefers, if it looks like one (`com`, `ru`, `co.uk`).
/// Anything else falls back to `com` rather than ending up in a URL.
fn sane_domain(domain: &str) -> &str {
    let d = domain.trim().trim_start_matches('.');
    if !d.is_empty() && d.len() <= 16 && d.chars().all(|c| c.is_ascii_lowercase() || c == '.') {
        d
    } else {
        "com"
    }
}

/// What a browser sends besides its user agent (which the client adds).
fn browser_headers(req: HttpRequest, host: &Host) -> HttpRequest {
    let hl = hl(host);
    let languages = if hl == "en" { "en-US,en;q=0.9".to_owned() } else { format!("{hl},en-US;q=0.8,en;q=0.7") };
    req.header("Accept", "*/*").header("Accept-Language", languages).no_retry_when_limited()
}

/// A web page where JSON or audio was expected: by its content type, or a
/// body that opens with a tag. Only the body's start is looked at, so a
/// translation that merely quotes HTML is not one.
fn is_web_page(resp: &HttpResponse) -> bool {
    let html = resp.header("content-type").is_some_and(|ct| ct.trim_start().starts_with("text/html"));
    html || resp.body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

/// The `google.com/sorry` captcha page: a web page that links to `/sorry/`
/// or says it saw "unusual traffic". It normally arrives with status 429
/// after the redirect, but has been seen with 200 and with 403.
fn is_captcha(resp: &HttpResponse) -> bool {
    if !is_web_page(resp) {
        return false;
    }
    let page = String::from_utf8_lossy(&resp.body[..resp.body.len().min(64 * 1024)]).to_ascii_lowercase();
    page.contains("/sorry/") || page.contains("unusual traffic")
}

/// Check a response for a refusal, then for any other error status, then
/// for some other web page in place of the answer (a consent or block page,
/// which one address can show while another answers).
fn checked(resp: HttpResponse, provider: &str) -> Result<HttpResponse> {
    if is_captcha(&resp) {
        return Err(Error::RateLimited { provider: provider.to_owned(), retry_after_secs: None });
    }
    let resp = check_status(resp, provider)?;
    if is_web_page(&resp) {
        return Err(Error::bad_response(provider, WEB_PAGE));
    }
    Ok(resp)
}

/// The detail of the error for an unexpected web page, which also marks it
/// as worth asking the next address about.
const WEB_PAGE: &str = "answered with a web page instead of a translation";

/// Ask `endpoints` in order, skipping those cooling off, until one answers.
///
/// A refusal puts the endpoint in the cooling-off table and moves on. A
/// server error, a 403 or an unexpected web page also moves on, without the
/// cooling-off: one address can give those while another works. So does a
/// failure to reach the address (a firewall or DNS can block one host), but
/// a second such failure in a row ends the search: then it is the network,
/// and every further address would only make the user wait longer. Any
/// other failure from the first endpoint asked (an unreadable answer, a
/// 4xx) is reported as is, since the next address would fail the same way.
fn first_answer<T>(
    limits: &Backoff,
    host: &Host,
    provider: &str,
    endpoints: &[Endpoint],
    mut call: impl FnMut(&Endpoint) -> Result<T>,
) -> Result<T> {
    let mut soonest: Option<Duration> = None;
    let mut note_wait = |wait: Duration| soonest = Some(soonest.map_or(wait, |s| s.min(wait)));
    let mut limited = false;
    let mut first_error: Option<Error> = None;
    let mut asked = 0;
    let mut unreachable = 0;
    for ep in endpoints {
        if let Some(wait) = limits.cooling(ep.key, Instant::now()) {
            tracing::debug!(endpoint = ep.key, secs = ceil_secs(wait), "skipped: still cooling off after a refusal");
            limited = true;
            note_wait(wait);
            continue;
        }
        host.check_cancelled()?;
        asked += 1;
        match call(ep) {
            Ok(v) => {
                limits.ok(ep.key);
                if asked > 1 || limited {
                    tracing::info!(endpoint = ep.key, "answered after another endpoint refused");
                }
                return Ok(v);
            }
            Err(Error::RateLimited { retry_after_secs, .. }) => {
                let wait = limits.limited(ep.key, retry_after_secs, Instant::now());
                tracing::info!(endpoint = ep.key, secs = wait.as_secs(), "rate-limited; leaving it alone for a while");
                limited = true;
                note_wait(wait);
                unreachable = 0;
            }
            Err(e @ Error::Cancelled) => return Err(e),
            Err(e @ (Error::Network(_) | Error::Timeout(_))) => {
                unreachable += 1;
                tracing::debug!(endpoint = ep.key, error = %e, "could not reach it");
                first_error.get_or_insert(e);
                if unreachable >= 2 {
                    break;
                }
            }
            Err(e) => {
                let switchable = match &e {
                    Error::HttpStatus { status, .. } => *status == 403 || *status >= 500,
                    Error::BadResponse { detail, .. } => detail == WEB_PAGE,
                    _ => false,
                };
                if asked == 1 && !limited && !switchable {
                    return Err(e);
                }
                tracing::debug!(endpoint = ep.key, error = %e, "failed; trying the next endpoint");
                first_error.get_or_insert(e);
                unreachable = 0;
            }
        }
    }
    if limited {
        return Err(Error::RateLimited { provider: provider.to_owned(), retry_after_secs: soonest.map(ceil_secs) });
    }
    Err(first_error.unwrap_or_else(|| Error::bad_response(provider, "no endpoint to ask")))
}

/// One `translate_a/single` call, on whichever endpoint answers.
fn query(
    host: &Host,
    limits: &Backoff,
    provider: &str,
    text: &str,
    from: LangId,
    to: LangId,
    dts: &[&str],
) -> Result<Value> {
    let mut params = format!("&sl={}&tl={}&hl={}", url_encode(code(from)), url_encode(code(to)), hl(host));
    for dt in dts {
        params.push_str("&dt=");
        params.push_str(dt);
    }
    params.push_str("&ie=UTF-8&oe=UTF-8");
    let encoded = url_encode(text);
    first_answer(limits, host, provider, &TRANSLATE, |ep| {
        let url = format!("{}{params}", ep.url(host, "/translate_a/single"));
        let req = if encoded.len() > GET_LIMIT {
            HttpRequest::post(url).form(&[("q", text)])
        } else {
            HttpRequest::get(format!("{url}&q={encoded}"))
        };
        let resp = checked(host.http.send(browser_headers(req, host).provider(provider))?, provider)?;
        parse_json(&resp, provider)
    })
}

/// Cut `text` into pieces of at most `max` characters that join back into
/// `text` exactly: after a line break if there is one in the second half of
/// the window, else after the space that follows a sentence end there (or
/// right after a CJK full stop, which has none), else after any whitespace,
/// else at the limit. Cutting after the space rather than the full stop
/// keeps it with the piece before, where it is put back on joining; Google
/// trims a piece's leading space, which would glue the sentences together.
fn split_keeping(text: &str, max: usize) -> Vec<&str> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((limit, _)) = rest.char_indices().nth(max) {
        let window = &rest[..limit];
        let after = |i: usize| i + window[i..].chars().next().map_or(1, char::len_utf8);
        let late = |cut: &usize| *cut >= limit / 2;
        let cut = window
            .rfind('\n')
            .map(after)
            .filter(late)
            .or_else(|| sentence_cut(window).filter(late))
            .or_else(|| window.rfind(char::is_whitespace).map(after).filter(|c| *c > 0))
            .unwrap_or(limit);
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Where the last sentence in `window` ends, counting the space after it.
fn sentence_cut(window: &str) -> Option<usize> {
    let mut best = None;
    let mut prev = None;
    for (i, c) in window.char_indices() {
        if c.is_whitespace() && matches!(prev, Some('.' | '!' | '?')) {
            best = Some(i + c.len_utf8());
        }
        if matches!(c, '\u{3002}' | '\u{FF01}' | '\u{FF1F}') {
            best = Some(i + c.len_utf8());
        }
        prev = Some(c);
    }
    best
}

/// The whitespace to put between two translated pieces for `gap`, the
/// whitespace the source had there: its line breaks if it had any, else a
/// single space.
fn joining_space(gap: &str) -> &str {
    match gap.find('\n') {
        Some(i) => gap[i..].trim_end_matches(|c: char| c != '\n' && c.is_whitespace()),
        None if gap.is_empty() => "",
        None => " ",
    }
}

/// The translated text: every segment's `[0]`, concatenated.
fn translated_text(v: &Value) -> Option<String> {
    let segments = v[0].as_array()?;
    let text: String = segments.iter().filter_map(|s| s[0].as_str()).collect();
    Some(text)
}

/// The transliteration segment, when `dt=rm` added one: the last segment,
/// recognisable by having no translated text in `[0]`.
fn translit_segment(v: &Value) -> Option<&Value> {
    v[0].as_array()?.last().filter(|s| s[0].as_str().is_none())
}

fn non_empty(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The translation's romanisation: slot 2 of the `rm` segment. Slot 3 is
/// the source's, which for English input is a phonetic spelling
/// (`həˈlō`), not something to show under a Russian translation.
pub(crate) fn romanisation(v: &Value) -> Option<String> {
    non_empty(&translit_segment(v)?[2])
}

/// The source's transliteration or pronunciation: slot 3 of the `rm`
/// segment (`Kon'nichiwa` for Japanese input).
pub(crate) fn source_romanisation(v: &Value) -> Option<String> {
    non_empty(&translit_segment(v)?[3])
}

/// The `bd` block as part-of-speech entries.
pub(crate) fn parts_of_speech(v: &Value) -> Vec<PartOfSpeechEntry> {
    let Some(blocks) = v[1].as_array() else { return Vec::new() };
    blocks
        .iter()
        .filter_map(|block| {
            let pos = block[0].as_str().unwrap_or("").to_owned();
            let senses: Vec<Sense> = match block[2].as_array() {
                Some(entries) => entries
                    .iter()
                    .filter_map(|e| {
                        let word = e[0].as_str()?.to_owned();
                        let back = e[1]
                            .as_array()
                            .map(|b| b.iter().filter_map(|w| w.as_str().map(str::to_owned)).collect())
                            .unwrap_or_default();
                        Some(Sense { word, back_translations: back })
                    })
                    .collect(),
                // Older answers carry only the bare word list.
                None => block[1]
                    .as_array()
                    .map(|ws| {
                        ws.iter()
                            .filter_map(|w| w.as_str())
                            .map(|w| Sense { word: w.to_owned(), back_translations: Vec::new() })
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            (!senses.is_empty()).then_some(PartOfSpeechEntry { part_of_speech: pos, senses })
        })
        .collect()
}

/// The source language Google reports: slot 2, else the `ld` block.
fn detected_source(v: &Value) -> Option<LangId> {
    v[2].as_str().and_then(detected).or_else(|| v[8][0][0].as_str().and_then(detected))
}

fn parse_translation(v: &Value, provider: &str) -> Result<Translation> {
    let text = translated_text(v).ok_or_else(|| Error::bad_response(provider, "no translation segments"))?;
    Ok(Translation {
        text,
        detected_from: detected_source(v),
        romanisation: romanisation(v),
        source_romanisation: source_romanisation(v),
        parts_of_speech: parts_of_speech(v),
        ..Default::default()
    })
}

/// Add the next chunk's translation to the ones before it. The dictionary
/// senses only come with a single word, so the first chunk's stand.
fn append_chunk(whole: &mut Translation, part: Translation) {
    whole.text.push_str(&part.text);
    let join = |a: &mut Option<String>, b: Option<String>| {
        if let Some(b) = b {
            match a {
                Some(a) => {
                    a.push(' ');
                    a.push_str(&b);
                }
                None => *a = Some(b),
            }
        }
    };
    join(&mut whole.romanisation, part.romanisation);
    join(&mut whole.source_romanisation, part.source_romanisation);
    whole.detected_from = whole.detected_from.or(part.detected_from);
}

/// The "open in browser" link, shared by the translator and the dictionary.
fn web_link_for(domain: &str, text: &str, pair: LangPair) -> String {
    format!(
        "https://translate.google.{}/?sl={}&tl={}&text={}&op=translate",
        sane_domain(domain),
        url_encode(code(pair.from)),
        url_encode(code(pair.to)),
        url_encode(text)
    )
}

/// Remembers the preferred domain from the last call, since
/// [`Provider::web_link`] gets no [`Host`] to read it from.
#[derive(Default)]
struct DomainMemo(RwLock<String>);

impl DomainMemo {
    fn remember(&self, host: &Host) {
        let d = sane_domain(host.preferred_domain);
        if let Ok(mut g) = self.0.write() {
            if *g != d {
                *g = d.to_owned();
            }
        }
    }

    fn get(&self) -> String {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }
}

/// Google Translate: translation, detection and speech.
pub struct Google {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
    domain: DomainMemo,
    limits: Arc<Backoff>,
}

impl Default for Google {
    fn default() -> Self {
        Self::new()
    }
}

impl Google {
    pub const ID: &'static str = "google";

    pub fn new() -> Self {
        Self::sharing(Arc::default())
    }

    /// With a cooling-off table shared with [`GoogleDictionary`], which asks
    /// the same endpoints.
    pub(crate) fn sharing(limits: Arc<Backoff>) -> Self {
        let mut info = ProviderInfo::new(
            Self::ID,
            "Google Translate",
            Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE | Capabilities::LISTEN,
        );
        info.description = "Google's free web translator. No key needed; also detects languages, shows \
                            romanisation and per-part-of-speech alternatives, and reads text aloud."
            .into();
        info.homepage = "https://translate.google.com".into();
        info.ready_to_use = true;
        info.languages = all_langs_with_auto();
        info.max_input_chars = Some(MAX_INPUT);
        let options = vec![
            OptionSpec::boolean("romanisation", "Show romanisation", true)
                .help("Add a Latin transcription of non-Latin text under the result."),
            OptionSpec::boolean("dictionary", "Show alternatives by part of speech", true)
                .help("For single words, list other translations grouped as noun, verb, …"),
        ];
        Self { info, options, domain: DomainMemo::default(), limits }
    }

    fn flag(host: &Host, key: &str) -> bool {
        // Default on: an option missing from the map means "not changed".
        host.option(key).and_then(Value::as_bool).unwrap_or(true)
    }
}

impl Provider for Google {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        self.domain.remember(host);
        let mut dts = vec!["t"];
        if Self::flag(host, "dictionary") {
            dts.push("bd");
        }
        if Self::flag(host, "romanisation") {
            dts.push("rm");
        }
        dts.push("ld");
        let chunks = split_keeping(&req.text, CHUNK);
        let mut out: Option<Translation> = None;
        for chunk in &chunks {
            let v = query(host, &self.limits, &self.info.name, chunk, req.from, req.to, &dts)?;
            let part = parse_translation(&v, &self.info.name)?;
            match out.as_mut() {
                None => out = Some(part),
                Some(whole) => {
                    // Google trims each request's answer: put back the
                    // whitespace the source had between the pieces, whether
                    // the cut left it at the end of one or the start of the
                    // next.
                    let lead = &chunk[..chunk.len() - chunk.trim_start().len()];
                    if !whole.text.ends_with(char::is_whitespace) {
                        whole.text.push_str(joining_space(lead));
                    }
                    append_chunk(whole, part);
                }
            }
            if chunks.len() > 1 {
                let tail = &chunk[chunk.trim_end().len()..];
                out.as_mut().expect("set above").text.push_str(joining_space(tail));
            }
        }
        let mut t = out.ok_or(Error::EmptyInput)?;
        if chunks.len() > 1 {
            t.text.truncate(t.text.trim_end().len());
        }
        Ok(t)
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        self.domain.remember(host);
        let v = query(host, &self.limits, &self.info.name, text, LangId::AUTO, LangId::ENGLISH, &["ld"])?;
        let lang = detected_source(&v).ok_or(Error::LanguageNotDetected)?;
        let confidence = v[6].as_f64().or_else(|| v[8][2][0].as_f64()).unwrap_or(1.0);
        Ok(Detection { lang, confidence: confidence.clamp(0.0, 1.0) as f32 })
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        if !req.lang.is_language() {
            return Err(Error::SourceLanguageUnsupported { provider: self.info.name.clone(), lang: req.lang });
        }
        self.domain.remember(host);
        let chunks = split_text(&req.text, TTS_CHUNK);
        if chunks.is_empty() {
            return Err(Error::EmptyInput);
        }
        let total = chunks.len();
        let provider = self.info.name.as_str();
        let mut bytes = Vec::new();
        for (idx, chunk) in chunks.iter().enumerate() {
            let audio = first_answer(&self.limits, host, provider, &SPEECH, |ep| {
                let mut url = format!(
                    "{}&ie=UTF-8&tl={}&total={total}&idx={idx}&textlen={}&q={}",
                    ep.url(host, "/translate_tts"),
                    url_encode(req.lang.legacy_code()),
                    chunk.chars().count(),
                    url_encode(chunk)
                );
                if req.slow {
                    url.push_str("&ttsspeed=0.24");
                }
                let resp = checked(
                    host.http.send(browser_headers(HttpRequest::get(url), host).provider(provider))?,
                    provider,
                )?;
                if resp.body.is_empty() {
                    return Err(Error::bad_response(provider, "speech endpoint returned no audio"));
                }
                Ok(resp.body)
            })?;
            // MP3 frames concatenate into a playable stream.
            bytes.extend_from_slice(&audio);
        }
        Ok(Audio { bytes, mime: "audio/mpeg".into() })
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        Some(web_link_for(&self.domain.get(), text, pair))
    }
}

/// Google's dictionary view of the same endpoint: definitions, examples and
/// synonyms in the source language, with the translations alongside.
pub struct GoogleDictionary {
    info: ProviderInfo,
    domain: DomainMemo,
    limits: Arc<Backoff>,
}

impl Default for GoogleDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleDictionary {
    pub const ID: &'static str = "google-dictionary";

    pub fn new() -> Self {
        Self::sharing(Arc::default())
    }

    /// With a cooling-off table shared with [`Google`].
    pub(crate) fn sharing(limits: Arc<Backoff>) -> Self {
        let mut info = ProviderInfo::new(Self::ID, "Google Dictionary", Capabilities::DICTIONARY);
        info.description = "Definitions, usage examples and synonyms from Google Translate's dictionary, \
                            with translations grouped by part of speech. No key needed."
            .into();
        info.homepage = "https://translate.google.com".into();
        info.ready_to_use = true;
        info.languages = all_langs_with_auto();
        info.max_input_chars = Some(200);
        Self { info, domain: DomainMemo::default(), limits }
    }
}

/// Build an [`Article`] from a `bd/md/ex/ss/rm/t` answer.
pub(crate) fn parse_article(v: &Value, query_text: &str) -> Article {
    let headword = v[0][0][1].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(query_text.trim()).to_owned();

    // Only the source's own transliteration is a pronunciation of the
    // headword; the target's would describe the translation.
    let pronunciations = translit_segment(v)
        .and_then(|seg| non_empty(&seg[3]))
        .map(|symbol| vec![Pronunciation { region: String::new(), symbol, audio_url: None }])
        .unwrap_or_default();

    let mut explanations = Vec::new();
    let mut examples = Vec::new();
    for group in v[12].as_array().into_iter().flatten() {
        let pos = group[0].as_str().unwrap_or("").to_owned();
        let mut definitions = Vec::new();
        for def in group[1].as_array().into_iter().flatten() {
            if let Some(d) = non_empty(&def[0]) {
                definitions.push(d);
            }
            if let Some(ex) = non_empty(&def[2]) {
                examples.push(Example { source: strip_tags(&ex), target: String::new() });
            }
        }
        if !definitions.is_empty() {
            explanations.push(Explanation { part_of_speech: pos, definitions });
        }
    }
    for ex in v[13][0].as_array().into_iter().flatten() {
        if let Some(html) = non_empty(&ex[0]) {
            let source = strip_tags(&html);
            if !examples.iter().any(|e| e.source == source) {
                examples.push(Example { source, target: String::new() });
            }
        }
    }

    let mut translations = Vec::new();
    if let Some(main) = translated_text(v).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
        if !main.eq_ignore_ascii_case(&headword) {
            translations.push(main);
        }
    }
    for entry in parts_of_speech(v) {
        let words: Vec<&str> = entry.senses.iter().map(|s| s.word.as_str()).collect();
        if entry.part_of_speech.is_empty() {
            translations.push(words.join(", "));
        } else {
            translations.push(format!("{}: {}", entry.part_of_speech, words.join(", ")));
        }
    }
    if !translations.is_empty() {
        explanations.push(Explanation { part_of_speech: "translation".into(), definitions: translations });
    }

    let mut associations: Vec<String> = Vec::new();
    for group in v[11].as_array().into_iter().flatten() {
        for set in group[1].as_array().into_iter().flatten() {
            for syn in set[0].as_array().into_iter().flatten().filter_map(|s| s.as_str()) {
                if !associations.iter().any(|a| a == syn) {
                    associations.push(syn.to_owned());
                }
            }
        }
    }

    Article { headword, pronunciations, explanations, examples, associations, ..Default::default() }
}

impl Provider for GoogleDictionary {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        check_pair(&self.info, req.from, req.to)?;
        self.domain.remember(host);
        let dts = ["bd", "md", "ex", "ss", "rm", "t"];
        let v = query(host, &self.limits, &self.info.name, req.text.trim(), req.from, req.to, &dts)?;
        if !v.is_array() {
            return Err(Error::bad_response(&self.info.name, "expected a JSON array"));
        }
        let mut article = parse_article(&v, &req.text);
        article.link = Some(web_link_for(&self.domain.get(), req.text.trim(), LangPair::new(req.from, req.to)));
        Ok(article)
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        Some(web_link_for(&self.domain.get(), text, pair))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_text, opts};
    use glossa_modules::provider::with_test_host;
    use glossa_modules::{CancelToken, OptionValues};
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;
    use serde_json::json;

    const HELLO_WORLD: &str = r#"[[["Привет, мир","Hello world",null,null,10],[null,null,"Privet, mir",null]],[["имя существительное",["мир","свет"],[["мир",["world","peace"],null,0.5],["свет",["light","world"],null,0.1]],"world",1]],"en",null,null,null,0.98,[],[["en"],null,[0.98],["en"]]]"#;

    fn ru() -> LangId {
        LangId::RUSSIAN
    }

    #[test]
    fn translate_builds_the_url_and_parses_everything() {
        let http = MockHttp::new().on(Method::Get, API, 200, HELLO_WORLD);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("Hello world", LangPair::new(LangId::AUTO, ru())))
        })
        .unwrap();
        assert_eq!(out.text, "Привет, мир");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        assert_eq!(out.romanisation.as_deref(), Some("Privet, mir"));
        assert_eq!(out.parts_of_speech.len(), 1);
        assert_eq!(out.parts_of_speech[0].part_of_speech, "имя существительное");
        assert_eq!(out.parts_of_speech[0].senses[0].word, "мир");
        assert_eq!(out.parts_of_speech[0].senses[0].back_translations, vec!["world", "peace"]);

        let req = &http.requests()[0];
        assert!(req.url.starts_with(&format!("{API}?client=dict-chrome-ex&sl=auto&tl=ru&hl=en")), "{}", req.url);
        for dt in ["dt=t", "dt=bd", "dt=rm", "dt=ld", "ie=UTF-8", "oe=UTF-8"] {
            assert!(req.url.contains(dt), "{dt} missing from {}", req.url);
        }
        assert!(req.url.ends_with("&q=Hello%20world"));
        assert_eq!(req.provider, "Google Translate");
    }

    #[test]
    fn google_uses_legacy_codes() {
        let http = MockHttp::new().on(Method::Get, API, 200, r#"[[["שלום","hello",null,null,1]],null,"en"]"#);
        with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("hello", LangPair::new(LangId::ENGLISH, LangId(26))))
        })
        .unwrap();
        assert!(http.requests()[0].url.contains("&sl=en&tl=iw&"));
    }

    #[test]
    fn options_can_turn_off_bd_and_rm() {
        let http = MockHttp::new().on(Method::Get, API, 200, r#"[[["x","y",null,null,1]],null,"en"]"#);
        let o = opts(&[("romanisation", json!(false)), ("dictionary", json!(false))]);
        with_test_host(&http, &o, |h| {
            Google::new().translate(h, &TranslateRequest::new("y", LangPair::new(LangId::ENGLISH, ru())))
        })
        .unwrap();
        let url = &http.requests()[0].url;
        assert!(!url.contains("dt=bd") && !url.contains("dt=rm"), "{url}");
    }

    #[test]
    fn source_and_target_romanisations_land_in_their_own_fields() {
        let v: Value = serde_json::from_str(
            r#"[[["Hello","こんにちは",null,null,10],[null,null,"Harō","Kon'nichiwa"]],null,"ja"]"#,
        )
        .unwrap();
        assert_eq!(romanisation(&v).as_deref(), Some("Harō"));
        assert_eq!(source_romanisation(&v).as_deref(), Some("Kon'nichiwa"));
        // English into Russian: the source slot holds a phonetic spelling,
        // which must not become the translation's romanisation.
        let v: Value =
            serde_json::from_str(r#"[[["Привет","hello",null,null,10],[null,null,"Privet","həˈlō"]],null,"en"]"#)
                .unwrap();
        let t = parse_translation(&v, "Google").unwrap();
        assert_eq!(t.romanisation.as_deref(), Some("Privet"));
        assert_eq!(t.source_romanisation.as_deref(), Some("həˈlō"));
        // Only a source transliteration: no romanisation of the translation.
        let v: Value =
            serde_json::from_str(r#"[[["Hello","こんにちは",null,null,10],[null,null,null,"Kon'nichiwa"]],null,"ja"]"#)
                .unwrap();
        assert_eq!(romanisation(&v), None);
        // No rm segment: a translation segment is never mistaken for one.
        let v: Value = serde_json::from_str(r#"[[["Hello","Hallo",null,null,10]],null,"de"]"#).unwrap();
        assert_eq!(romanisation(&v), None);
        assert_eq!(source_romanisation(&v), None);
    }

    #[test]
    fn bd_block_parses_with_and_without_back_translations() {
        let v: Value = serde_json::from_str(
            r#"[null,[["noun",["привет","приветствие"],[["привет",["hello","hi"],null,0.6],["приветствие",["greeting"]]],"hello",1],["interjection",["алло"]]]]"#,
        )
        .unwrap();
        let pos = parts_of_speech(&v);
        assert_eq!(pos.len(), 2);
        assert_eq!(pos[0].senses.len(), 2);
        assert_eq!(pos[0].senses[1].back_translations, vec!["greeting"]);
        assert_eq!(pos[1].part_of_speech, "interjection");
        assert_eq!(pos[1].senses[0].word, "алло");
        assert!(pos[1].senses[0].back_translations.is_empty());
        // Garbage in the slot is ignored, not a panic.
        let v: Value = serde_json::from_str(r#"[null, [42, "x", [null]]]"#).unwrap();
        assert!(parts_of_speech(&v).is_empty());
    }

    #[test]
    fn detected_language_falls_back_to_the_ld_block() {
        let v: Value =
            serde_json::from_str(r#"[[["a","b"]],null,null,null,null,null,0.5,[],[["fr"],null,[0.5],["fr"]]]"#)
                .unwrap();
        assert_eq!(detected_source(&v), Some(LangId(21)));
    }

    #[test]
    fn long_text_is_posted_as_a_form() {
        let http = MockHttp::new().on(Method::Post, API, 200, r#"[[["ok","x",null,null,1]],null,"en"]"#);
        let text = "word ".repeat(500);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new(text.clone(), LangPair::new(LangId::ENGLISH, ru())))
        })
        .unwrap();
        assert_eq!(out.text, "ok");
        let req = &http.requests()[0];
        assert_eq!(req.method, Method::Post);
        assert!(!req.url.contains("&q="));
        assert!(body_text(req).starts_with("q=word%20word"));
    }

    #[test]
    fn malformed_and_error_responses_are_errors_not_panics() {
        let http = MockHttp::new().on(Method::Get, API, 200, r#"{"unexpected":true}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, ru())))
        });
        assert!(matches!(r, Err(Error::BadResponse { .. })), "{r:?}");

        let http = MockHttp::new().on(Method::Get, API, 200, "<html>not json");
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, ru())))
        });
        assert!(matches!(r, Err(Error::BadResponse { .. })), "{r:?}");

        let http = MockHttp::new().on(Method::Get, API, 429, "Too Many Requests");
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, ru())))
        });
        assert!(matches!(r, Err(Error::RateLimited { .. })), "{r:?}");
    }

    #[test]
    fn unknown_target_is_rejected_before_any_request() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::AUTO)))
        });
        assert!(matches!(r, Err(Error::TargetLanguageUnsupported { .. })));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn detect_reads_language_and_confidence() {
        let http = MockHttp::new().on(
            Method::Get,
            API,
            200,
            r#"[null,null,"ru",null,null,null,0.93,[],[["ru"],null,[0.93],["ru"]]]"#,
        );
        let d = with_test_host(&http, &OptionValues::new(), |h| Google::new().detect(h, "привет")).unwrap();
        assert_eq!(d.lang, ru());
        assert!((d.confidence - 0.93).abs() < 1e-6);
        let url = &http.requests()[0].url;
        assert!(url.contains("sl=auto") && url.contains("dt=ld"), "{url}");

        let http = MockHttp::new().on(Method::Get, API, 200, r#"[null,null,"und"]"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| Google::new().detect(h, "???"));
        assert!(matches!(r, Err(Error::LanguageNotDetected)));
    }

    #[test]
    fn speech_is_chunked_and_concatenated() {
        let http = MockHttp::new().on(Method::Get, TTS, 200, "MP3");
        let text = "lorem ipsum dolor sit amet ".repeat(12); // ~324 chars
        let audio = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().speak(h, &SpeakRequest { text: text.clone(), lang: LangId::ENGLISH, slow: true })
        })
        .unwrap();
        assert_eq!(audio.mime, "audio/mpeg");
        let reqs = http.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(audio.bytes, b"MP3MP3");
        for (i, r) in reqs.iter().enumerate() {
            assert!(r.url.starts_with(&format!("{TTS}?client=tw-ob&ie=UTF-8&tl=en&")), "{}", r.url);
            assert!(r.url.contains(&format!("&total=2&idx={i}&")), "{}", r.url);
            assert!(r.url.ends_with("&ttsspeed=0.24"));
            let q = r.url.split("&q=").nth(1).unwrap_or_default();
            let q = q.trim_end_matches("&ttsspeed=0.24");
            // Every chunk ends on a word: no "%20" split in two, no partial word.
            assert!(!q.ends_with("lor") && !q.starts_with("em"), "{q}");
        }
    }

    #[test]
    fn speech_rejects_auto_and_reports_cancellation() {
        let http = MockHttp::new().on(Method::Get, TTS, 200, "MP3");
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().speak(h, &SpeakRequest { text: "hi".into(), lang: LangId::AUTO, slow: false })
        });
        assert!(r.is_err());

        let cancel = CancelToken::new();
        cancel.cancel();
        let options = OptionValues::new();
        let host = Host {
            http: &http,
            options: &options,
            ui_language: LangId::ENGLISH,
            preferred_domain: "com",
            cancel: &cancel,
        };
        let r = Google::new().speak(&host, &SpeakRequest { text: "hi".into(), lang: LangId::ENGLISH, slow: false });
        assert!(matches!(r, Err(Error::Cancelled)));
    }

    #[test]
    fn web_link_uses_the_preferred_domain() {
        let g = Google::new();
        let http = MockHttp::new().on(Method::Get, API, 200, HELLO_WORLD);
        let options = OptionValues::new();
        let cancel = CancelToken::new();
        let host = Host {
            http: &http,
            options: &options,
            ui_language: LangId::RUSSIAN,
            preferred_domain: "ru",
            cancel: &cancel,
        };
        g.translate(&host, &TranslateRequest::new("Hello world", LangPair::new(LangId::ENGLISH, ru()))).unwrap();
        assert!(http.requests()[0].url.contains("&hl=ru"));
        assert_eq!(
            g.web_link("a b", LangPair::new(LangId::AUTO, LangId(11))).unwrap(),
            "https://translate.google.ru/?sl=auto&tl=zh-CN&text=a%20b&op=translate"
        );
        assert_eq!(sane_domain("evil.com/x?"), "com");
    }

    const RUN: &str = r#"[
        [["бежать","run",null,null,10],[null,null,"bezhat'",null]],
        [["verb",["бежать","работать"],[["бежать",["run","flee"],null,0.4],["работать",["work","run","operate"],null,0.1]],"run",2]],
        "en",null,null,null,1,[],[["en"],null,[1],["en"]],null,null,
        [["verb",[[["sprint","race","dash"],"m_en_1"],[["operate","function"],"m_en_2"]],"run"]],
        [["verb",[["move at a speed faster than a walk","m_en_1","he <b>ran</b> across the road"],["be in charge of","m_en_2"]],"run"],["noun",[["an act of running","m_en_3"]],"run"]],
        [[["she <b>runs</b> every morning",null,null,null,3,"m_en_1"],["he <b>ran</b> across the road",null,null,null,3,"m_en_1"]]]
    ]"#;

    #[test]
    fn dictionary_article_has_definitions_examples_synonyms_and_translations() {
        let http = MockHttp::new().on(Method::Get, API, 200, RUN);
        let a = with_test_host(&http, &OptionValues::new(), |h| {
            GoogleDictionary::new()
                .dictionary(h, &DictRequest { text: " run ".into(), from: LangId::ENGLISH, to: ru(), exact: true })
        })
        .unwrap();
        assert_eq!(a.headword, "run");
        assert_eq!(a.explanations[0].part_of_speech, "verb");
        assert_eq!(a.explanations[0].definitions, vec!["move at a speed faster than a walk", "be in charge of"]);
        assert_eq!(a.explanations[1].part_of_speech, "noun");
        let tr = a.explanations.last().unwrap();
        assert_eq!(tr.part_of_speech, "translation");
        assert_eq!(tr.definitions, vec!["бежать", "verb: бежать, работать"]);
        // The definition's example and the ex block's duplicate collapse.
        assert_eq!(a.examples.len(), 2);
        assert_eq!(a.examples[0].source, "he ran across the road");
        assert_eq!(a.examples[1].source, "she runs every morning");
        assert_eq!(a.associations, vec!["sprint", "race", "dash", "operate", "function"]);
        // Target-side transliteration is not a pronunciation of "run".
        assert!(a.pronunciations.is_empty());
        assert!(a.link.as_deref().unwrap_or_default().contains("text=run"));
        assert!(!a.is_empty());

        let url = &http.requests()[0].url;
        for dt in ["dt=bd", "dt=md", "dt=ex", "dt=ss", "dt=rm", "dt=t"] {
            assert!(url.contains(dt), "{dt} missing from {url}");
        }
        assert!(url.ends_with("&q=run"));
    }

    #[test]
    fn dictionary_source_transliteration_becomes_a_pronunciation() {
        let v: Value =
            serde_json::from_str(r#"[[["cat","猫",null,null,1],[null,null,null,"Neko"]],null,"ja"]"#).unwrap();
        let a = parse_article(&v, "猫");
        assert_eq!(a.pronunciations[0].symbol, "Neko");
        assert_eq!(a.headword, "猫");
    }

    #[test]
    fn dictionary_errors_on_non_array_and_status() {
        let http = MockHttp::new().on(Method::Get, API, 200, r#"{"x":1}"#);
        let req = DictRequest { text: "run".into(), from: LangId::ENGLISH, to: ru(), exact: true };
        let r = with_test_host(&http, &OptionValues::new(), |h| GoogleDictionary::new().dictionary(h, &req));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
        let http = MockHttp::new().on(Method::Get, API, 500, "oops");
        let r = with_test_host(&http, &OptionValues::new(), |h| GoogleDictionary::new().dictionary(h, &req));
        assert!(matches!(r, Err(Error::HttpStatus { status: 500, .. })));
    }

    const SECOND: &str = "https://translate.googleapis.com/translate_a/single";
    const WEB: &str = "https://translate.google.com/translate_a/single";
    const OK: &str = r#"[[["ok","x",null,null,1]],null,"en"]"#;

    fn tr(g: &Google, http: &MockHttp) -> Result<Translation> {
        with_test_host(http, &OptionValues::new(), |h| {
            g.translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, ru())))
        })
    }

    fn hosts(http: &MockHttp) -> Vec<String> {
        http.requests().iter().map(|r| r.url.split('?').next().unwrap_or_default().to_owned()).collect()
    }

    #[test]
    fn a_refusal_moves_to_the_next_endpoint_and_is_remembered() {
        let http = MockHttp::new().on(Method::Get, API, 429, "Too Many Requests").on(Method::Get, SECOND, 200, OK);
        let g = Google::new();
        assert_eq!(tr(&g, &http).unwrap().text, "ok");
        assert_eq!(hosts(&http), vec![API, SECOND]);
        let req = &http.requests()[1];
        assert!(req.url.starts_with(&format!("{SECOND}?client=gtx&sl=en&tl=ru")), "{}", req.url);
        assert!(req.no_retry_when_limited, "the client must not retry a refusal itself");
        assert!(req.headers.iter().any(|(n, v)| n == "Accept-Language" && v.starts_with("en")), "{:?}", req.headers);

        // The refusing endpoint is left alone on the next call.
        assert_eq!(tr(&g, &http).unwrap().text, "ok");
        assert_eq!(hosts(&http), vec![API, SECOND, SECOND]);
    }

    #[test]
    fn a_cooled_off_endpoint_is_asked_again_and_forgiven() {
        let limits = Arc::new(Backoff::new(Duration::from_millis(20), Duration::from_millis(20)));
        let g = Google::sharing(limits.clone());
        let http = MockHttp::new().on_once(Method::Get, API, 429, "").on(Method::Get, API, 200, OK).on(
            Method::Get,
            SECOND,
            200,
            OK,
        );
        tr(&g, &http).unwrap();
        assert!(limits.cooling("clients5.google.com", Instant::now()).is_some());
        std::thread::sleep(Duration::from_millis(30));
        tr(&g, &http).unwrap();
        assert_eq!(hosts(&http), vec![API, SECOND, API]);
        assert_eq!(limits.cooling("clients5.google.com", Instant::now()), None);
        // Forgiven: the next refusal starts from the shortest wait again.
        assert_eq!(limits.limited("clients5.google.com", None, Instant::now()), Duration::from_millis(20));
    }

    #[test]
    fn when_every_endpoint_refuses_the_wait_is_reported_without_asking_again() {
        let http = MockHttp::new().on(Method::Get, API, 429, "").on(Method::Get, SECOND, 429, "").on(
            Method::Get,
            WEB,
            429,
            "",
        );
        let g = Google::new();
        let r = tr(&g, &http);
        assert!(matches!(r, Err(Error::RateLimited { retry_after_secs: Some(30), .. })), "{r:?}");
        assert_eq!(hosts(&http), vec![API, SECOND, WEB]);
        // All cooling off: an answer at once, and no request.
        let r = tr(&g, &http);
        assert!(matches!(r, Err(Error::RateLimited { retry_after_secs: Some(s), .. }) if s <= 30), "{r:?}");
        assert_eq!(http.requests().len(), 3);
        // Its message is the familiar one, with the wait.
        assert!(r.unwrap_err().to_string().contains("Google Translate is rate-limiting us"));
    }

    #[test]
    fn the_captcha_page_counts_as_a_refusal() {
        let captcha = "<html><body>Our systems have detected unusual traffic from your computer network. \
                       <a href=\"https://www.google.com/sorry/index?continue=x\">";
        let http = MockHttp::new().on(Method::Get, API, 200, captcha).on(Method::Get, SECOND, 200, OK);
        assert_eq!(tr(&Google::new(), &http).unwrap().text, "ok");
        assert_eq!(hosts(&http), vec![API, SECOND]);
    }

    #[test]
    fn a_server_error_tries_the_next_endpoint_without_a_cooling_off() {
        let limits = Arc::new(Backoff::default());
        let http = MockHttp::new().on(Method::Get, API, 503, "busy").on(Method::Get, SECOND, 200, OK);
        assert_eq!(tr(&Google::sharing(limits.clone()), &http).unwrap().text, "ok");
        assert_eq!(limits.cooling("clients5.google.com", Instant::now()), None);

        // Every endpoint failing that way reports the first failure.
        let http = MockHttp::new().on(Method::Get, "https://", 500, "oops");
        let r = tr(&Google::new(), &http);
        assert!(matches!(r, Err(Error::HttpStatus { status: 500, .. })), "{r:?}");
        assert_eq!(http.requests().len(), 3);
    }

    #[test]
    fn an_unreachable_host_tries_the_next_without_a_cooling_off() {
        // No route for the first address: the mock answers with a network
        // error, as a firewall or DNS blocking that one host would.
        let limits = Arc::new(Backoff::default());
        let http = MockHttp::new().on(Method::Get, SECOND, 200, OK);
        assert_eq!(tr(&Google::sharing(limits.clone()), &http).unwrap().text, "ok");
        assert_eq!(hosts(&http), vec![API, SECOND]);
        assert_eq!(limits.cooling("clients5.google.com", Instant::now()), None);
        assert_eq!(limits.cooling("translate.googleapis.com", Instant::now()), None);
    }

    #[test]
    fn no_network_gives_up_after_two_addresses() {
        let http = MockHttp::new();
        let r = tr(&Google::new(), &http);
        assert!(matches!(r, Err(Error::Network(_))), "{r:?}");
        assert_eq!(http.requests().len(), 2, "a second failure in a row is the network, not the host");
    }

    #[test]
    fn a_translation_that_mentions_the_captcha_is_not_a_refusal() {
        let echo = r#"[[["Наши системы обнаружили необычный трафик, см. /sorry/index","Our systems have detected unusual traffic, see /sorry/index",null,null,1]],null,"en"]"#;
        let limits = Arc::new(Backoff::default());
        let http = MockHttp::new().on(Method::Get, API, 200, echo);
        let out = tr(&Google::sharing(limits.clone()), &http).unwrap();
        assert!(out.text.starts_with("Наши системы"));
        assert_eq!(http.requests().len(), 1);
        assert_eq!(limits.cooling("clients5.google.com", Instant::now()), None);
    }

    #[test]
    fn another_web_page_is_its_own_error_and_tries_the_next_address() {
        let consent = "<!doctype html><html><head><title>Before you continue to Google</title></head>\
                       <body><form action=\"https://consent.google.com/save\">Accept all</form></body></html>";
        let limits = Arc::new(Backoff::default());
        let http = MockHttp::new().on(Method::Get, API, 200, consent).on(Method::Get, SECOND, 200, OK);
        assert_eq!(tr(&Google::sharing(limits.clone()), &http).unwrap().text, "ok");
        assert_eq!(limits.cooling("clients5.google.com", Instant::now()), None, "not a refusal");

        // Everywhere: reported as an unexpected page, not as a rate limit.
        let http = MockHttp::new().on(Method::Get, "https://", 200, consent);
        let r = tr(&Google::new(), &http);
        assert!(matches!(&r, Err(Error::BadResponse { detail, .. }) if detail == WEB_PAGE), "{r:?}");
        assert_eq!(http.requests().len(), 3);
    }

    #[test]
    fn the_captcha_with_a_403_is_a_refusal_too() {
        let captcha = "<html><body>unusual traffic from your computer network</body></html>";
        let http = MockHttp::new().on(Method::Get, API, 403, captcha).on(Method::Get, SECOND, 200, OK);
        let limits = Arc::new(Backoff::default());
        tr(&Google::sharing(limits.clone()), &http).unwrap();
        assert!(limits.cooling("clients5.google.com", Instant::now()).is_some());
    }

    #[test]
    fn a_long_paragraph_keeps_the_space_between_its_pieces() {
        // One paragraph, no line breaks: the cuts fall between sentences.
        let text = "The cat sat on the mat. ".repeat(900); // 21 600 characters
        let parts = split_keeping(&text, CHUNK);
        assert!(parts.len() == 3 && parts[..2].iter().all(|p| p.ends_with(". ")), "cut after the space");
        let http = MockHttp::new()
            .on_once(Method::Post, API, 200, r#"[[["one.","x",null,null,1]],null,"en"]"#)
            .on_once(Method::Post, API, 200, r#"[[["two.","x",null,null,1]],null,"en"]"#)
            .on_once(Method::Post, API, 200, r#"[[["three.","x",null,null,1]],null,"en"]"#);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new(text.clone(), LangPair::new(LangId::ENGLISH, ru())))
        })
        .unwrap();
        assert_eq!(out.text, "one. two. three.");
    }

    #[test]
    fn whitespace_at_the_start_of_a_piece_is_put_back() {
        assert_eq!(joining_space(" "), " ");
        assert_eq!(joining_space("  \n\n  "), "\n\n");
        assert_eq!(joining_space(""), "");
        // No break anywhere but a space right at the limit: the next piece
        // starts with it.
        let text = format!("{} {}", "x".repeat(100), "y".repeat(50));
        let parts = split_keeping(&text, 100);
        assert_eq!(parts[1], format!(" {}", "y".repeat(50)));
    }

    #[test]
    fn the_dictionary_shares_the_translators_cooling_off() {
        let limits = Arc::new(Backoff::default());
        let http = MockHttp::new().on(Method::Get, API, 429, "").on(Method::Get, SECOND, 200, RUN);
        tr(&Google::sharing(limits.clone()), &http).unwrap();
        let req = DictRequest { text: "run".into(), from: LangId::ENGLISH, to: ru(), exact: true };
        with_test_host(&http, &OptionValues::new(), |h| GoogleDictionary::sharing(limits).dictionary(h, &req)).unwrap();
        assert_eq!(hosts(&http), vec![API, SECOND, SECOND]);
    }

    #[test]
    fn speech_moves_to_the_next_endpoint_when_refused() {
        let http = MockHttp::new().on(Method::Get, TTS, 429, "").on(
            Method::Get,
            "https://translate.googleapis.com/translate_tts",
            200,
            "MP3",
        );
        let speak = |g: &Google| {
            with_test_host(&http, &OptionValues::new(), |h| {
                g.speak(h, &SpeakRequest { text: "hi".into(), lang: LangId::ENGLISH, slow: false })
            })
        };
        let g = Google::new();
        assert_eq!(speak(&g).unwrap().bytes, b"MP3");
        assert_eq!(speak(&g).unwrap().bytes, b"MP3");
        let urls: Vec<String> = http.requests().iter().map(|r| r.url.clone()).collect();
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[1].starts_with("https://translate.googleapis.com/translate_tts?client=gtx&"), "{}", urls[1]);
        assert!(urls[2].starts_with("https://translate.googleapis.com/"), "the refusal is remembered");
    }

    #[test]
    fn long_text_goes_in_a_few_large_requests_and_keeps_its_line_breaks() {
        let para = "The cat sat on the mat. ".repeat(200); // 4800 characters
        let text = format!("{para}\n{para}\n{para}\n{para}\n{para}");
        let http = MockHttp::new()
            .on_once(Method::Post, API, 200, r#"[[["one","x",null,null,1]],null,"en"]"#)
            .on_once(Method::Post, API, 200, r#"[[["two","x",null,null,1]],null,"en"]"#)
            .on_once(Method::Post, API, 200, r#"[[["three","x",null,null,1]],null,"en"]"#);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new(text.clone(), LangPair::new(LangId::ENGLISH, ru())))
        })
        .unwrap();
        assert_eq!(http.requests().len(), 3);
        assert_eq!(out.text, "one\ntwo\nthree");
        assert!(Google::new().info().max_input_chars.unwrap() >= 50_000);
    }

    #[test]
    fn splitting_keeps_every_character_and_prefers_line_breaks() {
        let text = format!("{}\n{}. {}", "a".repeat(70), "b".repeat(20), "c".repeat(30));
        let parts = split_keeping(&text, 100);
        assert_eq!(parts.concat(), text);
        assert_eq!(parts[0], format!("{}\n", "a".repeat(70)));
        assert!(parts.iter().all(|p| p.chars().count() <= 100));
        // A line break early in the window is passed over for a later space.
        let text = format!("ab\n{} {}", "x".repeat(80), "y".repeat(30));
        let parts = split_keeping(&text, 100);
        assert_eq!(parts[0], format!("ab\n{} ", "x".repeat(80)));
        // No break at all: cut at the limit, on a character boundary.
        let text = "я".repeat(250);
        let parts = split_keeping(&text, 100);
        assert_eq!(parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(), vec![100, 100, 50]);
        assert_eq!(split_keeping("short", 100), vec!["short"]);
        assert!(split_keeping("", 100).is_empty());
    }

    #[test]
    #[ignore = "live network"]
    fn live_google_translate() {
        let http = glossa_net::HttpClient::plain();
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Google::new().translate(h, &TranslateRequest::new("Hello world", LangPair::new(LangId::AUTO, ru())))
        })
        .unwrap();
        assert!(!out.text.is_empty());
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
    }

    /// Every endpoint on its own, so a report says which still answer.
    #[test]
    #[ignore = "live network"]
    fn live_every_endpoint_answers() {
        let http = glossa_net::HttpClient::plain();
        let mut answered = Vec::new();
        for ep in TRANSLATE.iter().chain(&SPEECH) {
            let limits = Backoff::default();
            let r = with_test_host(&http, &OptionValues::new(), |h| {
                let tts = ep.key.starts_with("tts");
                first_answer(&limits, h, "Google Translate", std::slice::from_ref(ep), |ep| {
                    let url = if tts {
                        format!("{}&ie=UTF-8&tl=en&total=1&idx=0&textlen=5&q=hello", ep.url(h, "/translate_tts"))
                    } else {
                        format!("{}&sl=en&tl=ru&dt=t&q=hello", ep.url(h, "/translate_a/single"))
                    };
                    let resp = checked(h.http.send(browser_headers(HttpRequest::get(url), h))?, "Google Translate")?;
                    Ok(resp.body.len())
                })
            });
            eprintln!("{:32} {r:?}", ep.key);
            answered.push((ep.key, r.is_ok()));
        }
        // One refusing address is what the fall-back is for; all of them
        // refusing means it would not help either.
        for group in ["translate", "tts"] {
            let ok = answered.iter().any(|(k, ok)| *ok && k.starts_with("tts") == (group == "tts"));
            assert!(ok, "no {group} endpoint answered: {answered:?}");
        }
    }
}
