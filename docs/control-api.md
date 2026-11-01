# Control API

A running Glossa answers HTTP requests on localhost. Anything on the machine
can use it: a script, a launcher, a Stream Deck, AutoHotkey, or a key binding
in the desktop environment. That last one is why it exists: on Wayland an
application cannot register global hotkeys itself (only through the
desktop's shortcut portal, which not every desktop has), but the compositor
can run a command, and `curl` to the control API makes the popup work there
too.

The API can translate, detect, look up, recognise and speak, returning JSON,
and it can ask the desktop program to perform any of its actions (show the
popup, recognise a screen region…), exactly as a hotkey would.

On Linux and the BSDs the actions and translation are also on the session
bus, with no token: see [D-Bus](#d-bus).

## Enabling it

The API is on by default. *Options → Control API*:

| setting | `config.toml` | default |
|---|---|---|
| Enable the local control API | `[server] enabled` | `true` |
| Listen on | `[server] bind` | `127.0.0.1:60828` |
| Token | `[server] token` | generated on first start |

Turning the API on or off and changing the address take effect at the next
start of the program; a new token applies at once. The page says whether the
API is running and, if it could not start (the port is taken, say), why. It
also lists ready-made `curl` commands for the most used actions, with the
token filled in.

`60828` is also pot-app's port, so tools made for pot-app find Glossa where
they expect it (see [pot-app compatibility](#pot-app-compatibility)).

Starting `glossa` while it is already running shows the running copy's
main window instead of opening a second one. That does not need the API:
one copy runs per user, login session and data folder, held by a lock of
the session's own (a named mutex and pipe on Windows, a lock file and a Unix
socket in the user's runtime folder elsewhere), and the second start hands
over through it. The API is only the fallback, for a running copy too old
to have the pipe. Copies with different data folders (`GLOSSA_HOME`) run
side by side.

[`glossa-cli serve`](cli.md#serve) runs the same API without the desktop
program, for scripts and for trying it out; there, actions are only printed.

## The token

Every request except `/health` (and, from this machine, pot-app's routes;
see [pot-app compatibility](#pot-app-compatibility)) must carry the token,
either as a header or in the query string:

```
Authorization: Bearer 0123456789abcdef…
GET /v1/action/show_popup?token=0123456789abcdef…
```

The header is matched exactly: `Bearer`, one space, the token. The token is
64 hexadecimal characters, generated the first time the engine starts (by
the desktop program or by any `glossa-cli` command that uses it) and stored
in `config.toml` under `[server] token`, so other programs can read it.
*Options → Control API → Regenerate* makes a new one; *Show token* reveals
it.

The token is always required; there is no way to switch the check off. An
empty token (a cleared field, a hand edit) is replaced with a new random
one and saved straight away: at the next start, or as soon as the settings
are applied with the field empty.

To read the token in a script:

```bash
# Linux (installed layout)
token=$(sed -n '/^\[server\]/,/^\[/s/^token = "\(.*\)"/\1/p' ~/.config/glossa/config.toml)
```

```powershell
# Windows (installed layout)
$cfg = Get-Content "$env:APPDATA\Glossa\config.toml" -Raw
$token = [regex]::Match($cfg, '(?ms)^\[server\].*?^token = "([^"]*)"').Groups[1].Value
```

Match the `[server]` section: `[ocr.lens]` has a `token` key of its own.
`glossa-cli config path` prints where `config.toml` is
([configuration.md](configuration.md#where-the-files-are)).

## Requests and responses

- All responses are JSON (`application/json; charset=utf-8`), except
  `/v1/speak`, which returns audio.
- Request bodies are JSON, whatever the `Content-Type` says (except for
  `/v1/ocr` with an image, below). An empty body counts as `{}`.
- Parameters (`text`, `from`, `to`, `service`, `engine`, `lang`,
  `translate_to`, `back`, `slow`, `history`) may be given in the JSON body
  or in the query string; the body wins. The query string is
  percent-decoded, and `+` is a space. A boolean is `true`/`false` in JSON;
  in the query string (or as a JSON string) `1`, `true`, `yes`, `on` and
  `0`, `false`, `no`, `off` are understood, and a bare `?back` means true.
  Anything else is a 400.
- Languages are codes: `en`, `ru`, `zh-CN`, `auto`. An unknown code is a
  400, as it is a usage error on the command line. An empty value counts as
  not given.
- A service is a provider id from `/v1/services`. An id that does not exist,
  or a provider without the capability the endpoint needs, is a 400 whose
  message names a few similar ids
  (`unknown service "gogle"; did you mean google, google-dictionary?`).
  With a valid id, fail-over to the other enabled services still applies.
- A known path asked with the wrong method is a 405 with an `Allow` header
  listing the methods it answers.
- A trailing `/` in the path is ignored. Bodies are limited to 32 MiB.
- Each request runs on its own thread: a slow translation does not hold up
  an action.
- Translations made through the API are added to the history only with
  `"history": true` (see [/v1/translate](#v1translate)).

### Errors

Errors have a status code and a body of the form:

```json
{"error": "source and target language are the same"}
```

| status | when |
|---|---|
| 400 | Invalid JSON, a missing `text`, empty text, an unknown language, capability, service or OCR engine id, a boolean that is not one, an unknown OCR `format`, the same source and target language, a configuration problem with a translation service. |
| 401 | The token is missing or wrong. |
| 403 | The request came from a browser: it carries an `Origin`, `Sec-Fetch-Site`, `Sec-Fetch-Mode`, `Sec-Fetch-Dest`, `Sec-Fetch-User` or `Referer` header (any value). Or its `Host` header is a name other than `localhost` (see [Security](#security)). |
| 404 | No such route or action; no service can do what was asked (no speech service). |
| 405 | The path exists but not for this method; the `Allow` header lists the methods it answers. |
| 412 | The service needs a key that is not set. |
| 413 | The body is larger than 32 MiB. |
| 422 | No text was found in the image, or the language could not be detected. |
| 429 | The service is rate-limiting. |
| 500 | A bug in Glossa: the code handling the request panicked. The program keeps running; the log has the details. |
| 502 | The service failed otherwise (network error, unexpected answer). |
| 503 | `/v1/ocr` only: the OCR engine is not installed or not set up (Lens without chrome-lens-py, Tesseract missing, no engine at all). The request was fine; the machine needs setting up. |
| 504 | The service timed out. |

## Endpoints

| method | path | does |
|---|---|---|
| GET | [`/health`](#get-health) | Is Glossa there? No token needed. |
| GET | [`/v1/services`](#get-v1services) | Providers with a capability. |
| GET, POST | [`/v1/translate`](#v1translate) | Translate text. |
| POST | [`/v1/detect`](#post-v1detect) | Detect the language. |
| GET, POST | [`/v1/dictionary`](#v1dictionary) | Look a word up. |
| POST | [`/v1/ocr`](#post-v1ocr) | Recognise the text in an image, optionally translate it. |
| POST | [`/v1/speak`](#post-v1speak) | Text to speech, as audio. |
| GET, POST | [`/v1/action/<action>`](#v1actionaction) | Ask the desktop program to perform an action. |
| any | [pot-app routes](#pot-app-compatibility) | `/selection_translate`, `/input_translate`, `/ocr_recognize`, `/ocr_translate`, `POST /translate`. |

The examples use `$T` for the token and the default address.

### GET /health

```console
$ curl -s http://127.0.0.1:60828/health
{"name":"glossa","version":"0.1.0","commit":"1a2b3c4d"}
```

`commit` is the commit the program was built from (`-dirty` when it had
uncommitted changes), empty when unknown. `/` answers the same. Neither needs the token, so a script can check that
Glossa is running before it does anything else.

### GET /v1/services

| parameter | default | meaning |
|---|---|---|
| `capability` | `translate` | `translate`, `dictionary`, `listen`, `ocr`, `detect_language`, `speech_input`, `batch`, `alternatives`, `promptable`, `offline` (aliases: `dict`, `tts`, `detect`, `stt`). An unknown value is a 400. |

```console
$ curl -s "http://127.0.0.1:60828/v1/services?capability=listen&token=$T"
[{"capabilities":["translate","detect_language","listen"],"enabled":true,"id":"google","name":"Google Translate","needs_credentials":false,"origin":"Built-in"}]
```

Each item has `id`, `name`, `origin` (`Built-in`, `Bundled`, or the kind of
module), `enabled` (shown in the service strip), `capabilities` and
`needs_credentials` (it wants a key or a login before it works), the same
fields as [`glossa-cli --json services`](cli.md#services).

### /v1/translate

GET or POST.

| parameter | type | default | meaning |
|---|---|---|---|
| `text` | string | required | The text. |
| `from` | language | `auto` | Source language. |
| `to` | language | `auto` | Target language; `auto` lets smart direction choose (first language ↔ second, see [`[general]`](configuration.md#general)). |
| `service` | provider id | the active service | Ask this service first; fail-over still applies when it is on. |
| `back` | boolean | `false` | Also translate the result back. |
| `history` | boolean | `false` | Add the translation to the history of the program serving the API (when `[history] enabled` is on), and write `history.json` at once. |

```console
$ curl -s "http://127.0.0.1:60828/v1/translate?text=Hello%20world&to=ru&token=$T"
{"text":"Привет, мир","from":"en","to":"ru","service":"google","romanization":"Privet, mir","source_romanization":"həˈlō wərld","alternatives":[],"back":null,"failed":[],"elapsed_ms":155}

$ curl -s -H "Authorization: Bearer $T" \
    -d '{"text":"Good morning","from":"en","to":"de","service":"yandex-web","back":true}' \
    http://127.0.0.1:60828/v1/translate
{"text":"Guten Morgen","from":"en","to":"de","service":"yandex-web","romanization":null,"source_romanization":null,"alternatives":[],"back":"Good morning","failed":[],"elapsed_ms":618}
```

The answer is the same object `glossa-cli --json translate` prints; every
field is always present:

| field | type | meaning |
|---|---|---|
| `text` | string | The translation. |
| `from`, `to` | string | The direction actually used. |
| `service` | string | The provider that answered. |
| `romanization` | string or null | The translation in Latin letters, when the service gives it. |
| `source_romanization` | string or null | The source's transliteration or pronunciation, when the service gives it (`Kon'nichiwa` for Japanese input). |
| `alternatives` | array of strings | Other translations, from services that offer them. |
| `back` | string or null | The back translation, when `back` was set. |
| `failed` | array | Services tried before this one that failed, as `[id, message]` pairs. |
| `elapsed_ms` | number | Time the answering service took. |

### POST /v1/detect

| parameter | meaning |
|---|---|
| `text` | The text. |

```console
$ curl -s -H "Authorization: Bearer $T" -d '{"text":"Dobrý den"}' http://127.0.0.1:60828/v1/detect
{"confidence":0.9833352565765381,"lang":"cs","name":"Czech"}
```

`lang` is the code, `name` the English name, `confidence` between 0 and 1.

### /v1/dictionary

GET or POST.

| parameter | default | meaning |
|---|---|---|
| `text` | required | The word. |
| `from` | `auto` | Its language. |
| `to` | `auto` | Language of the explanations. |
| `service` | the active dictionary | A dictionary provider id. |

```console
$ curl -s -H "Authorization: Bearer $T" \
    -d '{"text":"serendipity","from":"en","to":"ru","service":"wiktionary"}' \
    http://127.0.0.1:60828/v1/dictionary
{"elapsed_ms":703,"explanations":[{"explains":["The phenomenon of making an unplanned, fortunate discovery …","…"],"trait":"noun"}],"headword":"serendipity","link":"https://en.wiktionary.org/wiki/serendipity","provider_id":"wiktionary"}
```

The article has the same fields as [`glossa-cli --json dict`](cli.md#dict):
`headword`, `pronunciations`, `explanations` (`trait`, `explains`),
`examples`, `associations`, `html`, `link`, `provider_id`, `elapsed_ms`;
empty ones are left out.

### POST /v1/ocr

Two ways to send the image:

- The raw bytes as the body, with `Content-Type: image/…` (or
  `application/octet-stream`); `lang` and `translate_to` in the query string.
- A JSON body: `{"image_base64": "…", "lang": "ja", "translate_to": "en"}`.

| parameter | default | meaning |
|---|---|---|
| `lang` | `auto` | Language of the text in the image. |
| `translate_to` | none | Translate the recognised text into this language. |
| `engine` | `[ocr] provider`, with fail-over | Use only this OCR engine (`lens`, `windows-ocr`, `oneocr`, `macos-vision`, `tesseract`, `command-ocr`, a module's id), as `glossa-cli ocr -e` does. `service` is accepted as another name. An id that is not an OCR engine is a 400. |
| `format` | none | `text`, `json` (the full layout: blocks, lines, words with boxes), `hocr` or `alto`: answer with that document instead of the summary below. |
| `in_place` | `false` | Translate block by block, each block with its box, under `blocks` (into `translate_to`, else the first language). |

```console
$ curl -s -H "Authorization: Bearer $T" -H "Content-Type: image/png" \
    --data-binary @shot.png "http://127.0.0.1:60828/v1/ocr?translate_to=en"
{"detected":"ja","elapsed_ms":412,"service":"lens","text":"…recognised text…","translation":"…"}
```

The summary is the same object `glossa-cli --json ocr` prints.
`translation` is null without `translate_to`; `detected` is the language the
engine reported, or null; `service` is the engine that read the image and
`elapsed_ms` how long it took. `translator` is who translated (`lens` when
Google Lens translated the picture itself, as `[ocr] translate_with`
allows), `image_size` the picture's size, and `blocks` is filled with
`in_place` (the shape is described under [`glossa-cli ocr`](cli.md#ocr)). An
engine that is not installed or not set up answers 503 (see
[Errors](#errors)). Engines and their setup:
[ocr.md](ocr.md).

### POST /v1/speak

| parameter | default | meaning |
|---|---|---|
| `text` | required | The text. |
| `lang` | `auto` | Its language; `auto` detects it. |
| `slow` | see below | `true` for slow speech, `false` for normal. |

The response body is the audio, `audio/mpeg` (or `audio/wav` from a service
that makes WAV).

```console
$ curl -s -H "Authorization: Bearer $T" -d '{"text":"Hello there","lang":"en"}' \
    -o hi.mp3 http://127.0.0.1:60828/v1/speak
```

Without `slow`, the classic rule applies: the same text asked for again
within five seconds is spoken slowly, if `[advanced] slower_listen_repeat`
is on.

### /v1/action/<action>

GET or POST. Asks the desktop program to perform an action, as its hotkey
would, and returns at once:

```console
$ curl -s -X POST "http://127.0.0.1:60828/v1/action/show_popup?token=$T&text=hello%20world"
{"accepted":"show_popup"}
```

The status is `202 Accepted`: the action has been handed to the program, and
its result shows on screen, not in the response. If the window system was
unloaded to save memory, the request brings it back.

`text` (in the query string, or else the whole request body as plain UTF-8
text) is used instead of reading the selection by the actions that take
text: `show_main_window`, `show_popup`, `show_dictionary`,
`listen_selection`, `copy_translation`, `replace_selection`,
`convert_layout`, `listen_translation`. Without it, those actions read the
selection in the foreground program, as a hotkey does.

```console
$ curl -s -H "Authorization: Bearer $T" --data-binary 'text in the body' \
    http://127.0.0.1:60828/v1/action/show_main_window
{"accepted":"show_main_window"}

$ curl -s "http://127.0.0.1:60828/v1/action/self_destruct?token=$T"
{"error":"no action \"self_destruct\""}
```

The action ids are listed in [cli.md](cli.md#action): `show_main_window`,
`show_popup`, `show_dictionary`, `show_history`, `show_keyboard`,
`listen_selection`, `copy_translation`, `replace_selection`,
`toggle_mouse_mode`, `translate_clipboard`, `translate_clipboard_in_main`,
`translate_clipboard_in_popup`, `dictionary_from_clipboard`,
`convert_layout`, `listen_translation`, `text_recognition`, `speech_input`,
`recognize_and_translate`, `repeat_last_recognition`,
`recognize_to_clipboard`, `watch_region`, `translate_region_in_place`,
`swap_languages`, `next_service`,
`show_options`, `quit`.

[`glossa-cli action`](cli.md#action) sends these requests for you, reading
the address and token from `config.toml`.

## pot-app compatibility

Glossa answers the routes of pot-app's HTTP server, so key bindings and
tools made for pot-app can drive it:

| route | method | becomes |
|---|---|---|
| `/selection_translate` | any | `show_popup` |
| `/input_translate` | any | `show_main_window` |
| `/ocr_recognize` | any | `text_recognition` |
| `/ocr_translate` | any | `recognize_and_translate` |
| `/translate` | POST | `show_popup` with the request body as the text |

They answer `202 {"accepted": "<action>"}` like `/v1/action/…`. `/translate`
with another method is a 405.

pot-app's bindings send no token, so these five routes accept requests
without one when all of these hold:

- `[server] pot_compat_without_token` is on (the default);
- the request comes from a loopback address (this machine);
- it carries none of the headers browsers send (no request with one gets
  anywhere, as below), and its `Host` is `127.0.0.1`, `localhost` or
  `[::1]`.

A token, when sent, is still accepted. Every other route needs the token
regardless.

What this opens up: any program running on this machine, as any user, can
show the popup or the main window, start a screen recognition, or put text
of its choosing into the popup. It cannot read anything back (the answer is
only `{"accepted": …}`), cannot reach the translation, OCR or speech
endpoints that use your service keys, and cannot type into other programs
(`replace_selection` and `convert_layout` are not among these routes).

Web pages cannot use them. A page's scripts and forms send an `Origin`
header. An image or a link on a page (`<img
src="http://127.0.0.1:60828/selection_translate">`) sends no `Origin`, but
every current browser sends fetch metadata with it (`Sec-Fetch-Site`,
`Sec-Fetch-Mode`, `Sec-Fetch-Dest`; Chrome and Edge since 2020, Firefox
since version 90, Safari since 16.4), and Glossa refuses any request with
one, or with a `Referer`. That is why the tokenless default is safe to keep:
what remains open is programs already running on this machine, which could
equally press the hotkey. The one gap is a browser too old to send fetch
metadata, on an `https` page (whose links to `http` carry no `Referer`).
If that matters, set `pot_compat_without_token = false` and add `?token=…`
to the bindings' URLs.

## D-Bus

On Linux and the BSDs the desktop program (and [`glossa-cli
serve`](cli.md#serve)) also offers its actions and translation on the
session bus, the way desktop software there expects to be driven:

| | |
|---|---|
| name | `io.github.bropines.Glossa` |
| object | `/io/github/bropines/Glossa` |
| interface | `io.github.bropines.Glossa1` |

| member | signature | does |
|---|---|---|
| `Action(s action, s text)` | method | Performs any [action](#v1actionaction) by its id, as its hotkey would; `text`, when not empty, is used instead of the selection. An unknown id is an `org.freedesktop.DBus.Error.InvalidArgs` error listing the ids. |
| `Translate(s text, s from, s to, s service) → (s translation, s detected, s service)` | method | Translates and returns the translation, the source language used (after detection) and the service that answered. Empty `from`, `to` and `service` mean auto, the settings' direction and the active service, with fail-over. A bad language or service id, or the same language on both sides, is `InvalidArgs`; a failed translation is `org.freedesktop.DBus.Error.Failed` with Glossa's message. Not recorded in the history. |
| `ShowPopup(s text)` | method | `show_popup`: the popup for `text`, or for the selection when empty. |
| `ShowMain()` | method | `show_main_window`. |
| `Recognize()` | method | `text_recognition`: select a screen region and recognise it. |
| `Version` | property, `s` | Glossa's version. |

No token is needed: the session bus belongs to the logged-in user, and the
bus daemon lets no other user connect to it. Like `/v1/action/…`, an action
returns at once and wakes the window system if it was unloaded. One process
owns the name; a second Glossa (another data folder) runs without it, as
`glossa-cli doctor` shows. `[server] dbus = false` switches the service off
([configuration.md](configuration.md#server)); it is independent of the HTTP
API's `enabled`.

```bash
# The popup for the current selection
gdbus call --session --dest io.github.bropines.Glossa \
  --object-path /io/github/bropines/Glossa \
  --method io.github.bropines.Glossa1.Action show_popup ''

# The popup for given text
gdbus call --session --dest io.github.bropines.Glossa \
  --object-path /io/github/bropines/Glossa \
  --method io.github.bropines.Glossa1.ShowPopup 'Hello world'

# Translate (prints ('Привет', 'en', 'google'))
gdbus call --session --dest io.github.bropines.Glossa \
  --object-path /io/github/bropines/Glossa \
  --method io.github.bropines.Glossa1.Translate Hello '' ru ''

# The same with busctl (systemd) and qdbus (KDE)
busctl --user call io.github.bropines.Glossa /io/github/bropines/Glossa \
  io.github.bropines.Glossa1 Action ss text_recognition ''
busctl --user get-property io.github.bropines.Glossa /io/github/bropines/Glossa \
  io.github.bropines.Glossa1 Version
qdbus io.github.bropines.Glossa /io/github/bropines/Glossa \
  io.github.bropines.Glossa1.Action recognize_to_clipboard ''
```

`gdbus introspect --session --dest io.github.bropines.Glossa --object-path
/io/github/bropines/Glossa` lists the interface; D-Feet and Bustle show it
too.

## Binding hotkeys to it

On KDE Plasma 6, GNOME 48 and later and Hyprland, Glossa registers its
hotkeys through the desktop's shortcut portal, so nothing here is needed
there: the desktop asks once to confirm them, and they are changed in its
own settings. Elsewhere on Wayland (Sway and other wlroots compositors,
older GNOME), and for actions without a hotkey, bind a desktop shortcut to
a command.

On Linux the D-Bus call needs no token, which makes it the simplest
command:

```bash
gdbus call --session --dest io.github.bropines.Glossa --object-path /io/github/bropines/Glossa --method io.github.bropines.Glossa1.Action show_popup ''
```

- **KDE Plasma**: *System Settings → Keyboard → Shortcuts → Add New →
  Command or Script*, with the command above.
- **GNOME**: *Settings → Keyboard → View and Customise Shortcuts → Custom
  Shortcuts → +*, with the command above. To pass the highlighted text where
  Glossa cannot read it (GNOME has no PRIMARY access for programs), use
  `sh -c 'gdbus call --session --dest io.github.bropines.Glossa --object-path /io/github/bropines/Glossa --method io.github.bropines.Glossa1.ShowPopup "$(wl-paste --primary --no-newline)"'`.
- **Sway**: `bindsym Ctrl+Shift+q exec gdbus call --session --dest io.github.bropines.Glossa --object-path /io/github/bropines/Glossa --method io.github.bropines.Glossa1.Action show_popup ''`.

The recipes below use the HTTP API, which works everywhere (macOS and
Windows included). Replace `TOKEN` with your token. The examples use
`show_popup`; any action id works.

When the binding fires, the foreground program still has the focus, so
Glossa reads its selection as it would for its own hotkey. Where reading the
selection does not work (some Wayland compositors), pass the text yourself:
the primary selection is what is highlighted.

```bash
# Wayland: send the highlighted text with the request
wl-paste --primary --no-newline | curl -s --data-binary @- \
  "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"

# X11
xclip -o -selection primary | curl -s --data-binary @- \
  "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"
```

### GNOME

*Settings → Keyboard → View and Customise Shortcuts → Custom Shortcuts → +*.
Command:

```bash
curl -s "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"
```

For the pipe version, wrap it: `sh -c 'wl-paste --primary --no-newline | curl -s --data-binary @- "…"'`.

### KDE Plasma

*System Settings → Keyboard → Shortcuts → Add New → Command or Script*, with
the same command.

### Sway, i3, Hyprland

```
# ~/.config/sway/config (i3: ~/.config/i3/config)
bindsym Ctrl+Shift+q exec curl -s "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"
bindsym Ctrl+Shift+o exec curl -s "http://127.0.0.1:60828/v1/action/text_recognition?token=TOKEN"
```

```
# ~/.config/hypr/hyprland.conf
bind = CTRL SHIFT, Q, exec, curl -s "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"
```

### macOS

With the Shortcuts app: a new shortcut with a *Run Shell Script* action
running the `curl` command, then *Shortcut Details → Add Keyboard Shortcut*.
With [skhd](https://github.com/koekeishiya/skhd):

```
# ~/.config/skhd/skhdrc
ctrl + shift - q : curl -s "http://127.0.0.1:60828/v1/action/show_popup?token=TOKEN"
```

### Windows: AutoHotkey

Glossa registers its own hotkeys on Windows; AutoHotkey is useful for
combinations Windows will not register, mouse buttons, or per-program
bindings. AutoHotkey v2, without a console window flashing up:

```autohotkey
#Requires AutoHotkey v2.0

GlossaAction(action, text := "") {
    req := ComObject("WinHttp.WinHttpRequest.5.1")
    req.Open("POST", "http://127.0.0.1:60828/v1/action/" action, false)
    req.SetRequestHeader("Authorization", "Bearer TOKEN")
    req.Send(text)
}

^+q::GlossaAction("show_popup")           ; Ctrl+Shift+Q
XButton1::GlossaAction("text_recognition") ; the mouse's back button
#HotIf WinActive("ahk_exe chrome.exe")
^t::GlossaAction("show_popup")            ; Ctrl+T in Chrome only
#HotIf
```

### Windows: PowerShell

```powershell
$h = @{ Authorization = "Bearer $token" }
$api = "http://127.0.0.1:60828"

# Translate. Sending the body as UTF-8 bytes keeps non-ASCII text intact
# in Windows PowerShell 5.1 as well as in PowerShell 7.
$body = [Text.Encoding]::UTF8.GetBytes((@{ text = "Hello world"; to = "ru" } | ConvertTo-Json))
Invoke-RestMethod -Method Post -Uri "$api/v1/translate" -Headers $h `
  -ContentType "application/json; charset=utf-8" -Body $body

# Recognise a screenshot and translate it
Invoke-RestMethod -Method Post -Uri "$api/v1/ocr?translate_to=en" -Headers $h `
  -ContentType "image/png" -InFile .\shot.png

# Speak to a file
Invoke-WebRequest -Method Post -Uri "$api/v1/speak" -Headers $h `
  -Body '{"text":"Hello","lang":"en"}' -OutFile hi.mp3

# An action
Invoke-RestMethod -Method Post -Uri "$api/v1/action/show_popup" -Headers $h
```

In Windows PowerShell 5.1, `curl` is an alias of `Invoke-WebRequest`; call
`curl.exe` for the real curl, which ships with Windows 10 and 11.

## Security

- **Loopback only by default.** `127.0.0.1` is reachable from this machine
  alone. Binding another address (`0.0.0.0:60828`) requires a token; the
  traffic is plain HTTP, so the token and the text cross the network
  unencrypted. Expose it only on a network you trust, or reach it through an
  SSH tunnel instead.
- **The token is the main defence.** Any program running as any user on the
  machine can connect to a loopback port; without the token it gets only
  `/health` and, unless `[server] pot_compat_without_token` is off, the
  window-opening [pot-app routes](#pot-app-compatibility). The token is
  never empty. Keep the data folder private: the token is stored there in
  plain text, as are service keys.
- **Prefer the header.** A token in the query string ends up in shell
  history and in the process list while `curl` runs.
- **Browsers are refused.** A request with any of the headers only
  browsers send gets 403, whatever the value and even with the token:
  `Origin` (scripts and form posts; `null` from sandboxed frames and
  `file:` pages), `Sec-Fetch-Site`, `Sec-Fetch-Mode`, `Sec-Fetch-Dest`,
  `Sec-Fetch-User` (every request of a current browser, images and links
  included) and `Referer`. `curl`, PowerShell, AutoHotkey and the like send
  none of these.
- **The `Host` is checked.** It must be `127.0.0.1`, another loopback
  address, `[::1]` or `localhost`, with any port (or absent, which browsers
  never do). A page that points its own domain at 127.0.0.1 (DNS
  rebinding) still sends its domain as the host and gets 403. A server
  listening beyond loopback also accepts its network address as the host,
  but never a domain name.
- **What the token allows.** Translating text through your configured
  services (with your keys), typing into the foreground program
  (`replace_selection`, `convert_layout`), showing the clipboard
  (`translate_clipboard`), and quitting Glossa (`quit`). Responses to
  actions carry no data, so the API does not hand out the clipboard or the
  selection. Regenerate the token if it leaks.
