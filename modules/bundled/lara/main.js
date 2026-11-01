// Lara Translate, keyless: the endpoint laratranslate.com's page calls.
//
//   POST https://webapi.laratranslate.com/translate
//   { q, source: "" (detect) | "<code>", target: "<code>", instructions: [] }
//   → { status: 200, content: { source_language: "en", translation: "…" } }
//   → { status: 400, error: { type: "UnsupportedLanguageError", message } }
//
// Codes are BCP 47 as Glossa spells them (zh-Hans, pt, nb…), so no language
// map is needed. Line breaks survive the round trip.

function translate(req, host) {
  var r = host.fetch("https://webapi.laratranslate.com/translate", {
    method: "POST",
    headers: {
      Accept: "application/json",
      Origin: "https://laratranslate.com",
      Referer: "https://laratranslate.com/"
    },
    json: { q: req.text, source: req.from === "auto" ? "" : req.from, target: req.to, instructions: [] }
  });
  var body;
  try {
    body = JSON.parse(r.body);
  } catch (e) {
    throw new ModuleError(r.ok ? "bad_response" : "http", "Lara answered " + r.status);
  }
  var error = body.error || {};
  if (error.type === "UnsupportedLanguageError") throw new ModuleError("target_language", error.message);
  if (r.status === 429) throw new ModuleError("rate_limited", error.message || "too many requests");
  if (!r.ok || !body.content) throw new ModuleError("http", error.message || "Lara answered " + r.status);
  return { text: body.content.translation || "", detected_from: body.content.source_language || null };
}
