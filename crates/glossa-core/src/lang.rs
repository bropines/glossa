//! The canonical language table.
//!
//! Glossa identifies a language by a small integer, `LangId`. The numbering is
//! not ours: it is the one the classic QTranslate service scripts were written
//! against, where a script exposes a `SupportedLanguages` array and the host
//! indexes into it with the language id. Keeping the same numbering is what
//! lets an unmodified legacy `Service.js` run on this host (see
//! `docs/porting-qtranslate-services.md`).
//!
//! Two independent sources in the wild agree on the table — the Google service
//! script (which lists a code at nearly every slot) and the DeepL one (which
//! lists codes only where DeepL has a language, leaving `-1` elsewhere). Slot 0
//! is "unknown", slot 1 is "auto-detect", and slot 17 is English; everything
//! else follows from those two listings.
//!
//! New languages are appended at the end. A slot is never reused or renumbered,
//! because a script's array is positional.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A language, as a position in the canonical table.
///
/// `LangId` is deliberately cheap to copy and to store in a config file. Use
/// [`LangId::info`] to get at the names and codes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LangId(pub u16);

impl LangId {
    /// "I could not tell" — a failed detection, or a slot a service does not
    /// support.
    pub const UNKNOWN: LangId = LangId(0);
    /// "Work it out from the text."
    pub const AUTO: LangId = LangId(1);
    pub const ENGLISH: LangId = LangId(17);
    pub const RUSSIAN: LangId = LangId(46);

    /// Whether this is a real language, as opposed to `UNKNOWN` or `AUTO`.
    #[inline]
    pub fn is_language(self) -> bool {
        self.0 > Self::AUTO.0 && (self.0 as usize) < TABLE.len()
    }

    /// Whether the id names a slot that exists at all (including the two
    /// pseudo-languages).
    #[inline]
    pub fn is_known_slot(self) -> bool {
        (self.0 as usize) < TABLE.len()
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The row for this id, or the "unknown" row when the id is out of range.
    #[inline]
    pub fn info(self) -> &'static LangInfo {
        TABLE.get(self.index()).unwrap_or(&TABLE[0])
    }

    /// The code the legacy service scripts use (`iw` for Hebrew, `zh-CN` for
    /// Simplified Chinese, and so on). Kept exactly as those scripts expect.
    #[inline]
    pub fn legacy_code(self) -> &'static str {
        self.info().legacy_code
    }

    /// A modern BCP 47 tag, for talking to anything that is not a legacy
    /// script: `he`, `fil`, `zh-Hans`, `nb`.
    #[inline]
    pub fn bcp47(self) -> &'static str {
        self.info().bcp47
    }

    #[inline]
    pub fn english_name(self) -> &'static str {
        self.info().english_name
    }

    #[inline]
    pub fn native_name(self) -> &'static str {
        self.info().native_name
    }

    /// Whether the script runs right to left. Drives both UI alignment and the
    /// reordering in [`crate::text::for_display`].
    #[inline]
    pub fn is_rtl(self) -> bool {
        self.info().rtl
    }

    /// Whether the script is written without spaces between words, which
    /// changes how we count "length" and where we may truncate.
    #[inline]
    pub fn is_scriptio_continua(self) -> bool {
        self.info().continua
    }

    /// Look up by any code we recognise: the legacy code, the BCP 47 tag, or a
    /// close-enough variant (`he`, `zh`, `pt-BR`, `EN-GB`).
    pub fn from_code(code: &str) -> Option<LangId> {
        let needle = code.trim();
        if needle.is_empty() {
            return None;
        }
        if needle.eq_ignore_ascii_case("auto") {
            return Some(LangId::AUTO);
        }
        // Exact, case-insensitive, on either spelling.
        for (i, row) in TABLE.iter().enumerate() {
            if needle.eq_ignore_ascii_case(row.legacy_code) || needle.eq_ignore_ascii_case(row.bcp47)
            {
                return Some(LangId(i as u16));
            }
        }
        // `pt-BR` -> `pt`, `EN-GB` -> `en`. Chinese is excluded: its two slots
        // differ only by the subtag, so dropping it would lose the distinction.
        let base = needle.split(['-', '_']).next().unwrap_or(needle);
        if !base.eq_ignore_ascii_case("zh") {
            for (i, row) in TABLE.iter().enumerate() {
                if base.eq_ignore_ascii_case(row.legacy_code) || base.eq_ignore_ascii_case(row.bcp47)
                {
                    return Some(LangId(i as u16));
                }
            }
        } else {
            // Bare `zh`, or a script subtag we can map.
            let lower = needle.to_ascii_lowercase();
            return Some(if lower.contains("hant") || lower.contains("tw") || lower.contains("hk") {
                LangId(12)
            } else {
                LangId(11)
            });
        }
        None
    }

    /// Every real language, in table order.
    pub fn all() -> impl Iterator<Item = LangId> {
        (2..TABLE.len() as u16).map(LangId)
    }

    /// Every slot, including `UNKNOWN` and `AUTO`.
    pub fn all_slots() -> impl Iterator<Item = LangId> {
        (0..TABLE.len() as u16).map(LangId)
    }
}

