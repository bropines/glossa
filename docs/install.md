# Installing Glossa

Every [release](https://github.com/bropines/glossa/releases) has these files
(`1.2.3` stands for the version):

| system | file | what it is |
|---|---|---|
| Windows 10, 11 (x86-64) | `glossa-1.2.3-x86_64-windows-setup.exe` | installer |
| | `glossa-1.2.3-x86_64-windows.zip` | portable archive |
| macOS 11+ Apple silicon | `glossa-1.2.3-arm64-macos.dmg` | disk image with `Glossa.app` |
| macOS 10.15+ Apple silicon or Intel | `glossa-1.2.3-universal-macos.dmg` | the same, for both |
| | `glossa-1.2.3-*-macos.zip` | `Glossa.app`, `glossa-cli` and the documents |
| Debian 12, Ubuntu 22.04 and newer (x86-64) | `glossa_1.2.3_amd64.deb` | package |
| other Linux (x86-64) | `glossa-1.2.3-x86_64.AppImage` | single file to run |
| | `glossa-1.2.3-x86_64-linux.tar.gz` | archive |
| | `SHA256SUMS` | checksums of all of the above |

`Bropines.Glossa*.yaml` and `glossa.json` are winget and Scoop manifests (see
[below](#winget-and-scoop)).

## Windows

### The installer

Run `glossa-…-windows-setup.exe`. It installs for your account only, into
`%LOCALAPPDATA%\Programs\Glossa`, and needs no administrator rights. The
first page offers to install for all users instead (into `Program Files`;
that asks for administrator rights).

It puts Glossa in the Start menu, and on request on the desktop. *Start
Glossa when I sign in to Windows* is the same switch as *Options → Basics →
Start with Windows*: either one can turn it on or off later.

The installer is not signed yet, so SmartScreen may stop it the first time:
*More info → Run anyway*.

**Upgrading**: run the installer of the new version. It closes a running
Glossa first; your settings stay.

**Removing**: *Settings → Apps → Installed apps → Glossa → Uninstall*. It
asks whether to delete your settings, history and modules too; the answer
is no unless you say yes.

**Unattended installs** take the usual Inno Setup switches:

```bat
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER
glossa-1.2.3-x86_64-windows-setup.exe /VERYSILENT /ALLUSERS /TASKS="desktopicon,autostart"
"%LOCALAPPDATA%\Programs\Glossa\unins000.exe" /VERYSILENT
```

A silent upgrade keeps "start with Windows" as it is, and starts Glossa
again in the tray if it was running. A silent uninstall keeps the settings.

### The portable archive

Unpack the `.zip` anywhere and run `glossa.exe`. Settings go to
`%APPDATA%\Glossa`, as with the installer; put an empty file named
`portable` next to `glossa.exe` to keep them in a `data` folder beside it
instead (a USB stick, say). The installer never does that.

### winget and Scoop

Each release carries manifests for both, with that release's URLs and
checksums. They become usable only once the repository is public and the
release is published; until then the download links behind them do not
answer. After that:

- **Scoop**: `scoop install https://github.com/bropines/glossa/releases/download/v1.2.3/glossa.json`
  (or add the manifest to a bucket). It installs the portable archive and
  puts `glossa-cli` on the `PATH`.
- **winget**: the three `Bropines.Glossa*.yaml` files go in a pull request to
  [winget-pkgs](https://github.com/microsoft/winget-pkgs) (under
  `manifests/b/Bropines/Glossa/1.2.3/`); once merged,
  `winget install Bropines.Glossa`. Before that, `winget install --manifest
  <folder with the three files>` tries them locally.

## macOS

Open the `.dmg` and drag `Glossa.app` onto *Applications*. Glossa is a
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

The `.zip` holds the same app plus `glossa-cli` and the documents.

## Linux

### Debian and Ubuntu

```bash
sudo apt install ./glossa_1.2.3_amd64.deb
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

Unpack the `.tar.gz` anywhere and run `./glossa`; `./install.sh` adds it to
your application menu. It needs the same libraries as the AppImage.

## Where Glossa keeps your files

| | Windows | macOS | Linux |
|---|---|---|---|
| settings, history, modules | `%APPDATA%\Glossa` | `~/Library/Application Support/Glossa` | `~/.config/glossa` |
| cache and log | `%LOCALAPPDATA%\Glossa` | `~/Library/Caches/Glossa` | `~/.cache/glossa` |

A `portable` file or a `data` folder next to the executable, or the
`GLOSSA_HOME` variable, moves both there; see
[Configuration](configuration.md).

## Building the packages

The release workflow (`.github/workflows/release.yml`) builds everything on
a version tag. The scripts it calls work locally too:

| script | makes | needs |
|---|---|---|
| `packaging/linux/build-deb.sh <bin-dir> <version> [out]` | `glossa_<version>_<arch>.deb` | `dpkg-deb`, `dpkg-shlibdeps` (dpkg-dev) |
| `packaging/linux/build-appimage.sh <bin-dir> <version> [out]` | `glossa-<version>-x86_64.AppImage` | [appimagetool](https://github.com/AppImage/appimagetool) (`APPIMAGETOOL`, `APPIMAGE_RUNTIME`) |
| `iscc /DMyAppVersion=<version> /DStageDir=<unpacked zip> packaging\windows\glossa.iss` | the Windows installer | Inno Setup 6.3+ |
| `packaging/fill-manifests.sh <version> <dist>` | winget and Scoop manifests | the setup `.exe` and `.zip` in `<dist>` |

`<bin-dir>` holds `glossa` and `glossa-cli`, e.g.
`target/x86_64-unknown-linux-gnu/release`. Build the `.deb` on the oldest
system it should install on: its dependencies are read off that system's
libraries. The macOS disk images are made by `hdiutil` in the workflow.
