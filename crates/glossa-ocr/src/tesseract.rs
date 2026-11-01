//! Tesseract, the open-source recogniser, run as a program: offline, on every
//! OS, for the languages whose `.traineddata` is installed.
//!
//! The `tesseract` executable is found on `PATH` or where its installers put
//! it (Homebrew's folders, which a Finder-launched app does not have on its
//! `PATH`; `C:\Program Files\Tesseract-OCR` for the UB Mannheim build), or
//! taken from the settings. It reads a temporary copy of the picture and
//! prints TSV: one row per page, block, paragraph, line and word, with boxes
//! in pixels and a confidence per word. That gives the same lines-of-words
//! structure the other local engines produce, without an XML parser.
//!
//! Nothing is checked up front: when Tesseract fails to load a language,
//! `--list-langs` is asked afterwards so the error can say what is installed
//! and how to add the rest.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use glossa_core::config::TesseractConfig;
use glossa_core::ocr::{Rect, VerticalTextMode};
use glossa_core::{Capabilities, Error, LangId, OcrRequest, OcrResult, ProviderInfo, Result};
use glossa_modules::{Host, Provider};

use crate::command::{split_words, tesseract_code};
use crate::layout::{self, RawLine, RawWord, Shape};
use crate::process;
use crate::tempimage::TempImage;

pub const TESSERACT_ID: &str = "tesseract";
const NAME: &str = "Tesseract";
/// A large page with several languages takes a few seconds; a minute means
/// something is stuck.
const TIMEOUT: Duration = Duration::from_secs(60);
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Tesseract {
    info: ProviderInfo,
    cfg: TesseractConfig,
}

impl Tesseract {
    pub fn new(cfg: TesseractConfig) -> Self {
        let mut info = ProviderInfo::new(TESSERACT_ID, NAME, Capabilities::OCR);
        info.description =
            "The open-source recogniser, run as a program. Offline; reads the languages whose data is installed."
                .into();
        info.homepage = "https://tesseract-ocr.github.io/".into();
        Self { info, cfg }
    }

    fn executable(&self) -> Result<PathBuf> {
        find_executable(&self.cfg.path).ok_or_else(|| not_installed(&self.cfg.path))
    }

    /// Why a run that exited with an error failed, in words the user can act
    /// on.
    fn failure(&self, exe: &Path, langs: &str, out: &process::Output) -> Error {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if is_language_load_failure(&stderr) {
            let installed = list_languages(exe, &self.cfg.tessdata_dir).unwrap_or_default();
            return Error::Module { module: TESSERACT_ID.into(), detail: missing_language_message(langs, &installed) };
        }
        let tail = out.stderr_tail();
        Error::Module {
            module: TESSERACT_ID.into(),
            detail: format!(
                "{} exited with {}{}",
                exe.display(),
                out.code.map_or("a signal".to_owned(), |c| format!("code {c}")),
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            ),
        }
    }
}

impl Provider for Tesseract {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        host.check_cancelled()?;
        let exe = self.executable()?;
        let langs = language_arg(req.lang, req.vertical, &self.cfg.auto_languages)?;
        let bytes = req.image.to_bytes()?;
        let image = TempImage::write(&bytes)?;
        let mut cmd = Command::new(&exe);
        cmd.args(build_args(image.path(), &langs, &self.cfg)?);
        let started = Instant::now();
        let out = process::run(cmd, TIMEOUT, Some(host.cancel))?;
        if !out.success() {
            return Err(self.failure(&exe, &langs, &out));
        }
        let page = parse_tsv(&String::from_utf8_lossy(&out.stdout));
        let raw: Vec<RawLine> = page.lines.iter().map(TessLine::to_raw).collect();
        let mut result = layout::build_result(&raw, req, page.size.unwrap_or_default());
        if page.size.is_none() {
            result.image_size = None;
        }
        // Confidence has no home in `OcrBlock` yet; the per-line values are
        // ready in `TessLine::confidence` for when it does.
        tracing::debug!(
            lines = page.lines.len(),
            confidence = page.mean_confidence().unwrap_or(-1.0),
            langs = %langs,
            "tesseract read the picture"
        );
        result.provider_id = TESSERACT_ID.into();
        result.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(result)
    }
}

