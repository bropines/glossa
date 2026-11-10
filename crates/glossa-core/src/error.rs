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

    /// A module failed at a known place in its own code (a script's
    /// exception). Shown exactly like [`Error::Module`]: the place is for the
    /// module's author (the module editor, the test harness), not the user.
    #[error("module {module} failed: {detail}")]
    ModuleAt { module: String, detail: String, place: SourcePlace },

    /// A module took longer than its budget and was abandoned.
    #[error("module {module} exceeded its {limit_ms} ms budget")]
    ModuleTimeout { module: String, limit_ms: u64 },

    /// A module needs files it declares in its manifest (a model, a
    /// dictionary) that have not been downloaded yet. The front end offers
    /// the download; nothing is fetched without the user's say-so unless
    /// automatic downloads are switched on.
    #[error(
        "module {module} needs its files downloaded first ({}): use Download in its settings, or run `glossa-cli module assets download {module}`",
        missing.join(", ")
    )]
    AssetsMissing { module: String, missing: Vec<String> },

    /// A Python module needs something Glossa downloads for it (a Python
    /// build, `uv`, its packages) that is not there yet. Like
    /// [`Error::AssetsMissing`], the front end offers the download.
    #[error(
        "module {module} needs {what} first: use Download in its settings, or run `glossa-cli python install {module}`"
    )]
    RuntimeMissing { module: String, what: String },

    /// A module installed by hand asks for something the user has not
    /// approved yet (it is new, or an update widened its permissions), so it
    /// is listed but does not run.
    #[error(
        "module {module} is waiting for your approval: approve it in Options > Modules, or run `glossa-cli module approve {module}`"
    )]
    NeedsApproval { module: String },

    /// A downloaded module file did not match the checksum its manifest
    /// gives. The file has been deleted; downloading again fetches it anew.
    #[error("file {asset} of module {module} is damaged or was changed on the server (SHA-256 {actual}, expected {expected}); it was deleted")]
    AssetCorrupt { module: String, asset: String, expected: String, actual: String },

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

    /// "Offline only" is switched on and this would have left the machine:
    /// a request to a host that is not this computer, or a service that
    /// does not say it works without the network.
    #[error("offline only is switched on: {0}")]
    OfflineOnly(String),

    /// The user cancelled, or a newer request superseded this one.
    #[error("cancelled")]
    Cancelled,

    /// A bug in Glossa itself: code below the provider boundary panicked and
    /// the panic was caught, so the request failed instead of the program.
    #[error("internal error: {0}")]
    Internal(String),
}

/// A place in a module's source: a file relative to its folder, 1-based line
/// and column (0 when unknown).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourcePlace {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for SourcePlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (0, _) => write!(f, "{}", self.file),
            (l, 0) => write!(f, "{}:{l}", self.file),
            (l, c) => write!(f, "{}:{l}:{c}", self.file),
        }
    }
}

impl Error {
    /// Where in a module's own code this happened, when known.
    pub fn source_place(&self) -> Option<&SourcePlace> {
        match self {
            Error::ModuleAt { place, .. } => Some(place),
            _ => None,
        }
    }

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
            Error::MissingCredential { .. } | Error::Config(_) | Error::FileNotFound(_) | Error::NoSuchModule(_)
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

/// The message a caught panic carried: the `&str` or `String` given to
/// `panic!`, else a placeholder (a panic with another payload type).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic with a non-text payload".to_owned()
    }
}

/// Run `f`, turning a panic into [`Error::Internal`] (logged) so that a bug
/// in one request fails that request rather than the whole program. The
/// release build unwinds on panic for this; the panic hook still logs the
/// panic with its stack first.
pub fn catch_panic<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = panic_message(payload.as_ref());
            tracing::error!("{what} panicked: {msg}");
            Err(Error::Internal(format!("{what} panicked: {msg}")))
        }
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
        assert!(Error::RateLimited { provider: "G".into(), retry_after_secs: Some(3) }.is_transient());
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
        let e = Error::TargetLanguageUnsupported { provider: "DeepL".into(), lang: crate::lang::LangId::RUSSIAN };
        assert_eq!(e.to_string(), "DeepL does not support Russian as a target language");
    }

    #[test]
    fn a_caught_panic_becomes_an_internal_error() {
        let r: Result<u32> = catch_panic("test job", || panic!("index {} out of range", 7));
        match r {
            Err(Error::Internal(m)) => assert_eq!(m, "test job panicked: index 7 out of range"),
            other => panic!("expected an internal error, got {other:?}"),
        }
        assert_eq!(catch_panic("ok", || Ok(5)).unwrap(), 5);
        let r: Result<()> = catch_panic("static", || panic!("plain"));
        assert!(matches!(r, Err(Error::Internal(m)) if m.ends_with("plain")));
    }
}
