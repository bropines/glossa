//! Which operating system and display server we are running under, and what
//! that means for the features the user expects.
//!
//! The answer decides more than it looks: on Wayland there are no global
//! hotkeys and no synthetic keystrokes, so the whole "select text, press a
//! hotkey" workflow has to be rewired through a compositor shortcut and the
//! local HTTP API. Detecting that up front lets the UI say so once, clearly,
//! instead of failing silently every time a hotkey is pressed.

use std::fmt;

/// The operating-system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    /// A BSD or anything else unix-like; treated like Linux.
    OtherUnix,
}

impl Os {
    /// The OS this binary was compiled for.
    pub const fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::OtherUnix
        }
    }
}

/// A Wayland compositor, as far as it matters to us: whether it implements
/// the data-control protocol (`ext-data-control-v1` / `wlr-data-control`),
/// which is the only way an unfocused client can read the PRIMARY selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Compositor {
    Gnome,
    Kde,
    Sway,
    Hyprland,
    Niri,
    Cosmic,
    /// Another wlroots-based compositor (river, Wayfire, labwc, …).
    Wlroots(String),
    /// Something we do not recognise; the string is `XDG_CURRENT_DESKTOP`.
    Other(String),
    Unknown,
}

impl Compositor {
    /// Whether the compositor is known to offer a data-control protocol.
    /// GNOME (Mutter) deliberately does not; unknown compositors are assumed
    /// not to, so we promise nothing we cannot keep (we still try at runtime).
    pub fn has_data_control(&self) -> bool {
        matches!(
            self,
            Compositor::Kde
                | Compositor::Sway
                | Compositor::Hyprland
                | Compositor::Niri
                | Compositor::Cosmic
                | Compositor::Wlroots(_)
        )
    }
}

impl fmt::Display for Compositor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Compositor::Gnome => f.write_str("GNOME"),
            Compositor::Kde => f.write_str("KDE Plasma"),
            Compositor::Sway => f.write_str("Sway"),
            Compositor::Hyprland => f.write_str("Hyprland"),
            Compositor::Niri => f.write_str("niri"),
            Compositor::Cosmic => f.write_str("COSMIC"),
            Compositor::Wlroots(name) | Compositor::Other(name) => f.write_str(name),
            Compositor::Unknown => f.write_str("This compositor"),
        }
    }
}

/// The windowing system.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DisplayServer {
    Windows,
    MacOs,
    X11,
    Wayland {
        compositor: Compositor,
    },
    /// No graphical session at all: a TTY, ssh, a container, WSL without
    /// WSLg. Nearly everything in this crate fails here.
    Unknown,
}

/// The session the program runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub os: Os,
    pub display: DisplayServer,
    /// The desktop environment's own name (`XDG_CURRENT_DESKTOP` on Linux),
    /// for logs and bug reports. Empty when unknown.
    pub desktop: String,
}

impl Session {
    /// Detect the current session from the compile target and the process
    /// environment. Cheap (a few environment reads), so callers need not
    /// cache it.
    pub fn detect() -> Session {
        Session::detect_from(Os::current(), |k| std::env::var(k).ok())
    }

    /// Detection with an injectable environment, so the rules can be tested
    /// on any machine.
    pub fn detect_from(os: Os, env: impl Fn(&str) -> Option<String>) -> Session {
        let get = |k: &str| env(k).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        match os {
            Os::Windows => {
                Session { os, display: DisplayServer::Windows, desktop: "Windows".into() }
            }
            Os::MacOs => Session { os, display: DisplayServer::MacOs, desktop: "macOS".into() },
            Os::Linux | Os::OtherUnix => {
                let desktop = get("XDG_CURRENT_DESKTOP")
                    .or_else(|| get("XDG_SESSION_DESKTOP"))
                    .or_else(|| get("DESKTOP_SESSION"))
                    .unwrap_or_default();
                let session_type = get("XDG_SESSION_TYPE").unwrap_or_default().to_ascii_lowercase();
                // Under Wayland DISPLAY is usually set too (XWayland), so
                // Wayland has to win: an X11 client there still cannot see
                // other clients' keystrokes or inject its own.
                let wayland = get("WAYLAND_DISPLAY").is_some() || session_type == "wayland";
                let x11 = get("DISPLAY").is_some() || session_type == "x11";
                let display = if wayland {
                    DisplayServer::Wayland { compositor: detect_compositor(&get, &desktop) }
                } else if x11 {
                    DisplayServer::X11
                } else {
                    DisplayServer::Unknown
                };
                Session { os, display, desktop }
            }
        }
    }

