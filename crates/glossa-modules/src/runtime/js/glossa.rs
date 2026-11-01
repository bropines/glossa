//! Glossa's own JavaScript modules.
//!
//! A module is `module.toml` plus a script that defines `translate`,
//! `detect`, `dictionary`, `speak` or `recognize`. Requests and results are
//! the module protocol's JSON, the same a Python or native module sees, so
//! the three runtimes are interchangeable from the engine's point of view and
//! a module can be ported between them without reshaping its data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use boa_engine::JsValue;
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};

use super::engine::{self, Bridge};
use crate::protocol::Transport;

pub const GLOSSA_JS: &str = include_str!("glossa.js");

pub struct GlossaScript {
    pub path: PathBuf,
    pub source: String,
}

impl GlossaScript {
    pub fn from_dir(dir: &Path, entry: &str) -> Result<GlossaScript, String> {
        let entry = if entry.is_empty() { "main.js" } else { entry };
        let path = dir.join(entry);
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(GlossaScript { path, source: source.trim_start_matches('\u{feff}').to_owned() })
    }
}

pub struct GlossaTransport {
    pub script: Arc<GlossaScript>,
}

impl Transport for GlossaTransport {
    fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
    ) -> Result<serde_json::Value, ModuleError> {
        let _bridge = Bridge::install(host);
        let mut ctx = engine::new_context().map_err(ModuleError::other)?;
        engine::eval(&mut ctx, GLOSSA_JS, "glossa.js").map_err(ModuleError::other)?;
        let options = params.get("options").cloned().unwrap_or_else(|| serde_json::json!({}));
        engine::set_global_json(&mut ctx, "__options", &options).map_err(ModuleError::other)?;
        engine::eval(&mut ctx, "host.options = __options;", "options").map_err(ModuleError::other)?;
        let name = self.script.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        engine::eval(&mut ctx, &self.script.source, &name).map_err(engine::script_error)?;

        let entry =
            engine::call_global(&mut ctx, "__glossaEntry", &[JsValue::from(boa_engine::JsString::from(method))])
                .map_err(ModuleError::other)?;
        let Some(f) = entry.as_callable() else {
            return Err(ModuleError::unsupported(method));
        };
        let req = engine::from_json(&mut ctx, &params).map_err(ModuleError::other)?;
        let host_obj = engine::get_global(&mut ctx, "host");
        let out = f
            .call(&JsValue::undefined(), &[req, host_obj], &mut ctx)
            .map_err(|e| engine::script_error(format!("{method}: {}", engine::describe(&e, &mut ctx))))?;
        let out = engine::settle(&mut ctx, out).map_err(|e| engine::script_error(format!("{method}: {e}")))?;
        let value = engine::to_json(&mut ctx, &out).map_err(ModuleError::other)?;
        match (method, value) {
            ("translate", serde_json::Value::String(s)) => Ok(serde_json::json!({ "text": s })),
            (_, serde_json::Value::Null) => {
                Err(ModuleError::new(ErrorKind::BadResponse, format!("{method} returned nothing")))
            }
            (_, v) => Ok(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_api::ModuleCache;
    use crate::manifest::Manifest;
    use crate::protocol::ProtocolProvider;
    use crate::provider::{with_test_host, OptionValues, Provider};
    use glossa_core::{LangId, LangPair, TranslateRequest};
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    fn provider(source: &str, network: &str) -> ProtocolProvider<GlossaTransport> {
        let manifest = Manifest::parse(&format!(
            r#"
            capabilities = ["translate", "detect_language"]
            [module]
            id = "js-test"
            name = "JS Test"
            runtime = "js"
            [permissions]
            network = [{network}]
            "#
        ))
        .unwrap();
        ProtocolProvider::new(
            manifest,
            GlossaTransport { script: Arc::new(GlossaScript { path: "main.js".into(), source: source.into() }) },
            ModuleCache::new(),
        )
    }

    #[test]
    fn a_module_with_sync_fetch_translates() {
        let p = provider(
            r#"
            function translate(req, host) {
                var r = host.fetch("https://api.test/t?q=" + encodeURIComponent(req.text) + "&to=" + req.to,
                                   { headers: { "X-Key": host.options.key } });
                if (!r.ok) throw new ModuleError("http", "status " + r.status);
                return { text: r.json().out, detected_from: "en" };
            }
            "#,
            "\"api.test\"",
        );
        let http = MockHttp::new().on(Method::Get, "https://api.test/t", 200, r#"{"out":"мир"}"#);
        let mut opts = OptionValues::new();
        opts.insert("key".into(), serde_json::json!("secret"));
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("world", LangPair::new(LangId::AUTO, LangId::RUSSIAN)))
                .unwrap();
            assert_eq!(t.text, "мир");
            assert_eq!(t.detected_from, Some(LangId::ENGLISH));
        });
        let req = &http.requests()[0];
        assert_eq!(req.url, "https://api.test/t?q=world&to=ru");
        assert!(req.headers.iter().any(|(k, v)| k == "X-Key" && v == "secret"));
    }

    #[test]
    fn async_and_module_exports_styles_work() {
        let p = provider(
            r#"
            module.exports.translate = async function (req) {
                await null;
                return req.text.split("").reverse().join("");
            };
            exports.detect = function () { return { lang: "ru", confidence: 0.7 }; };
            "#,
            "",
        );
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("abc", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
                .unwrap();
            assert_eq!(t.text, "cba", "a bare string is accepted for translate");
            let d = p.detect(host, "x").unwrap();
            assert_eq!(d.lang, LangId::RUSSIAN);
            assert!((d.confidence - 0.7).abs() < 1e-6);
        });
    }

    #[test]
    fn typed_errors_and_permissions_reach_the_engine() {
        let p = provider(
            r#"
            function translate(req, host) {
                if (!host.options.key) throw new ModuleError("missing_credential", "key");
                return host.fetch("https://elsewhere.test/").text();
            }
            "#,
            "\"api.test\"",
        );
        let http = MockHttp::new();
        let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
        let empty = OptionValues::new();
        with_test_host(&http, &empty, |host| {
            let e = p.translate(host, &TranslateRequest::new("x", pair)).unwrap_err();
            assert!(matches!(e, glossa_core::Error::MissingCredential { ref what, .. } if what == "key"), "{e:?}");
        });
        let mut with_key = OptionValues::new();
        with_key.insert("key".into(), serde_json::json!("k"));
        with_test_host(&http, &with_key, |host| {
            let e = p.translate(host, &TranslateRequest::new("x", pair)).unwrap_err();
            assert!(e.to_string().contains("permissions"), "{e}");
        });
        assert!(http.requests().is_empty(), "the out-of-scope request never left");
    }

    #[cfg(feature = "image")]
    #[test]
    fn a_recognize_module_preprocesses_with_the_image_api() {
        let p = provider(
            r#"
            function recognize(req, host) {
                var g = host.image.grayscale(req.image_handle);
                var big = host.image.scaleToMinHeight(g, { min_height: 32 });
                var png = host.image.encode(big, { format: "png" });
                if (png.mime !== "image/png" || !png.data_base64) throw new Error("no png");
                var back = host.image.mapBoxBack([0, 0, big.width, big.height], big);
                return { text: big.width + "x" + big.height, image_size: [big.width, big.height],
                         blocks: [{ level: "line", text: "t", bbox: [8, 0, 16, 32] }],
                         transform: big.transform,
                         translation: back.join(",") };
            }
            "#,
            "",
        );
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(16, 8))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let req = glossa_core::OcrRequest::new(glossa_core::ocr::ImageSource::Bytes(bytes.clone()));
            let r = p.recognize(host, &req).unwrap();
            assert_eq!(r.text, "64x32");
            assert_eq!(r.translation.as_deref(), Some("0,0,16,8"), "mapBoxBack undoes the scaling");
            assert_eq!(r.blocks[0].bounds, Some(glossa_core::ocr::Rect::new(2.0, 0.0, 2.0, 8.0)));
            assert_eq!(r.image_size, Some((16, 8)), "the input's size, not the copy's");
        });
    }

    /// The Cloud Vision example against a canned reply: lines are rebuilt
    /// from the breaks, outlines become boxes, and the enlargement of the
    /// small capture is undone.
    #[cfg(feature = "image")]
    #[test]
    fn the_cloud_vision_example_maps_its_layout() {
        // Boa in a debug build needs more than a test thread's 2 MB to turn
        // a result this deep into JSON (release builds need a fraction);
        // engine workers get 8 MB.
        std::thread::Builder::new().stack_size(8 << 20).spawn(cloud_vision_example).unwrap().join().unwrap();
    }

    #[cfg(feature = "image")]
    fn cloud_vision_example() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/js/google-vision-ocr");
        let manifest = crate::manifest::Manifest::load(&dir.join("module.toml")).unwrap();
        let p = super::super::load_glossa(&dir, manifest, &crate::registry::RuntimeContext::default()).unwrap();
        let v = |x: u32, y: u32| serde_json::json!({ "x": x, "y": y });
        let word = |t: &str, x: u32, brk: &str| {
            serde_json::json!({ "boundingBox": { "vertices": [v(x, 0), v(x + 40, 0), v(x + 40, 48), v(x, 48)] },
                "confidence": 0.9,
                "symbols": [{ "text": t, "property": { "detectedBreak": { "type": brk } } }] })
        };
        let reply = serde_json::json!({ "responses": [{ "fullTextAnnotation": {
            "text": "Hi there\nyou\n",
            "pages": [{ "width": 96, "height": 96, "property": { "detectedLanguages": [{ "languageCode": "en" }] },
                "blocks": [{ "boundingBox": { "vertices": [{}, v(96, 0), v(96, 96), v(0, 96)] }, "confidence": 0.8,
                    "paragraphs": [{ "boundingBox": { "vertices": [{}, v(96, 0), v(96, 96), v(0, 96)] },
                        "words": [word("Hi", 0, "SPACE"), word("there", 48, "EOL_SURE_SPACE"), word("you", 0, "LINE_BREAK")] }] }] }]
        } }] });
        let http = MockHttp::new().on(Method::Post, "https://vision.googleapis.com/", 200, &reply.to_string());
        let mut opts = OptionValues::new();
        opts.insert("api_key".into(), serde_json::json!("fake-key"));
        opts.insert("min_height".into(), serde_json::json!(48));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(48, 24))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        with_test_host(&http, &opts, |host| {
            let req = glossa_core::OcrRequest::new(glossa_core::ocr::ImageSource::Bytes(bytes.clone()));
            let r = p.recognize(host, &req).unwrap();
            assert_eq!(r.text, "Hi there\nyou");
            assert_eq!(r.detected_lang, Some(LangId::ENGLISH));
            let para = &r.blocks[0].children[0];
            let lines: Vec<&str> = para.children.iter().map(|l| l.text.as_str()).collect();
            assert_eq!(lines, ["Hi there", "you"]);
            // The capture was doubled to 48 px high; boxes come back halved.
            assert_eq!(r.blocks[0].bounds, Some(glossa_core::ocr::Rect::new(0.0, 0.0, 48.0, 48.0)));
            let hi = &para.children[0].children[0];
            assert_eq!(hi.bounds, Some(glossa_core::ocr::Rect::new(0.0, 0.0, 20.0, 24.0)));
            assert_eq!(hi.confidence, Some(0.9));
            assert_eq!(r.image_size, Some((48, 24)));
        });
        let sent = &http.requests()[0];
        assert!(sent.url.contains("key=fake-key"));
    }

    #[test]
    fn missing_entry_points_are_unsupported_not_crashes() {
        let p = provider("function translate() { return 'x'; }", "");
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            assert!(matches!(p.detect(host, "x"), Err(glossa_core::Error::Unsupported { .. })));
        });
    }
}
