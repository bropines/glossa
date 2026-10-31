//! Microsoft Translator (the engine behind Bing Translator and Edge).
//!
//! Two modes. Without a key, it borrows the token Edge's built-in page
//! translator uses: `edge.microsoft.com/translate/auth` hands out a short-lived
//! JWT to anyone, which the `api-edge` host accepts as a bearer token. With an
//! Azure subscription key it talks to the documented Translator v3 endpoint
//! instead. Both speak the same v3 request and response shapes.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use glossa_core::{
    Capabilities, Detection, Error, LangId, LangPair, ProviderInfo, Result, TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest, HttpResponse};
use serde_json::{json, Value};

use crate::common::{check_pair, check_status, detected, is_auto, json_body, require_str};

const AUTH_URL: &str = "https://edge.microsoft.com/translate/auth";
const EDGE_API: &str = "https://api-edge.cognitive.microsofttranslator.com";
const KEYED_API: &str = "https://api.cognitive.microsofttranslator.com";
/// The Edge token lives ten minutes; refreshing at eight leaves room for a
/// slow request and clock skew.
const TOKEN_TTL: Duration = Duration::from_secs(8 * 60);

/// Table languages Microsoft does not offer.
const MISSING: &[&str] = &["be", "eo", "la", "yi", "tg"];

/// Microsoft's codes are BCP 47 with explicit scripts where a language has
/// several, plus `mww` (Hmong Daw) for Hmong.
fn code(lang: LangId) -> &'static str {
    match lang.bcp47() {
        "sr" => "sr-Cyrl",
        "mn" => "mn-Cyrl",
        "hmn" => "mww",
        other => other,
    }
}

fn from_code(code: &str) -> Option<LangId> {
    if code.eq_ignore_ascii_case("mww") {
        return LangId::from_code("hmn");
    }
    detected(code)
}

/// Microsoft Translator, keyless (Edge token) or with an Azure key.
pub struct Bing {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
    /// The Edge token and when it was fetched.
    token: Mutex<Option<(String, Instant)>>,
}

impl Default for Bing {
    fn default() -> Self {
        Self::new()
    }
}

impl Bing {
    pub const ID: &'static str = "bing";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(
            Self::ID,
            "Microsoft Translator",
            Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE,
        );
        info.description = "Microsoft's translator, as used by Bing and Edge. Works without a key; an Azure \
                            Translator key and region switch it to the official, metered API."
            .into();
        info.homepage = "https://www.bing.com/translator".into();
        info.languages = std::iter::once(LangId::AUTO)
            .chain(LangId::all().filter(|l| !MISSING.contains(&l.bcp47())))
            .collect();
        info.max_input_chars = Some(50_000);
        let options = vec![
            OptionSpec::secret("api_key", "Azure Translator key")
                .help("Optional. Leave empty to use the free endpoint Edge uses."),
            OptionSpec::text("region", "Azure region", "")
                .placeholder("westeurope")
                .help("The region of your Translator resource; leave empty for a global resource."),
        ];
        Self { info, options, token: Mutex::new(None) }
    }

    fn name(&self) -> &str {
        &self.info.name
    }

    /// The cached Edge token, or a fresh one.
    fn token(&self, host: &Host, force: bool) -> Result<String> {
        // Held across the fetch on purpose: concurrent jobs then share one
        // token request instead of racing for several.
        let mut guard = self.token.lock().unwrap_or_else(|e| e.into_inner());
        if !force {
            if let Some((tok, at)) = guard.as_ref() {
                if at.elapsed() < TOKEN_TTL {
                    return Ok(tok.clone());
                }
            }
        }
        let resp = check_status(host.http.send(HttpRequest::get(AUTH_URL).provider(self.name()))?, self.name())?;
        let tok = resp.text().trim().to_owned();
        // A JWT is three dot-separated parts; anything else is an error page.
        if tok.split('.').count() != 3 || tok.contains(char::is_whitespace) {
            return Err(Error::bad_response(self.name(), "authentication endpoint did not return a token"));
        }
        *guard = Some((tok.clone(), Instant::now()));
        Ok(tok)
    }

    /// POST a v3 call (`/translate?…`, `/detect?…`) in whichever mode the
    /// options select.
    fn call(&self, host: &Host, path: &str, body: &Value) -> Result<Value> {
        let key = host.option_str("api_key").trim();
        if !key.is_empty() {
            let mut req = HttpRequest::post(format!("{KEYED_API}{path}"))
                .json(body)
                .header("Ocp-Apim-Subscription-Key", key)
                .provider(self.name());
            let region = host.option_str("region").trim();
            if !region.is_empty() {
                req = req.header("Ocp-Apim-Subscription-Region", region);
            }
            return json_body(host.http.send(req)?, self.name());
        }

        let send = |tok: &str| -> Result<HttpResponse> {
            host.http.send(
                HttpRequest::post(format!("{EDGE_API}{path}"))
                    .json(body)
                    .header("Authorization", format!("Bearer {tok}"))
                    .provider(self.name()),
            )
        };
        let tok = self.token(host, false)?;
        host.check_cancelled()?;
        let mut resp = send(&tok)?;
        if resp.status == 401 {
            // The cached token expired early or was revoked: refresh once.
            let tok = self.token(host, true)?;
            host.check_cancelled()?;
            resp = send(&tok)?;
        }
        json_body(resp, self.name())
    }
}

