//! Keeping secrets (API keys, passwords, tokens) out of the settings file.
//!
//! With [`SecretStorage::System`] each secret goes to the system's keychain
//! (Windows Credential Manager, the macOS Keychain, the Secret Service on
//! Linux) and the settings file keeps only [`IN_STORE`] in its place. In
//! memory the settings always hold the real values, so nothing that reads
//! them needs to know where they are kept: secrets are fetched once, when
//! the store is attached ([`SecretSync::resolve`]), and moved out again
//! whenever the settings are written ([`SecretSync::prepare`]).
//!
//! The store itself is the platform's ([`SecretStore`]); this module only
//! decides what goes where, and is tested with a store in memory.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Config;

/// What the settings file says in place of a secret kept in the store.
pub const IN_STORE: &str = "@system-keychain";

/// Where secrets are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStorage {
    /// In the settings file, as plain text (it is readable by the user's
    /// account only).
    #[default]
    File,
    /// In the system's keychain.
    System,
}

/// A system keychain. Accounts are short paths such as
/// `service/deepl/api_key`; implementations file them under Glossa's name.
pub trait SecretStore: Send + Sync {
    /// The keychain's name as the user knows it ("Windows Credential
    /// Manager", "Keychain").
    fn name(&self) -> &'static str;
    /// The secret, or `None` when there is none for `account`.
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    /// Store `secret`, replacing any earlier one.
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// Forget it; not an error when there was none.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// A place in the settings that holds a secret.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SecretSlot {
    /// A service's option of the secret kind (`services.options.<id>.<key>`),
    /// AI endpoints' keys included.
    ServiceOption { service: String, key: String },
    /// `network.proxy.password`.
    ProxyPassword,
    /// `ocr.lens.token`.
    LensToken,
}

impl SecretSlot {
    /// The account the secret is kept under in the store.
    pub fn account(&self) -> String {
        match self {
            SecretSlot::ServiceOption { service, key } => format!("service/{service}/{key}"),
            SecretSlot::ProxyPassword => "network/proxy/password".into(),
            SecretSlot::LensToken => "ocr/lens/token".into(),
        }
    }

    /// The value in `cfg`, empty when unset.
    pub fn get(&self, cfg: &Config) -> String {
        match self {
            SecretSlot::ServiceOption { service, key } => cfg
                .services
                .options
                .get(service)
                .and_then(|o| o.get(key))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            SecretSlot::ProxyPassword => cfg.network.proxy.password.clone(),
            SecretSlot::LensToken => cfg.ocr.lens.token.clone(),
        }
    }

    /// Put `value` into `cfg`. A service option is only written where the
    /// service has options already, or `value` is not empty.
    pub fn set(&self, cfg: &mut Config, value: &str) {
        match self {
            SecretSlot::ServiceOption { service, key } => {
                if value.is_empty() && !cfg.services.options.get(service).is_some_and(|o| o.contains_key(key)) {
                    return;
                }
                cfg.services
                    .options
                    .entry(service.clone())
                    .or_default()
                    .insert(key.clone(), serde_json::Value::String(value.to_owned()));
            }
            SecretSlot::ProxyPassword => cfg.network.proxy.password = value.to_owned(),
            SecretSlot::LensToken => cfg.ocr.lens.token = value.to_owned(),
        }
    }

    /// The slots outside the services, which are always there.
    pub fn fixed() -> [SecretSlot; 2] {
        [SecretSlot::ProxyPassword, SecretSlot::LensToken]
    }
}

/// What the store holds, as far as this program knows, so writing the
/// settings (which happens on every window move) touches the store only
/// when a secret changed.
#[derive(Debug, Default)]
pub struct SecretSync {
    /// Account → the secret the store holds (`None`: known to hold none).
    known: HashMap<String, Option<String>>,
}

