# Academic (`dic-academic`)

**How it gets its answers: HTML scraping.**

A search across the dictionaries and encyclopedias collected on
dic.academic.ru. The module downloads the site's search page and reads the
results out of its HTML. The site offers no API; when the page's markup
changes, lookups come back as "not found" until the module is updated.

- **Key or account:** none.
- **What leaves your machine:** The word and the source language, in the address of a page on
  `dic.academic.ru`.
- **Limits:** One page per lookup. No quota is published.
- **Content:** Each entry is shown under the name of the dictionary it came from, whose
  publisher holds the rights.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
