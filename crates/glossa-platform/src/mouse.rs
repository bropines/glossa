//! Watching the mouse for selections: the classic "mouse mode", where
//! selecting text with the mouse is enough to get it translated.
//!
//! * **Windows**: a low-level mouse hook on a thread of its own reports a
//!   drag of a few pixels or a double/triple click with the left button.
//!   Clicks on Glossa's own windows are ignored. Injected mouse input is not:
//!   Glossa never synthesises mouse events itself, and accessibility and
//!   remote-control tools that do select text for real. What, if anything,
//!   was selected is for the caller to find out (UI Automation).
//! * **Linux**: the PRIMARY selection is the X11 meaning of "selected with
//!   the mouse", so it is watched directly and reported with its text.
//! * **macOS**: not available yet.
//!
//! The callback runs on the watcher's thread and must return quickly: hand
//! the gesture to a channel.

/// What the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gesture {
    /// A drag or a double click that may have selected something, at this
    /// screen position (physical pixels). `ctrl`: Ctrl was held.
    Selected { at: (i32, i32), ctrl: bool },
    /// A new PRIMARY selection with its text (Linux).
    Text { text: String, at: Option<(i32, i32)> },
}

/// Runs until dropped.
pub struct MouseWatcher {
    imp: imp::Watcher,
}

impl MouseWatcher {
    /// Whether mouse mode can work on this system at all, so the interface
    /// can grey it out rather than offer a switch that only fails. (It may
    /// still fail to start where it is supported, for want of a permission.)
    pub fn supported() -> bool {
        cfg!(any(windows, target_os = "linux"))
    }

    pub fn start(on_gesture: impl Fn(Gesture) + Send + 'static) -> crate::Result<MouseWatcher> {
        Ok(MouseWatcher { imp: imp::Watcher::start(Box::new(on_gesture))? })
    }
}

impl Drop for MouseWatcher {
    fn drop(&mut self) {
        self.imp.stop();
    }
}

/// Whether a press and a release this far apart are a drag, not a click.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_drag(down: (i32, i32), up: (i32, i32)) -> bool {
    let (dx, dy) = ((up.0 - down.0).abs(), (up.1 - down.1).abs());
    dx.max(dy) >= 5
}

/// Whether two clicks make a double click.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_double(prev: ((i32, i32), u32), now: ((i32, i32), u32), double_ms: u32) -> bool {
    let ((p, pt), (n, nt)) = (prev, now);
    nt.wrapping_sub(pt) <= double_ms && (n.0 - p.0).abs() <= 4 && (n.1 - p.1).abs() <= 4
}

