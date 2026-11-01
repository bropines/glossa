//! Reading text out of a picture.
//!
//! [`OcrResult`] is one page: the joined text plus a tree of [`OcrBlock`]s
//! (block → paragraph → line → word, the common ground of hOCR and ALTO
//! XML), each with its box, rotation, confidence, baseline, language and
//! writing direction where the engine reports them. [`export`] writes a
//! result as plain text, JSON, hOCR or ALTO; [`parse`] reads hOCR and ALTO
//! back, so an engine (or a module) that already speaks one of them needs no
//! mapping of its own.
//!
//! Coordinates are pixels of the image that was passed in, origin at the top
//! left, y growing downwards.

pub mod export;
pub mod parse;
mod xml;

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

    /// The text joined from the tree, a line per line. For results built
    /// from structure alone (hOCR, ALTO, a module that sent only blocks).
    pub fn text_from_blocks(&self) -> String {
        fn line_text(b: &OcrBlock) -> String {
            if !b.text.trim().is_empty() || b.children.is_empty() {
                return b.text.trim().to_owned();
            }
            b.children.iter().map(|w| w.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
        }
        fn lines_of(b: &OcrBlock, out: &mut Vec<String>) {
            match b.level {
                Some(OcrLevel::Line | OcrLevel::Word) => out.push(line_text(b)),
                _ if b.children.is_empty() => out.push(b.text.trim().to_owned()),
                _ => b.children.iter().for_each(|c| lines_of(c, out)),
            }
        }
        let levelled = OcrResult { blocks: self.blocks.clone(), ..Default::default() }.with_levels();
        let mut lines = Vec::new();
        for b in &levelled.blocks {
            lines_of(b, &mut lines);
        }
        lines.retain(|l| !l.is_empty());
        lines.join("\n")
    }

    /// The same result with every node's `level` filled in. Nodes whose
    /// producer set a level keep it; for the rest the shape of the tree
    /// decides: a top node two or more levels deep is a block, a top node
    /// one level deep is a block of lines when its text spans several lines
    /// and a line of words otherwise, and a leaf at the top is a line.
    pub fn with_levels(mut self) -> Self {
        for b in &mut self.blocks {
            infer_levels(b);
        }
        self
    }

    /// Every node in the tree, depth first.
    pub fn nodes(&self) -> impl Iterator<Item = &OcrBlock> {
        self.blocks.iter().flat_map(OcrBlock::walk)
    }
}

fn depth_below(b: &OcrBlock) -> usize {
    b.children.iter().map(|c| 1 + depth_below(c)).max().unwrap_or(0)
}

fn infer_levels(top: &mut OcrBlock) {
    if top.level.is_none() {
        top.level = Some(match depth_below(top) {
            0 => OcrLevel::Line,
            1 if top.text.trim().contains('\n') || top.children.iter().all(|c| c.text.contains(' ')) => OcrLevel::Block,
            1 => OcrLevel::Line,
            _ => OcrLevel::Block,
        });
    }
    fn below(parent: OcrLevel, node: &mut OcrBlock) {
        if node.level.is_none() {
            node.level = Some(match parent {
                OcrLevel::Block if depth_below(node) >= 2 => OcrLevel::Paragraph,
                OcrLevel::Block | OcrLevel::Paragraph => OcrLevel::Line,
                OcrLevel::Line | OcrLevel::Word => OcrLevel::Word,
            });
        }
        let me = node.level.unwrap_or(OcrLevel::Word);
        node.children.iter_mut().for_each(|c| below(me, c));
    }
    let me = top.level.unwrap_or(OcrLevel::Block);
    top.children.iter_mut().for_each(|c| below(me, c));
}

/// A recognised region. Nested: a block holds lines (or paragraphs holding
/// lines), a line holds words, depending on the granularity asked for.
///
/// Every field after `children` is optional and was added later: JSON
/// without them still loads, and they are left out of the JSON when unset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrBlock {
    pub text: String,
    /// Where it sits in the image: the text's box as it would be if it were
    /// level, placed about the text's centre. For level text (`angle` 0) this
    /// is simply the bounding box; [`OcrBlock::bbox`] gives the axis-aligned
    /// box around rotated text.
    #[serde(default)]
    pub bounds: Option<Rect>,
    /// Rotation in radians, clockwise on screen (y grows downwards), about
    /// the centre of `bounds`.
    #[serde(default)]
    pub angle: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OcrBlock>,
    /// What this node is. Producers set it; [`OcrResult::with_levels`] infers
    /// it from the shape of the tree where they did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<OcrLevel>,
    /// The outline as a polygon (usually the four corners of a rotated box,
    /// clockwise from the text's top left), when the engine gives one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygon: Vec<[f32; 2]>,
    /// How sure the engine is, 0–1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// The line the letters sit on, for lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
    /// The language of this text as a BCP 47 code, when the engine reports
    /// it per region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Which way the text runs; `None` means left to right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<WritingDirection>,
    /// A translation of this region, when the engine translated region by
    /// region (Lens does, per paragraph and per line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
}

