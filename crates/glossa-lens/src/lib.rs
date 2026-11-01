//! Google Lens for Glossa, on top of chrome-lens-rs
//! (https://github.com/bropines/chrome-lens-rs, `chrome_lens_ocr`): Chromium's
//! own `lens_server_proto` (vendored there, pinned to one Chromium commit,
//! re-pinned with its `update-protos`), the picture encoded the way Chromium
//! encodes it, and the request Chromium's Lens overlay sends (exactly one
//! filter, `AUTO_FILTER` or `TRANSLATE` in its place; the request time; the
//! Lens overlay rendering environment).
//!
//! Glossa sends the request through its own HTTP client (the user's proxy
//! settings, cancellation, a test double in tests), so this crate only
//! builds it: [`objects_request`] gives the body, [`headers`] the headers,
//! [`ENDPOINT`] the address. [`parse_response`] reads the reply into
//! paragraphs, lines and words with every box normalised, each paragraph's
//! translation, and each line's part of it, which the in-place overlay
//! places line by line.

pub use chrome_lens_ocr::{proto, GeometryData, Line, Page, Paragraph, Translation, Word};

use chrome_lens_ocr::image_processor::{self, AlphaMode, ProcessedImage};
use chrome_lens_ocr::ScanOptions;
use image::{DynamicImage, GenericImageView, ImageFormat};

/// Where Chromium's Lens overlay uploads to.
pub const ENDPOINT: &str = chrome_lens_ocr::LENS_CRUPLOAD_ENDPOINT;

/// The public key the Chromium-derived Lens clients send (owocr,
/// chrome-lens-py, chrome-lens-rs): a browser's key, not a secret of anyone's.
pub const PUBLIC_API_KEY: &str = chrome_lens_ocr::DEFAULT_API_KEY;

// -- the picture ------------------------------------------------------------------

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
    if image_processor::should_downscale(width, height) {
        image_processor::preferred_size(width, height)
    } else {
        (width, height)
    }
}

/// Encode `img` for upload: made smaller by Chromium's rule, then JPEG at
/// quality 40, a picture that really uses transparency laid on whichever of
/// white and black its content stands out against.
pub fn encode_image(img: &DynamicImage) -> Result<Upload, String> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    let p = image_processor::encode_for_api(img, AlphaMode::Contrast).map_err(|e| format!("{e:#}"))?;
    Ok(Upload { jpeg: p.bytes, width: p.width.max(0) as u32, height: p.height.max(0) as u32 })
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
            region: chrome_lens_ocr::constants::DEFAULT_CLIENT_REGION.into(),
            time_zone: chrome_lens_ocr::constants::DEFAULT_CLIENT_TIME_ZONE.into(),
        }
    }
}

/// The body of the objects request for `upload`: recognise the whole
/// picture (and translate it, with `query.translate_to`).
pub fn objects_request(upload: &Upload, query: &Query) -> Result<Vec<u8>, String> {
    let image = ProcessedImage { bytes: upload.jpeg.clone(), width: upload.width as i32, height: upload.height as i32 };
    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let options = ScanOptions {
        // None is the empty string on the wire: Lens tells the language.
        ocr_language: non_empty(&query.ocr_language),
        target_language: query.translate_to.as_deref().and_then(non_empty),
        source_language: query.translate_from.as_deref().and_then(non_empty),
        client_region: non_empty(&query.region),
        client_time_zone: non_empty(&query.time_zone),
        alpha_mode: AlphaMode::Contrast,
    };
    chrome_lens_ocr::request_body(&image, &options).map_err(|e| format!("{e:#}"))
}

/// The request's headers. chrome-lens-rs leaves the transport ones to the
/// client: Glossa's asks for gzip and decompresses what comes back.
pub fn headers(api_key: &str) -> Vec<(&'static str, String)> {
    let mut h = chrome_lens_ocr::request_headers(api_key);
    h.push(("accept-encoding", "gzip".into()));
    h
}

// -- the reply --------------------------------------------------------------------

