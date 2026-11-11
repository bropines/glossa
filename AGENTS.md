# Working on Glossa

Instructions for anyone, human or agent, changing this repository. Read
[docs/architecture.md](docs/architecture.md) for the map; this file is the
rules.

## Build and test in WSL, never on the Windows host

The Windows machine this lives on has no usable MSVC toolchain, and nothing
should be installed there for building. All cargo work runs in WSL (Debian)
through `scripts/wsl-cargo.sh`, which puts the target directory on the Linux
filesystem (`$CARGO_TARGET_DIR`, default `~/.cache/glossa-target`):

```bash
# from Windows (PowerShell / Git Bash); from a worktree, cd to the worktree instead:
wsl.exe -d Debian -e bash -lc 'cd /mnt/f/bropi/Projects/ocr-translate/glossa && ./scripts/wsl-cargo.sh test --workspace'

# inside WSL:
./scripts/wsl-cargo.sh test --workspace
./scripts/wsl-cargo.sh clippy --workspace --all-targets -- -D warnings                                    # Linux code
./scripts/wsl-cargo.sh clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings     # cfg(windows) code
./scripts/wsl-cargo.sh fmt --all -- --check
./scripts/wsl-cargo.sh build --release -p glossa-ui-egui -p glossa-cli                                    # Linux
./scripts/wsl-cargo.sh build --release -p glossa-ui-egui -p glossa-cli --target x86_64-pc-windows-gnu     # Windows
```

Built binaries land in `$CARGO_TARGET_DIR/{release,x86_64-pc-windows-gnu/release}/`.
Copy them to `dist/windows/` or `dist/linux/` (ignored by git) to run them.
Stop a running `glossa.exe` first: Windows locks the file.

**One shared target directory.** The WSL disk image lives on a nearly full
drive; per-agent target directories once filled it and the distribution
went read-only. Use `~/.cache/glossa-target`, or the single extra directory
you were told to use for a parallel session (`CARGO_TARGET_DIR=...`). Never
create a new one per task, and delete an extra one when its work is done.
Do not install anything and do not use `sudo` in WSL.

`cfg(windows)` code (UI Automation, the clipboard, synthetic input, WinRT)
is compiled and linted here through the `x86_64-pc-windows-gnu` target; CI's
Windows job runs its tests. macOS code is built and tested on the author's
Mac or in CI.

A faster debug app and a smaller target directory, for your own machine
only: put this in `~/.cargo/config.toml` (not the repository, where it
would slow every CI cache miss). Measured on a Mac, the workspace tests ran
in 45 s instead of 56 and the target directory was 3.6 GB instead of 4.5, for
a clean build of 118 s instead of 65.

```toml
[profile.dev.package."*"]
opt-level = 1
debug = "line-tables-only"
```

`cargo test` does not rebuild a binary it does not test: after changing
`glossa-cli` or `glossa-ui-egui`, build them explicitly before trying them.

## Where things are

[docs/architecture.md](docs/architecture.md) is the map. In short:

| path | what |
|---|---|
| `crates/glossa-core` | types, `config.rs` (every setting), `paths.rs` (data folders), `locale.rs` (English strings), `secrets.rs` |
| `crates/glossa-core/locales/ru.json` | the built-in Russian pack |
| `crates/glossa-engine` | the pipeline, history, control API, D-Bus, bundled modules (`src/bundled/`) |
| `crates/glossa-services` | built-in services and the system voice (`system_tts/`) |
| `crates/glossa-ocr`, `crates/glossa-lens` | OCR engines; the built-in Google Lens client |
| `crates/glossa-modules`, `crates/glossa-module-sdk` | module runtimes and registry; the wire types and the native ABI |
| `crates/glossa-module-test` | the module test harness (`glossa-cli module test`); fixtures live in each module's `tests/` |
| `crates/glossa-platform` | everything OS-specific ([its AGENTS.md](crates/glossa-platform/AGENTS.md)) |
| `crates/glossa-ui-egui` | the desktop app ([its AGENTS.md](crates/glossa-ui-egui/AGENTS.md)) |
| `crates/glossa-cli` | `glossa-cli`, and `doctor.rs` |
| `modules/`, `sdk/` | example and bundled modules; the Python and Java SDKs ([modules/AGENTS.md](modules/AGENTS.md), [sdk/AGENTS.md](sdk/AGENTS.md)) |
| `packaging/`, `.github/` | installers, packages, CI and the release ([packaging/AGENTS.md](packaging/AGENTS.md), [.github/AGENTS.md](.github/AGENTS.md)) |
| `docs/` | user and module-author documentation |

## Code per operating system

- OS calls live in `glossa-platform`, one file per system (`foo/win.rs`,
  `foo/mac.rs`, `foo/linux.rs`) behind a common function that compiles on
  every OS and fails politely (`Err(Error::Platform(..))` or `None`).
  Code in crates below `glossa-platform` follows the same layout where it
  is (see *Exceptions* below).
- How the window systems differ (own title bar, points or pixels, Cmd or
  Ctrl, a menu bar…) is answered by `glossa_platform::shell::SHELL`, one
  `shell/{win,mac,linux}.rs` per system. UI code asks `SHELL` instead of
  testing `cfg(target_os)`.
- "Linux" in the code is `not(any(windows, target_os = "macos"))`, so the
  BSDs come along. Linux has not been tested on real hardware yet; say so
  rather than claim it works.