impl Default for LangId {
    fn default() -> Self {
        LangId::AUTO
    }
}

impl fmt::Debug for LangId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LangId({}, {})", self.0, self.info().legacy_code)
    }
}

impl fmt::Display for LangId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.info().english_name)
    }
}

// Serialised as the bare number, so a config file written by one build still
// reads on the next even if we append languages.
impl Serialize for LangId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for LangId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Accept both the number and a code string, so a hand-edited config
        // may say `"from": "ru"`.
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LangId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a language id or code")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<LangId, E> {
                Ok(LangId(v as u16))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<LangId, E> {
                Ok(LangId(v.max(0) as u16))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LangId, E> {
                LangId::from_code(v)
                    .ok_or_else(|| E::custom(format!("unknown language code {v:?}")))
            }
        }
        d.deserialize_any(V)
    }
}

/// One row of the canonical table.
#[derive(Debug, Clone, Copy)]
pub struct LangInfo {
    /// What a legacy `SupportedLanguages` array calls it.
    pub legacy_code: &'static str,
    /// The modern tag to use everywhere else.
    pub bcp47: &'static str,
    pub english_name: &'static str,
    pub native_name: &'static str,
    pub rtl: bool,
    /// Written without word spaces (CJK, Thai, Lao, Khmer).
    pub continua: bool,
}

const fn row(
    legacy_code: &'static str,
    bcp47: &'static str,
    english_name: &'static str,
    native_name: &'static str,
) -> LangInfo {
    LangInfo { legacy_code, bcp47, english_name, native_name, rtl: false, continua: false }
}

const fn rtl(
    legacy_code: &'static str,
    bcp47: &'static str,
    english_name: &'static str,
    native_name: &'static str,
) -> LangInfo {
    LangInfo { legacy_code, bcp47, english_name, native_name, rtl: true, continua: false }
}

const fn cont(
    legacy_code: &'static str,
    bcp47: &'static str,
    english_name: &'static str,
    native_name: &'static str,
) -> LangInfo {
    LangInfo { legacy_code, bcp47, english_name, native_name, rtl: false, continua: true }
}

