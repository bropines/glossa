// {{name}}: Glossa text recognition.
//
// Glossa calls recognize(req, host) with
//   req = { image_base64, image_handle, lang, granularity, options }
// and expects { text, blocks } back. host.image prepares the picture.
// See docs/modules/ocr-modules.md.

function recognize(req, host) {
  var key = host.options.api_key;
  if (!key) throw new ModuleError("missing_credential", "api_key");

  var img = req.image_handle ? host.image.info(req.image_handle) : host.image.fromBase64(req.image_base64);
  img = host.image.resize(img, { max_side: 4000 });
  var upload = host.image.encode(img, { format: "png" });

  var r = host.fetch("https://api.example.com/v1/ocr", {
    method: "POST",
    headers: { Authorization: "Bearer " + key },
    json: { image: upload.data_base64, language: req.lang === "auto" ? null : req.lang },
    timeout: 30000
  });
  if (r.status === 401 || r.status === 403) throw new ModuleError("missing_credential", "api_key");
  if (!r.ok) throw new ModuleError("http", "HTTP " + r.status);

  var lines = r.json().lines || [];
  if (!lines.length) throw new ModuleError("not_found", "no text in the picture");
  console.log("recognised " + lines.length + " lines");
  return {
    text: lines.map(function (l) { return l.text; }).join("\n"),
    blocks: lines.map(function (l) {
      return { level: "line", text: l.text, bbox: l.box };
    })
  };
}