// -- finding the program ---------------------------------------------------------

/// The executable to run: `configured` when set (a file, or a folder holding
/// it), else the first `tesseract` on `PATH` or in a usual install folder.
pub fn find_executable(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.is_dir() {
            let inner = p.join(exe_name());
            return inner.is_file().then_some(inner);
        }
        // A bare name is looked up on PATH like any command.
        if p.components().count() == 1 && !p.is_file() {
            return search(std::env::var_os("PATH"), &[], configured);
        }
        return p.is_file().then_some(p);
    }
    search(std::env::var_os("PATH"), &fallback_dirs(), exe_name())
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    }
}

/// `name` in each directory of `path`, then in `extra`.
fn search(path: Option<OsString>, extra: &[PathBuf], name: &str) -> Option<PathBuf> {
    let dirs = path.map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    dirs.iter().chain(extra).map(|d| d.join(name)).find(|c| c.is_file()).or_else(|| {
        // `PATH` entries on Windows may name the program without `.exe`.
        if cfg!(windows) && !name.ends_with(".exe") {
            search_exact(&dirs, extra, &format!("{name}.exe"))
        } else {
            None
        }
    })
}

fn search_exact(dirs: &[PathBuf], extra: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().chain(extra).map(|d| d.join(name)).find(|c| c.is_file())
}

/// Where installers put Tesseract when it is not on the app's `PATH`.
fn fallback_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(PathBuf::from(dir).join("Tesseract-OCR"));
            }
        }
        out.push(PathBuf::from(r"C:\Program Files\Tesseract-OCR"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            out.push(PathBuf::from(local).join("Programs").join("Tesseract-OCR"));
        }
    } else {
        // Homebrew on Apple silicon, Homebrew on Intel and most Unix
        // installs from source, MacPorts, Linuxbrew, distributions.
        for dir in
            ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/home/linuxbrew/.linuxbrew/bin", "/usr/bin"]
        {
            out.push(PathBuf::from(dir));
        }
    }
    out
}

/// The error for a missing program, with how to install it here.
fn not_installed(configured: &str) -> Error {
    if !configured.trim().is_empty() {
        return Error::Config(format!(
            "Tesseract was not found at {}; correct the path in the OCR settings or clear it to search the usual places",
            configured.trim()
        ));
    }
    Error::Config(format!("Tesseract was not found. {}", install_hint()))
}

/// How to install Tesseract on this OS.
pub fn install_hint() -> &'static str {
    if cfg!(windows) {
        "Install the UB Mannheim build (https://github.com/UB-Mannheim/tesseract/wiki), \
         choosing the languages you need, or `winget install UB-Mannheim.TesseractOCR`; \
         or set the path to tesseract.exe in the OCR settings."
    } else if cfg!(target_os = "macos") {
        "Install it with Homebrew: `brew install tesseract tesseract-lang` \
         (or MacPorts: `sudo port install tesseract`), or set the path to tesseract in the OCR settings."
    } else {
        "Install it from your distribution: `sudo apt install tesseract-ocr tesseract-ocr-eng` (Debian, Ubuntu), \
         `sudo dnf install tesseract tesseract-langpack-eng` (Fedora), `sudo pacman -S tesseract tesseract-data-eng` \
         (Arch); or set the path to tesseract in the OCR settings."
    }
}

/// How to add the language pack `code` on this OS.
fn language_install_hint(code: &str) -> String {
    if cfg!(windows) {
        format!(
            "run the UB Mannheim installer again and tick it under Additional language data, or put {code}.traineddata \
             (from https://github.com/tesseract-ocr/tessdata_fast) into the tessdata folder"
        )
    } else if cfg!(target_os = "macos") {
        format!("`brew install tesseract-lang` adds every language, or put {code}.traineddata into the tessdata folder")
    } else {
        let deb = code.replace('_', "-");
        format!(
            "`sudo apt install tesseract-ocr-{deb}` (Debian, Ubuntu), `sudo dnf install tesseract-langpack-{code}` \
             (Fedora), `sudo pacman -S tesseract-data-{code}` (Arch)"
        )
    }
}

