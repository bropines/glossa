// translate.academic.ru, read from its result page.
//
//   GET https://translate.academic.ru/<word>/<from>/<to>/
//
// The page lists one entry per dictionary that has the word:
//
//   <div id="article"><ul class="terms-list">
//     <li><p><span class="item">1</span><strong>house</strong> <span class="sound" …></span></p>
//         <div class="translate_definition"> …the dictionary's own HTML… </div>
//         <p class="src"><a href="https://eng_rus_building.academic.ru">Англо-русский словарь…</a> > …</p></li>
//
// The entries are rich, irregular HTML (numbered senses, labels, phrase
// lists), so they are passed on as HTML for Glossa's article view, cut to the
// first few dictionaries, with site-relative links made absolute. A word with
// no entries gives a page without any translate_definition block.

var SITE = "https://translate.academic.ru";

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var from = req.from === "auto" ? (/[Ѐ-ӿ]/.test(word) ? "ru" : "en") : req.from;
  var to = req.to === "auto" || req.to === from ? (from === "ru" ? "en" : "ru") : req.to;
  // The word is a path segment, so "/" in it must be escaped too.
  var url = SITE + "/" + encodeURIComponent(word) + "/" + from + "/" + to + "/";
  var r = host.fetch(url);
  if (r.status === 404) throw new ModuleError("not_found", word);
  if (r.status === 429) throw new ModuleError("rate_limited", "Academic asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Academic answered " + r.status);

  var page = r.body;
  var start = page.indexOf('<ul class="terms-list"');
  if (start < 0 || page.indexOf('class="translate_definition"') < 0) throw new ModuleError("not_found", word);
  // The list alone, and in it the first entries, found with indexOf: a
  // common word's list runs to 200 KB, and regular expressions over that
  // much text are slow in the module runtime (seconds, not milliseconds).
  var end = page.indexOf("</ul>", start);
  var list = page.slice(start, end < 0 ? page.length : end);
  var limit = Number(req.options.max_entries) || 6;
  var starts = [];
  for (var at = list.indexOf("<li>"); at >= 0 && starts.length <= limit; at = list.indexOf("<li>", at + 4)) {
    // An entry opens with <li> and its heading paragraph.
    if (/^<li>\s*<p>/.test(list.substr(at, 64))) starts.push(at);
  }
  var items = [];
  for (var i = 0; i < starts.length && i < limit; i++) {
    var item = list.slice(starts[i], i + 1 < starts.length ? starts[i + 1] : list.length).replace(/^<li>\s*<p>/, "");
    var src = item.indexOf('<p class="src">');
    var close = src >= 0 ? item.indexOf("</p>", src) : -1;
    items.push("<li><p>" + (close >= 0 ? item.slice(0, close + 4) : item) + "</li>");
  }
  if (!items.length) throw new ModuleError("not_found", word);

  var html = ("<ul>" + items.join("\n") + "</ul>")
    .replace(/<span[^>]*class="(?:sound|item)"[^>]*>[\s\S]*?<\/span>/g, "")
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/(href|src)="\/(?!\/)/g, '$1="' + SITE + "/")
    .replace(/(href|src)="\/\//g, '$1="https://');
  return { headword: word, html: html, link: url };
}
