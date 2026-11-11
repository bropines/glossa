# .github/

CI, the release workflow and the issue templates. The root
[AGENTS.md](../AGENTS.md) applies. Run `actionlint` on a workflow before
pushing it; the release workflow can be tried without making a release by
running it by hand (*Actions → release → Run workflow*).

## Every workflow

- Actions are pinned to a commit, with the release it is in a comment
  (`uses: actions/checkout@<sha> # v4.4.0`). A tag can be moved to other
  code; a commit cannot. Dependabot proposes the updates
  (`dependabot.yml`, weekly, one pull request for all actions). To pin a
  new one: `gh api repos/OWNER/REPO/git/ref/tags/TAG` (follow an annotated
  tag to its commit with `git/tags/<sha>`).
- The compiler is one named version, `RUST_TOOLCHAIN` at the top of
  `ci.yml` and `release.yml`: what CI tests is what a release is built
  with, and a new stable release cannot turn a pull request red by itself.
  Raise both together after trying the new version locally. There is no
  `rust-toolchain.toml` on purpose: it would make every checkout download
  that exact toolchain, on machines (the WSL disk, see the root AGENTS.md)
  that have a stable one already. The module workflows pin their own
  `rust-version`.
- `dependabot.yml` also proposes crate updates weekly (minor and patch
  ones in one pull request). The egui crates are left out: they move with
  the vendored `third_party/eframe`, by hand.

## ci.yml

On a push to `main` and on pull requests (changes to Markdown files only
are skipped): `cargo fmt --check`, clippy with `-D warnings` and the tests
on Linux (Ubuntu, with `libgtk-3-dev` and `libasound2-dev`), macOS and
Windows (MSVC), `cargo doc` with warnings as errors, `cargo machete` for
unused dependencies, the licence check, and `cargo check` at the
`rust-version` in `Cargo.toml`.

The licence check (`licences`) runs `scripts/third-party-notices.py
--check`: every package in the two programs, for each release target, must
be under a licence on the script's `ALLOWED` list and have a licence text
for the notices. A new dependency under a new licence fails it until
someone reads the licence and adds it to the list (or drops the
dependency); a crate that ships no licence file needs its text put into
`scripts/licenses/` (its README says how). Nothing is compiled.

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
--require-runtimes --download` over every module in `modules/`, so each
passes its fixtures with nothing skipped (`--download` fetches the managed
Python and packages a Python example declares; `paddle-ocr`'s recognisers
are downloaded first). The one exception is `modules/process/local-llm`,
left out of that list until its program is released: its `bin` asset's
sha256 is still the all-zero placeholder, which Glossa refuses, so it cannot
load. `repo_modules.rs` (`UNRELEASED`) asserts that it fails for exactly
that reason; filling in the real hashes makes that assertion fail, which
is the reminder to drop both exceptions (the module's README, "Releasing
the binary"). Locally
the same checks run in WSL ([AGENTS.md](../AGENTS.md#build-and-test-in-wsl-never-on-the-windows-host)).

## lint.yml

For what `ci.yml` skips, on changes to Markdown files or workflows:
`lychee --offline` over every `*.md` (links between the repository's own
files; web links and `#anchors` are not followed, so it cannot fail on
someone else's server) and `actionlint` over the workflows.

## advisories.yml

`cargo deny check advisories` (the prebuilt action, configured by
`deny.toml`) every Monday, on a change to a `Cargo.lock`, and by hand: the
workspace and the two module programs. It fails on a vulnerability or an
unsoundness advisory; "unmaintained" notices are off (the GTK 3 bindings of
the Linux tray icon carry one). An advisory that does not apply goes into
`ignore` in `deny.toml` with its reason.

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
- The `notices` job runs first: the licence check, then, per Rust target,
  `THIRD-PARTY-NOTICES-<target>.txt` and a CycloneDX SBOM
  (`glossa-<version>-<target>.cdx.json`), all from
  `scripts/third-party-notices.py`. The build jobs put their target's
  notices into every package as `THIRD-PARTY-NOTICES.txt`
  ([packaging/AGENTS.md](../packaging/AGENTS.md)); the SBOMs become release
  files.
- The draft release's files get a build provenance attestation
  (`actions/attest-build-provenance`; `gh attestation verify <file> --repo
  bropines/glossa`). GitHub offers that to public repositories, so the
  step is skipped while the repository is private.
- Before tagging, besides the CHANGELOG: add a `<release>` to
  `assets/linux/io.github.bropines.Glossa.metainfo.xml`.

## paddle-ocr.yml

Builds the PaddleOCR module's program (`modules/process/paddle-ocr`, its
own cargo project outside the workspace) and packs it with ONNX Runtime
and its crates' licence notices into the module's `bin` archives
(`package.sh`, `third_party.py`).

- A tag `paddle-ocr-v<version>` (the module's `Cargo.toml` version) or a
  run by hand builds all five platforms: macOS arm64, Linux x64 and arm64,
  Windows x64 and arm64. A tag also creates a **draft** release with the
  archives, `SHA256SUMS` and `SIZES`. Publishing it, and copying the hashes
  into `module.toml`, is done by hand.
- A pull request that touches the module or this workflow runs only the
  Linux x64 job (tests, clippy, build, archive, smoke test). The macOS,
  Windows and arm64 runners are slow or scarce, and the code they would
  build is the same, so they wait for a tag or a manual run.
- The toolchain is pinned to the module's `rust-version`, and each ONNX
  Runtime archive is checked against the SHA-256 pinned beside
  `ORT_VERSION`. A new ONNX Runtime version needs all five hashes updated.
- The main release workflow (tags `v*`) does not build this program.

## local-llm.yml

Builds the local LLM module's program (`modules/process/local-llm`, its
own cargo project outside the workspace) and packs it with its crates'
licence notices into the module's `bin` archives (`package.sh`,
`third_party.py`). llama.cpp and the models are separate assets from their
own releases; this workflow does not touch them.

- Tags `local-llm-v<version>`, runs by hand and pull requests work as for
  `paddle-ocr.yml`: five platforms and a **draft** release for a tag, the
  Linux x64 job alone (tests with the stand-in llama-server, clippy,
  build, archive, smoke test) for a pull request.
- The toolchain is pinned to the module's `rust-version`.
- The main release workflow (tags `v*`) does not build this program.

## Templates

`ISSUE_TEMPLATE/` and `PULL_REQUEST_TEMPLATE.md` are user-facing text:
plain English, no emoji.
