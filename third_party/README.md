# third_party

Vendored crates with small local fixes. Each is wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`; drop the patch once the fix
is released upstream.

## eframe 0.37.0 (MIT OR Apache-2.0, https://github.com/emilk/egui)

Unchanged except for these fixes, marked `Glossa patch` in the source:

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
- `src/native/glow_integration.rs`, hidden windows: on macOS
  `ViewportInfo::visible()` stays unknown for a window hidden with
  `ViewportCommand::Visible(false)`, so a hidden root window (Glossa's main
  window while it sits in the tray) was painted and presented every frame,
  then slept 10 ms as an invisible window, and the immediate viewports drawn
  in its pass (the popup, Options…) were held back with it. A window winit
  reports hidden or minimised is no longer painted once it has been painted
  once, since eframe shows a new window only after its first paint. On Windows
  this also skips a minimised main window, which used to be painted. A hidden
  window whose pass paints a visible immediate child does not sleep. Which
  windows run their UI pass is unchanged.
- `src/native/glow_integration.rs` and `src/native/macos.rs`, frame pacing
  (macOS): every window's swap is vsync'd and all of them share one thread
  and GL context, so a pass that painted the main window and Options waited
  for two refreshes, and frames started at an uneven 5 ms / 16 ms cadence that
  made scrolling judder. An immediate viewport painted in the same pass as a
  visible root window now swaps without waiting, the root window's swap sets
  the interval back, and root passes are paced to the refresh rate of the
  fastest screen showing a window (`NSScreen.maximumFramesPerSecond`, which
  needs the `NSScreen` feature of `objc2-app-kit`). A window shown alone keeps
  its vsync'd swap: a translucent window swapping without waiting asked AppKit
  for another frame at once and redrew forever.
