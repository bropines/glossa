# packaging/

Scripts and templates that turn release binaries into installers and
packages. The root [AGENTS.md](../AGENTS.md) applies. They are run by
`.github/workflows/release.yml` ([.github/AGENTS.md](../.github/AGENTS.md));
what users download and how they install it is in
[docs/install.md](../docs/install.md).

| file | makes |
|---|---|
| `windows/glossa.iss` | the Windows installer (Inno Setup 6.3+), from the staged folder; `/DArch=x64` or `arm64` |
| `windows/version_resource.rs` | the version details of `glossa.exe` and `glossa-cli.exe`, shared by the two crates' `build.rs` |
| `portable.txt` | the marker put into portable archives: its name (`portable.*`) makes Glossa keep its data in `data/` beside the executable |
| `linux/stage.sh` | the `usr/` tree a Linux package installs (binaries, menu entry, icon, AppStream metadata, docs with the third-party notices, `share/glossa/sdk/python`, examples) |
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

## Third-party notices

Every package carries `THIRD-PARTY-NOTICES.txt`: each crate and bundled
component in that system's two programs, with its licence and the licence
texts its authors ship. Most of those licences (MIT, BSD, ISC, Apache-2.0,
MPL-2.0…) make the notice a condition of handing out the binaries, so a
package without the file is not a release.

- `scripts/third-party-notices.py --target <triple> -o <file>` writes it,
  from `cargo metadata` and the crates' sources in cargo's registry
  (Python 3.11+, nothing to install; `cargo fetch --locked` first on a
  fresh machine). `--check` is the licence check CI runs, `--sbom` writes
  the CycloneDX file, `--summary` the counts. Its header explains what
  counts as shipping; `COMPONENTS` in it lists what is not a crate (fonts,
  QuickJS, vendored and adapted code, the AppImage runtime) and must follow
  NOTICE.
- Where it goes, which is where `Paths::third_party_notices` (and so the
  About window's link) looks: beside the executables (portable archives,
  the Windows installer), `Glossa.app/Contents/Resources` (and the macOS
  zip's top folder), `usr/share/doc/glossa` (the `.deb`, the AppImage).
  The file name is `glossa_core::paths::THIRD_PARTY_NOTICES`.
- `linux/stage.sh` takes the file in `THIRD_PARTY_NOTICES` and refuses to
  stage without it (`ALLOW_NO_NOTICES=1` for a local try-out).
- Something new embedded in the programs that is not a crate (a font, a
  data file, copied code) gets an entry in `COMPONENTS` and in NOTICE, with
  its licence text in `scripts/licenses/` if no crate brings it. Do not
  guess an origin or a licence: find it, or leave the thing out.

## Versions and names

- The version in full (`0.1.0-beta.2`) goes wherever text is allowed: file
  names, `FileVersion` and `ProductVersion` strings, Inno Setup's
  `AppVersion`, `CFBundleShortVersionString`.
- Fields that take numbers only get them from one rule,
  `scripts/version-numbers.sh` (and the same in
  `windows/version_resource.rs`; a test in `glossa-cli` keeps the two
  equal): a build number of 1000+n for `-alpha.n`, 2000+n for `-beta.n`,
  3000+n for `-rc.n` and 9000 for the release, so a pre-release sorts
  before its release and after the one before it. Windows file versions
  are `major.minor.patch.build` (`0.1.0.2002`); `CFBundleVersion` is one
  growing number (`1002002`). Debian has its own spelling (`0.1.0~beta.2`,
  `build-deb.sh`). Never cut the suffix off: two betas then claim to be the
  same build.
- Two lines name who makes Glossa, and nothing else should: the publisher,
  `Bropines` (the installer's `AppPublisher`, the executables'
  `CompanyName`, winget's `Publisher`, which must match the installer's),
  and the copyright line, `Copyright (c) 2026 bropines and contributors`,
  as in LICENSE-MIT and NOTICE (`LegalCopyright`, `AppCopyright`,
  `NSHumanReadableCopyright`, the `.deb`'s `copyright`). A licence name is
  not a copyright line.

## Icons

The application icon is drawn by code (`app_icon` in
`crates/glossa-ui-egui/src/main.rs`, at any size); `assets/icons/glossa.png`
(256 px) and `glossa.ico` (16 to 256 px) are exports of it, written by the
ignored test `export_icon`. Nothing larger is in the repository, so the
release's `.icns` stops at 256 px (128@2x) and Linux gets the 256 px PNG
alone.

- TODO: have `export_icon` also write 512 and 1024 px PNGs (it renders at
  any size, so these are real pictures, not enlargements), build the
  `.icns` in `release.yml` from them (`icon_256x256@2x`, `icon_512x512`,
  `icon_512x512@2x`), and install 48, 64, 128 and 512 px PNGs under
  `usr/share/icons/hicolor/` in `linux/stage.sh` beside the 256 px one.
  Do not enlarge the 256 px PNG instead; `scripts/macos-bundle.sh` does
  that for the development bundle only, and it shows.

## Trying a script

The Linux scripts run in WSL on binaries built with
`scripts/wsl-cargo.sh build --release`; read each script's header for its
arguments and the tools it needs (`dpkg-deb`, `appimagetool`). Inno Setup
and the macOS packaging (inline in `release.yml`; `scripts/macos-bundle.sh`
for a local `Glossa.app`) need their own systems; do not install tools to
try them.
