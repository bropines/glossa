# Native modules

A native module is a shared library — `.dll` on Windows, `.so` on Linux,
`.dylib` on macOS — that Glossa loads into its own process. It is the fastest
kind of module and the only kind that can link arbitrary native code (an
offline engine, a vendor SDK), and it is also the least forgiving: read the
[safety](#safety) section before shipping one.

The interface is a tiny C ABI that carries JSON. You can write a module in
Rust with the SDK in a dozen lines, or in C, C++, Zig, Go or anything else
with a C FFI.

## Contents

- [Quick start (Rust)](#quick-start-rust)
- [The manifest](#the-manifest)
- [The C ABI](#the-c-abi)
- [Ownership and threading](#ownership-and-threading)
- [Building for each platform](#building-for-each-platform)
- [Safety](#safety)
- [Why JSON over a C ABI, not the Rust ABI](#why-json-over-a-c-abi-not-the-rust-abi)
- [Troubleshooting](#troubleshooting)

## Quick start (Rust)

```toml
# Cargo.toml
[package]
name = "my-translator"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
glossa-module-sdk = { git = "https://github.com/bropines/glossa" }   # or a path
serde_json = "1"
```

```rust
// src/lib.rs
use glossa_module_sdk::wire::*;
use glossa_module_sdk::{export_module, Host, Module};

#[derive(Default)]
struct MyTranslator;

impl Module for MyTranslator {
    fn translate(&mut self, host: &Host, req: TranslateParams) -> Result<TranslateResult, ModuleError> {
        let key = req.options.get("api_key").and_then(|v| v.as_str())
            .ok_or_else(|| ModuleError::missing_option("api_key"))?;
        host.log(LogLevel::Debug, "calling the service");
        let resp = host.http(
            &HttpRequest::post_json("https://api.example.com/translate",
                &serde_json::json!({ "q": req.text, "source": req.from, "target": req.to }))
                .header("Authorization", format!("Bearer {key}")),
        )?;
        let body: serde_json::Value = serde_json::from_str(&resp.ok_text()?)
            .map_err(|e| ModuleError::bad_response(e.to_string()))?;
        Ok(TranslateResult::text(body["translation"].as_str().unwrap_or_default()))
    }
}

export_module!(MyTranslator);
```

`export_module!` generates the exported `glossa_module_v1` entry point, the
JSON plumbing and a panic guard. Implement only the methods your manifest
promises; the others answer "unsupported" by default. One instance of your
type lives while the library is loaded and calls are serialised, so `&mut
self` state needs no lock.

Build it with `cargo build --release` and put the library next to a
`module.toml`:

```
modules/my-translator/
├── module.toml
└── my_translator.dll        (libmy_translator.so / libmy_translator.dylib)
```

A complete, tested example lives in
[`modules/native/example-rot13`](../../modules/native/example-rot13).

### Building errors

Make a `ModuleError` with its constructors (`ModuleError::new`,
`missing_option`, `bad_response`, `.at(location)` for where it happened).
The protocol adds optional fields over time (`location` is the latest), so
a struct literal that names every field stops compiling when one is added.
End a literal with `..Default::default()` instead:

```rust
// Breaks whenever a field is added:
// ModuleError { kind: ErrorKind::Http, message: msg, retry_after_secs: None }

// Keeps compiling:
let e = ModuleError { kind: ErrorKind::Http, message: msg, ..Default::default() };
// Or simply:
let e = ModuleError::new(ErrorKind::Http, msg);
```

The wire is unaffected either way: a module built against an older SDK
still speaks the same protocol.

Buttons the manifest declares in `[[actions]]` arrive at `Module::action`
(default: unsupported) with a `wire::ActionParams`; `req.handler` names the
function and `req.t("key", &[args])` reads the module's `[strings]`. Return
a `wire::ActionReply` ([actions](README.md#actions)).

## The manifest

```toml
capabilities = ["translate", "detect_language", "dictionary"]
languages = ["en", "de", "ru"]          # omit for "all"

[module]
id = "my-translator"                    # lowercase, unique
name = "My Translator"
version = "0.1.0"
api = 1                                 # protocol version
runtime = "native"
entry = "my-translator"                 # see below

[language_map]                          # optional: our code -> the service's
"zh-Hans" = "zh"

[permissions]
network = ["api.example.com"]           # hosts reachable through host.http

[[options]]
key = "api_key"
type = "secret"
label = "API key"
required = true
```

`entry` names the library. It must be a plain file name — a module can only
load a library from its own directory. Glossa tries, in order:

1. the name exactly as written (so `entry = "engine.so"` always works);
2. the platform conventions — `{entry}.dll`, `lib{entry}.dll` on Windows;
   `lib{entry}.so`, `{entry}.so` on Linux and other Unixes;
   `lib{entry}.dylib`, `{entry}.dylib` on macOS;
3. the same with `-` replaced by `_`, because cargo builds the package
   `my-translator` as `my_translator.dll` / `libmy_translator.so`.

So naming the cargo package is enough. If `entry` is empty the module `id` is
used. When nothing matches, the error lists every name that was tried.

Option values (`[[options]]`, merged with the user's settings) arrive in every
request as `options`; you never have to ask for them.

## The C ABI

The full contract, with comments on every field, is
[`include/glossa_module.h`](../../include/glossa_module.h). In short:

```c
typedef struct GlossaHostVTable {
    void *ctx;
    char *(*call)(void *ctx, const char *method, const char *params_json);
    void  (*free)(void *ctx, char *s);
} GlossaHostVTable;

typedef struct GlossaModuleVTable {
    uint32_t protocol_version;              /* GLOSSA_PROTOCOL_VERSION (1) */
    char *(*call)(const char *method, const char *params_json,
                  const GlossaHostVTable *host);
    void  (*free)(char *s);
} GlossaModuleVTable;

GLOSSA_EXPORT const GlossaModuleVTable *glossa_module_v1(void);
```

Every string is NUL-terminated UTF-8 JSON. `call` receives a method
(`translate`, `detect`, `dictionary`, `speak`, `recognize`) and its parameters,
and returns an envelope:

```json
{"result": {"text": "Hallo"}}
{"error": {"kind": "missing_credential", "message": "api_key"}}
```

Error kinds: `bad_request`, `unsupported`, `source_language`,
`target_language`, `missing_credential`, `bad_response`, `http`, `network`,
`rate_limited` (with optional `retry_after_secs`), `not_found`, `other`. The
UI turns each into a specific message, so pick the closest one.

While handling a call the module may call back into the host with the same
envelope convention: `http` (through the user's proxy, checked against
`[permissions] network`), `log`, `cache_get`, `cache_set` and the
`image.<operation>` calls of the image API (`Host::image` in the Rust SDK;
see [ocr-modules.md](ocr-modules.md#the-image-api)). The message shapes are
in the header and in `glossa-module-sdk/src/wire.rs`. An OCR module in Rust
that uses the image API is
[`modules/native/example-lines`](../../modules/native/example-lines).

A C module that uppercases text, with a twenty-line JSON reader and nothing
else, is in [`modules/native/example-c`](../../modules/native/example-c); the
test suite compiles and loads it to keep the ABI honest.

## Ownership and threading

Each side frees only what it allocated:

| string                                   | owner  | released by                  |
|------------------------------------------|--------|------------------------------|
| `method`, `params_json` passed to `call` | host   | host, after `call` returns   |
| reply returned by module `call`          | module | host calls the module `free` exactly once |
| `method`, `params` passed to host `call` | module | module                       |
| reply returned by host `call`            | host   | module calls host `free(ctx, s)` exactly once |

Never release a host string with your own `free()`: on Windows a DLL may use
a different C runtime, and therefore a different heap, from the program that
loaded it.

Threading:

* Glossa never makes two calls into one module at the same time, but
  successive calls may arrive on different threads.
* The `GlossaHostVTable` pointer is valid only during the call it was passed
  to, and only on that call's thread. Do not store it; host calls from any
  other thread are refused with an error.
* The table `glossa_module_v1` returns must stay valid while the library is
  loaded — make it `static const`.
* The library is unloaded when Glossa no longer needs the module. Stop any
  threads you started before your last call returns.
* Nothing may unwind across the boundary: catch C++ exceptions; in Rust the
  SDK catches panics for you (unless you build with `panic = "abort"`, in
  which case a panic ends the process).

## Building for each platform

The library must match Glossa's operating system **and architecture**: a
32-bit DLL cannot be loaded into 64-bit Glossa, nor an x86-64 library into an
ARM64 build. Build for the Glossa you run: the download's name says its
architecture (`x86_64` or `arm64`). On macOS a universal library loads into
either.

**Windows (MSVC toolchain).** Link the C runtime statically so the DLL does
not depend on a particular Visual C++ redistributable being installed:

```toml
# .cargo/config.toml of your module (or RUSTFLAGS="-C target-feature=+crt-static")
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

```sh
cargo build --release --target x86_64-pc-windows-msvc
```

For C/C++ with MSVC use `/MT` (static CRT) rather than `/MD`.

**Windows (cross-compiling from Linux or WSL with MinGW).**

```sh
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
# C: x86_64-w64-mingw32-gcc -shared -O2 -I include -o shout.dll shout.c
```

MinGW-built DLLs depend only on system DLLs (`msvcrt.dll`, `KERNEL32.dll`,
…); check with `x86_64-w64-mingw32-objdump -p my_translator.dll | grep "DLL Name"`
that nothing else sneaked in, and that `glossa_module_v1` is listed among the
exports.

**Linux.**

```sh
cargo build --release      # -> target/release/libmy_translator.so
# C: cc -shared -fPIC -O2 -I include -o libshout.so shout.c
```

Build on (or for) an old enough glibc for your users; a library built on a
bleeding-edge distribution may need symbol versions older systems lack.

**macOS.**

```sh
cargo build --release --target aarch64-apple-darwin   # and/or x86_64-apple-darwin
lipo -create -output libmy_translator.dylib \
    target/aarch64-apple-darwin/release/libmy_translator.dylib \
    target/x86_64-apple-darwin/release/libmy_translator.dylib
```

Unsigned or quarantined libraries may be refused by Gatekeeper; sign them, or
clear the quarantine attribute for local testing.

## Safety

A native module runs **inside** Glossa with all of Glossa's privileges:

* If it crashes — a null pointer, a stack overflow, a double free — Glossa
  crashes with it. There is no sandbox.
* Network permissions in `module.toml` only govern HTTP done *through the
  host*. Native code can open sockets or files on its own; the manifest is a
  statement of intent, not a fence.
* Calls cannot be interrupted. The host's call timeout is not enforced for
  native code (an overrun is only logged), so a module that hangs hangs the
  request. Use `timeout_ms` on host HTTP calls.

This is why native modules can be switched off entirely (*Load native
modules* in *Options → Modules*, `[modules] allow_native` in `config.toml`),
and why you should only install native modules from people you trust. When a module does not need native code, write it in
JavaScript or Python instead: those runtimes run it at arm's length.

## Why JSON over a C ABI, not the Rust ABI

Rust has no stable ABI. Passing Rust types — `String`, `Vec`, trait objects —
between a program and a plugin works only when both were built by the same
compiler version with the same flags and the same dependency versions; any
mismatch is undefined behaviour that usually shows up as a crash far from the
cause. Requiring every module author to rebuild for every Glossa release (and
match its exact toolchain) would make third-party modules impractical.

A C ABI with two function pointers and a version number has none of that
coupling:

* **No compiler coupling.** A module built with Rust 1.82 loads into a Glossa
  built with Rust 1.99, and a module built with MSVC, GCC, Clang or Zig loads
  equally well. Only the protocol version must match.
* **No struct layouts to keep in step.** The two tables are the entire binary
  interface. Everything else is JSON, which can grow optional fields without
  breaking old modules — new keys are ignored, missing keys take defaults.
* **Any language.** The SDK is a convenience, not a requirement; the C
  example proves it.
* **The cost is negligible.** A translation request spends milliseconds to
  seconds on the network; serialising a few hundred bytes of JSON is noise.

The lesson is also empirical. pot-app's DLL plugins passed native data
directly and broke for users in ways the authors could not reproduce: plugins
built against a newer MSVC redistributable than the user had installed failed
to load with an opaque "module not found", and 32-bit/64-bit mismatches did
the same. Glossa's choices follow from that: a JSON-over-C interface, a clear
error when the protocol version differs or the entry point is missing, a
recommendation to link the CRT statically on MSVC
(`-C target-feature=+crt-static`), and a reminder to build for the same
architecture as the Glossa you run.

## Troubleshooting

| message | meaning |
|---------|---------|
| `no library for entry "x" in … (tried: …)` | the file is missing or named differently; the list shows every name tried |
| `cannot load …` | the OS refused the file: wrong architecture, a missing dependency DLL/`.so`, or not a library at all |
| `… does not export glossa_module_v1` | the symbol is missing or mangled: use `export_module!`, or `extern "C"`/`GLOSSA_EXPORT` in C/C++ |
| `module speaks native protocol N, this Glossa speaks M` | rebuild against a matching `glossa-module-sdk` / header |
| `module reply is not a {"result"} / {"error"} envelope` | `call` returned something other than a reply envelope |
| `native modules are switched off` | the user disabled native modules in the settings |
