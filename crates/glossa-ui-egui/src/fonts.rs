//! Fonts, loaded only when the text needs them.
//!
//! egui's built-in faces cover Latin, Cyrillic and Greek. A translator meets
//! every script there is, but a CJK font alone is 15–20 MB of memory once
//! loaded, so loading every fallback up front would make an idle tray app
//! heavy. Instead [`Fonts::ensure`] looks at text on its way to the screen and
//! loads the face for a script the first time that script appears.
//!
//! Labels need the same care as translations: a language list shows every
//! language's own name, in its own script. A widget that is about to show
//! such names asks with [`request_language_fonts`]; the app serves the
//! request at the start of the next frame ([`Fonts::serve_requests`]).

use std::collections::HashSet;
use std::path::PathBuf;

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{FontData, FontFamily};

/// A writing system, as far as choosing a font goes. Scripts one system
/// font covers together on Windows still stay apart here: elsewhere each
/// usually has a font of its own (Noto Sans Bengali, Noto Sans Kannada…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Han,
    Kana,
    Hangul,
    Devanagari,
    Bengali,
    Gurmukhi,
    Gujarati,
    Oriya,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    Sinhala,
    OlChiki,
    Thai,
    Lao,
    Khmer,
    Myanmar,
    Tibetan,
    Mongolian,
    Javanese,
    Arabic,
    Hebrew,
    Syriac,
    Thaana,
    Nko,
    Armenian,
    Georgian,
    Ethiopic,
    Tifinagh,
    Vai,
    Cherokee,
    CanadianSyllabics,
    Symbols,
    /// Pictographs (the 1F000 planes), which turn up in translated and
    /// pasted text even though the interface never uses them.
    Emoji,
}

impl Script {
    /// Every script, for the tests.
    #[cfg(test)]
    const ALL: [Script; 34] = [
        Script::Han,
        Script::Kana,
        Script::Hangul,
        Script::Devanagari,
        Script::Bengali,
        Script::Gurmukhi,
        Script::Gujarati,
        Script::Oriya,
        Script::Tamil,
        Script::Telugu,
        Script::Kannada,
        Script::Malayalam,
        Script::Sinhala,
        Script::OlChiki,
        Script::Thai,
        Script::Lao,
        Script::Khmer,
        Script::Myanmar,
        Script::Tibetan,
        Script::Mongolian,
        Script::Javanese,
        Script::Arabic,
        Script::Hebrew,
        Script::Syriac,
        Script::Thaana,
        Script::Nko,
        Script::Armenian,
        Script::Georgian,
        Script::Ethiopic,
        Script::Tifinagh,
        Script::Vai,
        Script::Cherokee,
        Script::CanadianSyllabics,
        Script::Symbols,
    ];

