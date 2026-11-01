//! The system voice (`system-tts`): text to speech with the voices the
//! operating system already has, offline.
//!
//! * Windows: WinRT `Windows.Media.SpeechSynthesis` ([`win`]).
//! * macOS: `/usr/bin/say`, writing a WAV file ([`mac`]).
//! * Linux and the BSDs: `espeak-ng` (or `espeak`) with WAV on stdout
//!   ([`linux`]). Without either, the provider says it is not installed and
//!   is not switched on.
//!
//! Every backend answers with WAV. The voice is chosen by the request's
//! language; when no installed voice speaks it the call fails (so the engine
//! falls over to the next speech service) rather than reading the text with
//! a voice for another language. A request without a language uses the
//! system's default voice.
//!
//! The OS calls live here, behind `cfg`, for the same reason the OS OCR
//! engines live in `glossa-ocr`: the engine depends on this crate, not on
//! `glossa-platform`. `mac.rs` and `linux.rs` only run programs, so they are
//! compiled (and their logic tested) everywhere; `win.rs` is Windows only.

use std::time::Duration;

use glossa_core::{Audio, Capabilities, Error, LangId, ProviderInfo, Result, SpeakRequest};
use glossa_modules::{Host, Provider};

#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
pub mod linux;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod mac;
#[cfg(windows)]
mod win;

pub const SYSTEM_TTS_ID: &str = "system-tts";
const NAME: &str = "System voice";

/// Longer text is cut (at a space when there is one) before it is spoken:
/// about ten minutes of speech, and well inside the limits on a command
/// line's length.
pub const MAX_CHARS: usize = 10_000;

/// Speaking rate for "listen again, slower": a fraction of normal on
/// Windows, words per minute for `say` and espeak (whose normal is about 175).
#[cfg_attr(not(windows), allow(dead_code))]
const SLOW_RATE: f64 = 0.6;
const SLOW_WPM: u32 = 110;

/// One installed voice, as the OS lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    /// What the OS takes back to select it: a WinRT voice id, a `say` voice
    /// name, an espeak voice.
    pub id: String,
    /// What a person reads.
    pub name: String,
    /// Its language: `en-US`, `ru_RU`, `cmn`.
    pub tag: String,
    /// Picked only when nothing better speaks the language: 1 for robotic
    /// voices, 2 for novelty ones (macOS's Bubbles, Zarvox).
    pub penalty: u8,
}

/// The language a voice's tag names, if it is one Glossa knows. Accepts `_`
/// for `-` and espeak's `cmn` (Mandarin) and `yue` (Cantonese).
pub fn voice_language(tag: &str) -> Option<LangId> {
    let tag = tag.trim().replace('_', "-");
    let primary = tag.split('-').next().unwrap_or("");
    if primary.eq_ignore_ascii_case("cmn") {
        // `cmn-latn-pinyin` reads pinyin, not characters.
        return (!tag.to_ascii_lowercase().contains("latn")).then(|| LangId::from_code("zh-Hans")).flatten();
    }
    if primary.eq_ignore_ascii_case("yue") {
        return LangId::from_code("zh-Hant");
    }
    LangId::from_code(&tag).filter(|l| l.is_language())
}

/// The region a language is most often spoken in, for choosing between
/// voices of one language (`en-US` over `en-IN`).
fn home_region(lang: LangId) -> String {
    let primary = lang.bcp47().split('-').next().unwrap_or("");
    let region = match lang.bcp47() {
        "zh-Hans" => "CN",
        "zh-Hant" => "TW",
        "en" => "US",
        "ja" => "JP",
        "ko" => "KR",
        "sv" => "SE",
        "da" => "DK",
        "nb" => "NO",
        "he" => "IL",
        "el" => "GR",
        "uk" => "UA",
        "cs" => "CZ",
        "vi" => "VN",
        "hi" | "ta" | "te" | "kn" | "mr" => "IN",
        "ca" | "eu" | "gl" => "ES",
        "fil" => "PH",
        "ms" => "MY",
        "sr" => "RS",
        "ga" => "IE",
        "cy" => "GB",
        "fa" => "IR",
        "ur" => "PK",
        "ar" => "SA",
        "sl" => "SI",
        "et" => "EE",
        "kk" => "KZ",
        "bn" => "BD",
        "sw" => "KE",
        "af" => "ZA",
        "be" => "BY",
        "hy" => "AM",
        "ka" => "GE",
        "sq" => "AL",
        _ => "",
    };
    if region.is_empty() {
        primary.to_ascii_uppercase()
    } else {
        region.to_owned()
    }
}

