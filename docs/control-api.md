# Control API

A running Glossa answers HTTP requests on localhost. Anything on the machine
can use it: a script, a launcher, a Stream Deck, AutoHotkey, or a key binding
in the desktop environment. That last one is why it exists: on Wayland an
application cannot register global hotkeys, but the compositor can run a
command, and `curl` to the control API makes the popup work there too.

The API can translate, detect, look up, recognise and speak, returning JSON,
and it can ask the desktop program to perform any of its actions (show the
popup, recognise a screen region…), exactly as a hotkey would.

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

Glossa itself uses the API: starting `glossa` while it is already running
asks the running copy, through the API, to show its main window. With the
API off, a second start opens a second copy.

[`glossa-cli serve`](cli.md#serve) runs the same API without the desktop
program, for scripts and for trying it out; there, actions are only printed.

## The token

Every request except `/health` must carry the token, either as a header or
in the query string:

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

An empty token is replaced by a new one at the next start, so the check
cannot be switched off for good.

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
- String parameters (`text`, `from`, `to`, `service`, `lang`,
  `translate_to`) may be given in the JSON body or in the query string; the
  body wins. The query string is percent-decoded, and `+` is a space.
  Boolean parameters (`back`, `slow`) are read from the JSON body only.
- Languages are codes: `en`, `ru`, `zh-CN`, `auto`. Unlike the command line,
  the API does not reject an unknown code: it is treated as `auto`.
- A service is a provider id from `/v1/services`. An unknown id is skipped
  and the active service answers instead, as in the command line.
- A trailing `/` in the path is ignored. Bodies are limited to 32 MiB.
- Each request runs on its own thread: a slow translation does not hold up
  an action.
- Translations made through the API are not added to the history.

### Errors

Errors have a status code and a body of the form:

```json
{"error": "source and target language are the same"}
```

| status | when |
|---|---|
| 400 | Invalid JSON, a missing `text`, empty text, the same source and target language, a configuration problem (a service or OCR engine not set up). |
| 401 | The token is missing or wrong. |
| 403 | The request carries a browser `Origin` header. |
| 404 | No such route (a wrong method included) or action; no service can do what was asked (no OCR engine, no speech service). |
| 412 | The service needs a key that is not set. |
| 413 | The body is larger than 32 MiB. |
| 422 | No text was found in the image, or the language could not be detected. |
| 429 | The service is rate-limiting. |
| 502 | The service failed otherwise (network error, unexpected answer). |
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
{"name":"glossa","version":"0.1.0"}
```

`/` answers the same. Neither needs the token, so a script can check that
Glossa is running before it does anything else.

### GET /v1/services

| parameter | default | meaning |
|---|---|---|
| `capability` | `translate` | `translate`, `dictionary`, `listen`, `ocr`, `detect_language`, `speech_input`, `batch`, `alternatives`, `promptable`, `offline` (aliases: `dict`, `tts`, `detect`, `stt`). An unknown value means `translate`. |

```console
$ curl -s "http://127.0.0.1:60828/v1/services?capability=listen&token=$T"
[{"capabilities":["translate","detect_language","listen"],"enabled":true,"id":"google","name":"Google Translate","origin":"Built-in"}]
```

Each item has `id`, `name`, `origin` (`Built-in`, `Bundled`, or the kind of
module), `enabled` (shown in the service strip) and `capabilities`.

### /v1/translate

GET or POST.

| parameter | type | default | meaning |
|---|---|---|---|
| `text` | string | required | The text. |
| `from` | language | `auto` | Source language. |
| `to` | language | `auto` | Target language; `auto` lets smart direction choose (first language ↔ second, see [`[general]`](configuration.md#general)). |
| `service` | provider id | the active service | Ask this service first; fail-over still applies when it is on. |
| `back` | boolean, body only | `false` | Also translate the result back. |

```console
$ curl -s "http://127.0.0.1:60828/v1/translate?text=Hello%20world&to=ru&token=$T"
{"alternatives":[],"back":null,"elapsed_ms":155,"from":"en","romanization":"Privet, mir","service":"google","text":"Привет, мир","to":"ru"}

$ curl -s -H "Authorization: Bearer $T" \
    -d '{"text":"Good morning","from":"en","to":"de","service":"yandex-web","back":true}' \
    http://127.0.0.1:60828/v1/translate
{"alternatives":[],"back":"Good morning","elapsed_ms":618,"from":"en","romanization":null,"service":"yandex-web","text":"Guten Morgen","to":"de"}
```

| field | type | meaning |
|---|---|---|
| `text` | string | The translation. |
| `from`, `to` | string | The direction actually used. |
| `service` | string | The provider that answered. |
| `romanization` | string or null | A Latin transcription, when the service gives one. |
| `alternatives` | array of strings | Other translations, from services that offer them. |
| `back` | string or null | The back translation, when `back` was set. |
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
| `in_place` | `false` | Translate block by block, each block with its box, under `blocks` (into `translate_to`, else the first language). In the JSON body, or `in_place=1` in the query string. |

```console
$ curl -s -H "Authorization: Bearer $T" -H "Content-Type: image/png" \
    --data-binary @shot.png "http://127.0.0.1:60828/v1/ocr?translate_to=en"
{"detected":"ja","service":"lens","text":"…recognised text…","translation":"…"}
```

`translation` is null without `translate_to`; `detected` is the language the
engine reported, or null. The reply also has `translator` (who translated:
`lens` when Google Lens translated the picture itself, as `[ocr]
translate_with` allows), `image_size`, and `blocks` (with `in_place`; the same
shape as `glossa-cli --json ocr --in-place`, see [cli.md](cli.md#ocr)). The engine is the one set in `[ocr] provider`,
with fail-over to the others; the API cannot pick one per request (the
command line can, with `-e`). Engines and their setup:
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

They answer `202 {"accepted": "<action>"}` like `/v1/action/…`. They need
the token too, which pot-app's bindings do not send: add `?token=…` to the
URL.

## Binding hotkeys to it

Each recipe runs a command that sends an action. Replace `TOKEN` with your
token. The examples use `show_popup`; any action id works.

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
  `/health`. Keep the data folder private: the token is stored there in
  plain text, as are service keys.
- **Prefer the header.** A token in the query string ends up in shell
  history and in the process list while `curl` runs.
- **Browsers are refused.** A request with an `Origin` header (other than
  `null`) gets 403, so a web page's scripts cannot use the API, even with
  the token. Requests a page can make without an `Origin` header (a link,
  an image) still need the token, which a page has no way to learn. The
  `Host` header is not checked.
- **What the token allows.** Translating text through your configured
  services (with your keys), typing into the foreground program
  (`replace_selection`, `convert_layout`), showing the clipboard
  (`translate_clipboard`), and quitting Glossa (`quit`). Responses to
  actions carry no data, so the API does not hand out the clipboard or the
  selection. Regenerate the token if it leaks.
