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
    /// A copy may keep its data beside the program (portable). Not inside a
    /// macOS app bundle: writing there breaks its signature.
    pub portable_beside_exe: bool,
    /// The interface text for "start with the system", by its locale key.
    pub autostart_label: &'static str,
    /// What the backdrops are called here, by their locale keys: Windows'
    /// own names (Mica, Acrylic…) there and on Linux, the vibrancy
    /// materials they become on macOS.
    pub backdrop_names: BackdropNames,
    /// What the modifier keys are called on this system's keyboards, for
    /// showing a hotkey (the settings file keeps `Ctrl`, `Alt`, `Shift`,
    /// `Meta` everywhere).
    pub modifier_names: ModifierNames,
    /// The system tells which class a window and its focused control are
    /// (Windows, X11), so exception rules can name one. macOS tells only
    /// the app.
    pub window_classes: bool,
    /// A program name as the exceptions list wants it, for the hint in the
    /// empty field: an executable's file name, or an app's name on macOS.
    pub program_example: &'static str,
    /// The tray icon reports clicks (Windows, the macOS menu bar), so what
    /// they do can be chosen. Linux indicators (AppIndicator) report none:
    /// a click there always opens the menu.
    pub tray_clicks: bool,
}

/// Names of the modifier keys, in the order a hotkey shows them (the order
/// Apple and Microsoft both use: Control, Alt or Option, Shift, then the
/// Command, Windows or Super key). See [`Shell::modifier_names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierNames {
    pub ctrl: &'static str,
    pub alt: &'static str,
    pub shift: &'static str,
    pub meta: &'static str,
}

/// Locale keys for the backdrop choices and the hint under the backdrop
/// opacity slider, which talks about them. See [`Shell::backdrop_names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropNames {
    pub mica: &'static str,
    pub mica_alt: &'static str,
    pub acrylic: &'static str,
    pub blur: &'static str,
    pub opacity_hint: &'static str,
}

/// Windows' names, which Linux (KWin's blur) uses too.
pub const WINDOWS_BACKDROP_NAMES: BackdropNames = BackdropNames {
    mica: "options.backdrop.mica",
    mica_alt: "options.backdrop.mica_alt",
    acrylic: "options.backdrop.acrylic",
    blur: "options.backdrop.blur",
    opacity_hint: "options.appearance.backdrop_opacity_hint",
};

/// The NSVisualEffectView materials the choices become on macOS (window
/// background, under-window background, popover, full-screen UI).
pub const MAC_BACKDROP_NAMES: BackdropNames = BackdropNames {
    mica: "options.backdrop.mac.window",
    mica_alt: "options.backdrop.mac.under_window",
    acrylic: "options.backdrop.mac.popover",
    blur: "options.backdrop.mac.full_screen",
    opacity_hint: "options.appearance.backdrop_opacity_hint_mac",
};

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
        assert_eq!(
            SHELL.modifier_names.meta,
            if mac {
                "Cmd"
            } else if cfg!(windows) {
                "Win"
            } else {
                "Super"
            }
        );
        assert_eq!(SHELL.modifier_names.alt, if mac { "Option" } else { "Alt" });
        assert_eq!(SHELL.backdrop_names, if mac { MAC_BACKDROP_NAMES } else { WINDOWS_BACKDROP_NAMES });
    }

    #[test]
    fn backdrop_names_are_interface_strings() {
        for names in [WINDOWS_BACKDROP_NAMES, MAC_BACKDROP_NAMES] {
            for key in [names.mica, names.mica_alt, names.acrylic, names.blur, names.opacity_hint] {
                assert!(glossa_core::locale::STRINGS.iter().any(|(k, _, _)| *k == key), "{key} is not in the locale");
            }
        }
    }
}
