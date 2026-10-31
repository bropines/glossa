// Host objects the classic Windows script host offered, as far as service
// scripts used them.
//
// Classic scripts ran under Windows Script Host, where `new ActiveXObject(...)`
// reached COM components. A few scripts make their own HTTP requests that way
// (WinHttp or MSXML's XMLHTTP), usually to fetch a cookie or a token. Those
// objects are emulated here on top of Glossa's host HTTP, synchronously, as
// the originals were used. Other COM components are not available.

function ActiveXObject(progId) {
  var id = String(progId).toLowerCase();
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
