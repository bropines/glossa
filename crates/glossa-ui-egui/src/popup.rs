//! The selection popup.
//!
//! Built to match what the classic program does (see
//! `docs/research/classic-behaviour.md`): it opens at the mouse pointer offset
//! by 15 px, is 350–600 px wide depending on the text, has a one-line header
//! "<From> to <To> (<Service>)" with favourite / dictionary / listen / copy /
//! replace icons, shows only the translation, grows a strip of service tabs
//! while the pointer is over it, and hides itself a few seconds after the
//! pointer leaves — unless it was dragged, which pins it.

use std::sync::Arc;
use std::time::Instant;

use egui::{vec2, Color32, RichText, Sense};
use glossa_core::{Action, Capabilities, Error, LangId, LangPair};
use glossa_engine::{Slot, TranslateOptions, TranslationOutcome};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};

pub struct PopupState {
    pub source: String,
    pub job: u64,
    pub outcome: Option<Arc<TranslationOutcome>>,
    pub error: Option<String>,
    pub busy: bool,
    /// While a streaming service writes: the translation so far.
    pub partial: Option<String>,
    pub service: String,
    /// Screen position of the window's top-left, in points.
    pub pos: egui::Pos2,
    pub size: egui::Vec2,
    pub pinned: bool,
    /// The pointer position it was opened at, in physical pixels, while it is
    /// placed automatically (kept on screen as its size changes). `None` once
    /// it sits where the user put it.
    pub cursor_px: Option<(i32, i32)>,
    /// The user dragged it: remember where it ends up.
    pub moved: bool,
    /// Last time the pointer was over the popup (or it was opened).
    pub last_hover: Instant,
    pub hovered: bool,
    pub favorite: bool,
    /// A result from the OCR watch, shown without a translation round trip.
    pub from_ocr: bool,
}

const ID: &str = "glossa-popup";

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(ID)
}

/// Open (or reuse) the popup for `text`, near the pointer.
pub fn open(app: &mut GlossaApp, text: String, cursor: Option<(i32, i32)>) {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return;
    }
    app.fonts.ensure(&app.ctx, &text);
    let service = popup_service(app);
    let (pos, cursor_px) = anchor(app, cursor);
    let min_w = app.cfg.appearance.popup.min_width;
    let job = translate(app, &text, &service);
    let now = Instant::now();
    app.popup = Some(PopupState {
        source: text,
        job,
        outcome: None,
        error: None,
        busy: true,
        partial: None,
        service,
        pos,
        size: vec2(min_w, 60.0),
        pinned: false,
        cursor_px,
        moved: false,
        last_hover: now,
        hovered: false,
        favorite: false,
        from_ocr: false,
    });
    app.ctx.request_repaint();
}

/// Show a ready-made result (from the OCR watch) in the popup.
pub fn show_text(app: &mut GlossaApp, source: String, outcome: Option<Arc<TranslationOutcome>>, near: (i32, i32)) {
    app.fonts.ensure(&app.ctx, &source);
    if let Some(o) = &outcome {
        app.fonts.ensure(&app.ctx, &o.translation.text);
    }
    let now = Instant::now();
    let keep_pos = app.popup.as_ref().filter(|p| p.from_ocr).map(|p| (p.pos, p.pinned, None));
    let (pos, pinned, cursor_px) = keep_pos.unwrap_or_else(|| {
        let (pos, cursor_px) = anchor(app, Some(near));
        (pos, false, cursor_px)
    });
    app.popup = Some(PopupState {
        source,
        job: 0,
        busy: false,
        partial: None,
        error: None,
        service: outcome.as_ref().map(|o| o.translation.provider_id.clone()).unwrap_or_default(),
        outcome,
        pos,
        size: vec2(app.cfg.appearance.popup.min_width, 60.0),
        pinned,
        cursor_px,
        moved: false,
        last_hover: now,
        hovered: false,
        favorite: false,
        from_ocr: true,
    });
    app.ctx.request_repaint();
}

fn popup_service(app: &GlossaApp) -> String {
    let s = &app.cfg.services;
    if !s.popup_active.is_empty() && s.enabled.contains(&s.popup_active) {
        s.popup_active.clone()
    } else {
        s.active.clone()
    }
}

fn translate(app: &mut GlossaApp, text: &str, service: &str) -> u64 {
    // The popup always runs the smart direction: first language <-> second.
    let pair = LangPair::new(LangId::AUTO, LangId::AUTO);
    let opts = TranslateOptions { provider: Some(service.to_owned()).filter(|s| !s.is_empty()), ..Default::default() };
    app.engine.translate_async(Slot::Popup, text.to_owned(), pair, opts)
}

