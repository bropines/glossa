//! `paddle-ocr`: PaddleOCR (PP-OCRv5) text recognition as a Glossa process
//! module.
//!
//! Glossa starts it on the first OCR call and talks JSON lines over stdin
//! and stdout (docs/modules/process-modules.md). It implements `recognize`
//! and answers every other method `unsupported`. ONNX Runtime and the
//! models load on the first request and stay until stdin closes; Glossa
//! closes it after `idle_timeout_secs`, so an idle Glossa holds none of it.
//!
//! Outside Glossa it reads one picture and prints the result, for trying
//! and timing it:
//!
//! ```text
//! paddle-ocr --image shot.png [--lang ru] [--models DIR] [--threads N] [--max-side PX]
//! ```

mod engine;
mod onnx_meta;
mod scripts;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use serde_json::{json, Value};

use engine::{Engine, Line, ModelPaths};
use scripts::{Script, Settings};

const DET: &str = "det.onnx";
const CLS: &str = "cls.onnx";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("paddle-ocr {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", include_str!("usage.txt"));
        return;
    }
    if args.iter().any(|a| a == "--image") {
        std::process::exit(match once(&args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("paddle-ocr: {e}");
                1
            }
        });
    }
    serve();
}

// -- the module protocol ----------------------------------------------------------

/// A module error: `kind` is one of the protocol's (docs/modules/protocol.md).
struct Fail {
    kind: &'static str,
    message: String,
}

