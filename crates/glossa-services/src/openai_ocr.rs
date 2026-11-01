//! Text recognition with a vision model behind an AI endpoint: the same
//! OpenAI-compatible chat API the endpoint translates with (OpenAI,
//! OpenRouter, a local vLLM, Ollama or LM Studio server), with the picture
//! sent as an `image_url` data URL.
//!
//! Two answers can be asked for ([`Mode`]):
//!
//! - **Text**: the transcription, line breaks kept. Any vision model does
//!   this well.
//! - **Lines**: JSON, one entry per line of text with its box, for
//!   translation over the picture. Coordinates from language models vary:
//!   pixels of the image as sent, pixels of the image as the server resized
//!   it, 0–1000 (Qwen2-VL), 0–1000 with y first (Gemini) or fractions. The
//!   `ocr_box_units` setting says which; `auto` takes values up to 1 as
//!   fractions, values beyond the image as 0–1000, and pixels otherwise.
//!   Gemini answers in its own `box_2d` (`[ymin, xmin, ymax, xmax]`,
//!   0–1000) whatever it is asked for, so a Gemini model is asked for that
//!   and read that way.
//!
//! When the request asks for a translation too, the model is asked to give
//! it in the same answer, so one call does both. Models wrap JSON in prose
//! or code fences; the first object in the answer is taken.
//!
//! An endpoint is a text recognition engine only when chosen as one: the
//! engine never falls back to it on its own, as that would send screenshots
//! to a service nobody picked for them.

use base64::Engine as _;
use glossa_core::ocr::{OcrBlock, OcrGranularity, OcrLevel, Rect};
use glossa_core::{Error, LangId, OcrRequest, OcrResult, Result};
use glossa_modules::Host;
use glossa_net::HttpRequest;
use serde_json::{json, Value};

use super::OpenAi;

/// What to ask the model for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Text,
    Lines,
}

/// How the model gives a box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Units {
    Auto,
    Pixels,
    /// 0–1000, `[x1, y1, x2, y2]`.
    Thousand,
    /// 0–1000, `[y1, x1, y2, x2]` (Gemini).
    ThousandYx,
    /// 0–1.
    Fraction,
}

impl Units {
    fn from_option(s: &str) -> Units {
        match s.trim() {
            "pixels" => Units::Pixels,
            "1000" => Units::Thousand,
            "1000yx" => Units::ThousandYx,
            "fraction" => Units::Fraction,
            _ => Units::Auto,
        }
    }
}

/// The mode for this request: the setting, with `auto` asking for lines
/// only when the caller wants structure (translation over the picture).
pub(super) fn mode(host: &Host, req: &OcrRequest) -> Mode {
    match host.option_str("ocr_mode").trim() {
        "text" => Mode::Text,
        "lines" => Mode::Lines,
        _ if req.granularity == OcrGranularity::FullText => Mode::Text,
        _ => Mode::Lines,
    }
}

impl OpenAi {
    pub(super) fn recognize_image(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        let started = std::time::Instant::now();
        let bytes = req.image.to_bytes()?;
        let size = glossa_modules::protocol::image_dimensions(&bytes);
        let mode = mode(host, req);
        let model = self.ocr_model(host);
        let mut units = Units::from_option(host.option_str("ocr_box_units"));
        let format = box_format(&model, units);
        if format == BoxFormat::Gemini && units == Units::Auto {
            units = Units::ThousandYx;
        }
        let send = || -> Result<_> {
            let http_req = self.ocr_request(host, req, &bytes, size, mode, &model, format)?;
            self.check(host.http.send(http_req)?, host)
        };
        let resp = match send() {
            Err(e) if self.drop_effort_after(host, &model, &e) => send(),
            r => r,
        }?;
        let v = super::parse_json(&resp, self.name())?;
        let content = self.completion_text(&v, resp.status)?;
        let strip = host.option("strip_thinking").and_then(Value::as_bool).unwrap_or(true);
        let content = if strip { super::strip_thinking(&content) } else { content.as_str() };
        let mut out = match mode {
            Mode::Text => parse_text(content, req.translate_to.is_some()),
            Mode::Lines => parse_lines(content, size, units).ok_or_else(|| {
                Error::bad_response(self.name(), "the model did not answer with lines of text in JSON")
            })?,
        };
        out.image_size = size;
        out.provider_id = self.info.id.clone();
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(out)
    }

