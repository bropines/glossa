//! The image API modules get as host calls (`image.crop`, `image.resize`…):
//! the few operations OCR preprocessing needs, written once here and offered
//! to every runtime through [`HostApi`](crate::host_api::HostApi), so a
//! JavaScript, Python, native or process module needs neither PIL nor
//! OpenCV to prepare a picture for a network OCR service.
//!
//! Images live in an [`ImageStore`] that exists for one module call:
//! operations take a `handle` (or encoded bytes as `data_base64`) and return
//! a new handle, so a large picture crosses a process boundary once, as the
//! final encoded upload, instead of on every step. Every image carries the
//! [`Transform`] back to the image it was decoded from, composed through
//! every crop, scale, pad and rotation, so a module can map the boxes an
//! engine found in the prepared copy back onto the original.
//!
//! Pixels are grey (1 channel) or RGBA (4). Everything is bounded: decoded
//! images and results at most [`MAX_PIXELS`] and [`MAX_SIDE`], the store at
//! most [`STORE_BUDGET`] bytes and [`MAX_IMAGES`] images, filter sizes
//! capped. Bad parameters are `bad_request` errors; nothing here panics.

use std::io::Cursor;

use base64::Engine;
use glossa_module_sdk::wire::{ErrorKind, ImageInfo, ModuleError, Transform};
use image::{DynamicImage, ImageBuffer, Luma, Rgba};
use serde_json::Value;

/// The most pixels an image may have, decoded or produced.
pub const MAX_PIXELS: u64 = 100_000_000;
/// The longest side an image may have.
pub const MAX_SIDE: u32 = 50_000;
/// Decoded pixel bytes all live images of one call may hold together.
pub const STORE_BUDGET: u64 = 1 << 30;
/// How many images one call may hold at once.
pub const MAX_IMAGES: usize = 512;
/// The largest encoded input accepted.
pub const MAX_ENCODED: usize = 64 << 20;

type Res<T> = Result<T, ModuleError>;

fn bad(message: impl Into<String>) -> ModuleError {
    ModuleError::new(ErrorKind::BadRequest, message)
}

/// Pixels, row-major, top row first.
#[derive(Debug, Clone, PartialEq)]
pub struct Img {
    pub width: u32,
    pub height: u32,
    /// 1 (grey) or 4 (RGBA).
    pub channels: u8,
    pub data: Vec<u8>,
}

impl Img {
    fn bytes(&self) -> u64 {
        self.width as u64 * self.height as u64 * self.channels as u64
    }

    /// A blank image of `color`, checked against the limits.
    pub fn new(width: u32, height: u32, channels: u8, color: [u8; 4]) -> Res<Img> {
        check_size(width, height)?;
        let px = if channels == 1 { vec![luma(color)] } else { color.to_vec() };
        let data = px.repeat(width as usize * height as usize);
        Ok(Img { width, height, channels, data })
    }

    fn px(&self, x: u32, y: u32) -> &[u8] {
        let c = self.channels as usize;
        let at = (y as usize * self.width as usize + x as usize) * c;
        &self.data[at..at + c]
    }

    /// As RGBA, converting grey.
    fn to_rgba(&self) -> Img {
        if self.channels == 4 {
            return self.clone();
        }
        let data = self.data.iter().flat_map(|&v| [v, v, v, 255]).collect();
        Img { width: self.width, height: self.height, channels: 4, data }
    }

    /// As grey (Rec. 601 luma), transparent parts composited onto white:
    /// recognisers read dark text on light paper.
    fn to_gray(&self) -> Img {
        if self.channels == 1 {
            return self.clone();
        }
        let data = self.data.as_chunks::<4>().0.iter().map(|p| luma(*p)).collect();
        Img { width: self.width, height: self.height, channels: 1, data }
    }

    fn with_channels(&self, channels: u8) -> Img {
        if channels == 1 {
            self.to_gray()
        } else {
            self.to_rgba()
        }
    }

    fn from_dynamic(img: DynamicImage) -> Img {
        let (width, height) = (img.width(), img.height());
        match img {
            DynamicImage::ImageLuma8(b) => Img { width, height, channels: 1, data: b.into_raw() },
            DynamicImage::ImageLuma16(_) => Img { width, height, channels: 1, data: img.to_luma8().into_raw() },
            other => Img { width, height, channels: 4, data: other.to_rgba8().into_raw() },
        }
    }

    fn to_dynamic(&self) -> Option<DynamicImage> {
        if self.channels == 1 {
            ImageBuffer::<Luma<u8>, _>::from_raw(self.width, self.height, self.data.clone())
                .map(DynamicImage::ImageLuma8)
        } else {
            ImageBuffer::<Rgba<u8>, _>::from_raw(self.width, self.height, self.data.clone())
                .map(DynamicImage::ImageRgba8)
        }
    }
}

fn luma(p: [u8; 4]) -> u8 {
    let y = (299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32 + 500) / 1000;
    let a = p[3] as u32;
    ((y * a + 255 * (255 - a) + 127) / 255) as u8
}

fn check_size(width: u32, height: u32) -> Res<()> {
    if width == 0 || height == 0 {
        return Err(bad(format!("an image cannot be {width}×{height}")));
    }
    if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
        return Err(bad(format!(
            "{width}×{height} is larger than the image API allows ({MAX_PIXELS} pixels, {MAX_SIDE} per side)"
        )));
    }
    Ok(())
}

// -- the store ------------------------------------------------------------------------

enum Slot {
    Free,
    /// Registered but not decoded yet: the request's own image, which a
    /// module that never touches it should not pay for.
    Encoded(Vec<u8>),
    Ready {
        img: Img,
        transform: Transform,
        format: Option<&'static str>,
    },
}

/// The images of one module call.
#[derive(Default)]
pub struct ImageStore {
    slots: Vec<Slot>,
    used: u64,
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register encoded bytes to be decoded on first use; returns the handle.
    pub fn insert_encoded(&mut self, bytes: Vec<u8>) -> u32 {
        self.slots.push(Slot::Encoded(bytes));
        self.slots.len() as u32
    }

    fn insert(&mut self, img: Img, transform: Transform, format: Option<&'static str>) -> Res<u32> {
        let live = self.slots.iter().filter(|s| !matches!(s, Slot::Free)).count();
        if live >= MAX_IMAGES {
            return Err(bad(format!("more than {MAX_IMAGES} images at once; free the ones you are done with")));
        }
        if self.used + img.bytes() > STORE_BUDGET {
            return Err(bad("the images of this call use too much memory; free the ones you are done with"));
        }
        self.used += img.bytes();
        let slot = Slot::Ready { img, transform, format };
        match self.slots.iter().position(|s| matches!(s, Slot::Free)) {
            Some(i) => {
                self.slots[i] = slot;
                Ok(i as u32 + 1)
            }
            None => {
                self.slots.push(slot);
                Ok(self.slots.len() as u32)
            }
        }
    }

    fn info(&self, handle: u32) -> Res<ImageInfo> {
        match self.slots.get((handle as usize).wrapping_sub(1)) {
            Some(Slot::Ready { img, transform, format }) => Ok(ImageInfo {
                handle,
                width: img.width,
                height: img.height,
                channels: img.channels,
                transform: *transform,
                format: format.map(str::to_owned),
                ..Default::default()
            }),
            _ => Err(bad(format!("no image with handle {handle}"))),
        }
    }

    /// The image behind `handle`, decoding it if it is still encoded.
    fn get(&mut self, handle: u32) -> Res<(&Img, Transform)> {
        let i = (handle as usize).wrapping_sub(1);
        if let Some(Slot::Encoded(bytes)) = self.slots.get_mut(i) {
            let bytes = std::mem::take(bytes);
            match decode(&bytes) {
                Ok((img, format)) if self.used + img.bytes() <= STORE_BUDGET => {
                    self.used += img.bytes();
                    self.slots[i] = Slot::Ready { img, transform: Transform::IDENTITY, format: Some(format) };
                }
                failed => {
                    self.slots[i] = Slot::Encoded(bytes);
                    return Err(failed.err().unwrap_or_else(|| bad("the images of this call use too much memory")));
                }
            }
        }
        match self.slots.get(i) {
            Some(Slot::Ready { img, transform, .. }) => Ok((img, *transform)),
            _ => Err(bad(format!("no image with handle {handle}"))),
        }
    }

