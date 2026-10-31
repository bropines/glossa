// QTranslate service-script compatibility layer.
//
// Classic service scripts call a shared library of helpers that the original
// program shipped as Common.js. This file is an independent implementation of
// that library's *interface* — the same names, arguments and observable
// behaviour, written from scratch — so scripts written for the classic program
// run here unchanged. When a user points Glossa at a classic installation that
// still has its own Common.js, that file is used instead of this one.
//
// The host drives a script like this (see legacy.rs):
//   request  = serviceTranslateRequest(text, fromId, toId)    -> RequestData
//   ...host performs the HTTP request...
//   response = serviceTranslateResponse(text, body, fromId, toId) -> ResponseData
// Language arguments are positions in SupportedLanguages, which the script
// fills with its own codes (or -1 where it has none).

var Const = { MAX_URI_LEN: 1800, MAX_SOURCE_LEN: 5000, NL: "\r\n", NL2: "\r\n\r\n" };
var Capability = { TRANSLATE: 1, DETECT_LANGUAGE: 2, LISTEN: 4, DICTIONARY: 8 };
var HttpMethod = { UNDEFINED: 0, GET: 1, POST: 2 };
var CodePage = { WINDOWS1251: 1251, UTF8: 65001, ISO8859_1: 28591 };

var isArray = Array.isArray;

function ServiceHeader(id, name, info, capabilities) {
  this.id = id;
  this.name = name || "";
  this.info = info || "";
  this.capabilities = capabilities;
}

function RequestData(method, uri, data, headers, codepage, responseHandler) {
  this.method = method;
  this.uri = uri || "";
  this.data = data || "";
  if (headers) this.headers = headers;
  else if (method === HttpMethod.GET) this.headers = getHeader();
  else if (method === HttpMethod.POST) this.headers = postHeader();
  else this.headers = "";
  this.codepage = codepage || CodePage.UTF8;
  this.responseHandler = responseHandler || "";
}

function ResponseData(translation, sourceLanguage, translationLanguage, data, nextRequestHandler) {
  this.translation = translation || "";
  this.sourceLanguage = sourceLanguage;
  this.translationLanguage = translationLanguage;
  this.data = data || "";
  this.nextRequestHandler = nextRequestHandler || "";
}

// Host-provided settings (LanguageCode, PreferredDomain, user options...).
// The host overwrites this object before each call; addOption declares a
// default the user can change in the service's settings.
var Options = {};
var __declaredOptions = [];
function addOption(name, defaultValue) {
  __declaredOptions.push([name, defaultValue]);
  if (!Object.prototype.hasOwnProperty.call(Options, name)) Options[name] = defaultValue;
}

function getHeader() {
  var lang = Options.LanguageCode || "en";
  return [
    "Accept: */*",
    "Accept-Language: " + lang + ";q=0.8,en-US;q=0.6,en;q=0.4",
    "Accept-Encoding: gzip,deflate",
    "Accept-Charset: utf-8"
  ].join(Const.NL);
}

function postHeader(json) {
  var type = json ? "application/json" : "application/x-www-form-urlencoded";
  return getHeader() + Const.NL + "Content-Type: " + type + "; charset=utf-8";
}

function encodeUriParam(s) {
  return s ? encodeURIComponent(s) : "";
}

// For query strings: cap the encoded length, cutting at the last encoded
// space so a word is not split.
function encodeGetParam(s) {
  var encoded = encodeUriParam(s);
  if (encoded.length <= Const.MAX_URI_LEN) return encoded;
  encoded = encoded.substring(0, Const.MAX_URI_LEN);
  var space = encoded.lastIndexOf("%20");
  return space > 0 ? encoded.substring(0, space) : encoded;
}

function encodePostParam(s) {
  return encodeUriParam(s);
}

function prepareSource(s) {
  return String(s || "").replace(/\r\n?/g, "\n");
}

function limitSource(s, max) {
  max = max || Const.MAX_SOURCE_LEN;
  return s && s.length > max ? s.substring(0, max) : s;
}

// format("{0} and {1}", a, b)
function format(template) {
  var out = String(template);
  for (var i = 1; i < arguments.length; i++) {
    out = out.split("{" + (i - 1) + "}").join(String(arguments[i]));
  }
  return out;
}

// Find a string or a regular expression; {index, length} or undefined.
function stringFind(haystack, needle) {
  if (!needle) return undefined;
  if (typeof needle === "string") {
    var at = haystack.indexOf(needle);
    return at < 0 ? undefined : { index: at, length: needle.length };
  }
  var m = haystack.match(needle);
  return m ? { index: m.index, length: m[0].length } : undefined;
}

