//! The Glossa Module Protocol: message types.
//!
//! Languages travel as BCP 47 codes (`en`, `ru`, `zh-Hans`, `he`) plus the
//! pseudo-code `auto`. A module whose service spells codes differently maps
//! them itself, or declares a `[languages.map]` table in its manifest and
//! lets the host do it.
//!
//! Every request carries the module's option values (`options`), merged from
//! the manifest defaults and the user's settings, so a module never needs to
//! ask for them.

use std::collections::BTreeMap;

use base64::Engine;
use serde::{Deserialize, Serialize};

pub type Options = BTreeMap<String, serde_json::Value>;

// -- requests -----------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslateParams {
    pub text: String,
    /// Source code, or `auto`.
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glossary: Vec<GlossaryTerm>,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryTerm {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectParams {
    pub text: String,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DictionaryParams {
    pub text: String,
    pub from: String,
    pub to: String,
    #[serde(default = "yes")]
    pub exact: bool,
    #[serde(default)]
    pub options: Options,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakParams {
    pub text: String,
    pub lang: String,
    #[serde(default)]
    pub slow: bool,
    #[serde(default)]
    pub options: Options,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognizeParams {
    /// The image, base64-encoded (PNG, JPEG or WebP).
    pub image_base64: String,
    /// Language of the text, or `auto`.
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_to: Option<String>,
    #[serde(default = "yes")]
    pub preserve_line_breaks: bool,
    #[serde(default)]
    pub options: Options,
    /// How much structure the caller wants: `full_text`, `blocks`, `lines`
    /// or `words`. A hint: returning more is fine, and an engine that only
    /// gives text may ignore it. Absent from older hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
    /// The same image, already registered with the host's image API for
    /// this call (`image.*` host calls), so a module can preprocess it
    /// without sending the bytes back. Absent from older hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_handle: Option<u32>,
}

impl RecognizeParams {
    pub fn image_bytes(&self) -> Result<Vec<u8>, ModuleError> {
        base64::engine::general_purpose::STANDARD
            .decode(self.image_base64.as_bytes())
            .map_err(|e| ModuleError::new(ErrorKind::BadRequest, format!("image_base64: {e}")))
    }
}

// -- results ------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslateResult {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub romanization: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts_of_speech: Vec<PartOfSpeech>,
}

impl TranslateResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PartOfSpeech {
    pub part_of_speech: String,
    pub senses: Vec<Sense>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sense {
    pub word: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub back_translations: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectResult {
    pub lang: String,
    #[serde(default = "one")]
    pub confidence: f32,
}

fn one() -> f32 {
    1.0
}

/// A dictionary article. Field names follow pot-app's dictionary-shaped
/// translate result (`trait`, `explains`), so code written for it ports
/// directly; the descriptive names are accepted too.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArticleResult {
    #[serde(default)]
    pub headword: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pronunciations: Vec<Pronunciation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explanations: Vec<Explanation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "sentence")]
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub associations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pronunciation {
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Explanation {
    #[serde(default, rename = "trait", alias = "part_of_speech")]
    pub part_of_speech: String,
    #[serde(default, rename = "explains", alias = "definitions")]
    pub definitions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub source: String,
    #[serde(default)]
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakResult {
    pub audio_base64: String,
    /// `audio/mpeg`, `audio/wav`, `audio/ogg`.
    pub mime: String,
}

impl SpeakResult {
    pub fn from_bytes(bytes: &[u8], mime: impl Into<String>) -> Self {
        Self { audio_base64: base64::engine::general_purpose::STANDARD.encode(bytes), mime: mime.into() }
    }
}

/// What `recognize` returns. Only `text` is required (and may be empty when
/// `blocks`, `hocr` or `alto` carries the result: the host joins the text
/// from the structure then).
///
/// Coordinates are pixels of the image the module was given. A module that
/// recognised a preprocessed copy either maps its boxes back itself
/// ([`Transform::map_rect`]) or sends the copy's `transform` and lets the
/// host do it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognizeResult {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_lang: Option<String>,
    /// The layout: blocks of lines of words (or any part of that tree).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<OcrNode>,
    /// `[width, height]` of the image the coordinates refer to; the input
    /// image's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<[u32; 2]>,
    /// The result as an hOCR document instead of `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hocr: Option<String>,
    /// The result as an ALTO XML document instead of `blocks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alto: Option<String>,
    /// Set when the coordinates refer to a preprocessed copy of the input
    /// image: the copy's transform from the image API, which the host
    /// applies to every box to bring it back onto the input image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
}

impl RecognizeResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }
}

/// One region of a recognition result: a block, paragraph, line or word.
/// The JSON is the same as `glossa-cli ocr --format json` prints for a
/// block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OcrNode {
    #[serde(default)]
    pub text: String,
    /// `block`, `paragraph`, `line` or `word`. Inferred from the tree when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The text's box as it would be if it were level, about its centre;
    /// for level text simply its bounding box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    /// The same as `[x0, y0, x1, y1]`, for engines that think in corners;
    /// used when `bounds` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f32; 4]>,
    /// Rotation in radians, clockwise on screen, about the box's centre.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub angle: f32,
    /// The outline (`[[x, y], …]`, usually four corners clockwise from the
    /// text's top left) for rotated or irregular text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygon: Vec<[f32; 2]>,
    /// 0–1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// For lines: `y = slope * x + offset` from the bottom-left corner of
    /// the line's box (hOCR's convention).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
    /// BCP 47.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// `ltr`, `rtl` or `ttb` (vertical).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// This region translated, when the service translates region by region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OcrNode>,
}

impl OcrNode {
    /// A node of `level` (`block`, `paragraph`, `line`, `word`) with a box.
    pub fn new(level: &str, text: impl Into<String>, bounds: Option<Rect>) -> Self {
        Self { text: text.into(), level: Some(level.to_owned()), bounds, ..Default::default() }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// An axis-aligned rectangle in pixels.
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub slope: f32,
    pub offset: f32,
}

/// Where a pixel of a derived image (cropped, scaled, padded, rotated…)
/// came from in the original: `[a, b, c, d, e, f]` with
/// `x' = a·x + b·y + c` and `y' = d·x + e·y + f`. Every image the image API
/// returns carries one, composed all the way back to the image it was first
/// decoded from (for `recognize`, the input image).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform(pub [f64; 6]);

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    /// A point of the derived image in the original.
    pub fn map_point(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + b * y + c, d * x + e * y + f)
    }

    /// The axis-aligned box, in the original, around a box of the derived
    /// image: what a module calls on each box an engine found in a
    /// preprocessed copy (the "map_box_back" of the other SDKs).
    pub fn map_rect(&self, r: Rect) -> Rect {
        let corners = [(r.x, r.y), (r.x + r.width, r.y), (r.x + r.width, r.y + r.height), (r.x, r.y + r.height)];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (x, y) in corners {
            let (mx, my) = self.map_point(x as f64, y as f64);
            x0 = x0.min(mx);
            y0 = y0.min(my);
            x1 = x1.max(mx);
            y1 = y1.max(my);
        }
        Rect::new(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32)
    }

    /// The transform that applies `inner` first, then `self`.
    pub fn compose(&self, inner: &Transform) -> Transform {
        let [a, b, c, d, e, f] = self.0;
        let [p, q, r, s, t, u] = inner.0;
        Transform([a * p + b * s, a * q + b * t, a * r + b * u + c, d * p + e * s, d * q + e * t, d * r + e * u + f])
    }

    /// How much a length grows going back to the original (the square root
    /// of the determinant): 2 for an image scaled to half size.
    pub fn scale(&self) -> f64 {
        let [a, b, _, d, e, _] = self.0;
        (a * e - b * d).abs().sqrt()
    }

    /// The rotation going back to the original, radians clockwise.
    pub fn rotation(&self) -> f64 {
        let [a, _, _, d, _, _] = self.0;
        d.atan2(a)
    }

    pub fn is_identity(&self) -> bool {
        *self == Transform::IDENTITY
    }
}

