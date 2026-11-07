// Common prelude for every JavaScript dialect Glossa runs.
//
// The engine provides a handful of natives (__host_call, __b64encode,
// __b64decode, __digest, __log). This file turns them into the small set of
// browser-ish globals scripts tend to assume, and into `__call`, the one
// function every dialect's host API is built from.


// Annex B string methods older scripts rely on, where the engine lacks them.
if (typeof String.prototype.substr !== "function") {
  String.prototype.substr = function (start, length) {
    var s = String(this);
    var n = s.length;
    start = Math.trunc(Number(start)) || 0;
    if (start < 0) start = Math.max(n + start, 0);
    length = length === undefined ? n - start : Math.trunc(Number(length)) || 0;
    if (length <= 0 || start >= n) return "";
    return s.substring(start, Math.min(start + length, n));
  };
}

// Annex B escape/unescape. Old crypto code (CryptoJS's UTF-8 codec among it)
// converts strings with `unescape(encodeURIComponent(s))`.
if (typeof escape !== "function") {
  globalThis.escape = function escape(s) {
    s = String(s);
    var out = "";
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (/[A-Za-z0-9@*_+\-.\/]/.test(s.charAt(i))) out += s.charAt(i);
      else if (c < 256) out += "%" + (c < 16 ? "0" : "") + c.toString(16).toUpperCase();
      else out += "%u" + ("000" + c.toString(16).toUpperCase()).slice(-4);
    }
    return out;
  };
}
if (typeof unescape !== "function") {
  globalThis.unescape = function unescape(s) {
    return String(s).replace(/%u([0-9A-Fa-f]{4})|%([0-9A-Fa-f]{2})/g, function (m, u, b) {
      return String.fromCharCode(parseInt(u || b, 16));
    });
  };
}

/** Thrown by a module to report a typed error to Glossa. */
function ModuleError(kind, message) {
  this.name = "ModuleError";
  this.kind = kind;
  // Encoded so the kind survives the trip through the engine's error text.
  this.message = "glossa:" + kind + ":" + (message || "");
  // Where it was thrown, for the error's location.
  this.stack = new Error().stack;
}
ModuleError.prototype = Object.create(Error.prototype);
ModuleError.prototype.constructor = ModuleError;

/** Call the host; returns the result or throws a ModuleError. */
function __call(method, params) {
  var reply = JSON.parse(__host_call(method, JSON.stringify(params === undefined ? null : params)));
  if (reply && Object.prototype.hasOwnProperty.call(reply, "error")) {
    var e = reply.error || {};
    throw new ModuleError(e.kind || "other", e.message || "host call failed");
  }
  return reply ? reply.result : null;
}

// The web-standard names below are global properties, not declarations, so a
// script may declare its own (see web.js).
globalThis.console = (function () {
  function join(args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
      var a = args[i];
      if (typeof a === "string") parts.push(a);
      else {
        try { parts.push(JSON.stringify(a)); } catch (e) { parts.push(String(a)); }
      }
    }
    return parts.join(" ");
  }
  return {
    log: function () { __log("info", join(arguments)); },
    info: function () { __log("info", join(arguments)); },
    debug: function () { __log("debug", join(arguments)); },
    warn: function () { __log("warn", join(arguments)); },
    error: function () { __log("error", join(arguments)); }
  };
})();

globalThis.btoa = function btoa(s) { return __b64encode(String(s)); };
globalThis.atob = function atob(s) { return __b64decode(String(s)); };

/** UTF-8 bytes of a string, as an array of numbers. */
function __utf8Bytes(str) {
  var out = [];
  str = String(str);
  for (var i = 0; i < str.length; i++) {
    var c = str.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length) {
      var lo = str.charCodeAt(i + 1);
      if (lo >= 0xdc00 && lo <= 0xdfff) {
        c = 0x10000 + ((c - 0xd800) << 10) + (lo - 0xdc00);
        i++;
      }
    }
    if (c < 0x80) out.push(c);
    else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
    else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
  }
  return out;
}

/** Decode UTF-8 bytes back to a string. */
function __utf8Decode(bytes) {
  var s = "";
  for (var i = 0; i < bytes.length; ) {
    var b = bytes[i++];
    var c;
    if (b < 0x80) c = b;
    else if (b < 0xe0) c = ((b & 31) << 6) | (bytes[i++] & 63);
    else if (b < 0xf0) c = ((b & 15) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63);
    else c = ((b & 7) << 18) | ((bytes[i++] & 63) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63);
    if (c > 0xffff) {
      c -= 0x10000;
      s += String.fromCharCode(0xd800 + (c >> 10), 0xdc00 + (c & 1023));
    } else s += String.fromCharCode(c);
  }
  return s;
}

function __bytesToHex(bytes) {
  var h = "";
  for (var i = 0; i < bytes.length; i++) h += (bytes[i] < 16 ? "0" : "") + bytes[i].toString(16);
  return h;
}

function __hexToBytes(hex) {
  var out = [];
  for (var i = 0; i + 1 < hex.length; i += 2) out.push(parseInt(hex.substring(i, i + 2), 16));
  return out;
}

if (typeof TextEncoder === "undefined") {
  globalThis.TextEncoder = function TextEncoder() {};
  TextEncoder.prototype.encode = function (s) { return new Uint8Array(__utf8Bytes(s)); };
  globalThis.TextDecoder = function TextDecoder() {};
  TextDecoder.prototype.decode = function (b) { return __utf8Decode(Array.prototype.slice.call(b || [])); };
}

globalThis.setTimeout = function setTimeout(fn) {
  // There is no event loop between calls; run the callback on the next
  // microtask so code that defers work still makes progress.
  var args = Array.prototype.slice.call(arguments, 2);
  Promise.resolve().then(function () { fn.apply(null, args); });
  return 0;
};
globalThis.clearTimeout = function clearTimeout() {};
