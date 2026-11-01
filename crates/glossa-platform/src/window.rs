//! Windows, monitors, the mouse cursor and the text caret — everything the
//! program needs to decide which rules apply to the window the user is in
//! (the exceptions list matches process and class names) and where to put
//! the popup.
//!
//! Coordinates are virtual-desktop pixels as the OS reports them. On Windows
//! that means physical pixels only when the process is per-monitor DPI aware
//! (the GUI should declare PerMonitorV2 in its manifest); otherwise Windows
//! virtualises them. On macOS they are points.

use glossa_core::ocr::Rect;

/// The window the user is working in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowInfo {
    /// The executable's file name, e.g. `notepad.exe`; what the exceptions
    /// list matches on. Empty when the process cannot be opened (an elevated
    /// process seen from a non-elevated one).
    pub process_name: String,
    /// The window class (`Notepad`, `Chrome_WidgetWin_1`; the `WM_CLASS`
    /// class on X11). Empty on macOS.
    pub class_name: String,
    /// The class of the control with the keyboard focus inside that window
    /// (`Edit`, `SysListView32`), where the platform tells; else empty.
    pub focus_class: String,
    pub title: String,
    pub pid: u32,
}

/// One display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The display scale (1.0 = 96 DPI on Windows, 2.0 = Retina).
    pub scale: f32,
    /// The part not covered by taskbars, panels and docks. Reported on
    /// Windows and X11 (`_NET_WORKAREA`); elsewhere the full bounds.
    pub work_area: Rect,
    pub is_primary: bool,
}

impl Monitor {
    /// The full bounds as a rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.width as f32, self.height as f32)
    }

    /// Whether the point lies on this monitor. Half-open, so a point on the
    /// seam between two monitors belongs to exactly one of them.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let b = self.bounds();
        x >= b.x && y >= b.y && x < b.x + b.width && y < b.y + b.height
    }
}