/// What every `image.*` host call returns. The image fields describe the
/// resulting image (a new handle unless the operation had nothing to do);
/// the optional fields are filled by the operations that produce them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    #[serde(default)]
    pub handle: u32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// 1 (grey) or 4 (RGBA).
    #[serde(default)]
    pub channels: u8,
    /// Back to the original image; see [`Transform`].
    #[serde(default)]
    pub transform: Transform,
    /// The format the image was decoded from (`png`, `jpeg`, `webp`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// `image.encode` / `image.pixels`: the bytes, base64-encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    /// `image.encode`: `image/png`, `image/jpeg`, `image/webp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// `image.threshold`: the level used (Otsu's when not given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u8>,
    /// `image.deskew`: the rotation applied, degrees clockwise (the text
    /// was skewed by the opposite).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f32>,
    /// `image.split_tiles`: the tiles, in rows from the top left.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiles: Vec<ImageInfo>,
}

impl ImageInfo {
    /// The bytes of `data_base64`.
    pub fn bytes(&self) -> Result<Vec<u8>, ModuleError> {
        let b64 = self.data_base64.as_deref().unwrap_or("");
        base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| ModuleError::new(ErrorKind::BadResponse, format!("data_base64: {e}")))
    }
}

// -- host calls ----------------------------------------------------------------

