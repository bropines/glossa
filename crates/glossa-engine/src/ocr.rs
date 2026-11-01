//! What the front ends do with recognised text beyond showing it: copying it
//! as prose or as it was laid out, and translating it block by block for an
//! in-place rendering.
//!
//! An in-place translation needs a translation per block, positioned where
//! the block was. Google Lens provides that itself (see
//! `glossa_ocr::lens`); for every other engine the engine groups the lines
//! it read into paragraphs and has the current translator translate them,
//! all in one request when the translator keeps the paragraphs apart.

use glossa_core::ocr::{OcrBlock, OcrLevel, OcrResult, Rect};
use serde::Serialize;

/// The text to put on the clipboard: as recognised, or with the lines of
/// each paragraph joined (a blank line still separates paragraphs, and a
/// word hyphenated across lines is put back together).
pub fn text_for_copy(text: &str, join_lines: bool) -> String {
    let text = glossa_core::text::normalise_newlines(text);
    if !join_lines {
        return text.trim_end().to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let mut para = String::new();
        for line in paragraph.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let hyphenated = para.ends_with('-')
                && para.chars().rev().nth(1).is_some_and(char::is_alphabetic)
                && line.chars().next().is_some_and(char::is_lowercase);
            if hyphenated {
                para.pop();
            } else if !para.is_empty() && !joins_without_space(&para, line) {
                para.push(' ');
            }
            para.push_str(line);
        }
        out.push_str(&para);
    }
    out
}

/// The text of a recognition to put on the clipboard. Joining lines uses
/// the paragraphs the provider found (or that the lines group into, see
/// [`paragraphs`]) when it returned any, one paragraph per line, since a
/// provider's plain text separates paragraphs no differently from lines.
pub fn copy_text(ocr: &OcrResult, join_lines: bool) -> String {
    if join_lines {
        let paras = paragraphs(ocr);
        if !paras.is_empty() {
            return paras.iter().map(|p| text_for_copy(&p.text, true)).collect::<Vec<_>>().join("\n");
        }
    }
    text_for_copy(&ocr.text, join_lines)
}

/// Chinese and Japanese lines join with no space between them.
fn joins_without_space(before: &str, after: &str) -> bool {
    let cjk = |c: char| matches!(c as u32, 0x3000..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xff00..=0xffef);
    before.chars().last().is_some_and(cjk) && after.chars().next().is_some_and(cjk)
}

/// One translated stretch of the picture: a paragraph, as an in-place view
/// paints it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TranslatedBlock {
    /// What was read there.
    pub source: String,
    pub translation: String,
    /// Where, in the picture's pixels: the unrotated box around the centre.
    pub bounds: Rect,
    /// Rotation in radians.
    pub angle: f32,
    /// Line by line, when the translator set the translation on the source
    /// lines (Lens does); empty when the paragraph is to be reflowed into
    /// `bounds`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<TranslatedLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TranslatedLine {
    pub text: String,
    pub bounds: Rect,
    pub angle: f32,
}

/// The blocks a provider translated itself (Lens): every block with a
/// translation and a place.
pub fn provider_blocks(ocr: &OcrResult) -> Vec<TranslatedBlock> {
    ocr.blocks
        .iter()
        .filter_map(|b| {
            let translation = b.translation.clone().filter(|t| !t.trim().is_empty())?;
            let lines: Vec<TranslatedLine> = b
                .children
                .iter()
                .filter_map(|l| {
                    Some(TranslatedLine { text: l.translation.clone()?, bounds: l.bounds?, angle: l.angle })
                })
                .collect();
            let bounds = b.bounds.or_else(|| union(lines.iter().map(|l| l.bounds)))?;
            // Lines only count when every one of them has its text and place.
            let lines = if lines.len() == b.children.len() { lines } else { Vec::new() };
            Some(TranslatedBlock { source: block_text(b), translation, bounds, angle: b.angle, lines })
        })
        .collect()
}

/// A paragraph to have translated: its text and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub text: String,
    pub bounds: Rect,
    pub angle: f32,
}

