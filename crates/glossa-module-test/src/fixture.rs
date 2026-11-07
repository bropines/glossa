//! The fixture format: `tests/*.toml` or `tests/*.json` beside a module.
//!
//! ```toml
//! # Options and recorded HTTP shared by every case in the file.
//! options = { max_definitions = 2 }
//!
//! [[case]]
//! name = "looks up a word"
//! op = "dictionary"              # translate | detect | dictionary | speak | ocr
//! text = "hello"
//! from = "en"
//! to = "ru"
//!
//! [case.expect]
//! headword = "hello"
//! contains = ["greeting"]
//!
//! [[case.http]]                  # what the service answers, so the case runs offline
//! url = "https://api.dictionaryapi.dev/api/v2/entries/en/hello"
//! body_file = "responses/hello.json"
//! ```
//!
//! Every table refuses unknown keys: a misspelt expectation must fail the
//! fixture, not pass it by checking nothing.

use std::path::{Path, PathBuf};

use glossa_modules::OptionValues;
use serde::{Deserialize, Serialize};

/// One fixture file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    /// Option values for every case (fakes only: never a real key).
    #[serde(default)]
    pub options: OptionValues,
    /// Recorded responses every case may use, after its own.
    #[serde(default)]
    pub http: Vec<Exchange>,
    #[serde(default, rename = "case")]
    pub cases: Vec<Case>,
}

/// One call and what to expect of it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub op: Op,
    /// The text to translate, detect, look up or speak.
    #[serde(default)]
    pub text: String,
    /// Source language for `translate` and `dictionary`.
    #[serde(default = "auto")]
    pub from: String,
    /// Target language for `translate` and `dictionary`.
    #[serde(default = "english")]
    pub to: String,
    /// The language for `speak` (default `en`) and `ocr` (default `auto`).
    #[serde(default)]
    pub lang: Option<String>,
    /// `speak`: the slower second reading.
    #[serde(default)]
    pub slow: bool,
    /// `dictionary`: exact headword match (default true).
    #[serde(default)]
    pub exact: Option<bool>,
    /// `ocr`: the picture.
    #[serde(default)]
    pub image: Option<ImageSpec>,
    /// `ocr`: `full_text`, `blocks`, `lines` or `words` (default `words`).
    #[serde(default)]
    pub granularity: Option<String>,
    /// `ocr`: ask for a translation in the same call.
    #[serde(default)]
    pub translate_to: Option<String>,
    /// Option values for this case, over the file's.
    #[serde(default)]
    pub options: OptionValues,
    /// Recorded responses for this case, tried before the file's.
    #[serde(default)]
    pub http: Vec<Exchange>,
    /// The case needs the real service (the module does its own networking,
    /// so nothing can be recorded): skipped unless run with `--live` or
    /// `--record`.
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub expect: Expect,
}

fn auto() -> String {
    "auto".into()
}

fn english() -> String {
    "en".into()
}

/// The operation a case calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Translate,
    #[serde(alias = "detect_language")]
    Detect,
    #[serde(alias = "dict")]
    Dictionary,
    #[serde(alias = "listen")]
    Speak,
    #[serde(alias = "recognize")]
    Ocr,
}

impl Op {
    /// The manifest capability this operation needs.
    pub fn capability(self) -> &'static str {
        match self {
            Op::Translate => "translate",
            Op::Detect => "detect_language",
            Op::Dictionary => "dictionary",
            Op::Speak => "listen",
            Op::Ocr => "ocr",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Op::Translate => "translate",
            Op::Detect => "detect",
            Op::Dictionary => "dictionary",
            Op::Speak => "speak",
            Op::Ocr => "ocr",
        }
    }

    /// Every operation, with the capability that enables it.
    pub const ALL: [Op; 5] = [Op::Translate, Op::Detect, Op::Dictionary, Op::Speak, Op::Ocr];
}

/// The picture for an `ocr` case: a file beside the fixture, or one the
/// harness draws (dark bars on white, one per "line"), so a template needs
/// no binary file.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ImageSpec {
    File(String),
    Drawn {
        width: u32,
        height: u32,
        #[serde(default = "one")]
        lines: u32,
    },
}

fn one() -> u32 {
    1
}