    /// The script a character needs a font for, or `None` for what the
    /// built-in faces draw (and for scripts no font here covers).
    pub(crate) fn of(c: char) -> Option<Script> {
        let u = c as u32;
        Some(match u {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Kana,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => Script::Hangul,
            0x2E80..=0x2FDF
            | 0x3000..=0x303F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFF65
            | 0x20000..=0x2FA1F => Script::Han,
            0x0900..=0x097F | 0xA8E0..=0xA8FF => Script::Devanagari,
            0x0980..=0x09FF => Script::Bengali,
            0x0A00..=0x0A7F => Script::Gurmukhi,
            0x0A80..=0x0AFF => Script::Gujarati,
            0x0B00..=0x0B7F => Script::Oriya,
            0x0B80..=0x0BFF => Script::Tamil,
            0x0C00..=0x0C7F => Script::Telugu,
            0x0C80..=0x0CFF => Script::Kannada,
            0x0D00..=0x0D7F => Script::Malayalam,
            0x0D80..=0x0DFF => Script::Sinhala,
            0x1C50..=0x1C7F => Script::OlChiki,
            0x0E00..=0x0E7F => Script::Thai,
            0x0E80..=0x0EFF => Script::Lao,
            0x1780..=0x17FF | 0x19E0..=0x19FF => Script::Khmer,
            0x1000..=0x109F | 0xA9E0..=0xA9FF | 0xAA60..=0xAA7F => Script::Myanmar,
            0x0F00..=0x0FFF => Script::Tibetan,
            0x1800..=0x18AF => Script::Mongolian,
            0xA980..=0xA9DF => Script::Javanese,
            0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => Script::Hebrew,
            0x0700..=0x074F | 0x0860..=0x086F => Script::Syriac,
            0x0780..=0x07BF => Script::Thaana,
            0x07C0..=0x07FF => Script::Nko,
            0x0530..=0x058F | 0xFB13..=0xFB17 => Script::Armenian,
            0x10A0..=0x10FF | 0x1C90..=0x1CBF | 0x2D00..=0x2D2F => Script::Georgian,
            0x1200..=0x139F | 0x2D80..=0x2DDF | 0xAB00..=0xAB2F => Script::Ethiopic,
            0x2D30..=0x2D7F => Script::Tifinagh,
            0xA500..=0xA63F => Script::Vai,
            0x13A0..=0x13FF | 0xAB70..=0xABBF => Script::Cherokee,
            0x1400..=0x167F | 0x18B0..=0x18FF => Script::CanadianSyllabics,
            0x2190..=0x2BFF => Script::Symbols,
            0x1F000..=0x1FAFF => Script::Emoji,
            _ => return None,
        })
    }

