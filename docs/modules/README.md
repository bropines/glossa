# Modules

Everything Glossa translates, looks up, speaks or recognises comes from a
**provider**. Some are built in (Google, Bing, DeepL, Yandex, Lens OCR…); the
rest are **modules**: folders Glossa finds at start-up and loads without
being rebuilt.

![A module: Glossa sends a JSON request, the module may call back to the host (HTTP, log, cache, image), then answers with a JSON result](../images/module.svg)

Every kind of module speaks the same [module protocol](protocol.md): the
same JSON requests, the same results, the same host calls. To the engine
they are interchangeable, and a module can move from one runtime to another
without reshaping its data.

## Contents

- [Your first module](#your-first-module)
- [Which runtime?](#which-runtime)
- [Where modules live](#where-modules-live)
- [module.toml](#moduletoml)
- [Trying a module](#trying-a-module)
- [Permissions and trust](#permissions-and-trust)
- [Bundled modules](#bundled-modules)
- [Modules with downloads](#modules-with-downloads)
- [Scripts from other programs](#scripts-from-other-programs)
- [Guides](#guides)

## Your first module

A JavaScript module needs nothing installed. Make a folder `hello` in your
modules folder ([where](#where-modules-live)) with two files.

`module.toml`:

```toml
capabilities = ["translate"]

[module]
id = "hello"
name = "Hello"
runtime = "js"
```

`main.js`:

```js
function translate(req, host) {
  return req.text.toUpperCase();
}
```

Then check that it loaded and call it:

```bash
glossa-cli modules                         # "hello" is listed as loaded
glossa-cli translate -s hello -t ru "it works"
```

In the desktop program, *Options → Modules → Reload modules* picks the new
folder up, and *Options → Services* lists "Hello" to switch on. A real
module calls a web service through `host.fetch`; the
[JavaScript guide](js-modules.md) shows how.

## Which runtime?

| kind | you write | runs as | good for | guide |
|---|---|---|---|---|
| JavaScript | `main.js` | inside Glossa (QuickJS engine), a fresh context per call | web APIs: a few requests and some JSON | [js-modules.md](js-modules.md) |
| Python | `main.py` | a child process, started on first use, stopped when idle | anything needing Python libraries (ML models, `chrome-lens-py`, scraping) | [sdk/python](../../sdk/python/README.md) |
| Java | a `.jar` (Java, Kotlin, any JVM language) | a child process like Python, with Java found for you | JVM libraries and engines (Tess4J, ONNX Runtime for Java, Lucene), Kotlin code | [process-modules.md](process-modules.md#java-modules), [sdk/java](../../sdk/java/README.md) |
| Process | any program: `node main.mjs`, a Go or C# binary, a script | a child process like Python | any language with stdin and stdout; no SDK needed | [process-modules.md](process-modules.md) |
| Native | a `.dll` / `.so` / `.dylib` | loaded into Glossa | speed, offline engines, other languages with a C ABI (Rust, C, C++, Go, Zig) | [native-modules.md](native-modules.md) |

In short:

- A web API with JSON in and out: **JavaScript**. No process, no toolchain,
  nothing to install for the user.
- You need a Python library, or the code already exists in Python:
  **Python**. The process costs memory only while in use.
- The code is Java or Kotlin, or needs a JVM library: **Java**. Same process
  model as Python. The user needs a Java 17+ runtime, and Glossa says how to
  get one if it is missing.
- Any other language (Node, Go, C#, Ruby, a compiled binary): **process**.
  Read JSON lines on stdin, write them on stdout. No SDK, nothing linked
  into Glossa, and a crash only costs that one call.
- A local engine that must be as fast as possible, called from Rust, C, C++
  or Zig: **native**. No process boundary, but a crash in it takes Glossa
  down, so it is for code you trust.

| | JavaScript | Python | Java | process | native |
|---|---|---|---|---|---|
| user installs | nothing | nothing (Glossa downloads a Python when none is installed or the module has dependencies) | Java 17+ | the program's runtime | nothing |
| memory while idle | none | none (stopped) | none (stopped) | none (stopped) | the library |
| start-up | instant | tens of ms | a fraction of a second | the program's | instant |
| a crash takes down | the call | the call | the call | the call | Glossa |
| SDK | built in | `sdk/python` | `sdk/java` | none needed | `glossa-module-sdk`, `glossa_module.h` |

Python, Java and process modules share one transport (JSON lines over
stdio), so they behave identically.

Writing a text recognition module (a network OCR service, a local engine)?
[ocr-modules.md](ocr-modules.md) covers the result format (JSON layout, hOCR
or ALTO), the coordinate rules and the image API every runtime gets for
preprocessing.

## Where modules live

Glossa scans these folders, in order. A later one wins on a duplicate id.

1. `<folder of glossa.exe>/modules`
2. `<prefix>/share/glossa/modules` (a Unix install)
3. `<data folder>/modules`: `%APPDATA%\Glossa\modules` on Windows,
   `~/.config/glossa/modules` on Linux, `~/Library/Application Support/Glossa/modules`
   on macOS, or `data/modules` next to the executable in portable mode.

One module is one folder with a `module.toml`.

- *Options → Modules → Install module…* copies a folder or a `.potext` file
  into the last of these.
- *Reload modules* on the same page picks up changes without a restart.
- More folders can be added on the same page (`[modules] dirs` in
  `config.toml`).

```
modules/
├── free-dictionary/        a JavaScript module
│   ├── module.toml
│   └── main.js
├── lens-ocr/               a Python module
│   ├── module.toml
│   └── main.py
├── example-upper/          a Java module
│   ├── module.toml
│   └── module.jar
├── example-node/           a process module (Node.js)
│   ├── module.toml
│   └── main.mjs
└── shout/                  a native module
    ├── module.toml
    └── shout.dll
```

## module.toml

Only `id`, `name` and `runtime` are required. The full set:

```toml
capabilities = ["translate", "detect_language"]   # see below
languages = ["en", "ru", "de"]                    # omit for "all"

[module]
id = "my-translator"          # lowercase letters, digits, - _ . ; unique
name = "My Translator"        # shown in the UI
version = "1.0.0"
api = 1                       # module protocol major version
runtime = "js"                # js | python | java | process | native
entry = "main.js"             # main.js / main.py / module.jar / the library's file stem
description = "One line for the services list."
homepage = "https://example.com"
author = "You"
license = "MIT"
min_glossa = "0.1.0"          # optional: older Glossa builds refuse the module ("needs Glossa 0.1.0 or newer")
max_input_chars = 5000        # optional: longer text is cut at a word boundary
idle_timeout_secs = 300       # Python, Java, process: stop the process after this long unused
ready_to_use = true           # works with no key or setup: switched on when it first appears
# requirements = ["requests>=2.31"]  # python: packages, installed for this module alone (see below)
# python_index = "https://…/simple" # python: another package index than PyPI; its host in [permissions] network
# Python, Java and process modules only (see process-modules.md):
# command = ["node", "main.mjs"]   # process: the program; or command.windows / .macos / .linux / .unix / .default
# args = ["--quiet"]               # appended after the command, script or jar
# env = { KEY = "value" }          # environment for the process; ${MODULE_DIR} is the module's folder
# cwd = "."                        # working directory, relative to the module's folder
# main_class = "com.example.Main"  # java: run a class instead of `java -jar <entry>`
# classpath = ["classes", "lib/*"] # java: with main_class
# jvm_args = ["-Xmx64m"]           # java: JVM options

[language_map]                # optional: Glossa's code -> the service's
"zh-Hans" = "zh"
"he" = "iw"

[permissions]
network = ["api.example.com", "*.example.org"]   # "*" for any host; empty = no network

[[options]]                   # each becomes a field on the module's settings page
key = "api_key"
type = "secret"               # text | secret | url | bool | number | select | multiselect | multiline | lang | file
label = "API key"
help = "Get one at https://example.com/keys"
required = true

[[options]]
key = "formality"
type = "select"
label = "Formality"
default = "default"
choices = [{ value = "default", label = "Default" }, { value = "more", label = "Formal" }]

[[assets]]                    # files downloaded on demand: see Assets below
name = "model.onnx"
url = "https://example.com/model.onnx"
sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
size = 4718592
```

### Capabilities

They say which methods the module implements:

| capability | method | the module is offered as |
|---|---|---|
| `translate` | `translate` | a translator (main window, popup, service strip) |
| `detect_language` | `detect` | a language detector |
| `dictionary` | `dictionary` | a dictionary (dictionary window, single words) |
| `listen` | `speak` | text to speech |
| `ocr` | `recognize` | text recognition for screen regions ([guide](ocr-modules.md)) |

### Languages

Languages are BCP 47 codes: `en`, `ru`, `zh-Hans`, `zh-Hant`, `pt-BR`…
`glossa-cli services` and the languages list in the main window show the
full set. A source of `auto` means "detect it". If the service spells a
language differently, map it in `[language_map]` instead of in code.

### Permissions

Permissions limit the requests a module makes *through Glossa*
(`host.fetch` / `host.http`):

- A request to a host not on the list never leaves, and the module gets an
  error naming the permission.
- Those requests use the user's proxy settings and Glossa's timeouts and
  retries.
- A Python, Java, process or native module can open its own connections.
  Permissions do not cover those (they are ordinary programs), which is one
  reason to prefer the host calls.

### Switching on

Glossa keeps no fixed list of services. When a provider appears for the
first time (a built-in on the first run, your module the first time it
loads):

- it is switched on in the service strip if it declares
  `ready_to_use = true` and has no required options;
- otherwise it is only listed, for the user to switch on in
  *Options → Services*.

Either way the choice is remembered, so a service the user switched off
stays off.

### Options

Options are stored in the user's `config.toml` under
`[services.options.<module id>]`, and every request carries them as
`options`.

- When the user keeps secrets in the system keychain (`[general]
  secret_storage`), options of type `secret` are kept there instead. A
  module still receives the real value.
- A `required` option is marked with `*` on the settings page. When it is
  empty, fail with `missing_credential` and the option's key as the
  message, and Glossa tells the user which setting to fill in.

### Settings pages

The settings page is drawn from `[[options]]` alone, so a module never
ships UI code. Beyond `key`, `type`, `label`, `help`, `default`,
`required`, `choices`, `min`, `max` and `placeholder`, an option may say
more. Every key is optional; a module without them looks as before.

| key | for | meaning |
|---|---|---|
| `pattern` | text types | a regular expression the value must match; anchor it (`^…$`) to match the whole value |
| `pattern_message` | text types | shown under the field instead of the generic "does not match" |
| `min_length`, `max_length` | text types, `multiselect` | characters (for `multiselect`, how many are picked) |
| `step` | `number` | the drag or slider step |
| `widget = "slider"` | `number` with `min` and `max` | a slider instead of a number field |
| `allow_auto` | `lang` | offer "Detect language" (the value `auto`) |
| `filters` | `file` | extensions the file dialog offers: `["txt", "csv"]` |
| `visible_if` | any | a [condition](#conditions): the field is shown only while it holds |
| `enabled_if` | any | a condition: the field is greyed out unless it holds |
| `group` | any | put the field into the group with this id (declared in `[ui.settings]`, or made up on the spot and titled by its id) |

The types added for richer pages:

| `type` | value | control |
|---|---|---|
| `multiselect` | a list of strings from `choices` | checkboxes in a dropdown |
| `lang` | a language code (`de`, `zh-Hans`, `auto`) | Glossa's searchable language picker |
| `file` | a path | a text field and a *Browse…* button; the module gets the path, not the file |

A type this Glossa does not know (from a newer release) does not stop the
module: the field shows a "needs a newer version" line.

> **Older builds.** Glossa releases before these settings pages refuse to
> load a module that uses `multiselect`, `lang` or `file`: they do not know
> the types, and fail the whole manifest with a parse error. The other keys
> on this page are ignored there, which is harmless. Those releases do not
> know `min_glossa` either, so for them the only remedy is not to use the
> new types (a `select` or `text` works everywhere). Still set `min_glossa`
> to the release you need: every build from this one on honours it, and
> says "needs Glossa x.y.z or newer" instead of drawing a page it does not
> fully understand.

Patterns use ASCII classes: `\d`, `\w`, `[a-z]` and case-insensitive
matching cover ASCII only, while literal text in any script (`^ключ-`)
matches as written. A pattern is limited to 1,000 characters and a modest
compiled size. Conditions are limited to 1,000 characters and 32 levels of
`!` and parentheses. Layouts nest at most 16 containers deep, and each item
is drawn once even if it is listed twice.

#### Checks

Glossa checks values as the user types and shows a message under a field
that fails: `required` and empty, `pattern`, `min`/`max`,
`min_length`/`max_length`, and the type itself (a `url` must start with
`http://` or `https://`, a `number` must be a number, a `select` value one
of its `choices`, a `lang` value a language). Hidden and greyed-out fields
are not checked. *Apply* still saves what was typed, since the user may be
half-way through, and the services list marks the module "check the
settings". Checks only the service can make (is the key accepted?) stay
with the module: fail with `missing_credential` and the option's key.

#### Conditions

`visible_if` and `enabled_if` take a small expression, evaluated by Glossa
every frame with no call into the module:

```
options.api_key                         filled in (not empty, not false, not 0)
!options.api_key                        not filled in
options.tone == 'casual'                equal to a string, number or true/false
options.temperature > 0.5               < <= > >= compare numbers
options.mode == 'custom' && options.url
(options.a || options.b) && !options.c
```

A path is `options.<key>`; literals are `'text'`, `"text"`, numbers,
`true`, `false` and `null`. There are no function calls and no
arithmetic. Paths starting with `state.` are reserved for module panels and
read as empty on the settings page.

#### Layout

Without a layout, options are listed in order, and those with the same
`group` are gathered into one group box where the first of them stands.
`[ui.settings]` arranges the page:

```toml
[ui]
format = 1                       # the version of everything under [ui]

[ui.settings]
layout = ["pages"]               # the top level, in order

[[ui.settings.items]]
id = "pages"
type = "tabs"                    # each child group is one tab
children = ["connection", "advanced"]

[[ui.settings.items]]
id = "connection"
type = "group"                   # a titled box; collapsed = true folds it
label = "Connection"
children = ["note", "api_key", "size"]

[[ui.settings.items]]
id = "note"
type = "label"
style = "weak"                   # normal | weak | error | heading
label = "Create a key at example.com."

[[ui.settings.items]]
id = "size"
type = "row"                     # children side by side
children = ["enlarge", "min_height"]

[[ui.settings.items]]
id = "advanced"
type = "group"
label = "Advanced"
children = ["model", "timeout"]
```

`layout` and `children` list option keys and item ids. Item types are
`group` (with `collapsed`), `row`, `tabs` (children are groups), `label`
(with `style`) and `separator`; items also take `visible_if` and
`enabled_if`, which apply to everything inside. An option the layout does
not place is never lost: it is added at the end, or to its `group`. Older
Glossa builds ignore `[ui]` and list the options in order.
[`js/google-vision-ocr`](../../modules/js/google-vision-ocr) uses all of
this with tabs, a row, a slider, a multiselect and conditions.

#### Actions

Buttons that run module code: "Load models", "Test the key". Each
`[[actions]]` entry names a function in the module (its `handler`, or its
`id`), which gets the settings on screen and answers with what to show:

```toml
[[actions]]
id = "list_models"
label = "Load models"            # runs list_models(ev, host) in main.js

[[actions]]
id = "test"
label = "Test the key"
help = "Asks the service whether it accepts the key."
handler = "testKey"              # another function name
needs = ["api_key"]              # greyed out, naming the field, until it is filled in and passes its checks

[[options]]
key = "model"
type = "select"
choices = [{ value = "small" }]
choices_from = "list_models"     # the action's reply may replace the choices

[ui.settings]
layout = ["api_key", "model", "@test"]   # `@id` places an action's button
```

```js
function list_models(ev, host) {
  var r = host.fetch("https://api.example.com/models", { headers: { Authorization: ev.options.api_key } });
  return { choices: { model: r.json().map(function (m) { return { value: m.id, label: m.name }; }) } };
}

function testKey(ev, host) {
  var r = host.fetch("https://api.example.com/me", { headers: { Authorization: ev.options.api_key } });
  if (r.status === 401) return { errors: { api_key: host.t("bad_key") } };   // under the field
  return { notice: host.t("connected", r.json().plan), notice_level: "success" };
}
```

- A reply may set `options` (into the unsaved settings; the user applies
  them), `choices` (for `select` and `multiselect` options), `errors` (shown
  under fields until they are edited) and a `notice` beside the button with
  a `notice_level` (`info`, `success`, `warning`, `error`). All are
  optional; returning nothing is fine, and a plain string is the notice.
- A field in `needs` that is hidden (`visible_if`) or greyed out
  (`enabled_if`) does not hold the button back: the user could not fill
  it in, and the condition says the module does without it in that state.
  The page's own checks skip such fields for the same reason.
- A `choices_from` action that the layout does not place gets a small
  button beside its field. Until it has run, the declared `choices` show
  and any value is accepted, since loaded ones are not in the manifest;
  once a list is loaded, values are checked against that list. A saved
  value the list does not offer stays, shown as not in the list.
- A reply is checked before it is used: an `options` value of the wrong
  type for its option (text for a number, say) fails the action with an
  error, keys that are not options are ignored, choice values and labels
  over 1 KB and notices or field messages over 4 KB are cut.
- An action belongs to the settings it ran with: closing or reopening
  Options, or reloading the module, cancels it and drops its answer.
- Actions run on a worker: the page keeps responding, the button shows a
  spinner, and one action runs at a time. They use the module's network
  permissions like any other call.
- Python: `@mod.action("list_models")` on a function `(ev, host)`; Java:
  `@Action("list_models")` on a method `(Request, Host)`; native Rust: the
  `Module::action` method; any other program: the `action` method of the
  [protocol](protocol.md#action).
- `host.t(key, ...args)` reads `[strings]` from `locales/<lang>.toml` in the
  interface language, falling back to `locales/en.toml` key by key; `{0}`,
  `{1}`… are replaced by the arguments.
- `glossa-cli module action <id> <action> [--option KEY=VALUE]` runs one from
  the command line, and fixtures test them with `op = "action"`
  ([testing.md](testing.md#case-fields)).
- `call` and `builtin` actions (no code) run in module panels; see
  [panels.md](panels.md#actions-in-a-panel). From the command line they run
  with `--panel`.

### Panels

A module can have windows of its own, described in `module.toml` as
`[[ui.panels]]`: fields, buttons, lists, results (a translation, an
article, Markdown, a table), images and progress bars, opened from the tray,
the main menu, a hotkey, the control API or the CLI. Glossa draws them and
keeps their state; buttons run the module's actions, and a handler can
stream partial results. Everything is in [panels.md](panels.md); the
[Pocket Glossary](../../modules/js/pocket-glossary) example shows it all.

[`js/google-vision-ocr`](../../modules/js/google-vision-ocr) has a "Test the
key" button with `needs`, and "All languages" filling a multiselect with
`choices_from`.

#### Translations

The manifest's text is the English default. `locales/<lang>.toml` in the
module's folder translates it into the language of Glossa's interface, and
the page follows a switch of that language at once:

```toml
# locales/ru.toml
[module]
name = "Мой переводчик"
description = "…"

[options.api_key]
label = "Ключ API"
help = "…"
placeholder = "…"
pattern_message = "…"

[options.tone.choices]           # a choice's label, by its value
casual = "Разговорный"

[settings.items.connection]      # an item's label (and help)
label = "Подключение"

[actions.test]                   # an action's button (and help)
label = "Проверить ключ"

[strings]                        # host.t("bad_key") in code; English in locales/en.toml
bad_key = "Ключ не подошёл."
```

The file is named by the interface language's code (`ru`, `pt-BR`; a
regional file falls back to its base, `pt`). A missing key falls back to
the manifest. `glossa-cli module check` reports files that do not parse and
keys that name nothing.

### Assets

A module that needs files too big for its folder (an ML model, a
dictionary, a helper program) lists them as `[[assets]]`. Glossa downloads
them into its own data folder, checks each against its SHA-256 and gives
the module their paths.

```toml
[[assets]]
name = "det"                         # saved as <assets folder>/det (a folder when unpacked)
url = ["https://example.com/det.tar.gz", "https://mirror.example.org/det.tar.gz"]  # or one string
sha256 = "…64 hex digits of the downloaded file…"
size = 4718592                       # bytes of the downloaded file
unpack = "tar.gz"                    # none (default) | zip | tar.gz | gz

[[assets]]
name = "ocr-helper"
url = "https://example.com/ocr-helper-win-x64.exe"
sha256 = "…"
size = 1048576
os = ["windows"]                     # windows | macos | linux | unix; omit for all
arch = ["x86_64"]                    # x86_64 | aarch64 | …; omit for all
executable = true                    # mark it runnable (Unix)

[[assets]]
name = "ru-en"
url = "https://example.com/models/ru-en.bin.gz"
sha256 = "…"
size = 17825792
unpack = "gz"
optional = true                      # fetched only when the user picks it
```

- **Names** are letters, digits, `.`, `-` and `_`. Two entries may share a
  name when their `os` / `arch` never overlap (a binary per system); the
  one that applies here is used.
- **When.** Nothing is downloaded at start-up. The module is listed, but
  it is not loaded (no process, no interpreter, no library) until its
  required files are there. The user downloads them from the module's
  settings (*Options → Services → the module*), with `glossa-cli assets
  download <id>` or through the control API; a call to the module before
  that fails with "needs its files downloaded" and the settings page
  offers the download. With `[modules] auto_download_assets = true` the
  first call downloads them instead.
- **Optional** assets (one language pair, voice or recogniser of many) do
  not hold the module back. They are downloaded one by one, by name; the
  `assets` host call says which are still missing.
- **Integrity.** A download resumes a partial file (`Range` requests), is
  checked against `sha256` and `size` before it is put in place, and is
  deleted if it does not match. Afterwards, knowing a file is ready costs a
  file-system check, never a re-hash; *Check files* (or `glossa-cli assets
  verify`) hashes them again. An unpacked archive is checked by its files
  and their sizes, since the archive itself is not kept. A `sha256` of all
  zeros is taken for a placeholder: the manifest is refused, so fill in the
  real hash before publishing the module.
- **Where.** `<local data>/Glossa/models/<module id>/` (`data/models/` in a
  portable copy). `glossa-cli assets status <id>` prints the folder.
- **Network.** Every asset URL's host must be in `[permissions] network`
  (a manifest that downloads from elsewhere does not load), so the
  permissions list says everything the module may reach. Downloads use the
  user's proxy settings.
- **Limits.** An archive may unpack to at most 32 times its download size
  (at least 64 MiB, at most 64 GiB); one that unpacks further is refused
  and deleted. Writing and deleting stay inside the module's own folder.

The module finds its files

| runtime | how |
|---|---|
| any | the `assets` host call: `{"dir": …, "files": {"det": "/…/det"}, "missing": […]}`, only files verified for the current manifest ([protocol](protocol.md#assets)) |
| JavaScript | `host.assetPath("det")`, `host.assets()` |
| Python | `host.asset_path("det")`, `host.assets()` in the SDK |
| Java | `host.assetPath("det")`, `host.assets()` in the SDK |
| Python, Java, process | the environment: `GLOSSA_ASSETS_DIR`, and `GLOSSA_ASSET_DET` per required asset (upper case, other characters as `_`); optional ones through the host call |
| Python, Java, process | `${ASSETS_DIR}` in `command`, `args`, `env`, `cwd`, `classpath` and `jvm_args`, e.g. `command = ["${ASSETS_DIR}/ocr-helper"]` |
| native | the `assets` host call |

## Python dependencies

A Python module may use packages from PyPI. It names them, and Glossa
installs them for it; the user installs nothing, not even Python.

```toml
[module]
runtime = "python"
requirements = ["requests>=2.31", "idna==3.10"]   # package requirements
# python_index = "https://pypi.example.org/simple"  # optional: another index; its host must be in [permissions] network
```

Instead of `requirements`, a `requirements.txt` beside `module.toml`, or a
`pyproject.toml` with `[project] dependencies`, does the same (the first of
the three that names any package wins). Pin versions: the environment is
made again only when the list changes.

Each line names a package from the index, with optional extras, version
specifiers and environment markers (`requests[socks]>=2.31;
python_version >= "3.9"`). Glossa reads the file itself and refuses the
module, with the offending line, when a line is anything else: a pip
option (`-r`, `-c`, `-e`, `--index-url`, `--extra-index-url`,
`--find-links`, `--hash`…), a direct reference (`pkg @ https://…`,
`pkg @ file:///…`), a URL or a path. Only the checked lines reach `uv`, from
a file of Glossa's own; `[tool.uv.sources]` and the rest of a
`pyproject.toml` are ignored.

- **Which Python.** `[modules] python` ([configuration](../configuration.md#modules))
  is `auto` by default: a Python 3.8 or newer installed on the machine for
  a module without dependencies, and otherwise the *managed Python*, a
  [python-build-standalone](https://github.com/astral-sh/python-build-standalone)
  build of CPython 3.13 that Glossa downloads once (about 25 MB, 65 MB
  unpacked) and shares between modules. `system` and `managed` force one or
  the other; a path or command line names the interpreter.
- **The environment.** A module with dependencies runs in an environment
  of its own, `<local data>/Glossa/venvs/<module id>/`, made by
  [uv](https://github.com/astral-sh/uv) (downloaded once, about 15 MB) with
  `uv venv` and `uv pip install`. One module's packages never reach
  another's. A hash of the requirements, the interpreter and the `uv`
  version is kept in it; when any changes, the environment is made anew on
  the next install. `uv`'s download cache is in Glossa's cache folder.
- **When.** Nothing is downloaded at start-up. A call to a module whose
  Python or packages are missing fails with "needs Python …, packages …
  first", and its settings page offers *Download* with progress, as for
  [assets](#assets); `glossa-cli python install <id>` and
  [`/v1/python`](../control-api.md#v1python) do the same. With
  `[modules] auto_download_assets = true` the first call installs them.
- **Integrity.** The managed Python and `uv` are pinned in Glossa by
  version, URL and SHA-256 for each supported system (macOS arm64 and
  x86_64, Windows x86_64 and arm64, Linux x86_64 and aarch64), downloaded
  and unpacked by the same code as assets, with the same checks and limits.
  Packages come from one index: PyPI (`--index-url https://pypi.org/simple`)
  unless the module names another with `python_index`, an `https://` URL
  whose host must be in `[permissions] network`, as for asset downloads, so
  the permissions list says everything the module may fetch from. `uv` runs
  with no shell, with `--no-sources` and `--no-config`, and with a cleared
  environment: `UV_*` and `PIP_*` variables in the user's shell do not
  apply; the proxy is Glossa's own (*Options → Internet*). Packages are
  installed from wheels only (`--no-build`): building a source distribution
  would run its build script on the user's machine. A package (or a
  dependency of one) without a wheel for the user's system and the managed
  Python 3.13 fails to install, with uv's message.
- **Where.** `runtimes/` and `venvs/` beside `models/`
  ([data folders](../configuration.md)). `glossa-cli python status` lists
  them; `glossa-cli python remove` deletes them all.

[`modules/python/example-idna`](../../modules/python/example-idna) is a
complete example.

## Trying a module

```bash
glossa-cli modules                          # what loaded, what failed and why
glossa-cli services -c dictionary           # who offers what
glossa-cli translate -s my-translator -t ru "Hello"
glossa-cli dict -s free-dictionary -f en serendipity
glossa-cli -v translate -s my-translator -t de "Hi"   # with the module's log lines
```

To test a module on its own, with cases you write beside it and recorded
answers instead of the network:

```bash
glossa-cli module test modules/my-translator   # checks, loads and runs tests/*.toml
```

[testing.md](testing.md) has the fixture format, a checklist for a new
module and the templates to start from. In the desktop program, the
[module editor](editor.md) (*Options → Modules → Module editor*) does the
same from a window: it checks on every save, reloads only that module, and
runs the tests with F5.

`GLOSSA_HOME=<dir>` points Glossa at a separate data folder, handy for a
clean test with only your module in `<dir>/modules`. Each guide ends with
the debugging tips for its runtime.

*Options → Modules* shows the same list in the program, with what failed
and why:

<picture>
  <source media="(prefers-color-scheme: light)" srcset="../screenshots/options-modules-light.png">
  <img src="../screenshots/options-modules-dark.png" alt="Options, Modules: three modules loaded, none failed" width="700">
</picture>

## Permissions and trust

A module you install yourself does not run until you approve what it asks
for. Glossa reads that from `module.toml` alone, before any of the module's
code runs, and shows it in plain words at the top of *Options → Modules*
under *Waiting for your approval*:

- the hosts it may reach (`[permissions] network`), or "any website" for `*`;
- whether it runs sandboxed (JavaScript: no files, no programs), runs a
  program with your rights and so can read and change your files (Python,
  Java, `process`, with the command line), or is a native library that runs
  inside Glossa with full trust;
- each file it would download (`[[assets]]`), with its size and host;
- the Python packages it would install, and the index they come from
  (`requirements`, `requirements.txt` or `pyproject.toml`);
- its actions (`[[actions]]`);
- each [panel](panels.md) (`[[ui.panels]]`): a window of its own, and the
  tray or main-menu entry it adds. A module without panels is asked nothing
  about them, and its approval is unchanged.

*Approve* lets it run; *Decline* switches it off (switch it on again to be
asked again). Until then the module is listed with "needs approval", its
settings page can be looked at, nothing is downloaded or installed for it,
its panels, their tray and menu entries and their hotkeys do not appear,
and every call fails with "module … is waiting for your approval".

What you approved is kept in `config.toml` as `[modules.granted.<id>]`
([configuration.md](../configuration.md#modulesgrantedid)), with a hash of
the approved set. When an update changes `module.toml`:

- asking for **less or the same** (fewer hosts, a download removed) keeps it
  running;
- asking for **more** (a new host, a new or changed download, another
  program or command line, a new package or action) stops it until you
  approve again; the prompt says what is new.

Classic QTranslate scripts and pot-app plugins in your own folders are
asked about too, as "can reach any website", since they always had that.

An approval belongs to the module's id **and its folder**: another folder
whose `module.toml` claims an approved id waits for its own approval. A
module that waits never takes the place of a provider with the same id
that runs (a bundled service, or an approved module in another folder):
that one keeps working, and the newcomer is listed for approval only.

No approval is needed for:

- the modules compiled into Glossa;
- the folders an installer writes: `<folder of glossa.exe>/modules`,
  `/usr/share/glossa/modules` and `/usr/local/share/glossa/modules` on
  Linux, the app bundle's `Contents/Resources/modules` on macOS (and, in
  debug builds only, a development checkout's `modules/`). Other folders
  Glossa loads modules from, such as `<prefix>/share/glossa/modules` worked
  out from where the program sits, are asked about like your own;
- the `Service.js` scripts of a classic QTranslate installation you pointed
  Glossa at. A `module.toml` module in that folder is asked about;
- modules you create in the module editor, or copy there with *Copy and
  edit*, in this session: you are their author, and the editor's saves keep
  them approved as you change them. Saves also keep an approved module
  approved. A module that waits must be approved before *Edit* or *Copy and
  edit* is offered, its saves approve nothing, and the editor refuses to run
  its tests. A copy of a module that is itself still waiting is not
  approved by copying.

`glossa-cli module test DIR` loads a module with every permission and runs
it, approved or not: running it on a folder is the developer's explicit
choice.

**What is checked.** Trust covers what `module.toml` (and a Python module's
`requirements.txt` or `pyproject.toml`) declares, nothing more. The scripts,
programs and libraries themselves are not hashed: an update that changes
`main.js` or the executable but asks for nothing new keeps running. The
declared limits still hold for JavaScript (no files, only the approved
hosts); a program or library can do anything you can, which is why the
prompt says so.

**Upgrading.** The first time a version with approvals runs, it grants every
module already installed what it declares at that moment, and records
`[modules] trust_initialised = true`, so nothing that worked stops working.
Only modules added or widened afterwards ask. When a broken `config.toml`
is set aside and the defaults are used, this migration does not run: the
approvals went with the old file, and every hand-installed module asks
again, so nothing that was waiting or declined is approved by accident.

From the command line, `glossa-cli module pending` lists what waits and
what each asks for, and `glossa-cli module approve|revoke|decline ID`
approves, withdraws or declines ([cli.md](../cli.md#module)). The control
API has the same under [`/v1/trust`](../control-api.md#v1trust).

Approval is about what a module may do, not who wrote it: Glossa does not
sign or verify modules.

## Bundled modules

Some services ship inside Glossa as ordinary JavaScript modules. Their
sources live in [`modules/bundled/<id>/`](../../modules/bundled) in the
repository and are compiled into the executable (`include_str!` in
`crates/glossa-engine/src/bundled/`), so they need no files on disk. Each
`main.js` opens with a description of the service's protocol. All of them
work without a key or account and are switched on when they first appear.

| id | name | does | service |
|---|---|---|---|
| `yandex-web` | Yandex | translate, detect, dictionary | translate.yandex.net, the endpoint Yandex's browser clients use |
| `deepl-web` | DeepL | translate | the free endpoint of DeepL's iOS app (rate-limited after a burst) |
| `lara` | Lara | translate | laratranslate.com's web API |
| `modernmt` | ModernMT | translate | the form behind modernmt.com/translate |
| `multitran` | Multitran | dictionary | multitran.com result pages |
| `lingvo-live` | ABBYY Lingvo Live | dictionary | Lingvo Live's word-card JSON |
| `oxford-learners` | Oxford Learner's Dictionary | dictionary (English) | oxfordlearnersdictionaries.com entry pages |
| `wiktionary` | Wiktionary | dictionary | English Wiktionary's REST definitions |
| `youdao` | Youdao | dictionary (English, French, Chinese) | dict.youdao.com's JSON |
| `dic-academic` | Academic | dictionary | dic.academic.ru search |
| `translate-academic` | Academic Translations | dictionary | translate.academic.ru entries, as HTML |
| `wikipedia` | Wikipedia | dictionary | the Wikimedia REST page summary |
| `urban-dictionary` | Urban Dictionary | dictionary (English) | api.urbandictionary.com |

The keyed APIs stay Rust built-ins with names that say so: "Yandex Cloud (API
key)" (`yandex`) and "DeepL API (key)" (`deepl`).

**Changing one.** Bundled modules are registered before any module folder is
scanned, and a module with the same id replaces an earlier one.

1. Copy its folder from `modules/bundled/` into your modules folder.
2. Keep the `id`, edit, and *Reload modules*. `glossa-cli services` then
   shows it as `[JavaScript]` instead of `[Bundled]`.
3. Delete the copy to go back.

To hide one without replacing it, switch it off in *Options → Modules*
(`[modules] disabled` in `config.toml`).

## Modules with downloads

Larger modules live in `modules/` as source and fetch what they need as
[assets](#assets) once copied into a modules folder:

| id | name | does | downloads |
|---|---|---|---|
| `paddle-ocr` | PaddleOCR (offline) | text recognition on this machine: Chinese, Japanese, Korean, Cyrillic, Latin scripts, with line outlines ([README](../../modules/process/paddle-ocr/README.md)) | a Rust program with ONNX Runtime (9–14 MB), PP-OCRv5 models (6 MB, then 8–17 MB per script) |
| `local-llm` | Local LLM (offline) | translation by a small language model on this machine, through llama.cpp: Hy-MT2 1.8B (33 languages) or Qwen3 1.7B ([README](../../modules/process/local-llm/README.md)) | a Rust program (0.35 MB), llama.cpp (13–25 MB), one model (1.1–1.3 GB) |

## Scripts from other programs

Glossa also loads, without changes, scripts written for other programs:
classic **QTranslate** `Service.js` services and **pot-app** plugins
(`info.json` + `main.js`, or a `.potext` archive). See
[classic-scripts.md](classic-scripts.md).

## Guides

| guide | for |
|---|---|
| [js-modules.md](js-modules.md) | JavaScript modules |
| [sdk/python](../../sdk/python/README.md) | Python modules |
| [process-modules.md](process-modules.md) | Java, Kotlin, Node, Go, C# and any program over stdio |
| [sdk/java](../../sdk/java/README.md) | the Java SDK |
| [native-modules.md](native-modules.md) | shared libraries in Rust, C, C++, Zig |
| [ocr-modules.md](ocr-modules.md) | text recognition modules and the image API |
| [panels.md](panels.md) | windows of a module's own: elements, state, actions, streaming |
| [protocol.md](protocol.md) | the requests, results, errors and host calls every module uses |
| [testing.md](testing.md) | testing a module with `glossa-cli module test`, a checklist, templates, rules for AI agents |
| [editor.md](editor.md) | the module editor inside Glossa: templates, check, reload and test on every save |
| [classic-scripts.md](classic-scripts.md) | classic QTranslate services and pot-app plugins |
| [agents-template.md](agents-template.md) | an `AGENTS.md` to copy into your module's repository for an AI coding assistant |
