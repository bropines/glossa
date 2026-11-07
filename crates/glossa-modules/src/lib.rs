//! Providers and modules.
//!
//! * [`provider`] — the [`Provider`] trait every translation, dictionary,
//!   speech and OCR source implements, built-in or loaded.
//! * [`manifest`] — `module.toml`.
//! * [`assets`] — files a module declares (models, dictionaries), downloaded
//!   and verified on demand.
//! * [`registry`] — finding modules on disk and keeping the provider list.
//! * [`protocol`] / [`host_api`] — the module protocol, shared by the stdio
//!   (Python, Java, any program) and native runtimes.
//! * [`runtime`] — the runtimes: JavaScript (three dialects), Python, Java
//!   and generic processes over one stdio transport, native.
//! * [`image_ops`] — the image API modules reach through `image.*` host calls
//!   (the `image` feature).
//! * [`child`] — running a helper program with a deadline, for providers
//!   built on one (Tesseract, the system voice).

pub mod assets;
pub mod child;
pub mod host_api;
#[cfg(feature = "image")]
pub mod image_ops;
pub mod manifest;
pub mod protocol;
pub mod provider;
pub mod registry;
pub mod runtime;

pub use assets::{AssetState, AssetStatus, ModuleAssets};
pub use glossa_module_sdk::PROTOCOL_VERSION;
pub use manifest::{Manifest, Runtime};
pub use provider::{CancelToken, Host, OptionKind, OptionSpec, OptionValues, Provider};
pub use registry::{Entry, LoadReport, Origin, Registry, RuntimeContext};
