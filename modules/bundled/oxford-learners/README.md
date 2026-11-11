# Oxford Learner's Dictionary (`oxford-learners`)

**How it gets its answers: HTML scraping.**

English definitions, pronunciation and examples from the Oxford Advanced
Learner's Dictionary. The module downloads the public entry page and reads
it out of the HTML; Oxford's API is a paid product and is not used. When
the page's markup changes, lookups come back as "not found" until the
module is updated.

- **Key or account:** none.
- **What leaves your machine:** The word, in the address of a page on `www.oxfordlearnersdictionaries.com`.
- **Limits:** Up to three pages per lookup (the word in each part of speech). The site
  can be slow and may show a bot check instead of the entry.
- **Content:** The entries are copyright Oxford University Press; the module shows
  them with a link to the page.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
