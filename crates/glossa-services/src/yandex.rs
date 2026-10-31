//! Yandex Translate, through the official Yandex Cloud API (v2).
//!
//! Authentication is an API key of a service account (`Api-Key …`). An IAM
//! token (`t1.…`) works too and is sent as a bearer token; that route needs
//! the folder ID, which an API key does not.

use glossa_core::{
    Capabilities, Detection, Error, LangId, LangPair, ProviderInfo, Result, TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::{json, Value};

use crate::common::{check_pair, detected, is_auto, json_body, require_str};

const API: &str = "https://translate.api.cloud.yandex.net/translate/v2";

/// Table languages Yandex does not translate.
const MISSING: &[&str] = &["zh-Hant", "hmn"];

/// ISO 639-1, with one Chinese (`zh`), `no` for Norwegian and `tl` for
/// Filipino.
fn code(lang: LangId) -> &'static str {
    match lang.bcp47() {
        "zh-Hans" => "zh",
        "nb" => "no",
        "fil" => "tl",
        other => other,
    }
}

/// Yandex Cloud Translate.
pub struct Yandex {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for Yandex {
    fn default() -> Self {
        Self::new()
    }
}

impl Yandex {
    pub const ID: &'static str = "yandex";

    pub fn new() -> Self {
        let mut info =
            ProviderInfo::new(Self::ID, "Yandex Translate", Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE);
        info.description = "Yandex's translator through the official Yandex Cloud API. Strong for Russian and \
                            the languages of the former USSR. Needs a Yandex Cloud API key."
            .into();
        info.homepage = "https://yandex.cloud/en/services/translate".into();
        info.languages = std::iter::once(LangId::AUTO)
            .chain(LangId::all().filter(|l| !MISSING.contains(&l.bcp47())))
            .collect();
        info.max_input_chars = Some(10_000);
        info.needs_credentials = true;
        let options = vec![
            OptionSpec::secret("api_key", "API key")
                .required()
                .help("A service-account API key from the Yandex Cloud console (an IAM token also works)."),
            OptionSpec::text("folder_id", "Folder ID", "")
                .placeholder("b1g…")
                .help("Only needed when authenticating with an IAM token or a user account."),
        ];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }

