//! A [`Provider`] for any module that speaks the module protocol.
//!
//! The Python and native runtimes differ only in how a JSON message reaches
//! the module and how its reply comes back. That difference is the
//! [`Transport`] trait; everything else — turning our requests into protocol
//! messages and back, mapping language codes, answering the module's host
//! calls under its permissions, translating its errors — is done once, here.

use std::sync::Arc;
use std::time::Instant;

use glossa_core::dict::{Example, Explanation, Pronunciation};
use glossa_core::translate::{PartOfSpeechEntry, Sense};
use glossa_core::{
    Article, Audio, Detection, DictRequest, Error, OcrRequest, OcrResult, ProviderInfo, Result, SpeakRequest,
    TranslateRequest, Translation,
};
use glossa_module_sdk::wire::{self, ModuleError, Reply};

use crate::host_api::{from_module_error, HostApi, ModuleCache, ScopedHttp};
use crate::manifest::Manifest;
use crate::provider::{Host, OptionSpec, Provider};

/// Carries one call to a module and its reply back.
pub trait Transport: Send + Sync {
    /// Send `method(params)`. While the module works it may call back into
    /// the host; `host` answers those calls and must be used for every one.
    fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
    ) -> std::result::Result<serde_json::Value, ModuleError>;

    /// Free whatever the transport holds (a child process). The next call
    /// starts it again.
    fn suspend(&self) {}
}

pub struct ProtocolProvider<T: Transport> {
    manifest: Manifest,
    info: ProviderInfo,
    transport: T,
    cache: Arc<ModuleCache>,
}

impl<T: Transport> ProtocolProvider<T> {
    pub fn new(manifest: Manifest, transport: T, cache: Arc<ModuleCache>) -> Self {
        let info = manifest.provider_info();
        Self { manifest, info, transport, cache }
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn invoke<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        host: &Host,
        method: &str,
        params: P,
    ) -> Result<R> {
        host.check_cancelled()?;
        let api = HostApi {
            http: ScopedHttp {
                inner: host.http,
                permissions: &self.manifest.permissions,
                module_id: &self.manifest.module.id,
            },
            cache: &self.cache,
        };
        let params = serde_json::to_value(params).map_err(|e| Error::module(&self.info.id, e))?;
        let started = Instant::now();
        let value = self
            .transport
            .call(method, params, &|m, p| api.handle(m, p))
            .map_err(|e| from_module_error(&self.info.id, &self.info.name, e))?;
        tracing::debug!(module = %self.info.id, method, ms = started.elapsed().as_millis() as u64, "module call");
        host.check_cancelled()?;
        serde_json::from_value(value).map_err(|e| Error::bad_response(&self.info.name, format!("{method} reply: {e}")))
    }

    fn code(&self, lang: glossa_core::LangId) -> String {
        self.manifest.code_for(lang)
    }
}

impl<T: Transport> Provider for ProtocolProvider<T> {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.manifest.options
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        let params = wire::TranslateParams {
            text: req.text.clone(),
            from: self.code(req.from),
            to: self.code(req.to),
            style: req.style.clone(),
            glossary: req
                .glossary
                .iter()
                .map(|g| wire::GlossaryTerm { source: g.source.clone(), target: g.target.clone() })
                .collect(),
            options: host.options.clone(),
        };
        let r: wire::TranslateResult = self.invoke(host, "translate", params)?;
        Ok(Translation {
            text: r.text,
            detected_from: r.detected_from.as_deref().and_then(|c| self.manifest.lang_for(c)),
            actual_to: None,
            romanisation: r.romanization.filter(|s| !s.trim().is_empty()),
            alternatives: r.alternatives,
            parts_of_speech: r
                .parts_of_speech
                .into_iter()
                .map(|p| PartOfSpeechEntry {
                    part_of_speech: p.part_of_speech,
                    senses: p
                        .senses
                        .into_iter()
                        .map(|s| Sense { word: s.word, back_translations: s.back_translations })
                        .collect(),
                })
                .collect(),
            provider_id: String::new(),
            elapsed_ms: 0,
        })
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let r: wire::DetectResult =
            self.invoke(host, "detect", wire::DetectParams { text: text.to_owned(), options: host.options.clone() })?;
        let lang = self.manifest.lang_for(&r.lang).ok_or(Error::LanguageNotDetected)?;
        Ok(Detection { lang, confidence: r.confidence })
    }

    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        let r: wire::ArticleResult = self.invoke(
            host,
            "dictionary",
            wire::DictionaryParams {
                text: req.text.clone(),
                from: self.code(req.from),
                to: self.code(req.to),
                exact: req.exact,
                options: host.options.clone(),
            },
        )?;
        Ok(article_from_wire(r, &req.text))
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        let r: wire::SpeakResult = self.invoke(
            host,
            "speak",
            wire::SpeakParams {
                text: req.text.clone(),
                lang: self.code(req.lang),
                slow: req.slow,
                options: host.options.clone(),
            },
        )?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(r.audio_base64.as_bytes())
            .map_err(|e| Error::bad_response(&self.info.name, format!("audio_base64: {e}")))?;
        Ok(Audio { bytes, mime: if r.mime.is_empty() { "audio/mpeg".into() } else { r.mime } })
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        use base64::Engine;
        let bytes = req.image.to_bytes()?;
        let r: wire::RecognizeResult = self.invoke(
            host,
            "recognize",
            wire::RecognizeParams {
                image_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                lang: self.code(req.lang),
                translate_to: req.translate_to.map(|l| self.code(l)),
                preserve_line_breaks: req.preserve_line_breaks,
                options: host.options.clone(),
            },
        )?;
        Ok(OcrResult {
            text: r.text,
            translation: r.translation,
            detected_lang: r.detected_lang.as_deref().and_then(|c| self.manifest.lang_for(c)),
            ..Default::default()
        })
    }

    fn suspend(&self) {
        self.transport.suspend();
    }
}

