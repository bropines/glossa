//! Glossa's own JavaScript modules.
//!
//! A module is `module.toml` plus a script that defines `translate`,
//! `detect`, `dictionary`, `speak` or `recognize`. Requests and results are
//! the module protocol's JSON, the same a Python or native module sees, so
//! the three runtimes are interchangeable from the engine's point of view and
//! a module can be ported between them without reshaping its data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};

use super::engine::{self, Bridge};
use crate::protocol::Transport;

pub const GLOSSA_JS: &str = include_str!("glossa.js");

pub struct GlossaScript {
    pub path: PathBuf,
    pub script: engine::Script,
}

impl GlossaScript {
    pub fn new(path: PathBuf, source: &str) -> GlossaScript {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        GlossaScript { path, script: engine::Script::new(name, source.trim_start_matches('\u{feff}')) }
    }

    pub fn from_dir(dir: &Path, entry: &str) -> Result<GlossaScript, String> {
        let entry = if entry.is_empty() { "main.js" } else { entry };
        let path = dir.join(entry);
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(GlossaScript::new(path, &source))
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
        let value = engine::with_context(|ctx| -> Result<serde_json::Value, ModuleError> {
            engine::shared::WEB.eval(ctx).map_err(ModuleError::other)?;
            engine::shared::GLOSSA.eval(ctx).map_err(ModuleError::other)?;
            let options = params.get("options").cloned().unwrap_or_else(|| serde_json::json!({}));
            engine::set_global_json(ctx, "__options", &options).map_err(ModuleError::other)?;
            engine::eval(ctx, "host.options = __options;", "options").map_err(ModuleError::other)?;
            self.script.script.eval(ctx).map_err(engine::script_error)?;

            let entry =
                engine::call_global(ctx, "__glossaEntry", &[engine::str(ctx, method)]).map_err(ModuleError::other)?;
            if !entry.is_function() {
                return Err(ModuleError::unsupported(method));
            }
            let req = engine::from_json(ctx, &params).map_err(ModuleError::other)?;
            let host_obj = engine::get_global(ctx, "host");
            let out = engine::call(ctx, &entry, &[req, host_obj])
                .map_err(|e| engine::script_error(format!("{method}: {e}")))?;
            engine::to_json(ctx, &out).map_err(ModuleError::other)
        })
        .map_err(ModuleError::other)??;
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
            GlossaTransport { script: Arc::new(GlossaScript::new("main.js".into(), source)) },
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
    fn the_browser_fetch_and_url_work_for_bundled_libraries() {
        // The shape esbuild gives a bundle built with `--format=cjs`:
        // `module.exports` replaced by an object of getters.
        let p = provider(
            r#"
            var __defProp = Object.defineProperty;
            var src_exports = {};
            __defProp(src_exports, "translate", { get: () => translate, enumerable: true });
            module.exports = src_exports;
            async function translate(req) {
                const url = new URL("/v1/t?x=1", "https://api.test/base/");
                url.searchParams.set("q", req.text);
                url.searchParams.append("to", req.to);
                const body = new URLSearchParams({ a: "b c", d: "é&" });
                const r = await fetch(url, { method: "POST", body, headers: { "X-Key": "k" } });
                if (!r.ok || !r.headers.get("Content-Type").startsWith("application/json")) throw new Error("bad");
                const data = await r.json();
                return data.out + " " + url.host + url.pathname + " " + body;
            }
            "#,
            "\"api.test\"",
        );
        let http = MockHttp::new().on(Method::Post, "https://api.test/v1/t", 200, r#"{"out":"мир"}"#);
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("a b", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
                .unwrap();
            assert_eq!(t.text, "мир api.test/v1/t a=b+c&d=%C3%A9%26");
        });
        let req = &http.requests()[0];
        assert_eq!(req.url, "https://api.test/v1/t?x=1&q=a+b&to=ru");
        assert!(req.headers.iter().any(|(k, v)| k == "x-key" && v == "k"), "{:?}", req.headers);
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v.starts_with("application/x-www-form-urlencoded")));
    }

    #[test]
    fn url_and_search_params_follow_the_common_cases() {
        let p = provider(
            r#"
            function translate() {
                const out = [];
                const u = new URL("HTTPS://user:pw@Example.COM:443/a/./b/../c?x=1&y=2#frag");
                out.push(u.href, u.origin, u.port, u.search, u.hash, u.searchParams.get("y"));
                out.push(new URL("../d?z", "https://h.test/a/b/c").href);
                out.push(new URL("//cdn.test/x", "https://h.test/").href);
                out.push(new URL("http://h.test:8080").href);
                const sp = new URLSearchParams("?a=1&b=2&a=3&c=%20+");
                out.push(sp.getAll("a").join(","), sp.get("c"), String(sp.has("z")), sp.size);
                sp.delete("a"); sp.sort();
                out.push(sp.toString(), [...sp.keys()].join(","));
                out.push(String(URL.canParse("not a url")));
                const h = new Headers([["A", "1"], ["a", "2"]]);
                out.push(h.get("A"));
                return out.join("|");
            }
            "#,
            "",
        );
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
                .unwrap();
            let parts: Vec<&str> = t.text.split('|').collect();
            assert_eq!(
                parts,
                [
                    "https://user:pw@example.com/a/c?x=1&y=2#frag",
                    "https://example.com",
                    "",
                    "?x=1&y=2",
                    "#frag",
                    "2",
                    "https://h.test/a/d?z",
                    "https://cdn.test/x",
                    "http://h.test:8080/",
                    "1,3",
                    "  ",
                    "false",
                    "4",
                    "b=2&c=++",
                    "b,c",
                    "false",
                    "1, 2",
                ]
            );
        });
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