    /// Font files in `%WINDIR%\Fonts` that cover the script, best first.
    /// The first of each ships with Windows 10 and 11; the others are older
    /// systems' faces. Several scripts share a file (Nirmala UI draws every
    /// Indic script); [`Fonts`] loads a file once.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn windows_files(self) -> &'static [&'static str] {
        match self {
            Script::Han => &["msyh.ttc", "simsun.ttc", "YuGothM.ttc"],
            Script::Kana => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
            Script::Hangul => &["malgun.ttf", "gulim.ttc"],
            Script::Devanagari => &["Nirmala.ttc", "Nirmala.ttf", "mangal.ttf"],
            Script::Bengali => &["Nirmala.ttc", "Nirmala.ttf", "vrinda.ttf"],
            Script::Gurmukhi => &["Nirmala.ttc", "Nirmala.ttf", "raavi.ttf"],
            Script::Gujarati => &["Nirmala.ttc", "Nirmala.ttf", "shruti.ttf"],
            Script::Oriya => &["Nirmala.ttc", "Nirmala.ttf", "kalinga.ttf"],
            Script::Tamil => &["Nirmala.ttc", "Nirmala.ttf", "latha.ttf"],
            Script::Telugu => &["Nirmala.ttc", "Nirmala.ttf", "gautami.ttf"],
            Script::Kannada => &["Nirmala.ttc", "Nirmala.ttf", "tunga.ttf"],
            Script::Malayalam => &["Nirmala.ttc", "Nirmala.ttf", "kartika.ttf"],
            Script::Sinhala => &["Nirmala.ttc", "Nirmala.ttf", "iskpota.ttf"],
            Script::OlChiki => &["Nirmala.ttc", "Nirmala.ttf"],
            Script::Thai => &["LeelawUI.ttf", "leelawad.ttf", "tahoma.ttf"],
            Script::Lao => &["LeelawUI.ttf", "LaoUI.ttf", "DokChamp.ttf"],
            Script::Khmer => &["LeelawUI.ttf", "KhmerUI.ttf", "daunpenh.ttf"],
            Script::Myanmar => &["mmrtext.ttf"],
            Script::Tibetan => &["himalaya.ttf"],
            Script::Mongolian => &["monbaiti.ttf"],
            Script::Javanese => &["javatext.ttf"],
            Script::Arabic => &["segoeui.ttf", "arial.ttf"],
            Script::Hebrew => &["segoeui.ttf", "arial.ttf"],
            Script::Syriac => &["seguihis.ttf", "estre.ttf"],
            Script::Thaana => &["mvboli.ttf"],
            Script::Nko => &["ebrima.ttf"],
            Script::Armenian => &["segoeui.ttf", "sylfaen.ttf"],
            Script::Georgian => &["segoeui.ttf", "sylfaen.ttf"],
            Script::Ethiopic => &["ebrima.ttf", "nyala.ttf"],
            Script::Tifinagh => &["ebrima.ttf"],
            Script::Vai => &["ebrima.ttf"],
            Script::Cherokee => &["gadugi.ttf", "plantc.ttf"],
            Script::CanadianSyllabics => &["gadugi.ttf", "euphemia.ttf"],
            Script::Symbols => &["seguisym.ttf"],
            // Segoe UI Emoji has plain outlines under its colour layers,
            // which is what egui draws.
            Script::Emoji => &["seguiemj.ttf", "seguisym.ttf"],
        }
    }

    /// The fontconfig language to ask for on Unix: a language written in
    /// the script, which Noto (and most distributions' fonts) declare.
    #[cfg_attr(windows, allow(dead_code))]
    fn fc_lang(self) -> &'static str {
        match self {
            Script::Han => "zh-cn",
            Script::Kana => "ja",
            Script::Hangul => "ko",
            Script::Devanagari => "hi",
            Script::Bengali => "bn",
            Script::Gurmukhi => "pa",
            Script::Gujarati => "gu",
            Script::Oriya => "or",
            Script::Tamil => "ta",
            Script::Telugu => "te",
            Script::Kannada => "kn",
            Script::Malayalam => "ml",
            Script::Sinhala => "si",
            Script::OlChiki => "sat",
            Script::Thai => "th",
            Script::Lao => "lo",
            Script::Khmer => "km",
            Script::Myanmar => "my",
            Script::Tibetan => "bo",
            Script::Mongolian => "mn-cn",
            Script::Javanese => "jv",
            Script::Arabic => "ar",
            Script::Hebrew => "he",
            Script::Syriac => "syr",
            Script::Thaana => "dv",
            Script::Nko => "nqo",
            Script::Armenian => "hy",
            Script::Georgian => "ka",
            Script::Ethiopic => "am",
            Script::Tifinagh => "ber-ma",
            Script::Vai => "vai",
            Script::Cherokee => "chr",
            Script::CanadianSyllabics => "iu",
            Script::Symbols => "en",
            Script::Emoji => "und-zsye",
        }
    }

    /// Fonts that ship with macOS for the script, when fontconfig is absent.
    #[cfg(target_os = "macos")]
    fn mac_files(self) -> &'static [&'static str] {
        match self {
            Script::Han | Script::Kana => {
                &["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"]
            }
            Script::Hangul => &["/System/Library/Fonts/AppleSDGothicNeo.ttc"],
            Script::Devanagari
            | Script::Bengali
            | Script::Gurmukhi
            | Script::Gujarati
            | Script::Oriya
            | Script::Tamil
            | Script::Telugu
            | Script::Kannada
            | Script::Malayalam
            | Script::Sinhala
            | Script::OlChiki => &["/System/Library/Fonts/Kohinoor.ttc"],
            Script::Thai | Script::Lao | Script::Khmer => &["/System/Library/Fonts/Thonburi.ttc"],
            Script::Arabic | Script::Syriac | Script::Thaana | Script::Nko => &["/System/Library/Fonts/GeezaPro.ttc"],
            Script::Hebrew => &["/System/Library/Fonts/SFHebrew.ttf"],
            Script::Symbols => &["/System/Library/Fonts/Apple Symbols.ttf"],
            _ => &["/System/Library/Fonts/Supplemental/Arial Unicode.ttf"],
        }
    }
}

