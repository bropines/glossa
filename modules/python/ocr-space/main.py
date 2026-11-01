"""OCR.space text recognition, as a Glossa OCR module.

The service: POST https://api.ocr.space/parse/image (form fields
``base64Image`` as a data URL, ``language``, ``OCREngine``,
``isOverlayRequired``; the key in an ``apikey`` header). The reply's
``ParsedResults[0]`` holds ``ParsedText`` and, with the overlay asked for,
``TextOverlay.Lines[].Words[]`` with ``Left``/``Top``/``Width``/``Height``
in pixels of the uploaded picture.

The free plan takes files up to 1 MB, so the picture is shrunk and
re-encoded with the host's image API until it fits; the word boxes are then
mapped back onto the picture Glossa sent with ``map_box_back``. No imaging
library is needed, and every request goes through Glossa (its proxy, its
timeouts, the module's network permission).
"""

import base64
import json
from urllib.parse import urlencode

from glossa_module import Module, ModuleError, map_box_back

mod = Module()

URL = "https://api.ocr.space/parse/image"
LIMIT = 1024 * 1024
MAX_SIDE = 4000


def fit_upload(host, image):
    """The picture encoded under the size limit, and the image it was encoded from."""
    work = host.image.resize(image, max_side=MAX_SIDE)
    data, mime = host.image.to_bytes(work, "png"), "image/png"
    quality = 90
    while len(data) > LIMIT:
        if quality >= 50:
            data, mime = host.image.to_bytes(work, "jpeg", quality=quality), "image/jpeg"
            quality -= 20
        else:
            work = host.image.resize(work, scale=0.75)
            data = host.image.to_bytes(work, "jpeg", quality=70)
        if work["width"] < 64 or work["height"] < 16:
            raise ModuleError("bad_request", "the picture cannot be made small enough for OCR.space")
    return data, mime, work


def language(req, engine):
    lang = req.get("lang") or "auto"
    if len(lang) == 3 and lang.isalpha():
        return lang, engine
    # Unknown to the service, or "auto": engine 2 detects the language.
    return ("auto", "2") if engine == "2" or lang == "auto" else ("eng", engine)


@mod.recognize
def recognize(req, host):
    key = host.option("api_key")
    if not key:
        raise ModuleError("missing_credential", "api_key")
    image = req.get("image_handle")
    if image is None:  # an older Glossa without handles
        image = host.image.from_bytes(base64.b64decode(req["image_base64"]))
    data, mime, work = fit_upload(host, image)
    lang, engine = language(req, str(host.option("engine", "2")))

    form = urlencode(
        {
            "base64Image": "data:%s;base64,%s" % (mime, base64.b64encode(data).decode("ascii")),
            "language": lang,
            "OCREngine": engine,
            "isOverlayRequired": "true",
            "scale": "true",
        }
    )
    resp = host.http(
        "POST",
        URL,
        headers={"apikey": key, "Content-Type": "application/x-www-form-urlencoded"},
        body=form,
        timeout=60,
    )
    status, body = resp.get("status", 0), resp.get("body", "")
    if status in (401, 403) or "api key" in body.lower()[:300]:
        raise ModuleError("missing_credential", "api_key")
    if status == 429 or "maximum number" in body.lower()[:300]:
        raise ModuleError("rate_limited", body[:200])
    if not 200 <= status < 300:
        raise ModuleError("http", "HTTP %d: %s" % (status, body[:200]))
    try:
        reply = json.loads(body)
    except ValueError:
        raise ModuleError("bad_response", body[:200])
    if not isinstance(reply, dict):
        raise ModuleError("bad_response", body[:200])
    if reply.get("IsErroredOnProcessing"):
        message = reply.get("ErrorMessage") or reply.get("ErrorDetails") or "OCR.space could not process the picture"
        if isinstance(message, list):
            message = "; ".join(str(m) for m in message)
        raise ModuleError("bad_response", str(message))
    results = reply.get("ParsedResults") or []
    if not results:
        raise ModuleError("not_found", "no text in the picture")
    page = results[0]
    text = (page.get("ParsedText") or "").replace("\r\n", "\n").strip()

    lines = []
    for line in (page.get("TextOverlay") or {}).get("Lines") or []:
        words = []
        for w in line.get("Words") or []:
            x, y = w.get("Left", 0), w.get("Top", 0)
            box = map_box_back((x, y, x + w.get("Width", 0), y + w.get("Height", 0)), work)
            words.append({"level": "word", "text": w.get("WordText", ""), "bbox": list(box)})
        if words:
            lines.append({"level": "line", "text": line.get("LineText", ""), "children": words})
    if not text and not lines:
        raise ModuleError("not_found", "no text in the picture")
    return {"text": text, "blocks": lines}


mod.run()
