<p align="center">
  <img src="assets/icons/glossa.png" width="76" height="76" alt="">
</p>

<h1 align="center">Glossa</h1>

<p align="center">
  <b>A fast, small, extensible desktop translator.</b><br>
  Select text anywhere, press a hotkey, read the translation by the cursor. A clean-room successor to QTranslate, in Rust.
</p>

<p align="center">
  <a href="https://github.com/bropines/glossa/actions/workflows/ci.yml"><img src="https://github.com/bropines/glossa/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/bropines/glossa/releases/latest"><img src="https://img.shields.io/github/v/release/bropines/glossa?label=release" alt="Latest release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="MIT OR Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555" alt="Windows, macOS and Linux">
</p>

<p align="center">
  <img src="docs/screenshots/popup-dark.png" alt="The popup with a translation next to selected text, the service strip shown on hover" width="880">
</p>

<table>
  <tr>
    <td width="33%" valign="top"><img src="docs/screenshots/main-dark.png" alt="The main window with source and translation panes and service tabs"></td>
    <td width="33%" valign="top"><img src="docs/screenshots/options-ai-dark.png" alt="AI endpoints in the options: OpenRouter and Ollama made from presets"></td>
    <td width="33%" valign="top"><img src="docs/screenshots/dict-dark.png" alt="The dictionary window with an article"></td>
  </tr>
  <tr>
    <td align="center"><sub>The main window</sub></td>
    <td align="center"><sub>AI endpoints from presets</sub></td>
    <td align="center"><sub>Dictionary</sub></td>
  </tr>
</table>

## Contents

