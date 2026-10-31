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

        let entry = engine::call_global(&mut ctx, "__glossaEntry", &[JsValue::from(boa_engine::JsString::from(method))])
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
            let t = p.translate(host, &TranslateRequest::new("world", LangPair::new(LangId::AUTO, LangId::RUSSIAN))).unwrap();
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
            let t = p.translate(host, &TranslateRequest::new("abc", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN))).unwrap();
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