    fn free(&mut self, handle: u32) {
        if let Some(slot) = self.slots.get_mut((handle as usize).wrapping_sub(1)) {
            if let Slot::Ready { img, .. } = slot {
                self.used = self.used.saturating_sub(img.bytes());
            }
            *slot = Slot::Free;
        }
    }

    /// The input of an operation: `handle`, or `data_base64` decoded and
    /// registered on the spot.
    fn source(&mut self, p: &Value) -> Res<u32> {
        if let Some(h) = p.get("handle").and_then(Value::as_u64) {
            return u32::try_from(h).map_err(|_| bad("handle out of range"));
        }
        if let Some(b64) = p.get("data_base64").or_else(|| p.get("image_base64")).and_then(Value::as_str) {
            let bytes = b64_decode(b64)?;
            let (img, format) = decode(&bytes)?;
            return self.insert(img, Transform::IDENTITY, Some(format));
        }
        Err(bad("give the image as \"handle\" or \"data_base64\""))
    }

    /// Store `img`, derived from `parent` by `local` (a map from the new
    /// image's pixels to the parent's), and describe it.
    fn derive(&mut self, parent: Transform, local: Transform, img: Img) -> Res<ImageInfo> {
        let h = self.insert(img, parent.compose(&local), None)?;
        self.info(h)
    }

    /// Run one `image.<op>` host call.
    pub fn call(&mut self, op: &str, p: &Value) -> Res<ImageInfo> {
        match op {
            "decode" => {
                let h = self.source(p)?;
                self.get(h)?;
                self.info(h)
            }
            "info" => {
                let h = self.source(p)?;
                self.get(h)?;
                self.info(h)
            }
            "free" => {
                if let Some(h) = p.get("handle").and_then(Value::as_u64) {
                    self.free(h.min(u32::MAX as u64) as u32);
                }
                Ok(ImageInfo::default())
            }
            "new" => {
                let (w, h) = (u32_of(p, "width")?, u32_of(p, "height")?);
                let color = color_of(p.get("color"), [255, 255, 255, 255])?;
                let channels = if color[0] == color[1] && color[1] == color[2] && color[3] == 255 { 1 } else { 4 };
                let img = Img::new(w, h, channels, color)?;
                let h = self.insert(img, Transform::IDENTITY, None)?;
                self.info(h)
            }
            "encode" | "to_base64" => {
                let h = self.source(p)?;
                let (img, _) = self.get(h)?;
                let format = p.get("format").and_then(Value::as_str).unwrap_or("png");
                let quality = opt_f64(p, "quality")?.unwrap_or(90.0).clamp(1.0, 100.0) as u8;
                let (bytes, mime) = encode(img, format, quality)?;
                let mut info = self.info(h)?;
                info.data_base64 = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
                info.mime = Some(mime.into());
                Ok(info)
            }
            "pixels" => {
                let h = self.source(p)?;
                let (img, _) = self.get(h)?;
                let data = base64::engine::general_purpose::STANDARD.encode(&img.data);
                let mut info = self.info(h)?;
                info.data_base64 = Some(data);
                Ok(info)
            }
            "split_tiles" => {
                let h = self.source(p)?;
                let (img, t) = self.get(h)?;
                let tw = opt_u32(p, "max_width")?.unwrap_or(img.width).max(1);
                let th = opt_u32(p, "max_height")?.unwrap_or(img.height).max(1);
                let overlap = opt_u32(p, "overlap")?.unwrap_or(0);
                if overlap >= tw.min(th) {
                    return Err(bad("overlap must be smaller than a tile"));
                }
                let rects = tile_rects(img.width, img.height, tw, th, overlap);
                if rects.len() > MAX_IMAGES {
                    return Err(bad(format!("{} tiles is too many", rects.len())));
                }
                let crops: Vec<(Img, Transform)> = rects
                    .into_iter()
                    .map(|(x, y, w, h)| (crop(img, x, y, w, h), Transform([1.0, 0.0, x as f64, 0.0, 1.0, y as f64])))
                    .collect();
                let mut info = self.info(h)?;
                for (tile, local) in crops {
                    info.tiles.push(self.derive(t, local, tile)?);
                }
                Ok(info)
            }
            _ => self.transform_op(op, p),
        }
    }

