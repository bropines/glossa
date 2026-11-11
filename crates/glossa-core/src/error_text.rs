//! What an [`Error`] says to the user, in the interface language.
//!
//! `Error`'s own `Display` is English and written for logs and the command
//! line. A front end shows [`Locale::describe_error`] instead: one sentence
//! that names the service or module and, where there is one, the way out
//! ("Enter it in Options → Services"). What the service or the system said
//! in its own words ([`Error::detail`]) is kept apart, for the small print
//! under the message and for the log, because it is rarely in the user's
//! language and often long.

use crate::{Error, Locale};

/// Longest [`Error::detail`], in characters: a service may answer with a
/// whole HTML page.
const DETAIL_LIMIT: usize = 300;

/// Keys that replaced a classic message. A classic `lang.json` translates
/// the old key only, so a locale that has the old text and not the new one
/// shows the old one rather than falling back to English.
const CLASSIC: &[(&str, &str)] = &[
    ("error.source_unsupported", "msg.source_unsupported"),
    ("error.target_unsupported", "msg.target_unsupported"),
    ("error.unsupported", "msg.capability_unsupported"),
    ("error.network", "msg.no_connection"),
    ("error.timeout", "msg.timeout"),
    ("error.module", "msg.script_error"),
    ("error.internal", "msg.unexpected"),
];

impl Error {
    /// The locale key of this error's message. Every variant has one: the
    /// match has no catch-all, so a new variant does not compile without it.
    pub fn locale_key(&self) -> &'static str {
        match self {
            Error::EmptyInput => "msg.nothing_to_translate",
            Error::Unsupported { .. } => "error.unsupported",
            Error::SourceLanguageUnsupported { .. } => "error.source_unsupported",
            Error::TargetLanguageUnsupported { .. } => "error.target_unsupported",
            Error::LanguageNotDetected => "msg.cannot_detect",
            Error::SameLanguage => "error.same_language",
            Error::Network(_) => "error.network",
            Error::Timeout(_) => "error.timeout",
            Error::BadResponse { .. } => "error.bad_response",
            Error::HttpStatus { status, .. } => match status {
                401 | 403 => "error.http_auth",
                429 => "error.rate_limited",
                500..=599 => "error.http_server",
                _ => "error.http_status",
            },
            Error::MissingCredential { .. } => "error.missing_credential",
            Error::RateLimited { retry_after_secs: Some(_), .. } => "error.rate_limited_secs",
            Error::RateLimited { retry_after_secs: None, .. } => "error.rate_limited",
            Error::Module { .. } | Error::ModuleAt { .. } => "error.module",
            Error::ModuleTimeout { .. } => "error.module_timeout",
            Error::AssetsMissing { .. } => "error.assets_missing",
            Error::RuntimeMissing { .. } => "error.runtime_missing",
            Error::NeedsApproval { .. } => "error.needs_approval",
            Error::AssetCorrupt { .. } => "error.asset_corrupt",
            Error::NoSuchModule(_) => "error.no_such_module",
            Error::NoProvider(_) => "error.no_provider",
            Error::NoResults { .. } => "dict.no_results",
            Error::NoTextRecognised => "msg.no_text_in_image",
            Error::Config(_) => "error.config",
            Error::FileNotFound(_) => "error.file_not_found",
            Error::Io(_) => "error.io",
            Error::Json(_) => "error.json",
            Error::Platform(_) => "error.platform",
            Error::OfflineOnly(_) => "error.offline_only",
            Error::Cancelled => "error.cancelled",
            Error::Internal(_) => "error.internal",
        }
    }

    /// What the service, the module or the system said in its own words,
    /// where the localised message does not already carry it: the small
    /// print under the message. Cut to a few lines' worth.
    pub fn detail(&self) -> Option<String> {
        let raw = match self {
            Error::Network(d) | Error::Config(d) | Error::Platform(d) | Error::Internal(d) => d.clone(),
            Error::BadResponse { detail, .. } | Error::Module { detail, .. } | Error::ModuleAt { detail, .. } => {
                detail.clone()
            }
            Error::HttpStatus { body, .. } => body.clone(),
            Error::Unsupported { capability, .. } => (*capability).to_owned(),
            Error::NoProvider(what) => (*what).to_owned(),
            Error::Io(e) => e.to_string(),
            Error::Json(e) => e.to_string(),
            Error::AssetCorrupt { expected, actual, .. } => format!("SHA-256 {actual}, expected {expected}"),
            _ => return None,
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut short: String = trimmed.chars().take(DETAIL_LIMIT).collect();
        if trimmed.chars().count() > DETAIL_LIMIT {
            short.push('…');
        }
        Some(short)
    }
}

