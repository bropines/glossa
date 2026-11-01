# Writing Glossa modules in Python

A Python module is a directory with a `module.toml` and a script. Glossa starts
the script as a child process the first time the module is used, talks to it
over stdin/stdout, and stops it after a while without use.

```
my-translator/
├── module.toml
└── main.py
```

## module.toml

```toml
capabilities = ["translate"]          # translate, detect_language, dictionary, listen, ocr

[module]
id = "my-translator"                  # lowercase, digits, - _ .
name = "My Translator"
version = "1.0.0"
runtime = "python"
entry = "main.py"                     # the default
idle_timeout_secs = 300               # stop the process after this long unused; 0 = never

[permissions]
network = ["api.example.com"]         # hosts host.http() may reach; "*" for any

[[options]]                           # shown on the module's settings page
key = "api_key"
type = "secret"
label = "API key"
required = true
```

## main.py

```python
from glossa_module import Module, ModuleError

mod = Module()

@mod.translate
def translate(req, host):
    key = host.option("api_key")
    if not key:
        raise ModuleError("missing_credential", "api_key")
    data = host.http_json("POST", "https://api.example.com/translate",
                          headers={"Authorization": "Bearer " + key},
                          body={"q": req["text"], "source": req["from"], "target": req["to"]})
    return {"text": data["translation"], "detected_from": data.get("source")}

mod.run()
```

`glossa_module.py` is on the module's `PYTHONPATH` automatically. To pin a
version, copy it next to your `main.py`; the copy in the module's own
directory wins.

## Handlers

Each takes `(req, host)`; `req` is a dict and always carries `options` (the
module's settings). Languages are BCP 47 codes (`en`, `ru`, `zh-Hans`); the
source may be `auto`.

| decorator | `req` | return |
|---|---|---|
| `@mod.translate` | `text`, `from`, `to`, `style`?, `glossary`? | `{"text", "detected_from"?, "romanization"?, "alternatives"?}` or a string |
| `@mod.detect` | `text` | `{"lang", "confidence"}` or a code |
| `@mod.dictionary` | `text`, `from`, `to`, `exact` | `{"headword", "pronunciations", "explanations": [{"trait", "explains"}], "examples"}` |
| `@mod.speak` | `text`, `lang`, `slow` | `{"audio_base64", "mime"}` or `(bytes, mime)` |
| `@mod.recognize` | `image_base64`, `image_handle`, `lang`, `translate_to`?, `granularity`, `preserve_line_breaks` | `{"text", "translation"?, "detected_lang"?, "blocks"?, "hocr"?, "alto"?, "transform"?}` or the text ([OCR modules](../../docs/modules/ocr-modules.md)) |

## The host

| call | what it does |
|---|---|
| `host.http(method, url, headers=, body=, json_body=, body_bytes=, timeout=, binary=)` | An HTTP request made by Glossa: the user's proxy applies and only `permissions.network` hosts are allowed. Returns `{"status", "headers", "body"}` (`"body_bytes"` with `binary=True`). |
| `host.http_json(method, url, ...)` | The same, requiring 2xx, returning parsed JSON. |
| `host.log(level, message)` | Glossa's log (`trace` … `error`). |
| `host.cache_get(key, default)` / `host.cache_set(key, value, ttl_secs=)` | A small in-memory cache kept by Glossa, so tokens survive the process being stopped for idleness. |
| `host.option(key, default)` | A setting. |
| `host.image.<op>(image, **params)` | The image API: `crop`, `resize`, `rotate`, `grayscale`, `threshold`, `deskew`, `pad`, `split_tiles`… on a handle (`req["image_handle"]`), an info dict or encoded bytes; `host.image.from_bytes(data)`, `host.image.to_bytes(img, "png"\|"jpeg"\|"webp", quality=)`. |
| `map_box_back(box, img)` | (module-level) A box found in a prepared image, mapped back onto the original through `img["transform"]`. |

### Preparing a picture for an OCR service

```python
from glossa_module import Module, ModuleError, map_box_back

@mod.recognize
def recognize(req, host):
    img = host.image.grayscale(req["image_handle"])
    img = host.image.scale_to_min_height(img, min_height=32)
    words = my_service(host, host.image.to_bytes(img, "jpeg", quality=85))
    return {"text": " ".join(w["text"] for w in words),
            "blocks": [{"level": "word", "text": w["text"], "bbox": list(map_box_back(w["box"], img))}
                       for w in words]}
```

The operations, their parameters, the result format and the coordinate
rules are in [docs/modules/ocr-modules.md](../../docs/modules/ocr-modules.md);
[`modules/python/ocr-space`](../../modules/python/ocr-space) is a complete
example.

## Errors

Raise `ModuleError(kind, message)` with one of: `missing_credential` (message:
the option key), `rate_limited` (`retry_after_secs=`), `not_found`,
`source_language`, `target_language`, `bad_response`, `http`, `network`,
`unsupported`, `bad_request`, `other`. Any other exception becomes an error
carrying its traceback; the module keeps running.

## Things to know

- **Your own I/O is your business.** A module is an ordinary Python program:
  it may import anything installed and open its own connections (the
  `lens-ocr` example uses chrome-lens-py's own HTTP client). `permissions`
  only govern requests made through `host.http`, and Glossa's proxy setting
  only applies to those.
- `print()` is safe: stdout is redirected to stderr, which goes to Glossa's
  log. Do not write protocol frames yourself.
- One request at a time per module process. A call that runs past Glossa's
  module timeout gets the process killed; the next call starts a fresh one.
- The interpreter is the one set in Glossa's settings, else the first Python 3
  found (`python3`, `python`, `py -3` on Windows). Python 3.8 or newer.
- The process runs in the module's directory with `PYTHONIOENCODING=utf-8`,
  `PYTHONUNBUFFERED=1` and `PYTHONDONTWRITEBYTECODE=1`.

## The wire format

One JSON object per line, UTF-8. Glossa sends
`{"id": 1, "method": "translate", "params": {…}}`; the module answers
`{"id": 1, "result": …}` or `{"id": 1, "error": {"kind": …, "message": …}}`.
While working, the module may send `{"id": "h1", "method": "http", "params":
{…}}` (also `log`, `cache_get`, `cache_set`, `image.<op>`) and reads back `{"id": "h1",
"result" | "error": …}`. The SDK does all of this; the format is documented
for debugging, and so a module can do without the SDK.
