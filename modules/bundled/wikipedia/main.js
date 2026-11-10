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

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var lang = req.from === "auto" ? "en" : req.from;
  if (!/^[a-z]{2,3}$/.test(lang)) lang = "en";
  var url = "https://" + lang + ".wikipedia.org/api/rest_v1/page/summary/" + encodeURIComponent(word.replace(/ /g, "_"));
  var r = host.fetch(url, { timeout: TIMEOUT, headers: { "Api-User-Agent": "Glossa (https://github.com/bropines/glossa)" } });
  if (r.status === 404) throw new ModuleError("not_found", word);
  if (r.status === 429) throw new ModuleError("rate_limited", "Wikimedia asks to slow down");
  if (!r.ok) throw new ModuleError("http", "Wikipedia answered " + r.status);
  var page = r.json();
  if (!page || typeof page !== "object") throw new ModuleError("bad_response", "Wikipedia changed its answer: not an object");
  if (!page.extract) throw new ModuleError("not_found", word);

  var page_url = page.content_urls && page.content_urls.desktop && page.content_urls.desktop.page;
  return {
    headword: page.title || word,
    explanations: [{ trait: page.description || "", explains: String(page.extract).split(/\n+/).filter(function (p) { return p.trim(); }) }],
    link: page_url || "https://" + lang + ".wikipedia.org/wiki/" + encodeURIComponent(word.replace(/ /g, "_"))
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
dictionary = guarded("Wikipedia", dictionary);