    /// The operations that turn one image into another.
    fn transform_op(&mut self, op: &str, p: &Value) -> Res<ImageInfo> {
        let h = self.source(p)?;
        let (img, t) = self.get(h)?;
        let (w, hgt) = (img.width as f64, img.height as f64);
        let identity = Transform::IDENTITY;
        let (out, local, extra): (Img, Transform, Extra) = match op {
            "crop" => {
                let x = opt_f64(p, "x")?.unwrap_or(0.0).max(0.0).floor() as u32;
                let y = opt_f64(p, "y")?.unwrap_or(0.0).max(0.0).floor() as u32;
                let cw = opt_f64(p, "width")?.unwrap_or(w).max(0.0).round() as u32;
                let ch = opt_f64(p, "height")?.unwrap_or(hgt).max(0.0).round() as u32;
                let (x, y) = (x.min(img.width), y.min(img.height));
                let (cw, ch) = (cw.min(img.width - x), ch.min(img.height - y));
                if cw == 0 || ch == 0 {
                    return Err(bad("the crop is empty or outside the image"));
                }
                (crop(img, x, y, cw, ch), Transform([1.0, 0.0, x as f64, 0.0, 1.0, y as f64]), Extra::None)
            }
            "resize" => {
                let (nw, nh) = resize_target(img, p)?;
                if (nw, nh) == (img.width, img.height) {
                    return self.info(h);
                }
                let filter = filter_of(p.get("filter"))?;
                let out = resize(img, nw, nh, filter)?;
                (out, Transform([w / nw as f64, 0.0, 0.0, 0.0, hgt / nh as f64, 0.0]), Extra::None)
            }
            "scale_to_min_height" => {
                let min = opt_u32(p, "min_height")?.unwrap_or(32).max(1);
                let max_side = opt_u32(p, "max_side")?.unwrap_or(MAX_SIDE).min(MAX_SIDE);
                if img.height >= min {
                    return self.info(h);
                }
                let mut k = min as f64 / hgt;
                k = k.min(max_side as f64 / w.max(hgt));
                k = k.min((MAX_PIXELS as f64 / (w * hgt)).sqrt());
                let (nw, nh) = (((w * k).round() as u32).max(1), ((hgt * k).round() as u32).max(1));
                if nh <= img.height {
                    return self.info(h);
                }
                let out = resize(img, nw, nh, filter_of(p.get("filter").or(Some(&Value::from("lanczos"))))?)?;
                (out, Transform([w / nw as f64, 0.0, 0.0, 0.0, hgt / nh as f64, 0.0]), Extra::None)
            }
            "rotate" => {
                let angle = opt_f64(p, "angle")?.unwrap_or(0.0);
                let bg = color_of(p.get("background"), [255, 255, 255, 255])?;
                let expand = p.get("expand").and_then(Value::as_bool).unwrap_or(true);
                let (out, local) = rotate(img, angle, bg, expand)?;
                (out, local, Extra::None)
            }
            "flip" => {
                let vertical = matches!(p.get("direction").and_then(Value::as_str), Some("vertical" | "v"));
                let mut out = img.clone();
                flip(&mut out, vertical);
                let local = if vertical {
                    Transform([1.0, 0.0, 0.0, 0.0, -1.0, hgt])
                } else {
                    Transform([-1.0, 0.0, w, 0.0, 1.0, 0.0])
                };
                (out, local, Extra::None)
            }
            "grayscale" | "greyscale" => (img.to_gray(), identity, Extra::None),
            "invert" => {
                let mut out = img.clone();
                map_colour(&mut out, |v| 255 - v);
                (out, identity, Extra::None)
            }
            "threshold" => {
                let gray = img.to_gray();
                let level = match p.get("value") {
                    None | Some(Value::Null) => otsu(&histogram(&gray.data)),
                    Some(Value::String(s)) if s == "otsu" => otsu(&histogram(&gray.data)),
                    Some(v) => {
                        v.as_f64().ok_or_else(|| bad("value: a number 0-255 or \"otsu\""))?.clamp(0.0, 255.0) as u8
                    }
                };
                let mut out = gray;
                out.data.iter_mut().for_each(|v| *v = if *v > level { 255 } else { 0 });
                (out, identity, Extra::Threshold(level))
            }
            "adaptive_threshold" => {
                let block = opt_u32(p, "block_size")?.unwrap_or(31).clamp(3, 255) | 1;
                let c = opt_f64(p, "c")?.unwrap_or(10.0).clamp(-255.0, 255.0);
                (adaptive_threshold(&img.to_gray(), block, c), identity, Extra::None)
            }
            "adjust" | "contrast" | "brightness" | "gamma" => {
                let contrast = opt_f64(p, "contrast")?.unwrap_or(1.0).clamp(0.0, 100.0);
                let brightness = opt_f64(p, "brightness")?.unwrap_or(0.0).clamp(-255.0, 255.0);
                let gamma = opt_f64(p, "gamma")?.unwrap_or(1.0).clamp(0.01, 100.0);
                let lut: Vec<u8> = (0..256)
                    .map(|v| {
                        let v = (v as f64 - 128.0) * contrast + 128.0 + brightness;
                        let v = (v.clamp(0.0, 255.0) / 255.0).powf(1.0 / gamma) * 255.0;
                        v.round().clamp(0.0, 255.0) as u8
                    })
                    .collect();
                let mut out = img.clone();
                map_colour(&mut out, |v| lut[v as usize]);
                (out, identity, Extra::None)
            }
            "normalize" | "normalise" | "auto_levels" => {
                let cutoff = opt_f64(p, "cutoff")?.unwrap_or(0.5).clamp(0.0, 49.0);
                (normalize(img, cutoff), identity, Extra::None)
            }
            "blur" => {
                let radius = opt_f64(p, "radius")?.unwrap_or(1.0).clamp(0.0, 100.0);
                let gaussian = !matches!(p.get("kind").and_then(Value::as_str), Some("box"));
                (blur(img, radius, gaussian), identity, Extra::None)
            }
            "sharpen" => {
                let amount = opt_f64(p, "amount")?.unwrap_or(1.0).clamp(0.0, 10.0);
                let radius = opt_f64(p, "radius")?.unwrap_or(1.0).clamp(0.1, 20.0);
                (sharpen(img, radius, amount), identity, Extra::None)
            }
            "pad" => {
                let all = opt_u32(p, "all")?.unwrap_or(0);
                let side = |k: &str| -> Res<u32> { Ok(opt_u32(p, k)?.unwrap_or(all)) };
                let (top, right, bottom, left) = (side("top")?, side("right")?, side("bottom")?, side("left")?);
                let color = color_of(p.get("color"), [255, 255, 255, 255])?;
                let out = pad(img, top, right, bottom, left, color)?;
                (out, Transform([1.0, 0.0, -(left as f64), 0.0, 1.0, -(top as f64)]), Extra::None)
            }
            "deskew" => {
                let max = opt_f64(p, "max_angle")?.unwrap_or(10.0).clamp(0.5, 45.0);
                let skew = estimate_skew(img, max);
                if skew.abs() < 0.05 {
                    let mut info = self.info(h)?;
                    info.angle = Some(0.0);
                    return Ok(info);
                }
                let bg = color_of(p.get("background"), background_of(img))?;
                let (out, local) = rotate(img, -skew, bg, true)?;
                (out, local, Extra::Angle(-skew as f32))
            }
            "paste" | "composite" => {
                let src_handle = u32_of(p, "source")?;
                let base = img.clone();
                let (src, _) = self.get(src_handle)?;
                let x = opt_f64(p, "x")?.unwrap_or(0.0).round() as i64;
                let y = opt_f64(p, "y")?.unwrap_or(0.0).round() as i64;
                (paste(&base, src, x, y), identity, Extra::None)
            }
            "draw_rect" => {
                let color = color_of(p.get("color"), [255, 0, 0, 255])?;
                let thickness = opt_u32(p, "thickness")?.unwrap_or(2).clamp(1, 1000);
                let r = (opt_f64(p, "x")?, opt_f64(p, "y")?, opt_f64(p, "width")?, opt_f64(p, "height")?);
                let (Some(x), Some(y), Some(rw), Some(rh)) = r else {
                    return Err(bad("draw_rect needs x, y, width and height"));
                };
                (draw_rect(img, x, y, rw, rh, color, thickness), identity, Extra::None)
            }
            other => {
                return Err(ModuleError::new(ErrorKind::Unsupported, format!("unknown image operation {other:?}")))
            }
        };
        let mut info = self.derive(t, local, out)?;
        match extra {
            Extra::None => {}
            Extra::Threshold(v) => info.threshold = Some(v),
            Extra::Angle(a) => info.angle = Some(a),
        }
        Ok(info)
    }
}

enum Extra {
    None,
    Threshold(u8),
    Angle(f32),
}

// -- parameters ------------------------------------------------------------------------

fn b64_decode(s: &str) -> Res<Vec<u8>> {
    if s.len() > MAX_ENCODED / 3 * 4 + 4 {
        return Err(bad("the encoded image is too large"));
    }
    let s = s.split_once("base64,").map_or(s, |(_, rest)| rest);
    base64::engine::general_purpose::STANDARD.decode(s.trim().as_bytes()).map_err(|e| bad(format!("data_base64: {e}")))
}

fn opt_f64(p: &Value, key: &str) -> Res<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_f64().filter(|f| f.is_finite()).map(Some).ok_or_else(|| bad(format!("{key}: expected a number")))
        }
    }
}

fn opt_u32(p: &Value, key: &str) -> Res<Option<u32>> {
    Ok(opt_f64(p, key)?.map(|v| v.round().clamp(0.0, u32::MAX as f64) as u32))
}

fn u32_of(p: &Value, key: &str) -> Res<u32> {
    opt_u32(p, key)?.ok_or_else(|| bad(format!("{key} is required")))
}

/// `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"`, `"white"`, `"black"`,
/// `"transparent"`, `[r, g, b]`, `[r, g, b, a]` or a grey level.
pub fn color_of(v: Option<&Value>, default: [u8; 4]) -> Res<[u8; 4]> {
    let byte = |v: &Value| v.as_f64().map(|f| f.clamp(0.0, 255.0) as u8);
    match v {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => {
            let g = n.as_f64().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
            Ok([g, g, g, 255])
        }
        Some(Value::Array(a)) if a.len() == 3 || a.len() == 4 => {
            let mut out = [0, 0, 0, 255];
            for (i, v) in a.iter().enumerate() {
                out[i] = byte(v).ok_or_else(|| bad("colour: numbers 0-255"))?;
            }
            Ok(out)
        }
        Some(Value::String(s)) => {
            let s = s.trim().to_ascii_lowercase();
            match s.as_str() {
                "white" => return Ok([255, 255, 255, 255]),
                "black" => return Ok([0, 0, 0, 255]),
                "transparent" => return Ok([255, 255, 255, 0]),
                "red" => return Ok([255, 0, 0, 255]),
                _ => {}
            }
            let hex = s.trim_start_matches('#');
            let digits: Option<Vec<u8>> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect();
            match (hex.len(), digits) {
                (3, Some(d)) => Ok([d[0] * 17, d[1] * 17, d[2] * 17, 255]),
                (6, Some(d)) => Ok([d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], 255]),
                (8, Some(d)) => Ok([d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], d[6] * 16 + d[7]]),
                _ => Err(bad(format!("colour {s:?}: use #rrggbb, [r, g, b] or a name"))),
            }
        }
        Some(_) => Err(bad("colour: use #rrggbb, [r, g, b] or a name")),
    }
}

