//! pot-app plugins.
//!
//! pot-app was a popular cross-platform translator whose plugin catalogue
//! outlived it: the project was archived in 2026 with hundreds of plugins
//! still in circulation. A plugin is `info.json` (identity, settings, a
//! language-code table) and `main.js` (an async `translate`, `recognize` or
//! `tts` function). This module loads that pair, synthesises a manifest from
//! `info.json`, and runs `main.js` against a compatibility layer (`pot.js`)
//! that provides the Tauri-flavoured utilities plugins expect.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use boa_engine::JsValue;
use glossa_module_sdk::wire::{ErrorKind, ModuleError, Reply};
use serde::Deserialize;

use super::engine::{self, Bridge};
use crate::manifest::{Manifest, ModuleSection, Permissions, Runtime};
use crate::protocol::Transport;
use crate::provider::{Choice, OptionKind, OptionSpec};

pub const POT_JS: &str = include_str!("pot.js");

#[derive(Debug, Deserialize)]
struct Info {
    id: String,
    #[serde(default)]
    display: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    needs: Vec<Need>,
    #[serde(default)]
    language: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Need {
    key: String,
    #[serde(default)]
    display: String,
    #[serde(default, rename = "type")]
    kind: String,
    /// For `select`: value -> label.
    #[serde(default)]
    options: serde_json::Map<String, serde_json::Value>,
}

pub struct PotPlugin {
    pub dir: PathBuf,
    pub main: String,
    pub manifest: Manifest,
    pub kind: PotKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PotKind {
    Translate,
    Recognize,
    Tts,
}

impl PotPlugin {
    pub fn from_dir(dir: &Path) -> Result<PotPlugin, String> {
        let info_text = std::fs::read_to_string(dir.join("info.json")).map_err(|e| format!("info.json: {e}"))?;
        let info: Info =
            serde_json::from_str(info_text.trim_start_matches('\u{feff}')).map_err(|e| format!("info.json: {e}"))?;
        let main = std::fs::read_to_string(dir.join("main.js")).map_err(|e| format!("main.js: {e}"))?;
        let kind = probe_kind(&main)?;

        let mut language_map = std::collections::BTreeMap::new();
        let mut languages = Vec::new();
        for (pot_code, theirs) in &info.language {
            let Some(theirs) = theirs.as_str() else { continue };
            let Some(ours) = pot_to_bcp47(pot_code) else { continue };
            if !language_map.contains_key(ours) {
                language_map.insert(ours.to_owned(), theirs.to_owned());
                languages.push(ours.to_owned());
            }
        }

        let capability = match kind {
            PotKind::Translate => "translate",
            PotKind::Recognize => "ocr",
            PotKind::Tts => "listen",
        };
        let id = info.id.to_ascii_lowercase().replace(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)), "-");
        let manifest = Manifest {
            module: ModuleSection {
                id: format!("pot-{}", id.trim_start_matches("plugin.").trim_start_matches("[plugin].")),
                name: if info.display.is_empty() { info.id.clone() } else { info.display.clone() },
                version: String::new(),
                api: crate::PROTOCOL_VERSION,
                runtime: Runtime::Pot,
                entry: "main.js".into(),
                description: "pot-app plugin".into(),
                homepage: info.homepage.clone(),
                author: String::new(),
                license: String::new(),
                max_input_chars: None,
                idle_timeout_secs: None,
                ready_to_use: false,
            },
            capabilities: vec![capability.into()],
            languages,
            language_map,
            // pot-app plugins always had unrestricted network access.
            permissions: Permissions::allow_all(),
            options: info.needs.iter().map(need_to_option).collect(),
        };
        manifest.validate()?;
        Ok(PotPlugin { dir: dir.to_owned(), main, manifest, kind })
    }
}

