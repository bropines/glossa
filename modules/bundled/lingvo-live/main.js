// ABBYY Lingvo Live, through the word-card JSON behind lingvolive.com.
//
//   GET https://api.lingvolive.com/Translation/tutor-cards?text=<word>&srcLang=<lcid>&dstLang=<lcid>
//   → [ { dictionaryName: "LingvoUniversal (En-Ru)", heading: "house",
//         transcription: "haus; 'hauzɪz", translations: "дом; жилище; здание",
//         examples: "country house — дом в деревне \r\ndetached house — …" }, … ]
//
// One card per sense, dictionaries in Lingvo's order. An unknown word gives
// null or []; a language pair Lingvo cannot do gives HTTP 500 with
// { message: "An error has occurred." }. Languages are Windows locale ids
// (module.toml maps them); one side is normally Russian.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

var API = "https://api.lingvolive.com/Translation/tutor-cards";

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var ru = "1049", en = "1033";
  var from = req.from === "auto" ? (/[Ѐ-ӿ]/.test(word) ? ru : en) : req.from;
  var to = req.to === "auto" || req.to === from ? (from === ru ? en : ru) : req.to;
  var r = host.fetch(API + "?text=" + encodeURIComponent(word) + "&srcLang=" + from + "&dstLang=" + to, { timeout: TIMEOUT });
  if (r.status === 500) throw new ModuleError("source_language", "Lingvo has no dictionary for this pair");
  if (r.status === 429) throw new ModuleError("rate_limited", "Lingvo asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Lingvo answered " + r.status);
  var cards = r.json() || [];
  if (!Array.isArray(cards)) throw new ModuleError("bad_response", "Lingvo changed its answer: not a list of cards");
  if (!cards.length) throw new ModuleError("not_found", word);

  var maxExamples = req.options.max_examples === undefined ? 10 : Number(req.options.max_examples);
  var article = {
    headword: cards[0].heading || word,
    pronunciations: [],
    explanations: [],
    examples: [],
    link: "https://www.lingvolive.com/en-us/translate/" + langName(from) + "-" + langName(to) + "/" + encodeURIComponent(word)
  };
  var symbols = {};
  var byDictionary = {};
  cards.forEach(function (c) {
    (c.transcription || "").split(/;\s*/).forEach(function (t) {
      if (t && !symbols[t]) {
        symbols[t] = true;
        article.pronunciations.push({ region: "", symbol: t });
      }
    });
    var name = c.dictionaryName || "";
    if (!byDictionary[name]) {
      byDictionary[name] = { trait: name, explains: [] };
      article.explanations.push(byDictionary[name]);
    }
    if (c.translations && c.translations.trim()) byDictionary[name].explains.push(c.translations.trim());
    (c.examples || "").split(/\r?\n/).forEach(function (line) {
      line = line.trim();
      if (!line || article.examples.length >= maxExamples) return;
      var parts = line.split(/\s+—\s+/);
      article.examples.push({ source: parts[0], target: parts.slice(1).join(" — ") });
    });
  });
  article.explanations = article.explanations.filter(function (e) { return e.explains.length; });
  if (!article.explanations.length) throw new ModuleError("not_found", word);
  return article;
}

// The two-letter names lingvolive.com uses in its own page addresses.
function langName(lcid) {
  var names = { "1033": "en", "1049": "ru", "1058": "uk", "1028": "zh", "1036": "fr", "1034": "es", "1040": "it", "1142": "la", "1032": "el" };
  return names[lcid] || "en";
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
dictionary = guarded("Lingvo", dictionary);
