//! The welcome guide, on the first start (`general.show_welcome`) and
//! whenever Options → Basics asks for it: what Glossa is, the hotkeys
//! (changeable on the spot), how to use it, where to keep API keys and
//! data, and on macOS the permissions it needs. Every choice is applied as
//! it is made; Finish (or closing the window) only stops the guide from
//! coming back.
//!
//! It is drawn as a small onboarding flow rather than an options page: a
//! progress bar over the steps, a badge and a large title per page, content
//! in cards, live previews of the built-in themes, hotkeys as key caps, and
//! an accent Next button. Colours come from the current theme, so it looks
//! at home in every one of them, with or without a backdrop.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind};
use glossa_core::hotkey::{Action, Hotkey};
use glossa_core::secrets::SecretStorage;
use glossa_platform::shell::SHELL;

use crate::app::GlossaApp;
use crate::icons::{self, Icon};
use crate::style::{c, Look};

/// The actions the guide shows, most used first, with their icons.
const KEY_ACTIONS: [(Action, Icon); 6] = [
    (Action::ShowPopup, Icon::Translate),
    (Action::ShowMainWindow, Icon::Open),
    (Action::RecognizeAndTranslate, Icon::Camera),
    (Action::TextRecognition, Icon::Crop),
    (Action::ReplaceSelection, Icon::Replace),
    (Action::CopyTranslation, Icon::Copy),
];

/// Space around the page content.
const PAD: i8 = 28;
/// Corner radius of the cards.
const CARD_RADIUS: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Hello,
    Hotkeys,
    Usage,
    Storage,
    Permissions,
    Done,
}

fn pages() -> Vec<Page> {
    let mut v = vec![Page::Hello, Page::Hotkeys, Page::Usage, Page::Storage];
    if !glossa_platform::permissions::Permission::all().is_empty() {
        v.push(Page::Permissions);
    }
    v.push(Page::Done);
    v
}

/// The open guide.
#[derive(Debug, Default)]
pub struct WelcomeState {
    page: usize,
    /// "Keep the data beside the program" ticked, to be done on Finish.
    portable: bool,
    message: Option<(String, bool)>,
    /// Frames drawn, to bring the window to the front once it exists.
    frames: u32,
}

/// Open the guide (again).
pub fn open(app: &mut GlossaApp) {
    if app.welcome.is_some() {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(crate::chrome::WELCOME));
    } else {
        app.welcome = Some(WelcomeState::default());
    }
    app.ctx.request_repaint();
}

/// The guide's colours, from the theme.
#[derive(Clone, Copy)]
struct Palette {
    accent: Color32,
    accent_text: Color32,
    /// The accent, faint: badges and the chosen card.
    accent_soft: Color32,
    text: Color32,
    weak: Color32,
    /// A card: a veil of the text colour, so it reads on any background,
    /// a backdrop included.
    card: Color32,
    card_border: Color32,
}

impl Palette {
    fn new(look: &Look) -> Self {
        let text = look.text();
        let accent = look.accent();
        let veil = |a: u8| Color32::from_rgba_unmultiplied(text.r(), text.g(), text.b(), a);
        Palette {
            accent,
            // White or black, whichever reads on the accent (the theme's own
            // accent text may not match a system accent).
            accent_text: if luminance(accent) > 0.55 { Color32::BLACK } else { Color32::WHITE },
            accent_soft: Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40),
            text,
            weak: look.weak_text(),
            card: veil(if look.theme.dark { 12 } else { 14 }),
            card_border: veil(if look.theme.dark { 26 } else { 30 }),
        }
    }
}

