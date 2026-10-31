//! The main window, laid out like the classic one: navigation and the active
//! service on top, the source pane, a bar with the language direction, the
//! translation pane, and the service tabs along the bottom.

use egui::{vec2, Color32, RichText, Sense};
use glossa_core::config::{ResultsLayout, StartupAction};
use glossa_core::{Action, Capabilities, LangId, LangPair};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};
use crate::style::c;

pub fn show(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    let ctx = ui.ctx().clone();
    let custom = app.cfg.appearance.custom_frame;

    // Keyboard shortcuts of the classic window.
    let (go, swap, clear, back, fwd) = ctx.input(|i| {
        (
            i.modifiers.command && i.key_pressed(egui::Key::Enter),
            i.modifiers.command && i.key_pressed(egui::Key::I),
            i.modifiers.command && i.key_pressed(egui::Key::N),
            i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft),
            i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight),
        )
    });
    if go {
        app.main.translated_source.clear();
        app.translate_main();
    }
    if swap {
        app.perform(Action::SwapLanguages, None);
    }
    if clear {
        clear_all(app);
    }
    if back {
        navigate(app, true);
    }
    if fwd {
        navigate(app, false);
    }

    // Files dropped on the window: text files are read, images are OCRed.
    let dropped = ctx.input(|i| i.raw.dropped_files.clone());
    for f in dropped {
        let path = f.path().to_owned();
        if !path.as_os_str().is_empty() {
            on_drop(app, &path);
        }
    }

    let frame = egui::Frame::new()
        .fill(look.window_back())
        .inner_margin(egui::Margin::same(if custom { 1 } else { 6 }))
        .stroke(if custom {
            egui::Stroke::new(look.theme.metrics.border_width, look.border())
        } else {
            egui::Stroke::NONE
        });
    egui::CentralPanel::no_frame().frame(frame).show(ui, |ui| {
        if custom {
            title_bar(app, ui);
            resize_edges(ui);
        }
        let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6));
        inner.show(ui, |ui| {
            if app.cfg.general.show_top_pane {
                top_pane(app, ui);
            }
            let services_h =
                if app.cfg.general.show_services_pane { look.theme.metrics.service_tab_height + 8.0 } else { 0.0 };
            let middle_h = if app.cfg.general.show_middle_pane { 34.0 } else { 6.0 };
            let status_h = if app.toast.is_some() || app.main.error.is_some() { 0.0 } else { 0.0 };
            let avail = ui.available_height() - services_h - middle_h - status_h - 8.0;
            let back_pane =
                app.cfg.general.back_translation && app.main.outcome.as_ref().is_some_and(|o| o.back.is_some());
            let split = app.cfg.general.show_middle_pane;
            let top_h = if split { (avail * 0.48).max(60.0) } else { (avail * 0.5).max(60.0) };
            let bottom_h = (avail - top_h).max(60.0);

            source_pane(app, ui, top_h);
            if app.cfg.general.show_middle_pane {
                middle_bar(app, ui);
            } else {
                ui.add_space(6.0);
            }
            if app.cfg.general.results_layout == ResultsLayout::Stacked {
                stacked_results(app, ui, bottom_h);
            } else {
                let (main_h, back_h) =
                    if back_pane { (bottom_h * 0.62, bottom_h * 0.38 - 6.0) } else { (bottom_h, 0.0) };
                result_pane(app, ui, main_h);
                if back_pane {
                    ui.add_space(6.0);
                    back_translation_pane(app, ui, back_h);
                }
            }
            if app.cfg.general.show_services_pane && app.cfg.general.results_layout == ResultsLayout::Tabs {
                ui.add_space(6.0);
                service_strip(app, ui);
            }
        });
    });
    toast(app, &ctx);
}

