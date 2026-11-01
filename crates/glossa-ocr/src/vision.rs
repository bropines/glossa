//! Apple Vision: `VNRecognizeTextRequest`, the recogniser behind Live Text,
//! built into macOS 10.15 and later. Offline and free. The languages it reads
//! grow with the system (Russian and Ukrainian since macOS 13); the list is
//! asked of Vision at run time rather than kept here.
//!
//! With the picture's language on *auto*, macOS 13 and later detect it
//! (`automaticallyDetectsLanguage`); older systems read the configured list,
//! else English.
//!
//! Vision gives each line as a quadrilateral in fractions of the picture with
//! the origin at the bottom left; [`to_pixels`] turns that into the
//! top-left pixel quads the rest of the program uses, so rotated text keeps
//! its angle.
//!
//! The first recognition by a program file can take 25 seconds while macOS
//! prepares its text models for it (measured on an M4 with macOS 27: again
//! for a copy of the same binary at another path, and again after the Mac
//! restarts); later ones take a tenth of a second, in any process run from
//! that file. [`Provider::warm_up`] reads a tiny built-in picture once per
//! run, so that wait falls while the user is still choosing a region. It is
//! not done at start-up: the models it loads stay resident (35 to 50 MB),
//! which only a user of Vision should pay.
//!
//! An x86_64 build running under Rosetta only gets the fast recogniser: the
//! accurate one fails at once (`CRImageReaderError` 9, or no error at all),
//! even from a minimal Swift program. When accurate fails and fast reads the
//! picture, the provider stays on fast for the rest of the run. Fast reads
//! Latin scripts only, so Cyrillic and CJK need the native arm64 build there.
//!
//! The provider is compiled on every OS so its pure parts are tested
//! everywhere, but it is only registered on macOS.

use std::sync::atomic::{AtomicBool, Ordering};

use glossa_core::config::VisionConfig;
use glossa_core::{Capabilities, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, Provider};

use crate::layout::{Quad, RawLine, RawWord, Shape};

pub const VISION_ID: &str = "macos-vision";
const NAME: &str = "Apple Vision";

/// "Glossa" in dark grey on white, 92×61 pixels: enough text for Vision to
/// run its whole pipeline.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const WARM_UP_PNG: &[u8] = include_bytes!("../assets/vision-warm-up.png");

pub struct MacVision {
    info: ProviderInfo,
    cfg: VisionConfig,
    warmed_up: AtomicBool,
    /// Set once the accurate level failed where fast worked (Rosetta).
    accurate_broken: AtomicBool,
}

impl MacVision {
    pub fn new(cfg: VisionConfig) -> Self {
        let mut info = ProviderInfo::new(VISION_ID, NAME, Capabilities::OCR);
        info.description =
            "The text recognition built into macOS (Live Text). Offline; reads the languages your macOS version supports."
                .into();
        info.homepage = "https://developer.apple.com/documentation/vision/vnrecognizetextrequest".into();
        Self { info, cfg, warmed_up: AtomicBool::new(false), accurate_broken: AtomicBool::new(false) }
    }

    /// Reads the picture at the configured level, dropping to fast for good
    /// when only fast works on this system.
    #[cfg(target_os = "macos")]
    fn read(&self, bytes: &[u8], lang: LangId) -> Result<Vec<VisionLine>> {
        let fast =
            self.cfg.level == glossa_core::config::VisionLevel::Fast || self.accurate_broken.load(Ordering::Relaxed);
        let mut settings = sys::Settings {
            fast,
            language_correction: self.cfg.language_correction,
            lang,
            auto_languages: split_list(&self.cfg.auto_languages),
        };
        let accurate_error = match sys::recognize(bytes, &settings) {
            Err(e) if !fast => e,
            done => return done,
        };
        settings.fast = true;
        // The fast error says which languages fast reads; the note says why
        // only those, since upgrading macOS would not help.
        let accurate_error = match accurate_error {
            Error::Module { detail, .. } => detail,
            other => other.to_string(),
        };
        let lines = sys::recognize(bytes, &settings).map_err(|e| match e {
            Error::Module { module, detail } => Error::Module {
                module,
                detail: format!(
                    "{detail} The accurate recogniser failed here too ({accurate_error}); an Intel build under Rosetta only has the fast one, so use the native build."
                ),
            },
            other => other,
        })?;
        tracing::warn!("Vision: accurate recognition failed ({accurate_error}); using fast from now on");
        self.accurate_broken.store(true, Ordering::Relaxed);
        Ok(lines)
    }
}

