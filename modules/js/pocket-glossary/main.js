// Pocket Glossary: an example Glossa JavaScript module with a panel.
//
// `dictionary(req, host)` is an ordinary entry point: the dictionary
// window and the panel's "look up" (a `call = "dictionary"` action, no code)
// both reach it. The other functions are the panel's action handlers:
//
//   search(ev, host)        ev.state.query -> the list of matching words
//   wordOfTheDay(ev, host)  picks a word, streaming progress with host.ui.update
//
// A handler gets `ev = { action, panel, event, state, options, ui_lang }`
// and returns `{ state: {...} }` (merged into the panel), with an optional
// `notice`. See docs/modules/panels.md.

var TERMS = [
  { word: "calque", pos: "noun", meaning: "A word or phrase borrowed by translating it literally, part by part.", example: "\"Skyscraper\" became the calque \"gratte-ciel\" in French." },
  { word: "cognate", pos: "noun", meaning: "A word that shares its origin with a word in another language.", example: "English \"night\" and German \"Nacht\" are cognates." },
  { word: "false friend", pos: "noun", meaning: "A word that looks like one in another language but means something else.", example: "Spanish \"embarazada\" means pregnant, not embarrassed." },
  { word: "gloss", pos: "noun", meaning: "A short explanation or translation of a word, written beside it.", example: "The editor added a gloss for every dialect word." },
  { word: "idiom", pos: "noun", meaning: "A phrase whose meaning cannot be worked out from its words.", example: "\"Break a leg\" is an idiom for \"good luck\"." },
  { word: "localisation", pos: "noun", meaning: "Adapting a text or program to the language and customs of a place.", example: "Localisation changed the dates and the currency, not only the words." },
  { word: "loanword", pos: "noun", meaning: "A word taken from another language with little or no change.", example: "\"Kindergarten\" is a loanword from German." },
  { word: "source text", pos: "noun", meaning: "The text a translation is made from.", example: "Keep the source text open beside the draft." },
  { word: "target language", pos: "noun", meaning: "The language a text is translated into.", example: "Her target language is Portuguese." },
  { word: "transliteration", pos: "noun", meaning: "Writing a word in the letters of another script.", example: "\"Moskva\" is a transliteration of \"Москва\"." }
];

function find(word) {
  var w = String(word || "").trim().toLowerCase();
  for (var i = 0; i < TERMS.length; i++) {
    if (TERMS[i].word === w) return TERMS[i];
  }
  return null;
}

function article(t) {
  return {
    headword: t.word,
    explanations: [{ part_of_speech: t.pos, definitions: [t.meaning] }],
    examples: [{ source: t.example, target: "" }]
  };
}

function dictionary(req, host) {
  if (req.from !== "en" && req.from !== "auto") {
    throw new ModuleError("source_language", "the glossary is in English");
  }
  var t = find(req.text);
  if (!t) throw new ModuleError("not_found", String(req.text).trim());
  return article(t);
}

function rows(list) {
  return list.map(function (t) { return { title: t.word, subtitle: t.meaning }; });
}

function search(ev, host) {
  var q = String(ev.state.query || "").trim().toLowerCase();
  var hits = TERMS.filter(function (t) {
    return !q || t.word.indexOf(q) >= 0 || t.meaning.toLowerCase().indexOf(q) >= 0;
  });
  return {
    state: {
      words: rows(hits),
      status: hits.length ? host.t("found", hits.length) : host.t("nothing", q)
    }
  };
}

function wordOfTheDay(ev, host) {
  // Partial state shows at once while the handler works; a slow service
  // (an LLM writing an explanation) would send its text this way.
  host.ui.update({ status: host.t("picking"), progress: 30 });
  var day = Math.floor(Number(ev.state.day || Date.now() / 86400000));
  var t = TERMS[((day % TERMS.length) + TERMS.length) % TERMS.length];
  host.ui.update({ progress: 70 });
  return {
    state: {
      query: t.word,
      words: rows([t]),
      article: article(t),
      status: host.t("of_the_day"),
      progress: 100
    },
    notice: host.t("of_the_day_notice", t.word),
    notice_level: "success"
  };
}
