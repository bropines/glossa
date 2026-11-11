# Command line

`glossa-cli` is Glossa without a window. It uses the same engine, settings,
services and modules as the desktop program, so a service configured in
*Options* works here too, and the other way round. It does not need the
desktop program to be running, except for [`action`](#action), which asks a
running one to do something.

Results go to stdout; status lines, timings and errors go to stderr. With
`--json`, stdout carries one JSON document per command.

```bash
glossa-cli translate -t ru "Hello, world"     # translate
glossa-cli ocr screenshot.png -t en           # recognise and translate a picture
glossa-cli services                           # what is available
glossa-cli doctor                             # what works on this machine
```

## Contents

- [Synopsis](#synopsis): global flags, how arguments are read, languages and
  services
- [Exit codes](#exit-codes)
- [Commands](#commands): [`translate`](#translate), [`detect`](#detect),
  [`dict`](#dict), [`ocr`](#ocr), [`speak`](#speak),
  [`services`](#services), [`modules`](#modules), [`module`](#module),
  [`assets`](#assets), [`python`](#python), [`preset`](#preset),
  [`history`](#history),
  [`import-classic`](#import-classic), [`theme`](#theme),
  [`locale`](#locale), [`config`](#config), [`serve`](#serve),
  [`action`](#action), [`doctor`](#doctor), [`update`](#update)
- [Environment variables](#environment-variables)
- [The desktop program's own flags](#the-desktop-programs-own-flags)

## Synopsis

```
glossa-cli [--json] [--home DIR] [-v] <command> [options] [text…]
```

`glossa-cli` alone, `glossa-cli help`, or `-h` / `--help` anywhere prints the
command list and exits with 0.

### Global flags

| flag | does |
|---|---|
| `--json` | Machine-readable output on stdout (shapes are given per command below). Errors become `{"error": "…"}` on stdout. |
| `--home DIR` | Use `DIR` as the data folder, as `GLOSSA_HOME` does. `--home=DIR` works too. |
| `-v`, `--verbose` | Log at `debug` level to stderr. `GLOSSA_LOG`, when set, takes precedence. |
| `-V`, `--version` | Print the version and the commit it was built from (`glossa-cli 0.1.0 (1a2b3c4d)`; with `--json`, `{"name","version","commit"}`) and exit. A build without git (from a source archive) takes the commit from `GLOSSA_COMMIT` at build time, or shows the version alone. |
| `--` | Everything after it is text, even if it starts with `-`. |

Global flags may stand anywhere on the line, before or after the command.

### How arguments are read

- The first word that is not a flag is the command; the remaining words are
  its arguments. For `translate`, `detect`, `dict` and `speak` they are
  joined with single spaces into the text.
- Every flag that takes a value accepts `-t ru`, `--to ru`, `--to=ru` and
  `-t=ru` (`--home` included). Flags without a value (`--back`, `--json`)
  refuse one: `--back=yes` is a usage error.
- A word that starts with `-` followed by a digit (`-5`) is text, not a flag.
- If the same flag is given twice, the last one wins.
- A flag the command does not take is a usage error (exit 64) naming the
  closest flag it does take:

  ```console
  $ glossa-cli speak --slwo -o hi.mp3 Hello
  glossa-cli: unknown flag --slwo; did you mean --slow?
  $ glossa-cli detect --back hello
  glossa-cli: detect does not take --back
  ```

### Languages and services

Languages are codes: `en`, `ru`, `zh-CN`, `pt-BR`, `he`, and `auto` for
"detect". An unknown code is a usage error (exit 64). A target of `auto` (the
default) lets smart direction choose: text in the first language of
*Options → Basics* goes to the second, anything else to the first (see
[`[general]`](configuration.md#general)).

A service is a provider id as `glossa-cli services` lists it (`google`,
`yandex-web`, `deepl`, an AI endpoint's `ai-…` id, a module's id). An id that
does not exist, or names a provider without the capability the command
needs (a translator given to `dict`), is a usage error (exit 64) that names
a few similar ids:

```console
$ glossa-cli translate -s gogle hello
glossa-cli: unknown service "gogle"; did you mean google, google-dictionary?
```

With a valid id, fail-over to the other enabled services still applies when
`[services] fail_over` is on: check the `service` field of the output, or
the `[service · …]` line on stderr, to see who answered. Each service that
was asked first and failed adds a line on stderr before it (and an entry
in `failed` with `--json`):

```console
$ glossa-cli translate -t ru "Hello"
Привет
[google did not answer: Google Translate is rate-limiting us (try again in 30 s)]
[bing · English → Russian · 412 ms]
```

## Exit codes

| code | meaning |
|---|---|
| 0 | Success. |
| 1 | The operation failed: a network error, no service could answer, a file could not be read, the running app refused an action. The message is on stderr (or on stdout as `{"error": …}` with `--json`). |
| 64 | Bad command line: unknown command, subcommand or flag, a flag the command does not take, an unknown language, capability or service id, a missing argument or flag value. The message and the command list go to stderr, as text even with `--json`. |

## Commands

| command | does |
|---|---|
| [`translate`](#translate) (`tr`) | Translate text from arguments or stdin. |
| [`detect`](#detect) | Detect the language of a text. |
| [`dict`](#dict) (`dictionary`) | Look a word up in a dictionary service. |
| [`ocr`](#ocr) | Recognise the text in an image file, optionally translate it. |
| [`speak`](#speak) | Write text to speech to a file. |
| [`services`](#services) | List providers with a capability. |
| [`modules`](#modules) | Show which modules loaded, failed or were skipped. |
| [`module`](#module) | Check a module folder and run its fixtures (`module test`, `module check`); run an installed module's action (`module action`); approve modules installed by hand (`module pending`, `approve`, `revoke`, `decline`); list, show, run or open module panels (`module panel`). |
| [`history`](#history) | List, search or export the translation history. |
| [`import-classic`](#import-classic) | Import a classic QTranslate installation. |
| [`theme`](#theme) | List, show or convert themes. |
| [`locale`](#locale) | List or convert interface languages. |
| [`config`](#config) | Show where the settings are, or print them. |
| [`serve`](#serve) | Run the control API in the foreground. |
| [`action`](#action) | Ask a running Glossa to perform an action. |
| [`doctor`](#doctor) | Show what works on this machine and what does not. |
| [`update`](#update) | Ask whether a newer Glossa has been released. |

### translate

```
glossa-cli translate [-f FROM] [-t TO] [-s SERVICE] [--back] [--save-history] [--full] [TEXT…]
```

| option | default | meaning |
|---|---|---|
| `-f`, `--from LANG` | `auto` | Source language. |
| `-t`, `--to LANG` | `auto` | Target language. |
| `-s`, `--service ID` | the active service | Ask this service first. Fail-over to the other enabled services still applies when `[services] fail_over` is on. |
| `--back` | off | Also translate the result back into the source language. |
| `--save-history` | off | Add the translation to the history (when `[history] enabled` is on) and write `history.json` at once. |
| `--full` | off | Print the romanisation and dictionary senses even when stdout is not a terminal. |

With no text arguments the text is read from stdin. If stdin is a terminal
and no text was given, that is a usage error (64). Empty or blank text fails
with "no text to translate" (1).

```console
$ glossa-cli translate -t ru "Hello, world"
Привет, мир

Privet, mir
[google · English → Russian · 196 ms]

$ echo "Bonjour tout le monde" | glossa-cli translate -t en
Hello everyone
[google · French → English · 159 ms]

$ glossa-cli translate -s yandex-web -f en -t de "Good morning"
Guten Morgen
[yandex-web · English → German · 228 ms]
```

On a terminal, stdout shows what the main window shows: the translation,
then, after a blank line each, the romanisation of the translation and the
dictionary senses if the service sent them. When stdout is not a terminal
(a pipe, a file), it holds the translation alone, so
`glossa-cli translate -t ru < in.txt > out.txt` writes just the translated
text; `--full` asks for the whole block there too. The last line, on
stderr, names the service, the direction and the time taken; with `--back`,
the back translation is printed to stderr above it, after `↩`.

When stdout is a terminal and the service streams (AI endpoints with
streaming on), the answer is printed as it is written. If a service fails
part-way and another takes over, its answer starts on a new line. Piped
output, and `--json`, get the finished text only.

`--json` prints the same object as the control API's
[`/v1/translate`](control-api.md#v1translate), every field always present:

```console
$ glossa-cli --json translate -s google -t ru --back "The quick brown fox"
{"text":"Быстрая коричневая лиса","from":"en","to":"ru","service":"google","romanization":"Bystraya korichnevaya lisa","source_romanization":null,"alternatives":[],"back":"Quick brown fox","failed":[],"elapsed_ms":139}
```

| field | type | meaning |
|---|---|---|
| `text` | string | The translation. |
| `from`, `to` | string | The direction actually used, as codes (detection and smart direction resolved). |
| `service` | string | The provider that answered. |
| `romanization` | string or null | The translation in Latin letters, when the service gives it (`Privet` for `Привет`). |
| `source_romanization` | string or null | The source's transliteration or pronunciation, when the service gives it (`Kon'nichiwa` for Japanese input, `həˈlō` for "hello"). |
| `alternatives` | array of strings | Other translations, from services that offer them. |
| `back` | string or null | The back translation, with `--back`. |
| `failed` | array | Services tried before this one that failed, as `[id, message]` pairs. |
| `elapsed_ms` | number | Time the answering service took. |

`glossa-cli translate` adds to the history only with `--save-history` (and
when `[history] enabled` is on); translations made in the desktop program
are always added. The entry is written to `history.json` straight away. If
the desktop program is running at the same time, it does not see the entry
and replaces the file with its own history when it exits; to record into a
running Glossa, use the control API's `"history": true` instead.

### detect

```
glossa-cli detect [TEXT…]
```

Text from arguments or stdin, as for `translate`.

```console
$ glossa-cli detect "Это проверка"
Russian (ru)

$ glossa-cli --json detect "Это проверка"
{"confidence":1.0,"lang":"ru","name":"Russian"}
```

`confidence` is between 0 and 1.

### dict

```
glossa-cli dict [-f FROM] [-t TO] [-s SERVICE] WORD
```

| option | default | meaning |
|---|---|---|
| `-f`, `--from LANG` | `auto` | Language of the word. |
| `-t`, `--to LANG` | `auto` | Language of the explanations and translations. |
| `-s`, `--service ID` | the active dictionary | A dictionary provider id (`glossa-cli services -c dictionary`). |

Exact search follows `[dictionary] exact_search`.

```console
$ glossa-cli dict -f en -t ru serendipity
serendipity  [ˌserənˈdipədē]

noun
  1. the occurrence and development of events by chance in a happy or beneficial way.


translation
  1. случайность
  2. noun: интуитивная прозорливость


• a fortunate stroke of serendipity
https://translate.google.com/?sl=en&tl=ru&text=serendipity&op=translate
[google-dictionary · 317 ms]
```

The article goes to stdout; the link to the source page and the timing line
go to stderr. Services that return a ready-made HTML article (the scraped
dictionaries) are converted to plain text.

`--json` prints the article, pretty-printed. Empty fields are left out:

```json
{
  "headword": "serendipity",
  "pronunciations": [{ "region": "", "symbol": "ˌserənˈdipədē" }],
  "explanations": [
    { "trait": "noun", "explains": ["the occurrence and development of events by chance in a happy or beneficial way."] },
    { "trait": "translation", "explains": ["случайность", "noun: интуитивная прозорливость"] }
  ],
  "examples": [{ "source": "a fortunate stroke of serendipity", "target": "" }],
  "associations": ["chance", "happy chance", "accident", "fluke", "luck"],
  "link": "https://translate.google.com/?sl=en&tl=ru&text=serendipity&op=translate",
  "provider_id": "google-dictionary",
  "elapsed_ms": 333
}
```

| field | meaning |
|---|---|
| `headword` | The word as the service spells it. |
| `pronunciations` | `region` (`UK`, `US` or empty), `symbol`, and `audio_url` when there is one. |
| `explanations` | Senses grouped by part of speech: `trait` is the group, `explains` the definitions. |
| `examples` | Usage examples, `source` and `target`. |
| `associations` | Word forms, synonyms, "see also". |
| `html` | A ready-made HTML article, from services that scrape one. |
| `link` | The page the article came from. |
| `provider_id`, `elapsed_ms` | Who answered, and how long it took. |

### ocr

```
glossa-cli ocr IMAGE [-l LANG] [-t TO] [-e ENGINE] [-F FORMAT] [--in-place] [--join-lines] [--translate-with W]
```

| option | default | meaning |
|---|---|---|
| `-l`, `--lang LANG` | `auto` | Language of the text in the image. |
| `-t`, `--to LANG` | none | Translate the recognised text into this language. |
| `-e`, `--engine ID` (also `-s`, `--service`) | `[ocr] provider`, with fail-over | Use only this OCR provider: `lens`, `windows-ocr`, `oneocr`, `macos-vision`, `tesseract`, `command-ocr`, an AI endpoint's id (`openai`, `ai-…`) or a module's id. An id that is not an OCR provider is a usage error (64). |
| `-F`, `--format FORMAT` | none | Print the result as `text`, `json` (the full layout: blocks, lines, words with boxes), `hocr` or `alto` instead of the summary. |
| `--in-place` | off | Translate block by block, each block with its place in the picture (what an in-place rendering paints). Translates into `-t`, else the first language. |
| `--join-lines` | off | Join the lines of each paragraph into one, as *Join lines when copying text* does. |
| `--translate-with W` | `[ocr] translate_with` | `ocr`: the OCR engine translates when it can (Google Lens, no second request); `translator`: always the current translator. |

`IMAGE` is a file path (PNG, JPEG, whatever the engine reads). Without `-t`,
stdout is the recognised text; with it, the translation. The timing line goes
to stderr.

With `--in-place`, stdout has one line per block: the source and its
translation, separated by a tab.

`--json` prints the same object as the control API's
[`/v1/ocr`](control-api.md#post-v1ocr):

```json
{"text": "…recognised text…", "translation": "…or null…", "detected": "ja", "service": "lens",
 "translator": "lens", "elapsed_ms": 412, "image_size": [725, 280], "blocks": []}
```

`detected` is the language the engine reported, or null. `translator` is the
provider that translated (`lens` when Lens translated the picture itself), or
null. `blocks` is filled with `--in-place`: `{"source", "translation",
"bounds": {"x", "y", "width", "height"}, "angle", "lines"?}`, in the picture's
pixels, the angle in radians; `lines` (Lens) places the translation line by
line. Engines and their
setup are described in [ocr.md](ocr.md).

### speak

```
glossa-cli speak [-l LANG] [--slow] -o OUT TEXT…
```

| option | default | meaning |
|---|---|---|
| `-o`, `--out FILE` | required | Where to write the audio. |
| `-l`, `--lang LANG` | `auto` | Language of the text; `auto` detects it. |
| `--slow` | off | Ask for slow speech. |

The speech service is `[services] tts`, then any other service that can
speak. The file holds what the service sends: MP3 (`audio/mpeg`) from
Google, WAV (`audio/wav`) from the system voice (`system-tts`: the Windows
voices, `say` on macOS, `espeak-ng` on Linux). `glossa-cli services -c tts`
lists the speech services.

```console
$ glossa-cli speak -l en -o hi.mp3 Hello there
9024 bytes of audio/mpeg written to hi.mp3
```

### services

```
glossa-cli services [-c CAPABILITY]
```

`CAPABILITY` is one of `translate` (the default), `dictionary` (`dict`),
`listen` (`tts`), `ocr`, `detect_language` (`detect`), `speech_input`
(`stt`), `batch`, `alternatives`, `promptable`, `offline`. Anything else is a
usage error.

```console
$ glossa-cli services
* google                       Google Translate                 [Built-in]
* bing                         Microsoft Translator             [Built-in]
* mymemory                     MyMemory                         [Built-in]
* yandex-web                   Yandex                           [Bundled]
* deepl-web                    DeepL                            [Bundled]
* lara                         Lara                             [Bundled]
* modernmt                     ModernMT                         [Bundled]
  deepl                        DeepL API (key)                  [Built-in] (needs a key)
  deeplx                       DeepLX                           [Built-in]
  libretranslate               LibreTranslate                   [Built-in]
  lingva                       Lingva Translate                 [Built-in]
  openai                       AI Translator (OpenAI-compatible) [Built-in]
  yandex                       Yandex Cloud (API key)           [Built-in] (needs a key)
```

`*` marks services enabled in the service strip. The bracket is the origin:
`Built-in` (Rust), `Bundled` (a JavaScript module compiled into the program),
or the kind of module a provider came from.

`--json` gives an array:

```json
[
  {
    "id": "lens",
    "name": "Google Lens",
    "origin": "Built-in",
    "enabled": false,
    "capabilities": ["ocr"],
    "needs_credentials": false
  }
]
```

### modules

```
glossa-cli modules
```

Lists the modules found in the module folders: loaded (by provider id),
failed (path and error) and skipped (path and reason). The modules compiled
into the program are not listed here; `services` shows them as `Bundled`.

The text output has a `loaded (N):` heading followed by one id per line
(with the runtime in brackets, such as `[Python]`), then `failed (N):` and
`skipped (N):` sections with `path: reason` lines, each shown only when not
empty. `--json`:

```json
{"loaded": ["my-translator"], "failed": [{"path": "…", "error": "…"}], "skipped": [{"path": "…", "reason": "…"}]}
```

### module

```
glossa-cli module test [--live | --record] [--case NAME] [--set-from-env KEY=VAR]… [--set KEY=VALUE]…
                       [--timeout SECS] [--strict] [--require-runtimes] [--ignore-recordings] [--download] DIR…
glossa-cli module check [--strict] DIR…
glossa-cli module action ID ACTION [--option KEY=VALUE]… [--set-from-env KEY=VAR]… [--state KEY=VALUE]… [--panel PANEL]
glossa-cli module pending
glossa-cli module approve ID [--local] [--force]
glossa-cli module revoke ID [--local] [--force]
glossa-cli module decline ID [--local] [--force]
glossa-cli module panel [ID/PANEL [--action ACTION] [--state KEY=VALUE]… [--item JSON] [--open]]
```

The module test harness, for module authors. `module test` checks each
module's `module.toml` and files, loads the module through the same code
the program uses, runs the cases in its `tests/*.toml` (or `*.json`) with
recorded HTTP answers, and checks every answer against the module
protocol. `module check` does the first step only and runs no code. A
`DIR` without a `module.toml` is searched for modules below it. Neither
needs a data folder or reads `config.toml`, and neither asks for
[approval](modules/README.md#permissions-and-trust): `module test` loads and
runs the module with every permission it declares, which is the reason to
point it only at a folder you mean to run (the module editor's *Test*, by
contrast, refuses a module that waits for approval). The fixture format and every
check are described in [modules/testing.md](modules/testing.md).

| option | default | meaning |
|---|---|---|
| `--live` | off | Call the real services instead of the recorded answers. |
| `--record` | off | Call the real services and write their answers to `tests/recorded/`, without the values given by `--set` or `--set-from-env`. Later replays prefer these recordings over the fixtures' own `[[case.http]]` entries. |
| `--ignore-recordings` | off | Replay only the fixtures' own entries, ignoring `tests/recorded/`. |
| `--set-from-env KEY=VAR` | none | An option value read from the environment variable `VAR`; use this for keys, which `--set` would leave in the shell's history. Repeatable. |
| `--case NAME` | all | Only the cases whose name contains `NAME`. |
| `--set KEY=VALUE` | none | An option value over the fixtures' own (a number or `true`/`false` is passed as one). Repeatable. |
| `--timeout SECS` | 30 | The budget for one call. |
| `--strict` | off | Warnings fail a module too, not only errors. |
| `--require-runtimes` | off | A module that cannot run here (no interpreter, not built, its files not downloaded) fails instead of being skipped. |
| `--download` | off | Download a module's missing required files ([assets](modules/README.md#assets)), and the managed Python and packages a Python module needs ([Python dependencies](modules/README.md#python-dependencies)), before running it; without it such a module is skipped with the command to run. |

`module action` runs one of an installed module's
[`[[actions]]`](modules/README.md#actions) as its button on the settings
page would, with the saved settings and `--option KEY=VALUE` (or `--set`,
or `--set-from-env KEY=VAR` for a key) over them, and prints the reply: the
notice, the messages for fields, loaded choices and options it would set.
With `--panel PANEL` the action runs as that [panel](modules/panels.md)
runs it, `call` and `builtin` actions included, with `--state` as the
panel's state. An action whose `needs` are
not filled in fails before the module is called. The exit code is 1 when
the reply marks a field or its notice is an error. `--json` prints the
reply as the module sent it: `{"notice", "notice_level", "choices",
"errors", "options", "state"}`, each present only when set.

`module pending` lists the modules you installed by hand that wait for
your approval ([modules/README.md](modules/README.md#permissions-and-trust)),
each with what it asks for in plain words: the hosts it reaches, the
program it runs, the files it downloads with their sizes and hosts, the
Python packages it installs, its actions. `module approve ID` approves what
the module asks for now and prints it; `module revoke ID` withdraws an
approval, so the module waits again; `module decline ID` switches it off.
These go through the control API of a running Glossa when it answers, and
otherwise change `config.toml`, refusing while a Glossa that cannot be
reached holds the data folder (as `preset` does: `--local` skips the
running Glossa, `--force` changes the file anyway). `--json` prints
`pending` as a list of `{id, name, dir, pending, first_time, request, new,
hash}`.

`module panel` without a name lists the panels the loaded modules declare
(`module/panel`, the window title, where they are offered). With a name it
prints the panel's elements and its state as it opens (`--json`: the panel
as written and that state). `--action ACTION` runs one of its actions with
the declared state and `--state KEY=VALUE` over it (a JSON list or object
is taken as one), `--item JSON` as a clicked list row; the streamed updates
go to stderr as `update KEY = VALUE`, then the notice, what Glossa would do
(`would {"copy": "…"}`, `{"open_url": …}`, `{"play": …}`) and the state
after it. `--open` asks a running Glossa to open the panel, through the
[control API](control-api.md#panels).

The text report has one line per module (`PASS`, `FAIL` or `SKIP`, its id,
runtime, folder, load time and size), its findings (`error:`, `warning:`,
`note:`), and one line per case with its time, the number of HTTP requests
and the result. With several modules a summary line follows. `--json`
prints `{"modules": [...], "passed": N, "failed": N, "skipped": N}`, each
module with `dir`, `id`, `runtime`, `status`, `skipped`, `size_bytes`,
`load_ms`, `findings` and `cases`. The exit code is 1 when any module
failed.

### assets

```
glossa-cli assets [list]
glossa-cli assets status ID
glossa-cli assets download ID [NAME…]
glossa-cli assets verify ID
glossa-cli assets remove ID
```

The files modules declare in `[[assets]]` (models, dictionaries, helper
programs; see [modules/README.md](modules/README.md#assets)).
`glossa-cli module assets …` (or `modules assets …`) is the same command.

- `list` (the default) shows every module that declares files, `status`
  one module: each file with its size and state (`ready`, `missing`,
  `partial`, `out of date`), and the folder they go to.
- `download` fetches the required files that are missing, or the files
  named (optional ones, such as one language pair of many). It resumes a
  partial download, checks each file's SHA-256 before putting it in place
  and shows progress on stderr when it is a terminal.
- `verify` hashes the files again and deletes damaged ones; it exits with
  1 if anything is missing or damaged.
- `remove` deletes the module's files.

`--json` prints the same report the [control API](control-api.md#v1assets)
returns:

```json
{"module": "my-ocr", "dir": "…/models/my-ocr", "ready": true, "total_size": 4718592,
 "assets": [{"name": "det", "path": "…/models/my-ocr/det", "size": 4718592, "sha256": "…", "optional": false, "state": "ready"}]}
```

### python

```
glossa-cli python [status]
glossa-cli python install [ID]
glossa-cli python remove [ID]
```

The Python that Python modules run with ([modules/README.md](modules/README.md#python-dependencies)).

- `status` (the default) shows `[modules] python`, the Python found on the
  machine, the managed Python and `uv` (ready, partly downloaded, or not
  downloaded with their size), and each Python module: its dependencies
  and whether it is `ready`, `missing` something (listed), or cannot work
  as set up (`impossible`, with why).
- `install` downloads the managed Python and `uv`. With a module id it gets
  what that module needs: the managed Python if it uses it, `uv` and its
  packages if it has dependencies. Downloads resume, are checked against
  their pinned SHA-256 and show progress on stderr when it is a terminal.
- `remove` deletes the managed Python, `uv`, every module's environment and
  `uv`'s cache; with a module id, only that module's environment.

`--json` prints the report [`/v1/python`](control-api.md#v1python) returns,
with `system_python` added.

### preset

```
glossa-cli preset [list]
glossa-cli preset apply NAME
glossa-cli preset next
glossa-cli preset save NAME [--languages]
glossa-cli preset export NAME [-o FILE]
glossa-cli preset import FILE
glossa-cli preset rename NAME NEW
glossa-cli preset duplicate NAME [NEW]
glossa-cli preset delete NAME
```

Presets: named sets of services, their order, the active one, the
dictionaries, the voice, the OCR engine and module settings, and with
`--languages` the language pair ([configuration.md](configuration.md#presets)).
A preset never holds an API key or token.

- `list` (the default) shows the presets, built-in ones first; `*` marks the
  one applied last, and services a preset names that are not installed here
  are listed under it.
- `apply` switches to a preset (names ignore case). Services it names that
  are not installed are skipped and listed. `next` applies the one after the
  current one, as the `next_preset` action does.
- `save` stores the current setup under `NAME`, replacing a preset of yours
  with that name. Built-in names are taken.
- `export` prints the preset as a `.toml` file, or writes it to `FILE`, with
  secrets left out. `import` adds one; a name already taken gets a number
  (`Work (2)`), credentials someone put into the file are dropped, and
  services that are not installed are listed (installing them is up to you).
- `rename`, `duplicate` (built-in presets too; without `NEW` the copy is
  called `NAME (2)`) and `delete` work on your own presets.

When a Glossa is running with this data folder and the
[control API](control-api.md) on, every
subcommand goes through it ([`/v1/presets`](control-api.md#presets)), so the
running program sees the change at once and does not write its own
settings over it. Otherwise, or with `--local`, the command changes
`config.toml` directly. A Glossa that holds the data folder but cannot be
reached (its control API is off, or `--local` was given) would write its
settings over such a change, so the command refuses to make it and says
so; `--force` makes it anyway, with a warning. `list` and `export` only
read and always work. A Glossa running with *another* data folder is never
contacted, even when both name the same port: the command finds its own by
the data folder and checks who answers before sending the token
([control-api.md](control-api.md#how-glossa-cli-finds-glossa)); when
something else answers there it says so on stderr and carries on as if
nothing ran.

`import` keeps only the settings the installed modules declare, never a
key and never an address (URL, endpoint, host), and lists what it left out
([configuration.md](configuration.md#presets)).

```console
$ glossa-cli preset save Work
saved preset "Work"
$ glossa-cli preset apply private
applied preset "Private"
$ glossa-cli preset export Work -o work.toml
```

`--json` prints the report (`name`, `missing`, `dropped_secrets`, `dropped_options`) or the
list as the control API returns them.

### history

```
glossa-cli history [--search TEXT] [--favorites] [--export FILE]
```

| option | meaning |
|---|---|
| `--search TEXT` | Only entries whose source or a translation contains `TEXT` (case-insensitive). |
| `--favorites` | Only favourites. |
| `--export FILE` | Write the entries to `FILE` instead of listing them. The format follows the extension: `.txt`, `.csv`, `.html` or `.json`; another extension is a usage error. A file with no extension is written as text. `--favorites` applies; `--search` does not. |

Without `--export`, the 50 newest matching entries are listed, one per line,
as `source → latest translation`, with `★` before favourites; long lines are
shortened to 60 characters. `--json` prints every matching entry in full, in
this shape:

```json
[
  {
    "id": 12,
    "source": "Hello",
    "results": [{ "provider_id": "google", "from": 17, "to": 46, "text": "Привет" }],
    "favorite": true,
    "created": 1761980000
  }
]
```

`from` and `to` are numeric language ids, as in `history.json`
([configuration.md](configuration.md#historyjson)). `created` is in Unix
seconds.

```console
$ glossa-cli history --export history.csv
0 entries written to history.csv
```

### import-classic

```
glossa-cli import-classic DIR
```

`DIR` is a classic QTranslate folder: one with a `Services` folder and a
`Data` or `Themes` folder; anything else fails with exit 1. The import:

- merges `Data/Options.json` into `config.toml` (languages, hotkeys, service
  order, popup look, exceptions, proxy…), pointing classic services at
  Glossa's own versions where they exist;
- adds `Data/History.json` to the history;
- sets `[classic] path` to `DIR`, so its services, themes and interface
  languages are used in place.

The classic files are only read. The output has this form (the numbers
depend on the installation):

```
settings: imported
history entries: 214
providers now: 41 (modules loaded: 19, failed: 2)
note: …
```

`settings` is `not found` when there is no `Options.json`. `note:` lines
report settings that could not be carried over.

### theme

```
glossa-cli theme list
glossa-cli theme show NAME
glossa-cli theme convert FILE
```

| subcommand | does |
|---|---|
| `list` (the default) | Every theme the picker offers, with where it comes from: `built-in`, a file path, or `classic: <path>`. |
| `show NAME` | The theme with every field filled in, as TOML. A good start for a theme of your own. |
| `convert FILE` | A classic JSON theme as native TOML, on stdout. |

```console
$ glossa-cli theme list
Glossa Dark              built-in
Glossa Light             built-in
Glossa Midnight          built-in
Fluent Dark              built-in
Fluent Light             built-in
macOS Dark               built-in
macOS Light              built-in
```

Writing themes: [theming.md](theming.md).

### locale

```
glossa-cli locale list
glossa-cli locale convert FILE
glossa-cli locale check FILE
glossa-cli locale template [LANG]
```

| subcommand | does |
|---|---|
| `list` (the default) | Interface languages: code, English name, native name. |
| `convert FILE` | A classic `lang.json` as a native locale file (JSON) on stdout; stderr says how many of Glossa's strings it covers. |
| `check FILE` | Check a language pack (native or classic) against the program's keys: how many strings it translates, unknown keys and language tags (warnings), and strings whose `%s` placeholders differ from English (errors: the program skips them). `-v` lists the keys that fall back to English. Exits 1 when there are errors. With `--json`: `{code, name, native_name, translated, total, missing, issues: [{key, severity, message}]}`. |
| `template [LANG]` | A pack to translate from, on stdout: every key with its English text and its classic address (`[classic w:10:1070]`) in a comment above it, and the language table's names. Values are empty, or with `LANG` (a code or name from `list`) that language's current strings, user files included, so an existing pack can be brought up to date. Empty values fall back to English, so a half-filled template already loads. |

```console
$ glossa-cli locale list
en       English                  English
ru       Russian                  Русский
$ glossa-cli locale template > eo.json
$ glossa-cli locale check eo.json
Esperanto (eo): 7 of 691 strings
error: msg.ms: 0 placeholders where English has 1; the string is skipped
warning: no.such.key: unknown key, ignored
684 strings fall back to English (-v lists them)
glossa-cli: 1 strings skipped
```

### config

```
glossa-cli config path
glossa-cli config show
```

| subcommand | does |
|---|---|
| `path` (the default) | The path of `config.toml` on stdout; the data folder and whether the layout is portable on stderr. |
| `show` | The settings in effect, as TOML: the file with every default filled in and out-of-range values clamped. |

```console
$ glossa-cli config path
/home/me/.config/glossa/config.toml
data: /home/me/.config/glossa  (portable: false)
```

Neither subcommand starts the engine, so neither writes anything; `show`
reports a file that does not parse as an error. Every other command that
uses the engine creates `config.toml` on first use, to store a newly
generated control API token, and moves a file that does not parse to
`config.toml.broken-<unix seconds>`, as the desktop program does
([configuration.md](configuration.md#how-the-file-is-read-and-written)). The fields are described in
[configuration.md](configuration.md).

### serve

```
glossa-cli serve
```

Runs the [control API](control-api.md) in the foreground until Ctrl+C, with
the address and token from `[server]`. It starts even when `[server] enabled`
is false, printing a note to stderr: that setting decides whether the
desktop program starts the API, and running `serve` is itself the request
to start it. Requests for actions are printed, since there is no window to
perform them:

```console
$ glossa-cli serve
control API on http://127.0.0.1:60828  (token: 0123456789abcdef)
try: curl -s 'http://127.0.0.1:60828/v1/translate?text=hello&to=ru&token=0123456789abcdef'
action requested: show_popup (hello world)
action requested: show_dictionary (serendipity)
```

Useful for scripts on a machine without a desktop session, and for trying
the API. If the desktop program is running with the API on, the port is
taken and `serve` fails with exit 1. While it runs, the other `glossa-cli`
commands find it as they find the desktop program, by `control-api.json` in
the data folder.

On Linux and the BSDs `serve` also offers the [D-Bus
service](control-api.md#d-bus) (unless `[server] dbus` is false), printing
`D-Bus: io.github.bropines.Glossa on the session bus` and a `gdbus` example.
If the desktop program already owns the name, `serve` notes it and runs the
HTTP API alone. With no session bus at all (a server, an ssh login),
`dbus-run-session glossa-cli serve` provides a private one.

### action

```
glossa-cli action ACTION [TEXT…]
```

Asks the Glossa running with this data folder to perform an action, as if
its hotkey had been pressed (so `--home` or `GLOSSA_HOME` must point at the
same data folder as the running program). It sends
`POST /v1/action/ACTION` with the text as the body and the token from
`[server] token` in the `Authorization` header.

The address is not taken from `[server] bind`: the port there may belong
to another program, another user's Glossa or a Glossa with another data
folder, and whoever listens would get the token. The command reads
`control-api.json`, which the running Glossa keeps in its data folder,
connects to the address in it, and sends the token only after that
process has proved it wrote the file
([control-api.md](control-api.md#how-glossa-cli-finds-glossa)). Nothing is
printed on success.

```console
$ glossa-cli action show_popup "text to translate"
$ glossa-cli action text_recognition
$ glossa-cli action nonsense
glossa-cli: no action "nonsense"
```

If no Glossa runs with this data folder (or its control API is off), the
command says so and exits 1; nothing is sent anywhere. The same when
something else answers at the address: `note: not using the control API
(…); the token was not sent`.

`ACTION` is one of the ids below. `TEXT`, when given, is used instead of
reading the selection by the actions marked "text"; the others ignore it.

| action | does | text |
|---|---|---|
| `show_main_window` | Show the main window; with text, translate it there. | text |
| `show_popup` | Translate the selection in the popup. | text |
| `show_dictionary` | Open the dictionary; with text, look it up. | text |
| `show_history` | Open the history. | |
| `show_keyboard` | Open the on-screen keyboard. | |
| `listen_selection` | Read the selection aloud. | text |
| `copy_translation` | Translate the selection to the clipboard. | text |
| `replace_selection` | Replace the selection with its translation. | text |
| `toggle_mouse_mode` | Turn mouse mode on or off. | |
| `translate_clipboard` | Translate the clipboard, in the main window if it is visible, else the popup. | |
| `translate_clipboard_in_main` | Translate the clipboard in the main window. | |
| `translate_clipboard_in_popup` | Translate the clipboard in the popup. | |
| `dictionary_from_clipboard` | Look the clipboard up in the dictionary. | |
| `convert_layout` | Retype the selection in the other keyboard layout. | text |
| `listen_translation` | Translate the selection and read the translation aloud. | text |
| `text_recognition` | Select a screen region and recognise its text. | |
| `speech_input` | Speech input (not implemented yet). | |
| `recognize_and_translate` | Recognise a region and show the translation in the popup. | |
| `repeat_last_recognition` | Recognise the last region again. | |
| `recognize_to_clipboard` | Recognise a region straight to the clipboard. | |
| `watch_region` | Watch a region and re-translate when it changes. | |
| `translate_region_in_place` | Translate a region over the picture itself. | |
| `translate_screen_in_place` | The same over the whole monitor under the pointer, with no region to select. | |
| `swap_languages` | Swap the main window's direction. | |
| `next_service` | Switch to the next enabled translation service. | |
| `next_preset` | Apply the next [preset](#preset): built-in ones first, then yours. | |
| `show_options` | Open the options. | |
| `quit` | Exit Glossa. | |

The same ids name the actions in `[hotkeys.bindings]`.

### doctor

```
glossa-cli [--json] doctor
```

Looks at this machine and prints what works and what does not, for the
user and for bug reports. Attach its output to a bug report.

| section | shows |
|---|---|
| System | The OS, the processor, and the session: Windows, macOS, X11, or Wayland with the compositor. |
| Desktop features | Whether global hotkeys, reading the selection, synthetic input, screen capture, mouse mode and the tray icon work in this session, and its limitations. |
| Permissions | macOS only: Accessibility and Screen Recording. |
| OCR engines | Lens, Windows OCR with its language packs, Apple Vision, Tesseract with its languages. |
| Modules | How many loaded, and which failed or were skipped, with the reason. |
| Files | The settings file (and whether it parses), the data and cache folders, the log. |
| Network | The proxy requests go through, the timeout, and whether certificates are checked against the system's store. |
| Control API | Switched on or not, whether a Glossa, another program or nothing answers at its address, and whether a Glossa that proved to run with this data folder is there: the only one `glossa-cli` sends its token to ([how it is found](control-api.md#how-glossa-cli-finds-glossa)). With `--json`, `control_api.instance` has `state` (`verified`, `none` or `unverified`), with `addr`, `pid` and `version` when verified and `reason` when not. |
| D-Bus service | Linux and the BSDs: switched on or not, and whether its name is owned ([D-Bus](control-api.md#d-bus)). |
| Desktop portal | Wayland only: the GlobalShortcuts and Screenshot portal versions, asked live, so it tells whether hotkeys can work in this session. `--json` also gives the RemoteDesktop version (`portal.remote_desktop`), which copy and paste into other programs need. |

The last line says whether the desktop program runs for this data folder.

Nothing is changed: a `config.toml` that does not parse is reported, not
moved aside as the desktop program does (a `control-api.json` left by a
Glossa that crashed is the exception: it is removed). With `--json`, the same as one
object; each limitation there has a `key` (a locale key, which the
interface translates) and its English `text`.

```console
$ glossa-cli doctor
Glossa 0.1.0

System
  linux x86_64, display: Wayland (GNOME), desktop ubuntu:GNOME

Desktop features
  hotkeys          partly  the desktop portal's GlobalShortcuts (GNOME 48 or later); confirmed in its dialog
  selection        partly  Ctrl+C through the RemoteDesktop portal, then the clipboard
  …
OCR engines
  lens             yes     lens_scan at /home/me/.local/bin/lens_scan
  tesseract        yes     /usr/bin/tesseract (languages: eng, rus)
  …
Control API
  enabled on 127.0.0.1:60828: Glossa 0.1.0 is running
  this data folder: Glossa 0.1.0 (pid 4242) at 127.0.0.1:60828 proved to be this folder's

D-Bus service
  enabled, io.github.bropines.Glossa: owned (Glossa is running)

Desktop portal
  global shortcuts yes (version 1)
  screenshot       yes (version 2)
```

### update

```
glossa-cli [--json] update
```

Asks GitHub whether a version newer than this one has been released and
prints the answer. Nothing is downloaded or installed: when there is a
newer version, the address of its release page is printed for you to open.

It is the check behind *Options → Updates → Check now*: one `GET` to
`https://api.github.com/repos/bropines/glossa/releases`, through the proxy
in `[network.proxy]`, carrying nothing about you or your settings. Only
tags of the form `v1.2.3` count (module bundles released from the same
repository are skipped), versions compare as semantic versions
(`0.1.0-beta.2` is older than `0.1.0`), and pre-releases are offered only to
a pre-release. The answer and its time go to `update-check.json` in the data
folder, which the desktop program shows and its daily check
([`[updates]`](configuration.md#updates)) goes by.

The exit code is 0 whether or not there is a newer version, and 1 when
GitHub could not be asked.

```console
$ glossa-cli update
Glossa 0.2.0 is available (this is 0.1.0): https://github.com/bropines/glossa/releases/tag/v0.2.0
$ glossa-cli --json update
{"current":"0.1.0","latest":"0.2.0","up_to_date":false,"url":"https://github.com/bropines/glossa/releases/tag/v0.2.0"}
```

## Environment variables

| variable | read by | meaning |
|---|---|---|
| `GLOSSA_HOME` | both programs | The data folder. Implies the portable layout: the cache goes to `$GLOSSA_HOME/cache`. An empty value is ignored. `--home` overrides it. See [configuration.md](configuration.md#where-the-files-are). |
| `GLOSSA_LOG` | both programs | A log filter in `tracing` syntax: a level (`debug`), or per-crate levels (`warn,glossa_engine=debug,glossa_modules=trace`). `glossa-cli` logs to stderr, by default at `warn` (`debug` with `-v`). The desktop program logs to `glossa.log` in the cache folder, by default `info,glossa=debug`. |
| `GLOSSA_LANG` | desktop program | The interface language when `[general] ui_language` is empty (`ru`, `de-DE`). Overrides the system language. |
| `LC_ALL`, `LC_MESSAGES`, `LANG` | desktop program | The system language, when `GLOSSA_LANG` is not set (on Windows, the display language comes first). |
| `HTTPS_PROXY`, `ALL_PROXY`, `HTTP_PROXY` (and lower-case), `NO_PROXY` | both programs | With `[network.proxy] mode = "system"`, the first one set is the proxy, and `NO_PROXY` lists hosts that bypass it. On Windows, if none is set, the system (WinINet) proxy is used. |
| `GLOSSA_SCREENSHOT_SWITCH` | desktop program | With `--screenshot`: a language (`ru`) to switch the interface to once the window is open, through the settings as Options does, to check that every part follows without a restart. Writes `ui_language` to the settings, so use a throwaway `GLOSSA_HOME`. |
| `GLOSSA_SCREENSHOT_ERROR` | desktop program | With `--screenshot`: show a sample failure in the window pictured (`credential`, `files`, or anything else for a server error): as the window's own message, and in the main window, the popup and the dictionary as the failed request with its buttons. Nothing really fails. |
| `GLOSSA_SCREENSHOT_SERVICE` | desktop program | With `--screenshot options:services`: a provider id (`google-vision-ocr`) whose settings to show, in the first service group it belongs to. |
| `GLOSSA_SCREENSHOT_TAB` | desktop program | With `GLOSSA_SCREENSHOT_SERVICE`: `<tabs id>=<n>`, the tab (from 0) to show of a `tabs` item on that provider's settings page, as `pages=1`. |
| `GLOSSA_SCREENSHOT_ACTION` | desktop program | With `--screenshot panel:MODULE/PANEL`: an action of that panel to run once it is open. With `GLOSSA_SCREENSHOT_SERVICE`, or with `--screenshot module-editor` (its preview): the id of one of the module's [actions](modules/README.md#actions) to press when the page is first drawn, so the picture shows its result (or its spinner, if it takes longer than the window takes to settle). It runs for real, network included, so use a throwaway `GLOSSA_HOME`. A button greyed out by `needs` is not pressed. |
| `GLOSSA_FRAME_TRACE` | desktop program | Diagnosing stutter: a file path (or `1` for `frames.csv` in the cache folder) that gets one CSV line per frame of the main window, the popup and Options: when the frame started, the time since that window's previous frame, how long its UI took, the scroll events it consumed (both axes) and the scroll delta the window's scroll areas got. |
| `GLOSSA_RENDERER` | desktop program | Overrides `[appearance] renderer` for one run, with the same names `config.toml` takes: `open_gl` (or `glow`, `opengl`, `gl`), `gpu` (or `wgpu`), `software` (or `cpu`, `warp`). An unknown value is ignored. |
| `GLOSSA_PYTHON_SDK` | both programs | A folder holding `glossa_module.py`, searched first when starting Python modules. |
| `GTK_THEME` | desktop program, Linux | A name containing `dark` means the system prefers dark. |
| `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `DISPLAY`, `XDG_CURRENT_DESKTOP` | desktop program, Linux | How the session (Wayland or X11) and desktop are detected. |

Glossa sets `GLOSSA_MODULE_ID` and `GLOSSA_PROTOCOL` for the module
processes it starts (Python, Java and process modules). `GLOSSA_CRASH_TEST`,
`GLOSSA_ICON_OUT`, `GLOSSA_TEMPLATE_OUT`, `GLOSSA_OCR_SAMPLE`,
`GLOSSA_CLASSIC_THEMES` and `GLOSSA_QT_SERVICES` are development aids used
by tests.

## The desktop program's own flags

`glossa` (the desktop program) takes a few flags too:

| flag | does |
|---|---|
| `--hidden`, `--tray` | Start in the tray, whatever `[general] startup_action` says. Autostart uses this. |
| `--home DIR` | The data folder, as `GLOSSA_HOME`. |
| `--popup TEXT` | Show `TEXT` translated in the popup at start. |
| `--screenshot WHAT [FILE]` | Development aid: draw one window (`main`, `popup`, `options`, `options:PAGE`, `dictionary`, `history`, `keyboard`, `about`, `welcome`, `ocr-result`, `module-editor`, `module-editor:test`, `panel:MODULE/PANEL`) to a PNG and exit. `panel:` opens a loaded module's panel. `module-editor` opens the module editor on the folder in `GLOSSA_SCREENSHOT_MODULE`, or on a new module from the translator template; `module-editor:test` also runs its tests. `options:font` opens Options with the font list open; `options:autostart` shows the note given after turning on starting at login, without changing anything. It exits with 2 when the picture is blank, 3 when no picture came within 90 seconds of starting, 4 when a string has no text in any language and 6 for an unknown window. |
| `--screenshot-list` | Print every window `--screenshot` can draw, one per line, and exit (module panels, which depend on the data folder, are not listed). `scripts/screenshot-smoke.sh` renders them all. |
| `--version`, `-V` | Print the version (and the commit it was built from) and exit. |
| `--help`, `-h` | Print this list and exit. |

`--help`, `--version` and `--screenshot-list` answer on the terminal and
exit without opening a window or touching the data folder. A flag `glossa`
does not know, a stray word or a missing value is an error: it prints what
was wrong and exits with 64 instead of starting the program. (The arguments
macOS adds to an app's command line by itself, `-psn_…`, `-NS…` and
`-Apple…`, are passed over.) On Windows the release `glossa.exe` is a
windowed program: it prints to the terminal it was started from, and the
prompt may return before the text appears.

Starting `glossa` while it is already running shows the running program's
main window instead of starting a second copy (with `--popup TEXT`, the
running program shows `TEXT` in its popup). One copy runs per user, login
session and data folder, so copies with different `--home` folders run side
by side; the hand-over goes through a lock and a pipe (Windows) or socket
of the session's own, and through the control API only when the running
copy is too old to have them.
