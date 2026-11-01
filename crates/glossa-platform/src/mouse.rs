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
//! * **macOS**: a listen-only Quartz event tap on a thread of its own (with
//!   its own run loop) reports the same gestures as on Windows; the system's
//!   click count tells double clicks, with the user's double-click speed.
//!   Clicks on Glossa's own windows are ignored. The selection is then read
//!   through Accessibility (`AXSelectedText`). The tap needs Accessibility
//!   or Input Monitoring; without either, macOS refuses to create it.
//!
//! The callback runs on the watcher's thread and must return quickly: hand
//! the gesture to a channel.

/// What the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gesture {
    /// A drag or a double click that may have selected something, at this
    /// screen position (physical pixels; on macOS points, as
    /// [`crate::window::cursor_position`] gives them there). `ctrl`: Ctrl
    /// was held (on macOS, Ctrl or Cmd).
    Selected { at: (i32, i32), ctrl: bool },
    /// A new PRIMARY selection with its text (Linux).
    Text { text: String, at: Option<(i32, i32)> },
}

/// Runs until dropped.
pub struct MouseWatcher {
    imp: imp::Watcher,
}

impl MouseWatcher {
    pub fn start(on_gesture: impl Fn(Gesture) + Send + 'static) -> crate::Result<MouseWatcher> {
        Ok(MouseWatcher { imp: imp::Watcher::start(Box::new(on_gesture))? })
    }

    /// Whether mouse mode can work in this session (Windows, macOS, X11), so
    /// the interface can grey it out rather than offer a switch that only
    /// fails; it may still fail to start for want of a permission. Wayland
    /// gives no program the mouse or the PRIMARY selection of another.
    pub fn supported() -> bool {
        if cfg!(any(windows, target_os = "macos")) {
            true
        } else if cfg!(target_os = "linux") {
            !crate::Session::detect().is_wayland()
        } else {
            false
        }
    }
}

impl Drop for MouseWatcher {
    fn drop(&mut self) {
        self.imp.stop();
    }
}

