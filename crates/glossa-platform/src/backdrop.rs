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
}

impl Options {
    pub fn new(dark: bool) -> Self {
        Self { dark, unfocused: false }
    }

    pub fn unfocused(dark: bool) -> Self {
        Self { dark, unfocused: true }
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
    #[cfg(not(windows))]
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
    #[cfg(windows)]
    {
        win::find_own_window(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        None
    }
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
    let (rgb, alpha) = if dark { (0x20u32, 0x99u32) } else { (0xf3u32, 0x99u32) };
    (alpha << 24) | (rgb << 16) | (rgb << 8) | rgb
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use windows::core::{s, w, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    use super::{plan, Applied, Backdrop, Options, WinMethod, BUILD_SYSTEM_BACKDROP, BUILD_WINDOWS_11};

    /// Undocumented: Mica on Windows 11 21H2, before system backdrops.
    const DWMWA_MICA_EFFECT: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(1029);
    /// The dark-mode attribute's number before Windows 10 20H1.
    const DWMWA_USE_IMMERSIVE_DARK_MODE_OLD: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(19);
    const DWMSBT_AUTO: i32 = 0;
    /// `DWMWA_BORDER_COLOR` (Windows 11) and the value that restores the
    /// system's border colour.
    const DWMWA_BORDER_COLOR: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(34);
    const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        /// Left at 0: with 2 ("use the gradient colour"), which some
        /// programs pass, plain blur came out opaque black on build 26220.
        flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }

    #[repr(C)]
    struct CompositionData {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }

    type SetWindowCompositionAttribute = unsafe extern "system" fn(HWND, *mut CompositionData) -> BOOL;

    pub fn build() -> Option<u32> {
        #[repr(C)]
        struct OsVersionInfo {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }
        type RtlGetVersion = unsafe extern "system" fn(*mut OsVersionInfo) -> i32;
        // RtlGetVersion, unlike GetVersionEx, is not lied to by the
        // compatibility shims when the manifest lacks a supportedOS entry.
        // SAFETY: ntdll is always loaded; the transmuted signature is the
        // documented one; the struct is sized for the call.
        unsafe {
            let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
            let f = GetProcAddress(ntdll, s!("RtlGetVersion"))?;
            let f: RtlGetVersion = std::mem::transmute(f);
            let mut info: OsVersionInfo = std::mem::zeroed();
            info.size = std::mem::size_of::<OsVersionInfo>() as u32;
            (f(&mut info) == 0).then_some(info.build)
        }
    }

    fn set_attr<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, value: &T) -> windows::core::Result<()> {
        // SAFETY: `value` is a live `T` and its size is passed with it.
        unsafe { DwmSetWindowAttribute(hwnd, attr, (value as *const T).cast(), std::mem::size_of::<T>() as u32) }
    }

    /// Set a window composition attribute through the undocumented
    /// `SetWindowCompositionAttribute`.
    fn set_composition<T>(hwnd: HWND, attribute: u32, value: &mut T) -> Result<(), String> {
        // SAFETY: user32 is loaded in every GUI process; the function is
        // looked up by name and given the layout every caller since Windows
        // 10 1507 has used; `value` outlives the call and its size is passed.
        unsafe {
            let user32 = GetModuleHandleW(w!("user32.dll")).map_err(|e| format!("user32: {e}"))?;
            let f = GetProcAddress(user32, s!("SetWindowCompositionAttribute"))
                .ok_or("SetWindowCompositionAttribute is missing")?;
            let f: SetWindowCompositionAttribute = std::mem::transmute(f);
            let mut data =
                CompositionData { attribute, data: (value as *mut T).cast(), size: std::mem::size_of::<T>() };
            if f(hwnd, &mut data).as_bool() {
                Ok(())
            } else {
                Err(format!("SetWindowCompositionAttribute refused attribute {attribute}"))
            }
        }
    }

    fn set_accent(hwnd: HWND, state: u32, tint: u32) -> Result<(), String> {
        let mut policy = AccentPolicy {
            state,
            flags: 0,
            // Acrylic with a fully transparent tint is drawn wrongly.
            gradient_color: if state == ACCENT_ENABLE_ACRYLICBLURBEHIND { tint.max(0x0100_0000) } else { tint },
            animation_id: 0,
        };
        set_composition(hwnd, WCA_ACCENT_POLICY, &mut policy)
    }

    pub fn apply(hwnd: isize, kind: Backdrop, options: Options) -> Result<Applied, String> {
        if hwnd == 0 {
            return Err("no window".into());
        }
        let hwnd = HWND(hwnd as *mut c_void);
        let build = build().unwrap_or(0);
        let method = plan(kind, build, options.unfocused);
        tracing::debug!("backdrop {kind} on build {build}: {method:?}");

        // The title bar and the materials follow the theme's darkness;
        // harmless on builds that ignore it.
        // The title bar and the materials follow the theme's darkness. winit
        // sets the same thing to the system theme (when the window is created
        // and when the system theme changes), so the window's toolkit theme
        // must be set to match too, or Mica flips back; see the module docs.
        let dark = BOOL::from(options.dark);
        if set_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark).is_err() {
            let _ = set_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE_OLD, &dark);
        }

