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
// that DeepL splits by variant need one: English goes out as en-US or en-GB
// and Portuguese as pt-BR or pt-PT, as the module's settings say; Chinese
// targets keep their script (zh-Hans, zh-Hant).
//
// Other answers seen: 451 where DeepL does not offer the free service (by
// the address the request comes from), 403 when it refuses the request.

var URL = "https://oneshot-free.www.deepl.com/v1/translate";

// What the request says about the app. These age: when DeepL starts refusing
// them (403), put a current release here and nothing else needs to change.
// All three are in the User-Agent the app sends,
//   DeepL/<version> (com.linguee.DeepLMobileTranslator; build:<build>; iOS <os>)
// and `version` is also the newest entry of the app's version history on
// its App Store page.
var APP = { version: "26.42", build: "5443737", os: "27.0" };

// Well under Glossa's own budget for a call (30 s), so a stalled service
// fails with its name on the error instead of the call being abandoned.
var TIMEOUT = 15000;

// The variants a target without one is sent as, unless the settings say
// otherwise.
var VARIANTS = {
  en: { option: "english_variant", fallback: "en-US", allowed: ["en-US", "en-GB"] },
  pt: { option: "portuguese_variant", fallback: "pt-BR", allowed: ["pt-BR", "pt-PT"] }
};

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

function target(code, options) {
  var v = VARIANTS[code];
  if (!v) return code;
  var chosen = options && options[v.option];
  return v.allowed.indexOf(chosen) >= 0 ? chosen : v.fallback;
}

// A header by name, whatever its case.
function header(r, name) {
  var headers = r.headers || {};
  for (var k in headers) {
    if (k.toLowerCase() === name) return String(headers[k]);
  }
  return "";
}

function translate(req, host) {
  var payload = {
    text: [req.text],
    target_lang: target(req.to, req.options),
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
    timeout: TIMEOUT,
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
  var said = body && typeof body.message === "string" ? body.message : "";
  if (r.status === 429) {
    // The protocol's error has a place for the wait, but a JavaScript module
    // can only put it into words.
    var wait = parseInt(header(r, "retry-after"), 10);
    var limit = said || "DeepL's free service is rate-limited: too many requests in a row.";
    throw new ModuleError("rate_limited", wait > 0 ? limit + " Try again in " + wait + " s." : limit);
  }
  if (r.status === 451) {
    throw new ModuleError("http", "DeepL does not offer its free service in this region (HTTP 451). " +
      "A proxy in another country (Options, Internet) or the DeepL API service with a key gets around it.");
  }
  if (r.status === 403) {
    throw new ModuleError("http", "DeepL refused the request (HTTP 403): it blocks this address, " +
      "or no longer serves the app version this module presents." + (said ? " It said: " + said : ""));
  }
  if (!r.ok) throw new ModuleError("http", said || "DeepL answered " + r.status);
  var t = body && Array.isArray(body.translations) && body.translations[0];
  if (!t || typeof t.text !== "string" || !t.text) {
    throw new ModuleError("bad_response", "DeepL changed its answer: no translation in it");
  }
  return { text: t.text, detected_from: t.detected_source_language ? String(t.detected_source_language).toLowerCase() : null };
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
translate = guarded("DeepL", translate);