/// Where the popup opens, in points, and the pointer position to keep it
/// placed against (see [`PopupState::cursor_px`]).
///
/// With "auto position" on: pointer + 15 px, flipped to the other side of the
/// pointer near the screen's edges once its size is known. Off: where the
/// user last left it, if they ever moved it.
fn anchor(app: &GlossaApp, cursor: Option<(i32, i32)>) -> (egui::Pos2, Option<(i32, i32)>) {
    if !app.cfg.appearance.popup.auto_position {
        if let Some(g) = app.cfg.windows.get(REMEMBERED) {
            return (egui::pos2(g.x, g.y), None);
        }
    }
    let (x, y) = cursor.or_else(glossa_platform::window::cursor_position).unwrap_or((200, 200));
    let scale = scale_at(x, y);
    (egui::pos2((x as f32 + 15.0 * scale) / scale, (y as f32 + 15.0 * scale) / scale), Some((x, y)))
}

/// The key in `config.windows` for where the user last put the popup.
const REMEMBERED: &str = "popup";

fn scale_at(x: i32, y: i32) -> f32 {
    glossa_platform::window::monitor_at(x, y).map(|m| m.scale).filter(|s| *s > 0.0).unwrap_or(1.0)
}

/// Keep an automatically placed popup of `size` (points) on the screen of
/// the pointer: below-right of it, else flipped above or left.
fn placed(cursor_px: (i32, i32), size: egui::Vec2) -> egui::Pos2 {
    let (cx, cy) = cursor_px;
    let scale = scale_at(cx, cy);
    let anchor = glossa_core::ocr::Rect::new(cx as f32, cy as f32, 15.0 * scale, 15.0 * scale);
    let (x, y) = glossa_platform::window::place_popup(
        anchor,
        (size.x * scale, size.y * scale),
        &glossa_platform::window::monitors(),
    );
    egui::pos2(x / scale, y / scale)
}

impl PopupState {
    pub fn on_result(
        &mut self,
        job: u64,
        result: Result<Arc<TranslationOutcome>, Arc<Error>>,
        message: Option<String>,
    ) {
        if job != self.job {
            return;
        }
        self.busy = false;
        self.partial = None;
        match result {
            Ok(o) => {
                self.outcome = Some(o);
                self.error = None;
            }
            Err(_) => self.error = message,
        }
        self.last_hover = Instant::now();
    }

    pub fn on_progress(&mut self, job: u64, text: String) {
        if job == self.job && self.busy {
            self.partial = Some(text);
        }
    }

    pub fn on_detached(&mut self, _job: u64, _result: Result<Arc<TranslationOutcome>, Arc<Error>>) {}

    fn title(&self, app: &GlossaApp) -> String {
        let service_name = app
            .engine
            .registry()
            .get(&self.service)
            .map(|p| p.info().name.clone())
            .or_else(|| {
                self.outcome
                    .as_ref()
                    .and_then(|o| app.engine.registry().get(&o.translation.provider_id).map(|p| p.info().name.clone()))
            })
            .unwrap_or_default();
        match &self.outcome {
            Some(o) => {
                let short = short_service(&service_name);
                format!("{} to {} ({short})", o.pair.from.english_name(), o.pair.to.english_name())
            }
            None if self.busy => app.t("msg.translating"),
            None => service_name,
        }
    }
}

/// "Microsoft Translator" -> "Microsoft": the header has little room.
fn short_service(name: &str) -> String {
    name.trim_end_matches(" Translator").trim_end_matches(" Translate").to_owned()
}

/// Auto-hide.
pub fn tick(app: &mut GlossaApp) {
    let Some(p) = app.popup.as_ref() else { return };
    let delay = app.cfg.appearance.popup.auto_hide_secs;
    if p.pinned || p.hovered || p.busy || delay == 0 {
        return;
    }
    let left = std::time::Duration::from_secs(delay as u64).saturating_sub(p.last_hover.elapsed());
    if left.is_zero() {
        app.popup = None;
    } else {
        app.ctx.request_repaint_after(left);
    }
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut state) = app.popup.take() else { return };
    let cfg = &app.cfg.appearance.popup;
    let builder = egui::ViewportBuilder::default()
        .with_title("Glossa")
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_transparent(true)
        .with_active(cfg.activate_on_open)
        .with_position(state.pos)
        .with_inner_size(state.size);
    let mut close = false;
    let mut desired = state.size;
    let mut outer: Option<egui::Rect> = None;
    ctx.show_viewport_immediate(viewport_id(), builder, |ui, _class| {
        let (c, r) = body(app, ui, &mut state);
        close = c;
        desired = r;
        outer = ui.input(|i| i.viewport().outer_rect);
    });
    // Once dragged, remember where it went (for "auto position" off), and
    // stop placing it automatically.
    if state.moved {
        state.cursor_px = None;
        if let Some(o) = outer {
            let g = glossa_core::config::WindowGeometry {
                x: o.min.x,
                y: o.min.y,
                width: o.width(),
                height: o.height(),
                maximized: false,
            };
            if app.cfg.windows.get(REMEMBERED) != Some(&g) {
                app.cfg.windows.insert(REMEMBERED.into(), g);
            }
        }
    }
    if close {
        return;
    }
    // Fit the window to its content, and keep it on screen at its new size.
    if (desired - state.size).length() > 1.0 {
        tracing::trace!(?desired, old = ?state.size, "popup resized");
        state.size = desired;
        ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::InnerSize(desired));
        if let Some(c) = state.cursor_px {
            let pos = placed(c, desired);
            if (pos - state.pos).length() > 1.0 {
                state.pos = pos;
                ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::OuterPosition(pos));
            }
        }
    }
    app.popup = Some(state);
}

