# third_party

Vendored crates with small local fixes. Each is wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`; drop the patch once the fix
is released upstream.

## eframe 0.37.0 (MIT OR Apache-2.0, https://github.com/emilk/egui)

Unchanged except for two fixes, marked `Glossa patch` in the source:

- `src/native/run.rs` / `src/native/event_loop_context.rs`: on Windows, eframe
  paints invisible windows directly from `check_redraw_requests`, outside the
  event-loop context. An immediate viewport opened during such a pass (the
  popup, OCR overlay, options… opened from a hotkey or the tray while the main
  window is hidden) cannot create its window, and egui then panics with
  "egui backend is implemented incorrectly - the user callback was never
  called". The passes now run with the event loop available.
- `src/native/wgpu_integration.rs`: when wgpu presents through
  DirectComposition (DX12 with `Dx12SwapchainKind::DxgiFromVisual`, which
  Glossa asks for on Windows while a backdrop or window transparency is on),
  a transparent window is created with `WS_EX_NOREDIRECTIONBITMAP`. Without
  it the composition visual sits on top of the window's unpainted, opaque
  redirection surface, and neither the desktop nor Mica / Acrylic shows
  through. egui's `ViewportBuilder` has no setting for it.
