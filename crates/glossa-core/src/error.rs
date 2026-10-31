//! The one error type the whole pipeline speaks.
//!
//! The variants were chosen so that every message the classic UI could show
//! has a home here — "service doesn't support destination language", "no data
//! returned", "cannot detect language" and the rest — because the UI needs to
//! tell those apart to say something useful, and a stringly-typed error cannot.

use crate::lang::LangId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Nothing to work with: the selection or the clipboard was empty.
    #[error("no text to translate")]
    EmptyInput,

    /// The provider was asked for something it never claimed to do.
    #[error("{provider} does not support {capability}")]
    Unsupported { provider: String, capability: &'static str },

    /// The provider works, but not for this language.
    #[error("{provider} does not support {} as a source language", lang.english_name())]
    SourceLanguageUnsupported { provider: String, lang: LangId },

    #[error("{provider} does not support {} as a target language", lang.english_name())]
    TargetLanguageUnsupported { provider: String, lang: LangId },

    /// Detection ran and came back with nothing usable.
    #[error("cannot detect the source language; please choose it manually")]
    LanguageNotDetected,

    /// Asked to translate from a language into itself, with nothing to do.
    #[error("source and target language are the same")]
    SameLanguage,

    /// The network did not carry the request.
    #[error("network error: {0}")]
    Network(String),

    /// The request went out, the reply never came back in time.
    #[error("timed out after {0} ms")]
    Timeout(u64),

    /// A reply arrived and was not what the provider's parser expected.
    #[error("{provider} returned a response we could not parse: {detail}")]
    BadResponse { provider: String, detail: String },

    /// A reply arrived with an HTTP status that means "no".
    #[error("{provider} returned HTTP {status}{}", context_suffix(body))]
    HttpStatus { provider: String, status: u16, body: String },

    /// The provider wants a key, a token or a cookie we do not have.
    #[error("{provider} needs {what} to be set in its options")]
    MissingCredential { provider: String, what: String },

    /// The provider said we are asking too often.
    #[error("{provider} is rate-limiting us{}", retry_suffix(retry_after_secs))]
    RateLimited { provider: String, retry_after_secs: Option<u64> },

    /// A module misbehaved: threw, returned the wrong shape, or died.
    #[error("module {module} failed: {detail}")]
    Module { module: String, detail: String },

    /// A module took longer than its budget and was abandoned.
    #[error("module {module} exceeded its {limit_ms} ms budget")]
    ModuleTimeout { module: String, limit_ms: u64 },

    /// We were asked for a module that is not installed or not enabled.
    #[error("no module with id {0:?}")]
    NoSuchModule(String),

    /// Nothing installed can do the job at all.
    #[error("no provider available for {0}")]
    NoProvider(&'static str),

    /// OCR ran and found no text.
    #[error("no text found in the image")]
    NoTextRecognised,

    /// Something in the user's configuration does not make sense.
    #[error("configuration error: {0}")]
    Config(String),

    /// A file we needed is not there.
    #[error("cannot find file: {0}")]
    FileNotFound(String),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The platform refused: no clipboard, no screen, no hotkey registration.
    #[error("{0}")]
    Platform(String),

    /// The user cancelled, or a newer request superseded this one.
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    /// Whether retrying the identical request could plausibly work. The UI uses
    /// this to decide between "try again" and "fix something first".
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Network(_) | Error::Timeout(_) | Error::RateLimited { .. } => true,
            Error::HttpStatus { status, .. } => {
                matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
            }
            _ => false,
        }
    }

    /// Whether the fault is in the user's setup, so another provider would hit
    /// the same wall and failing over is pointless.
    pub fn is_configuration_fault(&self) -> bool {
        matches!(
            self,
            Error::MissingCredential { .. }
                | Error::Config(_)
                | Error::FileNotFound(_)
                | Error::NoSuchModule(_)
        )
    }

    /// Whether trying a different provider for the same text makes sense.
    pub fn should_try_next_provider(&self) -> bool {
        match self {
            Error::EmptyInput | Error::SameLanguage | Error::Cancelled => false,
            other => !other.is_configuration_fault(),
        }
    }

    pub fn network(e: impl std::fmt::Display) -> Error {
        Error::Network(e.to_string())
    }

    pub fn module(module: impl Into<String>, detail: impl std::fmt::Display) -> Error {
        Error::Module { module: module.into(), detail: detail.to_string() }
    }

    pub fn bad_response(provider: impl Into<String>, detail: impl std::fmt::Display) -> Error {
        Error::BadResponse { provider: provider.into(), detail: detail.to_string() }
    }
}

/// Trim a response body down to something that fits in a message box.
fn context_suffix(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    const LIMIT: usize = 180;
    let mut short: String = trimmed.chars().take(LIMIT).collect();
    if trimmed.chars().count() > LIMIT {
        short.push('…');
    }
    format!(": {short}")
}

fn retry_suffix(after: &Option<u64>) -> String {
    match after {
        Some(secs) => format!("; retry in {secs} s"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_the_ones_worth_retrying() {
        assert!(Error::Timeout(10_000).is_transient());
        assert!(Error::Network("reset".into()).is_transient());
        assert!(Error::RateLimited { provider: "G".into(), retry_after_secs: Some(3) }
            .is_transient());
        for status in [429, 500, 502, 503, 504] {
            let e = Error::HttpStatus { provider: "G".into(), status, body: String::new() };
            assert!(e.is_transient(), "HTTP {status} should be transient");
        }
        for status in [400, 401, 403, 404, 422] {
            let e = Error::HttpStatus { provider: "G".into(), status, body: String::new() };
            assert!(!e.is_transient(), "HTTP {status} should not be transient");
        }
        assert!(!Error::EmptyInput.is_transient());
    }

    #[test]
    fn configuration_faults_do_not_fail_over() {
        let e = Error::MissingCredential { provider: "DeepL".into(), what: "api_key".into() };
        assert!(e.is_configuration_fault());
        assert!(!e.should_try_next_provider());

        let e = Error::Timeout(1);
        assert!(!e.is_configuration_fault());
        assert!(e.should_try_next_provider());
    }

    #[test]
    fn pointless_work_does_not_fail_over_either() {
        assert!(!Error::EmptyInput.should_try_next_provider());
        assert!(!Error::SameLanguage.should_try_next_provider());
        assert!(!Error::Cancelled.should_try_next_provider());
    }

    #[test]
    fn http_body_is_trimmed_into_the_message() {
        let long = "x".repeat(500);
        let e = Error::HttpStatus { provider: "G".into(), status: 500, body: long };
        let msg = e.to_string();
        assert!(msg.starts_with("G returned HTTP 500: "));
        assert!(msg.ends_with('…'));
        assert!(msg.chars().count() < 230, "message should stay short: {}", msg.chars().count());

        let e = Error::HttpStatus { provider: "G".into(), status: 500, body: "   ".into() };
        assert_eq!(e.to_string(), "G returned HTTP 500");
    }

    #[test]
    fn language_errors_name_the_language_in_english() {
        let e = Error::TargetLanguageUnsupported {
            provider: "DeepL".into(),
            lang: crate::lang::LangId::RUSSIAN,
        };
        assert_eq!(e.to_string(), "DeepL does not support Russian as a target language");
    }
}
