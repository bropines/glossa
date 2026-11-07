//! Windows and monitors on Windows (Win32).

use super::{Monitor, WindowInfo};
use glossa_core::ocr::Rect;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MapWindowPoints, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindow, SetForegroundWindow, SetWindowLongPtrW,
    GUITHREADINFO, GWL_EXSTYLE, MONITORINFOF_PRIMARY, WS_EX_NOACTIVATE,
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

/// The taskbar's windows, and the overflow flyouts the tray icons may sit
/// in: what the foreground is while the user clicks a tray icon, never a
/// window to give the focus back to.
const SHELL_CLASSES: [&str; 5] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Windows.UI.Core.CoreWindow",
];

pub fn focus_mark() -> Option<isize> {
    // SAFETY: the handle comes straight from the OS; the class buffer is a
    // correctly sized local.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return None;
        }
        let mut class = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if SHELL_CLASSES.contains(&class.as_str()) {
            return None;
        }
        Some(hwnd.0 as isize)
    }
}

pub fn refocus(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: a stale handle makes both calls fail, which is harmless. The
    // shell lets the owner of a clicked tray icon set the foreground window,
    // which is what makes this allowed right after the click.
    unsafe { IsWindow(Some(hwnd)).as_bool() && SetForegroundWindow(hwnd).as_bool() }
}

pub fn double_click_time() -> std::time::Duration {
    // SAFETY: no preconditions.
    let ms = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    std::time::Duration::from_millis(u64::from(ms.clamp(100, 5000)))
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
