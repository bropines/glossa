# Text recognition (OCR)

Glossa reads text off the screen: drag over a region and it recognises what
is written there. The text can go straight to the clipboard, be shown with
its translation, or be translated **in place**, painted over the picture the
way Chrome's Google Lens overlay does it.

## Engines

Pick one in *Options → Text recognition → Provider*. If the chosen engine
fails, the others are tried in turn (except OneOCR, which only runs when
chosen).

### Google Lens (chrome-lens-py)

The most accurate engine, for every script, and the only one that translates
by itself. It goes through [chrome-lens-py](https://github.com/bropines/chrome-lens-py),
which you install once:

```
uv tool install chrome-lens-py      # or: pip install "chrome-lens-py[clipboard]"
```

Glossa finds it on its own, fastest way first:

1. a **running daemon** (`lens_scan daemon start`, or the login service from
   `lens_scan service install`): a scan takes a few hundred milliseconds;
2. failing that, it asks the CLI to **start the daemon**;
3. failing that, it runs **`lens_scan` per image** (a second or two each).

*Auto* does all of that; *Daemon only* and *CLI only* force one path. A daemon
URL and token can be set by hand, and so can the path to `lens_scan` when it
is not on `PATH`.

### Windows OCR

Built into Windows 10 and 11, offline. It reads the languages whose **OCR
pack** is installed: *Settings → Time & language → Language & region*, open a
language's options and add its optical character recognition feature (or add
the language). With the OCR language on *auto* it uses your Windows display
languages.

### OneOCR (Snipping Tool)

The engine of the Windows 11 Snipping Tool: offline, many scripts at once,
no language to choose. Glossa loads it from your own Snipping Tool; the first
time, it **copies the two libraries (about 55 MB) into its cache folder**,
once. Nothing is downloaded. A folder set by hand must hold `oneocr.dll`,
`onnxruntime.dll` and `oneocr.onemodel`.

### External command

Any program that prints the text on stdout: Tesseract, a cloud CLI, a script.
Set its command line in the provider's options, for example
`tesseract {image} - -l {lang3}`. `{image}` is a temporary PNG, `{lang}` the
language as `en`/`zh-Hans`/`auto`, `{lang3}` Tesseract's code. It runs
without a shell, so nothing in a path or language code can inject anything.

## Actions

Every action is in the tray menu, in the main window's camera button menu,
and in *Options → Hotkeys*, where any of them can be given a hotkey.

| action | default hotkey | what it does |
|---|---|---|
| Text recognition | Ctrl+Shift+O | select a region; then what *After selecting a region* says (below) |
| Recognise and translate | Ctrl+Alt+O | select a region; its text and translation appear beside it |
| **Copy text from the screen** | **Ctrl+Alt+C** | select a region; its text goes to the clipboard, nothing opens |
| **Translate the screen in place** | none | select a region; the translation is painted over it |
| Watch a region | none | select a region; whenever it changes, its translation pops up (subtitles); the hotkey again stops |
| Repeat the last region | none | recognise the last region again without selecting it |

In the selection overlay, Esc or a right click cancels, and dragging again
starts a new selection. The same actions are available to scripts:
`glossa-cli action recognize_to_clipboard`, or
`POST /v1/action/translate_region_in_place` on the control API.

### After selecting a region

*Options → Text recognition → After selecting a region* decides what plain
*Text recognition* does:

- **Show the text and its translation** (the default): a panel by the region
  with the text (editable), its translation when *Translate the recognised
  text* is on, and buttons to copy, translate in the main window, translate
  in place, copy or save the image.
- **Copy the text to the clipboard**: the same as *Copy text from the screen*.
- **Translate it in place**: the same as *Translate the screen in place*.
- **Ask, with a toolbar**: a bar appears under the selection, like the
  Snipping Tool's: *Copy text*, *Translate*, *Translate in place*, *Copy
  image*, *Save image*, *Cancel*. Keys: **Enter** does the default (translate
  when the setting is *Ask*), **C** copies the text, **T** translates, **I**
  translates in place, **Esc** cancels.

## Copy mode

*Copy text from the screen* closes the overlay as soon as you let go of the
mouse, so the program you were in keeps the focus. A small notice by the
region shows *Recognising…*, then *Copied N characters* with the start of the
text; it takes no focus and disappears by itself (a click dismisses it).

**Keep line breaks or join lines.** By default the text is copied with the
picture's line breaks, which is right for code, lists and tables. Turn on
*Join lines when copying text* for prose: the lines of each paragraph are
joined with spaces, a blank line still separates paragraphs, and a word
hyphenated across two lines is put back together. The same applies to the
*Copy text* buttons and to `glossa-cli ocr --join-lines`.

## Translation by Lens itself

Google Lens translates in the same request it recognises in, so no second
request goes to a translator. *Options → Text recognition → Translate with*:

- **The OCR engine itself when it can (Google Lens)** (the default): with
  Lens, its translation is shown directly; with an engine that cannot
  translate, the current translator does it.
- **The current translator**: Lens only recognises, and the translator you
  use in the main window translates.

A translation made by Lens is labelled and saved in the history under
*Google Lens*. Translations go into the target language of the main window
(the first language when the target is *auto*).

## Translating in place

*Translate the screen in place* recognises the region and opens a borderless
window exactly over it, showing the captured picture with the translation
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
translation* and *Copy text*. **Space** toggles the original, holding the
mouse button on the picture shows it while held, **C** copies the
translation, and **Esc** or a click anywhere else closes the window.

From the command line, `glossa-cli --json ocr picture.png --in-place -t ru`
prints each block with its box, and the control API takes
`{"image_base64": …, "in_place": true}` on `POST /v1/ocr`.

## Watching a region

*Watch a region* keeps capturing the selected rectangle (every
*Watched region check* milliseconds, 800 by default) and recognises and
translates it whenever its pixels change, showing the result in the popup.
Use it for game dialogue or video subtitles. Pressing the hotkey again, or
choosing the action again, stops it.

## Troubleshooting

- **"chrome-lens-py was not found"**: install it (above) or set the path to
  `lens_scan` in the Lens options. A GUI program may not see a `PATH` changed
  after you logged in; restart Glossa, or sign out and in.
- **Lens is slow (seconds per scan)**: it is running the CLI per image. Start
  the daemon (`lens_scan daemon start`) or install the login service
  (`lens_scan service install`).
- **No text found with Windows OCR**: the language's OCR pack is missing, or
  the OCR language is set to one without a pack. Install the pack, or set the
  language to *auto*.
- **OneOCR does not load**: it needs the Windows 11 Snipping Tool (a recent
  version). Glossa's log (`glossa.log` in the cache folder) says which file it
  could not find.
- **The in-place translation shows only the original**: the engine returned
  no positions for the text (an external command gives plain text). The
  translation then opens in the popup instead.
- **A hotkey does nothing**: another program has it. *Options → Hotkeys* shows
  which ones failed to register; pick another chord.
- **Copy mode copied nothing**: the region held no text Glossa could read; the
  notice says so. Try a larger region or another engine.
