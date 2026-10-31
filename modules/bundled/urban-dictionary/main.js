// Urban Dictionary through its public JSON API (the classic script read the
// HTML page, whose layout has since changed).
//
//   GET https://api.urbandictionary.com/v0/define?term=<word>
//   → { list: [ { word, definition, example, thumbs_up, thumbs_down, permalink }, … ] }
//
// An unknown term gives { list: [] }. Definitions mark cross-references with
// square brackets ("To [discard] an [item]"); the brackets are dropped.
// Entries come in Urban Dictionary's order and are sorted by votes here.

function plain(s) {
  return String(s || "").replace(/\[([^\]]*)\]/g, "$1").replace(/\r\n?/g, "\n").trim();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var r = host.fetch("https://api.urbandictionary.com/v0/define?term=" + encodeURIComponent(word));
  if (r.status === 429) throw new ModuleError("rate_limited", "Urban Dictionary asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Urban Dictionary answered " + r.status);
  var list = (r.json() || {}).list || [];
  if (!list.length) throw new ModuleError("not_found", word);

  var limit = Number(req.options.max_definitions) || 5;
  list = list.slice().sort(function (a, b) {
    return ((b.thumbs_up || 0) - (b.thumbs_down || 0)) - ((a.thumbs_up || 0) - (a.thumbs_down || 0));
  }).slice(0, limit);
  return {
    headword: list[0].word || word,
    explanations: [{ trait: "", explains: list.map(function (d) { return plain(d.definition); }) }],
    examples: list.filter(function (d) { return plain(d.example); }).map(function (d) {
      return { source: plain(d.example), target: "" };
    }),
    link: "https://www.urbandictionary.com/define.php?term=" + encodeURIComponent(word)
  };
}
