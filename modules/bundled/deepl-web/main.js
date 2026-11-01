// DeepL, keyless: the free "one-shot" endpoint DeepL's iOS app calls.
//
//   POST https://oneshot-free.www.deepl.com/v1/translate
//   User-Agent: DeepL/<version> (com.linguee.DeepLMobileTranslator; …; iOS …)
//   { text: ["…"], target_lang: "ru", source_lang?: "en", usage_type: "translate",
//     app_information: { os, os_version, app_version, app_build, instance_id } }
//   → { translations: [{ detected_source_language: "en", text: "…" }] }
//   → 429 { code: 429, message: "Too many requests…" } after a burst of requests
//
// It wants the shape of the app's request, not a key: the app information is
// the app's own description of itself, and `instance_id` is a random id for
// this installation, made once per run and kept in the module cache. Targets
// that DeepL splits by variant need one: English is sent as en-US, Portuguese
// as pt-BR; Chinese targets keep their script (zh-Hans, zh-Hant).

var URL = "https://oneshot-free.www.deepl.com/v1/translate";
var APP = { version: "26.42", build: "5443737", os: "27.0" };

function instanceId(host) {
  var id = host.cache.get("instance_id");
  if (!id) {
    id = "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (c) {
      var r = Math.floor(Math.random() * 16);
      return (c === "x" ? r : (r & 3) | 8).toString(16);
    });
    host.cache.set("instance_id", id, 0);
  }
  return id;
}

function target(code) {
  if (code === "en") return "en-US";
  if (code === "pt") return "pt-BR";
  return code;
}

function translate(req, host) {
  var payload = {
    text: [req.text],
    target_lang: target(req.to),
    usage_type: "translate",
    app_information: {
      os: "iOS",
      os_version: APP.os,
      app_version: APP.version,
      app_build: APP.build,
      instance_id: instanceId(host)
    }
  };
  // Sources carry no variant: zh-Hans is plain zh there.
  if (req.from !== "auto") payload.source_lang = req.from.split("-")[0];

  var r = host.fetch(URL, {
    method: "POST",
    headers: {
      "User-Agent": "DeepL/" + APP.version + " (com.linguee.DeepLMobileTranslator; build:" + APP.build + "; iOS " + APP.os + ")"
    },
    json: payload
  });
  var body = null;
  try {
    body = JSON.parse(r.body);
  } catch (e) {
    // Not JSON: an error page. The status says what happened.
  }
  if (r.status === 429) throw new ModuleError("rate_limited", (body && body.message) || "too many requests");
  if (!r.ok) throw new ModuleError("http", (body && body.message) || "DeepL answered " + r.status);
  var t = body && body.translations && body.translations[0];
  if (!t) throw new ModuleError("bad_response", "DeepL: no translation in the answer");
  return { text: t.text || "", detected_from: t.detected_source_language ? String(t.detected_source_language).toLowerCase() : null };
}
