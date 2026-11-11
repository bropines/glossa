# Wikipedia (`wikipedia`)

**How it gets its answers: an official API.**

The opening of the Wikipedia article on a word or name, through the
Wikimedia REST API (`/api/rest_v1/page/summary/`), a public and
documented interface. The module identifies itself with an
`Api-User-Agent` header, as Wikimedia asks.

- **Key or account:** none.
- **What leaves your machine:** The word, in the address of a request to the source language's
  Wikipedia (`<language>.wikipedia.org`).
- **Limits:** One request per lookup. HTTP 429 is reported as a rate limit.
- **Content:** Wikipedia's text is under CC BY-SA 4.0; the article links to its page.
- **Code:** MIT OR Apache-2.0, like Glossa.

Bundled with Glossa: `module.toml` and `main.js` are compiled into the
program, and this file is for people reading the source. To change the
module, copy the folder into your modules folder
([how](../../../docs/modules/README.md#bundled-modules)).
