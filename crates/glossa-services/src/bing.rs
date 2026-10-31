//! Microsoft Translator (the engine behind Bing Translator and Edge).
//!
//! Two modes. Without a key it talks to Bing Translator the way the web page
//! does: load `bing.com/translator` once to pick up the page id (`IG`), the
//! anti-abuse key and token, and the session cookies, then post to
//! `/ttranslatev3`. That is also what the classic QTranslate host did for its
//! Microsoft script. (Edge's anonymous token endpoint, which some translators
//! relied on, stopped answering in 2026.) With an Azure subscription key it
//! uses the documented Translator v3 API instead.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use glossa_core::{
    Capabilities, Detection, Error, LangId, LangPair, ProviderInfo, Result, TranslateRequest, Translation,
};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::{json, Value};

use crate::common::{check_pair, detected, is_auto, json_body, require_str};

const PAGE_URL: &str = "https://www.bing.com/translator";
const WEB_API: &str = "https://www.bing.com/ttranslatev3";
const KEYED_API: &str = "https://api.cognitive.microsofttranslator.com";
/// The page's token is good for an hour; refreshing well before that leaves
/// room for clock skew and a long-lived session.
const SESSION_TTL: Duration = Duration::from_secs(20 * 60);

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

/// What the translator page hands out to a browser.
#[derive(Debug, Clone)]
struct Session {
    ig: String,
    key: String,
    token: String,
    cookie: String,
    fetched: Instant,
}