/// The region subtag of a tag (`US` in `en-US`, `CN` in `zh-Hans-CN`), if
/// it has one.
fn region(tag: &str) -> Option<String> {
    tag.split('-')
        .skip(1)
        .find(|s| {
            (s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
                || (s.len() == 3 && s.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|s| s.to_ascii_uppercase())
}

/// How well a voice tagged `tag` fits `lang`, lower is better: 0 when the tag
/// is the language's own code (`ru`, `zh-CN` for Simplified Chinese), 1 for
/// the language as spoken in its home region (`ru-RU`, `en-US`), 2 for any
/// other region; `None` for another language.
pub fn rank(tag: &str, lang: LangId) -> Option<u8> {
    if !lang.is_language() || voice_language(tag) != Some(lang) {
        return None;
    }
    let tag = tag.trim().replace('_', "-");
    if tag.eq_ignore_ascii_case(lang.bcp47()) || tag.eq_ignore_ascii_case(lang.legacy_code()) {
        return Some(0);
    }
    if region(&tag).is_some_and(|r| r == home_region(lang)) {
        return Some(1);
    }
    Some(2)
}

/// The voice to read `lang` with: the default voice when it speaks the
/// language (the user chose it), else the best fit by penalty, then
/// [`rank`], then list order. `None` when no voice speaks it.
pub fn pick_voice(voices: &[Voice], lang: LangId, default: Option<usize>) -> Option<usize> {
    if let Some(d) = default.filter(|&d| voices.get(d).is_some_and(|v| rank(&v.tag, lang).is_some())) {
        return Some(d);
    }
    voices
        .iter()
        .enumerate()
        .filter_map(|(i, v)| rank(&v.tag, lang).map(|r| ((v.penalty, r, i), i)))
        .min()
        .map(|(_, i)| i)
}

/// The error for a language no installed voice speaks.
fn no_voice(lang: LangId, voices: &[Voice], how_to_add: &str) -> Error {
    let mut tags: Vec<String> = voices.iter().map(|v| v.tag.replace('_', "-")).collect();
    tags.sort();
    tags.dedup();
    let installed = if tags.is_empty() { "none".to_owned() } else { tags.join(", ") };
    Error::Module {
        module: SYSTEM_TTS_ID.into(),
        detail: format!(
            "no system voice for {} is installed (installed: {installed}). {how_to_add}",
            lang.english_name()
        ),
    }
}

/// `text` cut to [`MAX_CHARS`] characters, at the last space before the cut
/// when there is one.
pub fn clip(text: &str) -> &str {
    match text.char_indices().nth(MAX_CHARS) {
        None => text,
        Some((end, _)) => {
            let head = &text[..end];
            match head.rfind(char::is_whitespace) {
                Some(space) if space > end / 2 => &head[..space],
                _ => head,
            }
        }
    }
}

/// How long synthesis may take: engines run many times faster than speech,
/// so this only stops one that hangs.
fn timeout_for(text: &str) -> Duration {
    Duration::from_secs(30 + text.chars().count() as u64 / 100)
}

/// Check that `bytes` is a RIFF/WAVE file and make its sizes match its
/// length. espeak writing to a pipe cannot go back to fill them in and
/// leaves placeholders, which would make the player wait for audio that
/// never comes.
pub fn fix_wav(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    let bad = |what: &str| Error::bad_response(NAME, what);
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("the voice did not produce WAV audio"));
    }
    let len = bytes.len();
    let riff = u32::try_from(len - 8).unwrap_or(u32::MAX);
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    let mut pos = 12;
    while pos + 8 <= len {
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            let available = len - pos - 8;
            if available == 0 {
                return Err(bad("the voice produced no audio"));
            }
            if size > available {
                let fixed = u32::try_from(available).unwrap_or(u32::MAX);
                bytes[pos + 4..pos + 8].copy_from_slice(&fixed.to_le_bytes());
            }
            return Ok(bytes);
        }
        pos = pos.saturating_add(8).saturating_add(size).saturating_add(size & 1);
    }
    Err(bad("the WAV audio has no data"))
}

/// Text to speech with the operating system's own voices.
pub struct SystemTts {
    info: ProviderInfo,
    backend: Backend,
}

