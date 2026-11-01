// Yandex Translate, keyless.
//
// Yandex's browser and video-translation clients talk to translate.yandex.net
// with a client name (`srv`) instead of an API key. Without `srv` the same
// URLs answer 410 ("API v1 is no longer active").
//
//   translate   POST /api/v1/tr.json/translate?srv=…&lang=<from>-<to>
//               (lang=<to> alone: Yandex detects the source)
//               body, form-encoded: text=<piece>[&text=<piece>…]
//               → { code: 200, lang: "en-ru", text: ["…"] }
//               errors keep HTTP 200 or 4xx with { code, message }:
//               413 text too long, 501 direction not supported.
//   detect      GET  /api/v1/tr.json/detect?srv=…&text=…
//               → { code: 200, lang: "fr" }
//   dictionary  GET  /dicservice.json/lookup?srv=…&lang=<from>-<to>&text=…
//               → { def: [{ text, pos, ts, tr: [{ text, pos, syn: [{ text }],
//                    mean: [{ text }], ex: [{ text, tr: [{ text }] }] }] }] }
//               An unknown word gives def: [].
//
// One request carries at most about 10 000 characters, so longer input is cut
// into pieces at line or sentence ends and sent one piece at a time.

var BASE = "https://translate.yandex.net";
var SRV = "browser_video_translation";
var PIECE = 4000;

function fail(body, status) {
  var code = body && body.code;
  var message = (body && body.message) || "Yandex answered " + status;
  if (code === 501) throw new ModuleError("target_language", message);
  if (code === 413) throw new ModuleError("bad_request", message);
  if (code === 429 || status === 429) throw new ModuleError("rate_limited", message);
  throw new ModuleError("http", message);
}

function call(r) {
  var body = null;
  try {
    body = JSON.parse(r.body);
  } catch (e) {
    if (!r.ok) fail(null, r.status);
    throw new ModuleError("bad_response", "Yandex: not JSON: " + String(r.body).substring(0, 120));
  }
  if (!r.ok || (body.code && body.code !== 200)) fail(body, r.status);
  return body;
}

// Cut `text` into pieces of at most `max` characters, at the last line break,
// sentence end or space before the limit. The whitespace at each cut is kept
// aside (`gap`) and put back between the translated pieces.
function pieces(text, max) {
  var out = [];
  while (text.length > max) {
    var head = text.slice(0, max);
    var cut = -1;
    var patterns = [/\n\s*/g, /[.!?;。！？]\s+/g, /\s+/g];
    for (var i = 0; i < patterns.length && cut < 0; i++) {
      var m, last = null;
      while ((m = patterns[i].exec(head)) !== null) last = m;
      if (last && last.index > 0) cut = last.index + (i === 1 ? 1 : 0);
    }
    if (cut <= 0) cut = max;
    var rest = text.slice(cut);
    var gap = /^\s*/.exec(rest)[0];
    out.push({ text: text.slice(0, cut), gap: gap });
    text = rest.slice(gap.length);
  }
  if (text) out.push({ text: text, gap: "" });
  return out;
}

function translate(req, host) {
  var lang = req.from === "auto" ? req.to : req.from + "-" + req.to;
  var url = BASE + "/api/v1/tr.json/translate?srv=" + SRV + "&lang=" + encodeURIComponent(lang);
  var out = "";
  var detected = null;
  pieces(req.text, PIECE).forEach(function (p) {
    var body = call(host.fetch(url, {
      method: "POST",
      headers: { Referer: "https://translate.yandex.com/" },
      form: { text: p.text }
    }));
    if (!body.text) throw new ModuleError("bad_response", "Yandex: no text in the answer");
    out += body.text.join("") + p.gap;
    if (!detected && body.lang) detected = String(body.lang).split("-")[0];
  });

  var result = { text: out, detected_from: detected };
  // A word or two: add Yandex's dictionary entry, as the popup shows under
  // the translation. Its failure never spoils the translation itself.
  var word = req.text.trim();
  var from = req.from === "auto" ? detected : req.from;
  if (from && word.length <= 60 && !/[\r\n]/.test(word) && word.split(/\s+/).length <= 3) {
    try {
      result.parts_of_speech = lookup(host, word, from, req.to).map(function (def) {
        return {
          part_of_speech: def.pos || "",
          senses: (def.tr || []).map(function (tr) {
            return { word: tr.text, back_translations: (tr.mean || []).map(function (m) { return m.text; }) };
          })
        };
      });
    } catch (e) {
      host.log("debug", "dictionary entry skipped: " + e);
    }
  }
  return result;
}

function detect(req, host) {
  var url = BASE + "/api/v1/tr.json/detect?srv=" + SRV + "&text=" + encodeURIComponent(req.text.slice(0, 256));
  var body = call(host.fetch(url, { headers: { Referer: "https://translate.yandex.com/" } }));
  if (!body.lang) throw new ModuleError("bad_response", "Yandex did not name a language");
  return { lang: body.lang, confidence: 1 };
}

function lookup(host, word, from, to) {
  var url = BASE + "/dicservice.json/lookup?srv=" + SRV + "&lang=" + encodeURIComponent(from + "-" + to) +
    "&text=" + encodeURIComponent(word);
  var r = host.fetch(url);
  var body;
  try {
    body = JSON.parse(r.body);
  } catch (e) {
    throw new ModuleError(r.ok ? "bad_response" : "http", "Yandex dictionary answered " + r.status);
  }
  if (body.code === 501) throw new ModuleError("source_language", body.message || "pair not in the dictionary");
  if (!r.ok) fail(body, r.status);
  return body.def || [];
}

function dictionary(req, host) {
  var word = req.text.trim();
  if (!word) throw new ModuleError("bad_request", "nothing to look up");
  var from = req.from === "auto" ? detect(req, host).lang : req.from;
  var defs = lookup(host, word, from, req.to);
  if (!defs.length) throw new ModuleError("not_found", word);

  var article = {
    headword: defs[0].text || word,
    pronunciations: [],
    explanations: [],
    examples: [],
    link: "https://translate.yandex.com/?lang=" + from + "-" + req.to + "&text=" + encodeURIComponent(word)
  };
  var symbols = {};
  defs.forEach(function (def) {
    if (def.ts && !symbols[def.ts]) {
      symbols[def.ts] = true;
      article.pronunciations.push({ region: "", symbol: def.ts });
    }
    article.explanations.push({
      trait: def.pos || "",
      explains: (def.tr || []).map(function (tr) {
        var line = [tr.text].concat((tr.syn || []).map(function (s) { return s.text; })).join(", ");
        if (tr.mean && tr.mean.length) line += " (" + tr.mean.map(function (m) { return m.text; }).join(", ") + ")";
        return line;
      })
    });
    (def.tr || []).forEach(function (tr) {
      (tr.ex || []).forEach(function (ex) {
        article.examples.push({ source: ex.text, target: (ex.tr || []).map(function (t) { return t.text; }).join(", ") });
      });
    });
  });
  return article;
}
