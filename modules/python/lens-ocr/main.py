"""Google Lens OCR through the chrome-lens-py package, in-process.

An alternative to Glossa's built-in `lens` provider, which talks to the
chrome-lens-py daemon or CLI: this module imports the library directly into
its own (warm) Python process. It needs `pip install chrome-lens-py` in the
interpreter Glossa runs modules with.

The traffic to Google is chrome-lens-py's own; it does not pass through
Glossa, which is why the manifest asks for no network permissions.
"""

import asyncio
import base64
import sys

from glossa_module import Module, ModuleError

mod = Module()

# One event loop and one LensAPI for the life of the process, so the HTTP/2
# connection to Google stays warm between captures.
_loop = None
_api = None
_api_proxy = None


def _lens_api(proxy):
    global _loop, _api, _api_proxy
    try:
        from chrome_lens_py import LensAPI
    except ImportError:
        raise ModuleError(
            "other",
            "the chrome-lens-py package is not installed for %s; run: %s -m pip install chrome-lens-py"
            % (sys.executable, sys.executable),
        )
    if _loop is None:
        _loop = asyncio.new_event_loop()
    if _api is None or proxy != _api_proxy:
        if _api is not None:
            try:
                _loop.run_until_complete(_api.aclose())
            except Exception:
                pass
        _api = LensAPI(proxy=proxy or None)
        _api_proxy = proxy
    return _api


@mod.recognize
def recognize(req, host):
    try:
        image = base64.b64decode(req["image_base64"])
    except (KeyError, ValueError) as e:
        raise ModuleError("bad_request", "image_base64: %s" % e)

    api = _lens_api(host.option("proxy", ""))
    lang = req.get("lang") or "auto"
    translate_to = req.get("translate_to")

    try:
        result = _loop.run_until_complete(
            api.process_image(
                image,
                ocr_language=None if lang == "auto" else lang,
                target_translation_language=translate_to or None,
                ocr_preserve_line_breaks=bool(req.get("preserve_line_breaks", True)),
                output_format="full_text",
            )
        )
    except Exception as e:  # chrome_lens_py.exceptions.* and network errors
        name = type(e).__name__
        if "RateLimit" in name:
            raise ModuleError("rate_limited", str(e))
        if "Connection" in name or "Timeout" in name:
            raise ModuleError("network", str(e))
        raise ModuleError("other", "%s: %s" % (name, e))

    return {
        "text": result.get("ocr_text") or "",
        "translation": result.get("translated_text") or None,
        "detected_lang": result.get("detected_language") or None,
    }


mod.run()
