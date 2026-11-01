// English Wiktionary through the Wikimedia REST API.
//
//   GET https://en.wiktionary.org/api/rest_v1/page/definition/<title>
//   → { "<language code>": [ { partOfSpeech: "Noun", language: "English",
//         definitions: [ { definition: "<html>", examples: ["<html>", …] } ] } ], … }
//   → 404 when there is no page with that title.
//
// Titles are case-sensitive ("House" and "house" are different pages), so the
// word is tried as written and then in lower case, preferring a page that has
// a section for the source language. One page holds the word in every
// language that has it: the source language's section is shown when there is
// one, otherwise (or for an unknown source) the first few sections, each
// labelled with its language. Definitions are HTML fragments, some with an
// inline <style> or a nested list of sub-senses (which also come as entries
// of their own); they are reduced to text.

var API = "https://en.wiktionary.org/api/rest_v1/page/definition/";

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
    headers: { "Api-User-Agent": "Glossa (https://github.com/bropines/glossa)" }
  });
  if (r.status === 404) return null;
  if (r.status === 429) throw new ModuleError("rate_limited", "Wikimedia asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Wiktionary answered " + r.status);
  return r.json();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var title = null;
  var page = null;
  var candidates = word.toLowerCase() === word ? [word] : [word, word.toLowerCase()];
  for (var i = 0; i < candidates.length; i++) {
    var found = fetchPage(host, candidates[i]);
    if (!found) continue;
    if (!page) {
      page = found;
      title = candidates[i];
    }
    if (req.from === "auto" || found[req.from]) {
      page = found;
      title = candidates[i];
      break;
    }
  }
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