/// Read the reply to a request for `upload`. Every box comes out normalised
/// to 0..1, whatever coordinates Lens used, so it is multiplied by the size
/// of whichever copy of the picture is drawn on (the original or the
/// uploaded one: the resize keeps the proportions). Translations are paired
/// with paragraphs by position, a failed one leaving its paragraph without.
pub fn parse_response(body: &[u8], upload: &Upload) -> Result<Page, String> {
    chrome_lens_ocr::parse_response_bytes(body, upload.width as i32, upload.height as i32).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use prost::Message;

    use super::proto::*;
    use super::Query;
    use super::*;

    fn upload() -> Upload {
        Upload { jpeg: vec![0xff, 0xd8, 0xff], width: 800, height: 600 }
    }

    #[test]
    fn plain_recognition_sends_exactly_the_auto_filter() {
        let body = objects_request(&upload(), &Query::default()).unwrap();
        let req = LensOverlayServerRequest::decode(body.as_slice()).unwrap();
        let ctx = req.objects_request.unwrap().request_context.unwrap();
        let id = ctx.request_id.unwrap();
        assert!(id.uuid.is_some());
        assert_eq!((id.sequence_id, id.image_sequence_id), (Some(1), Some(1)));
        assert!(id.time_usec.unwrap() > 1_600_000_000_000_000);
        let client = ctx.client_context.unwrap();
        assert_eq!(client.platform, Platform::Web as i32);
        assert_eq!(client.surface, Surface::Chromium as i32);
        assert_eq!(
            client.rendering_context.unwrap().rendering_environment,
            LensRenderingEnvironment::RenderingEnvLensOverlay as i32
        );
        let filters = client.client_filters.unwrap().filter;
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].filter_type, Some(LensOverlayFilterType::AutoFilter as i32));
        assert!(client.app_id.is_empty(), "Chromium never sets it");
        // No OCR language: the field is left out, as Chromium leaves it
        // (implicit presence), not sent empty.
        let locale = client.locale_context.unwrap();
        assert_eq!(locale.language, "");
        let bytes = locale.encode_to_vec();
        assert_eq!(bytes[0], 0x12, "the first field is the region (2), not an empty language (1)");
    }

    #[test]
    fn translation_replaces_the_auto_filter() {
        let q = Query { translate_to: Some("ru".into()), translate_from: Some("en".into()), ..Query::default() };
        let req = LensOverlayServerRequest::decode(objects_request(&upload(), &q).unwrap().as_slice()).unwrap();
        let o = req.objects_request.unwrap();
        let filters = o.request_context.unwrap().client_context.unwrap().client_filters.unwrap().filter;
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].filter_type, Some(LensOverlayFilterType::Translate as i32));
        let Some(applied_filter::FilterPayload::Translate(t)) = &filters[0].filter_payload else { panic!() };
        assert_eq!((t.target_language.as_deref(), t.source_language.as_deref()), (Some("ru"), Some("en")));
        let meta = o.image_data.unwrap().image_metadata.unwrap();
        assert_eq!((meta.width, meta.height), (800, 600));
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
    fn pictures_go_up_as_jpeg() {
        let img = image::RgbaImage::from_pixel(40, 20, image::Rgba([0, 0, 0, 0]));
        let up = encode_image(&DynamicImage::ImageRgba8(img)).unwrap();
        assert_eq!(&up.jpeg[..2], &[0xff, 0xd8]);
        assert_eq!((up.width, up.height), (40, 20));
    }

    #[test]
    fn the_transport_headers_are_ours() {
        let h = headers("k");
        assert!(h.iter().any(|(n, v)| n.eq_ignore_ascii_case("x-goog-api-key") && v == "k"));
        assert!(h.iter().any(|(n, v)| n.eq_ignore_ascii_case("accept-encoding") && v == "gzip"));
    }

    fn word(text: &str, sep: &str) -> text_layout::Word {
        text_layout::Word { plain_text: text.into(), text_separator: sep.into(), ..Default::default() }
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
                    content_language: "en".into(),
                }),
                deep_gleams: vec![DeepGleamData {
                    rendering_oneof: Some(deep_gleam_data::RenderingOneof::Translation(translation)),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let page = parse_response(&reply.encode_to_vec(), &upload()).unwrap();
        assert_eq!(page.language.as_deref(), Some("en"));
        assert_eq!(page.full_text, "Hello, world\nagain");
        let b = page.paragraphs[0].lines[0].geometry.unwrap();
        assert!((b.center_y - 0.2).abs() < 1e-6);
        let p = &page.paragraphs[0];
        assert_eq!(p.translation.as_ref().map(|t| t.text.as_str()), Some("Привет, мир снова"));
        assert_eq!(
            (p.lines[0].translation.as_deref(), p.lines[1].translation.as_deref()),
            // As cut out of the paragraph's translation, spaces and all.
            (Some("Привет, мир"), Some(" снова"))
        );
        assert_eq!(page.translation.as_deref(), Some("Привет, мир снова"));
    }

    #[test]
    fn a_server_error_is_an_error() {
        let reply = LensOverlayServerResponse {
            error: Some(LensOverlayServerError { error_type: 1, ..Default::default() }),
            ..Default::default()
        };
        let e = parse_response(&reply.encode_to_vec(), &upload()).unwrap_err();
        assert!(e.contains("MISSING_REQUEST"), "{e}");
    }
}