impl OcrBlock {
    /// A node of `level` with `text` and nothing else.
    pub fn new(level: OcrLevel, text: impl Into<String>) -> Self {
        Self { text: text.into(), level: Some(level), ..Default::default() }
    }

    /// The axis-aligned box around the text, whatever its rotation: from the
    /// polygon when there is one, else from `bounds` turned by `angle`.
    pub fn bbox(&self) -> Option<Rect> {
        let points: Vec<[f32; 2]> = if self.polygon.len() >= 3 {
            self.polygon.clone()
        } else {
            let r = self.bounds?;
            if self.angle == 0.0 || !self.angle.is_finite() {
                return Some(r);
            }
            r.rotated_corners(self.angle).to_vec()
        };
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for [x, y] in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite())
            .then(|| Rect::from_corners(x0, y0, x1, y1))
            .or(self.bounds)
    }

    /// The four corners clockwise from the text's top left: the polygon when
    /// it has exactly four points, else `bounds` turned by `angle`.
    pub fn corners(&self) -> Option<[[f32; 2]; 4]> {
        if let [a, b, c, d] = self.polygon.as_slice() {
            return Some([*a, *b, *c, *d]);
        }
        self.bounds.map(|r| r.rotated_corners(self.angle))
    }

    /// Text runs top to bottom (vertical CJK).
    pub fn is_vertical(&self) -> bool {
        self.direction == Some(WritingDirection::Ttb)
    }

    /// This node and every node below it, depth first.
    pub fn walk(&self) -> impl Iterator<Item = &OcrBlock> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let next = stack.pop()?;
            stack.extend(next.children.iter().rev());
            Some(next)
        })
    }
}

/// The level of a node in the result tree. The names are the common ground
/// of hOCR and ALTO: `block` is hOCR's `ocr_carea` and ALTO's `TextBlock`,
/// `paragraph` hOCR's `ocr_par`, `line` `ocr_line` / `TextLine`, `word`
/// `ocrx_word` / `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrLevel {
    Block,
    Paragraph,
    Line,
    Word,
}

/// Which way text runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WritingDirection {
    /// Left to right.
    Ltr,
    /// Right to left (Arabic, Hebrew).
    Rtl,
    /// Top to bottom, columns from right to left (vertical Japanese).
    Ttb,
}

/// The level box and clockwise rotation (radians) of a quadrilateral given
/// clockwise from the text's top left — the inverse of
/// [`Rect::rotated_corners`]. Engines' quads are rarely exact rectangles, so
/// opposite edges are averaged; rotations under a third of a degree are
/// detector noise and come out as level.
pub fn quad_geometry(q: &[[f32; 2]; 4]) -> (Rect, f32) {
    let [[x1, y1], [x2, y2], [x3, y3], [x4, y4]] = *q;
    let dist = |ax: f32, ay: f32, bx: f32, by: f32| ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
    let cx = (x1 + x2 + x3 + x4) / 4.0;
    let cy = (y1 + y2 + y3 + y4) / 4.0;
    let w = (dist(x1, y1, x2, y2) + dist(x4, y4, x3, y3)) / 2.0;
    let h = (dist(x1, y1, x4, y4) + dist(x2, y2, x3, y3)) / 2.0;
    let mut angle = ((y2 - y1) + (y3 - y4)).atan2((x2 - x1) + (x3 - x4));
    if angle.abs() < 0.005 || !angle.is_finite() {
        angle = 0.0;
    }
    (Rect::new(cx - w / 2.0, cy - h / 2.0, w, h), angle)
}

/// The level box of text rotated `angle` radians whose axis-aligned box is
/// `aabb` — what hOCR's `bbox` with `textangle` and ALTO's box with
/// `ROTATION` describe. `None` near 45°, where the size cannot be told.
pub fn unrotated_box(aabb: Rect, angle: f32) -> Option<Rect> {
    if angle == 0.0 {
        return Some(aabb);
    }
    let (s, c) = angle.sin_cos();
    let (s, c) = (s.abs(), c.abs());
    let det = c * c - s * s;
    if det.abs() < 0.2 {
        return None;
    }
    let w = (aabb.width * c - aabb.height * s) / det;
    let h = (aabb.height * c - aabb.width * s) / det;
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return None;
    }
    let (cx, cy) = aabb.centre();
    Some(Rect::new(cx - w / 2.0, cy - h / 2.0, w, h))
}