impl Provider for Bing {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let mut path = format!("/translate?api-version=3.0&to={}", url_encode(code(req.to)));
        if !is_auto(req.from) {
            path.push_str("&from=");
            path.push_str(&url_encode(code(req.from)));
        }
        let v = self.call(host, &path, &json!([{ "Text": req.text }]))?;
        let item = &v[0];
        let text = require_str(&item["translations"][0]["text"], "translations[0].text", self.name())?;
        Ok(Translation {
            text: text.to_owned(),
            detected_from: item["detectedLanguage"]["language"].as_str().and_then(from_code),
            ..Default::default()
        })
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let v = self.call(host, "/detect?api-version=3.0", &json!([{ "Text": text }]))?;
        let item = &v[0];
        if !item.is_object() {
            return Err(Error::bad_response(self.name(), "expected an array of detections"));
        }
        let lang = item["language"].as_str().and_then(from_code).ok_or(Error::LanguageNotDetected)?;
        let confidence = item["score"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32;
        Ok(Detection { lang, confidence })
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        let from = if is_auto(pair.from) { "auto-detect" } else { code(pair.from) };
        Some(format!(
            "https://www.bing.com/translator?from={}&to={}&text={}",
            url_encode(from),
            url_encode(code(pair.to)),
            url_encode(text)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_json, header, opts, SeqHttp};
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJyZWdpb24iOiJnbG9iYWwifQ.c2lnbmF0dXJl";
    const TRANSLATED: &str = r#"[{"detectedLanguage":{"language":"en","score":1.0},"translations":[{"text":"Привет, мир","to":"ru"}]}]"#;

    fn req(from: LangId, to: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello world", LangPair::new(from, to))
    }

    #[test]
    fn keyless_mode_fetches_a_token_and_reuses_it() {
        let http = MockHttp::new()
            .on(Method::Get, AUTH_URL, 200, JWT)
            .on(Method::Post, &format!("{EDGE_API}/translate"), 200, TRANSLATED);
        let bing = Bing::new();
        let opts = OptionValues::new();
        let out = with_test_host(&http, &opts, |h| bing.translate(h, &req(LangId::AUTO, LangId::RUSSIAN))).unwrap();
        assert_eq!(out.text, "Привет, мир");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        with_test_host(&http, &opts, |h| bing.translate(h, &req(LangId::AUTO, LangId::RUSSIAN))).unwrap();

        let reqs = http.requests();
        // One token fetch for two translations.
        assert_eq!(reqs.iter().filter(|r| r.url == AUTH_URL).count(), 1);
        let t = &reqs[1];
        assert_eq!(t.url, format!("{EDGE_API}/translate?api-version=3.0&to=ru"));
        assert_eq!(header(t, "authorization"), Some(format!("Bearer {JWT}").as_str()));
        assert_eq!(body_json(t), json!([{ "Text": "Hello world" }]));
    }

    #[test]
    fn a_401_refreshes_the_token_once_and_retries() {
        let http = SeqHttp::new()
            .then(200, JWT)
            .then(401, r#"{"error":{"code":401000,"message":"expired"}}"#)
            .then(200, "aaa.bbb.ccc")
            .then(200, TRANSLATED);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        })
        .unwrap();
        assert_eq!(out.text, "Привет, мир");
        let reqs = http.requests();
        assert_eq!(reqs.len(), 4);
        assert!(reqs[1].url.ends_with("&to=ru&from=en"), "{}", reqs[1].url);
        assert_eq!(header(&reqs[3], "authorization"), Some("Bearer aaa.bbb.ccc"));
    }

    #[test]
    fn a_second_401_is_reported() {
        let http = SeqHttp::new()
            .then(200, JWT)
            .then(401, r#"{"error":{"code":401000,"message":"expired"}}"#)
            .then(200, JWT)
            .then(401, r#"{"error":{"code":401000,"message":"still expired"}}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        });
        match r {
            Err(Error::HttpStatus { status: 401, body, .. }) => assert_eq!(body, "still expired"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keyed_mode_uses_the_subscription_headers() {
        let http = MockHttp::new().on(Method::Post, &format!("{KEYED_API}/translate"), 200, TRANSLATED);
        let o = opts(&[("api_key", json!("k123")), ("region", json!("westeurope"))]);
        with_test_host(&http, &o, |h| Bing::new().translate(h, &req(LangId::AUTO, LangId(11)))).unwrap();
        let reqs = http.requests();
        assert_eq!(reqs.len(), 1, "no token fetch in keyed mode");
        assert_eq!(reqs[0].url, format!("{KEYED_API}/translate?api-version=3.0&to=zh-Hans"));
        assert_eq!(header(&reqs[0], "Ocp-Apim-Subscription-Key"), Some("k123"));
        assert_eq!(header(&reqs[0], "Ocp-Apim-Subscription-Region"), Some("westeurope"));
        assert_eq!(header(&reqs[0], "authorization"), None);
    }

    #[test]
    fn codes_follow_microsoft_spelling() {
        assert_eq!(code(LangId(26)), "he");
        assert_eq!(code(LangId(41)), "nb");
        assert_eq!(code(LangId(20)), "fil");
        assert_eq!(code(LangId(12)), "zh-Hant");
        assert_eq!(code(LangId(47)), "sr-Cyrl");
        assert_eq!(from_code("mww"), LangId::from_code("hmn"));
        assert_eq!(from_code("sr-Latn"), Some(LangId(47)));
    }

    #[test]
    fn unsupported_target_is_rejected() {
        let http = MockHttp::new();
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::from_code("eo").unwrap()))
        });
        assert!(matches!(r, Err(Error::TargetLanguageUnsupported { .. })));
    }

    #[test]
    fn detect_parses_language_and_score() {
        let http = MockHttp::new()
            .on(Method::Get, AUTH_URL, 200, JWT)
            .on(
                Method::Post,
                &format!("{EDGE_API}/detect"),
                200,
                r#"[{"language":"de","score":0.92,"isTranslationSupported":true,"isTransliterationSupported":false}]"#,
            );
        let d = with_test_host(&http, &OptionValues::new(), |h| Bing::new().detect(h, "Hallo Welt")).unwrap();
        assert_eq!(d.lang, LangId(23));
        assert!((d.confidence - 0.92).abs() < 1e-6);
        assert_eq!(http.requests()[1].url, format!("{EDGE_API}/detect?api-version=3.0"));
    }

    #[test]
    fn bad_token_and_bad_json_are_errors() {
        let http = MockHttp::new().on(Method::Get, AUTH_URL, 200, "<html>blocked</html>");
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        });
        assert!(matches!(r, Err(Error::BadResponse { .. })), "{r:?}");

        let http = MockHttp::new()
            .on(Method::Get, AUTH_URL, 200, JWT)
            .on(Method::Post, EDGE_API, 200, r#"[{"translations":[]}]"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        });
        assert!(matches!(r, Err(Error::BadResponse { .. })), "{r:?}");
    }

    #[test]
    fn web_link_points_at_bing() {
        let l = Bing::new().web_link("hi there", LangPair::new(LangId::AUTO, LangId::RUSSIAN)).unwrap();
        assert_eq!(l, "https://www.bing.com/translator?from=auto-detect&to=ru&text=hi%20there");
    }
}
