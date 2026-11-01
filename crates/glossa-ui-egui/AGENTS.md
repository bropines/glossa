# glossa-ui-egui

The desktop app (`glossa`). The root [AGENTS.md](../../AGENTS.md) applies;
these are the rules particular to this crate.
[docs/architecture.md](../../docs/architecture.md) explains the windows,
the threads and why the graphics unload.

## The UI thread

- Draw and dispatch only. Network, other programs, the disk beyond the
  config file, selection and screen capture run on a worker; results come
  back through the engine's events (`engine.subscribe()`) or the app's task
  channel, followed by a repaint request.
- Logic another front end would need belongs in `glossa-engine`, so
  `glossa-cli` and the control API get it too.
- Redraw only when something changes. A window that repaints every frame
  keeps the CPU awake in the tray; check idle CPU after touching animation
  or polling.
- Selection capture for hotkeys happens before any window opens
  (`actions::read_selection`).

## Per-OS behaviour

Ask `glossa_platform::shell::SHELL` (own frame, Cmd or Ctrl, menu bar,
points or pixels…) instead of writing `cfg(target_os)` here. OS calls go
into `glossa-platform`.

## Theme and Look

- Colours, radii, sizes and fonts come from the theme
  (`glossa_core::theme`) through `style::Look` (`app.look`), never
  hard-coded. Check every change with a dark and a light theme (Glossa,
  Fluent, macOS) and with a classic QTranslate theme.
- Icons are painted with `icons::paint`. No emoji or symbol glyphs: most
  fonts lack them.
- Theme files and keys: [docs/theming.md](../../docs/theming.md).

## Strings

Every user-facing string goes through `app.t("key")` (or `locale.fmt`).
A new key needs both:

1. the English text in `STRINGS` in `crates/glossa-core/src/locale.rs`;
2. the Russian text in `crates/glossa-core/locales/ru.json`.

`tests/locale_keys.rs` fails when the UI names a key `STRINGS` lacks, and
`glossa-core`'s tests fail when `ru.json` misses one or keeps a stale one.
Keep the `%s` placeholders the same in both.

## Settings

A new setting gets a field in `glossa_core::config` with `#[serde(default)]`
(an alias when renamed), a control on the right options page, and a row in
[docs/configuration.md](../../docs/configuration.md).

## Trying it

Build with `scripts/wsl-cargo.sh build -p glossa-ui-egui --target
x86_64-pc-windows-gnu` and run it with a throwaway `GLOSSA_HOME`, as the
root AGENTS.md shows. `--screenshot WHAT FILE` renders one window to a PNG
and exits; `--popup TEXT` opens a popup at start. Never touch windows or
processes you did not start.
