//! System window materials: Mica, Acrylic and blur on Windows, vibrancy on
//! macOS, blur-behind under KWin on X11.
//!
//! [`apply`] asks the window system to draw a [`Backdrop`] behind a window.
//! The material only shows where the window paints nothing or something
//! translucent, so the window must be created transparent (winit's
//! `with_transparent(true)`, which on Windows turns on DWM blur-behind with
//! an empty region: that is what lets per-pixel alpha through) and draw its
//! surfaces with alpha (`Theme::backdrop_fill`). Extending the DWM frame
//! into the client area is not needed for such a window, and on an
//! undecorated one it makes DWM draw ghost caption buttons, so it is not
//! done.
//!
//! What each request becomes on Windows ([`plan`]; checked on build 26220):
//!
//! | asked for | 11 22H2+ (22523+) | 11 21H2 (22000+) | 10 1803+ (17134+) | 10 before 1803 |
//! |---|---|---|---|---|
//! | Mica     | `DWMSBT_MAINWINDOW`      | `DWMWA_MICA_EFFECT` | accent acrylic | accent blur |
//! | Mica Alt | `DWMSBT_TABBEDWINDOW`    | `DWMWA_MICA_EFFECT` | accent acrylic | accent blur |
//! | Acrylic  | `DWMSBT_TRANSIENTWINDOW` | accent acrylic      | accent acrylic | accent blur |
//! | Blur     | accent blur              | accent blur         | accent blur    | accent blur |
//!
//! "Accent" is the undocumented `SetWindowCompositionAttribute` accent
//! policy (`ACCENT_ENABLE_ACRYLICBLURBEHIND`, `ACCENT_ENABLE_BLURBEHIND`)
//! that Windows 10 offered before system backdrops, and that Windows 11
//! still honours. Accent acrylic is known to make dragging and resizing lag
//! on Windows 10 1903 and later.
//!
//! **Light and dark.** The materials' tint follows the window's dark-mode
//! attribute, which [`apply`] sets from `dark`. winit also sets it, to the
//! *system* theme, when it creates a window and whenever the system theme
//! changes; so a light theme on a dark system (or the reverse) needs the
//! window's theme set to match as well (`egui::ViewportCommand::SetTheme`),
//! or the window flips back to the system's tint. A decorated window's
//! system caption text may still follow the system theme; Glossa's custom
//! frame draws its own caption.
//!
//! **Activation.** Windows draws Mica and the system backdrops only while
//! the window is active; an inactive one shows a solid colour (the way
//! Explorer's windows do). The accent policy does not care. A window that
//! is shown without taking focus, such as the selection popup, should
//! therefore be applied with [`Options::unfocused`], which turns Mica, Mica
//! Alt and Acrylic into accent acrylic on every Windows build.
//!
//! On macOS the material is an `NSVisualEffectView` behind the content
//! (window background, under-window background, popover and full-screen UI
//! materials respectively), kept active when the window is not key. On
//! Linux, KWin under X11 blurs behind any kind; elsewhere [`apply`] reports
//! an error and changes nothing.
//!
//! Every function compiles everywhere.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub use glossa_core::config::Backdrop;

/// What [`apply`] actually did, so the caller can tell a real blur from a
/// solid fallback (and shade a popup when there is none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Nothing asked for; any earlier material was removed.
    Cleared,
    /// `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2+). Solid while the
    /// window is inactive.
    SystemBackdrop,
    /// The pre-release `DWMWA_MICA_EFFECT` attribute (Windows 11 21H2).
    LegacyMica,
    /// `SetWindowCompositionAttribute` with acrylic blur (Windows 10/11).
    AccentAcrylic,
    /// `SetWindowCompositionAttribute` with plain blur (Windows 10/11).
    AccentBlur,
    /// An `NSVisualEffectView` behind the content (macOS).
    Vibrancy,
    /// `_KDE_NET_WM_BLUR_BEHIND_REGION` (KWin on X11).
    KdeBlur,
}

impl Applied {
    /// Whether something now blurs what is behind the window.
    pub fn blurs(self) -> bool {
        self != Applied::Cleared
    }
}

