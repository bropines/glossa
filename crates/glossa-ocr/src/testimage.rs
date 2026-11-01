//! Pictures of text drawn for the live recogniser tests, so they need no
//! sample files and no image tools: black lines of text in egui's bundled
//! Ubuntu Light (which has Latin and Cyrillic) on white, as PNG.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};

/// `lines` drawn at `px` pixels high, one under another, with a margin.
pub fn render_png(lines: &[&str], px: f32) -> Vec<u8> {
    let font = FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("bundled font");
    let scaled = font.as_scaled(PxScale::from(px));
    let margin = px;
    let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).ceil();
    let widest =
        lines.iter().map(|l| l.chars().map(|c| scaled.h_advance(font.glyph_id(c))).sum::<f32>()).fold(0.0f32, f32::max);
    let width = (widest + 2.0 * margin).ceil() as u32;
    let height = (line_height * lines.len() as f32 + 2.0 * margin).ceil() as u32;
    let mut img = image::GrayImage::from_pixel(width, height, image::Luma([255]));
    for (i, line) in lines.iter().enumerate() {
        let baseline = margin + scaled.ascent() + line_height * i as f32;
        let mut x = margin;
        let mut prev = None;
        for c in line.chars() {
            let id = font.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(px), point(x, baseline));
            if let Some(outline) = font.outline_glyph(glyph) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let (px_x, px_y) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                    if px_x >= 0 && px_y >= 0 && (px_x as u32) < width && (px_y as u32) < height {
                        let v = (255.0 * (1.0 - coverage.clamp(0.0, 1.0))) as u8;
                        let p = img.get_pixel_mut(px_x as u32, px_y as u32);
                        p.0[0] = p.0[0].min(v);
                    }
                });
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("encode PNG");
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_png_with_dark_text_on_white() {
        let png = render_png(&["Hi", "Привет"], 24.0);
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        assert!(img.width() > 48 && img.height() > 48);
        assert!(img.pixels().any(|p| p.0[0] < 64), "some ink");
        assert_eq!(img.get_pixel(0, 0).0[0], 255, "white margin");
    }
}