/// The paragraphs in what was read. Blocks and paragraphs a provider found
/// are taken at its word; lines (Windows OCR, OneOCR, Tesseract give lines
/// of words) are grouped: a line joins the paragraph above when it is about
/// as tall, starts close below it, and overlaps it sideways.
pub fn paragraphs(ocr: &OcrResult) -> Vec<Paragraph> {
    let ocr = ocr.clone().with_levels();
    // Each paragraph, and its last line while lines are still joining it.
    let mut out: Vec<(Paragraph, Option<Rect>)> = Vec::new();
    for b in &ocr.blocks {
        let Some(r) = b.bounds.filter(|r| !r.is_empty()) else { continue };
        let text = block_text(b);
        if text.trim().is_empty() {
            continue;
        }
        if matches!(b.level, Some(OcrLevel::Line | OcrLevel::Word)) {
            if let Some((p, Some(last))) = out.last_mut() {
                let similar_height = (r.height - last.height).abs() <= 0.35 * last.height.max(r.height);
                let gap = r.y - (last.y + last.height);
                let close_below = gap >= -0.3 * last.height && gap <= 0.8 * last.height.max(r.height);
                let overlaps = r.x < last.x + last.width && last.x < r.x + r.width;
                if similar_height && close_below && overlaps && (p.angle - b.angle).abs() < 0.05 {
                    p.text = text_for_copy(&format!("{}\n{}", p.text, text.trim()), true);
                    p.bounds = union([p.bounds, r]).unwrap_or(p.bounds);
                    *last = r;
                    continue;
                }
            }
            out.push((Paragraph { text: text.trim().to_owned(), bounds: r, angle: b.angle }, Some(r)));
            continue;
        }
        let paras: Vec<&OcrBlock> = b
            .children
            .iter()
            .filter(|c| c.level == Some(OcrLevel::Paragraph) && c.bounds.is_some_and(|r| !r.is_empty()))
            .collect();
        if paras.is_empty() {
            out.push((Paragraph { text, bounds: r, angle: b.angle }, None));
        } else {
            for p in paras {
                out.push((Paragraph { text: block_text(p), bounds: p.bounds.unwrap_or(r), angle: p.angle }, None));
            }
        }
    }
    out.into_iter().map(|(p, _)| p).collect()
}

/// A node's text: its own, else its children's, a line each.
fn block_text(b: &OcrBlock) -> String {
    if !b.text.trim().is_empty() {
        return b.text.clone();
    }
    b.children
        .iter()
        .map(block_text)
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn union(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    let mut it = rects.into_iter().filter(|r| !r.is_empty());
    let first = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.width, first.y + first.height);
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.width);
        y1 = y1.max(r.y + r.height);
    }
    Some(Rect::from_corners(x0, y0, x1, y1))
}

/// Several texts as one request: each on a line of its own, separated by a
/// blank line, which translators keep as a paragraph break.
pub fn batch(texts: &[String]) -> String {
    texts.iter().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n\n")
}

