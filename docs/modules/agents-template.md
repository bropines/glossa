# An AGENTS.md for your module

If you write a Glossa module with an AI coding assistant, put the text
below into your module's repository as `AGENTS.md` (Claude Code reads
`CLAUDE.md`: add one with the single line `@AGENTS.md`). It tells the
assistant what a module is, where the contract is written down, and how to
check its work in Glossa. Fill in the first section; delete the runtimes
you do not use.

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
  protocol.md (`text`, not `translation`; `lang`, not `language`).
- The module is listed but not in the service strip: it has a required
  option or lacks `ready_to_use = true`; switch it on in *Options →
  Services*.
````

The module guides themselves are listed in [README.md](README.md).