- **Exceptions.** `glossa-platform` depends on `glossa-core` and is linked
  only by the two front ends (`glossa-cli`, `glossa-ui-egui`); the crates
  below them cannot use it. Pulling it into them would add its audio,
  clipboard, hotkey and capture crates to the headless engine (and ALSA
  headers to its Linux build). So these stay where they are, with the same
  layout, one file per system behind functions that compile everywhere:
  - `glossa-core/src/fsio.rs` with `fsio/{unix,other}.rs`: private file
    permissions and durable writes. Core is below platform.
  - `glossa-modules/src/proc_tree/{mod,unix,win}.rs`: a module's process
    in its own process group or job object, so it is stopped with its
    children.
  - The OS OCR engines in `glossa-ocr` and the system voice in
    `glossa-services/src/system_tts`.

## Trying the desktop app on Windows

Use a throwaway data folder so the real settings stay untouched:

```powershell
$env:GLOSSA_HOME = "$env:LOCALAPPDATA\Temp\glossa-win-test"
$env:GLOSSA_LOG = "glossa=debug,glossa_platform=debug,glossa_engine=debug"
Start-Process dist\windows\glossa.exe -ArgumentList '--hidden'
Get-Content "$env:GLOSSA_HOME\cache\glossa.log" -Tail 30
```

- `--hidden` starts in the tray; `--popup TEXT` opens a popup;
  `--screenshot WHAT` opens one window (options, dictionary, history…) for a
  screenshot.
- Set `[appearance] unload_graphics_after_secs` low (8) in the test
  config to exercise the unload cycle quickly.
- QTranslate itself runs elevated on this machine; Windows blocks synthetic
  input from an unelevated process into it. Do not try to elevate.
- Never close or type into windows you did not open. Send keystrokes only to
  a window you just focused with a click, not via Alt tricks (an Alt press
  puts apps like Notepad into key-tip mode and the next keys edit the
  document).

## Rules

**Clean room.** No code, strings or assets from the QTranslate executable,
and no decompiling or disassembling it. What may be studied: the plain-text
files users already edit (`Options.json`, `Service.js`, `Common.js`, themes,
`lang.json`) and the running program's visible behaviour. Record what was
learned about behaviour in `docs/research/`. Classic scripts and themes are
never copied into this repository or the builds; they are read from the
user's installation.

**Secrets.** Never commit an API key, token or password, including ones found
in a user's files (a classic `Service.js` may contain a hard-coded key).
Tests use obvious fakes.

**Size and memory are features.** Before adding a dependency, check what it
pulls in (`cargo tree -e normal -i <crate>`) and that its licence passes
`python3 scripts/third-party-notices.py --check` (CI runs it; a release
ships the notices that script writes, see
[packaging/AGENTS.md](packaging/AGENTS.md#third-party-notices)). Load heavy things lazily (the
per-script fonts and Python module processes are examples). The resident
cost in the tray is measured in single megabytes; keep it there.

**The UI never blocks.** Anything touching the network, another program or
the disk beyond a config file runs on a worker thread and reports back
through a channel. Selection capture for hotkeys happens before the GUI
loads (see `actions::read_selection`).

**Everything below the UI.** A feature the app has should be reachable from
`glossa-engine` (and so from `glossa-cli` and the control API). The UI crate
draws and dispatches; it does not own logic another front end would need.

**Platform code stays in `glossa-platform`**, behind functions that compile
(and fail politely) on every OS ([above](#code-per-operating-system)),
except in the crates below it that cannot depend on it (the exceptions
listed there).

**No emoji or symbol glyphs in the UI.** Most fonts lack them and they
render as boxes. Draw icons with `icons::paint`, or use words.

**Locale strings.** User-facing text goes through `app.t("key")` with an
English default in `glossa-core::locale` (`STRINGS`) and the Russian text in
`crates/glossa-core/locales/ru.json`; tests fail when either lacks a key the
UI uses. Classic `lang.json` files map onto the same keys by their numeric
ids.

**Config compatibility.** `config.toml` is user-editable and must keep
loading: new fields get `#[serde(default)]`, renamed ones get an alias.

**Module protocol.** Changing `glossa-module-sdk::wire` changes a public
contract: only add optional fields; anything else bumps `PROTOCOL_VERSION`
and needs the docs in `docs/modules/` updated in the same change.

**Docs follow the code.** A new setting goes into `docs/configuration.md`, a
new flag into `docs/cli.md`, and anything a user would notice into
`CHANGELOG.md` under *Unreleased*, in the same change.

## Style

- Comments explain why, not what, in full sentences. Match the density of
  the surrounding code: module-level `//!` docs say what a file is for.
- British spelling in prose and identifiers already using it (`recognise`,
  `colour`); keep what the surrounding code uses.
- Errors carry enough context to act on (`glossa_core::Error` kinds map onto
  user messages in the UI's `describe`).
- Tests sit next to the code. Anything network-facing is tested with
  `glossa_net::mock::MockHttp`; live tests are `#[ignore]`.
- `cargo test --workspace` stays green, and clippy clean for Linux and the
  Windows target, before a commit.

## Vendored code

`third_party/eframe` is eframe 0.37.0 with one fix, wired in through
`[patch.crates-io]`; see `third_party/README.md`. When upgrading eframe,
check whether upstream fixed it; if not, carry the patch forward (it is
marked `Glossa patch`).

## Git

Commit locally in focused commits with a message saying what changed and
why. Do not push, publish or open pull requests unless asked. End commit
messages made by an agent with a `Co-Authored-By:` line for that agent.
