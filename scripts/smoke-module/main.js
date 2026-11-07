// Canned answers for the screenshot smoke test. Deterministic and offline:
// the same text always gives the same answer, and nothing is fetched.

var CANNED = {
  "Hello! This is Glossa, the resurrected QTranslate.": "Привет! Это Glossa, воскрешённый QTranslate.",
  "The quick brown fox jumps over the lazy dog.": "Шустрая бурая лиса прыгает через ленивого пса."
};

function translate(req, host) {
  var text = req.text.trim();
  return { text: CANNED[text] || "[" + req.to + "] " + text, detected_from: "en" };
}

function detect(req, host) {
  return { lang: "en", confidence: 1 };
}

function dictionary(req, host) {
  var word = req.text.trim().toLowerCase();
  return {
    headword: word,
    pronunciations: [{ region: "UK", symbol: "/ˌser.ənˈdɪp.ə.ti/" }],
    explanations: [
      { part_of_speech: "noun", definitions: [
        "The fact of finding interesting or valuable things by chance.",
        "An instance of such a lucky discovery."
      ] }
    ],
    examples: [{ source: "Meeting her there was pure serendipity.", target: "" }],
    associations: ["chance", "luck", "fortune"]
  };
}
