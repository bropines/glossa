//! The provider API.
//!
//! A *provider* is anything that can translate, detect a language, look a
//! word up, speak, or read an image. Built-in services implement [`Provider`]
//! directly in Rust; every module runtime (JavaScript, Python, native) wraps a
//! module in a type that implements it too, so the engine sees one kind of
//! thing no matter where it came from.
//!
//! A provider gets a [`Host`] with each call: the HTTP client (already scoped
//! to the module's network permissions), its option values, and a cancel
//! flag. It never reaches for globals, which is what lets the same provider be
//! exercised in tests against canned responses.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use glossa_core::{
    Article, Audio, Capabilities, Detection, DictRequest, Error, LangId, LangPair, OcrRequest, OcrResult, ProviderInfo,
    Result, SpeakRequest, TranslateRequest, Translation,
};
use glossa_net::Http;
use serde::{Deserialize, Serialize};

/// Everything a provider may use during one call.
pub struct Host<'a> {
    pub http: &'a dyn Http,
    /// This provider's option values: manifest defaults overlaid with what the
    /// user set.
    pub options: &'a OptionValues,
    /// The interface language, for services that localise part-of-speech
    /// labels and the like (`hl=` on Google).
    pub ui_language: LangId,
    /// The Google domain the user prefers (`com`, `ru`, …).
    pub preferred_domain: &'a str,
    pub cancel: &'a CancelToken,
}

impl Host<'_> {
    pub fn option(&self, key: &str) -> Option<&serde_json::Value> {
        self.options.get(key)
    }

    /// An option as text; empty if unset or not a string.
    pub fn option_str(&self, key: &str) -> &str {
        self.options.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }

    pub fn option_bool(&self, key: &str) -> bool {
        self.options.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn option_f64(&self, key: &str) -> Option<f64> {
        self.options.get(key).and_then(|v| v.as_f64())
    }

    /// An option that must be filled in, or the error the UI turns into "set
    /// this in the service's settings".
    pub fn require(&self, key: &str, provider: &str) -> Result<&str> {
        let v = self.option_str(key).trim();
        if v.is_empty() {
            Err(Error::MissingCredential { provider: provider.to_owned(), what: key.to_owned() })
        } else {
            Ok(v)
        }
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub type OptionValues = BTreeMap<String, serde_json::Value>;

/// Set when the user cancels or a newer request supersedes this one. A
/// provider checks it between steps; the engine drops a cancelled job's result
/// whether or not the provider noticed.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A provider. Every operation defaults to "unsupported"; implement those the
/// [`ProviderInfo::capabilities`] claim.
pub trait Provider: Send + Sync {
    fn info(&self) -> &ProviderInfo;

    /// The settings this provider takes, for the options page. The page is
    /// generated from these, so a provider never ships UI code.
    fn options(&self) -> &[OptionSpec] {
        &[]
    }

    /// The settings page in the interface language `lang`: [`Provider::options`]
    /// laid out and translated, with conditions and checks compiled. Built-in
    /// providers get their options in order; modules add `[ui.settings]` and
    /// their locale files.
    fn settings_page(&self, lang: &str) -> Arc<crate::settings::SettingsPage> {
        let _ = lang;
        Arc::new(crate::settings::SettingsPage::plain(self.options()))
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        let _ = (host, req);
        Err(self.unsupported("translation"))
    }

    /// Translate, reporting the translation so far through `progress` as it
    /// arrives (each call carries the whole text up to that point, not a
    /// delta). For services that answer piece by piece, such as LLMs; the
    /// default just translates and reports nothing.
    fn translate_stream(&self, host: &Host, req: &TranslateRequest, progress: &dyn Fn(&str)) -> Result<Translation> {
        let _ = progress;
        self.translate(host, req)
    }

    /// The models the service offers, for a picker on the settings page.
    fn list_models(&self, host: &Host) -> Result<Vec<String>> {
        let _ = host;
        Err(self.unsupported("listing models"))
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        let _ = (host, text);
        Err(self.unsupported("language detection"))
    }

    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        let _ = (host, req);
        Err(self.unsupported("dictionary lookup"))
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        let _ = (host, req);
        Err(self.unsupported("speech"))
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        let _ = (host, req);
        Err(self.unsupported("text recognition"))
    }

    /// The same query on the service's website, for "open in browser".
    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        let _ = (text, pair);
        None
    }

    /// Release resources (a child process, a warm connection). Called when
    /// the module is disabled or the program idles; the provider must cope
    /// with being called again afterwards.
    fn suspend(&self) {}

    /// Get ready for a call that is about to come (the user is choosing a
    /// screen region to recognise), for engines whose first call is slow.
    /// Blocks; the engine calls it on a worker thread. Must be cheap when
    /// there is nothing to do, as it may be called before every call.
    fn warm_up(&self) {}

    #[doc(hidden)]
    fn unsupported(&self, what: &'static str) -> Error {
        Error::Unsupported { provider: self.info().name.clone(), capability: what }
    }

    /// Whether this is already a [`Guarded`] wrapper, so wrapping twice is
    /// avoided. Providers never override it.
    #[doc(hidden)]
    fn is_guarded(&self) -> bool {
        false
    }
}

