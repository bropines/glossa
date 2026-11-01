// Wikipedia through the Wikimedia REST API (the classic script read the
// desktop page and broke when it began redirecting to the mobile site).
//
//   GET https://<lang>.wikipedia.org/api/rest_v1/page/summary/<title>
//   → { type: "standard" | "disambiguation", title, description, extract,
//       content_urls: { desktop: { page } } }
//   → 404 when there is no such article
//
// The title is normalised and redirects are followed by the API itself
// ("house" answers with the article "House"). The source language picks the
// Wikipedia; an unknown source uses English.

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var lang = req.from === "auto" ? "en" : req.from;
  if (!/^[a-z]{2,3}$/.test(lang)) lang = "en";
  var url = "https://" + lang + ".wikipedia.org/api/rest_v1/page/summary/" + encodeURIComponent(word.replace(/ /g, "_"));
  var r = host.fetch(url, { headers: { "Api-User-Agent": "Glossa (https://github.com/bropines/glossa)" } });
  if (r.status === 404) throw new ModuleError("not_found", word);
  if (r.status === 429) throw new ModuleError("rate_limited", "Wikimedia asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Wikipedia answered " + r.status);
  var page = r.json();
  if (!page.extract) throw new ModuleError("not_found", word);

  var page_url = page.content_urls && page.content_urls.desktop && page.content_urls.desktop.page;
  return {
    headword: page.title || word,
    explanations: [{ trait: page.description || "", explains: page.extract.split(/\n+/).filter(function (p) { return p.trim(); }) }],
    link: page_url || "https://" + lang + ".wikipedia.org/wiki/" + encodeURIComponent(word.replace(/ /g, "_"))
  };
}