impl Locale {
    /// The message for `e` in this language: one sentence, without the
    /// [`Error::detail`].
    pub fn describe_error(&self, e: &Error) -> String {
        let key = e.locale_key();
        // The classic text, where this locale has only that.
        let key = match CLASSIC.iter().find(|(new, _)| *new == key) {
            Some((new, old)) if !self.has(new) && self.has(old) => old,
            _ => key,
        };
        let args: Vec<String> = match e {
            Error::Unsupported { provider, .. }
            | Error::BadResponse { provider, .. }
            | Error::RateLimited { provider, retry_after_secs: None } => vec![provider.clone()],
            Error::SourceLanguageUnsupported { provider, lang }
            | Error::TargetLanguageUnsupported { provider, lang } => {
                vec![provider.clone(), self.language_name(*lang).to_owned()]
            }
            Error::Timeout(ms) => vec![seconds(*ms)],
            Error::HttpStatus { provider, status, .. } => match status {
                429 => vec![provider.clone()],
                _ => vec![provider.clone(), status.to_string()],
            },
            Error::MissingCredential { provider, what } => vec![provider.clone(), self.credential_name(what)],
            Error::RateLimited { provider, retry_after_secs: Some(secs) } => vec![provider.clone(), secs.to_string()],
            Error::Module { module, .. } | Error::ModuleAt { module, .. } | Error::NeedsApproval { module } => {
                vec![module.clone()]
            }
            Error::ModuleTimeout { module, limit_ms } => vec![module.clone(), seconds(*limit_ms)],
            Error::AssetsMissing { module, missing } => vec![module.clone(), missing.join(", ")],
            Error::RuntimeMissing { module, what } => vec![module.clone(), what.clone()],
            Error::AssetCorrupt { module, asset, .. } => vec![asset.clone(), module.clone()],
            Error::NoSuchModule(id) => vec![id.clone()],
            Error::NoResults { query, .. } => vec![query.clone()],
            Error::FileNotFound(path) => vec![path.clone()],
            Error::EmptyInput
            | Error::LanguageNotDetected
            | Error::SameLanguage
            | Error::Network(_)
            | Error::NoProvider(_)
            | Error::NoTextRecognised
            | Error::Config(_)
            | Error::Io(_)
            | Error::Json(_)
            | Error::Platform(_)
            | Error::OfflineOnly(_)
            | Error::Cancelled
            | Error::Internal(_) => Vec::new(),
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.fmt_args(key, &args)
    }

    /// What a service is missing, as a person would call it: services name
    /// the setting by its key in the configuration (`api_key`).
    fn credential_name(&self, what: &str) -> String {
        match what {
            "key" | "api_key" | "apikey" => self.get("error.credential.api_key").to_owned(),
            "token" => self.get("error.credential.token").to_owned(),
            other => self.fmt("error.credential.other", other),
        }
    }
}

/// Milliseconds as seconds for a message: `30`, `2.5`.
fn seconds(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        (ms / 1000).to_string()
    } else {
        format!("{:.1}", ms as f64 / 1000.0)
    }
}