/// A text line's baseline the way hOCR gives it: a straight line in the
/// coordinate system whose origin is the bottom-left corner of the line's
/// axis-aligned box, `y = slope * x + offset` (the offset is usually zero or
/// negative: descenders hang below the baseline).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub slope: f32,
    pub offset: f32,
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

    /// The corners of this rectangle turned `angle` radians clockwise about
    /// its centre, clockwise from the (turned) top left.
    pub fn rotated_corners(&self, angle: f32) -> [[f32; 2]; 4] {
        let (cx, cy) = self.centre();
        let (hw, hh) = (self.width / 2.0, self.height / 2.0);
        let (s, c) = if angle.is_finite() { angle.sin_cos() } else { (0.0, 1.0) };
        [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|(dx, dy)| [cx + dx * c - dy * s, cy + dx * s + dy * c])
    }

    /// Grow by `pad` on every side, clamped so nothing goes negative.
    pub fn expanded(&self, pad: f32) -> Rect {
        let x = (self.x - pad).max(0.0);
        let y = (self.y - pad).max(0.0);
        Rect { x, y, width: self.x + self.width + pad - x, height: self.y + self.height + pad - y }
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
    fn old_json_without_the_new_fields_still_loads_and_new_fields_stay_out() {
        let r: OcrResult = serde_json::from_str(
            r#"{"text":"a","blocks":[{"text":"a","bounds":{"x":1,"y":2,"width":3,"height":4},"angle":0.5,
                "children":[{"text":"a"}]}],"detected_lang":null,"image_size":[10,20]}"#,
        )
        .unwrap();
        assert_eq!(r.blocks[0].children.len(), 1);
        assert_eq!(r.blocks[0].confidence, None);
        let v = serde_json::to_value(&r.blocks[0].children[0]).unwrap();
        assert_eq!(v, serde_json::json!({"text": "a", "bounds": null, "angle": 0.0}), "unset fields are left out");
    }

    #[test]
    fn rotated_boxes_have_an_axis_aligned_bbox_and_corners() {
        let b = OcrBlock {
            bounds: Some(Rect::new(0.0, 0.0, 40.0, 10.0)),
            angle: std::f32::consts::FRAC_PI_2,
            ..Default::default()
        };
        let r = b.bbox().unwrap();
        assert!((r.x - 15.0).abs() < 1e-3 && (r.y + 15.0).abs() < 1e-3, "{r:?}");
        assert!((r.width - 10.0).abs() < 1e-3 && (r.height - 40.0).abs() < 1e-3, "{r:?}");
        let corners = b.corners().unwrap();
        let (back, angle) = quad_geometry(&corners);
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!((back.width - 40.0).abs() < 1e-3 && (back.x - 0.0).abs() < 1e-3, "{back:?}");
        let level = unrotated_box(r, std::f32::consts::FRAC_PI_2).unwrap();
        assert!((level.width - 40.0).abs() < 1e-3 && (level.height - 10.0).abs() < 1e-3, "{level:?}");
        assert_eq!(unrotated_box(r, std::f32::consts::FRAC_PI_4), None, "45° is ambiguous");
        let poly = OcrBlock { polygon: vec![[1.0, 5.0], [9.0, 2.0], [4.0, 8.0]], ..Default::default() };
        assert_eq!(poly.bbox(), Some(Rect::new(1.0, 2.0, 8.0, 6.0)));
    }

    #[test]
    fn levels_are_inferred_from_the_shape_of_the_tree() {
        let leaf = |t: &str| OcrBlock { text: t.into(), ..Default::default() };
        let r = OcrResult {
            blocks: vec![
                // Lens blocks: paragraphs of lines of words.
                OcrBlock {
                    text: "a b".into(),
                    children: vec![OcrBlock { children: vec![leaf("a")], ..leaf("a b") }],
                    ..Default::default()
                },
                // Windows OCR: a line of words.
                OcrBlock { children: vec![leaf("x"), leaf("y")], ..leaf("x y") },
                // A paragraph whose lines are strings.
                OcrBlock { children: vec![leaf("one two"), leaf("three")], ..leaf("one two\nthree") },
                leaf("alone"),
                // A producer's own level is kept.
                OcrBlock { children: vec![leaf("w")], ..OcrBlock::new(OcrLevel::Paragraph, "w") },
            ],
            ..Default::default()
        }
        .with_levels();
        let levels: Vec<_> = r.blocks.iter().map(|b| b.level.unwrap()).collect();
        assert_eq!(levels, [OcrLevel::Block, OcrLevel::Line, OcrLevel::Block, OcrLevel::Line, OcrLevel::Paragraph]);
        assert_eq!(r.blocks[0].children[0].level, Some(OcrLevel::Line));
        assert_eq!(r.blocks[0].children[0].children[0].level, Some(OcrLevel::Word));
        assert_eq!(r.blocks[1].children[0].level, Some(OcrLevel::Word));
        assert_eq!(r.blocks[4].children[0].level, Some(OcrLevel::Line));
        assert_eq!(r.text_from_blocks(), "a b\nx y\none two\nthree\nalone\nw");
    }

    #[test]
    fn emptiness_considers_blocks_as_well_as_text() {
        assert!(OcrResult::default().is_empty());
        let with_block =
            OcrResult { blocks: vec![OcrBlock { text: "hi".into(), ..Default::default() }], ..Default::default() };
        assert!(!with_block.is_empty());
    }
}
