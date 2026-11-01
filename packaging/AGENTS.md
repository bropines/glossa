# packaging/

Scripts and templates that turn release binaries into installers and
packages. The root [AGENTS.md](../AGENTS.md) applies. They are run by
`.github/workflows/release.yml` ([.github/AGENTS.md](../.github/AGENTS.md));
what users download and how they install it is in
[docs/install.md](../docs/install.md).

| file | makes |
|---|---|
| `windows/glossa.iss` | the Windows installer (Inno Setup 6.3+), from the staged folder; `/DArch=x64` or `arm64` |
| `portable.txt` | the marker put into portable archives: its name (`portable.*`) makes Glossa keep its data in `data/` beside the executable |
| `linux/stage.sh` | the `usr/` tree a Linux package installs (binaries, menu entry, icon, docs, `share/glossa/sdk/python`, examples) |
| `linux/build-deb.sh` | `glossa_<version>_<arch>.deb` (amd64 or arm64) from that tree |
| `linux/build-appimage.sh` | `glossa-<version>-<arch>.AppImage` (x86_64 or aarch64) |
| `winget/`, `scoop/`, `fill-manifests.sh` | winget and Scoop manifests, filled with the release's URLs and checksums |

## Rules

- File names are a contract with users, `docs/install.md`, the release
  notes in `release.yml` and the manifests: change them everywhere at once.
- The installer must never produce a portable layout: `glossa.iss`
  excludes `portable`, `portable.*` and `data` from the staged folder.
  Autostart is the same `HKCU\...\Run` value Glossa's own option writes
  (`crates/glossa-platform/src/autostart.rs`).
- Examples never go into a folder Glossa scans for live modules
  (`<exe>/modules`, `share/glossa/modules`); they ship as
  `examples/modules/`.
- `glossa_module.py` ships where `sdk_dirs()` in
  `crates/glossa-modules/src/runtime/python.rs` looks for it.
- Linux packages are built on Debian 12 so they run on Debian 12, Ubuntu
  22.04 and newer. None of them has been tried on real hardware yet.
- Windows files are signed only when the release has signing secrets, and
  macOS builds are signed ad hoc. Never commit a certificate, key or
  password.

## Trying a script

The Linux scripts run in WSL on binaries built with
`scripts/wsl-cargo.sh build --release`; read each script's header for its
arguments and the tools it needs (`dpkg-deb`, `appimagetool`). Inno Setup
and the macOS packaging (inline in `release.yml`; `scripts/macos-bundle.sh`
for a local `Glossa.app`) need their own systems; do not install tools to
try them.
