# modules/

Modules that live in this repository. The root [AGENTS.md](../AGENTS.md)
applies. How to write a module is in [docs/modules/](../docs/modules/README.md);
this file only says what is particular to the copies kept here. A ready
AGENTS.md for a module in its own repository is in
[docs/modules/agents-template.md](../docs/modules/agents-template.md).

## Two kinds

| folder | what | shipped as |
|---|---|---|
| `bundled/<id>/` | keyless services (Yandex, DeepL, Lara, Multitran…), JavaScript only | compiled into the executable with `include_str!` (`crates/glossa-engine/src/bundled/mod.rs`) |
| `js/`, `python/`, `java/`, `process/`, `native/` | examples, one per runtime | source in the release archives under `examples/modules/`; not loaded until copied into a module folder |

## Bundled modules

- Exactly two files are compiled in: `module.toml` and `main.js`. Anything
  else in the folder is ignored by the build and never reaches users.
- A new one needs its id added to `BUNDLED` in
  `crates/glossa-engine/src/bundled/mod.rs` (the folder is named after the
  id), tests in `bundled/tests.rs` against `glossa_net::mock::MockHttp`
  with recorded answers, and fixtures in `tests/basic.toml` (a case per
  capability and per error the module raises) for `glossa-cli module
  test`. The `tests/` folder is not compiled in.
- `every_bundled_module_loads_with_a_sane_manifest` enforces the rules:
  `runtime = "js"`, `entry = "main.js"`, a name, a description, an
  `https://` homepage, `max_input_chars`, `ready_to_use = true` with no
  required option, network permission for the service's own hosts only
  (no `"*"`), known language codes, an id no Rust built-in uses, and
  nothing in the script that looks like a key.
- List a new one in `docs/modules/README.md` (*Bundled modules*) and the
  README's feature list.
- Each has a `README.md` in the common layout (how it gets its answers,
  what leaves the machine, limits, content) and a `tests/live.toml` with
  live-only cases named `live: …` that check only the shape of a real
  answer ([testing.md](../docs/modules/testing.md#live-health-checks));
  `.github/workflows/modules-live.yml` runs those weekly.
- Every `host.fetch` passes a `timeout` under the call budget, and the
  entry points are wrapped in `guarded(...)` at the end of `main.js`, so
  an answer in an unexpected shape is a `bad_response` naming the service
  rather than a TypeError. Option labels are translated in
  `locales/ru.toml`, which only a copied module uses.

## Examples

- Keep them small and runnable; they are the templates module authors
  (and their AI agents) copy, listed in
  [docs/modules/testing.md](../docs/modules/testing.md#templates). A change
  to the protocol or the SDKs updates them in the same commit.
- Every example has fixtures in `tests/` that pass: a case per capability
  and per error it raises, with recorded HTTP answers and obvious fake
  keys. A new example comes with them; a changed one keeps them passing.
- `crates/glossa-modules/tests/process_modules.rs` loads `java/example-upper`
  and `process/`; the native runtime's tests build `native/example-rot13`.
- `native/example-rot13` and `native/example-lines` are workspace members
  (`Cargo.toml`), so `cargo test --workspace` and clippy cover them.
  `native/example-c` builds with a C compiler (commands at the top of `shout.c`).

## Checking a module

The harness (`crates/glossa-module-test`, behind `glossa-cli module test`)
checks a module's manifest and files, loads it through the registry, and
runs its fixtures offline. `cargo test -p glossa-module-test` runs it over
every module here (skipping those whose runtime is missing or that are not
built); CI's Linux job builds the native and Java examples and runs it with
nothing skipped.

```bash
glossa-cli module test --strict modules/js/free-dictionary   # one module
glossa-cli module test --strict modules                      # all of them
glossa-cli -v module test --case "unknown word" modules/bundled/multitran
./scripts/wsl-cargo.sh test -p glossa-module-test            # the same, as a test
./scripts/wsl-cargo.sh test -p glossa-engine bundled         # the bundled modules' Rust tests
GLOSSA_HOME=/tmp/gtest glossa-cli -v translate -s <id> -t de "Hello"   # in the engine, module copied to /tmp/gtest/modules
```

To test the native examples, build them (`cargo build -p
glossa-example-rot13 -p glossa-example-lines`, `cc` for `example-c`) and
copy the libraries beside their `module.toml`; for the Java example run
`build.sh`. The build output is ignored by git.

Files other than `module.toml`, the entry and what the module itself reads
(an `AGENTS.md`, a README) do not affect loading: the registry looks only
for `module.toml`, `Service.js` or `info.json` + `main.js` in each folder.