/// Relative brightness, 0 to 1, of a colour as sRGB.
fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.welcome.take() else { return };
    let title = app.t("welcome.title");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([780.0, 700.0]).with_min_inner_size([640.0, 560.0]);
    let custom = crate::chrome::custom(&app.cfg);
    let look = app.look.clone();
    let pal = Palette::new(&look);
    let mut open = true;
    let mut finished = false;
    // On the first start the main window opens in the same frames and would
    // cover the guide; bring the guide forward once both exist.
    st.frames += 1;
    if st.frames == 3 {
        crate::chrome::raise(ctx, crate::chrome::viewport(crate::chrome::WELCOME));
    }
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::WELCOME), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(
            ui,
            &look,
            custom,
            &crate::chrome::Caption { minimize: false, maximize: false, ..crate::chrome::Caption::new(&title) },
        );
        let all = pages();
        st.page = st.page.min(all.len() - 1);
        let page = all[st.page];
        let last = st.page + 1 == all.len();
        // The window's own fill (clear under a backdrop, as it should be).
        let fill = ui.visuals().panel_fill;

        egui::Panel::top("welcome-progress")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(PAD, 14)))
            .show(ui, |ui| {
                progress(ui, &pal, st.page, all.len());
                ui.add_space(6.0);
                let step = app
                    .locale
                    .fmt_args("welcome.step", &[(st.page + 1).to_string().as_str(), all.len().to_string().as_str()]);
                ui.label(RichText::new(step.to_uppercase()).size(11.0).color(pal.weak));
            });

        egui::Panel::bottom("welcome-nav")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin {
                left: PAD,
                right: PAD,
                top: 12,
                bottom: 16,
            }))
            .show(ui, |ui| {
                let line = ui.max_rect().top() - 12.0;
                ui.painter().hline(ui.max_rect().x_range(), line, Stroke::new(1.0, pal.card_border));
                ui.horizontal(|ui| {
                    if !last && link_button(ui, &pal, &app.t("welcome.skip")).clicked() {
                        finished = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let next = if last { app.t("welcome.finish") } else { app.t("welcome.next") };
                        if primary_button(ui, &pal, &next).clicked() {
                            if last {
                                finished = true;
                            } else {
                                st.page += 1;
                            }
                        }
                        if st.page > 0 && secondary_button(ui, &app.t("welcome.back")).clicked() {
                            st.page -= 1;
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin {
                left: PAD,
                right: PAD,
                top: 4,
                bottom: 8,
            }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    match page {
                        Page::Hello => hello(app, ui, &pal),
                        Page::Hotkeys => hotkeys(app, ui, &pal),
                        Page::Usage => usage(app, ui, &pal),
                        Page::Storage => storage(app, ui, &pal, &mut st),
                        Page::Permissions => permissions(app, ui, &pal),
                        Page::Done => done(app, ui, &pal, &st),
                    }
                    if let Some((text, error)) = &st.message {
                        ui.add_space(10.0);
                        let colour = if *error { look.error() } else { pal.weak };
                        ui.label(RichText::new(text).color(colour));
                    }
                });
            });
    });
    app.child_backdrop(&title, false);
    if finished {
        finish(app, &mut st);
        open = st.message.as_ref().is_some_and(|(_, error)| *error);
    } else if !open && !app.is_quitting() {
        // Closed with the window's own button: the same as Skip. Quitting
        // the program meanwhile is not, and the guide comes back next time.
        let _ = app.engine.update_config(|c| c.general.show_welcome = false);
    }
    if open {
        app.welcome = Some(st);
    }
}

// -- pieces ------------------------------------------------------------------

/// A bar of one segment per step, filled up to the current one.
fn progress(ui: &mut egui::Ui, pal: &Palette, page: usize, count: usize) {
    let gap = 6.0;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let seg = (width - gap * (count as f32 - 1.0)) / count as f32;
    for i in 0..count {
        let x = rect.left() + i as f32 * (seg + gap);
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(seg, rect.height()));
        let fill = if i <= page { pal.accent } else { pal.card_border };
        ui.painter().rect_filled(r, CornerRadius::same(2), fill);
    }
}