fn title_bar(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    let h = 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click_and_drag());
    let caption =
        look.theme.window.caption.map(c).unwrap_or(c(look.theme.window.back)).gamma_multiply(look.window_alpha);
    ui.painter().rect_filled(rect, 0.0, caption);
    let icon_rect = egui::Rect::from_min_size(rect.min + vec2(10.0, 7.0), vec2(16.0, 16.0));
    crate::icons::paint(ui.painter(), icon_rect, Icon::Translate, look.accent());
    ui.painter().text(
        rect.min + vec2(34.0, h / 2.0),
        egui::Align2::LEFT_CENTER,
        "Glossa",
        egui::FontId::proportional(look.theme.fonts.size * 1.4),
        look.text(),
    );
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
    // Caption buttons, right to left.
    let bw = 40.0;
    let mut x = rect.right();
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    for (icon, which) in
        [(Icon::Close, 0), (if maximized { Icon::Restore } else { Icon::Maximize }, 1), (Icon::Minimize, 2)]
    {
        x -= bw;
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(bw, h));
        let b = ui.interact(r, ui.scope_id().with(("caption", which)), Sense::click());
        let state = if b.is_pointer_button_down_on() {
            glossa_core::theme::WidgetState::Pressed
        } else if b.hovered() {
            glossa_core::theme::WidgetState::Hover
        } else {
            glossa_core::theme::WidgetState::Normal
        };
        let g = look.theme.window_button.get(state);
        if let Some(back) = g.back {
            ui.painter().rect_filled(
                r,
                0.0,
                if which == 0 && b.hovered() { Color32::from_rgb(196, 43, 28) } else { c(back) },
            );
        } else if which == 0 && b.hovered() {
            ui.painter().rect_filled(r, 0.0, Color32::from_rgb(196, 43, 28));
        }
        crate::icons::paint(ui.painter(), egui::Rect::from_center_size(r.center(), vec2(11.0, 11.0)), icon, c(g.main));
        if b.clicked() {
            match which {
                0 => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                1 => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                _ => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            }
        }
    }
}

/// Invisible handles along the edges of an undecorated window.
fn resize_edges(ui: &mut egui::Ui) {
    use egui::viewport::ResizeDirection as R;
    let screen = ui.ctx().content_rect();
    let t = 5.0;
    let zones = [
        (
            egui::Rect::from_min_max(screen.left_top(), egui::pos2(screen.right(), screen.top() + t)),
            R::North,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            egui::Rect::from_min_max(egui::pos2(screen.left(), screen.bottom() - t), screen.right_bottom()),
            R::South,
            egui::CursorIcon::ResizeVertical,
        ),
        (
            egui::Rect::from_min_max(screen.left_top(), egui::pos2(screen.left() + t, screen.bottom())),
            R::West,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            egui::Rect::from_min_max(egui::pos2(screen.right() - t, screen.top()), screen.right_bottom()),
            R::East,
            egui::CursorIcon::ResizeHorizontal,
        ),
        (
            egui::Rect::from_min_size(screen.right_bottom() - vec2(12.0, 12.0), vec2(12.0, 12.0)),
            R::SouthEast,
            egui::CursorIcon::ResizeNwSe,
        ),
    ];
    for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
        let r = ui.interact(rect, egui::Id::unique(("resize-edge", i)), Sense::drag());
        if r.hovered() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if r.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }
}

fn top_pane(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icons::sized_button(ui, &look, Icon::Back, &app.t("msg.back_tip"), 24.0, !app.main.back.is_empty()).clicked()
        {
            navigate(app, true);
        }
        if icons::sized_button(ui, &look, Icon::Forward, &app.t("msg.forward_tip"), 24.0, !app.main.forward.is_empty())
            .clicked()
        {
            navigate(app, false);
        }
        ui.add_space(6.0);
        let (globe, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), globe, Icon::Globe, look.weak_text());
        let id = app.cfg.services.active.clone();
        let name = app.engine.registry().get(&id).map(|p| p.info().name.clone()).unwrap_or_else(|| id.clone());
        let link = ui.add(egui::Link::new(RichText::new(&name).underline().color(look.text())));
        if link.clicked() {
            let pair = app.main.outcome.as_ref().map(|o| o.pair).unwrap_or(app.cfg.general.pair);
            if let Some(url) = app.engine.web_link(&id, &app.main.source, pair) {
                app.open_url(&url);
            }
        }
        if app.main.busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let menu_btn = icons::sized_button(ui, &look, Icon::Menu, "", 24.0, true);
            egui::Popup::menu(&menu_btn).show(|ui| main_menu(app, ui));
            if icons::sized_button(ui, &look, Icon::Gear, &app.t("menu.options"), 24.0, true).clicked() {
                app.perform(Action::ShowOptions, None);
            }
            if icons::sized_button(ui, &look, Icon::Keyboard, &app.t("menu.keyboard"), 24.0, true).clicked() {
                app.keyboard_open = !app.keyboard_open;
            }
            if icons::sized_button(ui, &look, Icon::Camera, &app.t("action.text_recognition"), 24.0, true).clicked() {
                app.perform(Action::TextRecognition, None);
            }
            if icons::sized_button(ui, &look, Icon::Dictionary, &app.t("menu.dictionary"), 24.0, true).clicked() {
                app.perform(
                    Action::ShowDictionary,
                    Some(app.main.source.clone()).filter(|s| glossa_core::text::looks_like_a_dictionary_lookup(s)),
                );
            }
            if icons::sized_button(ui, &look, Icon::History, &app.t("menu.history"), 24.0, true).clicked() {
                app.perform(Action::ShowHistory, None);
            }
        });
    });
    ui.add_space(2.0);
}

