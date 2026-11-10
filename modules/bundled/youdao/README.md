# Youdao (`youdao`)

**How it gets its answers: an unofficial endpoint.**

Youdao's dictionary: English and French words explained in Chinese,
Chinese words in English. It calls the JSON endpoint dict.youdao.com's own
pages load. It is not a documented API: it can change or close without
notice. Because one side is always Chinese, a lookup into any other
language is refused as an unsupported target and Glossa moves on to the
next dictionary.

- **Key or account:** none.
- **What leaves your machine:** The word, to `dict.youdao.com` (servers in China).
- **Limits:** One request per lookup. No quota is published.
- **Content:** The entries belong to NetEase Youdao and the dictionaries it licenses.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
