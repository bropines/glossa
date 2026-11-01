# glossa-platform

Operating-system integration. The root [AGENTS.md](../../AGENTS.md) applies;
these are the rules particular to this crate. The feature table at the top
of `src/lib.rs` says what each system supports.

## Layout

- One file per system: `foo.rs` holds the common API and picks
  `foo/win.rs`, `foo/mac.rs` or `foo/linux.rs` with `cfg`. New code follows
  that layout (see `spell`, `secrets`, `shell`, `backdrop`, `window`).
- Every public function compiles on every OS. Where a system cannot do it,
  return `Err(glossa_core::Error::Platform(..))` or `None`; never panic, and
  never leave a function missing on one OS.
- "Linux" is `not(any(windows, target_os = "macos"))`: the BSDs build the
  same code. Linux is not tested on real hardware yet.
- Differences in how windows behave go into `shell::SHELL`
  (`src/shell.rs` and `shell/{win,mac,linux}.rs`), so the UI asks a
  question instead of testing an OS.
- When a feature gains or loses support somewhere, update the table in
  `src/lib.rs` and `session.rs` (`Session::limitations`, which
  `glossa-cli doctor` and the UI show).

## Threads

- Nothing here may block the UI thread. Selection capture, screen capture,
  portal calls and spell checking on Linux can take tens to hundreds of
  milliseconds; callers run them on a worker.
- Exceptions: `hotkeys::HotkeyService` is created on the event-loop thread
  on Windows (its hotkeys belong to that thread's message queue). AppKit
  objects (macOS spell checking, backdrops, window corners) are touched
  only on the main thread; such calls check `MainThreadMarker` and return
  nothing elsewhere.

## Wayland portals

`src/portal.rs` talks to `org.freedesktop.portal.Desktop` over D-Bus:
GlobalShortcuts (hotkeys), Screenshot (capture), RemoteDesktop (copy, paste
and typing through `input.rs`). One connection per process; a request's
`Response` signal is subscribed to before the call. The RemoteDesktop
restore token is kept in the data folder as `remote-desktop-token`
(`set_state_dir`). The portal tests run a fake portal on the session bus
and skip themselves without one (run them under `dbus-run-session`). No
real KDE or GNOME desktop has been tried yet, so say "untested" in docs and
messages.

## Testing

- `../../scripts/wsl-cargo.sh test -p glossa-platform` for the Linux code,
  `clippy -p glossa-platform --all-targets --target x86_64-pc-windows-gnu
  -- -D warnings` for the Windows code. macOS code is checked on a Mac or
  in CI.
- Tests that need a real desktop, screen or keyboard are `#[ignore]`.
