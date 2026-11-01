//! Linux (X11 and Wayland) and other Unix systems: the desktop in
//! pixels, Glossa's own title bar, no system backdrops yet (KWin's blur is
//! applied to the main window only, through `backdrop`).

use super::Shell;

pub const SHELL: Shell = Shell {
    name: "Linux",
    desktop_in_points: false,
    resize_keeps_bottom_left: false,
    own_frame: true,
    transparency_needs_alpha_root: false,
    slow_child_windows: false,
    can_warp_pointer: true,
    backdrops_with_gl: false,
    backdrops_with_gpu: false,
    command_key: "Ctrl",
    command_is_ctrl: true,
    app_menu: false,
    switcher_follows_windows: false,
    portable_beside_exe: true,
    autostart_label: "options.basics.start_with_system_linux",
};

pub fn open(target: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
}
