//! Rendering dictionary articles: structured ones natively, HTML ones through
//! the simplified block model from `glossa-dict` — no web engine.

use egui::{Color32, RichText};
use glossa_core::Article;
use glossa_dict::{Block, Span};

use crate::style::{c, Look};

/// What a click in an article asks for.
pub enum ArticleClick {
    /// Look this word up.
    Lookup(String),
    /// Open this URL.
    Open(String),
    Listen(String),
}

pub fn article(ui: &mut egui::Ui, look: &Look, a: &Article, zoom: f32) -> Option<ArticleClick> {
    let mut click = None;
    let base = look.theme.fonts.size * 1.35 * zoom;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&a.headword).size(base * 1.6).strong());
        for p in &a.pronunciations {
            if p.symbol.is_empty() {
                continue;
            }
            let text =
                if p.region.is_empty() { format!("[{}]", p.symbol) } else { format!("{} [{}]", p.region, p.symbol) };
            ui.label(RichText::new(text).size(base).color(look.weak_text()));
        }
        if crate::icons::sized_button(ui, look, crate::icons::Icon::Listen, "", base * 1.4, true).clicked() {
            click = Some(ArticleClick::Listen(a.headword.clone()));
        }
    });
    ui.add_space(4.0);
    for e in &a.explanations {
        if !e.part_of_speech.is_empty() {
            ui.label(RichText::new(&e.part_of_speech).size(base).italics().color(look.accent()));
        }
        for (i, d) in e.definitions.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).size(base).color(look.weak_text()));
                ui.add(egui::Label::new(RichText::new(d).size(base)).selectable(true).wrap());
            });
        }
        ui.add_space(4.0);
    }
    if !a.examples.is_empty() {
        ui.add_space(2.0);
        for ex in &a.examples {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("•").size(base).color(look.weak_text()));
                ui.add(egui::Label::new(RichText::new(&ex.source).size(base).italics()).selectable(true).wrap());
                if !ex.target.is_empty() {
                    ui.label(RichText::new(format!("— {}", ex.target)).size(base).color(look.weak_text()));
                }
            });
        }
    }
    if !a.associations.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for w in &a.associations {
                if ui.link(RichText::new(w).size(base * 0.95)).clicked() {
                    click = Some(ArticleClick::Lookup(w.clone()));
                }
            }
        });
    }
    if let Some(html) = &a.html {
        ui.add_space(4.0);
        if let Some(c) = html_blocks(ui, look, &glossa_dict::to_blocks(html), base) {
            click = Some(c);
        }
    }
    if let Some(link) = &a.link {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if crate::icons::sized_button(ui, look, crate::icons::Icon::Open, "", 18.0, true).clicked()
                || ui.link(RichText::new(link).small()).clicked()
            {
                click = Some(ArticleClick::Open(link.clone()));
            }
        });
    }
    click
}

fn span_text(s: &Span, look: &Look, base: f32) -> RichText {
    let mut t = RichText::new(&s.text).size(if s.small || s.sup { base * 0.8 } else { base });
    if s.bold {
        t = t.strong();
    }
    if s.italic {
        t = t.italics();
    }
    if s.underline {
        t = t.underline();
    }
    if s.monospace {
        t = t.monospace();
    }
    if let Some(col) = s.color {
        t = t.color(readable(c(col), look));
    }
    if s.sup {
        t = t.raised();
    }
    t
}

/// Article colours were picked for a white page; on a dark theme lift the
/// ones that would vanish.
fn readable(col: Color32, look: &Look) -> Color32 {
    let back = c(look.theme.edit.normal.back);
    let gc = crate::style::from32(col);
    if gc.contrast(crate::style::from32(back)) < 2.5 {
        if look.theme.dark {
            col.lerp_to_gamma(Color32::WHITE, 0.55)
        } else {
            col.lerp_to_gamma(Color32::BLACK, 0.55)
        }
    } else {
        col
    }
}

fn spans(ui: &mut egui::Ui, look: &Look, spans: &[Span], base: f32) -> Option<ArticleClick> {
    let mut click = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            if s.text.is_empty() {
                continue;
            }
            match &s.link {
                Some(link) => {
                    if ui.link(span_text(s, look, base)).clicked() {
                        click = Some(match link.strip_prefix("dict://") {
                            Some(word) => ArticleClick::Lookup(word.to_owned()),
                            None if link.starts_with("http") => ArticleClick::Open(link.clone()),
                            None => ArticleClick::Lookup(s.text.trim().to_owned()),
                        });
                    }
                }
                None => {
                    ui.add(egui::Label::new(span_text(s, look, base)).selectable(true));
                }
            }
        }
    });
    click
}

pub fn html_blocks(ui: &mut egui::Ui, look: &Look, blocks: &[Block], base: f32) -> Option<ArticleClick> {
    let mut click = None;
    for b in blocks {
        let c = match b {
            Block::Heading { level, spans: s } => {
                let scale = match level {
                    1 => 1.5,
                    2 => 1.3,
                    _ => 1.12,
                };
                spans(ui, look, s, base * scale)
            }
            Block::Paragraph(s) => spans(ui, look, s, base),
            Block::ListItem { depth, ordered_index, spans: s } => {
                let mut out = None;
                ui.horizontal(|ui| {
                    ui.add_space(14.0 * (*depth as f32));
                    let bullet = ordered_index.map(|n| format!("{n}.")).unwrap_or_else(|| "•".into());
                    ui.label(RichText::new(bullet).size(base).color(look.weak_text()));
                    out = spans(ui, look, s, base);
                });
                out
            }
            Block::Rule => {
                ui.separator();
                None
            }
            Block::Quote(s) => {
                let mut out = None;
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    out = spans(ui, look, s, base);
                });
                out
            }
            Block::Table(rows) => {
                let mut out = None;
                egui::Grid::new(ui.next_auto_id()).striped(true).show(ui, |ui| {
                    for row in rows {
                        for cell in row {
                            if let Some(c) = spans(ui, look, cell, base) {
                                out = Some(c);
                            }
                        }
                        ui.end_row();
                    }
                });
                out
            }
        };
        if c.is_some() {
            click = c;
        }
    }
    click
}
