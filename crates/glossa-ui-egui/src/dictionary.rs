//! The dictionary window: a search box, the lookup history and the
//! dictionary services on the left, the article on the right.

use std::sync::Arc;

use egui::{vec2, RichText};
use glossa_core::{Article, Capabilities, Error, LangId, LangPair};
use glossa_engine::Slot;

use crate::app::GlossaApp;
use crate::rich::ArticleClick;

pub struct DictState {
    pub query: String,
    pub job: u64,
    pub article: Option<Arc<Article>>,
    pub error: Option<crate::errors::ErrorView>,
    /// The word the dictionary had no article for, when that is how the
    /// last lookup ended: told from the failures by the error's kind, and
    /// said in the interface language.
    no_results: Option<String>,
    /// The lookup history as it was before "Clear history", and when it was
    /// cleared: the clear can be undone for [`UNDO_FOR`].
    cleared: Option<(Vec<String>, std::time::Instant)>,
    pub busy: bool,
    pub service: Option<String>,
    pub from: LangId,
    pub to: LangId,
    focus: bool,
}

/// How long "Clear history" can be undone.
const UNDO_FOR: std::time::Duration = std::time::Duration::from_secs(8);

pub fn open(app: &mut GlossaApp, word: Option<String>) {
    if app.dictionary.is_some() {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(crate::chrome::DICTIONARY));
    }
    let (from, to) = (LangId::AUTO, app.cfg.dictionary.language);
    let mut st = app.dictionary.take().unwrap_or(DictState {
        query: String::new(),
        job: 0,
        article: None,
        error: None,
        no_results: None,
        cleared: None,
        busy: false,
        service: Some(app.cfg.services.dictionary_active.clone()).filter(|s| !s.is_empty()),
        from,
        to,
        focus: true,
    });
    st.focus = true;
    if let Some(w) = word.map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()) {
        st.query = w;
        lookup(app, &mut st);
    }
    app.dictionary = Some(st);
    app.ctx.request_repaint();
}

fn lookup(app: &mut GlossaApp, st: &mut DictState) {
    let q = st.query.trim().to_owned();
    if q.is_empty() {
        return;
    }
    app.fonts.ensure(&app.ctx, &q);
    st.busy = true;
    st.error = None;
    st.no_results = None;
    st.job = app.engine.dictionary_async(Slot::Dictionary, q, LangPair::new(st.from, st.to), st.service.clone());
}

impl DictState {
    pub fn on_result(
        &mut self,
        job: u64,
        _query: String,
        result: Result<Arc<Article>, Arc<Error>>,
        message: Option<crate::errors::ErrorView>,
    ) {
        if job != self.job {
            return;
        }
        self.busy = false;
        self.no_results = None;
        match result {
            Ok(a) => {
                self.article = Some(a);
                self.error = None;
            }
            Err(e) => {
                self.article = None;
                match &*e {
                    Error::NoResults { query, .. } => {
                        self.error = None;
                        self.no_results = Some(query.clone());
                    }
                    _ => self.error = message,
                }
            }
        }
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.dictionary.take() else { return };
    let title = app.t("dict.title");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([820.0, 560.0]).with_min_inner_size([480.0, 320.0]);
    let mut open = true;
    let custom = crate::chrome::custom(&app.cfg);
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::DICTIONARY), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        crate::toast::draw(app, ui.ctx());
        app.text_zoom_keys(ui);
        body(app, ui, &mut st);
    });
    app.child_backdrop(&title, false);
    if open {
        app.dictionary = Some(st);
    }
}

fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut DictState) {
    let look = app.look.clone();
    // The dictionary's own zoom, times the text zoom shared by all text areas.
    let zoom = app.cfg.dictionary.zoom as f32 / 100.0 * look.text_scale;
    let mut click: Option<ArticleClick> = None;
    egui::Panel::top("dict-top").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .desired_width(ui.available_width() - 330.0)
                    .hint_text(app.t("dict.title"))
                    .font(look.field_font()),
            );
            if st.focus {
                r.request_focus();
                st.focus = false;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let search_tip = app.t("dict.search");
            let search = crate::icons::sized_button(ui, &look, crate::icons::Icon::Search, &search_tip, 26.0, true);
            if enter || search.clicked() {
                lookup(app, st);
            }
            let disabled = app.cfg.languages.disabled.clone();
            let auto = app.t("main.auto_detect");
            if let Some(l) = crate::widgets::lang_combo(
                ui,
                "dict-from",
                st.from,
                true,
                &auto,
                &app.t("lang.search"),
                &disabled,
                110.0,
            ) {
                st.from = l;
            }
            ui.label("→");
            if let Some(l) =
                crate::widgets::lang_combo(ui, "dict-to", st.to, false, &auto, &app.t("lang.search"), &disabled, 110.0)
            {
                st.to = l;
                let _ = app.engine.update_config(move |c| c.dictionary.language = l);
                lookup(app, st);
            }
            let menu_tip = app.t("dict.menu");
            let menu = crate::icons::sized_button(ui, &look, crate::icons::Icon::Menu, &menu_tip, 24.0, true);
            crate::dropdown::menu(ui, &menu, |ui| {
                let mut exact = app.cfg.dictionary.exact_search;
                if ui.checkbox(&mut exact, app.t("dict.exact_search")).changed() {
                    let _ = app.engine.update_config(move |c| c.dictionary.exact_search = exact);
                }
                let mut pane = app.cfg.dictionary.show_services_pane;
                if ui.checkbox(&mut pane, app.t("dict.show_services")).changed() {
                    let _ = app.engine.update_config(move |c| c.dictionary.show_services_pane = pane);
                }
                if ui.button(app.t("dict.clear_history")).clicked() {
                    // Cleared at once, and offered back under the search
                    // box for a few seconds: a menu has no room to ask.
                    let words = app.engine.dictionary_history();
                    app.engine.clear_dictionary_history();
                    if !words.is_empty() {
                        st.cleared = Some((words, std::time::Instant::now()));
                    }
                    ui.close();
                }
                if ui.button(app.t("dict.options")).clicked() {
                    crate::options::open(app);
                    ui.close();
                }
            });
        });
        match st.cleared.as_ref().map(|(_, at)| UNDO_FOR.saturating_sub(at.elapsed())) {
            Some(left) if !left.is_zero() => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(app.t("dict.history_cleared")).color(look.weak_text()));
                    if ui.button(app.t("edit.undo")).clicked() {
                        if let Some((words, _)) = st.cleared.take() {
                            app.engine.restore_dictionary_history(words);
                        }
                    }
                });
                // Wake once more to take the offer away.
                ui.ctx().request_repaint_after(left);
            }
            Some(_) => st.cleared = None,
            None => {}
        }
        ui.add_space(4.0);
    });
    if app.cfg.dictionary.show_services_pane {
        egui::Panel::left("dict-left").default_size(200.0).resizable(true).show(ui, |ui| {
            ui.label(RichText::new(app.t("options.services.dictionaries")).small().color(look.weak_text()));
            let providers = app.engine.providers(Capabilities::DICTIONARY);
            let shown: Vec<_> =
                providers.iter().filter(|p| p.enabled || st.service.as_deref() == Some(&p.info.id)).cloned().collect();
            for p in &shown {
                let sel = st.service.as_deref() == Some(&p.info.id);
                ui.horizontal(|ui| {
                    crate::widgets::service_badge(ui, &p.info.id, &p.info.name, 16.0);
                    if ui.selectable_label(sel, &p.info.name).clicked() {
                        st.service = Some(p.info.id.clone());
                        let id = p.info.id.clone();
                        let _ = app.engine.update_config(move |c| c.services.dictionary_active = id);
                        lookup(app, st);
                    }
                });
            }
            ui.separator();
            ui.label(RichText::new(app.t("history.title")).small().color(look.weak_text()));
            egui::ScrollArea::vertical().id_salt("dict-history").show(ui, |ui| {
                for w in app.engine.dictionary_history().into_iter().take(100) {
                    if ui.selectable_label(w == st.query, &w).clicked() {
                        st.query = w;
                        lookup(app, st);
                    }
                }
            });
        });
    }
    // What "try the next dictionary" would ask, should the lookup have failed.
    let mut fix = None;
    let dictionaries: Vec<(String, String)> = app
        .engine
        .providers(Capabilities::DICTIONARY)
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| (p.info.id.clone(), p.info.name.clone()))
        .collect();
    let asked = st.service.clone().unwrap_or_default();
    let ids: Vec<String> = dictionaries.iter().map(|(id, _)| id.clone()).collect();
    let next = crate::errors::next_service(&ids, &asked).map(str::to_owned);
    let next_name =
        next.as_ref().and_then(|n| dictionaries.iter().find(|(id, _)| id == n)).map(|(_, name)| name.clone());
    let article_panel = egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("article").show(ui, |ui| {
            ui.set_width(ui.available_width());
            if st.busy {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.add(egui::Spinner::new().size(24.0)));
            } else if let Some(word) = &st.no_results {
                // Not a failure: the dictionary answered, it has no such
                // word. Said calmly, where the article would be.
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(app.locale.fmt("dict.no_results", word)).color(look.weak_text()));
                });
            } else if let Some(e) = &st.error {
                let body = egui::TextStyle::Body.resolve(ui.style());
                let style = crate::errors::Style { font: body, weak: look.weak_text(), compact: false };
                fix = crate::errors::show(ui, app, e, &style, next_name.as_deref());
            } else if let Some(a) = st.article.clone() {
                app.fonts.ensure(&app.ctx, &a.to_plain_text());
                if let Some(h) = &a.html {
                    app.fonts.ensure(&app.ctx, h);
                }
                click = crate::rich::article(ui, &look, &a, zoom);
                ui.add_space(8.0);
                // The dictionary by its name, as the history window shows
                // services; the id only when it is no longer installed.
                let provider = app
                    .engine
                    .registry()
                    .get(&a.provider_id)
                    .map(|p| p.info().name.clone())
                    .unwrap_or_else(|| a.provider_id.clone());
                ui.label(
                    RichText::new(format!("{} · {}", provider, app.locale.fmt("msg.ms", &a.elapsed_ms.to_string())))
                        .small()
                        .color(look.weak_text()),
                );
            } else {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), r, crate::icons::Icon::Dictionary, look.weak_text());
                    ui.add_space(6.0);
                    ui.label(RichText::new(app.t("dict.empty")).color(look.weak_text()));
                });
            }
        });
    });
    if let (Some(fix), Some(e)) = (fix, st.error.clone()) {
        match crate::errors::apply(app, &e, fix, &asked) {
            Some(crate::errors::Fix::Retry) => lookup(app, st),
            Some(crate::errors::Fix::NextService) => {
                if let Some(id) = next {
                    st.service = Some(id.clone());
                    let _ = app.engine.update_config(move |c| c.services.dictionary_active = id);
                    lookup(app, st);
                }
            }
            _ => {}
        }
    }
    app.text_zoom_wheel(ui, article_panel.response.rect);
    // The article's context menu, for a right click anywhere over it (its
    // text and links take the clicks themselves).
    let area = article_panel.response.rect;
    let right_click =
        ui.input(|i| i.pointer.secondary_clicked() && i.pointer.interact_pos().is_some_and(|p| area.contains(p)));
    let menu_id = egui::Id::unique("dict-article-menu");
    if let Some(a) = st.article.clone() {
        crate::dropdown::context_menu_in(ui, menu_id, right_click, area.center(), |ui| article_menu(app, ui, st, &a));
    }
    match click {
        Some(ArticleClick::Lookup(w)) => {
            st.query = w;
            lookup(app, st);
        }
        Some(ArticleClick::Open(url)) => app.open_url(&url),
        Some(ArticleClick::Listen(w)) => app.speak(&w, st.from),
        None => {}
    }
}

/// The article's context menu: copy it, translate or hear the word, or
/// search for it on the dictionary's website.
fn article_menu(app: &mut GlossaApp, ui: &mut egui::Ui, st: &DictState, a: &Article) {
    let word = st.query.trim().to_owned();
    if ui.button(app.t("article.copy")).clicked() {
        app.copy_text(&a.to_plain_text());
        ui.close();
    }
    if ui.button(app.t("article.translate")).clicked() {
        app.perform(glossa_core::Action::ShowMainWindow, Some(word.clone()));
        ui.close();
    }
    let link = st.service.as_deref().and_then(|s| app.engine.web_link(s, &word, LangPair::new(st.from, st.to)));
    if let Some(url) = link {
        if ui.button(app.t("article.search")).clicked() {
            app.open_url(&url);
            ui.close();
        }
    }
    if ui.button(app.t("article.listen")).clicked() {
        app.speak(&word, st.from);
        ui.close();
    }
}
