//! Right-to-left and mixed-direction text on a toolkit that lays text out
//! left to right.
//!
//! epaint, egui's text engine, shapes text with HarfRust (the Rust port of
//! HarfBuzz): Indic conjuncts and reordered vowel signs, Khmer and Myanmar
//! clusters, Arabic joining all come out right, as long as a run holds one
//! script going one way. It does not run the bidirectional algorithm. It
//! cuts a section into runs by font, guesses each run's script and direction
//! from its first letter, and places runs, and wraps rows, in logical order.
//! So a line that mixes Arabic and Latin in one font joins no Arabic letters,
//! a Latin word inside Arabic lands on the wrong side, and a wrapped Arabic
//! paragraph starts on its last row.
//!
//! [`layout`] fills that gap from the outside, without patching epaint:
//!
//! 1. The Unicode bidi algorithm (`unicode-bidi`) gives every character an
//!    embedding level.
//! 2. The text becomes a [`LayoutJob`] whose sections never mix levels or
//!    scripts, so each section is shaped alone, in the right direction. An
//!    odd-level (right-to-left) stretch is cut at spaces: epaint wraps rows
//!    between sections in logical order, which then is the right order, and
//!    each word is shaped right to left on its own.
//! 3. After epaint has laid the job out and wrapped it, each row's runs are
//!    put in visual order (rule L2 of UAX #9): their glyphs, and the glyphs'
//!    vertices in the already tessellated mesh, slide sideways. Brackets
//!    standing alone in a right-to-left stretch get their mirrored glyph.
//!
//! The galley's text stays the logical text, so copying a selection copies
//! what the provider sent. Positions inside a right-to-left row no longer
//! grow with the character index, which egui's cursor code assumes: the
//! selection highlight and the caret inside such rows are approximate.
//!
//! epaint keeps no section index in a laid-out glyph that code outside it can
//! read, so each section carries its index in the lowest bits of its line
//! height (a few thousandths of a point, which rounding to pixels removes).
//!
//! Text without right-to-left characters only goes through step 2, and only
//! when it mixes a script that needs shaping with another one (a face that
//! covers both would shape the pair with one script's rules). Everything
//! else takes egui's own path untouched.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::ByteIndex;
use egui::text::{LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{Align, Galley, RichText, WidgetText};
use glossa_core::text::is_strong_rtl;
use unicode_bidi::{BidiInfo, Level};

use crate::fonts::Script;

/// Whether `text` needs [`layout`] instead of egui's own layout.
pub fn needs_layout(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    let mut first: Option<Key> = None;
    let mut complex = false;
    let mut mixed = false;
    for c in text.chars() {
        if is_strong_rtl(c) {
            return true;
        }
        let Some(k) = key(c) else { continue };
        complex |= k.needs_shaping();
        match first {
            None => first = Some(k),
            Some(f) => mixed |= f != k,
        }
        if complex && mixed {
            return true;
        }
    }
    false
}

/// `text` for a label: a galley laid out here when it needs it, else the
/// rich text as it was, for egui to lay out as usual. `wrap` applies to the
/// galley only; a label given rich text wraps as the label says.
pub fn rich(ui: &egui::Ui, text: RichText, wrap: TextWrapping, rtl: bool) -> WidgetText {
    if !needs_layout(text.text()) {
        return text.into();
    }
    let source = text.text().to_owned();
    let mut job = LayoutJob::default();
    text.append_to(&mut job, ui.style(), egui::FontSelection::Default, Align::Center);
    let format = job.sections.first().map(|s| s.format.clone()).unwrap_or_default();
    let halign = if rtl { Align::RIGHT } else { Align::LEFT };
    WidgetText::Galley(layout(ui, &source, &format, wrap, rtl, halign, false))
}

/// [`rich`] wrapping at the available width, its direction guessed from the
/// text: for short pieces inside other layouts.
pub fn auto(ui: &egui::Ui, text: RichText) -> WidgetText {
    let rtl = glossa_core::text::guessed_direction(text.text()).is_rtl();
    rich(ui, text, TextWrapping::wrap_at_width(ui.available_width()), rtl)
}

/// A selectable block of text that wraps at the available width, aligned to
/// the right when `rtl`.
pub fn label(ui: &mut egui::Ui, text: RichText, rtl: bool) -> egui::Response {
    let wrap = TextWrapping::wrap_at_width(ui.available_width());
    let text = rich(ui, text, wrap, rtl);
    let label = egui::Label::new(text).selectable(true).wrap();
    if rtl {
        ui.with_layout(egui::Layout::top_down(Align::Max), |ui| ui.add(label)).inner
    } else {
        ui.add(label)
    }
}

/// The layouter of a multi-line text editor: what egui's `TextEdit` does by
/// default, through [`layout`] when the text needs it. The caret inside a
/// right-to-left row is approximate (see the module docs); the text itself
/// shows as it should.
pub fn editor_layout(
    ui: &egui::Ui,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    wrap_width: f32,
    halign: Align,
) -> Arc<Galley> {
    let line_height = ui.fonts_mut(|f| f.row_height(font));
    let mut format = TextFormat::simple(font.clone(), color);
    format.line_height = Some(line_height);
    let wrap = TextWrapping::wrap_at_width(wrap_width);
    if needs_layout(text) {
        let rtl = glossa_core::text::guessed_direction(text).is_rtl();
        return layout(ui, text, &format, wrap, rtl, halign, true);
    }
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = wrap;
    job.halign = halign;
    job.keep_trailing_whitespace = true;
    ui.fonts_mut(|f| f.layout_job(job))
}

/// Lay `text` out with right-to-left runs in visual order. `rtl` is the
/// paragraphs' base direction: it decides where neutral characters between
/// runs go. The galley's text is `text` itself.
pub fn layout(
    ui: &egui::Ui,
    text: &str,
    format: &TextFormat,
    wrap: TextWrapping,
    rtl: bool,
    halign: Align,
    keep_trailing_whitespace: bool,
) -> Arc<Galley> {
    let has_rtl = text.chars().any(is_strong_rtl);
    let levels: Vec<u8> = if has_rtl {
        let base = if rtl { Level::rtl() } else { Level::ltr() };
        BidiInfo::new(text, Some(base)).levels.iter().map(|l| l.number()).collect()
    } else {
        Vec::new()
    };
    let runs = runs(text, &levels);
    let line_height = format.line_height.unwrap_or_else(|| ui.fonts_mut(|f| f.row_height(&format.font_id)));
    let mut job = LayoutJob { text: text.to_owned(), wrap, halign, keep_trailing_whitespace, ..Default::default() };
    for (i, r) in runs.iter().enumerate() {
        let mut f = format.clone();
        f.line_height = Some(encode(line_height, i));
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(r.range.start)..ByteIndex(r.range.end),
            format: f,
        });
    }
    let source = ui.fonts_mut(|f| f.layout_job(job));
    if !has_rtl {
        return source;
    }
    let ctx = ui.ctx();
    if let Some(done) = cache_get(ctx, &source) {
        return done;
    }
    // The mirrored glyphs a lone bracket in a right-to-left stretch needs,
    // laid out in the same face.
    let mut mirrors: Vec<(char, Arc<Galley>)> = Vec::new();
    for r in &runs {
        if let Some(m) = r.mirror {
            if !mirrors.iter().any(|(c, _)| *c == m) {
                let mut f = format.clone();
                f.line_height = Some(line_height);
                let g = ui.fonts_mut(|fonts| fonts.layout_job(LayoutJob::single_section(m.to_string(), f)));
                mirrors.push((m, g));
            }
        }
    }
    let base = u8::from(rtl);
    let done = match reorder(&source, &runs, base, line_height, &mirrors) {
        Some(g) => Arc::new(g),
        None => {
            tracing::debug!("bidi: could not map the laid-out glyphs back to their runs");
            source.clone()
        }
    };
    cache_put(ctx, source, done.clone());
    done
}