/// The foreground window, or `None` when there is none or the platform will
/// not tell (Wayland, by design).
///
/// On Windows, UWP apps report `ApplicationFrameHost.exe`, the frame process,
/// rather than the app itself.
pub fn foreground_window() -> Option<WindowInfo> {
    #[cfg(windows)]
    {
        win::foreground_window()
    }
    #[cfg(target_os = "linux")]
    {
        if crate::Session::detect().is_wayland() {
            return None;
        }
        crate::screen::x11::active_window()
    }
    #[cfg(target_os = "macos")]
    {
        // xcap enumerates the on-screen windows and knows which one has
        // focus; all we need, if slower than a direct query.
        let windows = xcap::Window::all().ok()?;
        let w = windows.into_iter().find(|w| w.is_focused().unwrap_or(false))?;
        Some(WindowInfo {
            process_name: w.app_name().unwrap_or_default(),
            class_name: String::new(),
            focus_class: String::new(),
            title: w.title().unwrap_or_default(),
            pid: w.pid().unwrap_or(0),
        })
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// The mouse cursor position, or `None` where the platform hides it
/// (Wayland, or no display).
pub fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        win::cursor_position()
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use enigo::Mouse;
        if crate::Session::detect().is_wayland() {
            return None;
        }
        let enigo = enigo::Enigo::new(&enigo::Settings::default()).ok()?;
        enigo.location().ok()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// The text caret of the foreground window in screen coordinates, so the
/// popup can open next to where the user is typing. Windows only, and only
/// for programs that use the system caret (classic Win32 edit controls, many
/// editors); browsers and Electron apps draw their own and give `None`.
pub fn caret_rect() -> Option<Rect> {
    #[cfg(windows)]
    {
        win::caret_rect()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Every monitor. Empty when the platform cannot enumerate them.
pub fn monitors() -> Vec<Monitor> {
    #[cfg(windows)]
    {
        win::monitors()
    }
    #[cfg(not(windows))]
    {
        crate::screen::monitors().unwrap_or_default()
    }
}

/// The monitor under a point.
pub fn monitor_at(x: i32, y: i32) -> Option<Monitor> {
    monitors().into_iter().find(|m| m.contains(x as f32, y as f32))
}

/// Where to put a popup of `size` next to `anchor` (the caret, the
/// selection, or a small box around the mouse cursor): the popup's top-left
/// corner.
///
/// The preferred spot is below and to the right of the anchor. If the popup
/// would leave the work area of the anchor's monitor it flips to the other
/// side of the anchor on that axis, and if it fits on neither side it is
/// pushed inside the work area (covering the anchor rather than going
/// off-screen). A popup larger than the work area is pinned to its top-left.
/// With no monitors at all, the preferred spot is returned unchanged.
pub fn place_popup(anchor: Rect, size: (f32, f32), monitors: &[Monitor]) -> (f32, f32) {
    let (w, h) = (size.0.max(0.0), size.1.max(0.0));
    let right = anchor.x + anchor.width.max(0.0);
    let bottom = anchor.y + anchor.height.max(0.0);
    let Some(mon) = pick_monitor(anchor, monitors) else {
        return (right, bottom);
    };
    let area = if mon.work_area.is_empty() { mon.bounds() } else { mon.work_area };
    let x = fit_axis(anchor.x, right, w, area.x, area.x + area.width);
    let y = fit_axis(anchor.y, bottom, h, area.y, area.y + area.height);
    (x, y)
}

/// Where a dropdown list goes: the result of [`place_dropdown`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropdownPlace {
    pub x: f32,
    pub y: f32,
    /// The height it gets: what it asked for, or less when neither side of
    /// the anchor has that much room (it then scrolls).
    pub height: f32,
    /// It opens upwards, ending at the anchor's top.
    pub above: bool,
}

/// Where to put a dropdown list of `size` for the control at `anchor`, as a
/// native combo box does: under the control and aligned with its left edge;
/// above it only when the work area of its monitor has no room below and
/// more above. Horizontally it is kept inside the work area. With no
/// monitors it goes under the anchor at the size asked.
pub fn place_dropdown(anchor: Rect, size: (f32, f32), monitors: &[Monitor]) -> DropdownPlace {
    let (w, h) = (size.0.max(0.0), size.1.max(0.0));
    let bottom = anchor.y + anchor.height.max(0.0);
    let Some(mon) = pick_monitor(anchor, monitors) else {
        return DropdownPlace { x: anchor.x, y: bottom, height: h, above: false };
    };
    let area = if mon.work_area.is_empty() { mon.bounds() } else { mon.work_area };
    let (lo_x, hi_x) = (area.x, area.x + area.width);
    let x = if anchor.x + w <= hi_x { anchor.x.max(lo_x) } else { (hi_x - w).max(lo_x) };
    let below = (area.y + area.height - bottom).max(0.0);
    let above = (anchor.y - area.y).max(0.0);
    if h <= below || below >= above {
        DropdownPlace { x, y: bottom, height: h.min(below.max(1.0)), above: false }
    } else {
        let height = h.min(above);
        DropdownPlace { x, y: anchor.y - height, height, above: true }
    }
}

/// A top-level window of this process with exactly this title, visible or
/// not, as a Win32 `HWND`: for windows egui creates (immediate viewports),
/// whose handles it does not hand out. Always `None` off Windows.
pub fn own_window_by_title(title: &str) -> Option<isize> {
    #[cfg(windows)]
    {
        win::own_window_by_title(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        None
    }
}

/// Make a window one that clicks do not activate (`WS_EX_NOACTIVATE`), like
/// a combo box's list or a menu: the window it belongs to keeps the focus
/// and an active title bar, and closing the list hands the focus to no one
/// else. Cheap when it is already so; call it every frame, as the window
/// toolkit rewrites the extended style whenever the window changes.
/// Returns whether the style had to be set. Does nothing off Windows.
pub fn keep_from_activating(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        win::keep_from_activating(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

/// Place a span of `len` on one axis: after the anchor (`end`), else before
/// it (ending at `start`), else clamped into `[lo, hi]`.
fn fit_axis(start: f32, end: f32, len: f32, lo: f32, hi: f32) -> f32 {
    if end >= lo && end + len <= hi {
        return end;
    }
    if start <= hi && start - len >= lo {
        return start - len;
    }
    let max = hi - len;
    if max < lo {
        lo
    } else {
        end.clamp(lo, max)
    }
}

/// The monitor containing the anchor's centre, or the nearest one when the
/// centre is off every monitor (a caret reported in a gap between displays).
fn pick_monitor(anchor: Rect, monitors: &[Monitor]) -> Option<&Monitor> {
    let (cx, cy) = (anchor.x + anchor.width.max(0.0) / 2.0, anchor.y + anchor.height.max(0.0) / 2.0);
    if let Some(m) = monitors.iter().find(|m| m.contains(cx, cy)) {
        return Some(m);
    }
    monitors.iter().min_by(|a, b| distance_sq(a.bounds(), cx, cy).total_cmp(&distance_sq(b.bounds(), cx, cy)))
}

fn distance_sq(r: Rect, x: f32, y: f32) -> f32 {
    let dx = (r.x - x).max(0.0).max(x - (r.x + r.width));
    let dy = (r.y - y).max(0.0).max(y - (r.y + r.height));
    dx * dx + dy * dy
}

#[cfg(windows)]
mod win {
    use super::{Monitor, WindowInfo};
    use glossa_core::ocr::Rect;
    use windows::core::{BOOL, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, MapWindowPoints, HDC, HMONITOR, MONITORINFO,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, SetWindowLongPtrW, GUITHREADINFO, GWL_EXSTYLE,
        MONITORINFOF_PRIMARY, WS_EX_NOACTIVATE,
    };

    struct TitleSearch {
        pid: u32,
        title: Vec<u16>,
        found: Option<isize>,
    }

    unsafe extern "system" fn visit_title(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `TitleSearch` passed by
        // `own_window_by_title`, alive for the whole enumeration.
        let search = unsafe { &mut *(lparam.0 as *mut TitleSearch) };
        let mut pid = 0u32;
        // SAFETY: EnumWindows hands us valid top-level window handles.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != search.pid {
            return BOOL(1);
        }
        // SAFETY: as above; the buffer is sized from the reported length.
        let len = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
        if len != search.title.len() {
            return BOOL(1);
        }
        let mut buf = vec![0u16; len + 1];
        // SAFETY: as above.
        let got = unsafe { GetWindowTextW(hwnd, &mut buf) }.max(0) as usize;
        if got == len && buf[..got] == search.title[..] {
            search.found = Some(hwnd.0 as isize);
            return BOOL(0);
        }
        BOOL(1)
    }

    pub fn own_window_by_title(title: &str) -> Option<isize> {
        let mut search = TitleSearch {
            // SAFETY: no preconditions.
            pid: unsafe { GetCurrentProcessId() },
            title: title.encode_utf16().collect(),
            found: None,
        };
        // SAFETY: `visit_title` only uses `search` through the pointer while
        // EnumWindows runs; it stops early (an "error" result) once found.
        let _ = unsafe { EnumWindows(Some(visit_title), LPARAM(&mut search as *mut TitleSearch as isize)) };
        search.found
    }

    pub fn keep_from_activating(hwnd: isize) -> bool {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: a stale handle makes both calls fail, which is harmless.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let wanted = style | WS_EX_NOACTIVATE.0 as isize;
            if style == 0 || style == wanted {
                return false;
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            true
        }
    }

    pub fn foreground_window() -> Option<WindowInfo> {
        // SAFETY: every buffer passed is a live, correctly sized local; the
        // HWND comes straight from the OS and may go stale, which the calls
        // tolerate by failing.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut class = [0u16; 256];
            let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
            let class_name = String::from_utf16_lossy(&class[..n]);
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut title = vec![0u16; len + 1];
            let n = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            let title = String::from_utf16_lossy(&title[..n.min(len)]);
            Some(WindowInfo {
                process_name: process_name(pid).unwrap_or_default(),
                class_name,
                focus_class: focus_class(hwnd).unwrap_or_default(),
                title,
                pid,
            })
        }
    }

    /// The class of the focused control in a window's thread: what the
    /// exceptions' control rules (list views, hotkey fields…) match on.
    fn focus_class(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
        // SAFETY: GUITHREADINFO is initialised with its size as the API
        // requires; the class buffer is a correctly sized local.
        unsafe {
            let thread = GetWindowThreadProcessId(hwnd, None);
            let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread, &mut info).ok()?;
            if info.hwndFocus.is_invalid() {
                return None;
            }
            let mut class = [0u16; 256];
            let n = GetClassNameW(info.hwndFocus, &mut class).max(0) as usize;
            (n > 0).then(|| String::from_utf16_lossy(&class[..n]))
        }
    }

    /// The executable's file name for a process id.
    fn process_name(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        // SAFETY: the handle is closed on every path after it was opened, and
        // the buffer length passed matches the buffer.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
            let _ = CloseHandle(handle);
            ok.ok()?;
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            path.rsplit(['\\', '/']).next().map(str::to_owned)
        }
    }

    pub fn cursor_position() -> Option<(i32, i32)> {
        let mut p = POINT::default();
        // SAFETY: `p` is a valid out-pointer for the duration of the call.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x, p.y))
    }

    pub fn caret_rect() -> Option<Rect> {
        // SAFETY: GUITHREADINFO is initialised with its size as the API
        // requires; MapWindowPoints gets a two-element array it may write to.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            let thread = GetWindowThreadProcessId(hwnd, None);
            let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread, &mut info).ok()?;
            if info.hwndCaret.is_invalid() {
                return None;
            }
            let rc = info.rcCaret;
            if rc.right <= rc.left && rc.bottom <= rc.top {
                return None;
            }
            let mut pts = [POINT { x: rc.left, y: rc.top }, POINT { x: rc.right, y: rc.bottom }];
            MapWindowPoints(Some(info.hwndCaret), None, &mut pts);
            Some(Rect::from_corners(pts[0].x as f32, pts[0].y as f32, pts[1].x as f32, pts[1].y as f32))
        }
    }

    pub fn monitors() -> Vec<Monitor> {
        let mut out: Vec<Monitor> = Vec::new();
        // SAFETY: the callback only runs during EnumDisplayMonitors, while
        // `out` is alive and exclusively borrowed through the LPARAM.
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    unsafe extern "system" fn enum_proc(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<Monitor>` passed by `monitors`.
        let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        // SAFETY: `mi` is initialised with its size; the handle is the one
        // the enumeration handed us.
        if !unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
            return BOOL(1);
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        // SAFETY: valid out-pointers for the duration of the call.
        let scale = match unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } {
            Ok(()) => dx as f32 / 96.0,
            Err(_) => 1.0,
        };
        let r = mi.rcMonitor;
        let w = mi.rcWork;
        out.push(Monitor {
            x: r.left,
            y: r.top,
            width: (r.right - r.left).max(0) as u32,
            height: (r.bottom - r.top).max(0) as u32,
            scale,
            work_area: Rect::new(w.left as f32, w.top as f32, (w.right - w.left) as f32, (w.bottom - w.top) as f32),
            is_primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
        BOOL(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: u32, h: u32) -> Monitor {
        Monitor {
            x,
            y,
            width: w,
            height: h,
            scale: 1.0,
            work_area: Rect::new(x as f32, y as f32, w as f32, h as f32),
            is_primary: x == 0 && y == 0,
        }
    }

    /// A 1920×1080 monitor with a 40 px taskbar at the bottom.
    fn with_taskbar(x: i32, y: i32) -> Monitor {
        let mut m = mon(x, y, 1920, 1080);
        m.work_area = Rect::new(x as f32, y as f32, 1920.0, 1040.0);
        m
    }

    const POPUP: (f32, f32) = (400.0, 300.0);

    fn cursor(x: f32, y: f32) -> Rect {
        Rect::new(x, y, 16.0, 16.0)
    }

    #[test]
    fn below_right_when_it_fits() {
        let m = [mon(0, 0, 1920, 1080)];
        assert_eq!(place_popup(cursor(100.0, 100.0), POPUP, &m), (116.0, 116.0));
    }

    #[test]
    fn flips_left_near_the_right_edge() {
        let m = [mon(0, 0, 1920, 1080)];
        assert_eq!(place_popup(cursor(1700.0, 100.0), POPUP, &m), (1300.0, 116.0));
    }

    #[test]
    fn flips_above_near_the_bottom_and_respects_the_taskbar() {
        let m = [with_taskbar(0, 0)];
        // 116 + 300 = 416 fits; 800 + 16 + 300 = 1116 > 1040 does not.
        assert_eq!(place_popup(cursor(100.0, 800.0), POPUP, &m), (116.0, 500.0));
        // Just fits above the taskbar: 724 + 16 + 300 = 1040.
        assert_eq!(place_popup(cursor(100.0, 724.0), POPUP, &m), (116.0, 740.0));
        // One pixel lower and it flips.
        assert_eq!(place_popup(cursor(100.0, 725.0), POPUP, &m), (116.0, 425.0));
    }

    #[test]
    fn flips_both_ways_in_the_corner() {
        let m = [mon(0, 0, 1920, 1080)];
        assert_eq!(place_popup(cursor(1900.0, 1060.0), POPUP, &m), (1500.0, 760.0));
    }

    #[test]
    fn clamps_when_neither_side_fits() {
        // A short monitor: 300 px tall popup fits neither above nor below a
        // cursor in the middle, so it is pushed inside the work area.
        let m = [mon(0, 0, 1920, 400)];
        let (_, y) = place_popup(cursor(100.0, 190.0), POPUP, &m);
        assert_eq!(y, 100.0);
        assert!(y >= 0.0 && y + POPUP.1 <= 400.0);
    }

    #[test]
    fn oversized_popup_is_pinned_to_the_top_left_of_the_work_area() {
        let m = [mon(0, 0, 300, 200)];
        assert_eq!(place_popup(cursor(50.0, 50.0), POPUP, &m), (0.0, 0.0));
    }

    #[test]
    fn secondary_monitor_left_of_primary_with_negative_coordinates() {
        let m = [mon(0, 0, 1920, 1080), mon(-1920, 0, 1920, 1080)];
        // Near the right edge of the left monitor (x = -10): flipping keeps
        // the popup on that monitor instead of spilling onto the primary.
        assert_eq!(place_popup(cursor(-30.0, 100.0), POPUP, &m), (-430.0, 116.0));
        // Near its left edge it goes right as usual.
        assert_eq!(place_popup(cursor(-1900.0, 100.0), POPUP, &m), (-1884.0, 116.0));
    }

    #[test]
    fn monitor_above_the_primary() {
        let m = [mon(0, 0, 1920, 1080), mon(0, -1440, 2560, 1440)];
        // Bottom of the upper monitor (y = -20): flips above, stays on it.
        assert_eq!(place_popup(cursor(500.0, -40.0), POPUP, &m), (516.0, -340.0));
        // Top of the primary is unaffected by the monitor above it.
        assert_eq!(place_popup(cursor(500.0, 0.0), POPUP, &m), (516.0, 16.0));
    }

    #[test]
    fn mixed_sizes_and_offsets() {
        // Primary 2560×1440 with taskbar, a 1080p monitor to the right and
        // lower, as people really arrange them.
        let mut primary = mon(0, 0, 2560, 1440);
        primary.work_area = Rect::new(0.0, 0.0, 2560.0, 1392.0);
        let m = [primary, mon(2560, 360, 1920, 1080)];
        // Bottom-right of the second monitor.
        assert_eq!(place_popup(cursor(4400.0, 1400.0), POPUP, &m), (4000.0, 1100.0));
        // Bottom of the primary, above its taskbar.
        assert_eq!(place_popup(cursor(10.0, 1380.0), POPUP, &m), (26.0, 1080.0));
    }

    #[test]
    fn anchor_off_every_monitor_uses_the_nearest() {
        let m = [mon(0, 0, 1920, 1080), mon(1920, 0, 1920, 1080)];
        // A caret reported just below the right monitor.
        let (x, y) = place_popup(cursor(2000.0, 1200.0), POPUP, &m);
        assert!(m[1].contains(x, y), "({x}, {y}) should be on the right monitor");
        assert_eq!((x, y), (2016.0, 780.0));
    }

    #[test]
    fn selection_rect_anchor() {
        let m = [mon(0, 0, 1920, 1080)];
        let sel = Rect::new(200.0, 300.0, 500.0, 20.0);
        assert_eq!(place_popup(sel, POPUP, &m), (700.0, 320.0));
        // Wide selection near the right edge: the popup goes to its left.
        let sel = Rect::new(1400.0, 300.0, 500.0, 20.0);
        assert_eq!(place_popup(sel, POPUP, &m), (1000.0, 320.0));
    }

    #[test]
    fn no_monitors_returns_the_preferred_spot() {
        assert_eq!(place_popup(cursor(5.0, 5.0), POPUP, &[]), (21.0, 21.0));
    }

    #[test]
    fn empty_work_area_falls_back_to_bounds() {
        let mut m = mon(0, 0, 1000, 1000);
        m.work_area = Rect::default();
        assert_eq!(place_popup(cursor(900.0, 900.0), POPUP, &[m]), (500.0, 600.0));
    }

    const LIST: (f32, f32) = (200.0, 300.0);

    #[test]
    fn dropdowns_open_below_aligned_left() {
        let combo = Rect::new(100.0, 100.0, 150.0, 26.0);
        let p = place_dropdown(combo, LIST, &[with_taskbar(0, 0)]);
        assert_eq!(p, DropdownPlace { x: 100.0, y: 126.0, height: 300.0, above: false });
    }

    #[test]
    fn dropdowns_flip_above_only_without_room_below() {
        // 1040 - 926 = 114 below, 900 above.
        let combo = Rect::new(100.0, 900.0, 150.0, 26.0);
        let p = place_dropdown(combo, LIST, &[with_taskbar(0, 0)]);
        assert_eq!(p, DropdownPlace { x: 100.0, y: 600.0, height: 300.0, above: true });
        // Room for all 300 below, if only just: stays below.
        let combo = Rect::new(100.0, 714.0, 150.0, 26.0);
        assert!(!place_dropdown(combo, LIST, &[with_taskbar(0, 0)]).above);
    }

    #[test]
    fn dropdowns_shrink_on_the_roomier_side_when_neither_fits() {
        let short = mon(0, 0, 800, 400);
        let combo = Rect::new(10.0, 150.0, 100.0, 26.0);
        let p = place_dropdown(combo, LIST, &[short]);
        assert_eq!(p, DropdownPlace { x: 10.0, y: 176.0, height: 224.0, above: false });
        let combo = Rect::new(10.0, 300.0, 100.0, 26.0);
        let p = place_dropdown(combo, LIST, &[short]);
        assert_eq!(p, DropdownPlace { x: 10.0, y: 0.0, height: 300.0, above: true });
    }

    #[test]
    fn dropdowns_stay_on_their_monitor_horizontally() {
        let two = [with_taskbar(0, 0), with_taskbar(1920, 0)];
        let combo = Rect::new(1850.0, 100.0, 60.0, 26.0);
        assert_eq!(place_dropdown(combo, LIST, &two).x, 1920.0 - 200.0);
        let combo = Rect::new(1930.0, 100.0, 60.0, 26.0);
        assert_eq!(place_dropdown(combo, LIST, &two).x, 1930.0);
        assert_eq!(place_dropdown(combo, LIST, &[]).y, 126.0);
    }

    #[test]
    fn contains_is_half_open() {
        let a = mon(0, 0, 1920, 1080);
        let b = mon(1920, 0, 1920, 1080);
        assert!(a.contains(1919.5, 0.0));
        assert!(!a.contains(1920.0, 0.0));
        assert!(b.contains(1920.0, 0.0));
        assert_eq!(a.bounds(), Rect::new(0.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    #[ignore = "needs a display"]
    fn real_monitors_are_listed() {
        let all = monitors();
        assert!(!all.is_empty());
        let m = all[0];
        assert!(monitor_at(m.x, m.y).is_some());
    }
}
