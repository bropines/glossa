# Working on Glossa

Instructions for anyone, human or agent, changing this repository. Read
[docs/architecture.md](docs/architecture.md) for the map; this file is the
rules.

## Build and test in WSL, never on the Windows host

The Windows machine this lives on has no usable MSVC toolchain, and nothing
should be installed there for building. All cargo work runs in WSL (Debian),
with the target directory on the Linux filesystem:

```bash
# from Windows (PowerShell / Git Bash):
wsl.exe -d Debian -e bash -lc 'cd /mnt/f/bropi/Projects/ocr-translate/glossa && ./scripts/wsl-cargo.sh test --workspace'

# inside WSL:
./scripts/wsl-cargo.sh test --workspace
./scripts/wsl-cargo.sh clippy --workspace
./scripts/wsl-cargo.sh build --release -p glossa-ui-egui -p glossa-cli                                    # Linux
./scripts/wsl-cargo.sh build --release -p glossa-ui-egui -p glossa-cli --target x86_64-pc-windows-gnu     # Windows
```

Built binaries land in `~/.cache/glossa-target/{release,x86_64-pc-windows-gnu/release}/`.
Copy them to `dist/windows/` or `dist/linux/` (ignored by git) to run them.
Stop a running `glossa.exe` first: Windows locks the file.

`cargo test` does not rebuild a binary it does not test: after changing
`glossa-cli` or `glossa-ui-egui`, build them explicitly before trying them.

## Trying the desktop app on Windows

Use a throwaway data folder so the real settings stay untouched:

```powershell
$env:GLOSSA_HOME = "$env:LOCALAPPDATA\Temp\glossa-win-test"
$env:RUST_LOG = "glossa=debug,glossa_platform=debug,glossa_engine=debug"
Start-Process dist\windows\glossa.exe -ArgumentList '--hidden'
Get-Content "$env:GLOSSA_HOME\cache\glossa.log" -Tail 30
```

- `--hidden` starts in the tray; `--popup TEXT` opens a popup;
  `--screenshot WHAT` opens one window (options, dictionary, history…) for a
  screenshot.
- `[appearance] unload_graphics_after_secs` is set low (8) in the test
  config, to exercise the unload cycle quickly.
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
pulls in (`cargo tree -e normal -i <crate>`). Load heavy things lazily (the
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
(and fail politely) on every OS.

**No emoji or symbol glyphs in the UI.** Most fonts lack them and they
render as boxes. Draw icons with `icons::paint`, or use words.

**Locale strings.** User-facing text goes through `app.t("key")` with an
English default in `glossa-core::locale`; classic `lang.json` files map onto
the same keys by their numeric ids.

**Config compatibility.** `config.toml` is user-editable and must keep
loading: new fields get `#[serde(default)]`, renamed ones get an alias.

**Module protocol.** Changing `glossa-module-sdk::wire` changes a public
contract: only add optional fields; anything else bumps `PROTOCOL_VERSION`
and needs the docs in `docs/modules/` updated in the same change.

## Style

- Comments explain why, not what, in full sentences. Match the density of
  the surrounding code: module-level `//!` docs say what a file is for.
- British spelling in prose and identifiers already using it (`recognise`,
  `colour`); keep what the surrounding code uses.
- Errors carry enough context to act on (`glossa_core::Error` kinds map onto
  user messages in the UI's `describe`).
- Tests sit next to the code. Anything network-facing is tested with
  `glossa_net::mock::MockHttp`; live tests are `#[ignore]`.
- `cargo test --workspace` stays green and warning-free before a commit.

## Vendored code

`third_party/eframe` is eframe 0.37.0 with one fix, wired in through
`[patch.crates-io]`; see `third_party/README.md`. When upgrading eframe,
check whether upstream fixed it; if not, carry the patch forward (it is
marked `Glossa patch`).

## Git

Commit locally in focused commits with a message saying what changed and
why. Do not push, publish or open pull requests unless asked. End commit
messages made by an agent with a `Co-Authored-By:` line for that agent.
