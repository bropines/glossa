//! The main window, laid out like the classic one: navigation and the active
//! service on top, the source pane, a bar with the language direction, the
//! translation pane, and the service tabs along the bottom.

use egui::{vec2, RichText, Sense};
use glossa_core::config::{ResultsLayout, StartupAction};
use glossa_core::{Action, Capabilities, LangId, LangPair};

use crate::app::GlossaApp;
use crate::icons::{self, Icon};
use crate::style::c;

pub fn show(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    let ctx = ui.ctx().clone();
    let custom = crate::chrome::custom(&app.cfg);

    app.text_zoom_keys(ui);
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

    crate::chrome::frame(ui, &look, custom, &crate::chrome::Caption::new("Glossa"));
    let frame =
        egui::Frame::new().fill(look.window_back()).inner_margin(egui::Margin::same(if custom { 1 } else { 6 }));
    egui::CentralPanel::no_frame().frame(frame).show(ui, |ui| {
        let inner = egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6));
        inner.show(ui, |ui| {
            if app.cfg.general.show_top_pane {
                top_pane(app, ui);
            }
            banners(app, ui);
            // The service strip is a bottom panel: it always gets its room,
            // whatever the theme's sizes, and the panes share the rest.
            if app.cfg.general.show_services_pane && app.cfg.general.results_layout == ResultsLayout::Tabs {
                egui::Panel::bottom("main-service-strip").frame(egui::Frame::NONE).show(ui, |ui| {
                    ui.add_space(6.0);
                    service_strip(app, ui);
                });
            }
            let spacing = ui.spacing().item_spacing.y;
            let middle_h = if app.cfg.general.show_middle_pane {
                ui.spacing().interact_size.y.max(26.0) + 8.0 + 2.0 * spacing
            } else {
                6.0 + 2.0 * spacing
            };
            let avail = ui.available_height() - middle_h - 4.0;
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
        });
    });
    toast(app, &ctx);
}

/// Notes across the top of the window, each with a button to put it away:
/// the first-run hint that Glossa lives in the tray, and problems the
/// program met while starting.
fn banners(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    let radius = look.theme.metrics.button_radius.max(4.0);
    let banner = |ui: &mut egui::Ui, text: &str, error: bool, dismiss: &str| -> bool {
        let edge = if error { look.error() } else { look.accent() };
        let mut dismissed = false;
        egui::Frame::new()
            .fill(edge.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, edge.gamma_multiply(0.6)))
            .corner_radius(radius)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let room = (ui.available_width() - 100.0).max(120.0);
                    ui.add_sized(
                        vec2(room, 0.0),
                        egui::Label::new(RichText::new(text).color(if error { look.error() } else { look.text() }))
                            .wrap(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dismissed = ui.button(dismiss).clicked();
                    });
                });
            });
        ui.add_space(6.0);
        dismissed
    };
    if !app.startup_warnings.is_empty() {
        let text = format!(
            "{}\n{}",
            app.t("main.startup_warnings"),
            app.startup_warnings.iter().map(|w| format!("- {w}")).collect::<Vec<_>>().join("\n")
        );
        if banner(ui, &text, true, &app.t("main.hint_dismiss")) {
            app.startup_warnings.clear();
        }
    }
    if app.cfg.general.show_tray_hint && banner(ui, &tray_hint(app), false, &app.t("main.hint_dismiss")) {
        app.cfg.general.show_tray_hint = false;
        let _ = app.engine.update_config(|c| c.general.show_tray_hint = false);
    }
}

/// The first-run hint, with the hotkey that translates a selection as it
/// is set.
fn tray_hint(app: &GlossaApp) -> String {
    let hotkey = app.cfg.hotkeys.bindings.get(&Action::ShowPopup).filter(|_| app.cfg.hotkeys.enabled);
    match hotkey {
        Some(h) => app.locale.fmt("main.tray_hint", &crate::widgets::hotkey_label(h)),
        None => app.t("main.tray_hint_no_hotkey"),
    }
}

