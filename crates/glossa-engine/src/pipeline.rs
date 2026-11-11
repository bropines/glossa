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
//!    another might not (a timeout, a rate limit — not a missing key). Text
//!    longer than the provider takes goes in pieces cut at paragraph and
//!    sentence ends, and comes back joined.
//! 5. Optionally **translate back**, as a check that the meaning survived.
//!
//! Who may see the text is decided here and nowhere else. The language is
//! detected on this computer or by a service the user enabled, never by
//! "whatever can" (`[general] detect`); and with `[network] offline_only`
//! only providers that stay on this computer are candidates at all.

use std::sync::Arc;
use std::time::Instant;

use glossa_core::config::DetectMode;
use glossa_core::{
    text, Capabilities, Config, Detection, Error, LangId, LangPair, Result, TranslateRequest, Translation,
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
    /// Report the translation so far while a streaming provider writes it.
    pub progress: Option<Progress>,
}

/// A sink for partial translations (the whole text so far each time).
#[derive(Clone)]
pub struct Progress(pub Arc<dyn Fn(&str) + Send + Sync>);

impl Progress {
    pub fn new(f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Progress")
    }
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
    /// The end of the text was not translated: it is longer than the
    /// provider takes even in [`MAX_CHUNKS`] pieces. Front ends say so,
    /// because the translation looks complete.
    pub truncated: bool,
}

/// The most pieces one text is sent in. A provider with a small limit
/// (MyMemory takes 500 characters) would otherwise get hundreds of requests
/// for one long selection and answer with a ban.
pub const MAX_CHUNKS: usize = 40;

impl TranslationOutcome {
    /// The result as the command line (`--json`) and the control API give
    /// it, so the two cannot drift apart.
    pub fn report(&self) -> TranslateReport {
        TranslateReport {
            text: self.translation.text.clone(),
            from: self.pair.from.bcp47().to_owned(),
            to: self.pair.to.bcp47().to_owned(),
            service: self.translation.provider_id.clone(),
            romanization: self.translation.romanisation.clone(),
            source_romanization: self.translation.source_romanisation.clone(),
            alternatives: self.translation.alternatives.clone(),
            back: self.back.as_ref().map(|b| b.text.clone()),
            failed: self.failed.clone(),
            elapsed_ms: self.translation.elapsed_ms,
            truncated: self.truncated,
        }
    }
}