    pub fn is_wayland(&self) -> bool {
        matches!(self.display, DisplayServer::Wayland { .. })
    }

    /// Whether system-wide hotkeys can be registered. The `global-hotkey`
    /// crate covers Windows, macOS and X11; Wayland has no such protocol.
    pub fn supports_global_hotkeys(&self) -> bool {
        matches!(self.display, DisplayServer::Windows | DisplayServer::MacOs | DisplayServer::X11)
    }

    /// Whether the PRIMARY ("last highlighted text") selection can be read
    /// without a keystroke. Windows and macOS have no such thing.
    pub fn supports_primary_selection(&self) -> bool {
        match &self.display {
            DisplayServer::X11 => true,
            DisplayServer::Wayland { compositor } => compositor.has_data_control(),
            _ => false,
        }
    }

    /// Whether we can send keystrokes to other programs (the synthetic copy
    /// and paste). On macOS this additionally needs the Accessibility
    /// permission, which we cannot check from here without prompting.
    pub fn supports_synthetic_input(&self) -> bool {
        matches!(self.display, DisplayServer::Windows | DisplayServer::MacOs | DisplayServer::X11)
    }

    /// What will not work in this session, in words for the user.
    pub fn limitations(&self) -> Vec<String> {
        let mut out = Vec::new();
        match &self.display {
            DisplayServer::Windows => {
                out.push(
                    "Programs running as administrator cannot be read from or typed into \
                     unless Glossa also runs as administrator."
                        .into(),
                );
            }
            DisplayServer::MacOs => {
                out.push(
                    "Reading the selection and pasting translations need the Accessibility \
                     permission (System Settings → Privacy & Security → Accessibility)."
                        .into(),
                );
                out.push("Screen text recognition needs the Screen Recording permission.".into());
            }
            DisplayServer::X11 => {
                out.push(
                    "The selection is read from the PRIMARY selection: the text highlighted \
                     most recently, in any window."
                        .into(),
                );
            }
            DisplayServer::Wayland { compositor } => {
                out.push(
                    "Wayland does not allow global hotkeys. Bind a shortcut in your \
                     compositor's settings that calls Glossa's local HTTP API with curl."
                        .into(),
                );
                out.push(
                    "Wayland does not allow typing into other programs: \"replace selection\" \
                     leaves the translation on the clipboard for you to paste."
                        .into(),
                );
                if !compositor.has_data_control() {
                    out.push(format!(
                        "{compositor} does not let Glossa read the highlighted text; copy it \
                         with Ctrl+C first and Glossa will use the clipboard."
                    ));
                }
                out.push(
                    "The popup cannot be placed at the text cursor; it opens near the mouse \
                     or in the centre of the screen."
                        .into(),
                );
            }
            DisplayServer::Unknown => {
                out.push(
                    "No graphical session was found (neither WAYLAND_DISPLAY nor DISPLAY is \
                     set): clipboard, hotkeys and screen capture are unavailable."
                        .into(),
                );
            }
        }
        out
    }
}