// ---------------------------------------------------------------------------
// Runs

/// What decides whether two letters may share a shaping run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Script(Script),
    /// Latin, Greek, Cyrillic and other letters the built-in faces draw.
    Other,
}

impl Key {
    /// Scripts whose shaping rules differ from the default ones.
    fn needs_shaping(self) -> bool {
        use Script::*;
        matches!(
            self,
            Key::Script(
                Devanagari
                    | Bengali
                    | Gurmukhi
                    | Gujarati
                    | Oriya
                    | Tamil
                    | Telugu
                    | Kannada
                    | Malayalam
                    | Sinhala
                    | Thai
                    | Lao
                    | Khmer
                    | Myanmar
                    | Tibetan
                    | Mongolian
                    | Javanese
                    | Arabic
                    | Syriac
                    | Thaana
                    | Nko
                    | Hebrew
            )
        )
    }
}

/// The script a character pins its run to; `None` for spaces, digits,
/// punctuation and combining marks, which go with their neighbours.
fn key(c: char) -> Option<Key> {
    match Script::of(c) {
        Some(Script::Symbols | Script::Emoji) => None,
        Some(s) => Some(Key::Script(s)),
        None if c.is_alphabetic() => Some(Key::Other),
        None => None,
    }
}

/// One section of the job: shaped on its own, at one embedding level.
#[derive(Debug, Clone, PartialEq)]
struct Run {
    range: Range<usize>,
    level: u8,
    /// A lone bracket in a right-to-left stretch: the character whose glyph
    /// it shows.
    mirror: Option<char>,
    /// Whether it lays out at least one glyph (a run of line breaks does not).
    visible: bool,
}