/// How to apply a backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// The dark or light variant of the material, and on Windows the dark
    /// title bar.
    pub dark: bool,
    /// The window is shown without being activated (a popup, a tooltip):
    /// prefer materials that render while it is inactive. See the module
    /// docs.
    pub unfocused: bool,
    /// 0 to 100 percent of the tint the accent policy's acrylic is drawn
    /// with ([`accent_tint`]): lower is more see-through. The system
    /// backdrops (Mica, Windows 11 Acrylic) tint themselves and ignore it.
    pub tint: u8,
}

impl Options {
    pub fn new(dark: bool) -> Self {
        Self { dark, unfocused: false, tint: 100 }
    }

    pub fn unfocused(dark: bool) -> Self {
        Self { dark, unfocused: true, tint: 100 }
    }

    /// The same, with the acrylic tint at `percent`.
    pub fn with_tint(self, percent: u8) -> Self {
        Self { tint: percent.min(100), ..self }
    }
}

/// Apply `kind` to a window, given anything with a raw window handle:
/// `eframe::Frame`, `eframe::CreationContext`, a winit window. `dark` picks
/// the dark or light variant of the material (and, on Windows, the dark
/// title bar).
///
/// Call it once the window exists (eframe's creation context or first
/// frame) and again whenever the setting or the theme's darkness changes;
/// applying is idempotent, and [`Backdrop::None`] undoes it.
pub fn apply(window: &impl HasWindowHandle, kind: Backdrop, dark: bool) -> Result<Applied, String> {
    apply_with(window, kind, Options::new(dark))
}

/// [`apply`] with [`Options`].
pub fn apply_with(window: &impl HasWindowHandle, kind: Backdrop, options: Options) -> Result<Applied, String> {
    let handle = window.window_handle().map_err(|e| format!("no window handle: {e}"))?;
    apply_raw(handle.as_raw(), kind, options)
}

/// [`apply_with`] for a raw handle.
pub fn apply_raw(handle: RawWindowHandle, kind: Backdrop, options: Options) -> Result<Applied, String> {
    match handle {
        RawWindowHandle::Win32(h) => apply_hwnd_with(h.hwnd.get(), kind, options),
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(h) => mac::apply(h.ns_view, kind, options.dark),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xlib(h) => x11::apply(h.window as u32, kind),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(h) => x11::apply(h.window.get(), kind),
        other => {
            if kind.is_some() {
                Err(format!("window backdrops are not supported for this kind of window ({other:?})"))
            } else {
                Ok(Applied::Cleared)
            }
        }
    }
}

/// [`apply`] for a Win32 window handle (`HWND` as an integer), for windows
/// whose raw handle the toolkit does not expose, such as egui's immediate
/// viewports (find them with [`find_own_window`]). On other systems this
/// returns an error for anything but [`Backdrop::None`].
pub fn apply_hwnd(hwnd: isize, kind: Backdrop, dark: bool) -> Result<Applied, String> {
    apply_hwnd_with(hwnd, kind, Options::new(dark))
}

/// [`apply_hwnd`] with [`Options`].
pub fn apply_hwnd_with(hwnd: isize, kind: Backdrop, options: Options) -> Result<Applied, String> {
    #[cfg(windows)]
    {
        win::apply(hwnd, kind, options)
    }
    // On macOS the "HWND" is the NSWindow that [`find_own_window`] found.
    #[cfg(target_os = "macos")]
    {
        mac::apply_window(hwnd, kind, options.dark)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (hwnd, options);
        if kind.is_some() {
            Err("window backdrops by HWND exist only on Windows".into())
        } else {
            Ok(Applied::Cleared)
        }
    }
}

/// A visible top-level window of this process with exactly this title, as
/// a Win32 `HWND`. egui's immediate viewports (the popup, the options
/// window) do not hand out their raw handles, but their titles are ours;
/// give each a title no other window of the process has. Always `None` off
/// Windows.
pub fn find_own_window(title: &str) -> Option<isize> {
    crate::window::own_window_by_title(title)
}

