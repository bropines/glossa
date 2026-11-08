// {{name}}: a Glossa translator.
//
// Glossa calls translate(req, host) with
//   req = { text, from, to, options }   (from may be "auto")
// and expects { text, detected_from } back, or a plain string.
// host.fetch is synchronous; console.log lines show in the editor's Log.

function translate(req, host) {
  var key = host.options.api_key;
  if (!key) throw new ModuleError("missing_credential", "api_key");

  var r = host.fetch("https://api.example.com/v1/translate", {
    method: "POST",
    headers: { Authorization: "Bearer " + key },
    json: { text: req.text, source: req.from === "auto" ? null : req.from, target: req.to },
    timeout: 20000
  });
  if (r.status === 401 || r.status === 403) throw new ModuleError("missing_credential", "api_key");
  if (r.status === 429) throw new ModuleError("rate_limited", "too many requests");
  if (!r.ok) throw new ModuleError("http", "HTTP " + r.status);

  var data = r.json();
  console.log("translated " + req.text.length + " characters");
  return { text: data.translation, detected_from: data.detected_source || null };
}