/// Cut `text` into the runs [`layout`] gives epaint. `levels` holds a bidi
/// level per byte; empty means all left to right.
fn runs(text: &str, levels: &[u8]) -> Vec<Run> {
    struct Open {
        run: Run,
        key: Option<Key>,
        /// Takes no more characters.
        closed: bool,
    }
    let level_at = |i: usize| levels.get(i).copied().unwrap_or(0);
    let mut open: Vec<Open> = Vec::new();
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        if c == '\n' {
            // A line break rides on the run before it and ends it.
            match open.last_mut() {
                Some(o) if o.run.range.end == i => {
                    o.run.range.end = end;
                    o.closed = true;
                }
                _ => open.push(Open {
                    run: Run { range: i..end, level: level_at(i), mirror: None, visible: false },
                    key: None,
                    closed: true,
                }),
            }
            continue;
        }
        let level = level_at(i);
        let k = key(c);
        // Right to left, a space stands alone so rows break between words
        // and each word is shaped by itself. Left to right, a right-to-left
        // character (an Arabic-Indic digit) stands alone, or the shaper would
        // turn the number around.
        let alone = if level % 2 == 1 { c.is_whitespace() } else { is_strong_rtl(c) };
        let joins = match open.last() {
            Some(o) => {
                !o.closed
                    && !alone
                    && o.run.level == level
                    && match (o.key, k) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            }
            None => false,
        };
        if joins {
            let o = open.last_mut().expect("checked above");
            o.run.range.end = end;
            o.run.visible = true;
            o.key = o.key.or(k);
        } else {
            open.push(Open { run: Run { range: i..end, level, mirror: None, visible: true }, key: k, closed: alone });
        }
    }
    // A right-to-left word with no right-to-left letter in it ("...", "?!")
    // would be shaped left to right: give each character its own run, and a
    // bracket its mirror image.
    let mut out = Vec::with_capacity(open.len());
    for o in open {
        let r = o.run;
        let body = text[r.range.clone()].trim_end_matches('\n');
        if r.level % 2 == 0 || body.chars().any(is_strong_rtl) {
            out.push(r);
            continue;
        }
        let mut pieces = body.char_indices().peekable();
        while let Some((at, c)) = pieces.next() {
            let start = r.range.start + at;
            let end = if pieces.peek().is_some() { start + c.len_utf8() } else { r.range.end };
            out.push(Run { range: start..end, level: r.level, mirror: mirrored(c), visible: true });
        }
        if body.is_empty() {
            out.push(r);
        }
    }
    out
}

/// The mirror image of a paired punctuation mark (Bidi_Mirroring_Glyph, the
/// common ones).
fn mirrored(c: char) -> Option<char> {
    Some(match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        _ => return None,
    })
}

/// Visual order of units with the given embedding levels: rule L2 of UAX #9.
/// Returns unit indices from left to right.
fn visual_order(levels: &[u8]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    let (Some(&lowest), Some(&highest)) = (levels.iter().min(), levels.iter().max()) else {
        return order;
    };
    let lowest_odd = lowest | 1;
    for k in (lowest_odd..=highest).rev() {
        let mut i = 0;
        while i < order.len() {
            if levels[order[i]] < k {
                i += 1;
                continue;
            }
            let start = i;
            while i < order.len() && levels[order[i]] >= k {
                i += 1;
            }
            order[start..i].reverse();
        }
    }
    order
}

