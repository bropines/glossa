//! The offline-dictionaries provider: every opened dictionary file behind
//! one [`Provider`].
//!
//! QTranslate let the user drop dictionary files into a folder and showed
//! their articles alongside the online services'. Here that is one provider,
//! id `offline`, holding any number of [`DictionarySource`]s. A lookup asks
//! every source whose language pair fits the request (and every source whose
//! pair is unknown) and merges the answers into a single [`Article`]: one
//! explanation group per source, labelled with the source's name, and the
//! sources' HTML concatenated under a heading each.
//!
//! "Nothing found" is **not** an error: [`Provider::dictionary`] returns
//! `Ok` with an empty article, and callers check [`Article::is_empty`] to show
//! "no results". An error would make the engine fail over to the next
//! provider and report a fault where there is none.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use glossa_core::dict::Explanation;
use glossa_core::{Article, Capabilities, DictRequest, Error, LangId, ProviderInfo, Result};
use glossa_modules::{Host, Provider};

use crate::dsl::Dsl;
use crate::html::escape;
use crate::source::{merge_into, DictionarySource};
use crate::stardict::StarDict;
use crate::tsv::Tsv;
use crate::xdxf::Xdxf;

/// Map an I/O failure while opening onto the shared error type: a missing
/// file is a configuration fault the UI can name.
fn open_error(path: &Path, e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        Error::FileNotFound(format!("{}: {e}", path.display()))
    } else {
        Error::Io(e)
    }
}

/// Open a dictionary file, choosing the reader by extension and, failing
/// that, by content.
///
/// | extension                         | format       |
/// |-----------------------------------|--------------|
/// | `.ifo` (or `.idx`, `.dict`, `.dict.dz` next to one) | StarDict |
/// | `.dsl`, `.dsl.dz`                 | ABBYY Lingvo DSL |
/// | `.xdxf`, `.xdxf.dz`               | XDXF         |
/// | `.tsv`, `.txt`, `.csv`, `.tab`    | word list    |
pub fn open(path: &Path) -> Result<Box<dyn DictionarySource>> {
    if !path.is_file() {
        return Err(Error::FileNotFound(path.display().to_string()));
    }
    let lower = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ends = |suffixes: &[&str]| suffixes.iter().any(|s| lower.ends_with(s));
    let err = |e| open_error(path, e);

    if ends(&[".ifo"]) {
        return Ok(Box::new(StarDict::open(path).map_err(err)?));
    }
    if ends(&[".idx", ".idx.gz", ".dict", ".dict.dz", ".syn"]) {
        // Somebody picked another file of the set; find its .ifo.
        let stem = lower
            .trim_end_matches(".gz")
            .trim_end_matches(".dz")
            .trim_end_matches(".idx")
            .trim_end_matches(".dict")
            .trim_end_matches(".syn")
            .len();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ifo = path.with_file_name(format!("{}.ifo", &name[..stem.min(name.len())]));
        if ifo.is_file() {
            return Ok(Box::new(StarDict::open(&ifo).map_err(err)?));
        }
        return Err(Error::FileNotFound(ifo.display().to_string()));
    }
    if ends(&[".dsl", ".dsl.dz"]) {
        return Ok(Box::new(Dsl::open(path).map_err(err)?));
    }
    if ends(&[".xdxf", ".xdxf.dz", ".xdxf.gz"]) {
        return Ok(Box::new(Xdxf::open(path).map_err(err)?));
    }
    if ends(&[".tsv", ".txt", ".csv", ".tab"]) {
        return Ok(Box::new(Tsv::open(path).map_err(err)?));
    }

    // Unknown extension: sniff the start of the (possibly gzipped) file.
    let bytes = crate::source::read_maybe_gzip(path).map_err(err)?;
    let head = crate::source::decode_text(&bytes[..bytes.len().min(4096)], false);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("StarDict's dict ifo file") {
        return Ok(Box::new(StarDict::open(path).map_err(err)?));
    }
    if head.contains("<xdxf") {
        return Ok(Box::new(Xdxf::open(path).map_err(err)?));
    }
    if head.starts_with("#NAME") || head.starts_with("#INDEX_LANGUAGE") {
        return Ok(Box::new(Dsl::open(path).map_err(err)?));
    }
    Err(Error::Config(format!("{}: not a dictionary format Glossa can read", path.display())))
}

