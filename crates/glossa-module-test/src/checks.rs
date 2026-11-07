//! What every result must look like, and the case's own expectations.
//!
//! The runtimes already parse a module's reply into the protocol's wire
//! types and convert it, so a reply with a missing field or a wrong type
//! arrives here as a `bad_response` error naming the field. These checks
//! cover what parsing cannot: an empty translation that should have been an
//! error, OCR boxes outside the picture, audio that is not what its MIME
//! type says.

use glossa_core::{Article, Audio, Detection, Error, LangId, OcrResult, Translation};
use serde_json::Value;

use crate::fixture::Expect;

/// A finished call, in a form the checks and the report can use.
pub enum Outcome {
    Translation(Translation),
    Detection(Detection),
    Article(Article),
    Audio(Audio),
    Ocr { result: OcrResult, image_size: Option<(u32, u32)> },
}

impl Outcome {
    /// The text expectations look at.
    pub fn text(&self) -> String {
        match self {
            Outcome::Translation(t) => t.text.clone(),
            Outcome::Detection(d) => d.lang.bcp47().to_owned(),
            Outcome::Article(a) => {
                let mut s = a.to_plain_text();
                if let Some(h) = &a.html {
                    s.push('\n');
                    s.push_str(h);
                }
                s
            }
            Outcome::Audio(a) => a.mime.clone(),
            Outcome::Ocr { result, .. } => result.best_text().to_owned(),
        }
    }

    /// One line for the report.
    pub fn preview(&self) -> String {
        let one_line = |s: &str| {
            let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if flat.chars().count() > 60 {
                flat.chars().take(60).chain(std::iter::once('…')).collect()
            } else {
                flat
            }
        };
        match self {
            Outcome::Translation(t) => format!("{:?}", one_line(&t.text)),
            Outcome::Detection(d) => format!("{} ({:.2})", d.lang.bcp47(), d.confidence),
            Outcome::Article(a) => format!("{:?}, {} sense groups", a.headword, a.explanations.len()),
            Outcome::Audio(a) => format!("{} bytes of {}", a.bytes.len(), a.mime),
            Outcome::Ocr { result, .. } => {
                format!("{:?}, {} nodes", one_line(result.best_text()), result.nodes().count())
            }
        }
    }

    /// The result as JSON, for `expect.result`.
    pub fn to_json(&self) -> Value {
        let v = match self {
            Outcome::Translation(t) => serde_json::to_value(t),
            Outcome::Detection(d) => serde_json::to_value(d),
            Outcome::Article(a) => serde_json::to_value(a),
            Outcome::Audio(a) => Ok(serde_json::json!({ "mime": a.mime, "bytes": a.bytes.len() })),
            Outcome::Ocr { result, .. } => serde_json::to_value(result),
        };
        v.unwrap_or(Value::Null)
    }

    fn detected(&self) -> Option<LangId> {
        match self {
            Outcome::Translation(t) => t.detected_from,
            Outcome::Detection(d) => Some(d.lang),
            Outcome::Ocr { result, .. } => result.detected_lang,
            _ => None,
        }
    }
}

/// The shape every result of its kind must have. Each problem is one line.
pub fn shape(outcome: &Outcome) -> Vec<String> {
    let mut out = Vec::new();
    match outcome {
        Outcome::Translation(t) => {
            if t.text.trim().is_empty() {
                out.push("the translation is empty; fail with an error instead of returning nothing".into());
            }
        }
        Outcome::Detection(d) => {
            if !d.lang.is_language() {
                out.push(format!("detected {:?}, which is not a language", d.lang.bcp47()));
            }
            if !(0.0..=1.0).contains(&d.confidence) {
                out.push(format!("confidence {} is outside 0..1", d.confidence));
            }
        }
        Outcome::Article(a) => {
            if a.is_empty() {
                out.push("the article is empty; throw not_found when there is no entry".into());
            }
            if a.headword.trim().is_empty() {
                out.push("the article has no headword".into());
            }
        }
        Outcome::Audio(a) => audio_shape(a, &mut out),
        Outcome::Ocr { result, image_size } => ocr_shape(result, *image_size, &mut out),
    }
    out
}