/// Characters egui's built-in faces draw: Latin with its extensions, IPA,
/// spacing modifiers, combining marks, Greek, Cyrillic, general punctuation.
#[cfg(test)]
fn built_in(c: char) -> bool {
    let u = c as u32;
    u < 0x0530 || (0x1E00..=0x1FFF).contains(&u) || (0x2000..=0x206F).contains(&u)
}

#[derive(Default)]
pub struct Fonts {
    loaded: HashSet<Script>,
    tried: HashSet<Script>,
    /// Files already added, so scripts sharing a font load it once.
    files: HashSet<PathBuf>,
    /// The language names have been looked at.
    languages_done: bool,
    /// The interface face in use, so a theme change only loads a new one
    /// when it differs.
    base: Option<PathBuf>,
}

/// Ask for the fonts every language's own name needs (for a language list
/// about to open). Served by [`Fonts::serve_requests`] in the next frame.
pub fn request_language_fonts(ctx: &egui::Context) {
    let id = egui::Id::unique(LANGUAGE_FONTS_REQUEST);
    if !ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
        ctx.data_mut(|d| d.insert_temp(id, true));
        ctx.request_repaint();
    }
}

const LANGUAGE_FONTS_REQUEST: &str = "glossa-language-fonts-wanted";

/// Every language's own name, the text a language list shows.
fn language_names() -> String {
    glossa_core::LangId::all().map(|l| l.native_name()).collect::<Vec<_>>().join(" ")
}

/// The interface faces to try, in order: the one chosen in the options,
/// then the theme's (`fonts.family`, then `fonts.fallbacks`).
fn base_families<'a>(chosen: &'a str, theme: &'a glossa_core::Theme) -> Vec<&'a str> {
    std::iter::once(chosen).filter(|f| !f.trim().is_empty()).chain(theme.fonts.candidates()).collect()
}

impl Fonts {
    /// Install the interface face: the first of `families` the system has,
    /// else Segoe UI on Windows. A script whose font file is the same face
    /// (Segoe UI covers Arabic, Hebrew, Armenian and Georgian) then needs no
    /// further loading. Called again after a theme change; a face once loaded
    /// stays (egui cannot drop one), the new one just goes first.
    pub fn install_base(&mut self, ctx: &egui::Context, chosen: &str, theme: &glossa_core::Theme) {
        let Some((path, index)) = base_font(&base_families(chosen, theme), chosen) else {
            #[cfg(target_os = "macos")]
            self.ensure(ctx, "\u{2192}");
            return;
        };
        if self.base.as_ref() == Some(&path) {
            return;
        }
        let name = format!("ui:{}", path.display());
        if self.add(ctx, &name, &path, index, FontPriority::Highest) {
            // Of the macOS faces only SF covers the arrows and symbols the
            // interface draws; with another, Apple Symbols backs it up.
            #[cfg(target_os = "macos")]
            if !path.ends_with("SFNS.ttf") {
                self.ensure(ctx, "\u{2192}");
            }
            self.base = Some(path);
        }
    }

