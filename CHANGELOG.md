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
  translator page session, or the keyed API), DeepL API, DeepLX, Yandex
  Cloud, LibreTranslate, MyMemory, Lingva and any OpenAI-compatible API.
- **Bundled modules**: keyless services written as Glossa JavaScript modules
  (`modules/bundled/`) and compiled into the executable, switched on at first
  start: Yandex (translation, detection and dictionary), DeepL (the iOS app's
  free endpoint), Lara and ModernMT translators; Multitran, ABBYY Lingvo Live,
  Oxford Learner's, Wiktionary, Youdao, Academic (dic and translate),
  Wikipedia and Urban Dictionary dictionaries. A module with the same id in a
  modules folder replaces the bundled one. The keyed built-ins are now named
  "Yandex Cloud (API key)" and "DeepL API (key)".
- **Text recognition**: drag over a screen region to copy or translate its
  text, repeat the last region, or watch a region and re-translate when it
  changes. Google Lens through
  [chrome-lens-py](https://github.com/bropines/chrome-lens-py) (its warm
  daemon, else its CLI), or any external command.
- **OCR to the clipboard, discoverable**: *Copy text from the screen*
  (Ctrl+Alt+C) closes the overlay at once and copies the text, with a notice
  by the region that takes no focus ("Copied N characters" and a preview).
  Every recognition action is in the tray menu, the main window's camera
  button menu and Options → Hotkeys. *After selecting a region* chooses what
  plain text recognition does: show, copy, translate in place, or ask with a
  Snipping Tool style toolbar (Enter, C, T, I, Esc). *Join lines when
  copying* turns hard-wrapped prose into paragraphs.
- **OCR result window**: a recognised region opens in a tool window of its
  own, framed like the others, as soon as the region is chosen: the picture
  (larger on a click, with the boxes the engine found), what read it, the
  editable text in the panes' font and zoom, the direction with swap and a
  translator picker (Google Lens or any enabled translator), the
  translation, and an action bar (copy text or translation, translate in
  place, to the main window, read aloud, copy or save the picture, select
  again), each with a shortcut. It remembers its size and place
  (`[windows.ocr-result]`). The selection toolbar is drawn as a command bar
  with icons.
- **Translation by Google Lens itself**: Lens's own translation is shown
  without a second request and saved in the history under Google Lens;
  *Translate with* can send the text to the current translator instead, and
  `glossa-cli ocr --translate-with ocr|translator` chooses per run. The
  history window names each entry's translator.
- **Translate the screen in place**: the translation painted over the region
  in a window laid exactly over it, block by block, on the sampled
  background colour, in the text's colour and angle, fitted to each box (Lens
  line by line, as Chromium's overlay does; other engines' lines grouped
  into paragraphs and translated in one request). Also `glossa-cli ocr
  --in-place` and `"in_place": true` on the control API's `/v1/ocr`; see
  [docs/ocr.md](docs/ocr.md).
