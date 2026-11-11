# Academic Translations (`translate-academic`)

**How it gets its answers: HTML scraping.**

A word's entries in the bilingual dictionaries on translate.academic.ru.
The module downloads the site's result page and passes the entries on as
HTML, without scripts. The site offers no API; when the page's markup
changes, lookups come back as "not found" until the module is updated.

- **Key or account:** none.
- **What leaves your machine:** The word and the two languages, in the address of a page on
  `translate.academic.ru`.
- **Limits:** One page per lookup, which for a common word is a few hundred kilobytes.
  No quota is published.
- **Content:** Each entry names the dictionary it came from, whose publisher holds the
  rights.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
