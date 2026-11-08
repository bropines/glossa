// The host API for Glossa's own JavaScript modules.
//
// A module defines any of these functions, as globals or on `module.exports`:
//
//   translate(req, host)   req: { text, from, to, style?, glossary?, options }
//   detect(req, host)      req: { text, options }
//   dictionary(req, host)  req: { text, from, to, exact, options }
//   speak(req, host)       req: { text, lang, slow, options }
//   recognize(req, host)   req: { image_base64, image_handle?, lang, translate_to?,
//                                 granularity?, preserve_line_breaks, options }
//
// and, for each `[[actions]]` entry, a function named by its `handler` (the
// action's id by default):
//
//   list_models(ev, host)  ev: { action, handler, event, state, options, ui_lang }
//                          -> { options?, choices?, errors?, notice?, notice_level? }
//
// Each may be `async`. Return what the Glossa Module Protocol specifies for
// the method (a plain string is accepted for translate, and as the notice of
// an action; nothing at all is a fine answer to an action). Throw
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

  /**
   * Where the files from the manifest's [[assets]] are:
   * { dir: "…", files: { name: "/abs/path", … } }. The module only runs
   * once they are all downloaded and verified.
   */
  assets: function () { return __call("assets", null); },

  /** One asset's path, or null when the manifest declares none by that name. */
  assetPath: function (name) {
    var files = __call("assets", null).files || {};
    return Object.prototype.hasOwnProperty.call(files, name) ? files[name] : null;
  },

  /** The module's settings, also passed as req.options. */
  options: {},

  /** The module's [strings] in the interface language (set for actions). */
  strings: {},

  /**
   * The module's string `key` from `[strings]` in locales/<lang>.toml, in
   * Glossa's interface language with English as the fallback, `{0}`, `{1}`…
   * replaced by the further arguments. The key itself when there is none.
   */
  t: function (key) {
    var s = Object.prototype.hasOwnProperty.call(host.strings, key) ? String(host.strings[key]) : String(key);
    for (var i = 1; i < arguments.length; i++) s = s.split("{" + (i - 1) + "}").join(String(arguments[i]));
    return s;
  },

  /**
   * The image API (see docs/modules/ocr-modules.md). Each operation takes
   * the image (a handle number, an info object returned by another
   * operation, or nothing when the parameters carry `handle` or
   * `data_base64`) and its parameters, and returns an info object:
   *
   *   var g = host.image.grayscale(req.image_handle);
   *   var big = host.image.scaleToMinHeight(g, { min_height: 32 });
   *   var jpeg = host.image.encode(big, { format: "jpeg", quality: 85 }).data_base64;
   *   // boxes found in `big` -> the input image:
   *   var box = host.image.mapBoxBack({ x: 10, y: 4, width: 50, height: 12 }, big.transform);
   */
  image: (function () {
    function args(src, params) {
      var p = {}, k;
      if (typeof src === "number") {
        p.handle = src;
      } else if (src && typeof src === "object" && "handle" in src && "transform" in src) {
        p.handle = src.handle;
      } else if (src && typeof src === "object") {
        for (k in src) p[k] = src[k];
      }
      if (params) for (k in params) p[k] = params[k];
      return p;
    }
    var api = {
      /** Any operation by name: host.image.call("crop", { handle: 1, x: 0, … }). */
      call: function (op, params) {
        return __call("image." + op, params || {});
      },
      /** Register encoded bytes (base64) and get a handle. */
      fromBase64: function (b64) {
        return __call("image.decode", { data_base64: String(b64) });
      },
      /**
       * A box of a derived image as the axis-aligned box in the original.
       * `box` is {x, y, width, height} or [x0, y0, x1, y1] (returned in the
       * same shape); `transform` is an info object's `transform`.
       */
      mapBoxBack: function (box, transform) {
        var t = transform && transform.transform ? transform.transform : transform;
        if (!t) return box;
        var arr = Array.isArray(box);
        var x0 = arr ? box[0] : box.x, y0 = arr ? box[1] : box.y;
        var x1 = arr ? box[2] : box.x + box.width, y1 = arr ? box[3] : box.y + box.height;
        var pts = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(function (p) {
          return [t[0] * p[0] + t[1] * p[1] + t[2], t[3] * p[0] + t[4] * p[1] + t[5]];
        });
        var xs = pts.map(function (p) { return p[0]; }), ys = pts.map(function (p) { return p[1]; });
        var a = Math.min.apply(null, xs), b = Math.min.apply(null, ys);
        var c = Math.max.apply(null, xs), d = Math.max.apply(null, ys);
        return arr ? [a, b, c, d] : { x: a, y: b, width: c - a, height: d - b };
      }
    };
    var ops = ["decode", "info", "encode", "to_base64", "pixels", "free", "new", "crop", "resize",
      "scale_to_min_height", "rotate", "flip", "grayscale", "invert", "threshold", "adaptive_threshold",
      "adjust", "normalize", "auto_levels", "blur", "sharpen", "pad", "deskew", "split_tiles", "paste",
      "composite", "draw_rect"];
    ops.forEach(function (op) {
      var f = function (src, params) { return __call("image." + op, args(src, params)); };
      api[op] = f;
      var camel = op.replace(/_([a-z])/g, function (_, c) { return c.toUpperCase(); });
      if (camel !== op) api[camel] = f;
    });
    return api;
  })()
};

