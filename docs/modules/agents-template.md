# An AGENTS.md for your module

If you write a Glossa module with an AI coding assistant, put the text
below into your module's repository as `AGENTS.md` (Claude Code reads
`CLAUDE.md`: add one with the single line `@AGENTS.md`). It tells the
assistant what a module is, where the contract is written down, and how to
check its work with Glossa's test harness. Fill in the first section;
delete the runtimes you do not use.

Extra files in a module's folder, this one included, do not affect loading:
Glossa reads only `module.toml` and the files it names.

````markdown
# Agent notes for <module name>

This is a module for Glossa, a desktop translator
(https://github.com/bropines/glossa). Glossa loads it from a modules folder
and calls it with JSON requests; it is not a standalone program.

## This module

- id: `<id>` (in `module.toml`; lowercase letters, digits, `-`, `_`, `.`)
- runtime: `<js | python | java | process | native>`
- capabilities: `<translate, detect_language, dictionary, listen, ocr>`
- the service it talks to: <URL, docs, whether it needs a key>

## The contract

Read these before changing behaviour; do not guess field names.

- What a module is, `module.toml` and its fields, options, permissions:
  https://github.com/bropines/glossa/blob/main/docs/modules/README.md
- Requests, results, errors and host calls (protocol version 1, `api = 1`):
  https://github.com/bropines/glossa/blob/main/docs/modules/protocol.md
- Per runtime:
  - JavaScript: docs/modules/js-modules.md
  - Python: sdk/python/README.md (`from glossa_module import Module, ModuleError`)
  - Java and any program over stdio: docs/modules/process-modules.md
  - Native (Rust, C): docs/modules/native-modules.md
  - Text recognition results and the image API: docs/modules/ocr-modules.md
- Testing, the checklist for a module, and the fixture format:
  https://github.com/bropines/glossa/blob/main/docs/modules/testing.md

## Rules

- One function per capability: `translate`, `detect`, `dictionary`,
  `speak`, `recognize`. Return the shapes protocol.md gives; unknown fields
  are ignored, missing required ones are an error.
- Make HTTP requests through the host (`host.fetch` / `host.http` in
  JavaScript, `host.http` / `host.http_json` in Python). They use the
  user's proxy, timeouts and retries, and reach only the hosts listed in
  `[permissions] network`. Add every host the module calls there; prefer
  exact hosts to `"*"`.
- Fail with a typed error, not a crash: `missing_credential` with the
  option's key when a required setting is empty or rejected,
  `rate_limited`, `not_found`, `bad_response`, `http`, `network`. Glossa
  tells the user which setting to fill in, or moves on to the next service.
- Settings come in `req.options` (declared as `[[options]]` in
  `module.toml`). Never hard-code a key, token or password, and never
  commit one.
- Languages are BCP 47 codes (`en`, `zh-Hans`, `pt-BR`); `auto` means
  "detect". Map the service's own spellings in `[language_map]`, not in
  code.
- JavaScript runs in a fresh context per call: keep tokens in
  `host.cache`, not in globals. No DOM, no Node `require`, no browser
  `fetch`.
- Python, Java and process modules: stdout is the protocol channel. Never
  print anything but protocol frames to it (the Python SDK already sends
  `print()` to the log). Flush after each line. Log to stderr or with
  `host.log`.

- Every capability has a fixture case in `tests/`, and so does every error
  the module raises. Fixtures use obvious fakes for keys
  (`api_key = "test-key"`) and recorded answers for the network.

## Must and must not

You must:

- read the contract above before writing code, and start from the
  template for the runtime (listed in testing.md, *Templates*);
- run `glossa-cli module test --strict .` after every change and show its
  output before calling the work done;
- record real answers (`--record`) only with a key the user gave you for
  that purpose, passed with `--set-from-env` (never `--set`, which lands in
  the shell's history), and never write that key into a file.

You must not:

- put a key, token, cookie or password into any file of this repository,
  fixtures and recordings included;
- make a check pass by weakening it: deleting a failing case, loosening an
  expectation, adding `"*"` to the permissions, or returning an empty
  result instead of an error;
- change the manifest's `id` or `api` without being asked.

## Testing it

The harness loads the module the way Glossa does, runs the cases in
`tests/*.toml` against recorded HTTP answers (offline), and checks every
answer against the protocol:

```bash
glossa-cli module check .               # the manifest and files only
glossa-cli module test .                # load, run the fixtures, check the answers
glossa-cli module test --strict .       # warnings fail too; run this before a commit
glossa-cli -v module test --case "unknown word" .   # one case, with the module's log
MY_KEY=… glossa-cli module test --record --set-from-env api_key=MY_KEY .
                                        # write tests/recorded/ from the real service;
                                        # replays then prefer it over the [[case.http]] entries
glossa-cli --json module test .         # the report as JSON
```

A case:

```toml
# tests/basic.toml
[[case]]
name = "translates a greeting"
op = "translate"                 # translate | detect | dictionary | speak | ocr
text = "Hello"
from = "en"
to = "de"
options = { api_key = "test-key" }
expect = { text = "Hallo", detected = "en" }
[[case.http]]                    # what the service answers, matched by method and URL prefix
method = "POST"
url = "https://api.example.com/v1/translate"
body = '{"translation": "Hallo", "source": "en"}'
```

## Running it in Glossa

Use a throwaway data folder so the user's own settings stay untouched:

```bash
mkdir -p /tmp/gtest/modules
cp -r . /tmp/gtest/modules/<id>              # or a symlink to the working copy
glossa-cli --home /tmp/gtest modules         # loaded / failed (with the reason) / skipped
glossa-cli --home /tmp/gtest services        # the module's id, origin and capabilities
glossa-cli --home /tmp/gtest -v translate -s <id> -t de "Hello"   # -v shows the module's log
glossa-cli --home /tmp/gtest dict -s <id> -f en -t ru serendipity
glossa-cli --home /tmp/gtest ocr picture.png -e <id> -F json
```

For a speech module, set `[services] tts = "<id>"` in
`/tmp/gtest/config.toml`, then `glossa-cli --home /tmp/gtest speak -l en -o
out.wav "Hello"`. Settings for testing go into the same file under
`[services.options.<id>]`. On Windows, use a folder such as
`%TEMP%\gtest` instead of `/tmp/gtest`.

In the desktop app, *Options → Modules → Reload modules* picks up an edited
module without a restart.

## Typical mistakes

- `glossa-cli module test` reports an unknown key: a misspelt key, or one
  in the wrong table; Glossa would silently ignore it.
- `glossa-cli modules` lists the module under "failed": read the reason.
  Usual causes: a syntax error (file and line are given), a bad `id` or
  a value of the wrong type in `module.toml`, the entry file missing,
  Python or Java not found.
- The module is not listed at all: the folder is one level too deep or has
  no `module.toml`.
- "network access to … is not in the module's permissions": the host is
  missing from `[permissions] network`.
- A call that always times out (process modules): stdout is buffered, or
  something other than a frame was written to it.
- Results show up empty: the returned field names do not match
  protocol.md (`text`, not `translation`; `lang`, not `language`). The
  harness reports this as `bad_response … missing field`.
- "no recorded response for GET …" in a test: the case has no recorded
  answer for that request; add a `[[case.http]]` entry or run `--record`.
- The module is listed but not in the service strip: it has a required
  option or lacks `ready_to_use = true`; switch it on in *Options →
  Services*.
````

The module guides themselves are listed in [README.md](README.md).