    /// Load the fonts widgets asked for since the last frame.
    pub fn serve_requests(&mut self, ctx: &egui::Context) {
        let id = egui::Id::unique(LANGUAGE_FONTS_REQUEST);
        if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) {
            ctx.data_mut(|d| d.remove::<bool>(id));
            self.ensure_languages(ctx);
        }
    }

    /// Load what the language names need, once.
    pub fn ensure_languages(&mut self, ctx: &egui::Context) {
        if !std::mem::replace(&mut self.languages_done, true) {
            self.ensure(ctx, &language_names());
        }
    }

    /// Load whatever `text` needs and is not loaded yet.
    pub fn ensure(&mut self, ctx: &egui::Context, text: &str) {
        if text.is_ascii() {
            return;
        }
        let mut needed: Vec<Script> = Vec::new();
        for ch in text.chars() {
            if let Some(s) = Script::of(ch) {
                if !self.loaded.contains(&s) && !self.tried.contains(&s) && !needed.contains(&s) {
                    needed.push(s);
                }
            }
        }
        for s in needed {
            self.tried.insert(s);
            let Some((path, index)) = script_font(s) else {
                tracing::debug!("no font found for {s:?}");
                continue;
            };
            if self.files.contains(&path) || self.add(ctx, &format!("{s:?}"), &path, index, FontPriority::Lowest) {
                self.loaded.insert(s);
                // A CJK face also covers the other CJK scripts' basics.
                if s == Script::Han {
                    self.loaded.insert(Script::Kana);
                }
            }
        }
    }

    fn add(&mut self, ctx: &egui::Context, name: &str, path: &PathBuf, index: u32, priority: FontPriority) -> bool {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut data = FontData::from_owned(bytes);
                data.index = index;
                ctx.add_font(FontInsert::new(
                    name,
                    data,
                    vec![
                        InsertFontFamily { family: FontFamily::Proportional, priority },
                        InsertFontFamily { family: FontFamily::Monospace, priority: FontPriority::Lowest },
                    ],
                ));
                tracing::debug!("font {name}: {}", path.display());
                self.files.insert(path.clone());
                // The new face is used from the next frame on.
                ctx.request_repaint();
                true
            }
            Err(e) => {
                tracing::debug!("font {}: {e}", path.display());
                false
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn first_existing(candidates: &[&str]) -> Option<(PathBuf, u32)> {
    candidates.iter().map(PathBuf::from).find(|p| p.is_file()).map(|p| (p, 0))
}

#[cfg(windows)]
fn fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")
}

/// A family name without spaces, lower case: how it is matched to a file
/// name on Windows and to fontconfig's answer elsewhere.
fn squash(family: &str) -> String {
    family.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// `chosen` is the family picked in the options (empty for the theme's).
#[cfg(windows)]
fn base_font(families: &[&str], _chosen: &str) -> Option<(PathBuf, u32)> {
    let dir = fonts_dir();
    for family in families.iter().filter(|f| !f.trim().is_empty()) {
        let squashed = squash(family);
        for name in [format!("{squashed}.ttf"), format!("{squashed}.ttc"), format!("{squashed}.otf")] {
            let p = dir.join(&name);
            if p.is_file() {
                return Some((p, 0));
            }
        }
        // Most files are not named after their family ("Times New Roman" is
        // times.ttf); the system's list of installed fonts says which is.
        if let Some(found) = glossa_platform::fonts::registered_family(family) {
            return Some(found);
        }
    }
    let p = dir.join("segoeui.ttf");
    p.is_file().then_some((p, 0))
}

#[cfg(windows)]
fn script_font(s: Script) -> Option<(PathBuf, u32)> {
    let dir = fonts_dir();
    s.windows_files().iter().map(|n| dir.join(n)).find(|p| p.is_file()).map(|p| (p, 0))
}

#[cfg(not(windows))]
fn fc_match(pattern: &str) -> Option<(PathBuf, u32, String)> {
    let out =
        std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}\n%{lang}", pattern]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next()?.trim());
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    let langs = lines.next().unwrap_or("").to_owned();
    path.is_file().then_some((path, index, langs))
}

/// The first of `families` fontconfig really has. fontconfig always answers
/// with its closest face, so an answer for another family (DejaVu Sans for
/// "Segoe UI Variable") is passed over; with none left, egui's own face is
/// fine for the chrome.
#[cfg(not(windows))]
fn base_font(families: &[&str], _chosen: &str) -> Option<(PathBuf, u32)> {
    let found = families.iter().filter(|f| !f.trim().is_empty()).find_map(|family| {
        let out = std::process::Command::new("fc-match").args(["-f", "%{family}", family]).output().ok()?;
        let found = squash(&String::from_utf8_lossy(&out.stdout));
        found.split(',').any(|f| f == squash(family)).then(|| fc_match(family)).flatten().map(|(p, i, _)| (p, i))
    });
    #[cfg(target_os = "macos")]
    {
        found.or_else(|| mac_base_font(_chosen))
    }
    #[cfg(not(target_os = "macos"))]
    {
        found
    }
}

