# Google Cloud Vision OCR (`google-vision-ocr`)

**How it gets its answers: an official API, with a key.**

Text recognition by the [Cloud Vision API](https://cloud.google.com/vision/docs/ocr)
(`POST https://vision.googleapis.com/v1/images:annotate`): the full layout
down to words, with confidence. Also the example of an OCR module in
JavaScript with a rich settings page (tabs, a slider, conditions, two
buttons) and a Russian translation.

- **Key:** needed. In the [Google Cloud console](https://console.cloud.google.com/):
  create or pick a project, attach a billing account to it (Google asks
  for one even for the free allowance), enable *Cloud Vision API* under
  *APIs & Services → Library*, then *APIs & Services → Credentials →
  Create credentials → API key*. Restrict the key to the Cloud Vision API
  on the same page. Paste it into *Options → Services → Google Cloud Vision
  OCR → API key*; *Test the key* asks Vision whether it accepts it. Glossa
  keeps the key in `config.toml`, or in the system keychain when you chose
  that.
- **What leaves your machine:** the picture you capture, to
  `vision.googleapis.com`, with the language hints and the key. It goes
  through Glossa's own connection (your proxy setting applies). Small
  captures are enlarged and very large ones reduced first (*Longest side
  sent*), so what is uploaded may be a resized copy. How Google handles
  what the API receives is in its
  [data usage FAQ](https://cloud.google.com/vision/docs/data-usage); do
  not capture what Google should not see.
- **Limits and cost (Google's price list, November 2026; check
  [there](https://cloud.google.com/vision/pricing)):** each picture is one
  unit. The first 1,000 units a month are free; after that text detection
  is billed at 1.50 USD per 1,000 units. Requests over the project's quota
  answer with HTTP 429, which the module reports as a rate limit. Set a
  budget alert in the console if the key is used elsewhere too.
- **Code:** MIT.

## Setting it up

1. Copy this folder into your modules folder (*Options → Modules → Open
   folder*) and *Reload modules*; approve it when asked.
2. Enter the key as above and switch the service on under text
   recognition.

It needs Glossa 0.1.0-beta.2 or newer (the settings page's controls).

## Testing

```bash
glossa-cli module test modules/js/google-vision-ocr          # offline, recorded answers
VISION_KEY=… glossa-cli module test --live --set-from-env api_key=VISION_KEY modules/js/google-vision-ocr
```

The fixtures use a fake key. Never put a real one into a file here.
