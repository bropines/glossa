//! Turning the lines and words a local engine read into an [`OcrResult`].
//!
//! Windows OCR and OneOCR both report lines of words with geometry and no
//! paragraphs, so they share this: the line is the block, words are its
//! children when the caller asked for words. Kept free of any OS code so it
//! is tested everywhere.

use glossa_core::ocr::{OcrBlock, OcrGranularity, Rect};
use glossa_core::{OcrRequest, OcrResult};

/// A quadrilateral in image pixels, corners clockwise from the top left of
/// the text (which is not the top left of the picture when the text is
/// rotated): `[x1, y1, x2, y2, x3, y3, x4, y4]`.
pub type Quad = [f32; 8];

/// Where a line or word sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// An axis-aligned rectangle (Windows OCR).
    Rect(Rect),
    /// Four corners (OneOCR), possibly rotated.
    Quad(Quad),
}

impl Shape {
    /// The unrotated rectangle around the text's centre and its rotation in
    /// radians, the way Lens geometry is given: a rotated quad becomes the
    /// box it would be if it were level, plus the angle to turn it by.
    pub fn geometry(&self) -> (Rect, f32) {
        match *self {
            Shape::Rect(r) => (r, 0.0),
            Shape::Quad(q) => quad_geometry(&q),
        }
    }
}

/// See [`Shape::geometry`].
pub fn quad_geometry(q: &Quad) -> (Rect, f32) {
    let (x1, y1, x2, y2, x3, y3, x4, y4) = (q[0], q[1], q[2], q[3], q[4], q[5], q[6], q[7]);
    let cx = (x1 + x2 + x3 + x4) / 4.0;
    let cy = (y1 + y2 + y3 + y4) / 4.0;
    // Average opposite edges: an engine's quads are rarely exact rectangles.
    let w = (dist(x1, y1, x2, y2) + dist(x4, y4, x3, y3)) / 2.0;
    let h = (dist(x1, y1, x4, y4) + dist(x2, y2, x3, y3)) / 2.0;
    let mut angle = ((y2 - y1) + (y3 - y4)).atan2((x2 - x1) + (x3 - x4));
    // Below a fraction of a degree is noise from the detector; report level
    // text as level so the overlay does not draw it tilted.
    if angle.abs() < 0.005 {
        angle = 0.0;
    }
    (Rect::new(cx - w / 2.0, cy - h / 2.0, w, h), angle)
}

fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt()
}

/// The smallest rectangle holding all of `rects`.
pub fn union(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
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

#[derive(Debug, Clone, PartialEq)]
pub struct RawWord {
    pub text: String,
    pub shape: Option<Shape>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawLine {
    /// The line as the engine spelled it; joining the words ourselves would
    /// put spaces between CJK characters.
    pub text: String,
    /// The line's own geometry; the words' union when the engine gives none.
    pub shape: Option<Shape>,
    pub words: Vec<RawWord>,
}

impl RawLine {
    fn block(&self, with_words: bool) -> OcrBlock {
        let (bounds, angle) = match self.shape {
            Some(s) => {
                let (r, a) = s.geometry();
                (Some(r), a)
            }
            None => (union(self.words.iter().filter_map(|w| w.shape).map(|s| s.geometry().0)), 0.0),
        };
        let children = if with_words {
            self.words
                .iter()
                .map(|w| {
                    let (bounds, angle) = w.shape.map(|s| s.geometry()).map_or((None, 0.0), |(r, a)| (Some(r), a));
                    OcrBlock { text: w.text.clone(), bounds, angle, ..Default::default() }
                })
                .collect()
        } else {
            Vec::new()
        };
        OcrBlock { text: self.text.clone(), bounds, angle, children, ..Default::default() }
    }
}

/// The result for `req` from what the engine read in an image of `size`.
pub fn build_result(lines: &[RawLine], req: &OcrRequest, size: (u32, u32)) -> OcrResult {
    let lines: Vec<&RawLine> = lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
    let joined = lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>().join("\n");
    let text = if req.preserve_line_breaks { joined } else { joined.split_whitespace().collect::<Vec<_>>().join(" ") };
    let blocks = match req.granularity {
        OcrGranularity::FullText => Vec::new(),
        OcrGranularity::Blocks | OcrGranularity::Lines => lines.iter().map(|l| l.block(false)).collect(),
        OcrGranularity::Words => lines.iter().map(|l| l.block(true)).collect(),
    };
    OcrResult { text, blocks, image_size: Some(size), ..Default::default() }
}

/// Composite straight-alpha BGRA pixels onto white, in place. Engines read
/// the colour channels and ignore alpha, so dark text on a transparent
/// background (a PNG cut from a web page) would otherwise come out as black
/// on black.
pub fn flatten_on_white(bgra: &mut [u8]) {
    for px in bgra.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        px[3] = 255;
    }
}

/// Pad BGRA pixels with white on the right and bottom so each side is at
/// least `min`; the detectors reject pictures smaller than that. Boxes keep
/// their coordinates, since the picture grows away from the origin.
pub fn pad_to_min(width: u32, height: u32, pixels: Vec<u8>, min: u32) -> (u32, u32, Vec<u8>) {
    if width >= min && height >= min {
        return (width, height, pixels);
    }
    let (w, h) = (width.max(min), height.max(min));
    let mut out = vec![255u8; w as usize * h as usize * 4];
    let row = width as usize * 4;
    for y in 0..height as usize {
        out[y * w as usize * 4..][..row].copy_from_slice(&pixels[y * row..][..row]);
    }
    (w, h, out)
}

/// Surround BGRA pixels with `margin` white pixels on every side. Windows
/// OCR finds nothing in a capture cropped tight around a line of text; a
/// border of white is what makes it see the line at all.
pub fn add_margin(width: u32, height: u32, pixels: &[u8], margin: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width + 2 * margin, height + 2 * margin);
    let mut out = vec![255u8; w as usize * h as usize * 4];
    let row = width as usize * 4;
    for y in 0..height as usize {
        let at = ((y + margin as usize) * w as usize + margin as usize) * 4;
        out[at..][..row].copy_from_slice(&pixels[y * row..][..row]);
    }
    (w, h, out)
}

