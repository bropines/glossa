//! Providers and modules.
//!
//! * [`provider`] — the [`Provider`] trait every translation, dictionary,
//!   speech and OCR source implements, built-in or loaded.
//! * [`manifest`] — `module.toml`.
//! * [`registry`] — finding modules on disk and keeping the provider list.
//! * [`protocol`] / [`host_api`] — the module protocol, shared by the Python
//!   and native runtimes.
//! * [`runtime`] — the runtimes: JavaScript (three dialects), Python, native.

pub mod host_api;
pub mod manifest;
pub mod protocol;
pub mod provider;
pub mod registry;
pub mod runtime;

pub use glossa_module_sdk::PROTOCOL_VERSION;
pub use manifest::{Manifest, Runtime};
pub use provider::{CancelToken, Host, OptionKind, OptionSpec, OptionValues, Provider};
pub use registry::{Entry, LoadReport, Origin, Registry, RuntimeContext};