/// The table. **Append only** — see the module docs.
pub static TABLE: &[LangInfo] = &[
    /*  0 */ row("", "", "Unknown", ""),
    /*  1 */ row("auto", "auto", "Auto-detect", ""),
    /*  2 */ row("af", "af", "Afrikaans", "Afrikaans"),
    /*  3 */ row("az", "az", "Azerbaijani", "Azərbaycan"),
    /*  4 */ row("sq", "sq", "Albanian", "Shqip"),
    /*  5 */ rtl("ar", "ar", "Arabic", "العربية"),
    /*  6 */ row("hy", "hy", "Armenian", "Հայերեն"),
    /*  7 */ row("eu", "eu", "Basque", "Euskara"),
    /*  8 */ row("be", "be", "Belarusian", "Беларуская"),
    /*  9 */ row("bg", "bg", "Bulgarian", "Български"),
    /* 10 */ row("ca", "ca", "Catalan", "Català"),
    /* 11 */ cont("zh-CN", "zh-Hans", "Chinese (Simplified)", "简体中文"),
    /* 12 */ cont("zh-TW", "zh-Hant", "Chinese (Traditional)", "繁體中文"),
    /* 13 */ row("hr", "hr", "Croatian", "Hrvatski"),
    /* 14 */ row("cs", "cs", "Czech", "Čeština"),
    /* 15 */ row("da", "da", "Danish", "Dansk"),
    /* 16 */ row("nl", "nl", "Dutch", "Nederlands"),
    /* 17 */ row("en", "en", "English", "English"),
    /* 18 */ row("et", "et", "Estonian", "Eesti"),
    /* 19 */ row("fi", "fi", "Finnish", "Suomi"),
    /* 20 */ row("tl", "fil", "Filipino", "Filipino"),
    /* 21 */ row("fr", "fr", "French", "Français"),
    /* 22 */ row("gl", "gl", "Galician", "Galego"),
    /* 23 */ row("de", "de", "German", "Deutsch"),
    /* 24 */ row("el", "el", "Greek", "Ελληνικά"),
    /* 25 */ row("ht", "ht", "Haitian Creole", "Kreyòl ayisyen"),
    /* 26 */ rtl("iw", "he", "Hebrew", "עברית"),
    /* 27 */ row("hi", "hi", "Hindi", "हिन्दी"),
    /* 28 */ row("hu", "hu", "Hungarian", "Magyar"),
    /* 29 */ row("is", "is", "Icelandic", "Íslenska"),
    /* 30 */ row("id", "id", "Indonesian", "Bahasa Indonesia"),
    /* 31 */ row("it", "it", "Italian", "Italiano"),
    /* 32 */ row("ga", "ga", "Irish", "Gaeilge"),
    /* 33 */ cont("ja", "ja", "Japanese", "日本語"),
    /* 34 */ row("ka", "ka", "Georgian", "ქართული"),
    /* 35 */ cont("ko", "ko", "Korean", "한국어"),
    /* 36 */ row("lv", "lv", "Latvian", "Latviešu"),
    /* 37 */ row("lt", "lt", "Lithuanian", "Lietuvių"),
    /* 38 */ row("mk", "mk", "Macedonian", "Македонски"),
    /* 39 */ row("ms", "ms", "Malay", "Bahasa Melayu"),
    /* 40 */ row("mt", "mt", "Maltese", "Malti"),
    /* 41 */ row("no", "nb", "Norwegian", "Norsk"),
    /* 42 */ rtl("fa", "fa", "Persian", "فارسی"),
    /* 43 */ row("pl", "pl", "Polish", "Polski"),
    /* 44 */ row("pt", "pt", "Portuguese", "Português"),
    /* 45 */ row("ro", "ro", "Romanian", "Română"),
    /* 46 */ row("ru", "ru", "Russian", "Русский"),
    /* 47 */ row("sr", "sr", "Serbian", "Српски"),
    /* 48 */ row("sk", "sk", "Slovak", "Slovenčina"),
    /* 49 */ row("sl", "sl", "Slovenian", "Slovenščina"),
    /* 50 */ row("es", "es", "Spanish", "Español"),
    /* 51 */ row("sw", "sw", "Swahili", "Kiswahili"),
    /* 52 */ row("sv", "sv", "Swedish", "Svenska"),
    /* 53 */ cont("th", "th", "Thai", "ไทย"),
    /* 54 */ row("tr", "tr", "Turkish", "Türkçe"),
    /* 55 */ row("uk", "uk", "Ukrainian", "Українська"),
    /* 56 */ rtl("ur", "ur", "Urdu", "اردو"),
    /* 57 */ row("vi", "vi", "Vietnamese", "Tiếng Việt"),
    /* 58 */ row("cy", "cy", "Welsh", "Cymraeg"),
    /* 59 */ rtl("yi", "yi", "Yiddish", "ייִדיש"),
    /* 60 */ row("eo", "eo", "Esperanto", "Esperanto"),
    /* 61 */ row("hmn", "hmn", "Hmong", "Hmoob"),
    /* 62 */ row("la", "la", "Latin", "Latina"),
    /* 63 */ cont("lo", "lo", "Lao", "ລາວ"),
    /* 64 */ row("kk", "kk", "Kazakh", "Қазақ"),
    /* 65 */ row("uz", "uz", "Uzbek", "Oʻzbek"),
    /* 66 */ row("si", "si", "Sinhala", "සිංහල"),
    /* 67 */ row("tg", "tg", "Tajik", "Тоҷикӣ"),
    /* 68 */ row("te", "te", "Telugu", "తెలుగు"),
    /* 69 */ cont("km", "km", "Khmer", "ខ្មែរ"),
    /* 70 */ row("mn", "mn", "Mongolian", "Монгол"),
    /* 71 */ row("kn", "kn", "Kannada", "ಕನ್ನಡ"),
    /* 72 */ row("ta", "ta", "Tamil", "தமிழ்"),
    /* 73 */ row("mr", "mr", "Marathi", "मराठी"),
    /* 74 */ row("bn", "bn", "Bengali", "বাংলা"),
    /* 75 */ row("tt", "tt", "Tatar", "Татар"),
];

/// An ordered source → target pair, as the UI's direction selector holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LangPair {
    pub from: LangId,
    pub to: LangId,
}

impl LangPair {
    pub const fn new(from: LangId, to: LangId) -> Self {
        Self { from, to }
    }

    pub fn swapped(self) -> Self {
        Self { from: self.to, to: self.from }
    }

    /// A pair can be swapped only when both ends name a real language —
    /// swapping into `auto` as a target is meaningless.
    pub fn is_swappable(self) -> bool {
        self.from.is_language() && self.to.is_language()
    }
}

