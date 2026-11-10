//! The history window: searchable, with favourites, export and per-entry
//! actions.

use egui::RichText;
use glossa_core::history::HistoryEntry;
use glossa_engine::ExportFormat;

use crate::app::GlossaApp;
use crate::icons::{self, Icon};

pub struct HistoryState {
    pub search: String,
    pub favorites_only: bool,
    pub entries: Vec<HistoryEntry>,
    pub dirty: bool,
    pub confirm_clear: bool,
    /// The entry deleted last and when, for the "Deleted. Undo" offer in the
    /// bottom bar, which stays for [`UNDO_FOR`].
    deleted: Option<(HistoryEntry, std::time::Instant)>,
    /// Rows as last drawn, for laying out only the visible ones.
    rows: RowHeights,
}

/// How long a delete can be undone from the bottom bar.
const UNDO_FOR: std::time::Duration = std::time::Duration::from_secs(8);

/// The heights of the history's rows, by entry id, as last drawn: the list
/// lays out only the rows in view (it may hold thousands) and needs the
/// others' heights to place them and to size the scroll bar. Rows not drawn
/// yet are guessed at from those that were.
#[derive(Default)]
struct RowHeights {
    heights: std::collections::HashMap<u64, f32>,
    /// What the heights were measured for: a change in any of these
    /// changes how rows wrap.
    measured_for: Option<(u32, bool, u32)>,
}

impl RowHeights {
    /// Forget the heights when the width, the expanded setting or the text
    /// size changed.
    fn check(&mut self, width: f32, expand: bool, scale: f32) {
        let key = (width.round() as u32, expand, (scale * 100.0).round() as u32);
        if self.measured_for != Some(key) {
            self.heights.clear();
            self.measured_for = Some(key);
        }
    }

    /// A guess for a row not drawn yet: the average of those drawn.
    fn guess(&self) -> f32 {
        if self.heights.is_empty() {
            72.0
        } else {
            self.heights.values().sum::<f32>() / self.heights.len() as f32
        }
    }

    fn get(&self, id: u64, guess: f32) -> f32 {
        self.heights.get(&id).copied().unwrap_or(guess)
    }
}

/// Which rows of `heights` overlap `top..bottom`: the first, the one after
/// the last, and where the first starts.
fn visible_rows(heights: &[f32], top: f32, bottom: f32) -> (usize, usize, f32) {
    let mut y = 0.0;
    let mut first = heights.len();
    let mut first_y = 0.0;
    for (i, h) in heights.iter().enumerate() {
        if y + h > top {
            first = i;
            first_y = y;
            break;
        }
        y += h;
    }
    let mut end = first;
    let mut y = first_y;
    while end < heights.len() && y < bottom {
        y += heights[end];
        end += 1;
    }
    (first, end, first_y)
}

pub fn open(app: &mut GlossaApp) {
    if app.history.is_some() {
        crate::chrome::raise(&app.ctx, crate::chrome::viewport(crate::chrome::HISTORY));
    } else {
        app.history = Some(HistoryState {
            search: String::new(),
            favorites_only: false,
            entries: Vec::new(),
            dirty: true,
            confirm_clear: false,
            deleted: None,
            rows: RowHeights::default(),
        });
    }
    app.ctx.request_repaint();
}

pub fn show(app: &mut GlossaApp, ctx: &egui::Context) {
    let Some(mut st) = app.history.take() else { return };
    if st.dirty {
        st.entries = app.engine.search_history(&st.search, st.favorites_only);
        st.dirty = false;
    }
    let title = app.t("history.title");
    let builder =
        crate::chrome::builder(app, &title).with_inner_size([760.0, 520.0]).with_min_inner_size([420.0, 260.0]);
    let mut open = true;
    let custom = crate::chrome::custom(&app.cfg);
    ctx.show_viewport_immediate(crate::chrome::viewport(crate::chrome::HISTORY), builder, |ui, _| {
        if ui.input(|i| i.viewport().close_requested()) {
            open = false;
        }
        crate::chrome::frame(ui, &app.look, custom, &crate::chrome::Caption::new(&title));
        body(app, ui, &mut st, &mut open);
    });
    app.child_backdrop(&title, false);
    if open {
        app.history = Some(st);
    }
}