impl SecretSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace every [`IN_STORE`] in `cfg` with the secret from `store`.
    /// Returns the accounts the store had nothing for (their slots are
    /// left empty) and the errors it gave.
    pub fn resolve(&mut self, cfg: &mut Config, slots: &[SecretSlot], store: &dyn SecretStore) -> Vec<String> {
        let mut problems = Vec::new();
        for slot in slots {
            if slot.get(cfg) != IN_STORE {
                continue;
            }
            let account = slot.account();
            match store.get(&account) {
                Ok(Some(secret)) => {
                    slot.set(cfg, &secret);
                    self.known.insert(account, Some(secret));
                }
                Ok(None) => {
                    slot.set(cfg, "");
                    self.known.insert(account.clone(), None);
                    problems.push(format!("{account}: not in {}", store.name()));
                }
                // Left as the marker: written back as it is, so the secret
                // is not lost when the store answers next time.
                Err(e) => problems.push(format!("{account}: {e}")),
            }
        }
        problems
    }

    /// The settings as they go to the file under `cfg`'s storage choice:
    /// with [`SecretStorage::System`] and a store, each secret is put in the
    /// store (when it changed) and replaced by [`IN_STORE`]; otherwise the
    /// file gets the secrets themselves and the store forgets any it held.
    /// A secret the store refuses stays in the file. Returns the copy to
    /// write and the store's errors.
    pub fn prepare(
        &mut self,
        cfg: &Config,
        slots: &[SecretSlot],
        store: Option<&dyn SecretStore>,
    ) -> (Config, Vec<String>) {
        let mut out = cfg.clone();
        let mut errors = Vec::new();
        let system = cfg.general.secret_storage == SecretStorage::System;
        for slot in slots {
            let value = slot.get(cfg);
            let account = slot.account();
            // Never resolved (the store did not answer): keep the marker.
            if value == IN_STORE {
                continue;
            }
            let held = self.known.get(&account).cloned().flatten();
            match store {
                Some(store) if system => {
                    if value.is_empty() {
                        if held.is_some() {
                            match store.delete(&account) {
                                Ok(()) => drop(self.known.insert(account, None)),
                                Err(e) => errors.push(format!("{account}: {e}")),
                            }
                        }
                        continue;
                    }
                    if held.as_deref() != Some(value.as_str()) {
                        if let Err(e) = store.set(&account, &value) {
                            errors.push(format!("{account}: {e}"));
                            continue; // stays in the file
                        }
                        self.known.insert(account, Some(value));
                    }
                    slot.set(&mut out, IN_STORE);
                }
                Some(store) if held.is_some() => match store.delete(&account) {
                    Ok(()) => drop(self.known.insert(account, None)),
                    Err(e) => errors.push(format!("{account}: {e}")),
                },
                _ => {}
            }
        }
        (out, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory {
        items: Mutex<HashMap<String, String>>,
        writes: Mutex<u32>,
        broken: bool,
    }

    impl SecretStore for Memory {
        fn name(&self) -> &'static str {
            "memory"
        }
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            Ok(self.items.lock().unwrap().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            if self.broken {
                return Err("locked".into());
            }
            *self.writes.lock().unwrap() += 1;
            self.items.lock().unwrap().insert(account.into(), secret.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.items.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn slots() -> Vec<SecretSlot> {
        let mut v = vec![SecretSlot::ServiceOption { service: "deepl".into(), key: "api_key".into() }];
        v.extend(SecretSlot::fixed());
        v
    }

    fn config(storage: SecretStorage) -> Config {
        let mut c = Config::default();
        c.general.secret_storage = storage;
        c.services.set_option("deepl", "api_key", serde_json::json!("dk-123"));
        c.network.proxy.password = "hunter2".into();
        c
    }

    #[test]
    fn secrets_leave_the_file_for_the_store_and_come_back() {
        let store = Memory::default();
        let mut sync = SecretSync::new();
        let cfg = config(SecretStorage::System);
        let (file, errors) = sync.prepare(&cfg, &slots(), Some(&store));
        assert!(errors.is_empty());
        assert_eq!(file.services.option("deepl", "api_key"), Some(&serde_json::json!(IN_STORE)));
        assert_eq!(file.network.proxy.password, IN_STORE);
        // An empty secret is not stored and not marked.
        assert_eq!(file.ocr.lens.token, "");
        assert_eq!(store.get("service/deepl/api_key").unwrap().as_deref(), Some("dk-123"));
        // The next start reads the file and fetches the secrets.
        let mut loaded = Config::from_toml(&file.to_toml()).unwrap();
        assert!(SecretSync::new().resolve(&mut loaded, &slots(), &store).is_empty());
        assert_eq!(loaded.services.option("deepl", "api_key"), Some(&serde_json::json!("dk-123")));
        assert_eq!(loaded.network.proxy.password, "hunter2");
    }

    #[test]
    fn an_unchanged_secret_is_not_written_again() {
        let store = Memory::default();
        let mut sync = SecretSync::new();
        let cfg = config(SecretStorage::System);
        sync.prepare(&cfg, &slots(), Some(&store));
        sync.prepare(&cfg, &slots(), Some(&store));
        assert_eq!(*store.writes.lock().unwrap(), 2, "one write per secret, once");
    }

    #[test]
    fn switching_back_to_the_file_moves_them_back_and_clears_the_store() {
        let store = Memory::default();
        let mut sync = SecretSync::new();
        sync.prepare(&config(SecretStorage::System), &slots(), Some(&store));
        let (file, _) = sync.prepare(&config(SecretStorage::File), &slots(), Some(&store));
        assert_eq!(file.network.proxy.password, "hunter2");
        assert!(store.items.lock().unwrap().is_empty());
    }

    #[test]
    fn a_store_that_refuses_leaves_the_secret_in_the_file() {
        let store = Memory { broken: true, ..Memory::default() };
        let (file, errors) = SecretSync::new().prepare(&config(SecretStorage::System), &slots(), Some(&store));
        assert_eq!(errors.len(), 2);
        assert_eq!(file.network.proxy.password, "hunter2");
    }

    #[test]
    fn a_marker_never_resolved_is_kept_rather_than_lost() {
        let mut cfg = config(SecretStorage::System);
        cfg.network.proxy.password = IN_STORE.into();
        // No store at all this run (a Linux session without a keyring).
        let (file, _) = SecretSync::new().prepare(&cfg, &slots(), None);
        assert_eq!(file.network.proxy.password, IN_STORE);
    }
}
