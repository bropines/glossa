# Installing Glossa

Every [release](https://github.com/bropines/glossa/releases) has these files
(`1.2.3` stands for the version). Each comes for two kinds of processor:
`x86_64` is an ordinary Intel or AMD PC and an Intel Mac, `arm64` is ARM
(Apple silicon Macs, Snapdragon Windows laptops, ARM Linux boards).

| system | file | what it is |
|---|---|---|
| Windows 10, 11 | `glossa-1.2.3-x86_64-windows-setup.exe`, `glossa-1.2.3-arm64-windows-setup.exe` | installer |
| | `glossa-1.2.3-x86_64-windows-portable.zip`, `glossa-1.2.3-arm64-windows-portable.zip` | portable archive |
| macOS 11+ Apple silicon | `glossa-1.2.3-arm64-macos.dmg` | disk image with `Glossa.app` |
| macOS 10.15+ Intel | `glossa-1.2.3-x86_64-macos.dmg` | the same, for Intel |
| | `glossa-1.2.3-*-macos.zip` | `Glossa.app`, `glossa-cli` and the documents |
| Debian 12, Ubuntu 22.04 and newer | `glossa_1.2.3_amd64.deb`, `glossa_1.2.3_arm64.deb` | package |
| other Linux | `glossa-1.2.3-x86_64.AppImage`, `glossa-1.2.3-aarch64.AppImage` | single file to run |
| | `glossa-1.2.3-x86_64-linux-portable.tar.gz`, `glossa-1.2.3-arm64-linux-portable.tar.gz` | portable archive |
| | `SHA256SUMS` | checksums of all of the above |

The Linux builds have not been tried on real hardware yet: they are built
and smoke-tested by the release workflow only. Reports are welcome.

`Bropines.Glossa*.yaml` and `glossa.json` are winget and Scoop manifests (see
[below](#winget-and-scoop)).

Not sure which file to take? This chart picks one by system, processor and
installer or portable copy:

<p align="center">
  <img src="images/downloads.svg" alt="Which download to take: the system, then the processor, then an installer or a portable copy" width="760">
</p>

- [Windows](#windows): [the installer](#the-installer),
  [the portable archive](#the-portable-archive),
  [winget and Scoop](#winget-and-scoop)
- [macOS](#macos)
- [Linux](#linux): [Debian and Ubuntu](#debian-and-ubuntu),
  [AppImage](#appimage), [the archive](#the-archive)
- [Where Glossa keeps your files](#where-glossa-keeps-your-files)
- [Building the packages](#building-the-packages)

## Windows

### The installer

Run `glossa-…-windows-setup.exe`. It installs for your account only, into
`%LOCALAPPDATA%\Programs\Glossa`, and needs no administrator rights. The
first page offers to install for all users instead (into `Program Files`;
that asks for administrator rights).

It puts Glossa in the Start menu, and on request on the desktop. *Start
Glossa when I sign in to Windows* is the same switch as *Options → Basics →
Start with Windows*: either one can turn it on or off later.

Until the releases are signed, SmartScreen may stop the installer the first
time: *More info → Run anyway*. (The release workflow signs `glossa.exe`,
`glossa-cli.exe` and the installer as soon as the repository has a code
signing certificate in its secrets, `WINDOWS_SIGN_PFX` and
`WINDOWS_SIGN_PASSWORD`.)

Glossa runs without administrator rights. Windows does not let such a
program read or replace the selection in a program running as
administrator (an elevated terminal, say); start Glossa as administrator
too if you need that.

**Upgrading**: run the installer of the new version. It closes a running
Glossa first; your settings stay.

**Removing**: *Settings → Apps → Installed apps → Glossa → Uninstall*. It
asks whether to delete your settings, history and modules too; the answer
is no unless you say yes.

Take the `arm64` installer on an ARM laptop; the `x86_64` one also installs
there and runs under emulation, more slowly.

**Unattended installs** take the usual Inno Setup switches:

```bat
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /ALLUSERS /TASKS="desktopicon,autostart"
"%LOCALAPPDATA%\Programs\Glossa\unins000.exe" /VERYSILENT
```

A silent upgrade keeps "start with Windows" as it is, and starts Glossa
again in the tray if it was running. A silent uninstall keeps the settings.

### The portable archive

Unpack the `-portable.zip` anywhere (a USB stick, say) and run `glossa.exe`.
The `portable.txt` beside it keeps settings, history and modules in a `data`
folder next to `glossa.exe`, so the folder carries them along. Delete
`portable.txt` to keep them in `%APPDATA%\Glossa` instead, as the installer
does. To upgrade, unpack the new version over the old folder; `data` stays.

### winget and Scoop

Each release carries manifests for both, with that release's URLs and
checksums. They become usable only once the repository is public and the
release is published; until then the download links behind them do not
answer. After that:

- **Scoop**: `scoop install https://github.com/bropines/glossa/releases/download/v1.2.3/glossa.json`
  (or add the manifest to a bucket). It installs the portable archive for
  your processor, keeps its `data` folder across updates, and puts
  `glossa-cli` on the `PATH`.
- **winget**: the three `Bropines.Glossa*.yaml` files go in a pull request to
  [winget-pkgs](https://github.com/microsoft/winget-pkgs) (under
  `manifests/b/Bropines/Glossa/1.2.3/`); once merged,
  `winget install Bropines.Glossa`. Before that, `winget install --manifest
  <folder with the three files>` tries them locally.

## macOS

Take `arm64` for an Apple silicon Mac (M1 and later) and `x86_64` for an
Intel one. Open the `.dmg` and drag `Glossa.app` onto *Applications*. Glossa is a
menu-bar app: it has no Dock icon until one of its windows is open.

The app is signed ad hoc, not notarised, so macOS refuses the first launch.
On macOS 15 and later open it once, then *System Settings → Privacy &
Security → Open Anyway* (on older systems, right-click → Open). Or in
Terminal:

```bash
xattr -dr com.apple.quarantine /Applications/Glossa.app
```

The [README](../README.md#macos) lists the permissions it asks for. To remove
Glossa, quit it from the menu bar and move `Glossa.app` to the Bin; settings
are in `~/Library/Application Support/Glossa`.

The `.zip` holds the same app plus `glossa-cli` and the documents. A Mac
app keeps its settings in the user's Library rather than beside itself; to
carry them elsewhere, start it with `GLOSSA_HOME` set (see
[Configuration](configuration.md)).

## Linux

Not yet tried on real hardware (see above). `x86_64` and `amd64` are for
ordinary PCs, `arm64` and `aarch64` for ARM machines (a Raspberry Pi 4 or 5
with a 64-bit system, ARM laptops).

### Debian and Ubuntu

```bash
sudo apt install ./glossa_1.2.3_amd64.deb     # or glossa_1.2.3_arm64.deb
```

apt pulls in what Glossa needs (GTK 3, ALSA, OpenGL) and, unless you turn
recommends off, Tesseract for offline text recognition and the AppIndicator
library for the tray icon. On GNOME the tray icon also needs the
*AppIndicator and KStatusNotifierItem Support* extension (Ubuntu has it).
Python modules need `python3`.

The package installs:

| path | what |
|---|---|
| `/usr/bin/glossa`, `/usr/bin/glossa-cli` | the programs |
| `/usr/share/applications/io.github.bropines.Glossa.desktop` | the menu entry |
| `/usr/share/icons/hicolor/256x256/apps/io.github.bropines.Glossa.png` | the icon |
| `/usr/share/glossa/sdk/python/` | the Python module SDK, found by Glossa |
| `/usr/share/glossa/examples/modules/` | example modules, to copy into `~/.config/glossa/modules` |
| `/usr/share/doc/glossa/` | README, changelog, licences |

`sudo apt remove glossa` removes it; your settings in `~/.config/glossa`
stay.

### AppImage

```bash
chmod +x glossa-1.2.3-x86_64.AppImage
./glossa-1.2.3-x86_64.AppImage                      # the desktop program
./glossa-1.2.3-x86_64.AppImage cli translate -t ru hello   # glossa-cli
```

It holds only Glossa itself and uses the system's GTK 3, ALSA and glibc. It
is built on Debian 12, like the `.deb`, so distributions of that age or
newer run it (Ubuntu 22.04, Fedora 37 and later). Running it
needs FUSE; without it, add `--appimage-extract-and-run`. Turning on *Start
at login* registers the AppImage file, so keep it where it is afterwards.

### The archive

Unpack the `-portable.tar.gz` anywhere and run `./glossa`; `./install.sh`
adds it to your application menu. It needs the same libraries as the
AppImage. As on Windows, `portable.txt` keeps the settings in `data/` next to
the program; delete it to use `~/.config/glossa`.

## Where Glossa keeps your files

| | Windows | macOS | Linux |
|---|---|---|---|
| settings, history, modules | `%APPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.config/glossa` |
| cache and log | `%LOCALAPPDATA%\Glossa` | `~/Library/Caches/Glossa` | `~/.cache/glossa` |

A `portable` file or a `data` folder next to the executable, or the
`GLOSSA_HOME` variable, moves both there; see
[Configuration](configuration.md).

API keys, the proxy password and the Lens token stay in the settings file
unless *Options → Basics* (or the welcome guide) puts them in the system
keychain: Windows Credential Manager, the macOS Keychain, or the Secret
Service on Linux. The keychain stays on the computer, so a portable copy
on a USB stick is better off with the file.

## Building the packages

The release workflow (`.github/workflows/release.yml`) builds everything on
a version tag. The scripts it calls work locally too:

| script | makes | needs |
|---|---|---|
| `packaging/linux/build-deb.sh <bin-dir> <version> [out]` | `glossa_<version>_<arch>.deb` | `dpkg-deb`, `dpkg-shlibdeps` (dpkg-dev) |
| `packaging/linux/build-appimage.sh <bin-dir> <version> [out]` | `glossa-<version>-<arch>.AppImage` | [appimagetool](https://github.com/AppImage/appimagetool) (`APPIMAGETOOL`, `APPIMAGE_RUNTIME`, `APPIMAGE_ARCH`: `x86_64` or `aarch64`) |
| `iscc /DMyAppVersion=<version> /DArch=<x64\|arm64> /DStageDir=<unpacked zip> packaging\windows\glossa.iss` | the Windows installer | Inno Setup 6.3+ |
| `packaging/fill-manifests.sh <version> <dist>` | winget and Scoop manifests | the setup `.exe` and portable `.zip` of both architectures in `<dist>` |

`<bin-dir>` holds `glossa` and `glossa-cli`, e.g.
`target/x86_64-unknown-linux-gnu/release`. Build the `.deb` on the oldest
system it should install on: its dependencies are read off that system's
libraries. The macOS disk images are made by `hdiutil` in the workflow.
The ARM builds for Windows and Linux are cross-compiled on x86-64 runners
(Linux against Debian's arm64 libraries, with `DEB_HOST_ARCH=arm64` for
`dpkg-shlibdeps`). Running the workflow by hand (*Actions → release → Run
workflow*) builds everything without making a release.