/// A provider behind a panic barrier. JavaScript, classic and pot-app
/// scripts, native modules, the image code and the built-in services all run
/// in this process; a panic in any of them (a bug in the JavaScript engine, an out-of-range
/// index in a parser) fails that one call with [`Error::Module`] instead of
/// taking the program down. The [`Registry`](crate::Registry) wraps every
/// provider it holds in one, so callers never see an unguarded provider.
pub struct Guarded {
    inner: Arc<dyn Provider>,
}

impl Guarded {
    /// Wrap `inner`, unless it is already wrapped.
    pub fn wrap(inner: Arc<dyn Provider>) -> Arc<dyn Provider> {
        if inner.is_guarded() {
            inner
        } else {
            Arc::new(Guarded { inner })
        }
    }

    fn call<T>(&self, op: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(r) => r,
            Err(payload) => {
                let id = &self.inner.info().id;
                let msg = glossa_core::error::panic_message(payload.as_ref());
                tracing::error!(provider = %id, "{op} panicked: {msg}");
                Err(Error::module(id.clone(), format!("{op} panicked: {msg}")))
            }
        }
    }
}

impl Provider for Guarded {
    fn info(&self) -> &ProviderInfo {
        self.inner.info()
    }

    fn options(&self) -> &[OptionSpec] {
        self.inner.options()
    }

    fn settings_page(&self, lang: &str) -> Arc<crate::settings::SettingsPage> {
        self.inner.settings_page(lang)
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        self.call("translation", || self.inner.translate(host, req))
    }

    fn translate_stream(&self, host: &Host, req: &TranslateRequest, progress: &dyn Fn(&str)) -> Result<Translation> {
        self.call("translation", || self.inner.translate_stream(host, req, progress))
    }

    fn list_models(&self, host: &Host) -> Result<Vec<String>> {
        self.call("listing models", || self.inner.list_models(host))
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        self.call("language detection", || self.inner.detect(host, text))
    }

    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        self.call("dictionary lookup", || self.inner.dictionary(host, req))
    }

    fn speak(&self, host: &Host, req: &SpeakRequest) -> Result<Audio> {
        self.call("speech", || self.inner.speak(host, req))
    }

    fn recognize(&self, host: &Host, req: &OcrRequest) -> Result<OcrResult> {
        self.call("text recognition", || self.inner.recognize(host, req))
    }

    fn web_link(&self, text: &str, pair: LangPair) -> Option<String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.inner.web_link(text, pair))).ok().flatten()
    }

    fn suspend(&self) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.inner.suspend())).is_err() {
            tracing::error!(provider = %self.inner.info().id, "suspend panicked");
        }
    }

    fn is_guarded(&self) -> bool {
        true
    }
}