// -- the command line ------------------------------------------------------------

/// The `-l` value for a picture in `lang`: Tesseract's code, the vertical
/// model for vertical CJK when asked to keep it, and `auto` (the configured
/// list, else English) when the language is automatic.
pub fn language_arg(lang: LangId, vertical: VerticalTextMode, auto: &str) -> Result<String> {
    if !lang.is_language() {
        let list: Vec<&str> = auto.split(['+', ',', ' ']).map(str::trim).filter(|s| !s.is_empty()).collect();
        return Ok(if list.is_empty() { "eng".to_owned() } else { list.join("+") });
    }
    let code = tesseract_code(lang).ok_or_else(|| Error::SourceLanguageUnsupported { provider: NAME.into(), lang })?;
    if vertical == VerticalTextMode::Keep && matches!(code, "jpn" | "chi_sim" | "chi_tra" | "kor") {
        return Ok(format!("{code}_vert+{code}"));
    }
    Ok(code.to_owned())
}

/// The arguments after the program: the picture, output to stdout, the
/// options, and the `tsv` config last (Tesseract reads config names after
/// every option).
pub fn build_args(image: &Path, langs: &str, cfg: &TesseractConfig) -> Result<Vec<OsString>> {
    let mut args: Vec<OsString> = vec![image.into(), "-".into(), "-l".into(), langs.into()];
    if let Some(psm) = cfg.psm {
        args.extend(["--psm".into(), psm.to_string().into()]);
    }
    if let Some(oem) = cfg.oem {
        args.extend(["--oem".into(), oem.to_string().into()]);
    }
    if !cfg.tessdata_dir.trim().is_empty() {
        args.extend(["--tessdata-dir".into(), cfg.tessdata_dir.trim().into()]);
    }
    let extra = split_words(&cfg.extra_args).map_err(|e| Error::Config(format!("Tesseract arguments: {e}")))?;
    args.extend(extra.into_iter().map(OsString::from));
    args.push("tsv".into());
    Ok(args)
}

fn is_language_load_failure(stderr: &str) -> bool {
    stderr.contains("Failed loading language")
        || stderr.contains("couldn't load any languages")
        || stderr.contains("Error opening data file")
}

/// The message for a language Tesseract could not load.
pub fn missing_language_message(langs: &str, installed: &[String]) -> String {
    let missing: Vec<&str> = langs.split('+').filter(|l| !l.is_empty() && !installed.iter().any(|i| i == l)).collect();
    let list = if installed.is_empty() { "none".to_owned() } else { installed.join(", ") };
    let wanted = if missing.is_empty() { langs.to_owned() } else { missing.join(", ") };
    let how = missing.first().map(|c| language_install_hint(c)).unwrap_or_default();
    let mut msg = format!("Tesseract has no data for {wanted} (installed: {list})");
    if !how.is_empty() {
        msg.push_str(". To add it: ");
        msg.push_str(&how);
    }
    msg
}

/// What `tesseract --list-langs` printed: one code per line after a header
/// (`List of available languages in "/usr/share/tessdata/" (3):` in 5.x,
/// `List of available languages (3):` in 4.x). `osd` is the script
/// detector, not a language, and is left out.
pub fn parse_list_langs(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("List of") && !l.contains(' ') && !l.contains(':'))
        .filter(|l| *l != "osd")
        .map(str::to_owned)
        .collect()
}

/// The languages Tesseract has data for, e.g. `["eng", "rus"]`.
pub fn installed_languages(cfg: &TesseractConfig) -> Result<Vec<String>> {
    let exe = find_executable(&cfg.path).ok_or_else(|| not_installed(&cfg.path))?;
    list_languages(&exe, &cfg.tessdata_dir)
}

