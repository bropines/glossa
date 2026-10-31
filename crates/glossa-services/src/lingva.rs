//! Lingva Translate: an open-source, privacy-friendly front end that proxies
//! Google Translate. Any public or self-hosted instance works; they all share
//! the `/api/v1/{from}/{to}/{text}` REST route.

use glossa_core::translate::{PartOfSpeechEntry, Sense};
use glossa_core::{Capabilities, LangId, ProviderInfo, Result, TranslateRequest, Translation};
use glossa_modules::{Host, OptionSpec, Provider};
use glossa_net::{url_encode, HttpRequest};
use serde_json::Value;

use crate::common::{all_langs_with_auto, base_url, check_pair, detected, is_auto, json_body, require_str};

const DEFAULT_URL: &str = "https://lingva.ml";

/// Lingva mirrors Google's legacy codes, except for Chinese, which it spells
/// `zh` and `zh_HANT`.
fn code(lang: LangId) -> &'static str {
    if is_auto(lang) {
        return "auto";
    }
    match lang.legacy_code() {
        "zh-CN" => "zh",
        "zh-TW" => "zh_HANT",
        other => other,
    }
}

/// `info.extraTranslations` as part-of-speech entries.
fn extra_translations(v: &Value) -> Vec<PartOfSpeechEntry> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| {
            let senses: Vec<Sense> = group["list"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let word = item["word"].as_str()?.to_owned();
                    let back = item["meanings"]
                        .as_array()
                        .map(|m| m.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default();
                    Some(Sense { word, back_translations: back })
                })
                .collect();
            (!senses.is_empty()).then(|| PartOfSpeechEntry {
                part_of_speech: group["type"].as_str().unwrap_or("").to_owned(),
                senses,
            })
        })
        .collect()
}

/// Lingva Translate.
pub struct Lingva {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for Lingva {
    fn default() -> Self {
        Self::new()
    }
}

impl Lingva {
    pub const ID: &'static str = "lingva";

    pub fn new() -> Self {
        let mut info = ProviderInfo::new(Self::ID, "Lingva Translate", Capabilities::TRANSLATE);
        info.description = "Google Translate results through a Lingva instance, without Google seeing you. \
                            Point it at any public or self-hosted instance."
            .into();
        info.homepage = "https://github.com/thedaviddelta/lingva-translate".into();
        info.languages = all_langs_with_auto();
        info.max_input_chars = Some(5000);
        let options = vec![OptionSpec::url("url", "Instance URL", DEFAULT_URL)
            .help("Any Lingva instance; see the project page for a list of public ones.")];
        Self { info, options }
    }

    fn name(&self) -> &str {
        &self.info.name
    }
}

impl Provider for Lingva {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        check_pair(&self.info, req.from, req.to)?;
        let url = format!(
            "{}/api/v1/{}/{}/{}",
            base_url(host.option_str("url"), DEFAULT_URL),
            code(req.from),
            code(req.to),
            url_encode(&req.text)
        );
        let v = json_body(host.http.send(HttpRequest::get(url).provider(self.name()))?, self.name())?;
        let text = require_str(&v["translation"], "translation", self.name())?;
        let info = &v["info"];
        let pron = &info["pronunciation"];
        let romanisation = [&pron["query"], &pron["translation"]]
            .into_iter()
            .find_map(|p| p.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .map(str::to_owned);
        Ok(Translation {
            text: text.to_owned(),
            detected_from: info["detectedSource"].as_str().and_then(detected),
            romanisation,
            parts_of_speech: extra_translations(&info["extraTranslations"]),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::opts;
    use glossa_core::{Error, LangPair};
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;
    use serde_json::json;

    const OK: &str = r#"{"translation":"こんにちは","info":{"detectedSource":"en","pronunciation":{"translation":"Kon'nichiwa"},"definitions":[],"examples":[],"similar":[],"extraTranslations":[{"type":"interjection","list":[{"word":"こんにちは","meanings":["Hello!","Hi!","Good afternoon!"],"frequency":3},{"word":"今日は","meanings":["Hello!"],"frequency":1}]}]}}"#;

    #[test]
    fn get_path_and_full_parse() {
        let http = MockHttp::new().on(Method::Get, "https://lingva.example/api/v1/", 200, OK);
        let o = opts(&[("url", json!("https://lingva.example/"))]);
        let out = with_test_host(&http, &o, |h| {
            Lingva::new().translate(h, &TranslateRequest::new("hello / hi", LangPair::new(LangId::AUTO, LangId(33))))
        })
        .unwrap();
        assert_eq!(out.text, "こんにちは");
        assert_eq!(out.detected_from, Some(LangId::ENGLISH));
        assert_eq!(out.romanisation.as_deref(), Some("Kon'nichiwa"));
        assert_eq!(out.parts_of_speech[0].part_of_speech, "interjection");
        assert_eq!(out.parts_of_speech[0].senses.len(), 2);
        assert_eq!(out.parts_of_speech[0].senses[0].back_translations[2], "Good afternoon!");
        assert_eq!(http.requests()[0].url, "https://lingva.example/api/v1/auto/ja/hello%20%2F%20hi");
    }

    #[test]
    fn chinese_and_hebrew_codes() {
        assert_eq!(code(LangId(11)), "zh");
        assert_eq!(code(LangId(12)), "zh_HANT");
        assert_eq!(code(LangId(26)), "iw");
        assert_eq!(detected("zh_HANT"), Some(LangId(12)));
    }

    #[test]
    fn error_status_and_bad_body() {
        let http = MockHttp::new().on(Method::Get, DEFAULT_URL, 500, r#"{"error":"An error occurred while retrieving the translation"}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Lingva::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
        });
        match r {
            Err(Error::HttpStatus { status: 500, body, .. }) => assert!(body.starts_with("An error occurred")),
            other => panic!("{other:?}"),
        }
        let http = MockHttp::new().on(Method::Get, DEFAULT_URL, 200, r#"{"info":{}}"#);
        let r = with_test_host(&http, &OptionValues::new(), |h| {
            Lingva::new().translate(h, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
        });
        assert!(matches!(r, Err(Error::BadResponse { .. })));
    }
}
