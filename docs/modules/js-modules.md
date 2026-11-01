# Writing Glossa modules in JavaScript

A JavaScript module is a directory with a `module.toml` and a script. It runs
inside Glossa (the [boa](https://boajs.dev) engine), so there is nothing to
install, no process to start, and no toolchain to build with.

```
free-dictionary/
├── module.toml
└── main.js
```

The complete, working example lives in
[`modules/js/free-dictionary`](../../modules/js/free-dictionary): an English
dictionary over a free web API, in sixty lines.

## module.toml

```toml
capabilities = ["translate"]        # translate, detect_language, dictionary, listen, ocr

[module]
id = "my-translator"
name = "My Translator"
version = "1.0.0"
runtime = "js"
entry = "main.js"                   # the default

[permissions]
network = ["api.example.com"]       # hosts host.fetch() may reach

[[options]]
key = "api_key"
type = "secret"
label = "API key"
required = true
```

All the fields are described in [README.md](README.md#moduletoml).

## main.js

Define a function per capability, as a global or on `module.exports`:

```js
function translate(req, host) {
  if (!req.options.api_key) throw new ModuleError("missing_credential", "api_key");

  var r = host.fetch("https://api.example.com/v2/translate", {
    method: "POST",
    headers: { Authorization: "Bearer " + req.options.api_key },
    json: { q: req.text, source: req.from === "auto" ? null : req.from, target: req.to }
  });
  if (r.status === 429) throw new ModuleError("rate_limited", "slow down");
  if (!r.ok) throw new ModuleError("http", "status " + r.status);

  var data = r.json();
  return { text: data.translation, detected_from: data.detected };
}
```

| function | `req` | return |
|---|---|---|
| `translate(req, host)` | `text`, `from`, `to`, `style`?, `glossary`?, `options` | `{ text, detected_from?, romanization?, alternatives? }` or a string |
| `detect(req, host)` | `text`, `options` | `{ lang, confidence }` |
| `dictionary(req, host)` | `text`, `from`, `to`, `exact`, `options` | an article: `{ headword, pronunciations, explanations, examples, … }` |
| `speak(req, host)` | `text`, `lang`, `slow`, `options` | `{ audio_base64, mime }` |
| `recognize(req, host)` | `image_base64`, `image_handle`, `lang`, `translate_to`?, `granularity`, `options` | `{ text, translation?, detected_lang?, blocks?, hocr?, alto?, transform? }` ([OCR modules](ocr-modules.md)) |

The exact shapes are in [protocol.md](protocol.md). A function may be `async`
and return a promise; Glossa waits for it.

## The host

`host` is **synchronous**: `host.fetch()` returns the response, not a
promise. A module reads top to bottom, with no callbacks, and still keeps the
UI responsive, because every call runs on a worker thread.

| call | what it does |
|---|---|
| `host.fetch(url, init?)` | An HTTP request made by Glossa. `init`: `method`, `headers`, and one of `json` (sent as JSON), `form` (sent URL-encoded), `body` (a string); also `timeout` (ms) and `binary`. Returns `{ status, ok, headers, body, body_base64, text(), json() }`. `json()` throws a `bad_response` error if the body is not JSON. |
| `host.http(req)` | The raw [`http` host call](protocol.md#http), for full control. |
| `host.cache.get(key)` / `host.cache.set(key, value, ttlSecs?)` | A small store that outlives the call (see below). |
| `host.log(level, message)` | Glossa's log; `console.log/info/warn/error/debug` go there too. |
| `host.options` | The module's settings, the same object as `req.options`. |
| `host.image.crop(img, {…})`, `.resize`, `.grayscale`, `.deskew`, `.encode`… | The image API: preprocessing on handles, each result carrying the transform back to the original; `host.image.mapBoxBack(box, img)` maps a box back. See [ocr-modules.md](ocr-modules.md#the-image-api). |

`host.fetch` only reaches hosts in `permissions.network`, goes through the
user's proxy, and uses Glossa's timeouts and retries. A non-2xx status is
returned, not thrown: check `r.ok` or `r.status`.

## Errors

```js
throw new ModuleError("missing_credential", "api_key");   // the option to fill in
throw new ModuleError("not_found", word);
throw new ModuleError("rate_limited", "try again in a minute");
```

The kinds and what Glossa does with each are in
[protocol.md](protocol.md#errors). Any other exception becomes an `other`
error with its message; the next call starts clean.

## The environment

- **A fresh context per call.** Globals and module-level variables do not
  survive from one call to the next. Keep tokens and other state in
  `host.cache`.
- **Modern JavaScript**: `let`/`const`, arrow functions, classes,
  destructuring, template literals, `async`/`await`, `Map`/`Set`, regular
  expressions with Unicode, `JSON`.
- **Provided globals**: `ModuleError`, `console`, `btoa`/`atob`,
  `TextEncoder`/`TextDecoder` (UTF-8), `encodeURIComponent` and friends, and a
  `setTimeout` that runs its callback on the next microtask (there is no event
  loop between calls, so it does not wait).
- **Not available**: the DOM, `XMLHttpRequest`, the browser `fetch` (use
  `host.fetch`), Node's `require` and modules, timers that wait, the file
  system.
- **Limits**: 50 million loop iterations and a recursion depth of 400 per
  call, so a runaway loop fails the call instead of hanging Glossa. Glossa's
  module timeout also applies.
- The script is read when modules load. After editing it, use *Options →
  Modules → Reload modules* (or restart).

## Testing

```bash
GLOSSA_HOME=/tmp/gtest glossa-cli modules          # with your module in /tmp/gtest/modules
GLOSSA_HOME=/tmp/gtest glossa-cli -v translate -s my-translator -t ru "Hello"
```

`-v` shows the module's log lines (`host.log`, `console.*`). A syntax error
is caught when modules load: `glossa-cli modules` lists the module under
"failed" with the file and line.

## Porting

- **From pot-app**: pot plugins run unchanged; see
  [classic-scripts.md](classic-scripts.md). To turn one into a native Glossa
  module, replace `await fetch(url, { method, headers, body: Body.json(x) })`
  with `host.fetch(url, { method, headers, json: x })` and return the result
  instead of calling `setResult`. Dictionary results keep pot's field names
  (`trait`, `explains`).
- **From a classic QTranslate service**: those run unchanged too. A rewrite
  replaces the `RequestData`/`ResponseData` pair with one function that calls
  `host.fetch` and returns the text.
- **To Python or native**: the requests and results are the same JSON, so
  the logic moves over as is.
