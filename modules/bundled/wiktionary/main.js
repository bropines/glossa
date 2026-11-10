// English Wiktionary through the Wikimedia REST API.
//
//   GET https://en.wiktionary.org/api/rest_v1/page/definition/<title>
//   → { "<language code>": [ { partOfSpeech: "Noun", language: "English",
//         definitions: [ { definition: "<html>", examples: ["<html>", …] } ] } ], … }
//   → 404 when there is no page with that title.
//
// Titles are case-sensitive ("House" and "house" are different pages), and a
// word often arrives capitalised only because it began a sentence, where the
// capitalised page is a surname or a place ("Lamp": "A surname"). So a word
// with capitals is looked up both as written and in lower case, and the two
// pages are read as one: the lower-case page's senses first, then the
// capitalised page's. German and Luxembourgish capitalise every noun, so
// there the page as written comes first ("Essen" the meal before "essen").
// One page holds the word in every language that has it: the source language's section is shown when there is
// one, otherwise (or for an unknown source) the first few sections, each
// labelled with its language. Definitions are HTML fragments, some with an
// inline <style> or a nested list of sub-senses (which also come as entries
// of their own); they are reduced to text.

var API = "https://en.wiktionary.org/api/rest_v1/page/definition/";

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function text(html) {
  return String(html || "")
    .replace(/<style[\s\S]*?<\/style>/gi, "")
    .replace(/<(ol|ul|dl)\b[\s\S]*?<\/\1>/gi, "")
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&#(\d+);/g, function (_, n) { return String.fromCharCode(Number(n)); })
    .replace(/&#x([0-9a-f]+);/gi, function (_, n) { return String.fromCharCode(parseInt(n, 16)); })
    .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

function fetchPage(host, title) {
  var r = host.fetch(API + encodeURIComponent(title.replace(/ /g, "_")), {
    timeout: TIMEOUT,
    headers: { "Api-User-Agent": "Glossa (https://github.com/bropines/glossa)" }
  });
  if (r.status === 404) return null;
  if (r.status === 429) throw new ModuleError("rate_limited", "Wikimedia asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Wiktionary answered " + r.status);
  var page = r.json();
  if (!page || typeof page !== "object" || Array.isArray(page)) {
    throw new ModuleError("bad_response", "Wiktionary changed its answer: not a set of language sections");
  }
  return page;
}

// Languages whose spelling capitalises common nouns: the word as written
// is the better guess there.
var CAPITALISING = ["de", "lb"];

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var lower = word.toLowerCase();
  var candidates = lower === word ? [word] : [lower, word];
  if (candidates.length > 1 && CAPITALISING.indexOf(req.from) >= 0) candidates.reverse();
  // The pages that exist, merged language by language in that order. The
  // article is named after the first page with a section for the source
  // language, or the first page there is.
  var title = null;
  var page = null;
  var fallback = null;
  candidates.forEach(function (candidate) {
    var found = fetchPage(host, candidate);
    if (!found) return;
    if (!fallback) fallback = candidate;
    if (!title && (req.from === "auto" || found[req.from])) title = candidate;
    if (!page) page = {};
    Object.keys(found).forEach(function (code) {
      if (!Array.isArray(found[code])) return;
      page[code] = (page[code] || []).concat(found[code]);
    });
  });
  if (!title) title = fallback;
  if (!page) throw new ModuleError("not_found", word);

  var sections;
  var labelled = false;
  if (req.from !== "auto" && page[req.from]) {
    sections = page[req.from];
  } else {
    labelled = true;
    sections = [];
    // The widely used languages first, then the page's own order.
    var common = ["en", "ru", "de", "fr", "es", "it", "pt", "uk", "pl", "ja", "zh"];
    var codes = Object.keys(page).sort(function (a, b) {
      var x = common.indexOf(a), y = common.indexOf(b);
      return (x < 0 ? 99 : x) - (y < 0 ? 99 : y);
    });
    codes.slice(0, 4).forEach(function (code) { sections = sections.concat(page[code]); });
  }

  var article = {
    headword: title,
    explanations: [],
    examples: [],
    link: "https://en.wiktionary.org/wiki/" + encodeURIComponent(title.replace(/ /g, "_"))
  };
  sections.forEach(function (s) {
    var defs = [];
    (s.definitions || []).forEach(function (d) {
      var t = text(d.definition);
      if (!t) return;
      defs.push(t);
      (d.examples || []).slice(0, 2).forEach(function (x) {
        var e = text(x);
        if (e) article.examples.push({ source: e, target: "" });
      });
    });
    if (!defs.length) return;
    var pos = (s.partOfSpeech || "").toLowerCase();
    article.explanations.push({ trait: labelled && s.language ? s.language + ", " + pos : pos, explains: defs });
  });
  if (!article.explanations.length) throw new ModuleError("not_found", word);
  return article;
}

// A surprise in the shape of an answer (a field gone, a list that became an
// object) means the service changed, not that the user did something wrong:
// say so instead of letting a TypeError reach them.
function guarded(service, method) {
  return function (req, host) {
    try {
      return method(req, host);
    } catch (e) {
      if (e instanceof ModuleError) throw e;
      throw new ModuleError("bad_response", service + " changed its answer: " + (e && e.message ? e.message : String(e)));
    }
  };
}
dictionary = guarded("Wiktionary", dictionary);
