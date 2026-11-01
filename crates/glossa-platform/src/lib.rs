//! Operating-system integration: clipboard, selection capture, synthetic
//! input, screen capture, global hotkeys, autostart, audio playback and the
//! keyboard-layout fix; window backdrops and the system's accent colour.
//!
//! Everything here compiles on Windows, Linux and macOS. A feature the
//! platform cannot provide returns `Err(glossa_core::Error::Platform(..))` or
//! `None` instead of panicking, and [`Session::limitations`] explains to the
//! user, in words, what will not work and what to do instead.
//!
//! Windows is the primary platform; the other columns are best effort.
//!
//! | Feature                         | Windows                         | X11                     | Wayland                                         | macOS                          |
//! |---------------------------------|---------------------------------|-------------------------|-------------------------------------------------|--------------------------------|
//! | Global hotkeys ([`hotkeys`])    | yes                             | yes                     | **no** — bind a compositor shortcut to the HTTP API | yes                         |
//! | Read selection ([`selection`])  | UI Automation, then Ctrl+C      | PRIMARY selection       | PRIMARY via data-control (KDE, wlroots, Hyprland), else clipboard | AX `AXSelectedText`, then Cmd+C |
//! | Replace selection               | clipboard + Ctrl+V              | clipboard + Ctrl+V      | **no** — text is left on the clipboard          | clipboard + Cmd+V              |
//! | Synthetic input ([`input`])     | `SendInput`                     | XTest (enigo)           | **no**                                          | CGEvent (enigo), needs Accessibility |
//! | Foreground window ([`window`])  | process, class, title, pid      | EWMH: process, class, title, pid | none                                   | best effort (xcap)             |
//! | Caret rectangle                 | `GetGUIThreadInfo`              | none                    | none                                            | none                           |
//! | Monitors + work area            | yes, work area excludes taskbar | RandR + `_NET_WORKAREA` | with `wayland-capture` only (work area = bounds)| yes (work area = bounds)       |
//! | Screen capture ([`capture`])    | xcap                            | x11rb `GetImage`        | xcap with `wayland-capture`, else `grim`        | xcap, needs Screen Recording   |
//! | Autostart ([`autostart`])       | `HKCU\…\Run`                    | XDG autostart `.desktop`| XDG autostart `.desktop`                        | LaunchAgent plist              |
//! | Audio ([`audio`])               | rodio                           | rodio (ALSA)            | rodio (ALSA)                                    | rodio                          |
//! | Layout conversion ([`layout`])  | pure + switch foreground layout | pure                    | pure                                            | pure                           |
//! | Window backdrops ([`backdrop`]) | Mica / Acrylic / blur (DWM)     | KWin blur-behind        | none                                            | `NSVisualEffectView`           |
//! | Accent, dark mode ([`appearance`]) | registry                     | `gsettings` (dark only) | `gsettings` (dark only)                         | `NSColor`, `AppleInterfaceStyle` |
//!
//! Threading: [`hotkeys::HotkeyService`] must be created on the thread that
//! runs the event loop on Windows (the hotkeys are bound to that thread's
//! message queue). Everything else may be called from any thread; selection
//! capture and screen capture block for tens to hundreds of milliseconds and
//! belong on a worker thread, not the UI thread.

pub mod appearance;
pub mod audio;
pub mod autostart;
pub mod backdrop;
pub mod capture;
pub mod clipboard;
pub mod crash;
pub mod hotkeys;
pub mod idle;
pub mod input;
pub mod instance;
pub mod layout;
pub mod mouse;
pub mod permissions;
pub mod selection;
pub mod session;
pub mod window;

#[cfg(target_os = "macos")]
mod macos_ax;
mod screen;
#[cfg(windows)]
mod uia;

pub use audio::AudioPlayer;
pub use autostart::{is_autostart_enabled, set_autostart};
pub use backdrop::{find_own_window, Applied, Backdrop};
pub use capture::{capture_all, capture_rect, capture_virtual_desktop, encode_png, images_differ, MonitorShot};
pub use clipboard::{Clipboard, ClipboardSnapshot};
pub use hotkeys::{HotkeyError, HotkeyService};
pub use input::{release_modifiers, send_copy, send_paste, type_text};
pub use layout::{convert_layout, guess_direction, switch_keyboard_layout_of_foreground, Conversion};
pub use selection::{capture_selection, replace_selection, CaptureOptions};
pub use session::{Compositor, DisplayServer, Os, Session};
pub use window::{
    caret_rect, cursor_position, foreground_window, monitor_at, monitors, place_popup, Monitor, WindowInfo,
};

// On Linux xcap is an optional, renamed dependency (see Cargo.toml); give it
// its usual name.
#[cfg(all(target_os = "linux", feature = "wayland-capture"))]
extern crate xcap_linux as xcap;

/// Every fallible call in this crate speaks the pipeline's error type.
pub type Result<T> = glossa_core::Result<T>;

/// Shorthand for the one error variant this crate produces.
pub(crate) fn platform_err(msg: impl Into<String>) -> glossa_core::Error {
    glossa_core::Error::Platform(msg.into())
}
