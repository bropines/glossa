// pot-app plugin compatibility layer.
//
// pot-app (archived in 2026) ran plugins as `main.js` files exporting
//   async function translate(text, from, to, options)
//   async function recognize(base64, lang, options)
//   async function tts(text, lang, options)
// where `options` carries the plugin's settings and a `utils` bag modelled on
// Tauri's APIs. This file provides that bag on top of Glossa's host calls, so
// existing plugins run without changes. Only what plugins actually use is
// covered: HTTP, CryptoJS hashing/HMAC/encodings, and the plugin's own files.

var ResponseType = { JSON: 1, Text: 2, Binary: 3 };

var Body = {
  json: function (payload) { return { type: "Json", payload: payload }; },
  form: function (payload) { return { type: "Form", payload: payload }; },
  text: function (payload) { return { type: "Text", payload: payload }; },
  bytes: function (payload) { return { type: "Bytes", payload: payload }; }
};

function __bytesToBinaryString(bytes) {
  var s = "";
  for (var i = 0; i < bytes.length; i += 4096) {
    s += String.fromCharCode.apply(null, Array.prototype.slice.call(bytes, i, i + 4096));
  }
  return s;
}

function __binaryStringToBytes(s) {
  var out = new Array(s.length);
  for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 255;
  return out;
}

function __formEncode(obj) {
  var parts = [];
  for (var k in obj) {
    if (Object.prototype.hasOwnProperty.call(obj, k)) {
      parts.push(encodeURIComponent(k) + "=" + encodeURIComponent(obj[k] === undefined ? "" : obj[k]));
    }
  }
  return parts.join("&");
}

/** Tauri v1's `fetch`: resolves with {url, status, ok, headers, rawHeaders, data}. */
async function fetch(url, options) {
  options = options || {};
  var method = String(options.method || "GET").toUpperCase();
  var headers = {};
  var given = options.headers || {};
  for (var h in given) {
    if (Object.prototype.hasOwnProperty.call(given, h)) headers[h] = String(given[h]);
  }
  if (options.query) {
    var q = __formEncode(options.query);
    if (q) url += (url.indexOf("?") >= 0 ? "&" : "?") + q;
  }
  var req = { method: method, url: url, headers: headers, binary: true };
  if (options.timeout) req.timeout_ms = Math.round(options.timeout * (options.timeout < 1000 ? 1000 : 1));
  var body = options.body;
  if (body !== undefined && body !== null) {
    var payload = body.payload;
    var text = null;
    switch (body.type) {
      case "Json":
        text = JSON.stringify(payload);
        if (!headers["Content-Type"] && !headers["content-type"]) headers["Content-Type"] = "application/json";
        break;
      case "Form":
        text = __formEncode(payload);
        if (!headers["Content-Type"] && !headers["content-type"]) headers["Content-Type"] = "application/x-www-form-urlencoded";
        break;
      case "Bytes":
        req.body_base64 = btoa(__bytesToBinaryString(payload));
        break;
      default:
        text = typeof body === "string" ? body : String(payload);
    }
    if (text !== null) req.body_base64 = btoa(__bytesToBinaryString(__utf8Bytes(text)));
  }
  var resp = __call("http", req);
  var raw = __binaryStringToBytes(atob(resp.body_base64 || ""));
  var type = options.responseType || ResponseType.JSON;
  var data;
  if (type === ResponseType.Binary) {
    data = raw;
  } else {
    var str = __utf8Decode(raw);
    if (type === ResponseType.JSON) {
      try { data = JSON.parse(str); } catch (e) { data = str; }
    } else {
      data = str;
    }
  }
  var rawHeaders = {};
  for (var k in resp.headers || {}) rawHeaders[k] = [resp.headers[k]];
  return {
    url: url,
    status: resp.status,
    ok: resp.status >= 200 && resp.status < 300,
    headers: resp.headers || {},
    rawHeaders: rawHeaders,
    data: data
  };
}

/** The part of CryptoJS plugins use: hashes, HMACs, and the encoders. */
var CryptoJS = (function () {
  function WordArray(bytes) {
    this.bytes = bytes || [];
    this.sigBytes = this.bytes.length;
  }
  var Hex = {
    stringify: function (wa) { return __bytesToHex(wa.bytes); },
    parse: function (s) { return new WordArray(__hexToBytes(s)); }
  };
  var Utf8 = {
    stringify: function (wa) { return __utf8Decode(wa.bytes); },
    parse: function (s) { return new WordArray(__utf8Bytes(s)); }
  };
  var Latin1 = {
    stringify: function (wa) { return __bytesToBinaryString(wa.bytes); },
    parse: function (s) { return new WordArray(__binaryStringToBytes(s)); }
  };
  var Base64 = {
    stringify: function (wa) { return btoa(__bytesToBinaryString(wa.bytes)); },
    parse: function (s) { return new WordArray(__binaryStringToBytes(atob(s))); }
  };
  WordArray.prototype.toString = function (encoder) { return (encoder || Hex).stringify(this); };
  WordArray.prototype.concat = function (other) {
    this.bytes = this.bytes.concat(other.bytes);
    this.sigBytes = this.bytes.length;
    return this;
  };
  function bytesOf(x) {
    return x instanceof WordArray ? x.bytes : __utf8Bytes(String(x));
  }
  function hasher(alg) {
    return function (message) {
      return new WordArray(__hexToBytes(__digest(alg, __bytesToHex(bytesOf(message)))));
    };
  }
  function hmac(alg) {
    return function (message, key) {
      return new WordArray(__hexToBytes(__digest(alg, __bytesToHex(bytesOf(message)), __bytesToHex(bytesOf(key)))));
    };
  }
  return {
    MD5: hasher("md5"),
    SHA1: hasher("sha1"),
    SHA256: hasher("sha256"),
    SHA512: hasher("sha512"),
    HmacMD5: hmac("md5"),
    HmacSHA1: hmac("sha1"),
    HmacSHA256: hmac("sha256"),
    HmacSHA512: hmac("sha512"),
    enc: { Hex: Hex, Utf8: Utf8, Latin1: Latin1, Base64: Base64 },
    lib: { WordArray: { create: function (bytes) { return new WordArray(bytes); } } }
  };
})();

var __potResult = null;

/** Build the `options` argument pot-app passed to every plugin call. */
function __potOptions(config, detect) {
  return {
    config: config || {},
    detect: detect,
    setResult: function (value) { __potResult = value; },
    utils: {
      http: { fetch: fetch, Body: Body, ResponseType: ResponseType },
      CryptoJS: CryptoJS,
      readTextFile: async function (path) { return __call("local:read_text", { path: path }); },
      readBinaryFile: async function (path) {
        return __binaryStringToBytes(atob(__call("local:read_binary", { path: path })));
      },
      run: async function () {
        throw new Error("running programs is not available to plugins in Glossa");
      },
      Database: function () {
        throw new Error("the plugin database is not available in Glossa");
      },
      cacheDir: "",
      pluginDir: "",
      osType: __potOsType
    }
  };
}
