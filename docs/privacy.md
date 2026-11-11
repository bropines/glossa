# Privacy and data

What Glossa sends over the network, to whom and when; what it keeps on the
disk and where; how to work without the network; and how to delete what it
keeps. For vulnerabilities and the threat model see
[SECURITY.md](../SECURITY.md); for locking an installation down see
[Deployment](deployment.md).

- [In short](#in-short)
- [What leaves the machine](#what-leaves-the-machine)
- [Requests you might not expect](#requests-you-might-not-expect)
- [What is stored on the machine](#what-is-stored-on-the-machine)
- [Working offline](#working-offline)
- [Clearing data](#clearing-data)

## In short

- **No telemetry.** Glossa has no analytics, usage statistics or crash
  reporting. A crash is written to the local log file and goes nowhere.
- **No update check unless you ask for one.** Glossa asks GitHub whether
  a newer version exists only when you press *Check now*, run `glossa-cli
  update`, or switch the daily check on (`[updates] check_automatically`,
  off by default). It never downloads or installs a version itself.
- **No account, no Glossa server.** There is no service of the project's
  own between you and the translation services. Requests go straight from
  your machine (through your proxy, if one is set) to the service you
  picked.
- **A translator has to send the text somewhere.** Every online service
  receives the text it translates, and Google Lens receives the picture of
  the screen region it reads. The services' own privacy policies apply to
  what they do with it; Glossa has no say there. Most built-in services
  are reached without a key through the same addresses their websites use.
- **Nothing is downloaded on its own.** Module files, the managed Python
  and Python packages are fetched only when you press *Download* (or when
  `[modules] auto_download_assets` is on and a module is first used).

## What leaves the machine

A request is made when you use the feature, and to the service that is
selected for it. *Default* marks what a fresh installation has switched on.

### Translation

The text to translate, the language pair and, where the service takes it,
the interface language. With a key, the key goes to that service only.

| service (id) | contacts | default |
|---|---|---|
| Google Translate (`google`) | `clients5.google.com`, `translate.googleapis.com`, `translate.google.<domain>` (`[advanced] preferred_domain`, `com` unless changed), tried in turn | on, and the active one |
| Microsoft Translator (`bing`) | `www.bing.com`; with your own key `api.cognitive.microsofttranslator.com` | on |
| MyMemory (`mymemory`) | `api.mymemory.translated.net` | on |
| Yandex (`yandex-web`) | `translate.yandex.net` | on |
| DeepL (`deepl-web`) | `oneshot-free.www.deepl.com` | on |
| Lara (`lara`) | `webapi.laratranslate.com` | on |
| ModernMT (`modernmt`) | `www.modernmt.com` | on |
| DeepL API (`deepl`) | `api-free.deepl.com` or `api.deepl.com`, by the kind of key | off (needs a key) |
| Yandex Cloud (`yandex`) | `translate.api.cloud.yandex.net` | off (needs a key) |
| LibreTranslate (`libretranslate`) | the server you set; `libretranslate.com` until you set one | off |
| Lingva (`lingva`) | the instance you set; `lingva.ml` until you set one. Lingva is a proxy in front of Google Translate | off |
| DeepLX (`deeplx`) | the server you set; `127.0.0.1:1188` until you set one. DeepLX itself forwards to DeepL | off |
| AI endpoints (`ai-…`, `openai`) | the base URL you typed or the preset's: `api.openai.com`, `openrouter.ai`, `api.deepseek.com`, `generativelanguage.googleapis.com`, `api.groq.com`, `api.mistral.ai`, or a local Ollama or LM Studio on `localhost` | none exist until you add one |

"On" means the service is in the service strip. Only the selected service
is asked, with two exceptions: when it fails and *If a service fails, try
the next one* is on (`[services] fail_over`, on by default), the next
enabled one gets the same text; and with *Show all services at once* every
enabled service gets it.

An AI endpoint receives the text inside a prompt, together with your custom
prompt, style and glossary if you set them, and the model name. The
OpenRouter preset also sends the headers `HTTP-Referer:
https://github.com/bropines/glossa` and `X-Title: Glossa`, which tell
OpenRouter which program is calling; they can be edited or removed in the
endpoint's *Extra headers* setting.

### Dictionaries

The word or phrase looked up and the language pair.

| dictionary (id) | contacts |
|---|---|
| Google Dictionary (`google-dictionary`) | the same three Google addresses as Google Translate |
| Yandex (`yandex-web`) | `translate.yandex.net` |
| Multitran (`multitran`) | `www.multitran.com` |
| ABBYY Lingvo Live (`lingvo-live`) | `api.lingvolive.com` |
| Oxford Learner's Dictionary (`oxford-learners`) | `www.oxfordlearnersdictionaries.com` |
| Wiktionary (`wiktionary`) | `en.wiktionary.org` |
| Youdao (`youdao`) | `dict.youdao.com` |
| Academic (`dic-academic`), Academic Translations (`translate-academic`) | `dic.academic.ru`, `translate.academic.ru` |
| Wikipedia (`wikipedia`) | `<language>.wikipedia.org` |
| Urban Dictionary (`urban-dictionary`) | `api.urbandictionary.com` |
| Offline dictionaries (StarDict, DSL, XDXF, TSV files) | nothing: they are read from the disk |

All the online ones are switched on in a fresh installation; the one
selected in the dictionary window is asked.

### Text recognition (OCR)

| engine (id) | what goes where |
|---|---|
| Google Lens (`lens`), the default | the picture of the region, as a JPEG, to `lensfrontend-pa.googleapis.com`, with the target language when Lens is also asked to translate. In the *Auto* mode a running [chrome-lens-py](https://github.com/bropines/chrome-lens-py) daemon on this machine is used instead when there is one, and it sends the picture to Google itself |
| Windows OCR, OneOCR, Apple Vision, Tesseract | nothing: they run on the machine |
| External command (`command-ocr`) | whatever the command you configured does |
| AI endpoints (vision models) | the picture, to the endpoint you chose for it |
| OCR modules | what the module declares: the PaddleOCR module works on the machine, the OCR.space example sends the picture to `api.ocr.space`, the Google Cloud Vision example to `vision.googleapis.com` |

After recognition the text is translated like any other text, by Lens in
the same request or by the selected translator (`[ocr] translate_with`).
`[ocr] save_image_dir` also saves every captured region to a folder; it is
unset by default.

### Listening (text to speech)

| voice (id) | what goes where |
|---|---|
| Google (`google`), the default | the text, in pieces of up to 200 characters, to `translate.google.<domain>`, `translate.googleapis.com` or `clients5.google.com` |
| System voices (`system-tts`) | nothing: the Windows voices, macOS's `say`, or espeak-ng on Linux |

### Modules

- **JavaScript modules** (the bundled services above are such modules) can
  reach only the hosts listed in their `module.toml`, through Glossa's own
  HTTP client and your proxy settings.
- **Python, Java and process modules** are ordinary programs running as
  you. Their requests through Glossa are limited to the declared hosts, but
  nothing stops such a program from opening its own connections or reading
  your files. So are **native modules**, which run inside Glossa.
- **Classic QTranslate scripts and pot-app plugins** may reach any host, as
  they could in the programs they come from.
- A module installed by hand does not run until you approve what its
  `module.toml` asks for
  ([Permissions and trust](modules/README.md#permissions-and-trust)).

### Downloads

Made only when you ask for them (or with `[modules] auto_download_assets`).
Each download is checked against a SHA-256 written in the module or in
Glossa before it is used.

| what | from |
|---|---|
| The PaddleOCR module's program | `github.com` (this repository's releases) |
| The PaddleOCR models | `www.modelscope.cn` |
| The Local LLM module's program and llama.cpp | `github.com` (this repository's and llama.cpp's releases) |
| The Local LLM module's model | `huggingface.co` |
| The managed Python and `uv`, for Python modules | `github.com` (`astral-sh/python-build-standalone`, `astral-sh/uv`) |
| Python packages a module lists | `pypi.org` and the hosts PyPI serves files from, or the index the module names; fetched by `uv` |
| Files of other modules | the URLs in the module's `[[assets]]`, shown before you approve it |

These sites answer with redirects to their own storage hosts (GitHub's
release assets, Hugging Face's and ModelScope's file servers); Glossa
follows them. A download request carries no text of yours, only what any
HTTP request does: your address and Glossa's user agent.

### The update check

One request to `api.github.com` (the list of this project's releases),
through your proxy if one is set. It carries nothing about you, your
settings or your texts; GitHub sees your address, as any site does. It is
made when you press *Check now* in *Options → Updates* or run `glossa-cli
update`, and once a day, a little after the start, only if
`[updates] check_automatically` is on. It is off in a fresh installation.
The answer is kept in `update-check.json`. A newer version is named, and
its release page opens in the browser when you ask; nothing is downloaded.

### Links

*Help*, *Support the project*, *Open on GitHub* and "open in browser"
entries hand a URL to your browser. Glossa makes no request for them
itself. Opening a translation in the browser puts the text into that URL.

### The control API

The [control API](control-api.md) listens on `127.0.0.1:60828` and is on
by default. It accepts requests from this machine and does not call out.
On Linux Glossa also offers a D-Bus service on the session bus.

## Requests you might not expect

Things worth knowing before relying on "I switched that service off":

- **Language detection may go to another service.** When the source
  language is *Auto-detect* and either the target is automatic or smart
  direction is on (it is by default), or *always detect* is on, the text is
  first sent for detection. The selected service is asked if it can detect
  (Google, Microsoft, Yandex and LibreTranslate can; the others and AI
  endpoints cannot), then the other enabled ones, and **if none of them
  can, any installed service that can**, which in practice means Google
  Translate even when it is switched off in the list. To prevent it, choose
  the source language yourself and turn smart direction off, or remove the
  service altogether with `[modules] disabled`
  ([Working offline](#working-offline)).
- **Text recognition falls over to other engines.** When the chosen OCR
  engine fails in a way another might not (it is not installed, not set up,
  or the network is down), the others are tried: Google Lens first, then
  the system's engine, Tesseract and the external command. With an offline
  engine chosen, a failure can therefore send the picture to Google Lens.
  OneOCR and AI endpoints are never used as a fallback. This does not
  follow the fail-over switch. `[modules] disabled = ["lens"]` removes
  Lens.
- **Listening falls back too.** When the chosen voice fails, any other
  service that can speak is used: with the system voice chosen and
  failing, the text goes to Google.
- **The built-in *Private* preset is not offline.** It switches to Lingva
  and MyMemory, Wiktionary and Wikipedia, and the system voice: fewer and
  different parties, but the text still leaves the machine. With no
  instance of your own set, Lingva means the public `lingva.ml`.
- **Fail-over** sends the same text to the next enabled service when the
  first one fails. Turn it off in *Options → Services* if each text should
  go to one service only.

## What is stored on the machine

Everything is in the data folder and the cache folder
([where they are](configuration.md#where-the-files-are)):

| | Windows | macOS | Linux |
|---|---|---|---|
| data | `%APPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.config/glossa` |
| cache and log | `%LOCALAPPDATA%\Glossa` | `~/Library/Caches/Glossa` | `~/.cache/glossa` |
| module downloads | `%LOCALAPPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.local/share/glossa` |

A portable copy, or one started with `GLOSSA_HOME`, keeps all of it in one
folder instead.

| what | where | notes |
|---|---|---|
| Settings | `config.toml` (data) | Plain text. Includes the control API token, which always stays in this file. |
| API keys, the proxy password, the Lens daemon token | `config.toml`, in plain text, by default | With *Options → Basics → Keys and privacy* set to the system keychain (`[general] secret_storage = "system"`) they are kept in Windows Credential Manager (as `Glossa/…`), the macOS login Keychain (service `Glossa`) or the Secret Service on Linux, and the file holds `@system-keychain` in their place. Where no keychain answers they stay in the file. |
| Translation history | `history.json` (data) | Every translation the desktop program shows: source text, translations, service, languages, time. Up to `[history] max_entries` (5000). Plain text. `glossa-cli` and the control API add entries only when asked. |
| Dictionary look-ups | `dictionary-history.json` (data) | The words looked up in the dictionary window. |
| The main window's text | `[contents]` in `config.toml` | The source and translation panes, kept across restarts while *Save contents on exit* is on (it is by default). |
| The last update check | `update-check.json` (data) | When GitHub was last asked and what it answered. Safe to delete. |
| Module approvals | `[modules.granted]` in `config.toml` | What you approved for each module. |
| Captured regions | the folder in `[ocr] save_image_dir` | Only if you set one. |
| The log | `glossa.log` (cache) | Appended to and never trimmed by Glossa. Records events, errors and crashes, not the text you translate; request addresses are logged without their query string. What a Python, Java or process module prints is copied into it, so a module can put text there. `GLOSSA_LOG=debug` makes it more detailed. |
| Module files, the managed Python, Python environments | `models/`, `runtimes/`, `venvs/` | Downloaded programs and models. |
| Other caches | cache folder | Module caches, unpacked `.potext` archives, OneOCR's copied libraries, `uv`'s download cache. |
| The Wayland portal token | `remote-desktop-token` (data) | Linux only. |
| Autostart entry | the `Run` registry value, `~/Library/LaunchAgents/app.glossa.plist` or `~/.config/autostart/glossa.desktop` | Only when starting at login is on. |

On a shared machine, keep keys in the system keychain so that they are not
in a plain-text file at all. Presets, when exported, never contain keys.

## Working offline

Glossa starts and runs without a network; only the online services fail.
What works on the machine alone:

| for | use |
|---|---|
| Translation | An AI endpoint pointed at a model server on this machine (the *Ollama* and *LM Studio* presets, `localhost`), or a LibreTranslate server of your own. The Local LLM module is in the repository but its program is not released yet. |
| Dictionaries | StarDict, DSL, XDXF and tab-separated files: *Options → Services → Dictionaries → Add dictionary...* |
| Text recognition | Windows OCR or OneOCR on Windows, Apple Vision on macOS, Tesseract anywhere, or the PaddleOCR module once its files are downloaded: *Options → Text recognition → Provider* |
| Listening | The system voices: *Options → Services → Speech* |

Choosing these is not by itself a guarantee, because of the fallbacks
[above](#requests-you-might-not-expect). To make sure nothing reaches the
online services, take them out altogether. `[modules] disabled` in
`config.toml` lists ids that are not loaded at all, and it applies to
built-in and bundled services as well as to modules:

```toml
[modules]
disabled = [
  "google", "google-dictionary", "bing", "mymemory", "lingva", "libretranslate",
  "deepl", "deeplx", "yandex", "lens",
  "yandex-web", "deepl-web", "lara", "modernmt", "multitran", "lingvo-live",
  "oxford-learners", "wiktionary", "youdao", "dic-academic",
  "translate-academic", "wikipedia", "urban-dictionary",
]
```

Edit the file while Glossa is not running. `glossa-cli services` then shows
what is left. Add `"openai"` to remove the built-in AI translator too;
endpoints you add yourself are listed by their own ids (`ai-…`). There is
no single "offline mode" switch, and a newer version may add services with
new ids, so check the list after upgrading. A firewall rule for `glossa`
and `glossa-cli` is the stronger guarantee
([hosts by feature](deployment.md#network-access)).

## Clearing data

| to remove | do |
|---|---|
| The translation history | The history window's *Clear* removes every entry that is not a favourite; delete single entries from their context menu. To remove everything, quit Glossa and delete `history.json`. *Options → Basics → Clear history on exit* empties it at every quit; *Enable history* off stops recording. |
| Dictionary look-ups | The dictionary window's *Clear history*, or delete `dictionary-history.json`. |
| The main window's saved text | Turn off *Save contents on exit* in the main window's menu, or delete `[contents]` from `config.toml`. |
| Keys | Clear the fields in *Options → Services* and apply. With the keychain in use, that deletes the keychain entry as well; entries can also be removed in Credential Manager, Keychain Access or your desktop's password manager (look for *Glossa*). |
| The log and caches | Quit Glossa and delete the cache folder. On Windows the module downloads sit in the same folder (`models`, `runtimes`, `venvs`); they can be downloaded again. |
| Module files | The module's settings, *Delete files*; or `glossa-cli assets remove ID` and `glossa-cli python remove`. |
| Everything | Quit Glossa and delete the data and cache folders (and the module downloads folder where it is separate). [Removing Glossa](install.md#removing-glossa-completely) lists the rest: the autostart entry, keychain entries and the macOS permission entries. |

`glossa-cli config path` prints the data folder in use, and `glossa-cli
doctor` the cache folder and the log.
