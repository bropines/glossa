# ModernMT (`modernmt`)

**How it gets its answers: an unofficial endpoint.**

ModernMT with no key or account. It posts the form of the public page
modernmt.com/translate and reads the page's own data route. That route is
part of the website, not an API: it can change or close without notice.

- **Key or account:** none.
- **What leaves your machine:** The text and the language codes, to `www.modernmt.com`.
- **Limits:** 5,000 characters a request here. No quota is published; HTTP 429 is
  reported as a rate limit.
- **Content:** Translated's terms apply to what it returns. ModernMT's documented API
  needs a key and is not used here.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
