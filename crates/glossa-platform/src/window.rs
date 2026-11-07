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

/// Desktop coordinates per egui point at display `scale`. Windows and Linux
/// report the desktop (cursor, monitors, window positions) in pixels, so a
/// point is `scale` of them; macOS reports it in points already, so a point
/// is one. Captured images are in pixels everywhere: use the monitor's
/// `scale`, not this, to go between image pixels and points.
pub fn desktop_scale(scale: f32) -> f32 {
    if crate::shell::SHELL.desktop_in_points || scale.is_nan() || scale <= 0.0 {
        1.0
    } else {
        scale
    }
}

impl Monitor {
    /// Desktop coordinates per egui point on this monitor ([`desktop_scale`]).
    pub fn desktop_scale(&self) -> f32 {
        desktop_scale(self.scale)
    }

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
    /// The height it gets: what it asked for, or less when it is taller than
    /// the work area (it then scrolls).
    pub height: f32,
    /// It opens upwards, ending at the anchor's top.
    pub above: bool,
}

/// Where to put a dropdown list of `size` for the control at `anchor`, as a
/// native combo box does: under the control and aligned with its left edge;
/// above it only when the work area of its monitor has no room below and
/// enough above; over the control when neither side has the room but the
/// work area does, as native menus slide to fit; scrolling only when it is
/// taller than the work area. Horizontally it is kept inside the work area. With no
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
    let (top, floor) = (area.y, area.y + area.height);
    if h <= below {
        DropdownPlace { x, y: bottom, height: h, above: false }
    } else if h <= above {
        DropdownPlace { x, y: anchor.y - h, height: h, above: true }
    } else if h <= area.height {
        // Neither side has the room, but the screen does: slide it over the
        // control until it fits, as native menus do, rather than make a
        // list that scrolls by a few rows (the main window's menu, opened
        // from a window low on the screen).
        if below >= above {
            DropdownPlace { x, y: floor - h, height: h, above: false }
        } else {
            DropdownPlace { x, y: top, height: h, above: true }
        }
    } else if below >= above {
        DropdownPlace { x, y: bottom, height: below.max(1.0), above: false }
    } else {
        DropdownPlace { x, y: anchor.y - above, height: above, above: true }
    }
}

/// A top-level window of this process with exactly this title, visible or
/// not, as a Win32 `HWND`: for windows egui creates (immediate viewports),
/// whose handles it does not hand out. Always `None` off Windows.
/// macOS: whether Glossa shows in the Dock and the Cmd+Tab switcher. A
/// menu-bar app (`LSUIElement`) is hidden there, which suits it while only
/// the menu-bar icon and the popup are up, but a settings or main window
/// should be reachable with Cmd+Tab like any other. Must be called on the
/// main thread (the GUI's); elsewhere and on other systems it does nothing.
pub fn set_in_app_switcher(shown: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let policy =
            if shown { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
        app.setActivationPolicy(policy);
        if shown {
            // Becoming a regular app does not bring it forward by itself.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = shown;
}

/// One of this program's windows by its exact title, visible or not: a
/// Win32 `HWND` on Windows, a pointer to the `NSWindow` on macOS (for
/// [`crate::backdrop::apply_hwnd_with`]). For the windows egui creates
/// (immediate viewports), whose handles it does not hand out. `None`
/// elsewhere. Call it on the GUI's thread.
pub fn own_window_by_title(title: &str) -> Option<isize> {
    #[cfg(windows)]
    {
        win::own_window_by_title(title)
    }
    #[cfg(target_os = "macos")]
    {
        mac::own_window_by_title(title)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = title;
        None
    }
}

/// A maximise button the system tracks for a title bar Glossa draws itself
/// (see [`snap_layouts`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaximizeButton {
    pub hovered: bool,
    pub pressed: bool,
}

/// Tell the system where the maximise button of this program's window
/// `title` is, for a title bar drawn by Glossa (`button`: x, y, width,
/// height in the window's client-area pixels; `None` when it has none now).
/// On Windows 11 resting the pointer on it then offers Snap Layouts. The
/// system takes over the button's pointer handling, so its hover and press
/// come back from here, and a click maximises or restores the window by
/// itself. `None` where this is not done (off Windows, or no such window):
/// the button is then an ordinary one. Call it every frame from the GUI's
/// thread.
pub fn snap_layouts(title: &str, button: Option<(i32, i32, i32, i32)>) -> Option<MaximizeButton> {
    #[cfg(windows)]
    {
        snap::snap_layouts(title, button)
    }
    #[cfg(not(windows))]
    {
        let _ = (title, button);
        None
    }
}

/// Whether [`round_corners`] does anything here (macOS): callers skip
/// finding the window otherwise.
pub const CAN_ROUND_CORNERS: bool = cfg!(target_os = "macos");

/// Round a borderless window's corners by `radius` points, given the
/// pointer [`own_window_by_title`] found: on macOS, where nothing rounds
/// them (on Windows 11 `backdrop::style_frame` asks the system to). Returns
/// whether it was done; elsewhere it does nothing. Call it on the GUI's
/// thread.
pub fn round_corners(window: isize, radius: f32) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::round_corners(window, radius)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, radius);
        false
    }
}

