//! The system voice on macOS: `/usr/bin/say`, which writes a WAV file with
//! `--file-format=WAVE`. The voice comes from `say -v '?'`; macOS's novelty
//! voices (Bubbles, Zarvox) and the robotic Eloquence ones (Eddy, Flo) are
//! taken only when nothing else speaks the language.
//!
//! Only runs a program, so it is compiled and tested on every system;
//! the live test (`live_say_speaks_wav`) passes on macOS 27.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use glossa_core::{Error, Result, SpeakRequest};
use glossa_modules::child;
use glossa_modules::CancelToken;

use super::{no_voice, pick_voice, timeout_for, Voice, SLOW_WPM, SYSTEM_TTS_ID};

const SAY: &str = "/usr/bin/say";

const HOW_TO_ADD: &str = "Add one in System Settings → Accessibility → Spoken Content → System voice → Manage Voices.";

/// Voices that make sounds rather than read (`say -v '?'` lists them as
/// English).
const NOVELTY: &[&str] = &[
    "Albert",
    "Bad News",
    "Bahh",
    "Bells",
    "Boing",
    "Bubbles",
    "Cellos",
    "Fred",
    "Good News",
    "Jester",
    "Junior",
    "Kathy",
    "Organ",
    "Ralph",
    "Superstar",
    "Trinoids",
    "Whisper",
    "Wobble",
    "Zarvox",
];

/// The Eloquence voices, listed once per language as `Eddy (German
/// (Germany))`: understandable, but robotic next to the others.
const ELOQUENCE: &[&str] = &["Eddy", "Flo", "Grandma", "Grandpa", "Reed", "Rocko", "Sandy", "Shelley"];

pub struct Say {
    program: PathBuf,
    /// The voice list, read on first use.
    voices: Mutex<Option<Vec<Voice>>>,
}

impl Say {
    pub fn new() -> Self {
        Self::with_program(PathBuf::from(SAY))
    }

    /// With another program in place of `say` (tests).
    pub fn with_program(program: PathBuf) -> Self {
        Self { program, voices: Mutex::new(None) }
    }

    pub fn description(&self) -> String {
        format!("Reads text aloud with the voices installed in macOS, offline. {HOW_TO_ADD}")
    }

    pub fn homepage(&self) -> &'static str {
        "https://support.apple.com/guide/mac-help/mchlp2290/mac"
    }

    /// `say` is part of macOS.
    pub fn unavailable(&self) -> Option<String> {
        None
    }

    fn voices(&self, cancel: &CancelToken) -> Vec<Voice> {
        let mut cached = self.voices.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cached.as_ref() {
            return v.clone();
        }
        let mut cmd = Command::new(&self.program);
        cmd.args(["-v", "?"]);
        match child::run(cmd, Duration::from_secs(10), Some(cancel)) {
            Ok(out) if out.success() => {
                let list = parse_voices(&String::from_utf8_lossy(&out.stdout));
                *cached = Some(list.clone());
                list
            }
            Ok(out) => {
                tracing::warn!("say -v '?' failed: {}", out.stderr_tail());
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("say -v '?': {e}");
                Vec::new()
            }
        }
    }

    pub fn speak(&self, req: &SpeakRequest, cancel: &CancelToken) -> Result<Vec<u8>> {
        let voice = if req.lang.is_language() {
            let voices = self.voices(cancel);
            // An unreadable list leaves the choice to `say` (the system voice).
            if voices.is_empty() {
                None
            } else {
                let i = pick_voice(&voices, req.lang, None).ok_or_else(|| no_voice(req.lang, &voices, HOW_TO_ADD))?;
                Some(voices[i].id.clone())
            }
        } else {
            None
        };
        let out_file = TempWav::new();
        let mut cmd = Command::new(&self.program);
        cmd.args(args(voice.as_deref(), req.slow, &out_file.0, &req.text));
        let out = child::run(cmd, timeout_for(&req.text), Some(cancel))?;
        let bytes = std::fs::read(&out_file.0).unwrap_or_default();
        if !out.success() || bytes.is_empty() {
            let why = out.stderr_tail();
            return Err(Error::Module {
                module: SYSTEM_TTS_ID.into(),
                detail: format!("say failed{}", if why.is_empty() { String::new() } else { format!(": {why}") }),
            });
        }
        Ok(bytes)
    }
}