fn main_menu(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let mut g = app.cfg.general.clone();
    let before = g.clone();
    ui.checkbox(&mut g.instant_translation, app.t("menu.instant_translation"));
    ui.checkbox(&mut g.back_translation, app.t("menu.back_translation"));
    ui.checkbox(&mut g.read_phonetically, app.t("menu.read_phonetically"));
    ui.checkbox(&mut g.clear_on_drag_drop, app.t("menu.clear_on_drop"));
    ui.checkbox(&mut g.auto_cleanup_translation, app.t("menu.auto_cleanup"));
    ui.checkbox(&mut g.save_contents_on_exit, app.t("menu.save_on_exit"));
    let mut stacked = g.results_layout == ResultsLayout::Stacked;
    if ui.checkbox(&mut stacked, app.t("menu.stacked_results")).changed() {
        g.results_layout = if stacked { ResultsLayout::Stacked } else { ResultsLayout::Tabs };
    }
    ui.separator();
    ui.checkbox(&mut g.show_top_pane, app.t("menu.show_top_pane"));
    ui.checkbox(&mut g.show_middle_pane, app.t("menu.show_middle_pane"));
    ui.checkbox(&mut g.show_services_pane, app.t("menu.show_services_pane"));
    ui.separator();
    ui.checkbox(&mut g.minimize_to_tray_on_minimize, app.t("menu.tray_on_minimize"));
    ui.checkbox(&mut g.minimize_to_tray_on_close, app.t("menu.tray_on_close"));
    ui.menu_button(app.t("menu.on_startup"), |ui| {
        ui.radio_value(&mut g.startup_action, StartupAction::ShowWindow, app.t("menu.startup_show"));
        ui.radio_value(&mut g.startup_action, StartupAction::MinimizeToTray, app.t("menu.startup_tray"));
        ui.radio_value(&mut g.startup_action, StartupAction::RestorePrevious, app.t("menu.startup_restore"));
    });
    ui.separator();
    if ui.button(app.t("menu.dictionary")).clicked() {
        app.perform(Action::ShowDictionary, None);
        ui.close();
    }
    if ui.button(app.t("menu.history")).clicked() {
        app.perform(Action::ShowHistory, None);
        ui.close();
    }
    if ui.button(app.t("menu.keyboard")).clicked() {
        app.keyboard_open = true;
        ui.close();
    }
    if ui.button(app.t("menu.options")).clicked() {
        app.perform(Action::ShowOptions, None);
        ui.close();
    }
    ui.separator();
    if ui.button(app.t("menu.about")).clicked() {
        app.about_open = true;
        ui.close();
    }
    if ui.button(app.t("menu.exit")).clicked() {
        app.quit();
    }
    if g != before {
        app.cfg.general = g.clone();
        let _ = app.engine.update_config(move |c| c.general = g);
    }
}

fn source_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let font = look.text_font();
    app.fonts.ensure(&app.ctx, &app.main.source);
    let frame = egui::Frame::new()
        .fill(look.edit_back())
        .stroke(egui::Stroke::new(1.0, c(look.theme.edit.normal.border)))
        .corner_radius(2);
    frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), height));
        ui.set_max_height(height);
        let rect = ui.max_rect();
        egui::ScrollArea::vertical().id_salt("source").max_height(height).show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut app.main.source)
                .font(font)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .hint_text(app.locale.get("main.source_placeholder"))
                .lock_focus(false);
            let r = ui.add_sized(vec2(ui.available_width(), height - 6.0), edit);
            if r.changed() {
                app.main.edited_at = Some(std::time::Instant::now());
                if app.cfg.general.auto_cleanup_translation {
                    app.main.outcome = None;
                }
            }
            r.context_menu(|ui| edit_menu(app, ui, true));
        });
        // Listen and microphone in the bottom-right corner, as in the classic
        // window.
        let corner = egui::Rect::from_min_size(rect.right_bottom() - vec2(54.0, 26.0), vec2(52.0, 24.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(corner).layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if icons::sized_button(
            &mut child,
            &look,
            Icon::Listen,
            &app.t("msg.listen_tip"),
            22.0,
            !app.main.source.trim().is_empty(),
        )
        .clicked()
        {
            let lang = app.main.outcome.as_ref().map(|o| o.pair.from).unwrap_or(app.cfg.general.pair.from);
            let text = app.main.source.clone();
            app.speak(&text, lang);
        }
        let mic_tip = app.locale.fmt("msg.speech_on", app.cfg.general.speech_language.english_name());
        if icons::sized_button(&mut child, &look, Icon::Mic, &mic_tip, 22.0, true).clicked() {
            app.perform(Action::SpeechInput, None);
        }
    });
}