    /// The model that reads pictures: the vision model, else the endpoint's.
    fn ocr_model(&self, host: &Host) -> String {
        match host.option_str("ocr_model").trim() {
            "" => super::model_for(host, self.preset),
            m => m.to_owned(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ocr_request(
        &self,
        host: &Host,
        req: &OcrRequest,
        bytes: &[u8],
        size: Option<(u32, u32)>,
        mode: Mode,
        model: &str,
        format: BoxFormat,
    ) -> Result<HttpRequest> {
        let custom = host.option_str("ocr_prompt").trim();
        let instructions =
            if custom.is_empty() { prompt(mode, req.lang, req.translate_to, size, format) } else { custom.to_owned() };
        let url = format!("data:{};base64,{}", mime(bytes), base64::engine::general_purpose::STANDARD.encode(bytes));
        let mut body = json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": instructions },
                    { "type": "image_url", "image_url": { "url": url } },
                ],
            }],
        });
        // As for translation: reasoning models reject a temperature other
        // than their default, so a reasoning effort replaces it. Gemini 3
        // and other thinking models otherwise think at length over a
        // screenshot (a minute and more) before transcribing it.
        match self.effort(host, model) {
            // Transcription wants the most likely reading, not variety.
            None => body["temperature"] = json!(0),
            Some(effort) => body["reasoning_effort"] = json!(effort),
        }
        let max = host.option_f64("max_tokens").unwrap_or(0.0);
        if max >= 1.0 {
            let param = match host.option_str("max_tokens_param").trim() {
                "" => self.preset.max_tokens_param,
                p => p,
            };
            body[param] = json!(max as u64);
        }
        super::merge_extra_body(&mut body, host, self.name())?;
        let url = format!("{}/chat/completions", self.root(host));
        Ok(self.with_headers(HttpRequest::post(url).json(&body), host))
    }
}

/// How the instructions ask for boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoxFormat {
    /// `box`: `[x1, y1, x2, y2]` in pixels of the image.
    Pixels,
    /// Gemini's own `box_2d`: `[ymin, xmin, ymax, xmax]`, 0–1000.
    Gemini,
}

/// Gemini's format for a Gemini model, unless the settings name units that
/// are not Gemini's.
fn box_format(model: &str, units: Units) -> BoxFormat {
    let gemini = model.to_ascii_lowercase().contains("gemini");
    if gemini && matches!(units, Units::Auto | Units::ThousandYx) {
        BoxFormat::Gemini
    } else {
        BoxFormat::Pixels
    }
}