/// The page's badge, title and subtitle.
fn page_header(ui: &mut egui::Ui, pal: &Palette, icon: Icon, title: &str, subtitle: Option<&str>) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        badge(ui, pal, icon, 44.0);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.add_space(if subtitle.is_some() { 0.0 } else { 9.0 });
            ui.label(RichText::new(title).size(22.0).strong().color(pal.text));
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).size(13.0).color(pal.weak));
            }
        });
    });
    ui.add_space(16.0);
}

/// A rounded square in the faint accent with an icon in the accent.
fn badge(ui: &mut egui::Ui, pal: &Palette, icon: Icon, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same((size * 0.28) as u8), pal.accent_soft);
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(size * 0.5, size * 0.5)), icon, pal.accent);
}

/// Content in a card.
fn card<R>(ui: &mut egui::Ui, pal: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(1.0, pal.card_border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A small heading inside a card.
fn card_title(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.5).strong().color(pal.text));
    ui.add_space(8.0);
}

fn primary_button(ui: &mut egui::Ui, pal: &Palette, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).strong().color(pal.accent_text))
            .fill(pal.accent)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(112.0, 32.0)),
    )
}

fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).corner_radius(CornerRadius::same(8)).min_size(vec2(88.0, 32.0)))
}

/// Plain text that acts as a button: Skip.
fn link_button(ui: &mut egui::Ui, pal: &Palette, text: &str) -> egui::Response {
    let resp = ui.add(egui::Button::new(RichText::new(text).color(pal.weak)).frame(false).min_size(vec2(0.0, 32.0)));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A hotkey as key caps ("Ctrl" "Shift" "Q"), or `empty` in an outline
/// when there is none. `big` for the last page. Clickable.
fn keycaps(
    ui: &mut egui::Ui,
    look: &Look,
    pal: &Palette,
    hotkey: Option<&Hotkey>,
    empty: &str,
    big: bool,
) -> egui::Response {
    let height = if big { 40.0 } else { 26.0 };
    let font = FontId::proportional(if big { 17.0 } else { 12.5 });
    let pad = if big { 14.0 } else { 9.0 };
    let gap = if big { 8.0 } else { 5.0 };
    let text = hotkey.map(|h| h.to_string()).unwrap_or_default();
    let parts: Vec<&str> = text.split('+').filter(|p| !p.is_empty()).collect();
    let galleys: Vec<_> = if parts.is_empty() {
        vec![ui.painter().layout_no_wrap(empty.to_owned(), font.clone(), pal.weak)]
    } else {
        parts.iter().map(|p| ui.painter().layout_no_wrap((*p).to_owned(), font.clone(), pal.text)).collect()
    };
    let widths: Vec<f32> = galleys.iter().map(|g| (g.size().x + 2.0 * pad).max(height)).collect();
    let total = widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0).max(0.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(total, height + 2.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let button = look.theme.button.get(glossa_core::theme::WidgetState::Normal);
    let face = c(button.back()).to_opaque();
    let edge = c(button.border);
    let shade = face.lerp_to_gamma(Color32::BLACK, if look.theme.dark { 0.35 } else { 0.18 });
    let mut x = rect.left();
    for (galley, w) in galleys.into_iter().zip(widths) {
        let cap = Rect::from_min_size(pos2(x, rect.top()), vec2(w, height));
        let radius = CornerRadius::same(if big { 9 } else { 6 });
        if parts.is_empty() {
            ui.painter().rect_stroke(cap, radius, Stroke::new(1.0, pal.card_border), StrokeKind::Inside);
        } else {
            // A key: a darker lip under the face.
            ui.painter().rect_filled(cap.translate(vec2(0.0, 2.0)), radius, shade);
            ui.painter().rect(cap, radius, face, Stroke::new(1.0, edge), StrokeKind::Inside);
            if resp.hovered() {
                ui.painter().rect_stroke(cap, radius, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
            }
        }
        ui.painter().galley(cap.center() - galley.size() / 2.0, galley, pal.text);
        x += w + gap;
    }
    resp
}

// -- pages -------------------------------------------------------------------

fn hello(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette) {
    let icon = crate::about::app_icon_texture(ui.ctx());
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Image::new(&icon).fit_to_exact_size(vec2(72.0, 72.0)));
        ui.add_space(10.0);
        ui.label(RichText::new(app.t("welcome.title")).size(28.0).strong().color(pal.text));
        ui.add_space(2.0);
        ui.label(RichText::new(app.t("welcome.hello.title")).size(15.0).color(pal.accent));
        ui.add_space(12.0);
        let width = ui.available_width().min(560.0);
        ui.allocate_ui(vec2(width, 0.0), |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(app.t("welcome.hello.what")).size(13.5).color(pal.text));
                ui.add_space(6.0);
                let home = if SHELL.app_menu { app.t("welcome.hello.menu_bar") } else { app.t("welcome.hello.tray") };
                ui.label(RichText::new(home).size(12.5).color(pal.weak));
            });
        });
    });
    ui.add_space(18.0);
    let cfg = app.engine.config();
    card(ui, pal, |ui| {
        card_title(ui, pal, &app.t("welcome.hello.look"));
        ui.horizontal(|ui| {
            ui.label(RichText::new(app.t("options.basics.ui_language")).color(pal.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let locales = app.engine.locales();
                let current = locales
                    .iter()
                    .find(|l| l.code.eq_ignore_ascii_case(&cfg.general.ui_language))
                    .map(|l| format!("{} - {}", l.name, l.native_name))
                    .unwrap_or_else(|| app.t("options.basics.ui_language_system"));
                let mut pick = None;
                crate::dropdown::ComboBox::from_id_salt("welcome-ui-lang").width(240.0).selected_text(current).show_ui(
                    ui,
                    |ui| {
                        if ui
                            .selectable_label(
                                cfg.general.ui_language.is_empty(),
                                app.t("options.basics.ui_language_system"),
                            )
                            .clicked()
                        {
                            pick = Some(String::new());
                        }
                        for l in &locales {
                            let label = format!("{} - {}", l.name, l.native_name);
                            if ui.selectable_label(l.code == cfg.general.ui_language, label).clicked() {
                                pick = Some(l.code.clone());
                            }
                        }
                    },
                );
                if let Some(code) = pick {
                    let _ = app.engine.update_config(|c| c.general.ui_language = code);
                }
            });
        });
        ui.add_space(12.0);
        ui.label(RichText::new(app.t("options.appearance.theme")).color(pal.text));
        ui.add_space(6.0);
        let entries = app.engine.theme_entries();
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            for t in entries.iter().filter(|t| t.path.is_none()) {
                let Some(theme) = glossa_core::theme::builtin::by_name(&t.name) else { continue };
                if theme_swatch(ui, pal, &theme, &t.name, t.name == cfg.appearance.theme).clicked() {
                    pick = Some(t.name.clone());
                }
            }
        });
        if let Some(name) = pick {
            let _ = app.engine.update_config(|c| c.appearance.theme = name);
        }
    });
}

