//! How `glossa-cli` (or a second launch) finds the Glossa that owns its
//! data folder, and makes sure of it before sending the token.
//!
//! The port in `config.toml` says little: another program, another user's
//! Glossa or a Glossa with another data folder may be the one listening
//! there, and whoever listens would be handed the token. So a running
//! control API publishes itself in its own data folder:
//!
//! - `control-api.json` (mode 0600): the address it really listens on, its
//!   process id, and a secret made for this run, which never leaves the
//!   file and the process;
//! - `control-api.lock`, locked for as long as it runs. A JSON file whose
//!   lock is free is left over from a crash; the next reader removes it
//!   while holding the lock, so it cannot remove a file a starting server
//!   has just written.
//!
//! A client reads the file, connects to that address and sends a random
//! challenge to `/health`. The answer carries
//! `HMAC-SHA256(secret, challenge, the client's address as the server sees
//! it)`. Only the process that wrote the file can compute it, and a program
//! relaying the challenge to the real server gets a proof for its own
//! connection, not for the client's. Only then is the token sent. Nothing
//! secret is revealed by `/health` itself.

use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use glossa_core::{Config, Paths};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The file a running control API describes itself in, in the data folder.
pub const RUNTIME_FILE: &str = "control-api.json";
const LOCK_FILE: &str = "control-api.lock";

/// The longest challenge answered; ours are 64 characters.
pub(super) const MAX_CHALLENGE: usize = 128;

pub fn runtime_file(paths: &Paths) -> PathBuf {
    paths.data_dir.join(RUNTIME_FILE)
}

/// HMAC-SHA256 (RFC 2104), on the SHA-256 already in the tree.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new().chain_update(block.map(|b| b ^ 0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(block.map(|b| b ^ 0x5c)).chain_update(inner).finalize().into()
}

/// The proof that the holder of `secret` answered `challenge` on the
/// connection from `client`.
pub(super) fn proof(secret: &str, challenge: &str, client: SocketAddr) -> String {
    // The same address reads `::ffff:127.0.0.1` on a dual-stack listener
    // and `127.0.0.1` on the client's side.
    let client = SocketAddr::new(client.ip().to_canonical(), client.port());
    let mac = hmac_sha256(secret.as_bytes(), format!("glossa-control-v1\n{challenge}\n{client}").as_bytes());
    mac.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether a challenge is one we answer: short, and plain enough to go
/// into a query string and a MAC unescaped.
pub(super) fn challenge_ok(challenge: &str) -> bool {
    !challenge.is_empty() && challenge.len() <= MAX_CHALLENGE && challenge.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A published runtime file. Dropping it withdraws the file.
pub(super) struct Published {
    path: PathBuf,
    /// Held locked while the server runs.
    _lock: File,
}

impl Published {
    /// Describe the server listening on `addr` in the data folder. `None`
    /// (with a log line) when another control API already serves this data
    /// folder, or the folder cannot be written: the server still runs, but
    /// `glossa-cli` will not find it.
    pub(super) fn publish(paths: &Paths, addr: SocketAddr, secret: &str) -> Option<Published> {
        let path = runtime_file(paths);
        if let Err(e) = std::fs::create_dir_all(&paths.data_dir) {
            tracing::warn!("control API: {} cannot be created ({e}); glossa-cli will not find it", path.display());
            return None;
        }
        let lock = match open_lock(&paths.data_dir.join(LOCK_FILE), true) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("control API: no lock file ({e}); glossa-cli will not find this instance");
                return None;
            }
        };
        // A reader checking for a stale file holds the lock for an instant.
        let mut locked = false;
        for _ in 0..20 {
            match lock.try_lock() {
                Ok(()) => {
                    locked = true;
                    break;
                }
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
                Err(std::fs::TryLockError::Error(e)) => {
                    tracing::warn!("control API: the lock file cannot be locked ({e}); glossa-cli will not find it");
                    return None;
                }
            }
        }
        if !locked {
            tracing::warn!(
                "another control API already serves this data folder; glossa-cli keeps talking to that one, not \
                 to the one on {addr}"
            );
            return None;
        }
        let body = json!({
            "addr": super::connect_addr(&addr.to_string()),
            "pid": std::process::id(),
            "instance": secret,
            "version": glossa_core::VERSION,
        });
        match write_private(&path, body.to_string().as_bytes()) {
            Ok(()) => Some(Published { path, _lock: lock }),
            Err(e) => {
                tracing::warn!("control API: {} not written ({e}); glossa-cli will not find it", path.display());
                None
            }
        }
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        // Still holding the lock, so this is our own file.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn open_lock(path: &Path, create: bool) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(create).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Write `bytes` to `path` so that nobody else can read them (0600 from the
/// moment the file exists; on Windows the data folder's own access list
/// applies), through a temporary file, so a reader never sees half of it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    let done = written.and_then(|()| std::fs::rename(&tmp, path));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// A running control API that proved it owns this data folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// Where to connect.
    pub addr: String,
    /// The token to send, from this data folder's `config.toml`.
    pub token: String,
    pub pid: u32,
    pub version: String,
}

/// What [`find`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// No control API runs for this data folder.
    None,
    Running(Running),
    /// The runtime file is there and its lock is held, but what answers at
    /// its address did not prove to be the process that wrote it (or
    /// nothing answered). The token must not be sent; the reason is for the
    /// user.
    Unverified(String),
}

