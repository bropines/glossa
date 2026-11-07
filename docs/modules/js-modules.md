# Writing Glossa modules in JavaScript

A JavaScript module is a directory with a `module.toml` and a script. It runs
inside Glossa (the [QuickJS](https://github.com/quickjs-ng/quickjs) engine),
so there is nothing to install, no process to start, and no toolchain to
build with. Libraries from npm can be used by bundling them into the one
script ([below](#using-npm-packages)).

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
- **Modern JavaScript** (ES2023): `let`/`const`, arrow functions, classes
  with private fields, destructuring, template literals, `async`/`await`,
  `Map`/`Set`, typed arrays, `BigInt`, regular expressions with Unicode,
  `JSON`. No `Intl`.
- **Provided globals**: `ModuleError`, `console`, `btoa`/`atob`,
  `TextEncoder`/`TextDecoder` (UTF-8), `encodeURIComponent` and friends,
  `queueMicrotask`, and a `setTimeout` that runs its callback on the next
  microtask (there is no event loop between calls, so it does not wait).
- **Web stand-ins**, for code written for the browser (see
  [npm packages](#using-npm-packages)): `fetch` (over `host.http`, with the
  same network permissions; the request runs synchronously and the promise is
  already settled), `Headers`, `URL`, `URLSearchParams` and an
  `AbortController` that never aborts. They cover what request-building code
  uses, not every corner of the web standards. Like the browser's, these and
  the globals above are ordinary properties of the global object, so a
  module may declare its own `fetch`, `URL` or `console` with `const` or
  `class`.
- **Not available**: the DOM, `XMLHttpRequest`, Node's `require` and built-in
  modules (`fs`, `crypto`, `buffer`), timers that wait, the file system.
- **Limits**, per call: **128 MB** of JavaScript heap and **10 seconds** of
  script time (time spent waiting for `host.fetch` and other host calls does
  not count; the network has its own timeouts), and a native stack deep
  enough for about 500 nested calls. A script over a limit is stopped
  and the call fails with an `other` error: *the script ran for more than
  10 s and was stopped*, *the script used more than 128 MB of memory and was
  stopped*, or *the script recursed too deeply (stack overflow)*. A
  `try`/`catch` cannot swallow the time limit.
- The script is read when modules load. After editing it, use *Options →
  Modules → Reload modules* (or restart).

## Using npm packages

Glossa runs one file, with no `require` and no `node_modules`. To use a
library from npm, bundle the module and its dependencies into that file with
[esbuild](https://esbuild.github.io) (any bundler that writes CommonJS works;
`bun build --format=cjs` too):

```
my-module/
├── module.toml          # entry = "main.js" (the default)
├── main.js              # the bundle: what Glossa runs
├── package.json         # for npm; not read by Glossa
└── src/index.js         # your code
```

```js
// src/index.js
import { decode } from "he";                 // HTML entities
import queryString from "query-string";

export async function translate(req, host) {
  const url = "https://api.example.com/t?" + queryString.stringify({ q: req.text, to: req.to });
  const r = await fetch(url, { headers: { "X-Key": req.options.api_key } });
  if (!r.ok) throw new ModuleError("http", "status " + r.status);
  return { text: decode((await r.json()).translation) };
}
```

```bash
npm install he query-string
npx esbuild src/index.js --bundle --format=cjs --platform=neutral   --main-fields=module,main --target=es2022 --minify --outfile=main.js
```

- `--format=cjs` makes the bundle assign its exports to `module.exports`,
  where Glossa finds `translate` and the other entry points.
- `--platform=neutral` keeps Node's built-ins out. A library that needs
  `fs`, `crypto`, `buffer` or `process` will not work; look for a browser
  build of it, or one written in plain JavaScript.
- Glossa's `fetch`, `URL`, `URLSearchParams`, `Headers`, `TextEncoder` and
  `atob`/`btoa` are there for libraries that expect a browser. `fetch` goes
  through `host.http`, so `permissions.network` still decides which hosts
  are reachable. Its response body is text; pass `binary: true` in the
  options when you need the exact bytes from `arrayBuffer()`.
- Each call starts fresh, so the bundle runs its top-level code every time.
  A script over 16 KB is compiled once, when the module loads, and kept as
  bytecode (about one and a half times the minified size) while the module
  is loaded; smaller scripts are parsed each call and keep nothing. Size is
  still speed: `--minify`, and prefer small libraries. A 170 KB bundle
  (`he`, `query-string`, `crypto-js`) costs about 4 ms per call on an Apple
  silicon Mac; the network round trip usually costs more. In a compiled
  script, `Function.prototype.toString` does not return the source.
- Ship `main.js`; `node_modules` stays out of the module folder.

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