fn audio_shape(a: &Audio, out: &mut Vec<String>) {
    if a.bytes.is_empty() {
        out.push("the audio is empty".into());
        return;
    }
    let b = &a.bytes;
    let sniffed = if b.starts_with(b"ID3") || (b.len() > 1 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0) {
        Some("audio/mpeg")
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if b.starts_with(b"fLaC") {
        Some("audio/flac")
    } else {
        None
    };
    let mime = a.mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    let same = |m: &str| match (m, mime.as_str()) {
        ("audio/wav", "audio/x-wav" | "audio/wave" | "audio/vnd.wave") => true,
        ("audio/mpeg", "audio/mp3") => true,
        (x, y) => x == y,
    };
    match sniffed {
        Some(kind) if !same(kind) => {
            out.push(format!("the audio is {kind} but the reply says {:?}", a.mime));
        }
        None if !mime.starts_with("audio/") => out.push(format!("{:?} is not an audio type", a.mime)),
        _ => {}
    }
}

fn ocr_shape(r: &OcrResult, input: Option<(u32, u32)>, out: &mut Vec<String>) {
    if r.best_text().trim().is_empty() {
        out.push("nothing was recognised; fail with not_found when the picture has no text".into());
    }
    let size = match (r.image_size, input) {
        (Some(said), Some(real)) if said != real => {
            out.push(format!(
                "the reply gives the image as {}x{} but it is {}x{}; coordinates must refer to the input picture (send a transform when you scale it)",
                said.0, said.1, real.0, real.1
            ));
            Some(real)
        }
        (_, Some(real)) => Some(real),
        (said, None) => said,
    };
    let mut reported = 0;
    for node in r.nodes() {
        if reported >= 5 {
            out.push("… and more boxes like these".into());
            break;
        }
        let label = format!("{:?}", node.text.chars().take(24).collect::<String>());
        if let Some(c) = node.confidence {
            if !(0.0..=1.0).contains(&c) {
                out.push(format!("node {label}: confidence {c} is outside 0..1"));
                reported += 1;
            }
        }
        if !node.angle.is_finite() {
            out.push(format!("node {label}: the angle is not a number"));
            reported += 1;
        }
        let Some((w, h)) = size else { continue };
        let (w, h) = (w as f32, h as f32);
        // A pixel or two of slack: engines round outward.
        let tol_x = (w * 0.01).max(2.0);
        let tol_y = (h * 0.01).max(2.0);
        let inside = |x: f32, y: f32| x >= -tol_x && y >= -tol_y && x <= w + tol_x && y <= h + tol_y;
        if let Some(b) = node.bounds {
            let finite = [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite());
            if !finite || b.width < 0.0 || b.height < 0.0 {
                out.push(format!("node {label}: box {b:?} is not a valid rectangle"));
                reported += 1;
            } else if !inside(b.x, b.y) || !inside(b.x + b.width, b.y + b.height) {
                out.push(format!(
                    "node {label}: box at ({}, {}) size {}x{} lies outside the {}x{} picture",
                    b.x, b.y, b.width, b.height, w, h
                ));
                reported += 1;
            }
        }
        if node.polygon.iter().any(|[x, y]| !x.is_finite() || !y.is_finite() || !inside(*x, *y)) {
            out.push(format!("node {label}: a polygon point lies outside the {w}x{h} picture"));
            reported += 1;
        }
    }
}

/// The case's own expectations for a result.
pub fn expectations(outcome: &Outcome, expect: &Expect) -> Vec<String> {
    let mut out = Vec::new();
    let text = outcome.text();
    if let Some(want) = &expect.text {
        if text.trim() != want.trim() {
            out.push(format!("expected the text {want:?}, got {:?}", text.trim()));
        }
    }
    for part in &expect.contains.0 {
        if !text.contains(part.as_str()) {
            let shown: String = text.chars().take(200).collect();
            out.push(format!("expected the text to contain {part:?}; it was {shown:?}"));
        }
    }
    if let Some(want) = &expect.detected {
        match (LangId::from_code(want), outcome.detected()) {
            (None, _) => out.push(format!("expect.detected: {want:?} is not a language code")),
            (Some(w), Some(got)) if w == got => {}
            (Some(_), got) => {
                out.push(format!("expected {want:?} to be detected, got {:?}", got.map(|l| l.bcp47())));
            }
        }
    }
    if let Some(want) = &expect.headword {
        match outcome {
            Outcome::Article(a) if a.headword.trim() == want.trim() => {}
            Outcome::Article(a) => out.push(format!("expected the headword {want:?}, got {:?}", a.headword)),
            _ => out.push("expect.headword applies to dictionary cases only".into()),
        }
    }
    if let Some(want) = &expect.mime {
        match outcome {
            Outcome::Audio(a) if &a.mime == want => {}
            Outcome::Audio(a) => out.push(format!("expected {want:?} audio, got {:?}", a.mime)),
            _ => out.push("expect.mime applies to speak cases only".into()),
        }
    }
    if let Some(want) = &expect.result {
        let got = outcome.to_json();
        if let Some(path) = mismatch(want, &got, "result") {
            out.push(path);
        }
    }
    out
}

/// Where `got` fails to contain `want`, or `None` when it does.
pub fn mismatch(want: &Value, got: &Value, path: &str) -> Option<String> {
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => w.iter().find_map(|(k, wv)| match g.get(k) {
            Some(gv) => mismatch(wv, gv, &format!("{path}.{k}")),
            None => {
                // The JSON names are the serialised ones (`trait`, `explains`
                // in an article), which are easy to guess wrong: list them.
                let have: Vec<&str> = g.keys().map(String::as_str).collect();
                Some(format!("{path}.{k} is missing; {path} has {}", have.join(", ")))
            }
        }),
        (Value::Array(w), Value::Array(g)) => {
            if g.len() < w.len() {
                return Some(format!("{path} has {} elements, expected at least {}", g.len(), w.len()));
            }
            w.iter().zip(g).enumerate().find_map(|(i, (wv, gv))| mismatch(wv, gv, &format!("{path}[{i}]")))
        }
        (Value::Number(w), Value::Number(g)) => {
            let close = match (w.as_f64(), g.as_f64()) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-3,
                _ => w == g,
            };
            (!close).then(|| format!("{path} is {g}, expected {w}"))
        }
        (w, g) if w == g => None,
        (w, g) => Some(format!("{path} is {g}, expected {w}")),
    }
}