impl Provider for MacVision {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn warm_up(&self) {
        let first = !self.warmed_up.swap(true, Ordering::Relaxed);
        #[cfg(not(target_os = "macos"))]
        let _ = first;
        #[cfg(target_os = "macos")]
        if first {
            let started = std::time::Instant::now();
            match self.read(WARM_UP_PNG, LangId::AUTO) {
                Ok(lines) => tracing::debug!(
                    lines = lines.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Vision warmed up"
                ),
                Err(e) => tracing::debug!("Vision warm-up failed: {e}"),
            }
        }
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        host.check_cancelled()?;
        #[cfg(target_os = "macos")]
        {
            let started = std::time::Instant::now();
            let bytes = req.image.to_bytes()?;
            let lines = self.read(&bytes, req.lang)?;
            let size = crate::imagesize::dimensions(&bytes);
            let raw: Vec<RawLine> = lines.iter().map(|l| l.to_raw(size)).collect();
            let mut out = crate::layout::build_result(&raw, req, size.unwrap_or_default());
            if size.is_none() {
                out.image_size = None;
            }
            // Confidence has no home in `OcrBlock` yet; `VisionLine` keeps it.
            tracing::debug!(
                lines = lines.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Vision read the picture"
            );
            out.provider_id = VISION_ID.into();
            out.elapsed_ms = started.elapsed().as_millis() as u64;
            Ok(out)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (req, &self.cfg);
            Err(Error::Unsupported { provider: NAME.into(), capability: "text recognition outside macOS" })
        }
    }
}

/// `en-US, ru-RU` or `en-US ru-RU` as a list.
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' ', ';', '+']).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// The identifiers from Vision's `supported` list to ask for when reading
/// `lang`, best first: an exact match, then those with the same language
/// (`ru` finds `ru-RU`). Chinese scripts do not stand in for each other, and
/// Cantonese (`yue-Hans`) is not Mandarin. Empty when Vision cannot read it.
pub fn pick_languages(lang: LangId, supported: &[String]) -> Vec<String> {
    if !lang.is_language() {
        return Vec::new();
    }
    let tag = lang.bcp47();
    let primary = |t: &str| t.split(['-', '_']).next().unwrap_or(t).to_ascii_lowercase();
    let want = primary(tag);
    let script = tag.split('-').find(|s| s.len() == 4).map(str::to_ascii_lowercase);
    let mut out: Vec<String> = supported.iter().filter(|s| s.eq_ignore_ascii_case(tag)).cloned().collect();
    for s in supported {
        if primary(s) != want || out.contains(s) {
            continue;
        }
        if let Some(script) = &script {
            if !s.to_ascii_lowercase().split(['-', '_']).any(|p| p == script) {
                continue;
            }
        }
        out.push(s.clone());
    }
    out
}

/// The message for a language this macOS cannot read.
pub fn unsupported_message(lang: LangId, supported: &[String]) -> String {
    let list = if supported.is_empty() { "unknown".to_owned() } else { supported.join(", ") };
    format!(
        "Apple Vision on this Mac does not read {} (it reads: {list}). Newer macOS versions add languages; \
         Russian and Ukrainian need macOS 13 or later.",
        lang.english_name()
    )
}

/// A point Vision gives (fractions of the picture, origin at the bottom
/// left) in pixels from the top left of a `size` picture.
pub fn to_pixels(x: f64, y: f64, size: (u32, u32)) -> (f32, f32) {
    ((x * size.0 as f64) as f32, ((1.0 - y) * size.1 as f64) as f32)
}

/// Four corners in Vision's terms, as the text sees them: top left, top
/// right, bottom right, bottom left.
pub type Corners = [(f64, f64); 4];

/// [`Corners`] as a pixel quad, clockwise from the text's top left.
pub fn corners_to_quad(c: &Corners, size: (u32, u32)) -> Quad {
    let mut q = [0.0; 8];
    for (i, &(x, y)) in c.iter().enumerate() {
        let (px, py) = to_pixels(x, y, size);
        q[2 * i] = px;
        q[2 * i + 1] = py;
    }
    q
}