/// Whether the system rounds an undecorated window's corners and draws its
/// border once [`style_frame`] has been applied: Windows 11. Elsewhere an
/// app drawing its own frame draws the border too.
pub fn frame_drawn_by_system() -> bool {
    windows_build().is_some_and(|b| b >= BUILD_WINDOWS_11)
}

/// Give a window that draws its own title bar the outline a Windows 11
/// window has: rounded corners (`DWMWA_WINDOW_CORNER_PREFERENCE`, which an
/// undecorated window does not get by default) and a one-pixel border in
/// `border` (`DWMWA_BORDER_COLOR`; `None` keeps the system's colour).
/// Harmless before Windows 11 and a no-op off Windows.
pub fn style_frame(hwnd: isize, border: Option<(u8, u8, u8)>) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::style_frame(hwnd, border)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, border);
        Ok(())
    }
}

/// [`style_frame`] for anything with a raw window handle (`eframe::Frame`).
pub fn style_frame_of(window: &impl HasWindowHandle, border: Option<(u8, u8, u8)>) -> Result<(), String> {
    let handle = window.window_handle().map_err(|e| format!("no window handle: {e}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => style_frame(h.hwnd.get(), border),
        _ => Ok(()),
    }
}

/// How a request is carried out on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinMethod {
    Clear,
    /// `DWMWA_SYSTEMBACKDROP_TYPE` with this `DWM_SYSTEMBACKDROP_TYPE`.
    SystemBackdrop(i32),
    LegacyMica,
    AccentAcrylic,
    AccentBlur,
    Unsupported,
}

/// First build with `DWMWA_SYSTEMBACKDROP_TYPE` (22H2 Insider; 22621 shipped).
pub const BUILD_SYSTEM_BACKDROP: u32 = 22523;
/// Windows 11 21H2, with the undocumented `DWMWA_MICA_EFFECT`.
pub const BUILD_WINDOWS_11: u32 = 22000;
/// Windows 10 1803, the first with the acrylic accent policy.
pub const BUILD_ACCENT_ACRYLIC: u32 = 17134;
/// Windows 10 1507.
pub const BUILD_WINDOWS_10: u32 = 10240;

/// What [`apply`] does for `kind` on Windows build `build`. Pure, so the
/// table in the module docs is tested rather than promised.
pub fn plan(kind: Backdrop, build: u32, unfocused: bool) -> WinMethod {
    if kind == Backdrop::None {
        return WinMethod::Clear;
    }
    if build < BUILD_WINDOWS_10 {
        return WinMethod::Unsupported;
    }
    let tinted = kind.is_tinted();
    if kind == Backdrop::Blur || (tinted && build < BUILD_ACCENT_ACRYLIC) {
        return WinMethod::AccentBlur;
    }
    if unfocused {
        return WinMethod::AccentAcrylic;
    }
    match kind {
        Backdrop::Mica if build >= BUILD_SYSTEM_BACKDROP => WinMethod::SystemBackdrop(2),
        Backdrop::Acrylic if build >= BUILD_SYSTEM_BACKDROP => WinMethod::SystemBackdrop(3),
        Backdrop::MicaAlt if build >= BUILD_SYSTEM_BACKDROP => WinMethod::SystemBackdrop(4),
        Backdrop::Mica | Backdrop::MicaAlt if build >= BUILD_WINDOWS_11 => WinMethod::LegacyMica,
        _ => WinMethod::AccentAcrylic,
    }
}