/// Draw the popup. Returns (close requested, desired size).
fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut PopupState) -> (bool, egui::Vec2) {
    let look = app.look.clone();
    let cfg = app.cfg.appearance.popup.clone();
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested());
    let hovered_now = ui.input(|i| i.pointer.has_pointer());
    if hovered_now {
        st.last_hover = Instant::now();
    }
    st.hovered = hovered_now;

    let fill = look.popup_back.gamma_multiply(look.popup_opacity);
    let frame = egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), (look.popup_opacity * 255.0) as u8))
        .stroke(egui::Stroke::new(look.popup_frame_width.max(0.0), look.popup_frame))
        .inner_margin(egui::Margin::symmetric(6, 4));
    let text_color = look.popup_text;
    let max_w = cfg.max_width;
    let min_w = cfg.min_width;
    // Width follows the content: measure the text wrapped at the maximum, and
    // the header, and take the wider, within the configured bounds.
    let font = egui::FontId::proportional(look.text_size * 1.33);
    let body_text = st
        .partial
        .clone()
        .filter(|_| st.busy)
        .or_else(|| st.outcome.as_ref().map(|o| display_text(o)))
        .or_else(|| st.error.clone())
        .unwrap_or_default();
    let wrap_w = (max_w - 16.0).max(60.0);
    let text_w = ui.ctx().fonts_mut(|f| f.layout(body_text, font.clone(), Color32::WHITE, wrap_w)).size().x;
    let title_w = ui
        .ctx()
        .fonts_mut(|f| {
            f.layout_no_wrap(st.title(app), egui::FontId::proportional(look.text_size * 1.15), Color32::WHITE)
        })
        .size()
        .x
        + 14.0
        + 5.0 * 19.0
        + 16.0;
    // "Auto size" off: a fixed box, the maximum size, scrolling inside.
    let desired_w = if cfg.auto_size { (text_w.max(title_w) + 16.0).clamp(min_w, max_w) } else { max_w };

    let mut overflow = 0.0f32;
    let resp = egui::CentralPanel::no_frame().show(ui, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(desired_w - 14.0);
            if !cfg.auto_size {
                // A fixed box: the frame fills the window.
                ui.set_min_height(ui.available_height());
            }
            // -- header: drag handle, title, icons --------------------------
            let header = ui.horizontal(|ui| {
                let (drag_rect, drag) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click_and_drag());
                icons::paint(
                    ui.painter(),
                    drag_rect,
                    if st.pinned { Icon::Pin } else { Icon::Close },
                    look.icon(glossa_core::theme::WidgetState::Normal),
                );
                if drag.clicked() {
                    close = true;
                }
                let title = st.title(app);
                let title_resp = ui.add(
                    egui::Label::new(
                        RichText::new(title).size(look.text_size * 1.15).color(text_color.gamma_multiply(0.85)),
                    )
                    .sense(Sense::click_and_drag())
                    .truncate(),
                );
                if title_resp.drag_started() || drag.drag_started() {
                    st.moved = true;
                    if cfg.pin_when_dragging {
                        st.pinned = true;
                    }
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    let size = 18.0;
                    let translation = st.outcome.as_ref().map(|o| o.translation.text.clone()).unwrap_or_default();
                    let has = !translation.is_empty();
                    if cfg.icons.replace
                        && icons::sized_button(ui, &look, Icon::Replace, &app.t("popup.replace"), size, has).clicked()
                    {
                        app.perform(Action::ReplaceSelection, Some(st.source.clone()));
                        close = true;
                    }
                    if cfg.icons.copy
                        && icons::sized_button(ui, &look, Icon::Copy, &app.t("popup.copy"), size, has).clicked()
                    {
                        app.copy_text(&translation);
                    }
                    if cfg.icons.listen
                        && icons::sized_button(ui, &look, Icon::Listen, &app.t("popup.listen"), size, true).clicked()
                    {
                        // The classic icon read the selected text aloud.
                        let lang = st.outcome.as_ref().map(|o| o.pair.from).unwrap_or(LangId::AUTO);
                        app.speak(&st.source, lang);
                    }
                    if cfg.icons.dictionary
                        && icons::sized_button(ui, &look, Icon::Dictionary, &app.t("popup.dictionary"), size, true)
                            .clicked()
                    {
                        crate::dictionary::open(app, Some(st.source.clone()));
                    }
                    let star = if st.favorite { Icon::StarFilled } else { Icon::Star };
                    let tip = if st.favorite { app.t("msg.remove_favorite") } else { app.t("msg.add_favorite") };
                    if icons::sized_button(ui, &look, star, &tip, size, has && !st.from_ocr).clicked() {
                        st.favorite = !st.favorite;
                        if let Some(entry) = app.engine.history().into_iter().find(|h| h.source == st.source.trim()) {
                            app.engine.set_favorite(entry.id, st.favorite);
                        }
                    }
                });
            });
            let _ = header;

            // -- body ---------------------------------------------------------
            if let Some(partial) = st.partial.clone().filter(|_| st.busy) {
                let out = egui::ScrollArea::vertical().max_height(cfg.max_height - 70.0).stick_to_bottom(true).show(
                    ui,
                    |ui| {
                        ui.add(egui::Label::new(RichText::new(partial).font(font.clone()).color(text_color)).wrap());
                    },
                );
                overflow = (out.content_size.y - out.inner_rect.height()).max(0.0);
            } else if st.busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(app.t("msg.translating")).color(text_color));
                });
            } else if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(look.error()).font(font.clone()));
            } else if let Some(o) = &st.outcome {
                let shown = display_text(o);
                let out = egui::ScrollArea::vertical().max_height(cfg.max_height - 70.0).show(ui, |ui| {
                    let rtl = o.pair.to.is_rtl();
                    let text = if glossa_core::text::needs_bidi(&shown) {
                        glossa_core::text::for_display(&shown, rtl)
                    } else {
                        shown
                    };
                    let label = egui::Label::new(RichText::new(text).font(font.clone()).color(text_color))
                        .selectable(true)
                        .wrap();
                    if rtl {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| ui.add(label));
                    } else {
                        ui.add(label);
                    }
                });
                // The scroll area is no taller than the window is now, so a
                // longer text would never make the window grow: add what is
                // hidden.
                overflow = (out.content_size.y - out.inner_rect.height()).max(0.0);
            }

            // -- service strip, while the pointer is over the popup ----------
            if st.hovered && !st.from_ocr {
                let mut switch_to = None;
                ui.add_space(2.0);
                let reg = app.engine.registry();
                let enabled: Vec<_> = app
                    .cfg
                    .services
                    .enabled
                    .iter()
                    .filter_map(|id| reg.get(id))
                    .filter(|p| p.info().capabilities.contains(Capabilities::TRANSLATE))
                    .collect();
                if !enabled.is_empty() {
                    let avail = ui.available_width();
                    let each = (avail / enabled.len() as f32).max(24.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for p in &enabled {
                            let info = p.info();
                            let selected = info.id == st.service;
                            if crate::widgets::service_tab(ui, &look, &info.id, &info.name, selected, each - 1.0)
                                .clicked()
                                && !selected
                            {
                                switch_to = Some(info.id.clone());
                            }
                        }
                    });
                }
                if let Some(id) = switch_to {
                    st.service = id.clone();
                    st.busy = true;
                    st.partial = None;
                    st.job = translate(app, &st.source, &id);
                    let remembered = id.clone();
                    let _ = app.engine.update_config(move |c| c.services.popup_active = remembered);
                }
            }
        })
    });

    let content = resp.inner.response.rect.size();
    let h = if cfg.auto_size { (content.y + overflow).clamp(36.0, cfg.max_height) } else { cfg.max_height };
    (close, vec2(desired_w.ceil(), h.ceil()))
}

/// What the popup shows: the translation, then any dictionary block the
/// service returned (the classic popup did the same for single words).
fn display_text(o: &TranslationOutcome) -> String {
    o.translation.as_display_block()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_names_are_shortened_for_the_header() {
        assert_eq!(short_service("Microsoft Translator"), "Microsoft");
        assert_eq!(short_service("Google Translate"), "Google");
        assert_eq!(short_service("Yandex"), "Yandex");
    }
}
