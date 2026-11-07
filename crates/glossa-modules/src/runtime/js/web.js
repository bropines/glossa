// Small stand-ins for the web APIs npm libraries and pot-app plugins expect:
// URLSearchParams, URL, Headers, AbortController, queueMicrotask. Evaluated
// for Glossa modules and pot-app plugins (which ran in a browser view), not
// for classic scripts. These cover what request-building code uses; they are
// not full WHATWG implementations (no IDNA, no special-scheme quirks).
//
// Each is a plain property of the global object, as in a browser, not a
// top-level `var` or `function`: those would be non-configurable, and a
// script's own top-level `const URL = …` or `class Headers {}` would then
// fail with a redeclaration error.

if (typeof queueMicrotask !== "function") {
  globalThis.queueMicrotask = function queueMicrotask(fn) { Promise.resolve().then(fn); };
}

if (typeof URLSearchParams !== "function") globalThis.URLSearchParams = (function () {
  function decode(s) { return decodeURIComponent(String(s).replace(/\+/g, " ")); }
  function encode(s) {
    return encodeURIComponent(String(s)).replace(/%20/g, "+").replace(/[!'()~]/g, function (c) {
      return "%" + c.charCodeAt(0).toString(16).toUpperCase();
    });
  }
  function USP(init) {
    this._list = [];
    if (init == null) return;
    if (typeof init === "string") {
      var s = init.charAt(0) === "?" ? init.slice(1) : init;
      var parts = s ? s.split("&") : [];
      for (var i = 0; i < parts.length; i++) {
        if (!parts[i]) continue;
        var eq = parts[i].indexOf("=");
        this._list.push(eq < 0 ? [decode(parts[i]), ""] : [decode(parts[i].slice(0, eq)), decode(parts[i].slice(eq + 1))]);
      }
    } else if (typeof init[Symbol.iterator] === "function") {
      for (var pair of init) this._list.push([String(pair[0]), String(pair[1])]);
    } else {
      for (var k in init) if (Object.prototype.hasOwnProperty.call(init, k)) this._list.push([k, String(init[k])]);
    }
  }
  USP.prototype = {
    append: function (k, v) { this._list.push([String(k), String(v)]); this._changed(); },
    delete: function (k) { k = String(k); this._list = this._list.filter(function (p) { return p[0] !== k; }); this._changed(); },
    get: function (k) { k = String(k); for (var i = 0; i < this._list.length; i++) if (this._list[i][0] === k) return this._list[i][1]; return null; },
    getAll: function (k) { k = String(k); return this._list.filter(function (p) { return p[0] === k; }).map(function (p) { return p[1]; }); },
    has: function (k) { return this.get(k) !== null; },
    set: function (k, v) {
      k = String(k);
      var i = this._list.findIndex(function (p) { return p[0] === k; });
      if (i < 0) this._list.push([k, String(v)]);
      else {
        this._list[i][1] = String(v);
        this._list = this._list.filter(function (p, j) { return j <= i || p[0] !== k; });
      }
      this._changed();
    },
    sort: function () { this._list.sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; }); this._changed(); },
    forEach: function (fn, self) { var l = this._list.slice(); for (var i = 0; i < l.length; i++) fn.call(self, l[i][1], l[i][0], this); },
    keys: function () { return this._list.map(function (p) { return p[0]; })[Symbol.iterator](); },
    values: function () { return this._list.map(function (p) { return p[1]; })[Symbol.iterator](); },
    entries: function () { return this._list.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); },
    toString: function () { return this._list.map(function (p) { return encode(p[0]) + "=" + encode(p[1]); }).join("&"); },
    _changed: function () { if (this._url) this._url._search = this._list.length ? "?" + this.toString() : ""; }
  };
  Object.defineProperty(USP.prototype, "size", { get: function () { return this._list.length; } });
  USP.prototype[Symbol.iterator] = USP.prototype.entries;
  return USP;
})();