        // Undo whatever another kind left behind, so switching works.
        if build >= BUILD_SYSTEM_BACKDROP && !matches!(method, WinMethod::SystemBackdrop(_)) {
            let _ = set_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_AUTO);
        }
        if (BUILD_WINDOWS_11..BUILD_SYSTEM_BACKDROP).contains(&build) && method != WinMethod::LegacyMica {
            let _ = set_attr(hwnd, DWMWA_MICA_EFFECT, &BOOL::from(false));
        }
        if !matches!(method, WinMethod::AccentAcrylic | WinMethod::AccentBlur) {
            let _ = set_accent(hwnd, ACCENT_DISABLED, 0);
        }

        // Rounded corners, as WinUI windows have; an undecorated window
        // (Glossa's custom frame, the popup) is square otherwise. Clearing
        // the backdrop leaves the corners alone: a window drawing its own
        // frame keeps them round (`style_frame`).
        if build >= BUILD_WINDOWS_11 && kind.is_some() {
            let _ = set_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
        }

        match method {
            WinMethod::Clear => Ok(Applied::Cleared),
            WinMethod::SystemBackdrop(value) => set_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &value)
                .map(|_| Applied::SystemBackdrop)
                .map_err(|e| format!("DWMWA_SYSTEMBACKDROP_TYPE: {e}")),
            WinMethod::LegacyMica => set_attr(hwnd, DWMWA_MICA_EFFECT, &BOOL::from(true))
                .map(|_| Applied::LegacyMica)
                .map_err(|e| format!("DWMWA_MICA_EFFECT: {e}")),
            WinMethod::AccentAcrylic => {
                set_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, super::accent_tint(options.dark))
                    .map(|_| Applied::AccentAcrylic)
            }
            WinMethod::AccentBlur => set_accent(hwnd, ACCENT_ENABLE_BLURBEHIND, 0).map(|_| Applied::AccentBlur),
            WinMethod::Unsupported => Err(format!("window backdrops need Windows 10 or later (this is build {build})")),
        }
    }

    pub fn style_frame(hwnd: isize, border: Option<(u8, u8, u8)>) -> Result<(), String> {
        if hwnd == 0 {
            return Err("no window".into());
        }
        if build().unwrap_or(0) < BUILD_WINDOWS_11 {
            return Ok(());
        }
        let hwnd = HWND(hwnd as *mut c_void);
        set_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND)
            .map_err(|e| format!("DWMWA_WINDOW_CORNER_PREFERENCE: {e}"))?;
        // A COLORREF is 0x00BBGGRR; DWMWA_COLOR_DEFAULT puts the system's back.
        let colour: u32 = match border {
            Some((r, g, b)) => (b as u32) << 16 | (g as u32) << 8 | r as u32,
            None => DWMWA_COLOR_DEFAULT,
        };
        set_attr(hwnd, DWMWA_BORDER_COLOR, &colour).map_err(|e| format!("DWMWA_BORDER_COLOR: {e}"))
    }

    struct Search {
        pid: u32,
        title: Vec<u16>,
        found: Option<isize>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` passed to EnumWindows below,
        // alive for the whole enumeration.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: EnumWindows hands us valid top-level window handles.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != search.pid || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return BOOL::from(true);
        }
        // SAFETY: as above; the buffer is sized from the reported length.
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len as usize != search.title.len() {
            return BOOL::from(true);
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if got as usize == search.title.len() && buf[..got as usize] == search.title[..] {
            search.found = Some(hwnd.0 as isize);
            return BOOL::from(false);
        }
        BOOL::from(true)
    }

    pub fn find_own_window(title: &str) -> Option<isize> {
        let mut search = Search {
            // SAFETY: no preconditions.
            pid: unsafe { GetCurrentProcessId() },
            title: title.encode_utf16().collect(),
            found: None,
        };
        // SAFETY: `visit` only reads `search` through the pointer while
        // EnumWindows runs; it stops early (an "error" result) once found.
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
        search.found
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSUserInterfaceItemIdentification, NSView, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
    };
    use objc2_foundation::NSString;

    use super::{Applied, Backdrop};

    const IDENTIFIER: &str = "glossa.backdrop";

    /// The effect view sits in the content view's superview (the window's
    /// frame view), just below the content view: the GL or Metal surface is
    /// the content view's own layer, so a subview of it would cover what we
    /// draw.
    pub fn apply(ns_view: NonNull<c_void>, kind: Backdrop, dark: bool) -> Result<Applied, String> {
        let mtm = MainThreadMarker::new().ok_or("vibrancy must be applied on the main thread")?;
        // SAFETY: an AppKit raw handle's `ns_view` is a live NSView, and we
        // are on the main thread, where AppKit objects may be touched.
        let content: &NSView = unsafe { ns_view.cast::<NSView>().as_ref() };
        let Some(parent) = (unsafe { content.superview() }) else {
            return Err("the window's content view has no superview".into());
        };
        let identifier = NSString::from_str(IDENTIFIER);
        let existing: Option<Retained<NSView>> =
            parent.subviews().iter().find(|v| v.identifier().is_some_and(|id| id.isEqualToString(&identifier)));
        if let Some(old) = existing {
            old.removeFromSuperview();
        }
        let material = match kind {
            Backdrop::None => return Ok(Applied::Cleared),
            Backdrop::Mica => NSVisualEffectMaterial::WindowBackground,
            Backdrop::MicaAlt => NSVisualEffectMaterial::UnderWindowBackground,
            Backdrop::Acrylic => NSVisualEffectMaterial::Popover,
            Backdrop::Blur => NSVisualEffectMaterial::FullScreenUI,
        };
        let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), content.frame());
        effect.setMaterial(material);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        // Stay vivid when the window is not key: the popup never is.
        effect.setState(NSVisualEffectState::Active);
        effect.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        effect.setIdentifier(Some(&identifier));
        // SAFETY: the appearance names are AppKit's own constants.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        if let Some(appearance) = NSAppearance::appearanceNamed(name) {
            effect.setAppearance(Some(&appearance));
        }
        parent.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, Some(content));
        Ok(Applied::Vibrancy)
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    use super::{Applied, Backdrop};

    /// KWin blurs whatever is behind a window's translucent pixels when the
    /// window carries this property; an empty region means "all of it".
    /// Other window managers ignore it, and since KWin creates the atom,
    /// its absence means nobody is listening.
    pub fn apply(window: u32, kind: Backdrop) -> Result<Applied, String> {
        let (conn, _) = x11rb::connect(None).map_err(|e| format!("X11: {e}"))?;
        let atom = conn
            .intern_atom(true, b"_KDE_NET_WM_BLUR_BEHIND_REGION")
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        if kind == Backdrop::None {
            if atom != 0 {
                let _ = conn.delete_property(window, atom);
                let _ = conn.flush();
            }
            return Ok(Applied::Cleared);
        }
        if atom == 0 {
            return Err("this window manager offers no blur (KWin's blur-behind is not available)".into());
        }
        conn.change_property32(PropMode::REPLACE, window, atom, AtomEnum::CARDINAL, &[]).map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        Ok(Applied::KdeBlur)
    }
}

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
        assert_eq!(Options::unfocused(true), Options { dark: true, unfocused: true });
    }
}
