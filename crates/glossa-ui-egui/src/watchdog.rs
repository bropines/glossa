//! Notices when the UI thread stops answering, and says where it was.
//!
//! The UI thread must never block (see AGENTS.md), but when it does, the
//! user only sees a frozen window and the log shows nothing. Each frame marks
//! its start and end and names the part it is in ([`phase`]); a frame slower
//! than [`SLOW`] is logged at debug level when it ends, and a frame still
//! running after [`STUCK`] is reported at warn level by a watcher thread
//! while it is stuck, so a hang that never ends leaves a trace too.
//!
//! The watcher sleeps (parked, no polling) whenever no frame is running.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread::Thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// A frame this slow is logged when it ends.
const SLOW: Duration = Duration::from_millis(500);
/// A frame still running after this long is reported while it runs.
const STUCK: Duration = Duration::from_secs(2);

/// Milliseconds since [`epoch`] at which the running frame started, plus one;
/// 0 while no frame runs.
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);
static PHASE: Mutex<&'static str> = Mutex::new("");
/// The watcher is parked and needs an unpark to see the next frame.
static PARKED: AtomicBool = AtomicBool::new(false);
static WATCHER: OnceLock<Thread> = OnceLock::new();

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64 + 1
}

/// Time the work until the guard is dropped as one frame named `name`.
pub fn frame(name: &'static str) -> FrameGuard {
    frame_start();
    phase(name);
    FrameGuard(())
}

/// Ends its frame when dropped.
pub struct FrameGuard(());

impl Drop for FrameGuard {
    fn drop(&mut self) {
        frame_end();
    }
}

/// A frame starts.
pub fn frame_start() {
    BUSY_SINCE.store(now_ms(), Ordering::SeqCst);
    phase("frame");
    let watcher = WATCHER.get_or_init(spawn_watcher);
    if PARKED.swap(false, Ordering::SeqCst) {
        watcher.unpark();
    }
}

/// The frame ends; log it if it was slow.
pub fn frame_end() {
    let since = BUSY_SINCE.swap(0, Ordering::SeqCst);
    if since == 0 {
        return;
    }
    let took = Duration::from_millis(now_ms().saturating_sub(since));
    if took >= SLOW {
        tracing::debug!(ms = took.as_millis() as u64, last_phase = *PHASE.lock(), "slow UI frame");
    }
}

/// Name the part of the frame now running, for the reports.
pub fn phase(name: &'static str) {
    *PHASE.lock() = name;
}

fn spawn_watcher() -> Thread {
    std::thread::Builder::new()
        .name("glossa-watchdog".into())
        .spawn(watch)
        .map(|h| h.thread().clone())
        // Without a watcher the frames are still timed at their end.
        .unwrap_or_else(|_| std::thread::current())
}

fn watch() {
    let mut reported_for = 0u64;
    let mut last_report = Instant::now();
    loop {
        let since = BUSY_SINCE.load(Ordering::SeqCst);
        if since == 0 {
            PARKED.store(true, Ordering::SeqCst);
            // A frame may have started between the load and the store.
            if BUSY_SINCE.load(Ordering::SeqCst) == 0 {
                std::thread::park();
            }
            PARKED.store(false, Ordering::SeqCst);
            continue;
        }
        let running = Duration::from_millis(now_ms().saturating_sub(since));
        if running >= STUCK && (reported_for != since || last_report.elapsed() >= Duration::from_secs(10)) {
            tracing::warn!(
                ms = running.as_millis() as u64,
                phase = *PHASE.lock(),
                "the UI thread has not finished a frame; the window is not responding"
            );
            reported_for = since;
            last_report = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_timed_and_the_watcher_parks_between_them() {
        frame_start();
        phase("test");
        assert_ne!(BUSY_SINCE.load(Ordering::SeqCst), 0);
        frame_end();
        assert_eq!(BUSY_SINCE.load(Ordering::SeqCst), 0);
        // A second end without a start is harmless.
        frame_end();
        // The watcher notices the idle state and parks.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !PARKED.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(PARKED.load(Ordering::SeqCst));
    }
}