/// What the `assets` host call returns: where the files a module declares
/// in `[[assets]]` are. Each path is where the file (or the folder an
/// archive was unpacked into) is once downloaded; Glossa starts a module
/// only when its required assets are all there. Empty for a module without
/// assets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetsInfo {
    /// The module's assets folder (`GLOSSA_ASSETS_DIR` for process modules).
    #[serde(default)]
    pub dir: String,
    /// Asset name -> absolute path.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// The names in `files` not downloaded yet: optional assets the user
    /// has not asked for. A file at its path is always a verified one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpRequest {
    #[serde(default = "get")]
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// A text body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// A binary body, base64-encoded. Takes precedence over `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Return the body base64-encoded instead of as text (for audio, images).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

fn get() -> String {
    "GET".into()
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        Self { method: "POST".into(), url: url.into(), headers, body: Some(body.to_string()), ..Default::default() }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The body as text (decoded per the response charset).
    #[serde(default)]
    pub body: String,
    /// The body base64-encoded, when the request set `binary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body if the status was 2xx, else an error carrying the status.
    pub fn ok_text(self) -> Result<String, ModuleError> {
        if self.is_success() {
            Ok(self.body)
        } else {
            Err(ModuleError::new(ErrorKind::Http, format!("HTTP {}: {}", self.status, truncate(&self.body, 200))))
        }
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ModuleError> {
        serde_json::from_str(&self.body).map_err(|e| ModuleError::new(ErrorKind::BadResponse, e.to_string()))
    }

    pub fn bytes(&self) -> Result<Vec<u8>, ModuleError> {
        match &self.body_base64 {
            Some(b) => base64::engine::general_purpose::STANDARD
                .decode(b.as_bytes())
                .map_err(|e| ModuleError::new(ErrorKind::BadResponse, e.to_string())),
            None => Ok(self.body.clone().into_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

// -- envelopes ------------------------------------------------------------------

/// What a module (or the host) answers: a result or an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Result(serde_json::Value),
    Error(ModuleError),
}

impl Reply {
    pub fn ok(value: serde_json::Value) -> Self {
        Reply::Result(value)
    }

    pub fn err(error: ModuleError) -> Self {
        Reply::Error(error)
    }

    pub fn into_result(self) -> Result<serde_json::Value, ModuleError> {
        match self {
            Reply::Result(v) => Ok(v),
            Reply::Error(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleError {
    pub kind: ErrorKind,
    pub message: String,
    /// For `rate_limited`: seconds until a retry may work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl ModuleError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), retry_after_secs: None }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }

    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorKind::Unsupported, format!("{what} is not supported by this module"))
    }

    /// The module needs an option the user has not filled in.
    pub fn missing_option(key: &str) -> Self {
        Self::new(ErrorKind::MissingCredential, key)
    }

    pub fn bad_response(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadResponse, message)
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ModuleError {}

/// Error kinds, chosen to map onto the host's own error type so the UI can say
/// something specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request was malformed.
    BadRequest,
    /// The module does not do this.
    Unsupported,
    /// The source language is not supported.
    SourceLanguage,
    /// The target language is not supported.
    TargetLanguage,
    /// An option the module needs (a key, a URL) is not set.
    MissingCredential,
    /// The service answered with something unparseable.
    BadResponse,
    /// The service answered with an error status.
    Http,
    /// The network failed.
    Network,
    /// The service is rate-limiting.
    RateLimited,
    /// Nothing was found (dictionary miss, no text in image).
    NotFound,
    Other,
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        s.chars().take(n).chain(std::iter::once('…')).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_envelopes_serialise_as_documented() {
        let ok = serde_json::to_value(Reply::ok(serde_json::json!({"text": "hi"}))).unwrap();
        assert_eq!(ok, serde_json::json!({"result": {"text": "hi"}}));
        let err = serde_json::to_value(Reply::err(ModuleError::missing_option("api_key"))).unwrap();
        assert_eq!(err, serde_json::json!({"error": {"kind": "missing_credential", "message": "api_key"}}));
    }

    #[test]
    fn requests_tolerate_missing_optional_fields() {
        let p: TranslateParams = serde_json::from_str(r#"{"text":"hi","from":"auto","to":"ru"}"#).unwrap();
        assert!(p.options.is_empty());
        assert!(p.glossary.is_empty());
        let d: DictionaryParams = serde_json::from_str(r#"{"text":"run","from":"en","to":"ru"}"#).unwrap();
        assert!(d.exact, "exact defaults to true");
        let h: HttpRequest = serde_json::from_str(r#"{"url":"https://x"}"#).unwrap();
        assert_eq!(h.method, "GET");
    }

    #[test]
    fn pot_shaped_articles_parse() {
        let a: ArticleResult = serde_json::from_str(
            r#"{"explanations":[{"trait":"n.","explains":["a thing"]}],"sentence":[{"source":"a","target":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(a.explanations[0].part_of_speech, "n.");
        assert_eq!(a.examples.len(), 1, "pot's `sentence` maps to examples");
    }

    #[test]
    fn speak_result_round_trips_bytes() {
        let r = SpeakResult::from_bytes(&[1, 2, 3, 255], "audio/mpeg");
        let resp = HttpResponse { status: 200, body_base64: Some(r.audio_base64.clone()), ..Default::default() };
        assert_eq!(resp.bytes().unwrap(), vec![1, 2, 3, 255]);
    }

    #[test]
    fn recognition_messages_stay_compatible_both_ways() {
        // An old module's reply and an old host's request still parse.
        let r: RecognizeResult = serde_json::from_str(r#"{"text":"hi","detected_lang":"en"}"#).unwrap();
        assert!(r.blocks.is_empty() && r.hocr.is_none() && r.transform.is_none());
        let p: RecognizeParams = serde_json::from_str(r#"{"image_base64":"","lang":"auto"}"#).unwrap();
        assert_eq!((p.granularity, p.image_handle), (None, None));
        // New optional fields are left out until set.
        assert_eq!(serde_json::to_value(RecognizeResult::text("x")).unwrap(), serde_json::json!({"text": "x"}));
        let node: OcrNode =
            serde_json::from_str(r#"{"text":"a","level":"word","bbox":[1,2,3,4],"confidence":0.5}"#).unwrap();
        assert_eq!(node.bbox, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(
            serde_json::to_value(OcrNode::new("line", "t", None)).unwrap(),
            serde_json::json!({"text": "t", "level": "line"})
        );
    }

    #[test]
    fn transforms_map_points_boxes_and_compose() {
        let crop = Transform([1.0, 0.0, 50.0, 0.0, 1.0, 10.0]);
        let half = Transform([2.0, 0.0, 0.0, 0.0, 2.0, 0.0]);
        let t = crop.compose(&half); // scaled after cropping: undo the scale, then the crop
        assert_eq!(t.map_point(10.0, 10.0), (70.0, 30.0));
        assert_eq!(t.map_rect(Rect::new(0.0, 0.0, 5.0, 5.0)), Rect::new(50.0, 10.0, 10.0, 10.0));
        assert_eq!(t.scale(), 2.0);
        let turned = Transform([0.0, 1.0, 0.0, -1.0, 0.0, 8.0]); // a quarter turn of an 8-tall image
        assert_eq!(turned.map_rect(Rect::new(0.0, 0.0, 8.0, 2.0)), Rect::new(0.0, 0.0, 2.0, 8.0));
        assert!((turned.rotation().to_degrees() + 90.0).abs() < 1e-9);
        assert!(Transform::default().is_identity());
        assert_eq!(serde_json::to_value(crop).unwrap(), serde_json::json!([1.0, 0.0, 50.0, 0.0, 1.0, 10.0]));
    }

    #[test]
    fn non_success_responses_become_errors_with_the_status() {
        let r = HttpResponse { status: 401, body: "bad key".into(), ..Default::default() };
        let e = r.ok_text().unwrap_err();
        assert_eq!(e.kind, ErrorKind::Http);
        assert!(e.message.contains("401"));
    }
}