/// A translation as JSON: the shape `glossa-cli --json translate` prints
/// and `/v1/translate` answers. Every field is always present.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranslateReport {
    /// The translation.
    pub text: String,
    /// The direction used, as codes, after detection and smart direction.
    pub from: String,
    pub to: String,
    /// The provider that answered.
    pub service: String,
    /// The translation's romanisation, when the service gives one.
    pub romanization: Option<String>,
    /// The source's transliteration or pronunciation, when the service gives
    /// one.
    pub source_romanization: Option<String>,
    /// Other translations, from services that offer them.
    pub alternatives: Vec<String>,
    /// The back translation, when one was asked for.
    pub back: Option<String>,
    /// Services tried before this one that failed: `[id, message]` pairs.
    pub failed: Vec<(String, String)>,
    /// How long the answering service took.
    pub elapsed_ms: u64,
    /// `true` when the end of the text was left out: it was too long for
    /// the service even in pieces.
    pub truncated: bool,
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

    /// The providers to try for `cap`, in order: the preferred one, then
    /// (with fail-over) the enabled ones in the user's order. Nothing else:
    /// a provider the user did not choose or enable never sees the text.
    /// Only a configuration that names no service at all (a first run
    /// before the lists are filled) gets every capable provider, and never
    /// for language detection. With "offline only" on, providers that leave
    /// this computer are left out.
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
        let mut out: Vec<Arc<dyn Provider>> =
            ids.iter().filter_map(|id| self.registry.get(id)).filter(|p| p.info().capabilities.contains(cap)).collect();
        if out.is_empty() && enabled.is_empty() && cap != Capabilities::DETECT_LANGUAGE {
            // Nothing is configured: anything that can, rather than failing
            // a fresh install whose config names nothing. Detection never
            // gets this: the text would go to services nobody chose.
            out = self.registry.with_capability(cap);
        }
        if self.config.network.offline_only {
            out.retain(|p| self.offline_refusal(p.as_ref()).is_none());
        }
        out
    }

    /// With "offline only" on, why `provider` may not be used: it does not
    /// stay on this computer. `None` when it may (or the switch is off).
    pub fn offline_refusal(&self, provider: &dyn Provider) -> Option<Error> {
        if !self.config.network.offline_only || provider.is_local(&self.options_for(provider)) {
            return None;
        }
        Some(Error::OfflineOnly(format!("{} is an online service", provider.info().name)))
    }

    /// The error for a job no candidate is left for: "offline only" when
    /// that is why, else that nothing can do `what`.
    pub(crate) fn no_provider(&self, what: &'static str) -> Error {
        if self.config.network.offline_only {
            Error::OfflineOnly(format!("no enabled service for {what} works on this computer alone"))
        } else {
            Error::NoProvider(what)
        }
    }

    pub(crate) fn host<'b>(&'b self, options: &'b OptionValues) -> Host<'b> {
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
        glossa_modules::provider::resolve_options(
            provider.options(),
            self.config.services.options.get(&provider.info().id),
        )
    }

    /// Detect the language of `text` as `[general] detect` says: with the
    /// detector built into Glossa, with the first enabled service that can,
    /// or with the built-in one first and a service for what it is unsure
    /// of. The text never goes to a service the user did not enable.
    pub fn detect(&self, text: &str, preferred: Option<&str>) -> Result<Detection> {
        self.detect_noting(text, preferred, &mut Vec::new())
    }

    /// What the built-in detector says, with the user's two languages as
    /// the tie-breakers.
    fn detect_offline(&self, text: &str) -> Option<crate::langdetect::Guess> {
        let g = &self.config.general;
        let hints: Vec<LangId> =
            [g.first_language, g.second_language].into_iter().filter(|l| l.is_language()).collect();
        crate::langdetect::detect(text, &hints)
    }

    /// [`detect`](Self::detect), adding to `unreachable` each provider that
    /// failed for want of a connection (a timeout, a network error), so the
    /// translation that follows does not wait for it a second time.
    fn detect_noting(
        &self,
        text: &str,
        preferred: Option<&str>,
        unreachable: &mut Vec<(String, String)>,
    ) -> Result<Detection> {
        let mode = self.config.general.detect;
        let offline = if mode == DetectMode::Service { None } else { self.detect_offline(text) };
        let usable = offline.map(|g| g.detection).filter(Detection::is_usable);
        match (mode, offline) {
            (DetectMode::Offline, _) => return usable.ok_or(Error::LanguageNotDetected),
            (DetectMode::OfflineThenService, Some(g)) if g.sure && g.detection.is_usable() => return Ok(g.detection),
            _ => {}
        }
        let mut last =
            if usable.is_some() { Error::LanguageNotDetected } else { Error::NoProvider("language detection") };
        for p in self.candidates(Capabilities::DETECT_LANGUAGE, preferred, true) {
            self.cancel_check()?;
            let opts = self.options_for(p.as_ref());
            match p.detect(&self.host(&opts), text) {
                Ok(d) if d.is_usable() => return Ok(d),
                Ok(_) => last = Error::LanguageNotDetected,
                Err(e) => {
                    if matches!(e, Error::Timeout(_) | Error::Network(_)) {
                        unreachable.push((p.info().id.clone(), e.to_string()));
                    }
                    last = e;
                }
            }
        }
        // No service answered (none enabled, or no network): the built-in
        // detector's guess, unsure as it was, beats none.
        usable.ok_or(last)
    }

    /// Work out the direction for `text`, detecting when needed.
    pub fn resolve_direction(
        &self,
        text: &str,
        pair: LangPair,
        preferred: Option<&str>,
    ) -> (LangPair, Option<Detection>) {
        self.resolve_direction_noting(text, pair, preferred, &mut Vec::new())
    }

    fn resolve_direction_noting(
        &self,
        text: &str,
        pair: LangPair,
        preferred: Option<&str>,
        unreachable: &mut Vec<(String, String)>,
    ) -> (LangPair, Option<Detection>) {
        let g = &self.config.general;
        let must_detect =
            (pair.from == LangId::AUTO && (pair.to == LangId::AUTO || g.smart_direction)) || g.always_detect_language;
        if !must_detect {
            return (pair, None);
        }
        match self.detect_noting(text, preferred, unreachable) {
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
            return Err(self.no_provider("translation"));
        }
        let mut unreachable = Vec::new();
        let (pair, detected) = self.resolve_direction_noting(&source, pair, opts.provider.as_deref(), &mut unreachable);
        if pair.from.is_language() && pair.from == pair.to {
            return Err(Error::SameLanguage);
        }

        let was_unreachable = |id: &str| unreachable.iter().find(|(u, _)| u == id);
        // Skip those only while another candidate is left to try.
        let skip_unreachable = candidates.iter().any(|c| was_unreachable(&c.info().id).is_none());
        let mut failed = Vec::new();
        let mut last_err = None;
        for provider in &candidates {
            self.cancel_check()?;
            let info = provider.info();
            // It just timed out or could not connect while detecting; asking
            // again would only double the wait before the next provider (a
            // blocked host costs the whole timeout each time).
            if skip_unreachable {
                if let Some((_, why)) = was_unreachable(&info.id) {
                    tracing::debug!(provider = %info.id, "skipped: unreachable while detecting the language");
                    failed.push((info.id.clone(), why.clone()));
                    last_err = Some(Error::Network(why.clone()));
                    continue;
                }
            }
            if !info.supports_language(pair.to) {
                failed.push((
                    info.id.clone(),
                    Error::TargetLanguageUnsupported {
                        provider: info.name.clone(),
                        lang: pair.to,
                        detail: String::new(),
                    }
                    .to_string(),
                ));
                continue;
            }
            // A provider that cannot take `auto` gets the detected language,
            // and one that cannot take the detected language gets `auto`.
            let from = if info.supports_language(pair.from) {
                pair.from
            } else if info.supports_language(LangId::AUTO) {
                LangId::AUTO
            } else {
                failed.push((
                    info.id.clone(),
                    Error::SourceLanguageUnsupported {
                        provider: info.name.clone(),
                        lang: pair.from,
                        detail: String::new(),
                    }
                    .to_string(),
                ));
                continue;
            };
            let mut req = TranslateRequest::new(source.clone(), LangPair::new(from, pair.to));
            req.style = opts.style.clone();
            let opts_values = self.options_for(provider.as_ref());
            let call_started = Instant::now();
            let host = self.host(&opts_values);
            let result = self.translate_within_limit(provider.as_ref(), &host, &req, opts.progress.as_ref());
            match result {
                Ok((mut t, truncated)) if !t.is_empty() => {
                    t.provider_id = info.id.clone();
                    t.elapsed_ms = call_started.elapsed().as_millis() as u64;
                    let resolved_from = match (from, t.detected_from) {
                        (LangId::AUTO, Some(d)) if d.is_language() => d,
                        _ => pair.from,
                    };
                    let resolved = LangPair::new(resolved_from, pair.to);
                    let back = if opts.back_translate && resolved.from.is_language() {
                        let back_req = TranslateRequest::new(t.text.clone(), resolved.swapped());
                        let host = self.host(&opts_values);
                        self.translate_within_limit(provider.as_ref(), &host, &back_req, None).ok().map(|(b, _)| b)
                    } else {
                        None
                    };
                    tracing::debug!(provider = %info.id, total_ms = started.elapsed().as_millis() as u64, "translated");
                    if truncated {
                        tracing::warn!(provider = %info.id, "the text is too long for the provider; its end was left out");
                    }
                    return Ok(TranslationOutcome {
                        translation: t,
                        pair: resolved,
                        source,
                        detected,
                        back,
                        failed,
                        truncated,
                    });
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
            Some(p) => Error::TargetLanguageUnsupported {
                provider: p.info().name.clone(),
                lang: pair.to,
                detail: String::new(),
            },
            None => Error::NoProvider("translation"),
        }))
    }

    /// Translate `req` with `provider`, in pieces when the text is longer
    /// than the provider takes ([`glossa_core::ProviderInfo::max_input_chars`]).
    ///
    /// The pieces are cut at paragraph, line and sentence ends
    /// ([`text::split_for_limit`]) and sent one after another, each trimmed,
    /// with the whitespace the source had between them put back between the
    /// answers. One at a time, so the provider's own handling of a rate
    /// limit (its waits, its retries) works as for any request; a piece that
    /// fails fails the provider, and fail-over takes the whole text to the
    /// next one. Cancelling stops between pieces.
    ///
    /// Returns the translation and whether the end was left out: more than
    /// [`MAX_CHUNKS`] pieces are not sent.
    fn translate_within_limit(
        &self,
        provider: &dyn Provider,
        host: &Host,
        req: &TranslateRequest,
        progress: Option<&Progress>,
    ) -> Result<(Translation, bool)> {
        let call = |req: &TranslateRequest, before: &str| match progress {
            Some(progress) if before.is_empty() => provider.translate_stream(host, req, &|text| (progress.0)(text)),
            Some(progress) => provider.translate_stream(host, req, &|text| (progress.0)(&format!("{before}{text}"))),
            None => provider.translate(host, req),
        };
        let max = match provider.info().max_input_chars {
            Some(max) if text::char_count(&req.text) > max => max.max(1),
            _ => return call(req, "").map(|t| (t, false)),
        };
        let mut pieces = text::split_for_limit(&req.text, max);
        let truncated = pieces.len() > MAX_CHUNKS;
        pieces.truncate(MAX_CHUNKS);
        tracing::debug!(provider = %provider.info().id, pieces = pieces.len(), max, truncated, "long text sent in pieces");

        let mut whole: Option<Translation> = None;
        // The whitespace owed before the next answer: what the source had
        // after the last piece's text and before this one's.
        let mut gap = String::new();
        let mut from = req.from;
        for piece in pieces {
            self.cancel_check()?;
            let core = piece.trim();
            if core.is_empty() {
                gap.push_str(piece);
                continue;
            }
            let lead = &piece[..piece.len() - piece.trim_start().len()];
            let tail = &piece[piece.trim_end().len()..];
            gap.push_str(lead);
            let mut part_req = req.clone();
            part_req.text = core.to_owned();
            part_req.from = from;
            let part = match whole.as_mut() {
                None => {
                    let part = call(&part_req, "")?;
                    // The pieces after the first go out in the language the
                    // first was found to be in, so a short last piece is not
                    // taken for another language.
                    if let Some(d) = part.detected_from.filter(|d| d.is_language()) {
                        if from == LangId::AUTO && provider.info().supports_language(d) {
                            from = d;
                        }
                    }
                    whole = Some(Translation { alternatives: Vec::new(), parts_of_speech: Vec::new(), ..part });
                    gap = tail.to_owned();
                    continue;
                }
                Some(whole) => {
                    whole.text.push_str(&gap);
                    let part = call(&part_req, &whole.text)?;
                    if part.is_empty() {
                        return Err(Error::bad_response(
                            &provider.info().name,
                            "empty translation for part of the text",
                        ));
                    }
                    part
                }
            };
            let whole = whole.as_mut().expect("set by the first piece");
            whole.text.push_str(&part.text);
            let join = |a: &mut Option<String>, b: Option<String>| {
                if let (Some(a), Some(b)) = (a.as_mut(), b) {
                    a.push_str(&gap);
                    a.push_str(&b);
                }
            };
            join(&mut whole.romanisation, part.romanisation);
            join(&mut whole.source_romanisation, part.source_romanisation);
            gap = tail.to_owned();
        }
        whole.map(|t| (t, truncated)).ok_or(Error::EmptyInput)
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
            Ok(Translation::plain(format!(
                "{}:{}->{}:{}",
                id_owned,
                req.from.legacy_code(),
                req.to.legacy_code(),
                req.text
            )))
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
        // These tests are about the services; the built-in detector has its
        // own below.
        cfg.general.detect = DetectMode::Service;
        cfg.services.active = active.into();
        cfg.services.enabled = enabled.iter().map(|s| s.to_string()).collect();
        cfg.services.order = cfg.services.enabled.clone();
        (reg, cfg)
    }

    fn run(reg: &Registry, cfg: &Config, text: &str, pair: LangPair) -> Result<TranslationOutcome> {
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        Pipeline { registry: reg, config: cfg, http: &http, cancel: &cancel }.translate(
            text,
            pair,
            &TranslateOptions::default(),
        )
    }

    #[test]
    fn smart_direction_sends_the_first_language_to_the_second_and_back() {
        // first = ru, second = en (the defaults).
        // The detector is one of the user's enabled services: detection
        // asks no others.
        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::RUSSIAN)], "a", &["a", "det"]);
        let out = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.translation.text, "a:ru->en:привет");
        assert_eq!(out.pair, LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));

        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::from_code("de").unwrap())], "a", &["a", "det"]);
        let out = run(&reg, &cfg, "hallo", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.pair.to, LangId::RUSSIAN, "a third language comes home to the first");
    }

    #[test]
    fn a_target_equal_to_the_detected_language_flips_under_smart_direction() {
        let (reg, cfg) = setup(vec![echo("a"), detector(LangId::ENGLISH)], "a", &["a", "det"]);
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
        let (reg, cfg) = setup(vec![echo("a"), det], "a", &["a", "det"]);
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
    fn a_provider_unreachable_while_detecting_is_not_asked_to_translate() {
        let translations = Arc::new(AtomicUsize::new(0));
        let t = translations.clone();
        let blocked = FnProvider {
            info: ProviderInfo::new("a", "A", Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE),
            translate: Some(Box::new(move |_, _| {
                t.fetch_add(1, Ordering::SeqCst);
                Err(Error::Timeout(10_000))
            })),
            detect: Some(Box::new(|_, _| Err(Error::Timeout(10_000)))),
        };
        let (reg, cfg) = setup(vec![blocked, echo("b"), detector(LangId::ENGLISH)], "a", &["a", "b", "det"]);
        let out = run(&reg, &cfg, "hello", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.translation.provider_id, "b");
        assert_eq!(out.pair, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), "the next detector answered");
        assert_eq!(translations.load(Ordering::SeqCst), 0, "one timeout per provider and request");
        assert_eq!(out.failed[0].0, "a");

        // With nothing else left, it is still tried.
        let translations = Arc::new(AtomicUsize::new(0));
        let t = translations.clone();
        let only = FnProvider {
            info: ProviderInfo::new("a", "A", Capabilities::TRANSLATE | Capabilities::DETECT_LANGUAGE),
            translate: Some(Box::new(move |_, _| {
                t.fetch_add(1, Ordering::SeqCst);
                Ok(Translation::plain("ok"))
            })),
            detect: Some(Box::new(|_, _| Err(Error::Network("reset".into())))),
        };
        let (reg, cfg) = setup(vec![only], "a", &["a"]);
        assert_eq!(run(&reg, &cfg, "hello", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap().translation.text, "ok");
        assert_eq!(translations.load(Ordering::SeqCst), 1);
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
        assert!(matches!(
            run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)),
            Err(Error::Timeout(_))
        ));
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
        assert!(matches!(
            run(&reg, &cfg, "  \n ", LangPair::new(LangId::AUTO, LangId::ENGLISH)),
            Err(Error::EmptyInput)
        ));
        let (reg, mut cfg) = setup(vec![echo("a")], "a", &["a"]);
        cfg.general.smart_direction = false;
        assert!(matches!(
            run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::ENGLISH)),
            Err(Error::SameLanguage)
        ));
    }

    #[test]
    fn hard_wraps_are_undone_when_configured() {
        let (reg, mut cfg) = setup(vec![echo("a")], "a", &["a"]);
        cfg.advanced.remove_line_breaks = true;
        let out = run(&reg, &cfg, "one\r\ntwo", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.source, "one two");
    }

    /// A provider that upper-cases what it gets and counts its calls, with
    /// an input limit.
    fn shouting(id: &str, max: usize, calls: Arc<AtomicUsize>) -> FnProvider {
        let mut p = FnProvider::translator(id, move |_, req| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(req.text.chars().count() <= max, "over the limit: {:?}", req.text);
            assert_eq!(req.text, req.text.trim(), "pieces go out trimmed");
            Ok(Translation::plain(req.text.to_uppercase()))
        });
        p.info.max_input_chars = Some(max);
        p
    }

    /// The defect: text beyond the provider's limit was cut off silently and
    /// the result looked complete. Now it is sent in pieces and nothing is
    /// lost, the whitespace between the pieces included.
    #[test]
    fn long_text_is_translated_in_pieces_at_sentence_and_paragraph_ends() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (reg, cfg) = setup(vec![shouting("a", 30, calls.clone())], "a", &["a"]);
        let text =
            "First sentence here. Second one follows!\n\nA new paragraph starts. And ends.\n  Indented last line";
        let out = run(&reg, &cfg, text, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.text, text.to_uppercase(), "everything, with its line breaks and spaces");
        assert!(!out.truncated);
        assert!(calls.load(Ordering::SeqCst) >= 4, "{} requests", calls.load(Ordering::SeqCst));
        assert!(!out.report().truncated);

        // Text within the limit is one request, as before.
        calls.store(0, Ordering::SeqCst);
        let out = run(&reg, &cfg, "abcd efgh", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!((out.translation.text.as_str(), calls.load(Ordering::SeqCst)), ("ABCD EFGH", 1));

        // No spaces to cut at (CJK): still whole.
        let cjk = "今日は晴れです。明日は雨が降るでしょう。週末は友達と映画を見に行きます。楽しみにしています。";
        let out = run(&reg, &cfg, cjk, LangPair::new(LangId::from_code("ja").unwrap(), LangId::RUSSIAN)).unwrap();
        assert_eq!(out.translation.text, cjk);
    }

    #[test]
    fn text_too_long_even_in_pieces_is_marked_truncated() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (reg, cfg) = setup(vec![shouting("a", 10, calls.clone())], "a", &["a"]);
        let text = "word ".repeat(MAX_CHUNKS * 2 + 10);
        let out = run(&reg, &cfg, &text, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert!(out.truncated, "the result says its end is missing");
        assert!(out.report().truncated);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_CHUNKS);
        assert!(out.translation.text.starts_with("WORD WORD"));
        assert!(out.translation.text.len() < text.trim().len());
    }

    #[test]
    fn a_piece_that_fails_fails_the_provider_and_the_next_gets_the_whole_text() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut flaky = FnProvider::translator("a", move |_, req| {
            if c.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(Error::RateLimited { provider: "A".into(), retry_after_secs: None });
            }
            Ok(Translation::plain(req.text.to_uppercase()))
        });
        flaky.info.max_input_chars = Some(12);
        let (reg, cfg) = setup(vec![flaky, echo("b")], "a", &["a", "b"]);
        let out =
            run(&reg, &cfg, "one two three. four five six.", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap();
        assert_eq!(
            out.translation.text, "b:en->ru:one two three. four five six.",
            "not half of one and half of the other"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "the pieces after the failed one were not sent");
        assert_eq!(out.failed[0].0, "a");
    }

    #[test]
    fn cancelling_stops_between_pieces() {
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let mut p = FnProvider::translator("a", move |_, req| {
            c.cancel();
            Ok(Translation::plain(req.text.clone()))
        });
        p.info.max_input_chars = Some(8);
        let (reg, cfg) = setup(vec![p], "a", &["a"]);
        let http = MockHttp::new();
        let pipeline = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let result = pipeline.translate(
            "one two three four five six",
            LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
            &TranslateOptions::default(),
        );
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    }

    #[test]
    fn pieces_report_progress_as_one_growing_text_and_share_one_source_language() {
        let seen_from = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = seen_from.clone();
        let mut p = FnProvider::translator("a", move |_, req| {
            seen.lock().push(req.from);
            Ok(Translation { detected_from: Some(LangId::ENGLISH), ..Translation::plain(req.text.to_uppercase()) })
        });
        p.info.max_input_chars = Some(10);
        let (reg, mut cfg) = setup(vec![p], "a", &["a"]);
        cfg.general.smart_direction = false;
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        let pipeline = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let out = pipeline
            .translate(
                "alpha beta gamma delta",
                LangPair::new(LangId::AUTO, LangId::RUSSIAN),
                &TranslateOptions::default(),
            )
            .unwrap();
        assert_eq!(out.translation.text, "ALPHA BETA GAMMA DELTA");
        let from = seen_from.lock().clone();
        assert_eq!(from[0], LangId::AUTO);
        assert!(from[1..].iter().all(|f| *f == LangId::ENGLISH), "{from:?}");
        assert_eq!(out.pair.from, LangId::ENGLISH);
    }

    // -- who detects the language -------------------------------------------

    /// A detector that counts how often the text reached it.
    fn counting_detector(id: &str, lang: LangId, calls: Arc<AtomicUsize>) -> FnProvider {
        FnProvider {
            info: ProviderInfo::new(id, id, Capabilities::DETECT_LANGUAGE),
            translate: None,
            detect: Some(Box::new(move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Detection::certain(lang))
            })),
        }
    }

    const RUSSIAN_SENTENCE: &str = "Съешь же ещё этих мягких французских булок, да выпей чаю.";

    /// The defect: with no enabled service able to detect, the text went to
    /// every detector in the registry (Google, Bing, Yandex, LibreTranslate),
    /// whatever the user had enabled.
    #[test]
    fn detection_never_reaches_a_service_that_is_not_enabled() {
        let calls = Arc::new(AtomicUsize::new(0));
        for mode in [DetectMode::Service, DetectMode::OfflineThenService, DetectMode::Offline] {
            let (reg, mut cfg) = setup(
                vec![
                    echo("private"),
                    counting_detector("google", LangId::ENGLISH, calls.clone()),
                    counting_detector("bing", LangId::ENGLISH, calls.clone()),
                ],
                "private",
                &["private"],
            );
            cfg.general.detect = mode;
            // Short and long, sure and unsure.
            for text in ["hi", "привет", RUSSIAN_SENTENCE] {
                let out = run(&reg, &cfg, text, LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
                assert_eq!(out.translation.provider_id, "private");
            }
            assert_eq!(calls.load(Ordering::SeqCst), 0, "{mode:?}: the text reached a detector nobody enabled");
        }
    }

    #[test]
    fn the_builtin_detector_answers_without_any_service() {
        let (reg, mut cfg) = setup(vec![echo("a")], "a", &["a"]);
        cfg.general.detect = DetectMode::Offline;
        let out = run(&reg, &cfg, RUSSIAN_SENTENCE, LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.pair, LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));
        assert_eq!(out.detected.map(|d| d.lang), Some(LangId::RUSSIAN));
        let out = run(
            &reg,
            &cfg,
            "The weather is lovely today, let us go for a long walk.",
            LangPair::new(LangId::AUTO, LangId::AUTO),
        )
        .unwrap();
        assert_eq!(out.pair, LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
        // A word or two: the user's languages (ru, en) decide.
        let out = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.pair, LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));
    }

    #[test]
    fn offline_mode_asks_no_service_even_an_enabled_one() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (reg, mut cfg) =
            setup(vec![echo("a"), counting_detector("det", LangId::ENGLISH, calls.clone())], "a", &["a", "det"]);
        cfg.general.detect = DetectMode::Offline;
        for text in ["привет", RUSSIAN_SENTENCE, "12345"] {
            run(&reg, &cfg, text, LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_default_asks_an_enabled_service_only_about_what_the_builtin_detector_is_unsure_of() {
        let calls = Arc::new(AtomicUsize::new(0));
        let german = LangId::from_code("de").unwrap();
        let (reg, mut cfg) =
            setup(vec![echo("a"), counting_detector("det", german, calls.clone())], "a", &["a", "det"]);
        cfg.general.detect = DetectMode::OfflineThenService;
        assert_eq!(Config::default().general.detect, DetectMode::OfflineThenService);
        // Sure: answered here, the service never sees it.
        let out = run(&reg, &cfg, RUSSIAN_SENTENCE, LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!((out.pair.from, calls.load(Ordering::SeqCst)), (LangId::RUSSIAN, 0));
        // One short word: the enabled service decides.
        let out = run(&reg, &cfg, "gut", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!((out.pair.from, calls.load(Ordering::SeqCst)), (german, 1));

        // The service is unreachable (no network): the built-in guess stands.
        let down = FnProvider {
            info: ProviderInfo::new("det", "det", Capabilities::DETECT_LANGUAGE),
            translate: None,
            detect: Some(Box::new(|_, _| Err(Error::Network("down".into())))),
        };
        let (reg, mut cfg) = setup(vec![echo("a"), down], "a", &["a", "det"]);
        cfg.general.detect = DetectMode::OfflineThenService;
        let out = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.pair, LangPair::new(LangId::RUSSIAN, LangId::ENGLISH));
    }

    // -- offline only ---------------------------------------------------------

    #[test]
    fn offline_only_leaves_out_every_provider_that_is_not_local() {
        let online_calls = Arc::new(AtomicUsize::new(0));
        let c = online_calls.clone();
        let online = FnProvider::translator("online", move |_, _| {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(Translation::plain("leaked"))
        });
        let mut local = echo("local");
        local.info.capabilities |= Capabilities::OFFLINE;
        let (reg, mut cfg) = setup(
            vec![online, local, counting_detector("det", LangId::ENGLISH, online_calls.clone())],
            "online",
            &["online", "local", "det"],
        );
        cfg.network.offline_only = true;
        cfg.general.detect = DetectMode::OfflineThenService;
        let out = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap();
        assert_eq!(out.translation.provider_id, "local");
        assert_eq!(online_calls.load(Ordering::SeqCst), 0, "neither the translator nor the detector was asked");

        // Nothing local enabled: a clear error, not a request.
        cfg.services.enabled = vec!["online".into()];
        let err = run(&reg, &cfg, "привет", LangPair::new(LangId::AUTO, LangId::AUTO)).unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");
        // Asked for by name: refused too.
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        let p = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let err = p
            .translate(
                "x",
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
                &TranslateOptions { provider: Some("online".into()), fail_over: Some(false), ..Default::default() },
            )
            .unwrap_err();
        assert!(matches!(err, Error::OfflineOnly(_)), "{err:?}");
        assert_eq!(online_calls.load(Ordering::SeqCst), 0);
    }

    /// A service the user uninstalled or mistyped is not replaced by
    /// whatever else is installed.
    #[test]
    fn a_configured_install_does_not_fall_back_to_services_nobody_enabled() {
        let (reg, mut cfg) = setup(vec![echo("other")], "gone", &["gone"]);
        cfg.services.active = "gone".into();
        let err = run(&reg, &cfg, "x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)).unwrap_err();
        assert!(matches!(err, Error::NoProvider(_)), "{err:?}");
    }

    #[test]
    fn back_translation_runs_with_the_same_provider() {
        let (reg, cfg) = setup(vec![echo("a")], "a", &["a"]);
        let http = MockHttp::new();
        let cancel = CancelToken::new();
        let p = Pipeline { registry: &reg, config: &cfg, http: &http, cancel: &cancel };
        let out = p
            .translate(
                "x",
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
                &TranslateOptions { back_translate: true, ..Default::default() },
            )
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
            .translate(
                "x",
                LangPair::new(LangId::ENGLISH, LangId::RUSSIAN),
                &TranslateOptions { provider: Some("b".into()), ..Default::default() },
            )
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
        assert!(matches!(
            p.translate("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN), &TranslateOptions::default()),
            Err(Error::Cancelled)
        ));
    }
}
