"""Glossa module SDK for Python. Standard library only, one file.

A module is a script that Glossa runs as a child process and talks to over
stdin/stdout, one JSON object per line::

    from glossa_module import Module, ModuleError

    mod = Module()

    @mod.translate
    def translate(req, host):
        # req: {"text", "from", "to", "options", ...}
        r = host.http("GET", "https://api.example.com/t?q=" + host.quote(req["text"]))
        if r["status"] != 200:
            raise ModuleError("http", "HTTP %d" % r["status"])
        return {"text": r["body"]}

    mod.run()

Handlers take the request parameters (a dict) and a :class:`Host`, and return
a dict (``translate`` may also return a plain string). Raise
:class:`ModuleError` to report a specific failure; any other exception is
reported with its traceback, and the module keeps running.

``host.http`` goes through Glossa: the user's proxy applies, and only hosts in
the manifest's ``[permissions] network`` list are reachable that way. A module
is still an ordinary Python program and may do its own I/O; the permissions
only govern what Glossa does on its behalf.

Anything printed with ``print()`` goes to Glossa's log, not into the protocol.

Python 3.8 or newer.
"""

import json
import os
import sys
import traceback

__all__ = ["Module", "ModuleError", "Host", "Images", "map_box_back", "PROTOCOL_VERSION"]
__version__ = "1.1.0"

PROTOCOL_VERSION = 1

#: Error kinds Glossa understands; anything else is shown as a plain failure.
ERROR_KINDS = (
    "bad_request",
    "unsupported",
    "source_language",
    "target_language",
    "missing_credential",
    "bad_response",
    "http",
    "network",
    "rate_limited",
    "not_found",
    "other",
)


class ModuleError(Exception):
    """A failure Glossa can explain to the user.

    ``kind`` is one of :data:`ERROR_KINDS`: ``missing_credential`` with the
    option key as the message ("set api_key in the settings"),
    ``rate_limited`` with ``retry_after_secs``, ``not_found`` for "no
    result", and so on.
    """

    def __init__(self, kind, message="", retry_after_secs=None):
        if kind not in ERROR_KINDS:
            message = "%s: %s" % (kind, message) if message else str(kind)
            kind = "other"
        super().__init__(message)
        self.kind = kind
        self.message = str(message)
        self.retry_after_secs = retry_after_secs

    def to_wire(self):
        err = {"kind": self.kind, "message": self.message}
        if self.retry_after_secs is not None:
            err["retry_after_secs"] = int(self.retry_after_secs)
        return err


class _Channel:
    """The protocol stream: frames in on stdin, frames out on the real stdout.

    The real stdout is duplicated to a private descriptor and descriptor 1 is
    pointed at stderr, so ``print()``, a C extension writing to fd 1, or a
    subprocess inheriting it all end up in the log instead of the protocol.
    """

    def __init__(self):
        self._in = sys.stdin.buffer
        try:
            sys.stdout.flush()
            fd = os.dup(sys.stdout.fileno())
            os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
            self._out = os.fdopen(fd, "wb", buffering=0)
        except (OSError, ValueError, AttributeError):
            # No real descriptors (an embedded interpreter): fall back to the
            # buffer object, and still keep print() off it.
            self._out = sys.stdout.buffer
        sys.stdout = sys.stderr
        self._next_host_id = 0

    def send(self, obj):
        line = json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"
        self._out.write(line)
        self._out.flush()

    def receive(self):
        """The next frame, or None at end of input. Junk lines are skipped."""
        while True:
            raw = self._in.readline()
            if not raw:
                return None
            raw = raw.strip()
            if not raw:
                continue
            try:
                obj = json.loads(raw.decode("utf-8"))
            except ValueError:
                _log_local("ignoring a line that is not JSON")
                continue
            if isinstance(obj, dict):
                return obj

    def host_call(self, method, params):
        """Call back into Glossa and wait for its answer."""
        self._next_host_id += 1
        call_id = "h%d" % self._next_host_id
        self.send({"id": call_id, "method": method, "params": params})
        while True:
            frame = self.receive()
            if frame is None:
                raise ModuleError("other", "Glossa closed the connection")
            if frame.get("id") != call_id:
                _log_local("ignoring an unexpected frame while waiting for %s" % call_id)
                continue
            err = frame.get("error")
            if err:
                if isinstance(err, dict):
                    raise ModuleError(
                        err.get("kind", "other"), err.get("message", ""), err.get("retry_after_secs")
                    )
                raise ModuleError("other", str(err))
            return frame.get("result")