impl Default for Say {
    fn default() -> Self {
        Self::new()
    }
}

/// `say`'s arguments: the voice, the rate, 16-bit WAV into `out`, and the
/// text after `--` so text starting with `-` is not read as an option.
pub fn args(voice: Option<&str>, slow: bool, out: &Path, text: &str) -> Vec<OsString> {
    let mut v: Vec<OsString> = Vec::new();
    if let Some(name) = voice {
        v.push("-v".into());
        v.push(name.into());
    }
    if slow {
        v.push("-r".into());
        v.push(SLOW_WPM.to_string().into());
    }
    v.push("--file-format=WAVE".into());
    v.push("--data-format=LEI16@22050".into());
    v.push("-o".into());
    v.push(out.into());
    v.push("--".into());
    v.push(text.into());
    v
}

/// `say -v '?'`: one voice per line, the name (which may hold spaces and
/// brackets), the locale, then `#` and a sample sentence:
///
/// ```text
/// Milena (Russian (Russia)) ru_RU    # Здравствуйте! Меня зовут Милена.
/// ```
pub fn parse_voices(text: &str) -> Vec<Voice> {
    let mut out: Vec<Voice> = Vec::new();
    for line in text.lines() {
        let left = line.split_once('#').map_or(line, |(l, _)| l).trim_end();
        let Some((name, locale)) = left.rsplit_once(char::is_whitespace) else { continue };
        let name = name.trim();
        let locale_ok = locale.contains('_') && locale.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if name.is_empty() || !locale_ok || out.iter().any(|v| v.id == name) {
            continue;
        }
        let family = name.split(" (").next().unwrap_or(name);
        let penalty = if NOVELTY.contains(&name) {
            2
        } else if ELOQUENCE.contains(&family) {
            1
        } else {
            0
        };
        out.push(Voice { id: name.into(), name: name.into(), tag: locale.into(), penalty });
    }
    out
}

/// A file name in the temporary folder for `say` to write, removed when
/// dropped.
struct TempWav(PathBuf);

impl TempWav {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        TempWav(std::env::temp_dir().join(format!("glossa-say-{}-{n}.wav", std::process::id())))
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::LangId;

    /// Lines from `say -v '?'` on macOS 27.
    const VOICES: &str = "\
Albert              en_US    # Hello! My name is Albert.
Anna                de_DE    # Hallo! Ich heiße Anna.
Bubbles             en_US    # Hello! My name is Bubbles.
Daniel              en_GB    # Hello! My name is Daniel.
Eddy (German (Germany)) de_DE    # Hallo! Ich heiße Eddy.
Eddy (English (US)) en_US    # Hello! My name is Eddy.
Majed               ar_001   # مرحبًا! اسمي ماجد.
Milena (Russian (Russia)) ru_RU    # Здравствуйте! Меня зовут Милена.
Milena (Russian (Russia)) ru_RU    # Здравствуйте! Меня зовут Милена.
Samantha (English (US)) en_US    # Hello! My name is Samantha.
Sinji               zh_HK    # 你好！我叫善怡。
Meijia              zh_TW    # 你好，我叫美佳。
Tingting            zh_CN    # 你好！我叫婷婷。
";

    fn lang(code: &str) -> LangId {
        LangId::from_code(code).unwrap()
    }

    #[test]
    fn the_voice_list_is_parsed() {
        let v = parse_voices(VOICES);
        assert_eq!(v.len(), 12, "the repeated Milena is listed once");
        assert_eq!(v[0], Voice { id: "Albert".into(), name: "Albert".into(), tag: "en_US".into(), penalty: 2 });
        assert_eq!(v[4].id, "Eddy (German (Germany))");
        assert_eq!(v[4].tag, "de_DE");
        assert_eq!(v[4].penalty, 1);
        assert_eq!(v[7].id, "Milena (Russian (Russia))");
        assert_eq!(v[7].penalty, 0);
        assert!(parse_voices("").is_empty());
        assert!(parse_voices("no locale here\n# only a comment").is_empty());
    }