fn need_to_option(n: &Need) -> OptionSpec {
    let label = if n.display.is_empty() { n.key.clone() } else { n.display.clone() };
    let mut spec = OptionSpec::text(&n.key, &label, "");
    match n.kind.as_str() {
        "select" => {
            spec.kind = OptionKind::Select;
            spec.choices = n
                .options
                .iter()
                .map(|(value, label)| Choice {
                    value: value.clone(),
                    label: label.as_str().unwrap_or(value).to_owned(),
                })
                .collect();
            if let Some(first) = spec.choices.first() {
                spec.default = serde_json::Value::String(first.value.clone());
            }
        }
        _ => {
            let k = n.key.to_ascii_lowercase();
            if k.contains("key") || k.contains("secret") || k.contains("token") || k.contains("password") {
                spec.kind = OptionKind::Secret;
            }
        }
    }
    spec
}

/// Which entry point the plugin defines.
fn probe_kind(main: &str) -> Result<PotKind, String> {
    let mut ctx = engine::new_context()?;
    engine::eval(&mut ctx, "var __potOsType = 'Windows_NT';", "os")?;
    engine::eval(&mut ctx, POT_JS, "pot.js")?;
    engine::eval(&mut ctx, main, "main.js")?;
    if engine::has_function(&mut ctx, "translate") {
        Ok(PotKind::Translate)
    } else if engine::has_function(&mut ctx, "recognize") {
        Ok(PotKind::Recognize)
    } else if engine::has_function(&mut ctx, "tts") {
        Ok(PotKind::Tts)
    } else {
        Err("main.js defines none of translate, recognize or tts".into())
    }
}

/// pot-app's own language keys, to BCP 47.
fn pot_to_bcp47(code: &str) -> Option<&'static str> {
    Some(match code {
        "auto" => "auto",
        "zh_cn" => "zh-Hans",
        "zh_tw" => "zh-Hant",
        "pt_pt" | "pt_br" => "pt",
        "mn_cy" | "mn_mo" => "mn",
        "nb_no" | "nn_no" => "nb",
        "he" | "iw" => "he",
        other => {
            let id = glossa_core::LangId::from_code(other)?;
            if !id.is_language() {
                return None;
            }
            id.bcp47()
        }
    })
}

pub struct PotTransport {
    pub plugin: Arc<PotPlugin>,
}

impl PotTransport {
    fn local(&self, method: &str, params: &serde_json::Value) -> Reply {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");
        // Plugins may read only their own files.
        let target = self.plugin.dir.join(path);
        let inside = target
            .canonicalize()
            .ok()
            .zip(self.plugin.dir.canonicalize().ok())
            .is_some_and(|(t, root)| t.starts_with(root));
        if !inside {
            return Reply::err(ModuleError::new(
                ErrorKind::BadRequest,
                format!("{path}: outside the plugin directory"),
            ));
        }
        match method {
            "local:read_text" => match std::fs::read_to_string(&target) {
                Ok(s) => Reply::ok(s.into()),
                Err(e) => Reply::err(ModuleError::other(e.to_string())),
            },
            "local:read_binary" => match std::fs::read(&target) {
                Ok(b) => Reply::ok(base64::engine::general_purpose::STANDARD.encode(b).into()),
                Err(e) => Reply::err(ModuleError::other(e.to_string())),
            },
            _ => Reply::err(ModuleError::unsupported(method)),
        }
    }
}

