//! StarDict dictionaries: `.ifo` + `.idx` (or `.idx.gz`) + `.dict` (or
//! `.dict.dz`), optionally `.syn`.
//!
//! The format is built for lookups straight off disk, and so is this reader:
//!
//! * The `.idx` is read into memory once (it is needed for every search) but
//!   kept as the raw bytes plus a table of `u32` entry offsets — no `String`
//!   per headword. A 100k-entry index costs its file size plus 400 KB.
//! * The index is sorted the way StarDict sorts it (`g_ascii_strcasecmp`,
//!   ties broken by `strcmp`), so a lookup is a binary search. Indexes that
//!   are not sorted (hand-made, or by a tool with a different collation) are
//!   detected at open and get a sorted permutation instead of wrong answers.
//! * Article data is read on demand. A `.dict.dz` is a *dictzip* file: gzip
//!   whose `FEXTRA` field carries an `RA` subfield listing the compressed size
//!   of every fixed-length chunk, with the deflate stream fully flushed at
//!   each chunk boundary. That allows inflating only the chunks an article
//!   touches; a small LRU keeps recently used chunks. A `.dz` without the
//!   `RA` field is inflated whole into memory as a fallback.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use glossa_core::LangId;

use crate::source::{
    guess_lang_pair, invalid, lang_from_any, normalize_key, read_maybe_gzip, Body, DictionarySource, RawEntry,
    MAX_PREFIX_RESULTS,
};

/// The first line of every `.ifo`.
const IFO_MAGIC: &str = "StarDict's dict ifo file";

/// Refuse to allocate more than this for one article: a corrupt size field
/// must not take the process down.
const MAX_ENTRY_SIZE: usize = 32 << 20;

/// Decompressed dictzip chunks kept around. Neighbouring articles usually
/// share a chunk, and a chunk is ~58 KB.
const CHUNK_CACHE: usize = 8;

/// What the `.ifo` says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ifo {
    pub version: String,
    pub bookname: String,
    pub wordcount: usize,
    pub synwordcount: usize,
    pub idxfilesize: u64,
    /// 32 or 64.
    pub idxoffsetbits: u32,
    pub sametypesequence: Option<String>,
    pub description: String,
    /// Source and target language when the `.ifo` names them (a
    /// non-standard `lang=en-ru`, or `sourcelang`/`targetlang`).
    pub lang_pair: Option<(LangId, LangId)>,
}

impl Ifo {
    pub fn parse(text: &str, path: &Path) -> io::Result<Ifo> {
        let mut lines = text.trim_start_matches('\u{feff}').lines();
        if lines.next().map(str::trim) != Some(IFO_MAGIC) {
            return Err(invalid(path, "not a StarDict .ifo file"));
        }
        let mut ifo = Ifo { idxoffsetbits: 32, ..Default::default() };
        let (mut src, mut dst) = (None, None);
        for line in lines {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "version" => ifo.version = value.to_owned(),
                "bookname" => ifo.bookname = value.to_owned(),
                "wordcount" => ifo.wordcount = value.parse().unwrap_or(0),
                "synwordcount" => ifo.synwordcount = value.parse().unwrap_or(0),
                "idxfilesize" => ifo.idxfilesize = value.parse().unwrap_or(0),
                "idxoffsetbits" => {
                    ifo.idxoffsetbits = match value {
                        "32" => 32,
                        "64" => 64,
                        other => return Err(invalid(path, format!("unsupported idxoffsetbits={other}"))),
                    }
                }
                "sametypesequence" if !value.is_empty() => ifo.sametypesequence = Some(value.to_owned()),
                "description" => ifo.description = value.to_owned(),
                "lang" => ifo.lang_pair = guess_lang_pair(value),
                "sourcelang" | "source_lang" | "langfrom" | "lang_from" => src = lang_from_any(value),
                "targetlang" | "target_lang" | "langto" | "lang_to" => dst = lang_from_any(value),
                _ => {}
            }
        }
        if let (Some(a), Some(b)) = (src, dst) {
            ifo.lang_pair = Some((a, b));
        }
        Ok(ifo)
    }
}

/// `g_ascii_strcasecmp`: byte-wise, ASCII letters folded.
fn ascii_casecmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let (x, y) = (a[i].to_ascii_lowercase(), b[i].to_ascii_lowercase());
        if x != y {
            return x.cmp(&y);
        }
    }
    a.len().cmp(&b.len())
}

/// StarDict's collation: case-insensitive first, `strcmp` to break ties.
fn stardict_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    ascii_casecmp(a, b).then_with(|| a.cmp(b))
}

/// A `.idx` or `.syn` held as raw bytes plus entry offsets. Each record is
/// a NUL-terminated word followed by `tail` bytes of payload.
struct WordIndex {
    data: Vec<u8>,
    starts: Vec<u32>,
    /// Logical (sorted) position → record number, only when the file was
    /// not sorted.
    order: Option<Vec<u32>>,
    tail: usize,
}

