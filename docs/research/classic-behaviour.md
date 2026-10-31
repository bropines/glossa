# How the classic program behaves (observed)

Clean-room notes. Everything here comes from *using* the classic program and
reading its plain-text data files (`Options.json`, themes, locale files,
service scripts) — not from its machine code. These notes are what the Glossa
UI is built to match.

Observed on 2026-10-31 against a running classic install (theme "Flat Dark",
first language Russian, second English), by selecting text in Notepad and
pressing the popup hotkey, plus screenshots of every options page.

## The popup (selection + hotkey)

The defining feature. Select text anywhere, press the hotkey (`Ctrl+Shift+Q`
here), and a small window with the translation appears.

- **Placement:** at the mouse pointer, offset by **+15, +15** px — not at the
  selection or the caret. (Pointer at 111,377 → popup at 126,392.) Kept on
  screen.
- **Size:** width follows the content — **350 px** for a single word, up to a
  maximum of **600 px** for a sentence, which then wraps. Height fits the text
  (82 px for two lines; 166 px for a word with dictionary entries).
- **Chrome:** no title bar. A thin frame in the theme's border colour and a
  system drop shadow (`CS_DROPSHADOW`).
- **Header row:** a small icon on the left, then
  `"<From> to <To> (<Service>)"` — e.g. `English to Russian (Yandex)`. On the
  right, icons: ☆ add to favourites, dictionary, listen, copy, replace.
- **Body:** the translation only, *not* the source text. For a single word the
  service's dictionary block follows (`serendipity [serənˈdɪpɪtɪ] noun` /
  indented senses).
- **Service strip on hover:** while the pointer is over the popup, a strip of
  service tabs appears along the bottom (icon + name; names truncate when the
  popup is narrow). Clicking one re-translates in place with that service.
- **Own service choice:** the popup used Yandex while the main window's active
  tab was Lara — the popup keeps its own current service.
- **Auto-hide:** disappears after the configured delay (5 s) once the pointer
  is away; stays as long as the pointer is over it.
- **Colours:** with "Enable window style" on (the default), the popup uses the
  theme; the separate popup background/text/frame colours in the options are
  greyed out and apply only when window style is off.

## Main window (from screenshots)

Top to bottom: back/forward buttons, the active service's name as a link, a ⋮
menu; the source pane with microphone and listen icons in its bottom-right
corner; a middle bar — clear (new page) button, ⋮ language-pair menu,
"from" combo, swap button, "to" combo (shows `Auto-Detect (Russian…)` with the
detected language), and the Translate button; the translation pane with a
listen icon; and the service tab strip (icon + name) along the bottom.

## Options dialog (from screenshots)

Left-hand page list: Basics, Hotkeys, Internet, Services, Languages,
Appearance, Exceptions, Advanced, Updates; OK / Cancel / Apply at the bottom.

- **Hotkeys:** an "Enable hotkeys" checkbox over a two-column table
  (Action | Hotkey).
- **Services:** grouped list ("Translation", then dictionaries) of checkboxes
  with service icons, ▲/▼ to reorder, "Add dictionary…" / "Remove dictionary".
- **Languages:** a four-column grid of checkboxes, "Check / Uncheck all".
- **Appearance:** "General" (window style, theme) and "Popup window" (auto
  size, auto position, activate when opened, pin when dragging, auto-hide
  delay, transparency 0–90, frame thickness, the four icon toggles shown as
  icon buttons, three colour swatches).
- **Exceptions:** mode drop-down ("Disabled for these programs:"), a table
  (Program Name | Window Class Name, with `* All Programs *` /
  `* All Windows *` placeholders), Add / Modify / Delete, "Enable smart
  detection".
- **Internet:** timeout; proxy mode drop-down and scheme / host / port /
  user / password fields, disabled unless the mode is manual.
- **Advanced:** default browser, copy action (`Ctrl + C`), keyboard layout
  indicator, preferred domain, GUI translation, slower repeat listening,
  remove line breaks; "Mouse mode" group; "API keys" group (OCR key).

## Process and security notes

- The classic program is registered to start at logon through a scheduled
  task with `RunLevel=HighestAvailable` — it runs elevated. Windows then blocks
  synthetic input from normal programs into its windows (UIPI), which is also
  why selection capture had to be elevated to reach elevated apps. Glossa runs
  unelevated by default; capturing a selection inside an elevated window needs
  Glossa elevated too, and the docs say so.
- Several community service scripts in the classic `Services` folder carry
  hard-coded API keys. Glossa never bundles those scripts; it loads them from
  the user's own installation, and per-service keys live in the user's
  settings.