impl Fail {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

struct Io {
    input: io::StdinLock<'static>,
    out: io::Stdout,
    next_host_id: u64,
}

/// One line from stdin.
enum Input {
    Text(String),
    /// Not UTF-8; decoded lossily, only to find its id.
    Garbled(String),
}

impl Io {
    /// The next line, or `None` at the end of input (or a read error,
    /// which means Glossa is gone).
    fn next_line(&mut self) -> Option<Input> {
        let mut buf = Vec::new();
        match self.input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    buf.pop();
                }
                Some(match String::from_utf8(buf) {
                    Ok(s) => Input::Text(s),
                    Err(e) => Input::Garbled(String::from_utf8_lossy(e.as_bytes()).into_owned()),
                })
            }
        }
    }

    fn send(&mut self, v: &Value) {
        let mut out = self.out.lock();
        // A closed stdout means Glossa is gone; the read loop ends next.
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    /// A host call: ask, then read until the answer with our id comes.
    /// Glossa sends nothing else while a request is open.
    fn host(&mut self, method: &str, params: Value) -> Result<Value, Fail> {
        self.next_host_id += 1;
        let id = format!("h{}", self.next_host_id);
        self.send(&json!({"id": id, "method": method, "params": params}));
        while let Some(line) = self.next_line() {
            let Input::Text(line) = line else {
                eprintln!("paddle-ocr: a line that is not UTF-8 while waiting for {method}; skipped");
                continue;
            };
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                let text = err.get("message").and_then(Value::as_str).unwrap_or("host call failed");
                return Err(Fail::new("other", format!("{method}: {text}")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(Fail::new("other", "stdin closed during a host call"))
    }
}

fn serve() {
    let mut io = Io { input: io::stdin().lock(), out: io::stdout(), next_host_id: 0 };
    let mut engine = Engine::new();
    let mut ort_ready = false;
    while let Some(line) = io.next_line() {
        let line = match line {
            Input::Text(line) => line,
            // A request that is not UTF-8 gets an error if its id can be
            // read, and is logged otherwise; either way the next line is
            // read, as for any other line the module cannot use.
            Input::Garbled(lossy) => {
                match serde_json::from_str::<Value>(&lossy).ok().and_then(|m| m.get("id").cloned()) {
                    Some(id) => io.send(&json!({"id": id, "error": {
                        "kind": "bad_request", "message": "the request is not valid UTF-8"}})),
                    None => eprintln!("paddle-ocr: a line that is not UTF-8; skipped"),
                }
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            eprintln!("paddle-ocr: not JSON: {line}");
            continue;
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let answer = match method {
            "recognize" => {
                // A panic in the inference code would otherwise end the
                // process with the request unanswered.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    recognize(&mut io, &mut engine, &mut ort_ready, &params)
                }));
                match res {
                    Ok(r) => r,
                    Err(_) => {
                        engine = Engine::new();
                        Err(Fail::new("other", "the recogniser crashed on this picture"))
                    }
                }
            }
            other => Err(Fail::new("unsupported", format!("paddle-ocr only recognises text, not {other:?}"))),
        };
        let reply = match answer {
            Ok(result) => json!({"id": id, "result": result}),
            Err(f) => json!({"id": id, "error": {"kind": f.kind, "message": f.message}}),
        };
        io.send(&reply);
    }
}

fn recognize(io: &mut Io, engine: &mut Engine, ort_ready: &mut bool, params: &Value) -> Result<Value, Fail> {
    let started = Instant::now();
    let options = params.get("options").cloned().unwrap_or(Value::Null);
    let settings = Settings::from_options(&options);
    let lang = params.get("lang").and_then(Value::as_str).unwrap_or("auto");

    let files = asset_files(io)?;
    let path_of = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone());
    let det = path_of(DET).ok_or_else(|| Fail::new("other", format!("{DET} is not downloaded")))?;
    let paths = ModelPaths { det, cls: path_of(CLS) };
    let module_id = std::env::var("GLOSSA_MODULE_ID").unwrap_or_else(|_| "paddle-ocr".into());
    let chosen = scripts::choose(lang, &settings, &|s: &Script| path_of(&s.asset()).is_some()).map_err(|m| {
        let kind = if m.language.is_some() { "source_language" } else { "other" };
        Fail::new(kind, m.message(&module_id))
    })?;
    let recs: Vec<(Script, PathBuf)> =
        chosen.iter().map(|s| (*s, path_of(&s.asset()).expect("chosen from the downloaded ones"))).collect();

    let image = decode(params)?;
    let size = image.dimensions();
    if !*ort_ready {
        init_ort().map_err(|e| Fail::new("other", e))?;
        *ort_ready = true;
    }
    let lines = engine.recognize(image, &paths, &recs, &settings).map_err(|e| Fail::new("other", e))?;
    eprintln!(
        "paddle-ocr: {}x{} with {}: {} lines in {} ms",
        size.0,
        size.1,
        chosen.iter().map(|s| s.id).collect::<Vec<_>>().join("+"),
        lines.len(),
        started.elapsed().as_millis()
    );
    if lines.is_empty() {
        return Err(Fail::new("not_found", "no text found in the picture"));
    }
    Ok(result_json(&lines, size))
}

/// The module's files by name. Required ones also come in the environment,
/// but the optional recognisers only through the `assets` host call.
fn asset_files(io: &mut Io) -> Result<Vec<(String, PathBuf)>, Fail> {
    match io.host("assets", Value::Null) {
        Ok(v) => Ok(v
            .get("files")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), PathBuf::from(v.as_str()?)))).collect())
            .unwrap_or_default()),
        // A Glossa without the call: whatever is in the assets folder.
        Err(_) => Ok(std::env::var_os("GLOSSA_ASSETS_DIR").map(|d| files_in(Path::new(&d))).unwrap_or_default()),
    }
}

fn files_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut names = vec![DET.to_owned(), CLS.to_owned()];
    names.extend(scripts::ALL.iter().map(Script::asset));
    names.into_iter().map(|n| (n.clone(), dir.join(n))).filter(|(_, p)| p.is_file()).collect()
}

/// The limits Glossa's own image API keeps (`glossa_modules::image_ops`):
/// a picture over them is refused before its pixels are allocated.
const MAX_PIXELS: u64 = 100_000_000;
const MAX_SIDE: u32 = 50_000;
const MAX_ENCODED: usize = 64 << 20;

fn decode(params: &Value) -> Result<image::RgbImage, Fail> {
    let b64 = params
        .get("image_base64")
        .and_then(Value::as_str)
        .ok_or_else(|| Fail::new("bad_request", "no image_base64 in the request"))?;
    if b64.len() / 4 * 3 > MAX_ENCODED {
        return Err(Fail::new("bad_request", format!("the picture is over {} MB", MAX_ENCODED >> 20)));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| Fail::new("bad_request", format!("image_base64 is not base64: {e}")))?;
    let unreadable = |e: &dyn std::fmt::Display| Fail::new("bad_request", format!("unreadable picture: {e}"));
    let reader = || image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format();
    // The size from the header first, so a small file claiming a huge
    // picture never gets its pixels allocated.
    let (w, h) = reader().map_err(|e| unreadable(&e))?.into_dimensions().map_err(|e| unreadable(&e))?;
    check_size(w, h)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    // RGBA at the pixel limit, with room for the decoder's own buffers.
    limits.max_alloc = Some(MAX_PIXELS * 4 + (64 << 20));
    let mut r = reader().map_err(|e| unreadable(&e))?;
    r.limits(limits);
    Ok(r.decode().map_err(|e| unreadable(&e))?.into_rgb8())
}