#[derive(Debug, Clone, Copy)]
enum Filter {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

fn filter_of(v: Option<&Value>) -> Res<Filter> {
    match v.and_then(Value::as_str).unwrap_or("bilinear") {
        "nearest" => Ok(Filter::Nearest),
        "bilinear" | "linear" | "triangle" => Ok(Filter::Bilinear),
        "bicubic" | "cubic" => Ok(Filter::Bicubic),
        "lanczos" | "lanczos3" => Ok(Filter::Lanczos),
        other => Err(bad(format!("filter {other:?}: nearest, bilinear, bicubic or lanczos"))),
    }
}

/// The size `resize` aims for: `scale`; `max_side` (shrink only);
/// `width`/`height` with `mode` `fit` (inside the box, the default),
/// `fill` (cover the box) or `exact`; or one of `width`/`height` alone
/// (the other follows the aspect ratio).
fn resize_target(img: &Img, p: &Value) -> Res<(u32, u32)> {
    let (w, h) = (img.width as f64, img.height as f64);
    let k = if let Some(s) = opt_f64(p, "scale")? {
        if s <= 0.0 {
            return Err(bad("scale must be above zero"));
        }
        s
    } else if let Some(m) = opt_f64(p, "max_side")? {
        (m.max(1.0) / w.max(h)).min(1.0)
    } else {
        let (tw, th) = (opt_f64(p, "width")?, opt_f64(p, "height")?);
        let mode = p.get("mode").and_then(Value::as_str).unwrap_or("fit");
        match (tw, th) {
            (Some(tw), Some(th)) if mode == "exact" => return clamp_size(tw, th),
            (Some(tw), Some(th)) if mode == "fill" => (tw / w).max(th / h),
            (Some(tw), Some(th)) => (tw / w).min(th / h),
            (Some(tw), None) => tw / w,
            (None, Some(th)) => th / h,
            (None, None) => return Err(bad("resize needs scale, max_side, width or height")),
        }
    };
    clamp_size(w * k, h * k)
}

fn clamp_size(w: f64, h: f64) -> Res<(u32, u32)> {
    if !(w.is_finite() && h.is_finite()) {
        return Err(bad("the size is not a number"));
    }
    let (w, h) = (w.round().max(1.0), h.round().max(1.0));
    if w > MAX_SIDE as f64 || h > MAX_SIDE as f64 || w * h > MAX_PIXELS as f64 {
        return Err(bad(format!("{w}×{h} is larger than the image API allows")));
    }
    Ok((w as u32, h as u32))
}

// -- codecs ------------------------------------------------------------------------------

/// Decode encoded bytes, refusing pictures beyond the limits before their
/// pixels are allocated.
pub fn decode(bytes: &[u8]) -> Res<(Img, &'static str)> {
    if bytes.is_empty() {
        return Err(bad("the image is empty"));
    }
    if bytes.len() > MAX_ENCODED {
        return Err(bad("the encoded image is too large"));
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| bad(format!("cannot read the image: {e}")))?;
    let format = match reader.format() {
        Some(image::ImageFormat::Png) => "png",
        Some(image::ImageFormat::Jpeg) => "jpeg",
        Some(image::ImageFormat::WebP) => "webp",
        Some(_) => "other",
        None => return Err(bad("not an image in a format Glossa reads (PNG, JPEG, WebP)")),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_PIXELS * 4 + (16 << 20));
    let mut reader = reader;
    reader.limits(limits);
    let img = reader.decode().map_err(|e| bad(format!("cannot decode the image: {e}")))?;
    check_size(img.width(), img.height())?;
    Ok((Img::from_dynamic(img), format))
}

fn encode(img: &Img, format: &str, quality: u8) -> Res<(Vec<u8>, &'static str)> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    let failed = |e: image::ImageError| ModuleError::other(format!("encoding the image failed: {e}"));
    let color = if img.channels == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgba8 };
    match format.to_ascii_lowercase().as_str() {
        "png" => {
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(&img.data, img.width, img.height, color)
                .map_err(failed)?;
            Ok((out, "image/png"))
        }
        "jpeg" | "jpg" => {
            // JPEG has no alpha: flatten onto white.
            let (data, color) = if img.channels == 1 {
                (img.data.clone(), image::ExtendedColorType::L8)
            } else {
                let rgb = img
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| {
                        let a = p[3] as u32;
                        let f = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
                        [f(p[0]), f(p[1]), f(p[2])]
                    })
                    .collect();
                (rgb, image::ExtendedColorType::Rgb8)
            };
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(&data, img.width, img.height, color)
                .map_err(failed)?;
            Ok((out, "image/jpeg"))
        }
        "webp" => {
            let rgba;
            let (data, color) = if img.channels == 1 {
                rgba = img.to_rgba();
                (&rgba.data, image::ExtendedColorType::Rgba8)
            } else {
                (&img.data, color)
            };
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .write_image(data, img.width, img.height, color)
                .map_err(failed)?;
            Ok((out, "image/webp"))
        }
        other => Err(bad(format!("format {other:?}: png, jpeg or webp"))),
    }
}

// -- operations -------------------------------------------------------------------------

fn crop(img: &Img, x: u32, y: u32, w: u32, h: u32) -> Img {
    let c = img.channels as usize;
    let mut data = Vec::with_capacity(w as usize * h as usize * c);
    for row in y..y + h {
        let start = (row as usize * img.width as usize + x as usize) * c;
        data.extend_from_slice(&img.data[start..start + w as usize * c]);
    }
    Img { width: w, height: h, channels: img.channels, data }
}

fn resize(img: &Img, w: u32, h: u32, filter: Filter) -> Res<Img> {
    check_size(w, h)?;
    let f = match filter {
        Filter::Nearest => image::imageops::FilterType::Nearest,
        Filter::Bilinear => image::imageops::FilterType::Triangle,
        Filter::Bicubic => image::imageops::FilterType::CatmullRom,
        Filter::Lanczos => image::imageops::FilterType::Lanczos3,
    };
    let dynamic = img.to_dynamic().ok_or_else(|| ModuleError::other("inconsistent image buffer"))?;
    Ok(Img::from_dynamic(dynamic.resize_exact(w, h, f)).with_channels(img.channels))
}

/// Turn `img` by `degrees` clockwise. Right angles are exact; anything else
/// is resampled bilinearly, with `bg` where the picture does not reach.
/// `expand` grows the canvas to hold the whole turned picture; otherwise it
/// keeps its size, about the same centre. Returns the map from the new
/// image's pixels to the old one's.
fn rotate(img: &Img, degrees: f64, bg: [u8; 4], expand: bool) -> Res<(Img, Transform)> {
    let (w, h) = (img.width as f64, img.height as f64);
    let quarter = (degrees / 90.0).round();
    if (degrees - quarter * 90.0).abs() < 1e-9 {
        let turns = (quarter as i64).rem_euclid(4);
        let (out, local) = match turns {
            0 => (img.clone(), Transform::IDENTITY),
            1 => (rotate90(img), Transform([0.0, 1.0, 0.0, -1.0, 0.0, h])),
            2 => {
                let mut out = img.clone();
                flip(&mut out, false);
                flip(&mut out, true);
                (out, Transform([-1.0, 0.0, w, 0.0, -1.0, h]))
            }
            _ => (rotate270(img), Transform([0.0, -1.0, w, 1.0, 0.0, 0.0])),
        };
        return Ok((out, local));
    }
    let (s, c) = degrees.to_radians().sin_cos();
    let (nw, nh) =
        if expand { ((w * c.abs() + h * s.abs()).ceil(), (w * s.abs() + h * c.abs()).ceil()) } else { (w, h) };
    let (nw, nh) = clamp_size(nw, nh)?;
    let channels = if bg[3] == 255 && bg[0] == bg[1] && bg[1] == bg[2] { img.channels } else { 4 };
    let src = img.with_channels(channels);
    let mut out = Img::new(nw, nh, channels, bg)?;
    // New → old: old = R(-θ)(new - centre_new) + centre_old.
    let (cnx, cny) = (nw as f64 / 2.0, nh as f64 / 2.0);
    let (cox, coy) = (w / 2.0, h / 2.0);
    let local = Transform([c, s, cox - c * cnx - s * cny, -s, c, coy + s * cnx - c * cny]);
    let ch = channels as usize;
    for y in 0..nh {
        for x in 0..nw {
            let (sx, sy) = local.map_point(x as f64 + 0.5, y as f64 + 0.5);
            if let Some(px) = sample(&src, sx - 0.5, sy - 0.5) {
                let at = (y as usize * nw as usize + x as usize) * ch;
                out.data[at..at + ch].copy_from_slice(&px[..ch]);
            }
        }
    }
    Ok((out, local))
}

