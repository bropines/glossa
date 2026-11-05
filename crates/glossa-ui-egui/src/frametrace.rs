//! Frame tracing for diagnosing stutter: with `GLOSSA_FRAME_TRACE` set,
//! every frame of the traced windows adds a line to a CSV file (when it
//! started, the time since that window's previous frame, how long its UI
//! took, and the scroll events it consumed). Without the variable this is
//! one cached check per frame.
//!
//! `GLOSSA_FRAME_TRACE` is a file path, or `1` for `frames.csv` in the
//! cache folder.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

struct Trace {
    out: Mutex<BufWriter<File>>,
    start: Instant,
    /// Each window's previous frame start, in ms since `start`.
    last: Mutex<std::collections::HashMap<&'static str, f64>>,
}

static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

/// Open the trace file, if `GLOSSA_FRAME_TRACE` asks for one. `cache` is
/// the cache folder, for `GLOSSA_FRAME_TRACE=1`.
pub fn init(cache: &std::path::Path) {
    TRACE.get_or_init(|| {
        let v = std::env::var_os("GLOSSA_FRAME_TRACE")?;
        let path = if v == "1" { cache.join("frames.csv") } else { PathBuf::from(v) };
        let file = match File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("frame trace {}: {e}", path.display());
                return None;
            }
        };
        tracing::info!("tracing frames to {}", path.display());
        let mut out = BufWriter::new(file);
        let _ = writeln!(out, "start_ms,window,since_last_ms,ui_ms,wheel_events,wheel_dy,wheel_kinds");
        Some(Trace { out: Mutex::new(out), start: Instant::now(), last: Mutex::new(Default::default()) })
    });
}

/// Records one frame of `window` when dropped.
pub struct Frame {
    window: &'static str,
    began: Instant,
    wheel: (usize, f32, String),
}

/// Start tracing a frame of `window`; keep the guard for the frame's UI.
pub fn begin(ctx: &egui::Context, window: &'static str) -> Option<Frame> {
    TRACE.get()?.as_ref()?;
    let wheel = ctx.input(|i| {
        let mut n = 0;
        let mut dy = 0.0;
        let mut kinds = String::new();
        for e in &i.raw.events {
            if let egui::Event::MouseWheel { delta, phase, source, .. } = e {
                n += 1;
                dy += delta.y;
                let p = match phase {
                    egui::TouchPhase::Start => 'S',
                    egui::TouchPhase::Move => 'M',
                    egui::TouchPhase::End => 'E',
                    egui::TouchPhase::Cancel => 'C',
                };
                let s = match source {
                    egui::MouseWheelSource::Trackpad => 't',
                    egui::MouseWheelSource::Momentum => 'm',
                    egui::MouseWheelSource::Wheel => 'w',
                    egui::MouseWheelSource::Unknown => '?',
                };
                kinds.push(p);
                kinds.push(s);
            }
        }
        (n, dy, kinds)
    });
    Some(Frame { window, began: Instant::now(), wheel })
}

impl Drop for Frame {
    fn drop(&mut self) {
        let Some(Some(t)) = TRACE.get() else { return };
        let start_ms = self.began.duration_since(t.start).as_secs_f64() * 1000.0;
        let ui_ms = self.began.elapsed().as_secs_f64() * 1000.0;
        let since = {
            let mut last = t.last.lock().unwrap_or_else(|e| e.into_inner());
            let prev = last.insert(self.window, start_ms);
            prev.map_or(0.0, |p| start_ms - p)
        };
        let (n, dy, kinds) = &self.wheel;
        let mut out = t.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{start_ms:.2},{},{since:.2},{ui_ms:.2},{n},{dy:.1},{kinds}", self.window);
        // A trace is read while the program runs: keep the file current.
        let _ = out.flush();
    }
}
