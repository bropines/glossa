//! The macOS login Keychain: one generic password per secret, service
//! `Glossa`, the account as its name. macOS ties the item to the app's code
//! signature; a build signed with a lasting identity reads it without asking.

use std::sync::Arc;

use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

use super::{SecretStore, SERVICE};

/// `errSecItemNotFound`.
const NOT_FOUND: i32 = -25300;

struct Keychain;

pub fn store() -> Option<Arc<dyn SecretStore>> {
    Some(Arc::new(Keychain))
}

impl SecretStore for Keychain {
    fn name(&self) -> &'static str {
        "Keychain"
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match get_generic_password(SERVICE, account) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(format!("Keychain: {e}")),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        set_generic_password(SERVICE, account, secret.as_bytes()).map_err(|e| format!("Keychain: {e}"))
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match delete_generic_password(SERVICE, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == NOT_FOUND => Ok(()),
            Err(e) => Err(format!("Keychain: {e}")),
        }
    }
}
