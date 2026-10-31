//! Waiting with no windows open.
//!
//! A tray utility spends nearly all its life idle. Glossa unloads its
//! graphics while nothing is on screen and waits here instead: a bare message
//! loop that keeps the tray icon and the global hotkeys alive (both deliver
//! their events through window messages on this thread) and costs next to
//! nothing. Something that needs a window wakes the loop with a [`Waker`].
//!
//! Only Windows has an implementation; elsewhere [`IdlePump::supported`] is
//! false and the GUI simply stays loaded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Wakes an [`IdlePump`] from any thread.
#[derive(Clone)]
pub struct Waker {
    #[cfg(windows)]
    thread_id: u32,
    flag: Arc<AtomicBool>,
}

impl Waker {
    pub fn wake(&self) {
        self.flag.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};
            // SAFETY: posting a message to a thread id has no memory-safety
            // preconditions; a dead thread just makes the call fail.
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_APP + 0x47, WPARAM(0), LPARAM(0)) };
        }
    }
}

/// A message loop on the current thread.
pub struct IdlePump {
    #[cfg(windows)]
    thread_id: u32,
    flag: Arc<AtomicBool>,
}

impl IdlePump {
    /// Whether idling without windows works on this platform.
    pub fn supported() -> bool {
        cfg!(windows)
    }

    /// A pump for the calling thread. Create it on the thread that owns the
    /// tray icon and the hotkeys.
    pub fn for_current_thread() -> IdlePump {
        IdlePump {
            #[cfg(windows)]
            // SAFETY: no preconditions.
            thread_id: unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn waker(&self) -> Waker {
        Waker {
            #[cfg(windows)]
            thread_id: self.thread_id,
            flag: self.flag.clone(),
        }
    }

    /// Dispatch window messages until `done` says so. `done` is checked
    /// before waiting and after every message, so events delivered through
    /// window procedures (hotkeys, the tray) are seen at once. Returns false
    /// if the loop was asked to quit (`WM_QUIT`).
    pub fn run_until(&self, mut done: impl FnMut() -> bool) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};
            loop {
                self.flag.store(false, Ordering::SeqCst);
                if done() {
                    return true;
                }
                let mut msg = MSG::default();
                // SAFETY: `msg` is a valid out-pointer; a null window handle
                // means "any message for this thread".
                let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if got.0 == 0 || got.0 == -1 {
                    return false;
                }
                // SAFETY: `msg` was filled in by GetMessageW.
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        #[cfg(not(windows))]
        {
            while !done() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            true
        }
    }
}

/// Hand the process's idle pages back to the system, so an app sitting in the
/// tray shows its true footprint. Harmless: pages come back on demand.
pub fn trim_working_set() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Memory::{SetProcessWorkingSetSizeEx, SETPROCESSWORKINGSETSIZEEX_FLAGS};
        use windows::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: (usize::MAX, usize::MAX) is the documented "trim" request
        // for the current process's pseudo-handle.
        let _ = unsafe { SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, SETPROCESSWORKINGSETSIZEEX_FLAGS(0)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_woken_pump_returns() {
        let pump = IdlePump::for_current_thread();
        let waker = pump.waker();
        let flag = Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            f2.store(true, Ordering::SeqCst);
            waker.wake();
        });
        assert!(pump.run_until(|| flag.load(Ordering::SeqCst)));
    }
}