fn middle_bar(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if icons::sized_button(ui, &look, Icon::Clear, &app.t("msg.clear"), 26.0, true).clicked() {
            clear_all(app);
        }
        // Recent direction pairs, as the classic ⋮ next to the clear button.
        let pairs_btn = icons::sized_button(ui, &look, Icon::Menu, "", 26.0, true);
        egui::Popup::menu(&pairs_btn).show(|ui| {
            for p in app.cfg.general.recent_pairs.clone() {
                if ui.button(format!("{} → {}", p.from.english_name(), p.to.english_name())).clicked() {
                    app.set_pair(p);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(app.t("lang.reset")).clicked() {
                app.set_pair(LangPair::new(LangId::AUTO, LangId::AUTO));
                ui.close();
            }
            let mut always = app.cfg.general.always_detect_language;
            if ui.checkbox(&mut always, app.t("lang.always_detect")).changed() {
                let _ = app.engine.update_config(move |c| c.general.always_detect_language = always);
            }
        });
        let avail = ui.available_width();
        let combo_w = ((avail - 26.0 - 120.0 - 30.0) / 2.0).max(90.0);
        let disabled = app.cfg.languages.disabled.clone();
        let pair = app.cfg.general.pair;
        let auto = app.t("main.auto_detect");
        let detected = app.main.outcome.as_ref().map(|o| o.pair.from).filter(|l| l.is_language());
        let from_label = match (pair.from, detected) {
            (LangId::AUTO, Some(d)) => app.locale.fmt("main.auto_detect_as", d.english_name()),
            _ => auto.clone(),
        };
        if let Some(l) = crate::widgets::lang_combo(ui, "from", pair.from, true, &from_label, &disabled, combo_w) {
            app.set_pair(LangPair::new(l, pair.to));
            retranslate(app);
        }
        if icons::sized_button(ui, &look, Icon::Swap, &app.t("msg.swap_tip"), 26.0, true).clicked() {
            let p = app.cfg.general.pair;
            let resolved = app.main.outcome.as_ref().map(|o| o.pair).unwrap_or(p);
            let swapped = if p.is_swappable() { p.swapped() } else { resolved.swapped() };
            app.set_pair(swapped);
            if let Some(o) = app.main.outcome.clone() {
                app.main.source = o.translation.text.clone();
            }
            retranslate(app);
        }
        let target = app.main.outcome.as_ref().map(|o| o.pair.to).filter(|l| l.is_language());
        let to_label = match (pair.to, target) {
            (LangId::AUTO, Some(t)) => app.locale.fmt("main.auto_detect_as", t.english_name()),
            _ => auto.clone(),
        };
        if let Some(l) = crate::widgets::lang_combo(ui, "to", pair.to, true, &to_label, &disabled, combo_w) {
            app.set_pair(LangPair::new(app.cfg.general.pair.from, l));
            retranslate(app);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = ui
                .add(egui::Button::new(app.t("main.translate")).min_size(vec2(110.0, 26.0)))
                .on_hover_text(app.t("msg.translate_tip"));
            if b.clicked() {
                retranslate(app);
            }
        });
    });
    ui.add_space(4.0);
}

fn retranslate(app: &mut GlossaApp) {
    app.main.translated_source.clear();
    app.translate_main();
}