def _log_local(message):
    try:
        sys.stderr.write("[glossa_module] %s\n" % message)
        sys.stderr.flush()
    except Exception:
        pass


def map_box_back(box, transform):
    """A box found in a derived image, as the axis-aligned box in the original.

    ``transform`` is the ``"transform"`` of an image the image API returned
    (or that info dict itself); every image carries the map back to the image
    it was first decoded from (for ``recognize``, the request's image).
    ``box`` is ``(x0, y0, x1, y1)`` or ``{"x", "y", "width", "height"}`` and
    comes back in the same shape.
    """
    if isinstance(transform, dict):
        transform = transform.get("transform")
    if not transform:
        return box
    a, b, c, d, e, f = transform
    if isinstance(box, dict):
        x0, y0 = box["x"], box["y"]
        x1, y1 = x0 + box["width"], y0 + box["height"]
    else:
        x0, y0, x1, y1 = box
    pts = [(a * x + b * y + c, d * x + e * y + f) for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1))]
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    if isinstance(box, dict):
        return {"x": min(xs), "y": min(ys), "width": max(xs) - min(xs), "height": max(ys) - min(ys)}
    return (min(xs), min(ys), max(xs), max(ys))


class Images:
    """The host's image API, as ``host.image``: PIL-style preprocessing done
    by Glossa, so a module needs no imaging library.

    Every operation takes the image first -- a handle (``req["image_handle"]``
    for the request's image), an info dict another operation returned, or
    encoded ``bytes`` -- then keyword parameters, and returns an info dict:
    ``{"handle", "width", "height", "channels", "transform", ...}``. Handles
    last for the current request::

        g = host.image.grayscale(req["image_handle"])
        big = host.image.scale_to_min_height(g, min_height=32)
        jpeg = host.image.to_bytes(big, "jpeg", quality=85)
        ...
        box = map_box_back((x0, y0, x1, y1), big)   # back onto the input

    Operations: decode, info, encode, pixels, free, new, crop, resize,
    scale_to_min_height, rotate, flip, grayscale, invert, threshold,
    adaptive_threshold, adjust, normalize, blur, sharpen, pad, deskew,
    split_tiles, paste, draw_rect. Parameters are documented in
    docs/modules/ocr-modules.md.
    """

    OPS = (
        "decode", "info", "encode", "pixels", "free", "new", "crop", "resize", "scale_to_min_height",
        "rotate", "flip", "grayscale", "invert", "threshold", "adaptive_threshold", "adjust", "normalize",
        "auto_levels", "blur", "sharpen", "pad", "deskew", "split_tiles", "paste", "composite", "draw_rect",
    )

    def __init__(self, channel):
        self._channel = channel

    def call(self, op, image=None, **params):
        """Any operation by name."""
        import base64

        if isinstance(image, (bytes, bytearray)):
            params["data_base64"] = base64.b64encode(bytes(image)).decode("ascii")
        elif isinstance(image, dict):
            params["handle"] = image["handle"]
        elif image is not None:
            params["handle"] = int(image)
        return self._channel.host_call("image." + op, params) or {}

    def __getattr__(self, name):
        if name in Images.OPS:
            return lambda image=None, **params: self.call(name, image, **params)
        raise AttributeError(name)

    def __dir__(self):
        return list(super().__dir__()) + list(Images.OPS)

    def from_bytes(self, data):
        """Register encoded image bytes (PNG, JPEG, WebP) and get its info."""
        return self.call("decode", bytes(data))

    def to_bytes(self, image, format="png", quality=None):
        """Encode an image (``png``, ``jpeg`` or lossless ``webp``) and return the bytes."""
        import base64

        params = {"format": format}
        if quality is not None:
            params["quality"] = int(quality)
        out = self.call("encode", image, **params)
        return base64.b64decode(out.get("data_base64") or "")


