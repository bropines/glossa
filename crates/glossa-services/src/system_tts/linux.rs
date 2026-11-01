//! The system voice on Linux and the BSDs: `espeak-ng`, or the older
//! `espeak`, which print WAV on stdout with `--stdout`. Glossa looks for
//! them on `PATH` once, when it starts; without either the provider says it
//! is not installed.
//!
//! Only runs programs, so it is compiled and tested on every system.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use glossa_core::{Error, Result, SpeakRequest};
use glossa_modules::child;
use glossa_modules::CancelToken;

use super::{no_voice, pick_voice, timeout_for, Voice, SLOW_WPM};

/// The programs tried, best first.
const PROGRAMS: [&str; 2] = ["espeak-ng", "espeak"];

const INSTALL_HINT: &str = "Install espeak-ng from your distribution (`sudo apt install espeak-ng` on Debian and \
                            Ubuntu, `sudo dnf install espeak-ng` on Fedora, `sudo pacman -S espeak-ng` on Arch) \
                            and restart Glossa.";

pub struct Espeak {
    program: Option<PathBuf>,
    /// The voice list, read on first use.
    voices: Mutex<Option<Vec<Voice>>>,
}

impl Espeak {
    pub fn new() -> Self {
        Self::with_program(PROGRAMS.iter().find_map(|p| find_on_path(std::env::var_os("PATH"), p)))
    }

    /// With a given program, or none (tests).
    pub fn with_program(program: Option<PathBuf>) -> Self {
        Self { program, voices: Mutex::new(None) }
    }

    pub fn description(&self) -> String {
        match &self.program {
            Some(p) => format!("Reads text aloud offline with {}, the speech synthesiser on this system.", p.display()),
            None => format!("Not available: it needs espeak-ng (or espeak), which is not installed. {INSTALL_HINT}"),
        }
    }

    pub fn homepage(&self) -> &'static str {
        "https://github.com/espeak-ng/espeak-ng"
    }

    /// Why it cannot speak, if it cannot.
    pub fn unavailable(&self) -> Option<String> {
        self.program
            .is_none()
            .then(|| format!("the system voice needs espeak-ng, which is not installed. {INSTALL_HINT}"))
    }

    fn voices(&self, program: &Path, cancel: &CancelToken) -> Vec<Voice> {
        let mut cached = self.voices.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cached.as_ref() {
            return v.clone();
        }
        let mut cmd = Command::new(program);
        cmd.arg("--voices");
        match child::run(cmd, Duration::from_secs(10), Some(cancel)) {
            Ok(out) if out.success() => {
                let list = parse_voices(&String::from_utf8_lossy(&out.stdout));
                *cached = Some(list.clone());
                list
            }
            Ok(out) => {
                tracing::warn!("{} --voices failed: {}", program.display(), out.stderr_tail());
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("{} --voices: {e}", program.display());
                Vec::new()
            }
        }
    }

    pub fn speak(&self, req: &SpeakRequest, cancel: &CancelToken) -> Result<Vec<u8>> {
        let Some(program) = &self.program else {
            return Err(Error::Config(self.unavailable().unwrap_or_default()));
        };
        let voice = if req.lang.is_language() {
            let voices = self.voices(program, cancel);
            if voices.is_empty() {
                // The list could not be read: espeak also takes a language
                // code as a voice name.
                Some(fallback_voice(req.lang))
            } else {
                let i = pick_voice(&voices, req.lang, None).ok_or_else(|| {
                    no_voice(req.lang, &voices, "espeak-ng has no voice for it; another speech service can read it.")
                })?;
                Some(voices[i].id.clone())
            }
        } else {
            None
        };
        let mut cmd = Command::new(program);
        cmd.args(args(voice.as_deref(), req.slow, &req.text));
        let out = child::run(cmd, timeout_for(&req.text), Some(cancel))?;
        if !out.success() || out.stdout.is_empty() {
            let why = out.stderr_tail();
            return Err(Error::Module {
                module: super::SYSTEM_TTS_ID.into(),
                detail: format!(
                    "{} failed{}",
                    program.display(),
                    if why.is_empty() { String::new() } else { format!(": {why}") }
                ),
            });
        }
        Ok(out.stdout)
    }
}

