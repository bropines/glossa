// ModernMT, keyless, through its public translate page. The classic script
// went through a third-party proxy that no longer exists.
//
// modernmt.com/translate is a Remix app: the page's form posts to its own
// route, and `_data` asks for the route's JSON instead of a page.
//
//   POST https://www.modernmt.com/translate?_data=routes%2Ftranslate
//   form: text=…&targetLanguage=ru[&sourceLanguage=en]   (no source: detect)
//   → { status: "success", value: { translation: "…", detectedLanguage?: "de" } }
//   → { status: "error", error: { "": ["Language pair not supported: en > eu"] } }

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

var URL = "https://www.modernmt.com/translate?_data=routes%2Ftranslate";

function translate(req, host) {
  var form = { text: req.text, targetLanguage: req.to };
  if (req.from !== "auto") form.sourceLanguage = req.from;
  var r = host.fetch(URL, {
    method: "POST",
    timeout: TIMEOUT,
    headers: { Origin: "https://www.modernmt.com", Referer: "https://www.modernmt.com/translate" },
    form: form
  });
  if (r.status === 429) throw new ModuleError("rate_limited", "ModernMT asks to slow down");
  var body;
  try {
    body = JSON.parse(r.body);
  } catch (e) {
    throw new ModuleError(r.ok ? "bad_response" : "http", "ModernMT answered " + r.status);
  }
  if (!body || typeof body !== "object") throw new ModuleError("bad_response", "ModernMT changed its answer: not an object");
  if (body.status !== "success" || !body.value) {
    var messages = [];
    var errors = body.error || {};
    Object.keys(errors).forEach(function (k) { messages = messages.concat(errors[k]); });
    var message = messages.join("; ") || "ModernMT answered " + r.status;
    if (/not supported/i.test(message)) throw new ModuleError("target_language", message);
    throw new ModuleError(r.ok && !messages.length ? "bad_response" : "http", message);
  }
  if (typeof body.value.translation !== "string" || !body.value.translation) {
    throw new ModuleError("bad_response", "ModernMT changed its answer: no translation in it");
  }
  return { text: body.value.translation, detected_from: body.value.detectedLanguage || null };
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
translate = guarded("ModernMT", translate);
