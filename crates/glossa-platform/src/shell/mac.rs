//! macOS: the desktop in points, windows that keep their bottom-left
//! corner when resized (AppKit's coordinates start at the bottom), the
//! system's own title bar, a menu bar of Glossa's own, vibrancy only under
//! OpenGL (wgpu's Metal layer is opaque), and windows slow enough to make
//! that dropdowns stay inside theirs when they fit.

use super::Shell;

pub const SHELL: Shell = Shell {
    name: "macOS",
    desktop_in_points: true,
    resize_keeps_bottom_left: true,
    own_frame: false,
    transparency_needs_alpha_root: true,
    slow_child_windows: true,
    can_warp_pointer: false,
    backdrops_with_gl: true,
    backdrops_with_gpu: false,
    command_key: "Cmd",
    command_is_ctrl: false,
    app_menu: true,
    switcher_follows_windows: true,
    portable_beside_exe: false,
    autostart_label: "options.basics.start_with_system_macos",
    autostart_note: "options.basics.autostart_note_macos",
    login_items_url: Some("x-apple.systempreferences:com.apple.LoginItems-Settings.extension"),
    backdrop_names: super::MAC_BACKDROP_NAMES,
    modifier_names: super::ModifierNames { ctrl: "Ctrl", alt: "Option", shift: "Shift", meta: "Cmd" },
    keyboard: super::MAC_KEYBOARD,
    window_classes: false,
    program_example: "Safari",
    tray_clicks: true,
    default_button_last: true,
    string_variant: "mac",
};

pub fn open(target: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(target).spawn().map(|_| ())
}