/// Whether a press and a release this far apart are a drag, not a click.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
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

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::JoinHandle;

    use super::{is_drag, Gesture, Sink};

    // Declared by hand, like `macos_ax`: a dozen functions do not justify a
    // binding crate.
    type CFTypeRef = *const c_void;
    type CFMachPortRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFStringRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *mut c_void;
    type TapCallback = extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    const SESSION_EVENT_TAP: u32 = 1; // kCGSessionEventTap
    const HEAD_INSERT_EVENT_TAP: u32 = 0; // kCGHeadInsertEventTap
    const TAP_LISTEN_ONLY: u32 = 1; // kCGEventTapOptionListenOnly
    const LEFT_MOUSE_DOWN: u32 = 1; // kCGEventLeftMouseDown
    const LEFT_MOUSE_UP: u32 = 2; // kCGEventLeftMouseUp
    const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
    const FIELD_CLICK_STATE: u32 = 1; // kCGMouseEventClickState
    const FIELD_WINDOW_UNDER_POINTER: u32 = 91; // kCGMouseEventWindowUnderMousePointer
    const FLAG_CONTROL: u64 = 0x0004_0000; // kCGEventFlagMaskControl
    const FLAG_COMMAND: u64 = 0x0010_0000; // kCGEventFlagMaskCommand
    const WINDOW_LIST_INCLUDING_WINDOW: u32 = 1 << 3; // kCGWindowListOptionIncludingWindow
    const CF_NUMBER_SINT64: isize = 4; // kCFNumberSInt64Type
    const RUN_LOOP_FINISHED: i32 = 1; // kCFRunLoopRunFinished

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: TapCallback,
            user_info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
        fn CGEventGetFlags(event: CGEventRef) -> u64;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
        static kCGWindowOwnerPID: CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;
        fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: CFMachPortRef, order: isize)
            -> CFRunLoopSourceRef;
        fn CFMachPortInvalidate(port: CFMachPortRef);
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source_handled: u8) -> i32;
        fn CFRunLoopStop(run_loop: CFRunLoopRef);
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: CFTypeRef, kind: isize, out: *mut c_void) -> u8;
    }

    /// What the tap callback works with. Lives on the watcher thread's
    /// stack for as long as the tap exists, and is only touched there.
    struct TapState {
        tap: Cell<CFMachPortRef>,
        down: Cell<Option<(i32, i32)>>,
        sink: Sink,
    }

    /// The process that owns window number `window`, if any. Window owners
    /// are readable without the Screen Recording permission (titles are
    /// not, and are not needed).
    fn owner_pid(window: u32) -> Option<i64> {
        if window == 0 {
            return None;
        }
        // SAFETY: the copied array is ours and released below; the values
        // read from it are borrowed from it while it lives.
        unsafe {
            let list = CGWindowListCopyWindowInfo(WINDOW_LIST_INCLUDING_WINDOW, window);
            if list.is_null() {
                return None;
            }
            let mut pid = None;
            if CFArrayGetCount(list) > 0 {
                let info = CFArrayGetValueAtIndex(list, 0);
                let number = CFDictionaryGetValue(info, kCGWindowOwnerPID);
                let mut value: i64 = 0;
                if !number.is_null() && CFNumberGetValue(number, CF_NUMBER_SINT64, (&mut value as *mut i64).cast()) != 0
                {
                    pid = Some(value);
                }
            }
            CFRelease(list);
            pid
        }
    }

    fn point(event: CGEventRef) -> (i32, i32) {
        // SAFETY: `event` is the live event the tap was called with.
        let p = unsafe { CGEventGetLocation(event) };
        (p.x.round() as i32, p.y.round() as i32)
    }

    fn handle(st: &TapState, kind: u32, event: CGEventRef) {
        match kind {
            TAP_DISABLED_BY_TIMEOUT | TAP_DISABLED_BY_USER_INPUT => {
                // macOS switches off a tap it finds slow; switch it back on.
                let tap = st.tap.get();
                if !tap.is_null() {
                    // SAFETY: our own live tap.
                    unsafe { CGEventTapEnable(tap, true) };
                }
            }
            LEFT_MOUSE_DOWN => st.down.set(Some(point(event))),
            LEFT_MOUSE_UP => {
                let Some(down) = st.down.take() else { return };
                let at = point(event);
                // SAFETY: plain field reads from the live event.
                let (clicks, flags, window) = unsafe {
                    (
                        CGEventGetIntegerValueField(event, FIELD_CLICK_STATE),
                        CGEventGetFlags(event),
                        CGEventGetIntegerValueField(event, FIELD_WINDOW_UNDER_POINTER),
                    )
                };
                // The system counts clicks with the user's double-click speed.
                if !is_drag(down, at) && clicks < 2 {
                    return;
                }
                if owner_pid(window as u32) == Some(i64::from(std::process::id())) {
                    return;
                }
                (st.sink)(Gesture::Selected { at, ctrl: flags & (FLAG_CONTROL | FLAG_COMMAND) != 0 });
            }
            _ => {}
        }
    }

    extern "C" fn on_event(
        _proxy: CGEventTapProxy,
        kind: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        // A panic must not unwind into CoreGraphics.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: `user_info` is the `TapState` the watcher thread keeps
            // alive while its run loop, which calls this, runs.
            let st = unsafe { &*(user_info as *const TapState) };
            handle(st, kind, event);
        }));
        // A listen-only tap's return value is ignored.
        event
    }

    pub struct Watcher {
        stop: Arc<AtomicBool>,
        /// The watcher thread's run loop, retained (a `CFRunLoopRef`).
        run_loop: usize,
        thread: Option<JoinHandle<()>>,
    }

    impl Watcher {
        pub fn start(sink: Sink) -> crate::Result<Watcher> {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let (ready_tx, ready_rx) = mpsc::channel::<Option<usize>>();
            let thread = std::thread::Builder::new()
                .name("glossa-mouse".into())
                .spawn(move || {
                    let state = TapState { tap: Cell::new(std::ptr::null_mut()), down: Cell::new(None), sink };
                    let mask = (1u64 << LEFT_MOUSE_DOWN) | (1u64 << LEFT_MOUSE_UP);
                    // SAFETY: `state` outlives the tap, which is disabled,
                    // invalidated and released below before `state` drops;
                    // every CF object created here is released here.
                    unsafe {
                        let tap = CGEventTapCreate(
                            SESSION_EVENT_TAP,
                            HEAD_INSERT_EVENT_TAP,
                            TAP_LISTEN_ONLY,
                            mask,
                            on_event,
                            &state as *const TapState as *mut c_void,
                        );
                        if tap.is_null() {
                            let _ = ready_tx.send(None);
                            return;
                        }
                        state.tap.set(tap);
                        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                        if source.is_null() {
                            CFMachPortInvalidate(tap);
                            CFRelease(tap);
                            let _ = ready_tx.send(None);
                            return;
                        }
                        let run_loop = CFRunLoopGetCurrent();
                        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
                        CGEventTapEnable(tap, true);
                        CFRetain(run_loop);
                        let _ = ready_tx.send(Some(run_loop as usize));
                        // In slices, so a stop that lands between two runs
                        // (where `CFRunLoopStop` would be missed) still ends
                        // the loop within a second.
                        while !flag.load(Ordering::Relaxed) {
                            if CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0) == RUN_LOOP_FINISHED {
                                break;
                            }
                        }
                        CGEventTapEnable(tap, false);
                        CFMachPortInvalidate(tap);
                        CFRelease(source);
                        CFRelease(tap);
                    }
                })
                .map_err(|e| crate::platform_err(format!("mouse watcher: {e}")))?;
            match ready_rx.recv() {
                Ok(Some(run_loop)) => Ok(Watcher { stop, run_loop, thread: Some(thread) }),
                _ => {
                    let _ = thread.join();
                    // Show the system's Input Monitoring prompt (once); either
                    // permission lets the tap be made.
                    use crate::permissions::{request, Permission};
                    request(Permission::InputMonitoring);
                    Err(crate::platform_err(
                        "macOS does not let Glossa watch the mouse: allow it under System Settings > Privacy & \
                         Security > Accessibility (or Input Monitoring), then turn mouse mode off and on again",
                    ))
                }
            }
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if self.run_loop != 0 {
                // SAFETY: a run loop we retained; stopping one from another
                // thread is allowed.
                unsafe { CFRunLoopStop(self.run_loop as CFRunLoopRef) };
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            if self.run_loop != 0 {
                // SAFETY: the reference retained in `start`.
                unsafe { CFRelease(self.run_loop as CFTypeRef) };
                self.run_loop = 0;
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
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

    #[test]
    fn supported_where_there_is_a_watcher() {
        if cfg!(any(windows, target_os = "macos")) {
            assert!(MouseWatcher::supported());
        }
    }

    /// Installs the real hook or event tap and removes it again; nothing is
    /// clicked. On macOS the process (or the terminal running it) needs
    /// Accessibility or Input Monitoring:
    /// `cargo test -p glossa-platform mouse -- --ignored`.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    #[ignore = "needs a desktop session"]
    fn the_watcher_starts_and_stops_live() {
        let started = std::time::Instant::now();
        let w = MouseWatcher::start(|g| println!("{g:?}")).expect("watcher");
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(w);
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "stopping took {:?}", started.elapsed());
    }
}
