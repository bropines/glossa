// Host objects the classic Windows script host offered, as far as service
// scripts used them.
//
// Classic scripts ran under Windows Script Host, where `new ActiveXObject(...)`
// reached COM components. A few scripts make their own HTTP requests that way
// (WinHttp or MSXML's XMLHTTP), usually to fetch a cookie or a token. Those
// objects are emulated here on top of Glossa's host HTTP, synchronously, as
// the originals were used, along with a minimal "htmlfile" document. Other
// COM components are not available.

// Helpers the classic host offered scripts besides Common.js. Only what
// scripts are seen to use: `Utils.md5(text)`, the lowercase hex MD5 of the
// text's UTF-8 bytes.
var Utils = {
  md5: function (s) { return __digest("md5", __bytesToHex(__utf8Bytes(String(s)))); }
};

// `new ActiveXObject("htmlfile")`: Internet Explorer's HTML document, which
// a few dictionary scripts use to cut the page header and footer out of an
// article. This is a small string-based stand-in covering that use:
// write(), getElementById(), removing an element through its parent,
// setting inline styles, and reading the markup back.
function __HtmlFile() {
  var doc = this;
  var html = "";
  var elements = [];
  var VOID = /^(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)$/i;

  function escapeRe(s) { return String(s).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }

  // [start, openEnd, end) of the element with this id, or null.
  function locate(id) {
    var open = new RegExp("<([A-Za-z][\\w:-]*)\\b[^>]*?\\sid\\s*=\\s*([\"']?)" + escapeRe(id) + "\\2(?=[\\s/>])[^>]*>", "i");
    var m = open.exec(html);
    if (!m) return null;
    var start = m.index, openEnd = m.index + m[0].length, tag = m[1];
    if (VOID.test(tag) || /\/>$/.test(m[0])) return { start: start, openEnd: openEnd, end: openEnd };
    var re = new RegExp("<(/?)" + escapeRe(tag) + "\\b[^>]*>", "gi");
    re.lastIndex = openEnd;
    var depth = 1, t;
    while ((t = re.exec(html))) {
      if (t[1]) depth--;
      else if (!/\/>$/.test(t[0])) depth++;
      if (depth === 0) return { start: start, openEnd: openEnd, end: t.index + t[0].length };
    }
    return { start: start, openEnd: openEnd, end: html.length };
  }

  function applyStyles() {
    for (var i = 0; i < elements.length; i++) {
      var el = elements[i], decl = [];
      for (var k in el.style) {
        if (Object.prototype.hasOwnProperty.call(el.style, k) && typeof el.style[k] !== "function") {
          decl.push(k.replace(/[A-Z]/g, function (c) { return "-" + c.toLowerCase(); }) + ":" + el.style[k]);
        }
      }
      if (!decl.length) continue;
      var at = locate(el.id);
      if (!at) continue;
      var tagText = html.substring(at.start, at.openEnd);
      var styled = /\sstyle\s*=\s*"[^"]*"/i.test(tagText)
        ? tagText.replace(/(\sstyle\s*=\s*")([^"]*)"/i, function (m, a, b) { return a + b.replace(/;?\s*$/, ";") + decl.join(";") + '"'; })
        : tagText.replace(/^(<[A-Za-z][\w:-]*)/, '$1 style="' + decl.join(";") + '"');
      html = html.substring(0, at.start) + styled + html.substring(at.openEnd);
      el.style = {};
    }
  }

  function element(id) {
    var el = { id: id, style: {} };
    el.parentNode = {
      removeChild: function (child) {
        var at = locate(child.id);
        if (at) html = html.substring(0, at.start) + html.substring(at.end);
        return child;
      }
    };
    el.getAttribute = function (name) {
      var at = locate(id);
      if (!at) return null;
      var m = new RegExp("\\s" + escapeRe(name) + "\\s*=\\s*(\"([^\"]*)\"|'([^']*)'|([^\\s>]+))", "i").exec(html.substring(at.start, at.openEnd));
      return m ? (m[2] !== undefined ? m[2] : m[3] !== undefined ? m[3] : m[4]) : null;
    };
    Object.defineProperty(el, "outerHTML", { get: function () { applyStyles(); var at = locate(id); return at ? html.substring(at.start, at.end) : ""; } });
    Object.defineProperty(el, "innerHTML", { get: function () {
      applyStyles();
      var at = locate(id);
      if (!at || at.end === at.openEnd) return "";
      return html.substring(at.openEnd, html.lastIndexOf("<", at.end - 1));
    } });
    Object.defineProperty(el, "value", { get: function () { var v = el.getAttribute("value"); return v !== null ? v : unquoteHtml(el.innerHTML); } });
    elements.push(el);
    return el;
  }

  this.open = this.close = function () {};
  this.write = this.writeln = function () {
    for (var i = 0; i < arguments.length; i++) html += String(arguments[i]);
  };
  this.getElementById = function (id) { return locate(id) ? element(String(id)) : null; };
  this.documentElement = {};
  Object.defineProperty(this.documentElement, "outerHTML", { get: function () { applyStyles(); return html; } });
  this.body = {};
  Object.defineProperty(this.body, "innerHTML", { get: function () {
    applyStyles();
    var m = /<body\b[^>]*>([\s\S]*?)(<\/body>|$)/i.exec(html);
    return m ? m[1] : html;
  } });
}

function ActiveXObject(progId) {
  var id = String(progId).toLowerCase();
  if (id === "htmlfile") return new __HtmlFile();
  var http = [
    "winhttp.winhttprequest.5.1",
    "msxml2.xmlhttp", "msxml2.xmlhttp.3.0", "msxml2.xmlhttp.6.0",
    "microsoft.xmlhttp",
    "msxml2.serverxmlhttp", "msxml2.serverxmlhttp.6.0"
  ];
  if (http.indexOf(id) < 0) {
    throw new Error("ActiveXObject(\"" + progId + "\") is not available in Glossa");
  }
  var self = this;
  var method = "GET", url = "", headers = {}, response = null;

  function header(name) {
    if (!response) return "";
    var want = String(name).toLowerCase();
    for (var k in response.headers) {
      if (k.toLowerCase() === want) return response.headers[k];
    }
    return "";
  }

  this.readyState = 0;
  this.open = this.Open = function (m, u) {
    method = String(m || "GET").toUpperCase();
    url = String(u);
    headers = {};
    response = null;
    self.readyState = 1;
  };
  this.setRequestHeader = this.SetRequestHeader = function (k, v) { headers[k] = String(v); };
  this.setTimeouts = this.SetTimeouts = function () {};
  this.setOption = this.SetOption = function () {};
  this.send = this.Send = function (body) {
    var req = { method: method, url: url, headers: headers };
    if (body !== undefined && body !== null && body !== "") req.body = String(body);
    response = __call("http", req);
    self.status = self.Status = response.status;
    self.statusText = self.StatusText = String(response.status);
    self.responseText = self.ResponseText = response.body;
    self.readyState = 4;
    if (typeof self.onreadystatechange === "function") self.onreadystatechange();
  };
  this.getResponseHeader = this.GetResponseHeader = header;
  this.getAllResponseHeaders = this.GetAllResponseHeaders = function () {
    if (!response) return "";
    var out = [];
    for (var k in response.headers) out.push(k + ": " + response.headers[k]);
    return out.join("\r\n");
  };
  this.waitForResponse = this.WaitForResponse = function () { return true; };
}