/// macOS has no fontconfig unless one was installed: the family picked in
/// the options, found in the font folders (a scan, done once per run and
/// only for a family the user picked), else the system's interface face.
/// The themes' families are Windows faces, and egui's own face lacks arrows
/// and other symbols the interface uses ("→" in the popup's title).
#[cfg(target_os = "macos")]
fn mac_base_font(chosen: &str) -> Option<(PathBuf, u32)> {
    Some(chosen).filter(|f| !f.trim().is_empty()).and_then(glossa_platform::fonts::find_family).or_else(|| {
        first_existing(&[
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/HelveticaNeue.ttc",
            "/System/Library/Fonts/Helvetica.ttc",
        ])
    })
}

#[cfg(not(windows))]
fn script_font(s: Script) -> Option<(PathBuf, u32)> {
    if s == Script::Emoji {
        // What fontconfig offers for emoji is a colour bitmap font, which
        // egui cannot draw; its own emoji subset is all there is.
        return None;
    }
    // fontconfig always answers with something; take it only if it really
    // covers the language (else it is the default face, already drawn).
    let lang = s.fc_lang();
    if let Some((path, index, langs)) = fc_match(&format!(":lang={lang}")) {
        if langs.split('|').any(|l| l == lang) {
            return Some((path, index));
        }
    }
    #[cfg(target_os = "macos")]
    {
        first_existing(s.mac_files())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_recognised() {
        assert_eq!(Script::of('漢'), Some(Script::Han));
        assert_eq!(Script::of('か'), Some(Script::Kana));
        assert_eq!(Script::of('한'), Some(Script::Hangul));
        assert_eq!(Script::of('ह'), Some(Script::Devanagari));
        assert_eq!(Script::of('ব'), Some(Script::Bengali));
        assert_eq!(Script::of('ಕ'), Some(Script::Kannada));
        assert_eq!(Script::of('ខ'), Some(Script::Khmer));
        assert_eq!(Script::of('ລ'), Some(Script::Lao));
        assert_eq!(Script::of('ไ'), Some(Script::Thai));
        assert_eq!(Script::of('ש'), Some(Script::Hebrew));
        assert_eq!(Script::of('ع'), Some(Script::Arabic));
        assert_eq!(Script::of('ა'), Some(Script::Georgian));
        assert_eq!(Script::of('\u{1F539}'), Some(Script::Emoji));
        assert_eq!(Script::of('ж'), None, "Cyrillic is built in");
        assert_eq!(Script::of('a'), None);
    }

    /// Every language's own name (what the language lists show) is either
    /// drawn by the built-in faces or has a script with a font to load, on
    /// Windows and through fontconfig.
    #[test]
    fn every_language_name_has_a_font() {
        for lang in glossa_core::LangId::all() {
            for c in lang.native_name().chars().chain(lang.english_name().chars()) {
                if built_in(c) {
                    continue;
                }
                let script = Script::of(c)
                    .unwrap_or_else(|| panic!("{} ({:?}): no script for U+{:04X}", lang.english_name(), c, c as u32));
                assert!(!script.windows_files().is_empty(), "{script:?}: no Windows font");
                assert!(!script.fc_lang().is_empty(), "{script:?}: no fontconfig language");
            }
        }
    }

    #[test]
    fn every_script_has_fonts_everywhere() {
        for s in Script::ALL {
            assert!(!s.windows_files().is_empty(), "{s:?}");
            assert!(!s.fc_lang().is_empty(), "{s:?}");
        }
    }

    #[test]
    fn the_language_names_include_the_scripts_that_were_boxes() {
        let names = language_names();
        let scripts: HashSet<Script> = names.chars().filter_map(Script::of).collect();
        for s in [Script::Bengali, Script::Kannada, Script::Khmer, Script::Hangul, Script::Lao] {
            assert!(scripts.contains(&s), "{s:?}");
        }
    }
}
