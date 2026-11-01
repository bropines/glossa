//! The About window: what Glossa is, who made it, and its licences, with
//! the license texts and third-party notices built in to read.

use egui::{vec2, RichText, Sense};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};

const MIT: &str = include_str!("../../../LICENSE-MIT");
const APACHE: &str = include_str!("../../../LICENSE-APACHE");
const NOTICE: &str = include_str!("../../../NOTICE");
const REPO: &str = "https://github.com/bropines/glossa";
const AUTHOR_URL: &str = "https://github.com/bropines";

/// A text the window can show in place of the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Doc {
    Mit,
    Apache,
    Notice,
}

impl Doc {
    fn text(self) -> &'static str {
        match self {
            Doc::Mit => MIT,
            Doc::Apache => APACHE,
            Doc::Notice => NOTICE,
        }
    }

    fn title_key(self) -> &'static str {
        match self {
            Doc::Mit => "about.mit",
            Doc::Apache => "about.apache",
            Doc::Notice => "about.notices",
        }
    }

    fn url(self) -> String {
        let file = match self {
            Doc::Mit => "LICENSE-MIT",
            Doc::Apache => "LICENSE-APACHE",
            Doc::Notice => "NOTICE",
        };
        format!("{REPO}/blob/main/{file}")
    }
}

fn doc_id() -> egui::Id {
    egui::Id::unique("glossa-about-doc")
}

pub fn show_about(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let title = app.t("msg.about");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([520.0, 442.0]).with_min_inner_size([420.0, 392.0]);
    let (look, custom) = (app.look.clone(), crate::chrome::custom(&app.cfg));
    let mut open = true;
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::ABOUT), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(
            ui,
            &look,
            custom,
            &crate::chrome::Caption { minimize: false, maximize: false, ..crate::chrome::Caption::new(&title) },
        );
        egui::CentralPanel::default().show(ui, |ui| contents(app, ui, &mut open));
    });
    app.child_backdrop(&title, false);
    if !open {
        ctx.data_mut(|d| d.remove::<Option<Doc>>(doc_id()));
    }
    app.about_open = open;
}

/// The window's contents. Escape steps back from a license text, else
/// closes the window (`open` goes false).
fn contents(app: &mut GlossaApp, ui: &mut egui::Ui, open: &mut bool) {
    let mut doc: Option<Doc> = ui.data(|d| d.get_temp(doc_id())).flatten();
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if doc.is_some() {
            doc = None;
        } else {
            *open = false;
        }
    }
    match doc {
        Some(d) => document(app, ui, d, &mut doc),
        None => summary(app, ui, &mut doc),
    }
    ui.data_mut(|data| data.insert_temp(doc_id(), doc));
}

fn summary(app: &mut GlossaApp, ui: &mut egui::Ui, doc: &mut Option<Doc>) {
    let look = app.look.clone();
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        let texture = app_icon_texture(ui.ctx());
        ui.painter().image(
            texture.id(),
            r,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        ui.add_space(4.0);
        ui.heading(RichText::new("Glossa").strong());
        ui.label(RichText::new(format!("{}{}", app.t("msg.version"), glossa_core::VERSION)).color(look.weak_text()));
        ui.add_space(8.0);
        ui.add(egui::Label::new(app.t("about.description")).wrap());

        ui.add_space(14.0);
        ui.label(RichText::new(app.t("about.authors")).small().color(look.weak_text()));
        if ui.link("Bropines").on_hover_text(AUTHOR_URL).clicked() {
            app.open_url(AUTHOR_URL);
        }
        ui.label("Claude (Anthropic)");

        ui.add_space(14.0);
        ui.label(app.locale.fmt_args("about.license", &["MIT OR Apache-2.0"]));
        centered_row(ui, "about-links", |ui| {
            for (i, d) in [Doc::Mit, Doc::Apache, Doc::Notice].into_iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("·").color(look.weak_text()));
                }
                if ui.link(app.t(d.title_key())).clicked() {
                    *doc = Some(d);
                }
            }
        });

        ui.add_space(14.0);
        ui.label(RichText::new(app.t("about.not_affiliated")).small().color(look.weak_text()));
    });
}

/// A license or the notices, verbatim, in a scrolling view.
fn document(app: &mut GlossaApp, ui: &mut egui::Ui, d: Doc, doc: &mut Option<Doc>) {
    let look = app.look.clone();
    ui.horizontal(|ui| {
        if icons::sized_button(ui, &look, Icon::Back, &app.t("about.back"), 20.0, true).clicked() {
            *doc = None;
        }
        ui.label(RichText::new(app.t(d.title_key())).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let url = d.url();
            if ui.link(app.t("about.open_online")).on_hover_text(&url).clicked() {
                app.open_url(&url);
            }
        });
    });
    ui.separator();
    egui::ScrollArea::both().id_salt(("about-doc", d.title_key())).auto_shrink([false, false]).show(ui, |ui| {
        // Legal texts are shown as written, line for line.
        ui.add(egui::Label::new(RichText::new(d.text()).monospace().size(10.0)).extend());
    });
}

/// Lay out `add` in a row centred in the available width. The row's width
/// is known only after drawing it, so it is remembered from the last frame.
fn centered_row(ui: &mut egui::Ui, salt: &str, add: impl FnOnce(&mut egui::Ui)) {
    let id = ui.scope_id().with(salt);
    let width: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        let from = ui.cursor().min.x;
        add(ui);
        let used = ui.min_rect().right() - from;
        if (used - width).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, used));
            ui.ctx().request_repaint();
        }
    });
}

/// The application icon as a texture, made once per graphics context.
pub(crate) fn app_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("glossa-about-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let icon = crate::app_icon(128);
    let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    let t = ctx.load_texture("glossa-about-icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}