fn top_pane(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let back = icons::sized_button(
            ui,
            &look,
            Icon::Back,
            &crate::widgets::shortcut_text(&app.t("msg.back_tip")),
            24.0,
            !app.main.back.is_empty(),
        );
        if back.clicked() {
            navigate(app, true);
        }
        let forward = icons::sized_button(
            ui,
            &look,
            Icon::Forward,
            &crate::widgets::shortcut_text(&app.t("msg.forward_tip")),
            24.0,
            !app.main.forward.is_empty(),
        );
        if forward.clicked() {
            navigate(app, false);
        }
        // A right click on either, greyed out or not (a disabled button
        // takes no clicks itself), lists the recent texts.
        let arrows = back.rect.union(forward.rect);
        let right_click =
            ui.input(|i| i.pointer.secondary_clicked() && i.pointer.interact_pos().is_some_and(|p| arrows.contains(p)));
        let menu_id = back.id.with("recent-menu");
        crate::dropdown::context_menu_in(ui, menu_id, right_click, arrows.left_bottom(), |ui| recent_menu(app, ui));
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
            crate::dropdown::menu(ui, &menu_btn, |ui| main_menu(app, ui));
            if icons::sized_button(ui, &look, Icon::Gear, &app.t("menu.options"), 24.0, true).clicked() {
                app.perform(Action::ShowOptions, None);
            }
            if icons::sized_button(ui, &look, Icon::Keyboard, &app.t("menu.keyboard"), 24.0, true).clicked() {
                app.open_keyboard();
            }
            let ocr_btn = icons::sized_button(ui, &look, Icon::Camera, &app.t("action.text_recognition"), 24.0, true);
            crate::dropdown::menu(ui, &ocr_btn, |ui| ocr_menu(app, ui));
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

/// The back and forward buttons' right-click menu: the texts translated
/// recently (this session's, then history's), and the whole history.
fn recent_menu(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let mut recent: Vec<String> = Vec::new();
    let session = app.main.back.iter().rev().chain(app.main.forward.iter().rev()).cloned();
    let history = app.engine.recent_history(20).into_iter().map(|e| e.source);
    for s in session.chain(history) {
        if !s.trim().is_empty() && !recent.contains(&s) {
            recent.push(s);
        }
        if recent.len() >= 10 {
            break;
        }
    }
    if !recent.is_empty() {
        ui.label(RichText::new(app.t("main.recent")).small().color(app.look.weak_text()));
    }
    for s in recent {
        let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        let short: String =
            if flat.chars().count() > 60 { flat.chars().take(59).chain(['…']).collect() } else { flat };
        if ui.button(short).clicked() {
            app.main.source = s;
            retranslate(app);
            ui.close();
        }
    }
    ui.separator();
    if ui.button(app.t("history.show_full")).clicked() {
        app.perform(Action::ShowHistory, None);
        ui.close();
    }
}

/// The camera button's menu: every way to recognise a screen region, with
/// its hotkey where one is set.
fn ocr_menu(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let items = [
        (Action::TextRecognition, "ocr.menu.recognize"),
        (Action::RecognizeAndTranslate, "ocr.menu.recognize_translate"),
        (Action::RecognizeToClipboard, "ocr.menu.copy"),
        (Action::TranslateRegionInPlace, "ocr.menu.in_place"),
        (Action::TranslateScreenInPlace, "ocr.menu.screen_in_place"),
        (Action::WatchRegion, "ocr.menu.watch"),
        (Action::RepeatLastRecognition, "ocr.menu.repeat"),
    ];
    for (action, key) in items {
        let shortcut = app.cfg.hotkeys.bindings.get(&action).map(crate::widgets::hotkey_label).unwrap_or_default();
        if ui.add(egui::Button::new(app.t(key)).shortcut_text(shortcut)).clicked() {
            ui.close();
            app.perform(action, None);
        }
    }
}

fn main_menu(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let mut g = app.cfg.general.clone();
    let before = g.clone();
    ui.checkbox(&mut g.spell_checking, app.t("menu.spell_checking"));
    ui.checkbox(&mut g.instant_translation, app.t("menu.instant_translation"));
    ui.checkbox(&mut g.back_translation, app.t("menu.back_translation"));
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
    let mut on_top = app.cfg.appearance.main_always_on_top;
    if ui.checkbox(&mut on_top, app.t("msg.always_on_top")).changed() {
        app.cfg.appearance.main_always_on_top = on_top;
        let _ = app.engine.update_config(move |c| c.appearance.main_always_on_top = on_top);
        app.apply_main_window_level();
        // The window raised over everything would now cover this menu.
        ui.close();
    }
    ui.checkbox(&mut g.minimize_to_tray_on_minimize, app.t("menu.tray_on_minimize"));
    ui.checkbox(&mut g.minimize_to_tray_on_close, app.t("menu.tray_on_close"));
    // In line rather than a submenu: the menu is a window of its own, and a
    // submenu inside it would be cut off at its edge.
    ui.label(RichText::new(app.t("menu.on_startup")).color(app.look.weak_text()));
    ui.indent("startup-action", |ui| {
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
        app.open_keyboard();
        ui.close();
    }
    if ui.button(app.t("menu.options")).clicked() {
        app.perform(Action::ShowOptions, None);
        ui.close();
    }
    ui.separator();
    if ui.button(app.t("menu.help")).clicked() {
        app.open_url(HELP_URL);
        ui.close();
    }
    if ui.button(app.t("menu.support")).on_hover_text(glossa_core::SUPPORT_URL).clicked() {
        app.open_url(glossa_core::SUPPORT_URL);
        ui.close();
    }
    if ui.button(app.t("menu.about")).clicked() {
        app.open_about();
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

/// Where the Help item leads: the documentation.
const HELP_URL: &str = "https://github.com/bropines/glossa/tree/main/docs";

fn source_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let font = look.text_font();
    app.fonts.ensure(&app.ctx, &app.main.source);
    let inner_h = height - 2.0 * PANE_MARGIN as f32;
    let shown = pane_frame(&look).show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), inner_h));
        ui.set_max_height(inner_h);
        let rect = ui.max_rect();
        egui::ScrollArea::vertical().id_salt("source").max_height(inner_h).show(ui, |ui| {
            // No frame and no margin of its own: the pane's frame gives the
            // same inset as the translation pane's.
            // Right-to-left and mixed-direction text needs its own layout
            // (see `bidi`); the rest goes the default way.
            let color = c(look.theme.edit.normal.text);
            let halign = if glossa_core::text::guessed_direction(&app.main.source).is_rtl() {
                egui::Align::RIGHT
            } else {
                egui::Align::LEFT
            };
            let layout_font = font.clone();
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                crate::bidi::editor_layout(ui, buf.as_str(), &layout_font, color, wrap_width, halign)
            };
            let edit = egui::TextEdit::multiline(&mut app.main.source)
                .font(font)
                .text_color(color)
                .align(egui::Align2([halign, egui::Align::TOP]))
                .layouter(&mut layouter)
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .hint_text(app.locale.get("main.source_placeholder"))
                .lock_focus(false);
            // As `add_sized`, keeping the output for the spelling's lines.
            let layout = egui::Layout::centered_and_justified(ui.layout().main_dir());
            let out = ui.allocate_ui_with_layout(vec2(ui.available_width(), inner_h), layout, |ui| edit.show(ui)).inner;
            let r = out.response.response.clone();
            if r.changed() {
                source_edited(app);
            }
            spelling(app, ui, &out, &r);
            crate::text_menu::keep_selection(ui.ctx(), &r);
            crate::dropdown::context_menu(ui, &r, |ui| source_menu(app, ui, r.id));
        });
        // Listen in the bottom-right corner, as in the classic window. (Its
        // microphone joins it once there is speech input.)
        let corner = egui::Rect::from_min_size(rect.right_bottom() - vec2(28.0, 24.0), vec2(26.0, 24.0));
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
    });
    app.text_zoom_wheel(ui, shown.response.rect);
}

