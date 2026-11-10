// Oxford Learner's Dictionaries, read from the public entry pages.
//
//   GET https://www.oxfordlearnersdictionaries.com/search/english/?q=<word>
//
// redirects to the first entry, /definition/english/<word>_1. An unknown word
// lands on a spelling-suggestion page instead, without an entry. The parts of
// an entry page used here:
//
//   <div id="entryContent"> … <h1 class="headword">house</h1>
//     <span class="pos">noun</span>
//     <div class="phons_br"><div … data-src-mp3="…"></div><span class="phon">/haʊs/</span></div>
//     <div class="phons_n_am"> … same, American …
//     <li class="sense" …> <span class="def">a building for people to live in…</span>
//        <ul class="examples"><li><span class="x">We live in a two-bedroom house.</span></li> …
//     <div class="idioms"> …                  (not used)
//   <div id="relatedentries"> … <a href="…/definition/english/house_2"><span class='arl1'>house
//        <pos-g><pos>verb</pos></pos-g></span></a>  (arl1 = same word, another part of speech)
//
// The same word's other entries (house, verb) are fetched too, up to two.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

var SITE = "https://www.oxfordlearnersdictionaries.com";

function text(html) {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&#(\d+);/g, function (_, n) { return String.fromCharCode(Number(n)); })
    .replace(/&#x([0-9a-f]+);/gi, function (_, n) { return String.fromCharCode(parseInt(n, 16)); })
    .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

// The inner HTML of every element `<tag class="cls" …>` in `html`, with
// nested elements of the same tag counted so the right closing tag is found.
function elements(html, tag, cls) {
  var out = [];
  var open = new RegExp("<" + tag + "\\b[^>]*\\bclass=[\"']" + cls + "[\"'][^>]*>", "g");
  var m;
  while ((m = open.exec(html)) !== null) {
    var any = new RegExp("<(/?)" + tag + "\\b[^>]*>", "g");
    any.lastIndex = m.index + m[0].length;
    var depth = 1, t, end = html.length;
    while ((t = any.exec(html)) !== null) {
      depth += t[1] ? -1 : 1;
      if (depth === 0) { end = t.index; break; }
    }
    out.push(html.slice(m.index + m[0].length, end));
    open.lastIndex = end;
  }
  return out;
}

function between(html, start, stops) {
  var i = html.indexOf(start);
  if (i < 0) return null;
  var rest = html.slice(i);
  stops.forEach(function (s) {
    var j = rest.indexOf(s, 1);
    if (j > 0) rest = rest.slice(0, j);
  });
  return rest;
}

function load(host, url) {
  var r = host.fetch(url, { timeout: TIMEOUT });
  if (r.status === 404) return null;
  if (r.status === 429) throw new ModuleError("rate_limited", "Oxford asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Oxford answered " + r.status);
  return String(r.body || "");
}

// One entry page → { headword, pos, pronunciations, senses: [{ def, examples }] }.
function entry(page, examplesPerSense) {
  var content = between(page, 'id="entryContent"', ['id="relatedentries"', 'id="rightcolumn"']);
  if (!content || content.indexOf('class="headword"') < 0) return null;
  var main = between(content, 'id="entryContent"', ['<div class="idioms"', '<span class="idm-g"']) || content;
  var head = elements(main, "h1", "headword")[0];
  var pos = elements(main, "span", "pos")[0];
  var out = { headword: head ? text(head) : "", pos: pos ? text(pos) : "", pronunciations: [], senses: [] };

  [["phons_br", "UK"], ["phons_n_am", "US"]].forEach(function (p) {
    var block = elements(main, "div", p[0])[0];
    if (!block) return;
    var phon = elements(block, "span", "phon")[0];
    var mp3 = /data-src-mp3="([^"]+)"/.exec(block);
    if (phon) out.pronunciations.push({ region: p[1], symbol: text(phon), audio_url: mp3 ? mp3[1] : null });
  });

  main.split(/<li class="sense"/).slice(1).forEach(function (sense) {
    var def = elements(sense, "span", "def")[0];
    if (!def) return;
    out.senses.push({
      def: text(def),
      examples: elements(sense, "span", "x").slice(0, examplesPerSense).map(text)
    });
  });
  return out;
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var perSense = req.options.examples === undefined ? 2 : Number(req.options.examples);
  var page = load(host, SITE + "/search/english/?q=" + encodeURIComponent(word));
  var first = page && entry(page, perSense);
  if (!first) throw new ModuleError("not_found", word);

  var entries = [first];
  var related = between(page, 'id="relatedentries"', ['id="rightcolumn"', "</dl>"]) || "";
  var associations = [];
  var link = /<a href="([^"]+)"[^>]*>\s*<span class='arl(\d)'>([\s\S]*?)<\/span><\/a>/g;
  var m;
  while ((m = link.exec(related)) !== null) {
    var name = text(m[3].replace(/<pos-g[\s\S]*$/, ""));
    if (m[2] === "1" && name.toLowerCase() === first.headword.toLowerCase() && entries.length < 3) {
      var more = load(host, m[1]);
      var e = more && entry(more, perSense);
      if (e) entries.push(e);
    } else if (associations.indexOf(name) < 0 && associations.length < 12) {
      associations.push(name);
    }
  }

  var article = {
    headword: first.headword || word,
    pronunciations: first.pronunciations,
    explanations: [],
    examples: [],
    associations: associations,
    link: SITE + "/search/english/?q=" + encodeURIComponent(word)
  };
  entries.forEach(function (e) {
    if (!e.senses.length) return;
    article.explanations.push({ trait: e.pos, explains: e.senses.map(function (s) { return s.def; }) });
    e.senses.forEach(function (s) {
      s.examples.forEach(function (x) { article.examples.push({ source: x, target: "" }); });
    });
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
dictionary = guarded("Oxford", dictionary);