/// The `offline` provider.
pub struct OfflineDictionaries {
    info: ProviderInfo,
    sources: RwLock<Vec<Arc<dyn DictionarySource>>>,
}

impl Default for OfflineDictionaries {
    fn default() -> Self {
        Self::new()
    }
}

impl OfflineDictionaries {
    pub fn new() -> Self {
        let mut info =
            ProviderInfo::new("offline", "Offline dictionaries", Capabilities::DICTIONARY | Capabilities::OFFLINE);
        info.description = "Looks words up in local StarDict, Lingvo DSL, XDXF and tab-separated dictionaries.".into();
        OfflineDictionaries { info, sources: RwLock::new(Vec::new()) }
    }

    /// Open a dictionary file (see [`open`]).
    pub fn open(path: &Path) -> Result<Box<dyn DictionarySource>> {
        open(path)
    }

    /// Open a file and add it.
    pub fn add_path(&self, path: &Path) -> Result<Arc<dyn DictionarySource>> {
        let source: Arc<dyn DictionarySource> = Arc::from(open(path)?);
        self.add(source.clone());
        Ok(source)
    }

    /// Add an opened dictionary. Sources are consulted in the order added.
    pub fn add(&self, source: Arc<dyn DictionarySource>) {
        self.sources.write().unwrap_or_else(|e| e.into_inner()).push(source);
    }

    /// Remove the first dictionary called `name`. Returns whether one was.
    pub fn remove(&self, name: &str) -> bool {
        let mut sources = self.sources.write().unwrap_or_else(|e| e.into_inner());
        match sources.iter().position(|s| s.name() == name) {
            Some(i) => {
                sources.remove(i);
                true
            }
            None => false,
        }
    }

    /// The dictionaries, in lookup order.
    pub fn list(&self) -> Vec<Arc<dyn DictionarySource>> {
        self.sources.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Completions for the dictionary window, merged across sources.
    pub fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in self.list() {
            for w in s.suggest(prefix, limit) {
                if !out.iter().any(|o| o.to_lowercase() == w.to_lowercase()) {
                    out.push(w);
                }
            }
        }
        out.sort_by_key(|w| w.to_lowercase());
        out.truncate(limit);
        out
    }

    /// Whether a source with this language pair should answer `req`.
    fn fits(pair: Option<(LangId, LangId)>, req: &DictRequest) -> bool {
        let Some((from, to)) = pair else { return true };
        let matches = |want: LangId, have: LangId| !want.is_language() || want == have;
        matches(req.from, from) && matches(req.to, to)
    }
}