/// The instructions for `mode`, in English: models follow it best, and the
/// answer is the text in the picture either way.
fn prompt(
    mode: Mode,
    lang: LangId,
    translate_to: Option<LangId>,
    size: Option<(u32, u32)>,
    format: BoxFormat,
) -> String {
    let mut p = String::new();
    let lang_hint =
        if lang.is_language() { format!(" The text is in {}.", lang.english_name()) } else { String::new() };
    match (mode, translate_to) {
        (Mode::Text, None) => {
            p.push_str(
                "Transcribe all the text in this image exactly as written, keeping its line breaks and reading \
                 order. Answer with the text only: no comments, no quotes, no formatting. If there is no text, \
                 answer with nothing.",
            );
            p.push_str(&lang_hint);
        }
        (Mode::Text, Some(to)) => {
            p.push_str(&format!(
                "Transcribe all the text in this image exactly as written, keeping its line breaks, and translate \
                 it into {}. Answer with JSON only: {{\"text\": \"<the transcription>\", \"translation\": \"<the \
                 translation>\"}}.",
                to.english_name()
            ));
            p.push_str(&lang_hint);
        }
        (Mode::Lines, to) => {
            let (w, h) = size.unwrap_or((0, 0));
            p.push_str(&match format {
                BoxFormat::Pixels => format!(
                    "Find every line of text in this image, in reading order. Answer with JSON only: {{\"lines\": \
                     [{{\"text\": \"<the line exactly as written>\", \"box\": [x1, y1, x2, y2]}}]}}, where box is \
                     the line's bounding box in pixels of this {w}x{h} image, x to the right, y down."
                ),
                BoxFormat::Gemini => "Find every line of text in this image, in reading order. Answer with JSON only: \
                                      {\"lines\": [{\"text\": \"<the line exactly as written>\", \"box_2d\": [ymin, \
                                      xmin, ymax, xmax]}]}, where box_2d is the line's bounding box normalised to \
                                      0-1000."
                    .to_owned(),
            });
            if let Some(to) = to {
                p.push_str(&format!(
                    " Give each line a \"translation\" field too, translating it into {} in the context of the \
                     lines around it.",
                    to.english_name()
                ));
            }
            p.push_str(&lang_hint);
            p.push_str(" If there is no text, answer {\"lines\": []}.");
        }
    }
    p
}

/// The data URL type for an encoded image.
fn mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "image/webp"
    } else if b.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/png"
    }
}