fn result_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let frame = egui::Frame::new()
        .fill(look.edit_back())
        .stroke(egui::Stroke::new(1.0, c(look.theme.edit.normal.border)))
        .corner_radius(2)
        .inner_margin(egui::Margin::same(4));
    frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), height - 8.0));
        ui.set_max_height(height - 8.0);
        let rect = ui.max_rect();
        egui::ScrollArea::vertical().id_salt("result").show(ui, |ui| {
            if let Some(err) = app.main.error.clone() {
                ui.label(RichText::new(err).color(look.error()).font(look.text_font()));
            } else if let Some(o) = app.main.outcome.clone() {
                let text = o.translation.text.clone();
                app.fonts.ensure(&app.ctx, &text);
                let rtl = o.pair.to.is_rtl() || glossa_core::text::guessed_direction(&text).is_rtl();
                let shown = if glossa_core::text::needs_bidi(&text) {
                    glossa_core::text::for_display(&text, rtl)
                } else {
                    text.clone()
                };
                let layout = if rtl {
                    egui::Layout::top_down(egui::Align::Max)
                } else {
                    egui::Layout::top_down(egui::Align::Min)
                };
                ui.with_layout(layout, |ui| {
                    let r = ui.add(
                        egui::Label::new(
                            RichText::new(shown).font(look.text_font()).color(c(look.theme.edit.normal.text)),
                        )
                        .selectable(true)
                        .wrap(),
                    );
                    r.context_menu(|ui| {
                        if ui.button(app.t("edit.copy_all")).clicked() {
                            app.copy_text(&text);
                            ui.close();
                        }
                        if ui.button(app.t("edit.listen")).clicked() {
                            app.speak(&text, o.pair.to);
                            ui.close();
                        }
                        if ui.button(app.t("edit.dictionary")).clicked() {
                            crate::dictionary::open(app, Some(text.clone()));
                            ui.close();
                        }
                    });
                });
                if let Some(r) = &o.translation.romanisation {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("{}{r}", app.t("msg.romanization"))).color(look.weak_text()).italics(),
                    );
                }
                for entry in &o.translation.parts_of_speech {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&entry.part_of_speech).strong());
                    for s in &entry.senses {
                        let back = if s.back_translations.is_empty() {
                            String::new()
                        } else {
                            format!("  ({})", s.back_translations.join(", "))
                        };
                        ui.label(format!("    {}{back}", s.word));
                    }
                }
                if !o.failed.is_empty() {
                    ui.add_space(6.0);
                    let names: Vec<&str> = o.failed.iter().map(|(id, _)| id.as_str()).collect();
                    ui.label(
                        RichText::new(format!("Tried first: {}", names.join(", "))).small().color(look.weak_text()),
                    )
                    .on_hover_text(o.failed.iter().map(|(id, e)| format!("{id}: {e}")).collect::<Vec<_>>().join("\n"));
                }
            }
        });
        let has = app.main.outcome.is_some();
        let corner = egui::Rect::from_min_size(rect.right_bottom() - vec2(54.0, 24.0), vec2(52.0, 24.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(corner).layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if icons::sized_button(&mut child, &look, Icon::Listen, &app.t("msg.listen_tip"), 22.0, has).clicked() {
            if let Some(o) = app.main.outcome.clone() {
                app.speak(&o.translation.text, o.pair.to);
            }
        }
        if icons::sized_button(&mut child, &look, Icon::Copy, &app.t("popup.copy"), 22.0, has).clicked() {
            if let Some(o) = app.main.outcome.clone() {
                app.copy_text(&o.translation.text);
            }
        }
    });
}

fn back_translation_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let Some(back) = app.main.outcome.as_ref().and_then(|o| o.back.clone()) else { return };
    let frame = egui::Frame::new()
        .fill(c(look.theme.window.client_back))
        .stroke(egui::Stroke::new(1.0, look.border()))
        .corner_radius(2)
        .inner_margin(egui::Margin::same(4));
    frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), height - 8.0));
        egui::ScrollArea::vertical().id_salt("back").show(ui, |ui| {
            ui.label(RichText::new(app.t("menu.back_translation")).small().color(look.weak_text()));
            ui.add(
                egui::Label::new(RichText::new(&back.text).font(look.text_font()).color(look.weak_text()))
                    .selectable(true)
                    .wrap(),
            );
        });
    });
}

