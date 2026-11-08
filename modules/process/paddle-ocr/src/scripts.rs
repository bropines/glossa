//! Which recogniser reads which language, and the module's options.
//!
//! PP-OCRv5 has one detector for every script and one recogniser per script
//! family. Each recogniser is an optional asset the user downloads; an
//! option per script lets them switch a downloaded one off.

use serde_json::Value;

/// One recogniser: the asset holding it and the option that switches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script {
    /// Short name, as in the asset (`rec-<id>.onnx`) and the option
    /// (`script_<id>`).
    pub id: &'static str,
    /// What it reads, for messages.
    pub label: &'static str,
}

impl Script {
    pub fn asset(&self) -> String {
        format!("rec-{}.onnx", self.id)
    }

    pub fn option(&self) -> String {
        format!("script_{}", self.id)
    }
}

pub const CH: Script = Script { id: "ch", label: "Chinese and Japanese" };
pub const ESLAV: Script = Script { id: "eslav", label: "Russian, Ukrainian and Belarusian" };
pub const CYRILLIC: Script = Script { id: "cyrillic", label: "Cyrillic" };
pub const KOREAN: Script = Script { id: "korean", label: "Korean" };
pub const LATIN: Script = Script { id: "latin", label: "Latin-script languages" };

/// Every recogniser, in the order `auto` tries them. Each also reads
/// English, so the order only matters for ties.
pub const ALL: [Script; 5] = [LATIN, ESLAV, CYRILLIC, CH, KOREAN];

/// The recognisers that can read `lang`, best first; empty for `auto` or a
/// language none of them is meant for (then every enabled one is tried).
pub fn for_language(lang: &str) -> Vec<Script> {
    let primary = lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "zh" | "ja" | "yue" | "lzh" => vec![CH],
        "ko" => vec![KOREAN],
        "ru" | "uk" | "be" => vec![ESLAV, CYRILLIC],
        "bg" | "sr" | "mk" | "kk" | "ky" | "mn" | "tg" | "tt" | "ba" | "cv" | "ce" | "os" | "sah" | "ab" | "av"
        | "kbd" | "lez" | "dar" | "inh" | "lbe" | "tab" => {
            // Serbian is also written in Latin letters; sr-Latn goes there.
            if lang.to_ascii_lowercase().contains("latn") {
                vec![LATIN]
            } else {
                vec![CYRILLIC, ESLAV]
            }
        }
        // English is in every recogniser; Latin first, then whatever is there.
        "en" => vec![LATIN, ESLAV, CYRILLIC, CH, KOREAN],
        "de" | "fr" | "es" | "it" | "pt" | "nl" | "pl" | "cs" | "sk" | "sl" | "hr" | "bs" | "hu" | "ro" | "sv"
        | "da" | "no" | "nb" | "nn" | "fi" | "et" | "lv" | "lt" | "is" | "ga" | "cy" | "eu" | "ca" | "gl" | "oc"
        | "af" | "sq" | "tr" | "az" | "uz" | "id" | "ms" | "tl" | "sw" | "la" | "mt" | "lb" | "rm" | "fo" | "vi"
        | "eo" | "ku" | "qu" | "mi" | "haw" | "sm" | "to" | "fj" | "so" | "ha" | "yo" | "ig" | "zu" | "xh" | "st"
        | "tn" | "sn" | "rw" | "ln" | "kg" | "lg" | "wo" | "ff" => vec![LATIN],
        _ => Vec::new(),
    }
}

/// The settings a request carries, with defaults for the missing ones.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Scripts the user left on (all by default).
    pub enabled: Vec<Script>,
    /// ONNX Runtime threads per model; 0 lets it choose.
    pub threads: usize,
    /// The longest side the detector looks at, in pixels.
    pub max_side: u32,
}

pub const DEFAULT_MAX_SIDE: u32 = 2000;

impl Settings {
    pub fn from_options(options: &Value) -> Self {
        let enabled = ALL.iter().copied().filter(|s| truthy(options.get(s.option()), true)).collect();
        let threads = number(options.get("threads")).map_or(0, |n| n.clamp(0.0, 64.0) as usize);
        let max_side = number(options.get("max_side")).map_or(DEFAULT_MAX_SIDE, |n| n.clamp(320.0, 8000.0) as u32);
        Self { enabled, threads, max_side }
    }
}

/// Options arrive as JSON from the settings page or `config.toml`; accept
/// the loose spellings a hand-edited file might use.
fn truthy(v: Option<&Value>, default: bool) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => !matches!(s.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off"),
        Some(Value::Null) | None => default,
        Some(_) => default,
    }
}

fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Which recognisers to run for `lang`, given what is downloaded.
///
/// - A language with recognisers of its own uses the first of them that is
///   downloaded, enabled or not: asking for Korean means Korean.
/// - `auto` (or a language none is meant for) runs every enabled,
///   downloaded one and keeps the best reading of each line.
///
/// `Err` says what to download.
pub fn choose(lang: &str, settings: &Settings, downloaded: &dyn Fn(&Script) -> bool) -> Result<Vec<Script>, Missing> {
    let wanted = for_language(lang);
    if !wanted.is_empty() {
        // English: any enabled recogniser will do.
        if lang.split(['-', '_']).next().is_some_and(|p| p.eq_ignore_ascii_case("en")) {
            if let Some(s) = wanted.iter().find(|s| settings.enabled.contains(s) && downloaded(s)) {
                return Ok(vec![*s]);
            }
        }
        if let Some(s) = wanted.iter().find(|s| downloaded(s)) {
            return Ok(vec![*s]);
        }
        return Err(Missing { language: Some(lang.to_owned()), scripts: vec![wanted[0]] });
    }
    let usable: Vec<Script> = settings.enabled.iter().copied().filter(|s| downloaded(s)).collect();
    if usable.is_empty() {
        let scripts = if settings.enabled.is_empty() { ALL.to_vec() } else { settings.enabled.clone() };
        return Err(Missing { language: None, scripts });
    }
    Ok(usable)
}

/// No recogniser for the request is downloaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Missing {
    pub language: Option<String>,
    pub scripts: Vec<Script>,
}

impl Missing {
    pub fn message(&self, module_id: &str) -> String {
        let names: Vec<String> = self.scripts.iter().map(|s| s.asset()).collect();
        let what = match &self.language {
            Some(lang) => format!("no recogniser for {lang:?} is downloaded"),
            None => "no recogniser is downloaded and switched on".to_owned(),
        };
        format!(
            "{what}: get {} ({}) in Options > Services > PaddleOCR, or run `glossa-cli assets download {module_id} {}`",
            names.join(" or "),
            self.scripts.iter().map(|s| s.label).collect::<Vec<_>>().join(", "),
            names[0],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn all_on() -> Settings {
        Settings::from_options(&json!({}))
    }

    #[test]
    fn languages_go_to_their_recogniser() {
        assert_eq!(for_language("ja"), vec![CH]);
        assert_eq!(for_language("zh-Hant"), vec![CH]);
        assert_eq!(for_language("ko"), vec![KOREAN]);
        assert_eq!(for_language("ru"), vec![ESLAV, CYRILLIC]);
        assert_eq!(for_language("bg"), vec![CYRILLIC, ESLAV]);
        assert_eq!(for_language("sr-Latn"), vec![LATIN]);
        assert_eq!(for_language("pt-BR"), vec![LATIN]);
        assert!(for_language("auto").is_empty());
        assert!(for_language("ar").is_empty(), "no Arabic recogniser is offered");
    }

    #[test]
    fn options_default_to_everything_on() {
        let s = all_on();
        assert_eq!(s.enabled.len(), ALL.len());
        assert_eq!(s.threads, 0);
        assert_eq!(s.max_side, DEFAULT_MAX_SIDE);
        let s = Settings::from_options(
            &json!({"script_ch": false, "script_korean": "off", "threads": "2", "max_side": 99999}),
        );
        assert!(!s.enabled.contains(&CH) && !s.enabled.contains(&KOREAN) && s.enabled.contains(&LATIN));
        assert_eq!(s.threads, 2);
        assert_eq!(s.max_side, 8000);
    }

    #[test]
    fn auto_runs_every_enabled_downloaded_recogniser() {
        let have = |s: &Script| *s == ESLAV || *s == CH;
        assert_eq!(choose("auto", &all_on(), &have).unwrap(), vec![ESLAV, CH]);
        let no_ch = Settings::from_options(&json!({"script_ch": false}));
        assert_eq!(choose("auto", &no_ch, &have).unwrap(), vec![ESLAV]);
    }

    #[test]
    fn a_named_language_uses_its_recogniser_even_when_switched_off() {
        let have = |s: &Script| *s == CH;
        let no_ch = Settings::from_options(&json!({"script_ch": false}));
        assert_eq!(choose("ja", &no_ch, &have).unwrap(), vec![CH]);
        assert_eq!(choose("ru", &all_on(), &|s: &Script| *s == CYRILLIC).unwrap(), vec![CYRILLIC]);
        // English prefers an enabled one.
        let only_latin_off = Settings::from_options(&json!({"script_latin": false}));
        let both = |s: &Script| *s == LATIN || *s == ESLAV;
        assert_eq!(choose("en", &only_latin_off, &both).unwrap(), vec![ESLAV]);
    }

    #[test]
    fn missing_recognisers_say_what_to_download() {
        let none = |_: &Script| false;
        let err = choose("ko", &all_on(), &none).unwrap_err();
        assert_eq!(err.scripts, vec![KOREAN]);
        let msg = err.message("paddle-ocr");
        assert!(
            msg.contains("rec-korean.onnx") && msg.contains("glossa-cli assets download paddle-ocr rec-korean.onnx"),
            "{msg}"
        );
        let err = choose("auto", &all_on(), &none).unwrap_err();
        assert!(err.language.is_none() && err.scripts.len() == ALL.len());
    }
}