- [Features](#features) · [Getting started](#getting-started) · [macOS](#macos) · [Coming from QTranslate](#coming-from-qtranslate)
- [AI translators](#ai-translators) · [Modules](#modules) · [Command line](#command-line)
- [Documentation](#documentation) · [Building](#building) · [Status](#status) · [Clean room](#clean-room) · [License](#license)

## Features

- **Translate the selection** in any program: a popup by the cursor, or the
  main window with the source and several services side by side. The popup
  can be dragged, resized (the size is remembered) and pinned.
- **Mouse mode**, as in QTranslate: select text with the mouse and a button
  by the pointer (or the translation itself) appears, no hotkey needed. A
  click on the tray icon turns it on and off; the icon changes colour.
- **Recognise text on screen** (OCR) and translate it: drag over a region,
  copy its text, translate it in place over the picture, or watch it and
  re-translate when it changes. Google Lens through
  [chrome-lens-py](https://github.com/bropines/chrome-lens-py); offline with
  Windows OCR or the Snipping Tool's OneOCR engine on Windows, Apple Vision
  on macOS, and Tesseract anywhere; or through a module or any command.
- **Dictionary**: online (Google, Yandex, Multitran, ABBYY Lingvo Live,
  Oxford Learner's, Wiktionary, Youdao, Academic, Wikipedia, Urban
  Dictionary, and the classic dictionary scripts) and offline (StarDict, DSL,
  XDXF, TSV).
- **Listen**: text to speech for the source and the translation.
- **Replace, copy, convert layout**: translate the selection in place, copy
  the translation, fix text typed in the wrong keyboard layout.
- **Many services, built in**: Google, Bing, Yandex, DeepL, Lara, ModernMT
  and MyMemory with no key or account; the DeepL and Yandex Cloud APIs,
  DeepLX, LibreTranslate and Lingva with your own key or server; fail-over
  from one to the next. The keyless web services ship as readable JavaScript
  modules inside the executable ([list](docs/modules/README.md#bundled-modules)).
- **AI translators**: any number of OpenAI-compatible endpoints (OpenAI,
  OpenRouter, DeepSeek, Gemini, Groq, Mistral, a local Ollama or LM Studio),
  made from presets, with the answer streaming into the popup as it is written.
- **Modules** in JavaScript, Python, Java or any language over stdio, or
  native libraries (Rust, C, anything with a C ABI), with one protocol for
  all of them. Classic QTranslate services and pot-app plugins run unchanged.
- **Themes** as plain files, classic QTranslate themes included; Fluent
  (WinUI 3) themes following the system accent colour; Mica, Acrylic or blur
  backdrops on Windows 11 (vibrancy on macOS); window and popup
  transparency; Ctrl+wheel zooms the text areas; OpenGL, GPU or software
  rendering.
- **History** with favourites and export (text, CSV, HTML, JSON).
- **Command line and local API**: `glossa-cli` does everything without a
  window; other programs can drive a running Glossa over HTTP on localhost.
- **Small**: a few MB of memory in the tray (the window system unloads when
  nothing is on screen), one 17 MB executable, no web engine, no runtime to
  install.

## Getting started

Download Glossa for your system from the
[releases](https://github.com/bropines/glossa/releases): the installer on
Windows, the `.dmg` on macOS, the `.deb` or the AppImage on Linux, or an
archive to unpack anywhere ([Installing](docs/install.md) has the details).
Run `glossa`; it starts in the tray. Select text in any program and press:

| hotkey | Windows | does |
|---|---|---|
| **Ctrl+Shift+Q** | same | translate the selection in a popup |
| **Ctrl+Alt+Q** | **Win+Alt+Q** | open the main window with the selection |
| **Ctrl+Shift+O** | same | recognise text in a screen region |
| **Ctrl+Alt+O** | **Win+Alt+O** | recognise and translate a screen region |
| **Ctrl+Alt+C** | **Win+Alt+C** | copy the text in a screen region |

On Windows, Ctrl+Alt is what AltGr sends, so those hotkeys would eat
characters typed with AltGr. On macOS, Ctrl+Alt is Control+Option. All of
them, and the actions without a default key (reading the selection aloud
among them), are in *Options → Hotkeys*. A `portable` file next to the
executable keeps the settings beside it. On Linux, `./install.sh` from the
unpacked archive adds Glossa to the application menu (the `.deb` does that
itself).

For OCR, Google Lens works out of the box: Glossa has its own Lens client,
so a scan takes about half a second with nothing to install. A running
chrome-lens-py daemon is used instead when there is one. Offline, recognition
falls over to the system's engine. *Options → Text
recognition* lists the engines this system has:

| engine | id | works | platforms | setup |
|---|---|---|---|---|
| Google Lens | `lens` | online | all | built in (chrome-lens-py's daemon is used when running) |
| Windows OCR | `windows-ocr` | offline | Windows 10, 11 | the language's OCR pack (Settings → Time & language) |
| OneOCR | `oneocr` | offline | Windows 11 | the Snipping Tool; chosen only explicitly, never a fallback |
| Apple Vision | `macos-vision` | offline | macOS 10.15+ (Russian from 13) | nothing |
| Tesseract | `tesseract` | offline | all | `apt install tesseract-ocr tesseract-ocr-rus`, `brew install tesseract tesseract-lang`, the UB Mannheim installer on Windows |
| External command | `command-ocr` | either | all | a command line in its options |
| Module | its id | either | all | a Python or JavaScript module with the OCR capability |

### macOS

The `-macos.dmg` disk images hold `Glossa.app`, a menu-bar app (no Dock
icon until one of its windows is open); the `-macos.zip` archives hold it
and `glossa-cli`. Move `Glossa.app` to `/Applications` (or anywhere) before
the first launch.

The app is signed ad hoc, not notarised, so macOS refuses the first launch
of a download. On macOS 15 and later: open it once, then *System Settings →
Privacy & Security*, scroll to the message about Glossa and press **Open
Anyway**. Or clear the quarantine flag in Terminal:

```bash
xattr -dr com.apple.quarantine /Applications/Glossa.app
```

macOS asks before a program may read other programs' text or the screen.
*Options → Permissions* shows what is granted, with buttons that ask again or
open the right page of System Settings:

| permission | for |
|---|---|
| **Accessibility** | reading the selected text in other programs and copying or replacing it: the translation hotkeys, and mouse mode |
| **Screen Recording** | capturing a region for text recognition; applies after Glossa restarts |
| **Input Monitoring** | mouse mode's watch on the mouse buttons, only if Accessibility is not granted |

Global hotkeys need none of them. macOS ties a grant to the app's signature:
after replacing Glossa.app with a new build, remove the old Glossa entry from
each list and allow the new one.

**Building it yourself** (Rust and the Xcode Command Line Tools):

```bash
cargo build --release -p glossa-ui-egui -p glossa-cli
scripts/macos-bundle.sh            # Glossa.app in ~/Desktop/Glossa; or give a folder
```

An ad-hoc signature changes with every build, so macOS forgets the
permissions each time. Run `scripts/macos-dev-identity.sh` once, in Terminal
on the Mac (it asks for the keychain password): it creates a self-signed
"Glossa Dev" identity, which `macos-bundle.sh` then signs with, and the
permissions survive rebuilds.

## Coming from QTranslate

*Options → Modules → Classic QTranslate installation*: pick the folder with
`QTranslate.exe`, and its services, themes and interface languages are used
in place. A classic language file is laid over Glossa's own translation
(Russian is built in), so strings the classic program never had are
translated too; a native `<code>.json` in a `locales` folder rewords single
strings on top. To bring over the settings and history as well:

```bash
glossa-cli import-classic "C:\Program Files (x86)\QTranslate"
```

The import points each classic service at Glossa's own version of it where
one exists (classic Yandex becomes the keyless Yandex, the DeepL scripts
become DeepL, Multitran, Lingvo, Oxford and the other dictionaries their
bundled modules), so the tabs work without setup. Classic scripts with no
Glossa counterpart still load from the installation.

The popup, the main window and the options pages follow the classic program's
behaviour, studied by observing it ([notes](docs/research/classic-behaviour.md)).

## AI translators

*Options → Services → AI endpoints*: choose a preset, press **Add endpoint**,
paste a key. Each endpoint becomes a translator of its own in the service
strip, with its own model, prompt and settings:

- **Load models** lists what the endpoint offers; **Test** translates a sample
  with the settings as typed.
- Streaming (on by default), temperature or reasoning effort, a token limit,
  a timeout, extra headers and extra JSON request fields.
- `<think>` blocks of local reasoning models are hidden.

<p align="center">
  <img src="docs/screenshots/endpoint-dark.png" alt="An OpenRouter endpoint's settings with 452 models loaded" width="700">
</p>

Details: [docs/ai-endpoints.md](docs/ai-endpoints.md).

## Modules

A module is a folder with a `module.toml` and code, dropped into a modules
folder. Glossa calls JavaScript in process, starts Python, Java or any other
program (Node, Go, C#…) as a child process when first needed, and loads native
libraries directly; all of them get the same JSON requests and the same host
calls (HTTP through Glossa's proxy settings, limited to the hosts the module
declares).

```js
// modules/my-translator/main.js
function translate(req, host) {
  var r = host.fetch("https://api.example.com/translate", {
    method: "POST",
    json: { q: req.text, target: req.to },
  });
  return { text: r.json().translation };
}
```

Start with the [modules overview](docs/modules/README.md). Examples live in
[`modules/`](modules): a dictionary in JavaScript, Python, Java, Node.js and
Rust/C samples, and a Lens OCR module. The services bundled with Glossa are modules too, in
[`modules/bundled/`](modules/bundled); copying one into your modules folder
and editing it replaces the bundled copy.

## Command line

```bash
glossa-cli translate -t ru "Hello, world"
echo "Bonjour" | glossa-cli translate -t en
glossa-cli translate -s ai-deepseek -t de "streams to the terminal as it is written"
glossa-cli dict -f en -t ru serendipity
glossa-cli ocr screenshot.png -t en
glossa-cli ocr screenshot.png --engine tesseract   # or lens, windows-ocr, oneocr, macos-vision
glossa-cli services                  # what is available
glossa-cli action show_popup "text"  # ask the running app to show a popup
```

`glossa-cli` with no arguments lists every command; `--json` gives
machine-readable output. Every command and flag is in
[docs/cli.md](docs/cli.md); the HTTP API other programs use is in
[docs/control-api.md](docs/control-api.md).

## Documentation

All pages, by topic: [docs/README.md](docs/README.md)
([по-русски](docs/README.ru.md)).

- [Installing](docs/install.md): the Windows installer, the macOS disk image,
  the `.deb` and the AppImage, the archives, and removing Glossa.
- [Command line](docs/cli.md): every `glossa-cli` command, flag, exit code and
  environment variable
- [Control API](docs/control-api.md): driving a running Glossa over HTTP or
  (on Linux) D-Bus, and binding hotkeys to it on Wayland, macOS or with
  AutoHotkey
- [Configuration](docs/configuration.md): every `config.toml` key, and where
  the data folder is
- [Text recognition (OCR)](docs/ocr.md): engines, copy mode, Lens translation,
  in-place translation
- [Modules](docs/modules/README.md): guides for
  [JavaScript](docs/modules/js-modules.md), [Python](sdk/python/README.md),
  [Java and any other language over stdio](docs/modules/process-modules.md)
  and [native](docs/modules/native-modules.md) modules, the
  [protocol](docs/modules/protocol.md), and
  [classic QTranslate and pot-app scripts](docs/modules/classic-scripts.md)
- [AI endpoints](docs/ai-endpoints.md)
- [Themes](docs/theming.md)
- [Architecture](docs/architecture.md)
- [How the classic program behaved](docs/research/classic-behaviour.md)
- [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

## Building

Rust 1.95 or newer.

```bash
cargo build --release -p glossa-ui-egui -p glossa-cli
```

On Linux, install `libgtk-3-dev` and `libasound2-dev` first (Debian and
Ubuntu names). Windows binaries also cross-compile from Linux with the
`x86_64-pc-windows-gnu` target and mingw-w64. See
[CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md).

## Status

Working: everything listed under Features, on Windows (tested daily) and
Linux X11; on macOS (Apple silicon) it is in early testing.

Not yet: speech input, the update check. On Wayland, global hotkeys go
through the desktop's shortcut portal (KDE Plasma 6, GNOME 48 and later,
Hyprland) and screen capture through its screenshot portal or `grim`;
elsewhere, bind a desktop shortcut to Glossa's D-Bus service or HTTP API.
Mouse mode and reading the selection depend on the compositor, and
"replace selection" leaves the translation on the clipboard (Wayland allows
no typing into other programs). `glossa-cli doctor` says what works in your
session. On macOS, vibrancy needs the OpenGL engine.

Classic QTranslate service scripts and the keyless bundled services depend on
the websites they talk to; when a site changes, a classic script breaks for
QTranslate and Glossa alike, and a bundled module until it is updated (or
replaced by your own copy). `glossa-cli modules` and `glossa-cli -v` show what
failed and why.

## Clean room

Glossa contains no code from QTranslate. Its behaviour was studied from the
outside: the program's plain-text data files (options, themes, languages, the
JavaScript service scripts and their helper API, which users already edit) and
by observing the running program. Classic scripts and themes are not shipped;
Glossa reads them from an existing installation. See [NOTICE](NOTICE).

Not affiliated with or endorsed by QTranslate or its author.

## License

MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0
([LICENSE-APACHE](LICENSE-APACHE)), at your option. Third-party notices are in
[NOTICE](NOTICE).