impl Provider for OfflineDictionaries {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    /// Look the word up in every fitting dictionary. Returns `Ok` with an
    /// empty article when nothing matched — check [`Article::is_empty`].
    fn dictionary(&self, host: &Host, req: &DictRequest) -> Result<Article> {
        let started = Instant::now();
        let word = req.text.trim();
        if word.is_empty() {
            return Err(Error::EmptyInput);
        }
        let mut article = Article { provider_id: self.info.id.clone(), ..Default::default() };
        let mut html = String::new();
        for source in self.list() {
            host.check_cancelled()?;
            if !Self::fits(source.lang_pair(), req) {
                continue;
            }
            let entries = source.lookup(word, req.exact);
            if entries.is_empty() {
                continue;
            }
            let name = source.name().to_owned();
            html.push_str(&format!("<h3>{}</h3>", escape(&name)));
            let mut from_source = Article::default();
            let mut source_html = String::new();
            for entry in &entries {
                if article.headword.is_empty() {
                    article.headword = entry.headword.clone();
                }
                source_html.push_str(&format!("<p><b>{}</b></p>", escape(&entry.headword)));
                merge_into(&mut from_source, entry.to_article(), &mut source_html);
            }
            html.push_str(&source_html);
            // One group per source, labelled with its name; a part of speech
            // the source gave rides along in the label.
            for e in from_source.explanations {
                let label =
                    if e.part_of_speech.is_empty() { name.clone() } else { format!("{name} · {}", e.part_of_speech) };
                article.explanations.push(Explanation { part_of_speech: label, definitions: e.definitions });
            }
            for p in from_source.pronunciations {
                if !article.pronunciations.contains(&p) {
                    article.pronunciations.push(p);
                }
            }
            article.examples.extend(from_source.examples);
            for a in from_source.associations {
                if !article.associations.contains(&a) {
                    article.associations.push(a);
                }
            }
        }
        if article.headword.is_empty() {
            article.headword = word.to_owned();
        }
        if !html.is_empty() {
            article.html = Some(html);
        }
        article.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(article)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::testutil::{utf16le_with_bom, TempDir};
    use crate::source::{Body, RawEntry};
    use crate::stardict::testutil::Fixture;
    use glossa_modules::provider::with_test_host;
    use glossa_modules::OptionValues;
    use glossa_net::mock::MockHttp;

    fn req(text: &str, from: LangId, to: LangId, exact: bool) -> DictRequest {
        DictRequest { text: text.into(), from, to, exact }
    }

    fn lookup(p: &OfflineDictionaries, r: &DictRequest) -> Result<Article> {
        let http = MockHttp::new();
        let opts = OptionValues::new();
        with_test_host(&http, &opts, |host| p.dictionary(host, r))
    }

    /// A fixed source for provider tests.
    struct Fake(&'static str, Option<(LangId, LangId)>, &'static str);

    impl DictionarySource for Fake {
        fn name(&self) -> &str {
            self.0
        }
        fn lang_pair(&self) -> Option<(LangId, LangId)> {
            self.1
        }
        fn entry_count(&self) -> usize {
            1
        }
        fn lookup(&self, word: &str, _exact: bool) -> Vec<RawEntry> {
            if word.eq_ignore_ascii_case("cat") {
                vec![RawEntry::new("cat", Body::Plain(self.2.into()))]
            } else {
                Vec::new()
            }
        }
        fn suggest(&self, prefix: &str, _limit: usize) -> Vec<String> {
            if "cat".starts_with(prefix) {
                vec!["cat".into()]
            } else {
                Vec::new()
            }
        }
    }

    #[test]
    fn provider_identity() {
        let p = OfflineDictionaries::new();
        assert_eq!(p.info().id, "offline");
        assert_eq!(p.info().name, "Offline dictionaries");
        assert!(p.info().supports(Capabilities::DICTIONARY | Capabilities::OFFLINE));
    }

    #[test]
    fn results_merge_one_group_per_source() {
        let p = OfflineDictionaries::new();
        p.add(Arc::new(Fake("First", Some((LangId::ENGLISH, LangId::RUSSIAN)), "кошка")));
        p.add(Arc::new(Fake("Second", None, "кот\nкиса")));
        let a = lookup(&p, &req("CAT", LangId::ENGLISH, LangId::RUSSIAN, true)).unwrap();
        assert_eq!(a.headword, "cat");
        assert_eq!(a.provider_id, "offline");
        assert_eq!(a.explanations.len(), 2);
        assert_eq!(a.explanations[0].part_of_speech, "First");
        assert_eq!(a.explanations[0].definitions, vec!["кошка"]);
        assert_eq!(a.explanations[1].part_of_speech, "Second");
        assert_eq!(a.explanations[1].definitions, vec!["кот", "киса"]);
        let html = a.html.unwrap();
        assert!(html.contains("<h3>First</h3>") && html.contains("<h3>Second</h3>"), "{html}");
        assert!(html.find("First").unwrap() < html.find("Second").unwrap());
    }

    #[test]
    fn language_pair_filters_sources() {
        let p = OfflineDictionaries::new();
        p.add(Arc::new(Fake("EnRu", Some((LangId::ENGLISH, LangId::RUSSIAN)), "кошка")));
        p.add(Arc::new(Fake("Unknown", None, "кот")));
        let a = lookup(&p, &req("cat", LangId::RUSSIAN, LangId::ENGLISH, true)).unwrap();
        assert_eq!(a.explanations.len(), 1);
        assert_eq!(a.explanations[0].part_of_speech, "Unknown");
        // Auto-detect fits everything.
        let a = lookup(&p, &req("cat", LangId::AUTO, LangId::RUSSIAN, true)).unwrap();
        assert_eq!(a.explanations.len(), 2);
    }

    #[test]
    fn nothing_found_is_an_empty_article_not_an_error() {
        let p = OfflineDictionaries::new();
        p.add(Arc::new(Fake("F", None, "x")));
        let a = lookup(&p, &req("dog", LangId::AUTO, LangId::AUTO, true)).unwrap();
        assert!(a.is_empty());
        assert_eq!(a.headword, "dog");
        assert!(matches!(lookup(&p, &req("  ", LangId::AUTO, LangId::AUTO, true)), Err(Error::EmptyInput)));
        assert!(lookup(&OfflineDictionaries::new(), &req("x", LangId::AUTO, LangId::AUTO, true)).unwrap().is_empty());
    }

    #[test]
    fn add_remove_list_suggest() {
        let p = OfflineDictionaries::new();
        p.add(Arc::new(Fake("A", None, "x")));
        p.add(Arc::new(Fake("B", None, "y")));
        assert_eq!(p.list().iter().map(|s| s.name().to_owned()).collect::<Vec<_>>(), vec!["A", "B"]);
        assert_eq!(p.suggest("c", 10), vec!["cat"]);
        assert!(p.remove("A"));
        assert!(!p.remove("A"));
        assert_eq!(p.list().len(), 1);
    }

    #[test]
    fn open_dispatches_on_extension_and_content() {
        let dir = TempDir::new("offline");
        let ifo = Fixture::new("Star (En-Ru)", vec![("cat", "кошка".as_bytes().to_vec())]).write(dir.path(), "star");
        let dsl = dir.write(
            "lingvo.dsl",
            &utf16le_with_bom("#NAME \"Lingvo\"\n#INDEX_LANGUAGE \"English\"\n#CONTENTS_LANGUAGE \"Russian\"\ncat\n\t[trn]кот[/trn]\n"),
        );
        let xdxf = dir.write(
            "x.xdxf",
            "<xdxf lang_from=\"ENG\" lang_to=\"RUS\"><full_name>X</full_name><ar><k>cat</k>киса</ar></xdxf>".as_bytes(),
        );
        let tsv = dir.write("words.tsv", "cat\tкотик\n".as_bytes());
        let sniffed = dir.write("mystery.dict-file", "#NAME \"Sniffed\"\ncat\n\tкошара\n".as_bytes());

        let p = OfflineDictionaries::new();
        for path in [&ifo, &dsl, &xdxf, &tsv, &sniffed] {
            p.add_path(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
        // Picking the .dict of a StarDict set finds its .ifo.
        assert_eq!(open(&dir.path().join("star.dict")).unwrap().name(), "Star (En-Ru)");

        let a = lookup(&p, &req("Cat", LangId::ENGLISH, LangId::RUSSIAN, true)).unwrap();
        let labels: Vec<&str> = a.explanations.iter().map(|e| e.part_of_speech.as_str()).collect();
        assert_eq!(labels, vec!["Star (En-Ru)", "Lingvo", "X", "words", "Sniffed"]);
        let defs: Vec<&str> = a.explanations.iter().map(|e| e.definitions[0].as_str()).collect();
        assert_eq!(defs, vec!["кошка", "кот", "киса", "котик", "кошара"]);
        assert!(!a.is_empty());
    }

    #[test]
    fn open_reports_missing_and_unknown_files() {
        let dir = TempDir::new("offline-bad");
        assert!(matches!(open(&dir.path().join("nope.ifo")), Err(Error::FileNotFound(_))));
        let junk = dir.write("junk.bin", &[0, 1, 2, 3]);
        assert!(matches!(open(&junk), Err(Error::Config(_))));
        let lonely = dir.write("lonely.dict", b"data");
        assert!(matches!(open(&lonely), Err(Error::FileNotFound(_))));
        let bad_ifo = dir.write("bad.ifo", b"StarDict's dict ifo file\nbookname=x\n");
        assert!(matches!(open(&bad_ifo), Err(Error::FileNotFound(_))));
        let corrupt = dir.write("corrupt.xdxf", b"no root element here");
        assert!(matches!(open(&corrupt), Err(Error::Io(_))));
    }
}