/// A batched translation split back into one text per input, or `None`
/// when the translator merged or split paragraphs and the pieces no longer
/// line up.
pub fn unbatch(translated: &str, count: usize) -> Option<Vec<String>> {
    let parts: Vec<String> = glossa_core::text::normalise_newlines(translated)
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    (parts.len() == count).then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_keeps_lines_or_joins_them_into_paragraphs() {
        let read = "The quick brown\nfox jumps over the lazy\ndog.\n\nA second para-\ngraph here.";
        assert_eq!(text_for_copy(read, false), read);
        assert_eq!(
            text_for_copy(read, true),
            "The quick brown fox jumps over the lazy dog.\n\nA second paragraph here."
        );
        // Code-like dashes are not hyphenation.
        assert_eq!(text_for_copy("x = a -\nb", true), "x = a - b");
        assert_eq!(text_for_copy("COVID-\n19", true), "COVID- 19", "only a lowercase continuation rejoins");
        assert_eq!(text_for_copy("日本語の\n文章です", true), "日本語の文章です");
        assert_eq!(text_for_copy("a\r\nb\r\n", false), "a\nb");
    }

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrBlock {
        OcrBlock { text: text.into(), bounds: Some(Rect::new(x, y, w, h)), ..Default::default() }
    }

    #[test]
    fn lines_group_into_paragraphs() {
        let ocr = OcrResult {
            blocks: vec![
                line("Open the settings", 10.0, 10.0, 200.0, 20.0),
                line("to choose a language.", 10.0, 34.0, 220.0, 20.0),
                // Far below: a new paragraph.
                line("Press Enter", 10.0, 120.0, 120.0, 20.0),
                // Same row height but off to the side: not the same paragraph.
                line("Cancel", 400.0, 144.0, 60.0, 20.0),
                // A title twice the size above body text.
                line("Big title", 10.0, 200.0, 200.0, 44.0),
                line("small body", 10.0, 248.0, 120.0, 18.0),
            ],
            ..Default::default()
        };
        let p = paragraphs(&ocr);
        let texts: Vec<&str> = p.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Open the settings to choose a language.", "Press Enter", "Cancel", "Big title", "small body"]
        );
        assert_eq!(p[0].bounds, Rect::new(10.0, 10.0, 220.0, 44.0));
    }

    #[test]
    fn joined_copies_keep_the_paragraphs_found() {
        let ocr = OcrResult {
            text: "Welcome\nThe exhibition opens\nat ten.".into(),
            blocks: vec![
                line("Welcome", 10.0, 10.0, 200.0, 40.0),
                line("The exhibition opens", 10.0, 80.0, 200.0, 20.0),
                line("at ten.", 10.0, 104.0, 80.0, 20.0),
            ],
            ..Default::default()
        };
        assert_eq!(copy_text(&ocr, true), "Welcome\nThe exhibition opens at ten.");
        assert_eq!(copy_text(&ocr, false), "Welcome\nThe exhibition opens\nat ten.");
        let plain = OcrResult { blocks: Vec::new(), ..ocr };
        assert_eq!(copy_text(&plain, true), "Welcome The exhibition opens at ten.", "no structure: all joined");
    }

    #[test]
    fn provider_paragraphs_are_taken_as_they_are() {
        let mut para = line("", 0.0, 0.0, 100.0, 50.0);
        para.level = Some(OcrLevel::Block);
        para.children = vec![line("one", 0.0, 0.0, 100.0, 20.0), line("two", 0.0, 25.0, 100.0, 20.0)];
        let ocr = OcrResult { blocks: vec![para, line("lone", 0.0, 60.0, 50.0, 20.0)], ..Default::default() };
        let p = paragraphs(&ocr);
        assert_eq!(p.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["one\ntwo", "lone"]);
    }

    #[test]
    fn lens_blocks_come_with_their_lines() {
        let mut b = line("Hello\nworld", 0.0, 0.0, 100.0, 50.0);
        b.translation = Some("Привет мир".into());
        let mut l1 = line("Hello", 0.0, 0.0, 100.0, 20.0);
        l1.translation = Some("Привет".into());
        let mut l2 = line("world", 0.0, 25.0, 100.0, 20.0);
        l2.translation = Some("мир".into());
        b.children = vec![l1, l2];
        let untranslated = line("skip", 0.0, 80.0, 10.0, 10.0);
        let blocks = provider_blocks(&OcrResult { blocks: vec![b, untranslated], ..Default::default() });
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].translation, "Привет мир");
        assert_eq!(blocks[0].lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Привет", "мир"]);
    }

    #[test]
    fn batches_split_back_only_when_they_line_up() {
        let texts = vec!["Open the\nsettings".to_owned(), "Press Enter".to_owned()];
        assert_eq!(batch(&texts), "Open the settings\n\nPress Enter");
        assert_eq!(
            unbatch("Откройте настройки\n\nНажмите Enter\n", 2),
            Some(vec!["Откройте настройки".to_owned(), "Нажмите Enter".to_owned()])
        );
        assert_eq!(unbatch("Откройте настройки\nНажмите Enter", 2).map(|v| v.len()), Some(2), "single newlines too");
        assert_eq!(unbatch("Откройте настройки. Нажмите Enter", 2), None, "merged");
    }
}
