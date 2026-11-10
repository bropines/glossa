//! Text handling that sits between a provider and a screen.
//!
//! Two jobs live here. One is the small set of cleanups the old program applied
//! to a selection before sending it anywhere — normalising line endings,
//! stripping the hard wraps a PDF or a mail client leaves behind, capping the
//! length. The other is telling which way text runs ([`needs_bidi`],
//! [`guessed_direction`]); laying right-to-left text out is the front end's
//! business (the desktop app's `bidi` module).

use unicode_segmentation::UnicodeSegmentation;

/// Collapse every line ending to `\n`.
///
/// Providers are sent `\n` only; the old scripts did this in their
/// `prepareSource` helper and some services behave differently without it.
pub fn normalise_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    out
}

/// Undo hard wrapping, the way "remove line breaks" did.
///
/// A single newline between two lines that look like one flowing sentence
/// becomes a space; a blank line stays, because that is a paragraph break the
/// author meant. A line that ends in sentence punctuation is left alone — it
/// was probably a real break.
pub fn unwrap_hard_breaks(text: &str) -> String {
    let text = normalise_newlines(text);
    let mut out = String::with_capacity(text.len());
    let mut lines = text.split('\n').peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        out.push_str(trimmed);

        let Some(next) = lines.peek() else { break };

        if trimmed.is_empty() || next.trim().is_empty() {
            // A paragraph boundary: keep it.
            out.push('\n');
            continue;
        }

        let ends_sentence =
            trimmed.chars().last().is_some_and(|c| matches!(c, '.' | '!' | '?' | ':' | ';' | '。' | '！' | '？' | '؟'));
        let next_starts_list = next.trim_start().starts_with(['-', '*', '•', '–', '—']);

        if ends_sentence || next_starts_list {
            out.push('\n');
        } else if trimmed.ends_with('-') {
            // A word split across lines: rejoin it without a space.
            out.pop();
        } else {
            out.push(' ');
        }
    }
    out
}