fn body(app: &mut GlossaApp, ui: &mut egui::Ui, st: &mut HistoryState, open: &mut bool) {
    let look = app.look.clone();
    let expand = app.cfg.history.expand_items;
    egui::Panel::top("hist-top").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.search)
                    .hint_text(app.t("history.search"))
                    .desired_width(ui.available_width() - 200.0),
            );
            if r.changed() {
                st.dirty = true;
            }
            let star = if st.favorites_only { Icon::StarFilled } else { Icon::Star };
            if icons::sized_button(ui, &look, star, &app.t("history.favorites_only"), 24.0, true).clicked() {
                st.favorites_only = !st.favorites_only;
                st.dirty = true;
            }
            let count = app.locale.fmt("history.count", &st.entries.len().to_string());
            ui.label(RichText::new(count).small().color(look.weak_text()));
        });
        ui.add_space(4.0);
    });
    egui::Panel::bottom("hist-bottom").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(app.t("history.save_as")).clicked() {
                let mut dialog = rfd::FileDialog::new().set_file_name("glossa-history.html");
                for f in ExportFormat::ALL {
                    dialog = dialog.add_filter(app.t(format_key(f)), &[f.extension()]);
                }
                if let Some(path) = dialog.save_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt").to_owned();
                    let fmt = ExportFormat::from_extension(&ext).unwrap_or(ExportFormat::Text);
                    match app.engine.export_history(&path, fmt, st.favorites_only) {
                        Ok(n) => {
                            let (n, path) = (n.to_string(), path.display().to_string());
                            let text = app.locale.fmt_args("history.exported", &[&n, &path]);
                            app.notify(text, false);
                        }
                        Err(e) => app.notify(e.to_string(), true),
                    }
                }
            }
            if st.confirm_clear {
                ui.label(RichText::new(app.t("history.confirm_clear")).color(look.error()));
                if ui.button(app.t("msg.yes")).clicked() {
                    app.engine.clear_history(false);
                    st.confirm_clear = false;
                    st.deleted = None;
                    st.dirty = true;
                }
                if ui.button(app.t("msg.no")).clicked() {
                    st.confirm_clear = false;
                }
            } else if ui.button(app.t("history.clear")).clicked() {
                st.confirm_clear = true;
            }
            // A delete takes effect at once, so it can be taken back for a
            // few seconds rather than asked about every time.
            match st.deleted.as_ref().map(|(_, at)| UNDO_FOR.saturating_sub(at.elapsed())) {
                Some(left) if !left.is_zero() => {
                    ui.separator();
                    ui.label(RichText::new(app.t("history.deleted")).color(look.weak_text()));
                    if ui.button(app.t("edit.undo")).clicked() {
                        if let Some((entry, _)) = st.deleted.take() {
                            app.engine.restore_history(entry);
                            st.dirty = true;
                        }
                    }
                    // Wake once more to take the offer away.
                    ui.ctx().request_repaint_after(left);
                }
                Some(_) => st.deleted = None,
                None => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(app.t("popup.close")).clicked() {
                    *open = false;
                }
            });
        });
        ui.add_space(4.0);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        let mut action: Option<(u64, &'static str)> = None;
        if st.entries.is_empty() {
            // Say why the list is empty: nothing translated yet, the
            // history switched off, or a search that found nothing.
            let filtered = !st.search.trim().is_empty() || st.favorites_only;
            let (title, hint) = if filtered {
                ("history.no_match", "history.no_match_hint")
            } else if app.cfg.history.enabled {
                ("history.empty", "history.empty_hint")
            } else {
                ("history.empty", "history.disabled_hint")
            };
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.35).max(12.0));
                ui.label(RichText::new(app.t(title)).strong());
                ui.add_space(2.0);
                ui.label(RichText::new(app.t(hint)).color(look.weak_text()));
            });
            return;
        }
        // Row icons are the size of the main window's toolbar icons, and the
        // lines under the source start where the source does.
        let icon = 24.0;
        let indent = (icon * look.icon_scale).round() + ui.spacing().item_spacing.x;
        // Entries name their translator by id; show its name ("Google Lens"
        // for a translation Lens made of a picture).
        let registry = app.engine.registry();
        let name_of = |id: &str| registry.get(id).map(|p| p.info().name.clone()).unwrap_or_else(|| id.to_owned());
        // Only the rows in view are laid out (see `RowHeights`).
        st.rows.check(ui.available_width(), expand, look.text_scale);
        let guess = st.rows.guess();
        let heights: Vec<f32> = st.entries.iter().map(|e| st.rows.get(e.id, guess)).collect();
        let mut measured: Vec<(u64, f32)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("hist-list").show_viewport(ui, |ui, view| {
            ui.set_width(ui.available_width());
            let (first, end, first_y) = visible_rows(&heights, view.min.y, view.max.y);
            ui.add_space(first_y);
            for e in &st.entries[first..end] {
                let row_top = ui.cursor().top();
                let tr = e.latest().map(|r| r.text.clone()).unwrap_or_default();
                app.fonts.ensure(&app.ctx, &e.source);
                app.fonts.ensure(&app.ctx, &tr);
                let radius = crate::main_window::control_radius(&look);
                let frame = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).corner_radius(radius);
                // The row senses clicks behind its contents (a sensing
                // scope registers before its children), so the icons on it
                // still get the hover and the clicks.
                let row = egui::UiBuilder::new().id_salt(("hist-row", e.id)).sense(egui::Sense::click());
                let resp = ui
                    .scope_builder(row, |ui| {
                        frame.show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let star = if e.favorite { Icon::StarFilled } else { Icon::Star };
                                let tip = app.t("history.favorite");
                                if icons::sized_button(ui, &look, star, &tip, icon, true).clicked() {
                                    action = Some((e.id, "fav"));
                                }
                                let src = one_line(&e.source, if expand { 400 } else { 90 });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let tip = app.t("history.delete");
                                    if icons::sized_button(ui, &look, Icon::Trash, &tip, icon, true).clicked() {
                                        action = Some((e.id, "delete"));
                                    }
                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        let width = ui.available_width();
                                        let rtl = glossa_core::text::guessed_direction(&src).is_rtl();
                                        let wrap = egui::text::TextWrapping::truncate_at_width(width);
                                        let src = crate::bidi::rich(ui, RichText::new(src).strong(), wrap, rtl);
                                        ui.add(egui::Label::new(src).truncate());
                                    });
                                });
                            });
                            let shown = if expand { tr.clone() } else { one_line(&tr, 110) };
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let shown = crate::bidi::auto(ui, RichText::new(shown).color(look.weak_text()));
                                ui.add(egui::Label::new(shown).wrap());
                            });
                            if let Some(r) = e.latest() {
                                ui.horizontal(|ui| {
                                    ui.add_space(indent);
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {} → {}",
                                            name_of(&r.provider_id),
                                            crate::langs::name(r.from),
                                            crate::langs::name(r.to)
                                        ))
                                        .small()
                                        .color(look.weak_text()),
                                    );
                                });
                            }
                        });
                    })
                    .response;
                if resp.hovered() {
                    ui.painter().rect_stroke(
                        resp.rect,
                        crate::main_window::control_radius(&look),
                        egui::Stroke::new(1.0, look.border()),
                        egui::StrokeKind::Inside,
                    );
                }
                if resp.double_clicked() {
                    action = Some((e.id, "open"));
                }
                crate::dropdown::context_menu(ui, &resp, |ui| {
                    for (key, what) in [
                        ("history.open", "open"),
                        ("history.copy_text", "copy_text"),
                        ("history.copy_translation", "copy_tr"),
                        ("history.listen", "listen"),
                        ("history.delete", "delete"),
                    ] {
                        if ui.button(app.t(key)).clicked() {
                            action = Some((e.id, what));
                            ui.close();
                        }
                    }
                });
                ui.separator();
                let h = ui.cursor().top() - row_top;
                measured.push((e.id, h));
            }
            // The rows below, as room for the scroll bar.
            ui.add_space(heights[end..].iter().sum::<f32>());
        });
        let changed = measured.iter().any(|(id, h)| st.rows.heights.get(id).is_none_or(|old| (old - h).abs() > 0.5));
        st.rows.heights.extend(measured);
        if changed {
            // The scroll bar and the rows below move with the new heights.
            ui.ctx().request_repaint();
        }
        if let Some((id, what)) = action {
            let Some(entry) = st.entries.iter().find(|e| e.id == id).cloned() else { return };
            let tr = entry.latest().cloned();
            match what {
                "fav" => {
                    app.engine.set_favorite(id, !entry.favorite);
                    st.dirty = true;
                }
                "open" => {
                    app.main.source = entry.source.clone();
                    app.main.translated_source.clear();
                    app.translate_main();
                    app.show_main();
                }
                "copy_text" => app.copy_text(&entry.source),
                "copy_tr" => {
                    if let Some(r) = tr {
                        app.copy_text(&r.text);
                    }
                }
                "listen" => app.speak(&entry.source, tr.map(|r| r.from).unwrap_or(glossa_core::LangId::AUTO)),
                "delete" => {
                    app.engine.remove_history(id);
                    st.deleted = Some((entry, std::time::Instant::now()));
                    st.dirty = true;
                }
                _ => {}
            }
        }
    });
}

