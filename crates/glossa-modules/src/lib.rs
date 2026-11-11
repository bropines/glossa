//! Providers and modules.
//!
//! * [`provider`] — the [`Provider`] trait every translation, dictionary,
//!   speech and OCR source implements, built-in or loaded.
//! * [`manifest`] — `module.toml`.
//! * [`actions`] — `[[actions]]`: buttons a module's settings page can run.
//! * [`backoff`] — leaving a rate-limiting endpoint alone for a while.
//! * [`assets`] — files a module declares (models, dictionaries), downloaded
//!   and verified on demand.
//! * [`python_env`] — the Python a Python module runs with: a system one, or
//!   a managed build Glossa downloads, and per-module environments made by `uv`.
//! * [`settings`] — a provider's settings page as data: layout, conditions,
//!   validation and a module's translations.
//! * [`registry`] — finding modules on disk and keeping the provider list.
//! * [`protocol`] / [`host_api`] — the module protocol, shared by the stdio
//!   (Python, Java, any program) and native runtimes.
//! * [`runtime`] — the runtimes: JavaScript (three dialects), Python, Java
//!   and generic processes over one stdio transport, native.
//! * [`image_ops`] — the image API modules reach through `image.*` host calls
//!   (the `image` feature).
//! * [`proc_tree`] — stopping a module's process with the processes it
//!   started.
//! * [`trust`] — what a module installed by hand may do, and whether the
//!   user approved it.
//! * [`log_tap`] — what one module logs, for the module editor.
//! * [`child`] — running a helper program with a deadline, for providers
//!   built on one (Tesseract, the system voice).

pub mod actions;
pub mod assets;
pub mod backoff;
pub mod child;
pub mod host_api;
#[cfg(feature = "image")]
pub mod image_ops;
pub mod log_tap;
pub mod manifest;
pub mod panels;
#[cfg(any(feature = "python", feature = "process"))]
pub mod proc_tree;
pub mod protocol;
pub mod provider;
pub mod python_env;
pub mod registry;
pub mod runtime;
pub mod settings;
pub mod trust;

pub use assets::{AssetState, AssetStatus, ModuleAssets};
pub use glossa_module_sdk::{wire, PROTOCOL_VERSION};
pub use manifest::{Manifest, Runtime};
pub use provider::{CancelToken, Host, OptionKind, OptionSpec, OptionValues, Provider};
pub use registry::{Entry, LoadReport, Origin, Registry, RuntimeContext};
pub use settings::{FieldError, SettingsPage};
pub use trust::{TrustPolicy, TrustRequest};
