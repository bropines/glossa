# Urban Dictionary (`urban-dictionary`)

**How it gets its answers: an undocumented public API.**

English slang from Urban Dictionary, most upvoted definitions first. It
calls `api.urbandictionary.com/v0/define`, a JSON interface the site
serves. It answers without a key, but Urban Dictionary publishes no
documentation or promise for it: it can change or close without notice.

- **Key or account:** none.
- **What leaves your machine:** The word, to `api.urbandictionary.com`.
- **Limits:** One request per lookup. No quota is published.
- **Content:** Definitions are written by Urban Dictionary's users, are not moderated
  for accuracy and are often crude. They belong to Urban Dictionary and
  their authors.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
