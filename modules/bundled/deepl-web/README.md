# DeepL (`deepl-web`)

**How it gets its answers: an unofficial endpoint.**

DeepL translation with no key or account. It calls the free "one-shot"
endpoint of DeepL's iOS app and describes itself as that app. DeepL does
not document or promise this endpoint: it can change or close without
notice. When DeepL starts refusing the request (HTTP 403), the app version
in `APP` at the top of `main.js` is the first thing to refresh; the comment
there says where the numbers come from.

- **Key or account:** none.
- **What leaves your machine:** The text, the language codes, the app description and a random id made
  once per run, to `oneshot-free.www.deepl.com`.
- **Limits:** 1,500 characters a request here. DeepL pauses the service after a burst of
  requests (HTTP 429) and does not offer it from every country (HTTP 451);
  both are reported in words.
- **Content:** DeepL's terms apply to what it returns. The documented API, with a key, is
  the separate built-in service "DeepL API (key)".
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
