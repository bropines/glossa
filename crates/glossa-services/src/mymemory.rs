//! MyMemory, the translation-memory service from Translated. Keyless, with a
//! daily quota per IP that grows when an e-mail address is supplied.
//!
//! It always answers HTTP 200 and puts the real status in `responseStatus`
//! (sometimes a number, sometimes a string). When the quota runs out the
//! "translation" is a warning sentence, which must not reach the user as if it
//! were one.

use glossa_core::{Capabilities, Error, LangId, LangPair, ProviderInfo, Result, TranslateRequest, Translation};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::Value;

use crate::common::{check_pair, check_status, detected, parse_json, require_str};

const API: &str = "https://api.mymemory.translated.net/get";

/// MyMemory accepts ISO 639-1 codes and RFC 3066 tags; Chinese needs its
/// region tag to pick a script.
fn code(lang: LangId) -> &'static str {
    match lang.bcp47() {
        "zh-Hans" => "zh-CN",
        "zh-Hant" => "zh-TW",
        "fil" => "tl",
        other => other,
    }
}

fn status_of(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// MyMemory.
pub struct MyMemory {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for MyMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl MyMemory {
    pub const ID: &'static str = "mymemory";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(Self::ID, "MyMemory", Capabilities::TRANSLATE | Capabilities::ALTERNATIVES);
        info.description = "The world's largest translation memory, mixing human translations with machine \
                            output. No key; supplying an e-mail raises the free daily quota."
            .into();
        info.homepage = "https://mymemory.translated.net".into();
        info.ready_to_use = true;
        // Every table language, but no `auto`: MyMemory cannot detect, so the
        // engine has to work the source out first.
        info.languages = LangId::all().collect();
        info.max_input_chars = Some(500);
        let options = vec![OptionSpec::text("email", "E-mail", "")
            .placeholder("you@example.com")
            .help("Optional. A valid address raises the free quota from 5,000 to 50,000 characters a day.")];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }
}

impl Provider for MyMemory {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let mut url = format!(
            "{API}?q={}&langpair={}",
            url_encode(&req.text),
            url_encode(&format!("{}|{}", code(req.from), code(req.to)))
        );
        let email = host.option_str("email").trim();
        if !email.is_empty() {
            url.push_str("&de=");
            url.push_str(&url_encode(email));
        }
        let resp = check_status(host.http.send(HttpRequest::get(url).provider(self.name()))?, self.name())?;
        let v = parse_json(&resp, self.name())?;

        let text = v["responseData"]["translatedText"].as_str().unwrap_or("");
        let quota_hit = text.contains("MYMEMORY WARNING") || v["quotaFinished"].as_bool() == Some(true);
        let status = status_of(&v["responseStatus"]).unwrap_or(200);
        if quota_hit || status == 429 {
            return Err(Error::RateLimited { provider: self.name().to_owned(), retry_after_secs: None });
        }
        if status != 200 {
            let detail = v["responseDetails"].as_str().filter(|s| !s.is_empty()).unwrap_or(text).to_owned();
            return Err(Error::HttpStatus {
                provider: self.name().to_owned(),
                status: u16::try_from(status).unwrap_or(500),
                body: detail,
            });
        }
        let text = require_str(&v["responseData"]["translatedText"], "responseData.translatedText", self.name())?;

        // The memory's other matches, best first, as alternatives.
        let mut alternatives: Vec<String> = Vec::new();
        for m in v["matches"].as_array().into_iter().flatten() {
            if let Some(t) = m["translation"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
                if t != text.trim() && !alternatives.iter().any(|a| a == t) {
                    alternatives.push(t.to_owned());
                }
            }
        }
        Ok(Translation {
            text: text.to_owned(),
            detected_from: v["responseData"]["detectedLanguage"].as_str().and_then(detected),
            alternatives,
            ..Default::default()
        })
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        if !pair.from.is_language() {
            return None;
        }
        Some(format!(
            "https://mymemory.translated.net/en/{}/{}/{}",
            url_encode(pair.from.english_name()),
            url_encode(pair.to.english_name()),
            url_encode(text)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::opts;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;
    use serde_json::json;

    const OK: &str = r#"{"responseData":{"translatedText":"Привет мир","match":0.99},"quotaFinished":false,"mtLangSupported":null,"responseDetails":"","responseStatus":200,"responderId":null,"exception_code":null,"matches":[{"id":"1","segment":"Hello world","translation":"Привет мир","source":"en-GB","target":"ru-RU","quality":"74","match":0.99},{"id":"2","segment":"Hello world","translation":"Здравствуй, мир","source":"en-GB","target":"ru-RU","quality":"70","match":0.98}]}"#;

    fn req(from: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello world", LangPair::new(from, LangId::RUSSIAN))
    }

    #[test]
    fn get_with_langpair_and_email() {
        let http = MockHttp::new().on(Method::Get, API, 200, OK);
        let o = opts(&[("email", json!("me@example.com"))]);
        let out = with_test_host(&http, &o, |h| MyMemory::new().translate(h, &req(LangId::ENGLISH))).unwrap();
        assert_eq!(out.text, "Привет мир");
        assert_eq!(out.alternatives, vec!["Здравствуй, мир"]);
        assert_eq!(http.requests()[0].url, format!("{API}?q=Hello%20world&langpair=en%7Cru&de=me%40example.com"));
    }

    #[test]
    fn auto_source_is_refused_so_the_engine_detects_first() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| MyMemory::new().translate(h, &req(LangId::AUTO)));
        assert!(matches!(r, Err(Error::SourceLanguageUnsupported { lang: LangId::AUTO, .. })), "{r:?}");
        assert!(http.requests().is_empty());
    }

    #[test]
    fn quota_warning_is_rate_limiting() {
        let body = r#"{"responseData":{"translatedText":"MYMEMORY WARNING: YOU USED ALL AVAILABLE FREE TRANSLATIONS FOR TODAY. NEXT AVAILABLE IN  10 HOURS 20 MINUTES 05 SECONDS","match":0},"quotaFinished":true,"responseDetails":"","responseStatus":429}"#;
        let http = MockHttp::new().on(Method::Get, API, 200, body);
        let r = with_test_host(&http, &OptionValues::new(), |h| MyMemory::new().translate(h, &req(LangId::ENGLISH)));
        assert!(matches!(r, Err(Error::RateLimited { .. })), "{r:?}");
    }

    #[test]
    fn string_status_in_the_body_is_an_error() {
        let body = r#"{"responseData":{"translatedText":"'XX' IS AN INVALID TARGET LANGUAGE . EXAMPLE: LANGPAIR=EN|IT USING 2 LETTER ISO OR RFC3066 LIKE ZH-CN. ALMOST ALL LANGUAGES SUPPORTED BUT SOME MAY HAVE NO CONTENT"},"responseDetails":"'XX' IS AN INVALID TARGET LANGUAGE","responseStatus":"403"}"#;
        let http = MockHttp::new().on(Method::Get, API, 200, body);
        match with_test_host(&http, &OptionValues::new(), |h| MyMemory::new().translate(h, &req(LangId::ENGLISH))) {
            Err(Error::HttpStatus { status: 403, body, .. }) => assert!(body.contains("INVALID TARGET")),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Get, API, 200, r#"{"responseStatus":200}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| MyMemory::new().translate(h, &req(LangId::ENGLISH)));
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }

    #[test]
    fn chinese_uses_region_tags() {
        assert_eq!(code(LangId(11)), "zh-CN");
        assert_eq!(code(LangId(12)), "zh-TW");
        assert_eq!(code(LangId(26)), "he");
    }
}
