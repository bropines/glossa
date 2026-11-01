//! Snap Layouts for a title bar Glossa draws itself (Windows 11).
//!
//! Windows 11 offers its layouts when the pointer rests on a window's
//! maximise button, and it finds that button by asking the window what is
//! under the pointer (`WM_NCHITTEST` answering `HTMAXBUTTON`). Glossa's
//! caption buttons are drawn by egui inside the client area, so each window
//! with one is subclassed: over the button the hit test says `HTMAXBUTTON`,
//! and since the pointer is then outside the client area as far as the
//! window is concerned, the button's hover, press and click are tracked
//! here and handed back to the drawing code. On Windows 10 the same makes
//! the button work as before, without the layouts.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, IsZoomed, ShowWindow, HTCLIENT, HTMAXBUTTON, SW_MAXIMIZE, SW_RESTORE, WM_NCDESTROY, WM_NCHITTEST,
    WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};

use super::MaximizeButton;

/// This subclass's id among a window's subclasses.
const SUBCLASS_ID: usize = 0x6c6f_7373; // "loss", for Glossa

#[derive(Default, Clone, Copy)]
struct Snap {
    /// The button in client-area pixels, when the window has one now.
    button: Option<RECT>,
    hovered: bool,
    pressed: bool,
    /// A non-client leave notification is asked for.
    tracking: bool,
}

static WINDOWS: Mutex<Option<HashMap<isize, Snap>>> = Mutex::new(None);
/// The window of each title, found once.
static BY_TITLE: Mutex<Option<HashMap<String, isize>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<isize, Snap>) -> R) -> R {
    let mut guard = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// See `crate::window::snap_layouts`.
pub fn snap_layouts(title: &str, button: Option<(i32, i32, i32, i32)>) -> Option<MaximizeButton> {
    let hwnd = window_of(title)?;
    let rect = button.map(|(x, y, w, h)| RECT { left: x, top: y, right: x + w, bottom: y + h });
    let installed = with(|m| {
        let new = !m.contains_key(&hwnd);
        m.entry(hwnd).or_default().button = rect;
        new
    });
    if installed {
        // SAFETY: a live window of this thread (found by title just now);
        // the subclass is removed again on WM_NCDESTROY.
        let ok = unsafe { SetWindowSubclass(HWND(hwnd as *mut _), Some(subclass), SUBCLASS_ID, 0) };
        if !ok.as_bool() {
            with(|m| m.remove(&hwnd));
            return None;
        }
    }
    with(|m| m.get(&hwnd).map(|s| MaximizeButton { hovered: s.hovered, pressed: s.pressed }))
}

fn window_of(title: &str) -> Option<isize> {
    let mut guard = BY_TITLE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(&hwnd) = map.get(title) {
        // SAFETY: IsWindow takes any value.
        if unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool() {
            return Some(hwnd);
        }
        map.remove(title);
    }
    let hwnd = super::own_window_by_title(title)?;
    map.insert(title.to_owned(), hwnd);
    Some(hwnd)
}

fn redraw(hwnd: HWND) {
    // SAFETY: a live window; a repaint is all it asks for.
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Whether a hit test's point (screen coordinates in `lparam`) is on the
/// window's maximise button.
fn on_button(hwnd: HWND, lparam: LPARAM) -> bool {
    let Some(button) = with(|m| m.get(&(hwnd.0 as isize)).and_then(|s| s.button)) else { return false };
    let mut p = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
    // SAFETY: a live window and a point of ours.
    if !unsafe { ScreenToClient(hwnd, &mut p) }.as_bool() {
        return false;
    }
    p.x >= button.left && p.x < button.right && p.y >= button.top && p.y < button.bottom
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let key = hwnd.0 as isize;
    let on_max = wparam.0 == HTMAXBUTTON as usize;
    match msg {
        WM_NCHITTEST => {
            // SAFETY: the next procedure in the chain, with what we were given.
            let hit = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if hit.0 == HTCLIENT as isize && on_button(hwnd, lparam) {
                return LRESULT(HTMAXBUTTON as isize);
            }
            return hit;
        }
        WM_NCMOUSEMOVE => {
            let (changed, track) = with(|m| {
                let s = m.entry(key).or_default();
                let changed = s.hovered != on_max;
                s.hovered = on_max;
                let track = on_max && !s.tracking;
                s.tracking |= track;
                (changed, track)
            });
            if track {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: a filled-in request for a live window.
                let _ = unsafe { TrackMouseEvent(&mut t) };
            }
            if changed {
                redraw(hwnd);
            }
            if on_max {
                return LRESULT(0);
            }
        }
        WM_NCMOUSELEAVE => {
            with(|m| {
                let s = m.entry(key).or_default();
                *s = Snap { button: s.button, ..Snap::default() };
            });
            redraw(hwnd);
        }
        WM_NCLBUTTONDOWN if on_max => {
            with(|m| m.entry(key).or_default().pressed = true);
            redraw(hwnd);
            return LRESULT(0);
        }
        WM_NCLBUTTONUP if on_max => {
            let was = with(|m| std::mem::take(&mut m.entry(key).or_default().pressed));
            redraw(hwnd);
            if was {
                // SAFETY: a live window of this thread.
                unsafe {
                    let show = if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE };
                    let _ = ShowWindow(hwnd, show);
                }
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            with(|m| m.remove(&key));
            // SAFETY: the subclass this procedure belongs to.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            }
        }
        _ => {}
    }
    // SAFETY: as above.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
