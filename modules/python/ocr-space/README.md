# OCR.space (`ocr-space`)

**How it gets its answers: an official API, with a key.**

Text recognition by [OCR.space](https://ocr.space/ocrapi), through its
documented API (`POST https://api.ocr.space/parse/image`): lines and words
with their boxes. Also the example of an OCR module in Python: it fits the
picture into the service's limits with Glossa's image API and maps the
boxes back onto the original.

- **Key:** needed, free. Fill in the form at
  <https://ocr.space/ocrapi/freekey>; the key arrives by e-mail. Paste it
  into *Options → Services → OCR.space → API key*. Glossa keeps it in
  `config.toml`, or in the system keychain when you chose that.
- **What leaves your machine:** the picture you capture, to
  `api.ocr.space`, with the language and the key. It goes through Glossa's
  own connection (your proxy setting applies). A capture over the free
  plan's file size is shrunk or re-encoded first, so what is uploaded may
  be a smaller copy. What OCR.space does with uploads is in its
  [privacy policy](https://ocr.space/privacypolicy); do not capture what it
  should not see.
- **Limits (free plan, as its page said in November 2026; check there):**
  25,000 requests a month and 500 a day from one address, files up to
  1 MB. Past a limit the service
  answers with an error, which the module reports as a rate limit.
- **Cost:** the free plan costs nothing; the paid plans lift the limits.
- **Engines:** *Engine 2* detects the language itself and is better with
  Latin scripts; *Engine 1* knows more languages but must be told which.
- **Code:** MIT.

## Setting it up

1. Copy this folder into your modules folder (*Options → Modules → Open
   folder*) and *Reload modules*; approve it when asked.
2. Enter the key as above and switch the service on under text
   recognition.

It needs a Python 3.8 or newer; Glossa finds one on the machine or
downloads its own. The module has no packages to install.

## Testing

```bash
glossa-cli module test modules/python/ocr-space             # offline, recorded answers
OCR_SPACE_KEY=… glossa-cli module test --live --set-from-env api_key=OCR_SPACE_KEY modules/python/ocr-space
```

The fixtures use a fake key. Never put a real one into a file here.
