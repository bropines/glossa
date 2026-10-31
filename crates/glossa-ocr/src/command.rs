//! OCR through any program the user names: Tesseract, a cloud CLI, a script.
//!
//! The `command` option is a template such as
//! `tesseract {image} - -l {lang3}`. It is split into arguments the way a
//! shell would split it (quotes group, nothing else is special) and run
//! directly, **without a shell**: there is no globbing, no variable expansion
//! and no way for the image path or a language code to inject anything,
//! because placeholders are substituted inside arguments that are already
//! split. Whatever the program prints on stdout, as UTF-8, is the text.
//!
//! | placeholder | becomes |
//! |-------------|---------|
//! | `{image}`   | path of a temporary file holding the image (`.png` for PNG data, which captured regions always are) |
//! | `{lang}`    | the language as BCP 47 (`en`, `zh-Hans`), or `auto` |
//! | `{lang3}`   | Tesseract's code (`eng`, `chi_sim`); for `auto`, the `auto_lang3` option (default `eng`) |

use std::process::Command;
use std::time::Duration;

use glossa_core::{Capabilities, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, OptionKind, OptionSpec, Provider};

use crate::process;
use crate::tempimage::TempImage;

pub const COMMAND_OCR_ID: &str = "command-ocr";

pub struct CommandOcr {
    info: ProviderInfo,
    options: Vec<OptionSpec>,
}

impl Default for CommandOcr {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandOcr {
    pub fn new() -> Self {
        let mut info = ProviderInfo::new(COMMAND_OCR_ID, "External OCR command", Capabilities::OCR);
        info.description =
            "Runs a program of your choice (Tesseract, a script) on the captured image and reads its output.".into();
        info.needs_credentials = true;
        let timeout = OptionSpec {
            kind: OptionKind::Number,
            default: serde_json::json!(30),
            min: Some(1.0),
            max: Some(600.0),
            ..OptionSpec::text("timeout_secs", "Timeout, seconds", "")
        };
        let options = vec![
            OptionSpec::text("command", "Command", "")
                .required()
                .placeholder("tesseract {image} - -l {lang3}")
                .help("Run without a shell. {image} is the picture's file, {lang} the language (BCP 47), {lang3} its Tesseract code."),
            OptionSpec::text("auto_lang3", "{lang3} when the language is automatic", "eng"),
            timeout,
        ];
        Self { info, options }
    }
}

impl Provider for CommandOcr {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn options(&self) -> &[OptionSpec] {
        &self.options
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        let template = host.require("command", &self.info.name)?.to_owned();
        let auto3 = match host.option_str("auto_lang3").trim() {
            "" => "eng".to_owned(),
            v => v.to_owned(),
        };
        let timeout = Duration::from_secs_f64(host.option_f64("timeout_secs").unwrap_or(30.0).clamp(1.0, 600.0));

