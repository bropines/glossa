//! The system keychain, for API keys and passwords the user chose to keep
//! out of the settings file (see `glossa_core::secrets`).
//!
//! One file per system: `secrets/win.rs` (Windows Credential Manager,
//! generic credentials named `Glossa/<account>`), `secrets/mac.rs` (the
//! login Keychain, generic passwords of the service `Glossa`) and
//! `secrets/linux.rs` (the freedesktop Secret Service over D-Bus: GNOME
//! Keyring, KWallet, KeePassXC).

use std::sync::Arc;

pub use glossa_core::secrets::SecretStore;

/// Glossa's name in the keychain.
pub const SERVICE: &str = "Glossa";

#[cfg(windows)]
#[path = "secrets/win.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "secrets/mac.rs"]
mod imp;
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "secrets/linux.rs"]
mod imp;

/// This system's keychain, or `None` where there is none to use (a Linux
/// session with no Secret Service running).
pub fn system_store() -> Option<Arc<dyn SecretStore>> {
    #[cfg(any(windows, unix))]
    {
        imp::store()
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

/// The keychain's name as the user knows it, whether or not it answers now:
/// for the settings and the welcome guide.
pub fn system_store_name() -> &'static str {
    if cfg!(windows) {
        "Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "Keychain"
    } else {
        "Secret Service (GNOME Keyring, KWallet)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes, reads and removes a throwaway entry in the real keychain:
    /// `cargo test -p glossa-platform keychain_round_trip -- --ignored`.
    #[test]
    #[ignore = "touches the user's keychain"]
    fn keychain_round_trip() {
        let store = system_store().expect("a keychain");
        let account = format!("test/round-trip-{}", std::process::id());
        store.set(&account, "first").unwrap();
        store.set(&account, "second ключ").unwrap();
        assert_eq!(store.get(&account).unwrap().as_deref(), Some("second ключ"));
        store.delete(&account).unwrap();
        assert_eq!(store.get(&account).unwrap(), None);
        store.delete(&account).expect("deleting what is not there is fine");
    }
}