/// The language to check the source's spelling in: the source language
/// when one is set. With "detect", the one written in the script most of
/// the text is in, of the language the last translation detected and the
/// first and second languages (so checking starts before any translation,
/// and follows the text when it changes language); failing that, the
/// detected one. `None` when spell checking is off or nothing fits.
fn spell_lang(app: &GlossaApp) -> Option<LangId> {
    if !app.cfg.general.spell_checking {
        return None;
    }
    let from = app.cfg.general.pair.from;
    if from.is_language() {
        return Some(from);
    }
    let detected = app.main.outcome.as_ref().map(|o| o.pair.from).filter(|l| l.is_language());
    let g = &app.cfg.general;
    let candidates: Vec<LangId> =
        detected.into_iter().chain([g.first_language, g.second_language]).filter(|l| l.is_language()).collect();
    glossa_platform::spell::guess_language(&app.main.source, &candidates).or(detected)
}

/// Underline the source's misspelled words, and note the one a right
/// click lands on for the context menu.
fn spelling(app: &mut GlossaApp, ui: &egui::Ui, out: &egui::text_edit::TextEditOutput, r: &egui::Response) {
    let lang = spell_lang(app);
    let found = app.main.spell.update(ui.ctx(), &app.main.source, lang);
    // The galley is the text as laid out this frame; draw only over that.
    let current = out.galley.job.text == app.main.source;
    let origin = out.galley_pos - vec2(out.galley.rect.left(), 0.0);
    if current && !found.is_empty() {
        crate::spell::paint(ui, &out.galley, origin, &app.main.source, &found);
    }
    if r.secondary_clicked() {
        let at = ui.ctx().input(|i| i.pointer.interact_pos());
        let word =
            at.filter(|_| current).and_then(|p| crate::spell::hit(&out.galley, origin, &app.main.source, &found, p));
        app.main.spell.open_menu(ui.ctx(), word, lang);
    }
}