/// Microsoft Translator, keyless (Bing web session) or with an Azure key.
pub struct Bing {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
    session: Mutex<Option<Session>>,
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
        info.description = "Microsoft's translator, as used by Bing. Works without a key; an Azure Translator key \
                            and region switch it to the official, metered API."
            .into();
        info.homepage = "https://www.bing.com/translator".into();
        info.languages =
            std::iter::once(LangId::AUTO).chain(LangId::all().filter(|l| !MISSING.contains(&l.bcp47()))).collect();
        // The web endpoint refuses more than about a thousand characters.
        info.max_input_chars = Some(1000);
        let options = vec![
            OptionSpec::secret("api_key", "Azure Translator key")
                .help("Optional. Leave empty to use Bing Translator's free web endpoint (up to 1000 characters)."),
            OptionSpec::text("region", "Azure region", "")
                .placeholder("westeurope")
                .help("The region of your Translator resource; leave empty for a global resource."),
        ];
        Self { info, options, session: Mutex::new(None) }
    }

    fn name(&self) -> &str {
        &self.info.name
    }

    /// The cached web session, or a fresh one.
    fn session(&self, host: &Host, force: bool) -> Result<Session> {
        // Held across the fetch on purpose: concurrent jobs share one page
        // load instead of racing for several.
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if !force {
            if let Some(s) = guard.as_ref() {
                if s.fetched.elapsed() < SESSION_TTL {
                    return Ok(s.clone());
                }
            }
        }
        let resp = host.http.send(HttpRequest::get(PAGE_URL).provider(self.name()))?;
        if !resp.is_success() {
            return Err(Error::HttpStatus { provider: self.name().into(), status: resp.status, body: String::new() });
        }
        let body = resp.text();
        let ig = between(&body, "IG:\"", "\"")
            .ok_or_else(|| Error::bad_response(self.name(), "the translator page has no IG id"))?
            .to_owned();
        let helper = between(&body, "params_AbusePreventionHelper", "]")
            .ok_or_else(|| Error::bad_response(self.name(), "the translator page has no anti-abuse parameters"))?;
        let mut parts = helper.trim_start_matches([' ', '=', '[']).splitn(3, ',');
        let key = parts.next().unwrap_or("").trim().to_owned();
        let token = parts.next().unwrap_or("").trim().trim_matches('"').to_owned();
        if key.is_empty() || token.is_empty() {
            return Err(Error::bad_response(self.name(), "the anti-abuse parameters are incomplete"));
        }
        let cookie = resp
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .filter_map(|(_, v)| v.split(';').next())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        let s = Session { ig, key, token, cookie, fetched: Instant::now() };
        *guard = Some(s.clone());
        Ok(s)
    }

    /// One `/ttranslatev3` call. `to` is the target; detection-only calls
    /// still need one.
    fn web_call(&self, host: &Host, text: &str, from: &str, to: &str) -> Result<Value> {
        let send = |s: &Session| {
            let url = format!("{WEB_API}?isVertical=1&&IG={}&IID=translator.5027", s.ig);
            let mut req = HttpRequest::post(url)
                .form(&[
                    ("fromLang", from),
                    ("to", to),
                    ("text", text),
                    ("token", &s.token),
                    ("key", &s.key),
                    ("tryFetchingGenderDebiasedTranslations", "true"),
                ])
                .header("Origin", "https://www.bing.com")
                .header("Referer", PAGE_URL)
                .provider(self.name());
            if !s.cookie.is_empty() {
                req = req.header("Cookie", &s.cookie);
            }
            host.http.send(req)
        };
        let s = self.session(host, false)?;
        host.check_cancelled()?;
        let mut resp = send(&s)?;
        // An expired session answers with an error status or a bare status
        // object instead of the result array: refresh once.
        let stale = !resp.is_success() || !resp.body.trim_ascii_start().starts_with(b"[");
        if stale {
            let s = self.session(host, true)?;
            host.check_cancelled()?;
            resp = send(&s)?;
        }
        let v = json_body(resp, self.name())?;
        if !v.is_array() {
            let status = v.get("statusCode").and_then(|c| c.as_i64()).unwrap_or(0);
            return Err(if status == 429 {
                Error::RateLimited { provider: self.name().into(), retry_after_secs: None }
            } else {
                Error::bad_response(self.name(), format!("unexpected reply (status {status})"))
            });
        }
        Ok(v)
    }

    /// A v3 call with a subscription key.
    fn keyed_call(&self, host: &Host, key: &str, path: &str, body: &Value) -> Result<Value> {
        let mut req = HttpRequest::post(format!("{KEYED_API}{path}"))
            .json(body)
            .header("Ocp-Apim-Subscription-Key", key)
            .provider(self.name());
        let region = host.option_str("region").trim();
        if !region.is_empty() {
            req = req.header("Ocp-Apim-Subscription-Region", region);
        }
        json_body(host.http.send(req)?, self.name())
    }
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let len = s[from..].find(end)?;
    Some(&s[from..from + len])
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
        let key = host.option_str("api_key").trim().to_owned();
        let v = if key.is_empty() {
            let from = if is_auto(req.from) { "auto-detect" } else { code(req.from) };
            self.web_call(host, &req.text, from, code(req.to))?
        } else {
            let mut path = format!("/translate?api-version=3.0&to={}", url_encode(code(req.to)));
            if !is_auto(req.from) {
                path.push_str("&from=");
                path.push_str(&url_encode(code(req.from)));
            }
            self.keyed_call(host, &key, &path, &json!([{ "Text": req.text }]))?
        };
        let item = &v[0];
        let translation = &item["translations"][0];
        let text = require_str(&translation["text"], "translations[0].text", self.name())?;
        Ok(Translation {
            text: text.to_owned(),
            detected_from: item["detectedLanguage"]["language"].as_str().and_then(from_code),
            romanisation: translation["transliteration"]["text"].as_str().map(str::to_owned).filter(|s| !s.is_empty()),
            ..Default::default()
        })
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let key = host.option_str("api_key").trim().to_owned();
        let (lang, score) = if key.is_empty() {
            let v = self.web_call(host, text, "auto-detect", "en")?;
            let d = &v[0]["detectedLanguage"];
            (d["language"].as_str().map(str::to_owned), d["score"].as_f64())
        } else {
            let v = self.keyed_call(host, &key, "/detect?api-version=3.0", &json!([{ "Text": text }]))?;
            (v[0]["language"].as_str().map(str::to_owned), v[0]["score"].as_f64())
        };
        let lang = lang.as_deref().and_then(from_code).ok_or(Error::LanguageNotDetected)?;
        Ok(Detection { lang, confidence: score.unwrap_or(1.0).clamp(0.0, 1.0) as f32 })
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
    use crate::common::testing::{header, opts, SeqHttp};
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    const PAGE: &str = r#"<html><script>_G={IG:"5A1B2C3D",x:1};var params_AbusePreventionHelper = [1712345678901,"tokEN-x_9",3600000];</script></html>"#;
    const TRANSLATED: &str = r#"[{"detectedLanguage":{"language":"en","score":1.0},"translations":[{"text":"Привет, мир","transliteration":{"text":"Privet, mir","script":"Latn"},"to":"ru"}]}]"#;

    fn req(from: LangId, to: LangId) -> TranslateRequest {
        TranslateRequest::new("Hello world", LangPair::new(from, to))
    }

    fn form(r: &glossa_net::HttpRequest) -> String {
        String::from_utf8(r.body.clone().unwrap_or_default()).unwrap()
    }

    #[test]
    fn keyless_mode_loads_the_page_once_and_posts_the_form() {
        let http = MockHttp::new().on(Method::Get, PAGE_URL, 200, PAGE).on(Method::Post, WEB_API, 200, TRANSLATED);
        let bing = Bing::new();
        let o = OptionValues::new();
        let out = with_test_host(&http, &o, |h| bing.translate(h, &req(LangId::AUTO, LangId::RUSSIAN))).unwrap();
        assert_eq!(out.text, "Привет, мир");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        assert_eq!(out.romanisation.as_deref(), Some("Privet, mir"));
        with_test_host(&http, &o, |h| bing.translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))).unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.iter().filter(|r| r.url == PAGE_URL).count(), 1, "the session is reused");
        let t = &reqs[1];
        assert_eq!(t.url, format!("{WEB_API}?isVertical=1&&IG=5A1B2C3D&IID=translator.5027"));
        let body = form(t);
        assert!(body.contains("fromLang=auto-detect"), "{body}");
        assert!(body.contains("to=ru"));
        assert!(body.contains("text=Hello%20world"));
        assert!(body.contains("token=tokEN-x_9"));
        assert!(body.contains("key=1712345678901"));
        assert_eq!(header(t, "Referer"), Some(PAGE_URL));
        assert!(form(&reqs[2]).contains("fromLang=en"));
    }

    #[test]
    fn a_stale_session_is_refreshed_once() {
        let http =
            SeqHttp::new().then(200, PAGE).then(200, r#"{"statusCode":205}"#).then(200, PAGE).then(200, TRANSLATED);
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        })
        .unwrap();
        assert_eq!(out.text, "Привет, мир");
        assert_eq!(http.requests().len(), 4);
    }

    #[test]
    fn a_page_without_the_session_values_is_a_clear_error() {
        let http = MockHttp::new().on(Method::Get, PAGE_URL, 200, "<html>captcha</html>");
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::ENGLISH, LangId::RUSSIAN))
        });
        match r {
            Err(Error::BadResponse { detail, .. }) => assert!(detail.contains("IG"), "{detail}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keyed_mode_uses_the_subscription_headers() {
        let http = MockHttp::new().on(Method::Post, &format!("{KEYED_API}/translate"), 200, TRANSLATED);
        let o = opts(&[("api_key", json!("k123")), ("region", json!("westeurope"))]);
        with_test_host(&http, &o, |h| Bing::new().translate(h, &req(LangId::AUTO, LangId(11)))).unwrap();
        let reqs = http.requests();
        assert_eq!(reqs.len(), 1, "no page load in keyed mode");
        assert_eq!(reqs[0].url, format!("{KEYED_API}/translate?api-version=3.0&to=zh-Hans"));
        assert_eq!(header(&reqs[0], "Ocp-Apim-Subscription-Key"), Some("k123"));
        assert_eq!(header(&reqs[0], "Ocp-Apim-Subscription-Region"), Some("westeurope"));
    }

    #[test]
    fn detection_uses_the_web_endpoint_without_a_key() {
        let http = MockHttp::new().on(Method::Get, PAGE_URL, 200, PAGE).on(
            Method::Post,
            WEB_API,
            200,
            r#"[{"detectedLanguage":{"language":"de","score":0.92},"translations":[{"text":"Hello world","to":"en"}]}]"#,
        );
        let d = with_test_host(&http, &OptionValues::new(), |h| Bing::new().detect(h, "Hallo Welt")).unwrap();
        assert_eq!(d.lang, LangId(23));
        assert!((d.confidence - 0.92).abs() < 1e-6);
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
    fn web_link_points_at_bing() {
        let l = Bing::new().web_link("hi there", LangPair::new(LangId::AUTO, LangId::RUSSIAN)).unwrap();
        assert_eq!(l, "https://www.bing.com/translator?from=auto-detect&to=ru&text=hi%20there");
    }

    #[test]
    #[ignore]
    fn live_keyless_translation() {
        let http = glossa_net::HttpClient::plain();
        let out = with_test_host(&http, &OptionValues::new(), |h| {
            Bing::new().translate(h, &req(LangId::AUTO, LangId::RUSSIAN))
        })
        .unwrap();
        eprintln!("{out:?}");
        assert!(!out.text.is_empty());
    }
}
