# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases follow
[Semantic Versioning](https://semver.org/). The section for a version becomes
the notes of its GitHub release.

## [Unreleased]

The first version of Glossa, a clean-room successor to QTranslate in Rust,
for Windows and Linux.

### Added

- **Desktop app** (`glossa`, egui): a popup by the pointer for the selected
  text, with favourite, dictionary, listen, copy and replace buttons and a
  service strip; the main window with source and translation panes, a
  direction bar with searchable language pickers, service tabs, a stacked
  multi-service view, back translation and drag and drop of text and images;
  dictionary and history windows; every classic options page plus OCR,
  Modules and Control API; a tray icon with the classic menu. Hotkeys wake a
  hidden app, and a second launch hands over to the running one.
- **Small resident cost**: when nothing has been on screen for a while
  (`appearance.unload_graphics_after_secs`), the GUI shuts down and the
  process waits in a plain message loop, a few MB in the tray. Fonts for CJK,
  Indic, Thai and other scripts load only when such text appears.
- **Renderers and looks**: OpenGL, GPU (wgpu) or software rendering; window
  and popup transparency; themes as plain files, with classic QTranslate
  themes read in place; icons painted in code instead of emoji glyphs.
- **Translation pipeline** (`glossa-engine`): the classic smart direction
  between a first and second language, language detection, fail-over across
  enabled services that stops on configuration faults, per-service input
  limits, back translation, jobs cancelled when superseded.
- **Built-in services**: Google, Google Dictionary, Bing (keyless through the
  translator page session, or the keyed API), DeepL, DeepLX, Yandex,
  LibreTranslate, MyMemory, Lingva and any OpenAI-compatible API.
- **Text recognition**: drag over a screen region to copy or translate its
  text, repeat the last region, or watch a region and re-translate when it
  changes. Google Lens through
  [chrome-lens-py](https://github.com/bropines/chrome-lens-py) (its warm
  daemon, else its CLI), or any external command.
- **Dictionaries**: online (Google) and offline (StarDict with dictzip, DSL,
  XDXF, tab-separated), with articles simplified for display.
- **Text to speech** with the classic slower repeat; MP3, WAV and Ogg Vorbis
  playback.
- **Modules** in JavaScript (on boa), Python (child processes over stdio,
  stopped when idle) and native shared libraries (C ABI in
  `include/glossa_module.h`, `glossa-module-sdk` for Rust), all speaking one
  protocol, with per-module network permissions and options generated into
  the settings. Classic QTranslate service scripts (with a reimplemented
  `Common.js` helper API and WSH-style HTTP objects) and pot-app plugins run
  unchanged. Examples: `free-dictionary` (JS), `example-reverse` and
  `lens-ocr` (Python), `example-rot13` (Rust) and `example-c` (C).
- **Classic QTranslate**: point Glossa at an installation to use its
  services, themes and languages in place, or import its `Options.json` and
  history with `glossa-cli import-classic`.
- **AI endpoints**: any number of OpenAI-compatible translators made from
  presets (OpenAI, OpenRouter, DeepSeek, Gemini, Groq, Mistral, Ollama, LM
  Studio, custom), each with its model, prompt, temperature or reasoning
  effort, token limit, timeout, extra headers and extra request fields; *Load
  models* and *Test* in the options. Answers stream into the popup, the main
  window and the terminal as they are written.
- **History** with favourites and export to text, CSV, HTML and JSON.
- **Command line** (`glossa-cli`): translate, detect, dict, ocr, speak,
  services, modules, history, import-classic, theme, locale, config, serve
  and action, with `--json` output and input from stdin.
- **Control API**: HTTP on loopback with a token, refusing requests from web
  pages; translate, detect, dictionary, OCR, speech and UI actions, plus
  pot-app's endpoints so its key bindings keep working.
- **Platform integration** (`glossa-platform`): reading the selection
  through UI Automation, else a synthetic copy with the clipboard restored;
  global hotkeys, screen capture, synthetic input, autostart, audio. The
  selection is read before any window opens, so a new window cannot steal it.
- **Portable mode**: a `portable` file or a `data` folder beside the
  executable keeps settings, history and the log there; `GLOSSA_HOME`
  overrides the location.
- Licenses (MIT or Apache-2.0), NOTICE, CI on Linux and Windows, release
  builds, issue templates, contributing and security guides.

### Fixed

- Windows opened while the main window was hidden (popup, OCR overlay,
  options from a hotkey or the tray) made egui panic; eframe is vendored with
  a fix (`third_party/README.md`).
- Python installed through pyenv-win, or behind another `.bat`/`.cmd` shim,
  is found.
- The control API token generated on first run is saved, so `glossa-cli
  action` and other clients can use it.
- Popup: *Auto position* keeps it on screen near the edges (or reopens it
  where it was dragged), *Auto size* off gives a fixed box; exception rules
  for controls (list views, hotkey fields, Explorer's file view) match, and
  *Smart detection* works.

### Known gaps

- Mouse mode (translate on double-click or selection without a hotkey),
  speech input and the update check are not implemented yet.
- On Wayland, global hotkeys and reading the selection depend on the
  compositor; bind keys to the control API instead.
