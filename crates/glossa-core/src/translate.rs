//! What goes into a translation and what comes back.

use serde::{Deserialize, Serialize};

use crate::capability::Capabilities;
use crate::lang::{LangId, LangPair};

/// One translation job, as the engine hands it to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslateRequest {
    /// The text to translate, already cleaned up: `\n` line endings, hard
    /// wraps undone if the user asked for that, length capped.
    pub text: String,
    /// Where it is coming from. `AUTO` means the provider should work it out,
    /// or the engine should detect first if the provider cannot.
    pub from: LangId,
    /// Where it should end up. Never `AUTO`.
    pub to: LangId,
    /// Extra instructions for a provider that can take them (an LLM
    /// translator). Ignored by everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Terms that must come out a particular way, for a provider with
    /// [`Capabilities::PROMPTABLE`] or a real glossary feature.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glossary: Vec<GlossaryTerm>,
}

impl TranslateRequest {
    pub fn new(text: impl Into<String>, pair: LangPair) -> Self {
        Self { text: text.into(), from: pair.from, to: pair.to, style: None, glossary: Vec::new() }
    }

    pub fn pair(&self) -> LangPair {
        LangPair::new(self.from, self.to)
    }

    /// Whether there is anything to do. A request whose source and target are
    /// the same real language is a no-op, and so is an empty one.
    pub fn is_noop(&self) -> bool {
        self.text.trim().is_empty() || (self.from.is_language() && self.from == self.to)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryTerm {
    pub source: String,
    pub target: String,
}

/// What a provider produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Translation {
    /// The translated text.
    pub text: String,
    /// The language the provider says the input was in. Fills in a request
    /// that went out as `AUTO`.
    #[serde(default)]
    pub detected_from: Option<LangId>,
    /// The language the output is actually in, when the provider says so and
    /// it differs from what we asked for.
    #[serde(default)]
    pub actual_to: Option<LangId>,
    /// A romanisation of the source, where the provider offers one — the
    /// "Romanization:" line under a Japanese or Russian result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub romanisation: Option<String>,
    /// Alternative renderings, best first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    /// Per-part-of-speech alternatives — the "noun: ..." block a dictionary
    /// style answer carries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts_of_speech: Vec<PartOfSpeechEntry>,
    /// Which provider answered. Set by the engine, not the provider.
    #[serde(default)]
    pub provider_id: String,
    /// How long the whole call took, including retries.
    #[serde(default)]
    pub elapsed_ms: u64,
}

impl Translation {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Everything the provider gave us as one block of text, the way the
    /// classic output pane showed it: the translation, then the romanisation,
    /// then the part-of-speech lists.
    pub fn as_display_block(&self) -> String {
        let mut out = self.text.clone();
        if let Some(roman) = &self.romanisation {
            if !roman.trim().is_empty() {
                out.push_str("\n\n");
                out.push_str(roman);
            }
        }
        for entry in &self.parts_of_speech {
            out.push_str("\n\n");
            out.push_str(&entry.part_of_speech);
            out.push(':');
            for sense in &entry.senses {
                out.push_str("\n    ");
                out.push_str(&sense.word);
                if !sense.back_translations.is_empty() {
                    out.push_str(" (");
                    out.push_str(&sense.back_translations.join(", "));
                    out.push(')');
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartOfSpeechEntry {
    pub part_of_speech: String,
    pub senses: Vec<Sense>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sense {
    pub word: String,
    /// What this sense maps back to in the source language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back_translations: Vec<String>,
}

/// A detection result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub lang: LangId,
    /// 0.0 to 1.0 where the provider reports one, otherwise 1.0 for a bare
    /// answer.
    pub confidence: f32,
}

impl Detection {
    pub fn certain(lang: LangId) -> Self {
        Self { lang, confidence: 1.0 }
    }

    /// Whether the answer is worth acting on without asking the user. A hedged
    /// guess should not silently pick the wrong direction.
    pub fn is_usable(&self) -> bool {
        self.lang.is_language() && self.confidence >= 0.5
    }
}

/// What a provider says about itself: the runtime-independent half of a
/// module manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Stable, lowercase, hyphenated. Used in config files and on the command
    /// line.
    pub id: String,
    /// What the user sees.
    pub name: String,
    /// A sentence or two for the about box.
    #[serde(default)]
    pub description: String,
    /// The site behind it, for the "open in browser" action.
    #[serde(default)]
    pub homepage: String,
    pub capabilities: Capabilities,
    /// The languages it handles. Empty means "every language in the table".
    #[serde(default)]
    pub languages: Vec<LangId>,
    /// Longest input it accepts, in characters. `None` for no stated limit.
    #[serde(default)]
    pub max_input_chars: Option<usize>,
    /// Whether the provider needs a credential the user has to supply.
    #[serde(default)]
    pub needs_credentials: bool,
}

impl ProviderInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>, capabilities: Capabilities) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            homepage: String::new(),
            capabilities,
            languages: Vec::new(),
            max_input_chars: None,
            needs_credentials: false,
        }
    }

    pub fn supports_language(&self, lang: LangId) -> bool {
        if lang == LangId::AUTO {
            // Auto works if the provider can detect, or accepts `auto` itself.
            return self.capabilities.contains(Capabilities::DETECT_LANGUAGE)
                || self.languages.contains(&LangId::AUTO)
                || self.languages.is_empty();
        }
        self.languages.is_empty() || self.languages.contains(&lang)
    }

    pub fn supports(&self, capability: Capabilities) -> bool {
        self.capabilities.contains(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_requests_are_recognised() {
        let empty = TranslateRequest::new("   ", LangPair::new(LangId::AUTO, LangId::ENGLISH));
        assert!(empty.is_noop());
        let same = TranslateRequest::new("hi", LangPair::new(LangId::ENGLISH, LangId::ENGLISH));
        assert!(same.is_noop());
        // Auto to English is real work even though "auto" might turn out to
        // be English.
        let auto = TranslateRequest::new("hi", LangPair::new(LangId::AUTO, LangId::ENGLISH));
        assert!(!auto.is_noop());
    }

    #[test]
    fn display_block_matches_the_classic_layout() {
        let t = Translation {
            text: "привет".into(),
            romanisation: Some("privet".into()),
            parts_of_speech: vec![PartOfSpeechEntry {
                part_of_speech: "noun".into(),
                senses: vec![Sense {
                    word: "привет".into(), back_translations: vec!["hi".into(), "hello".into()]
                }],
            }],
            ..Default::default()
        };
        assert_eq!(t.as_display_block(), "привет\n\nprivet\n\nnoun:\n    привет (hi, hello)");
    }

    #[test]
    fn detection_threshold() {
        assert!(Detection::certain(LangId::RUSSIAN).is_usable());
        assert!(!Detection { lang: LangId::RUSSIAN, confidence: 0.2 }.is_usable());
        assert!(!Detection::certain(LangId::UNKNOWN).is_usable());
    }

    #[test]
    fn provider_language_support() {
        let mut p = ProviderInfo::new("x", "X", Capabilities::TRANSLATE);
        // No list means everything.
        assert!(p.supports_language(LangId::RUSSIAN));
        assert!(p.supports_language(LangId::AUTO));
        p.languages = vec![LangId::ENGLISH, LangId::RUSSIAN];
        assert!(p.supports_language(LangId::RUSSIAN));
        assert!(!p.supports_language(LangId(33)));
        // A list without auto, and no detection: auto is not supported.
        assert!(!p.supports_language(LangId::AUTO));
        p.capabilities |= Capabilities::DETECT_LANGUAGE;
        assert!(p.supports_language(LangId::AUTO));
    }
}