impl Default for SystemTts {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTts {
    /// Looks for the speech program (on Linux) but starts nothing.
    pub fn new() -> Self {
        let backend = Backend::new();
        let mut info = ProviderInfo::new(SYSTEM_TTS_ID, NAME, Capabilities::LISTEN | Capabilities::OFFLINE);
        info.description = backend.description();
        info.homepage = backend.homepage().into();
        info.max_input_chars = Some(MAX_CHARS);
        info.ready_to_use = backend.unavailable().is_none();
        Self { info, backend }
    }
}

impl Provider for SystemTts {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        host.check_cancelled()?;
        if let Some(why) = self.backend.unavailable() {
            return Err(Error::Config(why));
        }
        let text = clip(req.text.trim());
        if text.is_empty() {
            return Err(Error::EmptyInput);
        }
        let req = SpeakRequest { text: text.to_owned(), lang: req.lang, slow: req.slow };
        let bytes = self.backend.speak(&req, host.cancel)?;
        Ok(Audio { bytes: fix_wav(bytes)?, mime: "audio/wav".into() })
    }
}

#[cfg(windows)]
use win::Backend;

#[cfg(target_os = "macos")]
use mac::Say as Backend;

#[cfg(all(unix, not(target_os = "macos")))]
use linux::Espeak as Backend;

/// Somewhere with no voice we know how to reach.
#[cfg(not(any(windows, unix)))]
struct Backend;

