# Text recognition (OCR)

Glossa reads text off the screen: drag over a region and it recognises what
is written there. The text can go straight to the clipboard, open in the
result window with its translation, or be translated **in place**, painted
over the picture the way Chrome's Google Lens overlay does it.

## Contents

- [Quick start](#quick-start)
- [Actions](#actions)
- [The result window](#the-result-window)
- [Copy mode](#copy-mode)
- [Translation by Lens itself](#translation-by-lens-itself)
- [Translating in place](#translating-in-place)
- [Watching a region](#watching-a-region)
- [Engines](#engines): [Google Lens](#google-lens),
  [Windows OCR](#windows-ocr), [OneOCR](#oneocr-snipping-tool),
  [Apple Vision](#apple-vision-macos), [Tesseract](#tesseract),
  [External command](#external-command),
  [AI endpoints](#ai-endpoints-vision-models)
- [Troubleshooting](#troubleshooting)

## Quick start

Google Lens is the default engine and needs no setup.

1. Press **Ctrl+Alt+O** (Windows: **Win+Alt+O**).
2. Drag over the text on the screen.
3. The result window opens with the text and its translation.

**Ctrl+Alt+C** (Windows: **Win+Alt+C**) copies the text of a region to the
clipboard instead, and **Ctrl+Shift+O** does what *After selecting a region*
says (below). On macOS, Glossa first needs the **Screen Recording**
permission (*Options → Permissions*).

To work offline, pick another [engine](#engines) in *Options → Text
recognition → Provider*.

## Actions

Every action is in the tray menu, in the main window's camera button menu,
and in *Options → Hotkeys*, where any of them can be given a hotkey.

| action | default hotkey | what it does |
|---|---|---|
| Text recognition | Ctrl+Shift+O | select a region; then what *After selecting a region* says (below) |
| Recognise and translate | Ctrl+Alt+O (Windows: Win+Alt+O) | select a region; the result window opens with its text and translation |
| Translate the whole screen in place | Ctrl+Alt+S (Windows: Win+Alt+S) | no selection: everything on the monitor under the pointer is recognised and translated where it is, over a still of the screen; Space shows the original, C copies the translation, Escape or a click closes it |
| **Copy text from the screen** | **Ctrl+Alt+C** (Windows: Win+Alt+C) | select a region; its text goes to the clipboard, nothing opens |
| **Translate the screen in place** | none | select a region; the translation is painted over it |
| Watch a region | none | select a region; whenever it changes, its translation pops up (subtitles); the hotkey again stops |
| Repeat the last region | none | read the last region again, without selecting it, into the result window |

In the selection overlay, Esc or a right click cancels, and dragging again
starts a new selection.

The same actions are available to scripts:
`glossa-cli action recognize_to_clipboard`, or
`POST /v1/action/translate_region_in_place` on the
[control API](control-api.md).

### After selecting a region

*Options → Text recognition → After selecting a region* decides what plain
*Text recognition* does:

| choice | what happens |
|---|---|
| **Show the text and its translation** (the default) | the [result window](#the-result-window), with the translation when *Translate the recognised text* is on |
| **Copy the text to the clipboard** | the same as *Copy text from the screen* |
| **Translate it in place** | the same as *Translate the screen in place* |
| **Ask, with a toolbar** | a bar appears under the selection, like the Snipping Tool's: *Copy text*, *Translate*, *Translate in place*, then *Copy image*, *Save image* and *Cancel* |

Keys on the toolbar: **Enter** does the default (underlined; translate when
the setting is *Ask*), **C** copies the text, **T** translates, **I**
translates in place, **Esc** cancels.

## The result window

Recognising a region to read it opens a window of its own as soon as the
region is chosen. The text fills in when the engine has read it.

- **The picture** at the top, with what read it, how long it took, the
  language it found and how many characters and lines. A click on the
  picture (or the arrows beside it) makes it larger; the boxes button draws
  where the engine found text.
- **The text**, editable, in the same font and zoom as the main window's
  panes (Ctrl+wheel or Ctrl+plus/minus).
- **The direction bar**: the text's language, swap, the target language, the
  translator, and *Translate*. The translator list has *Google Lens* (its
  own translation of the picture, no second request) and your enabled
  translators. Changing the text's language while the text is as read has
  the engine read the picture again with that language.
- **The translation**, with the translator, direction and time under it, or
  a note that the text changed since it was translated.
- **The action bar**. Each action's tooltip shows its shortcut:

| action | shortcut |
|---|---|
| Copy text (with *Join lines when copying text* applied) | Ctrl+Shift+C |
| Copy translation | Ctrl+Shift+T |
| Translate in place (over the region the picture came from) | Ctrl+P |
| To main window (with the translation, no second request) | Ctrl+M |
| Read aloud (the translation, else the text) | Ctrl+L |
| Copy image | Ctrl+Shift+I |
| Save image | Ctrl+S |
| Select again | Ctrl+R |
| Translate | Ctrl+Enter |
| Swap: the translation becomes the text | Ctrl+I |
| Close | Esc |

On macOS the shortcuts use Cmd. The window remembers its size and place
(`[windows.ocr-result]` in `config.toml`); the first time it opens beside the
region. Translations made in it go into the history.

## Copy mode

*Copy text from the screen* closes the overlay as soon as you let go of the
mouse, so the program you were in keeps the focus. A small notice by the
region shows *Recognising…*, then *Copied N characters* with the start of the
text. It takes no focus and disappears by itself (a click dismisses it).

**Keep line breaks or join lines.** By default the text is copied with the
picture's line breaks, which is right for code, lists and tables. Turn on
*Join lines when copying text* for prose:

- the lines of each paragraph are joined with spaces;
- a blank line still separates paragraphs;
- a word hyphenated across two lines is put back together.

The same applies to the *Copy text* buttons and to
`glossa-cli ocr --join-lines`.

## Translation by Lens itself

Google Lens translates in the same request it recognises in, so no second
request goes to a translator. *Options → Text recognition → Translate with*:

| choice | what happens |
|---|---|
| **The OCR engine itself when it can (Google Lens, AI endpoints)** (the default) | with Lens, or an AI endpoint reading the picture, the engine's own translation is shown directly; with an engine that cannot translate, the current translator does it |
| **The current translator** | Lens only recognises, and the translator you use in the main window translates |

A translation made by Lens is labelled *Google Lens* and saved in the
history under it. Translations go into the target language of the main
window (the first language when the target is *auto*). The result window
can pick another one, and another translator, for the text at hand. From
the command line, `glossa-cli ocr --translate-with ocr|translator` makes the
same choice for one run.

## Translating in place

*Translate the screen in place* recognises the region and opens a borderless
window exactly over it. It shows the captured picture with the translation
laid over each block of text: the block's box is filled with the background
colour sampled around it, and the translation is drawn in the colour of the
original letters, at the original angle, sized to fit.

- With **Google Lens**, the translation comes from Lens line by line: each
  line is set where a source line was, scaled down rather than wrapped, as
  Chromium's overlay does.
- With **any other engine**, the lines it read are grouped into paragraphs
  and the current translator translates them, all in one request when it
  keeps the paragraphs apart (otherwise one request per paragraph). Each
  paragraph is reflowed into its box.

The strip under the picture (above it at the bottom of the screen) has
*Original* to switch between the original and the translation, *Copy
translation* and *Copy text*.

| key or mouse | does |
|---|---|
| Space | toggle the original |
| hold the mouse button on the picture | show the original while held |
| C | copy the translation |
| Esc, or a click anywhere else | close the window |

From the command line, `glossa-cli --json ocr picture.png --in-place -t ru`
prints each block with its box, and the control API takes
`{"image_base64": …, "in_place": true}` on `POST /v1/ocr`.

## Watching a region

*Watch a region* keeps capturing the selected rectangle (every
*Watched region check* milliseconds, 800 by default) and recognises and
translates it whenever its pixels change, showing the result in the popup.
Use it for game dialogue or video subtitles. Pressing the hotkey again, or
choosing the action again, stops it.

## Engines

Pick one in *Options → Text recognition → Provider*; the list shows the
engines this system has. If the chosen engine fails, the others are tried in
turn (except OneOCR and the AI endpoints, which only run when chosen).

| engine | id | works | platforms | setup |
|---|---|---|---|---|
| Google Lens | `lens` | online | all | nothing (chrome-lens-py is optional, below) |
| Windows OCR | `windows-ocr` | offline | Windows 10, 11 | the language's OCR pack |
| OneOCR | `oneocr` | offline | Windows 11 | the Snipping Tool |
| Apple Vision | `macos-vision` | offline | macOS 10.15+ | nothing |
| Tesseract | `tesseract` | offline | all | Tesseract and its language data |
| External command | `command-ocr` | either | all | a command line |
| AI endpoint | `openai`, `ai-…` | either | all | a vision model behind the endpoint (below) |
| Module | its id | either | all | a module with the OCR capability ([modules/ocr-modules.md](modules/ocr-modules.md)) |

### Google Lens

The most accurate engine, for every script, and the only one that translates
by itself. It works out of the box: Glossa has its own Lens client
(`crates/glossa-lens`), which sends the request Chrome's Lens overlay sends,
with nothing to install. A scan takes about half a second. When Google
answers with a network error or a 429 or 5xx status, the built-in client
tries again up to twice, waiting a little longer each time (as long as
Google's `Retry-After` asks, up to 30 seconds).

[chrome-lens-py](https://github.com/bropines/chrome-lens-py) can serve it
instead. *Options → Text recognition → Google Lens* has four modes:

| mode | what it uses |
|---|---|
| *Auto* (the default) | a running chrome-lens-py daemon (`lens_scan daemon start`, or the login service from `lens_scan service install`), which keeps a warm connection to Google; else the built-in client |
| *Built in* | the built-in client only; no Python needed |
| *Daemon only* | a chrome-lens-py daemon only |
| *CLI only* | `lens_scan` once per image, a second or two each |

A daemon URL and token can be set by hand, and so can the path to
`lens_scan` when it is not on `PATH`.

Every mode translates. The line-by-line placement used for in-place
translation comes with the built-in client and the daemon (with the CLI
alone, the translator translates paragraph by paragraph instead).

The built-in client is [chrome-lens-rs](https://github.com/bropines/chrome-lens-rs),
pinned by commit in `crates/glossa-lens/Cargo.toml`. Its protocol
definitions are Chromium's own `lens_server_proto`, pinned to one Chromium
commit there and re-pinned with its `update-protos`. Glossa sends its
requests through its own HTTP client, so the proxy settings apply.

### Windows OCR

Built into Windows 10 and 11, offline. It reads the languages whose **OCR
pack** is installed: *Settings → Time & language → Language & region*, open a
language's options and add its optical character recognition feature (or add
the language). With the OCR language on *auto* it reads with your Windows
display language, so set the OCR language when the picture is in another
script (on a Russian Windows, auto reads English as Cyrillic look-alikes).

### OneOCR (Snipping Tool)

The engine of the Windows 11 Snipping Tool: offline, many scripts at once,
no language to choose. Glossa loads it from your own Snipping Tool. The
first time, it **copies the two libraries (about 55 MB) into its cache
folder**, once. Nothing is downloaded. A folder set by hand must hold
`oneocr.dll`, `onnxruntime.dll` and `oneocr.onemodel`.

### Apple Vision (macOS)

The Live Text recogniser built into macOS 10.15 and later: offline, nothing
to install. *Accurate* or *Fast*, optional language correction, and on macOS
13 and later the language is detected by itself (Russian needs 13). Glossa
needs the **Screen Recording** permission to capture the screen; *Options →
Permissions* shows it and opens the right page of System Settings.

### Tesseract

Offline, on every system where it is installed:

| system | install |
|---|---|
| Debian, Ubuntu | `apt install tesseract-ocr tesseract-ocr-rus` |
| macOS | `brew install tesseract tesseract-lang` |
| Windows | the UB Mannheim installer |

Glossa looks for it on `PATH` and in the usual folders. The program, the
language data folder, the languages used on *auto* (`eng+rus`), the page
layout and engine modes are in *Options → Text recognition → Tesseract*.

### External command

Any program that prints the text on stdout: a cloud CLI, a script. Set its
command line in the provider's options, for example
`tesseract {image} - -l {lang3}`.

| placeholder | becomes |
|---|---|
| `{image}` | a temporary PNG |
| `{lang}` | the language as `en`, `zh-Hans` or `auto` |
| `{lang3}` | Tesseract's code for the language |

It runs without a shell, so nothing in a path or language code can inject
anything.

### AI endpoints (vision models)

Every AI endpoint (*Options → Services → Translation → AI endpoints*, see
[ai-endpoints.md](ai-endpoints.md)) can also read text from pictures with a
vision model: OpenAI's GPT-4o, Gemini, Claude through OpenRouter, or a local
Qwen2.5-VL, Gemma 3 or MiniCPM-V served by **vLLM**, Ollama or LM Studio.

- They are listed under *AI endpoints (vision models)* in the provider list.
- Nothing else needs setting up: the endpoint's address, key and headers are
  used as they are.
- An endpoint reads pictures **only when chosen**. Glossa never falls back
  to one on its own, since that would send screenshots to a service picked
  for translation.

The endpoint's settings (*Settings…* next to the provider, or *Services* and
the endpoint) add:

| setting | what it does |
|---|---|
| **Vision model** | the model that reads pictures, when the one used for translation does not take images; empty: the translation model |
| **Recognition answer** | *auto* asks for plain text, and for lines with their boxes when they are needed (translating in place, the whole screen); *plain text only*; *always lines with boxes* |
| **Box coordinates** | how the model gives a box: pixels of the image (GPT-4o, Claude, Qwen2.5-VL on the image as sent), 0–1000 (Qwen2-VL and others), 0–1000 with y first (Gemini's `box_2d`), or fractions; *Work it out* takes values up to 1 as fractions, values past the image as 0–1000, pixels otherwise |
| **Recognition instructions** | a prompt of your own; empty uses Glossa's, which ask for the text, or JSON lines with boxes, and nothing else |

When *Translate with* is *the OCR engine*, the model translates in the same
request: one call reads and translates.

Boxes from language models are less exact than Apple Vision's or Windows
OCR's. For translating in place, models trained to locate text (Qwen2.5-VL,
dots.ocr, PaddleOCR-VL) do best. vLLM resizes images to the model's limits;
if boxes come out scaled, set *Box coordinates* to match what the model
reports.

A local vLLM server, for example:

```
vllm serve Qwen/Qwen2.5-VL-7B-Instruct --port 8000
```

then an endpoint from the *Other (OpenAI-compatible)* preset with base URL
`http://localhost:8000/v1` and model `Qwen/Qwen2.5-VL-7B-Instruct`.

## Troubleshooting

**Google Lens**

- **"chrome-lens-py was not found"**: the Lens mode is *CLI only* and
  `lens_scan` is not on `PATH`. Choose *Auto* or *Built in*, which need no
  Python, or install chrome-lens-py (above) or set the path to `lens_scan` in
  the Lens options. A GUI program may not see a `PATH` changed after you
  logged in; restart Glossa, or sign out and in.
- **Lens is slow (seconds per scan)**: the mode is *CLI only*, which runs
  `lens_scan` once per image. Choose *Auto* or *Built in*, or start the daemon
  (`lens_scan daemon start`) or install its login service
  (`lens_scan service install`).
- **Lens recognises but its translation never shows**: *Translate with* is
  set to the current translator, or *Translate the recognised text* is off
  (the result window then waits for *Translate*).

**Offline engines**

- **No text found with Windows OCR**: the language's OCR pack is missing, or
  the OCR language is set to one without a pack. Install the pack, or set the
  language to *auto*. **Wrong letters** (Cyrillic look-alikes in English
  text): auto used your display language; set the OCR language.
- **OneOCR does not load**: it needs the Windows 11 Snipping Tool (a recent
  version). Glossa's log (`glossa.log` in the cache folder) says which file it
  could not find.
- **Tesseract reads gibberish**: its language data for that script is not
  installed, or *auto* reads English only; set *Languages when automatic*.

**Capture and windows**

- **Nothing is captured on macOS**: grant Screen Recording in *Options →
  Permissions* (or System Settings → Privacy & Security), then restart
  Glossa.
- **The in-place translation shows only the original**: the engine returned
  no positions for the text (an external command gives plain text). The
  translation then opens in the popup instead.
- **A hotkey does nothing**: another program has it. *Options → Hotkeys* shows
  which ones failed to register; pick another chord.
- **Copy mode copied nothing**: the region held no text Glossa could read; the
  notice says so. Try a larger region or another engine.
- **The result window opens off screen** (a monitor was unplugged): delete the
  `[windows.ocr-result]` table from `config.toml`.
