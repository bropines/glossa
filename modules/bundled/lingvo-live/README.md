# ABBYY Lingvo Live (`lingvo-live`)

**How it gets its answers: an unofficial endpoint.**

Word cards from the Lingvo dictionaries. It calls the JSON endpoint behind
lingvolive.com that the site's own pages use for their cards. It is not a
documented API: it can change or close without notice.

- **Key or account:** none.
- **What leaves your machine:** The word and the two languages, to `api.lingvolive.com`.
- **Limits:** One request per lookup. No quota is published. German lookups fail on
  the server (November 2026), so German is not offered.
- **Content:** The dictionaries belong to ABBYY and their publishers.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
