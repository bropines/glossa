# Glossa

A fast, small, extensible desktop translator: select text anywhere, press a
hotkey, read the translation in a popup by the cursor. A clean-room successor
to QTranslate, written in Rust, for Windows and Linux (macOS builds, untested).

- **Translate the selection** in any program: a popup by the cursor, or the
  main window with the source and several services side by side.
- **Recognise text on screen** (OCR) and translate it: drag over a region,
  copy it, or watch it and re-translate when it changes. Google Lens through
  [chrome-lens-py](https://github.com/bropines/chrome-lens-py).
- **Dictionary**: online (Google) and offline (StarDict, DSL, XDXF, TSV).
- **Listen**: text to speech for the source and the translation.
- **Replace, copy, convert layout**: translate the selection in place, copy
  the translation, fix text typed in the wrong keyboard layout.
- **Many services, built in**: Google, Bing, DeepL, DeepLX, Yandex,
  LibreTranslate, MyMemory, Lingva, any OpenAI-compatible API; fail-over from
  one to the next.
- **Modules** in JavaScript, Python or as native libraries, with one
  protocol for all three. Classic QTranslate services and pot-app plugins run
  unchanged.
- **Themes** as plain files, classic QTranslate themes included; window and
  popup transparency; OpenGL, GPU or software rendering.
- **History** with favourites and export (text, CSV, HTML, JSON).
- **Command line and local API**: `glossa-cli` does everything without a
  window; other programs can drive a running Glossa over HTTP on localhost.
- **Small**: ~4 MB of memory in the tray (the window system unloads when
  nothing is on screen), one 17 MB executable, no web engine, no runtime to
  install.

## Getting started

Download the build for your system, unpack, run `glossa`. It starts in the
tray. Select text in any program and press **Ctrl+Shift+Q** for the popup or
**Ctrl+Alt+Q** for the main window; **Ctrl+Shift+O** recognises a screen
region, **Ctrl+Alt+O** recognises and translates it, **Ctrl+E** reads the
selection aloud. All hotkeys are in *Options → Hotkeys*.

Coming from QTranslate? *Options → Modules → Classic QTranslate installation*
uses its services, themes and languages in place; or import its settings and
history too:

```bash
glossa-cli import-classic "C:\Program Files (x86)\QTranslate"
```

For OCR, install chrome-lens-py (`pip install "chrome-lens-py[clipboard]"`);
Glossa finds it.

## Command line

```bash
glossa-cli translate -t ru "Hello, world"
echo "Bonjour" | glossa-cli translate -t en
glossa-cli dict -f en -t ru serendipity
glossa-cli ocr screenshot.png -t en
glossa-cli services                  # what is available
glossa-cli action show_popup "text"  # ask the running app to show a popup
```

`glossa-cli` with no arguments lists every command.

## Documentation

- [Modules](docs/modules/README.md): overview, and guides for
  [JavaScript](docs/modules/js-modules.md),
  [Python](sdk/python/README.md) and
  [native](docs/modules/native-modules.md) modules; the
  [protocol](docs/modules/protocol.md);
  [classic QTranslate and pot-app scripts](docs/modules/classic-scripts.md)
- [Themes](docs/theming.md)
- [Architecture](docs/architecture.md)
- [How the classic program behaved](docs/research/classic-behaviour.md):
  the observations the UI was modelled on

## Building

Rust 1.82 or newer.

```bash
cargo build --release -p glossa-ui-egui -p glossa-cli
```

Linux needs the GTK 3 and X11 development packages for the tray and hotkeys
(`libgtk-3-dev libxdo-dev libxcb-shape0-dev libxcb-xfixes0-dev` on Debian).
Windows binaries cross-compile from Linux with the
`x86_64-pc-windows-gnu` target and mingw-w64. Contributors: see
[AGENTS.md](AGENTS.md).

## Status

Working: everything above, on Windows (tested) and Linux X11. Not yet: mouse
mode (translate on double-click or selection without a hotkey), speech input,
the update check. On Wayland, global hotkeys and reading the selection
depend on the compositor.

## Clean room

Glossa contains no code from QTranslate. Its behaviour was studied from the
outside: the program's documented data files (options, themes, languages,
the JavaScript service scripts and their helper API, which users already
edit) and by observing the running program. Classic scripts and themes are
not shipped; Glossa reads them from an existing installation.

## License

MIT or Apache-2.0, at your option.