fn check_size(w: u32, h: u32) -> Result<(), Fail> {
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Fail::new(
            "bad_request",
            format!(
                "the picture is {w}x{h}; at most {MAX_SIDE} pixels a side and {} megapixels",
                MAX_PIXELS / 1_000_000
            ),
        ));
    }
    Ok(())
}

/// The OCR result format of docs/modules/ocr-modules.md: lines with their
/// outlines; Glossa derives the boxes and angles from the four corners.
fn result_json(lines: &[Line], size: (u32, u32)) -> Value {
    // Round in f64: an f32 widened later prints as 0.9879999756813049.
    let round = |v: f32, scale: f64| (f64::from(v) * scale).round() / scale;
    let blocks: Vec<Value> = lines
        .iter()
        .map(|l| {
            json!({
                "level": "line",
                "text": l.text,
                "polygon": l.polygon.iter().map(|p| [round(p[0], 10.0), round(p[1], 10.0)]).collect::<Vec<_>>(),
                "confidence": round(l.confidence, 1000.0),
            })
        })
        .collect();
    json!({
        "text": lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"),
        "image_size": [size.0, size.1],
        "blocks": blocks,
    })
}

// -- ONNX Runtime -----------------------------------------------------------------

#[cfg(windows)]
const ORT_LIB: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const ORT_LIB: &str = "libonnxruntime.dylib";
#[cfg(not(any(windows, target_os = "macos")))]
const ORT_LIB: &str = "libonnxruntime.so";

/// Load ONNX Runtime: `ORT_DYLIB_PATH` when set, else the library beside
/// this binary, where the module's archive puts it.
fn init_ort() -> Result<(), String> {
    let path = match std::env::var_os("ORT_DYLIB_PATH") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe()
            .map_err(|e| format!("finding the module's folder: {e}"))?
            .parent()
            .map(|d| d.join(ORT_LIB))
            .ok_or("the binary has no folder")?,
    };
    if !path.is_file() {
        return Err(format!("ONNX Runtime not found at {}; download the module's files again", path.display()));
    }
    let builder = ort::init_from(&path).map_err(|e| format!("loading ONNX Runtime from {}: {e}", path.display()))?;
    builder.with_name("paddle-ocr").commit();
    Ok(())
}

// -- one picture from the command line ------------------------------------------

