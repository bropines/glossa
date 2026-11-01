//! Text recognition providers.
//!
//! * [`lens::LensProvider`] (`lens`) — Google Lens through the user's
//!   chrome-lens-py: its warm daemon when one is running (found through the
//!   files it writes), else its `lens_scan` CLI. Recognises and, in the same
//!   round trip, translates.
//! * [`winocr::WindowsOcr`] (`windows-ocr`, Windows only) — the recogniser
//!   built into Windows 10 and 11, for the languages whose OCR pack is
//!   installed.
//! * [`oneocr::OneOcr`] (`oneocr`, Windows only) — the Windows 11 Snipping
//!   Tool's engine, loaded from the user's own installation.
//! * [`vision::MacVision`] (`macos-vision`, macOS only) — Apple Vision's
//!   `VNRecognizeTextRequest`, the recogniser behind Live Text.
//! * [`tesseract::Tesseract`] (`tesseract`) — the open-source recogniser,
//!   run as a program, on every OS where it is installed.
//! * [`command::CommandOcr`] (`command-ocr`) — any program the user names,
//!   run without a shell on a temporary copy of the image.
//!
//! All implement [`glossa_modules::Provider::recognize`] and nothing else.
//!
//! The Windows and macOS engines' OS calls live here rather than in
//! `glossa-platform` because the engine and the command line depend on this
//! crate and not on that one (which carries audio, hotkeys and screen
//! capture); they sit behind `cfg(windows)` / `cfg(target_os = "macos")` and
//! the providers are simply not registered elsewhere.
//! A Python module can be an OCR provider too (see `modules/python/lens-ocr`);
//! those are loaded by the module registry, not listed here.

pub mod command;
#[cfg(any(target_os = "macos", test))]
mod imagesize;
pub mod layout;
pub mod lens;
pub mod oneocr;
mod tempimage;
pub mod tesseract;
#[cfg(test)]
mod testimage;
pub mod vision;
#[cfg(windows)]
mod winimage;
pub mod winocr;

use std::path::Path;
use std::sync::Arc;

use glossa_core::config::OcrConfig;
use glossa_modules::child as process;
use glossa_modules::Provider;

pub use command::CommandOcr;
pub use lens::LensProvider;
pub use oneocr::OneOcr;
pub use tesseract::Tesseract;
pub use vision::MacVision;
pub use winocr::WindowsOcr;

/// The OCR providers compiled into the program, configured from `cfg`;
/// `cache_dir` is the program's cache folder (OneOCR copies its engine
/// there). Constructing them is cheap: nothing is looked up, loaded or
/// started until the first recognition.
///
/// The order is the fail-over order after the chosen engine: Lens, then the
/// system's own offline recogniser, then Tesseract if it is installed, then
/// a user's command. (The engine skips OneOCR when falling over.)
pub fn builtin_providers(cfg: &OcrConfig, cache_dir: &Path) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = vec![Arc::new(LensProvider::new(cfg.lens.clone()))];
    if cfg!(windows) {
        out.push(Arc::new(WindowsOcr::new()));
        out.push(Arc::new(OneOcr::new(cfg.oneocr.folder.clone(), cache_dir)));
    }
    if cfg!(target_os = "macos") {
        out.push(Arc::new(MacVision::new(cfg.vision.clone())));
    }
    out.push(Arc::new(Tesseract::new(cfg.tesseract.clone())));
    out.push(Arc::new(CommandOcr::new()));
    out
}

/// Whether a built-in OCR engine looks usable on this machine, for
/// `glossa-cli doctor`: found or not, and what was found or what is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    pub id: &'static str,
    pub available: bool,
    pub detail: String,
}

/// Look for each built-in engine without recognising anything. Runs
/// `tesseract --list-langs` and asks Windows for its OCR packs, so it takes
/// a moment; call it off the UI thread.
pub fn engine_status(cfg: &OcrConfig) -> Vec<EngineStatus> {
    let mut out = Vec::new();
    let lens = lens::Locator::system(&cfg.lens.cli_path);
    out.push(if !cfg.lens.endpoint.trim().is_empty() {
        EngineStatus { id: "lens", available: true, detail: format!("daemon at {}", cfg.lens.endpoint.trim()) }
    } else {
        match &lens.cli {
            Some(cli) => {
                EngineStatus { id: "lens", available: true, detail: format!("lens_scan at {}", cli.display()) }
            }
            None if lens.daemon_file.as_ref().is_some_and(|f| f.is_file()) => {
                EngineStatus { id: "lens", available: true, detail: "a chrome-lens-py daemon is announced".into() }
            }
            None => EngineStatus {
                id: "lens",
                available: false,
                detail: "chrome-lens-py not found (pip install \"chrome-lens-py[clipboard]\")".into(),
            },
        }
    });
    if cfg!(windows) {
        let langs = winocr::installed_languages();
        out.push(EngineStatus {
            id: "windows-ocr",
            available: !langs.is_empty(),
            detail: if langs.is_empty() {
                "no OCR language pack installed".into()
            } else {
                format!("languages: {}", langs.join(", "))
            },
        });
        out.push(EngineStatus {
            id: "oneocr",
            available: true,
            detail: "opt-in; uses the Snipping Tool's engine when it is installed".into(),
        });
    }
    if cfg!(target_os = "macos") {
        out.push(EngineStatus { id: "macos-vision", available: true, detail: "built into macOS".into() });
    }
    out.push(match tesseract::find_executable(&cfg.tesseract.path) {
        Some(exe) => {
            let langs = tesseract::installed_languages(&cfg.tesseract).unwrap_or_default();
            EngineStatus {
                id: "tesseract",
                available: true,
                detail: format!(
                    "{} (languages: {})",
                    exe.display(),
                    if langs.is_empty() { "?".into() } else { langs.join(", ") }
                ),
            }
        }
        None => EngineStatus { id: "tesseract", available: false, detail: tesseract::install_hint().into() },
    });
    out.push(EngineStatus {
        id: "command-ocr",
        available: true,
        detail: "runs the command set in its service options".into(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::Capabilities;

    #[test]
    fn every_builtin_engine_has_a_status() {
        let cfg = OcrConfig::default();
        let ids: Vec<String> =
            builtin_providers(&cfg, &std::env::temp_dir()).iter().map(|p| p.info().id.clone()).collect();
        let status: Vec<String> = engine_status(&cfg).iter().map(|s| s.id.to_owned()).collect();
        assert_eq!(status, ids);
        let missing = OcrConfig {
            tesseract: glossa_core::config::TesseractConfig {
                path: std::env::temp_dir().join("no-such-tesseract").to_string_lossy().into_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = engine_status(&missing).into_iter().find(|s| s.id == "tesseract").unwrap();
        assert!(!t.available && !t.detail.is_empty());
    }

    #[test]
    fn builtins_are_ocr_providers_with_stable_ids() {
        let providers = builtin_providers(&OcrConfig::default(), &std::env::temp_dir());
        let ids: Vec<&str> = providers.iter().map(|p| p.info().id.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(ids, ["lens", "windows-ocr", "oneocr", "tesseract", "command-ocr"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(ids, ["lens", "macos-vision", "tesseract", "command-ocr"]);
        } else {
            assert_eq!(ids, ["lens", "tesseract", "command-ocr"]);
        }
        assert!(providers.iter().all(|p| p.info().capabilities.contains(Capabilities::OCR)));
        assert_eq!(OcrConfig::default().provider, ids[0], "the default provider exists");
    }
}
