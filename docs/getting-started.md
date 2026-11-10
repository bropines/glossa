# Getting started

From nothing to the first translation in a few minutes. The
[user guide](guide.md) continues from here, task by task. По-русски:
[ru/getting-started.md](ru/getting-started.md).

Glossa is in beta: Windows is used daily, macOS is in early testing, and
the Linux builds have not been tried on real hardware yet.

## 1. Install

Take the file for your system from the
[releases page](https://github.com/bropines/glossa/releases):

| system | file |
|---|---|
| Windows 10, 11 | `…-windows-setup.exe` (or the `…-windows-portable.zip` to unpack anywhere) |
| macOS | the `.dmg`: `arm64` for Apple silicon, `x86_64` for Intel. Drag `Glossa.app` to *Applications* |
| Debian, Ubuntu | the `.deb` |
| other Linux | the `.AppImage` |

The downloads are not signed yet, so the system warns once:

- **Windows**: *More info → Run anyway* in the SmartScreen window.
- **macOS**: open the app once, then *System Settings → Privacy &
  Security → Open Anyway*.

[Installing](install.md) has every file, the system requirements and the
details; [Troubleshooting](troubleshooting.md#the-program-does-not-start)
helps when the first launch is refused.

## 2. The first start

A **welcome guide** opens. Its pages:

1. the interface language and the theme;
2. the hotkeys, which you can change right there: click one, press the new
   combination (Esc cancels, Backspace clears);
3. how to use it;
4. *Keys and data*: whether API keys are kept in the settings file or in
   the system keychain, and where your settings and history live;
5. *All set*.

*Skip* closes it at any page; *Options → Basics → Show the welcome guide*
opens it again.

After the guide, Glossa sits in the **tray** (Windows, Linux) or the
**menu bar** (macOS). Closing a window does not quit it. The first time,
the main window opens with a note saying so; *Got it* dismisses the note
for good.

**On macOS**, allow two things when asked, or in *Options → Permissions*:
**Accessibility**, to read the text you select in other programs, and
**Screen Recording**, to read text off the screen (restart Glossa after
granting this one).

## 3. The first translation

Select some text in any program and press **Ctrl+Shift+Q**.

A popup opens by the text with the translation. Glossa works out the
direction by itself: text in your language is translated into your second
language, anything else into your own. Both are set in *Options → Basics*
(*First language*, *Second language*).

Press the hotkey again to close the popup; left alone, it hides after a
few seconds.

| to | press | on Windows |
|---|---|---|
| translate the selection in a popup | Ctrl+Shift+Q | the same |
| open the main window with the selection | Ctrl+Alt+Q | Win+Alt+Q |
| read text in a screen region | Ctrl+Shift+O | the same |
| read and translate a screen region | Ctrl+Alt+O | Win+Alt+O |
| copy the text of a screen region | Ctrl+Alt+C | Win+Alt+C |
| translate the whole screen in place | Ctrl+Alt+S | Win+Alt+S |

On macOS, Ctrl+Alt is Control+Option. On Wayland, hotkeys depend on the
desktop ([Troubleshooting](troubleshooting.md#linux-and-wayland)).

No account or key is needed: Google, Microsoft, Yandex, DeepL and several
other services work straight away. The text you translate is sent to the
service that translates it; [Privacy](privacy.md) says exactly what goes
where.

## 4. Where to go next

- **Everything is in the tray menu**: right-click the tray icon (click
  the menu-bar icon on macOS) for text recognition, the dictionary, the
  history, mouse mode and *Options*.
- [The user guide](guide.md): the popup and the main window, the
  dictionary, history, reading the screen, listening, presets, mouse
  mode, exceptions, hotkeys, AI translators and modules.
- [Text recognition](ocr.md), [AI endpoints](ai-endpoints.md) and
  [themes](theming.md) in depth.
- Coming from QTranslate?
  [Bring your services, themes and settings](../README.md#coming-from-qtranslate).
- Something does not work: [Troubleshooting](troubleshooting.md).