// ---------------------------------------------------------------------------
// Section indices in the line height

const CODE_SCALE: f32 = 4096.0;
const CODES: usize = 64;

fn encode(line_height: f32, section: usize) -> f32 {
    line_height + (section % CODES) as f32 / CODE_SCALE
}

fn decode(line_height: f32, glyph_line_height: f32) -> Option<usize> {
    let code = ((glyph_line_height - line_height) * CODE_SCALE).round();
    (0.0..CODES as f32).contains(&code).then_some(code as usize)
}

// ---------------------------------------------------------------------------
// Reordering a laid-out galley

/// A stretch of a row's glyphs from one run.
struct Unit {
    glyphs: Range<usize>,
    level: u8,
    run: usize,
}

/// `source` with each row's runs in visual order, or `None` when its glyphs
/// cannot be matched to `runs`.
fn reorder(
    source: &Galley,
    runs: &[Run],
    base: u8,
    line_height: f32,
    mirrors: &[(char, Arc<Galley>)],
) -> Option<Galley> {
    let mut galley = source.clone();
    let ppp = galley.pixels_per_point;
    let round = |x: f32| (x * ppp).round() / ppp;
    // Which run each glyph came from.
    let next_visible = |from: usize| (from..runs.len()).find(|&i| runs[i].visible);
    let mut run = next_visible(0)?;
    let mut owners: Vec<Vec<usize>> = Vec::with_capacity(galley.rows.len());
    for placed in &galley.rows {
        let mut owner = Vec::with_capacity(placed.row.glyphs.len());
        for g in &placed.row.glyphs {
            let code = decode(line_height, g.line_height)?;
            if code != run % CODES {
                run = next_visible(run + 1)?;
                if code != run % CODES {
                    return None;
                }
            }
            owner.push(run);
        }
        owners.push(owner);
    }

    // epaint expects a run's cluster offsets to grow from glyph to glyph; in
    // a right-to-left run they shrink, and it pads the run with empty glyphs
    // for characters it believes it skipped. Drop them, so glyphs and
    // characters pair up again (selecting and copying count on it).
    let mut excess: Vec<usize> = vec![0; runs.len()];
    for owner in &owners {
        for &r in owner {
            excess[r] += 1;
        }
    }
    for (e, r) in excess.iter_mut().zip(runs) {
        *e = e.saturating_sub(galley.job.text[r.range.clone()].chars().filter(|&c| c != '\n').count());
    }
    let mut changed = false;
    if excess.iter().any(|&e| e > 0) {
        for (placed, owner) in galley.rows.iter_mut().zip(&mut owners).rev() {
            let mut drop = vec![false; owner.len()];
            for i in (0..owner.len()).rev() {
                let g = &placed.row.glyphs[i];
                if excess[owner[i]] > 0 && g.advance_width == 0.0 && g.uv_rect.is_nothing() {
                    drop[i] = true;
                    excess[owner[i]] -= 1;
                }
            }
            if drop.contains(&true) {
                let row = Arc::make_mut(&mut placed.row);
                let mut keep = drop.iter().map(|d| !d);
                row.glyphs.retain(|_| keep.next().unwrap_or(true));
                let mut keep = drop.iter().map(|d| !d);
                owner.retain(|_| keep.next().unwrap_or(true));
                changed = true;
            }
        }
    }

    for (placed, owner) in galley.rows.iter_mut().zip(&owners) {
        if placed.row.glyphs.is_empty() {
            continue;
        }
        let mut units: Vec<Unit> = Vec::new();
        for (i, &run) in owner.iter().enumerate() {
            match units.last_mut() {
                Some(u) if u.run == run => u.glyphs.end = i + 1,
                _ => units.push(Unit { glyphs: i..i + 1, level: runs[run].level, run }),
            }
        }
        // Rule L1: spaces ending a row go with the paragraph.
        let glyphs = &placed.row.glyphs;
        let trailing = glyphs.iter().rev().take_while(|g| g.chr.is_whitespace()).count();
        if trailing > 0 {
            let cut = glyphs.len() - trailing;
            let mut kept: Vec<Unit> = Vec::with_capacity(units.len() + 1);
            for u in units {
                if u.glyphs.start >= cut {
                    continue;
                }
                kept.push(Unit { glyphs: u.glyphs.start..u.glyphs.end.min(cut), ..u });
            }
            kept.push(Unit { glyphs: cut..glyphs.len(), level: base, run: usize::MAX });
            units = kept;
        }
        let needs_mirror = units.iter().any(|u| u.run != usize::MAX && runs[u.run].mirror.is_some());
        if units.iter().all(|u| u.level == 0) && !needs_mirror {
            continue;
        }
        let order = visual_order(&units.iter().map(|u| u.level).collect::<Vec<_>>());
        let widths: Vec<f32> =
            units.iter().map(|u| glyphs[u.glyphs.clone()].iter().map(|g| g.advance_width).sum()).collect();
        let mut old_start = Vec::with_capacity(units.len());
        let mut x = 0.0;
        for w in &widths {
            old_start.push(x);
            x += w;
        }
        let mut new_start = vec![0.0; units.len()];
        let mut x = 0.0;
        for &u in &order {
            new_start[u] = x;
            x += widths[u];
        }
        let row = Arc::make_mut(&mut placed.row);
        for (u, unit) in units.iter().enumerate() {
            let dx = round(new_start[u] - old_start[u]);
            let mirror = (unit.run != usize::MAX).then(|| runs[unit.run].mirror).flatten();
            for gi in unit.glyphs.clone() {
                let g = &mut row.glyphs[gi];
                g.pos.x += dx;
                if g.uv_rect.is_nothing() {
                    continue;
                }
                let first = g.first_vertex as usize;
                let Some(vertices) = row.visuals.mesh.vertices.get_mut(first..first + 4) else { continue };
                let replacement = mirror.and_then(|m| mirrors.iter().find(|(c, _)| *c == m)).and_then(|(_, mg)| {
                    let mrow = mg.rows.first()?;
                    let mglyph = mrow.glyphs.first()?;
                    let mfirst = mglyph.first_vertex as usize;
                    let mv = mrow.visuals.mesh.vertices.get(mfirst..mfirst + 4)?;
                    Some((mglyph.pos, [mv[0], mv[1], mv[2], mv[3]]))
                });
                match replacement {
                    Some((mpos, mv)) => {
                        let shift = g.pos - mpos;
                        for (v, m) in vertices.iter_mut().zip(mv) {
                            v.pos = m.pos + shift;
                            v.pos.y = round(v.pos.y);
                            v.uv = m.uv;
                        }
                    }
                    None => {
                        for v in vertices {
                            v.pos.x += dx;
                        }
                    }
                }
            }
        }
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
        changed = true;
    }
    if changed {
        galley.mesh_bounds = galley
            .rows
            .iter()
            .fold(egui::Rect::NOTHING, |acc, r| acc.union(r.visuals.mesh_bounds.translate(r.pos.to_vec2())));
    }
    Some(galley)
}

