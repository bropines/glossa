//! The Glossa Module Protocol: message types.
//!
//! Languages travel as BCP 47 codes (`en`, `ru`, `zh-Hans`, `he`) plus the
//! pseudo-code `auto`. A module whose service spells codes differently maps
//! them itself, or declares a `[languages.map]` table in its manifest and
//! lets the host do it.
//!
//! Every request carries the module's option values (`options`), merged from
//! the manifest defaults and the user's settings, so a module never needs to
//! ask for them.

use std::collections::BTreeMap;

use base64::Engine;
use serde::{Deserialize, Serialize};

pub type Options = BTreeMap<String, serde_json::Value>;

// -- requests -----------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslateParams {
    pub text: String,
    /// Source code, or `auto`.
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glossary: Vec<GlossaryTerm>,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryTerm {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectParams {
    pub text: String,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DictionaryParams {
    pub text: String,
    pub from: String,
    pub to: String,
    #[serde(default = "yes")]
    pub exact: bool,
    #[serde(default)]
    pub options: Options,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakParams {
    pub text: String,
    pub lang: String,
    #[serde(default)]
    pub slow: bool,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognizeParams {
    /// The image, base64-encoded (PNG, JPEG or WebP).
    pub image_base64: String,
    /// Language of the text, or `auto`.
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_to: Option<String>,
    #[serde(default = "yes")]
    pub preserve_line_breaks: bool,
    #[serde(default)]
    pub options: Options,
}

impl RecognizeParams {
    pub fn image_bytes(&self) -> Result<Vec<u8>, ModuleError> {
        base64::engine::general_purpose::STANDARD
            .decode(self.image_base64.as_bytes())
            .map_err(|e| ModuleError::new(ErrorKind::BadRequest, format!("image_base64: {e}")))
    }
}

// -- results ------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslateResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub romanization: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts_of_speech: Vec<PartOfSpeech>,
}

impl TranslateResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PartOfSpeech {
    pub part_of_speech: String,
    pub senses: Vec<Sense>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sense {
    pub word: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back_translations: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectResult {
    pub lang: String,
    #[serde(default = "one")]
    pub confidence: f32,
}

fn one() -> f32 {
    1.0
}

/// A dictionary article. Field names follow pot-app's dictionary-shaped
/// translate result (`trait`, `explains`), so code written for it ports
/// directly; the descriptive names are accepted too.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArticleResult {
    #[serde(default)]
    pub headword: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pronunciations: Vec<Pronunciation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explanations: Vec<Explanation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "sentence")]
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub associations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pronunciation {
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Explanation {
    #[serde(default, rename = "trait", alias = "part_of_speech")]
    pub part_of_speech: String,
    #[serde(default, rename = "explains", alias = "definitions")]
    pub definitions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub source: String,
    #[serde(default)]
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakResult {
    pub audio_base64: String,
    /// `audio/mpeg`, `audio/wav`, `audio/ogg`.
    pub mime: String,
}

impl SpeakResult {
    pub fn from_bytes(bytes: &[u8], mime: impl Into<String>) -> Self {
        Self { audio_base64: base64::engine::general_purpose::STANDARD.encode(bytes), mime: mime.into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognizeResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_lang: Option<String>,
}

// -- host calls ----------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpRequest {
    #[serde(default = "get")]
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// A text body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// A binary body, base64-encoded. Takes precedence over `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Return the body base64-encoded instead of as text (for audio, images).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

fn get() -> String {
    "GET".into()
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        Self { method: "POST".into(), url: url.into(), headers, body: Some(body.to_string()), ..Default::default() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The body as text (decoded per the response charset).
    #[serde(default)]
    pub body: String,
    /// The body base64-encoded, when the request set `binary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body if the status was 2xx, else an error carrying the status.
    pub fn ok_text(self) -> Result<String, ModuleError> {
        if self.is_success() {
            Ok(self.body)
        } else {
            Err(ModuleError::new(ErrorKind::Http, format!("HTTP {}: {}", self.status, truncate(&self.body, 200))))
        }
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ModuleError> {
        serde_json::from_str(&self.body).map_err(|e| ModuleError::new(ErrorKind::BadResponse, e.to_string()))
    }

    pub fn bytes(&self) -> Result<Vec<u8>, ModuleError> {
        match &self.body_base64 {
            Some(b) => base64::engine::general_purpose::STANDARD
                .decode(b.as_bytes())
                .map_err(|e| ModuleError::new(ErrorKind::BadResponse, e.to_string())),
            None => Ok(self.body.clone().into_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// -- envelopes ------------------------------------------------------------------

/// What a module (or the host) answers: a result or an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Result(serde_json::Value),
    Error(ModuleError),
}

impl Reply {
    pub fn ok(value: serde_json::Value) -> Self {
        Reply::Result(value)
    }

    pub fn err(error: ModuleError) -> Self {
        Reply::Error(error)
    }

    pub fn into_result(self) -> Result<serde_json::Value, ModuleError> {
        match self {
            Reply::Result(v) => Ok(v),
            Reply::Error(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleError {
    pub kind: ErrorKind,
    pub message: String,
    /// For `rate_limited`: seconds until a retry may work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl ModuleError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), retry_after_secs: None }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }

    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorKind::Unsupported, format!("{what} is not supported by this module"))
    }

    /// The module needs an option the user has not filled in.
    pub fn missing_option(key: &str) -> Self {
        Self::new(ErrorKind::MissingCredential, key)
    }

    pub fn bad_response(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadResponse, message)
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ModuleError {}

/// Error kinds, chosen to map onto the host's own error type so the UI can say
/// something specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request was malformed.
    BadRequest,
    /// The module does not do this.
    Unsupported,
    /// The source language is not supported.
    SourceLanguage,
    /// The target language is not supported.
    TargetLanguage,
    /// An option the module needs (a key, a URL) is not set.
    MissingCredential,
    /// The service answered with something unparseable.
    BadResponse,
    /// The service answered with an error status.
    Http,
    /// The network failed.
    Network,
    /// The service is rate-limiting.
    RateLimited,
    /// Nothing was found (dictionary miss, no text in image).
    NotFound,
    Other,
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        s.chars().take(n).chain(std::iter::once('…')).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_envelopes_serialise_as_documented() {
        let ok = serde_json::to_value(Reply::ok(serde_json::json!({"text": "hi"}))).unwrap();
        assert_eq!(ok, serde_json::json!({"result": {"text": "hi"}}));
        let err = serde_json::to_value(Reply::err(ModuleError::missing_option("api_key"))).unwrap();
        assert_eq!(err, serde_json::json!({"error": {"kind": "missing_credential", "message": "api_key"}}));
    }

    #[test]
    fn requests_tolerate_missing_optional_fields() {
        let p: TranslateParams = serde_json::from_str(r#"{"text":"hi","from":"auto","to":"ru"}"#).unwrap();
        assert!(p.options.is_empty());
        assert!(p.glossary.is_empty());
        let d: DictionaryParams = serde_json::from_str(r#"{"text":"run","from":"en","to":"ru"}"#).unwrap();
        assert!(d.exact, "exact defaults to true");
        let h: HttpRequest = serde_json::from_str(r#"{"url":"https://x"}"#).unwrap();
        assert_eq!(h.method, "GET");
    }

    #[test]
    fn pot_shaped_articles_parse() {
        let a: ArticleResult = serde_json::from_str(
            r#"{"explanations":[{"trait":"n.","explains":["a thing"]}],"sentence":[{"source":"a","target":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(a.explanations[0].part_of_speech, "n.");
        assert_eq!(a.examples.len(), 1, "pot's `sentence` maps to examples");
    }

    #[test]
    fn speak_result_round_trips_bytes() {
        let r = SpeakResult::from_bytes(&[1, 2, 3, 255], "audio/mpeg");
        let resp = HttpResponse { status: 200, body_base64: Some(r.audio_base64.clone()), ..Default::default() };
        assert_eq!(resp.bytes().unwrap(), vec![1, 2, 3, 255]);
    }

    #[test]
    fn non_success_responses_become_errors_with_the_status() {
        let r = HttpResponse { status: 401, body: "bad key".into(), ..Default::default() };
        let e = r.ok_text().unwrap_err();
        assert_eq!(e.kind, ErrorKind::Http);
        assert!(e.message.contains("401"));
    }
}
