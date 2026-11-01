# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases follow
[Semantic Versioning](https://semver.org/). The section for a version becomes
the notes of its GitHub release.

## [Unreleased]

The first version of Glossa, a clean-room successor to QTranslate in Rust,
for Windows, macOS and Linux.

### Added

- **Desktop app** (`glossa`, egui): a popup by the pointer for the selected
  text, with favourite, dictionary, listen, copy and replace buttons and a
  service strip; the main window with source and translation panes, a
  direction bar with searchable language pickers, service tabs, a stacked
  multi-service view, back translation and drag and drop of text and images;
  dictionary and history windows; every classic options page plus OCR,
  Modules and Control API; a tray icon with the classic menu. Hotkeys wake a
  hidden app, and a second launch hands over to the running one.
- **The commit in the version**: About, `glossa-cli --version` and the
  control API's `/health` show the commit Glossa was built from
  (`0.1.0 (1a2b3c4d)`, `-dirty` for uncommitted changes).
- **Welcome guide** on the first start (and from *Options → Basics*): what
  Glossa is, its main hotkeys (changeable there), how to use it, where to
  keep API keys and data (a portable copy beside the program on request),
  and on macOS the permissions it needs. Drawn as an onboarding flow: a
  progress bar over the steps, live previews of the themes to pick from,
  hotkeys as key caps, the two places for keys as cards, and an accent
  Next button, in the colours of whichever theme is chosen.
- **API keys in the system keychain**, if chosen
  (`general.secret_storage = "system"`): Windows Credential Manager, the
  macOS Keychain or the Secret Service on Linux keep the services' keys, the
  proxy password and the Lens token; the settings file holds a placeholder.
- **macOS Dark and macOS Light themes** after AppKit's colours and metrics.
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
  `glossa-cli`, for Apple silicon and as a universal binary; not notarised
  (the README's macOS section says how to open it the first time).
- **Installers and packages** in every release, beside the archives
  ([docs/install.md](docs/install.md)): a Windows installer (per user
  without admin rights, or for all users; Start menu, optional desktop icon
  and autostart that agrees with Glossa's own option; upgrades in place and
  keeps the settings when uninstalling unless asked), a `.deb` for Debian
  and Ubuntu, an AppImage, macOS disk images, and winget and Scoop manifests
  for when the repository is public. The scripts are in `packaging/`.
  Autostart from an AppImage registers the image file, not its temporary
  mount.
- **Mouse mode on macOS**: a listen-only event tap reports drags and double
  clicks outside Glossa's own windows, and the selection is read through
  Accessibility. Options → Permissions has a row for Input Monitoring, which
  the tap needs when Accessibility is not granted.
- **A macOS menu bar** while a Glossa window is open: Glossa (About,
  Settings… Cmd+,, Hide, Hide Others, Show All, Quit Cmd+Q, which saves the
  settings like the tray's Exit), Edit (Undo, Redo, Cut, Copy, Paste, Select
  All, with their shortcuts) and Window (Minimize Cmd+M, Close Window Cmd+W,
  the window list).
- **Apple Vision warms up** while the region overlay is open: the first
  recognition by a newly installed or rebuilt Glossa (about 25 seconds on
  an M4) now overlaps choosing the region.
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
- **Popup target language**: a click on the popup's title ("English →
  Russian") opens a list with the automatic direction, a swap (the
  translation translated back), recent targets and every language; a
  picked target re-translates and stays for later popups in this run.
- **Popup header icon size** (`appearance.popup.icon_scale`); the header
  grows with the icons.
- **Backdrop opacity** for the windows and the popup
  (`appearance.backdrop_opacity`, `appearance.popup.backdrop_opacity`):
  how much of the theme's fills, and of the acrylic tint, covers the
  material, as in Windhawk's Translucent Windows mod.
- **Font picker**: the interface font is chosen from a searchable list of
  the installed families (font folders and `name` tables on Windows and
  macOS, fontconfig on Linux), listed once on a worker.
- A one-line explanation under the Advanced options, the renderer, the
  popup shade and the mouse mode switches.
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
- **First run**: the main window opens once with a note that Glossa lives
  in the tray (the menu bar on macOS) and which hotkey translates a
  selection, until *Got it* (`general.show_tray_hint`).
- **Text pane menus**: the source pane's context menu has Undo, Redo, Cut,
  Copy, Paste at the cursor (over the selection), Select all, and acts on
  the selection where there is one; the translation pane is selectable and
  has Copy, Copy all, Select all. The dictionary article has Copy,
  Translate, Search (on the dictionary's site) and Listen.
- **Always on top** for the main window, in its ⋮ menu
  (`appearance.main_always_on_top`); **Help** in the same menu opens the
  documentation. A right click on the back and forward arrows lists the
  recent texts and *Show full history*.
- **Language names in the interface language**: pickers and labels show
  them in Russian with the Russian interface (a `languages` table in the
  native locale format, complete for the built-in Russian pack), with each
  language's own name beside it; a search finds any of the names.
- Problems found at start-up (a `config.toml` that could not be read) are shown once.
- **D-Bus service** on Linux and the BSDs: `io.github.bropines.Glossa` on
  the session bus (`/io/github/bropines/Glossa`, interface
  `io.github.bropines.Glossa1`) with `Action`, `Translate`, `ShowPopup`,
  `ShowMain`, `Recognize` and a `Version` property, no token needed; for
  `gdbus`/`busctl`/`qdbus` and desktop shortcut settings. Also offered by
  `glossa-cli serve`; `[server] dbus = false` switches it off.
- **Global hotkeys on Wayland** through the desktop portal's GlobalShortcuts
  (KDE Plasma 6, GNOME 48 and later, Hyprland): the configured chords are
  suggested to the desktop, which may ask once to confirm them. Elsewhere
  the "bind a desktop shortcut" advice stays, now with the D-Bus command.
- **Screen capture on Wayland** through the Screenshot portal (GNOME, KDE;
  no PipeWire needed) before `grim`, which still goes first on wlroots
  compositors. GNOME asks once for permission.
- `glossa-cli doctor` shows the D-Bus service and, on Wayland, what the
  desktop portal offers; the Wayland limitations say which of the above
  applies.

### Fixed

- The welcome guide opened behind the main window on the first start, a
  tip began with a bare dash where its action had no hotkey, and quitting
  Glossa while the guide was open counted as skipping it.
- macOS: a light theme (macOS Light, Glossa Light…) with a backdrop drew the
  window title over the traffic lights: macOS 27 falls back to its old title
  bar layout once the frame view gets a subview while the window is light.
  The window is kept dark for that layout pass. The vibrancy now also covers
  the title bar, which showed what was behind the window sharp, and is
  changed in place on a theme change instead of being made anew.
- macOS: the interface used Helvetica Neue instead of SF where Homebrew's
  fontconfig is installed (it knows SF only by a hidden name), and the
  symbol fallback it picked (Verdana) has no arrows, so the popup title's
  "→" was a box. SF and Apple Symbols are now taken directly.
- macOS: the backdrop choices are named after the vibrancy materials they
  become (Window, Under window, Popover, Full-screen UI) instead of Mica and
  Acrylic, with a hint that fits.
- macOS themes: hints and captions were hard to read over a light material;
  secondary text is darker in macOS Light and lighter in macOS Dark, and no
  longer turns half transparent under a backdrop.
- A dropdown drawn inside its window settled a few points narrower than its
  control; the search box in language lists has its text centred.
- Messages were invisible while the app sat in the tray: a hotkey's "nothing
  to translate", OCR unavailable, a failed copy, replace or layout
  conversion and the speech input stub drew a toast in the hidden main
  window. They now show in a small notice by the pointer that takes no
  focus, errors in the error colour.
- Dropdown lists could close before a click on them counted (where the
  list takes the focus, as on macOS and X11), closed by themselves when
  opened from a window not in the foreground, and dropped a click made
  before the pointer moved over a freshly opened list.
- Mouse mode failing to start was silent: it now says why (a notice, and
  Options → Advanced), the tray's check mark shows whether it really runs,
  and where mouse mode cannot work the tray item and the options are greyed
  out.
- The history window laid out every entry each frame (up to 5000); it now
  lays out only the rows in view, with their heights remembered.
- Dead controls: the microphone button and the speech input language are
  hidden until there is speech input. Unused interface strings were removed
  (`article.print`, `article.select_all`, `lang.edit`, `msg.no_internet`).
- Corners follow the theme: text panes, cards, history rows, notices and the
  popup (where its window can be see-through) use its radii, and dropdown
  windows get the system's rounded corners on Windows 11.

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
- While a popup or another window was open with the main window hidden,
  the program redrew about ten times a second, so idle CPU never reached
  zero and a layout settling a frame late kept moving; it now draws only
  when something changes.
- The popup's frame was a couple of pixels larger than its window, cutting
  off its right and bottom edges.
- The action "Show popup window" is called "Translate selection", which is
  what it does.
- *Start with Windows* (*Open at login*, *Start at login*) is kept in
  `general.start_with_system`, the entry follows the program when it moves,
  and the classic import's setting is offered. Settings that did nothing
  (*Read phonetically*, *Check for updates automatically*) are no longer
  shown; proxy mode *Automatic* shows as *System*, which it is.
- Options → Control API said "Running." while the API was turned off.
- On macOS the interface uses the system font (SF), so arrows and symbols
  no longer log "No font has".
- "Open at login" on macOS starts `Glossa.app` by its bundle identifier
  (`open -b`), so it keeps working after the app is moved, and never points
  at the temporary path macOS runs a quarantined download from.
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

- Speech input and the update check are not implemented yet.
- On Wayland, mouse mode and reading the selection depend on the
  compositor, and global hotkeys need the GlobalShortcuts portal; without
  it (Sway and other wlroots compositors, GNOME before 48) bind keys to the
  D-Bus service or the control API instead. The portal paths were tested
  against a fake portal and, for screenshots, the real
  `xdg-desktop-portal` frontend, not yet on a real KDE or GNOME desktop.
- In a right-to-left row, the caret and a mouse selection that starts or
  ends inside the row are approximate (egui's cursor code expects positions
  to grow with the character index); selecting whole rows and *Copy all*
  work.