impl Default for Espeak {
    fn default() -> Self {
        Self::new()
    }
}

/// espeak's arguments: WAV on stdout, UTF-8 text, the voice, the rate, and
/// the text after `--` so text starting with `-` is not read as an option.
pub fn args(voice: Option<&str>, slow: bool, text: &str) -> Vec<OsString> {
    let mut out: Vec<OsString> = vec!["--stdout".into(), "-b".into(), "1".into()];
    if let Some(v) = voice {
        out.push("-v".into());
        out.push(v.into());
    }
    if slow {
        out.push("-s".into());
        out.push(SLOW_WPM.to_string().into());
    }
    out.push("--".into());
    out.push(text.into());
    out
}

/// The voice name to try when the list could not be read: the language's
/// code, with espeak's name for Mandarin.
fn fallback_voice(lang: glossa_core::LangId) -> String {
    match lang.bcp47() {
        "zh-Hans" | "zh-Hant" => "cmn".into(),
        other => other.to_ascii_lowercase(),
    }
}

/// `espeak-ng --voices`: a header, then one voice per line:
///
/// ```text
/// Pty Language       Age/Gender VoiceName          File                 Other Languages
///  5  en-us           --/M      English_(America)  gmw/en-US            (en 2)(en-r 5)
/// ```
///
/// The language column is what `-v` takes. Each language is listed once.
pub fn parse_voices(text: &str) -> Vec<Voice> {
    let mut out: Vec<Voice> = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 || cols[0] == "Pty" || cols[0].parse::<u32>().is_err() {
            continue;
        }
        let tag = cols[1];
        if out.iter().any(|v| v.id == tag) {
            continue;
        }
        out.push(Voice { id: tag.into(), name: cols[3].replace('_', " "), tag: tag.into(), penalty: 0 });
    }
    out
}

