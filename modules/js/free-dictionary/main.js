// Free Dictionary: an example Glossa JavaScript module.
//
// The engine calls `dictionary(req, host)` with
//   req = { text, from, to, exact, options }
// and expects an article back (see docs/modules/protocol.md, "dictionary").
// `host.fetch` is synchronous, so the code reads top to bottom.

function dictionary(req, host) {
  if (req.from !== "en" && req.from !== "auto") {
    throw new ModuleError("source_language", "Free Dictionary only has English words");
  }
  var word = req.text.trim().toLowerCase();
  if (!word || /\s/.test(word)) {
    throw new ModuleError("bad_request", "look up one word at a time");
  }

  // Answers rarely change: keep them for a day.
  var cacheKey = "entry:" + word;
  var entries = host.cache.get(cacheKey);
  if (!entries) {
    var r = host.fetch("https://api.dictionaryapi.dev/api/v2/entries/en/" + encodeURIComponent(word));
    if (r.status === 404) throw new ModuleError("not_found", word);
    if (!r.ok) throw new ModuleError("http", "dictionaryapi.dev answered " + r.status);
    entries = r.json();
    host.cache.set(cacheKey, entries, 24 * 3600);
  }

  var limit = Number(req.options.max_definitions) || 4;
  var article = { headword: word, pronunciations: [], explanations: [], examples: [], associations: [] };
  var seenSymbols = {};

  entries.forEach(function (entry) {
    (entry.phonetics || []).forEach(function (p) {
      if (!p.text || seenSymbols[p.text]) return;
      seenSymbols[p.text] = true;
      // The audio file name tells the accent: hello-uk.mp3, hello-us.mp3.
      var region = /-(uk|us|au)\.mp3$/.exec(p.audio || "");
      article.pronunciations.push({
        region: region ? region[1].toUpperCase() : "",
        symbol: p.text,
        audio_url: p.audio || null
      });
    });
    (entry.meanings || []).forEach(function (m) {
      var defs = (m.definitions || []).slice(0, limit);
      article.explanations.push({
        part_of_speech: m.partOfSpeech || "",
        definitions: defs.map(function (d) { return d.definition; })
      });
      defs.forEach(function (d) {
        if (d.example) article.examples.push({ source: d.example, target: "" });
      });
      (m.synonyms || []).forEach(function (s) {
        if (article.associations.indexOf(s) < 0) article.associations.push(s);
      });
    });
  });

  if (entries[0] && entries[0].sourceUrls) article.link = entries[0].sourceUrls[0];
  host.log("debug", word + ": " + article.explanations.length + " parts of speech");
  return article;
}
