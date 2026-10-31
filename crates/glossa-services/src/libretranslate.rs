//! LibreTranslate: the open-source, self-hostable translator built on Argos
//! Translate. The public instance at libretranslate.com wants a key; a
//! self-hosted one usually does not.

use glossa_core::{
    Capabilities, Detection, Error, LangId, ProviderInfo, Result, TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{HttpRequest, HttpResponse};
use serde_json::{json, Value};

use crate::common::{base_url, check_pair, check_status, detected, is_auto, parse_json, require_str};

const DEFAULT_URL: &str = "https://libretranslate.com";

/// LibreTranslate's codes, which are ISO 639-1 apart from Chinese: `zh` for
/// Simplified and `zt` for Traditional.
const LANGS: &[&str] = &[
    "ar", "az", "bg", "bn", "ca", "cs", "da", "de", "el", "en", "eo", "es", "et", "eu", "fa", "fi", "fr", "ga",
    "gl", "he", "hi", "hu", "id", "it", "ja", "ko", "lt", "lv", "ms", "nb", "nl", "pl", "pt", "ro", "ru", "sk",
    "sl", "sq", "sv", "th", "tl", "tr", "uk", "ur", "vi", "zh", "zt",
];

fn code(lang: LangId) -> &'static str {
    match lang.bcp47() {
        "zh-Hans" => "zh",
        "zh-Hant" => "zt",
        "fil" => "tl",
        other => other,
    }
}

fn from_code(code: &str) -> Option<LangId> {
    match code {
        "zt" => Some(LangId(12)),
        "pb" => LangId::from_code("pt"),
        other => detected(other),
    }
}

/// LibreTranslate.
pub struct LibreTranslate {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for LibreTranslate {
    fn default() -> Self {
        Self::new()
    }
}

impl LibreTranslate {
    pub const ID: &'static str = "libretranslate";

    pub fn new() -> Self {
        let mut info =
            ProviderInfo::new(Self::ID, "LibreTranslate", Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE);
        info.description = "Free and open-source machine translation you can host yourself. The public \
                            libretranslate.com instance needs an API key."
            .into();
        info.homepage = "https://libretranslate.com".into();
        info.languages = std::iter::once(LangId::AUTO).chain(LANGS.iter().filter_map(|c| from_code(c))).collect();
        info.max_input_chars = Some(5000);
        let options = vec![
            OptionSpec::url("url", "Server URL", DEFAULT_URL).help("Your own instance, e.g. http://localhost:5000."),
            OptionSpec::secret("api_key", "API key").help("Needed for libretranslate.com; most self-hosted servers do not ask for one."),
        ];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }

    fn post(&self, host: &Host, path: &str, mut body: Value) -> Result<Value> {
        let key = host.option_str("api_key").trim();
        if !key.is_empty() {
            body["api_key"] = Value::String(key.to_owned());
        }
        let url = format!("{}{path}", base_url(host.option_str("url"), DEFAULT_URL));
        let resp = host.http.send(HttpRequest::post(url).json(&body).provider(self.name()))?;
        self.check(resp, key.is_empty())
    }

