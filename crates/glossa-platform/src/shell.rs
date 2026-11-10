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
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// What turning it on did, said next to the checkbox, by its locale key:
    /// where the system lists the entry and how it can be switched off there.
    pub autostart_note: &'static str,
    /// A link that opens the system's own list of programs starting at
    /// login, where the user can allow or stop Glossa (macOS: Login Items).
    pub login_items_url: Option<&'static str>,
    /// What the backdrops are called here, by their locale keys: Windows'
    /// own names (Mica, Acrylic…) there and on Linux, the vibrancy
    /// materials they become on macOS.
    pub backdrop_names: BackdropNames,
    /// What the modifier keys are called on this system's keyboards, for
    /// showing a hotkey (the settings file keeps `Ctrl`, `Alt`, `Shift`,
    /// `Meta` everywhere).
    pub modifier_names: ModifierNames,
    /// How this system's keyboards are laid out and labelled, for the
    /// on-screen keyboard.
    pub keyboard: KeyboardKeys,
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
    /// A dialog's default button stands last, at the right end of the row
    /// (macOS: Cancel, then OK). Elsewhere it comes first (OK, Cancel,
    /// Apply), as Windows lays its dialogs out.
    pub default_button_last: bool,
    /// The suffix of the interface strings worded for this system: a key
    /// `main.tray_hint` has a variant `main.tray_hint_mac` where the text
    /// differs on macOS (the menu bar instead of the tray). Strings without
    /// such a variant are the same everywhere.
    pub string_variant: &'static str,
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

/// The keys of a keyboard that do not type a character: what this system's
/// keyboards print on them (by locale key) and the order of the bottom row.
/// The character keys are the same everywhere; a language's layout decides
/// what they type. See [`Shell::keyboard`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyboardKeys {
    pub backspace: &'static str,
    pub enter: &'static str,
    pub tab: &'static str,
    pub caps: &'static str,
    pub shift: &'static str,
    /// The bottom row, left to right, each key with its width in key units
    /// (one letter key is one unit). The widths add up to 15, the width of
    /// the rows above.
    pub bottom: &'static [(BottomKey, f32)],
}

/// A key of the bottom row. See [`KeyboardKeys::bottom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomKey {
    Space,
    /// A modifier or other key that types nothing, by the locale key of its
    /// label (Ctrl, Alt, Win; Fn, Control, Option, Cmd; Super).
    Named(&'static str),
    /// The context menu key, drawn as a little page rather than named.
    Menu,
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

/// A PC keyboard as Windows labels it: Ctrl, Win, Alt, the space bar, then
/// Alt, Win, the menu key and Ctrl.
pub const WINDOWS_KEYBOARD: KeyboardKeys = KeyboardKeys {
    bottom: &[
        (BottomKey::Named("keyboard.ctrl"), 1.25),
        (BottomKey::Named("keyboard.win"), 1.25),
        (BottomKey::Named("keyboard.alt"), 1.25),
        (BottomKey::Space, 6.25),
        (BottomKey::Named("keyboard.alt"), 1.25),
        (BottomKey::Named("keyboard.win"), 1.25),
        (BottomKey::Menu, 1.25),
        (BottomKey::Named("keyboard.ctrl"), 1.25),
    ],
    ..PC_KEYS
};

/// The same PC keyboard on Linux, whose desktops call the Windows key Super.
pub const LINUX_KEYBOARD: KeyboardKeys = KeyboardKeys {
    bottom: &[
        (BottomKey::Named("keyboard.ctrl"), 1.25),
        (BottomKey::Named("keyboard.super"), 1.25),
        (BottomKey::Named("keyboard.alt"), 1.25),
        (BottomKey::Space, 6.25),
        (BottomKey::Named("keyboard.alt"), 1.25),
        (BottomKey::Named("keyboard.super"), 1.25),
        (BottomKey::Menu, 1.25),
        (BottomKey::Named("keyboard.ctrl"), 1.25),
    ],
    ..PC_KEYS
};

/// A Mac keyboard: Delete and Return, and fn, Control, Option, Command
/// around the space bar, then Command and Option (where a Mac has its arrow
/// keys, which the on-screen keyboard leaves out).
pub const MAC_KEYBOARD: KeyboardKeys = KeyboardKeys {
    backspace: "keyboard.mac.delete",
    enter: "keyboard.mac.return",
    tab: "keyboard.tab",
    caps: "keyboard.caps",
    shift: "keyboard.shift",
    bottom: &[
        (BottomKey::Named("keyboard.mac.fn"), 1.25),
        (BottomKey::Named("keyboard.mac.control"), 1.25),
        (BottomKey::Named("keyboard.mac.option"), 1.5),
        (BottomKey::Named("keyboard.mac.cmd"), 1.75),
        (BottomKey::Space, 6.0),
        (BottomKey::Named("keyboard.mac.cmd"), 1.75),
        (BottomKey::Named("keyboard.mac.option"), 1.5),
    ],
};

/// The PC keyboards' names for the keys above the bottom row.
const PC_KEYS: KeyboardKeys = KeyboardKeys {
    backspace: "keyboard.backspace",
    enter: "keyboard.enter",
    tab: "keyboard.tab",
    caps: "keyboard.caps",
    shift: "keyboard.shift",
    bottom: &[],
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
        assert_eq!(SHELL.login_items_url.is_some(), mac);
        assert_eq!(SHELL.default_button_last, mac);
        assert_eq!(
            SHELL.string_variant,
            if mac {
                "mac"
            } else if cfg!(windows) {
                "win"
            } else {
                "linux"
            }
        );
    }

    #[test]
    fn keyboard_rows_are_fifteen_units_and_named_in_the_locale() {
        let known = |key: &str| glossa_core::locale::STRINGS.iter().any(|(k, _, _)| *k == key);
        for (name, kb) in [("Windows", WINDOWS_KEYBOARD), ("Mac", MAC_KEYBOARD), ("Linux", LINUX_KEYBOARD)] {
            let width: f32 = kb.bottom.iter().map(|(_, w)| w).sum();
            assert!((width - 15.0).abs() < 1e-4, "{name}: the bottom row is {width} units");
            assert_eq!(kb.bottom.iter().filter(|(k, _)| *k == BottomKey::Space).count(), 1, "{name}");
            let named = kb.bottom.iter().filter_map(|(k, _)| match k {
                BottomKey::Named(n) => Some(*n),
                _ => None,
            });
            for key in [kb.backspace, kb.enter, kb.tab, kb.caps, kb.shift].into_iter().chain(named) {
                assert!(known(key), "{name}: {key} is not in the locale");
            }
        }
        assert!(known(SHELL.autostart_note));
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
