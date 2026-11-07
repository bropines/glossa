# Google Translate without a key

What the built-in `google` and `google-dictionary` services call, what
Google does when it decides we are asking too much, and how other free
clients cope. Written while fixing "Google Translate is rate-limiting us"
(November 2026). Observations are from a Mac on a residential connection
in Russia, on 2026-11-07; Google changes these endpoints without notice, so
treat them as a snapshot.

## What triggers the message

`Error::RateLimited` ("{provider} is rate-limiting us") comes from
`HttpResponse::error_for_status` in `glossa-net`: status 429, or 503 with a
`Retry-After`. Google's refusal is a redirect to
`https://www.google.com/sorry/index?continue=…`, the "unusual traffic"
captcha page, which itself answers **429** with `text/html`. The HTTP
client follows redirects, so the provider sees the 429. The captcha has
also been reported to come back with status 200; none of these endpoints
answers HTML otherwise, so an HTML body (or one mentioning `/sorry/index`
or "unusual traffic") is treated as the same refusal.

Before this change two things made it worse:

- Only one address was asked (`translate.googleapis.com`, `client=gtx`).
  Once it refused, every translation failed until the block lifted.
- The HTTP client retried a 429 (`[network] max_retries`, after 0.6 s),
  so every refused popup asked twice, which keeps such a block alive.

## Keyless endpoints, as observed

All of these took the same query (`sl`, `tl`, `hl`, `dt=…`, `ie`, `oe`,
`q`) and, for long text, the same `q` in a form body (POST): 144 000
characters were answered in one request.

| endpoint | `client` | answer | notes |
|---|---|---|---|
| `https://translate.googleapis.com/translate_a/single` | `gtx` | the positional array the `google` module parses | what the browser extensions and most free clients use |
| `https://clients5.google.com/translate_a/single` | `dict-chrome-ex` | the same array, every `dt=` flag honoured | Google Dictionary extension's client id |
| `https://translate.google.<tld>/translate_a/single` | `gtx` | the same array | the user-facing site's host |
| `https://translate.googleapis.com/translate_a/t`, `https://clients5.google.com/translate_a/t` | `gtx`, `dict-chrome-ex` | `[[text, detected], …]`, one entry per `q=` | several texts in one request; no romanisation or dictionary data |
| `https://translate.google.com/m` | — | the mobile HTML page | had already sent this address to the captcha while the others answered |
| `https://<host>/translate_tts` | `tw-ob` on `translate.google.*`, `gtx` on `translate.googleapis.com`, `dict-chrome-ex` on `clients5.google.com` | `audio/mpeg` | 200 characters per request |

Left out: `translate-pa.googleapis.com/v1/translateHtml` (sent with a web
API key in an `X-Goog-Api-Key` header), the `client=t`/`client=webapp`
variants that need a computed `tk` token, and the batchexecute RPC
(`translate.google.com/_/TranslateWebserverUi/data/batchexecute`, rpcid
`MkEWBc`), which needs values scraped from the page. AGENTS.md rules out
committing keys, and the keyless addresses were enough.

## Who gets refused

The block is not only about volume. From one address, at the same minute:

- `curl` (SecureTransport) and Python's `urllib` (OpenSSL) got answers from
  every `client=gtx` address.
- Glossa's client (ureq with rustls) got the captcha from
  `translate.googleapis.com` and `translate.google.com` (`client=gtx`) on
  every request, with any of six `User-Agent`s (or the default), while `clients5.google.com` (`client=dict-chrome-ex`) and all three
  speech addresses answered it.
- Python's `http.client` with the same headers as `urllib` got answers on
  one run and the captcha on the next.

So Google appears to score the client as a whole (the TLS handshake among
other things), per endpoint and `client` id, as well as the address's
request rate. Which addresses refuse differs between machines and changes
over time, which is why the fix asks several in turn rather than choosing
"the good one".

## What Glossa does now

In `crates/glossa-services/src/google.rs` and `backoff.rs`:

- Three addresses for translation, in this order: `clients5.google.com`
  (`dict-chrome-ex`, first because it kept answering above),
  `translate.googleapis.com` (`gtx`), `translate.google.<preferred domain>`
  (`gtx`). Three for speech: `translate.google.<domain>` (`tw-ob`),
  `translate.googleapis.com` (`gtx`), `clients5.google.com`
  (`dict-chrome-ex`). The translator and the dictionary share one table.
