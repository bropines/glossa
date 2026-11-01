//! Screen capture for OCR: every monitor, the whole virtual desktop as one
//! picture (the backdrop of the fullscreen region-select overlay), or one
//! rectangle; plus the cheap change detector behind "watch a region and
//! re-recognise it when it changes".
//!
//! Capture goes through `xcap` on Windows and macOS (where it needs the
//! Screen Recording permission), and through our own X11 code or, on
//! Wayland, xcap or `grim` on Linux — see the private `screen` module.

use glossa_core::ocr::Rect;
use image::{GenericImageView, Rgba, RgbaImage};

use crate::screen;
use crate::window::Monitor;
use crate::{platform_err, Result};

/// One monitor and what was on it.
#[derive(Debug, Clone)]
pub struct MonitorShot {
    pub monitor: Monitor,
    pub image: RgbaImage,
}

impl MonitorShot {
    /// Image pixels per monitor coordinate unit: 1 on Windows and X11, the
    /// backing scale (usually 2) on macOS where monitors are measured in
    /// points.
    fn ratio(&self) -> f32 {
        if self.monitor.width == 0 {
            1.0
        } else {
            self.image.width() as f32 / self.monitor.width as f32
        }
    }
}

/// A sensible default for [`images_differ`]: a 4 % change in the brightness
/// of one cell of the comparison grid. Low enough for a subtitle line to
/// change it, high enough to ignore video compression shimmer.
pub const DEFAULT_DIFF_THRESHOLD: f32 = 0.04;

/// Capture every monitor.
pub fn capture_all() -> Result<Vec<MonitorShot>> {
    use crate::permissions::{self, Permission, Status};
    // Without the permission macOS returns only the desktop wallpaper, which
    // would be recognised as nothing at all; better to say why.
    if permissions::status(Permission::ScreenRecording) == Status::Missing
        && permissions::request(Permission::ScreenRecording) != Status::Granted
    {
        return Err(platform_err(
            "Glossa needs the Screen Recording permission to read text on the screen: allow it in \
             System Settings → Privacy & Security → Screen Recording, then restart Glossa",
        ));
    }
    let shots = screen::capture(None)?;
    if shots.is_empty() {
        return Err(platform_err("no monitors found to capture"));
    }
    Ok(shots)
}

/// Capture all monitors composited into one image covering the bounding box
/// of the virtual desktop, plus the desktop coordinates of its top-left
/// pixel (negative when a monitor sits left of or above the primary). Gaps
/// between monitors of different sizes are opaque black.
pub fn capture_virtual_desktop() -> Result<(RgbaImage, i32, i32)> {
    let shots = capture_all()?;
    Ok(composite(&shots))
}

/// Lay the shots out at their desktop positions on one canvas.
fn composite(shots: &[MonitorShot]) -> (RgbaImage, i32, i32) {
    // Positions are scaled by the shot's own pixel ratio, so a Retina
    // monitor's image lands where its points say. With mixed ratios
    // (macOS, one Retina and one plain display) the layout is approximate.
    let placed: Vec<(i64, i64, &RgbaImage)> = shots
        .iter()
        .map(|s| {
            let r = s.ratio() as f64;
            ((s.monitor.x as f64 * r).round() as i64, (s.monitor.y as f64 * r).round() as i64, &s.image)
        })
        .collect();
    let Some(min_x) = placed.iter().map(|p| p.0).min() else {
        return (RgbaImage::new(0, 0), 0, 0);
    };
    let min_y = placed.iter().map(|p| p.1).min().unwrap_or(0);
    let max_x = placed.iter().map(|p| p.0 + p.2.width() as i64).max().unwrap_or(0);
    let max_y = placed.iter().map(|p| p.1 + p.2.height() as i64).max().unwrap_or(0);
    let mut canvas = RgbaImage::from_pixel((max_x - min_x) as u32, (max_y - min_y) as u32, Rgba([0, 0, 0, 255]));
    for (x, y, img) in placed {
        image::imageops::replace(&mut canvas, img, x - min_x, y - min_y);
    }
    (canvas, min_x as i32, min_y as i32)
}

/// Capture one rectangle of the desktop, in the same coordinates as
/// [`Monitor`]. A rectangle on one monitor captures only that monitor; one
/// spanning several composites them.
pub fn capture_rect(rect: Rect) -> Result<RgbaImage> {
    if rect.is_empty() {
        return Err(platform_err("cannot capture an empty rectangle"));
    }
    if let Some(direct) = screen::capture_rect_direct(rect) {
        return direct;
    }
    let mut shots = screen::capture(Some(rect))?;
    // A rectangle wholly on one monitor needs only that monitor's pixels.
    if let Some(i) = shots.iter().position(|s| {
        let b = s.monitor.bounds();
        rect.x >= b.x && rect.y >= b.y && rect.x + rect.width <= b.x + b.width && rect.y + rect.height <= b.y + b.height
    }) {
        shots = vec![shots.swap_remove(i)];
    }
    crop_from_shots(&shots, rect).ok_or_else(|| platform_err("the rectangle is not on any monitor"))
}

