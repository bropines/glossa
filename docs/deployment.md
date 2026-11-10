# Deployment

For whoever installs Glossa on machines that are not their own: unattended
installation, where the files go, preparing settings for users, the proxy,
the settings that narrow what Glossa may do, the hosts it talks to, and
removal. End users want [Installing](install.md) instead.

Glossa is a beta made for single users. This page describes what the
installers and the program really do; it does not claim a managed-desktop
feature set. Linux packages are built and smoke-tested but have not been
tried on real hardware yet.

- [No policy layer](#no-policy-layer)
- [Unattended installation](#unattended-installation):
  [Windows](#windows), [macOS](#macos), [Linux](#linux)
- [Where the files are](#where-the-files-are)
- [Preparing settings for users](#preparing-settings-for-users)
- [Proxy and certificates](#proxy-and-certificates)
- [Settings that reduce risk](#settings-that-reduce-risk)
- [Network access](#network-access)
- [Updating](#updating)
- [Removal](#removal)

## No policy layer

There is no machine-wide configuration, no administrator lock and no
Group Policy or MDM template. Every setting lives in the user's own
`config.toml`, which the user can edit and which the program rewrites when
they change something in *Options*. Nothing on this page can be enforced
against the user from inside Glossa.

What you can do: put a prepared `config.toml` in place before the first
start ([below](#preparing-settings-for-users)), and enforce the rest from
outside (a firewall, application control, file permissions on the program
folder). If you need settings a user cannot change, Glossa does not offer
that yet.

## Unattended installation

Every release has a `SHA256SUMS` file with the checksum of each download.
The files carry no publisher signature yet: Windows binaries are signed
only when the release was built with a certificate (none so far), and the
macOS app is signed ad hoc and not notarised. Allow rules in application
control therefore have to name a path or a hash, not a publisher.

### Windows

The installer is Inno Setup and takes its usual switches. It needs
Windows 10 or later; the `x86_64` installer also installs on ARM, the
`arm64` one only there.

```bat
:: for the current user, no administrator rights
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER

:: for all users, into Program Files (must run elevated)
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /ALLUSERS

:: optional: /DIR="D:\Apps\Glossa"  /TASKS="desktopicon,autostart"  /LOG="C:\Temp\glossa-setup.log"
```

| | per user (`/CURRENTUSER`, the default) | all users (`/ALLUSERS`) |
|---|---|---|
| program folder | `%LOCALAPPDATA%\Programs\Glossa` | `%ProgramFiles%\Glossa` |
| Start menu entry | the user's | common |
| needs elevation | no | yes |

What the installer does and does not do:

- **Tasks.** `desktopicon` adds a desktop shortcut, `autostart` starts
  Glossa in the tray at sign-in. Both are off unless named in `/TASKS`.
- **Autostart is per user.** It is the value `Glossa` under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  (`"<folder>\glossa.exe" --hidden`), written into the hive of the account
  that runs the installer. An all-users install run by a deployment tool
  under the system account does not set it for the people who sign in. To
  start Glossa for a user, create that value in the user's hive (a logon
  script or a Group Policy preference); *Options → Basics → Start with
  Windows* then shows it as on.
- **Upgrades.** Run the new installer over the old one. A silent upgrade
  closes a Glossa running from the target folder, keeps autostart as it
  was, and starts Glossa again in the tray if it was running.
- **No data is written.** The installer does not create `config.toml`; the
  program does, at its first start, in the user's profile.
- **Never portable.** The installer leaves out the `portable` marker and
  any `data` folder, so an installed copy always keeps its data in the
  profile.

Uninstall silently with the uninstaller in the program folder; it keeps the
user's data (see [Removal](#removal)):

```bat
"%LOCALAPPDATA%\Programs\Glossa\unins000.exe" /VERYSILENT
```

**The portable archive** (`-windows-portable.zip`) is the same program
with a `portable.txt` beside it: unpack it to a folder the user can write
to, and settings, history, modules and downloads stay in `data\` next to
`glossa.exe`. Starting at login, if the user turns it on, is still a value
in the user's registry hive.

**winget and Scoop.** Glossa is not in the winget community repository and
has no Scoop bucket. Each release carries manifests that can be used
directly ([Installing](install.md#winget-and-scoop)).

### macOS

There is no `.pkg`. The release has a `.dmg` and a `.zip`, each holding
`Glossa.app`; the `.zip` also has `glossa-cli`. Take `arm64` for Apple
silicon (macOS 11 or later) and `x86_64` for Intel (macOS 10.15 or later).
A scripted install is a copy:

```bash
hdiutil attach -nobrowse -quiet glossa-1.2.3-arm64-macos.dmg   # mounts as "/Volumes/Glossa 1.2.3"
ditto "/Volumes/Glossa 1.2.3/Glossa.app" /Applications/Glossa.app
hdiutil detach -quiet "/Volumes/Glossa 1.2.3"
xattr -dr com.apple.quarantine /Applications/Glossa.app        # only needed if the download was quarantined
```

- **Gatekeeper.** The app is signed ad hoc and not notarised. A copy that
  carries the quarantine attribute (downloaded with a browser) is refused
  at first launch until the user allows it in *System Settings → Privacy &
  Security*, or the attribute is removed as above.
- **Permissions are granted by the user.** Reading the selection needs
  Accessibility, capturing the screen needs Screen Recording, and mouse
  mode may need Input Monitoring. macOS ties a grant to the app's
  signature, and an ad-hoc signature is different in every build, so the
  grants have to be given again after each update. Granting them centrally
  with a privacy (PPPC) profile has not been tried, and the lack of a
  Developer ID leaves such a profile no stable identity to name.
- **Bundle identifier:** `io.github.bropines.glossa`.
- **Starting at login** is a launch agent the program writes when the user
  turns it on: `~/Library/LaunchAgents/app.glossa.plist`.
- `glossa-cli` is not inside the app; copy it from the `.zip` to a folder
  on the `PATH` if scripts need it.

### Linux

Not tried on real hardware yet. The packages are built on Debian 12.

```bash
# Debian 12, Ubuntu 22.04 and newer
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y ./glossa_1.2.3_amd64.deb
```

The package depends on the libraries Glossa links against (GTK 3, ALSA,
OpenGL), recommends `tesseract-ocr` and an AppIndicator library, and
suggests `python3`. It has no maintainer scripts and installs no service.
There is no apt repository: a new version is a new `.deb` to install.

The AppImage is one file to place and mark executable; it needs FUSE, or
`--appimage-extract-and-run`. The `-linux-portable.tar.gz` is the portable
layout (data in `data/` beside the program); its `install.sh` adds a menu
entry for the current user only.

## Where the files are

The program keeps everything per user. [Configuration](configuration.md#where-the-files-are)
has the full table of files.

| | Windows | macOS | Linux |
|---|---|---|---|
| settings, history, user modules, themes (data) | `%APPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.config/glossa` (`$XDG_CONFIG_HOME`) |
| log and caches | `%LOCALAPPDATA%\Glossa` | `~/Library/Caches/Glossa` | `~/.cache/glossa` (`$XDG_CACHE_HOME`) |
| module downloads: `models`, `runtimes`, `venvs` | `%LOCALAPPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.local/share/glossa` |
| settings file | `…\Glossa\config.toml` | `…/Glossa/config.toml` | `…/glossa/config.toml` |
| log | `%LOCALAPPDATA%\Glossa\glossa.log` | `~/Library/Caches/Glossa/glossa.log` | `~/.cache/glossa/glossa.log` |

On Windows the data folder is in the roaming profile and the downloads are
in the local one, so models (which can be over a gigabyte) do not roam.

Two things move all of it into one folder:

- **`GLOSSA_HOME`** (an environment variable; `--home DIR` on the command
  line does the same): the data folder is that folder, the cache is
  `cache` inside it, and the downloads are inside it too. It is read by
  both `glossa` and `glossa-cli`. While it is set, Glossa leaves the
  autostart entry alone.
- **Portable mode**: a file named `portable` or `portable.<anything>`, or
  a folder named `data`, next to the executable puts everything into
  `data` beside it. The program folder must be writable by the user, so
  this does not suit `Program Files`.

`glossa-cli config path` prints the folder in use; `glossa-cli doctor`
prints all of them.

## Preparing settings for users

Put a `config.toml` into the user's data folder before Glossa first
starts (a logon script, a profile template, your deployment tool's file
step). Every key is optional, so the file needs only what you want to
differ from the defaults. All keys are in
[Configuration](configuration.md).

```toml
# %APPDATA%\Glossa\config.toml: a starting point, not a lock
[general]
ui_language = "en"
first_language = "de"
second_language = "en"
secret_storage = "system"      # keys go to the system keychain
show_welcome = true            # see below

[network.proxy]
mode = "manual"
scheme = "http"
host = "proxy.example.internal"
port = 3128

[modules]
trust_initialised = true       # see below
allow_native = false
allow_legacy_scripts = false

[server]
enabled = false
```

Things to know about a prepared file:

- **It is a starting point.** Glossa rewrites the whole file when it saves
  (comments and key order are lost, every key is written out), and the user
  can change anything.
- **The welcome guide and first-run hints** show only when there is no
  settings file at all. With a prepared file, `[general] show_welcome`,
  `show_tray_hint` and `[appearance] follow_system_theme` read as `false`
  unless the file sets them. Set `show_welcome = true` to keep the guide.
- **`[modules] trust_initialised`.** A settings file without this key is
  taken for one from before module approvals existed: on the first start
  Glossa approves every hand-installed module it finds, for what it
  declares at that moment, and then writes the key. Set it to `true` in a
  prepared file so that any module present, or added later, waits for the
  user's approval. Leave it out only if you ship modules you have vetted
  and want them approved without a prompt.
- **Do not put a control API token in it.** Leave `[server] token` out and
  each installation generates its own. A shared token is a shared
  password.
- **Keys.** An API key written in the file is plain text for everyone who
  can read the file or the share it is deployed from. With
  `secret_storage = "system"` Glossa moves it into the user's keychain
  the next time it saves the settings; until then it is in the file.
  Better to let users enter their own keys.
- **A file that does not parse** is moved aside as
  `config.toml.broken-<time>` and the defaults are used, with a notice.
  Check a prepared file with `glossa-cli --home <folder with it> config
  show`.
- **Modules, themes and language packs** can be shipped the same way, in
  `modules/`, `themes/` and `locales/` of the data folder, or in the
  folders next to the program, which the installer owns: a module in
  `<program folder>\modules` (or `/usr/share/glossa/modules`) counts as
  shipped with Glossa and needs no approval, so protect that folder from
  users' writes. A per-user install in `%LOCALAPPDATA%\Programs` is
  writable by the user.

## Proxy and certificates

Set in *Options → Internet*, or `[network.proxy]`:

| `mode` | what happens |
|---|---|
| `"system"` (default) | The environment variables `HTTPS_PROXY`, `ALL_PROXY`, `HTTP_PROXY` (first one set, with `NO_PROXY`). If none is set, on Windows the user's system proxy (the static server and bypass list of *Internet Options*). |
| `"manual"` | `scheme` (`http`, `https`, `socks4`, `socks5`), `host`, `port`, and optionally `username` and `password`. |
| `"none"` | Direct connections. |

Limits worth knowing:

- **No automatic configuration.** Proxy auto-discovery (WPAD) and PAC
  scripts are not implemented. A Windows machine configured only with a
  setup script gets a direct connection; use `manual`.
- **macOS and Linux system settings are not read.** There, `system` means
  the environment variables only, and a program started from the Dock or
  a desktop menu usually has none. Use `manual`.
- **Authentication** is a user name and password. Integrated (NTLM,
  Kerberos) proxy authentication is not implemented.
- **Loopback is never proxied**: `localhost`, `127.0.0.1`, `::1`.
- **What follows the setting.** Every request Glossa makes itself, the
  host calls of modules, module downloads, and `uv` when it installs Python
  packages. A Python, Java or process module that opens its own
  connections does not.

**TLS.** Certificates are checked against the operating system's trust
store, so a root certificate your organisation installs there (for TLS
inspection, say) is trusted by Glossa as well. On Linux the usual
locations and `SSL_CERT_FILE` / `SSL_CERT_DIR` are read; if the store is
empty, a built-in copy of the Mozilla roots is used. There is no setting
that turns certificate checking off. `glossa-cli doctor` prints the proxy
in use and which store is checked.

`[network] timeout_secs` (10) and `max_retries` (1) apply to every service
request; AI endpoints have their own timeout in their settings.

## Settings that reduce risk

All of these are in the user's `config.toml` and can be changed by the
user ([No policy layer](#no-policy-layer)).

| to | set | notes |
|---|---|---|
| Refuse native modules (shared libraries that run inside Glossa) | `[modules] allow_native = false` | Default `true`. |
| Refuse classic QTranslate `Service.js` scripts | `[modules] allow_legacy_scripts = false` | Default `true`. Also `[classic] use_services = false` for a linked classic installation. |
| Make every hand-installed module wait for approval | `[modules] trust_initialised = true` in a prepared file | See [above](#preparing-settings-for-users). Approvals are `[modules.granted.<id>]`; deleting a table withdraws one. |
| Never download module files without a click | `[modules] auto_download_assets = false` | The default. |
| Remove a service, built in or not | `[modules] disabled = ["id", …]` | Applies to built-in and bundled services, AI endpoints and modules alike: a listed id is not loaded at all. `glossa-cli services` lists the ids. The list for every online service is in [Privacy](privacy.md#working-offline). |
| Keep screen regions off Google Lens | `[ocr] provider = "windows-ocr"` (or `macos-vision`, `tesseract`) and `"lens"` in `[modules] disabled` | Choosing another engine alone is not enough: OCR [falls over](privacy.md#requests-you-might-not-expect) to Lens when the chosen engine fails. |
| Send each text to one service only | `[services] fail_over = false` | Language detection can still go to another service; see [Privacy](privacy.md#requests-you-might-not-expect). |
| Switch the control API off | `[server] enabled = false` | Default `true`, on `127.0.0.1:60828`. Takes effect at the next start. `glossa-cli action …` then cannot reach the running program. |
| Require the token on pot-app's routes too | `[server] pot_compat_without_token = false` | Default `true`. |
| Switch the D-Bus service off (Linux) | `[server] dbus = false` | Default `true`. It has no token. |
| Keep keys out of the settings file | `[general] secret_storage = "system"` | Default `"file"`. The control API token stays in the file either way. |
| Keep no history | `[history] enabled = false`, or `clear_on_exit = true` | Also `[general] save_contents_on_exit = false`, which otherwise keeps the main window's text in `config.toml`. |
| Keep the log quiet | do not set `GLOSSA_LOG` | The default level records events and errors, not translated text. |

Glossa never asks for administrator rights and does not need them. Do not
run it elevated to reach elevated windows unless that is really wanted: its
modules then run elevated too.

## Network access

**Inbound: none.** Glossa listens only on `127.0.0.1:60828` (the control
API, unless switched off or re-bound in `[server] bind`).

**Outbound**, all HTTPS on port 443 unless you configure a service
otherwise, made by `glossa` and `glossa-cli` (`glossa.exe`,
`glossa-cli.exe`). Allow what the features in use need:

| feature | hosts |
|---|---|
| Google Translate, Google Dictionary, Google speech | `clients5.google.com`, `translate.googleapis.com`, `translate.google.com` (or `translate.google.<domain>` with `[advanced] preferred_domain`) |
| Microsoft Translator | `www.bing.com`; with a key `api.cognitive.microsofttranslator.com` |
| MyMemory | `api.mymemory.translated.net` |
| Yandex (keyless), Yandex dictionary | `translate.yandex.net` |
| Yandex Cloud (key) | `translate.api.cloud.yandex.net` |
| DeepL (keyless) | `oneshot-free.www.deepl.com` |
| DeepL API (key) | `api-free.deepl.com`, `api.deepl.com` |
| Lara | `webapi.laratranslate.com` |
| ModernMT | `www.modernmt.com` |
| LibreTranslate, Lingva, DeepLX | the server set in the service's options (defaults: `libretranslate.com`, `lingva.ml`, `127.0.0.1:1188`) |
| AI endpoints | the endpoint's base URL: `api.openai.com`, `openrouter.ai`, `api.deepseek.com`, `generativelanguage.googleapis.com`, `api.groq.com`, `api.mistral.ai`, or your own |
| Google Lens (text recognition) | `lensfrontend-pa.googleapis.com` |
| Dictionaries | `www.multitran.com`, `api.lingvolive.com`, `www.oxfordlearnersdictionaries.com`, `en.wiktionary.org`, `dict.youdao.com`, `dic.academic.ru`, `translate.academic.ru`, `*.wikipedia.org`, `api.urbandictionary.com` |
| PaddleOCR module files | `github.com`, `www.modelscope.cn` |
| Local LLM module files | `github.com`, `huggingface.co` |
| Managed Python and `uv` | `github.com` |
| Python packages | `pypi.org`, `files.pythonhosted.org` (requested by `uv`, a separate program in the `runtimes` folder) |
| Other modules | the hosts in the module's `[permissions] network`, shown when it is approved |

Downloads from `github.com`, `huggingface.co` and `www.modelscope.cn` are
answered with redirects to those sites' file servers (for GitHub release
files, hosts under `githubusercontent.com`), which have to be reachable
too. Those names belong to the sites and can change.

Programs other than Glossa's own that may connect: the Python interpreter
and other programs that process modules start (they run from the
`runtimes`, `venvs` and `models` folders or from the module's folder), and
`lens_scan` if chrome-lens-py is used for Lens.

Glossa makes no request at start-up and none in the background: no
telemetry and no update check ([Privacy](privacy.md)).

## Updating

Glossa does not update itself and does not check for new versions.
Deploy a new version the way the first one was installed: the Windows
installer upgrades in place, on macOS replace `Glossa.app`, on Debian and
Ubuntu install the new `.deb`. Settings are kept, and a settings file from
an older version keeps loading. Releases and their notes are on the
[releases page](https://github.com/bropines/glossa/releases); what changed
is in the [changelog](../CHANGELOG.md).

Module programs (`paddle-ocr-v…`, `local-llm-v…`) are released separately
from the application and appear in the same list.

## Removal

[Removing Glossa completely](install.md#removing-glossa-completely) has
the steps for each system. In short, the uninstallers remove the program
and leave user data unless told otherwise:

| | removed by the uninstaller | left behind |
|---|---|---|
| Windows installer | the program folder, the Start menu and desktop shortcuts, the uninstall entry, the autostart value if it points at this copy | `%APPDATA%\Glossa` and `%LOCALAPPDATA%\Glossa` of every user (an interactive uninstall offers to delete them for the user running it; a silent one never does), Credential Manager entries named `Glossa/…`, autostart values in other users' hives |
| macOS | nothing: there is no uninstaller | everything listed in the install guide |
| `.deb` | `/usr/bin/glossa`, `/usr/bin/glossa-cli`, `/usr/share/…` | each user's `~/.config/glossa`, `~/.cache/glossa`, `~/.local/share/glossa`, `~/.config/autostart/glossa.desktop`, keyring entries |
