//! The domain model of Glossa.
//!
//! Nothing here does I/O beyond reading and writing its own files, knows about
//! a GUI toolkit, or talks to the network. It is the vocabulary every other
//! crate shares: languages, capabilities, requests and results, settings,
//! themes, history, interface strings, and the importer for a classic
//! installation.

pub mod capability;
pub mod color;
pub mod config;
pub mod dict;
pub mod error;
pub mod error_text;
pub mod fsio;
pub mod history;
pub mod hotkey;
pub mod jsonc;
pub mod lang;
pub mod legacy;
pub mod locale;
pub mod ocr;
pub mod paths;
pub mod preset;
pub mod secrets;
pub mod text;
pub mod theme;
pub mod translate;

pub use capability::Capabilities;
pub use color::Color;
pub use config::{Backdrop, Config};
pub use dict::{Article, Audio, DictRequest, SpeakRequest};
pub use error::{Error, Result};
pub use history::History;
pub use hotkey::{Action, Hotkey};
pub use lang::{LangId, LangPair};
pub use locale::Locale;
pub use ocr::{OcrRequest, OcrResult};
pub use paths::Paths;
pub use preset::Preset;
pub use theme::Theme;
pub use translate::{Detection, ProviderInfo, TranslateRequest, Translation};

/// Where to support the project: the author's Boosty page. Shown in the
/// about box, the menus and the command-line help.
pub const SUPPORT_URL: &str = "https://boosty.to/pinus";

/// The program's version, for the about box and the control API.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit the program was built from (`1a2b3c4d`, with `-dirty` when
/// tracked files had uncommitted changes), or empty when that is unknown (a
/// build from a source archive without `GLOSSA_COMMIT`). See `build.rs`.
pub const COMMIT: &str = env!("GLOSSA_COMMIT");

/// The version with its commit, `0.1.0 (1a2b3c4d)`, or the version alone.
pub fn long_version() -> String {
    if COMMIT.is_empty() {
        VERSION.to_owned()
    } else {
        format!("{VERSION} ({COMMIT})")
    }
}

/// The version as shown to the user (the about box): the commit without
/// the `-dirty` mark. That mark says the build had uncommitted changes,
/// which matters in a log or a bug report ([`long_version`]) and reads as
/// a fault to everyone else.
pub fn display_version() -> String {
    match COMMIT.strip_suffix("-dirty").unwrap_or(COMMIT) {
        "" => VERSION.to_owned(),
        commit => format!("{VERSION} ({commit})"),
    }
}

/// Seconds since the Unix epoch, for timestamps in history and logs.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
