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

    /// Put back an entry that [`History::remove`] took out (an undone
    /// delete), with its id, where its time places it among the others.
    /// Refused when the id is in use, or when the same text has been
    /// translated again since: that newer entry is the one to keep.
    pub fn restore(&mut self, entry: HistoryEntry) -> bool {
        if entry.id == 0 || self.entries.iter().any(|e| e.id == entry.id || e.source == entry.source) {
            return false;
        }
        // Ids are never reused, so one taken out and put back stays below
        // the counter; this guards a history loaded in between.
        self.next_id = self.next_id.max(entry.id + 1);
        let at = self.entries.iter().position(|e| e.created < entry.created).unwrap_or(self.entries.len());
        self.entries.insert(at, entry);
        self.trim();
        true
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
    /// Drop the oldest non-favourites until the limit holds. The newest
    /// entry always stays: it is the one `record` just added and returned
    /// the id of, and when favourites alone fill the limit it goes over the
    /// limit with them rather than vanish.
    fn trim(&mut self) {
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() > self.max_entries {
            match self.entries.iter().skip(1).rposition(|e| !e.favorite) {
                Some(i) => {
                    self.entries.remove(i + 1);
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
    fn a_removed_entry_goes_back_where_it_was() {
        let mut h = History::new(10);
        let a = h.record("one", res("g", "1"), 100);
        let b = h.record("two", res("g", "2"), 200);
        let c = h.record("three", res("g", "3"), 300);
        h.set_favorite(b, true);
        let taken = h.get(b).unwrap().clone();
        assert!(h.remove(b));
        assert!(h.restore(taken.clone()));
        assert_eq!(h.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b, a]);
        assert!(h.get(b).unwrap().favorite, "with everything it had");
        assert!(!h.restore(taken.clone()), "not twice");
        // Translated again since it was deleted: the new entry stays.
        assert!(h.remove(b));
        let again = h.record("two", res("g", "2"), 400);
        assert!(!h.restore(taken));
        assert_eq!(h.iter().map(|e| e.id).collect::<Vec<_>>(), vec![again, c, a]);
        // A new entry never takes a restored one's id.
        assert!(h.record("four", res("g", "4"), 500) > again);
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

    fn sources(h: &History) -> Vec<&str> {
        h.iter().map(|e| e.source.as_str()).collect()
    }

    #[test]
    fn surrounding_whitespace_does_not_make_a_new_entry() {
        let mut h = History::new(100);
        let a = h.record("  hello\n", res("g", "привет"), 1);
        let b = h.record("hello", res("d", "здравствуй"), 2);
        assert_eq!(a, b);
        assert_eq!(sources(&h), ["hello"], "stored trimmed");
    }

    #[test]
    fn joining_an_entry_refreshes_its_time() {
        let mut h = History::new(100);
        let id = h.record("a", res("g", "a"), 10);
        h.record("a", res("d", "a"), 20);
        assert_eq!(h.get(id).unwrap().created, 20);
        h.record("b", res("g", "b"), 30);
        h.record("a", res("g", "a2"), 40);
        assert_eq!(h.get(id).unwrap().created, 40, "moved to the top with the new time");
    }

    #[test]
    fn a_different_direction_from_the_same_service_is_kept_beside_the_first() {
        let mut h = History::new(100);
        h.record("hello", res("g", "привет"), 1);
        let de = HistoryResult {
            provider_id: "g".into(),
            from: LangId::RUSSIAN,
            to: LangId::from_code("de").unwrap(),
            text: "hallo".into(),
        };
        h.record("hello", de, 2);
        let e = h.iter().next().unwrap();
        assert_eq!(e.results.len(), 2, "one per target language");
        assert_eq!(e.latest().unwrap().text, "hallo");
    }

    #[test]
    fn a_zero_limit_keeps_everything() {
        let mut h = History::new(0);
        for i in 0..50 {
            h.record(&format!("t{i}"), res("g", "x"), i);
        }
        assert_eq!(h.len(), 50);
    }

    #[test]
    fn lowering_the_limit_trims_at_once_from_the_oldest_end() {
        let mut h = History::new(100);
        for s in ["a", "b", "c", "d", "e"] {
            h.record(s, res("g", s), 1);
        }
        h.set_max_entries(2);
        assert_eq!(sources(&h), ["e", "d"]);
    }

    #[test]
    fn favourites_may_exceed_the_limit() {
        let mut h = History::new(100);
        for s in ["a", "b", "c"] {
            let id = h.record(s, res("g", s), 1);
            h.set_favorite(id, true);
        }
        h.set_max_entries(2);
        assert_eq!(sources(&h), ["c", "b", "a"], "the limit never drops a favourite");
    }

    /// When favourites alone fill the limit, the entry `record` just added
    /// stays (over the limit, as favourites may be) instead of being trimmed
    /// at once: the id `record` returns reaches the UI and
    /// `Engine::record_translation`, and must name an entry.
    #[test]
    fn a_new_entry_is_not_dropped_by_its_own_trim() {
        let mut h = History::new(2);
        for s in ["a", "b"] {
            let id = h.record(s, res("g", s), 1);
            h.set_favorite(id, true);
        }
        let id = h.record("c", res("g", "c"), 2);
        assert!(h.get(id).is_some(), "record returned {id}, which is not in {:?}", sources(&h));
        assert_eq!(sources(&h), ["c", "b", "a"]);
        // The next one pushes the previous non-favourite out, not a favourite.
        let id = h.record("d", res("g", "d"), 3);
        assert!(h.get(id).is_some());
        assert_eq!(sources(&h), ["d", "b", "a"]);
    }

    #[test]
    fn unknown_ids_are_reported_not_ignored() {
        let mut h = History::new(100);
        let id = h.record("a", res("g", "a"), 1);
        assert!(!h.set_favorite(id + 100, true));
        assert!(!h.remove(id + 100));
        assert!(h.get(id + 100).is_none());
        assert!(h.remove(id));
        assert!(!h.remove(id), "already gone");
        assert!(h.is_empty());
    }

    #[test]
    fn a_removed_id_is_not_handed_out_again() {
        // The UI and the control API refer to entries by id; reusing one
        // could act on the wrong entry.
        let mut h = History::new(100);
        let a = h.record("a", res("g", "a"), 1);
        h.remove(a);
        let b = h.record("b", res("g", "b"), 2);
        assert_ne!(a, b);
        h.clear(true);
        let c = h.record("c", res("g", "c"), 3);
        assert!(c > b, "not even after clearing");
    }

    #[test]
    fn ids_start_at_one_even_from_a_default_history() {
        let mut h = History::default();
        assert_eq!(h.record("a", res("g", "a"), 1), 1, "0 means 'not recorded'");
        assert_eq!(h.record("b", res("g", "b"), 1), 2);
    }

    #[test]
    fn search_trims_the_needle_and_ignores_unrelated_entries() {
        let mut h = History::new(100);
        h.record("Hello", res("g", "Привет"), 1);
        h.record("Cat", res("g", "Кот"), 2);
        let found = h.search("  hello  ", false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, "Hello");
        assert!(h.search("dog", false).is_empty());
        // Newest first, as the list shows them.
        let all: Vec<&str> = h.search("   ", false).iter().map(|e| e.source.as_str()).collect();
        assert_eq!(all, ["Cat", "Hello"]);
    }

    #[test]
    fn loading_applies_the_current_limit() {
        let dir = std::env::temp_dir().join(format!("glossa-hist-limit-{}", std::process::id()));
        let path = dir.join("history.json");
        let mut h = History::new(100);
        for s in ["a", "b", "c", "d"] {
            h.record(s, res("g", s), 1);
        }
        h.save(&path).unwrap();
        let back = History::load(&path, 2).unwrap();
        assert_eq!(sources(&back), ["d", "c"]);
        let (back, _) = History::load_recovering(&path, 3).unwrap();
        assert_eq!(back.len(), 3);
        assert!(!path.with_extension("json.tmp").exists(), "the temp file is renamed away");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_next_id_in_the_file_is_corrected_on_load() {
        // A hand-edited or imported file may carry entries with ids at or
        // above its `next_id`; new entries must not collide with them.
        let dir = std::env::temp_dir().join(format!("glossa-hist-nextid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let file = r#"{"entries":[{"id":7,"source":"old","results":[]}],"next_id":2}"#;
        std::fs::write(&path, file).unwrap();
        for mut h in [History::load(&path, 100).unwrap(), History::load_recovering(&path, 100).unwrap().0] {
            let old = h.get(7).expect("loaded");
            assert!(!old.favorite && old.created == 0, "optional fields default");
            assert_eq!(h.record("new", res("g", "n"), 1), 8);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn imported_entries_go_to_the_bottom_with_fresh_ids() {
        let mut h = History::new(100);
        let a = h.record("a", res("g", "a"), 1);
        let imported = HistoryEntry { id: a, source: "old".into(), results: vec![], favorite: true, created: 0 };
        h.push_oldest(imported);
        assert_eq!(sources(&h), ["a", "old"]);
        let old_id = h.iter().last().unwrap().id;
        assert_ne!(old_id, a, "the file's id is replaced");
        assert!(h.get(old_id).unwrap().favorite, "the rest is kept");
        assert!(h.record("b", res("g", "b"), 2) > old_id);
    }
}
