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

__all__ = ["Module", "ModuleError", "Host", "PROTOCOL_VERSION"]
__version__ = "1.0.0"

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


class Host:
    """What a handler may ask of Glossa during one request."""

    def __init__(self, channel, params):
        self._channel = channel
        self.options = (params or {}).get("options") or {}

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
        """``fn(req, host)``; req has image_base64, lang, translate_to, preserve_line_breaks.

        Return ``{"text", "translation"?, "detected_lang"?}``.
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
