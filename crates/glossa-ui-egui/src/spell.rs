//! Spell checking in the main window's source pane: misspelled words get a
//! wavy red line, and the pane's context menu offers suggestions, "Ignore"
//! and "Add to dictionary".
//!
//! The checking itself is `glossa_platform::spell`. It runs on a worker
//! thread (on macOS, whose checker lives on the main thread, in the frame),
//! 300 ms after the last edit, and results are cached by text and
//! language. While the user types, the last results stay on screen for the
//! parts of the text the edit did not touch ([`carry_over`]).
//!
//! The lines are painted over the `TextEdit` after it is drawn, from its
//! galley, so the text edit itself (selection, IME, undo, the bidi layouter
//! and the text zoom) is left as it is.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{pos2, Color32, Galley, Pos2, Rect, Shape, Stroke};
use glossa_core::LangId;
use glossa_platform::spell::{self, Misspelling};

/// The pause after the last edit before the text is checked.
const DELAY: Duration = Duration::from_millis(300);
/// How many checked texts are remembered.
const CACHE: usize = 8;
/// A check that has not answered in this long is given up on.
const STUCK: Duration = Duration::from_secs(10);
/// The wavy line's colour.
const RED: Color32 = Color32::from_rgb(0xE8, 0x11, 0x23);

enum Reply {
    Checked { text: String, lang: LangId, found: Vec<Misspelling> },
    Suggested { word: String, lang: LangId, list: Vec<String> },
}

type Job = Box<dyn FnOnce() -> Option<Reply> + Send>;

/// The word the context menu was opened on.
#[derive(Clone)]
pub struct MenuWord {
    pub word: Misspelling,
    pub lang: LangId,
    /// `None` until the checker answers.
    pub suggestions: Option<Vec<String>>,
}

/// The source pane's spell-checking state.
pub struct Spelling {
    /// The text `found` belongs to, and its language.
    text: String,
    lang: Option<LangId>,
    found: Vec<Misspelling>,
    /// The text as last seen, and when to check it.
    seen: String,
    due: Option<Instant>,
    /// What was last given to the checker, and whether it is still at it.
    asked: Option<(String, LangId)>,
    /// Since when the checker is at it.
    busy: Option<Instant>,
    cache: VecDeque<(String, LangId, Vec<Misspelling>)>,
    /// Words ignored (or added) this session.
    ignored: HashSet<String>,
    menu: Option<MenuWord>,
    jobs: Option<Sender<Job>>,
    replies_tx: Sender<Reply>,
    replies: Receiver<Reply>,
}

impl Default for Spelling {
    fn default() -> Self {
        let (replies_tx, replies) = channel();
        Spelling {
            text: String::new(),
            lang: None,
            found: Vec::new(),
            seen: String::new(),
            due: None,
            asked: None,
            busy: None,
            cache: VecDeque::new(),
            ignored: HashSet::new(),
            menu: None,
            jobs: None,
            replies_tx,
            replies,
        }
    }
}

impl Spelling {
    /// Call once a frame with the source text and the language to check
    /// it in (`None`: checking is off or the language is unknown). Returns
    /// the misspellings to underline now, as byte ranges of `text`.
    pub fn update(&mut self, ctx: &egui::Context, text: &str, lang: Option<LangId>) -> Vec<Misspelling> {
        self.drain();
        let Some(lang) = lang else {
            self.found.clear();
            self.lang = None;
            self.asked = None;
            self.due = None;
            return Vec::new();
        };
        let now = Instant::now();
        if self.seen != text {
            self.seen = text.to_owned();
            self.due = Some(now + DELAY);
        }
        if self.lang != Some(lang) {
            // A new language: what was found no longer holds; check now.
            self.found.clear();
            self.text = text.to_owned();
            self.lang = Some(lang);
            self.due = Some(now);
        }
        if self.text != text {
            self.found = carry_over(&self.text, text, &self.found);
            self.text = text.to_owned();
        }
        let asked = self.asked.as_ref().is_some_and(|(t, l)| t == text && *l == lang);
        let busy = self.busy.is_some_and(|t| t.elapsed() < STUCK);
        if !asked && !busy {
            if let Some((_, _, hit)) = self.cache.iter().find(|(t, l, _)| t == text && *l == lang) {
                self.found = hit.clone();
                self.asked = Some((text.to_owned(), lang));
                self.due = None;
            } else {
                let due = *self.due.get_or_insert(now);
                if due <= now {
                    self.due = None;
                    self.asked = Some((text.to_owned(), lang));
                    self.busy = Some(now);
                    let text = text.to_owned();
                    self.run(
                        ctx,
                        Box::new(move || {
                            let found = spell::check(&text, lang);
                            Some(Reply::Checked { text, lang, found })
                        }),
                    );
                } else {
                    ctx.request_repaint_after(due - now);
                }
            }
        }
        self.found.iter().filter(|m| !self.ignored.contains(&m.word)).cloned().collect()
    }

