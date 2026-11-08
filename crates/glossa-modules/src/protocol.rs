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
use glossa_core::ocr::{parse, Baseline, OcrBlock, OcrGranularity, OcrLevel, Rect, WritingDirection};
use glossa_core::translate::{PartOfSpeechEntry, Sense};
use glossa_core::{
    Article, Audio, Detection, DictRequest, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result, SpeakRequest,
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
    /// The module's assets, for the `assets` host call.
    assets: Option<crate::assets::ModuleAssets>,
    page: crate::settings::PageCache,
}

impl<T: Transport> ProtocolProvider<T> {
    pub fn new(manifest: Manifest, transport: T, cache: Arc<ModuleCache>) -> Self {
        let info = manifest.provider_info();
        // The registry sets `assets_dir` to `<models>/<id>`.
        let assets = manifest
            .assets_dir
            .as_ref()
            .and_then(|dir| dir.parent())
            .and_then(|models| crate::assets::ModuleAssets::for_manifest(&manifest, models).ok().flatten());
        Self { manifest, info, transport, cache, assets, page: Default::default() }
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
        self.invoke_with(host, method, |_| params)
    }

    /// [`Self::invoke`] with the parameters built once the call's host API
    /// exists, so they can refer to it (an image handle).
    fn invoke_with<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        host: &Host,
        method: &str,
        params: impl FnOnce(&HostApi) -> P,
    ) -> Result<R> {
        self.invoke_full(host, method, None, params)
    }

    /// [`Self::invoke_with`], with the module's strings for the `t` host
    /// call (actions).
    fn invoke_full<P: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        host: &Host,
        method: &str,
        strings: Option<&std::collections::BTreeMap<String, String>>,
        params: impl FnOnce(&HostApi) -> P,
    ) -> Result<R> {
        host.check_cancelled()?;
        let api = HostApi::new(
            ScopedHttp {
                inner: host.http,
                permissions: &self.manifest.permissions,
                module_id: &self.manifest.module.id,
            },
            &self.cache,
        )
        .with_assets(self.assets.as_ref())
        .with_strings(strings);
        let params = serde_json::to_value(params(&api)).map_err(|e| Error::module(&self.info.id, e))?;
        let started = Instant::now();
        let value = self.transport.call(method, params, &|m, p| api.handle(m, p)).map_err(|e| match e.kind {
            // "Not found" means no text in an image only for recognition;
            // for a lookup it is the module's own "no article" message.
            wire::ErrorKind::NotFound if method != "recognize" => Error::module(&self.info.id, e.message),
            _ => from_module_error(&self.info.id, &self.info.name, e),
        })?;
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

    fn settings_page(&self, lang: &str) -> std::sync::Arc<crate::settings::SettingsPage> {
        self.page.get(lang, || self.manifest.settings_page(lang))
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
            source_romanisation: None,
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
        let size = image_dimensions(&bytes);
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let r: wire::RecognizeResult = self.invoke_with(host, "recognize", |api| wire::RecognizeParams {
            image_base64,
            lang: self.code(req.lang),
            translate_to: req.translate_to.map(|l| self.code(l)),
            preserve_line_breaks: req.preserve_line_breaks,
            options: host.options.clone(),
            granularity: Some(granularity_name(req.granularity).to_owned()),
            image_handle: api.register_image(bytes),
        })?;
        let mut out = ocr_from_wire(r, &|c| self.manifest.lang_for(c))
            .map_err(|e| Error::bad_response(&self.info.name, format!("recognize reply: {e}")))?;
        if out.image_size.is_none() {
            out.image_size = size;
        }
        Ok(out)
    }

    fn action(&self, host: &Host, mut params: wire::ActionParams) -> Result<wire::ActionReply> {
        let Some(spec) = self.manifest.actions.iter().find(|a| a.id == params.action) else {
            // Only what the manifest declares reaches the module: a module
            // without `[[actions]]` never sees the method at all.
            return Err(Error::module(&self.info.id, format!("module.toml declares no action {:?}", params.action)));
        };
        if spec.kind() != crate::actions::ActionKind::Handler {
            return Err(Error::Unsupported {
                provider: self.info.name.clone(),
                capability: "call and built-in actions (they arrive with module panels)",
            });
        }
        params.handler = spec.handler_name().to_owned();
        params.options = host.options.clone();
        if params.strings.is_empty() {
            params.strings = crate::actions::code_strings(&self.manifest.strings, &params.ui_lang);
        }
        let strings = params.strings.clone();
        let reply: serde_json::Value = self.invoke_full(host, "action", Some(&strings), |_| params)?;
        if reply.is_null() {
            // A handler with nothing to say ("done").
            return Ok(wire::ActionReply::default());
        }
        let mut reply: wire::ActionReply = serde_json::from_value(reply)
            .map_err(|e| Error::bad_response(&self.info.name, format!("action reply: {e}")))?;
        crate::actions::check_reply(&self.manifest.options, &mut reply)
            .map_err(|e| Error::bad_response(&self.info.name, format!("action reply: {e}")))?;
        Ok(reply)
    }

    fn suspend(&self) {
        self.transport.suspend();
    }
}

