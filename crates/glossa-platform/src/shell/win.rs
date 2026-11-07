//! Windows: the desktop in physical pixels, windows resized from their
//! top-left corner, Glossa's own title bar, DWM backdrops under both
//! renderers (wgpu presents through DirectComposition for them).

use super::Shell;

pub const SHELL: Shell = Shell {
    name: "Windows",
    desktop_in_points: false,
    resize_keeps_bottom_left: false,
    own_frame: true,
    transparency_needs_alpha_root: false,
    slow_child_windows: false,
    can_warp_pointer: true,
    backdrops_with_gl: true,
    backdrops_with_gpu: true,
    command_key: "Ctrl",
    command_is_ctrl: true,
    app_menu: false,
    switcher_follows_windows: false,
    portable_beside_exe: true,
    autostart_label: "options.basics.start_with_system",
    backdrop_names: super::WINDOWS_BACKDROP_NAMES,
    modifier_names: super::ModifierNames { ctrl: "Ctrl", alt: "Alt", shift: "Shift", meta: "Win" },
    window_classes: true,
    program_example: "program.exe",
    tray_clicks: true,
};

pub fn open(target: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", target])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}
