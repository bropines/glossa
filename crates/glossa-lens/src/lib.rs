//! A Google Lens client: the request Chromium's Lens overlay sends, the
//! picture encoded the way Chromium encodes it, and the reply as text with
//! its layout and, when asked for, its translation.
//!
//! The wire types are Chromium's own `lens_server_proto`, generated from
//! descriptors pinned to one Chromium commit ([`CHROMIUM_COMMIT`]; see
//! `proto/README.md`). The request mirrors chrome-lens-py's
//! `create_ocr_translate_request`, which mirrors
//! `LensOverlayQueryController`: exactly one filter (`AUTO_FILTER`, or
//! `TRANSLATE` in its place), the request time, the Lens overlay rendering
//! environment. Sending it is the caller's: [`objects_request`] gives the
//! body, [`headers`] the headers, [`ENDPOINT`] the address.

/// Chromium's `lens_server_proto`, generated (see `build.rs`).
#[allow(clippy::all, clippy::pedantic, missing_docs, rustdoc::all)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/lens.rs"));
}

use std::io::Cursor;

use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use prost::Message;

/// Where Chromium's Lens overlay uploads to.
pub const ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";

/// The public key the Chromium-derived Lens clients send (owocr,
/// chrome-lens-py): a browser's key, not a secret of anyone's.
pub const PUBLIC_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// The Chromium commit the protos come from.
pub fn chromium_commit() -> &'static str {
    include_str!("../proto/CHROMIUM_COMMIT").trim()
}

// -- the picture ------------------------------------------------------------------

/// Chromium's limits (`components/lens/lens_features.cc`): an image is made
/// smaller only when it is both larger than this many pixels and wider or
/// taller than the sides below.
const MAX_AREA: u64 = 1_500_000;
const MAX_SIDE: u32 = 1600;
/// `kLensOverlayImageCompressionQuality`.
const JPEG_QUALITY: u8 = 40;

/// The picture as it is uploaded.
#[derive(Debug, Clone)]
pub struct Upload {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The size an image of `width` × `height` is sent at (Chromium's
/// `ShouldDownscaleSize` and its preferred size).
pub fn upload_size(width: u32, height: u32) -> (u32, u32) {
    let large = u64::from(width) * u64::from(height) > MAX_AREA;
    if !(large && (width > MAX_SIDE || height > MAX_SIDE)) {
        return (width, height);
    }
    let scale = (MAX_SIDE as f64 / width as f64).min(MAX_SIDE as f64 / height as f64);
    (((width as f64 * scale) as u32).max(1), ((height as f64 * scale) as u32).max(1))
}

/// Encode `img` for upload: made smaller by Chromium's rule, then JPEG at
/// quality 40. A picture that really uses transparency is laid on white or
/// black, whichever its content stands out against: the server composites
/// transparency onto black, which would lose dark text (chrome-lens-py's
/// "contrast" choice, measured against the live service).
pub fn encode_image(img: &DynamicImage) -> Result<Upload, String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    let (tw, th) = upload_size(w, h);
    let img =
        if (tw, th) == (w, h) { img.clone() } else { img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3) };
    let rgb = flatten(&img);
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), JPEG_QUALITY);
    rgb.write_with_encoder(encoder).map_err(|e| format!("encoding the picture: {e}"))?;
    Ok(Upload { jpeg, width: tw, height: th })
}

/// The picture without its alpha channel: as it is when fully opaque,
/// otherwise on the background its content contrasts with.
fn flatten(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    if rgba.pixels().all(|p| p.0[3] == 255) {
        return img.to_rgb8();
    }
    // The mean brightness of the pixels that are mostly opaque.
    let (mut sum, mut count) = (0.0f64, 0u64);
    for p in rgba.pixels().filter(|p| p.0[3] > 128) {
        let [r, g, b, _] = p.0;
        sum += 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
        count += 1;
    }
    let back = if count == 0 || sum / (count as f64) < 128.0 { 255.0 } else { 0.0 };
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let k = a as f64 / 255.0;
        let mix = |c: u8| (c as f64 * k + back * (1.0 - k)).round() as u8;
        Rgb([mix(r), mix(g), mix(b)])
    })
}