if (typeof URL !== "function") globalThis.URL = (function () {
  var RE = /^([a-zA-Z][a-zA-Z0-9+.-]*:)(?:\/\/(?:([^:@\/?#]*)(?::([^@\/?#]*))?@)?(\[[^\]]*\]|[^:\/?#]*)(?::(\d*))?)?([^?#]*)(\?[^#]*)?(#.*)?$/;
  var DEFAULT_PORT = { "http:": "80", "https:": "443", "ws:": "80", "wss:": "443", "ftp:": "21" };
  function normalize(path) {
    var out = [];
    var segs = path.split("/");
    for (var i = 0; i < segs.length; i++) {
      if (segs[i] === "..") { if (out.length > 1) out.pop(); }
      else if (segs[i] !== ".") out.push(segs[i]);
    }
    var last = segs[segs.length - 1];
    if (last === "." || last === "..") out.push("");
    return out.join("/");
  }
  function resolve(rel, base) {
    var b = new URLCtor(base);
    var origin = b.protocol + "//" + b._auth() + b.host;
    if (rel.slice(0, 2) === "//") return b.protocol + rel;
    if (rel.charAt(0) === "/") return origin + normalize(rel);
    if (rel.charAt(0) === "?") return origin + b.pathname + rel;
    if (rel.charAt(0) === "#") return origin + b.pathname + b.search + rel;
    if (rel === "") return origin + b.pathname + b.search;
    return origin + normalize(b.pathname.replace(/[^\/]*$/, "") + rel);
  }
  function URLCtor(url, base) {
    url = String(url).trim();
    if (base !== undefined && !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(url)) {
      url = resolve(url, String(base));
    }
    var m = RE.exec(url);
    if (!m) throw new TypeError("Invalid URL: " + url);
    this.protocol = m[1].toLowerCase();
    this.username = m[2] || "";
    this.password = m[3] || "";
    this.hostname = (m[4] || "").toLowerCase();
    this.port = m[5] && m[5] !== DEFAULT_PORT[this.protocol] ? m[5] : "";
    var hier = m[4] !== undefined;
    this.pathname = hier ? normalize(m[6] || "/") || "/" : m[6];
    this._search = m[7] && m[7] !== "?" ? m[7] : "";
    this.hash = m[8] && m[8] !== "#" ? m[8] : "";
    this.searchParams = new globalThis.URLSearchParams(this._search);
    this.searchParams._url = this;
  }
  URLCtor.prototype = {
    _auth: function () { return this.username ? this.username + (this.password ? ":" + this.password : "") + "@" : ""; },
    toString: function () { return this.href; },
    toJSON: function () { return this.href; }
  };
  Object.defineProperties(URLCtor.prototype, {
    host: { get: function () { return this.hostname + (this.port ? ":" + this.port : ""); } },
    origin: { get: function () { return this.protocol + "//" + this.host; } },
    search: {
      get: function () { return this._search; },
      set: function (v) {
        v = String(v);
        this._search = v && v !== "?" ? (v.charAt(0) === "?" ? v : "?" + v) : "";
        this.searchParams = new globalThis.URLSearchParams(this._search);
        this.searchParams._url = this;
      }
    },
    href: {
      get: function () {
        var hier = this.hostname || this.protocol in DEFAULT_PORT || this.protocol === "file:";
        return this.protocol + (hier ? "//" + this._auth() + this.host : "") + this.pathname + this._search + this.hash;
      }
    }
  });
  URLCtor.canParse = function (url, base) { try { new URLCtor(url, base); return true; } catch (e) { return false; } };
  return URLCtor;
})();

if (typeof Headers !== "function") globalThis.Headers = (function () {
  function H(init) {
    // No prototype: header names such as "constructor" or "__proto__" are
    // plain keys here.
    this._map = Object.create(null);
    if (!init) return;
    if (init instanceof H) init = init._map;
    if (typeof init[Symbol.iterator] === "function") {
      for (var pair of init) this.append(pair[0], pair[1]);
    } else {
      for (var k in init) if (Object.prototype.hasOwnProperty.call(init, k)) this.append(k, init[k]);
    }
  }
  H.prototype = {
    append: function (k, v) { k = String(k).toLowerCase(); this._map[k] = k in this._map ? this._map[k] + ", " + v : String(v); },
    set: function (k, v) { this._map[String(k).toLowerCase()] = String(v); },
    get: function (k) { k = String(k).toLowerCase(); return k in this._map ? this._map[k] : null; },
    has: function (k) { return String(k).toLowerCase() in this._map; },
    delete: function (k) { delete this._map[String(k).toLowerCase()]; },
    forEach: function (fn, self) { for (var k in this._map) fn.call(self, this._map[k], k, this); },
    entries: function () { var m = this._map; return Object.keys(m).sort().map(function (k) { return [k, m[k]]; })[Symbol.iterator](); },
    keys: function () { return Object.keys(this._map).sort()[Symbol.iterator](); },
    values: function () { var m = this._map; return Object.keys(m).sort().map(function (k) { return m[k]; })[Symbol.iterator](); }
  };
  H.prototype[Symbol.iterator] = H.prototype.entries;
  return H;
})();

// Requests finish synchronously, so there is never anything to abort; the
// controller exists because libraries create one and pass its signal along.
if (typeof AbortController !== "function") {
  globalThis.AbortController = function AbortController() {
    this.signal = { aborted: false, reason: undefined, onabort: null,
      addEventListener: function () {}, removeEventListener: function () {},
      throwIfAborted: function () { if (this.aborted) throw this.reason; } };
  };
  AbortController.prototype.abort = function (reason) {
    this.signal.aborted = true;
    this.signal.reason = reason === undefined ? new Error("aborted") : reason;
  };
}
