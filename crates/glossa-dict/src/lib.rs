//! Offline dictionaries (StarDict, XDXF, DSL, tab-separated) and HTML article
//! simplification.
//!
//! * [`source`] — the [`DictionarySource`] trait every format implements, and
//!   [`RawEntry`] → [`glossa_core::Article`] conversion.
//! * [`stardict`] — StarDict `.ifo`/`.idx`/`.dict(.dz)`/`.syn`, read lazily,
//!   with true dictzip random access.
//! * [`dsl`] — ABBYY Lingvo DSL (UTF-16/UTF-8/CP1251, optionally gzipped).
//! * [`xdxf`] — XDXF, visual format.
//! * [`tsv`] — tab-separated word lists.
//! * [`html`] — tag-soup HTML → rich-text [`html::Block`]s for a GUI with no
//!   web engine; used for online dictionary HTML too.
//! * [`offline`] — the `offline` [`glossa_modules::Provider`] that merges
//!   every opened dictionary into one article.
//!
//! Generated HTML links to other headwords with the `dict://<word>` scheme;
//! the UI turns a click on one into a new lookup.

pub mod dsl;
pub mod html;
pub mod offline;
pub mod source;
pub mod stardict;
pub mod tsv;
pub mod xdxf;

pub use dsl::Dsl;
pub use html::{to_blocks, to_plain_text, Block, Span};
pub use offline::{open, OfflineDictionaries};
pub use source::{Body, DictionarySource, RawEntry, MAX_PREFIX_RESULTS};
pub use stardict::StarDict;
pub use tsv::Tsv;
pub use xdxf::Xdxf;
