//! DeepL, through the official API (free or Pro key).
//!
//! The key decides the host: DeepL issues free-plan keys with a `:fx`
//! suffix, and those only work against `api-free.deepl.com`.

use glossa_core::{
    Capabilities, Error, LangId, LangPair, ProviderInfo, Result, TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::{json, Value};

use crate::common::{check_pair, check_status, detected, is_auto, langs_from_codes, parse_json, require_str};

const FREE_HOST: &str = "https://api-free.deepl.com";
const PRO_HOST: &str = "https://api.deepl.com";

/// What DeepL translates, as table codes. Chinese is one source language
/// (`ZH`) but two targets (`ZH-HANS`, `ZH-HANT`).
const LANGS: &[&str] = &[
    "auto", "ar", "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "he", "hu", "id", "it", "ja", "ko",
    "lt", "lv", "nb", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "th", "tr", "uk", "vi", "zh-Hans", "zh-Hant",
];

/// Targets that accept a `formality` setting other than the `prefer_*` ones.
const FORMALITY_TARGETS: &[&str] = &["DE", "FR", "IT", "ES", "NL", "PL", "PT-BR", "PT-PT", "JA", "RU"];

/// The source code: the base language, upper-cased.
pub(crate) fn source_code(lang: LangId) -> String {
    match lang.bcp47() {
        "zh-Hans" | "zh-Hant" => "ZH".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

/// The target code. English and Portuguese must name a variant; which one is
/// the user's choice.
fn target_code(host: &Host, lang: LangId) -> String {
    match lang.bcp47() {
        "en" => variant(host.option_str("english_variant"), &["EN-US", "EN-GB"]),
        "pt" => variant(host.option_str("portuguese_variant"), &["PT-BR", "PT-PT"]),
        "zh-Hans" => "ZH-HANS".to_owned(),
        "zh-Hant" => "ZH-HANT".to_owned(),
        other => other.to_ascii_uppercase(),
    }
}

/// The chosen variant if it is one of `allowed`, else the first.
fn variant(chosen: &str, allowed: &[&str]) -> String {
    let c = chosen.trim().to_ascii_uppercase();
    if allowed.contains(&c.as_str()) {
        c
    } else {
        allowed[0].to_owned()
    }
}

/// The API host for a key.
pub(crate) fn host_for_key(key: &str) -> &'static str {
    if key.ends_with(":fx") {
        FREE_HOST
    } else {
        PRO_HOST
    }
}

/// DeepL API.
pub struct DeepL {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for DeepL {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepL {
    pub const ID: &'static str = "deepl";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(Self::ID, "DeepL", Capabilities::TRANSLATE);
        info.description = "DeepL's neural translator through its official API. Needs a DeepL API key; the free \
                            plan includes 500,000 characters a month."
            .into();
        info.homepage = "https://www.deepl.com/pro-api".into();
        info.languages = langs_from_codes(LANGS);
        info.max_input_chars = Some(30_000);
        info.needs_credentials = true;
        let options = vec![
            OptionSpec::secret("api_key", "API key")
                .required()
                .help("From your DeepL account page. Free-plan keys end in \":fx\" and are routed to api-free.deepl.com."),
            OptionSpec::select(
                "formality",
                "Formality",
                "default",
                &[
                    ("default", "Default"),
                    ("more", "More formal"),
                    ("less", "Less formal"),
                    ("prefer_more", "More formal, if available"),
                    ("prefer_less", "Less formal, if available"),
                ],
            )
            .help("Only some target languages (German, French, Spanish, Russian, Japanese, …) distinguish formality."),
            OptionSpec::select("english_variant", "English variant", "EN-US", &[("EN-US", "American"), ("EN-GB", "British")]),
            OptionSpec::select(
                "portuguese_variant",
                "Portuguese variant",
                "PT-BR",
                &[("PT-BR", "Brazilian"), ("PT-PT", "European")],
            ),
        ];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }
}

impl Provider for DeepL {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let key = host.require("api_key", self.name())?;
        let target = target_code(host, req.to);
        let mut body = json!({ "text": [req.text], "target_lang": target });
        if !is_auto(req.from) {
            body["source_lang"] = Value::String(source_code(req.from));
        }
        let formality = host.option_str("formality").trim();
        let formality_ok = match formality {
            "" | "default" => false,
            "prefer_more" | "prefer_less" => true,
            _ => FORMALITY_TARGETS.contains(&target.as_str()),
        };
        if formality_ok {
            body["formality"] = Value::String(formality.to_owned());
        }

        let resp = host.http.send(
            HttpRequest::post(format!("{}/v2/translate", host_for_key(key)))
                .json(&body)
                .header("Authorization", format!("DeepL-Auth-Key {key}"))
                .provider(self.name()),
        )?;
        let quirk = match resp.status {
            403 => Some("the API key was rejected; check it in DeepL's options (free keys end in \":fx\")"),
            456 => Some("the character quota for this billing period is used up"),
            _ => None,
        };
        if let Some(message) = quirk {
            return Err(Error::HttpStatus { provider: self.name().to_owned(), status: resp.status, body: message.into() });
        }
        let resp = check_status(resp, self.name())?;
        let v = parse_json(&resp, self.name())?;
        let item = &v["translations"][0];
        let text = require_str(&item["text"], "translations[0].text", self.name())?;
        Ok(Translation {
            text: text.to_owned(),
            detected_from: item["detected_source_language"].as_str().and_then(detected),
            ..Default::default()
        })
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        let from = if is_auto(pair.from) { "auto".to_owned() } else { source_code(pair.from).to_ascii_lowercase() };
        Some(format!(
            "https://www.deepl.com/translator#{from}/{}/{}",
            source_code(pair.to).to_ascii_lowercase(),
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

    const OK: &str = r#"{"translations":[{"detected_source_language":"EN","text":"Hallo, Welt!"}]}"#;

    fn req(from: LangId, to: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello, world!", LangPair::new(from, to))
    }

    fn de() -> LangId {
        LangId(23)
    }

    #[test]
    fn free_keys_go_to_the_free_host_with_the_auth_header() {
        let http = MockHttp::new().on(Method::Post, FREE_HOST, 200, OK);
        let o = opts(&[("api_key", json!("abc:fx"))]);
        let out = with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::AUTO, de()))).unwrap();
        assert_eq!(out.text, "Hallo, Welt!");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        let r = &http.requests()[0];
        assert_eq!(r.url, "https://api-free.deepl.com/v2/translate");
        assert_eq!(header(r, "Authorization"), Some("DeepL-Auth-Key abc:fx"));
        assert_eq!(body_json(r), json!({ "text": ["Hello, world!"], "target_lang": "DE" }));
    }

    #[test]
    fn pro_keys_go_to_the_pro_host_and_send_the_source() {
        let http = MockHttp::new().on(Method::Post, PRO_HOST, 200, OK);
        let o = opts(&[("api_key", json!("abc")), ("formality", json!("more"))]);
        with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::ENGLISH, de()))).unwrap();
        let r = &http.requests()[0];
        assert_eq!(r.url, "https://api.deepl.com/v2/translate");
        let b = body_json(r);
        assert_eq!(b["source_lang"], "EN");
        assert_eq!(b["formality"], "more");
    }

    #[test]
    fn target_variants_follow_the_options() {
        let http = MockHttp::new().on(Method::Post, PRO_HOST, 200, OK);
        let o = opts(&[("api_key", json!("k")), ("english_variant", json!("EN-GB")), ("formality", json!("less"))]);
        with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(de(), LangId::ENGLISH))).unwrap();
        let b = body_json(&http.requests()[0]);
        assert_eq!(b["target_lang"], "EN-GB");
        // English has no formality: the setting is left out rather than
        // making DeepL reject the request.
        assert!(b.get("formality").is_none());

        let http = MockHttp::new().on(Method::Post, PRO_HOST, 200, OK);
        let o = opts(&[("api_key", json!("k"))]);
        with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::ENGLISH, LangId(44)))).unwrap();
        with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId(12), LangId(11)))).unwrap();
        let reqs = http.requests();
        assert_eq!(body_json(&reqs[0])["target_lang"], "PT-BR");
        assert_eq!(body_json(&reqs[1])["target_lang"], "ZH-HANS");
        assert_eq!(body_json(&reqs[1])["source_lang"], "ZH");
    }

    #[test]
    fn missing_key_is_a_credential_error() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| DeepL::new().translate(h, &req(LangId::AUTO, de())));
        assert!(matches!(r, Err(Error::MissingCredential { ref what, .. }) if what == "api_key"), "{r:?}");
        assert!(http.requests().is_empty());
    }

    #[test]
    fn quota_and_bad_key_statuses_get_clear_messages() {
        let o = opts(&[("api_key", json!("k:fx"))]);
        let http = MockHttp::new().on(Method::Post, FREE_HOST, 456, r#"{"message":"Quota Exceeded"}"#);
        match with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::AUTO, de()))) {
            Err(Error::HttpStatus { status: 456, body, .. }) => assert!(body.contains("quota"), "{body}"),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Post, FREE_HOST, 403, r#"{"message":"Wrong endpoint"}"#);
        match with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::AUTO, de()))) {
            Err(Error::HttpStatus { status: 403, body, .. }) => assert!(body.contains("key"), "{body}"),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Post, FREE_HOST, 200, r#"{"translations":[]}"#);
        let r = with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::AUTO, de())));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }

    #[test]
    fn unsupported_languages_are_rejected() {
        let http = MockHttp::new();
        let o = opts(&[("api_key", json!("k"))]);
        let hindi = LangId::from_code("hi").unwrap();
        let r = with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(LangId::ENGLISH, hindi)));
        assert!(matches!(r, Err(Error::TargetLanguageUnsupported { .. })));
        let r = with_test_host(&http, &o, |h| DeepL::new().translate(h, &req(hindi, LangId::ENGLISH)));
        assert!(matches!(r, Err(Error::SourceLanguageUnsupported { .. })));
    }
}
