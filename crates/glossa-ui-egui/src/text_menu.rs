//! What the context menus of text areas need from egui's `TextEdit` state:
//! the selection, editing at the cursor, undo and redo.
//!
//! egui moves the cursor on any mouse press, the right button's too (and
//! drags the selection while the button is held), and a `TextEdit` without
//! the keyboard focus (which the open menu takes) drops its selection every
//! frame. So [`keep_selection`] remembers the selection frame by frame,
//! puts it back for as long as the right button is at work, and keeps it
//! while the menu is open; [`selection`] reads it from there, so "Copy" and
//! "Cut" act on what the user selected.

use std::ops::Range;

use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;
use egui::{Context, Id, Response};

fn kept_id(edit: Id) -> Id {
    edit.with("kept-selection")
}

/// Call once a frame after adding the text area `edit` (whose context menu
/// is [`crate::dropdown::context_menu`]).
pub fn keep_selection(ctx: &Context, edit: &Response) {
    let Some(mut state) = TextEditState::load(ctx, edit.id) else { return };
    let right = egui::PointerButton::Secondary;
    let right_at_work = ctx
        .input(|i| i.pointer.button_pressed(right) || i.pointer.button_down(right) || i.pointer.button_released(right));
    if right_at_work && edit.contains_pointer() {
        if let Some(kept) = ctx.data(|d| d.get_temp::<CCursorRange>(kept_id(edit.id))) {
            state.cursor.set_char_range(Some(kept));
            state.store(ctx, edit.id);
        }
    } else if !crate::dropdown::is_open(ctx, crate::dropdown::context_menu_id(edit)) {
        if let Some(r) = state.cursor.char_range() {
            ctx.data_mut(|d| d.insert_temp(kept_id(edit.id), r));
        }
    }
}

/// The text area's selection (or cursor) as [`keep_selection`] kept it,
/// else as the text area has it.
pub fn selection(ctx: &Context, edit: Id) -> Option<CCursorRange> {
    ctx.data(|d| d.get_temp::<CCursorRange>(kept_id(edit)))
        .or_else(|| TextEditState::load(ctx, edit).and_then(|s| s.cursor.char_range()))
}

/// The byte range of a character range in `text`, start before end.
fn byte_range(text: &str, r: CCursorRange) -> Range<usize> {
    let (a, b) = (r.primary.index.0, r.secondary.index.0);
    let (lo, hi) = (a.min(b), a.max(b));
    let at = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    at(lo)..at(hi)
}

/// The selected text, if anything is selected.
pub fn selected(text: &str, r: Option<CCursorRange>) -> Option<&str> {
    let range = byte_range(text, r?);
    Some(&text[range]).filter(|s| !s.is_empty())
}

/// Put `with` in place of the selection, or at the cursor when nothing is
/// selected, or at the end when there is no cursor. Returns the cursor
/// after it, as a character index.
pub fn replace(text: &mut String, r: Option<CCursorRange>, with: &str) -> usize {
    let range = match r {
        Some(r) => byte_range(text, r),
        None => text.len()..text.len(),
    };
    let start_chars = text[..range.start].chars().count();
    text.replace_range(range, with);
    start_chars + with.chars().count()
}

/// Remember `text` and its selection as a step to undo back to, before
/// the program changes the text itself (cut, paste).
pub fn remember_for_undo(ctx: &Context, edit: Id, text: &str) {
    let Some(mut state) = TextEditState::load(ctx, edit) else { return };
    let range = selection(ctx, edit).unwrap_or(CCursorRange::one(CCursor::new(text.chars().count())));
    let mut undoer = state.undoer();
    undoer.add_undo(&(range, text.to_owned()));
    state.set_undoer(undoer);
    state.store(ctx, edit);
}

fn current(state: &TextEditState, text: &str) -> (CCursorRange, String) {
    let range = state.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(text.chars().count())));
    (range, text.to_owned())
}

/// Whether there is a step to undo (or, with `redo`, to redo).
pub fn can_undo(ctx: &Context, edit: Id, text: &str, redo: bool) -> bool {
    TextEditState::load(ctx, edit).is_some_and(|s| {
        let now = current(&s, text);
        let undoer = s.undoer();
        if redo {
            undoer.has_redo(&now)
        } else {
            undoer.has_undo(&now)
        }
    })
}

/// Undo (or redo) one step of `text`. Returns whether anything changed.
pub fn undo(ctx: &Context, edit: Id, text: &mut String, redo: bool) -> bool {
    let Some(mut state) = TextEditState::load(ctx, edit) else { return false };
    let now = current(&state, text);
    let mut undoer = state.undoer();
    let step = if redo { undoer.redo(&now).cloned() } else { undoer.undo(&now).cloned() };
    let Some((range, before)) = step else { return false };
    *text = before;
    state.cursor.set_char_range(Some(range));
    state.set_undoer(undoer);
    state.store(ctx, edit);
    ctx.data_mut(|d| d.insert_temp(kept_id(edit), range));
    focus(ctx, edit);
    true
}

/// Put the cursor (or a selection) in the text area and give it the focus,
/// so the user sees where it is.
pub fn set_selection(ctx: &Context, edit: Id, range: CCursorRange) {
    if let Some(mut state) = TextEditState::load(ctx, edit) {
        state.cursor.set_char_range(Some(range));
        state.store(ctx, edit);
    }
    ctx.data_mut(|d| d.insert_temp(kept_id(edit), range));
    focus(ctx, edit);
}

/// Select all of `text`.
pub fn select_all(ctx: &Context, edit: Id, text: &str) {
    set_selection(ctx, edit, CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count())));
}

/// A cursor at character `at`.
pub fn cursor_at(at: usize) -> CCursorRange {
    CCursorRange::one(CCursor::new(at))
}

fn focus(ctx: &Context, edit: Id) {
    ctx.memory_mut(|m| m.request_focus(edit));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(a: usize, b: usize) -> Option<CCursorRange> {
        Some(CCursorRange::two(CCursor::new(a), CCursor::new(b)))
    }

    #[test]
    fn the_selection_is_found_by_characters_not_bytes() {
        let text = "Привет, мир";
        assert_eq!(selected(text, range(8, 11)), Some("мир"));
        // Either way round.
        assert_eq!(selected(text, range(11, 8)), Some("мир"));
        assert_eq!(selected(text, range(3, 3)), None);
        assert_eq!(selected(text, None), None);
    }

    #[test]
    fn pasting_goes_at_the_cursor_or_over_the_selection() {
        let mut t = "Привет мир".to_owned();
        let after = replace(&mut t, range(6, 6), ",");
        assert_eq!((t.as_str(), after), ("Привет, мир", 7));
        let after = replace(&mut t, range(8, 11), "свет");
        assert_eq!((t.as_str(), after), ("Привет, свет", 12));
        // Cut: replaced by nothing.
        let after = replace(&mut t, range(0, 8), "");
        assert_eq!((t.as_str(), after), ("свет", 0));
        // No cursor: at the end.
        replace(&mut t, None, "!");
        assert_eq!(t, "свет!");
    }
}
