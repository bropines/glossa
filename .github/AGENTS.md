# .github/

CI, the release workflow and the issue templates. The root
[AGENTS.md](../AGENTS.md) applies. Actions are switched off on the
repository for now, so a change here is not tried by pushing; read it twice.

## ci.yml

On a push to `main` and on pull requests (changes to Markdown files only
are skipped): `cargo fmt --check`, clippy with `-D warnings` and the tests
on Linux (Ubuntu, with `libgtk-3-dev` and `libasound2-dev`), macOS and
Windows (MSVC), and `cargo check` at the `rust-version` in `Cargo.toml`.
Live tests against Tesseract (Linux) and Apple Vision (macOS, allowed to
fail) run as extra steps. Locally
the same checks run in WSL ([AGENTS.md](../AGENTS.md#build-and-test-in-wsl-never-on-the-windows-host)).

## release.yml

- A tag `v<version>` that matches the workspace version in `Cargo.toml`
  builds every platform and processor, smoke-tests what it can (the CLI,
  under qemu for arm64; the `.deb`; the Windows installer) and creates a
  **draft** GitHub release, marked pre-release for a version with a `-`.
  Publishing is done by hand. Run by hand without a tag, it stops at the
  artifacts.
- The release notes are that version's section of `CHANGELOG.md`
  (`## [<version>]`), followed by a fixed *Downloads* list written in the
  workflow. Rename *Unreleased* to the version before tagging, or the notes
  fall back to GitHub's generated ones.
- The packaging scripts it calls are in `packaging/`
  ([packaging/AGENTS.md](../packaging/AGENTS.md)). File names are listed at
  the top of `release.yml`; they must match `docs/install.md`,
  `fill-manifests.sh` and the *Downloads* text.
- Windows signing runs only when the `WINDOWS_SIGN_PFX` secret is set.
  Never put a secret into a workflow file.

## Templates

`ISSUE_TEMPLATE/` and `PULL_REQUEST_TEMPLATE.md` are user-facing text:
plain English, no emoji.