/// Cut `rect` (desktop coordinates) out of the captured shots.
fn crop_from_shots(shots: &[MonitorShot], rect: Rect) -> Option<RgbaImage> {
    match shots {
        [] => None,
        [one] => {
            let r = one.ratio();
            let x = ((rect.x - one.monitor.x as f32) * r).round().max(0.0) as u32;
            let y = ((rect.y - one.monitor.y as f32) * r).round().max(0.0) as u32;
            crop_clamped(&one.image, x, y, (rect.width * r).round() as u32, (rect.height * r).round() as u32)
        }
        many => {
            let (canvas, ox, oy) = composite(many);
            let x = (rect.x - ox as f32).round().max(0.0) as u32;
            let y = (rect.y - oy as f32).round().max(0.0) as u32;
            crop_clamped(&canvas, x, y, rect.width.round() as u32, rect.height.round() as u32)
        }
    }
}

fn crop_clamped(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Option<RgbaImage> {
    if x >= img.width() || y >= img.height() {
        return None;
    }
    let w = w.min(img.width() - x);
    let h = h.min(img.height() - y);
    if w == 0 || h == 0 {
        return None;
    }
    Some(img.view(x, y, w, h).to_image())
}

/// Encode as PNG, the format every OCR provider accepts. Returns an empty
/// vector only if encoding fails, which for an in-memory buffer means the
/// image itself is degenerate.
pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    match image.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => out.into_inner(),
        Err(e) => {
            tracing::error!("PNG encoding failed: {e}");
            Vec::new()
        }
    }
}

/// The side of the comparison grid.
const GRID: u32 = 32;
/// At most this many pixels are sampled per grid cell per axis.
const SAMPLES: u32 = 8;

/// Whether two captures of the same region differ enough to be worth
/// recognising again.
///
/// Both images are reduced to a grid of up to 32×32 cells of average
/// brightness, sampling a few pixels per cell, so the cost does not grow
/// with the region; the images differ when any one cell's brightness moved
/// by more than `threshold` (0.0–1.0 of full scale). Measuring the worst
/// cell rather than the average is what makes a one-line subtitle change in
/// a large region count. Images of different sizes always differ.
pub fn images_differ(a: &RgbaImage, b: &RgbaImage, threshold: f32) -> bool {
    if a.dimensions() != b.dimensions() {
        return true;
    }
    let (w, h) = a.dimensions();
    if w == 0 || h == 0 {
        return false;
    }
    let ga = luma_grid(a);
    let gb = luma_grid(b);
    let limit = threshold.clamp(0.0, 1.0) * 255.0;
    ga.iter().zip(&gb).any(|(x, y)| (x - y).abs() > limit)
}