class Host:
    """What a handler may ask of Glossa during one request."""

    def __init__(self, channel, params):
        self._channel = channel
        self.options = (params or {}).get("options") or {}
        #: The image API; see :class:`Images`.
        self.image = Images(channel)

    # -- network ---------------------------------------------------------

    def http(self, method, url, headers=None, body=None, json_body=None, timeout=None, binary=False, body_bytes=None):
        """One HTTP request through Glossa.

        Returns ``{"status": int, "headers": {lowercase name: value}, "body":
        str}``; with ``binary=True`` the body comes back as bytes under
        ``"body_bytes"`` instead. Any status is a response; deciding that 4xx
        is an error is up to you (:meth:`http_json` does it for you).
        ``json_body`` serialises a value and sets the content type;
        ``body_bytes`` sends binary data. ``timeout`` is in seconds.
        """
        req = {"method": method.upper(), "url": url, "headers": dict(headers or {})}
        if json_body is not None:
            req["body"] = json.dumps(json_body, ensure_ascii=False)
            if not any(k.lower() == "content-type" for k in req["headers"]):
                req["headers"]["Content-Type"] = "application/json; charset=utf-8"
        elif body_bytes is not None:
            import base64

            req["body_base64"] = base64.b64encode(body_bytes).decode("ascii")
        elif body is not None:
            req["body"] = body if isinstance(body, str) else body.decode("utf-8")
        if timeout is not None:
            req["timeout_ms"] = int(timeout * 1000)
        if binary:
            req["binary"] = True
        resp = self._channel.host_call("http", req) or {}
        if binary and resp.get("body_base64") is not None:
            import base64

            resp["body_bytes"] = base64.b64decode(resp["body_base64"])
        return resp

    def http_json(self, method, url, headers=None, body=None, json_body=None, timeout=None):
        """:meth:`http`, requiring a 2xx status and returning the parsed JSON body.

        A dict or list passed as ``body`` is sent as JSON.
        """
        if json_body is None and isinstance(body, (dict, list)):
            json_body, body = body, None
        resp = self.http(method, url, headers=headers, body=body, json_body=json_body, timeout=timeout)
        status = resp.get("status", 0)
        text = resp.get("body", "")
        if status == 429:
            raise ModuleError("rate_limited", "HTTP 429: %s" % text[:200])
        if not 200 <= status < 300:
            raise ModuleError("http", "HTTP %d: %s" % (status, text[:200]))
        try:
            return json.loads(text)
        except ValueError as e:
            raise ModuleError("bad_response", "not JSON: %s" % e)

    @staticmethod
    def quote(text):
        """Percent-encode for a URL query value."""
        from urllib.parse import quote

        return quote(text, safe="")

    # -- the rest ----------------------------------------------------------

    def log(self, level, message):
        """Write to Glossa's log: trace, debug, info, warn or error."""
        self._channel.host_call("log", {"level": level, "message": str(message)})

    def cache_get(self, key, default=None):
        """A value this module stored with :meth:`cache_set`, or ``default``.

        The cache lives in Glossa's memory: it survives this process being
        stopped for idleness, not a restart of Glossa.
        """
        value = self._channel.host_call("cache_get", {"key": key})
        return default if value is None else value

    def cache_set(self, key, value, ttl_secs=None):
        """Store a JSON-serialisable value, optionally for ``ttl_secs`` seconds."""
        params = {"key": key, "value": value}
        if ttl_secs:
            params["ttl_secs"] = int(ttl_secs)
        self._channel.host_call("cache_set", params)

    def assets(self):
        """Where the files from the manifest's ``[[assets]]`` are:
        ``{"dir": "...", "files": {"name": "/abs/path", ...}}``.

        Glossa starts the module only once they are all downloaded and
        verified. The same paths are in the environment as
        ``GLOSSA_ASSETS_DIR`` and ``GLOSSA_ASSET_<NAME>``.
        """
        return self._channel.host_call("assets", None) or {"dir": "", "files": {}}

    def asset_path(self, name):
        """One asset's path, or ``None`` when the manifest has no asset by that name."""
        env = "GLOSSA_ASSET_" + "".join(c.upper() if c.isalnum() else "_" for c in name)
        if os.environ.get(env):
            return os.environ[env]
        return (self.assets().get("files") or {}).get(name)

    def option(self, key, default=None):
        """An option value (manifest default overlaid with the user's setting)."""
        value = self.options.get(key)
        return default if value in (None, "") else value


