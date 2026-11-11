// Multitran, read from its public result page.
//
//   GET https://www.multitran.com/m.exe?l1=<from>&l2=<to>&s=<word>&SHL=<labels>
//
// Languages are numbers (module.toml maps them); SHL picks the language of
// the page itself, 1 English, 2 Russian. The answer is an HTML page whose
// result table, per part of speech, looks like:
//
//   <a name="noun"></a><h1><a …>house</a></h1> <span style="color:gray">[haus]</span>
//       <em><span …>n</span></em> …
//   <tr><td class="subj"><a … title="General">gen.</a></td>
//       <td class="trans1"><a …>дом</a> <span style="color:gray">(note)</span>; <a …>здание</a>; …</td></tr>
//
// Each heading becomes a part of speech in the article and each row one line,
// "General: дом (note); здание; …". A row with no subject is a cross
// reference ("⇒ full house") and goes into the related words. A word
// Multitran does not know comes back as a page with no such rows.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function text(html) {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&#(\d+);/g, function (_, n) { return String.fromCharCode(Number(n)); })
    .replace(/&#x([0-9a-f]+);/gi, function (_, n) { return String.fromCharCode(parseInt(n, 16)); })
    .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'")
    .replace(/&ndash;/g, "–").replace(/&mdash;/g, "—")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  // Multitran needs both languages. Unknown source: Cyrillic means Russian,
  // anything else English, which covers most lookups.
  var l1 = req.from === "auto" ? (/[Ѐ-ӿ]/.test(word) ? "2" : "1") : req.from;
  var l2 = req.to === "auto" || req.to === l1 ? (l1 === "2" ? "1" : "2") : req.to;
  var url = "https://www.multitran.com/m.exe?l1=" + l1 + "&l2=" + l2 + "&s=" + encodeURIComponent(word) +
    "&SHL=" + (req.options.labels === "ru" ? "2" : "1");
  var r = host.fetch(url, { timeout: TIMEOUT });
  if (r.status === 429) throw new ModuleError("rate_limited", "Multitran asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Multitran answered " + r.status);
  var page = String(r.body || "");

  var limit = Number(req.options.max_rows) || 30;
  var article = { headword: word, pronunciations: [], explanations: [], examples: [], associations: [], link: url };
  var section = null;
  var symbols = {};
  // Headings and rows, in page order.
  var token = /<a name="[^"]*"><\/a>\s*<h1>([\s\S]*?)<\/h1>([\s\S]*?)<\/td>|<td class="subj"[^>]*>([\s\S]*?)<\/td>\s*<td class="trans1?"[^>]*>([\s\S]*?)<\/td>/g;
  var m;
  while ((m = token.exec(page)) !== null) {
    if (m[1] !== undefined) {
      var after = m[2];
      var ts = /\[([^\]<]+)\]/.exec(after);
      if (ts && !symbols[ts[1]]) {
        symbols[ts[1]] = true;
        article.pronunciations.push({ region: "", symbol: text(ts[1]) });
      }
      var pos = /<em>([\s\S]*?)<\/em>/.exec(after);
      if (!article.explanations.length) article.headword = text(m[1]) || word;
      section = { trait: (text(m[1]) + " " + (pos ? text(pos[1]) : "")).trim(), explains: [] };
      article.explanations.push(section);
      continue;
    }
    var title = /title="([^"]*)"/.exec(m[3]);
    var subject = title ? text(title[1]) : text(m[3]);
    var line = text(m[4]);
    if (!line) continue;
    if (!subject) {
      line.replace(/^⇒\s*/, "").split(/;\s*/).forEach(function (w) {
        if (w && article.associations.indexOf(w) < 0) article.associations.push(w);
      });
      continue;
    }
    if (!section) {
      section = { trait: "", explains: [] };
      article.explanations.push(section);
    }
    if (section.explains.length < limit) section.explains.push(subject + ": " + line);
  }
  article.explanations = article.explanations.filter(function (e) { return e.explains.length; });
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
dictionary = guarded("Multitran", dictionary);
