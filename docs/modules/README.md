# Modules

Everything Glossa translates, looks up, speaks or recognises comes from a
**provider**. Some are built in (Google, Bing, DeepL, Yandex, Lens OCR…); the
rest are **modules**: directories Glossa finds at start-up and loads without
being rebuilt.

| kind | you write | runs as | good for | guide |
|---|---|---|---|---|
| JavaScript | `main.js` | inside Glossa (boa engine), a fresh context per call | web APIs: a few requests and some JSON | [js-modules.md](js-modules.md) |
| Python | `main.py` | a child process, started on first use, stopped when idle | anything needing Python libraries (ML models, `chrome-lens-py`, scraping) | [../../sdk/python/README.md](../../sdk/python/README.md) |
| Native | a `.dll` / `.so` / `.dylib` | loaded into Glossa | speed, offline engines, other languages with a C ABI (Rust, C, C++, Go, Zig) | [native-modules.md](native-modules.md) |

Glossa also loads, without changes, scripts written for other programs:
classic **QTranslate** `Service.js` services and **pot-app** plugins
(`info.json` + `main.js`, or a `.potext` archive). See
[classic-scripts.md](classic-scripts.md).

All three native kinds speak the same [module protocol](protocol.md): the same
JSON requests, the same results, the same host calls. To the engine they are
interchangeable, and a module can move from one runtime to another without
reshaping its data.

## Where modules live

Glossa scans these directories, in order (later ones win on a duplicate id):

1. `<folder of glossa.exe>/modules`
2. `<prefix>/share/glossa/modules` (a Unix install)
3. `<data folder>/modules`: `%APPDATA%\Glossa\modules` on Windows,
   `~/.config/glossa/modules` on Linux, `~/Library/Application Support/Glossa/modules`
   on macOS, or `data/modules` next to the executable in portable mode.

One module is one directory with a `module.toml`. *Options → Modules →
Install module…* copies a directory or a `.potext` file into the last of
these; *Reload modules* on the same page picks up changes without a restart.
More folders can be added there too (`[modules] dirs` in `config.toml`).

```
modules/
├── free-dictionary/        a JavaScript module
│   ├── module.toml
│   └── main.js
├── lens-ocr/               a Python module
│   ├── module.toml
│   └── main.py
└── shout/                  a native module
    ├── module.toml
    └── shout.dll
```

## module.toml

```toml
capabilities = ["translate", "detect_language"]   # see below
languages = ["en", "ru", "de"]                    # omit for "all"

[module]
id = "my-translator"          # lowercase letters, digits, - _ . ; unique
name = "My Translator"        # shown in the UI
version = "1.0.0"
api = 1                       # module protocol major version
runtime = "js"                # js | python | native
entry = "main.js"             # main.js / main.py / the library's file stem
description = "One line for the services list."
homepage = "https://example.com"
author = "You"
license = "MIT"
max_input_chars = 5000        # optional: longer text is cut at a word boundary
idle_timeout_secs = 300       # Python only: stop the process after this long unused

[language_map]                # optional: Glossa's code -> the service's
"zh-Hans" = "zh"
"he" = "iw"

[permissions]
network = ["api.example.com", "*.example.org"]   # "*" for any host; empty = no network

[[options]]                   # each becomes a field on the module's settings page
key = "api_key"
type = "secret"               # text | secret | url | bool | number | select | multiline
label = "API key"
help = "Get one at https://example.com/keys"
required = true

[[options]]
key = "formality"
type = "select"
label = "Formality"
default = "default"
choices = [{ value = "default", label = "Default" }, { value = "more", label = "Formal" }]
```

**Capabilities** say which methods the module implements:

| capability | method | the module is offered as |
|---|---|---|
| `translate` | `translate` | a translator (main window, popup, service strip) |
| `detect_language` | `detect` | a language detector |
| `dictionary` | `dictionary` | a dictionary (dictionary window, single words) |
| `listen` | `speak` | text to speech |
| `ocr` | `recognize` | text recognition for screen regions |

**Languages** are BCP 47 codes: `en`, `ru`, `zh-Hans`, `zh-Hant`, `pt-BR`…
`glossa-cli services` and the languages list in the main window show the full
set. A source of `auto` means "detect it". If the service spells a language
differently, map it in `[language_map]` instead of in code.

**Permissions** limit the requests a module makes *through Glossa*
(`host.fetch` / `host.http`): a request to a host not on the list never
leaves, and the module gets an error naming the permission. Those requests use
the user's proxy settings and Glossa's timeouts and retries. A Python or
native module can open its own connections; permissions do not cover those
(they are ordinary programs), which is one reason to prefer the host calls.

**Options** are stored in the user's `config.toml` under
`[services.options.<module id>]`, and every request carries them as
`options`. A `required` option is marked with `*` on the settings page; when
it is empty, fail with `missing_credential` and the option's key as the
message, and Glossa tells the user which setting to fill in.

## Trying a module

```bash
glossa-cli modules                          # what loaded, what failed and why
glossa-cli services -c dictionary           # who offers what
glossa-cli translate -s my-translator -t ru "Hello"
glossa-cli dict -s free-dictionary -f en serendipity
glossa-cli -v translate -s my-translator -t de "Hi"   # with the module's log lines
```

`GLOSSA_HOME=<dir>` points Glossa at a separate data folder, handy for a
clean test with only your module in `<dir>/modules`.

## Which runtime?

- A web API with JSON in and out: **JavaScript**. No process, no toolchain,
  nothing to install for the user.
- You need a Python library, or the code already exists in Python: **Python**.
  The process costs memory only while in use.
- A local engine, heavy computation, or a language other than JS/Python:
  **native**. Fastest, but a crash in it takes Glossa down, so it is for code
  you trust.
