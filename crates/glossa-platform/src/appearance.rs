//! The system's look: accent colour and light or dark preference.
//!
//! | | Windows | macOS | Linux |
//! |---|---|---|---|
//! | accent | `HKCU\…\Explorer\Accent\AccentPalette` (all seven shades), else `HKCU\…\DWM\AccentColor` | `NSColor.controlAccentColor` | none |
//! | dark | `HKCU\…\Themes\Personalize\AppsUseLightTheme` | `AppleInterfaceStyle` | `gsettings` `color-scheme`, then the GTK theme's name |
//!
//! Each read is a registry value or a defaults key: cheap enough to call when
//! a theme loads, but not every frame.

use glossa_core::theme::SystemAccent;
use glossa_core::Color;

/// The system accent colour and its shades, or `None` where the system has
/// no such setting. Feed it to `Theme::resolve_system_accent`.
pub fn system_accent() -> Option<SystemAccent> {
    #[cfg(windows)]
    {
        win::accent()
    }
    #[cfg(target_os = "macos")]
    {
        mac::accent().map(SystemAccent::from_base)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Just the base accent colour.
pub fn system_accent_color() -> Option<Color> {
    system_accent().map(|a| a.base)
}

/// Whether the user chose dark mode for apps; `None` when it cannot be told.
pub fn system_prefers_dark() -> Option<bool> {
    #[cfg(windows)]
    {
        win::prefers_dark()
    }
    #[cfg(target_os = "macos")]
    {
        Some(mac::prefers_dark())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        linux::prefers_dark()
    }
}

/// Windows' `AccentPalette`: eight RGBA quadruples, lightest first (light 3,
/// light 2, light 1, base, dark 1, dark 2, dark 3, and one unused).
pub fn parse_accent_palette(bytes: &[u8]) -> Option<SystemAccent> {
    if bytes.len() < 28 {
        return None;
    }
    let at = |i: usize| Color::rgb(bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2]);
    Some(SystemAccent {
        light3: at(0),
        light2: at(1),
        light1: at(2),
        base: at(3),
        dark1: at(4),
        dark2: at(5),
        dark3: at(6),
    })
}

/// A DWM colour DWORD, `0xAABBGGRR`, as `AccentColor` stores it.
pub fn parse_abgr(value: u32) -> Color {
    Color::rgb((value & 0xff) as u8, ((value >> 8) & 0xff) as u8, ((value >> 16) & 0xff) as u8)
}

#[cfg(windows)]
mod win {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD};

    use super::{parse_abgr, parse_accent_palette};
    use glossa_core::theme::SystemAccent;

    fn read(
        key: PCWSTR,
        value: PCWSTR,
        flags: windows::Win32::System::Registry::REG_ROUTINE_FLAGS,
        buf: &mut [u8],
    ) -> Option<usize> {
        let mut len = buf.len() as u32;
        // SAFETY: `buf` and `len` describe a valid buffer; key and value
        // names are NUL-terminated constants.
        let rc = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key, value, flags, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
        };
        (rc == ERROR_SUCCESS).then_some(len as usize)
    }

    fn dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
        let mut buf = [0u8; 4];
        read(key, value, RRF_RT_REG_DWORD, &mut buf).map(|_| u32::from_le_bytes(buf))
    }

    pub fn accent() -> Option<SystemAccent> {
        let mut buf = [0u8; 32];
        if let Some(len) = read(
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            &mut buf,
        ) {
            if let Some(palette) = parse_accent_palette(&buf[..len]) {
                return Some(palette);
            }
        }
        dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))
            .map(|v| SystemAccent::from_base(parse_abgr(v)))
    }

    pub fn prefers_dark() -> Option<bool> {
        dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme"))
            .map(|light| light == 0)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use glossa_core::Color;
    use objc2_app_kit::{NSColor, NSColorSpace};
    use objc2_foundation::{NSString, NSUserDefaults};

    pub fn accent() -> Option<Color> {
        let accent = NSColor::controlAccentColor();
        let rgb = accent.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Color::rgb(byte(rgb.redComponent()), byte(rgb.greenComponent()), byte(rgb.blueComponent())))
    }

    pub fn prefers_dark() -> bool {
        let defaults = NSUserDefaults::standardUserDefaults();
        defaults
            .stringForKey(&NSString::from_str("AppleInterfaceStyle"))
            .is_some_and(|s| s.to_string().eq_ignore_ascii_case("dark"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod linux {
    use std::process::Command;

    fn gsettings(key: &str) -> Option<String> {
        let out = Command::new("gsettings").args(["get", "org.gnome.desktop.interface", key]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_owned())
    }

    /// The freedesktop colour-scheme setting as GNOME (and, through its
    /// GTK settings bridge, KDE) publish it, else a GTK theme named dark.
    pub fn prefers_dark() -> Option<bool> {
        if let Ok(theme) = std::env::var("GTK_THEME") {
            return Some(theme.to_ascii_lowercase().contains("dark"));
        }
        match gsettings("color-scheme").as_deref() {
            Some("prefer-dark") => return Some(true),
            Some("prefer-light") => return Some(false),
            _ => {}
        }
        gsettings("gtk-theme").map(|t| t.to_ascii_lowercase().contains("dark"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_palette_is_lightest_first() {
        // The default Windows 11 blue, as the registry holds it.
        let bytes: [u8; 32] = [
            0x99, 0xeb, 0xff, 0x00, 0x4c, 0xc2, 0xff, 0x00, 0x00, 0x91, 0xf8, 0x00, 0x00, 0x78, 0xd4, 0x00, 0x00, 0x67,
            0xc0, 0x00, 0x00, 0x3e, 0x92, 0x00, 0x00, 0x1a, 0x68, 0x00, 0xf7, 0x63, 0x0c, 0x00,
        ];
        let p = parse_accent_palette(&bytes).unwrap();
        assert_eq!(p, glossa_core::theme::builtin::WINDOWS_BLUE);
        assert!(parse_accent_palette(&bytes[..20]).is_none());
    }

    #[test]
    fn dwm_colours_are_abgr() {
        assert_eq!(parse_abgr(0xffd47800), Color::rgb(0x00, 0x78, 0xd4));
    }

    #[test]
    fn reading_the_system_never_panics() {
        let _ = system_accent();
        let _ = system_accent_color();
        let _ = system_prefers_dark();
    }
}