    #[test]
    fn voices_are_matched_to_the_language() {
        let v = parse_voices(VOICES);
        let pick = |l| pick_voice(&v, l, None).map(|i| v[i].id.as_str());
        assert_eq!(pick(LangId::ENGLISH), Some("Samantha (English (US))"));
        assert_eq!(pick(LangId::RUSSIAN), Some("Milena (Russian (Russia))"));
        assert_eq!(pick(lang("de")), Some("Anna"), "a natural voice over Eddy");
        assert_eq!(pick(lang("ar")), Some("Majed"));
        assert_eq!(pick(lang("zh-Hans")), Some("Tingting"));
        assert_eq!(pick(lang("zh-Hant")), Some("Meijia"));
        assert_eq!(pick(lang("ja")), None);
    }

    #[test]
    fn arguments_write_wav_and_put_the_text_last() {
        let s = |v: Vec<OsString>| v.into_iter().map(|a| a.into_string().unwrap()).collect::<Vec<_>>();
        assert_eq!(
            s(args(Some("Milena (Russian (Russia))"), true, Path::new("/tmp/x.wav"), "-привет")),
            [
                "-v",
                "Milena (Russian (Russia))",
                "-r",
                "110",
                "--file-format=WAVE",
                "--data-format=LEI16@22050",
                "-o",
                "/tmp/x.wav",
                "--",
                "-привет"
            ]
        );
        assert_eq!(
            s(args(None, false, Path::new("o.wav"), "hi"))[..2],
            ["--file-format=WAVE", "--data-format=LEI16@22050"]
        );
    }

    #[test]
    fn the_temporary_file_is_removed() {
        let t = TempWav::new();
        std::fs::write(&t.0, b"x").unwrap();
        let path = t.0.clone();
        assert_ne!(path, TempWav::new().0, "each call gets its own name");
        drop(t);
        assert!(!path.exists());
    }

    /// A stand-in for `say` that lists voices and writes a WAV to the file
    /// after `-o`, after checking the arguments it was given.
    #[cfg(unix)]
    #[test]
    fn speaks_through_a_fake_say() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("glossa-say-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("sample.wav");
        let sample = crate::system_tts::tests::wav(40, None);
        std::fs::write(&wav, &sample).unwrap();
        let script = dir.join("say");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1 $2\" = '-v ?' ]; then\ncat <<'EOF'\n{VOICES}EOF\nexit 0\nfi\n\
                 [ \"$1\" = -v ] && [ \"$2\" = 'Milena (Russian (Russia))' ] && [ \"$7\" = -o ] && [ \"$9\" = -- ] \
                 && [ \"${{10}}\" = 'привет' ] || {{ echo \"bad args: $*\" >&2; exit 1; }}\n\
                 cp '{}' \"$8\"\n",
                wav.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let say = Say::with_program(script);
        let req = SpeakRequest { text: "привет".into(), lang: LangId::RUSSIAN, slow: true };
        assert_eq!(say.speak(&req, &CancelToken::new()).unwrap(), sample);
        let ja = SpeakRequest { text: "こんにちは".into(), lang: lang("ja"), slow: false };
        let err = say.speak(&ja, &CancelToken::new()).unwrap_err().to_string();
        assert!(err.contains("Japanese") && err.contains("Spoken Content"), "{err}");
        let en = SpeakRequest { text: "hello".into(), lang: LangId::ENGLISH, slow: false };
        let err = say.speak(&en, &CancelToken::new()).unwrap_err().to_string();
        assert!(err.contains("bad args"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The real `say`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "speaks through macOS"]
    fn live_say_speaks_wav() {
        let say = Say::new();
        for (text, lang, slow) in [("Hello there", LangId::ENGLISH, false), ("Привет, мир", LangId::RUSSIAN, true)]
        {
            let req = SpeakRequest { text: text.into(), lang, slow };
            let bytes = crate::system_tts::fix_wav(say.speak(&req, &CancelToken::new()).unwrap()).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert!(bytes.len() > 10_000, "{} bytes", bytes.len());
        }
    }
}
