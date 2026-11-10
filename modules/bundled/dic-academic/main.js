// dic.academic.ru, read from its search page.
//
//   GET https://dic.academic.ru/searchall.php?SWord=<word>&from=<lang|xx>&to=ru&did=&stype=
//
// `from` narrows the search to dictionaries of one language (xx: any). The
// results are a list:
//
//   <ul class="terms-list" id="found_articles">
//     <li><p><span class="item">1</span><strong><a href="//cide.en-academic.com/84643/House">House</a></strong>
//         — (hous), n.; pl. {Houses}. … …</p>
//         <p class="src"><a href="//cide.en-academic.com/">The Collaborative International Dictionary of English</a></p></li>
//
// Each entry becomes a line "House — (hous), n.; …" under the name of the
// dictionary it came from. The snippets are escaped twice (&amp;#160;), so
// entities are decoded until none are left. No list means no results.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

var SITE = "https://dic.academic.ru";

function decode(s) {
  for (var i = 0; i < 3 && /&[#a-z0-9]+;/i.test(s); i++) {
    s = s
      .replace(/&nbsp;/g, " ")
      .replace(/&#(\d+);/g, function (_, n) { return String.fromCharCode(Number(n)); })
      .replace(/&#x([0-9a-f]+);/gi, function (_, n) { return String.fromCharCode(parseInt(n, 16)); })
      .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&#39;/g, "'")
      .replace(/&mdash;/g, "—").replace(/&ndash;/g, "–").replace(/&hellip;/g, "…")
      .replace(/&amp;/g, "&");
  }
  return s;
}

function text(html) {
  return decode(html.replace(/<[^>]*>/g, "")).replace(/\s+/g, " ").trim();
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var from = req.from === "auto" ? "xx" : req.from;
  var url = SITE + "/searchall.php?SWord=" + encodeURIComponent(word) + "&from=" + from + "&to=ru&did=&stype=";
  var r = host.fetch(url, { timeout: TIMEOUT });
  if (r.status === 429) throw new ModuleError("rate_limited", "Academic asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Academic answered " + r.status);

  var page = String(r.body || "");
  var start = page.indexOf('id="found_articles"');
  if (start < 0) throw new ModuleError("not_found", word);
  var list = page.slice(start);
  var end = list.indexOf("</ul>");
  if (end > 0) list = list.slice(0, end);

  var limit = Number(req.options.max_entries) || 20;
  var article = { headword: word, explanations: [], link: url };
  var bySource = {};
  var count = 0;
  list.split(/<li\b[^>]*>/).slice(1).forEach(function (item) {
    if (count >= limit) return;
    var body = /<p>([\s\S]*?)<\/p>/.exec(item);
    if (!body) return;
    var line = text(body[1].replace(/<span class="item">[\s\S]*?<\/span>/, ""));
    if (!line) return;
    var src = /<p class="src">([\s\S]*?)<\/p>/.exec(item);
    var source = src ? text(src[1]) : "";
    if (!bySource[source]) {
      bySource[source] = { trait: source, explains: [] };
      article.explanations.push(bySource[source]);
    }
    bySource[source].explains.push(line);
    count++;
  });
  if (!count) throw new ModuleError("not_found", word);
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
dictionary = guarded("Academic", dictionary);
