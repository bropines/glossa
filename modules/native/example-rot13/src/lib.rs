//! An example native Glossa module: ROT13.
//!
//! It shows the whole author side of the native ABI in a few lines:
//! implement [`Module`] for a `Default` type, override the methods the
//! manifest's `capabilities` promise, and invoke [`export_module!`]. The
//! macro generates the `glossa_module_v1` entry point, the JSON plumbing and
//! the panic guard; nothing here touches a raw pointer.

use glossa_module_sdk::wire::*;
use glossa_module_sdk::{export_module, Host, Module};

/// Rotate ASCII letters by 13 places; everything else passes through.
pub fn rot13(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            other => other,
        })
        .collect()
}

/// The module. One instance lives as long as the library is loaded and the
/// host serialises calls, so `&mut self` state needs no locking.
#[derive(Default)]
pub struct Rot13 {
    calls: u64,
}

impl Module for Rot13 {
    fn translate(&mut self, host: &Host, req: TranslateParams) -> Result<TranslateResult, ModuleError> {
        self.calls += 1;
        // A host callback: goes to Glossa's log, tagged with the module id.
        host.log(LogLevel::Debug, &format!("rot13 call #{}: {} chars", self.calls, req.text.chars().count()));
        Ok(TranslateResult { detected_from: Some(req.from), ..TranslateResult::text(rot13(&req.text)) })
    }

    fn detect(&mut self, _host: &Host, _req: DetectParams) -> Result<DetectResult, ModuleError> {
        Ok(DetectResult { lang: "en".into(), confidence: 0.5 })
    }

    fn dictionary(&mut self, _host: &Host, req: DictionaryParams) -> Result<ArticleResult, ModuleError> {
        let word = req.text.trim();
        if word.is_empty() {
            return Err(ModuleError::new(ErrorKind::NotFound, "empty query"));
        }
        let rotated = rot13(word);
        Ok(ArticleResult {
            headword: word.to_owned(),
            pronunciations: vec![Pronunciation {
                region: "ROT13".into(),
                symbol: format!("/{rotated}/"),
                audio_url: None,
            }],
            explanations: vec![Explanation { part_of_speech: "cipher".into(), definitions: vec![rotated.clone()] }],
            examples: vec![Example { source: word.to_owned(), target: rotated }],
            associations: vec!["Caesar cipher".into()],
            ..Default::default()
        })
    }
}

export_module!(Rot13);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_is_its_own_inverse_and_leaves_the_rest_alone() {
        assert_eq!(rot13("Hello, World! 123 ж"), "Uryyb, Jbeyq! 123 ж");
        assert_eq!(rot13(&rot13("The Quick Brown Fox")), "The Quick Brown Fox");
    }

    #[test]
    fn the_exported_entry_point_speaks_protocol_one() {
        let table = glossa_module_v1();
        // SAFETY: the generated entry point returns a pointer to a static.
        let table = unsafe { &*table };
        assert_eq!(table.protocol_version, glossa_module_sdk::PROTOCOL_VERSION);
    }
}
