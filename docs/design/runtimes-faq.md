# Module runtimes: plain answers

Two questions come up whenever modules are discussed: is Python built into
Glossa, and is JavaScript? Short answers first, details after. The longer
design discussion is in [declarative-modules.md](declarative-modules.md).

| | embedded in `glossa.exe`? | what the user needs | idle cost |
|---|---|---|---|
| JavaScript | **yes**, the boa engine, in-process | nothing | none (a fresh context per call) |
| Python | **no**, a child process | Python 3.8+ | none (the process stops when idle) |
| Java | no, a child process | Java 17+ | none (same) |
| process | no, any program over stdio | that program's runtime | none (same) |
| native | loaded into Glossa as a shared library | nothing | the library, once loaded |

## Is Python embedded?

No. A Python module runs as a separate `python` process that Glossa starts
on the module's first call and stops after `idle_timeout_secs` without calls
(300 by default). The two talk JSON lines over stdin and stdout, the same
transport Java and `runtime = "process"` modules use
(`crates/glossa-modules/src/runtime/stdio.rs`).

How Glossa finds Python (`runtime/python.rs`):

- `[modules] python` in `config.toml`, if set: a path, or a command line
  such as `py -3`.
- Otherwise the first of `python3`, `python` (and on Windows `py -3`, then
  the same names through `cmd /c` so pyenv-win and conda `.bat` shims
  work) that answers a version probe. The probe reads `sys.executable`, so
  the Windows Store's fake `python.exe` alias is skipped.
- The SDK (`sdk/python/glossa_module.py`) is put on `PYTHONPATH`; a copy
  inside the module's folder wins.

So the user must have Python 3.8 or newer installed, plus whatever packages
the module imports.

### What embedding would take

Three ways, none free:

**1. PyO3 linked against libpython.** Glossa would host CPython inside its
own process.

- Linking against the user's installed Python saves nothing over today: the
  user still needs Python, and now it must also be the exact version (or
  the stable ABI) Glossa was built for.
- Bundling libpython and its standard library adds tens of MB (the shared
  library alone is several MB, the standard library much more) to a program
  that is about 20 MB today.
- CPython cannot be reliably unloaded once started (`Py_Finalize` does not
  free everything, and many C extensions do not support re-initialisation),
  so the first Python call would leave its memory resident for the life of
  the process. That breaks the "few MB in the tray" rule.
- A crash in a C extension (NumPy, ONNX Runtime, Torch) would take Glossa
  down instead of failing one call.
- The packages people actually want from Python (ML models, OCR engines)
  still have to be installed with pip.

Verdict: no.

**2. RustPython.** A Python interpreter written in Rust, linked in.

- Adds several MB to the binary, more with its standard library frozen in.
- Much slower than CPython.
- Cannot load C extensions, which removes the main reason to write a
  module in Python at all: the libraries.

Verdict: no. For "a scripting language with nothing to install", JavaScript
already covers it.

**3. A lazily downloaded python-build-standalone.** Keep the child process,
but let Glossa fetch a relocatable CPython (the
[python-build-standalone](https://github.com/astral-sh/python-build-standalone)
builds that uv uses) the first time a Python module needs one and the user
has none.

- The binary does not grow. The download is tens of MB, once, into the
  data folder, and only for users who install a Python module.
- The archive's SHA-256 is pinned in Glossa (or in the module's manifest),
  using the hash-checked download code the module-assets work adds.
- Nothing about the transport changes; `[modules] python` would simply
  point at the downloaded interpreter.
- Packages remain the open part: a module's `requirements.txt` would need
  installing into a per-module virtual environment, ideally with hashes
  (`pip install --require-hashes`), with the user asked first.

Verdict: the right path if "the user has no Python" becomes a real
complaint. It is not part of the current plan.

## Is JavaScript embedded?

Yes. JavaScript modules run inside Glossa on [boa](https://boajs.dev)
(`boa_engine` 0.22, `crates/glossa-modules/src/runtime/js/`). No Node.js, no
browser engine, nothing for the user to install. The same engine runs
classic QTranslate `Service.js` scripts and pot-app plugins.

How it behaves:

- **A fresh context per call.** The script is evaluated, the entry
  function called, the result read and the context dropped. Nothing stays
  resident between calls, so installed JavaScript modules cost no memory
  while idle, and a script that breaks its globals cannot break the next
  call. Parsing a module's script takes a few milliseconds.
- **The host is synchronous.** `host.fetch()` returns the response directly;
  the call itself runs on a worker thread, so the UI never waits on it.
- **Sandboxed by construction.** The engine has no file, process or socket
  access of its own. The only way out is the host API (`host.fetch`,
  `host.cache`, `host.log`, `host.image`), and `host.fetch` only reaches the
  hosts in the module's `[permissions] network`.

Limits to know:

| limit | detail |
|---|---|
| one file | the module is one script (`entry`, default `main.js`); there is no `import` or `require`. A bundler (esbuild, Rollup) can flatten several files into one if the result uses no Node or browser APIs. |
| no npm runtime | packages that need Node built-ins (`fs`, `http`, `Buffer`) do not work; pure-JavaScript ones bundled in usually do. |
| no event loop between calls | `setTimeout` runs its callback on the next microtask and does not wait; `async`/`await` work, and Glossa waits for the returned promise. |
| no `Intl` | boa's `intl` feature is off (it would pull in ICU data), so `Intl.DateTimeFormat`, `toLocaleString` with locales and similar are missing. |
| CPU | 50 million loop iterations and a recursion depth of 400 per call, then the call fails; Glossa's module timeout also applies. |
| memory | there is no per-call heap limit; a script that allocates without bound can grow the process until the call times out. |
| speed | boa is an interpreter, several times slower than QuickJS and far slower than V8 (measured in [declarative-modules.md](declarative-modules.md#measurements)). For "a few requests and some JSON" this does not matter; for heavy text processing a process module in a faster language is better. |
| language level | modern ECMAScript: `let`/`const`, classes, destructuring, template literals, `async`/`await`, `Map`/`Set`, Unicode regular expressions. boa passes most, not all, of the test262 conformance suite; the rare gap shows up as a syntax or type error at load. |

The full guide is [../modules/js-modules.md](../modules/js-modules.md).