fn list_languages(exe: &Path, tessdata_dir: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new(exe);
    if !tessdata_dir.trim().is_empty() {
        cmd.args(["--tessdata-dir", tessdata_dir.trim()]);
    }
    cmd.arg("--list-langs");
    let out = process::run(cmd, LIST_TIMEOUT, None)?;
    // 3.x printed the list on stderr.
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(parse_list_langs(&text))
}

// -- reading the output ----------------------------------------------------------

/// A word from the TSV.
#[derive(Debug, Clone, PartialEq)]
pub struct TessWord {
    pub text: String,
    pub bounds: Rect,
    /// 0–100.
    pub confidence: f32,
}

/// A line of words, keyed by where Tesseract put it.
#[derive(Debug, Clone, PartialEq)]
pub struct TessLine {
    /// Block and paragraph numbers, for telling paragraphs apart.
    pub block: u32,
    pub paragraph: u32,
    pub bounds: Rect,
    pub words: Vec<TessWord>,
}

impl TessLine {
    /// The words joined, without spaces between characters of scripts that
    /// do not use them (Tesseract reports each Chinese character as a word).
    pub fn text(&self) -> String {
        let mut out = String::new();
        for w in &self.words {
            let glue = match (out.chars().last(), w.text.chars().next()) {
                (Some(a), Some(b)) => !(is_unspaced(a) && is_unspaced(b)),
                _ => false,
            };
            if glue {
                out.push(' ');
            }
            out.push_str(&w.text);
        }
        out
    }

    /// The line's confidence, 0–100: the mean of its words'.
    pub fn confidence(&self) -> Option<f32> {
        (!self.words.is_empty()).then(|| self.words.iter().map(|w| w.confidence).sum::<f32>() / self.words.len() as f32)
    }

    fn to_raw(&self) -> RawLine {
        RawLine {
            text: self.text(),
            shape: Some(Shape::Rect(self.bounds)),
            words: self
                .words
                .iter()
                .map(|w| RawWord { text: w.text.clone(), shape: Some(Shape::Rect(w.bounds)) })
                .collect(),
        }
    }
}

/// Characters of scripts written without spaces between words.
fn is_unspaced(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF     // Hiragana, Katakana
        | 0x3400..=0x4DBF   // CJK Extension A
        | 0x4E00..=0x9FFF   // CJK Unified Ideographs
        | 0xF900..=0xFAFF   // CJK Compatibility Ideographs
        | 0x3000..=0x303F   // CJK punctuation
        | 0xFF00..=0xFFEF   // full-width forms
        | 0x0E00..=0x0EFF   // Thai, Lao
        | 0x1000..=0x109F   // Myanmar
        | 0x1780..=0x17FF   // Khmer
        | 0x20000..=0x2FFFF // CJK Extensions B and later
    )
}

/// Everything read from one TSV.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TessPage {
    /// The picture's size, from the page row.
    pub size: Option<(u32, u32)>,
    pub lines: Vec<TessLine>,
}

impl TessPage {
    /// The mean word confidence, 0–100.
    pub fn mean_confidence(&self) -> Option<f32> {
        let words: Vec<f32> = self.lines.iter().flat_map(|l| l.words.iter().map(|w| w.confidence)).collect();
        (!words.is_empty()).then(|| words.iter().sum::<f32>() / words.len() as f32)
    }
}

