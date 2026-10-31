# third_party

Vendored crates with small local fixes. Each is wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`; drop the patch once the fix
is released upstream.

## eframe 0.37.0 (MIT OR Apache-2.0, https://github.com/emilk/egui)

Unchanged except for one fix, marked `Glossa patch` in the source:

- `src/native/run.rs` / `src/native/event_loop_context.rs`: on Windows, eframe
  paints invisible windows directly from `check_redraw_requests`, outside the
  event-loop context. An immediate viewport opened during such a pass (the
  popup, OCR overlay, options… opened from a hotkey or the tray while the main
  window is hidden) cannot create its window, and egui then panics with
  "egui backend is implemented incorrectly - the user callback was never
  called". The passes now run with the event loop available.
