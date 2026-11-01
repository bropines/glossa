//! Translation history and favourites.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::lang::LangId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Monotonic within one history file.
    pub id: u64,
    pub source: String,
    /// One per service that translated this text, most recent last.
    pub results: Vec<HistoryResult>,
    #[serde(default)]
    pub favorite: bool,
    /// Unix seconds.
    #[serde(default)]
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryResult {
    pub provider_id: String,
    pub from: LangId,
    pub to: LangId,
    pub text: String,
}

impl HistoryEntry {
    /// The latest translation, for the list view.
    pub fn latest(&self) -> Option<&HistoryResult> {
        self.results.last()
    }

    fn matches(&self, needle_lower: &str) -> bool {
        self.source.to_lowercase().contains(needle_lower)
            || self.results.iter().any(|r| r.text.to_lowercase().contains(needle_lower))
    }
}

/// The history list, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    entries: VecDeque<HistoryEntry>,
    next_id: u64,
    #[serde(skip)]
    max_entries: usize,
}

impl History {
    pub fn new(max_entries: usize) -> Self {
        Self { entries: VecDeque::new(), next_id: 1, max_entries }
    }

    pub fn set_max_entries(&mut self, max: usize) {
        self.max_entries = max;
        self.trim();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Newest first.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    pub fn get(&self, id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Record a translation. If the same text was translated last, the result
    /// joins that entry instead of starting a new one — switching tabs through
    /// five services should not leave five rows.
    pub fn record(&mut self, source: &str, result: HistoryResult, now: i64) -> u64 {
        let source = source.trim();
        if source.is_empty() || result.text.trim().is_empty() {
            return 0;
        }
        if let Some(front) = self.entries.front_mut() {
            if front.source == source {
                front
                    .results
                    .retain(|r| !(r.provider_id == result.provider_id && r.from == result.from && r.to == result.to));
                front.results.push(result);
                front.created = now;
                return front.id;
            }
        }
        // The same text further down moves to the top, keeping its favourite
        // flag and earlier results.
        let mut entry = match self.entries.iter().position(|e| e.source == source) {
            Some(i) => self.entries.remove(i).expect("index from position"),
            None => {
                let id = self.next_id.max(1);
                self.next_id = id + 1;
                HistoryEntry { id, source: source.to_owned(), results: Vec::new(), favorite: false, created: now }
            }
        };
        entry.results.retain(|r| !(r.provider_id == result.provider_id && r.to == result.to));
        entry.results.push(result);
        entry.created = now;
        let id = entry.id;
        self.entries.push_front(entry);
        self.trim();
        id
    }

    pub fn set_favorite(&mut self, id: u64, favorite: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) => {
                e.favorite = favorite;
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        before != self.entries.len()
    }

    /// Clear the history. Favourites survive unless `including_favorites`.
    pub fn clear(&mut self, including_favorites: bool) {
        if including_favorites {
            self.entries.clear();
        } else {
            self.entries.retain(|e| e.favorite);
        }
    }

    /// Case-insensitive search over sources and translations.
    pub fn search<'a>(&'a self, needle: &str, favorites_only: bool) -> Vec<&'a HistoryEntry> {
        let needle = needle.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| !favorites_only || e.favorite)
            .filter(|e| needle.is_empty() || e.matches(&needle))
            .collect()
    }

    /// Drop the oldest non-favourites beyond the limit. Favourites are never
    /// dropped by the limit.
    fn trim(&mut self) {
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() > self.max_entries {
            match self.entries.iter().rposition(|e| !e.favorite) {
                Some(i) => {
                    self.entries.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn load(path: &Path, max_entries: usize) -> crate::Result<History> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut h: History = serde_json::from_str(&text)?;
                h.next_id = h.next_id.max(h.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1);
                h.set_max_entries(max_entries);
                Ok(h)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(History::new(max_entries)),
            Err(e) => Err(e.into()),
        }
    }

    /// [`History::load`] for a program that will save the history again: a
    /// file that exists but cannot be parsed is first copied to
    /// `history.json.corrupt-<unix seconds>` beside it, and an empty history
    /// is returned with the copy's path, so the next save cannot destroy the
    /// only copy of the user's entries. When the file cannot be read or
    /// copied, this is an error and the caller must not save over it.
    pub fn load_recovering(path: &Path, max_entries: usize) -> crate::Result<(History, Option<PathBuf>)> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((History::new(max_entries), None)),
            Err(e) => return Err(e.into()),
        };
        match serde_json::from_slice::<History>(&bytes) {
            Ok(mut h) => {
                h.next_id = h.next_id.max(h.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1);
                h.set_max_entries(max_entries);
                Ok((h, None))
            }
            Err(parse) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut backup = path.with_file_name(format!("{name}.corrupt-{}", crate::now_unix()));
                let mut n = 1;
                while backup.exists() {
                    backup = path.with_file_name(format!("{name}.corrupt-{}-{n}", crate::now_unix()));
                    n += 1;
                }
                std::fs::write(&backup, &bytes).map_err(|e| {
                    crate::Error::Config(format!(
                        "{} cannot be read ({parse}) and could not be copied aside ({e})",
                        path.display()
                    ))
                })?;
                Ok((History::new(max_entries), Some(backup)))
            }
        }
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Insert an already-built entry at the bottom (oldest end). Used by the
    /// importer, which reads oldest-last.
    pub fn push_oldest(&mut self, mut entry: HistoryEntry) {
        entry.id = self.next_id.max(1);
        self.next_id = entry.id + 1;
        self.entries.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(provider: &str, text: &str) -> HistoryResult {
        HistoryResult { provider_id: provider.into(), from: LangId::RUSSIAN, to: LangId::ENGLISH, text: text.into() }
    }

    #[test]
    fn same_text_across_services_is_one_entry() {
        let mut h = History::new(100);
        let a = h.record("привет", res("google", "hello"), 1);
        let b = h.record("привет", res("deepl", "hi"), 2);
        assert_eq!(a, b);
        assert_eq!(h.len(), 1);
        assert_eq!(h.iter().next().unwrap().results.len(), 2);
        assert_eq!(h.iter().next().unwrap().latest().unwrap().provider_id, "deepl");
        // The same service again replaces its own earlier result.
        h.record("привет", res("google", "hey"), 3);
        let e = h.iter().next().unwrap();
        assert_eq!(e.results.len(), 2);
        assert_eq!(e.latest().unwrap().text, "hey");
    }

    #[test]
    fn re_translating_an_old_text_moves_it_to_the_top() {
        let mut h = History::new(100);
        let first = h.record("one", res("g", "1"), 1);
        h.set_favorite(first, true);
        h.record("two", res("g", "2"), 2);
        h.record("one", res("d", "1!"), 3);
        let top = h.iter().next().unwrap();
        assert_eq!(top.source, "one");
        assert_eq!(top.id, first, "keeps its identity");
        assert!(top.favorite, "keeps its favourite flag");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn empty_input_or_output_is_not_recorded() {
        let mut h = History::new(100);
        assert_eq!(h.record("  ", res("g", "x"), 1), 0);
        assert_eq!(h.record("x", res("g", " "), 1), 0);
        assert!(h.is_empty());
    }

    #[test]
    fn the_limit_drops_the_oldest_but_never_a_favourite() {
        let mut h = History::new(3);
        let oldest = h.record("a", res("g", "a"), 1);
        h.set_favorite(oldest, true);
        h.record("b", res("g", "b"), 2);
        h.record("c", res("g", "c"), 3);
        h.record("d", res("g", "d"), 4);
        let sources: Vec<&str> = h.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(sources, ["d", "c", "a"], "b went, the favourite a stayed");
    }

    #[test]
    fn search_and_favourites_filter() {
        let mut h = History::new(100);
        let id = h.record("Hello World", res("g", "Привет мир"), 1);
        h.record("Goodbye", res("g", "Пока"), 2);
        h.set_favorite(id, true);
        assert_eq!(h.search("world", false).len(), 1);
        assert_eq!(h.search("МИР", false).len(), 1, "matches translations, case-insensitively");
        assert_eq!(h.search("", false).len(), 2);
        assert_eq!(h.search("", true).len(), 1);
        assert_eq!(h.search("goodbye", true).len(), 0);
    }

    #[test]
    fn clear_keeps_favourites_by_default() {
        let mut h = History::new(100);
        let fav = h.record("a", res("g", "a"), 1);
        h.record("b", res("g", "b"), 2);
        h.set_favorite(fav, true);
        h.clear(false);
        assert_eq!(h.len(), 1);
        h.clear(true);
        assert!(h.is_empty());
    }

    #[test]
    fn persists_and_ids_continue_after_reload() {
        let dir = std::env::temp_dir().join(format!("glossa-hist-{}", std::process::id()));
        let path = dir.join("history.json");
        let mut h = History::new(100);
        h.record("a", res("g", "a"), 1);
        let b = h.record("b", res("g", "b"), 2);
        h.save(&path).unwrap();
        let mut back = History::load(&path, 100).unwrap();
        assert_eq!(back.len(), 2);
        let c = back.record("c", res("g", "c"), 3);
        assert!(c > b, "new ids must not collide with loaded ones");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_file_is_copied_aside_before_anything_can_replace_it() {
        let dir = std::env::temp_dir().join(format!("glossa-hist-corrupt-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");

        // Missing: empty, nothing copied.
        let (h, backup) = History::load_recovering(&path, 100).unwrap();
        assert!(h.is_empty() && backup.is_none());

        std::fs::write(&path, b"{\"entries\": [ {\"id\": 1, \"source\": \"hand-edited\",").unwrap();
        let (h, backup) = History::load_recovering(&path, 100).unwrap();
        assert!(h.is_empty());
        let backup = backup.expect("a copy was made");
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("history.json.corrupt-"));
        assert_eq!(std::fs::read(&backup).unwrap(), std::fs::read(&path).unwrap());
        // A second failed load keeps the first copy and makes another.
        let (_, again) = History::load_recovering(&path, 100).unwrap();
        assert_ne!(again.unwrap(), backup);

        // A good file loads as before.
        let mut good = History::new(100);
        good.record("a", res("g", "a"), 1);
        good.save(&path).unwrap();
        let (h, backup) = History::load_recovering(&path, 100).unwrap();
        assert_eq!(h.len(), 1);
        assert!(backup.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