/// Parse Tesseract's TSV (`level page_num block_num par_num line_num
/// word_num left top width height conf text`). Rows that do not parse are
/// skipped; words that are only whitespace (Tesseract reports some with a
/// confidence of -1 or 0) are dropped, and so are lines left empty.
pub fn parse_tsv(tsv: &str) -> TessPage {
    let mut page = TessPage::default();
    let mut current: Option<((u32, u32, u32, u32), TessLine)> = None;
    for row in tsv.lines() {
        let cols: Vec<&str> = row.splitn(12, '\t').collect();
        if cols.len() < 11 {
            continue;
        }
        let num = |i: usize| cols[i].trim().parse::<i64>().ok();
        let (Some(level), Some(pg), Some(block), Some(par), Some(line)) = (num(0), num(1), num(2), num(3), num(4))
        else {
            continue; // the header
        };
        let (Some(left), Some(top), Some(width), Some(height)) = (num(6), num(7), num(8), num(9)) else { continue };
        let bounds = Rect::new(left as f32, top as f32, width as f32, height as f32);
        match level {
            1 => {
                if page.size.is_none() && width > 0 && height > 0 {
                    page.size = Some((width as u32, height as u32));
                }
            }
            4 => {
                if let Some((_, l)) = current.take() {
                    push_line(&mut page, l);
                }
                let key = (pg as u32, block as u32, par as u32, line as u32);
                current =
                    Some((key, TessLine { block: block as u32, paragraph: par as u32, bounds, words: Vec::new() }));
            }
            5 => {
                let text = cols.get(11).map(|t| t.trim_end_matches(['\r', '\n'])).unwrap_or("");
                if text.trim().is_empty() {
                    continue;
                }
                let confidence = cols[10].trim().parse::<f32>().unwrap_or(-1.0).max(0.0);
                let key = (pg as u32, block as u32, par as u32, line as u32);
                // A word whose line row was missing still gets a line.
                if current.as_ref().is_none_or(|(k, _)| *k != key) {
                    if let Some((_, l)) = current.take() {
                        push_line(&mut page, l);
                    }
                    current =
                        Some((key, TessLine { block: block as u32, paragraph: par as u32, bounds, words: Vec::new() }));
                }
                if let Some((_, l)) = current.as_mut() {
                    l.words.push(TessWord { text: text.trim().to_owned(), bounds, confidence });
                }
            }
            _ => {}
        }
    }
    if let Some((_, l)) = current.take() {
        push_line(&mut page, l);
    }
    page
}

