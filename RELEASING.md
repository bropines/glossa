# Releasing

The checklist for a release of Glossa, and for the module programs that
are released on their own. It gathers what
[.github/AGENTS.md](.github/AGENTS.md) and
[packaging/AGENTS.md](packaging/AGENTS.md) say about the workflows and the
packaging scripts; those two stay the reference for how they work.

Nothing is published automatically. A tag builds everything and makes a
**draft** release; publishing is a click by the maintainer.

- [Tags](#tags)
- [An application release](#an-application-release)
- [A module program](#a-module-program)
- [If something goes wrong](#if-something-goes-wrong)

## Tags

One repository, three kinds of release, told apart by the tag:

| tag | workflow | builds |
|---|---|---|
| `v<version>` (`v0.1.0-beta.3`, `v0.1.0`) | `release.yml` | the application: installers, portable archives and packages for every system and processor, `SHA256SUMS`, winget and Scoop manifests |
| `paddle-ocr-v<version>` | `paddle-ocr.yml` | the PaddleOCR module's program, five platforms |
| `local-llm-v<version>` | `local-llm.yml` | the Local LLM module's program, five platforms |

The workflows do not trigger one another: a module tag does not build the
application, and `v*` does not build the module programs.

All of them land in the same GitHub release list. Only an application
release may be marked **latest**; see step 9 below.

## An application release

Versions follow [Semantic Versioning](https://semver.org/). A version with
a `-` in it (`0.1.0-beta.3`) becomes a pre-release.

### Before the tag

1. **`main` is green.** CI passes, and locally, as
   [AGENTS.md](AGENTS.md#build-and-test-in-wsl-never-on-the-windows-host)
   describes: `cargo test --workspace`, clippy with `-D warnings` for
   Linux and for the Windows target, `cargo fmt --all -- --check`.
2. **Bump the version** in one place: `version` under
   `[workspace.package]` in the root `Cargo.toml`. Then let cargo update
   `Cargo.lock` (any build does) and commit both: the release builds run
   with `--locked` and fail on a stale lock file.

   Nothing else holds the application's version by hand. The workflow
   derives the rest from it: the installer's version, the macOS
   `Info.plist` (`@VERSION@`, the numeric part only), the package names,
   the manifests.
3. **Places that name a version in prose.** Search for the old one and fix
   what is meant to move:
   - the *Beta* note near the top of `README.md`;
   - `min_glossa` in a module's `module.toml`, only where the module now
     needs something this release adds;
   - `docs/install.md`, if a download size or a supported system changed.
4. **`CHANGELOG.md`**: rename `## [Unreleased]` to
   `## [<version>] - <YYYY-MM-DD>` and put a new, empty `## [Unreleased]`
   above it. The workflow reads the section by its `[<version>]` heading.
   Read the section through once: entries written months apart contradict
   each other ("not released yet" next to "released").
5. **Release notes**: write `packaging/release-notes/<version>.md`, a
   short page (what it is, what is new, a download table with this
   version's links, first launch, known issues, a link to the changelog).
   Copy the previous one and change it. Without this file the notes are
   the whole changelog section plus a fixed *Downloads* list, which is too
   long for a big release.
6. **Documentation** matches the code: a new setting is in
   `docs/configuration.md`, a new flag in `docs/cli.md`, new pages are in
   `docs/README.md` and `docs/README.ru.md`. If a download's file name
   changed, it changed in `release.yml`, `docs/install.md`,
   `packaging/fill-manifests.sh` and the release notes at once.
7. **Dry run** (optional, worth it after touching a workflow or a
   packaging script): *Actions → release → Run workflow* on `main` builds
   everything and stops at the artifacts, without a release. Run
   `actionlint` on a workflow you changed before pushing it.

### The tag

8. Tag the commit that has the version bump, and push the tag:

   ```bash
   git tag v0.1.0-beta.3
   git push origin v0.1.0-beta.3
   ```

   The tag must be `v` plus exactly the workspace version, or the workflow
   stops at its first job. It then builds Windows (x86_64, arm64), macOS
   (arm64, x86_64) and Linux (x86_64, arm64), smoke-tests what it can
   (the CLI, under qemu for arm64; the `.deb`; the Windows installer),
   fills the winget and Scoop manifests, writes `SHA256SUMS` and creates
   the draft.

### After the build

9. **Check the draft** on the releases page:
   - 19 files: for each of `x86_64` and `arm64` a Windows `-setup.exe`
     and `-portable.zip`, a macOS `.dmg` and `.zip`, a Linux
     `-portable.tar.gz`; `glossa_<version>_amd64.deb` and `_arm64.deb`;
     `-x86_64.AppImage` and `-aarch64.AppImage`; `SHA256SUMS`; three
     `Bropines.Glossa*.yaml`; `glossa.json`;
   - the notes read well and every download link in them resolves to a
     file of this release;
   - *Set as a pre-release* is ticked for a version with a `-`.
10. **Try what was built**, at least on the systems at hand:
    - Windows: run the installer over the previous version (settings
      stay, autostart stays as it was), start Glossa, translate a
      selection, recognise a screen region; then the portable archive in
      an empty folder;
    - macOS: open the `.dmg`, replace the app, pass Gatekeeper, grant the
      permissions again, translate and recognise;
    - `glossa-cli --version` shows the version and the commit;
      `glossa-cli doctor` runs.

    Linux has no real-hardware test yet; the workflow's smoke tests are
    all there is. Say so in the notes, as the previous releases do.
11. **Publish** the draft. Tick *Set as the latest release* for a stable
    version. Signing: Windows files are signed only if the
    `WINDOWS_SIGN_PFX` and `WINDOWS_SIGN_PASSWORD` secrets exist; macOS
    builds are signed ad hoc and not notarised. The notes and
    `docs/install.md` must keep saying which of these is true.
12. **After publishing**:
    - download one file through the public link and compare it with
      `SHA256SUMS`;
    - the release badge in the README shows the new version (it filters
      on `v*` tags);
    - the Scoop manifest installs:
      `scoop install https://github.com/bropines/glossa/releases/download/v<version>/glossa.json`;
    - winget: the manifests are not submitted to
      [winget-pkgs](https://github.com/microsoft/winget-pkgs) yet. If and
      when they are, the three files go into a pull request under
      `manifests/b/Bropines/Glossa/<version>/`, and `docs/install.md`
      changes to say `winget install Bropines.Glossa` works;
    - close or update the issues the release fixes.

## A module program

The PaddleOCR and Local LLM modules run a small Rust program of their
own, built outside the workspace and downloaded by the module as a file.
Each module's README has its steps under *Releasing the binary*
([PaddleOCR](modules/process/paddle-ocr/README.md#releasing-the-binary),
[Local LLM](modules/process/local-llm/README.md#releasing-the-binary)).
In outline:

1. Set the version in the module's `Cargo.toml` and in `module.toml`
   (`[module] version`), and point the `bin` URLs in `module.toml` at
   `…/releases/download/<module>-v<version>/…`. Commit.
2. Tag and push `paddle-ocr-v<version>` or `local-llm-v<version>`.
3. The workflow tests, builds and packs five platforms (macOS arm64,
   Linux x64 and arm64, Windows x64 and arm64) and creates a **draft**
   release with the archives, `SHA256SUMS` and `SIZES`.
4. Copy each archive's `sha256` and `size` from `SIZES` into its `bin`
   entry in `module.toml`. An all-zero placeholder hash is refused by
   Glossa, so the module cannot load until this is done. For a module's
   first release, also drop its exclusions: the module list in
   `.github/workflows/ci.yml`, and `UNRELEASED` in
   `crates/glossa-module-test/tests/repo_modules.rs` (that test fails
   until you do). Commit.
5. Publish the draft, and **untick *Set as the latest release***: a
   module release marked latest takes over the repository's "latest
   release" link, which should always lead to the application.
6. Check from a clean data folder that the module downloads and runs:
   `glossa-cli assets download <id>`, then one call.

A new ONNX Runtime version for PaddleOCR needs all five pinned hashes in
`paddle-ocr.yml` updated. llama.cpp and the models are assets from other
projects' releases, pinned by URL and SHA-256 in `module.toml`; changing
one is an ordinary commit, not a release.

## If something goes wrong

- **The workflow failed part-way.** Fix it, delete the draft release if
  one was created, delete and re-push the tag on the fixed commit
  (`git tag -d`, `git push origin :refs/tags/<tag>`), and run again. Do
  not move a tag whose release was already published.
- **A published release is broken.** Do not replace its files: their
  checksums are public and package manifests point at them. Mark it as a
  pre-release or delete it, say so in its notes, and release the next
  version.
- **A secret got into a build log or a file.** Revoke it first, then
  clean up. Certificates, keys and passwords never go into the
  repository or a workflow file.
