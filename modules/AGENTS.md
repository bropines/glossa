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
  id) and tests in `bundled/tests.rs` against `glossa_net::mock::MockHttp`
  with recorded answers.
- `every_bundled_module_loads_with_a_sane_manifest` enforces the rules:
  `runtime = "js"`, `entry = "main.js"`, a name, a description, an
  `https://` homepage, `max_input_chars`, `ready_to_use = true` with no
  required option, network permission for the service's own hosts only
  (no `"*"`), known language codes, an id no Rust built-in uses, and
  nothing in the script that looks like a key.
- List a new one in `docs/modules/README.md` (*Bundled modules*) and the
  README's feature list.

## Examples

- Keep them small and runnable; they are what module authors copy. A
  change to the protocol or the SDKs updates them in the same commit.
- `crates/glossa-modules/tests/process_modules.rs` loads `java/example-upper`
  and `process/`; the native runtime's tests build `native/example-rot13`.
  The JavaScript and Python examples have no test of their own: try them
  with `glossa-cli` after a change.
- `native/example-rot13` and `native/example-lines` are workspace members
  (`Cargo.toml`), so `cargo test --workspace` and clippy cover them.
  `native/example-c` builds with a C compiler (commands at the top of `shout.c`).

## Checking a module

```bash
GLOSSA_HOME=/tmp/gtest glossa-cli modules                 # copy the module to /tmp/gtest/modules first
GLOSSA_HOME=/tmp/gtest glossa-cli -v translate -s <id> -t de "Hello"
./scripts/wsl-cargo.sh test -p glossa-engine bundled      # the bundled modules' tests
```

Files other than `module.toml`, the entry and what the module itself reads
(an `AGENTS.md`, a README) do not affect loading: the registry looks only
for `module.toml`, `Service.js` or `info.json` + `main.js` in each folder.
