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
//! * [`command::CommandOcr`] (`command-ocr`) — any program the user names,
//!   Tesseract being the usual one, run without a shell on a temporary copy
//!   of the image.
//!
//! All implement [`glossa_modules::Provider::recognize`] and nothing else.
//!
//! The Windows engines' OS calls live here rather than in `glossa-platform`
//! because the engine and the command line depend on this crate and not on
//! that one (which carries audio, hotkeys and screen capture); they sit
//! behind `cfg(windows)` and the providers are simply not registered
//! elsewhere.
//! A Python module can be an OCR provider too (see `modules/python/lens-ocr`);
//! those are loaded by the module registry, not listed here.

pub mod command;
pub mod layout;
pub mod lens;
pub mod oneocr;
mod process;
mod tempimage;
#[cfg(windows)]
mod winimage;
pub mod winocr;

use std::path::Path;
use std::sync::Arc;

use glossa_core::config::OcrConfig;
use glossa_modules::Provider;

pub use command::CommandOcr;
pub use lens::LensProvider;
pub use oneocr::OneOcr;
pub use winocr::WindowsOcr;

/// The OCR providers compiled into the program, configured from `cfg`;
/// `cache_dir` is the program's cache folder (OneOCR copies its engine
/// there). Constructing them is cheap: nothing is looked up, loaded or
/// started until the first recognition.
pub fn builtin_providers(cfg: &OcrConfig, cache_dir: &Path) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = vec![Arc::new(LensProvider::new(cfg.lens.clone()))];
    if cfg!(windows) {
        out.push(Arc::new(WindowsOcr::new()));
        out.push(Arc::new(OneOcr::new(cfg.oneocr.folder.clone(), cache_dir)));
    }
    out.push(Arc::new(CommandOcr::new()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::Capabilities;

    #[test]
    fn builtins_are_ocr_providers_with_stable_ids() {
        let providers = builtin_providers(&OcrConfig::default(), &std::env::temp_dir());
        let ids: Vec<&str> = providers.iter().map(|p| p.info().id.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(ids, ["lens", "windows-ocr", "oneocr", "command-ocr"]);
        } else {
            assert_eq!(ids, ["lens", "command-ocr"]);
        }
        assert!(providers.iter().all(|p| p.info().capabilities.contains(Capabilities::OCR)));
        assert_eq!(OcrConfig::default().provider, ids[0], "the default provider exists");
    }
}