fn stacked_results(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let reg = app.engine.registry();
    egui::ScrollArea::vertical().id_salt("stacked").max_height(height).show(ui, |ui| {
        for id in app.cfg.services.enabled.clone() {
            let Some(p) = reg.get(&id) else { continue };
            let frame = egui::Frame::new()
                .fill(look.edit_back())
                .stroke(egui::Stroke::new(1.0, look.border()))
                .corner_radius(3)
                .inner_margin(egui::Margin::same(6));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    crate::widgets::service_badge(ui, &id, &p.info().name, 16.0);
                    ui.label(RichText::new(&p.info().name).strong());
                    if let Some(Ok(o)) = app.main.stacked.get(&id).cloned() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::sized_button(ui, &look, Icon::Copy, "", 20.0, true).clicked() {
                                app.copy_text(&o.translation.text);
                            }
                            ui.label(
                                RichText::new(format!("{} ms", o.translation.elapsed_ms))
                                    .small()
                                    .color(look.weak_text()),
                            );
                        });
                    }
                });
                match app.main.stacked.get(&id) {
                    Some(Ok(o)) => {
                        app.fonts.ensure(&app.ctx, &o.translation.text);
                        ui.add(
                            egui::Label::new(RichText::new(&o.translation.text).font(look.text_font()))
                                .selectable(true)
                                .wrap(),
                        );
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(look.error()));
                    }
                    None if app.main.stacked_jobs.values().any(|v| *v == id) => {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                    None => {}
                }
            });
            ui.add_space(4.0);
        }
    });
}

fn service_strip(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    let reg = app.engine.registry();
    let services: Vec<(String, String)> = app
        .cfg
        .services
        .enabled
        .iter()
        .filter_map(|id| {
            reg.get(id)
                .filter(|p| p.info().capabilities.contains(Capabilities::TRANSLATE))
                .map(|p| (id.clone(), p.info().name.clone()))
        })
        .collect();
    if services.is_empty() {
        return;
    }
    let each = (ui.available_width() / services.len() as f32).clamp(32.0, 160.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        for (id, name) in &services {
            let selected = *id == app.cfg.services.active;
            if crate::widgets::service_tab(ui, &look, id, name, selected, each - 1.0).clicked() && !selected {
                clicked = Some(id.clone());
            }
        }
    });
    if let Some(id) = clicked {
        app.set_active_service(&id);
    }
}

pub fn edit_menu(app: &mut GlossaApp, ui: &mut egui::Ui, source: bool) {
    if ui.button(app.t("edit.copy_all")).clicked() {
        let t = app.main.source.clone();
        app.copy_text(&t);
        ui.close();
    }
    if source && ui.button(app.t("edit.cut_all")).clicked() {
        let t = std::mem::take(&mut app.main.source);
        app.copy_text(&t);
        ui.close();
    }
    if source && ui.button(app.t("edit.paste")).clicked() {
        if let Some(t) = app.clipboard_text() {
            app.main.source.push_str(&t);
            app.main.edited_at = Some(std::time::Instant::now());
        }
        ui.close();
    }
    ui.separator();
    if ui.button(app.t("edit.translate")).clicked() {
        retranslate(app);
        ui.close();
    }
    if ui.button(app.t("edit.dictionary")).clicked() {
        let t = app.main.source.clone();
        crate::dictionary::open(app, Some(t));
        ui.close();
    }
    if ui.button(app.t("edit.listen")).clicked() {
        let t = app.main.source.clone();
        app.speak(&t, LangId::AUTO);
        ui.close();
    }
}

fn clear_all(app: &mut GlossaApp) {
    app.main.source.clear();
    app.main.outcome = None;
    app.main.error = None;
    app.main.translated_source.clear();
    app.main.stacked.clear();
}

fn navigate(app: &mut GlossaApp, backwards: bool) {
    let (from, to) = if backwards {
        (&mut app.main.back, &mut app.main.forward)
    } else {
        (&mut app.main.forward, &mut app.main.back)
    };
    if let Some(text) = from.pop() {
        if !app.main.translated_source.is_empty() {
            to.push(std::mem::take(&mut app.main.translated_source));
        }
        app.main.source = text.clone();
        app.main.translated_source = text;
        let pair = app.cfg.general.pair;
        let opts =
            glossa_engine::TranslateOptions { provider: Some(app.cfg.services.active.clone()), ..Default::default() };
        app.main.busy = true;
        app.main.job = app.engine.translate_async(glossa_engine::Slot::Main, app.main.source.clone(), pair, opts);
    }
}