    fn post(&self, host: &Host, method: &str, mut body: Value) -> Result<Value> {
        let key = host.require("api_key", self.name())?;
        let folder = host.option_str("folder_id").trim();
        if !folder.is_empty() {
            body["folderId"] = Value::String(folder.to_owned());
        }
        let auth = if key.starts_with("t1.") { format!("Bearer {key}") } else { format!("Api-Key {key}") };
        let req = HttpRequest::post(format!("{API}/{method}"))
            .json(&body)
            .header("Authorization", auth)
            .provider(self.name());
        json_body(host.http.send(req)?, self.name())
    }
}

impl Provider for Yandex {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let mut body = json!({ "texts": [req.text], "targetLanguageCode": code(req.to) });
        if !is_auto(req.from) {
            body["sourceLanguageCode"] = Value::String(code(req.from).to_owned());
        }
        let v = self.post(host, "translate", body)?;
        let item = &v["translations"][0];
        let text = require_str(&item["text"], "translations[0].text", self.name())?;
        Ok(Translation {
            text: text.to_owned(),
            detected_from: item["detectedLanguageCode"].as_str().and_then(detected),
            ..Default::default()
        })
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let v = self.post(host, "detect", json!({ "text": text }))?;
        if !v.is_object() {
            return Err(Error::bad_response(self.name(), "expected an object"));
        }
        let lang = v["languageCode"].as_str().and_then(detected).ok_or(Error::LanguageNotDetected)?;
        Ok(Detection::certain(lang))
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        let from = if is_auto(pair.from) { "auto" } else { code(pair.from) };
        Some(format!(
            "https://translate.yandex.com/?source_lang={}&target_lang={}&text={}",
            from,
            code(pair.to),
            url_encode(text)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_json, header, opts};
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    fn req(from: LangId, to: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello world", LangPair::new(from, to))
    }

    #[test]
    fn translate_sends_the_documented_body_and_header() {
        let http = MockHttp::new().on(
            Method::Post,
            &format!("{API}/translate"),
            200,
            r#"{"translations":[{"text":"Привет, мир","detectedLanguageCode":"en"}]}"#,
        );
        let o = opts(&[("api_key", json!("AQVN-key")), ("folder_id", json!("b1gfolder"))]);
        let out = with_test_host(&http, &o, |h| Yandex::new().translate(h, &req(LangId::AUTO, LangId::RUSSIAN))).unwrap();
        assert_eq!(out.text, "Привет, мир");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        let r = &http.requests()[0];
        assert_eq!(header(r, "authorization"), Some("Api-Key AQVN-key"));
        assert_eq!(
            body_json(r),
            json!({ "texts": ["Hello world"], "targetLanguageCode": "ru", "folderId": "b1gfolder" })
        );
    }

    #[test]
    fn source_codes_and_iam_tokens() {
        let http = MockHttp::new().on(Method::Post, API, 200, r#"{"translations":[{"text":"x"}]}"#);
        let o = opts(&[("api_key", json!("t1.iam-token"))]);
        with_test_host(&http, &o, |h| Yandex::new().translate(h, &req(LangId(41), LangId(11)))).unwrap();
        let r = &http.requests()[0];
        assert_eq!(header(r, "authorization"), Some("Bearer t1.iam-token"));
        let b = body_json(r);
        assert_eq!(b["sourceLanguageCode"], "no");
        assert_eq!(b["targetLanguageCode"], "zh");
        assert!(b.get("folderId").is_none());
    }

    #[test]
    fn missing_key_and_unsupported_target() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| Yandex::new().translate(h, &req(LangId::AUTO, LangId::RUSSIAN)));
        assert!(matches!(r, Err(Error::MissingCredential { .. })));
        let o = opts(&[("api_key", json!("k"))]);
        let r = with_test_host(&http, &o, |h| Yandex::new().translate(h, &req(LangId::AUTO, LangId(12))));
        assert!(matches!(r, Err(Error::TargetLanguageUnsupported { .. })));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn api_errors_carry_the_message() {
        let http = MockHttp::new().on(
            Method::Post,
            API,
            401,
            r#"{"code":16,"message":"Unknown api key 'AQVN...'","details":[{"@type":"type.googleapis.com/google.rpc.RequestInfo","requestId":"abc"}]}"#,
        );
        let o = opts(&[("api_key", json!("AQVN"))]);
        match with_test_host(&http, &o, |h| Yandex::new().translate(h, &req(LangId::AUTO, LangId::RUSSIAN))) {
            Err(Error::HttpStatus { status: 401, body, .. }) => assert!(body.starts_with("Unknown api key")),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Post, API, 200, r#"{"translations":[{}]}"#);
        let r = with_test_host(&http, &o, |h| Yandex::new().translate(h, &req(LangId::AUTO, LangId::RUSSIAN)));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }

    #[test]
    fn detect_reads_the_language_code() {
        let http = MockHttp::new().on(Method::Post, &format!("{API}/detect"), 200, r#"{"languageCode":"uk"}"#);
        let o = opts(&[("api_key", json!("k"))]);
        let d = with_test_host(&http, &o, |h| Yandex::new().detect(h, "Привіт")).unwrap();
        assert_eq!(d.lang, LangId::from_code("uk").unwrap());
        assert_eq!(body_json(&http.requests()[0]), json!({ "text": "Привіт" }));

        let http = MockHttp::new().on(Method::Post, API, 200, r#"{"languageCode":""}"#);
        let r = with_test_host(&http, &o, |h| Yandex::new().detect(h, "?"));
        assert!(matches!(r, Err(Error::LanguageNotDetected)));
    }
}
