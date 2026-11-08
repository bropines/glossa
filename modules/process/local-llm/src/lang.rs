//! Language codes as Glossa sends them, and the English names the models
//! are prompted with. Hy-MT2 was trained on 33 languages (its model card);
//! Qwen3 claims 119, which covers every language Glossa lists.

/// Glossa's BCP 47 codes and their English names. Simplified Chinese is
/// "Chinese", as Hy-MT2's model card spells its prompts.
const NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("ar", "Arabic"),
    ("az", "Azerbaijani"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("eo", "Esperanto"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fil", "Filipino"),
    ("fr", "French"),
    ("ga", "Irish"),
    ("gl", "Galician"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hmn", "Hmong"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("la", "Latin"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mk", "Macedonian"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("nb", "Norwegian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yi", "Yiddish"),
    ("zh-Hans", "Chinese"),
    ("zh-Hant", "Traditional Chinese"),
];

/// The languages on Hy-MT2's model card that Glossa knows (it lists Burmese,
/// Gujarati, Tibetan, Uyghur and Cantonese too, which Glossa does not).
pub const HY_MT2: &[&str] = &[
    "ar", "bn", "cs", "de", "en", "es", "fa", "fil", "fr", "he", "hi", "id", "it", "ja", "kk", "km", "ko", "mn", "mr",
    "ms", "nl", "pl", "pt", "ru", "ta", "te", "th", "tr", "uk", "ur", "vi", "zh-Hans", "zh-Hant",
];

/// The canonical code in [`NAMES`] for what Glossa (or a user's
/// `[language_map]`) sent: `zh`, `zh-CN` and `zh-TW`, `pt-BR`, `tl`, `iw`,
/// `no` all land somewhere sensible.
pub fn canonical(code: &str) -> Option<&'static str> {
    let code = code.trim();
    if let Some((c, _)) = NAMES.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)) {
        return Some(c);
    }
    let lower = code.to_ascii_lowercase();
    let base = lower.split(['-', '_']).next().unwrap_or("");
    let alias = match base {
        "zh" => {
            if lower.contains("hant") || lower.ends_with("-tw") || lower.ends_with("-hk") || lower.ends_with("-mo") {
                "zh-Hant"
            } else {
                "zh-Hans"
            }
        }
        "tl" => "fil",
        "iw" => "he",
        "no" | "nn" => "nb",
        "in" => "id",
        other => other,
    };
    NAMES.iter().find(|(c, _)| *c == alias).map(|(c, _)| *c)
}

/// The English name for a code, if it is a language this module knows.
pub fn name(code: &str) -> Option<&'static str> {
    let c = canonical(code)?;
    NAMES.iter().find(|(k, _)| *k == c).map(|(_, n)| *n)
}

/// The code for a language name a model answered with ("Russian",
/// "russian.", "Simplified Chinese", "Chinese (Traditional)").
pub fn from_name(answer: &str) -> Option<&'static str> {
    let first = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let clean: String =
        first.chars().map(|c| if c.is_alphanumeric() || c == ' ' { c.to_ascii_lowercase() } else { ' ' }).collect();
    let words: Vec<&str> = clean.split_whitespace().filter(|w| *w != "language" && *w != "the").collect();
    let joined = words.join(" ");
    if joined.contains("chinese") || joined == "mandarin" {
        return Some(if joined.contains("traditional") { "zh-Hant" } else { "zh-Hans" });
    }
    let alias = match joined.as_str() {
        "farsi" => "Persian",
        "tagalog" => "Filipino",
        "norwegian bokmal" | "bokmal" => "Norwegian",
        "slovene" => "Slovenian",
        "haitian" => "Haitian Creole",
        other => other,
    };
    NAMES.iter().find(|(_, n)| n.eq_ignore_ascii_case(alias)).map(|(c, _)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_resolve_with_their_aliases() {
        assert_eq!(name("ru"), Some("Russian"));
        assert_eq!(name("zh-Hans"), Some("Chinese"));
        assert_eq!(name("zh-TW"), Some("Traditional Chinese"));
        assert_eq!(name("zh"), Some("Chinese"));
        assert_eq!(name("pt-BR"), Some("Portuguese"));
        assert_eq!(name("tl"), Some("Filipino"));
        assert_eq!(name("iw"), Some("Hebrew"));
        assert_eq!(name("xx"), None);
        assert_eq!(name("auto"), None);
    }

    #[test]
    fn hy_mt2_languages_are_all_known() {
        for c in HY_MT2 {
            assert_eq!(canonical(c), Some(*c), "{c}");
        }
    }

    #[test]
    fn model_answers_map_to_codes() {
        assert_eq!(from_name("Russian"), Some("ru"));
        assert_eq!(from_name("  russian.\n"), Some("ru"));
        assert_eq!(from_name("The language is Japanese"), None);
        assert_eq!(from_name("Japanese language"), Some("ja"));
        assert_eq!(from_name("Simplified Chinese"), Some("zh-Hans"));
        assert_eq!(from_name("Chinese (Traditional)"), Some("zh-Hant"));
        assert_eq!(from_name("Farsi"), Some("fa"));
        assert_eq!(from_name("Klingon"), None);
        assert_eq!(from_name(""), None);
    }
}
