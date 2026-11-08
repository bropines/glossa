# OCR modules

An OCR module recognises text in a picture: a screen region the user
selected, an image dropped on the main window, a file given to
`glossa-cli ocr`. It declares `capabilities = ["ocr"]` and implements one
method, `recognize`. Any runtime will do; for a network OCR service
(Cloud Vision, OCR.space, Azure, Yandex Vision, your own server) JavaScript
is usually the least work, and nothing needs installing.

Two things make this easier than it sounds:

- **A standard result.** Return the text alone, or the full layout (blocks,
  paragraphs, lines, words with boxes and confidence) as JSON, or an
  **hOCR** or **ALTO** document if the engine already produces one. Glossa
  turns all of them into the same structure, which the overlay, the
  translation and the exporters use.
- **An image API.** Cropping, scaling, rotating, straightening, binarising,
  encoding: the preprocessing an OCR service needs, done by Glossa as host
  calls, so a module needs neither PIL nor OpenCV. Every prepared image
  carries the transform back to the original, so boxes map back exactly.

Examples, each tested against a canned reply:

| example | runtime | shows |
|---|---|---|
| [`modules/js/google-vision-ocr`](../../modules/js/google-vision-ocr) | JavaScript | enlarging small captures, the full layout with outlines and confidence, `transform` |
| [`modules/python/ocr-space`](../../modules/python/ocr-space) | Python | fitting an upload under a size limit, `map_box_back` |
| [`modules/native/example-lines`](../../modules/native/example-lines) | Rust | deskew, threshold, reading pixels back, a layout of its own |
| [`modules/process/paddle-ocr`](../../modules/process/paddle-ocr) | Rust process | an offline engine: models and ONNX Runtime as [assets](README.md#assets), optional recognisers per script, line outlines |

To start from working code, copy one of these examples and read
[Writing one](#writing-one). The sections before it are the reference.

## Contents

- [The request](#the-request)
- [The result](#the-result): [nodes](#nodes),
  [returning hOCR or ALTO](#returning-hocr-or-alto)
- [Coordinates](#coordinates)
- [The image API](#the-image-api)
- [Writing one](#writing-one) in [JavaScript](#javascript),
  [Python](#python), [Rust](#rust), [C](#c) or
  [another language](#other-languages)
- [What Glossa does with the result](#what-glossa-does-with-the-result)
- [Testing](#testing)

## The request

```json
{ "image_base64": "iVBORw0KGgo…", "image_handle": 1,
  "lang": "auto", "translate_to": "ru", "granularity": "words",
  "preserve_line_breaks": true, "options": { "api_key": "…" } }
```

| field | meaning |
|---|---|
| `image_base64` | The picture: PNG, JPEG or WebP. |
| `image_handle` | The same picture, already registered with the image API for this call. Use it instead of decoding `image_base64` yourself. Absent from Glossa versions before the image API. |
| `lang` | The language of the text (BCP 47, through your `[language_map]`), or `auto`. |
| `translate_to` | Set when the user asked to recognise *and* translate. A service that does both in one go fills `translation`; otherwise Glossa translates. |
| `granularity` | How much structure the caller wants: `full_text`, `blocks`, `lines` or `words`. A hint: returning more is always fine. `glossa-cli ocr --format hocr` asks for `words`. |
| `preserve_line_breaks` | Keep the picture's line breaks in `text`; off joins lines into paragraphs. |

## The result

Only `text` is required, and a plain string is accepted in JavaScript and
Python. The rest is optional:

```json
{
  "text": "Hello world\nSecond line",
  "detected_lang": "en",
  "translation": "Привет, мир\nВторая строка",
  "image_size": [640, 200],
  "blocks": [
    { "level": "block", "text": "Hello world\nSecond line",
      "bounds": { "x": 12, "y": 10, "width": 300, "height": 70 },
      "translation": "Привет, мир\nВторая строка",
      "children": [
        { "level": "line", "text": "Hello world",
          "bounds": { "x": 12, "y": 10, "width": 180, "height": 28 },
          "baseline": { "slope": 0, "offset": -6 },
          "children": [
            { "level": "word", "text": "Hello", "bbox": [12, 10, 90, 38], "confidence": 0.98 },
            { "level": "word", "text": "world", "bbox": [100, 10, 192, 38], "confidence": 0.95 } ] },
        { "level": "line", "text": "Second line", "angle": -0.05,
          "polygon": [[12, 50], [300, 36], [301, 64], [13, 78]] } ] } ]
}
```

| field | meaning |
|---|---|
| `text` | Everything read, lines apart by `\n`. May be empty when `blocks`, `hocr` or `alto` is given: Glossa joins it from the structure. |
| `detected_lang` | The language the service detected (your code; mapped back through `[language_map]`). |
| `translation` | The whole translation, when `translate_to` was set and the service translated. |
| `blocks` | The layout, below. |
| `hocr` / `alto` | The layout as an hOCR or ALTO document instead of `blocks`; see [below](#returning-hocr-or-alto). |
| `image_size` | `[width, height]` of the picture the coordinates refer to. Glossa reads it from the input when absent. |
| `transform` | Set when the coordinates refer to a prepared copy of the input: that copy's transform from the image API. Glossa maps every box back. See [coordinates](#coordinates). |

### Nodes

A node is a block, paragraph, line or word; `children` nest them. Any part
of the tree will do: a list of lines (what Windows OCR and OCR.space give),
lines of words, blocks of lines, blocks of paragraphs of lines of words.

| field | meaning |
|---|---|
| `level` | `block`, `paragraph`, `line` or `word`. Inferred from the tree when absent; set it when you know. |
| `text` | The node's text. A node without text gets its children's (a line's words joined by spaces). |
| `bounds` | `{x, y, width, height}`: the box of the text **as if it were level**, placed about its centre. For level text that is simply its bounding box. |
| `bbox` | `[x0, y0, x1, y1]`, the same for engines that think in corners; used when `bounds` is absent. |
| `angle` | Rotation of `bounds` about its centre, radians, **clockwise** on screen. 0 for level text. |
| `polygon` | The outline, `[[x, y], …]`, usually four corners clockwise from the text's top left. An outline alone is enough: Glossa derives `bounds` and `angle` from four corners. |
| `confidence` | 0–1. |
| `baseline` | For lines, hOCR's convention: `y = slope·x + offset` with the origin at the bottom-left corner of the line's bounding box. |
| `lang` | The node's language, when the service reports it per region. |
| `direction` | `ltr`, `rtl` or `ttb` (vertical, columns right to left). |
| `translation` | This region translated, when the service translates region by region (Lens does, per paragraph and per line): the in-place overlay draws it over the region. |

A node without a box gets the union of its children's, so boxes on words
alone are enough.

This JSON is exactly what `glossa-cli ocr --format json` prints for a
result, so the easiest way to see good output is to run a built-in engine
on a picture.

### Returning hOCR or ALTO

If the engine speaks one of the standard formats, pass its document through:

```python
return {"text": "", "hocr": hocr_document}      # or {"alto": alto_document}
```

Glossa reads (tolerantly: HTML-style hOCR, missing end tags and unknown
elements are fine):

- **hOCR**: `ocr_page` (its `bbox` is the image size), `ocr_carea` /
  `ocr_column` as blocks, `ocr_par`, `ocr_line` and its kin (`ocr_caption`,
  `ocr_header`, `ocr_textfloat`, `ocrx_line`), `ocrx_word`; the `bbox`,
  `baseline`, `textangle`, `poly` and `x_wconf` properties; `lang` and `dir`
  attributes.
- **ALTO**, any version: `Page` (`WIDTH`, `HEIGHT`, `LANG`), `TextBlock`,
  `TextLine` (`BASELINE` as a number or as points), `String` (`CONTENT`,
  `WC`, `LANG`), `ROTATION`, `BASEDIRECTION`, `Shape/Polygon`.

`blocks` wins over `hocr`, which wins over `alto`, when more than one is
given. A `transform` applies to these too.

## Coordinates

**Coordinates refer to the picture the module was given** (`image_base64` /
`image_handle`): pixels, origin at the top left, y downwards.

A module that sends a prepared copy to its service (enlarged, padded,
cropped, rotated, re-encoded smaller) gets boxes in the copy's pixels. Every
image the image API returns carries a `transform`: six numbers
`[a, b, c, d, e, f]` mapping a point of that image back to the original,

```
x_original = a·x + b·y + c
y_original = d·x + e·y + f
```

composed through every step since the image was decoded (for `recognize`,
since the request's image). Either:

1. **map the boxes yourself** with the SDK helper — `host.image.mapBoxBack(box,
   img)` in JavaScript, `map_box_back(box, img)` in Python,
   `map_box_back(rect, &img.transform)` in Rust, `glossa_map_point` in C (map
   the four corners, take their bounding box); or
2. **send the copy's `transform`** in the result and let Glossa do it; it
   maps outlines and rotated boxes exactly.

Do one or the other, not both. Splitting into tiles works the same way:
each tile's transform includes its offset.

## The image API

Available to every runtime as host calls named `image.<operation>`; the SDKs
wrap them (`host.image.*`). Images live for the current call only, as
**handles**: an operation takes one and returns a new one, so a big picture
crosses into a Python process or a native library once, as the final
upload, instead of on every step.

Every operation takes its input as `handle` (a number) or `data_base64`
(encoded bytes, registered on the spot), plus its parameters, and returns an
info object:

```json
{ "handle": 3, "width": 1280, "height": 400, "channels": 1,
  "transform": [0.5, 0, 0, 0, 0.5, 0], "format": "png" }
```

`channels` is 1 (grey) or 4 (RGBA). Operations that have nothing to do (a
resize to the same size, `scale_to_min_height` on a tall picture, `deskew`
on level text) return the input's handle unchanged.

| operation | parameters | returns / does |
|---|---|---|
| `decode` | `data_base64` | Registers encoded bytes (PNG, JPEG, WebP); `format` says which. |
| `info` | — | Size, channels, transform. Decodes a lazily registered image. |
| `encode` (`to_base64`) | `format`: `png` (default), `jpeg`, `webp` (lossless); `quality` 1–100 for JPEG (default 90) | Adds `data_base64` and `mime`. JPEG flattens transparency onto white. |
| `pixels` | — | Adds `data_base64`: the raw pixels, row by row, `channels` bytes each. |
| `free` | `handle` | Releases an image early (memory counts against the call's budget). |
| `new` | `width`, `height`, `color` | A blank canvas. |
| `crop` | `x`, `y`, `width`, `height` | Clipped to the picture; an empty crop is an error. |
| `resize` | one of: `scale`; `max_side` (shrinks only); `width` and/or `height` with `mode` `fit` (default: inside the box), `fill` (cover it) or `exact`. `filter`: `nearest`, `bilinear` (default), `bicubic`, `lanczos` | The aspect ratio is kept unless `mode` is `exact`. |
| `scale_to_min_height` | `min_height` (default 32), `max_side`, `filter` (default `lanczos`) | Enlarges a picture shorter than `min_height`; recognisers want letters 20–40 px high. |
| `rotate` | `angle` (degrees, clockwise), `background` (default white), `expand` (default true: grow to hold the whole picture) | Right angles are exact; other angles are resampled bilinearly. |
| `flip` | `direction`: `horizontal` (default) or `vertical` | |
| `grayscale` | — | Rec. 601 luma, transparency composited onto white. |
| `invert` | — | Light text on dark becomes dark on light. Alpha is kept. |
| `threshold` | `value` 0–255, or omitted / `"otsu"` for Otsu's method | Black and white (grey channel); adds `threshold`, the level used. |
| `adaptive_threshold` | `block_size` (odd, 3–255, default 31), `c` (default 10) | Each pixel against the mean of the square around it, less `c`: copes with uneven lighting and shadows. |
| `adjust` | `contrast` (factor, 1 = unchanged), `brightness` (−255…255), `gamma` (> 1 lightens mid-tones) | |
| `normalize` (`auto_levels`) | `cutoff` (per cent ignored at each end, default 0.5) | Stretches each channel to the full range. |
| `blur` | `radius` (default 1), `kind`: `gaussian` (default, σ = radius) or `box` | |
| `sharpen` | `amount` (default 1), `radius` (default 1) | Unsharp masking. |
| `pad` | `all`, or `top` / `right` / `bottom` / `left`; `color` (default white) | Some engines miss text that touches the edge. |
| `deskew` | `max_angle` (degrees, default 10), `background` (default: the border's colour) | Estimates the skew from the projection profile of the ink and turns the picture level; adds `angle`, the rotation applied (degrees clockwise). |
| `split_tiles` | `max_width`, `max_height`, `overlap` | Adds `tiles`, an info object per tile, in rows from the top left; each tile's transform includes its offset. For services with a size limit. |
| `paste` (`composite`) | `source` (a handle), `x`, `y` | The source over a copy of the image, blended by its alpha, clipped. |
| `draw_rect` | `x`, `y`, `width`, `height`, `color` (default red), `thickness` (default 2) | For debugging: draw what was found, `encode`, and look. |

Colours are `"#rrggbb"`, `"#rrggbbaa"`, `"#rgb"`, `white`, `black`,
`transparent`, `red`, `[r, g, b]`, `[r, g, b, a]` or a grey level.

**Limits.** An image may have at most 100 megapixels and 50 000 pixels per
side, decoded or produced; the images of one call together at most 1 GiB of
pixels and 512 handles; encoded input at most 64 MB. A picture over the
limits is refused before its pixels are allocated. Bad parameters fail with
`bad_request` naming the parameter; an unknown operation with `unsupported`.

The image API is part of Glossa builds with the `image` feature (all
release builds); without it the calls answer `unsupported`.

## Writing one

### JavaScript

```js
function recognize(req, host) {
  if (!host.options.api_key) throw new ModuleError("missing_credential", "api_key");
  var img = host.image.grayscale(req.image_handle);
  img = host.image.scaleToMinHeight(img, { min_height: 40, max_side: 4000 });
  var jpeg = host.image.encode(img, { format: "jpeg", quality: 85 });

  var r = host.fetch("https://ocr.example.com/v1/read", {
    method: "POST", headers: { Authorization: "Bearer " + host.options.api_key },
    json: { image: jpeg.data_base64, lang: req.lang }
  });
  if (!r.ok) throw new ModuleError("http", "HTTP " + r.status);
  var data = r.json();
  return {
    text: data.text,
    blocks: data.lines.map(function (l) {
      return { level: "line", text: l.text, bbox: [l.x0, l.y0, l.x1, l.y1], confidence: l.score };
    }),
    transform: img.transform          // boxes are in the prepared copy
  };
}
```

Each `host.image.<op>(image, params)` takes a handle, an info object or
nothing (when `params` carries `handle` or `data_base64`); snake_case names
work too (`host.image.scale_to_min_height`). `host.image.fromBase64(b64)`
registers bytes, `host.image.call(op, params)` reaches any operation, and
`host.image.mapBoxBack(box, img)` maps a `{x, y, width, height}` or
`[x0, y0, x1, y1]` box back.

### Python

```python
from glossa_module import Module, ModuleError, map_box_back

mod = Module()

@mod.recognize
def recognize(req, host):
    img = host.image.grayscale(req["image_handle"])
    img = host.image.resize(img, max_side=2000)
    data = host.image.to_bytes(img, "png")
    words = call_my_service(host, data)            # through host.http
    return {
        "text": " ".join(w["text"] for w in words),
        "blocks": [{"level": "word", "text": w["text"],
                    "bbox": list(map_box_back(w["box"], img))} for w in words],
    }

mod.run()
```

`host.image.<op>(image, **params)` takes a handle, an info dict or encoded
`bytes`; `host.image.from_bytes(data)` and `host.image.to_bytes(img, format,
quality=)` convert; `map_box_back(box, img)` takes `(x0, y0, x1, y1)` or a
`{"x", "y", "width", "height"}` dict. A Python module may of course use
Pillow or OpenCV itself; the image API is for when it would rather not
depend on them.

### Rust

```rust
use glossa_module_sdk::wire::*;
use glossa_module_sdk::{export_module, map_box_back, Host, Module};
use serde_json::json;

#[derive(Default)]
struct MyOcr;

impl Module for MyOcr {
    fn recognize(&mut self, host: &Host, req: RecognizeParams) -> Result<RecognizeResult, ModuleError> {
        let input = req.image_handle.ok_or_else(|| ModuleError::unsupported("a Glossa without the image API"))?;
        let gray = host.image("grayscale", json!({ "handle": input }))?;
        let big = host.image("scale_to_min_height", json!({ "handle": gray.handle, "min_height": 32 }))?;
        let png = host.image_encode(big.handle, "png", None)?;
        let found: Vec<(String, Rect)> = my_engine(&png)?;
        let blocks = found
            .into_iter()
            .map(|(text, r)| OcrNode::new("line", text, Some(map_box_back(r, &big.transform))))
            .collect();
        Ok(RecognizeResult { blocks, ..Default::default() })
    }
}

export_module!(MyOcr);
```

`Host::image(op, params)` returns an [`ImageInfo`](../../crates/glossa-module-sdk/src/wire.rs);
`Host::image_decode` and `Host::image_encode` convert bytes. See
[`example-lines`](../../modules/native/example-lines/src/lib.rs).

### C

The host call is the same `host->call` as for `http`:

```c
char *reply = host->call(host->ctx, "image.grayscale", "{\"handle\": 1}");
/* {"result": {"handle": 2, "width": …, "transform": [a, b, c, d, e, f]}} */
host->free(host->ctx, reply);
```

`glossa_map_point(transform, x, y, &ox, &oy)` in
[`glossa_module.h`](../../include/glossa_module.h) maps a point back.

### Other languages

A runtime that carries host calls (the process runtime for Java and other
executables speaks the same stdio frames as Python) gets the image API
unchanged: send `{"id": "h1", "method": "image.crop", "params": {…}}`, read
the reply.

## What Glossa does with the result

The text goes where any recognised text goes (the main window, the popup,
the clipboard, the translation); the structure is there for front ends to
place text and translations over the picture, and the exporters write it
out:

```bash
glossa-cli ocr page.png -F json            # the structure above
glossa-cli ocr page.png -F hocr > page.hocr # hOCR 1.2 (XHTML), for hocr-tools, OCRmyPDF, gImageReader
glossa-cli ocr page.png -F alto > page.xml  # ALTO v4, for digitisation tools and viewers
glossa-cli ocr page.png -e my-ocr -F alto   # one engine, no fail-over
curl -s -H "Authorization: Bearer $TOKEN" -H "Content-Type: image/png" \
     --data-binary @page.png "localhost:60828/v1/ocr?format=hocr"
```

Why hOCR and ALTO: they are what OCR software already reads and writes.
hOCR is Tesseract's and OCRopus's native output; ALTO is the Library of
Congress format digitisation projects archive in. Both describe the same
tree Glossa uses (page, block, paragraph — hOCR only — line, word, with
boxes, rotation and confidence), so nothing is lost going out, and a module
wrapping an engine that writes either needs no mapping code. Per-region
translations have no place in either format and appear only in the JSON.

## Testing

```bash
GLOSSA_HOME=/tmp/gtest glossa-cli modules                         # did it load?
GLOSSA_HOME=/tmp/gtest glossa-cli -v ocr shot.png -e my-ocr -F json  # with the module's log
```

`draw_rect` and `encode` let a module save what it sent to its service:
log the `data_base64` or write it to a file of its own while developing.