impl Transport for PotTransport {
    fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        host: &dyn Fn(&str, serde_json::Value) -> Reply,
    ) -> Result<serde_json::Value, ModuleError> {
        let routed =
            |m: &str, p: serde_json::Value| if m.starts_with("local:") { self.local(m, &p) } else { host(m, p) };
        let _bridge = Bridge::install(&routed);

        let mut ctx = engine::new_context().map_err(ModuleError::other)?;
        let os = if cfg!(windows) {
            "Windows_NT"
        } else if cfg!(target_os = "macos") {
            "Darwin"
        } else {
            "Linux"
        };
        engine::eval(&mut ctx, &format!("var __potOsType = '{os}';"), "os").map_err(ModuleError::other)?;
        engine::eval(&mut ctx, POT_JS, "pot.js").map_err(ModuleError::other)?;
        engine::eval(&mut ctx, &self.plugin.main, "main.js").map_err(engine::script_error)?;

        let config = params.get("options").cloned().unwrap_or_else(|| serde_json::json!({}));
        let s = |k: &str| params.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let options = |ctx: &mut boa_engine::Context, detect: &str| -> Result<JsValue, ModuleError> {
            let config_js = engine::from_json(ctx, &config).map_err(ModuleError::other)?;
            let build = engine::get_global(ctx, "__potOptions");
            build
                .as_callable()
                .ok_or_else(|| ModuleError::other("pot.js is missing __potOptions"))?
                .call(&JsValue::undefined(), &[config_js, JsValue::from(js(detect))], ctx)
                .map_err(|e| ModuleError::other(engine::describe(&e, ctx)))
        };

        match (method, self.plugin.kind) {
            ("translate", PotKind::Translate) => {
                let (text, from, to) = (s("text"), s("from"), s("to"));
                let opts = options(&mut ctx, &from)?;
                let out = engine::call_global(
                    &mut ctx,
                    "translate",
                    &[JsValue::from(js(&text)), JsValue::from(js(&from)), JsValue::from(js(&to)), opts],
                )
                .map_err(engine::script_error)?;
                let mut value = engine::to_json(&mut ctx, &out).map_err(ModuleError::other)?;
                if value.is_null() {
                    // Streaming plugins report through setResult and return
                    // nothing.
                    let last = engine::get_global(&mut ctx, "__potResult");
                    value = engine::to_json(&mut ctx, &last).map_err(ModuleError::other)?;
                }
                translate_result(value)
            }
            ("recognize", PotKind::Recognize) => {
                let lang = s("lang");
                let opts = options(&mut ctx, &lang)?;
                let out = engine::call_global(
                    &mut ctx,
                    "recognize",
                    &[JsValue::from(js(&s("image_base64"))), JsValue::from(js(&lang)), opts],
                )
                .map_err(engine::script_error)?;
                let text = engine::to_string(&mut ctx, &out);
                if text.trim().is_empty() {
                    return Err(ModuleError::new(ErrorKind::NotFound, "no text recognised"));
                }
                Ok(serde_json::json!({ "text": text }))
            }
            ("speak", PotKind::Tts) => {
                let lang = s("lang");
                let opts = options(&mut ctx, &lang)?;
                let out = engine::call_global(
                    &mut ctx,
                    "tts",
                    &[JsValue::from(js(&s("text"))), JsValue::from(js(&lang)), opts],
                )
                .map_err(engine::script_error)?;
                let value = engine::to_json(&mut ctx, &out).map_err(ModuleError::other)?;
                let bytes: Vec<u8> = match value {
                    serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_u64()).map(|b| b as u8).collect(),
                    serde_json::Value::String(b64) => {
                        base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                if bytes.is_empty() {
                    return Err(ModuleError::new(ErrorKind::BadResponse, "the plugin returned no audio"));
                }
                Ok(serde_json::json!({
                    "audio_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    "mime": if bytes.starts_with(b"RIFF") { "audio/wav" } else { "audio/mpeg" },
                }))
            }
            (other, _) => Err(ModuleError::unsupported(other)),
        }
    }
}

