//! Windows OCR: the recogniser built into Windows 10 and 11
//! (`Windows.Media.Ocr`), offline and free. It reads one language at a time,
//! and only languages whose OCR pack is installed (Settings → Time &
//! language → Language & region → a language's options → Optical character
//! recognition). With the language on *auto* it uses the user's profile
//! languages.
//!
//! The provider is compiled on every OS so its pure parts are tested
//! everywhere, but it is only registered on Windows.

use glossa_core::{Capabilities, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, Provider};

pub const WINDOWS_OCR_ID: &str = "windows-ocr";
const NAME: &str = "Windows OCR";

pub struct WindowsOcr {
    info: ProviderInfo,
}

impl Default for WindowsOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsOcr {
    pub fn new() -> Self {
        let mut info = ProviderInfo::new(WINDOWS_OCR_ID, NAME, Capabilities::OCR);
        info.description =
            "The text recognition built into Windows 10 and 11. Offline; reads the languages whose OCR pack is installed."
                .into();
        info.homepage = "https://learn.microsoft.com/uwp/api/windows.media.ocr".into();
        Self { info }
    }
}

impl Provider for WindowsOcr {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        if host.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        #[cfg(windows)]
        {
            sys::recognize(req)
        }
        #[cfg(not(windows))]
        {
            let _ = req;
            Err(Error::Unsupported { provider: NAME.into(), capability: "text recognition outside Windows" })
        }
    }
}

/// The tags Windows OCR could be asked for, best first, for a language:
/// the full BCP 47 tag, then the bare language (Windows OCR packs are named
/// `en-US`, `zh-Hans-CN`, but `IsLanguageSupported` also accepts `en`, `zh-Hans`).
pub fn candidate_tags(lang: LangId) -> Vec<String> {
    let tag = lang.bcp47().to_owned();
    let mut out = vec![tag.clone()];
    if let Some((primary, _)) = tag.split_once('-') {
        // Chinese scripts are not interchangeable: keep `zh-Hans`/`zh-Hant`.
        if primary != "zh" {
            out.push(primary.to_owned());
        }
    }
    out
}

/// The message for a language with no OCR pack installed.
pub fn missing_pack_message(lang: LangId, installed: &[String]) -> String {
    let list = if installed.is_empty() { "none".to_owned() } else { installed.join(", ") };
    format!(
        "the Windows OCR pack for {} ({}) is not installed (installed: {list}). Add it in Settings → Time & language → \
         Language & region: add the language, then under its options install Optical character recognition. \
         An administrator can also list the packs with Get-WindowsCapability -Online -Name \"Language.OCR*\" \
         and add one with Add-WindowsCapability.",
        lang.english_name(),
        lang.bcp47(),
    )
}

#[cfg(windows)]
mod sys {
    use super::*;
    use crate::layout::{self, RawLine, RawWord, Shape};
    use crate::winimage::{self, Apartment};
    use glossa_core::ocr::Rect;
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    /// White border added around the picture; see [`layout::add_margin`].
    const MARGIN: u32 = 16;
    /// Pictures shorter than this are enlarged twice: Windows OCR misses
    /// text under about ten pixels high, which is what a capture of 8-pt
    /// interface text holds.
    const ENLARGE_BELOW: u32 = 40;

    fn fail(what: &str, e: windows::core::Error) -> Error {
        Error::Module {
            module: WINDOWS_OCR_ID.into(),
            detail: format!("{what}: {} (0x{:08X})", e.message(), e.code().0),
        }
    }

