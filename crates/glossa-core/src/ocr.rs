//! Reading text out of a picture.

use serde::{Deserialize, Serialize};

use crate::lang::LangId;

/// Where the image comes from. A provider may support only some of these; the
/// engine converts between them as needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// Encoded bytes; the format is implied by the content (PNG, JPEG, WebP).
    Bytes(Vec<u8>),
    /// A file on disk.
    Path(std::path::PathBuf),
}

impl ImageSource {
    /// The encoded bytes, reading the file if need be.
    pub fn to_bytes(&self) -> std::io::Result<Vec<u8>> {
        match self {
            ImageSource::Bytes(b) => Ok(b.clone()),
            ImageSource::Path(p) => std::fs::read(p),
        }
    }
}

/// One OCR job.
#[derive(Debug, Clone)]
pub struct OcrRequest {
    pub image: ImageSource,
    /// The language of the text in the picture. `AUTO` asks the provider to
    /// work it out.
    pub lang: LangId,
    /// Ask the provider to translate while it is in there, where it can do
    /// that in the same call. `None` means OCR only.
    pub translate_to: Option<LangId>,
    /// Keep the line structure the picture had. Off joins everything into one
    /// paragraph, which reads better for prose split across lines.
    pub preserve_line_breaks: bool,
    /// How much structure we want back.
    pub granularity: OcrGranularity,
    /// Vertical Japanese and comic-book panels need different handling.
    pub vertical: VerticalTextMode,
}

impl OcrRequest {
    pub fn new(image: ImageSource) -> Self {
        Self {
            image,
            lang: LangId::AUTO,
            translate_to: None,
            preserve_line_breaks: true,
            granularity: OcrGranularity::FullText,
            vertical: VerticalTextMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrGranularity {
    /// One string.
    #[default]
    FullText,
    /// Paragraphs, or speech bubbles on a comic page.
    Blocks,
    /// Single lines.
    Lines,
    /// Every word with its geometry.
    Words,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerticalTextMode {
    /// Let the provider decide.
    #[default]
    Auto,
    /// Keep vertical runs vertical.
    Keep,
    /// Flatten everything to horizontal.
    Horizontal,
}

/// What came back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcrResult {
    /// Everything the provider read, joined.
    pub text: String,
    /// The structure, when more than `FullText` was asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<OcrBlock>,
    /// The language the provider detected.
    #[serde(default)]
    pub detected_lang: Option<LangId>,
    /// A translation, when the provider did one in the same call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    /// Pixel size of the image the coordinates refer to.
    #[serde(default)]
    pub image_size: Option<(u32, u32)>,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub elapsed_ms: u64,
}

impl OcrResult {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.blocks.is_empty()
    }

    /// The text to act on: the translation when there is one, else what was
    /// read.
    pub fn best_text(&self) -> &str {
        match self.translation.as_deref() {
            Some(t) if !t.trim().is_empty() => t,
            _ => &self.text,
        }
    }
}

/// A recognised region. Nested: a block holds lines, a line holds words,
/// depending on the granularity asked for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OcrBlock {
    pub text: String,
    /// Where it sits in the image.
    #[serde(default)]
    pub bounds: Option<Rect>,
    /// Rotation in radians, for text that is not axis-aligned.
    #[serde(default)]
    pub angle: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OcrBlock>,
}

/// An axis-aligned rectangle in image pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    /// Build from two corners in any order, so a drag that went up and to the
    /// left still gives a positive-size rectangle.
    pub fn from_corners(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x: x0.min(x1), y: y0.min(y1), width: (x1 - x0).abs(), height: (y1 - y0).abs() }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.width && y <= self.y + self.height
    }

    /// Grow by `pad` on every side, clamped so nothing goes negative.
    pub fn expanded(&self, pad: f32) -> Rect {
        let x = (self.x - pad).max(0.0);
        let y = (self.y - pad).max(0.0);
        Rect {
            x,
            y,
            width: self.x + self.width + pad - x,
            height: self.y + self.height + pad - y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_in_any_direction_gives_a_positive_rectangle() {
        let dragged_up_left = Rect::from_corners(100.0, 100.0, 40.0, 30.0);
        assert_eq!(dragged_up_left, Rect::new(40.0, 30.0, 60.0, 70.0));
        assert!(!dragged_up_left.is_empty());
        assert!(Rect::from_corners(10.0, 10.0, 10.0, 10.0).is_empty());
    }

    #[test]
    fn rect_geometry_helpers() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.centre(), (60.0, 45.0));
        assert!(r.contains(10.0, 20.0));
        assert!(r.contains(110.0, 70.0));
        assert!(!r.contains(9.0, 20.0));
        // Clamped at the image edge, and the far edges still move out by pad.
        assert_eq!(Rect::new(5.0, 5.0, 10.0, 10.0).expanded(20.0), Rect::new(0.0, 0.0, 35.0, 35.0));
        assert_eq!(Rect::new(50.0, 50.0, 10.0, 10.0).expanded(5.0), Rect::new(45.0, 45.0, 20.0, 20.0));
    }

    #[test]
    fn best_text_prefers_a_translation_but_not_an_empty_one() {
        let mut r = OcrResult { text: "original".into(), ..Default::default() };
        assert_eq!(r.best_text(), "original");
        r.translation = Some("  ".into());
        assert_eq!(r.best_text(), "original");
        r.translation = Some("translated".into());
        assert_eq!(r.best_text(), "translated");
    }

    #[test]
    fn emptiness_considers_blocks_as_well_as_text() {
        assert!(OcrResult::default().is_empty());
        let with_block = OcrResult {
            blocks: vec![OcrBlock { text: "hi".into(), ..Default::default() }],
            ..Default::default()
        };
        assert!(!with_block.is_empty());
    }
}
