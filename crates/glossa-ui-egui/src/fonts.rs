//! Fonts, loaded only when the text needs them.
//!
//! egui's built-in faces cover Latin, Cyrillic and Greek. A translator meets
//! every script there is, but a CJK font alone is 15–20 MB of memory once
//! loaded, so loading every fallback up front would make an idle tray app
//! heavy. Instead [`Fonts::ensure`] looks at text on its way to the screen and
//! loads the face for a script the first time that script appears.

use std::collections::HashSet;
use std::path::PathBuf;

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{FontData, FontFamily};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Han,
    Kana,
    Hangul,
    Indic,
    Thai,
    ArabicHebrew,
    ArmenianGeorgian,
    Symbols,
}

impl Script {
    fn of(c: char) -> Option<Script> {
        let u = c as u32;
        Some(match u {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Kana,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Script::Hangul,
            0x2E80..=0x2FDF
            | 0x3000..=0x303F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFF65
            | 0x20000..=0x2FA1F => Script::Han,
            0x0900..=0x0DFF => Script::Indic,
            0x0E00..=0x0EFF | 0x1780..=0x17FF => Script::Thai,
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF => Script::ArabicHebrew,
            0x0530..=0x058F | 0x10A0..=0x10FF | 0x2D00..=0x2D2F => Script::ArmenianGeorgian,
            0x2190..=0x2BFF => Script::Symbols,
            _ => return None,
        })
    }

    /// The fontconfig language to ask for on Unix.
    #[cfg(not(windows))]
    fn fc_lang(self) -> &'static str {
        match self {
            Script::Han => "zh-cn",
            Script::Kana => "ja",
            Script::Hangul => "ko",
            Script::Indic => "hi",
            Script::Thai => "th",
            Script::ArabicHebrew => "ar",
            Script::ArmenianGeorgian => "hy",
            Script::Symbols => "en",
        }
    }
}

#[derive(Default)]
pub struct Fonts {
    loaded: HashSet<Script>,
    tried: HashSet<Script>,
}

impl Fonts {
    /// Install the interface face. On Windows that is Segoe UI, which also
    /// covers Arabic, Hebrew, Armenian and Georgian, so those scripts need no
    /// further loading there.
    pub fn install_base(&mut self, ctx: &egui::Context, family: &str) {
        if let Some((path, index)) = base_font(family) {
            if self.add(ctx, "ui", &path, index, FontPriority::Highest) && cfg!(windows) {
                self.loaded.insert(Script::ArabicHebrew);
                self.loaded.insert(Script::ArmenianGeorgian);
            }
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
            if self.add(ctx, &format!("{s:?}"), &path, index, FontPriority::Lowest) {
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
                true
            }
            Err(e) => {
                tracing::debug!("font {}: {e}", path.display());
                false
            }
        }
    }
}

#[cfg(not(windows))]
fn first_existing(candidates: &[&str]) -> Option<(PathBuf, u32)> {
    candidates.iter().map(PathBuf::from).find(|p| p.is_file()).map(|p| (p, 0))
}

#[cfg(windows)]
fn fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")
}

#[cfg(windows)]
fn base_font(family: &str) -> Option<(PathBuf, u32)> {
    let dir = fonts_dir();
    if !family.trim().is_empty() {
        let squashed: String = family.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        for name in [format!("{squashed}.ttf"), format!("{squashed}.ttc"), format!("{squashed}.otf")] {
            let p = dir.join(&name);
            if p.is_file() {
                return Some((p, 0));
            }
        }
    }
    let p = dir.join("segoeui.ttf");
    p.is_file().then_some((p, 0))
}

#[cfg(windows)]
fn script_font(s: Script) -> Option<(PathBuf, u32)> {
    let dir = fonts_dir();
    let names: &[&str] = match s {
        Script::Han => &["msyh.ttc", "simsun.ttc", "YuGothM.ttc"],
        Script::Kana => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
        Script::Hangul => &["malgun.ttf", "gulim.ttc"],
        Script::Indic => &["Nirmala.ttc", "Nirmala.ttf", "mangal.ttf"],
        Script::Thai => &["LeelawUI.ttf", "leelawad.ttf", "tahoma.ttf"],
        Script::ArabicHebrew => &["segoeui.ttf", "arial.ttf"],
        Script::ArmenianGeorgian => &["segoeui.ttf", "sylfaen.ttf"],
        Script::Symbols => &["seguisym.ttf"],
    };
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file()).map(|p| (p, 0))
}

#[cfg(not(windows))]
fn fc_match(pattern: &str) -> Option<(PathBuf, u32)> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}\n%{index}", pattern]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next()?.trim());
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    path.is_file().then_some((path, index))
}

#[cfg(not(windows))]
fn base_font(family: &str) -> Option<(PathBuf, u32)> {
    if family.trim().is_empty() {
        // egui's own face is fine for the chrome on Unix; only load what was
        // asked for.
        return None;
    }
    fc_match(family)
}

#[cfg(not(windows))]
fn script_font(s: Script) -> Option<(PathBuf, u32)> {
    if let Some(found) = fc_match(&format!(":lang={}", s.fc_lang())) {
        return Some(found);
    }
    let mac: &[&str] = match s {
        Script::Han | Script::Kana => {
            &["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"]
        }
        Script::Hangul => &["/System/Library/Fonts/AppleSDGothicNeo.ttc"],
        Script::Indic => &["/System/Library/Fonts/Kohinoor.ttc"],
        Script::Thai => &["/System/Library/Fonts/Thonburi.ttc"],
        Script::ArabicHebrew => &["/System/Library/Fonts/GeezaPro.ttc", "/System/Library/Fonts/SFHebrew.ttf"],
        Script::ArmenianGeorgian => &["/System/Library/Fonts/Supplemental/Arial Unicode.ttf"],
        Script::Symbols => &["/System/Library/Fonts/Apple Symbols.ttf"],
    };
    first_existing(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_recognised() {
        assert_eq!(Script::of('漢'), Some(Script::Han));
        assert_eq!(Script::of('か'), Some(Script::Kana));
        assert_eq!(Script::of('한'), Some(Script::Hangul));
        assert_eq!(Script::of('ह'), Some(Script::Indic));
        assert_eq!(Script::of('ไ'), Some(Script::Thai));
        assert_eq!(Script::of('ש'), Some(Script::ArabicHebrew));
        assert_eq!(Script::of('ع'), Some(Script::ArabicHebrew));
        assert_eq!(Script::of('ა'), Some(Script::ArmenianGeorgian));
        assert_eq!(Script::of('ж'), None, "Cyrillic is built in");
        assert_eq!(Script::of('a'), None);
    }
}