        let bytes = req.image.to_bytes()?;
        let image = TempImage::write(&bytes)?;
        let vars = Placeholders {
            image: image.path().to_string_lossy().into_owned(),
            lang: if req.lang.is_language() { req.lang.bcp47().to_owned() } else { "auto".into() },
            lang3: if req.lang.is_language() {
                tesseract_code(req.lang).map(str::to_owned).unwrap_or_else(|| req.lang.bcp47().to_owned())
            } else {
                auto3
            },
        };
        let argv = build_argv(&template, &vars)?;
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        let started = std::time::Instant::now();
        let out = process::run(cmd, timeout, Some(host.cancel))?;
        if !out.success() {
            let stderr = out.stderr_tail();
            return Err(Error::Module {
                module: COMMAND_OCR_ID.into(),
                detail: format!(
                    "{} exited with {}{}",
                    argv[0],
                    out.code.map_or("a signal".to_owned(), |c| format!("code {c}")),
                    if stderr.is_empty() { String::new() } else { format!(": {stderr}") }
                ),
            });
        }
        let raw = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        // Tesseract ends each page with a form feed.
        let mut text = raw.trim_start_matches('\u{feff}').trim_end_matches(['\n', '\r', '\u{c}', ' ']).to_owned();
        if !req.preserve_line_breaks {
            text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        Ok(OcrResult {
            text,
            provider_id: COMMAND_OCR_ID.into(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            ..Default::default()
        })
    }
}

/// Values for the template's placeholders.
#[derive(Debug, Clone, Default)]
pub struct Placeholders {
    pub image: String,
    pub lang: String,
    pub lang3: String,
}

/// Split the template and substitute the placeholders in each argument.
pub fn build_argv(template: &str, vars: &Placeholders) -> Result<Vec<String>> {
    let words = split_words(template).map_err(|e| Error::Config(format!("OCR command: {e}")))?;
    if words.is_empty() {
        return Err(Error::Config("the OCR command is empty".into()));
    }
    Ok(words
        .into_iter()
        .map(|w| w.replace("{image}", &vars.image).replace("{lang3}", &vars.lang3).replace("{lang}", &vars.lang))
        .collect())
}

/// Shell-like word splitting, without the shell.
///
/// Whitespace separates words; `'…'` groups literally; `"…"` groups and
/// allows `\"` inside. A backslash escapes only a quote character (`\"`,
/// `\'`): everywhere else it is literal, because on Windows it is the path
/// separator and `C:\Tools\tesseract.exe` must survive unquoted.
pub fn split_words(s: &str) -> std::result::Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if chars.peek() == Some(&'"') => {
                            cur.push('"');
                            chars.next();
                        }
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' if matches!(chars.peek(), Some('"') | Some('\'')) => {
                in_word = true;
                if let Some(q) = chars.next() {
                    cur.push(q);
                }
            }
            other => {
                in_word = true;
                cur.push(other);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Tesseract's traineddata name for a language, where it has one.
pub fn tesseract_code(lang: LangId) -> Option<&'static str> {
    if !lang.is_language() {
        return None;
    }
    let bcp = lang.bcp47();
    if bcp.eq_ignore_ascii_case("zh-Hant") {
        return Some("chi_tra");
    }
    let base = bcp.split('-').next().unwrap_or(bcp).to_ascii_lowercase();
    TESSERACT.iter().find(|(code, _)| *code == base).map(|(_, t)| *t)
}

/// BCP 47 primary subtag -> Tesseract code (ISO 639-2/T, with Tesseract's own
/// exceptions such as `chi_sim`).
const TESSERACT: &[(&str, &str)] = &[
    ("af", "afr"), ("am", "amh"), ("ar", "ara"), ("as", "asm"), ("az", "aze"), ("be", "bel"),
    ("bg", "bul"), ("bn", "ben"), ("bo", "bod"), ("bs", "bos"), ("ca", "cat"), ("ceb", "ceb"),
    ("cs", "ces"), ("cy", "cym"), ("da", "dan"), ("de", "deu"), ("dv", "div"), ("el", "ell"),
    ("en", "eng"), ("eo", "epo"), ("es", "spa"), ("et", "est"), ("eu", "eus"), ("fa", "fas"),
    ("fi", "fin"), ("fil", "fil"), ("fo", "fao"), ("fr", "fra"), ("fy", "fry"), ("ga", "gle"),
    ("gd", "gla"), ("gl", "glg"), ("gu", "guj"), ("he", "heb"), ("iw", "heb"), ("hi", "hin"),
    ("hr", "hrv"), ("ht", "hat"), ("hu", "hun"), ("hy", "hye"), ("id", "ind"), ("is", "isl"),
    ("it", "ita"), ("ja", "jpn"), ("jv", "jav"), ("ka", "kat"), ("kk", "kaz"), ("km", "khm"),
    ("kn", "kan"), ("ko", "kor"), ("ku", "kmr"), ("ky", "kir"), ("la", "lat"), ("lb", "ltz"),
    ("lo", "lao"), ("lt", "lit"), ("lv", "lav"), ("mi", "mri"), ("mk", "mkd"), ("ml", "mal"),
    ("mn", "mon"), ("mr", "mar"), ("ms", "msa"), ("mt", "mlt"), ("my", "mya"), ("nb", "nor"),
    ("ne", "nep"), ("nl", "nld"), ("nn", "nor"), ("no", "nor"), ("oc", "oci"), ("or", "ori"),
    ("pa", "pan"), ("pl", "pol"), ("ps", "pus"), ("pt", "por"), ("qu", "que"), ("ro", "ron"),
    ("ru", "rus"), ("sa", "san"), ("sd", "snd"), ("si", "sin"), ("sk", "slk"), ("sl", "slv"),
    ("sq", "sqi"), ("sr", "srp"), ("su", "sun"), ("sv", "swe"), ("sw", "swa"), ("ta", "tam"),
    ("te", "tel"), ("tg", "tgk"), ("th", "tha"), ("ti", "tir"), ("tl", "tgl"), ("to", "ton"),
    ("tr", "tur"), ("tt", "tat"), ("ug", "uig"), ("uk", "ukr"), ("ur", "urd"), ("uz", "uzb"),
    ("vi", "vie"), ("yi", "yid"), ("yo", "yor"), ("zh", "chi_sim"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::ImageSource;
    use glossa_modules::provider::{resolve_options, with_test_host};
    use glossa_net::mock::MockHttp;

    #[test]
    fn words_split_like_a_shell_without_one() {
        assert_eq!(split_words("tesseract {image} - -l {lang3}").unwrap(), ["tesseract", "{image}", "-", "-l", "{lang3}"]);
        assert_eq!(
            split_words(r#""C:\Program Files\Tesseract-OCR\tesseract.exe" {image} stdout"#).unwrap(),
            [r"C:\Program Files\Tesseract-OCR\tesseract.exe", "{image}", "stdout"]
        );
        assert_eq!(split_words(r"C:\Tools\ocr.exe  a").unwrap(), [r"C:\Tools\ocr.exe", "a"]);
        assert_eq!(split_words(r"\\server\share\ocr.exe").unwrap(), [r"\\server\share\ocr.exe"]);
        assert_eq!(split_words(r#"echo 'it''s' "say \"hi\"" a\"b"#).unwrap(), ["echo", "its", "say \"hi\"", "a\"b"]);
        assert_eq!(split_words(r#"x "" y"#).unwrap(), ["x", "", "y"], "an empty quoted word is a word");
        assert_eq!(split_words("   ").unwrap(), Vec::<String>::new());
        assert!(split_words("a 'open").is_err());
        assert!(split_words("a \"open").is_err());
    }

    #[test]
    fn placeholders_are_substituted_inside_split_words() {
        let vars = Placeholders { image: "/tmp/my image.png".into(), lang: "zh-Hans".into(), lang3: "chi_sim".into() };
        let argv = build_argv("ocr --in={image} -l {lang3} --bcp {lang} '{lang}'", &vars).unwrap();
        assert_eq!(argv, ["ocr", "--in=/tmp/my image.png", "-l", "chi_sim", "--bcp", "zh-Hans", "zh-Hans"]);
        // A path with shell metacharacters stays one inert argument.
        let evil = Placeholders { image: "a; rm -rf ~".into(), ..vars };
        assert_eq!(build_argv("t {image}", &evil).unwrap(), ["t", "a; rm -rf ~"]);
        assert!(build_argv("  ", &Placeholders::default()).is_err());
    }

    #[test]
    fn tesseract_codes() {
        let code = |c: &str| tesseract_code(LangId::from_code(c).unwrap());
        assert_eq!(code("en"), Some("eng"));
        assert_eq!(code("ru"), Some("rus"));
        assert_eq!(code("de"), Some("deu"));
        assert_eq!(code("ja"), Some("jpn"));
        assert_eq!(code("zh-CN"), Some("chi_sim"));
        assert_eq!(code("zh-TW"), Some("chi_tra"));
        assert_eq!(code("iw"), Some("heb"));
        assert_eq!(code("pt-BR"), Some("por"));
        assert_eq!(tesseract_code(LangId::AUTO), None);
    }

    #[test]
    fn a_missing_command_asks_for_one() {
        let p = CommandOcr::new();
        let http = MockHttp::new();
        let opts = resolve_options(p.options(), None);
        with_test_host(&http, &opts, |host| {
            let err = p.recognize(host, &OcrRequest::new(ImageSource::Bytes(vec![1, 2, 3]))).unwrap_err();
            assert!(matches!(err, Error::MissingCredential { ref what, .. } if what == "command"), "{err:?}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn runs_the_command_and_reads_stdout() {
        let p = CommandOcr::new();
        let http = MockHttp::new();
        let mut user = glossa_modules::OptionValues::new();
        // Print the image back plus the language: proves both reached the
        // program, each as one argument.
        user.insert(
            "command".into(),
            serde_json::json!(r#"sh -c 'cat "$1"; printf " %s\n\f" "$2"' sh {image} {lang3}"#),
        );
        let opts = resolve_options(p.options(), Some(&user));
        with_test_host(&http, &opts, |host| {
            let mut req = OcrRequest::new(ImageSource::Bytes(b"line one\nline two".to_vec()));
            req.lang = LangId::RUSSIAN;
            let r = p.recognize(host, &req).unwrap();
            assert_eq!(r.text, "line one\nline two rus");
            req.preserve_line_breaks = false;
            req.lang = LangId::AUTO;
            assert_eq!(p.recognize(host, &req).unwrap().text, "line one line two eng");
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_reports_its_stderr() {
        let p = CommandOcr::new();
        let http = MockHttp::new();
        let mut user = glossa_modules::OptionValues::new();
        user.insert("command".into(), serde_json::json!("sh -c 'echo no such language >&2; exit 1'"));
        let opts = resolve_options(p.options(), Some(&user));
        with_test_host(&http, &opts, |host| {
            let err = p.recognize(host, &OcrRequest::new(ImageSource::Bytes(vec![0]))).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("code 1") && msg.contains("no such language"), "{msg}");
        });
    }
}
