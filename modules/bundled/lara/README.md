# Lara (`lara`)

**How it gets its answers: an unofficial endpoint.**

Lara Translate (Translated's model) with no key or account. It posts to
the endpoint laratranslate.com's own page calls. Translated does not
document or promise that endpoint for other clients: it can change or
close without notice.

- **Key or account:** none.
- **What leaves your machine:** The text and the language codes, to `webapi.laratranslate.com`.
- **Limits:** 5,000 characters a request here. No quota is published; HTTP 429 is
  reported as a rate limit.
- **Content:** Translated's terms apply to what it returns.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