impl Found {
    pub fn running(self) -> Option<Running> {
        match self {
            Found::Running(r) => Some(r),
            _ => None,
        }
    }
}

/// Find the control API of the Glossa that uses `paths`, and check that it
/// is the one before anything secret is sent to it. A runtime file left by
/// a crash is removed.
pub fn find(paths: &Paths) -> Found {
    let path = runtime_file(paths);
    let Ok(text) = std::fs::read_to_string(&path) else { return Found::None };
    // Stale? Whoever wrote the file holds the lock while it runs.
    match open_lock(&paths.data_dir.join(LOCK_FILE), false) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(&path);
            return Found::None;
        }
        Err(_) => {}
        Ok(lock) => {
            if lock.try_lock().is_ok() {
                // Nobody runs. Under the lock, a server starting right now
                // waits for us, so the file removed is the stale one.
                if std::fs::read_to_string(&path).is_ok_and(|now| now == text) {
                    let _ = std::fs::remove_file(&path);
                    tracing::debug!("removed a stale {}", path.display());
                }
                let _ = lock.unlock();
                return Found::None;
            }
        }
    }
    let Ok(file) = serde_json::from_str::<Value>(&text) else {
        return Found::Unverified(format!("{} is not readable JSON", path.display()));
    };
    let (Some(addr), Some(secret)) = (file["addr"].as_str(), file["instance"].as_str()) else {
        return Found::Unverified(format!("{} lacks the address or the instance secret", path.display()));
    };
    let Ok(token) = Config::load(&paths.config_file()).map(|c| c.server.token) else {
        return Found::Unverified("config.toml cannot be read for the token".into());
    };
    match verify(addr, secret) {
        Ok(()) => Found::Running(Running {
            addr: addr.to_owned(),
            token,
            pid: file["pid"].as_u64().unwrap_or(0) as u32,
            version: file["version"].as_str().unwrap_or("").to_owned(),
        }),
        Err(why) => Found::Unverified(format!("{addr}: {why}")),
    }
}

/// Challenge what listens at `addr`; `Ok` when it holds `secret`.
fn verify(addr: &str, secret: &str) -> Result<(), String> {
    let sock: SocketAddr = addr.parse().map_err(|e| format!("not an address ({e})"))?;
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&sock, timeout).map_err(|e| format!("nothing answers ({e})"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let local = stream.local_addr().map_err(|e| e.to_string())?;
    let challenge = crate::random_token();
    // No token, no cookie, nothing of the user's: this request may be read
    // by anyone.
    let request = format!("GET /health?challenge={challenge} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    let mut answer = Vec::new();
    let _ = stream.take(64 * 1024).read_to_end(&mut answer);
    let answer = String::from_utf8_lossy(&answer);
    let body = answer.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let health: Value = serde_json::from_str(body).map_err(|_| "another program has the port".to_owned())?;
    let given = health["proof"].as_str().unwrap_or("");
    if super::constant_time_eq(given, &proof(secret, &challenge, local)) {
        Ok(())
    } else if health["name"] == "glossa" {
        Err("a Glossa answers, but not the one that owns this data folder".into())
    } else {
        Err("another program has the port".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // A key longer than the block is hashed first (test case 6).
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn proofs_depend_on_the_secret_the_challenge_and_the_connection() {
        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let p = proof("secret", "abc", peer);
        assert_eq!(p.len(), 64);
        assert_eq!(p, proof("secret", "abc", peer));
        assert_ne!(p, proof("other", "abc", peer));
        assert_ne!(p, proof("secret", "abd", peer));
        assert_ne!(p, proof("secret", "abc", "127.0.0.1:50001".parse().unwrap()), "a relay's connection differs");
        // A dual-stack listener sees the same client as an IPv4-mapped address.
        assert_eq!(p, proof("secret", "abc", "[::ffff:127.0.0.1]:50000".parse().unwrap()));
        assert!(challenge_ok("0123abcXYZ"));
        for bad in ["", "a b", "a&b=c", "a\nb", &"x".repeat(MAX_CHALLENGE + 1)] {
            assert!(!challenge_ok(bad), "{bad:?}");
        }
    }
}