/// A small picture of a theme: its window, a pane, two lines of text and
/// an accent button, with the name under it.
fn theme_swatch(
    ui: &mut egui::Ui,
    pal: &Palette,
    theme: &glossa_core::Theme,
    name: &str,
    chosen: bool,
) -> egui::Response {
    let size = vec2(88.0, 58.0);
    let (outer, resp) = ui.allocate_exact_size(size + vec2(0.0, 22.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let r = Rect::from_min_size(outer.min, size);
    let radius = CornerRadius::same(10);
    let back = c(theme.window.back).to_opaque();
    p.rect(r, radius, back, Stroke::new(1.0, c(theme.window.border).to_opaque()), StrokeKind::Inside);
    // The system's title bar buttons (traffic lights where the system draws
    // the frame, minimise, maximise and close where Glossa does), then a
    // pane, text lines and an accent button.
    let text = c(theme.window.text).to_opaque();
    if SHELL.own_frame {
        let glyph = Stroke::new(1.0, text.gamma_multiply(0.7));
        let y = r.top() + 9.0;
        let x = r.right() - 10.0;
        p.line_segment([pos2(x - 2.5, y - 2.5), pos2(x + 2.5, y + 2.5)], glyph);
        p.line_segment([pos2(x + 2.5, y - 2.5), pos2(x - 2.5, y + 2.5)], glyph);
        p.rect_stroke(
            Rect::from_center_size(pos2(x - 10.0, y), vec2(5.0, 5.0)),
            CornerRadius::ZERO,
            glyph,
            StrokeKind::Middle,
        );
        p.line_segment([pos2(x - 22.5, y), pos2(x - 17.5, y)], glyph);
    } else {
        for (i, colour) in [0xff5f57u32, 0xfebc2e, 0x28c840].iter().enumerate() {
            let col = Color32::from_rgb((colour >> 16) as u8, (colour >> 8) as u8, *colour as u8);
            p.circle_filled(pos2(r.left() + 10.0 + i as f32 * 8.0, r.top() + 9.0), 2.5, col);
        }
    }
    let pane = Rect::from_min_max(pos2(r.left() + 8.0, r.top() + 18.0), pos2(r.right() - 8.0, r.bottom() - 8.0));
    p.rect_filled(pane, CornerRadius::same(5), c(theme.edit.normal.back).to_opaque());
    let line = |y: f32, w: f32| {
        let l = Rect::from_min_size(pos2(pane.left() + 7.0, pane.top() + y), vec2(pane.width() * w, 3.0));
        p.rect_filled(l, CornerRadius::same(2), text.gamma_multiply(0.75));
    };
    line(8.0, 0.55);
    line(16.0, 0.35);
    let accent = c(theme.accent).to_opaque();
    let button = Rect::from_min_size(pos2(pane.right() - 30.0, pane.bottom() - 12.0), vec2(24.0, 7.0));
    p.rect_filled(button, CornerRadius::same(3), accent);
    if chosen {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(12), Stroke::new(2.0, pal.accent), StrokeKind::Outside);
        let dot = pos2(r.right() - 2.0, r.top() + 2.0);
        p.circle_filled(dot, 9.0, pal.accent);
        icons::paint(p, Rect::from_center_size(dot, vec2(12.0, 12.0)), Icon::Check, pal.accent_text);
    } else if resp.hovered() {
        p.rect_stroke(r.expand(1.0), CornerRadius::same(11), Stroke::new(1.0, pal.weak), StrokeKind::Outside);
    }
    // The names stay in the text colour: the disabled grey was too faint on
    // light themes. The chosen one is bold.
    let label = RichText::new(name).size(11.5).color(pal.text);
    let label = if chosen { label.strong() } else { label };
    let galley = egui::WidgetText::from(label).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        size.x,
        egui::TextStyle::Body,
    );
    ui.painter().galley(pos2(outer.center().x - galley.size().x / 2.0, r.bottom() + 6.0), galley, pal.text);
    resp
}