/// The words of `text` with their ranges in UTF-16 code units, which is what
/// `NSRange` counts.
pub fn word_ranges_utf16(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut unit = 0usize;
    let mut start: Option<(usize, usize)> = None; // (byte, unit)
    for (byte, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some((b0, u0)) = start.take() {
                out.push((u0, unit - u0, &text[b0..byte]));
            }
        } else if start.is_none() {
            start = Some((byte, unit));
        }
        unit += c.len_utf16();
    }
    if let Some((b0, u0)) = start {
        out.push((u0, unit - u0, &text[b0..]));
    }
    out
}

/// A word Vision read, with its corners when Vision gave them.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionWord {
    pub text: String,
    pub corners: Option<Corners>,
}

/// A line Vision read.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionLine {
    pub text: String,
    /// 0–1.
    pub confidence: f32,
    pub corners: Corners,
    pub words: Vec<VisionWord>,
}

impl VisionLine {
    /// The line in the shape [`crate::layout`] builds results from; without
    /// the picture's size there is no geometry.
    pub fn to_raw(&self, size: Option<(u32, u32)>) -> RawLine {
        RawLine {
            text: self.text.clone(),
            shape: size.map(|s| Shape::Quad(corners_to_quad(&self.corners, s))),
            words: self
                .words
                .iter()
                .map(|w| RawWord {
                    text: w.text.clone(),
                    shape: size.zip(w.corners.as_ref()).map(|(s, c)| Shape::Quad(corners_to_quad(c, s))),
                    confidence: Some(self.confidence.clamp(0.0, 1.0)),
                })
                .collect(),
        }
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use super::*;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{AnyObject, NSObjectProtocol};
    use objc2::{sel, AnyThread};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSError, NSRange, NSString};
    use objc2_vision::{
        VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRectangleObservation, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    pub(super) struct Settings {
        pub fast: bool,
        pub language_correction: bool,
        pub lang: LangId,
        pub auto_languages: Vec<String>,
    }

    fn fail(what: &str, e: &NSError) -> Error {
        Error::Module { module: VISION_ID.into(), detail: format!("{what}: {}", e.localizedDescription()) }
    }

    fn strings(list: &[String]) -> Retained<NSArray<NSString>> {
        let items: Vec<Retained<NSString>> = list.iter().map(|s| NSString::from_str(s)).collect();
        NSArray::from_retained_slice(&items)
    }

    fn corners(o: &VNRectangleObservation) -> Corners {
        // SAFETY: plain property reads on a live observation.
        let (tl, tr, br, bl) = unsafe { (o.topLeft(), o.topRight(), o.bottomRight(), o.bottomLeft()) };
        [(tl.x, tl.y), (tr.x, tr.y), (br.x, br.y), (bl.x, bl.y)]
    }

    /// The languages the request can read at its current level.
    fn supported(request: &VNRecognizeTextRequest) -> Vec<String> {
        let list = if request.respondsToSelector(sel!(supportedRecognitionLanguagesAndReturnError:)) {
            // SAFETY: the selector exists (macOS 12 and later).
            unsafe { request.supportedRecognitionLanguagesAndReturnError() }.ok()
        } else {
            // macOS 10.15 and 11 only have the class method.
            // SAFETY: plain reads; the revision comes from the request itself.
            #[allow(deprecated)]
            unsafe {
                VNRecognizeTextRequest::supportedRecognitionLanguagesForTextRecognitionLevel_revision_error(
                    request.recognitionLevel(),
                    request.revision(),
                )
            }
            .ok()
        };
        list.map(|a| a.to_vec().iter().map(|s| s.to_string()).collect()).unwrap_or_default()
    }

    pub(super) fn recognize(bytes: &[u8], s: &Settings) -> Result<Vec<VisionLine>> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(if s.fast {
                VNRequestTextRecognitionLevel::Fast
            } else {
                VNRequestTextRecognitionLevel::Accurate
            });
            request.setUsesLanguageCorrection(s.language_correction);
            if s.lang.is_language() {
                let supported = supported(&request);
                let langs = if supported.is_empty() {
                    // Could not ask; let Vision judge the tag itself.
                    vec![s.lang.bcp47().to_owned()]
                } else {
                    pick_languages(s.lang, &supported)
                };
                if langs.is_empty() {
                    return Err(Error::Module {
                        module: VISION_ID.into(),
                        detail: unsupported_message(s.lang, &supported),
                    });
                }
                request.setRecognitionLanguages(&strings(&langs));
            } else {
                if !s.auto_languages.is_empty() {
                    request.setRecognitionLanguages(&strings(&s.auto_languages));
                }
                // An unknown selector would raise, so only on macOS 13+.
                if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
                    request.setAutomaticallyDetectsLanguage(true);
                }
            }