    pub(super) fn installed_languages() -> Vec<String> {
        crate::winimage::keep_mta();
        OcrEngine::AvailableRecognizerLanguages()
            .map(|v| v.into_iter().filter_map(|l| l.LanguageTag().ok()).map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    fn engine_for(lang: LangId) -> Result<OcrEngine> {
        if !lang.is_language() {
            return OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| Error::Module {
                module: WINDOWS_OCR_ID.into(),
                detail: format!(
                    "none of your Windows display languages has an OCR pack installed (installed: {}); \
                     choose the picture's language or install a pack in Settings → Time & language",
                    {
                        let l = installed_languages();
                        if l.is_empty() {
                            "none".to_owned()
                        } else {
                            l.join(", ")
                        }
                    }
                ),
            });
        }
        for tag in candidate_tags(lang) {
            let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag.as_str())) else { continue };
            if OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                    return Ok(engine);
                }
            }
        }
        Err(Error::Module { module: WINDOWS_OCR_ID.into(), detail: missing_pack_message(lang, &installed_languages()) })
    }

    pub(super) fn recognize(req: &OcrRequest) -> Result<OcrResult> {
        let _apartment = Apartment::enter();
        let bytes = req.image.to_bytes()?;
        let engine = engine_for(req.lang)?;
        let max = OcrEngine::MaxImageDimension().unwrap_or(0).saturating_sub(2 * MARGIN);
        let img = winimage::decode(&bytes, max, ENLARGE_BELOW, WINDOWS_OCR_ID)?;
        let (width, height, pixels) = layout::add_margin(img.width, img.height, &img.pixels, MARGIN);
        let writer = DataWriter::new().map_err(|e| fail("cannot prepare the image", e))?;
        writer.WriteBytes(&pixels).map_err(|e| fail("cannot prepare the image", e))?;
        let buffer = writer.DetachBuffer().map_err(|e| fail("cannot prepare the image", e))?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
            BitmapAlphaMode::Premultiplied,
        )
        .map_err(|e| fail("cannot prepare the image", e))?;
        let result =
            engine.RecognizeAsync(&bitmap).and_then(|op| op.join()).map_err(|e| fail("recognition failed", e))?;
        let mut lines = Vec::new();
        for line in result.Lines().map_err(|e| fail("cannot read the result", e))? {
            let words = line
                .Words()
                .map(|ws| {
                    ws.into_iter()
                        .map(|w| RawWord {
                            text: w.Text().map(|t| t.to_string()).unwrap_or_default(),
                            shape: w.BoundingRect().ok().map(|r| Shape::Rect(Rect::new(r.X, r.Y, r.Width, r.Height))),
                            // Windows.Media.Ocr reports no confidence.
                            confidence: None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            lines.push(RawLine { text: line.Text().map(|t| t.to_string()).unwrap_or_default(), shape: None, words });
        }
        let mut out = layout::build_result(&lines, req, img.source);
        layout::unmap(&mut out.blocks, MARGIN as f32, img.scale);
        Ok(out)
    }
}

/// The language tags with an OCR pack installed, e.g. `["en-US", "ru-RU"]`.
/// Empty outside Windows.
pub fn installed_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        let _apartment = crate::winimage::Apartment::enter();
        sys::installed_languages()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_fall_back_to_the_bare_language_but_not_across_chinese_scripts() {
        let en = LangId::from_code("en").unwrap();
        assert_eq!(candidate_tags(en), ["en"]);
        let pt = LangId::from_code("pt-BR").unwrap();
        assert_eq!(candidate_tags(pt).first().map(String::as_str), Some(pt.bcp47()));
        if pt.bcp47().contains('-') {
            assert_eq!(candidate_tags(pt).last().map(String::as_str), Some("pt"));
        }
        let zh = LangId::from_code("zh-CN").unwrap();
        assert!(candidate_tags(zh).iter().all(|t| t.starts_with("zh-")), "{:?}", candidate_tags(zh));
    }

    #[test]
    fn a_missing_pack_says_what_is_installed_and_how_to_add_it() {
        let ru = LangId::from_code("ru").unwrap();
        let m = missing_pack_message(ru, &["en-US".into(), "de-DE".into()]);
        assert!(m.contains("Russian") && m.contains("en-US, de-DE") && m.contains("Language.OCR*"), "{m}");
        assert!(missing_pack_message(ru, &[]).contains("installed: none"));
    }

    #[test]
    fn describes_itself_as_an_ocr_provider() {
        let p = WindowsOcr::new();
        assert_eq!(p.info().id, WINDOWS_OCR_ID);
        assert!(p.info().capabilities.contains(Capabilities::OCR));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs Windows with an OCR language pack"]
    fn lists_installed_languages() {
        assert!(!installed_languages().is_empty());
    }

    /// `GLOSSA_OCR_SAMPLE=picture.png`: recognise it with word geometry.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs Windows with an OCR language pack and GLOSSA_OCR_SAMPLE"]
    fn recognises_a_sample_live() {
        use glossa_core::ocr::{ImageSource, OcrGranularity};
        let path = std::env::var_os("GLOSSA_OCR_SAMPLE").expect("set GLOSSA_OCR_SAMPLE");
        let mut req = OcrRequest::new(ImageSource::Path(path.into()));
        req.granularity = OcrGranularity::Words;
        let http = glossa_net::mock::MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        let started = std::time::Instant::now();
        let r =
            glossa_modules::provider::with_test_host(&http, &opts, |h| WindowsOcr::new().recognize(h, &req)).unwrap();
        println!("{} ms, installed {:?}\n{}", started.elapsed().as_millis(), installed_languages(), r.text);
        for line in &r.blocks {
            println!("  {:?} words {}", line.bounds, line.children.len());
        }
        assert!(!r.text.trim().is_empty());
        assert!(r.blocks.iter().all(|b| b.bounds.is_some()));
    }
}
