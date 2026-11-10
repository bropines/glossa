# Multitran (`multitran`)

**How it gets its answers: HTML scraping.**

Translations of words and phrases by subject area from Multitran. The
module downloads the same result page a browser would and reads the
translations out of its HTML. Multitran offers no API; when the page's
markup changes, lookups come back as "not found" until the module is
updated.

- **Key or account:** none.
- **What leaves your machine:** The word and the two languages, in the address of a page on
  `www.multitran.com`.
- **Limits:** One page per lookup. No quota is published; be gentle, it is a
  volunteer-run dictionary.
- **Content:** The entries belong to Multitran and its contributors; the module shows
  them with a link to the page.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