fn hotkeys(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette) {
    page_header(ui, pal, Icon::Keyboard, &app.t("welcome.hotkeys.title"), Some(&app.t("welcome.hotkeys.intro")));
    let cfg = app.engine.config();
    let look = app.look.clone();
    if !cfg.hotkeys.enabled || app.hotkeys.is_none() {
        card(ui, pal, |ui| {
            ui.label(RichText::new(app.t("welcome.hotkeys.unavailable")).color(look.error()));
        });
        ui.add_space(10.0);
    }
    let mut change = None;
    card(ui, pal, |ui| {
        for (i, (action, icon)) in KEY_ACTIONS.into_iter().enumerate() {
            if i > 0 {
                ui.add_space(6.0);
                let y = ui.cursor().top();
                ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, pal.card_border));
                ui.add_space(6.0);
            }
            let id = crate::widgets::hotkey_capture_id(ui, &format!("welcome-{}", action.id()));
            let capturing = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, pal.accent);
                ui.add_space(4.0);
                ui.label(RichText::new(app.t(&format!("action.{}", action.id()))).color(pal.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if capturing {
                        let resp = ui.add(
                            egui::Button::new(
                                RichText::new(app.t("options.hotkeys.press_keys")).italics().color(pal.accent),
                            )
                            .stroke(Stroke::new(1.5, pal.accent))
                            .fill(pal.accent_soft)
                            .corner_radius(CornerRadius::same(6))
                            .min_size(vec2(150.0, 28.0)),
                        );
                        if resp.clicked() {
                            ui.data_mut(|d| d.insert_temp(id, false));
                        }
                    } else {
                        let current = cfg.hotkeys.bindings.get(&action);
                        if keycaps(ui, &look, pal, current, &app.t("welcome.hotkeys.none"), false).clicked() {
                            ui.data_mut(|d| d.insert_temp(id, true));
                        }
                    }
                });
            });
            if let Some(c) = crate::widgets::hotkey_capture(ui, id) {
                change = Some((action, c));
            }
        }
    });
    if let Some((action, hk)) = change {
        let _ = app.engine.update_config(|c| match hk {
            Some(hk) => {
                c.hotkeys.bindings.insert(action, hk);
            }
            None => {
                c.hotkeys.bindings.remove(&action);
            }
        });
    }
    ui.add_space(10.0);
    ui.label(RichText::new(app.t("welcome.hotkeys.more")).size(12.0).color(pal.weak));
}

