//! What the options window checks before it saves: whether the draft
//! differs from the settings in effect, and whether what was typed into
//! its free-text fields can work. Pure functions over the settings, so they
//! are tested without a window; the window says what they found next to
//! the field (`options.rs`).

use std::net::SocketAddr;

use glossa_core::config::ProxyMode;
use glossa_core::{Action, Config};

/// The draft as saving it would leave the settings: the parts the options
/// window does not own are taken from `current` (the engine keeps window
/// geometry, pane contents and module approvals up to date itself, and
/// Apply leaves them alone), and "start with the system" is the checkbox.
fn as_saved(draft: &Config, autostart: bool, current: &Config) -> Config {
    let mut c = draft.clone();
    c.general.start_with_system = autostart;
    c.windows = current.windows.clone();
    c.contents = current.contents.clone();
    c.modules.granted = current.modules.granted.clone();
    c.modules.trust_initialised = current.modules.trust_initialised;
    c
}

/// Whether Apply would change anything: the draft differs from the
/// settings in effect, or the autostart checkbox from the system's entry.
pub fn dirty(draft: &Config, autostart: bool, autostart_initial: bool, current: &Config) -> bool {
    if autostart != autostart_initial {
        return true;
    }
    // The setting follows the system's entry (see `options::apply`), so a
    // settings file that disagrees with it is not a change of the user's.
    let mut current = current.clone();
    current.general.start_with_system = autostart;
    as_saved(draft, autostart, &current) != current
}

/// A field of the options window that cannot be saved as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A manual proxy without a host.
    ProxyHost,
    /// A global hotkey on a typing key with no modifier.
    HotkeyNeedsModifier(Action),
    /// The same, for a module panel's hotkey.
    PanelHotkeyNeedsModifier(String),
    /// The control API's address is not `IP:port`.
    ListenAddress,
}

/// Everything in `draft` that cannot be saved, in the order of the pages.
/// Paths that do not exist are found separately, on a worker (they need
/// the disk): see `options::PathChecks`.
pub fn problems(draft: &Config) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut actions: Vec<Action> = draft
        .hotkeys
        .bindings
        .iter()
        .filter(|(_, hk)| crate::widgets::hotkey_needs_modifier(hk))
        .map(|(a, _)| *a)
        .collect();
    actions.sort_by_key(|a| Action::ALL.iter().position(|x| x == a));
    out.extend(actions.into_iter().map(Problem::HotkeyNeedsModifier));
    let mut panels: Vec<&String> = draft
        .hotkeys
        .panels
        .iter()
        .filter(|(_, hk)| crate::widgets::hotkey_needs_modifier(hk))
        .map(|(id, _)| id)
        .collect();
    panels.sort();
    out.extend(panels.into_iter().map(|id| Problem::PanelHotkeyNeedsModifier(id.clone())));
    let proxy = &draft.network.proxy;
    if proxy.mode == ProxyMode::Manual && proxy.host.trim().is_empty() {
        out.push(Problem::ProxyHost);
    }
    // An address that cannot be parsed matters only while the API is on;
    // switched off, whatever is in the field is not used.
    if draft.server.enabled && listen_address(&draft.server.bind) == Listen::Invalid {
        out.push(Problem::ListenAddress);
    }
    out
}

/// What the control API's "Listen on" field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// An address of this computer only (`127.0.0.1:8787`, `[::1]:8787`).
    Loopback,
    /// An address other computers can reach: allowed, with a warning.
    Network,
    /// Not `IP:port`.
    Invalid,
}

/// Read a listen address the way the server does (`SocketAddr`): an IP
/// address and a port, no host names.
pub fn listen_address(bind: &str) -> Listen {
    match bind.trim().parse::<SocketAddr>() {
        // Port 0 asks the system for any free port: nothing could be bound
        // to a key, since the port would change with every start.
        Ok(addr) if addr.port() == 0 => Listen::Invalid,
        Ok(addr) if addr.ip().is_loopback() => Listen::Loopback,
        Ok(_) => Listen::Network,
        Err(_) => Listen::Invalid,
    }
}

/// The port of a listen address, for messages about it.
pub fn listen_port(bind: &str) -> Option<u16> {
    bind.trim().parse::<SocketAddr>().ok().map(|a| a.port())
}

/// Whether the server's start-up error says its address is taken. The
/// engine hands the error over as text, which for an OS error ends in the
/// system's code: `EADDRINUSE` is 48 on macOS and the BSDs, 98 on Linux,
/// and Windows reports `WSAEADDRINUSE`, 10048.
pub fn address_in_use(error: &str) -> bool {
    ["(os error 48)", "(os error 98)", "(os error 10048)"].iter().any(|code| error.contains(code))
        || error.to_ascii_lowercase().contains("address already in use")
}