impl WordIndex {
    fn parse(data: Vec<u8>, tail: usize, path: &Path) -> io::Result<WordIndex> {
        if data.len() > u32::MAX as usize {
            return Err(invalid(path, "index larger than 4 GiB"));
        }
        let mut starts = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            let Some(nul) = data[pos..].iter().position(|&b| b == 0) else {
                return Err(invalid(path, format!("truncated record at byte {pos}")));
            };
            let end = pos + nul + 1 + tail;
            if end > data.len() {
                return Err(invalid(path, format!("truncated record at byte {pos}")));
            }
            starts.push(pos as u32);
            pos = end;
        }
        starts.shrink_to_fit();
        let mut index = WordIndex { data, starts, order: None, tail };
        let sorted = (1..index.starts.len())
            .all(|i| ascii_casecmp(index.word(i - 1), index.word(i)) != std::cmp::Ordering::Greater);
        if !sorted {
            tracing::warn!("{}: index is not sorted; sorting in memory", path.display());
            let mut order: Vec<u32> = (0..index.starts.len() as u32).collect();
            order.sort_by(|&a, &b| stardict_cmp(index.word(a as usize), index.word(b as usize)));
            index.order = Some(order);
        }
        Ok(index)
    }

    fn len(&self) -> usize {
        self.starts.len()
    }

    fn physical(&self, pos: usize) -> usize {
        match &self.order {
            Some(o) => o[pos] as usize,
            None => pos,
        }
    }

    /// The word of record `i` (physical numbering).
    fn word(&self, i: usize) -> &[u8] {
        let rest = &self.data[self.starts[i] as usize..];
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        &rest[..end]
    }

    fn payload(&self, i: usize) -> &[u8] {
        let start = self.starts[i] as usize + self.word(i).len() + 1;
        &self.data[start..start + self.tail]
    }

    /// First logical position whose word is not less than `key`.
    fn lower_bound(&self, key: &[u8]) -> usize {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if ascii_casecmp(self.word(self.physical(mid)), key) == std::cmp::Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Records whose word starts with `key` (ASCII case folded), in logical
    /// order, as physical record numbers. Stops after `scan` records.
    fn prefixed(&self, key: &[u8], exact_only: bool, scan: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut pos = self.lower_bound(key);
        while pos < self.len() && out.len() < scan {
            let i = self.physical(pos);
            let w = self.word(i);
            if w.len() < key.len() || !w[..key.len()].eq_ignore_ascii_case(key) {
                break;
            }
            if exact_only && w.len() != key.len() {
                break;
            }
            out.push(i);
            pos += 1;
        }
        out
    }
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be_u64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn le_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// Where the articles live.
enum DictData {
    File { file: Mutex<File>, len: u64 },
    DictZip(DictZip),
    Memory(Vec<u8>),
}

impl DictData {
    fn open(path: &Path) -> io::Result<DictData> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let mut magic = [0u8; 2];
        let is_gzip = len >= 2 && (&file).read_exact(&mut magic).is_ok() && magic == [0x1f, 0x8b];
        if !is_gzip {
            return Ok(DictData::File { file: Mutex::new(file), len });
        }
        match DictZip::open(file, len, path)? {
            Some(dz) => Ok(DictData::DictZip(dz)),
            None => {
                tracing::info!("{}: gzip without dictzip chunks; inflating it whole", path.display());
                Ok(DictData::Memory(read_maybe_gzip(path)?))
            }
        }
    }

    fn read(&self, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        if size > MAX_ENTRY_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("article of {size} bytes")));
        }
        let out_of_range = || io::Error::new(io::ErrorKind::UnexpectedEof, "article beyond end of .dict");
        match self {
            DictData::File { file, len } => {
                if offset.checked_add(size as u64).is_none_or(|end| end > *len) {
                    return Err(out_of_range());
                }
                let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
                f.seek(SeekFrom::Start(offset))?;
                let mut buf = vec![0; size];
                f.read_exact(&mut buf)?;
                Ok(buf)
            }
            DictData::Memory(data) => {
                let start = usize::try_from(offset).map_err(|_| out_of_range())?;
                let end = start.checked_add(size).ok_or_else(out_of_range)?;
                data.get(start..end).map(<[u8]>::to_vec).ok_or_else(out_of_range)
            }
            DictData::DictZip(dz) => dz.read(offset, size),
        }
    }
}

/// Random access into a dictzip file.
struct DictZip {
    file: Mutex<File>,
    chunk_len: usize,
    /// Byte offset of each compressed chunk in the file, plus the end of
    /// the last one.
    bounds: Vec<u64>,
    cache: Mutex<VecDeque<(usize, Arc<Vec<u8>>)>>,
}

impl DictZip {
    /// Read the gzip header. `Ok(None)` if it is plain gzip (no `RA`
    /// subfield) and the caller should inflate the whole file instead.
    fn open(mut file: File, len: u64, path: &Path) -> io::Result<Option<DictZip>> {
        // The header is at most 10 + 2 + 65535 bytes plus file name and
        // comment; read a generous slab of it.
        file.seek(SeekFrom::Start(0))?;
        let mut head = Vec::new();
        (&mut file).take(256 * 1024).read_to_end(&mut head)?;
        let bad = |what: &str| invalid(path, format!("bad dictzip header: {what}"));
        if head.len() < 10 || head[0] != 0x1f || head[1] != 0x8b || head[2] != 8 {
            return Err(bad("not gzip/deflate"));
        }
        let flags = head[3];
        let mut pos = 10;
        let mut chunks: Option<(usize, Vec<u16>)> = None;
        if flags & 0x04 != 0 {
            let xlen = le_u16(&head, pos).ok_or_else(|| bad("truncated FEXTRA"))? as usize;
            pos += 2;
            let extra = head.get(pos..pos + xlen).ok_or_else(|| bad("truncated FEXTRA"))?;
            pos += xlen;
            let mut sub = 0;
            while sub + 4 <= extra.len() {
                let (si1, si2) = (extra[sub], extra[sub + 1]);
                let sublen = le_u16(extra, sub + 2).unwrap_or(0) as usize;
                let data = extra.get(sub + 4..sub + 4 + sublen).ok_or_else(|| bad("truncated subfield"))?;
                if si1 == b'R' && si2 == b'A' {
                    let ver = le_u16(data, 0).ok_or_else(|| bad("short RA field"))?;
                    let chlen = le_u16(data, 2).ok_or_else(|| bad("short RA field"))? as usize;
                    let count = le_u16(data, 4).ok_or_else(|| bad("short RA field"))? as usize;
                    if ver != 1 || chlen == 0 {
                        return Err(bad("unsupported RA version"));
                    }
                    let sizes: Option<Vec<u16>> = (0..count).map(|i| le_u16(data, 6 + 2 * i)).collect();
                    chunks = Some((chlen, sizes.ok_or_else(|| bad("short RA field"))?));
                }
                sub += 4 + sublen;
            }
        }
        for flag in [0x08u8, 0x10] {
            // FNAME, FCOMMENT: NUL-terminated.
            if flags & flag != 0 {
                let nul = head.get(pos..).and_then(|r| r.iter().position(|&b| b == 0)).ok_or_else(|| bad("unterminated name"))?;
                pos += nul + 1;
            }
        }
        if flags & 0x02 != 0 {
            pos += 2; // FHCRC
        }
        let Some((chunk_len, sizes)) = chunks else { return Ok(None) };
        let mut bounds = Vec::with_capacity(sizes.len() + 1);
        let mut at = pos as u64;
        bounds.push(at);
        for s in sizes {
            at += s as u64;
            bounds.push(at);
        }
        if at > len {
            return Err(invalid(path, "dictzip chunks run past the end of the file"));
        }
        Ok(Some(DictZip { file: Mutex::new(file), chunk_len, bounds, cache: Mutex::new(VecDeque::new()) }))
    }