/// How much to scale a `width`×`height` picture for an engine: down so
/// neither side exceeds `max_side` (0: no limit), or up two times when it
/// is shorter than `min_height` (0: never), since small captures hold small
/// text and recognisers want letters a dozen pixels high or more.
pub fn scale_factor(width: u32, height: u32, max_side: u32, min_height: u32) -> f64 {
    let longest = width.max(height) as f64;
    if max_side > 0 && longest > max_side as f64 {
        return max_side as f64 / longest;
    }
    if min_height > 0 && height < min_height && (max_side == 0 || longest * 2.0 <= max_side as f64) {
        return 2.0;
    }
    1.0
}

/// Map every rectangle in `blocks` back onto the caller's picture after it
/// was scaled by `factor` and then given a border of `margin` pixels.
pub fn unmap(blocks: &mut [OcrBlock], margin: f32, factor: f64) {
    let k = (1.0 / factor) as f32;
    for b in blocks {
        if let Some(r) = &mut b.bounds {
            *r = Rect::new((r.x - margin) * k, (r.y - margin) * k, r.width * k, r.height * k);
        }
        unmap(&mut b.children, margin, factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::ImageSource;

    fn req(granularity: OcrGranularity, preserve: bool) -> OcrRequest {
        let mut r = OcrRequest::new(ImageSource::Bytes(Vec::new()));
        r.granularity = granularity;
        r.preserve_line_breaks = preserve;
        r
    }

    fn word(text: &str, x: f32) -> RawWord {
        RawWord { text: text.into(), shape: Some(Shape::Rect(Rect::new(x, 10.0, 20.0, 8.0))) }
    }

    fn sample() -> Vec<RawLine> {
        vec![
            RawLine { text: "Hello world".into(), shape: None, words: vec![word("Hello", 0.0), word("world", 30.0)] },
            RawLine { text: "  ".into(), shape: None, words: vec![] },
            RawLine {
                text: "second".into(),
                shape: Some(Shape::Quad([0.0, 40.0, 50.0, 40.0, 50.0, 50.0, 0.0, 50.0])),
                words: vec![],
            },
        ]
    }

    #[test]
    fn text_keeps_or_joins_lines_and_drops_blank_ones() {
        let r = build_result(&sample(), &req(OcrGranularity::FullText, true), (100, 60));
        assert_eq!(r.text, "Hello world\nsecond");
        assert!(r.blocks.is_empty());
        assert_eq!(r.image_size, Some((100, 60)));
        let r = build_result(&sample(), &req(OcrGranularity::FullText, false), (100, 60));
        assert_eq!(r.text, "Hello world second");
    }

    #[test]
    fn lines_get_their_own_or_their_words_bounds() {
        let r = build_result(&sample(), &req(OcrGranularity::Lines, true), (100, 60));
        assert_eq!(r.blocks.len(), 2);
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(0.0, 10.0, 50.0, 8.0)), "union of the words");
        assert!(r.blocks[0].children.is_empty(), "no words at line granularity");
        assert_eq!(r.blocks[1].bounds, Some(Rect::new(0.0, 40.0, 50.0, 10.0)));
        assert_eq!(r.blocks[1].angle, 0.0);
    }

    #[test]
    fn words_hang_under_their_line() {
        let r = build_result(&sample(), &req(OcrGranularity::Words, true), (100, 60));
        let words: Vec<&str> = r.blocks[0].children.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(words, ["Hello", "world"]);
        assert_eq!(r.blocks[0].children[1].bounds, Some(Rect::new(30.0, 10.0, 20.0, 8.0)));
    }

    #[test]
    fn a_rotated_quad_becomes_a_level_box_and_an_angle() {
        // A 40×10 box turned 90° clockwise about (50, 50).
        let q = [55.0, 30.0, 55.0, 70.0, 45.0, 70.0, 45.0, 30.0];
        let (r, a) = quad_geometry(&q);
        assert!((a - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "angle {a}");
        assert!((r.width - 40.0).abs() < 1e-4 && (r.height - 10.0).abs() < 1e-4, "{r:?}");
        assert_eq!(r.centre(), (50.0, 50.0));
    }

    #[test]
    fn alpha_is_composited_onto_white() {
        let mut px = [0, 0, 0, 0, 10, 20, 30, 255, 0, 0, 0, 128];
        flatten_on_white(&mut px);
        assert_eq!(&px[..4], &[255, 255, 255, 255], "transparent becomes white");
        assert_eq!(&px[4..8], &[10, 20, 30, 255], "opaque is untouched");
        assert_eq!(px[8], 127, "half-transparent black is mid grey");
    }

    #[test]
    fn small_images_are_padded_with_white() {
        let px = vec![1, 2, 3, 255, 4, 5, 6, 255];
        let (w, h, out) = pad_to_min(2, 1, px.clone(), 3);
        assert_eq!((w, h), (3, 3));
        assert_eq!(&out[..8], &px[..], "the original pixels stay at the origin");
        assert!(out[8..].iter().all(|&b| b == 255));
        let (w, h, same) = pad_to_min(2, 1, px.clone(), 1);
        assert_eq!((w, h, same), (2, 1, px));
    }

    #[test]
    fn margins_surround_the_picture_with_white() {
        let px = [1, 2, 3, 255, 4, 5, 6, 255];
        let (w, h, out) = add_margin(2, 1, &px, 1);
        assert_eq!((w, h), (4, 3));
        assert!(out[..16].iter().all(|&b| b == 255), "top row is white");
        assert_eq!(&out[16 + 4..16 + 12], &px[..], "the picture sits one pixel in");
        assert!(out[32..].iter().all(|&b| b == 255), "bottom row is white");
    }

    #[test]
    fn pictures_are_scaled_to_suit_the_engine() {
        assert_eq!(scale_factor(1000, 800, 2000, 0), 1.0);
        assert!((scale_factor(4000, 1000, 2000, 60) - 0.5).abs() < 1e-9, "too big: shrunk, even if short");
        assert_eq!(scale_factor(300, 20, 2000, 60), 2.0, "a small capture is enlarged");
        assert_eq!(scale_factor(300, 20, 0, 60), 2.0);
        assert_eq!(scale_factor(1500, 20, 2000, 60), 1.0, "unless that would make it too big");
        assert_eq!(scale_factor(300, 100, 2000, 60), 1.0);
    }

    #[test]
    fn boxes_are_mapped_back_onto_the_original_picture() {
        let mut blocks = vec![OcrBlock {
            bounds: Some(Rect::new(11.0, 12.0, 3.0, 4.0)),
            children: vec![OcrBlock { bounds: Some(Rect::new(11.0, 11.0, 1.0, 1.0)), ..Default::default() }],
            ..Default::default()
        }];
        unmap(&mut blocks, 10.0, 0.5);
        assert_eq!(blocks[0].bounds, Some(Rect::new(2.0, 4.0, 6.0, 8.0)));
        assert_eq!(blocks[0].children[0].bounds, Some(Rect::new(2.0, 2.0, 2.0, 2.0)));
    }
}