/// The source text changed (typed, pasted, cut, undone).
fn source_edited(app: &mut GlossaApp) {
    app.main.edited_at = Some(std::time::Instant::now());
    if app.cfg.general.auto_cleanup_translation {
        app.main.outcome = None;
    }
}

/// The inner margin of the source and translation panes, the same for both
/// so their texts line up.
pub(crate) const PANE_MARGIN: i8 = 4;

/// The frame of the source and translation panes.
pub(crate) fn pane_frame(look: &crate::style::Look) -> egui::Frame {
    egui::Frame::new()
        .fill(look.edit_back())
        .stroke(egui::Stroke::new(1.0, c(look.theme.edit.normal.border)))
        .corner_radius(control_radius(look))
        .inner_margin(egui::Margin::same(PANE_MARGIN))
}

/// The theme's corner radius for controls (4 for the Fluent themes), which
/// the text panes and cards share.
pub(crate) fn control_radius(look: &crate::style::Look) -> f32 {
    look.theme.metrics.button_radius.clamp(2.0, 12.0)
}

fn middle_bar(app: &mut GlossaApp, ui: &mut egui::Ui) {
    let look = app.look.clone();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if icons::sized_button(ui, &look, Icon::Clear, &shortcut_tip(app, "msg.clear"), 26.0, true).clicked() {
            clear_all(app);
        }
        // Recent direction pairs, as the classic ⋮ next to the clear button.
        let pairs_btn = icons::sized_button(ui, &look, Icon::Menu, "", 26.0, true);
        crate::dropdown::menu(ui, &pairs_btn, |ui| {
            for p in app.cfg.general.recent_pairs.clone() {
                if ui.button(format!("{} → {}", crate::langs::name(p.from), crate::langs::name(p.to))).clicked() {
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
        // What is left goes to the two language buttons (their labels are
        // cut short to fit). The Translate button keeps its word while the
        // languages get a sensible width, and becomes an icon when not.
        let avail = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let icon_w = (26.0 * look.icon_scale).round();
        let translate_label = app.t("main.translate");
        let text_w = {
            let font = egui::TextStyle::Button.resolve(ui.style());
            let galley = ui.fonts_mut(|f| f.layout_no_wrap(translate_label.clone(), font, egui::Color32::WHITE));
            galley.size().x + 2.0 * ui.spacing().button_padding.x
        };
        let translate_w = text_w.max(110.0);
        let combos = |translate: f32| ((avail - icon_w - translate - 3.0 * gap - 1.0) / 2.0).floor();
        let compact = combos(translate_w) < 120.0;
        let combo_w = combos(if compact { icon_w } else { translate_w }).max(60.0);
        let disabled = app.cfg.languages.disabled.clone();
        let pair = app.cfg.general.pair;
        let auto = app.t("main.auto_detect");
        let detected = app.main.outcome.as_ref().map(|o| o.pair.from).filter(|l| l.is_language());
        let from_label = match (pair.from, detected) {
            (LangId::AUTO, Some(d)) => app.locale.fmt("main.auto_detect_as", &crate::langs::name(d)),
            _ => auto.clone(),
        };
        if let Some(l) = crate::widgets::lang_combo(
            ui,
            "from",
            pair.from,
            true,
            &from_label,
            &app.t("lang.search"),
            &disabled,
            combo_w,
        ) {
            app.set_pair(LangPair::new(l, pair.to));
            retranslate(app);
        }
        if icons::sized_button(ui, &look, Icon::Swap, &shortcut_tip(app, "msg.swap_tip"), 26.0, true).clicked() {
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
            (LangId::AUTO, Some(t)) => app.locale.fmt("main.auto_detect_as", &crate::langs::name(t)),
            _ => auto.clone(),
        };
        if let Some(l) =
            crate::widgets::lang_combo(ui, "to", pair.to, true, &to_label, &app.t("lang.search"), &disabled, combo_w)
        {
            app.set_pair(LangPair::new(app.cfg.general.pair.from, l));
            retranslate(app);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = shortcut_tip(app, "msg.translate_tip");
            let b = if compact {
                icons::sized_button(ui, &look, Icon::Translate, &tip, 26.0, true)
            } else {
                ui.add(egui::Button::new(translate_label).min_size(vec2(translate_w, 26.0))).on_hover_text(tip)
            };
            if b.clicked() {
                retranslate(app);
            }
        });
    });
    ui.add_space(4.0);
}

/// A tooltip whose text names a Ctrl shortcut. The strings (and classic
/// `lang.json` files) spell it "Ctrl+Enter"; the shortcut itself is egui's
/// `command` modifier, which is Cmd on macOS.
fn shortcut_tip(app: &GlossaApp, key: &str) -> String {
    let text = app.t(key);
    let shell = glossa_platform::shell::SHELL;
    if shell.command_is_ctrl {
        text
    } else {
        text.replace("Ctrl+", &format!("{}+", shell.command_key))
    }
}

fn retranslate(app: &mut GlossaApp) {
    app.main.translated_source.clear();
    app.translate_main();
}

fn result_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let inner_h = height - 2.0 * PANE_MARGIN as f32;
    let shown = pane_frame(&look).show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), inner_h));
        ui.set_max_height(inner_h);
        let rect = ui.max_rect();
        egui::ScrollArea::vertical().id_salt("result").stick_to_bottom(app.main.busy).show(ui, |ui| {
            if let Some(partial) = app.main.partial.clone().filter(|_| app.main.busy) {
                // A streaming service is still writing.
                let rtl = glossa_core::text::guessed_direction(&partial).is_rtl();
                let text = RichText::new(partial).font(look.text_font()).color(c(look.theme.edit.normal.text));
                crate::bidi::label(ui, text, rtl);
            } else if let Some(err) = app.main.error.clone() {
                ui.label(RichText::new(err).color(look.error()).font(look.text_font()));
            } else if let Some(o) = app.main.outcome.clone() {
                let text = o.translation.text.clone();
                app.fonts.ensure(&app.ctx, &text);
                let rtl = o.pair.to.is_rtl() || glossa_core::text::guessed_direction(&text).is_rtl();
                {
                    // Read-only, but selectable with a cursor of its own, so
                    // part of it can be copied from the menu as well as
                    // with the keys.
                    let color = c(look.theme.edit.normal.text);
                    let font = look.text_font();
                    let halign = if rtl { egui::Align::RIGHT } else { egui::Align::LEFT };
                    let layout_font = font.clone();
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        crate::bidi::editor_layout(ui, buf.as_str(), &layout_font, color, wrap_width, halign)
                    };
                    let mut shown = text.as_str();
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .id_salt("translation")
                            .font(font)
                            .text_color(color)
                            .align(egui::Align2([halign, egui::Align::TOP]))
                            .layouter(&mut layouter)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .desired_width(f32::INFINITY)
                            .desired_rows(1),
                    );
                    crate::text_menu::keep_selection(ui.ctx(), &r);
                    crate::dropdown::context_menu(ui, &r, |ui| translation_menu(app, ui, r.id, &text, o.pair.to));
                }
                // The rest of the result follows the text zoom too.
                let body = ui.style().text_styles.get(&egui::TextStyle::Body).map_or(14.0, |f| f.size);
                let size = body * look.text_scale;
                if let Some(r) = &o.translation.romanisation {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("{}{r}", app.t("msg.romanization")))
                            .size(size)
                            .color(look.weak_text())
                            .italics(),
                    );
                }
                for entry in &o.translation.parts_of_speech {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&entry.part_of_speech).size(size).strong());
                    for s in &entry.senses {
                        let back = if s.back_translations.is_empty() {
                            String::new()
                        } else {
                            format!("  ({})", s.back_translations.join(", "))
                        };
                        let line = RichText::new(format!("    {}{back}", s.word)).size(size);
                        ui.label(crate::bidi::auto(ui, line));
                    }
                }
                if !o.failed.is_empty() {
                    ui.add_space(6.0);
                    let names: Vec<&str> = o.failed.iter().map(|(id, _)| id.as_str()).collect();
                    ui.label(
                        RichText::new(app.locale.fmt("main.tried_first", &names.join(", ")))
                            .small()
                            .color(look.weak_text()),
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
    app.text_zoom_wheel(ui, shown.response.rect);
}

fn back_translation_pane(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let Some(back) = app.main.outcome.as_ref().and_then(|o| o.back.clone()) else { return };
    let frame = egui::Frame::new()
        .fill(c(look.theme.window.client_back))
        .stroke(egui::Stroke::new(1.0, look.border()))
        .corner_radius(control_radius(&look))
        .inner_margin(egui::Margin::same(PANE_MARGIN));
    let shown = frame.show(ui, |ui| {
        ui.set_min_size(vec2(ui.available_width(), height - 2.0 * PANE_MARGIN as f32));
        egui::ScrollArea::vertical().id_salt("back").show(ui, |ui| {
            ui.label(RichText::new(app.t("menu.back_translation")).small().color(look.weak_text()));
            ui.add(
                egui::Label::new(RichText::new(&back.text).font(look.text_font()).color(look.weak_text()))
                    .selectable(true)
                    .wrap(),
            );
        });
    });
    app.text_zoom_wheel(ui, shown.response.rect);
}

fn stacked_results(app: &mut GlossaApp, ui: &mut egui::Ui, height: f32) {
    let look = app.look.clone();
    let reg = app.engine.registry();
    let shown = egui::ScrollArea::vertical().id_salt("stacked").max_height(height).show(ui, |ui| {
        for id in app.cfg.services.enabled.clone() {
            let Some(p) = reg.get(&id) else { continue };
            let frame = egui::Frame::new()
                .fill(look.edit_back())
                .stroke(egui::Stroke::new(1.0, look.border()))
                .corner_radius(control_radius(&look))
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
                                RichText::new(app.locale.fmt("msg.ms", &o.translation.elapsed_ms.to_string()))
                                    .small()
                                    .color(look.weak_text()),
                            );
                        });
                    }
                });
                match app.main.stacked.get(&id) {
                    Some(Ok(o)) => {
                        app.fonts.ensure(&app.ctx, &o.translation.text);
                        let text = &o.translation.text;
                        let rtl = glossa_core::text::guessed_direction(text).is_rtl();
                        crate::bidi::label(ui, RichText::new(text).font(look.text_font()), rtl);
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
    app.text_zoom_wheel(ui, shown.inner_rect);
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

/// The source pane's context menu: undo and redo, the clipboard at the
/// cursor or over the selection, select all, and the selection (or the
/// whole text) translated, looked up or read aloud.
fn source_menu(app: &mut GlossaApp, ui: &mut egui::Ui, edit: egui::Id) {
    use crate::text_menu as tm;
    let ctx = ui.ctx().clone();
    let range = tm::selection(&ctx, edit);
    let selected = tm::selected(&app.main.source, range).map(str::to_owned);
    let has_text = !app.main.source.is_empty();
    let can_undo = tm::can_undo(&ctx, edit, &app.main.source, false);
    let can_redo = tm::can_undo(&ctx, edit, &app.main.source, true);
    let locale = app.locale.clone();
    let button =
        |ui: &mut egui::Ui, enabled: bool, key: &str| ui.add_enabled(enabled, egui::Button::new(locale.get(key)));
    let mut changed = false;
    // A misspelled word under the pointer: its suggestions first.
    if let Some(m) = app.main.spell.menu_word(&app.main.source) {
        match &m.suggestions {
            None => {
                ui.add_enabled(false, egui::Button::new("…"));
            }
            Some(list) if list.is_empty() => {
                ui.add_enabled(false, egui::Button::new(locale.get("edit.no_suggestions")));
            }
            Some(list) => {
                for s in list {
                    if ui.button(egui::RichText::new(s).strong()).clicked() {
                        tm::remember_for_undo(&ctx, edit, &app.main.source);
                        let range = m.word.range.clone();
                        let at = app.main.source[..range.start].chars().count() + s.chars().count();
                        app.main.source.replace_range(range, s);
                        tm::set_selection(&ctx, edit, tm::cursor_at(at));
                        changed = true;
                        ui.close();
                    }
                }
            }
        }
        if ui.button(locale.get("edit.ignore_word")).clicked() {
            app.main.spell.ignore(&m.word.word);
            ui.close();
        }
        if glossa_platform::spell::CAN_ADD && ui.button(locale.get("edit.add_to_dictionary")).clicked() {
            app.main.spell.add(&ctx, &m.word.word, m.lang);
            ui.close();
        }
        ui.separator();
    }
    if button(ui, can_undo, "edit.undo").clicked() {
        changed |= tm::undo(&ctx, edit, &mut app.main.source, false);
        ui.close();
    }
    if button(ui, can_redo, "edit.redo").clicked() {
        changed |= tm::undo(&ctx, edit, &mut app.main.source, true);
        ui.close();
    }
    ui.separator();
    if button(ui, selected.is_some(), "edit.cut").clicked() {
        if let Some(s) = &selected {
            app.copy_text(s);
            tm::remember_for_undo(&ctx, edit, &app.main.source);
            let at = tm::replace(&mut app.main.source, range, "");
            tm::set_selection(&ctx, edit, tm::cursor_at(at));
            changed = true;
        }
        ui.close();
    }
    if button(ui, selected.is_some(), "edit.copy").clicked() {
        if let Some(s) = &selected {
            app.copy_text(s);
        }
        ui.close();
    }
    if ui.button(app.t("edit.paste")).clicked() {
        if let Some(t) = app.clipboard_text() {
            tm::remember_for_undo(&ctx, edit, &app.main.source);
            let at = tm::replace(&mut app.main.source, range, &t);
            tm::set_selection(&ctx, edit, tm::cursor_at(at));
            changed = true;
        }
        ui.close();
    }
    if button(ui, has_text, "edit.select_all").clicked() {
        tm::select_all(&ctx, edit, &app.main.source);
        ui.close();
    }
    ui.separator();
    if button(ui, has_text, "edit.copy_all").clicked() {
        let t = app.main.source.clone();
        app.copy_text(&t);
        ui.close();
    }
    if button(ui, has_text, "edit.cut_all").clicked() {
        tm::remember_for_undo(&ctx, edit, &app.main.source);
        let t = std::mem::take(&mut app.main.source);
        app.copy_text(&t);
        tm::set_selection(&ctx, edit, tm::cursor_at(0));
        changed = true;
        ui.close();
    }
    ui.separator();
    // The selection, or all of it.
    let subject = selected.clone().unwrap_or_else(|| app.main.source.clone());
    if button(ui, has_text, "edit.translate").clicked() {
        if selected.is_some() {
            app.main.source = subject.clone();
        }
        retranslate(app);
        ui.close();
    }
    if button(ui, has_text, "edit.dictionary").clicked() {
        crate::dictionary::open(app, Some(subject.clone()));
        ui.close();
    }
    if button(ui, has_text, "edit.listen").clicked() {
        app.speak(&subject, LangId::AUTO);
        ui.close();
    }
    if changed {
        source_edited(app);
    }
}

/// The translation pane's context menu: copy the selection or all of it,
/// select all, read aloud, look up.
fn translation_menu(app: &mut GlossaApp, ui: &mut egui::Ui, edit: egui::Id, text: &str, lang: LangId) {
    use crate::text_menu as tm;
    let ctx = ui.ctx().clone();
    let selected = tm::selected(text, tm::selection(&ctx, edit)).map(str::to_owned);
    if ui.add_enabled(selected.is_some(), egui::Button::new(app.t("edit.copy"))).clicked() {
        if let Some(s) = &selected {
            app.copy_text(s);
        }
        ui.close();
    }
    if ui.button(app.t("edit.copy_all")).clicked() {
        app.copy_text(text);
        ui.close();
    }
    if ui.button(app.t("edit.select_all")).clicked() {
        tm::select_all(&ctx, edit, text);
        ui.close();
    }
    ui.separator();
    let subject = selected.unwrap_or_else(|| text.to_owned());
    if ui.button(app.t("edit.listen")).clicked() {
        app.speak(&subject, lang);
        ui.close();
    }
    if ui.button(app.t("edit.dictionary")).clicked() {
        crate::dictionary::open(app, Some(subject));
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
        app.main.partial = None;
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
            egui::Frame::popup(ui.style()).corner_radius(ui.visuals().menu_corner_radius).show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.label(RichText::new(&t.text).color(col));
            });
        });
}
