# .github/

CI, the release workflow and the issue templates. The root
[AGENTS.md](../AGENTS.md) applies. Run `actionlint` on a workflow before
pushing it; the release workflow can be tried without making a release by
running it by hand (*Actions → release → Run workflow*).

## ci.yml

On a push to `main` and on pull requests (changes to Markdown files only
are skipped): `cargo fmt --check`, clippy with `-D warnings` and the tests
on Linux (Ubuntu, with `libgtk-3-dev` and `libasound2-dev`), macOS and
Windows (MSVC), `cargo doc` with warnings as errors, `cargo machete` for
unused dependencies, and `cargo check` at the `rust-version` in
`Cargo.toml`.

The screenshot smoke test (`scripts/screenshot-smoke.sh`) follows the tests
on all three systems: every window `glossa --screenshot-list` names, in
Glossa Dark, Glossa Light (with the Russian interface) and macOS Light,
each from a throwaway data folder. It is offline: every service is
`scripts/smoke-module`, a JavaScript module with canned answers, and the
proxy points at a closed port. A crash, a hang, a blank picture or a
string with no text fails it; pixels are not compared, since fonts differ
between systems. The pictures are kept as the `screenshots-<os>` artifact
of the run, which is where to look at a change to the UI without a machine
of that system. Linux draws under Xvfb with Mesa's software OpenGL, Windows
with the software renderer (WARP, `GLOSSA_RENDERER=software`).
Live tests against Tesseract (Linux) and Apple Vision (macOS, allowed to
fail) run as extra steps. The Linux job also builds the native and Java
example modules and runs `glossa-cli module test --strict
--require-runtimes --download modules`, so every module in `modules/` passes
its fixtures with nothing skipped (`--download` fetches the managed Python
and packages a Python example declares). Locally
the same checks run in WSL ([AGENTS.md](../AGENTS.md#build-and-test-in-wsl-never-on-the-windows-host)).

## release.yml

- A tag `v<version>` that matches the workspace version in `Cargo.toml`
  builds every platform and processor, smoke-tests what it can (the CLI,
  under qemu for arm64; the `.deb`; the Windows installer) and creates a
  **draft** GitHub release, marked pre-release for a version with a `-`.
  Publishing is done by hand. Run by hand without a tag, it stops at the
  artifacts.
- The release notes are `packaging/release-notes/<version>.md` when it
  exists: a short page (what it is, a download table, first launch, known
  issues, a link to the CHANGELOG). Write one for every release; the
  CHANGELOG section of a big release is far too long to read there. Without
  it, the notes are that version's section of `CHANGELOG.md`
  (`## [<version>]`) with a fixed *Downloads* list from the workflow. Rename
  *Unreleased* to the version before tagging either way.
- The packaging scripts it calls are in `packaging/`
  ([packaging/AGENTS.md](../packaging/AGENTS.md)). File names are listed at
  the top of `release.yml`; they must match `docs/install.md`,
  `fill-manifests.sh` and the *Downloads* text.
- Windows signing runs only when the `WINDOWS_SIGN_PFX` secret is set.
  Never put a secret into a workflow file.

## Templates

`ISSUE_TEMPLATE/` and `PULL_REQUEST_TEMPLATE.md` are user-facing text:
plain English, no emoji.