/// A hotkey's text for the tips, if it has one.
fn key(app: &GlossaApp, action: Action) -> Option<String> {
    app.engine.config().hotkeys.bindings.get(&action).map(|h| h.to_string())
}

fn usage(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette) {
    page_header(ui, pal, Icon::Translate, &app.t("welcome.usage.title"), None);
    let with_key = |app: &GlossaApp, k: &str, action: Action| key(app, action).map(|hk| app.locale.fmt(k, &hk));
    let tips: Vec<(Icon, String)> = [
        (Icon::Translate, with_key(app, "welcome.usage.select", Action::ShowPopup)),
        (Icon::Open, with_key(app, "welcome.usage.main", Action::ShowMainWindow)),
        (Icon::Camera, with_key(app, "welcome.usage.ocr", Action::RecognizeAndTranslate)),
        (
            Icon::Replace,
            // Replace has no hotkey out of the box: say how to give it one
            // rather than starting the sentence with nothing.
            Some(
                with_key(app, "welcome.usage.replace", Action::ReplaceSelection)
                    .unwrap_or_else(|| app.t("welcome.usage.replace_unset")),
            ),
        ),
        (Icon::Expand, Some(app.locale.fmt("welcome.usage.zoom", SHELL.command_key))),
        (Icon::Globe, Some(app.t("welcome.usage.services"))),
    ]
    .into_iter()
    .filter_map(|(icon, tip)| tip.map(|t| (icon, t)))
    .collect();
    for (i, (icon, tip)) in tips.into_iter().enumerate() {
        card(ui, pal, |ui| {
            ui.horizontal(|ui| {
                // A numbered circle, then the tip beside its icon.
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 13.0, pal.accent_soft);
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    FontId::proportional(13.0),
                    pal.accent,
                );
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui.painter(), r, icon, pal.weak);
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(tip).size(13.5).color(pal.text)).wrap());
            });
        });
        ui.add_space(8.0);
    }
}