    fn chunk_count(&self) -> usize {
        self.bounds.len() - 1
    }

    fn chunk(&self, i: usize) -> io::Result<Arc<Vec<u8>>> {
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(at) = cache.iter().position(|(n, _)| *n == i) {
                let hit = cache.remove(at).expect("position is in range");
                let data = hit.1.clone();
                cache.push_front(hit);
                return Ok(data);
            }
        }
        let (start, end) = (self.bounds[i], self.bounds[i + 1]);
        let mut compressed = vec![0; (end - start) as usize];
        {
            let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
            f.seek(SeekFrom::Start(start))?;
            f.read_exact(&mut compressed)?;
        }
        let data = Arc::new(inflate_chunk(&compressed, self.chunk_len)?);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.push_front((i, data.clone()));
        cache.truncate(CHUNK_CACHE);
        Ok(data)
    }

    fn read(&self, offset: u64, size: usize) -> io::Result<Vec<u8>> {
        let eof = || io::Error::new(io::ErrorKind::UnexpectedEof, "article beyond end of .dict.dz");
        if size == 0 {
            return Ok(Vec::new());
        }
        let chlen = self.chunk_len as u64;
        let first = (offset / chlen) as usize;
        let last = (offset.checked_add(size as u64 - 1).ok_or_else(eof)? / chlen) as usize;
        if last >= self.chunk_count() {
            return Err(eof());
        }
        let mut out = Vec::with_capacity(size);
        for i in first..=last {
            let chunk = self.chunk(i)?;
            let chunk_start = i as u64 * chlen;
            let from = offset.saturating_sub(chunk_start) as usize;
            let want = size - out.len();
            let to = (from + want).min(chunk.len());
            if from >= to {
                return Err(eof());
            }
            out.extend_from_slice(&chunk[from..to]);
        }
        if out.len() != size {
            return Err(eof());
        }
        Ok(out)
    }
}

/// Inflate one dictzip chunk: a raw deflate segment that ends on a full
/// flush, so it decodes on its own.
fn inflate_chunk(input: &[u8], chunk_len: usize) -> io::Result<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut d = Decompress::new(false);
    let mut out = Vec::with_capacity(chunk_len);
    loop {
        let consumed = d.total_in() as usize;
        let produced = out.len();
        let status = d
            .decompress_vec(&input[consumed.min(input.len())..], &mut out, FlushDecompress::Sync)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("dictzip chunk: {e}")))?;
        let progress = d.total_in() as usize != consumed || out.len() != produced;
        if status == Status::StreamEnd || d.total_in() as usize >= input.len() || out.len() >= chunk_len {
            break;
        }
        if !progress {
            if out.len() == out.capacity() && out.len() < chunk_len {
                out.reserve(chunk_len - out.len());
                continue;
            }
            break;
        }
    }
    Ok(out)
}

/// An opened StarDict dictionary.
pub struct StarDict {
    name: String,
    lang_pair: Option<(LangId, LangId)>,
    ifo: Ifo,
    idx: WordIndex,
    syn: Option<WordIndex>,
    dict: DictData,
}

/// `base` + each suffix in turn; the first that exists.
fn sibling(base: &Path, suffixes: &[&str]) -> Option<PathBuf> {
    suffixes
        .iter()
        .map(|s| {
            let mut p = base.as_os_str().to_owned();
            p.push(s);
            PathBuf::from(p)
        })
        .find(|p| p.is_file())
}