fn on_drop(app: &mut GlossaApp, path: &std::path::Path) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif") {
        match std::fs::read(path) {
            Ok(bytes) => {
                let to =
                    Some(app.cfg.general.pair.to).filter(|l| l.is_language()).or(Some(app.cfg.general.first_language));
                crate::ocr::recognize_bytes(app, bytes, to);
            }
            Err(e) => app.notify(e.to_string(), true),
        }
        return;
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if app.cfg.general.clear_on_drag_drop {
                app.main.source = text;
            } else {
                app.main.source.push_str(&text);
            }
            retranslate(app);
        }
        Err(e) => app.notify(e.to_string(), true),
    }
}

fn toast(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(t) = &app.toast else { return };
    let look = &app.look;
    let col = if t.error { look.error() } else { look.text() };
    egui::Area::new(egui::Id::unique("toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -58.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.label(RichText::new(&t.text).color(col));
            });
        });
}

pub fn show_about(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.about_open {
        return;
    }
    let look = app.look.clone();
    let builder = egui::ViewportBuilder::default()
        .with_title(app.t("msg.about"))
        .with_inner_size([440.0, 330.0])
        .with_resizable(false)
        .with_icon(crate::app_icon(32));
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-about"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Translate, look.accent());
                ui.heading("Glossa");
                ui.label(format!("{}{}", app.t("msg.version"), glossa_core::VERSION));
                ui.add_space(6.0);
                ui.label("A clean-room reincarnation of QTranslate.");
                ui.label(
                    RichText::new("Translation, dictionaries and screen OCR — Rust, egui, no web engine.")
                        .small()
                        .color(look.weak_text()),
                );
                ui.add_space(10.0);
                ui.label(RichText::new("OCR: chrome-lens-py by Bropines (MIT)").small());
                ui.label(RichText::new("Scripting: Boa (JavaScript), CPython modules, native modules").small());
                ui.label(
                    RichText::new("Compatible with classic QTranslate services, themes, locales, and pot-app plugins")
                        .small()
                        .color(look.weak_text()),
                );
                ui.add_space(10.0);
                let s = app.engine.module_status();
                ui.label(
                    RichText::new(format!(
                        "{} providers · {} modules loaded · {} failed",
                        app.engine.registry().entries().len(),
                        s.loaded.len(),
                        s.failed.len()
                    ))
                    .small()
                    .color(look.weak_text()),
                );
            });
        });
    });
    app.about_open = open;
}

/// The on-screen keyboard: types into the main window's source pane, in the
/// first language's layout or English.
pub fn show_keyboard(app: &mut GlossaApp, ctx: &egui::Context) {
    if !app.keyboard_open {
        return;
    }
    let builder = egui::ViewportBuilder::default()
        .with_title(app.t("msg.keyboard"))
        .with_inner_size([620.0, 230.0])
        .with_icon(crate::app_icon(32));
    let mut open = true;
    let id = egui::Id::unique("keyboard-layout");
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("glossa-keyboard"), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let mut russian: bool =
                ui.data(|d| d.get_temp(id)).unwrap_or(app.cfg.general.first_language == LangId::RUSSIAN);
            let mut shift: bool = ui.data(|d| d.get_temp(id.with("shift"))).unwrap_or(false);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut russian, false, "EN");
                ui.selectable_value(&mut russian, true, "RU");
                ui.toggle_value(&mut shift, "Shift");
            });
            let rows_en = ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"];
            let rows_ru = ["ё1234567890-=", "йцукенгшщзхъ\\", "фывапролджэ", "ячсмитьбю."];
            let rows = if russian { rows_ru } else { rows_en };
            for row in rows {
                ui.horizontal(|ui| {
                    for ch in row.chars() {
                        let label = if shift { ch.to_uppercase().to_string() } else { ch.to_string() };
                        if ui.add(egui::Button::new(&label).min_size(vec2(34.0, 30.0))).clicked() {
                            app.main.source.push_str(&label);
                            app.main.edited_at = Some(std::time::Instant::now());
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Space").min_size(vec2(260.0, 30.0))).clicked() {
                    app.main.source.push(' ');
                }
                if ui.add(egui::Button::new("Backspace").min_size(vec2(60.0, 30.0))).clicked() {
                    app.main.source.pop();
                }
                if ui.add(egui::Button::new("Enter").min_size(vec2(60.0, 30.0))).clicked() {
                    app.main.source.push('\n');
                }
            });
            ui.data_mut(|d| {
                d.insert_temp(id, russian);
                d.insert_temp(id.with("shift"), shift);
            });
        });
    });
    app.keyboard_open = open;
}