fn push_line(page: &mut TessPage, mut line: TessLine) {
    if line.words.is_empty() {
        return;
    }
    if line.bounds.is_empty() {
        if let Some(u) = layout::union(line.words.iter().map(|w| w.bounds)) {
            line.bounds = u;
        }
    }
    page.lines.push(line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::{ImageSource, OcrGranularity};
    use glossa_modules::provider::with_test_host;
    use glossa_net::mock::MockHttp;

    /// What Tesseract 4.1.1 printed for a picture of two lines, English and
    /// Russian (`tesseract sample.png - -l eng+rus tsv`).
    const TSV: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t520\t150\t-1\t
2\t1\t1\t0\t0\t0\t23\t25\t183\t26\t-1\t
3\t1\t1\t1\t0\t0\t23\t25\t183\t26\t-1\t
4\t1\t1\t1\t1\t0\t23\t25\t183\t26\t-1\t
5\t1\t1\t1\t1\t1\t23\t25\t80\t26\t96\tHello,
5\t1\t1\t1\t1\t2\t116\t25\t90\t23\t96\tworld!
2\t1\t2\t0\t0\t0\t23\t91\t180\t28\t-1\t
3\t1\t2\t1\t0\t0\t23\t91\t180\t28\t-1\t
4\t1\t2\t1\t1\t0\t23\t91\t180\t28\t-1\t
5\t1\t2\t1\t1\t1\t23\t91\t111\t28\t95\tПривет
5\t1\t2\t1\t1\t2\t147\t97\t56\t22\t96\tмир
";

    #[test]
    fn tsv_becomes_lines_of_words_with_boxes_and_confidence() {
        let page = parse_tsv(TSV);
        assert_eq!(page.size, Some((520, 150)));
        assert_eq!(page.lines.len(), 2);
        let first = &page.lines[0];
        assert_eq!(first.text(), "Hello, world!");
        assert_eq!(first.bounds, Rect::new(23.0, 25.0, 183.0, 26.0));
        assert_eq!(first.words[1].bounds, Rect::new(116.0, 25.0, 90.0, 23.0));
        assert_eq!(first.confidence(), Some(96.0));
        assert_eq!(page.lines[1].text(), "Привет мир");
        assert_eq!(page.lines[1].block, 2);
        assert!((page.mean_confidence().unwrap() - 95.75).abs() < 1e-4);
    }

    #[test]
    fn tsv_quirks_are_tolerated() {
        // CRLF (Windows builds), a blank word, a word with no line row before
        // it, a fractional confidence (5.x prints 96.432...), garbage rows.
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\r
1\t1\t0\t0\t0\t0\t0\t0\t100\t40\t-1\t\r
4\t1\t1\t1\t1\t0\t0\t0\t50\t10\t-1\t\r
5\t1\t1\t1\t1\t1\t0\t0\t10\t10\t95.5\t\u{4F60}\r
5\t1\t1\t1\t1\t2\t12\t0\t10\t10\t93.5\t\u{597D}\r
5\t1\t1\t1\t1\t3\t30\t0\t10\t10\t-1\t \r
not a row\r
5\t1\t1\t1\t2\t1\t0\t20\t30\t10\t88\tnext\r
";
        let page = parse_tsv(tsv);
        assert_eq!(page.size, Some((100, 40)));
        let texts: Vec<String> = page.lines.iter().map(TessLine::text).collect();
        assert_eq!(texts, ["\u{4F60}\u{597D}", "next"], "no space between Chinese characters");
        assert_eq!(page.lines[0].confidence(), Some(94.5));
        assert_eq!(page.lines[1].bounds, Rect::new(0.0, 20.0, 30.0, 10.0), "a line made from its word");
        assert_eq!(parse_tsv(""), TessPage::default());
    }

    #[test]
    fn the_result_has_lines_words_and_the_page_size() {
        let page = parse_tsv(TSV);
        let raw: Vec<RawLine> = page.lines.iter().map(TessLine::to_raw).collect();
        let mut req = OcrRequest::new(ImageSource::Bytes(Vec::new()));
        req.granularity = OcrGranularity::Words;
        let r = layout::build_result(&raw, &req, page.size.unwrap());
        assert_eq!(r.text, "Hello, world!\nПривет мир");
        assert_eq!(r.image_size, Some((520, 150)));
        assert_eq!(r.blocks.len(), 2);
        assert_eq!(r.blocks[1].children[1].text, "мир");
        assert_eq!(r.blocks[1].children[1].bounds, Some(Rect::new(147.0, 97.0, 56.0, 22.0)));
    }

    #[test]
    fn languages_map_to_tesseract_codes() {
        let lang = |c: &str| LangId::from_code(c).unwrap();
        let arg = |c: &str| language_arg(lang(c), VerticalTextMode::Auto, "").unwrap();
        assert_eq!(arg("en"), "eng");
        assert_eq!(arg("ru"), "rus");
        assert_eq!(arg("uk"), "ukr");
        assert_eq!(arg("ja"), "jpn");
        assert_eq!(arg("ko"), "kor");
        assert_eq!(arg("zh-CN"), "chi_sim");
        assert_eq!(arg("zh-TW"), "chi_tra");
        assert_eq!(arg("de"), "deu");
        assert_eq!(language_arg(lang("ja"), VerticalTextMode::Keep, "").unwrap(), "jpn_vert+jpn");
        assert_eq!(language_arg(lang("ru"), VerticalTextMode::Keep, "").unwrap(), "rus");
        assert_eq!(language_arg(LangId::AUTO, VerticalTextMode::Auto, "").unwrap(), "eng");
        assert_eq!(language_arg(LangId::AUTO, VerticalTextMode::Auto, " eng, rus ").unwrap(), "eng+rus");
        assert_eq!(language_arg(LangId::AUTO, VerticalTextMode::Auto, "eng+jpn").unwrap(), "eng+jpn");
        assert!(matches!(
            language_arg(lang("hmn"), VerticalTextMode::Auto, ""),
            Err(Error::SourceLanguageUnsupported { .. })
        ));
    }

    #[test]
    fn arguments_put_options_before_the_tsv_config() {
        let cfg = TesseractConfig {
            psm: Some(6),
            oem: Some(1),
            tessdata_dir: "/data/tess".into(),
            extra_args: "-c preserve_interword_spaces=1".into(),
            ..Default::default()
        };
        let args = build_args(Path::new("/tmp/a b.png"), "eng+rus", &cfg).unwrap();
        let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            [
                "/tmp/a b.png",
                "-",
                "-l",
                "eng+rus",
                "--psm",
                "6",
                "--oem",
                "1",
                "--tessdata-dir",
                "/data/tess",
                "-c",
                "preserve_interword_spaces=1",
                "tsv"
            ]
        );
        let plain = build_args(Path::new("x.png"), "eng", &TesseractConfig::default()).unwrap();
        assert_eq!(plain.len(), 5);
        let bad = TesseractConfig { extra_args: "'open".into(), ..Default::default() };
        assert!(matches!(build_args(Path::new("x.png"), "eng", &bad), Err(Error::Config(_))));
    }

    #[test]
    fn installed_languages_are_read_from_either_version() {
        let v5 = "List of available languages in \"/usr/share/tesseract-ocr/5/tessdata/\" (3):\neng\nosd\nrus\n";
        assert_eq!(parse_list_langs(v5), ["eng", "rus"]);
        let v4 = "List of available languages (4):\r\nchi_sim\r\neng\r\nosd\r\njpn_vert\r\n";
        assert_eq!(parse_list_langs(v4), ["chi_sim", "eng", "jpn_vert"]);
        assert!(parse_list_langs("").is_empty());
    }

    #[test]
    fn a_missing_language_names_what_is_installed_and_how_to_add_it() {
        let m = missing_language_message("eng+jpn", &["eng".into(), "rus".into()]);
        assert!(m.contains("no data for jpn") && m.contains("installed: eng, rus"), "{m}");
        assert!(m.contains("jpn"), "{m}");
        let none = missing_language_message("rus", &[]);
        assert!(none.contains("installed: none"), "{none}");
        assert!(is_language_load_failure("Failed loading language 'xyz'\nTesseract couldn't load any languages!"));
        assert!(!is_language_load_failure("Error in pixReadStream"));
    }

    #[test]
    fn a_missing_program_says_how_to_install_it() {
        let msg = not_installed("").to_string();
        let expected = if cfg!(windows) {
            "UB Mannheim"
        } else if cfg!(target_os = "macos") {
            "brew install"
        } else {
            "apt install"
        };
        assert!(msg.contains(expected), "{msg}");
        assert!(not_installed("/nowhere/tesseract").to_string().contains("/nowhere/tesseract"));
    }

    #[test]
    fn the_program_is_found_on_the_path_or_where_configured() {
        let dir = std::env::temp_dir().join(format!("glossa-tess-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name());
        std::fs::write(&exe, b"").unwrap();
        let path = std::env::join_paths([PathBuf::from("/definitely/not/here"), dir.clone()]).unwrap();
        assert_eq!(search(Some(path), &[], exe_name()), Some(exe.clone()));
        assert_eq!(search(None, std::slice::from_ref(&dir), exe_name()), Some(exe.clone()), "fallback folders");
        assert_eq!(search(None, &[], exe_name()), None);
        let configured = exe.to_string_lossy().into_owned();
        assert_eq!(find_executable(&configured), Some(exe.clone()));
        assert_eq!(find_executable(&dir.to_string_lossy()), Some(exe.clone()), "a folder holding it");
        assert_eq!(find_executable(&dir.join("missing").to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_configured_path_that_is_wrong_is_a_configuration_error() {
        let p = Tesseract::new(TesseractConfig { path: "/definitely/not/tesseract".into(), ..Default::default() });
        let http = MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        let err = with_test_host(&http, &opts, |h| p.recognize(h, &OcrRequest::new(ImageSource::Bytes(vec![1]))))
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    /// A stand-in program that prints the TSV above, or fails like Tesseract
    /// does for a language it has no data for.
    #[cfg(unix)]
    fn fake_tesseract(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("tesseract");
        let tsv_file = dir.join("out.tsv");
        std::fs::write(&tsv_file, TSV).unwrap();
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --list-langs ]; then printf 'List of available languages (2):\\neng\\nosd\\n'; exit 0; fi\n\
             case \"$4\" in *xxx*) echo \"Failed loading language 'xxx'\" >&2; echo \"Tesseract couldn't load any languages!\" >&2; exit 1;; esac\n\
             [ -f \"$1\" ] || exit 3\n\
             cat '{}'\n",
            tsv_file.display()
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    #[cfg(unix)]
    #[test]
    fn runs_the_program_and_reads_its_tsv() {
        let dir = std::env::temp_dir().join(format!("glossa-tess-fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = fake_tesseract(&dir);
        let cfg = TesseractConfig { path: exe.to_string_lossy().into_owned(), ..Default::default() };
        let p = Tesseract::new(cfg.clone());
        let http = MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        let mut req = OcrRequest::new(ImageSource::Bytes(vec![0x89, b'P', b'N', b'G']));
        req.granularity = OcrGranularity::Lines;
        let r = with_test_host(&http, &opts, |h| p.recognize(h, &req)).unwrap();
        assert_eq!(r.text, "Hello, world!\nПривет мир");
        assert_eq!(r.provider_id, TESSERACT_ID);
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(23.0, 25.0, 183.0, 26.0)));

        let bad = Tesseract::new(TesseractConfig { auto_languages: "xxx".into(), ..cfg.clone() });
        let err = with_test_host(&http, &opts, |h| bad.recognize(h, &OcrRequest::new(ImageSource::Bytes(vec![1]))))
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no data for xxx") && msg.contains("installed: eng"), "{msg}");
        assert_eq!(installed_languages(&cfg).unwrap(), ["eng"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs the real `tesseract` (Debian: `apt install tesseract-ocr
    /// tesseract-ocr-eng`) on a picture drawn here.
    #[test]
    #[ignore = "needs tesseract with English data installed"]
    fn recognises_a_drawn_picture_live() {
        let png = crate::testimage::render_png(&["Hello world", "Glossa reads text"], 40.0);
        let p = Tesseract::new(TesseractConfig::default());
        let http = MockHttp::new();
        let opts = glossa_modules::OptionValues::new();
        let mut req = OcrRequest::new(ImageSource::Bytes(png));
        req.lang = LangId::ENGLISH;
        req.granularity = OcrGranularity::Words;
        let started = Instant::now();
        let r = with_test_host(&http, &opts, |h| p.recognize(h, &req)).unwrap();
        println!(
            "{} ms, installed {:?}\n{}",
            started.elapsed().as_millis(),
            installed_languages(&TesseractConfig::default()),
            r.text
        );
        for b in &r.blocks {
            println!("  {:?} {:?}", b.text, b.bounds);
            for w in &b.children {
                println!("    {:?} {:?}", w.text, w.bounds);
            }
        }
        assert!(r.text.contains("Hello") && r.text.contains("world"), "{}", r.text);
        assert!(r.text.contains("Glossa"), "{}", r.text);
        assert_eq!(r.blocks.len(), 2);
        assert!(r.blocks.iter().all(|b| b.bounds.is_some() && !b.children.is_empty()));

        // Russian too, where its data is installed (`tesseract-ocr-rus`).
        if installed_languages(&TesseractConfig::default()).unwrap_or_default().iter().any(|l| l == "rus") {
            let mut req = OcrRequest::new(ImageSource::Bytes(crate::testimage::render_png(&["Привет, мир"], 40.0)));
            req.lang = LangId::RUSSIAN;
            let r = with_test_host(&http, &opts, |h| p.recognize(h, &req)).unwrap();
            println!("{}", r.text);
            assert!(r.text.contains("Привет"), "{}", r.text);
        }
    }
}