fn detect_compositor(get: &impl Fn(&str) -> Option<String>, desktop: &str) -> Compositor {
    // Compositor-specific sockets are more reliable than the desktop name,
    // which users and display managers set to all sorts of things.
    if get("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return Compositor::Hyprland;
    }
    if get("SWAYSOCK").is_some() {
        return Compositor::Sway;
    }
    if get("NIRI_SOCKET").is_some() {
        return Compositor::Niri;
    }
    if get("KDE_FULL_SESSION").is_some() {
        return Compositor::Kde;
    }
    // XDG_CURRENT_DESKTOP is a colon-separated list, e.g. "ubuntu:GNOME".
    for part in desktop.split(':').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "kde" | "plasma" => return Compositor::Kde,
            "gnome" | "gnome-classic" | "gnome-flashback" | "unity" | "pantheon" | "budgie" => {
                return Compositor::Gnome
            }
            "sway" => return Compositor::Sway,
            "hyprland" => return Compositor::Hyprland,
            "niri" => return Compositor::Niri,
            "cosmic" => return Compositor::Cosmic,
            "river" | "wayfire" | "labwc" | "dwl" | "qtile" | "hikari" | "cage" => {
                return Compositor::Wlroots(part)
            }
            _ => {}
        }
    }
    if desktop.is_empty() {
        Compositor::Unknown
    } else {
        Compositor::Other(desktop.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn linux(vars: &[(&str, &str)]) -> Session {
        let map: HashMap<String, String> =
            vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Session::detect_from(Os::Linux, move |k| map.get(k).cloned())
    }

    #[test]
    fn windows_and_macos_ignore_the_environment() {
        let w = Session::detect_from(Os::Windows, |_| Some("wayland-0".into()));
        assert_eq!(w.display, DisplayServer::Windows);
        assert!(w.supports_global_hotkeys() && w.supports_synthetic_input());
        assert!(!w.supports_primary_selection());
        let m = Session::detect_from(Os::MacOs, |_| None);
        assert_eq!(m.display, DisplayServer::MacOs);
        assert!(m.limitations().iter().any(|l| l.contains("Accessibility")));
    }

    #[test]
    fn plain_x11() {
        let s = linux(&[("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", "XFCE")]);
        assert_eq!(s.display, DisplayServer::X11);
        assert_eq!(s.desktop, "XFCE");
        assert!(s.supports_global_hotkeys());
        assert!(s.supports_primary_selection());
        assert!(s.supports_synthetic_input());
        assert!(!s.is_wayland());
    }

    #[test]
    fn wayland_wins_over_xwayland_display() {
        let s = linux(&[
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
        ]);
        assert_eq!(s.display, DisplayServer::Wayland { compositor: Compositor::Gnome });
        assert!(s.is_wayland());
        assert!(!s.supports_global_hotkeys());
        assert!(!s.supports_synthetic_input());
        assert!(!s.supports_primary_selection(), "GNOME has no data-control");
        let lim = s.limitations().join("\n");
        assert!(lim.contains("global hotkeys"));
        assert!(lim.contains("GNOME does not let Glossa read"));
    }

    #[test]
    fn session_type_alone_is_enough() {
        let s = linux(&[("XDG_SESSION_TYPE", "wayland"), ("XDG_CURRENT_DESKTOP", "KDE")]);
        assert_eq!(s.display, DisplayServer::Wayland { compositor: Compositor::Kde });
        assert!(s.supports_primary_selection());
        let s = linux(&[("XDG_SESSION_TYPE", "X11")]);
        assert_eq!(s.display, DisplayServer::X11);
    }

    #[test]
    fn compositor_sockets_beat_desktop_names() {
        let s = linux(&[
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ]);
        assert_eq!(s.display, DisplayServer::Wayland { compositor: Compositor::Hyprland });
        let s = linux(&[("WAYLAND_DISPLAY", "wayland-1"), ("SWAYSOCK", "/run/sway.sock")]);
        assert_eq!(s.display, DisplayServer::Wayland { compositor: Compositor::Sway });
        assert!(s.supports_primary_selection());
        assert!(!s.limitations().iter().any(|l| l.contains("Ctrl+C first")));
    }

    #[test]
    fn wlroots_family_and_unknowns() {
        let s = linux(&[("WAYLAND_DISPLAY", "w"), ("XDG_CURRENT_DESKTOP", "river")]);
        assert_eq!(
            s.display,
            DisplayServer::Wayland { compositor: Compositor::Wlroots("river".into()) }
        );
        assert!(s.supports_primary_selection());
        let s = linux(&[("WAYLAND_DISPLAY", "w"), ("XDG_CURRENT_DESKTOP", "Weird")]);
        assert_eq!(
            s.display,
            DisplayServer::Wayland { compositor: Compositor::Other("Weird".into()) }
        );
        assert!(!s.supports_primary_selection());
        let s = linux(&[("WAYLAND_DISPLAY", "w")]);
        assert_eq!(s.display, DisplayServer::Wayland { compositor: Compositor::Unknown });
        assert!(s.limitations().iter().any(|l| l.starts_with("This compositor")));
    }

    #[test]
    fn no_display_at_all() {
        let s = linux(&[("DISPLAY", "  "), ("XDG_SESSION_TYPE", "tty")]);
        assert_eq!(s.display, DisplayServer::Unknown);
        assert!(!s.supports_global_hotkeys());
        assert!(!s.supports_synthetic_input());
        assert!(s.limitations()[0].contains("No graphical session"));
    }

    #[test]
    fn desktop_falls_back_through_the_variables() {
        let s = linux(&[("DISPLAY", ":1"), ("DESKTOP_SESSION", "openbox")]);
        assert_eq!(s.desktop, "openbox");
    }
}
