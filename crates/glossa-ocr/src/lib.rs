//! Text recognition providers.
//!
//! * [`lens::LensProvider`] (`lens`) — Google Lens through the user's
//!   chrome-lens-py: its warm daemon when one is running (found through the
//!   files it writes), else its `lens_scan` CLI. Recognises and, in the same
//!   round trip, translates.
//! * [`command::CommandOcr`] (`command-ocr`) — any program the user names,
//!   Tesseract being the usual one, run without a shell on a temporary copy
//!   of the image.
//!
//! Both implement [`glossa_modules::Provider::recognize`] and nothing else.
//! A Python module can be an OCR provider too (see `modules/python/lens-ocr`);
//! those are loaded by the module registry, not listed here.

pub mod command;
pub mod lens;
mod process;
mod tempimage;

use std::sync::Arc;

use glossa_core::config::OcrConfig;
use glossa_modules::Provider;

pub use command::CommandOcr;
pub use lens::LensProvider;

/// The OCR providers compiled into the program, configured from `cfg`.
/// Constructing them is cheap: nothing is looked up or started until the
/// first recognition.
pub fn builtin_providers(cfg: &OcrConfig) -> Vec<Arc<dyn Provider>> {
    vec![Arc::new(LensProvider::new(cfg.lens.clone())), Arc::new(CommandOcr::new())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::Capabilities;

    #[test]
    fn builtins_are_ocr_providers_with_stable_ids() {
        let providers = builtin_providers(&OcrConfig::default());
        let ids: Vec<&str> = providers.iter().map(|p| p.info().id.as_str()).collect();
        assert_eq!(ids, ["lens", "command-ocr"]);
        assert!(providers.iter().all(|p| p.info().capabilities.contains(Capabilities::OCR)));
        assert_eq!(OcrConfig::default().provider, ids[0], "the default provider exists");
    }
}