/// The first JSON object in `s`, past any prose or code fence around it.
fn first_object(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, ch) in s[start..].char_indices() {
        if in_str {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&s[start..start + i + 1]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// A text answer, or with `translated` the `{text, translation}` object
/// (taken as plain text when the model ignored the format).
fn parse_text(content: &str, translated: bool) -> OcrResult {
    let content = strip_fence(content);
    if translated {
        if let Some(v) = first_object(content) {
            if let Some(text) = v["text"].as_str() {
                let translation = v["translation"].as_str().map(str::to_owned).filter(|t| !t.trim().is_empty());
                return OcrResult { text: text.trim().to_owned(), translation, ..Default::default() };
            }
        }
    }
    OcrResult { text: content.trim().to_owned(), ..Default::default() }
}

/// A ```fenced``` answer's inside.
fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else { return t };
    let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or(rest);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

/// A lines answer: a line block each, with its box in the image's pixels.
/// A translated line becomes a block holding that one line, both carrying
/// the translation, which is the shape a provider's own translation takes.
fn parse_lines(content: &str, size: Option<(u32, u32)>, units: Units) -> Option<OcrResult> {
    let v = first_object(content)?;
    let lines = v["lines"].as_array().or_else(|| v["blocks"].as_array())?;
    let raw: Vec<(String, Option<[f32; 4]>, Option<String>)> = lines
        .iter()
        .filter_map(|l| {
            let text = l["text"].as_str()?.trim().to_owned();
            if text.is_empty() {
                return None;
            }
            let b = l["box"].as_array().or_else(|| l["bbox"].as_array()).or_else(|| l["box_2d"].as_array());
            let quad = b.filter(|b| b.len() == 4).and_then(|b| {
                let n: Vec<f32> = b.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
                (n.len() == 4).then(|| [n[0], n[1], n[2], n[3]])
            });
            let translation = l["translation"].as_str().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
            Some((text, quad, translation))
        })
        .collect();
    let units = resolve_units(units, raw.iter().filter_map(|(_, q, _)| *q), size);
    let mut out = OcrResult::default();
    let mut texts = Vec::new();
    let mut translations = Vec::new();
    for (text, quad, translation) in raw {
        let bounds = quad.and_then(|q| to_rect(q, units, size));
        let mut line = OcrBlock::new(OcrLevel::Line, text.clone());
        line.bounds = bounds;
        texts.push(text.clone());
        let block = match translation {
            Some(t) => {
                translations.push(t.clone());
                line.translation = Some(t.clone());
                let mut block = OcrBlock::new(OcrLevel::Block, text);
                block.bounds = bounds;
                block.translation = Some(t);
                block.children.push(line);
                block
            }
            None => line,
        };
        out.blocks.push(block);
    }
    out.text = texts.join("\n");
    if !translations.is_empty() {
        out.translation = Some(translations.join("\n"));
    }
    Some(out)
}

/// What `auto` means for these boxes: fractions when nothing passes 1,
/// 0–1000 when something passes the image, pixels otherwise.
fn resolve_units(units: Units, quads: impl Iterator<Item = [f32; 4]>, size: Option<(u32, u32)>) -> Units {
    if units != Units::Auto {
        return units;
    }
    let (mut max_x, mut max_y) = (0f32, 0f32);
    let mut any = false;
    for q in quads {
        any = true;
        max_x = max_x.max(q[0]).max(q[2]);
        max_y = max_y.max(q[1]).max(q[3]);
    }
    if !any {
        return Units::Pixels;
    }
    if max_x <= 1.0 && max_y <= 1.0 {
        return Units::Fraction;
    }
    match size {
        Some((w, h)) if (max_x > w as f32 * 1.05 || max_y > h as f32 * 1.05) && max_x <= 1000.0 && max_y <= 1000.0 => {
            Units::Thousand
        }
        _ => Units::Pixels,
    }
}

/// A box in the image's pixels, clamped to it.
fn to_rect(q: [f32; 4], units: Units, size: Option<(u32, u32)>) -> Option<Rect> {
    let (w, h) = size.map(|(w, h)| (w as f32, h as f32)).unwrap_or((0.0, 0.0));
    let [x1, y1, x2, y2] = match units {
        Units::ThousandYx => [q[1], q[0], q[3], q[2]],
        _ => q,
    };
    let scale = |v: f32, full: f32| match units {
        Units::Thousand | Units::ThousandYx => v / 1000.0 * full,
        Units::Fraction => v * full,
        Units::Pixels | Units::Auto => v,
    };
    if units != Units::Pixels && units != Units::Auto && size.is_none() {
        return None;
    }
    let (mut l, mut r) = (scale(x1, w).min(scale(x2, w)), scale(x1, w).max(scale(x2, w)));
    let (mut t, mut b) = (scale(y1, h).min(scale(y2, h)), scale(y1, h).max(scale(y2, h)));
    if size.is_some() {
        l = l.clamp(0.0, w);
        r = r.clamp(0.0, w);
        t = t.clamp(0.0, h);
        b = b.clamp(0.0, h);
    }
    (r > l && b > t).then(|| Rect::new(l, t, r - l, b - t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::{body_json, header, opts};
    use glossa_core::ocr::ImageSource;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::Provider;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    /// A 2x1 PNG header is all the size reader needs.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    fn answer(content: &str) -> String {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] }).to_string()
    }

    #[test]
    fn the_picture_goes_to_the_vision_model_and_text_comes_back() {
        let url = "http://localhost:8000/v1/chat/completions";
        let http = MockHttp::new().on(Method::Post, url, 200, &answer("Hello\nworld"));
        let o = opts(&[
            ("base_url", json!("http://localhost:8000/v1")),
            ("model", json!("text-model")),
            ("ocr_model", json!("Qwen/Qwen2.5-VL-7B-Instruct")),
            ("api_key", json!("vllm-key")),
        ]);
        let mut req = OcrRequest::new(ImageSource::Bytes(png(640, 480)));
        req.lang = LangId::ENGLISH;
        let out = with_test_host(&http, &o, |h| OpenAi::new().recognize(h, &req)).unwrap();
        assert_eq!(out.text, "Hello\nworld");
        assert_eq!(out.image_size, Some((640, 480)));
        assert_eq!(out.provider_id, "openai");
        let r = &http.requests()[0];
        assert_eq!(header(r, "authorization"), Some("Bearer vllm-key"));
        let b = body_json(r);
        assert_eq!(b["model"], "Qwen/Qwen2.5-VL-7B-Instruct");
        assert_eq!(b["temperature"], 0);
        assert!(b.get("reasoning_effort").is_none());
        let parts = &b["messages"][0]["content"];
        assert!(parts[0]["text"].as_str().unwrap().contains("in English"));
        assert!(parts[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn gemini_is_asked_for_its_own_boxes_and_read_that_way() {
        assert_eq!(box_format("gemini-2.5-flash", Units::Auto), BoxFormat::Gemini);
        assert_eq!(box_format("models/Gemini-3.8-flash", Units::ThousandYx), BoxFormat::Gemini);
        assert_eq!(box_format("gemini-2.5-flash", Units::Pixels), BoxFormat::Pixels);
        assert_eq!(box_format("Qwen2.5-VL-7B", Units::Auto), BoxFormat::Pixels);

        // What gemini-2.5-flash answered for a 1800x800 test page, whatever
        // it was asked: y first, 0-1000.
        let url = "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions";
        let reply = answer(
            r#"```json
{"lines": [{"text": "The quick brown fox", "box_2d": [326, 66, 401, 665]}]}
```"#,
        );
        let http = MockHttp::new().on(Method::Post, url, 200, &reply);
        let o = opts(&[
            ("base_url", json!("https://generativelanguage.googleapis.com/v1beta/openai")),
            ("ocr_model", json!("gemini-2.5-flash")),
            ("ocr_mode", json!("lines")),
        ]);
        let req = OcrRequest::new(ImageSource::Bytes(png(1800, 800)));
        let out = with_test_host(&http, &o, |h| OpenAi::new().recognize(h, &req)).unwrap();
        let prompt = body_json(&http.requests()[0])["messages"][0]["content"][0]["text"].as_str().unwrap().to_owned();
        assert!(prompt.contains("box_2d") && prompt.contains("0-1000"), "{prompt}");
        let b = out.blocks[0].bounds.unwrap();
        // The line is drawn at x 120..1190, y 265..315 on the page.
        assert!((b.x - 118.8).abs() < 0.1 && (b.y - 260.8).abs() < 0.1, "{b:?}");
        assert!((b.x + b.width - 1197.0).abs() < 0.1 && (b.y + b.height - 320.8).abs() < 0.1, "{b:?}");
    }

    #[test]
    fn a_reasoning_effort_is_sent_instead_of_a_temperature() {
        let url = "https://api.openai.com/v1/chat/completions";
        let http = MockHttp::new().on(Method::Post, url, 200, &answer("text"));
        let o = opts(&[("reasoning_effort", json!("minimal"))]);
        let req = OcrRequest::new(ImageSource::Bytes(png(10, 10)));
        with_test_host(&http, &o, |h| OpenAi::new().recognize(h, &req)).unwrap();
        let b = body_json(&http.requests()[0]);
        assert_eq!(b["reasoning_effort"], "minimal");
        assert!(b.get("temperature").is_none());
    }

    #[test]
    fn in_place_translation_asks_for_translated_lines() {
        let url = "https://api.openai.com/v1/chat/completions";
        let reply = answer(
            r#"```json
{"lines":[{"text":"Hello","box":[10,10,110,40],"translation":"Привет"}]}
```"#,
        );
        let http = MockHttp::new().on(Method::Post, url, 200, &reply);
        let mut req = OcrRequest::new(ImageSource::Bytes(png(200, 100)));
        req.granularity = OcrGranularity::Blocks;
        req.translate_to = Some(LangId::RUSSIAN);
        let out = with_test_host(&http, &opts(&[]), |h| OpenAi::new().recognize(h, &req)).unwrap();
        let prompt = body_json(&http.requests()[0])["messages"][0]["content"][0]["text"].as_str().unwrap().to_owned();
        assert!(prompt.contains("200x100") && prompt.contains("into Russian"), "{prompt}");
        assert_eq!(out.translation.as_deref(), Some("Привет"));
        assert_eq!(out.blocks[0].bounds, Some(Rect::new(10.0, 10.0, 100.0, 30.0)));
    }

    #[test]
    fn json_is_found_inside_prose_and_fences() {
        let v = first_object("Sure! ```json\n{\"lines\": [{\"text\": \"a } b\"}]}\n``` done").unwrap();
        assert_eq!(v["lines"][0]["text"], "a } b");
        assert!(first_object("no json here").is_none());
    }

    #[test]
    fn plain_text_and_translated_text() {
        assert_eq!(parse_text("```\nHello\nworld\n```", false).text, "Hello\nworld");
        let r = parse_text("{\"text\": \"Hello\", \"translation\": \"Привет\"}", true);
        assert_eq!((r.text.as_str(), r.translation.as_deref()), ("Hello", Some("Привет")));
        // A model that ignored the format still gives its text.
        assert_eq!(parse_text("Hello", true).text, "Hello");
    }

    #[test]
    fn lines_get_boxes_in_pixels_whatever_the_units() {
        let size = Some((2000, 1000));
        let pixels = r#"{"lines":[{"text":"Title","box":[100,50,900,120]},{"text":"Body","box":[100,200,1500,260]}]}"#;
        let r = parse_lines(pixels, size, Units::Auto).unwrap();
        assert_eq!(r.text, "Title\nBody");
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(100.0, 50.0, 800.0, 70.0)));
        assert_eq!(r.blocks[0].level, Some(OcrLevel::Line));

        let fractions = r#"{"lines":[{"text":"A","box":[0.1,0.2,0.5,0.3]}]}"#;
        let b = parse_lines(fractions, size, Units::Auto).unwrap().blocks[0].bounds.unwrap();
        assert!((b.x - 200.0).abs() < 0.01 && (b.y - 200.0).abs() < 0.01 && (b.width - 800.0).abs() < 0.01);

        // Past the 600 px height: 0–1000.
        let thousand = r#"{"lines":[{"text":"A","box":[100,500,500,900]}]}"#;
        let b = parse_lines(thousand, Some((800, 600)), Units::Auto).unwrap().blocks[0].bounds.unwrap();
        assert_eq!(b, Rect::new(80.0, 300.0, 320.0, 240.0));

        // Gemini: y first.
        let yx = r#"{"lines":[{"text":"A","box_2d":[500,100,900,500]}]}"#;
        let b = parse_lines(yx, Some((800, 600)), Units::ThousandYx).unwrap().blocks[0].bounds.unwrap();
        assert_eq!(b, Rect::new(80.0, 300.0, 320.0, 240.0));
    }

    #[test]
    fn translated_lines_take_a_providers_own_shape() {
        let s = r#"{"lines":[{"text":"Hello","box":[0,0,50,10],"translation":"Привет"}]}"#;
        let r = parse_lines(s, Some((100, 100)), Units::Auto).unwrap();
        assert_eq!(r.translation.as_deref(), Some("Привет"));
        let b = &r.blocks[0];
        assert_eq!((b.level, b.translation.as_deref()), (Some(OcrLevel::Block), Some("Привет")));
        assert_eq!(b.children[0].translation.as_deref(), Some("Привет"));
        assert!(b.children[0].bounds.is_some());
    }

    #[test]
    fn boxes_are_clamped_and_empty_ones_dropped() {
        let s = r#"{"lines":[{"text":"A","box":[-5,-5,150,20]},{"text":"B","box":[10,10,10,10]},{"text":"C"}]}"#;
        let r = parse_lines(s, Some((100, 100)), Units::Pixels).unwrap();
        assert_eq!(r.blocks[0].bounds, Some(Rect::new(0.0, 0.0, 100.0, 20.0)));
        assert_eq!(r.blocks[1].bounds, None);
        assert_eq!(r.blocks[2].bounds, None);
        assert_eq!(r.text, "A\nB\nC");
    }

    #[test]
    fn images_are_sent_with_their_type() {
        assert_eq!(mime(&[0xff, 0xd8, 0xff]), "image/jpeg");
        assert_eq!(mime(b"\x89PNG\r\n"), "image/png");
        assert_eq!(mime(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
    }
}