/// One of every variant, for the tests here and in the front ends: the
/// compiler keeps [`Error::locale_key`] complete, this list is kept complete
/// by the test that counts the `error.` keys it reaches.
#[doc(hidden)]
pub fn samples() -> Vec<Error> {
    use crate::error::SourcePlace;
    use crate::LangId;
    let http = |status| Error::HttpStatus { provider: "DeepL".into(), status, body: "quota exceeded".into() };
    vec![
        Error::EmptyInput,
        Error::OfflineOnly("translate.googleapis.com".into()),
        Error::Unsupported { provider: "DeepL".into(), capability: "speech" },
        Error::SourceLanguageUnsupported { provider: "DeepL".into(), lang: LangId::RUSSIAN },
        Error::TargetLanguageUnsupported { provider: "DeepL".into(), lang: LangId::RUSSIAN },
        Error::LanguageNotDetected,
        Error::SameLanguage,
        Error::Network("connection reset".into()),
        Error::Timeout(2500),
        Error::BadResponse { provider: "DeepL".into(), detail: "expected an array".into() },
        http(401),
        http(404),
        http(429),
        http(503),
        Error::MissingCredential { provider: "DeepL".into(), what: "api_key".into() },
        Error::MissingCredential { provider: "DeepL".into(), what: "token".into() },
        Error::MissingCredential { provider: "DeepL".into(), what: "folder_id".into() },
        Error::RateLimited { provider: "DeepL".into(), retry_after_secs: Some(30) },
        Error::RateLimited { provider: "DeepL".into(), retry_after_secs: None },
        Error::Module { module: "lingva".into(), detail: "TypeError: x is undefined".into() },
        Error::ModuleAt {
            module: "lingva".into(),
            detail: "TypeError: x is undefined".into(),
            place: SourcePlace { file: "main.js".into(), line: 3, column: 1 },
        },
        Error::ModuleTimeout { module: "lingva".into(), limit_ms: 30_000 },
        Error::AssetsMissing { module: "local-llm".into(), missing: vec!["model.gguf".into()] },
        Error::RuntimeMissing { module: "argos".into(), what: "Python 3.12".into() },
        Error::NeedsApproval { module: "lingva".into() },
        Error::AssetCorrupt {
            module: "local-llm".into(),
            asset: "model.gguf".into(),
            expected: "aa".into(),
            actual: "bb".into(),
        },
        Error::NoSuchModule("lingva".into()),
        Error::NoProvider("translation"),
        Error::NoTextRecognised,
        Error::Config("unknown theme".into()),
        Error::FileNotFound("/tmp/a.png".into()),
        Error::Io(std::io::Error::other("disk full")),
        Error::Json(serde_json::from_str::<u8>("x").unwrap_err()),
        Error::Platform("no clipboard".into()),
        Error::Cancelled,
        Error::Internal("index out of range".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::STRINGS;
    use std::collections::BTreeSet;

    fn english(key: &str) -> Option<&'static str> {
        STRINGS.iter().find(|(k, _, _)| *k == key).map(|(_, text, _)| *text)
    }

    #[test]
    fn every_error_has_a_message_in_english_and_in_the_built_in_packs() {
        let packs: Vec<Locale> = std::iter::once(Locale::english())
            .chain(crate::locale::BUILTIN_LOCALES.iter().map(|p| Locale::builtin(p.code).unwrap()))
            .collect();
        for e in samples() {
            let key = e.locale_key();
            assert!(english(key).is_some(), "{e:?}: no English text for {key}");
            for locale in &packs {
                let msg = locale.describe_error(&e);
                assert!(!msg.contains("%s"), "{}: {key} left a placeholder unfilled: {msg}", locale.code);
                assert!(!msg.contains(key), "{}: {key} is not translated: {msg}", locale.code);
            }
        }
    }

    #[test]
    fn the_samples_reach_every_error_key() {
        let reached: BTreeSet<&str> = samples().iter().map(Error::locale_key).collect();
        let declared: BTreeSet<&str> = STRINGS
            .iter()
            .map(|(k, _, _)| *k)
            .filter(|k| k.starts_with("error."))
            .filter(|k| !["error.credential.", "error.action.", "error.download_"].iter().any(|p| k.starts_with(p)))
            .collect();
        let unreached: Vec<_> = declared.difference(&reached).collect();
        assert!(unreached.is_empty(), "error keys no sample reaches: {unreached:?}");
    }

    #[test]
    fn messages_name_the_service_and_the_specifics() {
        let en = Locale::english();
        let ru = Locale::builtin("ru").unwrap();
        let e = Error::MissingCredential { provider: "DeepL".into(), what: "api_key".into() };
        assert_eq!(en.describe_error(&e), "DeepL needs an API key. Enter it in Options → Services.");
        assert_eq!(ru.describe_error(&e), "DeepL: не указан ключ API. Введите его в разделе Настройки → Сервисы.");
        let e = Error::TargetLanguageUnsupported { provider: "DeepL".into(), lang: crate::LangId::RUSSIAN };
        assert_eq!(en.describe_error(&e), "DeepL does not translate into Russian.");
        assert_eq!(ru.describe_error(&e), "DeepL не поддерживает язык перевода (Русский).");
        let e = Error::HttpStatus { provider: "Bing".into(), status: 503, body: String::new() };
        assert_eq!(en.describe_error(&e), "Bing is having trouble (HTTP 503). Try again later.");
        assert_eq!(en.describe_error(&Error::Timeout(2500)), "The service did not answer within 2.5 s.");
        let e = Error::RateLimited { provider: "Bing".into(), retry_after_secs: Some(30) };
        assert_eq!(en.describe_error(&e), "Bing is getting too many requests. Try again in 30 s.");
    }

    #[test]
    fn the_detail_is_what_the_other_side_said_cut_short() {
        assert_eq!(Error::Network("connection reset".into()).detail().as_deref(), Some("connection reset"));
        assert_eq!(Error::EmptyInput.detail(), None);
        let e = Error::HttpStatus { provider: "G".into(), status: 500, body: "  ".into() };
        assert_eq!(e.detail(), None, "an empty body is no detail");
        let e = Error::HttpStatus { provider: "G".into(), status: 500, body: "x".repeat(1000) };
        let d = e.detail().unwrap();
        assert_eq!(d.chars().count(), DETAIL_LIMIT + 1);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn a_classic_pack_keeps_its_own_wording_for_the_messages_it_has() {
        let classic = Locale::parse(r#"{"code":"de","strings":{"msg.timeout":"Zeitüberschreitung."}}"#).unwrap();
        assert_eq!(classic.describe_error(&Error::Timeout(1000)), "Zeitüberschreitung.");
        // A message it lacks comes in English, with its specifics.
        assert_eq!(classic.describe_error(&Error::NoSuchModule("x".into())), "There is no service or module named x.");
    }
}