/// `name` in the folders of `path`, if it is there as a file.
fn find_on_path(path: Option<OsString>, name: &str) -> Option<PathBuf> {
    std::env::split_paths(&path?).map(|dir| dir.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::LangId;

    const VOICES: &str = "\
Pty Language       Age/Gender VoiceName          File                 Other Languages
 5  af              --/M      Afrikaans          gmw/af
 5  cmn             --/M      Chinese_(Mandarin,_latin_as_English) sit/cmn              (zh-cmn 5)(zh 5)
 5  cmn-latn-pinyin --/M      Chinese_(Mandarin,_latin_as_Pinyin) sit/cmn-Latn-pinyin  (zh-cmn 5)(zh 5)
 2  en-gb           --/M      English_(Great_Britain) gmw/en               (en 2)
 5  en-us           --/M      English_(America)  gmw/en-US            (en 3)
 5  ru              --/M      Russian            zle/ru
 5  ru              --/F      Russian_female     zle/ru-f
";

    fn lang(code: &str) -> LangId {
        LangId::from_code(code).unwrap()
    }

    #[test]
    fn the_voice_list_is_parsed() {
        let v = parse_voices(VOICES);
        let ids: Vec<&str> = v.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["af", "cmn", "cmn-latn-pinyin", "en-gb", "en-us", "ru"]);
        assert_eq!(v[4].name, "English (America)");
        assert!(parse_voices("").is_empty());
        assert!(parse_voices("garbage\n\n  x").is_empty());
    }

    #[test]
    fn voices_are_matched_to_the_language() {
        let v = parse_voices(VOICES);
        let pick = |l| pick_voice(&v, l, None).map(|i| v[i].id.as_str());
        assert_eq!(pick(LangId::ENGLISH), Some("en-us"));
        assert_eq!(pick(LangId::RUSSIAN), Some("ru"));
        assert_eq!(pick(lang("zh-Hans")), Some("cmn"));
        assert_eq!(pick(lang("ja")), None);
        assert_eq!(fallback_voice(lang("zh-Hant")), "cmn");
        assert_eq!(fallback_voice(lang("he")), "he");
    }

    #[test]
    fn arguments_put_the_text_after_the_options() {
        let s = |v: Vec<OsString>| v.into_iter().map(|a| a.into_string().unwrap()).collect::<Vec<_>>();
        assert_eq!(s(args(Some("ru"), false, "привет")), ["--stdout", "-b", "1", "-v", "ru", "--", "привет"]);
        assert_eq!(s(args(None, true, "-x")), ["--stdout", "-b", "1", "-s", "110", "--", "-x"]);
    }

    #[test]
    fn without_espeak_it_is_unavailable_not_broken() {
        let e = Espeak::with_program(None);
        assert!(e.unavailable().unwrap().contains("espeak-ng"));
        assert!(e.description().starts_with("Not available"));
        let req = SpeakRequest { text: "hi".into(), lang: LangId::ENGLISH, slow: false };
        assert!(matches!(e.speak(&req, &CancelToken::new()), Err(Error::Config(_))));
        assert_eq!(find_on_path(None, "espeak-ng"), None);
    }

    /// A stand-in for espeak: lists voices, and prints a WAV with
    /// placeholder sizes as espeak does through a pipe, after checking the
    /// arguments it was given.
    #[cfg(unix)]
    #[test]
    fn speaks_through_a_fake_espeak() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("glossa-espeak-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("out.wav");
        std::fs::write(&wav, crate::system_tts::tests::wav(50, Some(0x7fff_f000))).unwrap();
        let script = dir.join("espeak-ng");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --voices ]; then\ncat <<'EOF'\n{VOICES}EOF\nexit 0\nfi\n\
                 [ \"$*\" = \"--stdout -b 1 -v ru -s 110 -- привет\" ] || {{ echo \"bad args: $*\" >&2; exit 1; }}\n\
                 cat '{}'\n",
                wav.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = Espeak::with_program(Some(script));
        assert!(e.unavailable().is_none());
        let req = SpeakRequest { text: "привет".into(), lang: LangId::RUSSIAN, slow: true };
        let bytes = e.speak(&req, &CancelToken::new()).unwrap();
        let fixed = crate::system_tts::fix_wav(bytes).unwrap();
        assert_eq!(fixed, crate::system_tts::tests::wav(50, None));
        // A language with no voice is an error naming the language.
        let ja = SpeakRequest { text: "こんにちは".into(), lang: lang("ja"), slow: false };
        let err = e.speak(&ja, &CancelToken::new()).unwrap_err().to_string();
        assert!(err.contains("Japanese"), "{err}");
        // The program's complaint reaches the error.
        let en = SpeakRequest { text: "hello".into(), lang: LangId::ENGLISH, slow: false };
        let err = e.speak(&en, &CancelToken::new()).unwrap_err().to_string();
        assert!(err.contains("bad args"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The real espeak-ng, when it is installed.
    #[test]
    #[ignore = "needs espeak-ng or espeak"]
    fn live_espeak_speaks_wav() {
        let e = Espeak::new();
        if e.unavailable().is_some() {
            eprintln!("espeak-ng is not installed; skipped");
            return;
        }
        for (text, lang, slow) in [("Hello there", LangId::ENGLISH, false), ("Привет, мир", LangId::RUSSIAN, true)]
        {
            let req = SpeakRequest { text: text.into(), lang, slow };
            let bytes = crate::system_tts::fix_wav(e.speak(&req, &CancelToken::new()).unwrap()).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert!(bytes.len() > 10_000, "{} bytes", bytes.len());
        }
    }
}
