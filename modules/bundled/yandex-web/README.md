# Yandex (`yandex-web`)

**How it gets its answers: an unofficial endpoint.**

Translation, language detection and a dictionary for single words from
Yandex Translate, with no key. It calls `translate.yandex.net` the way
Yandex's own browser clients do, with a client name instead of an API key.
Yandex does not document or promise this endpoint: it can change or close
without notice, and then this module stops working until it is updated.

- **Key or account:** none.
- **What leaves your machine:** The text to translate or look up and the two language codes, to
  `translate.yandex.net`.
- **Limits:** About 10,000 characters a request (longer text is sent in pieces). Yandex
  publishes no quota for this endpoint; a captcha page or HTTP 429 means it
  wants a pause.
- **Content:** Yandex's terms of use apply to what it returns. The keyed, documented API
  is the separate built-in service "Yandex Cloud (API key)".
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