/// One setting a provider takes. Mirrors the `[[options]]` table of a module
/// manifest; built-in providers build the same struct in code.
///
/// The first block of fields is the original format. The rest were added
/// with richer settings pages (the declarative-modules RFC, phase 1); all
/// are optional and skipped when empty, so old manifests read the same and
/// a serialised spec only carries what was set. What they mean at run time
/// lives in [`crate::settings`]: conditions in `settings::condition`, checks
/// in `settings::validate`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionSpec {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(rename = "type", default)]
    pub kind: OptionKind,
    #[serde(default)]
    pub default: serde_json::Value,
    #[serde(default)]
    pub help: String,
    /// The provider cannot work without it.
    #[serde(default)]
    pub required: bool,
    /// For `select` and `multiselect`: the allowed values and their labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,

    // -- validation -----------------------------------------------------------
    /// A regular expression the value must match (`regex-lite` syntax). It
    /// matches anywhere unless anchored, so write `^...$` for the whole value.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// Shown under the field when `pattern` does not match, instead of the
    /// generic message.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern_message: String,
    /// Text length in characters (for `multiselect`: how many are picked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,

    // -- presentation ---------------------------------------------------------
    /// `number`: the drag or slider step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Another control for the type: `"slider"` for a `number` with `min` and
    /// `max`. Empty for the type's usual one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub widget: String,
    /// `lang`: offer "Detect language" (`auto`) as a choice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_auto: bool,
    /// `file`: extensions the file dialog offers, without the dot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,

    // -- layout ---------------------------------------------------------------
    /// A [condition](crate::settings::condition): show the field only while
    /// it holds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub visible_if: String,
    /// A condition: the field is greyed out unless it holds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub enabled_if: String,
    /// Put the field into the settings-page group with this id, creating the
    /// group when `[ui.settings]` does not declare it. Shorthand for listing
    /// the key in the group's `children`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
}

impl OptionSpec {
    pub fn text(key: &str, label: &str, default: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: OptionKind::Text,
            default: serde_json::Value::String(default.into()),
            ..Self::default()
        }
    }

    pub fn secret(key: &str, label: &str) -> Self {
        Self { kind: OptionKind::Secret, ..Self::text(key, label, "") }
    }

    pub fn url(key: &str, label: &str, default: &str) -> Self {
        Self { kind: OptionKind::Url, ..Self::text(key, label, default) }
    }

    pub fn boolean(key: &str, label: &str, default: bool) -> Self {
        Self { kind: OptionKind::Bool, default: serde_json::Value::Bool(default), ..Self::text(key, label, "") }
    }

    pub fn number(key: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        Self {
            kind: OptionKind::Number,
            default: serde_json::json!(default),
            min: Some(min),
            max: Some(max),
            ..Self::text(key, label, "")
        }
    }

    pub fn multiline(key: &str, label: &str, default: &str) -> Self {
        Self { kind: OptionKind::Multiline, ..Self::text(key, label, default) }
    }

    pub fn select(key: &str, label: &str, default: &str, choices: &[(&str, &str)]) -> Self {
        Self {
            kind: OptionKind::Select,
            choices: choices.iter().map(|(v, l)| Choice { value: (*v).into(), label: (*l).into() }).collect(),
            ..Self::text(key, label, default)
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
        self
    }

    pub fn placeholder(mut self, p: &str) -> Self {
        self.placeholder = p.into();
        self
    }
}

/// An option's `type`, written in the manifest as a lowercase name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionKind {
    #[default]
    Text,
    /// Shown masked; a key or a password.
    Secret,
    Url,
    Bool,
    Number,
    Select,
    Multiline,
    /// Several of `choices`; the value is a list of strings.
    Multiselect,
    /// A language code, picked with Glossa's language picker.
    Lang,
    /// A path, typed or picked with a file dialog. The module gets the path,
    /// not the file.
    File,
    /// A type from a newer format, with its name as written. The module
    /// still loads, the settings page says the field needs a newer Glossa,
    /// and the name is written back unchanged (the module editor rewrites
    /// manifests).
    Unknown(String),
}

impl OptionKind {
    const NAMES: [(&'static str, OptionKind); 10] = [
        ("text", OptionKind::Text),
        ("secret", OptionKind::Secret),
        ("url", OptionKind::Url),
        ("bool", OptionKind::Bool),
        ("number", OptionKind::Number),
        ("select", OptionKind::Select),
        ("multiline", OptionKind::Multiline),
        ("multiselect", OptionKind::Multiselect),
        ("lang", OptionKind::Lang),
        ("file", OptionKind::File),
    ];

    /// The name as `module.toml` writes it.
    pub fn name(&self) -> &str {
        match self {
            OptionKind::Unknown(name) => name,
            known => Self::NAMES.iter().find(|(_, k)| k == known).map_or("text", |(n, _)| n),
        }
    }

    pub fn from_name(name: &str) -> OptionKind {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or_else(|| OptionKind::Unknown(name.to_owned()), |(_, k)| k.clone())
    }

    /// Whether the value is a string the user types.
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            OptionKind::Text | OptionKind::Secret | OptionKind::Url | OptionKind::Multiline | OptionKind::File
        )
    }
}

