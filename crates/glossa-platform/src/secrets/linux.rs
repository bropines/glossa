//! The freedesktop Secret Service (GNOME Keyring, KWallet, KeePassXC) over
//! the session bus, with the "plain" session: the secret travels over the
//! local bus unencrypted, as `secret-tool` sends it, and is stored
//! encrypted by the keyring. Items carry the attributes
//! `application = glossa` and `account = <account>`, and live in the
//! default collection. A locked collection is unlocked through the
//! keyring's own prompt.

use std::collections::HashMap;
use std::sync::Arc;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{SecretStore, SERVICE};

const DEST: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
const SCHEMA: &str = "io.github.bropines.Glossa";

/// A secret as the API passes it: session, parameters, value, content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

struct SecretService {
    conn: Connection,
}

pub fn store() -> Option<Arc<dyn SecretStore>> {
    let conn = Connection::session().ok()?;
    let bus = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").ok()?;
    let running: bool = bus.call("NameHasOwner", &(DEST,)).unwrap_or(false);
    let startable = || {
        bus.call::<_, _, Vec<String>>("ListActivatableNames", &())
            .map(|names| names.iter().any(|n| n == DEST))
            .unwrap_or(false)
    };
    if !running && !startable() {
        return None;
    }
    Some(Arc::new(SecretService { conn }))
}

fn err(e: impl std::fmt::Display) -> String {
    format!("Secret Service: {e}")
}

impl SecretService {
    fn proxy<'a>(&'a self, path: &'a str, iface: &'a str) -> Result<Proxy<'a>, String> {
        Proxy::new(&self.conn, DEST, path, iface).map_err(err)
    }

    fn service(&self) -> Result<Proxy<'_>, String> {
        self.proxy(SERVICE_PATH, "org.freedesktop.Secret.Service")
    }

    fn open_session(&self) -> Result<OwnedObjectPath, String> {
        let (_, session): (OwnedValue, OwnedObjectPath) =
            self.service()?.call("OpenSession", &("plain", Value::from(""))).map_err(err)?;
        Ok(session)
    }

    fn attributes(account: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "glossa"), ("account", account), ("xdg:schema", SCHEMA)])
    }

    /// Run a prompt the keyring asked for (unlocking, confirming) and wait
    /// for the user to finish with it. Returns false if they dismissed it.
    fn prompt(&self, path: &OwnedObjectPath) -> Result<bool, String> {
        if path.as_str() == "/" {
            return Ok(true);
        }
        let prompt = self.proxy(path.as_str(), "org.freedesktop.Secret.Prompt")?;
        let mut completed = prompt.receive_signal("Completed").map_err(err)?;
        prompt.call::<_, _, ()>("Prompt", &("",)).map_err(err)?;
        let Some(message) = completed.next() else { return Ok(false) };
        let (dismissed, _): (bool, OwnedValue) = message.body().deserialize().map_err(err)?;
        Ok(!dismissed)
    }

    /// The items for `account`, unlocked (asking the keyring if needed).
    fn find(&self, account: &str) -> Result<Vec<OwnedObjectPath>, String> {
        let service = self.service()?;
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            service.call("SearchItems", &(Self::attributes(account),)).map_err(err)?;
        if !locked.is_empty() {
            let (now, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                service.call("Unlock", &(&locked,)).map_err(err)?;
            unlocked.extend(now);
            if prompt.as_str() != "/" && self.prompt(&prompt)? {
                unlocked.extend(locked);
            }
        }
        unlocked.dedup();
        Ok(unlocked)
    }
}

impl SecretStore for SecretService {
    fn name(&self) -> &'static str {
        "Secret Service"
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let Some(item) = self.find(account)?.into_iter().next() else { return Ok(None) };
        let session = self.open_session()?;
        let secret: Secret =
            self.proxy(item.as_str(), "org.freedesktop.Secret.Item")?.call("GetSecret", &(&session,)).map_err(err)?;
        Ok(Some(String::from_utf8_lossy(&secret.2).into_owned()))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let session = self.open_session()?;
        let collection = self.proxy(DEFAULT_COLLECTION, "org.freedesktop.Secret.Collection")?;
        let label = format!("{SERVICE}: {account}");
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label.as_str())),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(Self::attributes(account))),
        ]);
        let value: Secret = (session, Vec::new(), secret.as_bytes().to_vec(), "text/plain".into());
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) =
            collection.call("CreateItem", &(properties, value, true)).map_err(err)?;
        if self.prompt(&prompt)? {
            Ok(())
        } else {
            Err(err("the keyring stayed locked"))
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        for item in self.find(account)? {
            let prompt: OwnedObjectPath =
                self.proxy(item.as_str(), "org.freedesktop.Secret.Item")?.call("Delete", &()).map_err(err)?;
            self.prompt(&prompt)?;
        }
        Ok(())
    }
}
