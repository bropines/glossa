//! What happens between "translate this" and a result on screen.
//!
//! 1. **Prepare** the text: normalise line endings, undo hard wraps if the
//!    user asked for that, refuse empty input.
//! 2. **Pick providers**: the one asked for (or the active tab), then — when
//!    fail-over is on — the other enabled ones in the user's order.
//! 3. **Resolve the direction.** This is the classic program's "smart
//!    detection": with a first and a second language configured, text in the
//!    first language goes to the second and anything else comes home to the
//!    first. So a Russian speaker can select English or Russian text anywhere
//!    and always get the other one, with no language menu touched.
//! 4. **Translate**, moving on to the next provider when one fails in a way
//!    another might not (a timeout, a rate limit — not a missing key).
//! 5. Optionally **translate back**, as a check that the meaning survived.

use std::sync::Arc;
use std::time::Instant;

use glossa_core::{
    text, Capabilities, Config, Detection, Error, LangId, LangPair, Result, TranslateRequest,
    Translation,
};
use glossa_modules::{CancelToken, Host, OptionValues, Provider, Registry};
use glossa_net::Http;

/// Knobs for one translation.
#[derive(Debug, Clone, Default)]
pub struct TranslateOptions {
    /// Use this provider first instead of the active tab.
    pub provider: Option<String>,
    /// Override the configured fail-over setting.
    pub fail_over: Option<bool>,
    /// Also translate the result back into the source language.
    pub back_translate: bool,
    /// Extra instructions for LLM providers.
    pub style: Option<String>,
}

/// The result, with everything the UI wants to show about how it was made.
#[derive(Debug, Clone)]
pub struct TranslationOutcome {
    pub translation: Translation,
    /// The direction actually used, after detection.
    pub pair: LangPair,
    /// The text as sent (after preparation).
    pub source: String,
    /// The language detection settled on, if detection ran.
    pub detected: Option<Detection>,
    pub back: Option<Translation>,
    /// Providers that were tried and failed before the one that answered.
    pub failed: Vec<(String, String)>,
}

/// One pass through the pipeline. Borrowed state only: build one per call.
pub struct Pipeline<'a> {
    pub registry: &'a Registry,
    pub config: &'a Config,
    pub http: &'a dyn Http,
    pub cancel: &'a CancelToken,
}

