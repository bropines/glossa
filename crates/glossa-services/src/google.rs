//! Google Translate, through the keyless `client=gtx` endpoint the browser
//! extensions use, plus the `translate_tts` speech endpoint.
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

use std::sync::RwLock;

use glossa_core::dict::{Example, Explanation, Pronunciation};
use glossa_core::translate::{PartOfSpeechEntry, Sense};
use glossa_core::{
    Article, Audio, Capabilities, Detection, DictRequest, Error, LangId, LangPair, ProviderInfo, Result, SpeakRequest,
    TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::Value;

use crate::common::{
    all_langs_with_auto, check_pair, check_status, detected, is_auto, json_body, split_text, strip_tags,
};

const API: &str = "https://translate.googleapis.com/translate_a/single";
const TTS: &str = "https://translate.google.com/translate_tts";
/// Past this many bytes of encoded text the query string gets long enough for
/// Google to answer 413/400, so the text moves into a form body.
const GET_LIMIT: usize = 1800;
/// The TTS endpoint refuses longer input.
const TTS_CHUNK: usize = 200;

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

/// One call to the `gtx` endpoint.
fn query(host: &Host, provider: &str, text: &str, from: LangId, to: LangId, dts: &[&str]) -> Result<Value> {
    let mut url = format!("{API}?client=gtx&sl={}&tl={}&hl={}", url_encode(code(from)), url_encode(code(to)), hl(host));
    for dt in dts {
        url.push_str("&dt=");
        url.push_str(dt);
    }
    url.push_str("&ie=UTF-8&oe=UTF-8");
    let encoded = url_encode(text);
    let req = if encoded.len() > GET_LIMIT {
        HttpRequest::post(url).form(&[("q", text)])
    } else {
        HttpRequest::get(format!("{url}&q={encoded}"))
    };
    json_body(host.http.send(req.provider(provider))?, provider)
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

/// Romanisation for the result pane: the source's (Japanese, Russian input)
/// when there is one, else the translation's.
pub(crate) fn romanisation(v: &Value) -> Option<String> {
    let seg = translit_segment(v)?;
    non_empty(&seg[3]).or_else(|| non_empty(&seg[2]))
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
        parts_of_speech: parts_of_speech(v),
        ..Default::default()
    })
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
}

impl Default for Google {
    fn default() -> Self {
        Self::new()
    }
}

impl Google {
    pub const ID: &'static str = "google";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(
            Self::ID,
            "Google Translate",
            Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE | Capabilities::LISTEN,
        );
        info.description = "Google's free web translator. No key needed; also detects languages, shows \
                            romanisation and per-part-of-speech alternatives, and reads text aloud."
            .into();
        info.homepage = "https://translate.google.com".into();
        info.languages = all_langs_with_auto();
        info.max_input_chars = Some(5000);
        let options = vec![
            OptionSpec::boolean("romanisation", "Show romanisation", true)
                .help("Add a Latin transcription of non-Latin text under the result."),
            OptionSpec::boolean("dictionary", "Show alternatives by part of speech", true)
                .help("For single words, list other translations grouped as noun, verb, …"),
        ];
        Self { info, options, domain: DomainMemo::default() }
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
        let v = query(host, &self.info.name, &req.text, req.from, req.to, &dts)?;
        parse_translation(&v, &self.info.name)
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        self.domain.remember(host);
        let v = query(host, &self.info.name, text, LangId::AUTO, LangId::ENGLISH, &["ld"])?;
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
        let mut bytes = Vec::new();
        for (idx, chunk) in chunks.iter().enumerate() {
            host.check_cancelled()?;
            let mut url = format!(
                "{TTS}?ie=UTF-8&client=tw-ob&tl={}&total={total}&idx={idx}&textlen={}&q={}",
                url_encode(req.lang.legacy_code()),
                chunk.chars().count(),
                url_encode(chunk)
            );
            if req.slow {
                url.push_str("&ttsspeed=0.24");
            }
            let resp = check_status(host.http.send(HttpRequest::get(url).provider(&self.info.name))?, &self.info.name)?;
            // A captcha page arrives as HTML with status 200.
            if resp.header("content-type").is_some_and(|ct| ct.starts_with("text/html")) || resp.body.is_empty() {
                return Err(Error::bad_response(&self.info.name, "speech endpoint returned no audio"));
            }
            // MP3 frames concatenate into a playable stream.
            bytes.extend_from_slice(&resp.body);
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
}

impl Default for GoogleDictionary {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleDictionary {
    pub const ID: &'static str = "google-dictionary";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(Self::ID, "Google Dictionary", Capabilities::DICTIONARY);
        info.description = "Definitions, usage examples and synonyms from Google Translate's dictionary, \
                            with translations grouped by part of speech. No key needed."
            .into();
        info.homepage = "https://translate.google.com".into();
        info.languages = all_langs_with_auto();
        info.max_input_chars = Some(200);
        Self { info, domain: DomainMemo::default() }
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
        let v = query(host, &self.info.name, req.text.trim(), req.from, req.to, &["bd", "md", "ex", "ss", "rm", "t"])?;
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
    fn translate_builds_the_gtx_url_and_parses_everything() {
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
        assert!(req.url.starts_with(&format!("{API}?client=gtx&sl=auto&tl=ru&hl=en")), "{}", req.url);
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
    fn source_romanisation_wins_over_target() {
        let v: Value = serde_json::from_str(
            r#"[[["Hello","こんにちは",null,null,10],[null,null,"Harō","Kon'nichiwa"]],null,"ja"]"#,
        )
        .unwrap();
        assert_eq!(romanisation(&v).as_deref(), Some("Kon'nichiwa"));
        // No rm segment: a translation segment is never mistaken for one.
        let v: Value = serde_json::from_str(r#"[[["Hello","Hallo",null,null,10]],null,"de"]"#).unwrap();
        assert_eq!(romanisation(&v), None);
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
            assert!(r.url.contains("client=tw-ob&tl=en"), "{}", r.url);
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
}