fn storage(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette, st: &mut WelcomeState) {
    page_header(ui, pal, Icon::Lock, &app.t("welcome.storage.title"), Some(&app.t("options.basics.secret_storage")));
    let cfg = app.engine.config();
    let keychain = app.engine.secret_store_name();
    let mut storage = cfg.general.secret_storage;
    let system_name = glossa_platform::secrets::system_store_name();
    ui.columns(2, |cols| {
        if choice_card(
            &mut cols[0],
            pal,
            Icon::Save,
            &app.t("welcome.storage.file_title"),
            &app.t("welcome.storage.file_text"),
            storage == SecretStorage::File,
            true,
        )
        .clicked()
        {
            storage = SecretStorage::File;
        }
        let text = if keychain.is_some() {
            app.t("welcome.storage.system_text")
        } else {
            app.t("options.basics.secrets_none")
        };
        if choice_card(
            &mut cols[1],
            pal,
            Icon::Lock,
            system_name,
            &text,
            storage == SecretStorage::System,
            keychain.is_some(),
        )
        .clicked()
        {
            storage = SecretStorage::System;
        }
    });
    if storage != cfg.general.secret_storage {
        let _ = app.engine.update_config(|c| c.general.secret_storage = storage);
    }
    ui.add_space(14.0);
    let look = app.look.clone();
    card(ui, pal, |ui| {
        card_title(ui, pal, &app.t("welcome.storage.data"));
        let paths = app.engine.paths().clone();
        let key = if paths.home_override {
            "welcome.storage.home_now"
        } else if paths.portable {
            "welcome.storage.portable_now"
        } else {
            "welcome.storage.profile_now"
        };
        // The sentence without its path, then the path on its own line.
        let sentence = app.locale.fmt(key, "").trim_end().trim_end_matches(':').to_owned();
        ui.label(RichText::new(sentence).color(pal.text));
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(pal.card)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(paths.data_dir.display().to_string()).monospace().size(11.5).color(pal.weak),
                    )
                    .wrap(),
                );
            });
        if !paths.portable && !paths.home_override && SHELL.portable_beside_exe && paths.exe_dir_writable() {
            ui.add_space(8.0);
            ui.checkbox(&mut st.portable, app.t("welcome.storage.make_portable"));
            ui.label(
                RichText::new(app.locale.fmt("welcome.storage.portable_note", &paths.exe_dir.display().to_string()))
                    .size(12.0)
                    .color(look.weak_text()),
            );
        }
    });
}

/// One of two big choices: an icon, a title and what it means; chosen
/// shows the accent around it and a filled radio dot.
fn choice_card(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: Icon,
    title: &str,
    text: &str,
    chosen: bool,
    enabled: bool,
) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(if chosen { pal.accent_soft } else { pal.card })
        .stroke(Stroke::new(if chosen { 2.0 } else { 1.0 }, if chosen { pal.accent } else { pal.card_border }))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(egui::Margin::same(16));
    let inner = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(120.0);
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                badge(ui, pal, icon, 36.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    let ring = if chosen { pal.accent } else { pal.weak };
                    ui.painter().circle_stroke(r.center(), 8.0, Stroke::new(1.5, ring));
                    if chosen {
                        ui.painter().circle_filled(r.center(), 4.5, pal.accent);
                    }
                });
            });
            ui.add_space(10.0);
            ui.label(RichText::new(title).size(14.5).strong().color(pal.text));
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(pal.weak)).wrap());
        });
    });
    let resp = ui.interact(inner.response.rect, inner.response.id.with("choice"), Sense::click());
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if enabled {
        resp
    } else {
        resp.on_hover_text(text)
    }
}