    /// The context menu was opened on `word` (or not on a misspelling):
    /// ask for suggestions.
    pub fn open_menu(&mut self, ctx: &egui::Context, word: Option<Misspelling>, lang: Option<LangId>) {
        self.menu = None;
        let (Some(word), Some(lang)) = (word, lang) else { return };
        let w = word.word.clone();
        self.menu = Some(MenuWord { word, lang, suggestions: None });
        self.run(
            ctx,
            Box::new(move || {
                let list = spell::suggest(&w, lang);
                Some(Reply::Suggested { word: w, lang, list })
            }),
        );
    }

    /// The word the context menu is for, while it is still in `text`
    /// where it was.
    pub fn menu_word(&self, text: &str) -> Option<MenuWord> {
        self.menu.clone().filter(|m| text.get(m.word.range.clone()) == Some(m.word.word.as_str()))
    }

    /// Stop underlining `word` for the rest of the session.
    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(word.to_owned());
    }

    /// Put `word` in the user's dictionary.
    pub fn add(&mut self, ctx: &egui::Context, word: &str, lang: LangId) {
        self.ignore(word);
        let w = word.to_owned();
        self.run(
            ctx,
            Box::new(move || {
                if !spell::add(&w, lang) {
                    tracing::warn!("spell: could not add {w:?} to the dictionary");
                }
                None
            }),
        );
        // What was checked before may list the word again.
        self.cache.clear();
    }

    fn drain(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Checked { text, lang, found } => {
                    self.busy = None;
                    if self.lang == Some(lang) {
                        self.found = carry_over(&text, &self.text, &found);
                    }
                    self.cache.retain(|(t, l, _)| !(*t == text && *l == lang));
                    self.cache.push_front((text, lang, found));
                    self.cache.truncate(CACHE);
                }
                Reply::Suggested { word, lang, list } => {
                    if let Some(m) = &mut self.menu {
                        if m.word.word == word && m.lang == lang {
                            m.suggestions = Some(list);
                        }
                    }
                }
            }
        }
    }

    /// Run `job` on the worker thread, started on first use; or right here
    /// where the checker wants the main thread.
    fn run(&mut self, ctx: &egui::Context, job: Job) {
        if spell::MAIN_THREAD_ONLY {
            if let Some(r) = job() {
                let _ = self.replies_tx.send(r);
            }
            ctx.request_repaint();
            return;
        }
        if self.jobs.is_none() {
            self.jobs = spawn_worker(ctx.clone(), self.replies_tx.clone());
        }
        let job = match &self.jobs {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                Err(back) => back.0,
            },
            None => job,
        };
        // No worker: do it here rather than not at all.
        self.jobs = None;
        if let Some(r) = job() {
            let _ = self.replies_tx.send(r);
        }
        ctx.request_repaint();
    }
}

/// The worker: one thread for as long as the window state lives, so the
/// system checker's per-thread state (COM on Windows) is set up once.
fn spawn_worker(ctx: egui::Context, replies: Sender<Reply>) -> Option<Sender<Job>> {
    let (tx, rx) = channel::<Job>();
    std::thread::Builder::new()
        .name("glossa-spell".into())
        .spawn(move || {
            for job in rx {
                // A panic in a checker costs that one answer, not the thread.
                let Ok(reply) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) else { continue };
                if let Some(r) = reply {
                    if replies.send(r).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
        })
        .map_err(|e| tracing::warn!("spell: no worker thread: {e}"))
        .ok()?;
    Some(tx)
}

/// `found`, made for `old`, moved onto `new`: misspellings before the edit
/// stay, those after it shift with it, and those it touched are dropped
/// (until the next check).
pub fn carry_over(old: &str, new: &str, found: &[Misspelling]) -> Vec<Misspelling> {
    if old == new {
        return found.to_vec();
    }
    let prefix = common_prefix(old, new);
    let suffix = common_suffix(&old[prefix..], &new[prefix..]);
    let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '\'' | '’' | '-' | '_');
    // Whether the edit starts (ends) right against a word character, in
    // the old text or the new: then a word touching it changed.
    let joins_after = old[prefix..].starts_with(is_word) || new[prefix..].starts_with(is_word);
    let joins_before = old[..old_end].ends_with(is_word) || new[..new_end].ends_with(is_word);
    found
        .iter()
        .filter_map(|m| {
            if m.range.end < prefix || (m.range.end == prefix && !joins_after) {
                Some(m.clone())
            } else if m.range.start > old_end || (m.range.start == old_end && !joins_before) {
                let shift = |b: usize| b - old_end + new_end;
                Some(Misspelling { range: shift(m.range.start)..shift(m.range.end), word: m.word.clone() })
            } else {
                None
            }
        })
        .filter(|m| new.get(m.range.clone()) == Some(m.word.as_str()))
        .collect()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).find(|((_, x), y)| x != y).map_or(a.len().min(b.len()), |((i, _), _)| i)
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n
}