/// Lift one of this program's windows, by its exact title, above the menu
/// bar and the Dock: on macOS a window meant to cover a whole screen is
/// otherwise kept below the menu bar. Elsewhere an always-on-top window
/// already covers the taskbar, and this does nothing. Returns whether the
/// window was found and lifted. Call it on the GUI's thread.
pub fn lift_above_menu_bar(title: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::lift_above_menu_bar(title)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        false
    }
}

/// Lift one of this program's windows, by its exact title, over everything
/// on screen, the menu bar, the Dock and the system's edge strips (Stage
/// Manager) included: for the screen-region overlay, which the pointer
/// drags to the screen's edges. On macOS a lower window lets the Stage
/// Manager strip slide in over it there. Elsewhere an always-on-top window
/// is enough, and this does nothing. Returns whether the window was found.
/// Call it on the GUI's thread.
pub fn lift_over_everything(title: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::lift_over_everything(title)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        false
    }
}

/// Before a synthetic paste: if clicking one of this program's windows made
/// it the active application (macOS activates the application whose window
/// is clicked, the popup's included), give the focus back to the program
/// the user was working in, so the keystroke reaches it. Waits up to
/// `timeout`. Elsewhere the popup does not take the focus and this does
/// nothing. Any thread.
pub fn give_focus_back(timeout: std::time::Duration) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::give_focus_back(timeout)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = timeout;
        true
    }
}

/// Whether [`float_free`] does anything here (macOS), so callers can skip
/// looking the window up elsewhere.
pub const CAN_FLOAT_FREE: bool = cfg!(target_os = "macos");

/// Keep one of this program's windows, by its exact title, out of the
/// system's window management: on macOS it then follows the user to any
/// Space or full-screen app and Stage Manager neither gives it a stage of
/// its own nor switches to the stage of another Glossa window to show it.
/// Elsewhere a borderless always-on-top window already behaves so, and this
/// does nothing. Returns whether the window was found. Call it on the GUI's
/// thread.
pub fn float_free(title: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::float_free(title)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        false
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

/// A window to give the keyboard focus back to; see [`focus_mark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusMark(#[allow(dead_code)] isize);

/// The window the user is working in, to give the focus back to after a
/// click elsewhere took it: a click on a Windows tray icon activates the
/// taskbar, so the selection a tray action should read is no longer in the
/// foreground window. `None` when nothing will need giving back — on macOS
/// (a click on a menu-bar item leaves the active app active), on Linux, and
/// when the foreground window is the taskbar or this program.
pub fn focus_mark() -> Option<FocusMark> {
    #[cfg(windows)]
    {
        win::focus_mark().map(FocusMark)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Bring a window marked by [`focus_mark`] back to the foreground. `false`
/// when it is gone or the system refused.
pub fn refocus(mark: FocusMark) -> bool {
    #[cfg(windows)]
    {
        win::refocus(mark.0)
    }
    #[cfg(not(windows))]
    {
        let _ = mark;
        false
    }
}

/// The longest gap between the two clicks of a double click, as the user
/// set it in the system (500 ms on Windows by default; macOS lets the
/// accessibility settings stretch it). 400 ms where the system will not tell.
pub fn double_click_time() -> std::time::Duration {
    #[cfg(windows)]
    {
        win::double_click_time()
    }
    #[cfg(target_os = "macos")]
    {
        let secs = objc2_app_kit::NSEvent::doubleClickInterval();
        if secs.is_finite() && secs > 0.05 {
            return std::time::Duration::from_secs_f64(secs.min(5.0));
        }
        std::time::Duration::from_millis(400)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::time::Duration::from_millis(400)
    }
}

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod snap;
#[cfg(windows)]
mod win;

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
    fn dropdowns_slide_over_the_control_rather_than_scroll_when_the_screen_has_room() {
        let short = mon(0, 0, 800, 400);
        // 224 below, 150 above: neither holds 300, the screen does. It ends
        // at the bottom of the screen, over the control, whole.
        let combo = Rect::new(10.0, 150.0, 100.0, 26.0);
        let p = place_dropdown(combo, LIST, &[short]);
        assert_eq!(p, DropdownPlace { x: 10.0, y: 100.0, height: 300.0, above: false });
        // Room above: it opens there, as before.
        let combo = Rect::new(10.0, 300.0, 100.0, 26.0);
        let p = place_dropdown(combo, LIST, &[short]);
        assert_eq!(p, DropdownPlace { x: 10.0, y: 0.0, height: 300.0, above: true });
    }

    #[test]
    fn dropdowns_taller_than_the_screen_scroll_on_the_roomier_side() {
        let short = mon(0, 0, 800, 400);
        let tall = (200.0, 600.0);
        let combo = Rect::new(10.0, 150.0, 100.0, 26.0);
        let p = place_dropdown(combo, tall, &[short]);
        assert_eq!(p, DropdownPlace { x: 10.0, y: 176.0, height: 224.0, above: false });
        let combo = Rect::new(10.0, 300.0, 100.0, 26.0);
        let p = place_dropdown(combo, tall, &[short]);
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