/// What a case expects. Everything is optional; the shape checks run
/// regardless (a translation is not empty, OCR boxes lie inside the image…).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// The whole text, trimmed: the translation, the recognised text, or
    /// the detected language's code.
    pub text: Option<String>,
    /// Substrings the text must contain. For a dictionary article, its
    /// plain-text rendering and HTML.
    #[serde(default)]
    pub contains: OneOrMany,
    /// The detected language (translate, detect, ocr).
    pub detected: Option<String>,
    /// The article's headword (dictionary).
    pub headword: Option<String>,
    /// The audio's MIME type (speak).
    pub mime: Option<String>,
    /// The call must fail with this kind: `missing_credential`,
    /// `unsupported`, `source_language`, `target_language`, `rate_limited`,
    /// `network`, `http`, `bad_response`, `not_found`, `timeout` or
    /// `module` (any other module error).
    pub error: Option<String>,
    /// The error message must contain this.
    pub error_contains: Option<String>,
    /// The call must finish within this many milliseconds.
    pub max_ms: Option<u64>,
    /// A partial match on the result as JSON: every key given must be
    /// present with an equal value; arrays match element by element for as
    /// many elements as are given.
    pub result: Option<serde_json::Value>,
}

/// One string or several.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(from = "OneOrManyRepr")]
pub struct OneOrMany(pub Vec<String>);

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrManyRepr {
    One(String),
    Many(Vec<String>),
}

impl From<OneOrManyRepr> for OneOrMany {
    fn from(r: OneOrManyRepr) -> Self {
        match r {
            OneOrManyRepr::One(s) => OneOrMany(vec![s]),
            OneOrManyRepr::Many(v) => OneOrMany(v),
        }
    }
}

/// One recorded HTTP exchange: a request the module makes (matched by
/// method and URL prefix) and the answer it gets.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    #[serde(default = "get")]
    pub method: String,
    /// Matches any request URL starting with this.
    pub url: String,
    #[serde(default = "ok")]
    pub status: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_base64: String,
    /// The body from a file, relative to the fixture file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_file: String,
}

fn get() -> String {
    "GET".into()
}

fn ok() -> u16 {
    200
}

/// A file of exchanges written by `--record`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub exchanges: Vec<Exchange>,
}

/// The folder fixtures live in.
pub fn dir(module: &Path) -> PathBuf {
    module.join("tests")
}

/// The fixture files of a module, sorted: `tests/*.toml` and `tests/*.json`.
pub fn files(module: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir(module)) else { return Vec::new() };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "toml" || e == "json"))
        .collect();
    out.sort();
    out
}

/// Parse one fixture file.
pub fn load(path: &Path) -> Result<FixtureFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let parsed = if path.extension().is_some_and(|e| e == "json") {
        serde_json::from_str(&text).map_err(|e| e.to_string())
    } else {
        toml::from_str(&text).map_err(|e| e.to_string())
    };
    let file: FixtureFile = parsed.map_err(|e| format!("{}: {e}", path.display()))?;
    if file.cases.is_empty() {
        return Err(format!("{}: no [[case]] in the file", path.display()));
    }
    Ok(file)
}

/// Where `--record` keeps the exchanges of a case:
/// `tests/recorded/<fixture file stem>/<case name as a file name>.json`.
pub fn recording_path(fixture: &Path, case: &str) -> PathBuf {
    let stem = fixture.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = fixture.parent().unwrap_or(Path::new(".")).join("recorded").join(stem);
    dir.join(format!("{}.json", slug(case)))
}

/// A case name as a portable file name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "case".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixture_parses_with_defaults_and_aliases() {
        let f: FixtureFile = toml::from_str(
            r#"
            options = { n = 2 }
            [[case]]
            name = "one"
            op = "recognize"
            image = { width = 100, height = 40, lines = 2 }
            [case.expect]
            contains = "x"
            [[case.http]]
            url = "https://a/"
            body = "{}"
            "#,
        )
        .unwrap();
        let c = &f.cases[0];
        assert_eq!(c.op, Op::Ocr);
        assert_eq!((c.from.as_str(), c.to.as_str()), ("auto", "en"));
        assert!(matches!(c.image, Some(ImageSpec::Drawn { width: 100, height: 40, lines: 2 })));
        assert_eq!(c.expect.contains.0, ["x"]);
        assert_eq!((c.http[0].method.as_str(), c.http[0].status), ("GET", 200));
    }

    #[test]
    fn misspelt_keys_are_refused() {
        let e =
            toml::from_str::<FixtureFile>("[[case]]\nname = \"a\"\nop = \"translate\"\n[case.expect]\ntxet = \"b\"")
                .unwrap_err();
        assert!(e.to_string().contains("txet"), "{e}");
        assert!(toml::from_str::<FixtureFile>("[[case]]\nname = \"a\"\nop = \"teleport\"").is_err());
    }

    #[test]
    fn recordings_go_beside_the_fixture() {
        let p = recording_path(Path::new("m/tests/basic.toml"), "Looks up: «hello»!");
        assert_eq!(p, Path::new("m/tests/recorded/basic/looks-up-hello.json"));
        assert_eq!(slug("???"), "case");
    }
}