            let data = NSData::with_bytes(bytes);
            let options = NSDictionary::<VNImageOption, AnyObject>::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);
            let as_request: &VNRequest = &request;
            handler
                .performRequests_error(&NSArray::from_slice(&[as_request]))
                .map_err(|e| fail("recognition failed", &e))?;

            let mut lines = Vec::new();
            for obs in request.results().map(|r| r.to_vec()).unwrap_or_default() {
                let Some(best) = obs.topCandidates(1).to_vec().into_iter().next() else { continue };
                let text = best.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let words = word_ranges_utf16(&text)
                    .into_iter()
                    .map(|(start, len, word)| {
                        // SAFETY: the range lies inside the candidate's own string.
                        let rect = unsafe { best.boundingBoxForRange_error(NSRange::new(start, len)) }.ok();
                        VisionWord { text: word.to_owned(), corners: rect.as_deref().map(corners) }
                    })
                    .collect();
                lines.push(VisionLine { confidence: best.confidence(), corners: corners(&obs), text, words });
            }
            Ok(lines)
        })
    }
}

/// The languages Vision reads on this Mac at the accurate level, e.g.
/// `["en-US", "fr-FR", …, "ru-RU"]`. Empty elsewhere.
pub fn supported_languages() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        sys_supported()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn sys_supported() -> Vec<String> {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::NSObjectProtocol;
    use objc2::sel;
    use objc2_vision::VNRecognizeTextRequest;
    autoreleasepool(|_| {
        let request = VNRecognizeTextRequest::new();
        if request.respondsToSelector(sel!(supportedRecognitionLanguagesAndReturnError:)) {
            // SAFETY: the selector exists (macOS 12 and later).
            unsafe { request.supportedRecognitionLanguagesAndReturnError() }
                .map(|a| a.to_vec().iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::{ImageSource, OcrGranularity, Rect};

    /// What macOS 14 lists at the accurate level.
    fn sonoma() -> Vec<String> {
        [
            "en-US", "fr-FR", "it-IT", "de-DE", "es-ES", "pt-BR", "zh-Hans", "zh-Hant", "yue-Hans", "yue-Hant",
            "ko-KR", "ja-JP", "ru-RU", "uk-UA", "th-TH", "vi-VT", "ar-SA", "ars-SA",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn languages_are_matched_to_what_vision_reads() {
        let pick = |c: &str| pick_languages(LangId::from_code(c).unwrap(), &sonoma());
        assert_eq!(pick("en"), ["en-US"]);
        assert_eq!(pick("ru"), ["ru-RU"]);
        assert_eq!(pick("uk"), ["uk-UA"]);
        assert_eq!(pick("ja"), ["ja-JP"]);
        assert_eq!(pick("pt-BR"), ["pt-BR"]);
        assert_eq!(pick("zh-CN"), ["zh-Hans"], "not Cantonese, not Traditional");
        assert_eq!(pick("zh-TW"), ["zh-Hant"]);
        assert_eq!(pick("ar"), ["ar-SA"]);
        assert!(pick("pl").is_empty());
        assert!(pick_languages(LangId::AUTO, &sonoma()).is_empty());
        // Catalina listed only English.
        assert!(pick_languages(LangId::RUSSIAN, &["en-US".to_owned()]).is_empty());
    }

    #[test]
    fn an_unsupported_language_says_what_this_mac_reads() {
        let m = unsupported_message(LangId::RUSSIAN, &["en-US".into(), "fr-FR".into()]);
        assert!(m.contains("Russian") && m.contains("en-US, fr-FR") && m.contains("macOS 13"), "{m}");
    }

    #[test]
    fn lists_split_on_commas_and_spaces() {
        assert_eq!(split_list(" en-US, ru-RU ;ja-JP"), ["en-US", "ru-RU", "ja-JP"]);
        assert!(split_list("  ").is_empty());
    }

    #[test]
    fn vision_points_become_top_left_pixels() {
        assert_eq!(to_pixels(0.0, 0.0, (200, 100)), (0.0, 100.0), "bottom left");
        assert_eq!(to_pixels(1.0, 1.0, (200, 100)), (200.0, 0.0), "top right");
        assert_eq!(to_pixels(0.25, 0.75, (200, 100)), (50.0, 25.0));
    }

    #[test]
    fn a_line_becomes_a_level_box_and_words_keep_theirs() {
        // A line across the top half of a 200×100 picture: x 0.1–0.9, y 0.6–0.9.
        let line = VisionLine {
            text: "Hello world".into(),
            confidence: 0.5,
            corners: [(0.1, 0.9), (0.9, 0.9), (0.9, 0.6), (0.1, 0.6)],
            words: vec![
                VisionWord { text: "Hello".into(), corners: Some([(0.1, 0.9), (0.45, 0.9), (0.45, 0.6), (0.1, 0.6)]) },
                VisionWord { text: "world".into(), corners: None },
            ],
        };
        let mut req = OcrRequest::new(ImageSource::Bytes(Vec::new()));
        req.granularity = OcrGranularity::Words;
        let r = crate::layout::build_result(&[line.to_raw(Some((200, 100)))], &req, (200, 100));
        let b = r.blocks[0].bounds.unwrap();
        let close = |a: Rect, e: Rect| {
            (a.x - e.x).abs() < 1e-3
                && (a.y - e.y).abs() < 1e-3
                && (a.width - e.width).abs() < 1e-3
                && (a.height - e.height).abs() < 1e-3
        };
        assert!(close(b, Rect::new(20.0, 10.0, 160.0, 30.0)), "{b:?}");
        assert_eq!(r.blocks[0].angle, 0.0);
        let w = r.blocks[0].children[0].bounds.unwrap();
        assert!(close(w, Rect::new(20.0, 10.0, 70.0, 30.0)), "{w:?}");
        assert_eq!(r.blocks[0].children[1].bounds, None);
        assert_eq!(line.to_raw(None).shape, None, "no size, no geometry");
    }

    #[test]
    fn word_ranges_count_utf16_units() {
        let w = word_ranges_utf16("  Привет 𝔘nicode  мир");
        assert_eq!(w, [(2, 6, "Привет"), (9, 8, "𝔘nicode"), (19, 3, "мир")]);
        assert!(word_ranges_utf16("   ").is_empty());
        assert_eq!(word_ranges_utf16("你好"), [(0, 2, "你好")]);
    }

    #[test]
    fn the_warm_up_picture_is_a_small_png() {
        assert_eq!(crate::imagesize::dimensions(WARM_UP_PNG), Some((92, 61)));
        // Off macOS it only notes that it ran. (On a Mac every new test
        // binary would wait out Vision's 25-second first call.)
        if cfg!(not(target_os = "macos")) {
            let p = MacVision::new(VisionConfig::default());
            p.warm_up();
            assert!(p.warmed_up.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn describes_itself_as_an_ocr_provider() {
        let p = MacVision::new(VisionConfig::default());
        assert_eq!(p.info().id, VISION_ID);
        assert!(p.info().capabilities.contains(Capabilities::OCR));
    }

    /// Draws two lines and reads them with Vision. Run on a Mac:
    /// `cargo test -p glossa-ocr vision -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs macOS"]
    fn recognises_a_drawn_picture_live() {
        let png = crate::testimage::render_png(&["Hello world", "Привет мир"], 40.0);
        println!("supported: {:?}", supported_languages());
        let http = glossa_net::mock::MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        for lang in [LangId::AUTO, LangId::ENGLISH] {
            let mut req = OcrRequest::new(ImageSource::Bytes(png.clone()));
            req.lang = lang;
            req.granularity = OcrGranularity::Words;
            let r = glossa_modules::provider::with_test_host(&http, &opts, |h| {
                MacVision::new(VisionConfig::default()).recognize(h, &req)
            })
            .unwrap();
            println!("{lang:?}: {} ms\n{}", r.elapsed_ms, r.text);
            for b in &r.blocks {
                println!("  {:?} {:?} words {}", b.text, b.bounds, b.children.len());
            }
            assert!(r.text.contains("Hello"), "{}", r.text);
            assert!(r.blocks.iter().all(|b| b.bounds.is_some()));
        }
    }
}
