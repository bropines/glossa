# The Glossa module protocol

Version 1 (`api = 1` in `module.toml`). This is the contract between the
engine and a module, whatever the module is written in. The runtimes only
differ in how the messages travel:

| runtime | a request arrives as | a host call is |
|---|---|---|
| JavaScript | `translate(req, host)` etc. | `host.fetch(…)`, `host.http(…)`, `host.cache.get(…)`, `host.image.crop(…)` (synchronous) |
| Python, Java, process | a JSON line on stdin | a JSON line on stdout, answered on stdin (the SDKs hide both; framing in [process-modules.md](process-modules.md#framing)) |
| Native | `call(method, params_json)` through the C ABI | `host->call(ctx, method, params_json)` |

Everything is JSON, UTF-8. Unknown fields are ignored in both directions, so
adding a field is not a breaking change; removing or retyping one is, and
bumps `api`.

## Contents

- [Conventions](#conventions)
- [Methods](#methods): [`translate`](#translate), [`detect`](#detect),
  [`dictionary`](#dictionary), [`speak`](#speak), [`recognize`](#recognize),
  [`action`](#action)
- [Errors](#errors)
- [Host calls](#host-calls): [`http`](#http), [`log`](#log),
  [`cache_get` / `cache_set`](#cache_get--cache_set), [`assets`](#assets),
  [`image`](#image), [`t`](#t), [`ui_update`](#ui_update)
- [Versioning](#versioning)

## Conventions

- **Languages** are BCP 47 codes: `en`, `ru`, `zh-Hans`, `zh-Hant`, `pt-BR`,
  `sr-Latn`… A source of `auto` means "unknown, detect it". Codes pass
  through the module's `[language_map]` first, so a module sees its service's
  spelling if it declared one.
- **Options**: every request carries `options`, the module's settings with
  defaults filled in (see `[[options]]` in `module.toml`).
- **Binary data** (images, audio) travels base64-encoded, in fields ending in
  `_base64`.
- **Optional fields** may be missing or `null`; empty lists may be missing.

## Methods

### `translate`

```json
{ "text": "Hello, world", "from": "auto", "to": "ru",
  "style": "formal",
  "glossary": [{ "source": "world", "target": "мир" }],
  "options": { "api_key": "…" } }
```

`style` and `glossary` are hints a service may ignore. The result:

```json
{ "text": "Привет, мир",
  "detected_from": "en",
  "romanization": "Privet, mir",
  "alternatives": ["Здравствуй, мир"],
  "parts_of_speech": [
    { "part_of_speech": "noun",
      "senses": [{ "word": "мир", "back_translations": ["world", "peace"] }] } ] }
```

Only `text` is required. JavaScript and Python may return a bare string.
`detected_from` matters when `from` was `auto`: the UI shows it and uses it
for "smart" direction switching. `parts_of_speech` is the dictionary block
some services return for single words (Google does); the popup shows it under
the translation.

### `detect`

```json
{ "text": "Bonjour", "options": {} }
```
→
```json
{ "lang": "fr", "confidence": 0.98 }
```

`confidence` is 0–1 and defaults to 1. Python may return just the code.

### `dictionary`

```json
{ "text": "serendipity", "from": "en", "to": "ru", "exact": false, "options": {} }
```

`exact` asks for the headword only, no fuzzy matches. The result is an
article:

```json
{ "headword": "serendipity",
  "pronunciations": [{ "region": "UK", "symbol": "/ˌsɛɹ.ənˈdɪp.ɪ.ti/", "audio_url": "https://…" }],
  "explanations": [{ "trait": "noun", "explains": ["A happy accident.", "…"] }],
  "examples": [{ "source": "It was pure serendipity.", "target": "Это была чистая случайность." }],
  "associations": ["chance", "fluke"],
  "html": null,
  "link": "https://en.wiktionary.org/wiki/serendipity" }
```

`trait` / `explains` are pot-app's names, so pot dictionary code ports as is;
`part_of_speech` / `definitions` are accepted as well. `examples` may also be
called `sentence`. A service that returns ready HTML can put it in `html`;
Glossa simplifies it to its own styling.

### `speak`

```json
{ "text": "Hello", "lang": "en", "slow": false, "options": {} }
```
→
```json
{ "audio_base64": "SUQzBAAAAAAA…", "mime": "audio/mpeg" }
```

`audio/mpeg`, `audio/wav` and `audio/ogg` play everywhere. Python may return
`(bytes, mime)`.

### `recognize`

```json
{ "image_base64": "iVBORw0KGgo…", "image_handle": 1, "lang": "auto", "translate_to": "ru",
  "granularity": "words", "preserve_line_breaks": true, "options": {} }
```

The image is PNG, JPEG or WebP. `image_handle` is the same image registered
with the [image API](#image) for this call. `translate_to` is set when the
user asked to recognise *and* translate; a module whose service can do both
in one go fills `translation`, otherwise Glossa translates the text itself.
`granularity` (`full_text`, `blocks`, `lines`, `words`) says how much
structure the caller wants; more is always fine.

```json
{ "text": "Recognised text\nsecond line", "translation": "…", "detected_lang": "en",
  "blocks": [{ "level": "line", "text": "Recognised text", "bbox": [10, 8, 210, 36], "confidence": 0.97 }],
  "image_size": [640, 200], "hocr": null, "alto": null, "transform": null }
```

Only `text` is required, and it may be empty when the structure carries the
result. `blocks` is the layout (blocks, paragraphs, lines, words with boxes,
rotation, outline, confidence, baseline, language, direction and per-region
translation); `hocr` or `alto` may carry it instead as a standard document;
`transform` says the coordinates belong to a prepared copy of the input
(Glossa maps them back). Coordinates are pixels of the input image. The
fields, the formats and the coordinate rules are in
[ocr-modules.md](ocr-modules.md).

### `action`

A button the module declares in `[[actions]]` was pressed (see
[README.md](README.md#actions)). Glossa sends this method only for actions
the manifest declares, so a module without `[[actions]]` never sees it.

```json
{ "action": "test", "handler": "testKey",
  "event": { "type": "click", "element": "test" },
  "state": {}, "options": { "api_key": "AIza…", "model": "small" },
  "ui_lang": "ru", "strings": { "key_ok": "Ключ принят.", "key_refused": "Ключ не подошёл: {0}" } }
```

`action` is the entry's `id`; `handler` is the function to run (the entry's
`handler`, or its id). `options` are the values on the settings page,
applied or not, so "Test" checks what the user typed. From a
[panel](panels.md), `panel` is the panel's id, `state` its state, `options`
the saved settings, and `event` says what set the action off: `type` is
`click`, `submit`, `change`, `open` or `shortcut`, `element` the element's
id, and `item` a clicked list row. On the settings page `panel` is absent
and `state` empty.
`ui_lang` is Glossa's interface language, and `strings` the module's
`[strings]` in it (from `locales/<lang>.toml`, falling back to
`locales/en.toml` key by key), so an SDK's `t()` needs no round trip.

```json
{ "options": { "model": "large" },
  "choices": { "model": [{ "value": "large", "label": "Large" }, "small"] },
  "errors":  { "api_key": "The key was refused." },
  "notice":  "Connected.",
  "notice_level": "success" }
```

Every field is optional, and `{}` (or `null`) is "done". `state` is merged
into a panel's state (only keys the panel declares, within its
[limits](panels.md#limits)). `options` is merged
into the settings being edited (the user still applies them; a panel
ignores it, and `choices`); only the
module's own option keys are taken, and a value of the wrong type for its
option fails the action (`bad_response`). `choices` replaces the choices of
`select` and `multiselect` options, by key; a choice is `{value, label?}` or
a plain string. `errors` are shown under the named fields until they are
edited. `notice` is shown beside the button; `notice_level` is `info` (the
default), `success`, `warning` or `error`. A thrown error is shown beside the
button as the error's message. Actions run with the module's network
permissions, like every other call.

## Errors

A method fails with a typed error:

```json
{ "kind": "missing_credential", "message": "api_key", "retry_after_secs": null }
```

| kind | meaning |
|---|---|
| `missing_credential` | a required setting is empty or was rejected; `message` = the option key |
| `rate_limited` | try later; `retry_after_secs` if known |
| `not_found` | the word or text has no result |
| `source_language` / `target_language` | the service cannot do this language |
| `unsupported` | the method is not implemented |
| `bad_request` | the input is unusable (empty, too long…) |
| `bad_response` | the service answered something unreadable |
| `http` / `network` | the request failed |
| `other` | anything else |

`missing_credential` is a setup problem: Glossa stops there and tells the
user which setting to fill in. Any other error moves on to the next service
in the fail-over chain (the active service first, then the other enabled ones
in their order in *Options → Services*), when fail-over is on; the user sees
the error only if every service fails.

JavaScript throws `new ModuleError(kind, message)`; Python raises
`ModuleError(kind, message)`; a native module returns `{"error": {…}}`. An
exception that is not a `ModuleError` becomes `other` with its message (and
the traceback, for Python).

An error may also say where in the module's own code it was raised, for
the author's benefit (the [module editor](editor.md) underlines the line):

```json
{ "kind": "other", "message": "TypeError: cannot read property 'x' of undefined",
  "location": { "file": "main.js", "line": 23, "column": 14 } }
```

`location` is optional and additive, and so is each of its fields: `file`
is relative to the module's folder, `line` and `column` are 1-based (0 or
left out when unknown). Glossa's JavaScript runtime fills it from the
exception's stack, skipping its own preludes. The place stays out of
`message`, which is what users see; the module editor and `glossa-cli module
test` show it beside the error. Other runtimes may fill it from a
traceback; a host that does not know the field ignores it.

In Rust, build a `ModuleError` with `ModuleError::new(kind, message)` (and
`.at(location)`), or end a struct literal with `..Default::default()`, so
optional fields added later do not break it.

## Host calls

While handling a request, a module may ask Glossa for these. Each returns a
result or an error of the same shape as above.

### `http`

```json
{ "method": "POST", "url": "https://api.example.com/v1/translate",
  "headers": { "Content-Type": "application/json" },
  "body": "{\"q\":\"Hello\"}",
  "body_base64": null,
  "timeout_ms": 10000,
  "binary": false }
```
→
```json
{ "status": 200, "headers": { "content-type": "application/json" },
  "body": "{\"text\":\"Привет\"}", "body_base64": null }
```

- Only hosts listed in `permissions.network` are reachable; anything else
  fails before a byte is sent, with a message naming the permission.
- The user's proxy, Glossa's timeout and retry settings apply.
- `body_base64` sends binary data and wins over `body`. `binary: true` returns
  the response in `body_base64` instead of `body` (for images, audio).
- `body` is decoded using the response's charset (UTF-8 if none).
- A non-2xx status is **not** an error: you get the status and decide.
- Response header names are lower-case; repeated headers are joined with `\n`.

### `log`

```json
{ "level": "info", "message": "token refreshed" }
```

`trace`, `debug`, `info`, `warn`, `error`. Lines go to Glossa's log
(`<cache folder>/glossa.log`) tagged with the module id; `glossa-cli -v`
prints them.

### `cache_get` / `cache_set`

```json
{ "key": "token" }                                   → the value, or null
{ "key": "token", "value": {"t": "…"}, "ttl_secs": 600 }   → null
```

A small in-memory store per module, kept by Glossa for the life of the
process. Use it for session tokens and other things worth not fetching on
every call: a JavaScript module gets a fresh context each call and a Python
process is stopped when idle, so module-level variables do not last. `ttl_secs`
of 0 keeps the value until Glossa exits.

### `assets`

Takes no parameters (`null`). Returns where the files from the manifest's
`[[assets]]` are ([README](README.md#assets)):

```json
{ "dir": "/home/me/.local/share/glossa/models/my-ocr",
  "files": { "det": "/home/me/.local/share/glossa/models/my-ocr/det",
             "ru-en": "/home/me/.local/share/glossa/models/my-ocr/ru-en" },
  "missing": ["ru-en"] }
```

- `files` holds the assets that are downloaded and verified against the
  hash the manifest names now, by name: a file, or the folder an archive
  was unpacked into.
- `missing` lists the rest: optional assets not downloaded yet, or a file
  left from an older version of the module. Glossa starts a module only
  when its required assets are all verified.
- A module without assets gets `{"dir": "", "files": {}}`.
- Process modules also find their required assets in the environment
  (`GLOSSA_ASSETS_DIR`, `GLOSSA_ASSET_<NAME>`), so they can load a model
  before the first request.

A Glossa older than this call answers it with `unsupported`.

### `image`

`image.<operation>` — crop, resize, rotate, deskew, threshold, encode and
the rest of the image API, for preparing pictures (OCR above all) without an
imaging library:

```json
"image.scale_to_min_height"  { "handle": 1, "min_height": 32 }
→ { "handle": 2, "width": 640, "height": 64, "channels": 4,
    "transform": [0.5, 0, 0, 0, 0.5, 0] }
```

Images are handles that live until the current request returns; each
operation takes `handle` (or `data_base64`) and returns a new image whose
`transform` maps its pixels back to the original. The operations, their
parameters and the limits are in [ocr-modules.md](ocr-modules.md#the-image-api).

### `t`

During an [`action`](#action): one of the module's `[strings]` in the
interface language, `{0}`, `{1}`… replaced by `args`; the key itself when
there is none. For modules without an SDK; the SDKs read the `strings` table
sent with the call instead.

```json
"t"  { "key": "key_refused", "args": ["HTTP 400"] }   → "Ключ не подошёл: HTTP 400"
```

### `ui_update`

During an [`action`](#action) from a [panel](panels.md#streaming): partial
state, merged into the panel at once, while the handler is still working
(an answer as it is written). The params are an object of state keys; the
answer is `null`. Bounded like a reply; outside a panel it is accepted and
does nothing.

```json
"ui_update"  { "result": "The word comes from…", "progress": 40 }   → null
```

## Versioning

The engine refuses a module whose `api` is not the version it speaks, and
`glossa-cli modules` says so. Additions that old modules can ignore do not
change the version: the `action` method and the `t` and `ui_update` host
calls came with `api = 1`, and only modules that declare `[[actions]]`
receive the method. Panels added only optional uses of fields the method
already had (`panel`, `state`, `event.item`).