type Sink = Box<dyn Fn(Gesture) + Send>;

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetDoubleClickTime, VK_CONTROL};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, WindowFromPoint, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_QUIT,
    };

    use super::{is_double, is_drag, Gesture, Sink};

    struct HookState {
        down: Option<(i32, i32)>,
        last_click: Option<((i32, i32), u32)>,
        double_ms: u32,
        /// Gestures seen by the hook, handed to the sink by the message loop.
        pending: Vec<Gesture>,
    }

    /// Posted by the hook to its own thread: "gestures are pending".
    const WM_GESTURE: u32 = WM_APP + 1;

    thread_local! {
        // Low-level hooks call back on the thread that installed them.
        static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    }

    fn own_window_at(pt: POINT) -> bool {
        // SAFETY: plain queries; a stale HWND makes them fail harmlessly.
        unsafe {
            let hwnd = WindowFromPoint(pt);
            if hwnd.is_invalid() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            pid == GetCurrentProcessId()
        }
    }

    /// The hook itself. Every mouse event in the session waits for it, and
    /// Windows silently removes a hook that is too slow, so it only notes the
    /// gesture and posts a message; the sink runs from the message loop.
    /// It must not panic either: release builds abort on panic.
    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            // SAFETY: for WH_MOUSE_LL, lparam points at an MSLLHOOKSTRUCT
            // valid for the duration of the call.
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let msg = wparam.0 as u32;
            if msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP {
                let at = (info.pt.x, info.pt.y);
                STATE.with(|s| {
                    // Re-entered (the loop is draining): skip this event
                    // rather than panic on a second borrow.
                    let Ok(mut s) = s.try_borrow_mut() else { return };
                    let Some(st) = s.as_mut() else { return };
                    if msg == WM_LBUTTONDOWN {
                        st.down = Some(at);
                        return;
                    }
                    let Some(down) = st.down.take() else { return };
                    let selected = if is_drag(down, at) {
                        st.last_click = None;
                        true
                    } else {
                        let now = (at, info.time);
                        let double = st.last_click.is_some_and(|prev| is_double(prev, now, st.double_ms));
                        st.last_click = Some(now);
                        double
                    };
                    if selected && !own_window_at(info.pt) {
                        // SAFETY: no preconditions; the sign bit means "down now".
                        let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
                        st.pending.push(Gesture::Selected { at, ctrl });
                        // SAFETY: posting to this very thread's queue.
                        unsafe {
                            let _ = PostThreadMessageW(GetCurrentThreadId(), WM_GESTURE, WPARAM(0), LPARAM(0));
                        }
                    }
                });
            }
        }
        // SAFETY: passing the event on unchanged, as every hook must.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub struct Watcher {
        thread_id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl Watcher {
        pub fn start(sink: Sink) -> crate::Result<Watcher> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
            let thread = std::thread::Builder::new()
                .name("glossa-mouse".into())
                .spawn(move || {
                    // SAFETY: the hook is installed and removed on this
                    // thread, which pumps messages in between as low-level
                    // hooks require; GetDoubleClickTime has no preconditions.
                    unsafe {
                        STATE.with(|s| {
                            *s.borrow_mut() = Some(HookState {
                                down: None,
                                last_click: None,
                                double_ms: GetDoubleClickTime(),
                                pending: Vec::new(),
                            })
                        });
                        let module = GetModuleHandleW(None).ok();
                        let handle = match SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module.map(Into::into), 0) {
                            Ok(h) => h,
                            Err(e) => {
                                let _ = ready_tx.send(Err(e.to_string()));
                                return;
                            }
                        };
                        let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            if msg.message != WM_GESTURE {
                                continue;
                            }
                            let pending = STATE.with(|s| {
                                s.try_borrow_mut()
                                    .ok()
                                    .and_then(|mut s| s.as_mut().map(|st| std::mem::take(&mut st.pending)))
                            });
                            for g in pending.unwrap_or_default() {
                                sink(g);
                            }
                        }
                        let _ = UnhookWindowsHookEx(handle);
                        STATE.with(|s| *s.borrow_mut() = None);
                    }
                })
                .map_err(|e| crate::platform_err(format!("mouse watcher: {e}")))?;
            let thread_id = ready_rx
                .recv()
                .map_err(|e| crate::platform_err(format!("mouse watcher: {e}")))?
                .map_err(|e| crate::platform_err(format!("mouse hook: {e}")))?;
            Ok(Watcher { thread_id, thread: Some(thread) })
        }

        pub fn stop(&mut self) {
            // SAFETY: posting WM_QUIT to a thread we started; it may have
            // ended already, in which case the call just fails.
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::{Gesture, Sink};

    pub struct Watcher {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Watcher {
        pub fn start(sink: Sink) -> crate::Result<Watcher> {
            let mut probe = crate::Clipboard::new()?;
            let mut last = probe.get_primary();
            drop(probe);
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::Builder::new()
                .name("glossa-primary".into())
                .spawn(move || {
                    let Ok(mut cb) = crate::Clipboard::new() else { return };
                    while !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(300));
                        let now = cb.get_primary();
                        if now.is_some() && now != last {
                            last = now.clone();
                            if let Some(text) = now.filter(|t| !t.trim().is_empty()) {
                                sink(Gesture::Text { text, at: crate::window::cursor_position() });
                            }
                        }
                    }
                })
                .map_err(|e| crate::platform_err(format!("selection watcher: {e}")))?;
            Ok(Watcher { stop, thread: Some(thread) })
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Sink;

    pub struct Watcher;

    impl Watcher {
        pub fn start(_sink: Sink) -> crate::Result<Watcher> {
            Err(crate::platform_err("mouse mode is not available on this system yet"))
        }

        pub fn stop(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_and_double_clicks_are_told_apart_from_clicks() {
        assert!(!is_drag((100, 100), (102, 103)), "a click wobbles a little");
        assert!(is_drag((100, 100), (106, 100)));
        assert!(is_drag((100, 100), (100, 40)));
        assert!(is_double(((10, 10), 1000), ((11, 12), 1300), 500));
        assert!(!is_double(((10, 10), 1000), ((11, 12), 1600), 500), "too slow");
        assert!(!is_double(((10, 10), 1000), ((30, 10), 1100), 500), "too far");
        assert!(is_double(((10, 10), u32::MAX - 100), ((10, 10), 200), 500), "the tick counter wraps");
    }
}
