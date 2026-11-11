"""Google Lens OCR through the chrome-lens-py package, in-process.

An alternative to Glossa's built-in `lens` provider, which talks to the
chrome-lens-py daemon or CLI: this module imports the library directly into
its own (warm) Python process. The package is named in module.toml
(`requirements`), so Glossa installs it into an environment of this module's
own; nothing is installed by hand.

The traffic to Google is chrome-lens-py's own; it does not pass through
Glossa, which is why the manifest asks for no network permissions.
"""

import asyncio
import base64

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
            "the chrome-lens-py package is missing from this module's Python environment; "
            "download the module's packages on its settings page, or run: "
            "glossa-cli python install lens-ocr-python",
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
        # httpx names its failures ConnectError, ConnectTimeout, ProxyError,
        # ReadTimeout…; chrome-lens-py passes them on or wraps their text.
        if any(word in name or word in str(e) for word in ("Connect", "Timeout", "Proxy", "Network")):
            raise ModuleError("network", "Google Lens could not be reached: %s" % (str(e) or name))
        raise ModuleError("other", "%s: %s" % (name, e))

    text = result.get("ocr_text") or ""
    if not text.strip():
        # An empty result is not an answer: not_found lets Glossa say there
        # was no text instead of showing an empty window.
        raise ModuleError("not_found", "no text in the picture")
    return {
        "text": text,
        "translation": result.get("translated_text") or None,
        "detected_lang": result.get("detected_language") or None,
    }


mod.run()