impl fmt::Display for LangPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} → {}", self.from.english_name(), self.to.english_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slots the legacy scripts hard-code. If one of these moves, every
    /// ported service breaks silently, so they are nailed down here.
    #[test]
    fn anchor_slots_match_the_legacy_numbering() {
        assert_eq!(TABLE[0].legacy_code, "");
        assert_eq!(TABLE[1].legacy_code, "auto");
        assert_eq!(LangId::ENGLISH.legacy_code(), "en");
        assert_eq!(LangId::RUSSIAN.legacy_code(), "ru");
        // Sampled against the DeepL script's sparse array, which only fills
        // slots it supports: 9 BG, 14 CS, 24 EL, 33 JA, 43 PL, 52 SV.
        assert_eq!(LangId(9).legacy_code(), "bg");
        assert_eq!(LangId(14).legacy_code(), "cs");
        assert_eq!(LangId(24).legacy_code(), "el");
        assert_eq!(LangId(33).legacy_code(), "ja");
        assert_eq!(LangId(43).legacy_code(), "pl");
        assert_eq!(LangId(52).legacy_code(), "sv");
    }

    #[test]
    fn codes_are_unique_per_spelling() {
        let mut legacy: Vec<&str> = TABLE[2..].iter().map(|r| r.legacy_code).collect();
        legacy.sort_unstable();
        let before = legacy.len();
        legacy.dedup();
        assert_eq!(before, legacy.len(), "duplicate legacy code in the table");

        let mut modern: Vec<&str> = TABLE[2..].iter().map(|r| r.bcp47).collect();
        modern.sort_unstable();
        let before = modern.len();
        modern.dedup();
        assert_eq!(before, modern.len(), "duplicate BCP 47 tag in the table");
    }

    #[test]
    fn lookup_accepts_both_spellings_and_variants() {
        assert_eq!(LangId::from_code("ru"), Some(LangId::RUSSIAN));
        assert_eq!(LangId::from_code("RU"), Some(LangId::RUSSIAN));
        // Hebrew: legacy `iw`, modern `he`.
        assert_eq!(LangId::from_code("iw"), Some(LangId(26)));
        assert_eq!(LangId::from_code("he"), Some(LangId(26)));
        // Filipino: legacy `tl`, modern `fil`.
        assert_eq!(LangId::from_code("tl"), Some(LangId(20)));
        assert_eq!(LangId::from_code("fil"), Some(LangId(20)));
        // Regional variants fall back to the base language.
        assert_eq!(LangId::from_code("pt-BR"), Some(LangId(44)));
        assert_eq!(LangId::from_code("EN-GB"), Some(LangId::ENGLISH));
        // Chinese keeps its two slots apart.
        assert_eq!(LangId::from_code("zh-CN"), Some(LangId(11)));
        assert_eq!(LangId::from_code("zh-Hans"), Some(LangId(11)));
        assert_eq!(LangId::from_code("zh-TW"), Some(LangId(12)));
        assert_eq!(LangId::from_code("zh-Hant"), Some(LangId(12)));
        assert_eq!(LangId::from_code("zh"), Some(LangId(11)));
        assert_eq!(LangId::from_code("auto"), Some(LangId::AUTO));
        assert_eq!(LangId::from_code("klingon"), None);
    }

    #[test]
    fn pseudo_languages_are_not_languages() {
        assert!(!LangId::UNKNOWN.is_language());
        assert!(!LangId::AUTO.is_language());
        assert!(LangId::ENGLISH.is_language());
        assert!(!LangId(9999).is_language());
        assert!(!LangId(9999).is_known_slot());
    }

    #[test]
    fn rtl_and_continua_flags_are_set_where_expected() {
        for code in ["ar", "iw", "fa", "ur", "yi"] {
            assert!(LangId::from_code(code).unwrap().is_rtl(), "{code} should be RTL");
        }
        for code in ["en", "ru", "ja"] {
            assert!(!LangId::from_code(code).unwrap().is_rtl(), "{code} should not be RTL");
        }
        for code in ["ja", "ko", "th", "zh-CN", "lo", "km"] {
            let id = LangId::from_code(code).unwrap();
            assert!(id.is_scriptio_continua(), "{code} has no word spaces");
        }
        assert!(!LangId::ENGLISH.is_scriptio_continua());
    }

    #[test]
    fn serde_round_trips_and_accepts_codes() {
        let json = serde_json::to_string(&LangId::RUSSIAN).unwrap();
        assert_eq!(json, "46");
        assert_eq!(serde_json::from_str::<LangId>("46").unwrap(), LangId::RUSSIAN);
        assert_eq!(serde_json::from_str::<LangId>("\"ru\"").unwrap(), LangId::RUSSIAN);
    }

    #[test]
    fn pairs_swap_only_between_real_languages() {
        let pair = LangPair::new(LangId::RUSSIAN, LangId::ENGLISH);
        assert!(pair.is_swappable());
        assert_eq!(pair.swapped(), LangPair::new(LangId::ENGLISH, LangId::RUSSIAN));
        assert!(!LangPair::new(LangId::AUTO, LangId::ENGLISH).is_swappable());
    }
}
