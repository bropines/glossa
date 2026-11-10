# Troubleshooting

By symptom. If nothing here helps, [the last section](#reporting-a-bug)
says what to collect before opening an issue. По-русски:
[ru/troubleshooting.md](ru/troubleshooting.md).

- [First steps](#first-steps)
- [The program does not start](#the-program-does-not-start):
  [SmartScreen](#windows-smartscreen), [Gatekeeper](#macos-gatekeeper)
- [A hotkey does nothing](#a-hotkey-does-nothing)
- [The popup is empty, shows the wrong text or does not appear](#the-popup-is-empty-shows-the-wrong-text-or-does-not-appear)
- [macOS permissions](#macos-permissions)
- [Windows: programs running as administrator](#windows-programs-running-as-administrator)
- [Linux and Wayland](#linux-and-wayland)
- [Network, proxy and certificate errors](#network-proxy-and-certificate-errors)
- ["… is rate-limiting us"](#-is-rate-limiting-us)
- [Text recognition finds nothing](#text-recognition-finds-nothing)
- [A module is waiting for approval](#a-module-is-waiting-for-approval)
- [Module files do not download](#module-files-do-not-download)
- [Settings were reset](#settings-were-reset)
- [The log](#the-log)
- [Reporting a bug](#reporting-a-bug)

## First steps

`glossa-cli doctor` looks at the machine and prints what works: hotkeys,
reading the selection, screen capture, the tray, the macOS permissions,
the OCR engines, modules that failed to load, the folders in use, the
proxy, and whether the control API answers. Most answers below start
there.

```console
$ glossa-cli doctor
```

`glossa-cli` is installed beside `glossa` on Windows and Linux; on macOS
it is in the release's `.zip`, not inside `Glossa.app`.

Two more quick checks:

- `glossa-cli -v translate -t en "Bonjour"` translates without the
  desktop program and prints what went wrong, which separates a network
  or service problem from a hotkey or window problem.
- `glossa-cli modules` lists the modules that loaded and those that
  failed, with the reason; `glossa-cli module pending` lists the ones
  waiting for approval.

## The program does not start

Glossa starts in the tray (the menu bar on macOS) and may show no window:
look for its icon there first. On Windows it can be among the hidden
icons behind the arrow. On GNOME the tray icon needs the *AppIndicator and
KStatusNotifierItem Support* extension.

Only one copy runs per user and data folder: starting Glossa again hands
the request to the running copy instead of opening a second one.

### Windows: SmartScreen

The downloads are not signed with a publisher certificate yet, so
SmartScreen may stop the installer or `glossa.exe` the first time with
"Windows protected your PC". Choose *More info → Run anyway*. If there is
no *Run anyway*, your organisation's policy blocks unknown programs and
only an administrator can allow it.

### macOS: Gatekeeper

The app is signed ad hoc and not notarised, so macOS refuses the first
launch of a downloaded copy ("cannot be opened because Apple cannot check
it for malicious software", or a similar message).

- macOS 15 and later: try to open it once, then *System Settings →
  Privacy & Security*, scroll down and press *Open Anyway*.
- Older systems: right-click `Glossa.app` → *Open* → *Open*.
- Or in Terminal: `xattr -dr com.apple.quarantine /Applications/Glossa.app`

If macOS says the app "is damaged and can't be opened", it is the same
quarantine check; the `xattr` command clears it. Move the app to
*Applications* before opening it.

### A blank or black window

Drawing uses OpenGL by default. On a machine with a broken or missing
graphics driver (some virtual machines and remote sessions), start once
with another renderer:

```powershell
$env:GLOSSA_RENDERER = "software"; & "$env:LOCALAPPDATA\Programs\Glossa\glossa.exe"
```

If that helps, choose it in *Options → Appearance → Graphics engine*
(`[appearance] renderer`: `open_gl`, `gpu` or `software`) so it stays. On
macOS and Linux set the variable the same way before starting `glossa`.

## A hotkey does nothing

1. **Is it registered?** *Options → Hotkeys* marks a hotkey that another
   program already holds ("… is already in use by another program") and
   one bound to two actions ("Also bound to: …"). Choose another
   combination: click the hotkey, press the new keys; Esc cancels,
   Backspace clears.
2. **Are hotkeys on?** The tray menu's *Enable global hotkeys* and
   *Options → Hotkeys → Enable hotkeys* are the same switch.
3. **Is it the default you expect?** On Windows the main-window and
   screen hotkeys use Win+Alt (Win+Alt+Q, Win+Alt+O…), not Ctrl+Alt,
   because Ctrl+Alt is what AltGr sends. Translating the selection is
   Ctrl+Shift+Q everywhere.
4. **Is the program in front an exception?** *Options → Exceptions* lists
   programs where reading the selection is switched off; remote desktop
   windows and consoles are there by default on Windows.
5. **Is the window in front running as administrator?** See
   [below](#windows-programs-running-as-administrator).
6. **Wayland?** Programs cannot register global hotkeys there by
   themselves; see [Linux and Wayland](#linux-and-wayland).

On macOS the hotkeys themselves need no permission, but what they do
(reading the selection, capturing the screen) does: a hotkey that
"does nothing" there is usually a missing permission
([below](#macos-permissions)).

## The popup is empty, shows the wrong text or does not appear

When the hotkey is pressed, Glossa reads the selected text from the
program in front: through the accessibility interface where the program
offers it, otherwise by sending the copy keystroke and reading the
clipboard.

- **"No data to translate."** Nothing was selected and the clipboard was
  empty. Select text first.
- **It translated something you copied earlier.** The selection could not
  be read, so the popup fell back to the clipboard. The cases below are
  the usual reasons.
- **The program is slow to copy.** Raise *Options → Advanced → Selection
  copy wait* (`[advanced] copy_wait_ms`, 250 ms by default).
- **A terminal.** Ctrl+C would interrupt the running command, so in
  terminals Glossa does not send it by default and relies on the
  accessibility interface or on what the terminal already put on the
  clipboard. *Options → Advanced → Copy action* and
  `[advanced] terminal_copy_action` change that
  ([Configuration](configuration.md#advanced)).
- **The program is an exception**, or the control under the pointer is one
  that smart detection skips (a list, a toolbar): *Options → Exceptions*.
- **macOS**: Accessibility is not granted, or was granted to an older
  build ([below](#macos-permissions)).
- **The popup opens and shows an error** instead of a translation: the
  service failed. The message says why; see
  [Network](#network-proxy-and-certificate-errors) and
  [rate limiting](#-is-rate-limiting-us). Pick another service in the
  strip at the bottom of the popup (move the pointer over the popup to see
  it).
- **The popup appears and vanishes.** It hides after a few seconds
  without the pointer over it (*Options → Appearance → Popup*,
  `[appearance.popup] auto_hide_secs`; `0` never hides). The pin button
  keeps it open.
- **The popup is not where the text is.** On Wayland a program cannot
  learn where the text cursor is; the popup opens near the pointer or in
  the middle of the screen.

## macOS permissions

*Options → Permissions* shows what is granted and has a button for each
that asks again or opens the right page of System Settings.

| permission | without it |
|---|---|
| Accessibility | the translation hotkeys find no selection; replacing the selection and mouse mode do not work |
| Screen Recording | text recognition captures nothing; macOS applies a new grant only after Glossa restarts |
| Input Monitoring | mouse mode does not notice selections, if Accessibility alone did not make it work |

**After an update the permissions are gone, or look granted and do not
work.** macOS ties a grant to the app's signature. Glossa's builds are
signed ad hoc, so every new build is a different program to macOS, and
the old entry in the list no longer applies even though its switch still
shows as on. For each of the three lists in *System Settings → Privacy &
Security*:

1. select the old *Glossa* entry and remove it with the minus button;
2. open *Options → Permissions* in Glossa and press *Ask* (or add
   `Glossa.app` with the plus button);
3. for Screen Recording, quit and start Glossa again.

The same from Terminal, which removes every Glossa entry at once (Glossa
then asks again):

```bash
tccutil reset Accessibility io.github.bropines.glossa
tccutil reset ScreenCapture io.github.bropines.glossa
tccutil reset ListenEvent io.github.bropines.glossa
```

`glossa-cli doctor` prints the three states under *Permissions*.

## Windows: programs running as administrator

Windows does not let an ordinary program read from or type into a program
running as administrator (an elevated terminal, an installer, Task
Manager, classic QTranslate if it was started elevated). In front of such
a window the hotkeys reach Glossa, but the selection cannot be copied and
the translation cannot be pasted.

Either copy the text yourself and use *Translate clipboard content*, or
start Glossa as administrator too. The second has a cost: everything
Glossa starts, modules included, then runs elevated.

## Linux and Wayland

Linux builds have not been tried on real hardware yet; expect rough
edges and please report them. `glossa-cli doctor` says what the session
allows.

Under **X11** everything works the direct way; the selection is read from
the PRIMARY selection (the text highlighted most recently in any window).

Under **Wayland** a program cannot do these things by itself, and Glossa
uses the desktop's portals where they exist:

| feature | how | where |
|---|---|---|
| Global hotkeys | the GlobalShortcuts portal; the desktop asks once to confirm them, and they are changed in the desktop's own settings | KDE Plasma 6, GNOME 48 and later, Hyprland |
| Screen capture | the Screenshot portal, or `grim` | most desktops; wlroots compositors with `grim` |
| Copying from and pasting into other programs | the RemoteDesktop portal, which asks once | GNOME, KDE Plasma |
| Placing the popup at the text cursor | not possible | the popup opens near the pointer or centred |

Where there is no shortcut portal (Sway and other wlroots compositors,
GNOME before 48), bind a key in the compositor's own settings to a
command that calls Glossa:
[Control API, binding hotkeys](control-api.md#binding-hotkeys-to-it).
Where the highlighted text cannot be read, copy it with Ctrl+C first;
Glossa then uses the clipboard.

A click on the tray icon always opens the menu on Linux: indicators do
not report clicks.

## Network, proxy and certificate errors

Messages such as "Unable to establish Internet connection", "No data
returned (timeout while sending data)." or a service's own error.

1. **Check the proxy Glossa uses.** `glossa-cli doctor` prints it under
   *Network*. *Options → Internet* has three choices: the system's
   settings, no proxy, or a manual one.
   - *Use system proxy settings* means the `HTTPS_PROXY` / `ALL_PROXY` /
     `HTTP_PROXY` environment variables, and on Windows, when none is set,
     the proxy from the system settings. On **macOS and Linux the
     system's proxy settings are not read**, and a program started from
     the Dock or the menu has no such variables: choose *Manual proxy
     configuration* there.
   - Automatic proxy scripts (PAC, WPAD) are not supported. Enter the
     proxy by hand.
   - A stale variable is as bad as a missing one: a VPN or proxy tool that
     is no longer running leaves Glossa pointing at a closed port. Choose
     *No proxy*.
2. **Timeouts.** *Options → Internet → Timeout* (10 s by default) and
   *Retries*. A slow proxy or a far-away service may need more. AI
   endpoints have a timeout of their own in their settings.
3. **Certificate errors.** Glossa checks certificates against the
   system's trust store and has no switch to skip the check. Behind a
   proxy or antivirus that inspects HTTPS, its root certificate has to be
   installed in the system store (on Linux, `SSL_CERT_FILE` or
   `SSL_CERT_DIR` can point at it). A wrong system clock produces the
   same errors.
4. **One service only.** A service that is blocked where you are, or
   whose website changed, fails while the others work. Switch to another
   in the service strip, and leave *If a service fails, try the next one*
   on (*Options → Services*). Keyless services depend on the websites they
   talk to; a broken one is fixed in a later release.
5. **A local server** (Ollama, LM Studio, LibreTranslate, DeepLX) on
   `localhost` is always reached directly, never through the proxy. "no
   DeepLX server answers at …" and the like mean the server is not
   running at that address.

## "… is rate-limiting us"

The service answered that too many requests came from your address
(HTTP 429, or a captcha page). It is the service's decision, usually
about the address you share with others: a VPN exit, a company or
provider's NAT.

- Wait. The message says how long when the service told Glossa.
- With fail-over on, Glossa already moved on to the next enabled service;
  the popup says "From …: … did not answer".
- Google Translate is asked at three addresses in turn, and an address
  that refuses is left alone for 30 seconds, doubling up to 10 minutes.
  The error shows only when all three refuse.
- Switch to another service, change the VPN exit, or use a service with
  your own key (*Options → Services*), which has its own allowance.
- Instant translation (translate as you type) multiplies requests; turn
  it off if it triggers the limit.

## Text recognition finds nothing

- **macOS, nothing is captured or the picture is only the desktop
  background**: Screen Recording is not granted, or Glossa was not
  restarted after granting it ([above](#macos-permissions)).
- **"No text found in the selected area."** The engine read the picture
  and found no text. Select a larger region with some margin around the
  text. Very small text reads better after zooming the source in.
- **"OCR is not available: …"** names what is missing. Google Lens needs
  the network; the offline engines need their parts: Windows OCR a
  language pack for the script, OneOCR the Windows 11 Snipping Tool,
  Tesseract its program and language data (`glossa-cli doctor` lists the
  engines and their languages).
- **Wrong or garbled letters with an offline engine**: set the OCR
  language instead of *auto* (*Options → Text recognition*), or use
  Google Lens for that text.
- **Lens answers slowly or not at all**: it goes through the same proxy
  settings as translation; see [Network](#network-proxy-and-certificate-errors).
- **Wayland**: the first capture asks through the desktop's dialog;
  declining it leaves nothing to read.

More in [Text recognition, troubleshooting](ocr.md#troubleshooting).

## A module is waiting for approval

"This module is waiting for your approval" on a call, or *needs approval*
beside a module: a module you installed yourself does not run until you
have approved what it asks for.

- Open *Options → Modules*. At the top, under *Waiting for your
  approval*, each module lists the hosts it reaches, whether it runs a
  program or a native library, the files it downloads and its actions.
  *Approve* lets it run; *Decline* switches it off.
- **It asks again after an update**: the new version declares more than
  you approved (a new host, a new download). The prompt marks what is
  new.
- **Every module asks again**: the settings file was replaced or reset
  ([below](#settings-were-reset)); approvals are kept in it.
- From a terminal: `glossa-cli module pending`, then `glossa-cli module
  approve ID`.

After approving, switch the service on in *Options → Services* if it is
not in the strip. Details: [Permissions and trust](modules/README.md#permissions-and-trust).

## Module files do not download

"This service needs files that have not been downloaded yet": open the
service's settings (*Options → Services*) and press *Download* under
*Files*. Nothing is downloaded on its own unless `[modules]
auto_download_assets` is on.

If the download fails:

- It needs the hosts the module names, and those sites redirect to their
  file servers: `github.com` (and `githubusercontent.com`),
  `huggingface.co`, `www.modelscope.cn`; for Python modules also
  `pypi.org` and `files.pythonhosted.org`. A firewall or a regional block
  on any of them stops it. Downloads use Glossa's proxy settings.
- Press *Download* again: it resumes where it stopped.
- "damaged, deleted": the file did not match its checksum and was
  removed. Download again; if it repeats, something between you and the
  server alters the file.
- Models are large (the Local LLM model is over a gigabyte): check the
  free space in the downloads folder
  ([where](configuration.md#where-the-files-are)).
- A module whose manifest still has an all-zero checksum for a file is
  refused: that file has not been published yet. The Local LLM module is
  in that state in this version.
- From a terminal, with the reason printed: `glossa-cli assets status ID`,
  `glossa-cli assets download ID`, `glossa-cli python status`.

## Settings were reset

"config.toml could not be read, so Glossa started with the default
settings": the file had a syntax error or a value of the wrong type
(usually after a hand edit). Nothing is lost: the file was moved to
`config.toml.broken-<number>` in the same folder. Quit Glossa, fix the
line the message names, rename the file back to `config.toml` and start
again. `glossa-cli config show` checks a file without moving it.

Edit `config.toml` only while Glossa is not running: the program rewrites
the whole file when it saves.

A window that opens off screen (a monitor was unplugged): delete that
window's table under `[windows]` in `config.toml`.

## The log

The desktop program writes `glossa.log` in its cache folder:

| system | log file |
|---|---|
| Windows | `%LOCALAPPDATA%\Glossa\glossa.log` |
| macOS | `~/Library/Caches/Glossa/glossa.log` |
| Linux | `~/.cache/glossa/glossa.log` |
| portable copy | `data\cache\glossa.log` beside the program |
| `GLOSSA_HOME` set | `$GLOSSA_HOME/cache/glossa.log` |

`glossa-cli doctor` prints the path in use. The file only grows; delete it
while Glossa is not running if it gets large.

For more detail, set `GLOSSA_LOG` before starting Glossa. It takes a
level (`debug`, `trace`) or levels per part of the program:

```powershell
# Windows (PowerShell): quit Glossa from the tray first
$env:GLOSSA_LOG = "glossa=debug,glossa_platform=debug,glossa_engine=debug"
& "$env:LOCALAPPDATA\Programs\Glossa\glossa.exe"
```

```bash
# macOS: quit Glossa from the menu bar first
GLOSSA_LOG=debug /Applications/Glossa.app/Contents/MacOS/glossa

# Linux
GLOSSA_LOG=debug glossa
```

`glossa-cli` logs to the terminal instead; `-v` is the same as
`GLOSSA_LOG=debug`. A crash is written to the log as well; there is no
crash reporter that sends anything.

## Reporting a bug

Open an issue at
[github.com/bropines/glossa/issues](https://github.com/bropines/glossa/issues/new/choose),
in English or Russian. Include:

1. **What you did, what you expected, what happened.** The exact text of
   any message.
2. **The output of `glossa-cli doctor`.** It has the version, the system,
   what works in the session, the folders and the proxy. It prints no
   keys or tokens and the proxy address without its password, but it does
   show folder paths with your user name; trim what you do not want
   public.
3. **The part of the log around the problem**, ideally from a run with
   `GLOSSA_LOG=debug`.
4. For a service or module: its name, whether it is built in, a module or
   a classic script, and the output of `glossa-cli modules`.
5. For something on screen: a screenshot, the theme, and the display
   scaling.

**Never paste API keys, tokens or passwords.** Check the log and any part
of `config.toml` before posting; replace secrets with `***`. Security
problems are reported privately: [SECURITY.md](../SECURITY.md).