impl StarDict {
    /// Open from the `.ifo` path; the other files are found next to it.
    pub fn open(ifo_path: &Path) -> io::Result<StarDict> {
        let text = std::fs::read(ifo_path)?;
        let ifo = Ifo::parse(&String::from_utf8_lossy(&text), ifo_path)?;
        let base = ifo_path.with_extension("");
        let not_found = |what: &str| {
            io::Error::new(io::ErrorKind::NotFound, format!("{}: {what} not found", ifo_path.display()))
        };
        let idx_path = sibling(&base, &[".idx", ".idx.gz", ".IDX"]).ok_or_else(|| not_found(".idx"))?;
        let dict_path = sibling(&base, &[".dict", ".dict.dz", ".DICT", ".DICT.DZ"]).ok_or_else(|| not_found(".dict"))?;

        let idx_bytes = read_maybe_gzip(&idx_path)?;
        if ifo.idxfilesize != 0 && ifo.idxfilesize != idx_bytes.len() as u64 {
            tracing::warn!(
                "{}: idxfilesize says {} bytes, file has {}",
                idx_path.display(),
                ifo.idxfilesize,
                idx_bytes.len()
            );
        }
        let offset_len = if ifo.idxoffsetbits == 64 { 8 } else { 4 };
        let idx = WordIndex::parse(idx_bytes, offset_len + 4, &idx_path)?;
        if ifo.wordcount != 0 && ifo.wordcount != idx.len() {
            tracing::warn!("{}: wordcount says {}, index has {}", ifo_path.display(), ifo.wordcount, idx.len());
        }
        let syn = match sibling(&base, &[".syn", ".syn.gz"]) {
            Some(p) => match WordIndex::parse(read_maybe_gzip(&p)?, 4, &p) {
                Ok(s) => Some(s),
                Err(e) => {
                    // Synonyms are a convenience; a broken .syn should not
                    // cost the whole dictionary.
                    tracing::warn!("ignoring synonyms: {e}");
                    None
                }
            },
            None => None,
        };
        let dict = DictData::open(&dict_path)?;
        let stem = base.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = if ifo.bookname.trim().is_empty() { stem.clone() } else { ifo.bookname.trim().to_owned() };
        let lang_pair = ifo.lang_pair.or_else(|| guess_lang_pair(&name)).or_else(|| guess_lang_pair(&stem));
        Ok(StarDict { name, lang_pair, ifo, idx, syn, dict })
    }

    pub fn ifo(&self) -> &Ifo {
        &self.ifo
    }

    fn entry_location(&self, i: usize) -> (u64, usize) {
        let p = self.idx.payload(i);
        if self.ifo.idxoffsetbits == 64 {
            (be_u64(&p[..8]), be_u32(&p[8..12]) as usize)
        } else {
            (be_u32(&p[..4]) as u64, be_u32(&p[4..8]) as usize)
        }
    }

    fn read_entry(&self, i: usize) -> io::Result<RawEntry> {
        let (offset, size) = self.entry_location(i);
        let data = self.dict.read(offset, size)?;
        let fields = parse_fields(&data, self.ifo.sametypesequence.as_deref().map(str::as_bytes));
        let mut parts: Vec<Body> = fields.into_iter().filter_map(|(t, d)| field_body(t, d)).collect();
        let body = match parts.len() {
            0 => Body::Plain(String::new()),
            1 => parts.remove(0),
            _ => Body::Parts(parts),
        };
        Ok(RawEntry { headword: String::from_utf8_lossy(self.idx.word(i)).into_owned(), body })
    }

    /// Spellings to try for `word`: the ASCII-folded binary search cannot
    /// fold "Привет" onto "привет", so the common case forms are tried too.
    fn variants(word: &str) -> Vec<String> {
        let w = word.trim();
        let mut v = vec![w.to_owned(), w.to_lowercase(), w.to_uppercase()];
        let mut chars = w.chars();
        if let Some(first) = chars.next() {
            v.push(first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect());
        }
        v.retain(|s| !s.is_empty());
        let mut seen = Vec::new();
        v.retain(|s| {
            let fresh = !seen.contains(s);
            if fresh {
                seen.push(s.clone());
            }
            fresh
        });
        v
    }

    /// Ranked idx record numbers for a lookup.
    fn find(&self, word: &str, exact: bool, cap: usize) -> Vec<usize> {
        let query = normalize_key(word);
        if query.is_empty() {
            return Vec::new();
        }
        let scan = if exact { 64 } else { cap * 8 };
        // (is_exact, length in chars, record)
        let mut hits: Vec<(bool, usize, usize)> = Vec::new();
        let consider = |w: &[u8], record: usize, hits: &mut Vec<(bool, usize, usize)>| {
            let key = normalize_key(&String::from_utf8_lossy(w));
            if key == query {
                hits.push((true, key.chars().count(), record));
            } else if !exact && key.starts_with(&query) {
                hits.push((false, key.chars().count(), record));
            }
        };
        for variant in Self::variants(word) {
            let key = variant.as_bytes();
            for i in self.idx.prefixed(key, exact, scan) {
                consider(self.idx.word(i), i, &mut hits);
            }
            if let Some(syn) = &self.syn {
                for s in syn.prefixed(key, exact, scan) {
                    let target = be_u32(syn.payload(s)) as usize;
                    if target < self.idx.len() {
                        consider(syn.word(s), target, &mut hits);
                    }
                }
            }
        }
        hits.sort_by_key(|&(is_exact, len, record)| (!is_exact, len, record));
        let mut out: Vec<usize> = Vec::new();
        for (_, _, record) in hits {
            if !out.contains(&record) {
                out.push(record);
                if out.len() >= cap {
                    break;
                }
            }
        }
        out
    }
}

/// Split an article into `(type, data)` fields, following
/// `sametypesequence` when the dictionary has one. Lowercase types are
/// text (NUL-terminated unless last), uppercase types are binary (a 4-byte
/// big-endian size unless last). Malformed data ends the list early.
fn parse_fields<'a>(data: &'a [u8], sts: Option<&[u8]>) -> Vec<(u8, &'a [u8])> {
    let mut out = Vec::new();
    let mut rest = data;
    let take = |t: u8, last: bool, rest: &mut &'a [u8]| -> Option<&'a [u8]> {
        if last {
            let field = *rest;
            *rest = &[];
            return Some(field);
        }
        if t.is_ascii_uppercase() {
            if rest.len() < 4 {
                return None;
            }
            let size = (be_u32(rest) as usize).min(rest.len() - 4);
            let field = &rest[4..4 + size];
            *rest = &rest[4 + size..];
            Some(field)
        } else {
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            let field = &rest[..end];
            *rest = rest.get(end + 1..).unwrap_or(&[]);
            Some(field)
        }
    };
    match sts {
        Some(seq) if !seq.is_empty() => {
            for (k, &t) in seq.iter().enumerate() {
                match take(t, k + 1 == seq.len(), &mut rest) {
                    Some(f) => out.push((t, f)),
                    None => break,
                }
            }
        }
        _ => {
            while let Some((&t, tail)) = rest.split_first() {
                rest = tail;
                match take(t, false, &mut rest) {
                    Some(f) => out.push((t, f)),
                    None => break,
                }
            }
        }
    }
    out
}

