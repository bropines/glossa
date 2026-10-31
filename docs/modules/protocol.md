# The Glossa module protocol

Version 1 (`api = 1` in `module.toml`). This is the contract between the
engine and a module, whatever the module is written in. The runtimes only
differ in how the messages travel:

| runtime | a request arrives as | a host call is |
|---|---|---|
| JavaScript | `translate(req, host)` etc. | `host.fetch(…)`, `host.http(…)`, `host.cache.get(…)` (synchronous) |
| Python | a JSON line on stdin | a JSON line on stdout, answered on stdin (the SDK hides both) |
| Native | `call(method, params_json)` through the C ABI | `host->call(ctx, method, params_json)` |

Everything is JSON, UTF-8. Unknown fields are ignored in both directions, so
adding a field is not a breaking change; removing or retyping one is, and
bumps `api`.

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
{ "image_base64": "iVBORw0KGgo…", "lang": "auto", "translate_to": "ru",
  "preserve_line_breaks": true, "options": {} }
```

The image is PNG, JPEG or WebP. `translate_to` is set when the user asked to
recognise *and* translate; a module whose service can do both in one go fills
`translation`, otherwise Glossa translates the text itself.

```json
{ "text": "Recognised text\nsecond line", "translation": "…", "detected_lang": "en" }
```

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

## Versioning

The engine refuses a module whose `api` is not the version it speaks, and
`glossa-cli modules` says so. Additions that old modules can ignore do not
change the version.