pub fn article_from_wire(r: wire::ArticleResult, query: &str) -> Article {
    Article {
        headword: if r.headword.is_empty() { query.to_owned() } else { r.headword },
        pronunciations: r
            .pronunciations
            .into_iter()
            .map(|p| Pronunciation { region: p.region, symbol: p.symbol, audio_url: p.audio_url })
            .collect(),
        explanations: r
            .explanations
            .into_iter()
            .map(|e| Explanation { part_of_speech: e.part_of_speech, definitions: e.definitions })
            .collect(),
        examples: r.examples.into_iter().map(|e| Example { source: e.source, target: e.target }).collect(),
        associations: r.associations,
        html: r.html,
        link: r.link,
        provider_id: String::new(),
        elapsed_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{with_test_host, OptionValues};
    use glossa_core::{LangId, LangPair};
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    /// A transport that answers like a tiny module would, including one host
    /// callback.
    struct Echo;

    impl Transport for Echo {
        fn call(
            &self,
            method: &str,
            params: serde_json::Value,
            host: &dyn Fn(&str, serde_json::Value) -> Reply,
        ) -> std::result::Result<serde_json::Value, ModuleError> {
            match method {
                "translate" => {
                    let fetched = host("http", serde_json::json!({"url": "https://api.test/x"})).into_result()?;
                    Ok(serde_json::json!({
                        "text": format!("{}|{}|{}|{}", params["text"].as_str().unwrap(), params["from"], params["to"], fetched["body"]),
                        "detected_from": "ZH",
                        "romanization": " ",
                    }))
                }
                "detect" => Ok(serde_json::json!({ "lang": "ru", "confidence": 0.9 })),
                "dictionary" => Ok(serde_json::json!({ "explanations": [{ "trait": "n", "explains": ["x"] }] })),
                _ => Err(ModuleError::unsupported(method)),
            }
        }
    }

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
            capabilities = ["translate", "detect_language", "dictionary"]
            [module]
            id = "echo"
            name = "Echo"
            runtime = "python"
            [language_map]
            "zh-Hans" = "ZH"
            [permissions]
            network = ["api.test"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn requests_and_replies_cross_the_protocol() {
        let p = ProtocolProvider::new(manifest(), Echo, ModuleCache::new());
        let http = MockHttp::new().on(Method::Get, "https://api.test/", 200, "ok");
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(
                    host,
                    &TranslateRequest::new("hi", LangPair::new(LangId::AUTO, LangId::from_code("zh-CN").unwrap())),
                )
                .unwrap();
            assert_eq!(t.text, "hi|\"auto\"|\"ZH\"|\"ok\"", "codes mapped out, host call answered");
            assert_eq!(t.detected_from, LangId::from_code("zh-Hans"), "codes mapped back");
            assert_eq!(t.romanisation, None, "blank romanisation dropped");

            let d = p.detect(host, "привет").unwrap();
            assert_eq!(d.lang, LangId::RUSSIAN);

            let a = p
                .dictionary(
                    host,
                    &DictRequest { text: "w".into(), from: LangId::ENGLISH, to: LangId::RUSSIAN, exact: true },
                )
                .unwrap();
            assert_eq!(a.headword, "w", "headword defaults to the query");
            assert_eq!(a.explanations[0].definitions, ["x"]);

            assert!(matches!(
                p.speak(host, &SpeakRequest { text: "x".into(), lang: LangId::ENGLISH, slow: false }),
                Err(Error::Unsupported { .. })
            ));
        });
    }

    #[test]
    fn host_calls_outside_the_permissions_fail_the_call() {
        let mut m = manifest();
        m.permissions.network = vec!["other.test".into()];
        let p = ProtocolProvider::new(m, Echo, ModuleCache::new());
        let http = MockHttp::new().on(Method::Get, "https://api.test/", 200, "ok");
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let err = p
                .translate(host, &TranslateRequest::new("hi", LangPair::new(LangId::AUTO, LangId::ENGLISH)))
                .unwrap_err();
            assert!(err.to_string().contains("permissions"), "{err}");
        });
        assert!(http.requests().is_empty());
    }
}