/// What a field of a given type becomes. Binary fields (sounds, pictures)
/// and resource lists are dropped: there is nothing to show for them.
fn field_body(t: u8, data: &[u8]) -> Option<Body> {
    if t.is_ascii_uppercase() || t == b'r' {
        return None;
    }
    let mut end = data.len();
    while end > 0 && data[end - 1] == 0 {
        end -= 1;
    }
    let text = String::from_utf8_lossy(&data[..end]).into_owned();
    Some(match t {
        b'h' | b'g' | b'k' => Body::Html(text),
        b'x' => Body::Xdxf(text),
        b't' | b'y' => Body::Phonetic(text),
        // m (plain), l (locale text), w (wiki) and types we do not know.
        _ => Body::Plain(text),
    })
}

impl DictionarySource for StarDict {
    fn name(&self) -> &str {
        &self.name
    }

    fn lang_pair(&self) -> Option<(LangId, LangId)> {
        self.lang_pair
    }

    fn entry_count(&self) -> usize {
        self.idx.len()
    }

    fn lookup(&self, word: &str, exact: bool) -> Vec<RawEntry> {
        self.find(word, exact, MAX_PREFIX_RESULTS)
            .into_iter()
            .filter_map(|i| match self.read_entry(i) {
                Ok(e) => Some(e),
                Err(err) => {
                    tracing::warn!("{}: cannot read entry {i}: {err}", self.name);
                    None
                }
            })
            .collect()
    }

    fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        let query = normalize_key(prefix);
        if query.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        let indexes = std::iter::once(&self.idx).chain(self.syn.as_ref());
        for index in indexes {
            for variant in Self::variants(prefix) {
                for i in index.prefixed(variant.as_bytes(), false, limit * 4) {
                    let w = String::from_utf8_lossy(index.word(i)).into_owned();
                    if normalize_key(&w).starts_with(&query) && !out.contains(&w) {
                        out.push(w);
                    }
                }
            }
        }
        out.sort_by(|a, b| stardict_cmp(a.as_bytes(), b.as_bytes()));
        out.truncate(limit);
        out
    }
}