// The text between `start` and `end`; the flags say whether to include the
// markers themselves.
function stringFindSub(text, start, includeStart, end, includeEnd) {
  if (!text || !start || !end) return "";
  var s = stringFind(text, start);
  if (!s) return "";
  var rest = text.substring(includeStart ? s.index : s.index + s.length);
  var e = stringFind(rest, end);
  if (!e) return "";
  return rest.substring(0, includeEnd ? e.index + e.length : e.index);
}

function trimString(s) {
  return s ? String(s).replace(/^\s+|\s+$/g, "") : "";
}

function startsWith(s, prefix) {
  return String(s).substring(0, prefix.length) === prefix;
}

function endsWith(s, suffix) {
  s = String(s);
  return s.length >= suffix.length && s.substring(s.length - suffix.length) === suffix;
}

function removeIfStartsWith(s, prefix) {
  return startsWith(s, prefix) ? String(s).substring(prefix.length) : s;
}

function stringSplit(s, separator) {
  return String(s).split(separator);
}

function unquoteHtml(s) {
  if (!s) return "";
  return String(s)
    .replace(/&amp;?/g, "&")
    .replace(/&lt;?/g, "<")
    .replace(/&gt;?/g, ">")
    .replace(/&quot;?/g, '"')
    .replace(/&#0?39;/g, "'");
}

// Collapse runs of blank lines to one, normalise to CRLF, trim.
function removeEmptyLines(s) {
  if (!s) return "";
  var lines = String(s).replace(/\r/g, "").split("\n");
  for (var i = 0; i < lines.length; i++) {
    if (!/\S/.test(lines[i])) lines[i] = "";
  }
  return trimString(lines.join("\n").replace(/\n{3,}/g, "\n\n").replace(/\n/g, Const.NL));
}

function stripHtml(s) {
  if (!s) return "";
  s = String(s)
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/<(script|style)[\s\S]*?<\/(script|style)>/gi, "")
    .replace(/<br\s*\/?>/gi, Const.NL)
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&#x27;/gi, "'")
    .replace(/ {2,}/g, " ");
  return removeEmptyLines(s);
}

function regExpRemove(s, names, pattern) {
  if (!names) return s;
  return String(s).replace(new RegExp(format(pattern, names.join("|")), "g"), "");
}

function removeAttributes(s, names) {
  return regExpRemove(s, names, " ({0})(?:\\s*=\\s*(?:\"[^\"]*\"|'[^']*'|[^>\\s]+))?");
}

function removeTags(s, names) {
  return regExpRemove(s, names, "</?({0})[^>]*>");
}

function removeElements(s, names) {
  return regExpRemove(s, names, "<\\s*({0})[^>]*>[\\s\\S]*?<\\s*/\\s*({0})>");
}

// Make root-relative and protocol-relative links absolute against `base`.
function updateHtmlLinks(html, base) {
  if (!html) return "";
  var scheme = startsWith(base, "https") ? "https://" : "http://";
  var host = removeIfStartsWith(removeIfStartsWith(base, "https://"), "http://");
  if (!endsWith(host, "/")) host += "/";
  return String(html).replace(/(href|src)=(["'])\/(\/?)/gi, function (m, attr, q, second) {
    return attr + "=" + q + (second ? scheme : scheme + host);
  });
}

// -- languages ---------------------------------------------------------------
var SupportedLanguages = null;
var UNKNOWN_LANGUAGE_CODE = -1;
var UNKNOWN_LANGUAGE = 0;
var AUTO_DETECT_LANGUAGE = 1;
var ENGLISH_LANGUAGE = 17;

function isLanguage(id) {
  return id > AUTO_DETECT_LANGUAGE && id < SupportedLanguages.length;
}

function codeFromLanguage(id) {
  return id === AUTO_DETECT_LANGUAGE || isLanguage(id) ? SupportedLanguages[id] : UNKNOWN_LANGUAGE_CODE;
}

function languageFromCode(code) {
  if (SupportedLanguages[ENGLISH_LANGUAGE] === code) return ENGLISH_LANGUAGE;
  for (var i = AUTO_DETECT_LANGUAGE; i < SupportedLanguages.length; i++) {
    if (SupportedLanguages[i] === code) return i;
  }
  return UNKNOWN_LANGUAGE;
}

function usesAutoDetectCode() {
  return SupportedLanguages[AUTO_DETECT_LANGUAGE] !== UNKNOWN_LANGUAGE_CODE;
}

// Classic scripts parse responses that are not always strict JSON (JavaScript
// object literals, single quotes, trailing commas), so evaluate as an
// expression rather than JSON.parse.
function parseJSON(text) {
  try {
    return JSON.parse(text);
  } catch (e) {
    return new Function("return " + text)();
  }
}

function stringifyJSON(value) {
  return JSON.stringify(value);
}