/// Byte ranges of `text` as character ranges (what the galley counts).
pub fn char_ranges(text: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out = Vec::with_capacity(ranges.len());
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut to = |b: usize| -> usize {
        let b = b.min(text.len());
        if b < byte {
            (byte, chars) = (0, 0);
        }
        chars += text.get(byte..b).map_or(0, |s| s.chars().count());
        byte = b;
        chars
    };
    for r in ranges {
        let a = to(r.start);
        let b = to(r.end);
        out.push(a..b);
    }
    out
}

/// Where the character ranges `chars` (in order, not overlapping) are in
/// `galley`: one rectangle per range and row, relative to the galley, with
/// the range's index. Works on reordered (right-to-left) rows too: the
/// glyphs stay in logical order there, only their x moves.
pub fn spans(galley: &Galley, chars: &[Range<usize>]) -> Vec<(usize, Rect)> {
    let mut out = Vec::new();
    let (mut at, mut k) = (0usize, 0usize);
    for placed in &galley.rows {
        if k >= chars.len() {
            break;
        }
        let mut span: Option<(usize, Rect)> = None;
        for (i, g) in placed.row.glyphs.iter().enumerate() {
            let c = at + i;
            while k < chars.len() && chars[k].end <= c {
                k += 1;
            }
            if k >= chars.len() {
                break;
            }
            if c < chars[k].start || g.chr.is_whitespace() {
                continue;
            }
            let r = g.logical_rect().translate(placed.pos.to_vec2());
            match &mut span {
                Some((j, s)) if *j == k => *s = s.union(r),
                _ => {
                    out.extend(span.take());
                    span = Some((k, r));
                }
            }
        }
        out.extend(span);
        at += placed.row.glyphs.len() + usize::from(placed.ends_with_newline);
    }
    out
}

fn char_spans(galley: &Galley, text: &str, found: &[Misspelling]) -> Vec<(usize, Rect)> {
    let ranges: Vec<Range<usize>> = found.iter().map(|m| m.range.clone()).collect();
    spans(galley, &char_ranges(text, &ranges))
}

/// Underline `found` (byte ranges of `text`, the galley's text, in order)
/// with wavy red lines. `origin` is where the galley was painted.
pub fn paint(ui: &egui::Ui, galley: &Galley, origin: Pos2, text: &str, found: &[Misspelling]) {
    let painter = ui.painter();
    for (_, r) in char_spans(galley, text, found) {
        let r = r.translate(origin.to_vec2());
        if ui.is_rect_visible(r) {
            painter.add(wave(r));
        }
    }
}

/// A zigzag under `r`, sized to its text.
fn wave(r: Rect) -> Shape {
    let amp = (r.height() * 0.05).clamp(0.8, 2.5);
    let step = amp * 2.0;
    let y = r.bottom() - amp - 0.5;
    let mut points = Vec::new();
    let mut x = r.left();
    let mut up = true;
    while x < r.right() {
        points.push(pos2(x, if up { y - amp } else { y + amp }));
        x += step;
        up = !up;
    }
    points.push(pos2(r.right(), if up { y - amp } else { y + amp }));
    Shape::line(points, Stroke::new((amp * 0.8).max(1.0), RED))
}

