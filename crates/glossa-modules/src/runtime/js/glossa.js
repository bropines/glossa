// The host API for Glossa's own JavaScript modules.
//
// A module defines any of these functions, as globals or on `module.exports`:
//
//   translate(req, host)   req: { text, from, to, style?, glossary?, options }
//   detect(req, host)      req: { text, options }
//   dictionary(req, host)  req: { text, from, to, exact, options }
//   speak(req, host)       req: { text, lang, slow, options }
//   recognize(req, host)   req: { image_base64, lang, translate_to?, options }
//
// Each may be `async`. Return what the Glossa Module Protocol specifies for
// the method (a plain string is accepted for translate). Throw
// `new ModuleError(kind, message)` for a typed failure.
//
// `host` is synchronous on purpose: `host.fetch(url)` returns the response
// directly, so a module reads top to bottom with no callback plumbing.

var module = { exports: {} };
var exports = module.exports;

var host = {
  /** Raw protocol request: {method, url, headers, body, body_base64, timeout_ms, binary}. */
  http: function (req) {
    return __call("http", req);
  },

  /** A fetch-like convenience. Synchronous. */
  fetch: function (url, init) {
    init = init || {};
    var req = { method: String(init.method || "GET").toUpperCase(), url: String(url), headers: {} };
    var headers = init.headers || {};
    for (var k in headers) {
      if (Object.prototype.hasOwnProperty.call(headers, k)) req.headers[k] = String(headers[k]);
    }
    if (init.json !== undefined) {
      req.body = JSON.stringify(init.json);
      if (!req.headers["Content-Type"]) req.headers["Content-Type"] = "application/json";
    } else if (init.form !== undefined) {
      var parts = [];
      for (var f in init.form) parts.push(encodeURIComponent(f) + "=" + encodeURIComponent(init.form[f]));
      req.body = parts.join("&");
      if (!req.headers["Content-Type"]) req.headers["Content-Type"] = "application/x-www-form-urlencoded";
    } else if (init.body !== undefined) {
      req.body = String(init.body);
    }
    if (init.timeout) req.timeout_ms = init.timeout;
    if (init.binary) req.binary = true;
    var r = __call("http", req);
    return {
      status: r.status,
      ok: r.status >= 200 && r.status < 300,
      headers: r.headers || {},
      body: r.body,
      body_base64: r.body_base64,
      text: function () { return r.body; },
      json: function () {
        try {
          return JSON.parse(r.body);
        } catch (e) {
          throw new ModuleError("bad_response", "not JSON: " + String(r.body).substring(0, 200));
        }
      }
    };
  },

  log: function (level, message) {
    if (message === undefined) { message = level; level = "info"; }
    __log(String(level), String(message));
  },

  cache: {
    get: function (key) { return __call("cache_get", { key: String(key) }); },
    set: function (key, value, ttlSecs) {
      __call("cache_set", { key: String(key), value: value, ttl_secs: ttlSecs || 0 });
    }
  },

  /** The module's settings, also passed as req.options. */
  options: {}
};

function __glossaEntry(name) {
  if (module.exports && typeof module.exports[name] === "function") return module.exports[name];
  if (typeof globalThis[name] === "function") return globalThis[name];
  return null;
}
