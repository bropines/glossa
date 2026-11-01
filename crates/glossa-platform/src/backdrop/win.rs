//! Backdrops on Windows: DWM system backdrops (Windows 11) and the
//! accent policy (Windows 10 acrylic and blur), and the window frame.

use std::ffi::c_void;

use windows::core::{s, w, BOOL};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

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
        let mut data = CompositionData { attribute, data: (value as *mut T).cast(), size: std::mem::size_of::<T>() };
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
            set_accent(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, super::accent_tint_at(options.dark, options.tint))
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
