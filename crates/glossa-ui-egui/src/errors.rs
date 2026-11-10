//! A failed request as the windows show it: the message in the interface
//! language, what the service said in small print, and buttons for the way
//! out that fits the kind of failure (try again, the service's settings,
//! the next service).
//!
//! The main window's result pane, the popup and the dictionary all show
//! their failure through [`show`], and carry out what was clicked with
//! [`apply`]; trying again and switching service are theirs to do, since
//! only they know what was asked.

use egui::{Color32, RichText};
use glossa_core::locale::Locale;
use glossa_core::Error;

use crate::app::GlossaApp;

/// A way out of an error, offered as a button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Ask again: the failure may pass by itself.
    Retry,
    /// Open the service's page in Options (a key is missing or refused).
    ServiceSettings,
    /// Download the files the module needs.
    Download { module: String },
    /// Install what a Python module runs on.
    InstallRuntime { module: String },
    /// Open Options → Modules (a module waits for approval).
    Modules,
    /// Ask the next service in the strip instead.
    NextService,
}

/// An error, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorView {
    pub headline: String,
    pub detail: Option<String>,
    pub fixes: Vec<Fix>,
    /// The service or module the error names (a name or an id), to find its
    /// settings by.
    subject: Option<String>,
}

/// The buttons an error of this kind gets, most useful first.
pub fn fixes(e: &Error) -> Vec<Fix> {
    let mut out = Vec::new();
    match e {
        Error::MissingCredential { .. } | Error::HttpStatus { status: 401 | 403, .. } => out.push(Fix::ServiceSettings),
        Error::AssetsMissing { module, .. } => {
            out.push(Fix::Download { module: module.clone() });
            out.push(Fix::ServiceSettings);
        }
        Error::AssetCorrupt { module, .. } => out.push(Fix::Download { module: module.clone() }),
        Error::RuntimeMissing { module, .. } => {
            out.push(Fix::InstallRuntime { module: module.clone() });
            out.push(Fix::ServiceSettings);
        }
        Error::NeedsApproval { .. } => out.push(Fix::Modules),
        Error::BadResponse { .. } | Error::ModuleTimeout { .. } => out.push(Fix::Retry),
        e if e.is_transient() => out.push(Fix::Retry),
        _ => {}
    }
    // Another service is worth a try when this one is what failed, not the
    // request itself (nothing to translate) or the machine (no network).
    let service_at_fault = matches!(
        e,
        Error::Unsupported { .. }
            | Error::SourceLanguageUnsupported { .. }
            | Error::TargetLanguageUnsupported { .. }
            | Error::Timeout(_)
            | Error::BadResponse { .. }
            | Error::HttpStatus { .. }
            | Error::MissingCredential { .. }
            | Error::RateLimited { .. }
            | Error::Module { .. }
            | Error::ModuleAt { .. }
            | Error::ModuleTimeout { .. }
            | Error::AssetsMissing { .. }
            | Error::RuntimeMissing { .. }
            | Error::NeedsApproval { .. }
            | Error::AssetCorrupt { .. }
    );
    if service_at_fault {
        out.push(Fix::NextService);
    }
    out
}

impl ErrorView {
    pub fn new(locale: &Locale, e: &Error) -> Self {
        let subject = match e {
            Error::Unsupported { provider, .. }
            | Error::SourceLanguageUnsupported { provider, .. }
            | Error::TargetLanguageUnsupported { provider, .. }
            | Error::BadResponse { provider, .. }
            | Error::HttpStatus { provider, .. }
            | Error::MissingCredential { provider, .. }
            | Error::RateLimited { provider, .. } => Some(provider.clone()),
            Error::Module { module, .. }
            | Error::ModuleAt { module, .. }
            | Error::ModuleTimeout { module, .. }
            | Error::AssetsMissing { module, .. }
            | Error::RuntimeMissing { module, .. }
            | Error::NeedsApproval { module }
            | Error::AssetCorrupt { module, .. } => Some(module.clone()),
            _ => None,
        };
        ErrorView { headline: locale.describe_error(e), detail: e.detail(), fixes: fixes(e), subject }
    }

    /// A message already in words, with nothing to press.
    pub fn plain(headline: String) -> Self {
        ErrorView { headline, detail: None, fixes: Vec::new(), subject: None }
    }
}

/// How [`show`] draws in one window.
pub struct Style {
    pub font: egui::FontId,
    /// The colour of the small print.
    pub weak: Color32,
    /// Little room (the popup): the small print on one line, cut short,
    /// with the whole of it in a tooltip.
    pub compact: bool,
}

/// Draw the error. `next` is the name of the service "try the next one"
/// would ask, when there is another. Returns the button clicked.
pub fn show(ui: &mut egui::Ui, app: &GlossaApp, view: &ErrorView, style: &Style, next: Option<&str>) -> Option<Fix> {
    let small = egui::FontId::new((style.font.size * 0.85).max(10.0), style.font.family.clone());
    ui.add(egui::Label::new(RichText::new(&view.headline).color(app.look.error()).font(style.font.clone())).wrap());
    if let Some(d) = &view.detail {
        let text = RichText::new(d).font(small.clone()).color(style.weak);
        if style.compact {
            ui.add(egui::Label::new(text).truncate()).on_hover_text(d);
        } else {
            ui.add(egui::Label::new(text).wrap());
        }
    }
    let mut clicked = None;
    let buttons: Vec<(&Fix, String)> = view
        .fixes
        .iter()
        .filter_map(|fix| {
            let label = match fix {
                Fix::Retry => app.t("error.action.retry"),
                Fix::ServiceSettings => app.t("error.action.service_settings"),
                Fix::Download { .. } | Fix::InstallRuntime { .. } => app.t("error.action.download"),
                Fix::Modules => app.t("error.action.modules"),
                Fix::NextService => app.locale.fmt("error.action.next_service", next?),
            };
            Some((fix, label))
        })
        .collect();
    if !buttons.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            for (fix, label) in buttons {
                if ui.add(egui::Button::new(RichText::new(label).font(small.clone())).small()).clicked() {
                    clicked = Some(fix.clone());
                }
            }
        });
    }
    clicked
}