/// The misspelling under `pos`, if any.
pub fn hit(galley: &Galley, origin: Pos2, text: &str, found: &[Misspelling], pos: Pos2) -> Option<Misspelling> {
    let local = pos - origin.to_vec2();
    char_spans(galley, text, found)
        .into_iter()
        .find(|(_, r)| r.contains(local))
        .and_then(|(i, _)| found.get(i).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str, word: &str) -> Misspelling {
        let start = text.find(word).unwrap();
        Misspelling { range: start..start + word.len(), word: word.to_owned() }
    }

    fn words(found: &[Misspelling]) -> Vec<&str> {
        found.iter().map(|m| m.word.as_str()).collect()
    }

    #[test]
    fn byte_ranges_become_character_ranges() {
        let text = "Привет 😀 helo мирр";
        let found = [m(text, "😀"), m(text, "helo"), m(text, "мирр"), m(text, "Привет")];
        let ranges: Vec<_> = found.iter().map(|m| m.range.clone()).collect();
        // Out of order on purpose: the last one goes back to the start.
        assert_eq!(char_ranges(text, &ranges), [7..8, 9..13, 14..18, 0..6]);
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[9..13].iter().collect::<String>(), "helo");
    }

    #[test]
    fn misspellings_survive_edits_elsewhere() {
        let old = "Helo wrld мирр";
        let found = vec![m(old, "Helo"), m(old, "wrld"), m(old, "мирр")];
        // Typing before every word shifts them all.
        let new = "Ну, Helo wrld мирр";
        assert_eq!(carry_over(old, new, &found), [m(new, "Helo"), m(new, "wrld"), m(new, "мирр")]);
        // Typing inside a word drops that word only.
        let new = "Helo world мирр";
        assert_eq!(words(&carry_over(old, new, &found)), ["Helo", "мирр"]);
        // Typing right after a word changes it.
        let new = "Helox wrld мирр";
        assert_eq!(words(&carry_over(old, new, &found)), ["wrld", "мирр"]);
        // A space after the last word leaves it be.
        let new = "Helo wrld мирр ";
        assert_eq!(words(&carry_over(old, new, &found)), ["Helo", "wrld", "мирр"]);
        // Deleting the word between two others.
        let new = "Helo мирр";
        assert_eq!(carry_over(old, new, &found), [m(new, "Helo"), m(new, "мирр")]);
        // An emoji typed in front is not part of the word.
        let new = "😀Helo wrld мирр";
        assert_eq!(carry_over(old, new, &found), [m(new, "Helo"), m(new, "wrld"), m(new, "мирр")]);
        // A letter typed in front is.
        let new = "xHelo wrld мирр";
        assert_eq!(words(&carry_over(old, new, &found)), ["wrld", "мирр"]);
    }

    #[test]
    fn spans_follow_the_galley() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(14.0);
            let text = "ab helo\nмирр 😀 cd";
            let g = crate::bidi::editor_layout(ui, text, &font, Color32::WHITE, 1000.0, egui::Align::LEFT);
            let found = [m(text, "helo"), m(text, "мирр"), m(text, "cd")];
            let rects = char_spans(&g, text, &found);
            assert_eq!(rects.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2], "{rects:?}");
            let r: Vec<Rect> = rects.iter().map(|(_, r)| *r).collect();
            // "helo" on the first row, after "ab "; "мирр" starts the second.
            assert!(r[0].left() > 0.0);
            assert!(r[1].top() >= r[0].bottom() - 0.5);
            assert!(r[1].left() < 1.0);
            assert!(r[2].left() > r[1].right());
            // A word wrapped over two rows gets a rectangle on each.
            let narrow = crate::bidi::editor_layout(ui, "abcdefghij", &font, Color32::WHITE, 30.0, egui::Align::LEFT);
            assert!(narrow.rows.len() > 1);
            let whole = [Range { start: 0, end: 10 }];
            assert_eq!(spans(&narrow, &whole).len(), narrow.rows.len());
            // A right-to-left word in English text: one span, between its
            // neighbours, wherever the reordering put its glyphs.
            let mixed = "ab \u{5d0}\u{5d1}\u{5d2} cd";
            let g2 = crate::bidi::editor_layout(ui, mixed, &font, Color32::WHITE, 1000.0, egui::Align::LEFT);
            let s = spans(&g2, &[0..2, 3..6, 7..9]);
            assert_eq!(s.len(), 3, "{s:?}");
            assert!(s[0].1.right() <= s[1].1.left() + 0.5 && s[1].1.right() <= s[2].1.left() + 0.5, "{s:?}");
            // A pointer over "мирр" finds it.
            let p = r[1].center();
            assert_eq!(hit(&g, Pos2::ZERO, text, &found, p).map(|m| m.word), Some("мирр".to_owned()));
            assert_eq!(hit(&g, Pos2::ZERO, text, &found, pos2(-5.0, -5.0)), None);
        });
        out.textures_delta.clear();
    }
}
