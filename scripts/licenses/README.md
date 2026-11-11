# scripts/licenses

Licence texts `scripts/third-party-notices.py` falls back on when a crate
ships none of its own, and texts for code that is not a crate.

| file | what | from |
|---|---|---|
| `Apache-2.0.txt` | the Apache License 2.0, appendix not filled in | apache.org's text, as the `log` crate ships it |
| `MPL-2.0.txt` | the Mozilla Public License 2.0 | Symphonia's `LICENSE` (its crates do not carry it) |
| `BSL-1.0.txt` | the Boost Software License 1.0 | boost.org's text, as the `ryu` crate ships it |
| `MIT.txt` | the MIT License with `@HOLDERS@` for the copyright line | the standard text; the script fills in the crate's `authors` |
| `objc2-LICENSE-*.txt` | the licences of the objc2 crates (objc2, block2, objc2-foundation, objc2-app-kit…), which are published without them | <https://github.com/madsmtm/objc2> `LICENSE-MIT.txt`, `LICENSE-APACHE.txt`, `LICENSE-ZLIB.txt` |
| `chromium-LICENSE.txt` | Chromium's licence, for the `lens_server_proto` definitions the crate chrome_lens_ocr vendors without it | <https://github.com/chromium/chromium> `LICENSE` |
| `fastframe-MIT.txt` | fastframe's licence, for the code adapted in `crates/glossa-ui-egui/src/scroll.rs` | <https://github.com/crmne/fastframe> `LICENSE` |

The Apache, MPL and Boost texts are the same for every project, so using them for
a crate that names the licence but ships no file gives its exact terms.
`MIT.txt` is different: the copyright line is built from the crate's
manifest because its authors published no notice. The script's `--summary`
names every crate treated that way; check a new one against its upstream
repository first.
