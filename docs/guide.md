# User guide

How to do things with Glossa, task by task. New here? Start with
[Getting started](getting-started.md). Names in *italics* are what the
English interface shows; on macOS read Cmd for Ctrl inside Glossa's
windows.

Some things described here were added after 0.1.0-beta.2 and are in the
next release; the [changelog](../CHANGELOG.md) lists them under
*Unreleased*. They are marked "(next release)".

- [Where everything is](#where-everything-is)
- [Translating a selection](#translating-a-selection)
- [The popup](#the-popup)
- [The main window](#the-main-window)
- [Services](#services)
- [The dictionary](#the-dictionary)
- [History](#history)
- [Text on the screen](#text-on-the-screen)
- [Listening](#listening)
- [Presets](#presets)
- [Mouse mode](#mouse-mode)
- [Exceptions](#exceptions)
- [Hotkeys](#hotkeys)
- [Adding an AI translator](#adding-an-ai-translator)
- [Installing a module](#installing-a-module)
- [Keys and privacy](#keys-and-privacy)
- [Quitting](#quitting)

## Where everything is

Glossa lives in the tray (the menu bar on macOS). Its menu has *Show
Glossa*, *Text recognition*, *Dictionary*, *History*, *Keyboard*, *Enable
global hotkeys*, *Mouse mode*, *Next preset* (next release), *Options*,
*About Glossa* and *Exit*.

*Options* is where the settings are. Its pages: *Basics*, *Appearance*,
*Hotkeys*, *Services*, *Languages*, *Exceptions*, *Text recognition*,
*Internet*, *Permissions* (macOS), *Modules*, *Control API*, *Advanced*
and *Updates*. Changes take effect with *Apply* or *OK*. The main window's
gear button opens it too, and on macOS so does *Settings…* in the
application menu.

Every setting is also a key in `config.toml`
([Configuration](configuration.md)).

## Translating a selection

Select text in any program and press **Ctrl+Shift+Q** (the action
*Translate selection*). The translation opens in a popup next to the
text.

Other ways to get text in:

- **Ctrl+Alt+Q** (Windows: **Win+Alt+Q**) opens the main window with the
  selection.
- *Translate clipboard content* translates what you copied; it has no
  hotkey until you give it one in *Options → Hotkeys*. When nothing is
  selected, the popup hotkey translates the clipboard as well.
- *Replace selected text* puts the translation in place of the selection,
  right where you type. Give it a hotkey, or use the popup's button.
- *Change keyboard layout and selected text* fixes text typed in the wrong
  keyboard layout.

**The direction** is worked out for you. Set your own language as *First
language* and the one you usually translate into as *Second language*
(*Options → Basics*). Text in the first language goes into the second;
anything else goes into the first.

## The popup

- The **header** names the direction and the service. Click the language
  to translate into another one.
- **Buttons**: look the text up in the dictionary, listen, copy the
  translation, replace the selected text with it. *Options → Appearance →
  Popup* chooses which buttons show.
- The **service strip** appears along the bottom when the pointer is over
  the popup: click another service to translate with it. The popup
  remembers its own service, apart from the main window's.
- **Drag** it by the header; drag an edge to **resize** it (the size is
  remembered; *Back to automatic size* undoes that). The **pin** keeps it
  open.
- It **hides** after a few seconds without the pointer over it, or when
  you press the hotkey again. The delay, the widths, the transparency and
  the border are in *Options → Appearance → Popup*.
- A **double click on the header** opens the main window with the same
  text; *Open the main window with the popup* does it every time (next
  release).
- When the first service fails and another answers, the popup says which
  one did and why (next release).

## The main window

Open it with **Ctrl+Alt+Q** (Windows: **Win+Alt+Q**) or with *Show
Glossa* in the tray menu.

- Type, paste or drop text into the **source pane**. **Ctrl+Enter**
  translates, **Ctrl+N** clears, **Ctrl+I** swaps the direction.
- The **language bar** between the panes has the source and target
  languages (*Auto-detect* for the source), the swap button and
  *Translate*. The menu button beside it lists recent directions.
- The **service tabs** switch the translator. AI endpoints sit in their
  own group at the end (next release).
- The **toolbar** has the menu, *Options*, the on-screen *Keyboard*,
  *Text recognition*, *Dictionary* and *History*.
- **Ctrl + mouse wheel** over a text area makes its text bigger or
  smaller.

The **menu** (the first toolbar button) holds the window's switches:

| item | what it does |
|---|---|
| *Spell checking* | underlines misspelled words in the source pane, with suggestions in its context menu |
| *Instant translation* | translates as you type, after a pause |
| *Backward translation* | translates the result back, in a third pane, as a check |
| *Show all services at once* | one card per enabled service instead of tabs |
| *Clear input on Drag & Drop*, *Auto-cleanup of translation*, *Save contents on exit* | housekeeping |
| *Show top pane*, *Show middle pane*, *Show services pane* | hide parts of the window |
| *Always on top* | keeps the window above others |
| *Minimize to system tray on minimize*, *… on close* | what minimising and closing do; closing hides to the tray by default |
| *On startup* | *Show window*, *Minimize to tray* or *Restore previous state* |
| *Presets* | switch to a [preset](#presets) (next release) |

## Services

*Options → Services* has a tab for each kind: *Translation*,
*Dictionaries*, *Speech*, *Text recognition* and *Presets* (next release).

- Tick the services you want in the strip and put them in order with the
  up and down buttons. Services that work without a key are on from the
  start.
- Select a service to see its settings. A service marked *needs a key*
  works once you paste one; the key stays on your machine and goes only
  to that service.
- *If a service fails, try the next one* (on by default) sends the text
  to the next enabled service when the selected one does not answer.

*Options → Languages* hides languages you never use from every list.

## The dictionary

Open it from the tray menu (*Dictionary*), the main window's toolbar, or
the popup's dictionary button, which looks up the popup's text. The action
*Show dictionary window* can be given a hotkey.

- Type a word and press Enter. The services pane switches between
  dictionaries; *Show services pane* hides it.
- *Exact search* matches the headword exactly instead of taking the
  service's nearest suggestion.
- The window keeps the words you looked up; *Clear history* forgets them.

**Offline dictionaries.** *Options → Services → Dictionaries → Add
dictionary...* takes StarDict (`.ifo`), DSL, XDXF and tab-separated files.
They are read from the disk and need no network.

## History

Every translation the desktop program shows is recorded. Open the list
from the tray menu or the main window's toolbar (*History*).

- **Search** with the box at the top; the star beside it shows
  *Favourites only*.
- The star on an entry makes it a **favourite**. Favourites are never
  dropped when the list reaches its limit, and *Clear* leaves them.
- **Double-click** an entry to open it in the main window, where its
  text is translated again. Its context menu has *Open*, *Copy text*,
  *Copy translation*, *Listen* and *Delete*.
- *Save as...* exports the list (or the favourites, when filtered) as
  text, CSV, HTML or JSON.

*Options → Basics → History* has *Enable history*, *Clear history on exit*
and *Expand items* (show each translation in the list). The file is
`history.json` in the data folder ([Privacy](privacy.md#what-is-stored-on-the-machine)).

## Text on the screen

For text you cannot select: a picture, a video, a game, a scanned PDF.

| to | press | on Windows |
|---|---|---|
| read a region and show the text with its translation | Ctrl+Alt+O | Win+Alt+O |
| copy a region's text to the clipboard | Ctrl+Alt+C | Win+Alt+C |
| translate the whole screen where it is | Ctrl+Alt+S | Win+Alt+S |
| read a region, then do what *After selecting a region* says | Ctrl+Shift+O | the same |

Press the hotkey and drag over the text; Esc or a right click cancels.
The same actions are in the tray menu's *Text recognition* and the main
window's camera button. More actions without a default hotkey: *Translate
a screen region in place*, *Watch a screen region and translate changes*
(subtitles), *Repeat the last recognised region*.

Google Lens reads the picture by default, which sends that picture to
Google. *Options → Text recognition → Provider* switches to an engine on
your machine: Windows OCR or OneOCR on Windows, Apple Vision on macOS,
Tesseract anywhere. On macOS Glossa needs the Screen Recording permission
first. Everything else is in [Text recognition](ocr.md).

## Listening

- The popup's and the main window's **listen buttons** read the text
  aloud.
- The actions *Listen to selected text* and *Listen to selected text
  translation* read a selection in any program; give them hotkeys in
  *Options → Hotkeys* (they have none by default).
- *Options → Services → Speech* chooses the voice: Google's online voice
  (the default), or the system's own voices, which work offline: the
  Windows voices, macOS's `say`, espeak-ng on Linux.
- Listening to the same text again within a few seconds plays it slowly
  (*Options → Advanced*).

There is no speech input yet.

## Presets

(Next release.) A preset is a named set of services: which are on, their
order, the selected one, the dictionaries, the voice, the OCR engine,
module settings and, if you like, the language pair. It never holds a key.

- *Options → Services → Presets*: type a name under *Save current setup
  as* and press *Save*; tick *With the language pair* to include the
  direction. *Apply*, *Rename*, *Duplicate*, *Delete*, *Export* (a `.toml`
  file) and *Import* are there too. Apply changes made elsewhere in
  Options first: presets work with the applied settings.
- Switch from the main window's menu (*Presets*), the tray's *Next
  preset*, or the action *Switch to the next preset* bound to a hotkey or
  a tray click.
- Three are built in and use only services that need no key: *Everyday*,
  *Private* and *Word study*. They cannot be changed; duplicate one to
  make your own. *Private* still uses online services
  ([what that means](privacy.md#requests-you-might-not-expect)).

## Mouse mode

Translate by selecting with the mouse, no hotkey.

- Turn it on in the tray menu (*Mouse mode → Enabled*) or in *Options →
  Advanced → Mouse mode*. On Windows a click on the tray icon toggles it.
- Choose what a selection does: *Show icon* (a small button appears by
  the pointer; click it for the translation), *Show translation* (the
  popup opens at once) or *Show translation and read*.
- *Enable mouse mode only when Ctrl is pressed* keeps it quiet unless you
  hold the key while selecting.

On macOS it needs Accessibility, and Input Monitoring if that alone is not
enough. Under Wayland it depends on the desktop and may not be available;
Glossa says so in *Options*.

What the clicks on the tray icon do (click, double click, middle click)
is set in *Options → Advanced → Tray icon* (next release). On Linux any
click opens the menu.

## Exceptions

*Options → Exceptions* lists programs where reading the selection and
mouse mode stay out of the way: a game, a remote desktop window, a
terminal.

- *Add* a program by its executable's name, or press *From the active
  window* to take the window you were in before Options.
- The list means *Disabled for these programs* by default. Switch it to
  *Enabled for these programs* to turn the logic round: Glossa then works
  only there.
- *Enable smart detection* also skips controls that never hold
  selectable text (lists, toolbars, buttons).

On Windows a rule can name a window class as well as a program; on macOS
only the app's name.

## Hotkeys

*Options → Hotkeys* lists every action with its hotkey. Click a hotkey,
press the new combination; Esc cancels, Backspace clears it.

- A combination another program already holds, or one given to two
  actions, is marked.
- *Enable hotkeys* (and *Enable global hotkeys* in the tray menu) switches
  all of them off and on.
- The defaults on Windows use Win+Alt where other systems use Ctrl+Alt,
  because Ctrl+Alt is what the AltGr key sends.
- Under Wayland, hotkeys go through the desktop's shortcut portal where
  there is one, and otherwise are bound in the desktop's own settings to a
  command ([how](control-api.md#binding-hotkeys-to-it)).

Any action can also be run from a script: `glossa-cli action show_popup`
([Command line](cli.md#action)).

## Adding an AI translator

Any service with an OpenAI-compatible API can translate: OpenAI,
OpenRouter, DeepSeek, Gemini, Groq, Mistral, or a model on your own
machine through Ollama or LM Studio.

1. *Options → Services → Translation*, the panel *AI endpoints
   (OpenAI-compatible)*.
2. Choose a preset and press *Add endpoint*.
3. Paste the key (a local server needs none).
4. *Load models* lists what the endpoint offers; pick one.
5. *Test* translates a short sample with the settings as typed.
6. *Apply*. The endpoint is now a translator of its own in the service
   strip.

The prompt, style, glossary, temperature and streaming are in the
endpoint's settings: [AI endpoints](ai-endpoints.md).

## Installing a module

A module adds a translator, a dictionary, a voice or an OCR engine. It is
a folder with a `module.toml`, or a pot-app `.potext` file.

1. *Options → Modules → Install module…* and choose the folder or the
   file. (*Open my modules folder* shows where modules live, if you prefer
   to copy the folder yourself; *Reload modules* picks it up.)
2. **Approve it** (next release). The module appears at the top of the
   page under *Waiting for your approval*, with what it asks for: the
   hosts it may reach, whether it runs a program on your computer or a
   library inside Glossa, the files it would download. Read it; *Approve*
   lets it run, *Decline* switches it off. A sandboxed JavaScript module
   can do far less than one that runs a program with your rights.
3. Switch the new service on in *Options → Services*.
4. If its settings show *Files* with *not downloaded*, press *Download*
   there. Models can be large; nothing is downloaded without that click.

Install modules only from people you trust
([what approval does and does not check](../SECURITY.md#what-glossa-protects-and-what-it-does-not)).

**From QTranslate.** *Options → Modules → Classic QTranslate
installation → Choose…* and pick the folder with `QTranslate.exe`: its
services, themes and interface languages are used in place. *Import its
settings and history* brings those over once.

**Writing one** is described in the [modules overview](modules/README.md);
*Options → Modules → Module editor* is an editor for it inside Glossa
(next release).

## Keys and privacy

*Options → Basics → Keys and privacy* chooses where API keys, the proxy
password and the Lens token are kept: in the settings file as plain
text, or in the system keychain (Windows Credential Manager, the macOS
Keychain, the Secret Service on Linux). A portable copy on a USB stick is
better off with the file; a shared computer with the keychain.

What is sent where, what is stored, and how to work without the network:
[Privacy and data](privacy.md).

## Quitting

Closing a window only hides it. To quit, choose *Exit* in the tray menu
or in the main window's menu; on macOS, *Quit Glossa* (Cmd+Q) as well.