/// Whether a field that takes "a program or a command" holds a path that
/// can be looked for on disk: an absolute one (quotes around it are
/// allowed, as Windows copies paths). A bare command (`java`, `py -3`) is
/// found through `PATH` when it runs and is not checked.
pub fn path_to_check(value: &str) -> Option<std::path::PathBuf> {
    let value = value.trim().trim_matches('"');
    let path = std::path::Path::new(value);
    (!value.is_empty() && path.is_absolute()).then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::config::ProxyMode;

    #[test]
    fn a_draft_is_dirty_only_when_saving_it_would_change_something() {
        let current = Config::default();
        let mut draft = current.clone();
        assert!(!dirty(&draft, false, false, &current));
        draft.network.timeout_secs += 1;
        assert!(dirty(&draft, false, false, &current));
        draft = current.clone();
        // The autostart checkbox is a change by itself.
        assert!(dirty(&draft, true, false, &current));
        // The engine's own bookkeeping is not the window's to save.
        let mut moved = current.clone();
        moved.modules.trust_initialised = !moved.modules.trust_initialised;
        moved.modules.granted.insert("some-module".into(), Default::default());
        assert!(!dirty(&draft, false, false, &moved));
        // A settings file that disagrees with the system's autostart entry
        // is brought in line on Apply without being the user's change.
        let mut stale = current.clone();
        stale.general.start_with_system = true;
        assert!(!dirty(&draft, false, false, &stale));
    }

    #[test]
    fn listen_addresses_are_ip_and_port() {
        assert_eq!(listen_address("127.0.0.1:8787"), Listen::Loopback);
        assert_eq!(listen_address(" [::1]:8787 "), Listen::Loopback);
        assert_eq!(listen_address("0.0.0.0:8787"), Listen::Network);
        assert_eq!(listen_address("192.168.1.5:9000"), Listen::Network);
        for bad in ["", "localhost:8787", "127.0.0.1", "127.0.0.1:99999", "8787", "127.0.0.1:0", "http://127.0.0.1:80"]
        {
            assert_eq!(listen_address(bad), Listen::Invalid, "{bad:?}");
        }
        assert_eq!(listen_port("127.0.0.1:8787"), Some(8787));
        assert_eq!(listen_port("nonsense"), None);
    }

    #[test]
    fn a_taken_address_is_told_from_other_start_up_errors() {
        assert!(address_in_use("control API on 127.0.0.1:8787: Address already in use (os error 48)"));
        assert!(address_in_use("control API on 127.0.0.1:8787: Address already in use (os error 98)"));
        assert!(address_in_use(
            "control API on 127.0.0.1:8787: Only one usage of each socket address is normally permitted. (os error 10048)"
        ));
        assert!(!address_in_use("control API on 1.2.3.4:8787: Can't assign requested address (os error 49)"));
        assert!(!address_in_use("server.bind \"x\": invalid socket address syntax"));
    }

    #[test]
    fn problems_name_the_fields_that_cannot_be_saved() {
        let mut c = Config::default();
        c.hotkeys.bindings.clear();
        assert!(problems(&c).is_empty(), "{:?}", problems(&c));
        c.network.proxy.mode = ProxyMode::Manual;
        c.network.proxy.host = "  ".into();
        c.hotkeys.bindings.insert(Action::ShowPopup, "Q".parse().unwrap());
        c.hotkeys.bindings.insert(Action::ShowMainWindow, "Ctrl+Q".parse().unwrap());
        c.hotkeys.panels.insert("notes".into(), "Shift+A".parse().unwrap());
        c.hotkeys.panels.insert("clock".into(), "F9".parse().unwrap());
        c.server.enabled = true;
        c.server.bind = "localhost:1".into();
        assert_eq!(
            problems(&c),
            [
                Problem::HotkeyNeedsModifier(Action::ShowPopup),
                Problem::PanelHotkeyNeedsModifier("notes".into()),
                Problem::ProxyHost,
                Problem::ListenAddress,
            ]
        );
        // Not checked while it is not used.
        c.server.enabled = false;
        c.network.proxy.mode = ProxyMode::System;
        assert_eq!(problems(&c).len(), 2);
    }

    #[test]
    fn the_default_settings_have_no_problems() {
        assert!(problems(&Config::default()).is_empty(), "{:?}", problems(&Config::default()));
    }

    #[test]
    fn only_absolute_paths_are_looked_for() {
        assert_eq!(path_to_check("java"), None);
        assert_eq!(path_to_check("py -3"), None);
        assert_eq!(path_to_check(""), None);
        let abs = if cfg!(windows) { "C:\\Program Files\\Java\\bin\\java.exe" } else { "/usr/bin/java" };
        assert_eq!(path_to_check(&format!(" \"{abs}\" ")), Some(std::path::PathBuf::from(abs)));
    }
}