impl Pipeline<'_> {
    /// Prepare text the way every entry point does.
    pub fn prepare(&self, raw: &str) -> Result<String> {
        let mut t = text::normalise_newlines(raw);
        if self.config.advanced.remove_line_breaks {
            t = text::unwrap_hard_breaks(&t);
        }
        let t = t.trim_matches(|c: char| c.is_whitespace() && c != '\u{a0}').to_owned();
        if t.is_empty() {
            return Err(Error::EmptyInput);
        }
        Ok(t)
    }

    /// The providers to try for `cap`, in order: the preferred one, then the
    /// enabled ones in the user's order, then (when `include_all`) every other
    /// capable provider.
    pub fn candidates(&self, cap: Capabilities, preferred: Option<&str>, fail_over: bool) -> Vec<Arc<dyn Provider>> {
        let s = &self.config.services;
        let (active, enabled, order) = if cap == Capabilities::DICTIONARY {
            (s.dictionary_active.as_str(), &s.dictionary_enabled, &s.dictionary_order)
        } else {
            (s.active.as_str(), &s.enabled, &s.order)
        };
        let mut ids: Vec<String> = Vec::new();
        let mut push = |id: &str| {
            if !id.is_empty() && !ids.iter().any(|x| x == id) {
                ids.push(id.to_owned());
            }
        };
        push(preferred.unwrap_or(active));
        if fail_over || preferred.is_none() && active.is_empty() {
            let rank = |id: &String| order.iter().position(|o| o == id).unwrap_or(usize::MAX);
            let mut rest: Vec<String> = enabled.clone();
            rest.sort_by_key(rank);
            for id in &rest {
                push(id);
            }
        }
        let mut out: Vec<Arc<dyn Provider>> = ids
            .iter()
            .filter_map(|id| self.registry.get(id))
            .filter(|p| p.info().capabilities.contains(cap))
            .collect();
        if out.is_empty() {
            // Nothing configured can do it: fall back to anything that can,
            // rather than failing a fresh install whose config names nothing.
            out = self.registry.with_capability(cap);
        }
        out
    }

    fn host<'b>(&'b self, options: &'b OptionValues) -> Host<'b> {
        Host {
            http: self.http,
            options,
            ui_language: LangId::from_code(&self.config.general.ui_language).unwrap_or(LangId::ENGLISH),
            preferred_domain: &self.config.advanced.preferred_domain,
            cancel: self.cancel,
        }
    }

    /// The option values for one provider: its declared defaults overlaid
    /// with the user's settings.
    pub fn options_for(&self, provider: &dyn Provider) -> OptionValues {
        glossa_modules::provider::resolve_options(provider.options(), self.config.services.options.get(&provider.info().id))
    }

    /// Detect the language of `text` with the first provider that can.
    pub fn detect(&self, text: &str, preferred: Option<&str>) -> Result<Detection> {
        let mut last = Error::NoProvider("language detection");
        for p in self.candidates(Capabilities::DETECT_LANGUAGE, preferred, true) {
            self.cancel_check()?;
            let opts = self.options_for(p.as_ref());
            match p.detect(&self.host(&opts), text) {
                Ok(d) if d.is_usable() => return Ok(d),
                Ok(_) => last = Error::LanguageNotDetected,
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Work out the direction for `text`, detecting when needed.
    pub fn resolve_direction(&self, text: &str, pair: LangPair, preferred: Option<&str>) -> (LangPair, Option<Detection>) {
        let g = &self.config.general;
        let must_detect = (pair.from == LangId::AUTO && (pair.to == LangId::AUTO || g.smart_direction)) || g.always_detect_language;
        if !must_detect {
            return (pair, None);
        }
        match self.detect(text, preferred) {
            Ok(d) => {
                let from = d.lang;
                let to = if pair.to == LangId::AUTO || (g.smart_direction && pair.to == from) {
                    self.config.smart_target(from)
                } else {
                    pair.to
                };
                (LangPair::new(from, to), Some(d))
            }
            Err(e) => {
                tracing::debug!("detection failed, letting the provider guess: {e}");
                let to = if pair.to == LangId::AUTO { g.first_language } else { pair.to };
                (LangPair::new(pair.from, to), None)
            }
        }
    }

    pub fn translate(&self, raw: &str, pair: LangPair, opts: &TranslateOptions) -> Result<TranslationOutcome> {
        let started = Instant::now();
        let source = self.prepare(raw)?;
        let fail_over = opts.fail_over.unwrap_or(self.config.services.fail_over);
        let candidates = self.candidates(Capabilities::TRANSLATE, opts.provider.as_deref(), fail_over);
        if candidates.is_empty() {
            return Err(Error::NoProvider("translation"));
        }
        let (pair, detected) = self.resolve_direction(&source, pair, opts.provider.as_deref());
        if pair.from.is_language() && pair.from == pair.to {
            return Err(Error::SameLanguage);
        }

        let mut failed = Vec::new();
        let mut last_err = None;
        for provider in &candidates {
            self.cancel_check()?;
            let info = provider.info();
            if !info.supports_language(pair.to) {
                failed.push((info.id.clone(), Error::TargetLanguageUnsupported { provider: info.name.clone(), lang: pair.to }.to_string()));
                continue;
            }
            // A provider that cannot take `auto` gets the detected language,
            // and one that cannot take the detected language gets `auto`.
            let from = if info.supports_language(pair.from) {
                pair.from
            } else if info.supports_language(LangId::AUTO) {
                LangId::AUTO
            } else {
                failed.push((info.id.clone(), Error::SourceLanguageUnsupported { provider: info.name.clone(), lang: pair.from }.to_string()));
                continue;
            };
            let text = match info.max_input_chars {
                Some(max) => text::truncate_at_boundary(&source, max).to_owned(),
                None => source.clone(),
            };
            let mut req = TranslateRequest::new(text, LangPair::new(from, pair.to));
            req.style = opts.style.clone();
            let opts_values = self.options_for(provider.as_ref());
            let call_started = Instant::now();
            match provider.translate(&self.host(&opts_values), &req) {
                Ok(mut t) if !t.is_empty() => {
                    t.provider_id = info.id.clone();
                    t.elapsed_ms = call_started.elapsed().as_millis() as u64;
                    let resolved_from = match (from, t.detected_from) {
                        (LangId::AUTO, Some(d)) if d.is_language() => d,
                        _ => pair.from,
                    };
                    let resolved = LangPair::new(resolved_from, pair.to);
                    let back = if opts.back_translate && resolved.from.is_language() {
                        let back_req = TranslateRequest::new(t.text.clone(), resolved.swapped());
                        provider.translate(&self.host(&opts_values), &back_req).ok()
                    } else {
                        None
                    };
                    tracing::debug!(provider = %info.id, total_ms = started.elapsed().as_millis() as u64, "translated");
                    return Ok(TranslationOutcome { translation: t, pair: resolved, source, detected, back, failed });
                }
                Ok(_) => {
                    let e = Error::bad_response(&info.name, "empty translation");
                    failed.push((info.id.clone(), e.to_string()));
                    last_err = Some(e);
                }
                Err(e) => {
                    tracing::debug!(provider = %info.id, error = %e, "provider failed");
                    let stop = !e.should_try_next_provider() || !fail_over;
                    failed.push((info.id.clone(), e.to_string()));
                    if stop {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| match candidates.first() {
            Some(p) => Error::TargetLanguageUnsupported { provider: p.info().name.clone(), lang: pair.to },
            None => Error::NoProvider("translation"),
        }))
    }

    fn cancel_check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ProviderInfo;
    use glossa_modules::provider::FnProvider;
    use glossa_net::mock::MockHttp;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn detector(lang: LangId) -> FnProvider {
        FnProvider {
            info: ProviderInfo::new("det", "Detector", Capabilities::DETECT_LANGUAGE),
            translate: None,
            detect: Some(Box::new(move |_, _| Ok(Detection::certain(lang)))),
        }
    }

    fn echo(id: &str) -> FnProvider {
        let id_owned = id.to_owned();
        FnProvider::translator(id, move |_, req| {
            Ok(Translation::plain(format!("{}:{}->{}:{}", id_owned, req.from.legacy_code(), req.to.legacy_code(), req.text)))
        })
    }

    fn failing(id: &str, err: fn() -> Error) -> FnProvider {
        FnProvider::translator(id, move |_, _| Err(err()))
    }

    fn setup(providers: Vec<FnProvider>, active: &str, enabled: &[&str]) -> (Registry, Config) {
        let mut reg = Registry::new();
        for p in providers {
            reg.add_builtin(Arc::new(p));
        }
        let mut cfg = Config::default();
        cfg.services.active = active.into();
        cfg.services.enabled = enabled.iter().map(|s| s.to_string()).collect();
        cfg.services.order = cfg.services.enabled.clone();
        (reg, cfg)
    }

    fn run(reg: &Registry, cfg: &Config, text: &str, pair: LangPair) -> Result<TranslationOutcome> {
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        Pipeline { registry: reg, config: cfg, http: &http, cancel: &cancel }.translate(text, pair, &TranslateOptions::default())
    }

    #[test]
    fn smart_direction_sends_the_first_language_to_the_second_and_back() {
        // first = ru, second = en (the defaults).
        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::RUSSIAN)], "a", &["a"]);
        let out = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.translation.text, "a:ru->en:привет");
        assert_eq!(out.pair, LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));

        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::from_code("de").unwrap())], "a", &["a"]);
        let out = run(&reg, &cfg, "hallo", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.pair.to, LangId::RUSSIAN, "a third language comes home to the first");
    }

    #[test]
    fn a_target_equal_to_the_detected_language_flips_under_smart_direction() {
        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::ENGLISH)], "a", &["a"]);
        let out = run(&reg, &cfg, "hello", LangPair::new(LangId::AUTO, LangId::ENGLISH)).unwrap();
        assert_eq!(out.pair, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
    }

    #[test]
    fn explicit_directions_skip_detection() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let det = FnProvider {
            info: ProviderInfo::new("det", "Detector", Capabilities::DETECT_LANGUAGE),
            translate: None,
            detect: Some(Box::new(move |_, _| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(Detection::certain(LangId::ENGLISH))
            })),
        };
        let (reg, cfg) = setup(vec![echo("a"), det], "a", &["a"]);
        let out = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.text, "a:en->ru:x");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn transient_failures_fall_over_to_the_next_enabled_provider() {
        let (reg, cfg) = setup(vec![failing("a", || Error::Timeout(5)), echo("b")], "a", &["a", "b"]);
        let out = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.provider_id, "b");
        assert_eq!(out.failed.len(), 1);
        assert_eq!(out.failed[0].0, "a");
    }

    #[test]
    fn configuration_faults_do_not_fall_over() {
        let (reg, cfg) = setup(
            vec![failing("a", || Error::MissingCredential { provider: "A".into(), what: "key".into() }), echo("b")],
            "a",
            &["a", "b"],
        );
        let err = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap_err();
        assert!(matches!(err, Error::MissingCredential { .. }));
    }

    #[test]
    fn fail_over_can_be_switched_off() {
        let (reg, mut cfg) = setup(vec![failing("a", || Error::Timeout(5)), echo("b")], "a", &["a", "b"]);
        cfg.services.fail_over = false;
        assert!(matches!(run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)), Err(Error::Timeout(_))));
    }

    #[test]
    fn providers_that_lack_the_target_language_are_skipped() {
        let mut limited = echo("a");
        limited.info.languages = vec![LangId::ENGLISH, LangId::from_code("de").unwrap()];
        let (reg, cfg) = setup(vec![limited, echo("b")], "a", &["a", "b"]);
        let out = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.provider_id, "b");
    }

    #[test]
    fn empty_input_and_same_language_are_refused() {
        let (reg, cfg) = setup(vec![echo("a")], "a", &["a"]);
        assert!(matches!(run(&reg, &cfg, "  \n ", LangPair::new(LangId::AUTO, LangId::ENGLISH)), Err(Error::EmptyInput)));
        let (reg, mut cfg) = setup(vec![echo("a")], "a", &["a"]);
        cfg.general.smart_direction = false;
        assert!(matches!(run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::ENGLISH)), Err(Error::SameLanguage)));
    }

    #[test]
    fn hard_wraps_are_undone_when_configured() {
        let (reg, mut cfg) = setup(vec![echo("a")], "a", &["a"]);
        cfg.advanced.remove_line_breaks = true;
        let out = run(&reg, &cfg, "one\r\ntwo", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.source, "one two");
    }

    #[test]
    fn long_text_is_cut_to_the_providers_limit() {
        let mut short = echo("a");
        short.info.max_input_chars = Some(5);
        let (reg, cfg) = setup(vec![short], "a", &["a"]);
        let out = run(&reg, &cfg, "abcd efgh", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.text, "a:en->ru:abcd");
    }

    #[test]
    fn back_translation_runs_with_the_same_provider() {
        let (reg, cfg) = setup(vec![echo("a")], "a", &["a"]);
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        let p = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let out = p
            .translate("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions { back_translate: true, ..Default::default() })
            .unwrap();
        assert_eq!(out.back.unwrap().text, "a:ru->en:a:en->ru:x");
    }

    #[test]
    fn a_provider_named_explicitly_goes_first() {
        let (reg, cfg) = setup(vec![echo("a"), echo("b")], "a", &["a", "b"]);
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        let p = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let out = p
            .translate("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions { provider: Some("b".into()), ..Default::default() })
            .unwrap();
        assert_eq!(out.translation.provider_id, "b");
    }

    #[test]
    fn an_unconfigured_install_still_finds_a_provider() {
        let (reg, mut cfg) = setup(vec![echo("only")], "", &[]);
        cfg.services.active = "missing".into();
        let out = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.provider_id, "only");
    }

    #[test]
    fn cancellation_stops_the_pipeline() {
        let (reg, cfg) = setup(vec![echo("a")], "a", &["a"]);
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        cancel.cancel();
        let p = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        assert!(matches!(p.translate("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions::default()), Err(Error::Cancelled)));
    }
}
