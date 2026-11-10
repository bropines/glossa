// Urban Dictionary through its public JSON API (the classic script read the
// HTML page, whose layout has since changed).
//
//   GET https://api.urbandictionary.com/v0/define?term=<word>
//   → { list: [ { word, definition, example, thumbs_up, thumbs_down, permalink }, … ] }
//
// An unknown term gives { list: [] }. Definitions mark cross-references with
// square brackets ("To [discard] an [item]"); the brackets are dropped.
// Entries come in Urban Dictionary's order and are sorted by votes here.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function plain(s) {
  return String(s || "").replace(/\[([^\]]*)\]/g, "$1").replace(/\r\n?/g, "\n").trim();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var r = host.fetch("https://api.urbandictionary.com/v0/define?term=" + encodeURIComponent(word), { timeout: TIMEOUT });
  if (r.status === 429) throw new ModuleError("rate_limited", "Urban Dictionary asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Urban Dictionary answered " + r.status);
  var list = (r.json() || {}).list;
  if (!Array.isArray(list)) throw new ModuleError("bad_response", "Urban Dictionary changed its answer: no list of definitions in it");
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
dictionary = guarded("Urban Dictionary", dictionary);