/// Carry out a fix that needs nothing from the window it was clicked in.
/// `asked` is the service the failed request went to. Returns the fix back
/// when it is the window's to do ([`Fix::Retry`], [`Fix::NextService`]).
pub fn apply(app: &mut GlossaApp, view: &ErrorView, fix: Fix, asked: &str) -> Option<Fix> {
    match fix {
        Fix::Retry | Fix::NextService => return Some(fix),
        Fix::ServiceSettings => {
            let id = settings_target(app, view, asked);
            crate::options::open(app);
            crate::options::select_service(app, &id);
        }
        Fix::Modules => crate::options::open_at(app, "modules"),
        Fix::Download { module } => {
            let r = app.engine.start_asset_download(&module, &[]);
            started(app, &module, r);
        }
        Fix::InstallRuntime { module } => {
            let r = app.engine.start_python_install(Some(&module));
            started(app, &module, r);
        }
    }
    None
}

/// Say that a download began (it runs on the engine's worker), or why not.
fn started(app: &mut GlossaApp, module: &str, result: glossa_core::Result<()>) {
    match result {
        Ok(()) => {
            let m = app.locale.fmt("error.download_started", module);
            app.notify(m, false);
        }
        Err(e) => app.report(&e),
    }
}

/// The service whose settings an error is about: the one it names, which
/// after a fail-over is not the one asked, else the one asked.
fn settings_target(app: &GlossaApp, view: &ErrorView, asked: &str) -> String {
    let registry = app.engine.registry();
    let named = view.subject.as_deref().and_then(|s| {
        if registry.get(s).is_some() {
            return Some(s.to_owned());
        }
        // Built-in services put their name in the error, not their id.
        registry.entries().iter().find(|e| e.info().name == s).map(|e| e.info().id.clone())
    });
    named.unwrap_or_else(|| asked.to_owned())
}

/// The service after `current` among `ids`, going round, when there is
/// another: what "try the next service" asks.
pub fn next_service<'a>(ids: &'a [String], current: &str) -> Option<&'a str> {
    if !ids.iter().any(|id| id != current) {
        return None;
    }
    let next = match ids.iter().position(|id| id == current) {
        Some(i) => &ids[(i + 1) % ids.len()],
        None => &ids[0],
    };
    Some(next.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_can_be_shown_and_names_no_bare_key() {
        let en = Locale::english();
        for e in glossa_core::error_text::samples() {
            let v = ErrorView::new(&en, &e);
            assert!(!v.headline.is_empty() && !v.headline.contains("%s"), "{e:?}: {}", v.headline);
            assert_ne!(v.headline, e.locale_key(), "{e:?} has no text");
            assert_ne!(v.detail.as_deref(), Some(""), "{e:?}");
        }
    }

    #[test]
    fn the_buttons_fit_the_kind_of_failure() {
        let has = |e: &Error, f: &Fix| fixes(e).contains(f);
        assert!(has(&Error::Network("reset".into()), &Fix::Retry));
        assert!(!has(&Error::Network("reset".into()), &Fix::NextService), "offline: no service would answer");
        assert!(has(&Error::Timeout(1), &Fix::Retry));
        assert!(has(&Error::Timeout(1), &Fix::NextService));
        assert!(has(&Error::RateLimited { provider: "G".into(), retry_after_secs: None }, &Fix::Retry));
        assert!(has(&Error::bad_response("G", "x"), &Fix::Retry));
        let key = Error::MissingCredential { provider: "DeepL".into(), what: "api_key".into() };
        assert_eq!(fixes(&key), [Fix::ServiceSettings, Fix::NextService]);
        for status in [401, 403] {
            let e = Error::HttpStatus { provider: "G".into(), status, body: String::new() };
            assert!(has(&e, &Fix::ServiceSettings) && !has(&e, &Fix::Retry), "HTTP {status}");
        }
        let e = Error::HttpStatus { provider: "G".into(), status: 503, body: String::new() };
        assert!(has(&e, &Fix::Retry) && !has(&e, &Fix::ServiceSettings));
        let e = Error::AssetsMissing { module: "m".into(), missing: vec!["a".into()] };
        assert_eq!(fixes(&e)[0], Fix::Download { module: "m".into() });
        assert_eq!(fixes(&Error::NeedsApproval { module: "m".into() })[0], Fix::Modules);
        // Nothing to press when the request itself is at fault.
        assert!(fixes(&Error::EmptyInput).is_empty());
        assert!(fixes(&Error::SameLanguage).is_empty());
        assert!(fixes(&Error::Cancelled).is_empty());
    }

    #[test]
    fn the_next_service_goes_round_and_needs_another() {
        let ids = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(next_service(&ids, "a"), Some("b"));
        assert_eq!(next_service(&ids, "c"), Some("a"));
        assert_eq!(next_service(&ids, "gone"), Some("a"));
        assert_eq!(next_service(&ids[..1], "a"), None);
        assert_eq!(next_service(&[], "a"), None);
    }
}