- A 429 or the captcha moves on to the next address and leaves the refusing
  one alone for 30 s, doubling with each consecutive refusal up to 10
  minutes, or for the server's `Retry-After` if longer. An answer clears
  it. A 403 or 5xx also moves on, without the cooling-off. No network, a
  timeout or an unreadable answer from the first address is reported at
  once: the next would fail the same way.
- These requests are not retried by the HTTP client on 429
  (`HttpRequest::no_retry_when_limited`).
- When every address is cooling off, the provider answers at once with
  `RateLimited { retry_after_secs }` (the shortest wait) and sends nothing;
  `[services] fail_over` then moves on to the next enabled service, and the
  popup says which service answered and that Google did not.
- Requests carry `Accept` and `Accept-Language` (from the interface
  language) besides the client's browser `User-Agent`. Connections persist
  in the shared ureq agent's pool (15 s idle), so a burst of popups reuses
  one TLS connection per host.
- Long text: up to 10 000 characters per request (POST past 1 800 bytes of
  encoded text), cut after a line break where possible, at most five
  requests (50 000 characters). The old cap was 5 000 characters with the
  rest cut off.

## Other clients

Read from each project's current default branch on GitHub in November 2026.
Only their behaviour is described here; no code was taken from them.

| project | endpoint | long text | on 429 / captcha |
|---|---|---|---|
| Crow Translate (QOnlineTranslator) | `translate.googleapis.com/translate_a/single`, `client=gtx`; speech `translate_tts?client=gtx` | 5 000-character chunks, cut after ". " or a space, sent one after another | no retry, no fall-back; an HTML body or a 503 becomes "suspicious traffic, try later" |
| translate-shell | `http://translate.googleapis.com/translate_a/single`, `client=gtx` (plain HTTP, fixed host); speech `client=gtx` | no splitting found | prints that rate limiting is in effect and stops; follows one redirect, so the captcha ends in a parse error |
| deep-translator | the mobile page `translate.google.com/m`, parsed as HTML | refuses more than 5 000 characters | raises `TooManyRequests`; no retry |
| py-googletrans 4.0.0 | `translate.googleapis.com/translate_a/single`, `client=gtx`, over HTTP/2 (other hosts: `client=webapp` with a `tk` token); batchexecute only on an unreleased branch | no limit | by default quietly returns the source text as the "translation"; no retry |
| pot-desktop | `{custom_url}/translate_a/single`, `client=gtx`; the host is configurable for a mirror, `translate.google.com` by default | no limit | throws "Http Request Error"; no retry, no fall-back (the user can point it at a mirror) |
| Linguist (translate-tools/core) | `translate.googleapis.com/translate_a/t`, `client=dict-chrome-ex`, several `q=` per request (token-free class); others use `client=t`/`te` with a `tk` token; no key | batches of up to 4 000 characters, 300 ms between requests | retries a failed task (default limit 2); no special handling of 429 |
| Subtitle Edit | `translate_a/single`, `client=gtx`, then `clients5.google.com/translate_a/t`, `client=dict-chrome-ex` | per subtitle line | retries 429 and 5xx three times (about 1, 3, 7 s); on the "unusual traffic" page switches to `clients5`, and only then reports the block |
| translatepy | `clients5.google.com/translate_a/t`, `client=dict-chrome-ex`, besides batchexecute and `client=gtx` | — | not checked |

What follows for Glossa:

- Nearly everyone uses `client=gtx` on `translate.googleapis.com`, so that
  address and client id carry the most traffic from free clients, and are
  the likeliest to be watched.
- The clients that hold up when blocked are the ones with somewhere else
  to go, such as Subtitle Edit (to `clients5`/`dict-chrome-ex`). Those that retry the same address
  (Linguist, Subtitle Edit's first step) add to the block; Glossa does not
  retry a refusal and remembers it instead.
- Nobody else keeps a cooling-off memory between requests; Linguist's
  300 ms spacing is the closest thing.
- 5 000 characters per request is a convention taken from the web page's
  limit, not the endpoint's; it answered far more in one form body.