class Module:
    """Collects handlers and runs the protocol loop."""

    METHODS = ("translate", "detect", "dictionary", "speak", "recognize")

    def __init__(self):
        self._handlers = {}
        self._channel = _Channel()

    def handler(self, method):
        """Register ``fn`` for any method name: ``@mod.handler("translate")``."""

        def register(fn):
            self._handlers[method] = fn
            return fn

        return register

    def translate(self, fn):
        """``fn(req, host)``; req has text, from, to, style, glossary, options."""
        return self.handler("translate")(fn)

    def detect(self, fn):
        """``fn(req, host)`` -> ``{"lang": code, "confidence": 0..1}``."""
        return self.handler("detect")(fn)

    def dictionary(self, fn):
        """``fn(req, host)`` -> an article: headword, pronunciations, explanations, examples."""
        return self.handler("dictionary")(fn)

    def speak(self, fn):
        """``fn(req, host)`` -> ``{"audio_base64", "mime"}``; or return ``(bytes, mime)``."""
        return self.handler("speak")(fn)

    def recognize(self, fn):
        """``fn(req, host)``; req has image_base64, image_handle, lang, translate_to,
        granularity, preserve_line_breaks.

        Return ``{"text", "translation"?, "detected_lang"?, "blocks"?, "hocr"?,
        "alto"?, "transform"?}`` or just the text; see docs/modules/ocr-modules.md.
        """
        return self.handler("recognize")(fn)

    def run(self):
        """Serve requests until Glossa closes stdin."""
        while True:
            try:
                frame = self._channel.receive()
            except KeyboardInterrupt:
                return
            except Exception:
                _log_local("reading a request failed:\n" + traceback.format_exc())
                return
            if frame is None:
                return
            if "method" not in frame or "id" not in frame:
                continue
            self._serve(frame)

    def _serve(self, frame):
        req_id = frame["id"]
        method = frame.get("method")
        params = frame.get("params") or {}
        try:
            fn = self._handlers.get(method)
            if fn is None:
                raise ModuleError("unsupported", "%s is not supported by this module" % method)
            result = _normalise(method, fn(params, Host(self._channel, params)))
            reply = {"id": req_id, "result": result}
        except ModuleError as e:
            reply = {"id": req_id, "error": e.to_wire()}
        except Exception as e:  # never let one request kill the module
            reply = {
                "id": req_id,
                "error": {
                    "kind": "other",
                    "message": "%s: %s\n%s" % (type(e).__name__, e, traceback.format_exc()),
                },
            }
        try:
            self._channel.send(reply)
        except (TypeError, ValueError) as e:
            # The handler returned something JSON cannot carry.
            self._channel.send(
                {"id": req_id, "error": {"kind": "bad_response", "message": "result is not JSON-serialisable: %s" % e}}
            )


def _normalise(method, result):
    """Accept the obvious shorthands."""
    if method == "translate" and isinstance(result, str):
        return {"text": result}
    if method == "recognize" and isinstance(result, str):
        return {"text": result}
    if method == "speak" and isinstance(result, tuple) and len(result) == 2:
        import base64

        data, mime = result
        return {"audio_base64": base64.b64encode(data).decode("ascii"), "mime": mime}
    if method == "detect" and isinstance(result, str):
        return {"lang": result, "confidence": 1.0}
    return result
