//! Remembering, for a while, which endpoints are rate-limiting us: the
//! cooling-off periods of [`glossa_modules::backoff`], which modules share.

pub(crate) use glossa_modules::backoff::{ceil_secs, Backoff};
