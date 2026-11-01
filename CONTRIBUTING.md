# Contributing to Glossa

Bug reports, fixes, new services and modules are welcome. The rules for
changing the code (clean room, no secrets, size, a UI that never blocks,
config compatibility, the module protocol) are in [AGENTS.md](AGENTS.md);
they apply to people and agents alike. [docs/architecture.md](docs/architecture.md)
is the map of the workspace.

## Building

You need Rust 1.95 or newer (`rust-version` in `Cargo.toml`). On Linux, the
GTK 3 and ALSA development packages too:

```bash
sudo apt install libgtk-3-dev libasound2-dev     # Debian, Ubuntu
sudo dnf install gtk3-devel alsa-lib-devel       # Fedora
```

Then, from the repository root:

```bash
cargo build --release -p glossa-ui-egui -p glossa-cli
cargo test --workspace
```

On Windows a plain `cargo build` with the MSVC toolchain works (CI builds
that way). The author's own machine is different: nothing is installed on
the Windows host for building, and everything runs in WSL (Debian) through
`scripts/wsl-cargo.sh`, cross-compiling Windows binaries with the
`x86_64-pc-windows-gnu` target. AGENTS.md has the commands; you only need
them if your setup is like that.

Before sending a change, run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Adding a translation service

Most services are best written as a **module**, which needs no change to
Glossa and no Rust: a folder with a `module.toml` and a script, in
JavaScript or Python, or a native library. Start with
[docs/modules/README.md](docs/modules/README.md) and the examples in
`modules/` (`modules/js/free-dictionary` is the smallest JavaScript one).
Put the folder in the `modules` directory of Glossa's data folder and check
it with `glossa-cli modules` and `glossa-cli translate -s <id> -t de "Hello"`.
Working with an AI assistant? [docs/modules/agents-template.md](docs/modules/agents-template.md)
is an `AGENTS.md` to put in your module's repository.

A service becomes **built in** (`crates/glossa-services`) when it is widely
used and worth having without any setup. Implement the `Provider` trait next
to the existing ones, add it to `builtin_providers()` in the crate root, and
test it against `glossa_net::mock::MockHttp` with recorded responses; a test
against the live service is `#[ignore]`d. Never commit a real key.

The module protocol is a public contract: only add optional fields to
`glossa-module-sdk::wire`. Anything else bumps `PROTOCOL_VERSION` and updates
`docs/modules/` in the same change.

## Interface text

Every string the interface shows goes through the locale: the English text
in `STRINGS` in `crates/glossa-core/src/locale.rs`, and the Russian one in
`crates/glossa-core/locales/ru.json`. The tests fail when the UI uses a key
that either file lacks. If you do not speak Russian, add the English text
there too and say so in the pull request.

## Testing on Linux

Linux builds pass the tests but have not been tried on real hardware by the
author yet. Reports from X11 and Wayland desktops (with
`glossa-cli doctor` output) are especially welcome.

## Commits and pull requests

Focused commits, each saying what changed and why. Subjects follow
[Conventional Commits](https://www.conventionalcommits.org/) with the crate or
area as the scope:

```
feat(ocr): watch a region and re-translate when it changes
fix(modules): find Python behind pyenv-win and other .bat/.cmd shims
docs(modules): the native ABI's memory rules
ci: test on Windows
chore: clippy clean on Linux and Windows targets
```

Add a line to `CHANGELOG.md` under *Unreleased* for anything a user would
notice. By contributing you agree that your work is licensed under the
project's terms, MIT or Apache-2.0 at the user's option.
