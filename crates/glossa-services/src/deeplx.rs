//! DeepLX: a self-hosted bridge that exposes DeepL's web translator as a
//! small JSON API (`github.com/OwO-Network/DeepLX` and compatible servers).
//!
//! The server answers HTTP 200 with its own `code` field, so both the HTTP
//! status and `code` are checked.

use glossa_core::{Capabilities, Error, LangId, ProviderInfo, Result, TranslateRequest, Translation};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::HttpRequest;
use serde_json::{json, Value};

use crate::common::{api_message, base_url, check_pair, check_status, detected, is_auto, langs_from_codes, parse_json, require_str};
use crate::deepl::source_code;

const DEFAULT_URL: &str = "http://127.0.0.1:1188/translate";

/// The same languages as DeepL proper.
const LANGS: &[&str] = &[
    "auto", "ar", "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "he", "hu", "id", "it", "ja", "ko",
    "lt", "lv", "nb", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "th", "tr", "uk", "vi", "zh-Hans", "zh-Hant",
];

/// DeepLX takes the bare language for targets (it picks DeepL's default
/// variant), except Traditional Chinese, which only exists as a variant.
fn target_code(lang: LangId) -> String {
    match lang.bcp47() {
        "zh-Hant" => "ZH-HANT".to_owned(),
        _ => source_code(lang),
    }
}

/// DeepLX.
pub struct DeepLx {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for DeepLx {
    fn default() -> Self {
        Self::new()
    }
}

impl DeepLx {
    pub const ID: &'static str = "deeplx";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(Self::ID, "DeepLX", Capabilities::TRANSLATE | Capabilities::ALTERNATIVES);
        info.description = "DeepL quality without an API key, through a DeepLX server you run yourself \
                            (or one you trust)."
            .into();
        info.homepage = "https://github.com/OwO-Network/DeepLX".into();
        info.languages = langs_from_codes(LANGS);
        info.max_input_chars = Some(5000);
        let options = vec![
            OptionSpec::url("url", "Server URL", DEFAULT_URL).help("The full address of the /translate endpoint."),
            OptionSpec::secret("token", "Access token").help("Only if the server was started with a token."),
        ];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }
}

impl Provider for DeepLx {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let url = base_url(host.option_str("url"), DEFAULT_URL);
        let mut body = json!({ "text": req.text, "target_lang": target_code(req.to) });
        // Left out for auto: DeepLX detects when the field is empty.
        if !is_auto(req.from) {
            body["source_lang"] = Value::String(source_code(req.from));
        }
        let token = host.option_str("token").trim();
        let mut http_req = HttpRequest::post(url).json(&body).provider(self.name());
        if !token.is_empty() {
            http_req = http_req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = host.http.send(http_req)?;
        if resp.status == 401 && token.is_empty() {
            return Err(Error::MissingCredential { provider: self.name().to_owned(), what: "token".into() });
        }
        let resp = check_status(resp, self.name())?;
        let v = parse_json(&resp, self.name())?;
        if let Some(code) = v["code"].as_u64() {
            if code != 200 {
                let body = api_message(&resp.text()).unwrap_or_else(|| resp.text());
                return Err(match code {
                    429 => Error::RateLimited { provider: self.name().to_owned(), retry_after_secs: None },
                    status => Error::HttpStatus {
                        provider: self.name().to_owned(),
                        status: u16::try_from(status).unwrap_or(500),
                        body,
                    },
                });
            }
        }
        let text = require_str(&v["data"], "data", self.name())?;
        let alternatives = v["alternatives"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).filter(|s| *s != text).map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(Translation {
            text: text.to_owned(),
            detected_from: v["source_lang"].as_str().and_then(detected),
            alternatives,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_json, header, opts};
    use glossa_core::LangPair;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    const OK: &str = r#"{"alternatives":["Hallo Welt!","Hallo, Welt"],"code":200,"data":"Hallo, Welt!","id":8356681003,"method":"Free","source_lang":"EN","target_lang":"DE"}"#;

    fn req(from: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello, world!", LangPair::new(from, LangId(23)))
    }

    #[test]
    fn posts_to_the_default_url_and_parses_alternatives() {
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, OK);
        let out = with_test_host(&http, &OptionValues::new(), |h| DeepLx::new().translate(h, &req(LangId::AUTO))).unwrap();
        assert_eq!(out.text, "Hallo, Welt!");
        assert_eq!(out.alternatives, vec!["Hallo Welt!", "Hallo, Welt"]);
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        let r = &http.requests()[0];
        assert_eq!(r.url, DEFAULT_URL);
        assert_eq!(body_json(r), json!({ "text": "Hello, world!", "target_lang": "DE" }));
        assert_eq!(header(r, "authorization"), None);
    }

    #[test]
    fn custom_url_token_and_source_are_sent() {
        let http = MockHttp::new().on(Method::Post, "https://dlx.example/translate", 200, OK);
        let o = opts(&[("url", json!("https://dlx.example/translate")), ("token", json!("s3cret"))]);
        with_test_host(&http, &o, |h| DeepLx::new().translate(h, &req(LangId::ENGLISH))).unwrap();
        let r = &http.requests()[0];
        assert_eq!(header(r, "authorization"), Some("Bearer s3cret"));
        assert_eq!(body_json(r)["source_lang"], "EN");
    }

    #[test]
    fn error_codes_in_the_body_are_errors() {
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, r#"{"code":503,"message":"blocked by DeepL"}"#);
        match with_test_host(&http, &OptionValues::new(), |h| DeepLx::new().translate(h, &req(LangId::AUTO))) {
            Err(Error::HttpStatus { status: 503, body, .. }) => assert_eq!(body, "blocked by DeepL"),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, r#"{"code":429,"message":"Too many requests"}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| DeepLx::new().translate(h, &req(LangId::AUTO)));
        assert!(matches!(r, Err(Error::RateLimited { .. })));
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 200, r#"{"code":200}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| DeepLx::new().translate(h, &req(LangId::AUTO)));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }

    #[test]
    fn unauthorised_without_a_token_asks_for_one() {
        let http = MockHttp::new().on(Method::Post, DEFAULT_URL, 401, r#"{"code":401,"message":"Invalid access token"}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| DeepLx::new().translate(h, &req(LangId::AUTO)));
        assert!(matches!(r, Err(Error::MissingCredential { ref what, .. }) if what == "token"), "{r:?}");
    }

    #[test]
    fn traditional_chinese_target_keeps_its_variant() {
        assert_eq!(target_code(LangId(12)), "ZH-HANT");
        assert_eq!(target_code(LangId(11)), "ZH");
        assert_eq!(target_code(LangId::ENGLISH), "EN");
    }
}
