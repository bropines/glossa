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
    pub error: Option<String>,
    pub busy: bool,
    pub service: Option<String>,
    pub from: LangId,
    pub to: LangId,
    focus: bool,
}

pub fn open(app: &mut GlossaApp, word: Option<String>) {
    let (from, to) = (LangId::AUTO, app.cfg.dictionary.language);
    let mut st = app.dictionary.take().unwrap_or(DictState {
        query: String::new(),
        job: 0,
        article: None,
        error: None,
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
    st.job = app.engine.dictionary_async(Slot::Dictionary, q, LangPair::new(st.from, st.to), st.service.clone());
}

impl DictState {
    pub fn on_result(
        &mut self,
        job: u64,
        _query: String,
        result: Result<Arc<Article>, Arc<Error>>,
        message: Option<String>,
    ) {
        if job != self.job {
            return;
        }
        self.busy = false;
        match result {
            Ok(a) => {
                self.article = Some(a);
                self.error = None;
            }
            Err(_) => {
                self.article = None;
                self.error = message;
            }
        }
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.dictionary.take() else { return };
    let title = app.t("dict.title");
    let builder = egui::ViewportBuilder::default()
        .with_title(title.clone())
        .with_transparent(app.look.window_backdrop != glossa_core::config::Backdrop::None)
        .with_inner_size([820.0, 560.0])
        .with_min_inner_size([480.0, 320.0])
        .with_icon(crate::app_icon(32));
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-dictionary"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
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
            if enter || crate::icons::sized_button(ui, &look, crate::icons::Icon::Search, "", 26.0, true).clicked() {
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
            let menu = crate::icons::sized_button(ui, &look, crate::icons::Icon::Menu, "", 24.0, true);
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
                    app.engine.clear_dictionary_history();
                    ui.close();
                }
                if ui.button(app.t("dict.options")).clicked() {
                    crate::options::open(app);
                    ui.close();
                }
            });
        });
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
    let article_panel = egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("article").show(ui, |ui| {
            ui.set_width(ui.available_width());
            if st.busy {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| ui.add(egui::Spinner::new().size(24.0)));
            } else if let Some(e) = &st.error {
                let msg =
                    if e.contains("no results") { app.locale.fmt("dict.no_results", &st.query) } else { e.clone() };
                ui.label(RichText::new(msg).color(look.error()));
            } else if let Some(a) = st.article.clone() {
                app.fonts.ensure(&app.ctx, &a.to_plain_text());
                if let Some(h) = &a.html {
                    app.fonts.ensure(&app.ctx, h);
                }
                click = crate::rich::article(ui, &look, &a, zoom);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        a.provider_id,
                        app.locale.fmt("msg.ms", &a.elapsed_ms.to_string())
                    ))
                    .small()
                    .color(look.weak_text()),
                );
            } else {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), r, crate::icons::Icon::Dictionary, look.weak_text());
                });
            }
        });
    });
    app.text_zoom_wheel(ui, article_panel.response.rect);
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