fn permissions(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette) {
    use glossa_platform::permissions::{self as perm, Permission, Status};
    page_header(ui, pal, Icon::Gear, &app.t("options.permissions"), Some(&app.t("options.permissions.intro")));
    // Grants happen in System Settings, outside this window: look again now
    // and then.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    let look = app.look.clone();
    for &p in Permission::all() {
        let (icon, name, why) = match p {
            Permission::Accessibility => {
                (Icon::Replace, "options.permissions.accessibility", "options.permissions.accessibility_why")
            }
            Permission::ScreenRecording => {
                (Icon::Camera, "options.permissions.screen", "options.permissions.screen_why")
            }
            Permission::InputMonitoring => {
                (Icon::Keyboard, "options.permissions.input", "options.permissions.input_why")
            }
        };
        let granted = perm::status(p) == Status::Granted;
        card(ui, pal, |ui| {
            ui.horizontal(|ui| {
                badge(ui, pal, icon, 36.0);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(app.t(name)).size(14.5).strong().color(pal.text));
                        ui.add_space(4.0);
                        let (text, colour) = if granted {
                            (app.t("options.permissions.granted"), Color32::from_rgb(0x34, 0xc7, 0x59))
                        } else {
                            (app.t("options.permissions.missing"), look.error())
                        };
                        status_pill(ui, &text, colour);
                    });
                    ui.add(egui::Label::new(RichText::new(app.t(why)).size(12.0).color(pal.weak)).wrap());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if !granted && primary_button(ui, pal, &app.t("options.permissions.request")).clicked() {
                            perm::request(p);
                        }
                        if secondary_button(ui, &app.t("options.permissions.open_settings")).clicked() {
                            if let Err(e) = perm::open_settings(p) {
                                app.notify(e.to_string(), true);
                            }
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);
    }
    ui.label(RichText::new(app.t("options.permissions.rebuild_note")).size(11.5).color(pal.weak));
}

/// A rounded label in a faint version of `colour`: a permission's state.
fn status_pill(ui: &mut egui::Ui, text: &str, colour: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), colour);
    let size = galley.size() + vec2(16.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let soft = Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 36);
    ui.painter().rect_filled(rect, CornerRadius::same((size.y / 2.0) as u8), soft);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, colour);
}

fn done(app: &mut GlossaApp, ui: &mut egui::Ui, pal: &Palette, st: &WelcomeState) {
    let look = app.look.clone();
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 46.0, pal.accent_soft);
        ui.painter().circle_filled(r.center(), 34.0, pal.accent);
        icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(36.0, 36.0)), Icon::Check, pal.accent_text);
        ui.add_space(16.0);
        ui.label(RichText::new(app.t("welcome.done.title")).size(28.0).strong().color(pal.text));
        ui.add_space(18.0);
    });
    card(ui, pal, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(app.t("welcome.done.try")).size(13.5).strong().color(pal.text));
            ui.add_space(4.0);
            ui.label(RichText::new(app.t("welcome.done.select")).size(13.0).color(pal.weak));
            ui.add_space(12.0);
            let popup = app.engine.config().hotkeys.bindings.get(&Action::ShowPopup).cloned();
            let none = app.t("welcome.hotkeys.none");
            // Centre the caps: measure them once, then lay out at that width.
            let id = ui.make_persistent_id("done-caps");
            let width: f32 = ui.data(|d| d.get_temp(id).unwrap_or(0.0));
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                let r = keycaps(ui, &look, pal, popup.as_ref(), &none, true);
                if (r.rect.width() - width).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(id, r.rect.width()));
                    ui.ctx().request_repaint();
                }
            });
            ui.add_space(6.0);
        });
    });
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(app.t("welcome.done.later")).size(12.0).color(pal.weak));
        if st.portable {
            ui.add_space(6.0);
            ui.label(RichText::new(app.t("welcome.done.restart")).size(12.0).color(pal.text));
        }
    });
}

fn finish(app: &mut GlossaApp, st: &mut WelcomeState) {
    let _ = app.engine.update_config(|c| {
        c.general.show_welcome = false;
        // The guide said where Glossa lives; the main window's note would
        // say it again.
        c.general.show_tray_hint = false;
    });
    app.cfg.general.show_tray_hint = false;
    if st.portable {
        match app.engine.paths().make_portable() {
            Ok(_) => app.restart(),
            Err(e) => st.message = Some((app.locale.fmt("welcome.storage.portable_failed", &e.to_string()), true)),
        }
    }
}
