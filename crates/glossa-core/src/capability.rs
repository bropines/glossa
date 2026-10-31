//! What a provider can do.
//!
//! The four bits are the ones the legacy service scripts declare, with the
//! same values, so a ported script's `Capability.TRANSLATE | ...` expression
//! carries over unchanged. The rest of the set is ours.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A set of capabilities, stored as a bit mask.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(pub u32);

impl Capabilities {
    // -- legacy-compatible bits (do not renumber) --------------------------
    /// Turn text in one language into text in another.
    pub const TRANSLATE: Capabilities = Capabilities(1);
    /// Say what language a piece of text is in.
    pub const DETECT_LANGUAGE: Capabilities = Capabilities(2);
    /// Produce audio for a piece of text.
    pub const LISTEN: Capabilities = Capabilities(4);
    /// Return a dictionary article rather than a translation.
    pub const DICTIONARY: Capabilities = Capabilities(8);

    // -- Glossa additions --------------------------------------------------
    /// Read text out of an image.
    pub const OCR: Capabilities = Capabilities(16);
    /// Turn speech into text.
    pub const SPEECH_INPUT: Capabilities = Capabilities(32);
    /// Translate a batch in one call more cheaply than one at a time.
    pub const BATCH: Capabilities = Capabilities(64);
    /// Offer alternative renderings of the same input.
    pub const ALTERNATIVES: Capabilities = Capabilities(128);
    /// Accept a glossary or a style instruction (LLM translators).
    pub const PROMPTABLE: Capabilities = Capabilities(256);
    /// Works with no network at all.
    pub const OFFLINE: Capabilities = Capabilities(512);

    pub const NONE: Capabilities = Capabilities(0);

    #[inline]
    pub const fn contains(self, other: Capabilities) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn union(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 | other.0)
    }

    #[inline]
    pub const fn without(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 & !other.0)
    }

    /// The individual bits that are set, for display.
    pub fn names(self) -> Vec<&'static str> {
        const ALL: &[(Capabilities, &str)] = &[
            (Capabilities::TRANSLATE, "translate"),
            (Capabilities::DETECT_LANGUAGE, "detect_language"),
            (Capabilities::LISTEN, "listen"),
            (Capabilities::DICTIONARY, "dictionary"),
            (Capabilities::OCR, "ocr"),
            (Capabilities::SPEECH_INPUT, "speech_input"),
            (Capabilities::BATCH, "batch"),
            (Capabilities::ALTERNATIVES, "alternatives"),
            (Capabilities::PROMPTABLE, "promptable"),
            (Capabilities::OFFLINE, "offline"),
        ];
        ALL.iter().filter(|(bit, _)| self.contains(*bit)).map(|(_, name)| *name).collect()
    }

    /// Parse one capability name as a manifest writes it.
    pub fn from_name(name: &str) -> Option<Capabilities> {
        Some(match name {
            "translate" => Self::TRANSLATE,
            "detect_language" | "detect" => Self::DETECT_LANGUAGE,
            "listen" | "tts" => Self::LISTEN,
            "dictionary" | "dict" => Self::DICTIONARY,
            "ocr" => Self::OCR,
            "speech_input" | "stt" => Self::SPEECH_INPUT,
            "batch" => Self::BATCH,
            "alternatives" => Self::ALTERNATIVES,
            "promptable" => Self::PROMPTABLE,
            "offline" => Self::OFFLINE,
            _ => return None,
        })
    }
}

impl std::ops::BitOr for Capabilities {
    type Output = Capabilities;
    fn bitor(self, rhs: Capabilities) -> Capabilities {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for Capabilities {
    fn bitor_assign(&mut self, rhs: Capabilities) {
        self.0 |= rhs.0;
    }
}

impl FromIterator<Capabilities> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capabilities>>(iter: I) -> Self {
        iter.into_iter().fold(Capabilities::NONE, Capabilities::union)
    }
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capabilities({})", self.names().join("|"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bit_values_are_frozen() {
        // A ported script computes `1|2|4` and the host must read it back the
        // same way.
        assert_eq!(Capabilities::TRANSLATE.0, 1);
        assert_eq!(Capabilities::DETECT_LANGUAGE.0, 2);
        assert_eq!(Capabilities::LISTEN.0, 4);
        assert_eq!(Capabilities::DICTIONARY.0, 8);
    }

    #[test]
    fn contains_is_about_every_requested_bit() {
        let set = Capabilities::TRANSLATE | Capabilities::LISTEN;
        assert!(set.contains(Capabilities::TRANSLATE));
        assert!(set.contains(Capabilities::LISTEN));
        assert!(set.contains(Capabilities::TRANSLATE | Capabilities::LISTEN));
        assert!(!set.contains(Capabilities::DICTIONARY));
        assert!(!set.contains(Capabilities::TRANSLATE | Capabilities::DICTIONARY));
        // The empty set is contained in everything, including itself.
        assert!(set.contains(Capabilities::NONE));
        assert!(Capabilities::NONE.is_empty());
    }

    #[test]
    fn names_round_trip_through_from_name() {
        let all = Capabilities(0b11_1111_1111);
        for name in all.names() {
            let bit = Capabilities::from_name(name).expect("name should parse");
            assert!(all.contains(bit));
        }
        assert_eq!(Capabilities::from_name("teleport"), None);
        // Aliases a manifest may use.
        assert_eq!(Capabilities::from_name("dict"), Some(Capabilities::DICTIONARY));
        assert_eq!(Capabilities::from_name("tts"), Some(Capabilities::LISTEN));
    }

    #[test]
    fn without_clears_only_the_named_bits() {
        let set = Capabilities::TRANSLATE | Capabilities::LISTEN | Capabilities::OCR;
        let trimmed = set.without(Capabilities::LISTEN);
        assert!(trimmed.contains(Capabilities::TRANSLATE | Capabilities::OCR));
        assert!(!trimmed.contains(Capabilities::LISTEN));
    }
}
