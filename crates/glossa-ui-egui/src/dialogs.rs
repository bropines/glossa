//! The system's file dialogs, without stopping the interface.
//!
//! A native file dialog is modal and may stay open for minutes, so it is
//! never called from the code that draws a window. A click starts one here
//! under a slot (an id the caller makes up); the dialog runs on a worker
//! thread and its answer waits in the slot until the caller's next frame
//! takes it:
//!
//! ```ignore
//! let slot = egui::Id::new("ocr-save-dir");
//! if ui.button(browse).clicked() {
//!     dialogs::pick_folder_async(ui.ctx(), slot);
//! }
//! if let Some(dir) = dialogs::picked(slot) {
//!     draft.save_image_dir = Some(dir);
//! }
//! ```
//!
//! Why a worker thread works on every system, macOS included, where a file
//! panel may only run on the main thread: `rfd`'s blocking dialogs called
//! from another thread hand the panel to the main thread themselves (a
//! synchronous dispatch to the main queue, which the running application's
//! event loop serves) and wait for it there. The worker blocks; the main
//! thread runs the panel's modal session as AppKit wants it and goes back
//! to drawing afterwards. On Windows the dialog gets its own COM apartment
//! on the worker, and on Linux `rfd` talks to the desktop portal (or GTK on
//! a thread of its own). `rfd`'s `AsyncFileDialog` would need an executor
//! for the same result.
//!
//! One dialog at a time: a second click while one is open is ignored, since
//! two modal dialogs over each other serve nobody.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use parking_lot::Mutex;

/// A file type offered in a dialog: its name and extensions (no dots).
pub type Filter = (String, Vec<String>);

/// A dialog that is open, or answered and not yet taken.
struct Pending {
    slot: egui::Id,
    rx: Receiver<Vec<PathBuf>>,
    /// The answer and when it came, once the worker has sent it.
    answer: Option<(Vec<PathBuf>, std::time::Instant)>,
}

impl Pending {
    /// Fetch the answer if it has come. Returns false when the worker is
    /// gone without one.
    fn settle(&mut self) -> bool {
        if self.answer.is_none() {
            match self.rx.try_recv() {
                Ok(paths) => self.answer = Some((paths, std::time::Instant::now())),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        true
    }
}

/// How long an answer nobody takes is kept: its window was closed while
/// the dialog was open, and it must not keep later dialogs from opening.
const UNCLAIMED: std::time::Duration = std::time::Duration::from_secs(2);

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// Run `ask` (which shows a dialog and blocks) on a worker, for `slot`.
/// Returns false, and does nothing, while another dialog is open.
fn start(ctx: &egui::Context, slot: egui::Id, ask: impl FnOnce() -> Vec<PathBuf> + Send + 'static) -> bool {
    let mut pending = PENDING.lock();
    pending.retain_mut(|p| p.settle() && p.answer.as_ref().is_none_or(|(_, at)| at.elapsed() < UNCLAIMED));
    if pending.iter().any(|p| p.answer.is_none()) {
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("glossa-file-dialog".into()).spawn(move || {
        let _ = tx.send(ask());
        ctx.request_repaint();
    });
    if spawned.is_err() {
        return false;
    }
    pending.push(Pending { slot, rx, answer: None });
    true
}

fn with_filters(mut dialog: rfd::FileDialog, filters: &[Filter]) -> rfd::FileDialog {
    for (name, extensions) in filters {
        if !extensions.is_empty() {
            dialog = dialog.add_filter(name, extensions);
        }
    }
    dialog
}

/// One filter from a name and its extensions.
pub fn filter(name: impl Into<String>, extensions: &[&str]) -> Filter {
    (name.into(), extensions.iter().map(|e| e.trim_start_matches('.').to_owned()).collect())
}

/// Ask for one existing file.
pub fn pick_file_async(ctx: &egui::Context, slot: egui::Id, filters: Vec<Filter>) -> bool {
    start(ctx, slot, move || with_filters(rfd::FileDialog::new(), &filters).pick_file().into_iter().collect())
}

/// Ask for any number of existing files.
pub fn pick_files_async(ctx: &egui::Context, slot: egui::Id, filters: Vec<Filter>) -> bool {
    start(ctx, slot, move || with_filters(rfd::FileDialog::new(), &filters).pick_files().unwrap_or_default())
}

/// Ask for a folder.
pub fn pick_folder_async(ctx: &egui::Context, slot: egui::Id) -> bool {
    start(ctx, slot, || rfd::FileDialog::new().pick_folder().into_iter().collect())
}

/// Ask where to save a file, offering `file_name`.
pub fn save_file_async(ctx: &egui::Context, slot: egui::Id, file_name: String, filters: Vec<Filter>) -> bool {
    start(ctx, slot, move || {
        with_filters(rfd::FileDialog::new(), &filters).set_file_name(file_name).save_file().into_iter().collect()
    })
}

/// Whether `slot`'s dialog is still open.
pub fn waiting(slot: egui::Id) -> bool {
    PENDING.lock().iter().any(|p| p.slot == slot && p.answer.is_none())
}

/// What `slot`'s dialog was answered with, once: the paths chosen, or
/// nothing while it is open, was cancelled or was never started.
pub fn picked_all(slot: egui::Id) -> Option<Vec<PathBuf>> {
    let mut pending = PENDING.lock();
    let at = pending.iter().position(|p| p.slot == slot)?;
    if !pending[at].settle() {
        // The worker died without an answer: do not wait for it for ever.
        pending.remove(at);
        return None;
    }
    pending[at].answer.as_ref()?;
    let (paths, _) = pending.remove(at).answer?;
    Some(paths).filter(|p| !p.is_empty())
}

/// [`picked_all`] for a dialog that gives one path.
pub fn picked(slot: egui::Id) -> Option<PathBuf> {
    picked_all(slot).and_then(|mut paths| paths.drain(..).next())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait for the worker of `slot` and take its answer.
    fn answer(slot: egui::Id) -> Option<Vec<PathBuf>> {
        for _ in 0..200 {
            if let Some(paths) = picked_all(slot) {
                return Some(paths);
            }
            if !PENDING.lock().iter().any(|p| p.slot == slot) {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }

    // One test, since the slots are shared by the whole process.
    #[test]
    fn an_answer_is_taken_once_and_a_cancelled_dialog_gives_nothing() {
        let ctx = egui::Context::default();
        let (a, b) = (egui::Id::new("dialog-a"), egui::Id::new("dialog-b"));
        let (release, hold) = std::sync::mpsc::channel::<()>();
        assert!(start(&ctx, a, move || {
            let _ = hold.recv();
            vec![PathBuf::from("/tmp/x")]
        }));
        assert!(waiting(a));
        assert!(picked(a).is_none(), "still open");
        assert!(!start(&ctx, b, Vec::new), "one dialog at a time");
        assert!(!waiting(b));
        release.send(()).unwrap();
        assert_eq!(answer(a), Some(vec![PathBuf::from("/tmp/x")]));
        assert!(!waiting(a));
        assert!(picked(a).is_none(), "taken once");

        // Cancelled: no paths, and the slot is free again.
        assert!(start(&ctx, b, Vec::new));
        assert_eq!(answer(b), None);
        assert!(!waiting(b));
    }
}