/// Average brightness per grid cell.
fn luma_grid(img: &RgbaImage) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let (cols, rows) = (GRID.min(w), GRID.min(h));
    let mut cells = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        let (y0, y1) = (row * h / rows, (row + 1) * h / rows);
        for col in 0..cols {
            let (x0, x1) = (col * w / cols, (col + 1) * w / cols);
            let step_x = ((x1 - x0) / SAMPLES).max(1);
            let step_y = ((y1 - y0) / SAMPLES).max(1);
            let (mut sum, mut n) = (0.0f32, 0u32);
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let [r, g, b, _] = img.get_pixel(x, y).0;
                    sum += 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                    n += 1;
                    x += step_x;
                }
                y += step_y;
            }
            cells.push(if n == 0 { 0.0 } else { sum / n as f32 });
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: u32, h: u32, v: u8) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([v, v, v, 255]))
    }

    fn shot(x: i32, y: i32, w: u32, h: u32, v: u8) -> MonitorShot {
        MonitorShot {
            monitor: Monitor {
                x,
                y,
                width: w,
                height: h,
                scale: 1.0,
                work_area: Rect::new(x as f32, y as f32, w as f32, h as f32),
                is_primary: false,
            },
            image: grey(w, h, v),
        }
    }

    #[test]
    fn identical_images_do_not_differ() {
        let a = grey(640, 480, 100);
        assert!(!images_differ(&a, &a.clone(), DEFAULT_DIFF_THRESHOLD));
        assert!(!images_differ(&a, &a.clone(), 0.0));
    }

    #[test]
    fn size_change_always_differs() {
        assert!(images_differ(&grey(10, 10, 0), &grey(10, 11, 0), 1.0));
    }

    #[test]
    fn a_small_text_change_in_a_big_region_is_seen() {
        let a = grey(800, 200, 20);
        let mut b = a.clone();
        // A bright 40×12 "word" in the lower-left corner: 0.3 % of the area.
        for y in 170..182 {
            for x in 30..70 {
                b.put_pixel(x, y, Rgba([240, 240, 240, 255]));
            }
        }
        assert!(images_differ(&a, &b, DEFAULT_DIFF_THRESHOLD));
    }

    #[test]
    fn faint_uniform_noise_is_ignored() {
        let a = grey(800, 200, 100);
        let mut b = a.clone();
        for (i, p) in b.pixels_mut().enumerate() {
            let v = if i % 2 == 0 { 103 } else { 98 };
            *p = Rgba([v, v, v, 255]);
        }
        assert!(!images_differ(&a, &b, DEFAULT_DIFF_THRESHOLD));
        // ...but a zero threshold notices it.
        assert!(images_differ(&a, &grey(800, 200, 103), 0.0));
    }

    #[test]
    fn threshold_scales_with_the_change() {
        let a = grey(64, 64, 100);
        let b = grey(64, 64, 120); // a 20/255 ≈ 7.8 % shift
        assert!(images_differ(&a, &b, 0.05));
        assert!(!images_differ(&a, &b, 0.10));
    }

    #[test]
    fn tiny_images_work() {
        let a = grey(1, 1, 0);
        assert!(images_differ(&a, &grey(1, 1, 255), 0.5));
        assert!(!images_differ(&RgbaImage::new(0, 0), &RgbaImage::new(0, 0), 0.0));
    }

    #[test]
    fn composite_handles_negative_origins_and_gaps() {
        let shots = [shot(0, 0, 100, 50, 200), shot(-80, 10, 80, 30, 50)];
        let (img, ox, oy) = composite(&shots);
        assert_eq!((ox, oy), (-80, 0));
        assert_eq!(img.dimensions(), (180, 50));
        assert_eq!(img.get_pixel(100, 0).0[0], 200, "primary at x = 0 → canvas 80");
        assert_eq!(img.get_pixel(0, 10).0[0], 50, "left monitor");
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255], "gap above the left monitor");
    }

    #[test]
    fn composite_of_nothing_is_empty() {
        let (img, ox, oy) = composite(&[]);
        assert_eq!((img.width(), img.height(), ox, oy), (0, 0, 0, 0));
    }

    #[test]
    fn crop_within_and_across_monitors() {
        let shots = [shot(0, 0, 100, 100, 10), shot(100, 0, 100, 100, 90)];
        let one = crop_from_shots(&shots[1..], Rect::new(120.0, 10.0, 30.0, 20.0)).unwrap();
        assert_eq!(one.dimensions(), (30, 20));
        assert_eq!(one.get_pixel(0, 0).0[0], 90);
        let both = crop_from_shots(&shots, Rect::new(90.0, 0.0, 20.0, 5.0)).unwrap();
        assert_eq!(both.dimensions(), (20, 5));
        assert_eq!(both.get_pixel(0, 0).0[0], 10);
        assert_eq!(both.get_pixel(19, 0).0[0], 90);
        // Clipped at the edge rather than failing.
        let edge = crop_from_shots(&shots[..1], Rect::new(90.0, 90.0, 50.0, 50.0)).unwrap();
        assert_eq!(edge.dimensions(), (10, 10));
        assert!(crop_from_shots(&shots[..1], Rect::new(500.0, 0.0, 5.0, 5.0)).is_none());
        assert!(crop_from_shots(&[], Rect::new(0.0, 0.0, 5.0, 5.0)).is_none());
    }

    #[test]
    fn retina_ratio_is_applied_when_cropping() {
        let mut s = shot(0, 0, 100, 100, 0);
        s.image = grey(200, 200, 0); // 2× backing scale
        let c = crop_from_shots(&[s], Rect::new(10.0, 10.0, 20.0, 20.0)).unwrap();
        assert_eq!(c.dimensions(), (40, 40));
    }

    #[test]
    fn png_has_a_signature() {
        let png = encode_png(&grey(4, 4, 7));
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    #[ignore = "needs a display"]
    fn capture_the_real_desktop() {
        let (img, _, _) = capture_virtual_desktop().unwrap();
        assert!(img.width() > 0 && img.height() > 0);
    }
}
