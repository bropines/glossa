// {{name}}: a Glossa dictionary.
//
// Glossa calls dictionary(req, host) with
//   req = { text, from, to, exact, options }
// and expects an article: { headword, pronunciations, explanations, examples }.
// See docs/modules/protocol.md, "dictionary".

function dictionary(req, host) {
  if (req.from !== "en" && req.from !== "auto") {
    throw new ModuleError("source_language", "only English words");
  }
  var word = req.text.trim().toLowerCase();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");

  var r = host.fetch("https://api.dictionaryapi.dev/api/v2/entries/en/" + encodeURIComponent(word));
  if (r.status === 404) throw new ModuleError("not_found", word);
  if (!r.ok) throw new ModuleError("http", "HTTP " + r.status);

  var limit = Number(req.options.max_definitions) || 3;
  var article = { headword: word, pronunciations: [], explanations: [], examples: [] };
  r.json().forEach(function (entry) {
    (entry.phonetics || []).forEach(function (p) {
      if (p.text) article.pronunciations.push({ symbol: p.text });
    });
    (entry.meanings || []).forEach(function (m) {
      var defs = (m.definitions || []).slice(0, limit);
      article.explanations.push({
        part_of_speech: m.partOfSpeech || "",
        definitions: defs.map(function (d) { return d.definition; })
      });
      defs.forEach(function (d) {
        if (d.example) article.examples.push({ source: d.example });
      });
    });
  });
  console.log(word + ": " + article.explanations.length + " parts of speech");
  return article;
}