/// Fixture writers for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use std::io::Write;

    pub(crate) struct Fixture<'a> {
        pub(crate) bookname: &'a str,
        pub(crate) entries: Vec<(&'a str, Vec<u8>)>,
        pub(crate) sametypesequence: Option<&'a str>,
        pub(crate) bits64: bool,
        pub(crate) dictzip_chunk: Option<usize>,
        pub(crate) synonyms: Vec<(&'a str, &'a str)>,
        pub(crate) gzip_idx: bool,
        pub(crate) sort: bool,
    }

    impl<'a> Fixture<'a> {
        pub(crate) fn new(bookname: &'a str, entries: Vec<(&'a str, Vec<u8>)>) -> Self {
            Fixture {
                bookname,
                entries,
                sametypesequence: Some("m"),
                bits64: false,
                dictzip_chunk: None,
                synonyms: Vec::new(),
                gzip_idx: false,
                sort: true,
            }
        }

        /// Write `<dir>/<base>.ifo` and friends; returns the `.ifo` path.
        pub(crate) fn write(mut self, dir: &Path, base: &str) -> PathBuf {
            if self.sort {
                self.entries.sort_by(|a, b| stardict_cmp(a.0.as_bytes(), b.0.as_bytes()));
            }
            let mut dict = Vec::new();
            let mut idx = Vec::new();
            for (word, data) in &self.entries {
                idx.extend_from_slice(word.as_bytes());
                idx.push(0);
                if self.bits64 {
                    idx.extend_from_slice(&(dict.len() as u64).to_be_bytes());
                } else {
                    idx.extend_from_slice(&(dict.len() as u32).to_be_bytes());
                }
                idx.extend_from_slice(&(data.len() as u32).to_be_bytes());
                dict.extend_from_slice(data);
            }
            let mut ifo = format!(
                "{IFO_MAGIC}\nversion={}\nbookname={}\nwordcount={}\nidxfilesize={}\n",
                if self.bits64 { "3.0.0" } else { "2.4.2" },
                self.bookname,
                self.entries.len(),
                idx.len()
            );
            if let Some(sts) = self.sametypesequence {
                ifo.push_str(&format!("sametypesequence={sts}\n"));
            }
            if self.bits64 {
                ifo.push_str("idxoffsetbits=64\n");
            }
            if !self.synonyms.is_empty() {
                let mut syn: Vec<(&str, u32)> = self
                    .synonyms
                    .iter()
                    .map(|(s, target)| {
                        let at = self.entries.iter().position(|(w, _)| w == target).expect("synonym target");
                        (*s, at as u32)
                    })
                    .collect();
                syn.sort_by(|a, b| stardict_cmp(a.0.as_bytes(), b.0.as_bytes()));
                let mut bytes = Vec::new();
                for (s, at) in &syn {
                    bytes.extend_from_slice(s.as_bytes());
                    bytes.push(0);
                    bytes.extend_from_slice(&at.to_be_bytes());
                }
                std::fs::write(dir.join(format!("{base}.syn")), bytes).unwrap();
                ifo.push_str(&format!("synwordcount={}\n", syn.len()));
            }
            std::fs::write(dir.join(format!("{base}.ifo")), ifo).unwrap();
            if self.gzip_idx {
                std::fs::write(dir.join(format!("{base}.idx.gz")), crate::source::testutil::gzip(&idx)).unwrap();
            } else {
                std::fs::write(dir.join(format!("{base}.idx")), idx).unwrap();
            }
            match self.dictzip_chunk {
                Some(chunk) => std::fs::write(dir.join(format!("{base}.dict.dz")), dictzip(&dict, chunk)).unwrap(),
                None => std::fs::write(dir.join(format!("{base}.dict")), dict).unwrap(),
            }
            dir.join(format!("{base}.ifo"))
        }
    }

    /// A minimal dictzip writer: raw deflate with a full flush every
    /// `chunk_len` input bytes, chunk sizes in an `RA` extra subfield.
    pub(crate) fn dictzip(data: &[u8], chunk_len: usize) -> Vec<u8> {
        use flate2::{Compress, Compression, FlushCompress};
        let mut c = Compress::new(Compression::best(), false);
        let pieces: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(chunk_len).collect() };
        let mut compressed: Vec<Vec<u8>> = Vec::new();
        for (i, piece) in pieces.iter().enumerate() {
            let flush = if i + 1 == pieces.len() { FlushCompress::Finish } else { FlushCompress::Full };
            let mut out = Vec::new();
            let start_in = c.total_in();
            loop {
                let consumed = (c.total_in() - start_in) as usize;
                out.reserve(piece.len() + 1024);
                let status = c.compress_vec(&piece[consumed..], &mut out, flush).unwrap();
                let done_input = (c.total_in() - start_in) as usize == piece.len();
                // A flush is complete once a call leaves output space unused.
                let finished = match flush {
                    FlushCompress::Finish => status == flate2::Status::StreamEnd,
                    _ => done_input && out.len() < out.capacity(),
                };
                if finished {
                    break;
                }
            }
            compressed.push(out);
        }
        let mut gz = vec![0x1f, 0x8b, 8, 0x04 | 0x08, 0, 0, 0, 0, 2, 3];
        let ra_len = 6 + 2 * compressed.len();
        gz.extend_from_slice(&((4 + ra_len) as u16).to_le_bytes());
        gz.extend_from_slice(b"RA");
        gz.extend_from_slice(&(ra_len as u16).to_le_bytes());
        gz.extend_from_slice(&1u16.to_le_bytes());
        gz.extend_from_slice(&(chunk_len as u16).to_le_bytes());
        gz.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
        for chunk in &compressed {
            gz.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        }
        gz.extend_from_slice(b"test.dict\0");
        for chunk in &compressed {
            gz.extend_from_slice(chunk);
        }
        let mut crc = flate2::Crc::new();
        crc.update(data);
        gz.write_all(&crc.sum().to_le_bytes()).unwrap();
        gz.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        gz
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{dictzip, Fixture};
    use super::*;
    use crate::source::testutil::TempDir;

    fn plain(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    fn sample() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("apple", plain("яблоко")),
            ("Apple", plain("Apple Inc.")),
            ("run", plain("бежать\nуправлять")),
            ("runner", plain("бегун")),
            ("running", plain("бег")),
            ("rune", plain("руна")),
            ("привет", plain("hello")),
            ("zebra", plain("зебра")),
        ]
    }

    fn texts(entries: &[RawEntry]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|e| {
                let body = match &e.body {
                    Body::Plain(t) => t.clone(),
                    other => format!("{other:?}"),
                };
                (e.headword.clone(), body)
            })
            .collect()
    }

    #[test]
    fn ifo_is_parsed() {
        let ifo = Ifo::parse(
            "StarDict's dict ifo file\nversion=3.0.0\nbookname=Big (En-Ru)\nwordcount=10\nidxoffsetbits=64\nsametypesequence=tm\nlang=en-ru\n",
            Path::new("x.ifo"),
        )
        .unwrap();
        assert_eq!(ifo.wordcount, 10);
        assert_eq!(ifo.idxoffsetbits, 64);
        assert_eq!(ifo.sametypesequence.as_deref(), Some("tm"));
        assert_eq!(ifo.lang_pair, Some((LangId::ENGLISH, LangId::RUSSIAN)));
        assert!(Ifo::parse("garbage\nversion=1", Path::new("x.ifo")).is_err());
        assert!(Ifo::parse("StarDict's dict ifo file\nidxoffsetbits=48", Path::new("x.ifo")).is_err());
    }

    #[test]
    fn exact_lookup_is_case_insensitive() {
        let dir = TempDir::new("sd-exact");
        let ifo = Fixture::new("Test (En-Ru)", sample()).write(dir.path(), "test");
        let d = StarDict::open(&ifo).unwrap();
        assert_eq!(d.name(), "Test (En-Ru)");
        assert_eq!(d.entry_count(), 8);
        assert_eq!(d.lang_pair(), Some((LangId::ENGLISH, LangId::RUSSIAN)));

        let hits = texts(&d.lookup("APPLE", true));
        assert_eq!(hits.len(), 2);
        assert!(hits.contains(&("apple".into(), "яблоко".into())));
        assert!(hits.contains(&("Apple".into(), "Apple Inc.".into())));

        // Non-ASCII case folding goes through the spelling variants.
        assert_eq!(texts(&d.lookup("Привет", true)), vec![("привет".into(), "hello".into())]);
        assert!(d.lookup("app", true).is_empty());
        assert!(d.lookup("", false).is_empty());
        assert!(d.lookup("nothing", false).is_empty());
    }

    #[test]
    fn prefix_lookup_ranks_exact_then_shorter() {
        let dir = TempDir::new("sd-prefix");
        let ifo = Fixture::new("t", sample()).write(dir.path(), "t");
        let d = StarDict::open(&ifo).unwrap();
        let words: Vec<String> = d.lookup("run", false).into_iter().map(|e| e.headword).collect();
        assert_eq!(words, vec!["run", "rune", "runner", "running"]);
        assert_eq!(d.suggest("RU", 10), vec!["run", "rune", "runner", "running"]);
        assert_eq!(d.suggest("ru", 2).len(), 2);
    }

    #[test]
    fn sametypesequence_with_several_fields() {
        let dir = TempDir::new("sd-sts");
        let mut data = b"kat".to_vec();
        data.push(0);
        data.extend_from_slice("кошка".as_bytes());
        let mut fx = Fixture::new("t", vec![("cat", data)]);
        fx.sametypesequence = Some("tm");
        let d = StarDict::open(&fx.write(dir.path(), "t")).unwrap();
        let e = &d.lookup("cat", true)[0];
        assert_eq!(e.body, Body::Parts(vec![Body::Phonetic("kat".into()), Body::Plain("кошка".into())]));
        let a = e.to_article();
        assert_eq!(a.pronunciations[0].symbol, "kat");
        assert_eq!(a.explanations[0].definitions, vec!["кошка"]);
    }

    #[test]
    fn typed_entries_without_sametypesequence() {
        let dir = TempDir::new("sd-typed");
        let mut data = Vec::new();
        data.push(b'm');
        data.extend_from_slice("собака".as_bytes());
        data.push(0);
        data.push(b'W'); // a sound: size-prefixed, dropped
        data.extend_from_slice(&3u32.to_be_bytes());
        data.extend_from_slice(&[1, 2, 3]);
        data.push(b'h');
        data.extend_from_slice(b"<b>dog</b>");
        data.push(0);
        let mut fx = Fixture::new("t", vec![("dog", data)]);
        fx.sametypesequence = None;
        let d = StarDict::open(&fx.write(dir.path(), "t")).unwrap();
        let e = &d.lookup("dog", true)[0];
        assert_eq!(e.body, Body::Parts(vec![Body::Plain("собака".into()), Body::Html("<b>dog</b>".into())]));
    }

    #[test]
    fn sixty_four_bit_offsets() {
        let dir = TempDir::new("sd-64");
        let mut fx = Fixture::new("t", sample());
        fx.bits64 = true;
        let d = StarDict::open(&fx.write(dir.path(), "t")).unwrap();
        assert_eq!(d.ifo().idxoffsetbits, 64);
        assert_eq!(texts(&d.lookup("zebra", true)), vec![("zebra".into(), "зебра".into())]);
        assert_eq!(texts(&d.lookup("run", true)), vec![("run".into(), "бежать\nуправлять".into())]);
    }

    #[test]
    fn dictzip_random_access_reads_across_chunks() {
        let dir = TempDir::new("sd-dz");
        // Enough entries that articles straddle 50-byte chunks.
        let words: Vec<String> = (0..200).map(|i| format!("word{i:03}")).collect();
        let entries: Vec<(&str, Vec<u8>)> =
            words.iter().map(|w| (w.as_str(), format!("definition of {w}, with some padding text").into_bytes())).collect();
        let mut fx = Fixture::new("dz", entries);
        fx.dictzip_chunk = Some(50);
        let ifo = fx.write(dir.path(), "dz");
        assert!(dir.path().join("dz.dict.dz").exists());
        let d = StarDict::open(&ifo).unwrap();
        assert!(matches!(d.dict, DictData::DictZip(_)));
        for w in ["word000", "word001", "word123", "word199"] {
            let hits = texts(&d.lookup(w, true));
            assert_eq!(hits, vec![(w.to_owned(), format!("definition of {w}, with some padding text"))]);
        }
        // The whole file is also valid gzip.
        let mut all = Vec::new();
        flate2::read::GzDecoder::new(&std::fs::read(dir.path().join("dz.dict.dz")).unwrap()[..])
            .read_to_end(&mut all)
            .unwrap();
        assert!(String::from_utf8(all).unwrap().starts_with("definition of word000"));
    }

    #[test]
    fn large_index_opens_compactly_and_searches() {
        let dir = TempDir::new("sd-large");
        let words: Vec<String> = (0..100_000).map(|i| format!("w{:06}", (i * 7919) % 100_000)).collect();
        let entries: Vec<(&str, Vec<u8>)> = words.iter().map(|w| (w.as_str(), w.as_bytes().to_vec())).collect();
        let mut fx = Fixture::new("big", entries);
        fx.dictzip_chunk = Some(4096);
        let d = StarDict::open(&fx.write(dir.path(), "big")).unwrap();
        assert_eq!(d.entry_count(), 100_000);
        // Sorted input: no permutation table, just one u32 per entry.
        assert!(d.idx.order.is_none());
        assert_eq!(d.idx.starts.len(), 100_000);
        for w in ["w000000", "w050000", "w099999", "W012345"] {
            let hits = d.lookup(w, true);
            assert_eq!(hits.len(), 1, "{w}");
            assert_eq!(hits[0].body, Body::Plain(w.to_lowercase()));
        }
        assert_eq!(d.lookup("w0123", false).len(), MAX_PREFIX_RESULTS);
    }

    #[test]
    fn plain_gzip_dict_falls_back_to_inflating_whole() {
        let dir = TempDir::new("sd-gz");
        let ifo = Fixture::new("t", sample()).write(dir.path(), "t");
        let dict = std::fs::read(dir.path().join("t.dict")).unwrap();
        std::fs::remove_file(dir.path().join("t.dict")).unwrap();
        std::fs::write(dir.path().join("t.dict.dz"), crate::source::testutil::gzip(&dict)).unwrap();
        let d = StarDict::open(&ifo).unwrap();
        assert!(matches!(d.dict, DictData::Memory(_)));
        assert_eq!(texts(&d.lookup("rune", true)), vec![("rune".into(), "руна".into())]);
    }

    #[test]
    fn synonyms_and_gzipped_index() {
        let dir = TempDir::new("sd-syn");
        let mut fx = Fixture::new("t", sample());
        fx.synonyms = vec![("jog", "run"), ("Malus", "apple")];
        fx.gzip_idx = true;
        let d = StarDict::open(&fx.write(dir.path(), "t")).unwrap();
        assert_eq!(texts(&d.lookup("jog", true)), vec![("run".into(), "бежать\nуправлять".into())]);
        assert_eq!(texts(&d.lookup("malus", true)), vec![("apple".into(), "яблоко".into())]);
        assert!(d.suggest("j", 5).contains(&"jog".to_owned()));
    }

    #[test]
    fn unsorted_index_still_finds_everything() {
        let dir = TempDir::new("sd-unsorted");
        let mut fx = Fixture::new("t", vec![("zebra", plain("z")), ("apple", plain("a")), ("mango", plain("m"))]);
        fx.sort = false;
        let d = StarDict::open(&fx.write(dir.path(), "t")).unwrap();
        for w in ["zebra", "apple", "mango"] {
            assert_eq!(d.lookup(w, true).len(), 1, "{w}");
        }
    }

    #[test]
    fn corrupted_files_are_errors_not_panics() {
        let dir = TempDir::new("sd-bad");
        let ifo = Fixture::new("t", sample()).write(dir.path(), "t");

        // Truncated index.
        let idx = std::fs::read(dir.path().join("t.idx")).unwrap();
        std::fs::write(dir.path().join("t.idx"), &idx[..idx.len() - 3]).unwrap();
        assert!(StarDict::open(&ifo).is_err());

        // Garbage index: an error or a dictionary, never a panic.
        let mut state = 7u32;
        for len in [1usize, 7, 64, 1000] {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 24) as u8
                })
                .collect();
            std::fs::write(dir.path().join("t.idx"), junk).unwrap();
            if let Ok(d) = StarDict::open(&ifo) {
                let _ = d.lookup("a", false);
                let _ = d.suggest("a", 5);
            }
        }

        // Index pointing past the end of the .dict: the entry is skipped.
        std::fs::write(dir.path().join("t.idx"), &idx).unwrap();
        std::fs::write(dir.path().join("t.dict"), b"short").unwrap();
        let d = StarDict::open(&ifo).unwrap();
        assert!(d.lookup("zebra", true).is_empty());

        // Missing .dict.
        std::fs::remove_file(dir.path().join("t.dict")).unwrap();
        let err = StarDict::open(&ifo).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        // Bad magic.
        std::fs::write(&ifo, "not a dictionary").unwrap();
        assert!(StarDict::open(&ifo).is_err());
    }

    #[test]
    fn corrupted_dictzip_chunks_do_not_panic() {
        let dir = TempDir::new("sd-dzbad");
        let mut fx = Fixture::new("t", sample());
        fx.dictzip_chunk = Some(16);
        let ifo = fx.write(dir.path(), "t");
        let path = dir.path().join("t.dict.dz");
        let good = std::fs::read(&path).unwrap();

        let mut flipped = good.clone();
        let n = flipped.len();
        for b in &mut flipped[n / 2..n - 8] {
            *b ^= 0xA5;
        }
        std::fs::write(&path, &flipped).unwrap();
        if let Ok(d) = StarDict::open(&ifo) {
            for w in ["apple", "run", "zebra", "привет"] {
                let _ = d.lookup(w, false);
            }
        }

        // Truncated: the RA table promises more than the file holds.
        std::fs::write(&path, &good[..good.len() / 2]).unwrap();
        assert!(StarDict::open(&ifo).is_err());

        // A header cut inside FEXTRA.
        std::fs::write(&path, &good[..14]).unwrap();
        assert!(StarDict::open(&ifo).is_err());
    }

    #[test]
    fn dictzip_writer_round_trips_through_reader() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let dir = TempDir::new("sd-dzrt");
        let path = dir.write("x.dict.dz", &dictzip(&data, 300));
        let d = DictData::open(&path).unwrap();
        assert_eq!(d.read(0, 10).unwrap(), &data[..10]);
        assert_eq!(d.read(295, 20).unwrap(), &data[295..315]);
        assert_eq!(d.read(4990, 10).unwrap(), &data[4990..]);
        assert_eq!(d.read(0, 5000).unwrap(), data);
        assert!(d.read(4995, 10).is_err());
        assert!(d.read(u64::MAX - 2, 10).is_err());
    }

    #[test]
    fn field_parsing_survives_malformed_data() {
        assert_eq!(parse_fields(b"W\x00\x00", None), vec![]);
        assert_eq!(parse_fields(b"mabc", None), vec![(b'm', &b"abc"[..])]);
        assert_eq!(parse_fields(b"W\xff\xff\xff\xffxy", None), vec![(b'W', &b"xy"[..])]);
        assert_eq!(parse_fields(b"abc", Some(b"tm")), vec![(b't', &b"abc"[..]), (b'm', &b""[..])]);
        assert_eq!(parse_fields(b"", Some(b"m")), vec![(b'm', &b""[..])]);
    }
}
