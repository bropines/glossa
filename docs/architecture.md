# Architecture

Glossa is a Cargo workspace. The rule that shapes it: **everything a user can
do exists below the UI**. The desktop app, the command line and the control
API are three front ends over one engine, and another toolkit could be a
fourth without touching the rest.

```
                       glossa-ui-egui (glossa)      glossa-cli
                             │      │                    │
                 glossa-platform    └──── glossa-engine ─┘
                 (OS integration)          │   │   │   │
                             ┌─────────────┘   │   │   └──────────┐
                     glossa-services   glossa-ocr  glossa-dict    │
                     (built-in APIs)   (Lens…)     (StarDict…)    │
                             └───────────┬─────┴───────┘          │
                                   glossa-modules ────────────────┘
                          (provider API; JS / Python / native runtimes)
                                   │            │
                              glossa-net   glossa-module-sdk
                                   │        (wire types, C ABI)
                              glossa-core
                 (languages, config, themes, history, errors; no I/O)
```

## Crates

| crate | role |
|---|---|
| `glossa-core` | The domain model: the language table, capabilities, requests and results, `config.toml`, themes, locales, history, hotkey and action definitions, the classic-format importers. Pure data and logic: no network, no threads, no UI. |
| `glossa-net` | HTTP on `ureq`: proxies (including the Windows system proxy), timeouts, retries, legacy code pages, an HTTP mock for tests. |
| `glossa-module-sdk` | What a module author depends on: the protocol's wire types, the C ABI (`include/glossa_module.h`), and `export_module!` for Rust modules. Depends on nothing else in the workspace. |
| `glossa-modules` | The `Provider` trait every translator, dictionary, voice and OCR engine implements; the host API given to modules (scoped HTTP, cache, log); the registry that scans module folders; the runtimes: JavaScript (Glossa, classic QTranslate and pot-app dialects, on boa), Python (stdio child processes), native (shared libraries). |
| `glossa-services` | Built-in providers in Rust: Google, Google Dictionary, Bing (keyless web session or keyed API), DeepL, DeepLX, LibreTranslate, MyMemory, Lingva, OpenAI-compatible, Yandex. |
| `glossa-ocr` | OCR providers: Google Lens through `chrome-lens-py` (warm daemon, else its CLI), and any external command. |
| `glossa-dict` | Offline dictionaries (StarDict with dictzip, DSL, XDXF, tab-separated) and the HTML simplifier for dictionary articles. |
| `glossa-engine` | The front ends' shared core: the keyless JavaScript service modules compiled in from `modules/bundled/`, the translation pipeline (direction detection, fail-over, back translation, input limits), async jobs with cancellation, history, the theme and locale catalogues, exports, the classic import, and the control API (HTTP on localhost with a token). |
| `glossa-platform` | Everything OS-specific: clipboard, reading the selection (UI Automation, then a synthetic copy), synthetic input, screen capture, global hotkeys, the mouse watcher for mouse mode, autostart, audio playback, window placement, window backdrops (Mica, Acrylic, vibrancy), the system accent colour and dark-mode preference, the idle message pump. |
| `glossa-ui-egui` | The desktop app (binary `glossa`): main window, selection popup, OCR overlay, dictionary, history, options, tray. |
| `glossa-cli` | The command line (binary `glossa-cli`). |

`third_party/eframe` is eframe 0.37 with one fix; see
[../third_party/README.md](../third_party/README.md).

## How a translation flows

1. A hotkey fires (`glossa-platform::hotkeys`) or the user types.
2. For selection actions the app reads the selection: UI Automation first
   (instant, leaves the clipboard alone), else a synthetic Ctrl+C with the
   clipboard saved and restored.
3. `Engine::translate_async` runs the pipeline on a worker thread: normalise
   the text, pick the direction (detecting the language when the source is
   *auto*, swapping for "smart" pairs), then try the active service and fall
   over to the next enabled one on failure.
4. A provider is called with a `Host` (scoped HTTP, cache, options). A module
   provider turns the call into a protocol request for its runtime.
5. The result returns as an engine event; the UI shows it and records it in
   history.

The CLI calls the blocking versions of the same functions.

## The desktop app and memory

The goal is a resident cost close to the classic program's. The app is split
in two:

- **The shell**, which lives as long as the process: the engine, hotkeys, the
  tray icon, the control API, clipboard and audio. In the tray it costs a few
  MB.
- **The GUI**, eframe with its OpenGL (or wgpu) context, fonts and windows,
  which is the expensive part (≈75 MB with OpenGL).

When no window has been visible for `appearance.unload_graphics_after_secs`
(default 45), the GUI shuts down completely and the process waits in a plain
message loop (`glossa-platform::idle`), with its working set trimmed. A
hotkey, a tray click or a control-API call brings it back in a few hundred
milliseconds. Things that must happen before any window exists (reading the
selection, whose focus a new window would steal) are done in that loop.

Other choices made for size: fonts for CJK, Arabic, Indic and other scripts
are loaded only when text in that script first appears; a Python module's
process starts on first use and stops when idle; the renderer is selectable
(OpenGL is the lightest).

## Threads

- The UI thread runs egui. It never blocks: anything that touches another
  program or the network runs on a worker and reports back through a channel
  (`actions::Task`, engine events).
- Engine jobs run on short-lived threads with a cancellation token per slot
  (main window, popup, dictionary…); a newer job in a slot cancels the older.
- Each Python module has one process; calls to it are serialised.
- The control API runs on its own thread.

## Configuration and data

`config.toml` in the data folder holds every setting, readable and editable
by hand; the app rewrites it when settings are applied. History is
`history.json`, the log is `glossa.log` in the cache folder. Portable mode
(a `portable` file or a `data` folder next to the executable) keeps all of it
beside the program; `GLOSSA_HOME` overrides the location.

## Testing

Unit tests sit next to the code (`cargo test --workspace`). Network-facing
code is tested against `glossa_net::mock::MockHttp`; live tests against real
services are `#[ignore]`d and run on request. Build and test in WSL; see
[../AGENTS.md](../AGENTS.md).
