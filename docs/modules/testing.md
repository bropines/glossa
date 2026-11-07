# Testing a module

`glossa-cli module test` checks a module folder, loads it the way the
program does, runs the cases written beside it, and checks every answer
against the [module protocol](protocol.md). Recorded HTTP answers stand in
for the service, so the cases run offline, in CI and on a train.

This page has four parts: a checklist for a new module, the harness and
its fixture format, debugging with logs, and the rules for an AI agent
writing a module.

## Contents

- [Checklist for a new module](#checklist-for-a-new-module)
- [Running the harness](#running-the-harness)
- [Fixtures](#fixtures)
- [Recorded HTTP](#recorded-http)
- [What is checked](#what-is-checked)
- [Templates](#templates)
- [Debugging with logs](#debugging-with-logs)
- [Rules for AI agents](#rules-for-ai-agents)

## Checklist for a new module

Go through this before you publish a module. `glossa-cli module check`
finds most of it for you; the rest needs a moment's thought.

**Manifest**

- `id` is lowercase letters, digits, `-`, `_` or `.`, and unique. Do not
  reuse a built-in or bundled id unless you mean to replace that service.
- `name` and `description` are written for the user: the services list
  shows them.
- `version` is set, so users can tell releases apart.
- `api = 1`. A different number is refused, because it would mean a
  different protocol.
- No key the documentation does not list. Glossa ignores unknown keys, so a
  misspelt `entyr` silently does nothing; the harness reports it.
- Settings-page keys check out: conditions parse and name options that
  exist, patterns compile, text defaults pass their own checks, every id in
  `[ui.settings]` names an option or an item, and `locales/*.toml` parse
  with keys that name something. The harness reports each.

**Capabilities**

- List exactly the methods you implement: `translate`, `detect_language`,
  `dictionary`, `listen` (the `speak` method), `ocr` (the `recognize`
  method). A capability without its method fails every call the program
  makes; a method without its capability is never called.
- Give every capability at least one fixture case.

**Languages**

- `languages` lists BCP 47 codes Glossa knows (`en`, `zh-Hans`, `pt-BR`),
  or is left out for "any".
- When the service spells a code differently, map it in `[language_map]`
  (Glossa's code on the left, the service's on the right) instead of in
  code, so `detected_from` comes back mapped too.
- An unsupported pair fails with `source_language` or `target_language`,
  so the program can say so and try another service.

**Errors**

- Fail with a typed error, never with an empty answer. An empty
  translation, article or recognition result is a bug: the user sees an
  empty window and the program does not move on to the next service.
- Use the kinds in [protocol.md](protocol.md#errors): `missing_credential`
  (with the option's key as the message) when a required setting is empty
  or rejected, `rate_limited`, `not_found` for a word without an entry or
  a picture without text, `http`, `network`, `bad_response` when the
  service answered with something you cannot read.

**Timeouts**

- Every call has a budget (`[modules] call_timeout`, 30 seconds by
  default). A slow service should pass a `timeout` to `host.fetch` /
  `host.http` below that, so the module can fail with a clear error
  instead of being abandoned.
- Process modules: flush stdout after every frame. A buffered reply looks
  like a call that never finished.

**Secrets**

- Never put a key, token or password in the module, its tests or its
  recordings, not even your own "for testing". Declare an option with
  `type = "secret"`; the user fills it in, and it is kept in the system
  keychain when they choose that.
- Fixtures use obvious fakes (`api_key = "test-key"`). The harness scans
  every file for things that look like keys and fails the module.
- For a live run, pass a real key from an environment variable
  (`--set-from-env api_key=MY_SERVICE_KEY`), not with `--set` on the command
  line, where it lands in the shell's history.
- `--record` cuts every value given with `--set` or `--set-from-env` out of
  what it writes, but read a recording before committing it all the same: a
  service can echo a key or personal data back in its answer.

**Network**

- Make HTTP requests through the host (`host.fetch` in JavaScript,
  `host.http` in Python and the SDKs). They use the user's proxy, timeouts
  and retries, can be recorded, and reach only the hosts in `[permissions]
  network`.
- List exact hosts. `"*"` is reported as a warning: it lets the module
  reach anything.

**Size and memory**

- Keep the folder small; download models or data on first use instead of
  shipping them. The harness warns above 20 MB.
- Set `max_input_chars` to what the service accepts, so Glossa cuts long
  text at a word boundary instead of sending what the service refuses.
- Process modules: set `idle_timeout_secs` so an idle interpreter does not
  sit in memory. Load heavy libraries inside the call that needs them, not
  at start-up.

## Running the harness

```bash
glossa-cli module check modules/my-module        # the manifest and files only; runs nothing
glossa-cli module test modules/my-module         # load, run the fixtures offline, check the answers
glossa-cli module test modules/                  # every module below a folder
glossa-cli module test --case "unknown word" .   # only the cases whose name contains this
glossa-cli --json module test .                  # the report as JSON, for scripts and agents
```

| flag | meaning |
|---|---|
| `--live` | Call the real service instead of the recordings. |
| `--record` | Call the real service and write what it answered to `tests/recorded/` (see [below](#recorded-http)). |
| `--set-from-env KEY=VAR` | An option value from the environment variable `VAR`, over the fixtures' own: the way to pass a real key for a live run. Repeatable. |
| `--set KEY=VALUE` | An option value over the fixtures' own. Not for keys: the command line ends up in the shell's history. Repeatable. |
| `--ignore-recordings` | Replay only the hand-written `[[case.http]]` entries, ignoring `tests/recorded/`. |
| `--timeout SECS` | The budget for one call (default 30). |
| `--strict` | Warnings fail the module too. CI runs with it. |
| `--require-runtimes` | A module that cannot run here (no Python, no Node, no JDK, a native library not built, its [files](README.md#assets) not downloaded) fails instead of being skipped. |
| `--download` | Download a module's missing required [files](README.md#assets) before running it, instead of skipping it. Uses the network even without `--live`. |

The report gives one line per module (PASS, FAIL or SKIP, the runtime, the
time it took to load, the folder's size), then the findings and one line
per case with its time, the HTTP requests it made and what came back:

```
PASS free-dictionary (JavaScript) modules/js/free-dictionary  load 5 ms  5 KB
    note: module.toml: no `max_input_chars`; say how much text the service takes, …
    ok   tests/basic.toml: looks up a word [dictionary] 51 ms, 1 http -> "hello", 2 sense groups
    ok   tests/basic.toml: an unknown word is not found [dictionary] 20 ms, 1 http -> error module: …
    ok   tests/basic.toml: only English is offered [dictionary] 18 ms -> error source_language: …
```

Findings come in three strengths. An **error** fails the module. A
**warning** fails it under `--strict`: a capability without a case, no
fixtures at all, `network = ["*"]`. A **note** is advice and never fails
anything.

The exit code is 1 when any module failed. A skipped module (its runtime
is missing here) does not fail the run unless `--require-runtimes` is
given.

A module with [assets](README.md#assets) runs only when its required files
are downloaded: the harness looks for them where the program keeps them
(`glossa-cli module assets status <id>` shows the folder). When they are
missing, the module is skipped with the command that fetches them
(`glossa-cli module assets download <id>`), or `--download` fetches them
first. Until then only the manifest and the entry file are checked: a
`command` naming `${ASSETS_DIR}` cannot be resolved before the files exist.

The harness does not read `config.toml`: a module is tested on its own,
with the options its fixtures give. The data folder is used only to find
downloaded module files.

## Fixtures

Fixtures are TOML (or JSON with the same structure) files in the module's
`tests/` folder. Each holds one or more cases:

```toml
# tests/basic.toml
options = { max_definitions = 1 }      # for every case in the file; fakes only

[[case]]
name = "looks up a word"
op = "dictionary"                      # translate | detect | dictionary | speak | ocr
text = "Hello"
from = "en"                            # default "auto"
to = "ru"                              # default "en"
[case.expect]
headword = "hello"
contains = ["greeting", "/həˈləʊ/"]
[[case.http]]
url = "https://api.dictionaryapi.dev/api/v2/entries/en/hello"
body_file = "responses/hello.json"

[[case]]
name = "an unknown word is not found"
op = "dictionary"
text = "qwzxv"
from = "en"
expect = { error = "not_found" }
[[case.http]]
url = "https://api.dictionaryapi.dev/api/v2/entries/en/qwzxv"
status = 404
body = '{"title": "No Definitions Found"}'
```

Every table refuses keys it does not know, so a misspelt expectation fails
the fixture instead of passing it by checking nothing.

### Case fields

| field | for | meaning |
|---|---|---|
| `name` | all | Shown in the report; also names the recording. |
| `op` | all | `translate`, `detect`, `dictionary`, `speak` or `ocr`. The manifest must declare the matching capability. |
| `text` | all but `ocr` | The input. |
| `from`, `to` | `translate`, `dictionary` | Language codes; `from` defaults to `auto`, `to` to `en`. |
| `lang` | `speak`, `ocr` | The language; `en` for speech and `auto` for recognition by default. |
| `slow` | `speak` | The slower second reading. |
| `exact` | `dictionary` | Exact headword match (default true). |
| `image` | `ocr` | A picture file relative to the fixture (`"pictures/menu.png"`), or one the harness draws: `{ width = 320, height = 80, lines = 2 }` gives dark bars on white, one per line. |
| `granularity` | `ocr` | `full_text`, `blocks`, `lines` or `words` (default `words`, the most structure to check). |
| `translate_to` | `ocr` | Ask for a translation in the same call. |
| `options` | all | Option values for this case, over the file's. |
| `http` | all | Recorded answers for this case, tried before the file's. |
| `live` | all | The case needs the real service (the module does its own networking, so nothing can be recorded): skipped unless `--live` or `--record` is given. |

### Expectations

All are optional; the [shape checks](#what-is-checked) run regardless.

| field | meaning |
|---|---|
| `text` | The whole text, trimmed: the translation, the recognised text, or the detected language's code. |
| `contains` | One substring or a list. For a dictionary article, its plain-text rendering and its HTML. |
| `detected` | The detected language (translate, detect, ocr). |
| `headword` | The article's headword. |
| `mime` | The audio's MIME type. |
| `error` | The call must fail with this kind: `missing_credential`, `unsupported`, `source_language`, `target_language`, `rate_limited`, `network`, `http`, `bad_response`, `not_found`, `timeout`, `assets_missing` (the module's files are not downloaded), `runtime_missing` (a Python module's Python or packages are not there) or `module` (any other module error). A dictionary's `not_found` reaches the program as a plain module error carrying your message, the same as a crash would, so a `not_found` expectation that meets a module error also needs `error_contains` with words from your not_found message; without it the case fails. |
| `error_contains` | The error message must contain this. |
| `max_ms` | The call must finish within this many milliseconds. Leave room: the first call to a process module includes starting it. |
| `result` | A partial match on the result as JSON: every key given must be there with an equal value, and arrays match element by element for as many elements as are given. The names are the program's serialised ones (`trait` and `explains` in an article's explanations, `bounds` for an OCR box); when a key is missing, the report lists the ones that are there. |

## Recorded HTTP

A `[[case.http]]` entry (or a file-level `[[http]]`) is one answer:

```toml
[[case.http]]
method = "POST"                      # default GET
url = "https://api.example.com/v1/translate"   # matches any URL starting with this
status = 200                         # default 200
body = '{"text": "Hallo"}'           # or body_file = "responses/x.json", or body_base64 = "…"
content_type = "application/json"    # guessed from the body when left out
```

Each request is matched by method and URL prefix against the case's
recording first (if `--record` has written one), then the case's entries,
then the file's, in order. A recording is the service's newer answer, so
it wins over a hand-written entry for the same URL;
`--ignore-recordings` pins the hand-written entries instead. An entry
answers once; when every match has been used, the last one keeps
answering, so a module that asks twice for the same thing needs one entry.
A request that matches nothing fails with "no recorded response for …",
naming it, which is also how you find out what a module calls.

**Recording.** `glossa-cli module test --record --set-from-env api_key=MY_KEY .`
runs the cases against the real service and writes what came back to
`tests/recorded/<fixture file>/<case name>.json`. Later replays use those
files before the fixture's own entries. Every value given with `--set` or
`--set-from-env` is cut out of URLs (everything from the value on, so the
rest still matches as a prefix) and replaced in bodies. Read the files
before committing them; trim big pages to what the parser needs.

Only requests made through the host can be recorded. A Python, Java,
process or native module that opens its own connections cannot be tested
offline; mark such cases `live = true`.

Permissions still apply: a request to a host the manifest does not list
fails before it reaches the recordings, exactly as in the program.

## What is checked

**Without running anything** (`module check` and `module test`):

- `module.toml` parses, passes the program's own validation, and has no
  unknown keys;
- language codes in `languages` and `[language_map]` are ones Glossa knows;
- options: unique keys; no default on a `secret` option; a `select` has
  choices and its default is one of them; number defaults within min/max;
  credentials (`api_key`, `token`…) typed as `secret`;
- `[permissions] network` holds host names, not URLs, and not `"*"`;
- the entry exists (`main.js`, `main.py`), and what the runtime needs is
  here: Python, the process command, Java and the jar, the native library;
- every case's operation is a declared capability, and every declared
  capability has a case;
- no file looks like it holds a key, token or private key;
- the folder's size.

**For each answer**, besides the case's expectations:

- the reply matches the protocol's wire types (a missing or mistyped field
  fails as `bad_response`, naming it);
- a translation is not empty; a detected language is a language, with a
  confidence between 0 and 1;
- an article is not empty and has a headword;
- audio is not empty, and its bytes are what the MIME type says (MP3, WAV,
  Ogg, FLAC);
- recognised text is not empty; every box and polygon point lies within
  the picture (a pixel or two of slack), confidences are between 0 and 1,
  and the reply's `image_size`, if any, is the input's (send a `transform`
  when you scaled the picture, as [ocr-modules.md](ocr-modules.md)
  explains);
- an error is the expected kind, with the expected message.

**Timing:** how long loading took and every call, the first call of a
process module including its start.

## Templates

Each runtime has a small example in the repository that passes its own
fixtures. Copy the one for your runtime, change the `id`, and keep the
`tests/` folder growing with the module:

| runtime | template | shows |
|---|---|---|
| JavaScript | [`modules/js/free-dictionary`](../../modules/js/free-dictionary) | `host.fetch`, `host.cache`, typed errors, recorded answers |
| JavaScript (OCR) | [`modules/js/google-vision-ocr`](../../modules/js/google-vision-ocr) | the image API, a layout result, a secret option with a fake in tests |
| Python | [`modules/python/example-reverse`](../../modules/python/example-reverse) | the smallest SDK module |
| Python (dependencies) | [`modules/python/example-idna`](../../modules/python/example-idna) | `requirements` installed by Glossa; test with `--download` |
| Python (OCR) | [`modules/python/ocr-space`](../../modules/python/ocr-space) | `host.http`, `map_box_back`, error mapping |
| Java | [`modules/java/example-upper`](../../modules/java/example-upper) | the Java SDK; build `module.jar` with `build.sh` first |
| process | [`modules/process/example-node`](../../modules/process/example-node) | the protocol by hand, host calls, no SDK |
| native (Rust) | [`modules/native/example-rot13`](../../modules/native/example-rot13) | `export_module!`; copy the built library beside `module.toml` first |
| native (C) | [`modules/native/example-c`](../../modules/native/example-c) | the C ABI; compile `shout.c` first |
| native (OCR) | [`modules/native/example-lines`](../../modules/native/example-lines) | the image API from Rust, a drawn test picture |

The Java and native templates are tested once built; until then the
harness reports them as skipped and says what is missing.

## Debugging with logs

- `glossa-cli -v module test .` shows debug logs on stderr: each module
  call with its duration, process start and stop, host calls, and what the
  module itself logs (`host.log` in JavaScript and Python, stderr of a
  process module, `host.log` in the native SDK).
- `GLOSSA_LOG=glossa_modules=trace` narrows the log to the module
  runtimes and adds more detail; `GLOSSA_LOG` takes the usual
  `target=level` list.
- A process module's stderr goes to the log, so `print()` in Python (the
  SDK moves it to stderr) and `console.error` in Node land there. Never
  write anything but protocol frames to stdout.
- The report names every HTTP request a case made, without the query
  string. A request that is not recorded fails with its method and URL,
  which tells you what to record.
- In the desktop program, the same logs go to `glossa.log` in the cache
  folder; *Options → Modules* lists modules that failed to load, with the
  reason.

Typical failures and what they mean:

| report says | usually |
|---|---|
| `unknown key `module.entyr`` | a misspelt key, or a key in the wrong table (`languages` under `[module]`) |
| `the module does not load: …` | a syntax error (file and line given), a missing entry file |
| `bad_response: … reply: missing field `text`` | the reply's field names do not match [protocol.md](protocol.md) |
| `timeout` on a process module | stdout not flushed, or something other than a frame written to it |
| `no recorded response for GET …` | the case has no answer for that request: add one, or `--record` |
| `network access to … is not in the module's permissions` | the host is missing from `[permissions] network` |
| `the translation is empty` | the module returned `""` where it should have thrown an error |

## Rules for AI agents

An AI agent writing or changing a module must:

- read [README.md](README.md) and [protocol.md](protocol.md) before writing
  code, and the guide for the runtime, instead of guessing field names;
- start from the [template](#templates) for its runtime;
- write fixtures as it writes the code: one passing case per capability,
  and a case for each error the module raises (missing key, unknown word,
  unsupported language, rate limit);
- record real answers with `--record` only when the user has given it a
  key for that purpose, passed through an environment variable
  (`--set-from-env`), and never write the key into any file; otherwise build the recorded answers from the service's
  documented examples;
- run `glossa-cli module test --strict .` and show its output before saying
  the work is done; a module that only "should work" is not done;
- keep the module's network permissions to the service's own hosts.

It must not:

- put a key, token, cookie or password into the module, its fixtures or
  its recordings, including one found in the user's files or in a classic
  `Service.js`;
- weaken a check to make it pass: delete a failing case, loosen an
  expectation until it means nothing, add `"*"` to the permissions, or
  return an empty result to avoid an error;
- change Glossa itself (the protocol, the SDKs, the harness) to suit one
  module; report what is missing instead;
- copy code, strings or assets from another program's module or plugin
  unless its licence allows it.

[agents-template.md](agents-template.md) is an `AGENTS.md` to put into a
module's own repository; it carries these rules.