- **Offline OCR on Windows**: Windows OCR (the recogniser built into Windows
  10 and 11, for the languages whose OCR pack is installed) and OneOCR (the
  Windows 11 Snipping Tool's engine, loaded from the user's own installation;
  nothing of Microsoft's is shipped). Both pick-able in *Options → Text
  recognition* and with `glossa-cli ocr --engine windows-ocr|oneocr`.
- **Offline OCR on macOS and Linux**: Apple Vision (`macos-vision`, the Live
  Text recogniser built into macOS 10.15 and later; accurate or fast,
  language correction, automatic language on macOS 13+) and Tesseract
  (`tesseract`, on every OS where it is installed; lines and words with
  boxes from its TSV output, page segmentation and engine modes, clear
  install instructions for a missing program or language). Both have their
  own section in *Options → Text recognition*.
- **OCR fail-over reaches an offline engine** when the chosen one is not
  installed (Lens without chrome-lens-py): Windows OCR, Apple Vision or
  Tesseract, in that order of availability. OneOCR is never a fallback.
- **macOS builds** in CI and the release: `Glossa.app` (a menu-bar app) and
  `glossa-cli`, for Apple silicon and as a universal binary; not notarised.
- **A standard OCR result**: every engine's result is one page of blocks,
  paragraphs, lines and words with boxes, rotation, outlines, confidence,
  baselines, language, writing direction and per-region translations (Lens's
  per-paragraph and per-line translations are kept; OneOCR reports word
  confidence). It exports to hOCR 1.2, ALTO v4, JSON and text
  (`glossa-cli ocr --format hocr|alto|json|text`, `format` on `POST /v1/ocr`),
  and hOCR or ALTO from an engine or module is read back.
- **OCR modules with an image API**: an OCR module may return the layout as
  JSON blocks, hOCR or ALTO, and gets PIL/OpenCV-style preprocessing from
  Glossa as `image.*` host calls in every runtime (crop, resize, rotate,
  deskew, thresholds including Otsu and adaptive, levels, blur, sharpen, pad,
  tiles, PNG/JPEG/WebP encoding…), on handles that carry the transform back to
  the original picture. Guide: `docs/modules/ocr-modules.md`; examples
  `google-vision-ocr` (JS), `ocr-space` (Python), `example-lines` (Rust).
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
  unchanged. Examples: `free-dictionary` and `google-vision-ocr` (JS),
  `example-reverse`, `lens-ocr` and `ocr-space` (Python), `example-rot13` and
  `example-lines` (Rust) and `example-c` (C).
- **Java and any-language modules**: `runtime = "process"` runs any program
  that speaks the module protocol over stdio (`command`, per-platform
  `command.windows` / `.macos` / `.linux`, `args`, `env`, `cwd`), and
  `runtime = "java"` runs a jar or a main class with Java found through the
  new *Java* setting, `JAVA_HOME`, `PATH` or the usual install folders (a
  missing Java fails the load with install instructions). Python, Java and
  process modules share one stdio transport: same timeouts, idle stop,
  restart and logging, and every host call. A dependency-free Java 17 SDK
  (`sdk/java`, usable from Kotlin) and examples `example-upper` (Java) and
  `example-node` (Node.js, no SDK). `glossa-cli modules` shows each module's
  runtime.
- **Classic QTranslate**: point Glossa at an installation to use its
  services, themes and languages in place, or import its `Options.json` and
  history with `glossa-cli import-classic`. The import maps classic services
  onto the keyless Glossa ones (classic Yandex onto the bundled Yandex rather
  than the keyed API, the DeepL and DeepLX scripts onto DeepL, the Lara copy
  onto Lara), merging classic ids that land on the same service.
- **AI endpoints**: any number of OpenAI-compatible translators made from
  presets (OpenAI, OpenRouter, DeepSeek, Gemini, Groq, Mistral, Ollama, LM
  Studio, custom), each with its model, prompt, temperature or reasoning
  effort, token limit, timeout, extra headers and extra request fields; *Load
  models* and *Test* in the options. Answers stream into the popup, the main
  window and the terminal as they are written.
- **Interface languages**: every label, tooltip, message and window title
  goes through the locale, with English and Russian compiled in. A language
  loads in layers: English, the built-in pack, a classic `lang.json` from the
  QTranslate installation, then a native `<code>.json` of the user's, so a
  classic file no longer leaves Glossa's newer strings in English and a user
  file can reword single strings. *System* follows the Windows display
  language. A test fails when the UI names a key the table lacks or a
  built-in pack misses one.
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
- **Mouse mode**: select text with the mouse and a button by the pointer (or
  the translation itself) appears, no hotkey; a low-level mouse hook on
  Windows, the PRIMARY selection on Linux; the text is read through UI
  Automation only. A click on the tray icon toggles it, a double click opens
  the window; the tray icon takes the theme's accent colour and another hue
  while mouse mode is on.
- **Popup**: title on the left, actions centred, window buttons (reset size,
  pin, close) on the right, all lighting up on hover; resizable with the
  size remembered, draggable by any empty spot; room for the service strip
  always kept; clear separators, the romanisation and dictionary block
  below a line in the middle of a fixed-size popup; border optional; a shade
  when nothing blurs behind it.
- **Look**: Fluent Dark and Fluent Light (WinUI 3) themes following the
  system accent colour, sized and spaced to Microsoft's WinUI 3 numbers
  (32 px controls, 4 / 8 px corners, Segoe UI Variable on the type ramp,
  thin scroll bars, flyout shadows; `docs/research/fluent.md`), with new
  theme metrics `control_height`, `overlay_radius` and `controls`; Mica,
  Mica Alt, Acrylic and blur backdrops for the windows and the popup
  (Windows 11 with any graphics engine, the GPU one presenting through
  DirectComposition; vibrancy on macOS with OpenGL); the tray menu dark or
  light with the theme (Windows 10 1809+); an icon size setting; changing the graphics engine offers to restart.
- **Text zoom**: Ctrl+wheel over a text area (the main window's panes, the
  popup, the dictionary article) and Ctrl+plus / minus / 0 zoom the text
  alone (`appearance.text_scale`); the interface scale is set in Options
  only.
- **Dropdowns in their own windows**: language lists, the main window's
  menus and the panes' context menus open in a borderless window under the
  control (above it only when the screen has no room below), no longer cut
  off by a short window or opened off to the side. The window they belong to
  keeps the focus; typing searches, the arrow keys and Enter pick.
- **Options**: the Services page uses the whole window, the list in a
  resizable pane beside the settings.
- **On-screen keyboard** drawn like a real one: staggered ANSI rows with wide
  modifier keys, keycaps that sink when pressed, shifted characters printed
  above, sticky Shift and Caps Lock that light up, key repeat; the English
  and the Russian (ЙЦУКЕН, Latin letters in the corners) layouts. Ctrl+wheel
  or dragging the window's edge scales it on its own
  (`appearance.keyboard_scale`).
- **About** shows what Glossa is, its co-authors and its licences, with the
  license texts and third-party notices readable in the window.
- **One frame for every window**: with window style on, the options,
  dictionary, history, keyboard and About windows draw the same themed
  title bar as the main window (Windows 11 caption buttons, double-click to
  maximise, resizing from every edge and corner, rounded corners and a
  theme-coloured border from DWM on Windows 11); macOS keeps its system
  decorations.
- Opening a window that is already open (toolbar, menu, tray, hotkey)
  brings it to the front and focuses it.
- Licenses (MIT or Apache-2.0), NOTICE, CI on Linux and Windows, release
  builds, issue templates, contributing and security guides.
- `glossa-cli doctor` (and `--json`): the session, which desktop features
  work there and its limitations, macOS permissions, installed OCR engines,
  modules that loaded or failed, the files in use, the proxy and
  certificate store, the control API and whether the desktop program runs.
  The limitations are locale keys with English and Russian text, for the
  interface to show later.
- `[advanced] terminals` and `terminal_copy_action`: terminal windows where
  selection capture never sends Ctrl+C.

### Fixed

- Classic themes such as Metro left the options window without visible
  labels: their `Window.Text` is the caption's (white on a blue caption)
  and was used on the white body. Labels now take the theme's other text
  colour, text fields without an `Edit` section use the body colours, and
  every loaded theme has unreadable text-on-background pairs repaired.
- The theme reaches every window: egui's light and dark styles both carry
  it, so a window whose system theme differs no longer falls back to egui's
  defaults.
- Window transparency works under the GPU and software engines (a
  DirectComposition swap chain on Windows) and in the other windows, not
  only the main one.

- Windows opened while the main window was hidden (popup, OCR overlay,
  options from a hotkey or the tray) made egui panic; eframe is vendored with
  a fix (`third_party/README.md`).
- A theme's own font (`[fonts] family` / `fallbacks`) was never loaded, so
  the Fluent themes drew in Segoe UI rather than Segoe UI Variable; it is
  now, and again after a theme change.
- Windows that egui resolved to the other system theme (Options with a
  light theme on a dark system) drew with egui's default colours.
- Python installed through pyenv-win, or behind another `.bat`/`.cmd` shim,
  is found.
- The control API token generated on first run is saved, so `glossa-cli
  action` and other clients can use it.
- Popup: *Auto position* keeps it on screen near the edges (or reopens it
  where it was dragged), *Auto size* off gives a fixed box; exception rules
  for controls (list views, hotkey fields, Explorer's file view) match, and
  *Smart detection* works.
- History: the star and delete icons are toolbar-sized and light up on
  hover (the row used to take the hover from them); emoji in entries' text
  draw on Windows instead of boxes.
- Language lists showed boxes for Bengali, Kannada, Khmer, Korean, Lao and
  other scripts: the fonts the language names need now load when a list
  opens, and every script in the language table has a font on Windows and
  through fontconfig. Right-to-left names read the right way round.
- The source and translation panes have the same inner margins.
- Arabic, Persian, Urdu and Hebrew text came out backwards with letters
  joined wrongly: a hand-made reversal fought egui's own shaping. The text
  now goes through the Unicode bidi algorithm (`bidi` module): each run is
  shaped in its own direction, rows wrap in logical order and their runs
  are placed in visual order, so mixed Arabic and Latin lines, numbers,
  brackets and wrapped paragraphs read right, in the popup, the main
  window (both panes), the stacked view, history, dictionary articles and
  the language lists. Copying still copies the logical text. Indic, Khmer,
  Myanmar and Sinhala text was already shaped correctly and is unchanged.
- `glossa-cli` accepts `--flag=value` for every option (`--home=DIR` was
  silently ignored), and an unknown flag, or one the command does not take,
  is a usage error (64) naming the closest real flag instead of doing
  nothing.
- An unknown service id (`-s`, `-e`, the API's `service`/`engine`) is an
  error listing similar ids (CLI exit 64, API 400) instead of being skipped
  while another service answered.
- The control API rejects an unknown language code, capability or boolean
  with 400, as the command line does, instead of reading it as `auto` or
  `translate`; `back` and `slow` are read from the query string too.
- Piped `glossa-cli translate` output is the translation alone, as
  documented; romanisation and dictionary senses stay on a terminal or
  with `--full`.
- Google (and Lingva) put the source's pronunciation in `romanization`
  ("hello" into Russian gave `həˈlō`): the translation's romanisation and
  the source's transliteration now have fields of their own
  (`source_romanization`).
- The control API token can no longer end up empty: clearing it makes a new
  one, saved at once, and an empty token never disables the check.
- pot-app key bindings work unchanged: its routes accept requests without
  the token from loopback with no `Origin` header
  (`[server] pot_compat_without_token`, on by default). `Origin: null` is
  refused like any other browser origin.
- `glossa-cli serve` notes that it ignores `[server] enabled`;
  `glossa-cli action` connects to loopback when `bind` is `0.0.0.0` or `::`.
- `[appearance] renderer` also reads `glow`, `opengl`, `gl` and `wgpu`.
- An invalid `first_language` falls back to the default (Russian), not
  English; `recent_pairs` drops every repeat, not only adjacent ones.
- A `history.json` that cannot be parsed is copied to
  `history.json.corrupt-<time>` before a new history starts, and is never
  overwritten when it cannot be copied; switching the history on reads the
  file before anything saves over it.
- The CLI and the API give translations the same JSON shape (with `failed`,
  `alternatives`, `source_romanization`) and can record them in the history
  on request (`--save-history`, `"history": true`); `/v1/services` reports
  `needs_credentials`; `/v1/ocr` takes `engine` and returns `elapsed_ms`; a
  wrong method is 405 with `Allow`; an OCR engine that is not set up is 503.
- Proxy mode `auto` is documented as what it is: an alias of `system`.
- Selection capture no longer sends Ctrl+C into terminals (Windows
  Terminal, the console, mintty, WezTerm, Alacritty, PuTTY, VS Code's
  terminal and other xterm.js terminals…), where it interrupted the running
  command when UI Automation found no selection; built-in terminal rules
  reach existing configs once, through a versioned marker.
- A panic in a module, a built-in service, the image code or an engine job
  fails that one request (`Error::Module` / `Error::Internal`, HTTP 500 on
  the control API) instead of ending the program: release builds unwind
  (about +2.2 MB on `glossa.exe`) and the panic hook still logs the panic
  with its frames.
- Web pages can no longer reach the tokenless pot-app routes with an image
  or a link: requests with `Sec-Fetch-*` or `Referer` headers are refused
  like those with `Origin`, and the `Host` must be a loopback address or
  `localhost` on every route (DNS rebinding).
- Only one copy runs per user, session and data folder even with the
  control API off or its port taken, and a second launch no longer hands
  over to another user's copy: a session-local lock and pipe (Windows) or
  socket (Linux, macOS) carry the hand-over, with the API as a fallback.
- A `config.toml` that does not parse no longer stops the program: it is
  moved to `config.toml.broken-<time>`, the defaults are used, and the
  engine reports it once (`Engine::startup_warnings`).
- Default hotkeys: no default for "listen" (Ctrl+E is the browser's search
  box and end-of-line in shells and macOS text fields), and on Windows
  Win+Alt instead of Ctrl+Alt, which AltGr sends (Ctrl+Alt+Q swallowed `@`
  on German keyboards). Existing configs keep their bindings.
- Linux checks TLS certificates against the system's store (corporate CAs
  work), falling back to the bundled roots when the store is empty.

### Known gaps

- Speech input and the update check are not implemented yet; mouse mode is
  not available on macOS yet.
- On Wayland, global hotkeys, mouse mode and reading the selection depend on
  the compositor; bind keys to the control API instead.
- In a right-to-left row, the caret and a mouse selection that starts or
  ends inside the row are approximate (egui's cursor code expects positions
  to grow with the character index); selecting whole rows and *Copy all*
  work.