/// The locale key naming an export format in the save dialog.
fn format_key(f: ExportFormat) -> &'static str {
    match f {
        ExportFormat::Text => "history.format_text",
        ExportFormat::Csv => "history.format_csv",
        ExportFormat::Html => "history.format_html",
        ExportFormat::Json => "history.format_json",
    }
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        flat.chars().take(max - 1).chain(std::iter::once('…')).collect()
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_rows_in_view_are_picked() {
        let h = [50.0, 50.0, 100.0, 50.0, 50.0];
        // The top of the list.
        assert_eq!(visible_rows(&h, 0.0, 120.0), (0, 3, 0.0));
        // Scrolled into the third row: it starts above the view.
        assert_eq!(visible_rows(&h, 150.0, 240.0), (2, 4, 100.0));
        // The very end.
        assert_eq!(visible_rows(&h, 290.0, 400.0), (4, 5, 250.0));
        // Past the end (the list just got shorter): nothing.
        assert_eq!(visible_rows(&h, 500.0, 600.0), (5, 5, 0.0));
        assert_eq!(visible_rows(&[], 0.0, 100.0), (0, 0, 0.0));
    }

    #[test]
    fn unmeasured_rows_are_guessed_from_the_measured() {
        let mut r = RowHeights::default();
        r.check(500.0, false, 1.0);
        assert_eq!(r.guess(), 72.0);
        r.heights.insert(1, 40.0);
        r.heights.insert(2, 60.0);
        assert_eq!(r.get(3, r.guess()), 50.0);
        assert_eq!(r.get(1, r.guess()), 40.0);
        // A new width rewraps everything.
        r.check(400.0, false, 1.0);
        assert!(r.heights.is_empty());
    }
}
