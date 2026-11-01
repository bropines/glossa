//! What sets the window systems apart, said once per system.
//!
//! The interface is the same everywhere; what differs between Windows,
//! macOS and Linux (how the desktop counts, which corner a window keeps
//! when it is resized, whether Glossa may draw its own title bar…) is
//! written down here, in one file per system, as [`SHELL`]. The UI asks
//! these questions instead of testing for an operating system, so a
//! change for one system lands in that system's file and nowhere else.
//!
//! The implementations behind the answers live with their subject, also
//! one file per system: `window/win.rs`, `backdrop/{win,mac,x11}.rs`.

/// How this system's windows behave. See [`SHELL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shell {
    /// The system's name, for logs and diagnostics.
    pub name: &'static str,
    /// Desktop coordinates (monitors, window positions, the cursor) are in
    /// points (macOS) rather than physical pixels: a Retina screen's scale
    /// factor of 2 is not applied to them.
    pub desktop_in_points: bool,
    /// A window that changes size keeps its bottom-left corner where it was
    /// (macOS) instead of its top-left: one that should stay put under
    /// something has to be moved back after it is resized.
    pub resize_keeps_bottom_left: bool,
    /// Glossa may draw its own title bar and frame. macOS windows keep the
    /// system's (the traffic lights, its corners and shadow).
    pub own_frame: bool,
    /// A translucent window can only be made when the main window was
    /// created with an alpha channel (macOS: the GL configuration is shared
    /// and fixed at start-up). Elsewhere any window may be translucent.
    pub transparency_needs_alpha_root: bool,
    /// Making a window takes long enough to see (macOS: a window and its GL
    /// context per open), so a dropdown that fits in the window it belongs
    /// to is drawn there instead of in a window of its own.
    pub slow_child_windows: bool,
    /// A program may move the pointer without stalling it (not on macOS).
    pub can_warp_pointer: bool,
    /// System backdrops (Mica, Acrylic, vibrancy) can show through windows
    /// drawn with OpenGL, and with the GPU renderer (wgpu).
    pub backdrops_with_gl: bool,
    pub backdrops_with_gpu: bool,
    /// How the shortcut modifier is written: "Ctrl" or "Cmd".
    pub command_key: &'static str,
    /// egui's `command` modifier is the Ctrl key (everywhere but macOS,
    /// where it is Cmd and Ctrl is a key of its own).
    pub command_is_ctrl: bool,
    /// The application has a menu bar of its own, at the top of the screen
    /// (macOS), which follows the interface language.
    pub app_menu: bool,
    /// A menu-bar app that shows in the app switcher (Cmd+Tab) and the Dock
    /// only while one of its full windows is open (macOS).
    pub switcher_follows_windows: bool,
    /// The interface text for "start with the system", by its locale key.
    pub autostart_label: &'static str,
}

#[cfg(windows)]
#[path = "shell/win.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "shell/mac.rs"]
mod imp;
#[cfg(not(any(windows, target_os = "macos")))]
#[path = "shell/linux.rs"]
mod imp;

/// This system's answers.
pub const SHELL: Shell = imp::SHELL;

/// Open a URL or a file with the system's default handler.
pub fn open(target: &str) -> std::io::Result<()> {
    imp::open(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_macos_counts_in_points_and_keeps_its_own_frame() {
        let mac = cfg!(target_os = "macos");
        assert_eq!(SHELL.desktop_in_points, mac);
        assert_eq!(SHELL.resize_keeps_bottom_left, mac);
        assert_eq!(SHELL.own_frame, !mac);
        assert_eq!(SHELL.command_is_ctrl, !mac);
        assert_eq!(SHELL.command_key, if mac { "Cmd" } else { "Ctrl" });
    }
}