/// The running Windows build number (`26100` and so on), `None` elsewhere.
pub fn windows_build() -> Option<u32> {
    #[cfg(windows)]
    {
        win::build()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The tint the accent policy draws under acrylic, as `0xAABBGGRR`. The
/// system backdrops tint themselves; this one has to be told. The colours
/// are WinUI's base backgrounds, at 60 %.
pub fn accent_tint(dark: bool) -> u32 {
    accent_tint_at(dark, 100)
}

/// [`accent_tint`] with its alpha scaled to `percent` (the way Windhawk's
/// "Translucent Windows" mod exposes the same accent policy's opacity).
pub fn accent_tint_at(dark: bool, percent: u8) -> u32 {
    let rgb = if dark { 0x20u32 } else { 0xf3u32 };
    let alpha = (0x99 as f32 * percent.min(100) as f32 / 100.0).round() as u32;
    (alpha << 24) | (rgb << 16) | (rgb << 8) | rgb
}

#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "linux")]
mod x11;

#[cfg(test)]
mod tests {
    use super::*;

    const W11_24H2: u32 = 26100;
    const W11_21H2: u32 = 22000;
    const W10_22H2: u32 = 19045;
    const W10_1709: u32 = 16299;

    #[test]
    fn windows_plan_follows_the_build() {
        let p = |kind, build| plan(kind, build, false);
        assert_eq!(p(Backdrop::None, W11_24H2), WinMethod::Clear);
        assert_eq!(p(Backdrop::Mica, W11_24H2), WinMethod::SystemBackdrop(2));
        assert_eq!(p(Backdrop::Acrylic, W11_24H2), WinMethod::SystemBackdrop(3));
        assert_eq!(p(Backdrop::MicaAlt, W11_24H2), WinMethod::SystemBackdrop(4));
        assert_eq!(p(Backdrop::Blur, W11_24H2), WinMethod::AccentBlur);

        assert_eq!(p(Backdrop::Mica, W11_21H2), WinMethod::LegacyMica);
        assert_eq!(p(Backdrop::MicaAlt, W11_21H2), WinMethod::LegacyMica);
        assert_eq!(p(Backdrop::Acrylic, W11_21H2), WinMethod::AccentAcrylic);

        assert_eq!(p(Backdrop::Mica, W10_22H2), WinMethod::AccentAcrylic);
        assert_eq!(p(Backdrop::Acrylic, W10_22H2), WinMethod::AccentAcrylic);
        assert_eq!(p(Backdrop::Blur, W10_22H2), WinMethod::AccentBlur);

        assert_eq!(p(Backdrop::Acrylic, W10_1709), WinMethod::AccentBlur);
        assert_eq!(p(Backdrop::Mica, 9600), WinMethod::Unsupported);
        assert_eq!(p(Backdrop::None, 9600), WinMethod::Clear);
    }

    #[test]
    fn unfocused_windows_get_a_material_that_survives_inactivity() {
        for kind in [Backdrop::Mica, Backdrop::MicaAlt, Backdrop::Acrylic] {
            assert_eq!(plan(kind, W11_24H2, true), WinMethod::AccentAcrylic, "{kind}");
            assert_eq!(plan(kind, W11_21H2, true), WinMethod::AccentAcrylic, "{kind}");
        }
        assert_eq!(plan(Backdrop::Blur, W11_24H2, true), WinMethod::AccentBlur);
        assert_eq!(plan(Backdrop::Acrylic, W10_1709, true), WinMethod::AccentBlur);
        assert_eq!(plan(Backdrop::None, W11_24H2, true), WinMethod::Clear);
    }

    #[test]
    fn accent_tint_is_abgr_with_alpha() {
        assert_eq!(accent_tint(true), 0x99202020);
        assert_eq!(accent_tint(false), 0x99f3f3f3);
        assert_eq!(accent_tint_at(true, 0), 0x00202020);
        assert_eq!(accent_tint_at(true, 50), 0x4d202020);
        assert_eq!(accent_tint_at(false, 200), 0x99f3f3f3);
    }

    #[test]
    fn nothing_to_clear_is_never_an_error() {
        // Off Windows there is no HWND; clearing still succeeds, so callers
        // can apply the setting unconditionally.
        if !cfg!(windows) {
            assert_eq!(apply_hwnd(1, Backdrop::None, true), Ok(Applied::Cleared));
            assert!(apply_hwnd(1, Backdrop::Mica, true).is_err());
            assert_eq!(find_own_window("Glossa"), None);
            assert_eq!(windows_build(), None);
        }
        assert!(Applied::AccentBlur.blurs() && !Applied::Cleared.blurs());
        assert_eq!(Options::unfocused(true), Options { dark: true, unfocused: true, tint: 100 });
        assert_eq!(Options::new(false).with_tint(130).tint, 100);
    }
}
