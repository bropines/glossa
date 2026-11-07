# Configuration

Every setting lives in one file, `config.toml`, readable and editable by
hand. The options dialog writes it when you press *Apply* or *OK*; the
program also writes it when you change something from a window (the
direction, the active service, a window's size). This page lists every key,
what it does, and where in the program it is set.

## Where the files are

All of Glossa's own files sit in two folders: the **data folder** (settings,
history, your themes and modules) and the **cache folder** (the log and
throwaway files). Which folders, depends on the layout:

| layout | chosen when | data folder | cache folder |
|---|---|---|---|
| `GLOSSA_HOME` | the `GLOSSA_HOME` environment variable is set, or `--home DIR` is given | `$GLOSSA_HOME` | `$GLOSSA_HOME/cache` |
| portable | a file named `portable` (or `portable.<anything>`) or a folder named `data` is next to the executable | `<exe folder>/data` | `<exe folder>/data/cache` |
| installed | otherwise | Windows `%APPDATA%\Glossa`, Linux `~/.config/glossa`, macOS `~/Library/Application Support/Glossa` | Windows `%LOCALAPPDATA%\Glossa`, Linux `~/.cache/glossa`, macOS `~/Library/Caches/Glossa` |

The first match wins. On Linux the installed folders follow
`$XDG_CONFIG_HOME` and `$XDG_CACHE_HOME` when they are set.
`glossa-cli config path` prints the folder in use:

```console
$ glossa-cli config path
/home/me/.config/glossa/config.toml
data: /home/me/.config/glossa  (portable: false)
```