pub fn granularity_name(g: OcrGranularity) -> &'static str {
    match g {
        OcrGranularity::FullText => "full_text",
        OcrGranularity::Blocks => "blocks",
        OcrGranularity::Lines => "lines",
        OcrGranularity::Words => "words",
    }
}

/// A module's recognition result as ours. The structure comes from
/// `blocks`, else from `hocr`, else from `alto`; boxes are brought back
/// onto the input image when the module sent a `transform`; the text is
/// joined from the structure when the module sent none. `lang_for` maps the
/// module's language codes to ours.
pub fn ocr_from_wire(
    r: wire::RecognizeResult,
    lang_for: &dyn Fn(&str) -> Option<LangId>,
) -> std::result::Result<OcrResult, String> {
    let mut out = if !r.blocks.is_empty() {
        OcrResult {
            blocks: r.blocks.into_iter().map(|n| node_from_wire(n, lang_for)).collect(),
            image_size: r.image_size.map(|[w, h]| (w, h)).filter(|(w, h)| *w > 0 && *h > 0),
            ..Default::default()
        }
    } else if let Some(doc) = r.hocr.as_deref().filter(|d| !d.trim().is_empty()) {
        parse::hocr(doc)?
    } else if let Some(doc) = r.alto.as_deref().filter(|d| !d.trim().is_empty()) {
        parse::alto(doc)?
    } else {
        OcrResult::default()
    };
    if let Some(t) = r.transform.filter(|t| !t.is_identity()) {
        if t.0.iter().any(|v| !v.is_finite()) {
            return Err("transform has a value that is not a number".into());
        }
        for b in &mut out.blocks {
            map_back(b, &t);
        }
        // The size given was the preprocessed copy's.
        out.image_size = None;
    }
    for b in &mut out.blocks {
        fill_bounds(b);
    }
    out = out.with_levels();
    for b in &mut out.blocks {
        fill_text(b);
    }
    out.text = if r.text.trim().is_empty() { out.text_from_blocks() } else { r.text };
    out.translation = r.translation.filter(|t| !t.trim().is_empty());
    if let Some(code) = r.detected_lang.as_deref() {
        out.detected_lang = lang_for(code);
    }
    Ok(out)
}

/// Give a node without a box the union of its children's, so a module may
/// send boxes for words only.
fn fill_bounds(b: &mut OcrBlock) {
    for c in &mut b.children {
        fill_bounds(c);
    }
    if b.bounds.is_none() && b.polygon.is_empty() {
        let boxes: Vec<Rect> = b.children.iter().filter_map(OcrBlock::bbox).collect();
        if let Some(first) = boxes.first() {
            let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.width, first.y + first.height);
            for r in &boxes[1..] {
                x0 = x0.min(r.x);
                y0 = y0.min(r.y);
                x1 = x1.max(r.x + r.width);
                y1 = y1.max(r.y + r.height);
            }
            b.bounds = Some(Rect::from_corners(x0, y0, x1, y1));
        }
    }
}