/// A pot translate plugin returns either a string or a dictionary-shaped
/// object (`pronunciations`, `explanations`, `associations`, `sentence`).
fn translate_result(value: serde_json::Value) -> Result<serde_json::Value, ModuleError> {
    match value {
        serde_json::Value::String(s) if !s.trim().is_empty() => Ok(serde_json::json!({ "text": s })),
        serde_json::Value::Object(o) => {
            let mut text = String::new();
            let mut parts = Vec::new();
            for e in o.get("explanations").and_then(|v| v.as_array()).into_iter().flatten() {
                let pos = e.get("trait").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                let defs: Vec<String> = e
                    .get("explains")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                if !defs.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    if !pos.is_empty() {
                        text.push_str(&pos);
                        text.push(' ');
                    }
                    text.push_str(&defs.join("; "));
                    parts.push(serde_json::json!({
                        "part_of_speech": pos,
                        "senses": defs.iter().map(|d| serde_json::json!({ "word": d })).collect::<Vec<_>>(),
                    }));
                }
            }
            let romanization = o
                .get("pronunciations")
                .and_then(|v| v.as_array())
                .and_then(|a| a.iter().find_map(|p| p.get("symbol").and_then(|s| s.as_str()).map(str::to_owned)));
            if text.is_empty() {
                return Err(ModuleError::new(ErrorKind::BadResponse, "the plugin returned an empty result"));
            }
            Ok(serde_json::json!({ "text": text, "parts_of_speech": parts, "romanization": romanization }))
        }
        _ => Err(ModuleError::new(ErrorKind::BadResponse, "the plugin returned no translation")),
    }
}

