//! Dictionary lookups, and text-to-speech.
//!
//! A dictionary answer is structured, not a string: headword, pronunciations,
//! senses grouped by part of speech, example pairs, related forms. The shape
//! is deliberately the one pot-app's translate plugins return when they answer
//! like a dictionary, so such a plugin's result maps across field for field.
//!
//! Classic dictionary services answered with an HTML fragment instead. That is
//! kept as-is in [`Article::html`] and the UI renders it through a simplified
//! HTML reader; a provider may fill both.

use serde::{Deserialize, Serialize};

use crate::lang::LangId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictRequest {
    pub text: String,
    pub from: LangId,
    pub to: LangId,
    /// Match the headword exactly rather than accepting the provider's
    /// nearest suggestion — the dictionary window's "exact search" toggle.
    #[serde(default = "yes")]
    pub exact: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Article {
    /// The word the article is about, as the provider spells it (it may have
    /// normalised case or inflection).
    pub headword: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pronunciations: Vec<Pronunciation>,
    /// Senses, grouped by part of speech.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explanations: Vec<Explanation>,
    /// Usage examples with their translations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    /// Word forms, synonyms, "see also".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub associations: Vec<String>,
    /// A ready-made HTML article, from a provider that scrapes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// The page this came from, for "open in browser".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub elapsed_ms: u64,
}

impl Article {
    pub fn is_empty(&self) -> bool {
        self.explanations.iter().all(|e| e.definitions.is_empty())
            && self.examples.is_empty()
            && self.html.as_deref().map_or(true, |h| h.trim().is_empty())
    }

    /// A plain-text rendering, for the clipboard and the popup.
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.headword);
        for p in &self.pronunciations {
            if p.symbol.is_empty() {
                continue;
            }
            out.push_str("  ");
            if !p.region.is_empty() {
                out.push_str(&p.region);
                out.push(' ');
            }
            out.push('[');
            out.push_str(&p.symbol);
            out.push(']');
        }
        for e in &self.explanations {
            out.push_str("\n\n");
            if !e.part_of_speech.is_empty() {
                out.push_str(&e.part_of_speech);
                out.push('\n');
            }
            for (i, d) in e.definitions.iter().enumerate() {
                out.push_str(&format!("  {}. {}\n", i + 1, d));
            }
        }
        if !self.examples.is_empty() {
            out.push('\n');
            for ex in &self.examples {
                out.push_str("\n• ");
                out.push_str(&ex.source);
                if !ex.target.is_empty() {
                    out.push_str(" — ");
                    out.push_str(&ex.target);
                }
            }
        }
        out.trim_end().to_owned()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pronunciation {
    /// "UK", "US", or empty.
    #[serde(default)]
    pub region: String,
    /// IPA or the provider's own notation.
    #[serde(default)]
    pub symbol: String,
    /// Where the audio lives, when the provider gives a link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    /// "noun", "verb", or empty for an ungrouped list.
    #[serde(default, rename = "trait", alias = "part_of_speech")]
    pub part_of_speech: String,
    #[serde(default, rename = "explains", alias = "definitions")]
    pub definitions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub source: String,
    #[serde(default)]
    pub target: String,
}

/// A text-to-speech job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakRequest {
    pub text: String,
    pub lang: LangId,
    /// The classic "listen again, slower" — a second press within a few
    /// seconds of the first.
    #[serde(default)]
    pub slow: bool,
}

/// Encoded audio, ready for the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audio {
    pub bytes: Vec<u8>,
    /// `audio/mpeg`, `audio/wav`, `audio/ogg`.
    pub mime: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pot_style_json_deserialises() {
        // The field names pot-app plugins use: `trait` and `explains`.
        let json = r#"{
            "headword": "run",
            "pronunciations": [{ "region": "US", "symbol": "rʌn" }],
            "explanations": [{ "trait": "verb", "explains": ["move fast", "operate"] }],
            "examples": [{ "source": "run fast", "target": "бежать быстро" }]
        }"#;
        let a: Article = serde_json::from_str(json).unwrap();
        assert_eq!(a.explanations[0].part_of_speech, "verb");
        assert_eq!(a.explanations[0].definitions.len(), 2);
        assert!(!a.is_empty());
    }

    #[test]
    fn plain_text_rendering() {
        let a = Article {
            headword: "run".into(),
            pronunciations: vec![Pronunciation { region: "US".into(), symbol: "rʌn".into(), audio_url: None }],
            explanations: vec![Explanation { part_of_speech: "verb".into(), definitions: vec!["move fast".into()] }],
            examples: vec![Example { source: "run fast".into(), target: "бежать".into() }],
            ..Default::default()
        };
        assert_eq!(a.to_plain_text(), "run  US [rʌn]\n\nverb\n  1. move fast\n\n\n• run fast — бежать");
    }

    #[test]
    fn html_only_article_is_not_empty() {
        let a = Article { html: Some("<b>x</b>".into()), ..Default::default() };
        assert!(!a.is_empty());
        assert!(Article::default().is_empty());
    }
}