#[cfg(not(any(windows, unix)))]
impl Backend {
    fn new() -> Self {
        Backend
    }
    fn description(&self) -> String {
        "Not available on this system.".into()
    }
    fn homepage(&self) -> &'static str {
        "https://github.com/bropines/glossa"
    }
    fn unavailable(&self) -> Option<String> {
        Some("there is no system voice on this platform".into())
    }
    fn speak(&self, _: &SpeakRequest, _: &glossa_modules::CancelToken) -> Result<Vec<u8>> {
        Err(Error::Unsupported { provider: NAME.into(), capability: "speech on this platform" })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn voice(id: &str, tag: &str) -> Voice {
        Voice { id: id.into(), name: id.into(), tag: tag.into(), penalty: 0 }
    }

    fn lang(code: &str) -> LangId {
        LangId::from_code(code).unwrap()
    }

    /// A minimal WAV: 16-bit mono, `samples` samples of silence.
    pub(crate) fn wav(samples: usize, data_size: Option<u32>) -> Vec<u8> {
        let data = samples * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&22050u32.to_le_bytes());
        out.extend_from_slice(&44100u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_size.unwrap_or(data as u32).to_le_bytes());
        out.resize(out.len() + data, 0);
        out
    }

    #[test]
    fn voice_tags_map_to_languages() {
        assert_eq!(voice_language("en-US"), Some(LangId::ENGLISH));
        assert_eq!(voice_language("ru_RU"), Some(LangId::RUSSIAN));
        assert_eq!(voice_language("en-gb-x-rp"), Some(LangId::ENGLISH));
        assert_eq!(voice_language("he_IL"), Some(lang("he")));
        assert_eq!(voice_language("nb_NO"), Some(lang("no")));
        assert_eq!(voice_language("ar_001"), Some(lang("ar")));
        assert_eq!(voice_language("zh-CN"), Some(lang("zh-Hans")));
        assert_eq!(voice_language("zh_TW"), Some(lang("zh-Hant")));
        assert_eq!(voice_language("zh_HK"), Some(lang("zh-Hant")));
        assert_eq!(voice_language("cmn"), Some(lang("zh-Hans")));
        assert_eq!(voice_language("yue"), Some(lang("zh-Hant")));
        assert_eq!(voice_language("cmn-latn-pinyin"), None);
        assert_eq!(voice_language("qya"), None);
        assert_eq!(voice_language(""), None);
    }

    #[test]
    fn rank_prefers_the_exact_code_then_the_home_region() {
        assert_eq!(rank("ru", LangId::RUSSIAN), Some(0));
        assert_eq!(rank("ru-RU", LangId::RUSSIAN), Some(1));
        assert_eq!(rank("en-US", LangId::ENGLISH), Some(1));
        assert_eq!(rank("en_GB", LangId::ENGLISH), Some(2));
        assert_eq!(rank("zh-CN", lang("zh-Hans")), Some(0));
        assert_eq!(rank("zh-TW", lang("zh-Hant")), Some(0));
        assert_eq!(rank("zh-HK", lang("zh-Hant")), Some(2));
        assert_eq!(rank("he_IL", lang("he")), Some(1));
        assert_eq!(rank("ru-RU", LangId::ENGLISH), None);
        assert_eq!(rank("en-US", LangId::AUTO), None);
    }

    #[test]
    fn the_best_voice_for_the_language_is_picked() {
        let voices = [
            voice("david", "en-GB"),
            voice("irina", "ru-RU"),
            voice("zira", "en-US"),
            voice("huihui", "zh-CN"),
            voice("tracy", "zh-HK"),
            voice("hanhan", "zh-TW"),
        ];
        assert_eq!(pick_voice(&voices, LangId::ENGLISH, None), Some(2), "en-US over en-GB");
        assert_eq!(pick_voice(&voices, LangId::RUSSIAN, None), Some(1));
        assert_eq!(pick_voice(&voices, lang("zh-Hans"), None), Some(3));
        assert_eq!(pick_voice(&voices, lang("zh-Hant"), None), Some(5), "Mandarin (zh-TW) before Cantonese");
        assert_eq!(pick_voice(&voices, lang("ja"), None), None);
        // The user's default voice wins when it speaks the language…
        assert_eq!(pick_voice(&voices, LangId::ENGLISH, Some(0)), Some(0));
        // …and is ignored when it does not.
        assert_eq!(pick_voice(&voices, LangId::RUSSIAN, Some(0)), Some(1));
        assert_eq!(pick_voice(&voices, LangId::RUSSIAN, Some(99)), Some(1));
    }

    #[test]
    fn penalised_voices_come_last() {
        let mut bubbles = voice("Bubbles", "en_US");
        bubbles.penalty = 2;
        let mut eddy = voice("Eddy (English (US))", "en_US");
        eddy.penalty = 1;
        let voices = [bubbles, eddy, voice("Daniel", "en_GB"), voice("Samantha", "en_US")];
        assert_eq!(pick_voice(&voices, LangId::ENGLISH, None), Some(3));
        assert_eq!(pick_voice(&voices[..2], LangId::ENGLISH, None), Some(1), "a robotic voice over a novelty one");
    }

    #[test]
    fn the_missing_voice_error_lists_what_is_installed() {
        let e = no_voice(lang("ja"), &[voice("a", "en_US"), voice("b", "en-US"), voice("c", "ru-RU")], "Add one.");
        let text = e.to_string();
        assert!(text.contains("Japanese") && text.contains("en-US, ru-RU") && text.contains("Add one."), "{text}");
        assert!(no_voice(lang("ja"), &[], "").to_string().contains("installed: none"));
    }

    #[test]
    fn long_text_is_clipped_at_a_space() {
        assert_eq!(clip("short"), "short");
        let long = "word ".repeat(MAX_CHARS);
        let c = clip(&long);
        assert!(c.chars().count() <= MAX_CHARS && c.ends_with("word"), "{}", c.len());
        let solid = "я".repeat(MAX_CHARS + 5);
        assert_eq!(clip(&solid).chars().count(), MAX_CHARS);
    }

    #[test]
    fn wav_sizes_are_fixed_to_the_real_length() {
        let good = wav(100, None);
        assert_eq!(fix_wav(good.clone()).unwrap(), good);
        // espeak through a pipe: placeholder sizes.
        let mut piped = wav(100, Some(0x7fff_f000));
        piped[4..8].copy_from_slice(&0x7fff_f024u32.to_le_bytes());
        let fixed = fix_wav(piped).unwrap();
        assert_eq!(fixed, good);
        // A chunk before the data (macOS writes `FLLR`).
        let mut padded = good[..36].to_vec();
        padded.extend_from_slice(b"FLLR");
        padded.extend_from_slice(&3u32.to_le_bytes());
        padded.extend_from_slice(&[0, 0, 0, 0]); // three bytes and the pad byte
        padded.extend_from_slice(&good[36..]);
        let fixed = fix_wav(padded.clone()).unwrap();
        assert_eq!(u32::from_le_bytes(fixed[4..8].try_into().unwrap()) as usize, padded.len() - 8);
        assert!(fix_wav(b"ID3\x04 not a wav file".to_vec()).is_err());
        assert!(fix_wav(wav(0, None)).is_err(), "no samples");
        assert!(fix_wav(good[..36].to_vec()).is_err(), "no data chunk");
    }

    #[test]
    fn the_provider_is_an_offline_voice() {
        let p = SystemTts::new();
        let info = p.info();
        assert_eq!(info.id, SYSTEM_TTS_ID);
        assert!(info.supports(Capabilities::LISTEN | Capabilities::OFFLINE));
        assert!(!info.supports(Capabilities::TRANSLATE));
        assert!(!info.needs_credentials);
        assert!(info.homepage.starts_with("https://"));
        assert!(!info.description.is_empty());
        // Ready to use exactly when the system has a voice program.
        assert_eq!(info.ready_to_use, p.backend.unavailable().is_none());
    }
}