fn js(s: &str) -> boa_engine::JsString {
    boa_engine::JsString::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_api::ModuleCache;
    use crate::protocol::ProtocolProvider;
    use crate::provider::{with_test_host, OptionValues, Provider};
    use glossa_core::{LangId, LangPair, TranslateRequest};
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    /// A plugin in pot-app's template style: JSON body, query params,
    /// CryptoJS signing, a select setting.
    const MAIN: &str = r#"
        async function translate(text, from, to, options) {
            const { config, utils } = options;
            const { http, CryptoJS } = utils;
            const { fetch, Body } = http;
            const sign = CryptoJS.MD5(config.appid + text + "salt").toString();
            const res = await fetch("https://api.pot.test/translate", {
                method: "POST",
                query: { sign: sign },
                headers: { "X-Mode": config.mode },
                body: Body.json({ q: text, from: from, to: to })
            });
            if (!res.ok) throw `Http Request Error\nHttp Status: ${res.status}`;
            return res.data.result + "|" + from + "|" + to;
        }
    "#;

    const INFO: &str = r#"{
        "id": "plugin.com.example.test",
        "display": "Test Plugin",
        "homepage": "https://example.com",
        "icon": "icon.png",
        "needs": [
            { "key": "appid", "display": "App ID", "type": "input" },
            { "key": "secret_key", "display": "Secret", "type": "input" },
            { "key": "mode", "display": "Mode", "type": "select", "options": { "fast": "Fast", "good": "Good" } }
        ],
        "language": { "auto": "auto", "zh_cn": "zh", "en": "en", "ru": "ru" }
    }"#;

    fn plugin_dir(main: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glossa-pot-{}-{}", std::process::id(), main.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("info.json"), INFO).unwrap();
        std::fs::write(dir.join("main.js"), main).unwrap();
        dir
    }

    fn provider(main: &str) -> ProtocolProvider<PotTransport> {
        let plugin = PotPlugin::from_dir(&plugin_dir(main)).unwrap();
        let manifest = plugin.manifest.clone();
        ProtocolProvider::new(manifest, PotTransport { plugin: Arc::new(plugin) }, ModuleCache::new())
    }

    #[test]
    fn info_json_becomes_a_manifest() {
        let p = provider(MAIN);
        let info = p.info();
        assert_eq!(info.id, "pot-com.example.test");
        assert_eq!(info.name, "Test Plugin");
        assert!(info.supports_language(LangId::from_code("zh-CN").unwrap()));
        assert!(info.supports_language(LangId::AUTO));
        assert!(!info.supports_language(LangId::from_code("de").unwrap()));
        let opts = p.options();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[1].kind, OptionKind::Secret, "secret-looking keys are masked");
        assert_eq!(opts[2].kind, OptionKind::Select);
        assert_eq!(opts[2].default, "fast");
    }

    #[test]
    fn a_template_style_plugin_translates() {
        let p = provider(MAIN);
        let http = MockHttp::new().on(Method::Post, "https://api.pot.test/translate", 200, r#"{"result":"你好"}"#);
        let mut opts = OptionValues::new();
        opts.insert("appid".into(), serde_json::json!("A1"));
        opts.insert("mode".into(), serde_json::json!("good"));
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(
                    host,
                    &TranslateRequest::new(
                        "hello",
                        LangPair::new(LangId::ENGLISH, LangId::from_code("zh-CN").unwrap()),
                    ),
                )
                .unwrap();
            assert_eq!(t.text, "你好|en|zh", "codes went out in the plugin's own spelling");
        });
        let req = &http.requests()[0];
        // md5("A1hellosalt")
        assert!(req.url.ends_with("?sign=60bcaf2982247773186516d342777da2"), "{}", req.url);
        assert!(req.headers.iter().any(|(k, v)| k == "X-Mode" && v == "good"));
        assert!(req.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
        let body: serde_json::Value = serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["q"], "hello");
    }

    #[test]
    fn cryptojs_matches_reference_values() {
        let main = r#"
            async function translate(text, from, to, options) {
                const C = options.utils.CryptoJS;
                return [
                    C.MD5("abc").toString(),
                    C.SHA256("abc").toString(C.enc.Base64),
                    C.HmacSHA1("data", "key").toString(),
                    C.enc.Base64.stringify(C.enc.Utf8.parse("привет")),
                ].join(",");
            }
        "#;
        let p = provider(main);
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let t = p
                .translate(host, &TranslateRequest::new("x", LangPair::new(LangId::ENGLISH, LangId::RUSSIAN)))
                .unwrap();
            let parts: Vec<&str> = t.text.split(',').collect();
            assert_eq!(parts[0], "900150983cd24fb0d6963f7d28e17f72");
            assert_eq!(parts[1], "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=");
            assert_eq!(parts[2], "104152c5bfdca07bc633eebd46199f0255c9f49d");
            assert_eq!(parts[3], "0L/RgNC40LLQtdGC");
        });
    }

    #[test]
    fn dictionary_shaped_results_and_set_result_streaming() {
        let main = r#"
            async function translate(text, from, to, options) {
                if (text === "stream") { options.setResult("partial"); options.setResult("final"); return; }
                return { pronunciations: [{ region: "US", symbol: "rʌn" }],
                         explanations: [{ trait: "v.", explains: ["бежать", "работать"] }] };
            }
        "#;
        let p = provider(main);
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
            let d = p.translate(host, &TranslateRequest::new("run", pair)).unwrap();
            assert_eq!(d.text, "v. бежать; работать");
            assert_eq!(d.romanisation.as_deref(), Some("rʌn"));
            assert_eq!(d.parts_of_speech[0].senses.len(), 2);
            let s = p.translate(host, &TranslateRequest::new("stream", pair)).unwrap();
            assert_eq!(s.text, "final");
        });
    }

    #[test]
    fn plugins_cannot_read_outside_their_directory() {
        let main = r#"
            async function translate(text, from, to, options) {
                return await options.utils.readTextFile(text);
            }
        "#;
        let p = provider(main);
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let pair = LangPair::new(LangId::ENGLISH, LangId::RUSSIAN);
            let ok = p.translate(host, &TranslateRequest::new("info.json", pair)).unwrap();
            assert!(ok.text.contains("plugin.com.example.test"));
            let denied = p.translate(host, &TranslateRequest::new("../../../etc/passwd", pair)).unwrap_err();
            assert!(denied.to_string().contains("outside the plugin directory"), "{denied}");
        });
    }

    #[test]
    fn pot_language_codes_map_to_ours() {
        assert_eq!(pot_to_bcp47("zh_cn"), Some("zh-Hans"));
        assert_eq!(pot_to_bcp47("zh_tw"), Some("zh-Hant"));
        assert_eq!(pot_to_bcp47("pt_br"), Some("pt"));
        assert_eq!(pot_to_bcp47("ru"), Some("ru"));
        assert_eq!(pot_to_bcp47("yue"), None, "Cantonese has no slot of its own");
    }
}