/// Decode any picture Glossa may hand over (PNG, JPEG, WebP, BMP…).
pub fn decode_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    let format = image::guess_format(bytes).unwrap_or(ImageFormat::Png);
    image::load_from_memory_with_format(bytes, format).map_err(|e| format!("reading the picture: {e}"))
}

// -- the request ------------------------------------------------------------------

/// What to ask Lens for.
#[derive(Debug, Clone)]
pub struct Query {
    /// The language of the text, a BCP 47 code, or empty to let Lens tell.
    pub ocr_language: String,
    /// Translate into this language (Google Translate's code) as well.
    pub translate_to: Option<String>,
    /// The source language of the translation, when known.
    pub translate_from: Option<String>,
    pub region: String,
    pub time_zone: String,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            ocr_language: String::new(),
            translate_to: None,
            translate_from: None,
            region: "US".into(),
            time_zone: "America/New_York".into(),
        }
    }
}

/// A random request id (Chromium's `uuid`): any 63-bit value.
pub fn random_uuid() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish() >> 1
}

fn now_usec() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

/// The body of the objects request for `upload`: recognise the whole
/// picture (and translate it, with `query.translate_to`).
pub fn objects_request(upload: &Upload, query: &Query, uuid: u64) -> Vec<u8> {
    use proto::*;
    // Chromium always sends exactly one filter, and TRANSLATE takes the place
    // of AUTO_FILTER rather than joining it. Without TRANSLATE the reply
    // carries no translation at all.
    let filter = match query.translate_to.as_deref().filter(|t| !t.is_empty()) {
        Some(to) => AppliedFilter {
            filter_type: Some(LensOverlayFilterType::Translate as i32),
            filter_payload: Some(applied_filter::FilterPayload::Translate(applied_filter::Translate {
                target_language: Some(to.to_owned()),
                source_language: query.translate_from.clone().filter(|s| !s.is_empty()),
            })),
        },
        None => AppliedFilter { filter_type: Some(LensOverlayFilterType::AutoFilter as i32), filter_payload: None },
    };
    let request = LensOverlayServerRequest {
        objects_request: Some(LensOverlayObjectsRequest {
            request_context: Some(LensOverlayRequestContext {
                request_id: Some(LensOverlayRequestId {
                    uuid: Some(uuid),
                    sequence_id: Some(1),
                    image_sequence_id: Some(1),
                    time_usec: Some(now_usec()),
                    ..Default::default()
                }),
                client_context: Some(LensOverlayClientContext {
                    platform: Some(Platform::Web as i32),
                    surface: Some(Surface::Chromium as i32),
                    locale_context: Some(LocaleContext {
                        language: Some(query.ocr_language.clone()),
                        region: Some(query.region.clone()),
                        time_zone: Some(query.time_zone.clone()),
                    }),
                    client_filters: Some(AppliedFilters { filter: vec![filter] }),
                    // Chromium pairs SURFACE_CHROMIUM / PLATFORM_WEB with this
                    // (LensOverlayQueryController::CreateClientContext).
                    rendering_context: Some(RenderingContext {
                        rendering_environment: Some(LensRenderingEnvironment::RenderingEnvLensOverlay as i32),
                    }),
                    ..Default::default()
                }),
            }),
            image_data: Some(ImageData {
                payload: Some(ImagePayload { image_bytes: Some(upload.jpeg.clone()) }),
                image_metadata: Some(ImageMetadata {
                    width: Some(upload.width as i32),
                    height: Some(upload.height as i32),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    request.encode_to_vec()
}

/// The request's headers.
pub fn headers(api_key: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Content-Type", "application/x-protobuf".into()),
        ("User-Agent", USER_AGENT.into()),
        ("Accept", "*/*".into()),
        ("Accept-Encoding", "gzip".into()),
        ("X-Goog-Api-Key", api_key.into()),
    ]
}

// -- the reply --------------------------------------------------------------------

/// A box, as Lens gives it: centre and size, normalised to 0..1 of the
/// uploaded picture unless `normalized` is false (then pixels), turned by
/// `rotation` radians about its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub center_x: f32,
    pub center_y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub normalized: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    /// What follows the word: a space, nothing, a line break.
    pub separator: String,
    pub bounds: Option<Bounds>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
    pub bounds: Option<Bounds>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| format!("{}{}", w.text, w.separator)).collect::<String>().trim().to_owned()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub lines: Vec<Line>,
    pub bounds: Option<Bounds>,
    pub language: Option<String>,
}

impl Paragraph {
    pub fn text(&self) -> String {
        self.lines.iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }
}

/// One paragraph's translation.
#[derive(Debug, Clone, PartialEq)]
pub struct Translated {
    /// The paragraph it translates: Lens sends one deep gleam per paragraph,
    /// in order.
    pub paragraph: usize,
    pub text: String,
    pub source_language: Option<String>,
    pub target_language: Option<String>,
    /// Each line's part of `text`, as UTF-16 code unit ranges.
    pub lines: Vec<(usize, usize)>,
}

impl Translated {
    /// Line `i`'s part of the translation.
    pub fn line(&self, i: usize) -> Option<String> {
        let (start, end) = *self.lines.get(i)?;
        let units: Vec<u16> = self.text.encode_utf16().collect();
        let (start, end) = (start.min(units.len()), end.min(units.len()));
        Some(String::from_utf16_lossy(&units[start..end.max(start)]).trim().to_owned())
    }
}

/// The text Lens found, with its layout and translations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    /// The language Lens took the text to be in.
    pub language: Option<String>,
    pub paragraphs: Vec<Paragraph>,
    pub translations: Vec<Translated>,
}

impl Page {
    pub fn text(&self) -> String {
        self.paragraphs.iter().map(Paragraph::text).collect::<Vec<_>>().join("\n")
    }

    /// All the translations, one paragraph per line.
    pub fn translation(&self) -> Option<String> {
        let t = self.translations.iter().map(|t| t.text.trim()).collect::<Vec<_>>().join("\n");
        (!t.trim().is_empty()).then_some(t)
    }
}

fn bounds(g: Option<&proto::Geometry>) -> Option<Bounds> {
    let b = g?.bounding_box?;
    Some(Bounds {
        center_x: b.center_x?,
        center_y: b.center_y?,
        width: b.width.unwrap_or(0.0),
        height: b.height.unwrap_or(0.0),
        rotation: b.rotation_z.unwrap_or(0.0),
        normalized: b.coordinate_type != Some(proto::CoordinateType::Image as i32),
    })
}

fn non_empty(s: Option<&String>) -> Option<String> {
    s.map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Read a reply. A server error in the reply is an error here.
pub fn parse_response(body: &[u8]) -> Result<Page, String> {
    let reply =
        proto::LensOverlayServerResponse::decode(body).map_err(|e| format!("the reply is not a Lens response: {e}"))?;
    if let Some(e) = reply.error.as_ref().filter(|e| e.error_type != 0) {
        let kind = proto::lens_overlay_server_error::ErrorType::try_from(e.error_type)
            .map_or_else(|_| e.error_type.to_string(), |t| t.as_str_name().to_owned());
        return Err(format!("Lens answered with an error ({kind})"));
    }
    let Some(objects) = reply.objects_response else { return Ok(Page::default()) };
    let text = objects.text.unwrap_or_default();
    let paragraphs = text
        .text_layout
        .map(|l| l.paragraphs)
        .unwrap_or_default()
        .into_iter()
        .map(|p| Paragraph {
            bounds: bounds(p.geometry.as_ref()),
            language: non_empty(p.content_language.as_ref()),
            lines: p
                .lines
                .into_iter()
                .map(|l| Line {
                    bounds: bounds(l.geometry.as_ref()),
                    words: l
                        .words
                        .into_iter()
                        .map(|w| Word {
                            bounds: bounds(w.geometry.as_ref()),
                            text: w.plain_text.unwrap_or_default(),
                            separator: w.text_separator.unwrap_or_default(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    let success = proto::translation_data::status::Code::Success as i32;
    let translations = objects
        .deep_gleams
        .into_iter()
        .enumerate()
        .filter_map(|(i, gleam)| match gleam.rendering_oneof {
            Some(proto::deep_gleam_data::RenderingOneof::Translation(t))
                if t.status.and_then(|s| s.code) == Some(success) =>
            {
                Some(Translated {
                    paragraph: i,
                    text: t.translation.unwrap_or_default(),
                    source_language: non_empty(t.source_language.as_ref()),
                    target_language: non_empty(t.target_language.as_ref()),
                    lines: t
                        .line
                        .iter()
                        .map(|l| (l.start.unwrap_or(0).max(0) as usize, l.end.unwrap_or(0).max(0) as usize))
                        .collect(),
                })
            }
            _ => None,
        })
        .filter(|t| !t.text.trim().is_empty())
        .collect();
    Ok(Page { language: non_empty(text.content_language.as_ref()), paragraphs, translations })
}

#[cfg(test)]
mod tests {
    use super::proto::*;
    use super::*;
    use super::Query;

    fn upload() -> Upload {
        Upload { jpeg: vec![0xff, 0xd8, 0xff], width: 800, height: 600 }
    }

    #[test]
    fn the_descriptors_are_pinned() {
        assert_eq!(chromium_commit().len(), 40);
    }

    #[test]
    fn plain_recognition_sends_exactly_the_auto_filter() {
        let body = objects_request(&upload(), &Query::default(), 42);
        let req = LensOverlayServerRequest::decode(body.as_slice()).unwrap();
        let ctx = req.objects_request.unwrap().request_context.unwrap();
        let id = ctx.request_id.unwrap();
        assert_eq!((id.uuid, id.sequence_id, id.image_sequence_id), (Some(42), Some(1), Some(1)));
        assert!(id.time_usec.unwrap() > 1_600_000_000_000_000);
        let client = ctx.client_context.unwrap();
        assert_eq!(client.platform, Some(Platform::Web as i32));
        assert_eq!(client.surface, Some(Surface::Chromium as i32));
        assert_eq!(
            client.rendering_context.unwrap().rendering_environment,
            Some(LensRenderingEnvironment::RenderingEnvLensOverlay as i32)
        );
        let filters = client.client_filters.unwrap().filter;
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].filter_type, Some(LensOverlayFilterType::AutoFilter as i32));
        assert!(client.app_id.is_none(), "Chromium never sets it");
    }

    #[test]
    fn translation_replaces_the_auto_filter() {
        let q = Query { translate_to: Some("ru".into()), translate_from: Some("en".into()), ..Query::default() };
        let req = LensOverlayServerRequest::decode(objects_request(&upload(), &q, 1).as_slice()).unwrap();
        let o = req.objects_request.unwrap();
        let filters = o.request_context.unwrap().client_context.unwrap().client_filters.unwrap().filter;
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].filter_type, Some(LensOverlayFilterType::Translate as i32));
        let Some(applied_filter::FilterPayload::Translate(t)) = &filters[0].filter_payload else { panic!() };
        assert_eq!((t.target_language.as_deref(), t.source_language.as_deref()), (Some("ru"), Some("en")));
        let meta = o.image_data.unwrap().image_metadata.unwrap();
        assert_eq!((meta.width, meta.height), (Some(800), Some(600)));
    }

    #[test]
    fn only_large_and_oversized_pictures_are_made_smaller() {
        // Chromium keeps a 1600×900 screenshot as it is.
        assert_eq!(upload_size(1600, 900), (1600, 900));
        assert_eq!(upload_size(1000, 1000), (1000, 1000));
        assert_eq!(upload_size(2560, 1440), (1600, 900));
        assert_eq!(upload_size(1200, 3000), (640, 1600));
    }

    #[test]
    fn pictures_go_up_as_jpeg_and_transparency_takes_a_contrasting_background() {
        let mut img = image::RgbaImage::from_pixel(40, 20, image::Rgba([0, 0, 0, 0]));
        for x in 5..35 {
            img.put_pixel(x, 10, image::Rgba([250, 250, 250, 255])); // white text
        }
        let up = encode_image(&DynamicImage::ImageRgba8(img.clone())).unwrap();
        assert_eq!(&up.jpeg[..2], &[0xff, 0xd8]);
        // White content: laid on black, so it still shows.
        assert!(flatten(&DynamicImage::ImageRgba8(img)).get_pixel(0, 0).0[0] < 10);
    }

    fn word(text: &str, sep: &str) -> text_layout::Word {
        text_layout::Word { plain_text: Some(text.into()), text_separator: Some(sep.into()), ..Default::default() }
    }

    fn geometry(cx: f32, cy: f32) -> Option<Geometry> {
        Some(Geometry {
            bounding_box: Some(CenterRotatedBox {
                center_x: Some(cx),
                center_y: Some(cy),
                width: Some(0.4),
                height: Some(0.1),
                rotation_z: Some(0.0),
                coordinate_type: Some(CoordinateType::Normalized as i32),
            }),
            ..Default::default()
        })
    }

    #[test]
    fn a_reply_becomes_paragraphs_lines_words_and_translations() {
        let paragraph = text_layout::Paragraph {
            lines: vec![
                text_layout::Line { words: vec![word("Hello,", " "), word("world", "")], geometry: geometry(0.5, 0.2) },
                text_layout::Line { words: vec![word("again", "")], geometry: geometry(0.5, 0.4) },
            ],
            geometry: geometry(0.5, 0.3),
            ..Default::default()
        };
        let translation = TranslationData {
            status: Some(translation_data::Status { code: Some(translation_data::status::Code::Success as i32) }),
            translation: Some("Привет, мир снова".into()),
            target_language: Some("ru".into()),
            line: vec![
                translation_data::Line { start: Some(0), end: Some(11), ..Default::default() },
                translation_data::Line { start: Some(11), end: Some(17), ..Default::default() },
            ],
            ..Default::default()
        };
        let reply = LensOverlayServerResponse {
            objects_response: Some(LensOverlayObjectsResponse {
                text: Some(Text {
                    text_layout: Some(TextLayout { paragraphs: vec![paragraph] }),
                    content_language: Some("en".into()),
                }),
                deep_gleams: vec![DeepGleamData {
                    rendering_oneof: Some(deep_gleam_data::RenderingOneof::Translation(translation)),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let page = parse_response(&reply.encode_to_vec()).unwrap();
        assert_eq!(page.language.as_deref(), Some("en"));
        assert_eq!(page.text(), "Hello, world\nagain");
        let b = page.paragraphs[0].lines[0].bounds.unwrap();
        assert!(b.normalized && (b.center_y - 0.2).abs() < 1e-6);
        let t = &page.translations[0];
        assert_eq!((t.paragraph, t.line(0).as_deref(), t.line(1).as_deref()), (0, Some("Привет, мир"), Some("снова")));
        assert_eq!(page.translation().as_deref(), Some("Привет, мир снова"));
    }

    #[test]
    fn a_server_error_is_an_error() {
        let reply = LensOverlayServerResponse {
            error: Some(LensOverlayServerError { error_type: 1, ..Default::default() }),
            ..Default::default()
        };
        let e = parse_response(&reply.encode_to_vec()).unwrap_err();
        assert!(e.contains("MISSING_REQUEST"), "{e}");
    }
}