/// Give a node without text its children's: a line's words joined by
/// spaces, anything else's parts a line each.
fn fill_text(b: &mut OcrBlock) {
    for c in &mut b.children {
        fill_text(c);
    }
    if b.text.trim().is_empty() && !b.children.is_empty() {
        let sep = if b.level == Some(OcrLevel::Line) { " " } else { "\n" };
        b.text = b.children.iter().map(|c| c.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(sep);
    }
}

/// Move a node (and its children) from a preprocessed copy's pixels onto
/// the original's through the copy's transform.
fn map_back(b: &mut OcrBlock, t: &wire::Transform) {
    let pt = |p: [f32; 2]| {
        let (x, y) = t.map_point(p[0] as f64, p[1] as f64);
        [x as f32, y as f32]
    };
    if let Some(corners) = b.corners() {
        let mapped = corners.map(pt);
        let (r, angle) = glossa_core::ocr::quad_geometry(&mapped);
        b.bounds = Some(r);
        b.angle = angle;
    }
    for p in &mut b.polygon {
        *p = pt(*p);
    }
    if let Some(base) = &mut b.baseline {
        base.offset *= t.scale() as f32;
    }
    for c in &mut b.children {
        map_back(c, t);
    }
}

fn node_from_wire(n: wire::OcrNode, lang_for: &dyn Fn(&str) -> Option<LangId>) -> OcrBlock {
    let finite = |v: f32| v.is_finite();
    let bounds = n
        .bounds
        .map(|r| Rect::new(r.x, r.y, r.width, r.height))
        .or_else(|| n.bbox.map(|[x0, y0, x1, y1]| Rect::from_corners(x0, y0, x1, y1)))
        .filter(|r| [r.x, r.y, r.width, r.height].into_iter().all(finite));
    let polygon: Vec<[f32; 2]> = n.polygon.into_iter().filter(|p| p.iter().copied().all(finite)).collect();
    let mut angle = if n.angle.is_finite() { n.angle } else { 0.0 };
    // An outline alone (what Cloud Vision and PAGE-XML give) is enough: the
    // box and rotation follow from it.
    let bounds = match (bounds, polygon.as_slice()) {
        (Some(b), _) => Some(b),
        (None, [a, b, c, d]) => {
            let (r, quad_angle) = glossa_core::ocr::quad_geometry(&[*a, *b, *c, *d]);
            angle = quad_angle;
            Some(r)
        }
        (None, _) => OcrBlock { polygon: polygon.clone(), ..Default::default() }.bbox(),
    };
    OcrBlock {
        text: n.text,
        bounds,
        angle,
        level: n.level.as_deref().and_then(|l| match l.to_ascii_lowercase().as_str() {
            "block" | "carea" | "region" => Some(OcrLevel::Block),
            "paragraph" | "par" => Some(OcrLevel::Paragraph),
            "line" => Some(OcrLevel::Line),
            "word" | "string" => Some(OcrLevel::Word),
            _ => None,
        }),
        polygon,
        confidence: n.confidence.filter(|c| c.is_finite()).map(|c| c.clamp(0.0, 1.0)),
        baseline: n
            .baseline
            .filter(|b| b.slope.is_finite() && b.offset.is_finite())
            .map(|b| Baseline { slope: b.slope, offset: b.offset }),
        lang: n.lang.filter(|l| !l.is_empty()).map(|c| lang_for(&c).map(|l| l.bcp47().to_owned()).unwrap_or(c)),
        direction: n.direction.as_deref().and_then(|d| match d.to_ascii_lowercase().as_str() {
            "ltr" => Some(WritingDirection::Ltr),
            "rtl" => Some(WritingDirection::Rtl),
            "ttb" | "vertical" => Some(WritingDirection::Ttb),
            _ => None,
        }),
        translation: n.translation.filter(|t| !t.trim().is_empty()),
        children: n.children.into_iter().map(|c| node_from_wire(c, lang_for)).collect(),
    }
}

/// Our result as a module would send it: the other direction of
/// [`ocr_from_wire`], for tools that pass results on to modules.
pub fn ocr_to_wire(r: &OcrResult) -> wire::RecognizeResult {
    fn node(b: &OcrBlock) -> wire::OcrNode {
        wire::OcrNode {
            text: b.text.clone(),
            level: b.level.map(|l| {
                match l {
                    OcrLevel::Block => "block",
                    OcrLevel::Paragraph => "paragraph",
                    OcrLevel::Line => "line",
                    OcrLevel::Word => "word",
                }
                .to_owned()
            }),
            bounds: b.bounds.map(|r| wire::Rect::new(r.x, r.y, r.width, r.height)),
            bbox: None,
            angle: b.angle,
            polygon: b.polygon.clone(),
            confidence: b.confidence,
            baseline: b.baseline.map(|x| wire::Baseline { slope: x.slope, offset: x.offset }),
            lang: b.lang.clone(),
            direction: b.direction.map(|d| {
                match d {
                    WritingDirection::Ltr => "ltr",
                    WritingDirection::Rtl => "rtl",
                    WritingDirection::Ttb => "ttb",
                }
                .to_owned()
            }),
            translation: b.translation.clone(),
            children: b.children.iter().map(node).collect(),
        }
    }
    wire::RecognizeResult {
        text: r.text.clone(),
        translation: r.translation.clone(),
        detected_lang: r.detected_lang.filter(|l| l.is_language()).map(|l| l.bcp47().to_owned()),
        blocks: r.blocks.iter().map(node).collect(),
        image_size: r.image_size.map(|(w, h)| [w, h]),
        ..Default::default()
    }
}

/// The pixel size of a PNG, JPEG or WebP from its header, without decoding
/// it: what a module's coordinates refer to when it did not say.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let le = |at: usize, n: usize| bytes.get(at..at + n).map(|b| b.iter().rev().fold(0u32, |a, &x| a << 8 | x as u32));
    let size = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR") {
        (be32(16)?, be32(20)?)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        match bytes.get(12..16)? {
            b"VP8X" => (le(24, 3)? + 1, le(27, 3)? + 1),
            b"VP8L" => {
                let v = le(21, 4)?;
                ((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1)
            }
            b"VP8 " => (le(26, 2)? & 0x3fff, le(28, 2)? & 0x3fff),
            _ => return None,
        }
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        // Walk the JPEG segments to the first start-of-frame.
        let mut at = 2;
        loop {
            while bytes.get(at) == Some(&0xff) && bytes.get(at + 1) == Some(&0xff) {
                at += 1;
            }
            if bytes.get(at) != Some(&0xff) {
                return None;
            }
            let marker = *bytes.get(at + 1)?;
            let len = u16::from_be_bytes([*bytes.get(at + 2)?, *bytes.get(at + 3)?]) as usize;
            if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                let h = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]) as u32;
                let w = u16::from_be_bytes([*bytes.get(at + 7)?, *bytes.get(at + 8)?]) as u32;
                break (w, h);
            }
            at = at.checked_add(2 + len)?;
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
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

    /// Answers `recognize` the way the option `mode` asks, checking what the
    /// host sent along the way.
    struct OcrEcho;

    impl Transport for OcrEcho {
        fn call(
            &self,
            method: &str,
            params: serde_json::Value,
            host: &dyn Fn(&str, serde_json::Value) -> Reply,
        ) -> std::result::Result<serde_json::Value, ModuleError> {
            assert_eq!(method, "recognize");
            assert_eq!(params["granularity"], "words");
            let handle = params["image_handle"].clone();
            match params["options"]["mode"].as_str().unwrap_or("") {
                "blocks" => Ok(serde_json::json!({
                    "blocks": [{"level": "line", "text": "hi", "bbox": [10, 10, 30, 20], "confidence": 1.5,
                                "lang": "ZH", "direction": "rtl",
                                "children": [{"text": "hi", "bounds": {"x": 10, "y": 10, "width": 20, "height": 10}}]}],
                    "transform": [2, 0, 0, 0, 2, 0],
                    "detected_lang": "ZH",
                })),
                "hocr" => Ok(serde_json::json!({
                    "hocr": "<div class='ocr_page' title='bbox 0 0 50 40'><span class='ocr_line' title='bbox 1 2 3 4'>x</span></div>",
                    "translation": "икс",
                })),
                "alto" => Ok(serde_json::json!({
                    "text": "own text",
                    "alto": "<alto><Layout><Page WIDTH='9' HEIGHT='9'><TextBlock ID='b'><TextLine><String CONTENT='y'/></TextLine></TextBlock></Page></Layout></alto>",
                })),
                "image" => {
                    let info = host("image.info", serde_json::json!({ "handle": handle })).into_result()?;
                    let gray = host("image.grayscale", serde_json::json!({ "handle": handle })).into_result()?;
                    let crop = host(
                        "image.crop",
                        serde_json::json!({ "handle": gray["handle"], "x": 2, "y": 1, "width": 4, "height": 2 }),
                    )
                    .into_result()?;
                    Ok(serde_json::json!({
                        "text": format!("{}x{}", info["width"], info["height"]),
                        "blocks": [{"text": "w", "bounds": {"x": 0, "y": 0, "width": 4, "height": 2}}],
                        "transform": crop["transform"],
                    }))
                }
                _ => Ok(serde_json::json!({ "text": "plain" })),
            }
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        // A PNG signature and IHDR are all the size sniffing needs.
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b
    }

    fn recognise(mode: &str, image: Vec<u8>) -> OcrResult {
        let p = ProtocolProvider::new(manifest(), OcrEcho, ModuleCache::new());
        let http = MockHttp::new();
        let mut opts = OptionValues::new();
        opts.insert("mode".into(), serde_json::json!(mode));
        let mut req = OcrRequest::new(glossa_core::ocr::ImageSource::Bytes(image));
        req.granularity = OcrGranularity::Words;
        let mut out = None;
        with_test_host(&http, &opts, |host| out = Some(p.recognize(host, &req).unwrap()));
        out.unwrap()
    }

    #[test]
    fn recognition_results_carry_structure_and_come_back_onto_the_input() {
        let r = recognise("plain", png(64, 32));
        assert_eq!(r.text, "plain");
        assert_eq!(r.image_size, Some((64, 32)), "sniffed from the input when the module did not say");

        let r = recognise("blocks", png(64, 32));
        assert_eq!(r.text, "hi", "joined from the blocks");
        let line = &r.blocks[0];
        assert_eq!(line.level, Some(OcrLevel::Line));
        assert_eq!(line.bounds, Some(Rect::new(20.0, 20.0, 40.0, 20.0)), "bbox corners, mapped back ×2");
        assert_eq!(line.children[0].bounds, Some(Rect::new(20.0, 20.0, 40.0, 20.0)));
        assert_eq!(line.children[0].level, Some(OcrLevel::Word), "inferred");
        assert_eq!(line.confidence, Some(1.0), "clamped");
        assert_eq!(line.lang.as_deref(), Some("zh-Hans"), "codes mapped back");
        assert_eq!(line.direction, Some(WritingDirection::Rtl));
        assert_eq!(r.detected_lang, LangId::from_code("zh-Hans"));
        assert_eq!(r.image_size, Some((64, 32)));

        let r = recognise("hocr", Vec::new());
        assert_eq!(r.text, "x");
        assert_eq!(r.image_size, Some((50, 40)));
        assert_eq!(r.translation.as_deref(), Some("икс"));
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(1.0, 2.0, 2.0, 2.0)));

        let r = recognise("alto", Vec::new());
        assert_eq!(r.text, "own text", "the module's own text wins");
        assert_eq!(r.blocks[0].children[0].children[0].text, "y");
    }

    #[cfg(feature = "image")]
    #[test]
    fn modules_preprocess_the_input_through_its_handle() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 4))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let r = recognise("image", bytes);
        assert_eq!(r.text, "8x4");
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(2.0, 1.0, 4.0, 2.0)), "the crop's offset undone");
        assert_eq!(r.image_size, Some((8, 4)));
    }

    #[test]
    fn wire_and_core_results_convert_both_ways() {
        let core = OcrResult {
            text: "a".into(),
            detected_lang: Some(LangId::ENGLISH),
            image_size: Some((10, 10)),
            blocks: vec![OcrBlock {
                bounds: Some(Rect::new(1.0, 2.0, 3.0, 4.0)),
                angle: 0.25,
                confidence: Some(0.5),
                baseline: Some(Baseline { slope: 0.1, offset: -2.0 }),
                direction: Some(WritingDirection::Ttb),
                translation: Some("б".into()),
                children: vec![OcrBlock::new(OcrLevel::Word, "a")],
                ..OcrBlock::new(OcrLevel::Line, "a")
            }],
            ..Default::default()
        };
        let wire = ocr_to_wire(&core);
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["blocks"][0]["level"], "line");
        assert_eq!(json["blocks"][0]["direction"], "ttb");
        let back = ocr_from_wire(serde_json::from_value(json).unwrap(), &LangId::from_code).unwrap();
        assert_eq!(back.blocks, core.blocks);
        assert_eq!(back.detected_lang, core.detected_lang);
        assert_eq!(back.image_size, core.image_size);
    }

    #[test]
    fn image_sizes_are_read_from_headers() {
        assert_eq!(image_dimensions(&png(640, 480)), Some((640, 480)));
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 30, 0, 40, 3];
        assert_eq!(image_dimensions(&jpeg), Some((40, 30)));
        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        vp8l.extend_from_slice(&((99u32) | (49u32 << 14)).to_le_bytes());
        assert_eq!(image_dimensions(&vp8l), Some((100, 50)));
        for junk in [&b""[..], b"\xff\xd8", b"\xff\xd8\xff\xe0\xff\xff", b"RIFF\0\0\0\0WEBPVP8X", b"GIF89a"] {
            assert_eq!(image_dimensions(junk), None);
        }
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
