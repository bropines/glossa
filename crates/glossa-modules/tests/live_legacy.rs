//! Live checks against a real classic installation, when one is around.
//!
//! Ignored by default (they need the network and a classic `Services`
//! folder). Run with:
//!   GLOSSA_QT_SERVICES=/path/to/Qtranslate/Services cargo test -p glossa-modules --test live_legacy -- --ignored --nocapture

use std::path::PathBuf;

use glossa_core::{LangId, LangPair, TranslateRequest};
use glossa_modules::{Registry, RuntimeContext};
use glossa_net::HttpClient;

fn services_dir() -> Option<PathBuf> {
    std::env::var_os("GLOSSA_QT_SERVICES").map(PathBuf::from).filter(|p| p.is_dir())
}

#[test]
#[ignore]
fn classic_services_load_and_translate() {
    let Some(dir) = services_dir() else {
        eprintln!("GLOSSA_QT_SERVICES not set; skipping");
        return;
    };
    let mut reg = Registry::new();
    let report = reg.load_dir(&dir, &RuntimeContext::default());
    eprintln!("loaded {} classic services: {:?}", report.loaded.len(), report.loaded);
    for (path, why) in &report.failed {
        eprintln!("FAILED {}: {why}", path.display());
    }
    assert!(!report.loaded.is_empty());

    let http = HttpClient::plain();
    let opts = Default::default();
    for id in ["qt-google-translate", "qt-microsoft-translator", "qt-yandex", "qt-deeplx", "qt-lara"] {
        let Some(p) = reg.get(id) else { continue };
        let res = glossa_modules::provider::with_test_host(&http, &opts, |host| {
            p.translate(host, &TranslateRequest::new("Hello, world! How are you?", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
        });
        match res {
            Ok(t) => eprintln!("{id:28} OK   {:?}", t.text),
            Err(e) => eprintln!("{id:28} ERR  {e}"),
        }
    }
}