| file or folder | in | what it is |
|---|---|---|
| `config.toml` | data | The settings (this page). |
| `history.json` | data | The translation history ([below](#historyjson)). |
| `dictionary-history.json` | data | Words looked up in the dictionary window. |
| `themes/` | data | Your themes, `.toml` or `.json` ([theming.md](theming.md)). |
| `locales/` | data | Interface languages and rewordings ([below](#interface-languages)). |
| `modules/` | data | Your modules ([modules/README.md](modules/README.md)). |
| `runtimes/` | local data | The managed Python and `uv` Glossa downloads for Python modules (`python-<version>/`, `uv-<version>/`), shared by every module. Beside `models/`. |
| `venvs/` | local data | One Python environment per module that declares dependencies (`venvs/<module id>/`). Beside `models/`. |
| `models/` | local data | Files modules download (models, dictionaries), one folder per module ([modules/README.md](modules/README.md#assets)). Windows `%LOCALAPPDATA%\Glossa\models`, Linux `~/.local/share/glossa/models`, macOS `~/Library/Application Support/Glossa/models`; `models/` in the data folder for a portable copy or `GLOSSA_HOME`. |
| `remote-desktop-token` | data | Wayland only: the desktop's permission for Glossa to copy and paste through the RemoteDesktop portal, so it asks once. Delete it, or revoke the permission in the desktop's settings, to be asked again. |
| `glossa.log` | cache | The desktop program's log. `GLOSSA_LOG` sets how much goes into it ([cli.md](cli.md#environment-variables)). |
| `modules/` | cache | Module caches; unpacked pot-app `.potext` archives under `modules/potext/`. |
| `oneocr/` | cache | The OneOCR engine's libraries, copied from the Snipping Tool ([ocr.md](ocr.md)). |

Everything in the cache folder can be deleted while Glossa is not running.

Themes, locales and modules are also looked for next to the executable
(`themes/`, `locales/`, `modules/`) and, for a Unix install, in
`<prefix>/share/glossa/`. Files in the data folder are read last, so yours
win over shipped ones of the same name.

## How the file is read and written

- **Every key is optional.** A missing key, or a missing section, takes its
  default. A file holding only the few settings you changed is valid, and a
  missing file means all defaults.
- **Unknown keys are ignored**, so a file written by a newer version still
  loads in an older one. When a key or a value is renamed, the old name stays
  accepted, so an older file keeps loading in a newer version (for example
  `backdrop = "mica_alt"` still reads as `micaalt`). Keys that are no longer
  used are ignored like unknown ones, and dropped at the next save.
- **Out-of-range numbers are pulled into range** when the file is read (the
  ranges are in the tables below), so a hand edit cannot break the program.
- **A malformed file is set aside, not lost**: on a syntax error, or a
  value of the wrong type (`timeout_secs = "ten"`), the desktop program and
  the commands that start the engine move the file to
  `config.toml.broken-<unix seconds>` beside it, start with the defaults,
  and say so once, naming the line. Fix the copy and rename it back while
  Glossa is not running. `glossa-cli config show` reports the error
  without moving anything.
- **The program rewrites the whole file** when it saves: comments and key
  order are not kept, and every key is written out with its value. Writes go
  to `config.toml.tmp` first and then replace the file, so a crash cannot
  leave half a file. Edit the file while Glossa is not running, or your
  edits may be overwritten by its next save.
- `glossa-cli config show` prints the settings in effect: the file with
  every default filled in and values clamped.

### Value formats

| type | written as | notes |
|---|---|---|
| language | a number: `46` | The program writes numeric ids (`1` auto, `17` English, `46` Russian; the full table is in `crates/glossa-core/src/lang.rs`). When editing by hand, a code works too: `first_language = "uk"`. |
| language pair | `{ from = 17, to = 46 }` | Written as a sub-table (`[general.pair]`) or array of tables. |
| hotkey | `"Ctrl+Shift+Q"` | Modifiers `Ctrl`, `Alt`, `Shift`, `Meta` (also `Win`, `Super`, `Cmd`), in any order and any case, then one key: `A`–`Z`, `0`–`9`, `F1`–`F24`, `Space`, `Enter`, `Tab`, `Insert`, `Delete`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Up`, `Right`, `Down`, `PrintScreen`, `Pause`, `Numpad0`… |
| colour | `"#rrggbb"` or `"#rrggbbaa"` | The classic shorthand (`"39f"`) is accepted. |
| path | `"C:\\Users\\me\\dict.ifo"` | TOML strings: double the backslashes, or use single quotes (`'C:\Users\me'`). |

## Sections

The sections follow the pages of the options dialog. "Set in" says where in
the program a key is changed; "file only" means only by editing the file;
"not used yet" means the key is stored (and imported from a classic
installation) but nothing reads it yet.

### `[general]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `ui_language` | string | `""` | Options → Basics | Interface language: a code (`ru`), a language name, or a classic locale folder name. Empty follows `GLOSSA_LANG`, then the system language. |
| `start_with_system` | bool | `false` | Options → Basics, *Start with Windows* (*Open at login* on macOS) | Whether Glossa starts with the session. The checkbox shows and writes the system's own entry (`glossa --hidden`): the `Run` registry key on Windows, `~/Library/LaunchAgents/app.glossa.plist` on macOS, `~/.config/autostart/glossa.desktop` on Linux; Apply records the choice here. While it is on, each start points the entry at the running copy of the program, so it survives the program being moved; an entry removed elsewhere (Task Manager, System Settings) is not added back. The classic import fills it in, and the checkbox then offers it. Ignored with `GLOSSA_HOME` set. |
| `first_language` | language | `46` (Russian) | Options → Basics | Your own language. With smart direction, text in this language is translated into the second; anything else into this one. Not a real language (such as `auto`) becomes the default, Russian. |
| `second_language` | language | `17` (English) | Options → Basics | The language your own is usually translated into. Not a real language becomes English. |
| `speech_language` | language | `46` (Russian) | file only | Language for speech input. Not used yet: there is no speech input, so the options no longer show it. |
| `smart_direction` | bool | `true` | Options → Basics | Choose the target from the first and second language when the target is `auto`. |
| `always_detect_language` | bool | `false` | main window, ⋮ menu by the languages | Detect the source language even when one is chosen. |
| `pair` | language pair | `{ from = 1, to = 1 }` | main window, language selectors | The main window's direction. A `to` of `0` becomes `auto`. |
| `recent_pairs` | array of language pairs | ru→en, en→ru | kept by the program | Recently used directions, newest first, offered in the ⋮ menu. At most 12; pairs with `auto` are not added. A pair listed more than once keeps only its first (newest) place. |
| `instant_translation` | bool | `false` | main window menu | Translate as you type, after a pause. |
| `instant_delay_ms` | integer | `700` | file only | The pause before an instant translation, 150–5000 ms. |
| `spell_checking` | bool | `true` | main window menu | Underline misspelled words in the source pane and offer suggestions in its context menu. Checks in the source language, else the one the last translation detected; nothing is underlined while the language is unknown. Uses the system's checker (Windows Spell Checking API, macOS `NSSpellChecker`, Hunspell dictionaries on Linux); a language without a dictionary is not checked. |
| `back_translation` | bool | `false` | main window menu | Translate the translation back, shown in a third pane. |
| `read_phonetically` | bool | `false` | file only | Kept from the classic settings; not used yet, so the main window menu no longer shows it. |
| `clear_on_drag_drop` | bool | `true` | main window menu | Clear the source pane before text is dropped on it. |
| `auto_cleanup_translation` | bool | `false` | main window menu | Clear the translation when the source text changes. |
| `save_contents_on_exit` | bool | `true` | main window menu | Keep the panes' text across restarts, in [`[contents]`](#contents). |
| `show_top_pane` | bool | `true` | main window menu | Show the toolbar above the source pane. |
| `show_middle_pane` | bool | `true` | main window menu | Show the bar between the source and the translation. |
| `show_services_pane` | bool | `true` | main window menu | Show the service tabs (with the `tabs` layout). |
| `results_layout` | `"tabs"`, `"stacked"` | `"tabs"` | main window menu, *Show all services at once* | `tabs`: one service at a time, picked from the tab strip. `stacked`: every enabled service at once, one card each. |
| `minimize_to_tray_on_minimize` | bool | `false` | main window menu | Minimising the main window hides it in the tray. |
| `minimize_to_tray_on_close` | bool | `true` | main window menu | Closing the main window hides it in the tray instead of quitting (when there is a tray). |
| `startup_action` | `"show_window"`, `"minimize_to_tray"`, `"restore_previous"` | `"minimize_to_tray"` | main window menu, *On start-up* | What the program shows when it starts. `--hidden` overrides it. |
| `show_tray_hint` | bool | `true` for a new configuration, `false` when the key is missing from an existing file | the note's *Got it* button; finishing the welcome guide also turns it off | While on, the main window opens at start with a note that Glossa lives in the tray (the menu bar on macOS) and which hotkey translates a selection. *Got it* turns it off. A file written before the note existed reads as off, so only a first run sees it. |
| `show_welcome` | bool | `true` for a new configuration, `false` when the key is missing from an existing file | the welcome guide's *Finish* or *Skip*; *Options → Basics → Show the welcome guide* opens it again | Show the welcome guide at start: what Glossa is, its hotkeys, how to use it, and where to keep keys and data. |
| `secret_storage` | `"file"`, `"system"` | `"file"` | *Options → Basics*, the welcome guide | Where API keys (`services.options`, AI endpoints included), the proxy password and the Lens token are kept. `"system"` puts each in the system keychain (Windows Credential Manager, the macOS Keychain, the Secret Service on Linux) and this file keeps `"@system-keychain"` in its place. Where no keychain answers (a Linux session without one), they stay in the file. |

A language pair (`pair`, each item of `recent_pairs`) has two keys, `from`
and `to`, each a language:

```toml
[general.pair]
from = 17
to = 46

[[general.recent_pairs]]
from = 46
to = 17
```

### `[history]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `enabled` | bool | `true` | Options → Basics | Record translations made in the desktop program. |
| `clear_on_exit` | bool | `false` | Options → Basics | Empty the history when the program quits. |
| `expand_items` | bool | `false` | Options → Basics | Show each entry's translation in the history list, not only the source. |
| `max_entries` | integer | `5000` | file only | Beyond this, the oldest entries that are not favourites are dropped. 10–1 000 000. |

### `[[presets]]`

Named sets of services to switch between in one click: *Options → Services
→ Presets*, the main window menu, the `next_preset` action (a hotkey, the
tray menu or a tray click), [`glossa-cli preset`](cli.md#preset) and the
[control API](control-api.md#presets). One table per preset you saved or
imported. The built-in presets (*Everyday*, *Private*, *Word study*, which
use only services that need no key) are not written here; they cannot be
changed, only duplicated.

A preset never holds an API key, token or password. Saving and exporting
leave out every option a service declares as secret, anything kept in the
system keychain, and, for a module that is not installed, any option whose
name looks like a credential (`api_key`, `token`, `password`…). Applying a
preset uses the keys already set in [`[services.options]`](#services).

A list left empty leaves that part of the settings alone: a preset without
`dictionary_enabled` does not touch the dictionaries. Services the preset
names that are not installed are skipped when it is applied, and listed.

| key | type | default | what it does |
|---|---|---|---|
| `name` | string | required | Shown in lists and menus. Unique, ignoring case. |
| `order` | array of ids | `[]` | Translation services in tab order. Services it does not name keep their place after these. |
| `enabled` | array of ids | `[]` | The translation services switched on. |
| `active` | id | `""` | The selected service; empty or not enabled: the first enabled one. |
| `dictionary_order` | array of ids | `[]` | Dictionary services in order. |
| `dictionary_enabled` | array of ids | `[]` | The dictionaries in use. |
| `dictionary_active` | id | `""` | The selected dictionary. |
| `tts` | id | `""` | The speech service. |
| `ocr` | id | `""` | The OCR engine ([`[ocr] provider`](#ocr)). |
| `pair` | language pair | absent | The main window's direction (its two keys `from` and `to`, as in [`[general]`](#general)), when the preset was saved with it. |
| `options` | table: service id → table | `{}` | Service settings, as in [`[services.options]`](#services), merged over the current values. Never secrets. |

```toml
[[presets]]
name = "Work"
order = ["deepl", "google"]
enabled = ["deepl", "google"]
active = "deepl"

[presets.options.deepl]
formality = "more"
```

An exported preset is the same table on its own, with a format line first:

```toml
glossa_preset = 1
name = "Work"
enabled = ["deepl", "google"]
```

### `[hotkeys]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `enabled` | bool | `true` | Options → Hotkeys | Register the global hotkeys at all. |
| `bindings` | table: action id → hotkey | see below | Options → Hotkeys | One hotkey per action. An action not listed has no hotkey. |

```toml
[hotkeys.bindings]
show_popup = "Ctrl+Shift+Q"
text_recognition = "Ctrl+Shift+O"
recognize_and_translate = "Ctrl+Alt+O"   # Windows: Win+Alt+O
translate_screen_in_place = "Ctrl+Alt+S" # Windows: Win+Alt+S
recognize_to_clipboard = "Ctrl+Alt+C"    # Windows: Win+Alt+C
show_main_window = "Ctrl+Alt+Q"          # Windows: Win+Alt+Q
```

These six are the defaults of a new config (on macOS, Ctrl+Alt is
Control+Option). They differ by system because Windows reports AltGr as
Ctrl+Alt: a Ctrl+Alt+letter hotkey there would swallow AltGr+letter, which
types `@` on German keyboards and ó or ć on Polish ones. "Listen" has no
default, since Ctrl+E is a browser's search box, Explorer's search and the
end of the line in a shell and in macOS text fields. A config that already
has bindings keeps them, and importing a classic installation brings its
own. The action ids are listed in
[cli.md](cli.md#action). Two actions bound to the same chord cannot both be
registered; the Hotkeys page marks such conflicts. On Wayland, where
programs cannot register global hotkeys, bind keys to the
[control API](control-api.md#binding-hotkeys-to-it) instead.

### `[network]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `timeout_secs` | integer | `10` | Options → Internet | Timeout for one request to a service, 1–300 s. (AI endpoints have their own timeout.) |
| `max_retries` | integer | `1` | Options → Internet | Retries after a connection error, a timeout, or a 429 or 5xx answer, 0–5. Google Translate does not retry a 429: it asks its next address instead, and leaves the refusing one alone for a while (30 s, doubling to 10 min). |
| `user_agent` | string | `""` | file only | Sent with every request unless a service sets its own. Empty uses Glossa's default. |
| `proxy` | table | | | See [`[network.proxy]`](#networkproxy). |

### `[network.proxy]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `mode` | `"system"`, `"none"`, `"manual"`, `"auto"` | `"system"` | Options → Internet | `system`: the `HTTPS_PROXY`/`ALL_PROXY`/`HTTP_PROXY` environment variables (with `NO_PROXY`), else on Windows the system proxy. `none`: connect directly. `manual`: the fields below. `auto` (the classic choice) is an alias of `system`: proxy auto-discovery (WPAD, PAC scripts) is not implemented; the value is kept so classic settings load, and the options show it as *System*. |
| `scheme` | `"http"`, `"https"`, `"socks4"`, `"socks5"` | `"http"` | Options → Internet | The proxy's protocol, with `manual`. |
| `host` | string | `"127.0.0.1"` | Options → Internet | The proxy's address, with `manual`. Empty means no proxy. |
| `port` | integer | `8080` | Options → Internet | 1–65535. |
| `username` | string | `""` | Options → Internet | Leave empty for a proxy without a login. |
| `password` | string | `""` | Options → Internet | Stored in plain text, unless [`[general] secret_storage`](#general) is `"system"`. |

Requests to `localhost` and `127.0.0.1` never go through the proxy.

### `[services]`

Which services exist is only known once the program has loaded its built-in
services and modules, so the lists start empty and the program fills them
in. A service it has not offered before is switched on if it works without
setup, otherwise only listed.

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `order` | array of ids | `[]` | Options → Services → Translation | Translation services in tab order. Services not listed come after, in the order they were found. |
| `enabled` | array of ids | `[]` | Options → Services → Translation | The translation services shown in the tab strip and used for fail-over. |
| `active` | id | `""` | main window, service tabs | The selected translation service. |
| `popup_active` | id | `""` | popup, service strip | The popup's own service. Empty, or a service that is not enabled, follows `active`. |
| `dictionary_order` | array of ids | `[]` | Options → Services → Dictionaries | Dictionary services in order. |
| `dictionary_enabled` | array of ids | `[]` | Options → Services → Dictionaries | The dictionary services in use. |
| `dictionary_active` | id | `""` | dictionary window, service list | The selected dictionary service. |
| `tts` | id | `""` | Options → Services → Speech | The speech service for "listen": `google` on a fresh install, `system-tts` for the system's own voices offline, or a module. Empty, or one that fails, falls back to any service that can speak. |
| `fail_over` | bool | `true` | Options → Services | When the active service fails, try the next enabled one. |
| `options` | table: service id → table | `{}` | Options → Services, the selected service's settings | Each service's settings, by option key, as its manifest declares them (keys, models, URLs, prompts…). See below. |
| `seen` | array of ids | `[]` | kept by the program | Every service offered once, so one you switched off stays off. |
| `ai_endpoints` | array of tables | `[]` | Options → Services → Translation → AI endpoints | Your OpenAI-compatible endpoints; see [`[[services.ai_endpoints]]`](#servicesai_endpoints). |
| `preset` | string | `""` | Options → Services → Presets, the main window menu, the `next_preset` action | The name of the preset applied last, marked as current in lists. Empty when none was. Changing services by hand afterwards does not clear it. |

Service settings are kept as the service declares them:

```toml
[services.options.deepl]
api_key = "…"

[services.options.libretranslate]
url = "http://localhost:5000"
```

A key that is absent takes the service's default. API keys and other
secret options are stored in plain text unless
[`[general] secret_storage`](#general) is `"system"`, in which case the file
holds `"@system-keychain"` in their place; with the file, keep the data
folder private.

### `[[services.ai_endpoints]]`

One table per endpoint. Each becomes a translator of its own; its settings
(URL, key, model, prompt…) are in `[services.options.<id>]`
([ai-endpoints.md](ai-endpoints.md)).

| key | type | default | what it does |
|---|---|---|---|
| `id` | string | required | The provider id: `ai-` and a slug of the name, unique. |
| `name` | string | required | Shown in the service strip and lists. |
| `preset` | string | `""` | The preset it was made from (`openai`, `openrouter`, `deepseek`, `ollama`…), for its icon and defaults; `custom` for none. |

### `[languages]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `disabled` | array of languages | `[]` | Options → Languages | Languages hidden from every language list. |

### `[appearance]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `theme` | string | `"Glossa Dark"` | Options → Appearance | The theme's name, as the theme list shows it ([theming.md](theming.md)). |
| `follow_system_theme` | bool | `true` for a new configuration, `false` when the key is missing from an existing file | *Options → Appearance* | Follow the system's light or dark mode: `theme_light` while it is light, `theme_dark` while dark, switching when the system does. Off, `theme` is used. Where the mode cannot be told, `theme` is used too. |
| `theme_light` | theme name | `"Fluent Light"` on Windows, `"macOS Light"` on macOS, `"Glossa Light"` elsewhere | *Options → Appearance* | The theme while the system is light, with `follow_system_theme`. |
| `theme_dark` | theme name | `"Fluent Dark"` on Windows, `"macOS Dark"` on macOS, `"Glossa Dark"` elsewhere | *Options → Appearance* | The theme while the system is dark, with `follow_system_theme`. |
| `custom_frame` | bool | `true` | Options → Appearance, *Enable window style* | Draw Glossa's own window frame in theme colours instead of the system's. |
| `font_family` | string | `""` | Options → Basics | Interface font. Empty uses the theme's. |
| `ui_scale` | number | `1.0` | Options → Appearance | Interface scale, 0.5–3.0 (the slider offers 0.75–2.0). |
| `text_size` | number | `11.0` | Options → Basics | Size of the text in the panes, in points, 6–48 (the slider offers 7–28). |
| `text_scale` | number | `1.0` | Ctrl+wheel or Ctrl+plus/minus over the text | A zoom on the text areas alone, 0.5–4.0. |
| `icon_scale` | number | `1.2` | Options → Appearance | Size of toolbar and popup icons, 0.75–3.0. |
| `keyboard_scale` | number | `1.0` | the on-screen keyboard (Ctrl+wheel, or resizing it) | Size of the on-screen keyboard, 0.5–3.0. |
| `window_transparency` | integer | `0` | Options → Appearance | Main window transparency, 0 (opaque) to 90 percent. |
| `renderer` | `"open_gl"`, `"gpu"`, `"software"` | `"open_gl"` | Options → Appearance | How windows are drawn: OpenGL (the lightest), the GPU through Direct3D 12, Vulkan or Metal, or a software rasteriser. Takes effect at the next start. `GLOSSA_RENDERER` overrides it for one run. The file is written with `open_gl`; `glow`, `opengl` and `gl` (the names `GLOSSA_RENDERER` uses) are read as `open_gl`, and `wgpu` as `gpu`. |
| `unload_graphics_after_secs` | integer | `45` | Options → Appearance | After this long with every window hidden, the window system is unloaded and only the tray, hotkeys and engine stay in memory. `0` keeps it loaded. |
| `backdrop` | `"none"`, `"mica"`, `"micaalt"`, `"acrylic"`, `"blur"` | `"none"` | Options → Appearance | The system material behind the main window and the other windows (Windows 11; vibrancy on macOS). `mica_alt` and `mica-alt` are read as `micaalt`. See [theming.md](theming.md#backdrops). |
| `main_always_on_top` | bool | `false` | main window, ⋮ menu, *Always on top* | Keep the main window above other windows. |
| `backdrop_opacity` | integer | `100` | Options → Appearance | 0–100 percent: how much of the theme's fills, and of the tint an Acrylic or blur backdrop is drawn with, cover the material. Lower lets more of what is behind show through (like the Windhawk *Translucent Windows* mod). Mica and macOS vibrancy tint themselves: there only Glossa's own fills thin out. |
| `popup` | table | | | See [`[appearance.popup]`](#appearancepopup). |

### `[appearance.popup]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `auto_size` | bool | `true` | Options → Appearance → Popup | Size the popup to its text. |
| `auto_position` | bool | `true` | Options → Appearance → Popup | Place it next to the selection, kept on screen. |
| `activate_on_open` | bool | `false` | Options → Appearance → Popup | Take the keyboard focus when it opens. |
| `pin_when_dragging` | bool | `true` | Options → Appearance → Popup | Stop hiding automatically once it has been dragged. |
| `show_border` | bool | `true` | Options → Appearance → Popup | Draw the coloured border. |
| `auto_hide_secs` | integer | `5` | Options → Appearance → Popup | Hide after this many seconds without the pointer over it, 0–600. `0` never hides. |
| `transparency` | integer | `15` | Options → Appearance → Popup | 0 (opaque) to 90 percent. |
| `frame_thickness` | number | `2.0` | Options → Appearance → Popup | Border width, 0–10 (the slider offers 0–8). |
| `min_width` | number | `350.0` | Options → Appearance → Popup | Narrowest popup, in logical pixels, 80 up to `max_width`. |
| `max_width` | number | `600.0` | Options → Appearance → Popup | Widest popup, 160–4000. |
| `max_height` | number | `480.0` | file only | Tallest popup before its text scrolls, 80–4000. |
| `backdrop` | as `[appearance] backdrop` | `"none"` | Options → Appearance → Popup | The material behind the popup; `acrylic` suits it best. |
| `backdrop_opacity` | integer | `100` | Options → Appearance → Popup | As `[appearance] backdrop_opacity`, for the popup. |
| `icon_scale` | number | `1.0` | Options → Appearance → Popup | Size of the popup's header icons, 0.75–2.5, as a multiple of `[appearance] icon_scale`. The header grows with them. |
| `shade` | integer | `0` | Options → Appearance → Popup | 0–90 percent: how much a translucent popup is darkened when nothing blurs what is behind it, to keep the text readable. |
| `back` | colour | the theme's | Options → Appearance → Popup | Background colour override. Editable only with *Enable window style* off. Absent keeps the theme's. |
| `text` | colour | the theme's | Options → Appearance → Popup | Text colour override, likewise. |
| `frame` | colour | the theme's | Options → Appearance → Popup | Border colour override, likewise. |
| `icons` | table | | | See below. |

The popup's own size, when you resize it, is remembered in
[`[windows]`](#windows) as `popup-size`.

### `[appearance.popup.icons]`

The buttons along the popup's edge, all `true` by default, set in
*Options → Appearance → Popup*.

| key | type | default | what it does |
|---|---|---|---|
| `dictionary` | bool | `true` | Look the text up in the dictionary. |
| `listen` | bool | `true` | Read the translation aloud. |
| `copy` | bool | `true` | Copy the translation. |
| `replace` | bool | `true` | Replace the selection with the translation. |

### `[exceptions]`

Programs where reading the selection and mouse mode stay out of the way: a
terminal, a game, a remote desktop window. Set in *Options → Exceptions*.

| key | type | default | what it does |
|---|---|---|---|
| `mode` | `"disabled_for"`, `"enabled_for"` | `"disabled_for"` | `disabled_for`: work everywhere except where a rule matches. `enabled_for`: work only where a rule matches. |
| `smart_detection` | bool | `true` | Also skip controls that never hold selectable text (list and tree views, scroll bars, hotkey fields, toolbars, buttons, Explorer's file view), with no rule needed. |
| `rules` | array of tables | see below | The rules; see [`[[exceptions.rules]]`](#exceptionsrules). |

### `[[exceptions.rules]]`

| key | type | default | what it does |
|---|---|---|---|
| `program` | string | `""` | The executable's name (`mstsc.exe`), compared without regard to case. Empty matches any program. |
| `window_class` | string | `""` | The window class of the window or of the focused control inside it, compared exactly. Empty matches any window. |

A rule matches when both of its fields match. The defaults: the window
classes `SysListView32`, `SysTreeView32`, `ListBox`, `ScrollBar`, `ComboBox`,
`msctls_hotkey32` and `ConsoleWindowClass` in any program, and all of
`mstsc.exe` (Windows and Linux). macOS names no window classes, only the app
(`program` is its name, `Safari`), so there the list starts empty and
`window_class` and `smart_detection` do nothing; a config still holding the
untouched Windows list is given the empty one when it loads.

### `[advanced]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `browser` | string | `""` | Options → Advanced | Command to open links with. Empty uses the system's default browser. |
| `copy_action` | `"ctrl_c"`, `"ctrl_insert"`, `"accessibility_only"` | `"ctrl_c"` | Options → Advanced | How the selection is copied out of another program when the accessibility interface does not give it: Ctrl+C, Ctrl+Insert (for terminals where Ctrl+C interrupts), or never send a keystroke. |
| `layout_indicator` | `"none"`, `"caret"`, `"tray"` | `"none"` | file only | Kept from the classic settings (a keyboard layout indicator); not implemented, not in the options. |
| `preferred_domain` | string | `"com"` | Options → Advanced | The Google domain to use: `com`, `ru`, `com.ua`, `de`, `co.jp`… Used for "open in browser", for speech, and for the last of Google Translate's three addresses (`translate.google.<domain>`). |
| `gui_translation` | bool | `false` | file only | Kept from the classic settings (translating other programs' controls); not implemented, not in the options. |
| `slower_listen_repeat` | bool | `true` | Options → Advanced | Listening to the same text again within five seconds plays it slowly. |
| `remove_line_breaks` | bool | `false` | Options → Advanced | Join lines before translating. |
| `copy_wait_ms` | integer | `250` | Options → Advanced | How long to wait for the other program to put the selection on the clipboard after the copy keystroke, 20–3000 ms. |
| `terminals` | array of tables | see below | `config.toml` | Terminal windows, where Ctrl+C would interrupt the running command (Windows Terminal with nothing selected, mintty, PuTTY, VS Code's terminal); see [`[[advanced.terminals]]`](#advancedterminals). |
| `terminal_copy_action` | `"ctrl_c"`, `"ctrl_insert"`, `"accessibility_only"` | `"accessibility_only"` | `config.toml` | How the selection is copied out of a terminal when UI Automation does not give it. The default sends no keystroke: Windows Terminal and the console answer through UI Automation, and in mintty and WezTerm a selection is already on the clipboard, which the popup falls back to. `ctrl_insert` copies in the console, Windows Terminal, mintty and WezTerm, but a terminal that does not bind it passes it on to the shell as an escape sequence. `copy_action = "accessibility_only"` wins over this. |
| `terminal_rules_version` | integer | `1` | written by Glossa | Which built-in terminal rules this file has received. A newer Glossa adds the rules it knows beyond this version once, skipping ones already in the list, so a rule you delete stays deleted. |

### `[[advanced.terminals]]`

Windows only: on macOS Cmd+C copies in terminals, and on Linux the selection
is read from PRIMARY (or the clipboard) without any keystroke.

| key | type | default | what it does |
|---|---|---|---|
| `program` | string | `""` | The executable's name (`mintty.exe`), compared without regard to case. Empty matches any program. |
| `window_class` | string | `""` | The class of the window, of the focused control, or of the focused UI Automation element (in Chromium and Electron programs that is the HTML element's class), compared exactly. Empty matches any window. |

A rule matches when both of its fields match; a rule with both empty matches
nothing. The defaults: the classes `CASCADIA_HOSTING_WINDOW_CLASS` (Windows
Terminal), `ConsoleWindowClass` (the console) and `xterm-helper-textarea`
(xterm.js, the terminal of VS Code, Cursor, Hyper, Tabby and web terminals;
VS Code's editor keeps Ctrl+C), and the programs `mintty.exe`,
`wezterm-gui.exe`, `alacritty.exe`, `putty.exe`, `kitty.exe`,
`MobaXterm.exe`, `ConEmu.exe`, `ConEmu64.exe`, `Hyper.exe` and `Tabby.exe`.

### `[mouse_mode]`

Translating a mouse selection without a hotkey. Set in *Options → Advanced →
Mouse mode*.

| key | type | default | what it does |
|---|---|---|---|
| `enabled` | bool | `false` | Mouse mode on. Also toggled by the `toggle_mouse_mode` action, which a tray click runs by default (see [`[tray]`](#tray)). |
| `behaviour` | `"show_icon"`, `"show_translation"`, `"show_translation_and_read"` | `"show_icon"` | After a selection: show a small button by the pointer, show the translation straight away, or show it and read it aloud. |
| `ctrl_only` | bool | `false` | React only while Ctrl is held. |
| `tray_click_toggles` | bool | — | Old: read from older files and moved into `[tray] click` (`false` becomes `"show_main_window"`), then dropped. |

### `[tray]`

What clicks on the tray icon (the menu-bar icon on macOS) do. Set in
*Options → Advanced → Tray icon*. Each value is an action id, the same ones
`[hotkeys]` binds (`show_main_window`, `show_popup`,
`translate_clipboard_in_popup`, `text_recognition`, `show_dictionary`,
`show_history`, `toggle_mouse_mode`…), or `"menu"` (open the tray menu) or
`"none"`. A value that names nothing falls back to that click's default,
with a warning in the log. Actions that work on the selection read it from
the program that was in front before the click: on Windows the click
activates the taskbar (or the overflow flyout is in front), so Glossa gives
the focus back to the last window that was neither the shell nor Glossa;
on macOS a menu-bar click leaves it in front. Linux indicators report no clicks at all, so
these settings do nothing there: any click opens the menu.

| key | type | default | what it does |
|---|---|---|---|
| `click` | action | `"toggle_mouse_mode"`; `"menu"` on macOS | A single left click. `show_main_window` here hides the window again when it is showing. Where mouse mode cannot run, `toggle_mouse_mode` shows the main window instead. |
| `double_click` | action | `"show_main_window"`; `"none"` while `click` is `"menu"` (so on macOS) | A double left click. While it is anything but `"none"`, a single click waits out the system's double-click time before it acts. Not told apart while `click` is `"menu"`. |
| `middle_click` | action | `"none"` | A click with the middle button (the wheel). |

### `[ocr]`

Text recognition. Set in *Options → Text recognition*; the engines are
described in [ocr.md](ocr.md).

| key | type | default | what it does |
|---|---|---|---|
| `provider` | id | `"lens"` | The OCR engine: `lens` (Google Lens, built in or through chrome-lens-py), `windows-ocr`, `oneocr`, `macos-vision`, `tesseract`, `command-ocr`, or a module's id. |
| `language` | language | `1` (auto) | Language of the text in the picture; auto lets the engine decide. |
| `translate_after` | bool | `true` | Translate the recognised text, into the main window's target language. |
| `preserve_line_breaks` | bool | `true` | Keep the picture's line breaks in the text. |
| `save_image_dir` | path | absent | Save each captured region in this folder. Absent saves nothing. |
| `watch_interval_ms` | integer | `800` | How often a watched region is checked for changes, 200–60 000 ms. |
| `after_select` | `"show"`, `"copy"`, `"in_place"`, `"ask"` | `"show"` | What *Text recognition* (and Enter on the selection toolbar) does once a region is selected: show the result window, copy the text to the clipboard, paint the translation over the region, or show a toolbar to choose. The dedicated actions (to the clipboard, translate, in place) ignore it. |
| `translate_with` | `"ocr_engine"`, `"translator"` | `"ocr_engine"` | Who translates recognised text: the OCR engine itself in the same request when it can (Google Lens, recorded in history as "Google Lens"), or always the current translator. |
| `copy_joined_lines` | bool | `false` | Join the lines of recognised text into paragraphs when copying it (prose), rather than keeping the picture's line breaks (code, lists). |
| `lens` | table | | See [`[ocr.lens]`](#ocrlens). |
| `oneocr` | table | | See [`[ocr.oneocr]`](#ocroneocr). |
| `tesseract` | table | | See [`[ocr.tesseract]`](#ocrtesseract). |
| `vision` | table | | See [`[ocr.vision]`](#ocrvision). |

### `[ocr.lens]`

How to reach Google Lens: Glossa's built-in client, or chrome-lens-py.
Set in *Options → Text recognition → Google Lens*.

| key | type | default | what it does |
|---|---|---|---|
| `mode` | `"auto"`, `"native"`, `"daemon"`, `"cli"` | `"auto"` | `auto`: a running chrome-lens-py daemon when there is one (it keeps a warm connection to Google), else the built-in client. `native`: the built-in client only, no Python needed. `daemon`: a chrome-lens-py daemon only. `cli`: chrome-lens-py's `lens_scan` once per image. |
| `endpoint` | string | `""` | A daemon URL to use instead of finding one (`http://127.0.0.1:8765`). |
| `token` | string | `""` | Bearer token for `endpoint`. Kept in the system keychain with `secret_storage = "system"`. |
| `cli_path` | string | `""` | The `lens_scan` executable, if it is not on `PATH`. |

### `[ocr.oneocr]`

| key | type | default | what it does |
|---|---|---|---|
| `folder` | string | `""` | A folder holding `oneocr.dll`, `onnxruntime.dll` and `oneocr.onemodel`. Empty finds the Snipping Tool's and copies the libraries into the cache folder. Windows only; set in *Options → Text recognition*. |

### `[ocr.tesseract]`

How to run Tesseract. Set in *Options → Text recognition → Tesseract*.

| key | type | default | what it does |
|---|---|---|---|
| `path` | string | `""` | The `tesseract` executable. Empty looks on `PATH`, then in the usual install folders (`C:\Program Files\Tesseract-OCR`, Homebrew's, MacPorts'). |
| `tessdata_dir` | string | `""` | Where the `.traineddata` files are, if not Tesseract's own folder. |
| `auto_languages` | string | `""` | Languages for pictures whose language is automatic, in Tesseract's form (`eng+rus`). Empty uses English. |
| `psm` | integer or absent | absent | Page segmentation mode (`--psm`); absent keeps Tesseract's default. |
| `oem` | integer or absent | absent | OCR engine mode (`--oem`); absent keeps Tesseract's default. |
| `extra_args` | string | `""` | More arguments, split the way a shell splits them (`-c preserve_interword_spaces=1`). |

### `[ocr.vision]`

How to ask Apple Vision (macOS only). Set in *Options → Text recognition →
Apple Vision*.

| key | type | default | what it does |
|---|---|---|---|
| `level` | `"accurate"`, `"fast"` | `"accurate"` | Vision's trade-off between speed and accuracy. |
| `language_correction` | bool | `true` | Let Vision correct words with its language model. Off reads codes, names and other non-words more literally. |
| `auto_languages` | string | `""` | Vision's language identifiers (`en-US, ru-RU`) for pictures whose language is automatic. Empty lets macOS 13 and later detect the language; older systems read English. |

### `[dictionary]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `show_services_pane` | bool | `true` | dictionary window | Show the list of dictionary services. |
| `zoom` | integer | `100` | file only | Article zoom, percent, 50–400. |
| `exact_search` | bool | `true` | dictionary window | Match the headword exactly instead of accepting the service's nearest suggestion. |
| `offline` | array of paths | `[]` | Options → Services → Dictionaries | Offline dictionary files: StarDict `.ifo`, XDXF, DSL, tab-separated. |
| `language` | language | `46` (Russian) | dictionary window | The language articles are looked up into. |

### `[updates]`

| key | type | default | set in | what it does |
|---|---|---|---|---|
| `check_automatically` | bool | `false` | file only | Kept; there is no update server yet, so *Options → Updates* has no switch for it. |

### `[server]`

The local [control API](control-api.md). Set in *Options → Control API*.

| key | type | default | what it does |
|---|---|---|---|
| `enabled` | bool | `true` | Start the API with the desktop program. Takes effect at the next start. [`glossa-cli serve`](cli.md#serve) ignores it (with a note on stderr): running that command is the request to start the API. |
| `bind` | string | `"127.0.0.1:60828"` | Address and port to listen on. An address beyond loopback requires a token. Takes effect at the next start. With `0.0.0.0` or `[::]`, `glossa-cli action` connects to `127.0.0.1` or `[::1]` on that port. |
| `token` | string | generated | Required on every request (but see the next key). Never empty: an empty or missing token is replaced with a new random one, and saved, at the next start or as soon as the settings are applied with the field cleared. Applies at once. |
| `pot_compat_without_token` | bool | `true` | Let [pot-app's routes](control-api.md#pot-app-compatibility) through without the token, so pot-app key bindings work unchanged. Only for requests from this machine (a loopback address) that carry none of the headers browsers send (`Origin`, `Sec-Fetch-*`, `Referer`), so a web page cannot trigger them; those routes can open windows but never return data. Set to `false` to require the token there too. |
| `dbus` | bool | `true` | Linux and the BSDs: offer the actions and translation on the session bus as `io.github.bropines.Glossa` ([D-Bus](control-api.md#d-bus)), with no token (the session bus is the user's own). Independent of `enabled`. File only; takes effect at the next start. Ignored on Windows and macOS. |

### `[modules]`

Set in *Options → Modules*, except `disabled`. See
[modules/README.md](modules/README.md).

| key | type | default | what it does |
|---|---|---|---|
| `dirs` | array of paths | `[]` | Extra folders to look for modules in, after the built-in ones. |
| `python` | string | `"auto"` | Which Python runs Python modules. `auto`: a Python 3.8 or newer installed on the machine for a module without dependencies, and otherwise the managed Python, which Glossa downloads once (about 25 MB) into `runtimes/`. `system`: always an installed one. `managed`: always Glossa's own. Anything else is the interpreter to use: a path, or a command line such as `py -3`. Empty means `auto`. A module with dependencies gets its own environment in `venvs/` whichever is chosen. See [modules/README.md](modules/README.md#python-dependencies). |
| `java` | string | `""` | `java` for Java modules: the executable or a Java home folder. Empty looks in `JAVA_HOME`, on `PATH` and in the usual install folders. |
| `allow_native` | bool | `true` | Load native (shared library) modules. They run with the program's full privileges. |
| `allow_legacy_scripts` | bool | `true` | Load classic QTranslate `Service.js` scripts. |
| `call_timeout_ms` | integer | `30000` | Time limit for one call into a module, 1000–600 000 ms. |
| `disabled` | array of ids | `[]` | Module ids not to load. File only. |
| `auto_download_assets` | bool | `false` | Download the files a module declares (models, dictionaries), and the managed Python and packages a Python module needs, on its first use without asking. Off, a module missing them fails with a message and its settings offer the download. See [modules/README.md](modules/README.md#assets). |

### `[contents]`

Kept by the program when `[general] save_contents_on_exit` is on: what was
in the main window's panes when it closed.

| key | type | default | what it does |
|---|---|---|---|
| `source` | string | `""` | The source pane. |
| `translation` | string | `""` | The translation pane. |
| `back_translation` | string | `""` | The back-translation pane. |

### `[windows]`

Kept by the program: window positions and sizes, one sub-table per window
name (`main`, `popup`, `popup-size`, `ocr-result` for the OCR result
window…).

```toml
[windows.main]
x = 120.0
y = 80.0
width = 760.0
height = 520.0
maximized = false
```

| key | type | what it does |
|---|---|---|
| `x`, `y` | number | Position of the top-left corner, in logical pixels. |
| `width`, `height` | number | Size, in logical pixels. |
| `maximized` | bool | Whether the window was maximised. Optional, `false` when absent. |

Delete a window's table to reset its position.

### `[classic]`

A classic QTranslate installation whose services, themes and interface
languages are used in place, without copying them. Set in *Options → Modules
→ Classic QTranslate installation*, or by
[`glossa-cli import-classic`](cli.md#import-classic).

| key | type | default | what it does |
|---|---|---|---|
| `path` | path | absent | The folder holding `QTranslate.exe` and its `Services`, `Themes`, `Locales` and `Data` folders. |
| `use_services` | bool | `true` | Load its `Services` scripts as modules. |
| `use_themes` | bool | `true` | Offer its `Themes` in the theme list. |
| `use_locales` | bool | `true` | Offer its `Locales` as interface languages. |

Importing the installation's settings and history is a separate, one-off
step: the *Import* button on the same page, or `glossa-cli import-classic`.
The classic files are only read. See
[modules/classic-scripts.md](modules/classic-scripts.md).

## history.json

The translation history, newest first, in the data folder. The desktop
program adds an entry for each translation it shows; `glossa-cli translate`
and the control API add one only when asked (`--save-history`,
`"history": true`). An entry is one source text, with one result per
service that translated it:

```json
{
  "entries": [
    {
      "id": 12,
      "source": "Hello",
      "results": [
        { "provider_id": "google", "from": 17, "to": 46, "text": "Привет" }
      ],
      "favorite": true,
      "created": 1761980000
    }
  ],
  "next_id": 13
}
```

`from` and `to` are numeric language ids; `created` is in Unix seconds.
`[history] max_entries` limits the length; favourites are never dropped.
The history window exports it as text, CSV, HTML or JSON, as does
[`glossa-cli history --export`](cli.md#history).

If `history.json` exists but cannot be parsed (a bad hand edit, a damaged
disk), it is copied first to `history.json.corrupt-<unix seconds>` beside
it, a warning is logged, and a new, empty history starts; the copy is never
touched again, so fix it and rename it back while Glossa is not running.
If the file cannot even be read or copied, the history is not saved at all
that session, so data that was never parsed is never overwritten. Turning
`[history] enabled` on while the program runs reads the file first, for
the same reason.

## Interface languages

English is built in, and so is Russian. A language can come from:

- a native locale file, `<code>.json`, in a `locales` folder (next to the
  executable or in the data folder);
- a classic `Locales/<Name>/lang.json`, from a `locales` folder or from the
  classic installation in [`[classic]`](#classic).

Files for the same language are layered over the built-in strings: classic
files first, then native ones, so a native file can reword single strings
of a built-in or classic translation. `glossa-cli locale convert lang.json`
turns a classic file into a native one, and `glossa-cli locale list` shows
what is available.

The interface language applies at once, with no restart: every window, the
tray menu and the macOS menu bar switch as soon as the setting is applied.

### Your own language pack

Put your files in the `locales` folder of the data folder (*Options →
Basics → Language folder* opens it). It is searched last, so its files win.
A native file looks like this:

```json
{
  "code": "eo",
  "name": "Esperanto",
  "native_name": "Esperanto",
  "author": "you",
  "strings": { "main.translate": "Traduki" },
  "languages": { "de": "Germana" }
}
```

- `code` is required; `native_name` is what the language list shows next
  to `name`. A new code joins the list; the code of a built-in language
  (`ru.json`, `en.json`) lays your strings over it key by key, so a single
  string can be fixed without waiting for a release.
- Keys a file lacks, and empty strings, fall back to the layer below and
  finally to English.
- Each file is checked when its language is chosen: an unknown key or
  language tag is a warning and is ignored; a string whose `%s`
  placeholders do not match the English text in number is an error and is
  skipped, so the layer below shows instead. A file in this folder that
  cannot be read (or is over 4 MB) is an error too. In a classic
  `lang.json` from the QTranslate installation the same problems are only
  warnings, named with the file's folder: the strings are skipped all the
  same, but the file is not yours to fix. Problems go to the log and are
  listed under the language setting in Options.
- Only the chosen language is loaded; for the others, only the header is
  read to fill the list. A rescan that finds the chosen language's files
  unchanged keeps what is loaded.
- After editing a file, *Reload languages* in Options applies it.

`glossa-cli locale template` writes a file to start from, with every key,
its English text and its classic address; `glossa-cli locale check FILE`
checks one ([cli.md](cli.md#locale)).
