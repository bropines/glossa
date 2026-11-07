// Google Cloud Vision document text detection, as a Glossa OCR module.
//
// The service: POST https://vision.googleapis.com/v1/images:annotate?key=KEY
// with {requests: [{image: {content: <base64>}, features: [{type:
// "DOCUMENT_TEXT_DETECTION"}], imageContext: {languageHints: [...]}}]}.
// The reply's fullTextAnnotation holds pages > blocks > paragraphs > words >
// symbols, each with a boundingBox of four vertices and a confidence. Lines
// are not listed: one ends where a symbol's detectedBreak is EOL_SURE_SPACE,
// LINE_BREAK or HYPHEN.
//
// What the module shows: preparing the picture with host.image (the
// request's image is already a handle), and returning the full layout plus
// the prepared copy's transform, so Glossa maps every box back onto the
// picture the user captured.

function recognize(req, host) {
  var key = host.options.api_key;
  if (!key) throw new ModuleError("missing_credential", "api_key");

  var img = req.image_handle ? host.image.info(req.image_handle) : host.image.fromBase64(req.image_base64);
  // Vision reads tiny text poorly: enlarge small captures. Then stay well
  // inside its limits (20 MB per request, about 75 megapixels).
  var maxSide = Number(host.options.max_side || 4000);
  var minHeight = host.options.enlarge === false ? 0 : Number(host.options.min_height || 0);
  if (minHeight > 0) img = host.image.scaleToMinHeight(img, { min_height: minHeight, max_side: maxSide });
  img = host.image.resize(img, { max_side: maxSide });
  var upload = host.image.encode(img, { format: "png" });
  if (upload.data_base64.length > 14000000) upload = host.image.encode(img, { format: "jpeg", quality: 90 });

  var feature = host.options.feature || "DOCUMENT_TEXT_DETECTION";
  var request = { image: { content: upload.data_base64 }, features: [{ type: feature }] };
  var hints = languageHints(req, host.options);
  if (hints.length) request.imageContext = { languageHints: hints };
  var r = host.fetch("https://vision.googleapis.com/v1/images:annotate?key=" + encodeURIComponent(key), {
    method: "POST",
    json: { requests: [request] },
    timeout: 30000
  });
  if (r.status === 429) throw new ModuleError("rate_limited", errorMessage(r));
  if (r.status === 400 || r.status === 401 || r.status === 403) {
    var message = errorMessage(r);
    if (/api key|api_key|permission_denied|unauthenticated/i.test(message)) {
      throw new ModuleError("missing_credential", "api_key");
    }
    throw new ModuleError("http", message);
  }
  if (!r.ok) throw new ModuleError("http", "HTTP " + r.status + ": " + errorMessage(r));

  var res = (r.json().responses || [])[0] || {};
  if (res.error) throw new ModuleError("bad_response", res.error.message || "Vision reported an error");
  var doc = res.fullTextAnnotation;
  if (!doc || !doc.text) throw new ModuleError("not_found", "no text in the picture");
  var page = (doc.pages || [])[0] || {};
  return {
    text: doc.text.replace(/\n+$/, ""),
    detected_lang: language(page),
    blocks: (page.blocks || []).map(block),
    // The boxes are in the prepared copy's pixels.
    transform: img.transform
  };
}

// The languages to tell Vision about: the one Glossa asks for (the
// default), the user's own list, or none at all.
function languageHints(req, options) {
  var mode = options.hints || "request";
  if (mode === "none") return [];
  if (mode === "custom") return options.custom_hints || [];
  return req.lang && req.lang !== "auto" ? [req.lang] : [];
}

function block(b) {
  var paragraphs = (b.paragraphs || []).map(paragraph);
  return {
    level: "block",
    text: paragraphs.map(function (p) { return p.text; }).join("\n"),
    polygon: polygon(b.boundingBox),
    confidence: b.confidence,
    lang: language(b),
    children: paragraphs
  };
}

function paragraph(p) {
  var lines = [];
  var words = [];
  (p.words || []).forEach(function (w) {
    var text = "";
    var brk = null;
    (w.symbols || []).forEach(function (s) {
      text += s.text || "";
      var d = s.property && s.property.detectedBreak;
      if (d) brk = d.type;
    });
    words.push({
      level: "word",
      text: text,
      polygon: polygon(w.boundingBox),
      confidence: w.confidence,
      lang: language(w),
      // What follows the word: a space, or nothing (CJK has no spaces).
      sep: brk === "SPACE" || brk === "SURE_SPACE" ? " " : ""
    });
    if (brk === "EOL_SURE_SPACE" || brk === "LINE_BREAK" || brk === "HYPHEN") {
      if (brk === "HYPHEN") words[words.length - 1].text += "-";
      lines.push(line(words));
      words = [];
    }
  });
  if (words.length) lines.push(line(words));
  return {
    level: "paragraph",
    text: lines.map(function (l) { return l.text; }).join("\n"),
    polygon: polygon(p.boundingBox),
    confidence: p.confidence,
    children: lines
  };
}

function line(words) {
  var text = words.map(function (w) { return w.text + w.sep; }).join("").trim();
  var xs = [], ys = [];
  words.forEach(function (w) {
    w.polygon.forEach(function (pt) { xs.push(pt[0]); ys.push(pt[1]); });
    delete w.sep;
  });
  var bbox = xs.length ? [Math.min.apply(null, xs), Math.min.apply(null, ys), Math.max.apply(null, xs), Math.max.apply(null, ys)] : undefined;
  return { level: "line", text: text, bbox: bbox, children: words };
}

// Vision leaves out a coordinate that is zero.
function polygon(box) {
  return ((box && box.vertices) || []).map(function (v) { return [v.x || 0, v.y || 0]; });
}

function language(node) {
  var langs = node && node.property && node.property.detectedLanguages;
  return langs && langs.length ? langs[0].languageCode : undefined;
}

function errorMessage(r) {
  try {
    var e = r.json().error;
    if (e && e.message) return e.message;
  } catch (ignored) {}
  return String(r.body || "").substring(0, 200);
}