// ---------------------------------------------------------------------------
// Cache

/// Reordered galleys, keyed by the galley epaint laid out: epaint hands out
/// the same `Arc` for the same job until its fonts change, which is exactly
/// when a reordered copy goes stale too.
#[derive(Clone, Default)]
struct Cache {
    entries: Vec<(Arc<Galley>, Arc<Galley>, u64)>,
}

const CACHE_ENTRIES: usize = 128;
/// Frames an entry may go unused before it is dropped.
const CACHE_FRAMES: u64 = 600;

fn cache_id() -> egui::Id {
    egui::Id::unique("glossa-bidi-galleys")
}

fn cache_get(ctx: &egui::Context, source: &Arc<Galley>) -> Option<Arc<Galley>> {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(cache_id());
        cache.entries.iter_mut().find(|e| Arc::ptr_eq(&e.0, source)).map(|e| {
            e.2 = frame;
            e.1.clone()
        })
    })
}

fn cache_put(ctx: &egui::Context, source: Arc<Galley>, done: Arc<Galley>) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(cache_id());
        cache.entries.retain(|e| frame.saturating_sub(e.2) < CACHE_FRAMES);
        if cache.entries.len() >= CACHE_ENTRIES {
            if let Some(oldest) = cache.entries.iter().enumerate().min_by_key(|(_, e)| e.2).map(|(i, _)| i) {
                cache.entries.swap_remove(oldest);
            }
        }
        cache.entries.push((source, done, frame));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels_of(text: &str, rtl: bool) -> Vec<u8> {
        let base = if rtl { Level::rtl() } else { Level::ltr() };
        BidiInfo::new(text, Some(base)).levels.iter().map(|l| l.number()).collect()
    }

    fn pieces(text: &str, rtl: bool) -> Vec<(&str, u8)> {
        let levels = if text.chars().any(is_strong_rtl) { levels_of(text, rtl) } else { Vec::new() };
        runs(text, &levels).into_iter().map(|r| (&text[r.range], r.level)).collect()
    }

    #[test]
    fn plain_text_takes_egui_s_own_path() {
        assert!(!needs_layout("Hello, world"));
        assert!(!needs_layout("Привет, мир"));
        assert!(!needs_layout("漢字かな"));
        // One script that needs shaping is fine: epaint shapes it right.
        assert!(!needs_layout("हिन्दी में अनुवाद क्षमता"));
        assert!(!needs_layout("ភាសាខ្មែរ"));
        assert!(needs_layout("مرحبا بالعالم"));
        assert!(needs_layout("שלום"));
        // Mixed with another script it may share a face with.
        assert!(needs_layout("Google अनुवाद"));
    }

    #[test]
    fn left_to_right_text_splits_by_script_only() {
        assert_eq!(pieces("Google अनुवाद, ok", false), vec![("Google ", 0), ("अनुवाद, ", 0), ("ok", 0)]);
    }

    #[test]
    fn right_to_left_text_splits_into_words_and_spaces() {
        assert_eq!(pieces("مرحبا بالعالم", true), vec![("مرحبا", 1), (" ", 1), ("بالعالم", 1)]);
    }

    #[test]
    fn levels_split_runs_and_numbers_stay_whole() {
        let text = "يترجم Glossa النص في 30 ثانية.";
        let p = pieces(text, true);
        assert!(p.contains(&("Glossa", 2)), "{p:?}");
        assert!(p.contains(&("30", 2)), "{p:?}");
        assert!(p.contains(&("ثانية.", 1)), "{p:?}");
        // Every byte is in exactly one run, in order.
        let joined: String = p.iter().map(|(s, _)| *s).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn arabic_inside_left_to_right_text_is_one_right_to_left_run() {
        let p = pieces("The word مرحبا means hello.", false);
        assert!(p.contains(&("مرحبا", 1)), "{p:?}");
        assert_eq!(p.first(), Some(&("The word ", 0)));
    }

    #[test]
    fn arabic_indic_digits_in_a_number_are_kept_apart() {
        // Arabic-Indic digits resolve to an even level; shaped together the
        // shaper would read them as Arabic and reverse them.
        let p = pieces("سنة ١٢٣", true);
        assert!(p.contains(&("١", 2)) && p.contains(&("٢", 2)) && p.contains(&("٣", 2)), "{p:?}");
    }

    #[test]
    fn lone_brackets_in_right_to_left_text_are_mirrored() {
        let text = "شركة (Google) هنا";
        let levels = levels_of(text, true);
        let r = runs(text, &levels);
        let open = r.iter().find(|r| &text[r.range.clone()] == "(").expect("a run for the bracket");
        assert_eq!(open.level, 1);
        assert_eq!(open.mirror, Some(')'));
        // Brackets inside a right-to-left word are the shaper's business.
        let r = runs("(كلمة)", &levels_of("(كلمة)", true));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].mirror, None);
    }

    #[test]
    fn line_breaks_ride_on_the_run_before() {
        let text = "مرحبا\n\nعالم";
        let r = runs(text, &levels_of(text, true));
        let texts: Vec<&str> = r.iter().map(|r| &text[r.range.clone()]).collect();
        assert_eq!(texts, vec!["مرحبا\n\n", "عالم"]);
        let r = runs("\nabc", &[]);
        assert_eq!(r[0], Run { range: 0..1, level: 0, mirror: None, visible: false });
        assert!(r[1].visible);
    }

    #[test]
    fn visual_order_follows_rule_l2() {
        // All left to right: unchanged.
        assert_eq!(visual_order(&[0, 0, 0]), vec![0, 1, 2]);
        // A right-to-left paragraph of three words reverses.
        assert_eq!(visual_order(&[1, 1, 1]), vec![2, 1, 0]);
        // A Latin word (level 2) inside Arabic keeps its place among the
        // reversed Arabic words.
        assert_eq!(visual_order(&[1, 1, 2, 1, 1]), vec![4, 3, 2, 1, 0]);
        // Two Latin words in a row stay left to right between Arabic.
        assert_eq!(visual_order(&[1, 2, 2, 2, 1]), vec![4, 1, 2, 3, 0]);
        // Arabic inside an English sentence.
        assert_eq!(visual_order(&[0, 1, 1, 1, 0]), vec![0, 3, 2, 1, 4]);
        assert_eq!(visual_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn section_codes_round_trip() {
        for lh in [13.0_f32, 17.5, 22.333, 40.0] {
            for i in [0, 1, 7, 63, 64, 65, 1000] {
                assert_eq!(decode(lh, encode(lh, i)), Some(i % CODES), "{lh} {i}");
            }
        }
        assert_eq!(decode(17.0, 18.0), None);
    }

    /// Lay out through a real egui context (egui's built-in faces: Hebrew
    /// comes out as boxes, one per character, which is all this needs) and
    /// read the visual order back from the glyph positions.
    fn visual(text: &str, rtl: bool, width: f32) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut rows = Vec::new();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let format = TextFormat::simple(egui::FontId::proportional(14.0), egui::Color32::WHITE);
            let halign = if rtl { Align::RIGHT } else { Align::LEFT };
            let g = layout(ui, text, &format, TextWrapping::wrap_at_width(width), rtl, halign, false);
            for r in &g.rows {
                let mut glyphs: Vec<_> = r.glyphs.iter().filter(|g| !g.chr.is_whitespace()).collect();
                glyphs.sort_by(|a, b| a.pos.x.total_cmp(&b.pos.x));
                rows.push(glyphs.iter().map(|g| g.chr).collect());
            }
        });
        out.textures_delta.clear();
        rows
    }

    #[test]
    fn a_hebrew_word_inside_english_reads_right_to_left() {
        let rows = visual("ab \u{5d0}\u{5d1}\u{5d2} cd", false, 1000.0);
        assert_eq!(rows, vec!["ab\u{5d2}\u{5d1}\u{5d0}cd".to_owned()]);
    }

    #[test]
    fn hebrew_words_and_wrapped_rows_go_right_to_left() {
        let rows = visual("\u{5d0}\u{5d1} \u{5d2}\u{5d3} ab", true, 1000.0);
        // "ab" stays left to right, at the far (left) end.
        assert_eq!(rows, vec!["ab\u{5d3}\u{5d2}\u{5d1}\u{5d0}".to_owned()]);
        // Wrapped: the first row holds the first words.
        let long = "\u{5d0}\u{5d1} \u{5d2}\u{5d3} \u{5d4}\u{5d5} \u{5d6}\u{5d7}";
        let rows = visual(long, true, 40.0);
        assert!(rows.len() > 1, "{rows:?}");
        assert!(rows[0].ends_with("\u{5d1}\u{5d0}"), "{rows:?}");
        assert!(rows.last().unwrap().starts_with("\u{5d7}\u{5d6}"), "{rows:?}");
    }

    #[test]
    fn the_galley_keeps_the_logical_text() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let format = TextFormat::simple(egui::FontId::proportional(14.0), egui::Color32::WHITE);
            let text = "abc \u{5d0}\u{5d1}\u{5d2} (x) \u{5d3}";
            let g = layout(ui, text, &format, TextWrapping::wrap_at_width(1000.0), true, Align::RIGHT, false);
            assert_eq!(g.text(), text);
            let glyphs: Vec<String> = g.rows.iter().map(|r| r.glyphs.iter().map(|g| g.chr).collect()).collect();
            assert_eq!(glyphs.concat().chars().count(), text.chars().count(), "{glyphs:?}");
        });
        out.textures_delta.clear();
    }
}
