//! What modules log, for whoever is watching one module.
//!
//! Module log lines (`host.log`, `console.*`, a process module's stderr)
//! go to `tracing` and so to `glossa.log`. The module editor also wants
//! them for the module being edited, without reading the log file: it opens
//! a [`LogTap`] for that module id and every line logged under the id while
//! the tap lives is kept in it too, up to [`KEEP`] lines.
//!
//! With no tap open, [`emit`] costs one atomic load.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

/// Lines a tap keeps; older ones are dropped.
pub const KEEP: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// `trace`, `debug`, `info`, `warn`, `error`, or `stderr` for a
    /// process's own output.
    pub level: String,
    pub message: String,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    module: String,
    lines: Mutex<VecDeque<LogLine>>,
    /// Lines ever added, so a reader can tell something new arrived.
    added: AtomicUsize,
    wake: Option<Wake>,
}

static OPEN: AtomicUsize = AtomicUsize::new(0);
static TAPS: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());

/// The lines one module logs while this lives.
pub struct LogTap {
    shared: Arc<Shared>,
}

impl LogTap {
    /// Start keeping what `module` logs. `wake` is called after each line
    /// (to repaint a window), from whatever thread logged it.
    pub fn open(module: &str, wake: Option<Wake>) -> LogTap {
        let shared = Arc::new(Shared {
            module: module.to_owned(),
            lines: Mutex::new(VecDeque::new()),
            added: AtomicUsize::new(0),
            wake,
        });
        let mut taps = TAPS.lock();
        taps.retain(|t| t.strong_count() > 0);
        taps.push(Arc::downgrade(&shared));
        OPEN.store(taps.len(), Ordering::Release);
        LogTap { shared }
    }

    pub fn module(&self) -> &str {
        &self.shared.module
    }

    /// Every line kept, oldest first.
    pub fn lines(&self) -> Vec<LogLine> {
        self.shared.lines.lock().iter().cloned().collect()
    }

    /// How many lines were ever added: changes whenever one arrives.
    pub fn generation(&self) -> usize {
        self.shared.added.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        self.shared.lines.lock().clear();
        self.shared.added.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for LogTap {
    fn drop(&mut self) {
        let mut taps = TAPS.lock();
        let me = Arc::downgrade(&self.shared);
        taps.retain(|t| t.strong_count() > 0 && !t.ptr_eq(&me));
        OPEN.store(taps.len(), Ordering::Release);
    }
}

/// Hand a module's log line to the taps open for it.
pub fn emit(module: &str, level: &str, message: &str) {
    if OPEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let taps: Vec<Arc<Shared>> = TAPS.lock().iter().filter_map(Weak::upgrade).filter(|t| t.module == module).collect();
    for tap in taps {
        {
            let mut lines = tap.lines.lock();
            if lines.len() >= KEEP {
                lines.pop_front();
            }
            lines.push_back(LogLine { level: level.to_owned(), message: message.to_owned() });
        }
        tap.added.fetch_add(1, Ordering::AcqRel);
        if let Some(wake) = &tap.wake {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_sees_its_own_module_only_while_it_lives() {
        emit("tap-a", "info", "before");
        let tap = LogTap::open("tap-a", None);
        emit("tap-a", "info", "one");
        emit("tap-b", "info", "other module");
        emit("tap-a", "error", "two");
        let lines = tap.lines();
        assert_eq!(lines.iter().map(|l| l.message.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(lines[1].level, "error");
        assert_eq!(tap.generation(), 2);
        drop(tap);
        let again = LogTap::open("tap-a", None);
        assert!(again.lines().is_empty());
    }
}