/// Drop runs of blank lines and trim the ends, so a scraped page does not
/// arrive as a column of whitespace.
pub fn squeeze_blank_lines(text: &str) -> String {
    let text = normalise_newlines(text);
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank_run = 0;
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

/// Cut `text` to at most `max_chars` characters, preferring a word boundary.
///
/// Counted in characters, not bytes, and never splits a grapheme cluster, so an
/// emoji or a combining mark survives whole. Languages written without word
/// spaces have no boundary to find, so they are cut at the limit.
pub fn truncate_at_boundary(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    let end = text.grapheme_indices(true).nth(max_chars).map_or(text.len(), |(offset, _)| offset);
    if end == text.len() {
        return text;
    }

    let head = &text[..end];
    // Back up to the last space, but not so far that we throw most of it away.
    if let Some(space) = head.rfind(char::is_whitespace) {
        if space * 4 >= head.len() * 3 {
            return &head[..space];
        }
    }
    head
}

/// Cut `text` into pieces of at most `max_chars` characters that join back
/// into `text` exactly, for a service with an input limit.
///
/// Each cut is made at the best boundary in the second half of the window:
/// after a blank line (a paragraph), else after a line break, else after the
/// space that follows a sentence end (or right after a CJK full stop, which
/// has none), else after any whitespace, else at the limit without splitting
/// a grapheme cluster. The whitespace at a cut stays with the piece before
/// it, so a caller that sends the pieces trimmed can put it back.
pub fn split_for_limit(text: &str, max_chars: usize) -> Vec<&str> {
    let max = max_chars.max(1);
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((limit, _)) = rest.char_indices().nth(max) {
        let window = &rest[..limit];
        let late = |cut: &usize| *cut >= limit / 2;
        let after_last = |pat: &str| window.rfind(pat).map(|i| i + pat.len());
        let cut = after_last("\n\n")
            .filter(late)
            .or_else(|| after_last("\n").filter(late))
            .or_else(|| sentence_cut(window).filter(late))
            .or_else(|| {
                let (i, c) = window.char_indices().rev().find(|(_, c)| c.is_whitespace())?;
                Some(i + c.len_utf8()).filter(|cut| *cut > 0)
            })
            .unwrap_or_else(|| {
                // No boundary at all (a long URL, text without spaces): the
                // last whole grapheme cluster that fits.
                rest.grapheme_indices(true)
                    .map(|(i, g)| i + g.len())
                    .take_while(|end| *end <= limit)
                    .last()
                    .unwrap_or(limit)
            });
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Where the last sentence in `window` ends, counting the space after it.
fn sentence_cut(window: &str) -> Option<usize> {
    let mut best = None;
    let mut prev = None;
    for (i, c) in window.char_indices() {
        if c.is_whitespace() && matches!(prev, Some('.' | '!' | '?' | '…' | '؟' | '।')) {
            best = Some(i + c.len_utf8());
        }
        if matches!(c, '\u{3002}' | '\u{FF01}' | '\u{FF1F}') {
            best = Some(i + c.len_utf8());
        }
        prev = Some(c);
    }
    best
}

/// How many characters the text has, for the "5000 max" style limits a
/// provider imposes.
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// Whether the text contains anything the bidi algorithm would move.
///
/// Checks for strong right-to-left characters: Hebrew, Arabic, Syriac, Thaana,
/// NKo, Arabic Supplement/Extended and the presentation forms. Text without
/// them lays out the same either way.
pub fn needs_bidi(text: &str) -> bool {
    text.chars().any(is_strong_rtl)
}

/// Whether `c` is a letter of a right-to-left script (or a character from
/// one of their blocks: Arabic-Indic digits and Arabic punctuation count).
pub fn is_strong_rtl(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x05FF   // Hebrew
        | 0x0600..=0x06FF // Arabic
        | 0x0700..=0x074F // Syriac
        | 0x0750..=0x077F // Arabic Supplement
        | 0x0780..=0x07BF // Thaana
        | 0x07C0..=0x07FF // NKo
        | 0x0800..=0x083F // Samaritan
        | 0x08A0..=0x08FF // Arabic Extended-A
        | 0xFB1D..=0xFDFF // Hebrew/Arabic presentation forms A
        | 0xFE70..=0xFEFF // Arabic presentation forms B
        | 0x10800..=0x10FFF
        | 0x1E800..=0x1EFFF
    )
}

/// Which way a piece of text wants to be aligned, judged from the text itself.
///
/// Used for the output pane when the provider did not say what language it
/// produced.
pub fn guessed_direction(text: &str) -> Direction {
    let mut rtl = 0usize;
    let mut ltr = 0usize;
    for c in text.chars() {
        if is_strong_rtl(c) {
            rtl += 1;
        } else if c.is_alphabetic() {
            ltr += 1;
        }
        if rtl + ltr > 512 {
            break; // A sample is enough.
        }
    }
    if rtl > ltr {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

impl Direction {
    pub fn is_rtl(self) -> bool {
        self == Direction::RightToLeft
    }
}

/// Whether the text is short enough and plain enough to be a dictionary
/// lookup rather than a translation: one or two words, no sentence
/// punctuation.
pub fn looks_like_a_dictionary_lookup(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || char_count(trimmed) > 64 {
        return false;
    }
    if trimmed.contains('\n') {
        return false;
    }
    if trimmed.chars().any(|c| matches!(c, '.' | '!' | '?' | ',' | ';' | '。' | '！' | '？')) {
        return false;
    }
    trimmed.unicode_words().count() <= 2
}

/// Levenshtein distance between two strings, counted in characters and
/// ignoring ASCII case: how many single-character edits turn one into the
/// other.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            row[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

/// Up to `max` of `candidates` that look like a misspelling of `needle`,
/// closest first: for "did you mean" hints after an unknown flag or id. A
/// candidate qualifies when it contains `needle`, or is within a third of
/// its length in edits (at least two).
pub fn closest<'a>(needle: &str, candidates: impl IntoIterator<Item = &'a str>, max: usize) -> Vec<&'a str> {
    let lower = needle.to_ascii_lowercase();
    let limit = (needle.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, &str)> = candidates
        .into_iter()
        .filter_map(|c| {
            let d = edit_distance(needle, c);
            let related = lower.len() > 1 && c.to_ascii_lowercase().contains(&lower);
            (d <= limit || related).then_some((if related { d.min(limit) } else { d }, c))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(max).map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misspellings_find_their_closest_matches() {
        assert_eq!(edit_distance("slwo", "slow"), 2);
        assert_eq!(edit_distance("Google", "google"), 0);
        assert_eq!(closest("--slwo", ["--slow", "--search", "--service"], 3), ["--slow"]);
        assert_eq!(closest("gogle", ["google", "google-dictionary", "bing"], 3)[0], "google");
        assert_eq!(closest("deep", ["deepl", "deepl-web", "deeplx", "lara"], 3), ["deepl", "deepl-web", "deeplx"]);
        assert!(closest("zzzzzz", ["google", "bing"], 3).is_empty());
    }

    #[test]
    fn newlines_collapse_to_unix() {
        assert_eq!(normalise_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
        // A string with no carriage returns is returned as-is.
        assert_eq!(normalise_newlines("plain"), "plain");
        // A trailing lone CR still becomes a newline.
        assert_eq!(normalise_newlines("a\r"), "a\n");
    }

    #[test]
    fn hard_wraps_rejoin_into_sentences() {
        let wrapped = "The quick brown\nfox jumps over\nthe lazy dog.";
        assert_eq!(unwrap_hard_breaks(wrapped), "The quick brown fox jumps over the lazy dog.");
    }

    #[test]
    fn paragraph_breaks_and_real_breaks_survive_unwrapping() {
        let text = "First paragraph.\n\nSecond one\ncontinues here.";
        assert_eq!(unwrap_hard_breaks(text), "First paragraph.\n\nSecond one continues here.");

        // A list keeps its shape.
        let list = "Shopping:\n- milk\n- bread";
        assert_eq!(unwrap_hard_breaks(list), "Shopping:\n- milk\n- bread");
    }

    #[test]
    fn hyphenated_words_rejoin_without_a_space() {
        assert_eq!(unwrap_hard_breaks("inter-\nnational"), "international");
    }

    #[test]
    fn blank_line_runs_are_squeezed() {
        assert_eq!(squeeze_blank_lines("a\n\n\n\nb\n\n"), "a\n\nb");
        assert_eq!(squeeze_blank_lines("   \n\n  a  \n"), "a");
    }

    #[test]
    fn truncation_prefers_a_word_boundary_but_not_at_any_cost() {
        assert_eq!(truncate_at_boundary("hello world again", 11), "hello world");
        // Backing up would throw away most of the text, so it cuts instead.
        assert_eq!(truncate_at_boundary("a verylongsingleword", 12), "a verylongsi");
        // Shorter than the limit: untouched.
        assert_eq!(truncate_at_boundary("short", 100), "short");
        assert_eq!(truncate_at_boundary("anything", 0), "");
    }

    #[test]
    fn splitting_for_a_limit_loses_nothing_and_prefers_real_boundaries() {
        let text = "First sentence here. Second one follows!\n\nA new paragraph starts. And ends.\nLast line";
        for max in [1, 5, 12, 25, 40, 200] {
            let parts = split_for_limit(text, max);
            assert_eq!(parts.concat(), text, "max {max}");
            assert!(parts.iter().all(|p| p.chars().count() <= max && !p.is_empty()), "max {max}: {parts:?}");
        }
        // Paragraph first, then sentence.
        assert_eq!(
            split_for_limit(text, 45),
            vec!["First sentence here. Second one follows!\n\n", "A new paragraph starts. And ends.\nLast line"]
        );
        assert_eq!(
            split_for_limit(text, 36),
            vec!["First sentence here. ", "Second one follows!\n\n", "A new paragraph starts. And ends.\n", "Last line"]
        );
        assert_eq!(split_for_limit("One two. Three four. Five six.", 22), vec!["One two. Three four. ", "Five six."]);
        // No sentence end: a word boundary.
        assert_eq!(split_for_limit("alpha beta gamma delta", 12), vec!["alpha beta ", "gamma delta"]);
        // CJK: after the full stop, else at the limit.
        assert_eq!(split_for_limit("今日は晴れ。明日は雨。", 8), vec!["今日は晴れ。", "明日は雨。"]);
        assert_eq!(split_for_limit("абвгдеж", 3), vec!["абв", "где", "ж"]);
        // A cluster is never split.
        assert_eq!(split_for_limit("ab👨‍👩‍👧cd", 5), vec!["ab", "👨‍👩‍👧", "cd"]);
        assert!(split_for_limit("", 5).is_empty());
        assert_eq!(split_for_limit("short", 5), vec!["short"]);
    }

    #[test]
    fn truncation_counts_characters_and_keeps_clusters_whole() {
        // Four characters, eight bytes: a byte-based cut would split them.
        let text = "абвг";
        assert_eq!(truncate_at_boundary(text, 2), "аб");
        assert_eq!(char_count(text), 4);
        // A family emoji is one cluster of several code points.
        let emoji = "👨‍👩‍👧 hello";
        assert_eq!(truncate_at_boundary(emoji, 1), "👨‍👩‍👧");
    }

    #[test]
    fn right_to_left_text_is_recognised() {
        assert!(!needs_bidi("Hello, мир!"));
        assert!(needs_bidi("abc \u{5e9}\u{5dc}\u{5d5}\u{5dd} def"));
        assert!(needs_bidi("مرحبا"));
        assert!(is_strong_rtl('\u{5d0}') && is_strong_rtl('ب') && !is_strong_rtl('a'));
    }

    #[test]
    fn direction_is_guessed_from_the_bulk_of_the_text() {
        assert_eq!(guessed_direction("hello world"), Direction::LeftToRight);
        assert_eq!(guessed_direction("\u{5e9}\u{5dc}\u{5d5}\u{5dd}"), Direction::RightToLeft);
        // One stray Latin word does not flip a Hebrew sentence.
        assert_eq!(guessed_direction("\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5d7}\u{5d1}\u{5e8} ok"), Direction::RightToLeft);
        assert!(Direction::RightToLeft.is_rtl());
    }

    #[test]
    fn dictionary_heuristic_separates_words_from_prose() {
        assert!(looks_like_a_dictionary_lookup("serendipity"));
        assert!(looks_like_a_dictionary_lookup("hello world"));
        assert!(!looks_like_a_dictionary_lookup("hello world again"));
        assert!(!looks_like_a_dictionary_lookup("Is this a sentence?"));
        assert!(!looks_like_a_dictionary_lookup("two\nlines"));
        assert!(!looks_like_a_dictionary_lookup(""));
        assert!(!looks_like_a_dictionary_lookup(&"x".repeat(100)));
    }
}
