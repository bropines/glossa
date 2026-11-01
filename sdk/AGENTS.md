# sdk/

The module SDKs for Python (`python/glossa_module.py`) and Java
(`java/`). The root [AGENTS.md](../AGENTS.md) applies. What the SDKs offer
module authors is in [python/README.md](python/README.md) and
[java/README.md](java/README.md); the protocol they implement is
[docs/modules/protocol.md](../docs/modules/protocol.md), framing in
[docs/modules/process-modules.md](../docs/modules/process-modules.md#framing).

## Rules

- The SDKs speak the module protocol, a public contract
  (`glossa-module-sdk::wire`, `PROTOCOL_VERSION`). Add only optional
  fields; a change in the Rust wire types lands in both SDKs, their READMEs
  and `docs/modules/` in the same commit.
- No dependencies. The Python SDK is one file on the standard library,
  Python 3.8 or newer; the Java SDK uses nothing beyond Java 17.
- Python: stdout is the protocol channel. The SDK moves file descriptor 1
  to stderr at start so `print()` and C extensions land in Glossa's log;
  keep it that way. Anything written to the real stdout other than a frame
  breaks the module.
- Glossa ships `glossa_module.py` beside the executable (`sdk/python/`, the
  app bundle's `Resources/sdk/python`, `<prefix>/share/glossa/sdk/python`)
  and adds it to `PYTHONPATH` (`GLOSSA_PYTHON_SDK` names another folder);
  a copy vendored in a module's folder wins.
  See `sdk_dirs()` in `crates/glossa-modules/src/runtime/python.rs`.

## Testing

- Python: the tests in `crates/glossa-modules/src/runtime/python.rs` run
  real modules against this SDK (they skip without a Python 3 interpreter):
  `../scripts/wsl-cargo.sh test -p glossa-modules python`.
- Java: `java/build.sh test` (or `build.ps1`) builds the jar and runs
  `SdkTest`; `crates/glossa-modules/tests/process_modules.rs` runs the
  `modules/java/example-upper` example when a JDK is found.
