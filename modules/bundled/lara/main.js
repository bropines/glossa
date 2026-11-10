// Lara Translate, keyless: the endpoint laratranslate.com's page calls.
//
//   POST https://webapi.laratranslate.com/translate
//   { q, source: "" (detect) | "<code>", target: "<code>", instructions: [] }
//   → { status: 200, content: { source_language: "en", translation: "…" } }
//   → { status: 400, error: { type: "UnsupportedLanguageError", message } }
//
// Codes are BCP 47 as Glossa spells them (zh-Hans, pt, nb…), so no language
// map is needed. Line breaks survive the round trip.

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

function translate(req, host) {
  var r = host.fetch("https://webapi.laratranslate.com/translate", {
    method: "POST",
    timeout: TIMEOUT,
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
  if (!body || typeof body !== "object") throw new ModuleError("bad_response", "Lara changed its answer: not an object");
  var error = body.error || {};
  if (error.type === "UnsupportedLanguageError") throw new ModuleError("target_language", error.message);
  if (r.status === 429) throw new ModuleError("rate_limited", error.message || "too many requests");
  if (!r.ok) throw new ModuleError("http", error.message || "Lara answered " + r.status);
  if (!body.content || typeof body.content.translation !== "string" || !body.content.translation) {
    throw new ModuleError("bad_response", error.message || "Lara changed its answer: no translation in it");
  }
  return { text: body.content.translation, detected_from: body.content.source_language || null };
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
translate = guarded("Lara", translate);