    /// A 400/403 about a key, with no key set, means the instance wants one.
    fn check(&self, resp: HttpResponse, no_key: bool) -> Result<Value> {
        if no_key && matches!(resp.status, 400 | 403) && resp.text().to_ascii_lowercase().contains("api key") {
            return Err(Error::MissingCredential { provider: self.name().to_owned(), what: "api_key".into() });
        }
        let resp = check_status(resp, self.name())?;
        parse_json(&resp, self.name())
    }
}

impl Provider for LibreTranslate {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let source = if is_auto(req.from) { "auto" } else { code(req.from) };
        let v = self.post(
            host,
            "/translate",
            json!({ "q": req.text, "source": source, "target": code(req.to), "format": "text" }),
        )?;
        let text = require_str(&v["translatedText"], "translatedText", self.name())?;
        let alternatives = v["alternatives"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(Translation {
            text: text.to_owned(),
            detected_from: v["detectedLanguage"]["language"].as_str().and_then(from_code),
            alternatives,
            ..Default::default()
        })
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let v = self.post(host, "/detect", json!({ "q": text }))?;
        let best = &v[0];
        if !best.is_object() {
            return Err(Error::bad_response(self.name(), "expected an array of detections"));
        }
        let lang = best["language"].as_str().and_then(from_code).ok_or(Error::LanguageNotDetected)?;
        // LibreTranslate reports confidence as a percentage.
        let confidence = (best["confidence"].as_f64().unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32;
        Ok(Detection { lang, confidence })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_json, opts};
    use glossa_core::LangPair;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    #[test]
    fn translate_posts_json_and_reads_the_detected_language() {
        let http = MockHttp::new().on(
            Method::Post,
            "http://localhost:5000/translate",
            200,
            r#"{"detectedLanguage":{"confidence":90.0,"language":"fr"},"translatedText":"Hello!"}"#,
        );
        let o = opts(&[("url", json!("http://localhost:5000/"))]);
        let out = with_test_host(&http, &o, |h| {
            LibreTranslate::new().translate(h, &TranslateRequest::new("Bonjour!", LangPair::new(LangId::AUTO, LangId::ENGLISH)))
        })
        .unwrap();
        assert_eq!(out.text, "Hello!");
        assert_eq!(out.detected_from, Some(LangId(21)));
        let b = body_json(&http.requests()[0]);
        assert_eq!(b, json!({ "q": "Bonjour!", "source": "auto", "target": "en", "format": "text" }));
    }

    #[test]
    fn api_key_and_chinese_codes_are_sent() {
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, r#"{"translatedText":"你好"}"#);
        let o = opts(&[("api_key", json!("lt-key"))]);
        with_test_host(&http, &o, |h| {
            LibreTranslate::new().translate(h, &TranslateRequest::new("hi", LangPair::new(LangId::ENGLISH, LangId(12))))
        })
        .unwrap();
        let r = &http.requests()[0];
        assert_eq!(r.url, "https://libretranslate.com/translate");
        let b = body_json(r);
        assert_eq!(b["api_key"], "lt-key");
        assert_eq!(b["source"], "en");
        assert_eq!(b["target"], "zt");
    }

    #[test]
    fn a_key_demand_without_a_key_is_a_credential_error() {
        let http = MockHttp::new().on(
            Method::Post,
            DEFAULT_URL,
            400,
            r#"{"error":"Visit https://portal.libretranslate.com to get an API key"}"#,
        );
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            LibreTranslate::new().translate(h, &TranslateRequest::new("hi", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
        });
        assert!(matches!(r, Err(Error::MissingCredential { .. })), "{r:?}");
    }

    #[test]
    fn other_errors_carry_the_server_message() {
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 400, r#"{"error":"ru is not supported"}"#);
        let o = opts(&[("api_key", json!("k"))]);
        match with_test_host(&http, &o, |h| {
            LibreTranslate::new().translate(h, &TranslateRequest::new("hi", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
        }) {
            Err(Error::HttpStatus { status: 400, body, .. }) => assert_eq!(body, "ru is not supported"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn detect_scales_the_percentage() {
        let http = MockHttp::new().on(
            Method::Post,
            "https://libretranslate.com/detect",
            200,
            r#"[{"confidence":85.0,"language":"de"},{"confidence":10.0,"language":"nl"}]"#,
        );
        let d = with_test_host(&http, &OptionValues::new(), |h| LibreTranslate::new().detect(h, "Guten Tag")).unwrap();
        assert_eq!(d.lang, LangId(23));
        assert!((d.confidence - 0.85).abs() < 1e-6);
        assert_eq!(body_json(&http.requests()[0]), json!({ "q": "Guten Tag" }));

        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, r#"[]"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| LibreTranslate::new().detect(h, "?"));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }

    #[test]
    fn unsupported_target_is_rejected() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            LibreTranslate::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::from_code("km").unwrap())))
        });
        assert!(matches!(r, Err(Error::TargetLanguageUnsupported { .. })));
    }
}
