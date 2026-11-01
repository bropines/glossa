//! An example native OCR module: it finds where the lines of text are in a
//! picture, without reading them, using only the host's image API.
//!
//! What it shows:
//!
//! * preprocessing through `image.*` host calls on the request's
//!   `image_handle` (straighten, binarise) instead of linking an imaging
//!   library;
//! * reading pixels back (`image.pixels`) for analysis of one's own;
//! * mapping boxes found in the prepared copy back onto the input with
//!   [`map_box_back`] and the copy's transform;
//! * returning the structured result (`blocks` of lines with boxes).
//!
//! The "text" of each line is a placeholder (`line 1`, `line 2`…): a real
//! engine would recognise the crop of each line here.

use glossa_module_sdk::wire::*;
use glossa_module_sdk::{export_module, map_box_back, Host, Module};
use serde_json::json;

#[derive(Default)]
pub struct Lines;

impl Module for Lines {
    fn recognize(&mut self, host: &Host, req: RecognizeParams) -> Result<RecognizeResult, ModuleError> {
        let input = match req.image_handle {
            Some(h) => host.image("info", json!({ "handle": h }))?,
            // An older host without the handle: register the bytes ourselves.
            None => host.image_decode(&req.image_bytes()?)?,
        };
        let level = host.image("deskew", json!({ "handle": input.handle, "max_angle": 10 }))?;
        let binary = host.image("threshold", json!({ "handle": level.handle }))?;
        let pixels = host.image("pixels", json!({ "handle": binary.handle }))?;
        let data = pixels.bytes()?;
        let found = find_lines(&data, binary.width as usize, binary.height as usize);
        if found.is_empty() {
            return Err(ModuleError::new(ErrorKind::NotFound, "no lines of text"));
        }
        let blocks: Vec<OcrNode> = found
            .iter()
            .enumerate()
            .map(|(i, r)| OcrNode {
                confidence: Some(1.0),
                ..OcrNode::new("line", format!("line {}", i + 1), Some(map_box_back(*r, &binary.transform)))
            })
            .collect();
        let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        Ok(RecognizeResult { text, blocks, ..Default::default() })
    }
}

/// Bands of rows holding ink in a binarised picture (0 or 255 per pixel),
/// each with the columns its ink spans. The ink is whichever value is the
/// minority, so light text on a dark background works too.
pub fn find_lines(px: &[u8], width: usize, height: usize) -> Vec<Rect> {
    if width == 0 || px.len() < width * height {
        return Vec::new();
    }
    let dark = px.iter().filter(|&&v| v < 128).count();
    let ink_is_dark = dark * 2 <= px.len();
    let ink = |v: u8| (v < 128) == ink_is_dark;
    let rows: Vec<bool> = (0..height).map(|y| px[y * width..(y + 1) * width].iter().any(|&v| ink(v))).collect();
    let mut out = Vec::new();
    let mut y = 0;
    while y < height {
        if !rows[y] {
            y += 1;
            continue;
        }
        let top = y;
        while y < height && rows[y] {
            y += 1;
        }
        let (mut x0, mut x1) = (width, 0);
        for row in top..y {
            let line = &px[row * width..(row + 1) * width];
            if let Some(first) = line.iter().position(|&v| ink(v)) {
                x0 = x0.min(first);
            }
            if let Some(last) = line.iter().rposition(|&v| ink(v)) {
                x1 = x1.max(last + 1);
            }
        }
        out.push(Rect::new(x0 as f32, top as f32, (x1 - x0) as f32, (y - top) as f32));
    }
    out
}

export_module!(Lines);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_of_ink_become_lines() {
        let (w, h) = (10, 8);
        let mut px = vec![255u8; w * h];
        for x in 2..7 {
            px[w + x] = 0;
            px[2 * w + x] = 0;
        }
        px[5 * w + 8] = 0;
        let lines = find_lines(&px, w, h);
        assert_eq!(lines, [Rect::new(2.0, 1.0, 5.0, 2.0), Rect::new(8.0, 5.0, 1.0, 1.0)]);
        assert!(find_lines(&[], 0, 0).is_empty());
        assert!(find_lines(&[0; 3], 2, 2).is_empty(), "short buffers are refused");
    }
}
