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

var URL = "https://www.modernmt.com/translate?_data=routes%2Ftranslate";

function translate(req, host) {
  var form = { text: req.text, targetLanguage: req.to };
  if (req.from !== "auto") form.sourceLanguage = req.from;
  var r = host.fetch(URL, {
    method: "POST",
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
  if (body.status !== "success" || !body.value) {
    var messages = [];
    var errors = body.error || {};
    Object.keys(errors).forEach(function (k) { messages = messages.concat(errors[k]); });
    var message = messages.join("; ") || "ModernMT answered " + r.status;
    if (/not supported/i.test(message)) throw new ModuleError("target_language", message);
    throw new ModuleError("http", message);
  }
  return { text: body.value.translation || "", detected_from: body.value.detectedLanguage || null };
}
