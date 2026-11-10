// Youdao Dictionary through the JSON behind dict.youdao.com.
//
//   GET https://dict.youdao.com/jsonapi?q=<word>[&le=fr]
//
// `le` is the foreign language of the lookup (en by default, fr for French);
// the other side is always Chinese. The answer carries one block per source
// dictionary; those used here:
//
//   ec  English → Chinese  { word: [{ ukphone, usphone, ukspeech, usspeech,
//                            trs: [{ tr: [{ l: { i: ["n. 房子，住宅；…"] } }] }],
//                            wfs: [{ wf: { name, value } }] }] }
//   fc  French → Chinese   { word: [{ phone, trs: [{ pos: "f.", tr: [{ l: { i: ["房屋，房子；…"] } }] }] }] }
//   ce  Chinese → English  { word: [{ phone: "fáng zǐ",
//                            trs: [{ tr: [{ l: { pos?, i: ["", { "#text": "house" }, …], "#tran": "…" } }] }] }] }
//   blng_sents_part        { "sentence-pair": [{ sentence, "sentence-translation" }] }
//
// A word Youdao does not know comes back with none of these blocks.
//
// Whatever is asked, one side is Chinese: an English or French word is
// explained in Chinese, a Chinese word in English. A lookup into any other
// language is refused as an unsupported target, so Glossa moves on to a
// dictionary that has that language instead of showing Chinese to someone
// who asked for German. Audio for
// a speech key ("house&type=1") is at /dictvoice?audio=<key>.

var SITE = "https://dict.youdao.com";

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function isChinese(code) {
  return /^zh\b/.test(String(code || ""));
}

// What the article will be written in, given what the word is in.
function checkTarget(from, to) {
  if (to === "auto") return;
  if (isChinese(from)) {
    if (to !== "en") throw new ModuleError("target_language", "Youdao explains Chinese words in English only");
  } else if (!isChinese(to)) {
    throw new ModuleError("target_language", "Youdao explains English and French words in Chinese only");
  }
}

function strip(s) {
  return String(s || "").replace(/<[^>]*>/g, "").replace(/\s+/g, " ").trim();
}

// `l.i` mixes plain strings and link objects ({ "#text": "house" }).
function words(i) {
  return (Array.isArray(i) ? i : [i]).map(function (x) {
    return typeof x === "string" ? x : (x && x["#text"]) || "";
  }).join("").trim();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  if (req.from !== "auto") checkTarget(req.from, req.to);
  var url = SITE + "/jsonapi?q=" + encodeURIComponent(word) + (req.from === "fr" ? "&le=fr" : "");
  var r = host.fetch(url, { timeout: TIMEOUT });
  if (r.status === 429) throw new ModuleError("rate_limited", "Youdao asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Youdao answered " + r.status);
  var data = r.json();
  if (!data || typeof data !== "object") throw new ModuleError("bad_response", "Youdao changed its answer: not an object");

  var article = {
    headword: word,
    pronunciations: [],
    explanations: [],
    examples: [],
    associations: [],
    link: SITE + "/result?word=" + encodeURIComponent(word) + "&lang=" + (req.from === "fr" ? "fr" : "en")
  };

  var block = data.ec || data.fc || data.ce;
  var entry = block && block.word && (Array.isArray(block.word) ? block.word[0] : block.word);
  if (!entry) throw new ModuleError("not_found", word);
  // An unknown source: the block that answered says which way this went.
  if (req.from === "auto") checkTarget(block === data.ce ? "zh" : "en", req.to);

  var rp = entry["return-phrase"];
  if (rp) article.headword = words(rp.l ? rp.l.i : rp) || word;
  if (entry.ukphone) article.pronunciations.push({ region: "UK", symbol: entry.ukphone, audio_url: entry.ukspeech ? SITE + "/dictvoice?audio=" + entry.ukspeech : null });
  if (entry.usphone) article.pronunciations.push({ region: "US", symbol: entry.usphone, audio_url: entry.usspeech ? SITE + "/dictvoice?audio=" + entry.usspeech : null });
  if (entry.phone && !entry.ukphone) article.pronunciations.push({ region: "", symbol: entry.phone, audio_url: entry.speech ? SITE + "/dictvoice?audio=" + entry.speech : null });

  (entry.trs || []).forEach(function (t) {
    (t.tr || []).forEach(function (tr) {
      var l = tr.l || {};
      var line = words(l.i);
      var pos = t.pos || l.pos || "";
      if (block === data.ce) {
        // Chinese → English: the English words, then Youdao's gloss of them.
        if (l["#tran"]) line = line ? line + " (" + strip(l["#tran"]) + ")" : strip(l["#tran"]);
      } else if (!pos) {
        // "n. 房子，住宅；…": the part of speech leads the line.
        var m = /^([a-z]+\.(?:\s*&\s*[a-z]+\.)?)\s*(.*)$/i.exec(line);
        if (m) { pos = m[1]; line = m[2]; }
      }
      // Senses within a line are separated by full-width semicolons.
      var senses = block === data.ce ? [line] : line.split(/\s*；\s*/);
      senses = senses.filter(function (s) { return s; });
      if (!senses.length) return;
      var last = article.explanations[article.explanations.length - 1];
      if (last && last.trait === pos) last.explains = last.explains.concat(senses);
      else article.explanations.push({ trait: pos, explains: senses });
    });
  });

  (entry.wfs || []).forEach(function (w) {
    var v = w.wf && w.wf.value;
    if (v && article.associations.indexOf(v) < 0) article.associations.push(v);
  });

  var pairs = (data.blng_sents_part && data.blng_sents_part["sentence-pair"]) || [];
  pairs.slice(0, 5).forEach(function (p) {
    article.examples.push({ source: strip(p.sentence || p["sentence-eng"]), target: strip(p["sentence-translation"]) });
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
dictionary = guarded("Youdao", dictionary);