/// Bilinear sample at a pixel position (pixel centres at integers); `None`
/// outside the picture.
fn sample(img: &Img, x: f64, y: f64) -> Option<[u8; 4]> {
    let (w, h) = (img.width as f64, img.height as f64);
    if !(x > -1.0 && y > -1.0 && x < w && y < h) {
        return None;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let clamp = |v: f64, max: u32| (v.max(0.0) as u32).min(max - 1);
    let (xa, xb) = (clamp(x0, img.width), clamp(x0 + 1.0, img.width));
    let (ya, yb) = (clamp(y0, img.height), clamp(y0 + 1.0, img.height));
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate().take(img.channels as usize) {
        let p = |x, y| img.px(x, y)[i] as f64;
        let top = p(xa, ya) * (1.0 - fx) + p(xb, ya) * fx;
        let bottom = p(xa, yb) * (1.0 - fx) + p(xb, yb) * fx;
        *o = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

fn rotate90(img: &Img) -> Img {
    let (w, h, c) = (img.width as usize, img.height as usize, img.channels as usize);
    let mut data = vec![0; img.data.len()];
    for y in 0..h {
        for x in 0..w {
            // Old (x, y) lands at new (h - 1 - y, x) in a w-tall, h-wide image.
            let to = (x * h + (h - 1 - y)) * c;
            let from = (y * w + x) * c;
            data[to..to + c].copy_from_slice(&img.data[from..from + c]);
        }
    }
    Img { width: img.height, height: img.width, channels: img.channels, data }
}

fn rotate270(img: &Img) -> Img {
    let (w, h, c) = (img.width as usize, img.height as usize, img.channels as usize);
    let mut data = vec![0; img.data.len()];
    for y in 0..h {
        for x in 0..w {
            let to = ((w - 1 - x) * h + y) * c;
            let from = (y * w + x) * c;
            data[to..to + c].copy_from_slice(&img.data[from..from + c]);
        }
    }
    Img { width: img.height, height: img.width, channels: img.channels, data }
}

fn flip(img: &mut Img, vertical: bool) {
    let (w, h, c) = (img.width as usize, img.height as usize, img.channels as usize);
    let row = w * c;
    if vertical {
        for y in 0..h / 2 {
            let (top, rest) = img.data.split_at_mut((h - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut rest[..row]);
        }
    } else {
        for line in img.data.chunks_exact_mut(row) {
            for x in 0..w / 2 {
                for k in 0..c {
                    line.swap(x * c + k, (w - 1 - x) * c + k);
                }
            }
        }
    }
}

/// Apply `f` to the colour channels, leaving alpha alone.
fn map_colour(img: &mut Img, f: impl Fn(u8) -> u8) {
    if img.channels == 1 {
        img.data.iter_mut().for_each(|v| *v = f(*v));
    } else {
        for p in img.data.as_chunks_mut::<4>().0 {
            for v in &mut p[..3] {
                *v = f(*v);
            }
        }
    }
}

fn histogram(values: &[u8]) -> [u64; 256] {
    let mut h = [0u64; 256];
    for &v in values {
        h[v as usize] += 1;
    }
    h
}

/// Otsu's threshold: the level that best separates the histogram into two
/// classes (pixels above it are the light class).
pub fn otsu(hist: &[u64; 256]) -> u8 {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return 127;
    }
    let sum: f64 = hist.iter().enumerate().map(|(i, &n)| i as f64 * n as f64).sum();
    let (mut sum_b, mut w_b) = (0.0, 0u64);
    let (mut best, mut best_var) = (0u8, -1.0);
    for (t, &n) in hist.iter().enumerate() {
        w_b += n;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * n as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f).powi(2);
        if var > best_var {
            best_var = var;
            best = t as u8;
        }
    }
    best
}

/// Each pixel against the mean of the `block`×`block` square around it,
/// less `c`: lighter is white. Copes with uneven lighting where one global
/// threshold cannot.
fn adaptive_threshold(gray: &Img, block: u32, c: f64) -> Img {
    let (w, h) = (gray.width as usize, gray.height as usize);
    let r = (block / 2) as usize;
    // Horizontal window sums; a window holds at most 255 pixels of 255, so
    // they fit 16 bits, which halves the memory this needs.
    let mut hsum = vec![0u16; w * h];
    for y in 0..h {
        let row = &gray.data[y * w..(y + 1) * w];
        let mut acc: u32 = row[..(r + 1).min(w)].iter().map(|&v| v as u32).sum();
        for x in 0..w {
            hsum[y * w + x] = acc.min(u16::MAX as u32) as u16;
            if x + r + 1 < w {
                acc += row[x + r + 1] as u32;
            }
            if x >= r {
                acc -= row[x - r] as u32;
            }
        }
    }
    let count_x = |x: usize| (x + r + 1).min(w) - x.saturating_sub(r);
    let mut out = vec![0u8; w * h];
    let mut col: Vec<u64> = (0..w).map(|x| (0..(r + 1).min(h)).map(|y| hsum[y * w + x] as u64).sum()).collect();
    for y in 0..h {
        let count_y = (y + r + 1).min(h) - y.saturating_sub(r);
        for x in 0..w {
            let mean = col[x] as f64 / (count_x(x) * count_y) as f64;
            out[y * w + x] = if gray.data[y * w + x] as f64 > mean - c { 255 } else { 0 };
        }
        for (x, acc) in col.iter_mut().enumerate() {
            if y + r + 1 < h {
                *acc += hsum[(y + r + 1) * w + x] as u64;
            }
            if y >= r {
                *acc -= hsum[(y - r) * w + x] as u64;
            }
        }
    }
    Img { width: gray.width, height: gray.height, channels: 1, data: out }
}

/// Stretch each colour channel so that, after ignoring `cutoff` per cent of
/// the darkest and lightest pixels, it spans 0–255.
fn normalize(img: &Img, cutoff: f64) -> Img {
    let c = img.channels as usize;
    let colours = if c == 1 { 1 } else { 3 };
    let mut out = img.clone();
    for ch in 0..colours {
        let mut hist = [0u64; 256];
        for p in img.data.chunks_exact(c) {
            hist[p[ch] as usize] += 1;
        }
        let total: u64 = hist.iter().sum();
        let cut = (total as f64 * cutoff / 100.0) as u64;
        let (mut lo, mut acc) = (0usize, 0u64);
        while lo < 255 && acc + hist[lo] <= cut {
            acc += hist[lo];
            lo += 1;
        }
        let (mut hi, mut acc) = (255usize, 0u64);
        while hi > 0 && acc + hist[hi] <= cut {
            acc += hist[hi];
            hi -= 1;
        }
        if hi <= lo {
            continue;
        }
        let scale = 255.0 / (hi - lo) as f64;
        let lut: Vec<u8> = (0..256).map(|v| ((v as f64 - lo as f64) * scale).round().clamp(0.0, 255.0) as u8).collect();
        for p in out.data.chunks_exact_mut(c) {
            p[ch] = lut[p[ch] as usize];
        }
    }
    out
}

/// One pass of a box blur of radius `r` along rows (or columns), on every
/// channel.
fn box_pass(img: &Img, r: usize, vertical: bool) -> Img {
    let (w, h, c) = (img.width as usize, img.height as usize, img.channels as usize);
    let mut out = img.clone();
    let (lines, len) = if vertical { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize, k: usize| if vertical { (i * w + line) * c + k } else { (line * w + i) * c + k };
    for line in 0..lines {
        for k in 0..c {
            let mut acc: u32 = (0..(r + 1).min(len)).map(|i| img.data[idx(line, i, k)] as u32).sum();
            for i in 0..len {
                let n = ((i + r + 1).min(len) - i.saturating_sub(r)) as u32;
                out.data[idx(line, i, k)] = ((acc + n / 2) / n) as u8;
                if i + r + 1 < len {
                    acc += img.data[idx(line, i + r + 1, k)] as u32;
                }
                if i >= r {
                    acc -= img.data[idx(line, i - r, k)] as u32;
                }
            }
        }
    }
    out
}

/// A box blur of `radius`, or a Gaussian of that standard deviation
/// (approximated by three box blurs).
fn blur(img: &Img, radius: f64, gaussian: bool) -> Img {
    if radius < 0.5 {
        return img.clone();
    }
    let radii: Vec<usize> = if gaussian {
        // Three boxes whose combined variance matches σ² (Kovesi).
        let sigma = radius;
        let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
        let lower = (ideal.floor() as usize) | 1;
        let lower = if lower as f64 > ideal { lower.saturating_sub(2).max(1) } else { lower };
        let upper = lower + 2;
        let m = ((12.0 * sigma * sigma - 3.0 * (lower * lower) as f64 - 12.0 * lower as f64 - 9.0)
            / (-4.0 * lower as f64 - 4.0))
            .round()
            .clamp(0.0, 3.0) as usize;
        (0..3).map(|i| (if i < m { lower } else { upper }) / 2).collect()
    } else {
        vec![radius.round() as usize]
    };
    let mut out = img.clone();
    for r in radii.into_iter().filter(|r| *r > 0) {
        out = box_pass(&box_pass(&out, r, false), r, true);
    }
    out
}

/// Unsharp masking: push each pixel away from its blurred neighbourhood.
fn sharpen(img: &Img, radius: f64, amount: f64) -> Img {
    let blurred = blur(img, radius, true);
    let mut out = img.clone();
    let c = img.channels as usize;
    for (i, v) in out.data.iter_mut().enumerate() {
        if c == 4 && i % 4 == 3 {
            continue;
        }
        let orig = img.data[i] as f64;
        *v = (orig + amount * (orig - blurred.data[i] as f64)).round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn pad(img: &Img, top: u32, right: u32, bottom: u32, left: u32, color: [u8; 4]) -> Res<Img> {
    let w = img.width.checked_add(left).and_then(|v| v.checked_add(right)).ok_or_else(|| bad("padding too large"))?;
    let h = img.height.checked_add(top).and_then(|v| v.checked_add(bottom)).ok_or_else(|| bad("padding too large"))?;
    let channels = if color[3] == 255 && color[0] == color[1] && color[1] == color[2] { img.channels } else { 4 };
    let src = img.with_channels(channels);
    let mut out = Img::new(w, h, channels, color)?;
    let c = channels as usize;
    let row = img.width as usize * c;
    for y in 0..img.height as usize {
        let to = ((y + top as usize) * w as usize + left as usize) * c;
        out.data[to..to + row].copy_from_slice(&src.data[y * row..(y + 1) * row]);
    }
    Ok(out)
}

/// `src` over a copy of `base` with its top left at (`x`, `y`), clipped,
/// blended by the source's alpha.
fn paste(base: &Img, src: &Img, x: i64, y: i64) -> Img {
    let channels = if base.channels == 4 || src.channels == 4 { 4 } else { 1 };
    let mut out = base.with_channels(channels);
    let src = src.with_channels(channels);
    let c = channels as usize;
    for sy in 0..src.height as i64 {
        let ty = y + sy;
        if ty < 0 || ty >= out.height as i64 {
            continue;
        }
        for sx in 0..src.width as i64 {
            let tx = x + sx;
            if tx < 0 || tx >= out.width as i64 {
                continue;
            }
            let from = ((sy * src.width as i64 + sx) as usize) * c;
            let to = ((ty * out.width as i64 + tx) as usize) * c;
            if c == 1 {
                out.data[to] = src.data[from];
            } else {
                let a = src.data[from + 3] as u32;
                for k in 0..3 {
                    let (s, d) = (src.data[from + k] as u32, out.data[to + k] as u32);
                    out.data[to + k] = ((s * a + d * (255 - a) + 127) / 255) as u8;
                }
                let da = out.data[to + 3] as u32;
                out.data[to + 3] = (a + da * (255 - a) / 255).min(255) as u8;
            }
        }
    }
    out
}

fn draw_rect(img: &Img, x: f64, y: f64, w: f64, h: f64, color: [u8; 4], thickness: u32) -> Img {
    let grey = color[0] == color[1] && color[1] == color[2] && color[3] == 255;
    let mut out = if grey { img.clone() } else { img.to_rgba() };
    let c = out.channels as usize;
    let px = if c == 1 { vec![color[0]] } else { color.to_vec() };
    let clampx = |v: f64| v.round().clamp(0.0, out.width as f64) as u32;
    let clampy = |v: f64| v.round().clamp(0.0, out.height as f64) as u32;
    let (x0, x1, y0, y1) = (clampx(x), clampx(x + w), clampy(y), clampy(y + h));
    let t = thickness;
    let mut fill = |ax: u32, ay: u32, bx: u32, by: u32| {
        for yy in ay..by.min(out.height) {
            for xx in ax..bx.min(out.width) {
                let at = (yy as usize * out.width as usize + xx as usize) * c;
                out.data[at..at + c].copy_from_slice(&px);
            }
        }
    };
    fill(x0, y0, x1, y0.saturating_add(t));
    fill(x0, y1.saturating_sub(t), x1, y1);
    fill(x0, y0, x0.saturating_add(t), y1);
    fill(x1.saturating_sub(t), y0, x1, y1);
    out
}

/// The colour most of the border has: what a rotation should fill with.
fn background_of(img: &Img) -> [u8; 4] {
    let gray = |x: u32, y: u32| {
        let p = img.px(x, y);
        if img.channels == 1 {
            p[0]
        } else {
            luma([p[0], p[1], p[2], p[3]])
        }
    };
    let mut hist = [0u64; 256];
    for x in 0..img.width {
        hist[gray(x, 0) as usize] += 1;
        hist[gray(x, img.height - 1) as usize] += 1;
    }
    for y in 0..img.height {
        hist[gray(0, y) as usize] += 1;
        hist[gray(img.width - 1, y) as usize] += 1;
    }
    let mean = |range: std::ops::Range<usize>| {
        let n: u64 = hist[range.clone()].iter().sum();
        (n, range.map(|i| i as u64 * hist[i]).sum::<u64>().checked_div(n).unwrap_or(0))
    };
    let (dark, dark_mean) = mean(0..128);
    let (light, light_mean) = mean(128..256);
    let g = if dark > light { dark_mean } else { light_mean } as u8;
    [g, g, g, 255]
}

/// The skew of the text in degrees (clockwise is positive), within
/// ±`max`: the angle whose horizontal projection of the ink is the most
/// sharply peaked, since level lines of text make tall narrow peaks.
pub fn estimate_skew(img: &Img, max: f64) -> f64 {
    // Work on a copy small enough to keep this quick.
    let longest = img.width.max(img.height) as f64;
    let k = (1000.0 / longest).min(1.0);
    let small = if k < 1.0 {
        let (w, h) = (((img.width as f64 * k).round() as u32).max(1), ((img.height as f64 * k).round() as u32).max(1));
        resize(img, w, h, Filter::Bilinear).unwrap_or_else(|_| img.clone())
    } else {
        img.clone()
    };
    let gray = small.to_gray();
    let level = otsu(&histogram(&gray.data));
    let dark = gray.data.iter().filter(|&&v| v <= level).count();
    // The ink is whichever side of the threshold is the minority.
    let ink_is_dark = dark * 2 <= gray.data.len();
    let w = gray.width as usize;
    let mut ink: Vec<(f64, f64)> = Vec::new();
    for (i, &v) in gray.data.iter().enumerate() {
        if (v <= level) == ink_is_dark {
            ink.push(((i % w) as f64, (i / w) as f64));
        }
    }
    if ink.len() < 16 {
        return 0.0;
    }
    let step = (ink.len() / 200_000).max(1);
    let ink: Vec<(f64, f64)> = ink.into_iter().step_by(step).collect();
    let diag = ((gray.width as f64).powi(2) + (gray.height as f64).powi(2)).sqrt();
    let bins = (2.0 * diag) as usize + 4;
    let score = |deg: f64| -> f64 {
        let (s, c) = deg.to_radians().sin_cos();
        let mut hist = vec![0u32; bins];
        for &(x, y) in &ink {
            let row = (y * c - x * s + diag + 1.0) as usize;
            if let Some(b) = hist.get_mut(row) {
                *b += 1;
            }
        }
        hist.iter().map(|&n| (n as f64).powi(2)).sum()
    };
    // On a tie (small turns of short lines can leave every bin unchanged)
    // the angle nearest level wins: never turn a picture for nothing.
    let search = |from: f64, to: f64, step: f64| {
        let mut best: (f64, f64) = (0.0, f64::MIN);
        let n = ((to - from) / step).round() as i64;
        for i in 0..=n {
            let a = from + i as f64 * step;
            let s = score(a);
            let tie = (s - best.1).abs() <= best.1.abs() * 1e-9;
            if (s > best.1 && !tie) || (tie && a.abs() < best.0.abs()) {
                best = (a, s);
            }
        }
        best.0
    };
    let coarse = search(-max, max, 0.5);
    search(coarse - 0.5, coarse + 0.5, 0.05)
}

/// Tiles of at most `tw`×`th` covering a `w`×`h` image, overlapping by
/// `overlap`, in rows from the top left.
fn tile_rects(w: u32, h: u32, tw: u32, th: u32, overlap: u32) -> Vec<(u32, u32, u32, u32)> {
    let starts = |len: u32, tile: u32| -> Vec<u32> {
        if len <= tile {
            return vec![0];
        }
        let stride = tile - overlap;
        let mut v: Vec<u32> = (0..).map(|i| i * stride).take_while(|&s| s + tile < len).collect();
        v.push(len - tile);
        v.dedup();
        v
    };
    let mut out = Vec::new();
    for y in starts(h, th) {
        for x in starts(w, tw) {
            out.push((x, y, tw.min(w - x), th.min(h - y)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(img: &Img) -> String {
        let (bytes, _) = encode(img, "png", 90).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn gradient(w: u32, h: u32) -> Img {
        let data = (0..w * h).map(|i| ((i % w) * 255 / w.max(1)) as u8).collect();
        Img { width: w, height: h, channels: 1, data }
    }

    /// Black bars of "text" on white, every 20 rows, skewed by `deg`.
    fn lines(w: u32, h: u32, deg: f64) -> Img {
        let t = deg.to_radians().tan();
        let mut img = Img::new(w, h, 1, [255; 4]).unwrap();
        for y in 0..h {
            for x in 0..w {
                let yy = y as f64 - x as f64 * t;
                if yy.rem_euclid(20.0) < 6.0 && x > 10 && x < w - 10 {
                    img.data[(y * w + x) as usize] = 0;
                }
            }
        }
        img
    }

    fn call(store: &mut ImageStore, op: &str, p: Value) -> Res<ImageInfo> {
        store.call(op, &p)
    }

    #[test]
    fn handles_flow_through_operations_with_composed_transforms() {
        let mut s = ImageStore::new();
        let input = s.insert_encoded(encode(&gradient(200, 100), "png", 90).unwrap().0);
        let info = call(&mut s, "info", json!({ "handle": input })).unwrap();
        assert_eq!((info.width, info.height, info.channels), (200, 100, 1));
        assert_eq!(info.format.as_deref(), Some("png"));
        assert!(info.transform.is_identity());

        let crop =
            call(&mut s, "crop", json!({ "handle": input, "x": 50, "y": 10, "width": 100, "height": 50 })).unwrap();
        assert_eq!((crop.width, crop.height), (100, 50));
        let big = call(&mut s, "resize", json!({ "handle": crop.handle, "scale": 2 })).unwrap();
        assert_eq!((big.width, big.height), (200, 100));
        let padded = call(&mut s, "pad", json!({ "handle": big.handle, "all": 10 })).unwrap();
        assert_eq!((padded.width, padded.height), (220, 120));
        // A box found at (10, 10)-(30, 30) in the padded copy is (50, 10)-(60, 20) in the input.
        let r = padded.transform.map_rect(glossa_module_sdk::wire::Rect::new(10.0, 10.0, 20.0, 20.0));
        assert_eq!((r.x, r.y, r.width, r.height), (50.0, 10.0, 10.0, 10.0));
    }

    #[test]
    fn right_angle_rotations_and_flips_map_back() {
        let mut img = Img::new(3, 2, 1, [0; 4]).unwrap();
        img.data = vec![1, 2, 3, 4, 5, 6];
        let (r90, t) = rotate(&img, 90.0, [255; 4], true).unwrap();
        assert_eq!((r90.width, r90.height), (2, 3));
        assert_eq!(r90.data, vec![4, 1, 5, 2, 6, 3]);
        // The new top-left pixel's centre (0.5, 0.5) was old (0.5, 1.5): bottom left.
        assert_eq!(t.map_point(0.5, 0.5), (0.5, 1.5));
        let (r270, t) = rotate(&img, -90.0, [255; 4], true).unwrap();
        assert_eq!(r270.data, vec![3, 6, 2, 5, 1, 4]);
        assert_eq!(t.map_point(0.5, 0.5), (2.5, 0.5));
        let (r180, t) = rotate(&img, 180.0, [255; 4], true).unwrap();
        assert_eq!(r180.data, vec![6, 5, 4, 3, 2, 1]);
        assert_eq!(t.map_point(0.5, 0.5), (2.5, 1.5));
        let mut f = img.clone();
        flip(&mut f, false);
        assert_eq!(f.data, vec![3, 2, 1, 6, 5, 4]);
        let mut f = img.clone();
        flip(&mut f, true);
        assert_eq!(f.data, vec![4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn arbitrary_rotation_expands_and_maps_the_centre_to_the_centre() {
        let img = gradient(100, 50);
        let (out, t) = rotate(&img, 30.0, [255; 4], true).unwrap();
        assert!(out.width >= 111 && out.height >= 93, "{}×{}", out.width, out.height);
        let (cx, cy) = t.map_point(out.width as f64 / 2.0, out.height as f64 / 2.0);
        assert!((cx - 50.0).abs() < 1e-6 && (cy - 25.0).abs() < 1e-6);
        assert!((t.rotation().to_degrees() + 30.0).abs() < 1e-6 || (t.rotation().to_degrees() - 30.0).abs() < 1e-6);
        let (same, _) = rotate(&img, 30.0, [255; 4], false).unwrap();
        assert_eq!((same.width, same.height), (100, 50));
    }

    #[test]
    fn otsu_and_thresholds_separate_ink_from_paper() {
        let mut s = ImageStore::new();
        let doc = lines(120, 80, 0.0);
        let h = s.insert(doc.clone(), Transform::IDENTITY, None).unwrap();
        let t = call(&mut s, "threshold", json!({ "handle": h })).unwrap();
        assert!(t.threshold.is_some());
        let fixed = call(&mut s, "threshold", json!({ "handle": h, "value": 10 })).unwrap();
        assert_eq!(fixed.threshold, Some(10));
        let a = call(&mut s, "adaptive_threshold", json!({ "handle": h, "block_size": 15, "c": 5 })).unwrap();
        assert_eq!((a.width, a.height, a.channels), (120, 80, 1));
        let mut bimodal = [0u64; 256];
        bimodal[20] = 100;
        bimodal[220] = 100;
        let level = otsu(&bimodal);
        assert!((20..220).contains(&level), "{level}");
    }

    #[test]
    fn deskew_finds_the_skew_and_levels_it() {
        let skewed = lines(400, 200, 4.0);
        let skew = estimate_skew(&skewed, 10.0);
        assert!((skew - 4.0).abs() < 0.3, "estimated {skew}");
        assert!(estimate_skew(&lines(400, 200, 0.0), 10.0).abs() < 0.2);
        let mut s = ImageStore::new();
        let h = s.insert(skewed, Transform::IDENTITY, None).unwrap();
        let d = call(&mut s, "deskew", json!({ "handle": h })).unwrap();
        let applied = d.angle.unwrap();
        assert!((applied + 4.0).abs() < 0.3, "applied {applied}");
        let blank = s.insert(Img::new(50, 50, 1, [255; 4]).unwrap(), Transform::IDENTITY, None).unwrap();
        assert_eq!(call(&mut s, "deskew", json!({ "handle": blank })).unwrap().angle, Some(0.0));
    }

    #[test]
    fn filters_keep_sizes_and_stay_in_range() {
        let mut s = ImageStore::new();
        let rgba = Img { width: 7, height: 5, channels: 4, data: (0..140).map(|i| (i * 7 % 256) as u8).collect() };
        let h = s.insert(rgba, Transform::IDENTITY, None).unwrap();
        for (op, p) in [
            ("grayscale", json!({})),
            ("invert", json!({})),
            ("adjust", json!({ "contrast": 1.5, "brightness": 10, "gamma": 0.8 })),
            ("normalize", json!({ "cutoff": 1 })),
            ("blur", json!({ "radius": 2 })),
            ("blur", json!({ "radius": 1, "kind": "box" })),
            ("sharpen", json!({ "amount": 2 })),
            ("flip", json!({ "direction": "vertical" })),
            ("draw_rect", json!({ "x": 1, "y": 1, "width": 4, "height": 3, "color": "#00ff00" })),
            ("rotate", json!({ "angle": 12.5, "background": "transparent" })),
            ("scale_to_min_height", json!({ "min_height": 32 })),
            ("resize", json!({ "width": 20, "height": 20, "mode": "fill", "filter": "lanczos" })),
            ("resize", json!({ "max_side": 3, "filter": "nearest" })),
        ] {
            let mut p = p;
            p["handle"] = h.into();
            let out = call(&mut s, op, p).unwrap_or_else(|e| panic!("{op}: {e:?}"));
            assert!(out.width > 0 && out.height > 0, "{op}");
        }
        let up = call(&mut s, "scale_to_min_height", json!({ "handle": h, "min_height": 40 })).unwrap();
        assert_eq!(up.height, 40);
        assert_eq!(up.width, 56);
        assert!((up.transform.scale() - 0.125).abs() < 1e-9);
    }

    #[test]
    fn encode_decode_round_trip_in_every_format() {
        let mut s = ImageStore::new();
        let h = s.insert(gradient(16, 8).to_rgba(), Transform::IDENTITY, None).unwrap();
        for (format, mime) in [("png", "image/png"), ("jpeg", "image/jpeg"), ("webp", "image/webp")] {
            let e = call(&mut s, "encode", json!({ "handle": h, "format": format, "quality": 80 })).unwrap();
            assert_eq!(e.mime.as_deref(), Some(mime));
            let back = call(&mut s, "decode", json!({ "data_base64": e.data_base64.unwrap() })).unwrap();
            assert_eq!((back.width, back.height), (16, 8), "{format}");
        }
        let gray = s.insert(gradient(5, 5), Transform::IDENTITY, None).unwrap();
        let px = call(&mut s, "pixels", json!({ "handle": gray })).unwrap();
        assert_eq!(px.bytes().unwrap().len(), 25);
    }

    #[test]
    fn tiles_cover_the_image_and_paste_composites() {
        assert_eq!(tile_rects(10, 10, 4, 10, 0), vec![(0, 0, 4, 10), (4, 0, 4, 10), (6, 0, 4, 10)]);
        assert_eq!(tile_rects(10, 10, 20, 20, 0), vec![(0, 0, 10, 10)]);
        let mut s = ImageStore::new();
        let h = s.insert(gradient(100, 30), Transform::IDENTITY, None).unwrap();
        let t = call(&mut s, "split_tiles", json!({ "handle": h, "max_width": 40, "overlap": 5 })).unwrap();
        assert_eq!(t.tiles.len(), 3);
        assert_eq!(t.tiles[1].transform.map_point(0.0, 0.0), (35.0, 0.0));
        let canvas = call(&mut s, "new", json!({ "width": 50, "height": 50, "color": "white" })).unwrap();
        assert_eq!(canvas.channels, 1);
        let pasted =
            call(&mut s, "paste", json!({ "handle": canvas.handle, "source": t.tiles[0].handle, "x": -10, "y": 40 }))
                .unwrap();
        assert_eq!((pasted.width, pasted.height), (50, 50));
    }

    #[test]
    fn odd_sizes_and_bad_input_are_errors_not_panics() {
        let mut s = ImageStore::new();
        let one = s.insert(Img::new(1, 1, 4, [10, 20, 30, 40]).unwrap(), Transform::IDENTITY, None).unwrap();
        for op in [
            "grayscale",
            "invert",
            "threshold",
            "adaptive_threshold",
            "adjust",
            "normalize",
            "blur",
            "sharpen",
            "deskew",
            "scale_to_min_height",
            "flip",
            "rotate",
            "pad",
            "encode",
            "pixels",
            "info",
            "split_tiles",
        ] {
            let _ = call(&mut s, op, json!({ "handle": one, "angle": 33, "radius": 50, "all": 3 }));
        }
        assert!(call(&mut s, "crop", json!({ "handle": one, "x": 5, "y": 5, "width": 1, "height": 1 })).is_err());
        assert!(call(&mut s, "crop", json!({ "handle": one, "width": 0 })).is_err());
        assert!(call(&mut s, "new", json!({ "width": 0, "height": 5 })).is_err());
        assert!(call(&mut s, "new", json!({ "width": 20000, "height": 20000 })).is_err(), "over the pixel limit");
        assert!(call(&mut s, "resize", json!({ "handle": one, "width": 1e12 })).is_err());
        assert!(call(&mut s, "resize", json!({ "handle": one, "scale": -1 })).is_err());
        assert!(call(&mut s, "resize", json!({ "handle": one, "width": "x" })).is_err());
        assert!(call(&mut s, "pad", json!({ "handle": one, "left": 4_000_000_000u64 })).is_err());
        let _ = call(&mut s, "rotate", json!({ "handle": one, "angle": 1e300 }));
        assert!(call(&mut s, "info", json!({ "handle": 999 })).is_err());
        assert!(call(&mut s, "info", json!({ "handle": 0 })).is_err());
        assert!(call(&mut s, "info", json!({})).is_err());
        assert!(call(&mut s, "decode", json!({ "data_base64": "bm90IGFuIGltYWdl" })).is_err());
        assert!(call(&mut s, "decode", json!({ "data_base64": "!!!" })).is_err());
        assert!(call(&mut s, "encode", json!({ "handle": one, "format": "gif" })).is_err());
        assert!(call(&mut s, "frobnicate", json!({ "handle": one })).is_err());
        assert!(call(&mut s, "pad", json!({ "handle": one, "color": "#12" })).is_err());
        let bad_png = s.insert_encoded(vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
        assert!(call(&mut s, "info", json!({ "handle": bad_png })).is_err());
        // A PNG header claiming a huge picture is refused before allocating.
        let mut huge = encode(&Img::new(1, 1, 1, [0; 4]).unwrap(), "png", 90).unwrap().0;
        huge[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(decode(&huge).is_err());
        let _ = png(&gradient(2, 2));
    }

    #[test]
    fn freeing_returns_memory_and_handles_are_reused() {
        let mut s = ImageStore::new();
        let a = s.insert(gradient(10, 10), Transform::IDENTITY, None).unwrap();
        assert_eq!(s.used, 100);
        call(&mut s, "free", json!({ "handle": a })).unwrap();
        assert_eq!(s.used, 0);
        assert!(call(&mut s, "info", json!({ "handle": a })).is_err());
        let b = s.insert(gradient(10, 10), Transform::IDENTITY, None).unwrap();
        assert_eq!(b, a, "the free slot is reused");
    }
}