/**
 * The browser's `fetch`, made through host.http, for code written against it
 * (libraries bundled from npm). The request runs synchronously, so the
 * returned promise is already settled. The response body is text, decoded by
 * its charset; pass `binary: true` (a Glossa extension) to get exact bytes
 * from `arrayBuffer()`. Only hosts in `permissions.network` are reachable.
 */
globalThis.fetch = function fetch(input, init) {
  try {
    init = init || {};
    var isRequest = input && typeof input === "object" && !(input instanceof globalThis.URL);
    var url = isRequest ? input.url : String(input);
    var headers = new globalThis.Headers(init.headers || (isRequest ? input.headers : undefined));
    var req = { method: String(init.method || (isRequest && input.method) || "GET").toUpperCase(), url: url, headers: {} };
    var body = init.body;
    if (body != null) {
      if (body instanceof globalThis.URLSearchParams) {
        if (!headers.has("content-type")) headers.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8");
        req.body = body.toString();
      } else if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) {
        var bytes = new Uint8Array(body instanceof ArrayBuffer ? body : body.buffer, body.byteOffset || 0, body.byteLength);
        var bin = "";
        for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
        req.body_base64 = globalThis.btoa(bin);
      } else {
        if (!headers.has("content-type")) headers.set("content-type", "text/plain;charset=UTF-8");
        req.body = String(body);
      }
    }
    headers.forEach(function (v, k) { req.headers[k] = v; });
    if (init.timeout) req.timeout_ms = init.timeout;
    if (init.binary) req.binary = true;
    return Promise.resolve(new __Response(__call("http", req), url, !!init.binary));
  } catch (e) {
    return Promise.reject(e);
  }
};

function __Response(r, url, binary) {
  this._r = r;
  this._binary = binary;
  this.status = r.status;
  this.ok = r.status >= 200 && r.status < 300;
  this.statusText = "";
  this.headers = new globalThis.Headers(r.headers || {});
  this.url = url;
  this.redirected = false;
  this.type = "basic";
  this.bodyUsed = false;
}
__Response.prototype = {
  _bytes: function () {
    if (!this._binary) return new Uint8Array(__utf8Bytes(this._r.body || ""));
    var bin = globalThis.atob(this._r.body_base64 || "");
    var out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  },
  text: function () {
    this.bodyUsed = true;
    return Promise.resolve(this._binary ? __utf8Decode(this._bytes()) : this._r.body || "");
  },
  json: function () { return this.text().then(JSON.parse); },
  arrayBuffer: function () { this.bodyUsed = true; return Promise.resolve(this._bytes().buffer); },
  bytes: function () { this.bodyUsed = true; return Promise.resolve(this._bytes()); },
  clone: function () { return new __Response(this._r, this.url, this._binary); }
};

function __glossaEntry(name) {
  if (module.exports && typeof module.exports[name] === "function") return module.exports[name];
  if (typeof globalThis[name] === "function") return globalThis[name];
  return null;
}