/// The name a fixture uses for an error, as in `expect.error`.
pub fn error_kind(e: &Error) -> &'static str {
    match e {
        Error::MissingCredential { .. } => "missing_credential",
        Error::Unsupported { .. } => "unsupported",
        Error::SourceLanguageUnsupported { .. } => "source_language",
        Error::TargetLanguageUnsupported { .. } => "target_language",
        Error::RateLimited { .. } => "rate_limited",
        Error::Network(_) => "network",
        Error::Timeout(_) | Error::ModuleTimeout { .. } => "timeout",
        Error::HttpStatus { .. } => "http",
        Error::BadResponse { .. } => "bad_response",
        Error::NoTextRecognised | Error::LanguageNotDetected => "not_found",
        Error::Module { .. } => "module",
        Error::AssetsMissing { .. } => "assets_missing",
        Error::RuntimeMissing { .. } => "runtime_missing",
        _ => "other",
    }
}

/// Whether an error is the expected one. `not_found` from a dictionary
/// arrives as a module error carrying the module's message (the kind is
/// dropped in `glossa_modules::protocol`), so `not_found` accepts a module
/// error too; the caller then insists on `error_contains`, since a crash is
/// a module error as well.
pub fn error_matches(e: &Error, want: &str) -> bool {
    let got = error_kind(e);
    got == want || (want == "not_found" && got == "module")
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::ocr::{OcrBlock, OcrLevel, Rect};
    use serde_json::json;

    #[test]
    fn partial_json_matches() {
        let got = json!({"text": "a", "list": [1, 2, 3], "n": {"x": 1.0}});
        assert_eq!(mismatch(&json!({"list": [1, 2], "n": {"x": 1}}), &got, "r"), None);
        assert_eq!(mismatch(&json!({"text": "b"}), &got, "r").unwrap(), "r.text is \"a\", expected \"b\"");
        assert!(mismatch(&json!({"missing": 1}), &got, "r").unwrap().contains("missing"));
        assert!(mismatch(&json!({"list": [1, 2, 3, 4]}), &got, "r").unwrap().contains("at least 4"));
    }

    #[test]
    fn ocr_boxes_outside_the_picture_are_reported() {
        let mut line = OcrBlock::new(OcrLevel::Line, "hello");
        line.bounds = Some(Rect::new(10.0, 10.0, 500.0, 20.0));
        line.confidence = Some(87.0);
        let r = OcrResult { text: "hello".into(), blocks: vec![line], ..Default::default() };
        let problems = shape(&Outcome::Ocr { result: r, image_size: Some((200, 50)) });
        assert!(problems.iter().any(|p| p.contains("outside the 200x50 picture")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("confidence 87")), "{problems:?}");
    }

    #[test]
    fn audio_must_be_what_it_says() {
        let wav = Audio { bytes: b"RIFF\0\0\0\0WAVEfmt ".to_vec(), mime: "audio/mpeg".into() };
        assert!(shape(&Outcome::Audio(wav))[0].contains("audio/wav"));
        let mp3 = Audio { bytes: b"ID3\x03rest".to_vec(), mime: "audio/mpeg".into() };
        assert!(shape(&Outcome::Audio(mp3)).is_empty());
    }

    #[test]
    fn empty_answers_are_not_results() {
        assert!(!shape(&Outcome::Translation(Translation::plain("  "))).is_empty());
        assert!(!shape(&Outcome::Article(Article::default())).is_empty());
    }
}