impl Serialize for OptionKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for OptionKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(OptionKind::from_name(&String::deserialize(d)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    #[serde(default)]
    pub label: String,
}

/// Overlay user values on the declared defaults.
pub fn resolve_options(specs: &[OptionSpec], user: Option<&OptionValues>) -> OptionValues {
    let mut out: OptionValues =
        specs.iter().filter(|s| !s.default.is_null()).map(|s| (s.key.clone(), s.default.clone())).collect();
    if let Some(user) = user {
        for (k, v) in user {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

type TranslateFn = Box<dyn Fn(&Host, &TranslateRequest) -> Result<Translation> + Send + Sync>;
type DetectFn = Box<dyn Fn(&Host, &str) -> Result<Detection> + Send + Sync>;

/// A provider made of closures, for tests and for tiny built-ins.
pub struct FnProvider {
    pub info: ProviderInfo,
    pub translate: Option<TranslateFn>,
    pub detect: Option<DetectFn>,
}

impl FnProvider {
    pub fn translator(
        id: &str,
        f: impl Fn(&Host, &TranslateRequest) -> Result<Translation> + Send + Sync + 'static,
    ) -> Self {
        Self { info: ProviderInfo::new(id, id, Capabilities::TRANSLATE), translate: Some(Box::new(f)), detect: None }
    }
}

impl Provider for FnProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn translate(&self, host: &Host, req: &TranslateRequest) -> Result<Translation> {
        match &self.translate {
            Some(f) => f(host, req),
            None => Err(self.unsupported("translation")),
        }
    }

    fn detect(&self, host: &Host, text: &str) -> Result<Detection> {
        match &self.detect {
            Some(f) => f(host, text),
            None => Err(self.unsupported("language detection")),
        }
    }
}

/// Build a throwaway [`Host`] around a test double.
#[doc(hidden)]
pub fn with_test_host<T>(http: &dyn Http, options: &OptionValues, f: impl FnOnce(&Host) -> T) -> T {
    let cancel = CancelToken::new();
    let host = Host { http, options, ui_language: LangId::ENGLISH, preferred_domain: "com", cancel: &cancel };
    f(&host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;

    #[test]
    fn user_options_overlay_defaults() {
        let specs = vec![OptionSpec::url("url", "URL", "https://default"), OptionSpec::secret("key", "Key")];
        let mut user = OptionValues::new();
        user.insert("key".into(), serde_json::json!("abc"));
        let resolved = resolve_options(&specs, Some(&user));
        assert_eq!(resolved["url"], "https://default");
        assert_eq!(resolved["key"], "abc");
    }

    #[test]
    fn require_reports_the_missing_key() {
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| match host.require("api_key", "DeepL") {
            Err(Error::MissingCredential { provider, what }) => {
                assert_eq!(provider, "DeepL");
                assert_eq!(what, "api_key");
            }
            other => panic!("{other:?}"),
        });
    }

    #[test]
    fn unsupported_operations_say_so() {
        let p = FnProvider::translator("t", |_, req| Ok(Translation::plain(req.text.to_uppercase())));
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| {
            let out = p.translate(host, &TranslateRequest::new("hi", LangPair::new(LangId::AUTO, LangId::RUSSIAN)));
            assert_eq!(out.unwrap().text, "HI");
            assert!(matches!(p.detect(host, "x"), Err(Error::Unsupported { .. })));
        });
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn option_specs_deserialise_from_manifest_toml() {
        #[derive(Deserialize)]
        struct M {
            options: Vec<OptionSpec>,
        }
        let m: M = toml::from_str(
            r#"
            [[options]]
            key = "api_key"
            type = "secret"
            label = "API key"
            required = true

            [[options]]
            key = "model"
            type = "select"
            default = "fast"
            choices = [ { value = "fast", label = "Fast" }, { value = "good" } ]
            "#,
        )
        .unwrap();
        assert_eq!(m.options[0].kind, OptionKind::Secret);
        assert!(m.options[0].required);
        assert_eq!(m.options[1].choices.len(), 2);
        assert_eq!(m.options[1].default, "fast");
    }
}
