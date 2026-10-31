//! Built-in translation, dictionary and speech services, in Rust.
//!
//! Each module here is one online service implementing
//! [`glossa_modules::Provider`]. They hold no global state and reach the
//! network only through the [`Host`](glossa_modules::Host) they are handed,
//! so the same code runs against the real client in the app and against
//! canned responses in the tests below each module.
//!
//! | id                  | service                                   | key        |
//! |---------------------|-------------------------------------------|------------|
//! | `google`            | Google Translate (+ detection, speech)    | none       |
//! | `google-dictionary` | Google Translate's dictionary data        | none       |
//! | `bing`              | Microsoft Translator                      | optional   |
//! | `deepl`             | DeepL API                                 | required   |
//! | `deeplx`            | DeepLX bridge (self-hosted)               | optional   |
//! | `libretranslate`    | LibreTranslate                            | optional   |
//! | `mymemory`          | MyMemory                                  | none       |
//! | `lingva`            | Lingva (Google proxy)                     | none       |
//! | `openai`            | Any OpenAI-compatible LLM                 | optional   |
//! | `yandex`            | Yandex Cloud Translate                    | required   |
//!
//! These are clean-room implementations written from the services' public
//! API documentation and observed response shapes; parsers are written to
//! degrade (fewer details, or [`Error::BadResponse`](glossa_core::Error)) when
//! a service changes its answer, never to panic.

use std::sync::Arc;

use glossa_modules::Provider;

mod common;

pub mod bing;
pub mod deepl;
pub mod deeplx;
pub mod google;
pub mod libretranslate;
pub mod lingva;
pub mod mymemory;
pub mod openai;
pub mod yandex;

pub use bing::Bing;
pub use deepl::DeepL;
pub use deeplx::DeepLx;
pub use google::{Google, GoogleDictionary};
pub use libretranslate::LibreTranslate;
pub use lingva::Lingva;
pub use mymemory::MyMemory;
pub use openai::OpenAi;
pub use yandex::Yandex;

/// Every built-in service, in the order the options page lists them: the
/// keyless ones first, so a fresh install has something that works.
pub fn builtin_providers() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(Google::new()),
        Arc::new(GoogleDictionary::new()),
        Arc::new(Bing::new()),
        Arc::new(DeepL::new()),
        Arc::new(DeepLx::new()),
        Arc::new(LibreTranslate::new()),
        Arc::new(MyMemory::new()),
        Arc::new(Lingva::new()),
        Arc::new(OpenAi::new()),
        Arc::new(Yandex::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::Capabilities;
    use glossa_modules::OptionKind;

    #[test]
    fn every_builtin_is_listed_once_with_a_sane_manifest() {
        let all = builtin_providers();
        let ids: Vec<&str> = all.iter().map(|p| p.info().id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "google",
                "google-dictionary",
                "bing",
                "deepl",
                "deeplx",
                "libretranslate",
                "mymemory",
                "lingva",
                "openai",
                "yandex"
            ]
        );
        for p in &all {
            let info = p.info();
            assert!(!info.name.is_empty() && !info.description.is_empty(), "{}", info.id);
            assert!(info.homepage.starts_with("https://"), "{}", info.id);
            assert!(!info.capabilities.is_empty(), "{}", info.id);
            assert!(info.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{}", info.id);
            assert!(info.languages.iter().all(|l| l.is_language() || *l == glossa_core::LangId::AUTO));
            // Option keys are unique, and selects default to one of their choices.
            let mut keys: Vec<&str> = p.options().iter().map(|o| o.key.as_str()).collect();
            keys.sort_unstable();
            let n = keys.len();
            keys.dedup();
            assert_eq!(n, keys.len(), "{}", info.id);
            for o in p.options() {
                if o.kind == OptionKind::Select {
                    assert!(o.choices.iter().any(|c| o.default == c.value.as_str()), "{}.{}", info.id, o.key);
                }
            }
            // A provider that needs a key declares a required option for it.
            if info.needs_credentials {
                assert!(p.options().iter().any(|o| o.required), "{}", info.id);
            }
        }
        let dict = &all[1];
        assert!(dict.info().capabilities.contains(Capabilities::DICTIONARY));
    }
}