fn once(args: &[String]) -> Result<(), String> {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let image_path = value("--image").ok_or("--image needs a file")?;
    let lang = value("--lang").unwrap_or_else(|| "auto".into());
    let dir = value("--models")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("GLOSSA_ASSETS_DIR").map(PathBuf::from))
        .ok_or("--models DIR (or GLOSSA_ASSETS_DIR) names the folder with det.onnx and the recognisers")?;
    let mut options = serde_json::Map::new();
    if let Some(t) = value("--threads") {
        options.insert("threads".into(), json!(t));
    }
    if let Some(m) = value("--max-side") {
        options.insert("max_side".into(), json!(m));
    }
    let settings = Settings::from_options(&Value::Object(options));
    let files = files_in(&dir);
    let path_of = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone());
    let paths = ModelPaths { det: path_of(DET).ok_or(format!("no {DET} in {}", dir.display()))?, cls: path_of(CLS) };
    let chosen = scripts::choose(&lang, &settings, &|s: &Script| path_of(&s.asset()).is_some())
        .map_err(|m| m.message("paddle-ocr"))?;
    let recs: Vec<(Script, PathBuf)> = chosen.iter().map(|s| (*s, path_of(&s.asset()).unwrap())).collect();

    let image = image::open(&image_path).map_err(|e| format!("{image_path}: {e}"))?.into_rgb8();
    let size = image.dimensions();
    let t0 = Instant::now();
    init_ort()?;
    let mut engine = Engine::new();
    let lines = engine.recognize(image.clone(), &paths, &recs, &settings)?;
    let cold = t0.elapsed();
    let t1 = Instant::now();
    let again = engine.recognize(image, &paths, &recs, &settings)?;
    let warm = t1.elapsed();
    debug_assert_eq!(lines.len(), again.len());
    println!("{}", serde_json::to_string_pretty(&result_json(&lines, size)).unwrap_or_default());
    eprintln!(
        "paddle-ocr: {}x{}, recognisers {}: {} lines; first run (loading included) {} ms, second {} ms",
        size.0,
        size.1,
        chosen.iter().map(|s| s.id).collect::<Vec<_>>().join("+"),
        lines.len(),
        cold.as_millis(),
        warm.as_millis()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_is_lines_with_outlines() {
        let lines = vec![
            Line {
                text: "Hello".into(),
                polygon: [[1.04, 2.0], [50.0, 2.0], [50.0, 20.0], [1.0, 20.0]],
                confidence: 0.98765,
            },
            Line {
                text: "Привет".into(),
                polygon: [[1.0, 30.0], [60.0, 30.0], [60.0, 48.0], [1.0, 48.0]],
                confidence: 0.9,
            },
        ];
        let v = result_json(&lines, (100, 50));
        assert_eq!(v["text"], "Hello\nПривет");
        assert_eq!(v["image_size"], json!([100, 50]));
        assert_eq!(v["blocks"][0]["level"], "line");
        assert_eq!(v["blocks"][0]["polygon"][0], json!([1.0, 2.0]));
        assert_eq!(v["blocks"][0]["confidence"], json!(0.988));
        assert_eq!(v["blocks"][1]["text"], "Привет");
    }

    #[test]
    fn bad_pictures_are_bad_requests() {
        assert_eq!(decode(&json!({})).err().map(|f| f.kind), Some("bad_request"));
        assert_eq!(decode(&json!({"image_base64": "%%%"})).err().map(|f| f.kind), Some("bad_request"));
        assert_eq!(decode(&json!({"image_base64": "aGVsbG8="})).err().map(|f| f.kind), Some("bad_request"));
    }

    #[test]
    fn pictures_over_the_limits_are_refused_before_decoding() {
        assert!(check_size(1920, 1080).is_ok());
        assert_eq!(check_size(50_001, 10).err().map(|f| f.kind), Some("bad_request"));
        assert_eq!(check_size(20_000, 20_000).err().map(|f| f.kind), Some("bad_request"));
        // A tiny PNG whose header claims 60000x60000 pixels: refused from the
        // header, without allocating them.
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(1, 1))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let err = decode(&json!({"image_base64": b64})).err().unwrap();
        assert_eq!(err.kind, "bad_request");
        assert!(err.message.contains("60000x60000"), "{}", err.message);
    }

    /// PNG's chunk checksum, so the doctored header above stays valid.
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    #[test]
    fn a_png_decodes() {
        let img = image::RgbImage::from_pixel(3, 2, image::Rgb([1, 2, 3]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(decode(&json!({"image_base64": b64})).ok().map(|i| i.dimensions()), Some((3, 2)));
    }

    /// The whole pipeline on a real picture. Needs the models and ONNX
    /// Runtime: `PADDLE_OCR_MODELS=<dir with det.onnx, cls.onnx, rec-*.onnx>
    /// ORT_DYLIB_PATH=<libonnxruntime> PADDLE_OCR_IMAGE=<png> cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_recognition() {
        let dir = PathBuf::from(std::env::var("PADDLE_OCR_MODELS").expect("PADDLE_OCR_MODELS"));
        let image = image::open(std::env::var("PADDLE_OCR_IMAGE").expect("PADDLE_OCR_IMAGE")).unwrap().into_rgb8();
        init_ort().unwrap();
        let files = files_in(&dir);
        let path_of = |name: &str| files.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone());
        let paths = ModelPaths { det: path_of(DET).unwrap(), cls: path_of(CLS) };
        let settings = Settings::from_options(&Value::Null);
        let chosen = scripts::choose("auto", &settings, &|s: &Script| path_of(&s.asset()).is_some()).unwrap();
        let recs: Vec<(Script, PathBuf)> = chosen.iter().map(|s| (*s, path_of(&s.asset()).unwrap())).collect();
        let lines = Engine::new().recognize(image, &paths, &recs, &settings).unwrap();
        assert!(!lines.is_empty());
        for l in &lines {
            eprintln!("{:.3} {}", l.confidence, l.text);
        }
    }
}
