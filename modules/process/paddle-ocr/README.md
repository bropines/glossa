# PaddleOCR (offline)

Text recognition that runs on your own machine: PaddleOCR's PP-OCRv5
mobile models on ONNX Runtime. It reads Chinese, Japanese, Korean,
Russian and the other Cyrillic languages, and the languages written in
Latin letters, and returns each line with its outline, so in-place
translation can draw over the picture.

It is a [process module](../../../docs/modules/process-modules.md):
`paddle-ocr`, a small Rust program in `src/`, which Glossa starts on the
first OCR call and stops after two minutes without one. Until then nothing
is loaded and the module costs no memory.

## Contents

- [Setting it up](#setting-it-up)
- [Options](#options)
- [Speed, memory and accuracy](#speed-memory-and-accuracy)
- [Licences](#licences)
- [How it works](#how-it-works)
- [Testing](#testing)
- [Testing a local build](#testing-a-local-build)
- [Releasing the binary](#releasing-the-binary)

## Setting it up

1. Copy this folder into your modules folder (*Options → Modules → Open
   folder*), or into `<GLOSSA_HOME>/modules/paddle-ocr`.
2. *Options → Services → PaddleOCR (offline) → Files*: *Download* fetches
   the program with ONNX Runtime and the two shared models, then *Get* the
   recogniser for each script you read:

   | asset | reads | size |
   |---|---|---|
   | `bin` | the program and ONNX Runtime, for your system | 9–14 MB |
   | `det.onnx` | finds the lines (every script) | 4.8 MB |
   | `cls.onnx` | turns upside-down lines round | 1.0 MB |
   | `rec-latin.onnx` | English, German, French, Spanish, Polish, Turkish, Vietnamese… | 7.9 MB |
   | `rec-eslav.onnx` | Russian, Ukrainian, Belarusian, English | 7.9 MB |
   | `rec-cyrillic.onnx` | Bulgarian, Serbian, Kazakh, Mongolian and other Cyrillic, English | 8.1 MB |
   | `rec-ch.onnx` | Simplified and Traditional Chinese, Japanese, English | 16.6 MB |
   | `rec-korean.onnx` | Korean, English | 13.5 MB |

   From a terminal:

   ```bash
   glossa-cli assets download paddle-ocr                          # bin, det.onnx, cls.onnx
   glossa-cli assets download paddle-ocr rec-eslav.onnx rec-ch.onnx
   glossa-cli ocr shot.png -e paddle-ocr
   ```

   A typical set (the shared files and two recognisers) is 30–45 MB.
3. Pick *PaddleOCR (offline)* as the OCR engine, or keep it in the
   fail-over list behind another one.

Systems: Windows x64 and arm64, Linux x64 and arm64, macOS on Apple
silicon. Intel Macs are left out: ONNX Runtime stopped shipping them, and
they have Apple Vision. Linux has not been tried on real hardware yet.

## Options

| option | default | meaning |
|---|---|---|
| *Latin-script languages*, *Russian…*, *Other Cyrillic*, *Chinese and Japanese*, *Korean* | on | With the language set to *auto*, every downloaded recogniser that is switched on reads each line, and the most confident reading wins. Switch off the ones you rarely need: each costs time and memory. |
| *Threads* (`threads`) | 0 | Processor threads per model; 0 uses up to four. |
| *Largest picture side* (`max_side`) | 2000 | Larger pictures are scaled down before lines are looked for: lower is faster, higher finds smaller text on big screenshots. |

When the source language is set (Japanese, Russian, Korean…), that
language's recogniser is used, switched on or not. If it is not
downloaded, the module fails with a message naming the file to get, and
Glossa moves on to the next engine.

## Speed, memory and accuracy

Measured on an Apple M-series Mac (16 GB) with other builds running, on a
760×330 picture of six lines (English, Russian, Japanese, 20–28 px text):

| recognisers | first picture (models loading) | next pictures | peak memory |
|---|---|---|---|
| one (`rec-eslav`) | 0.34 s | 0.26 s | 170 MB |
| one (`rec-ch`) | 0.38 s | 0.30 s | 230 MB |
| three (*auto*: latin, eslav, ch) | 0.85 s | 0.74 s | 350 MB |
| one, a single-line capture (386×28) | 0.11 s | 0.03 s | 75 MB |

Through Glossa, the first call of a session (starting the program, ONNX
Runtime and three models) took 1.4 s for the same picture.

Every line of the test picture came back exactly, with *auto* and three
recognisers (confidence 0.99–1.0). With one recogniser, lines in another
script come back as nonsense or are dropped: that is what the language
setting and the script options are for. The memory is given back when
Glossa stops the process, two minutes after the last call
(`idle_timeout_secs`).

## Licences

| part | licence | from |
|---|---|---|
| `paddle-ocr` (this folder) | MIT OR Apache-2.0, like Glossa | this repository |
| its Rust dependencies (`oar-ocr`, `ort`, `image`…) | Apache-2.0, MIT, BSD, Zlib, Unicode-3.0; nothing copyleft | crates.io |
| ONNX Runtime 1.31.1, packed in `bin` | MIT (`LICENSE-onnxruntime.txt`, `ThirdPartyNotices-onnxruntime.txt` in the archive) | [microsoft/onnxruntime](https://github.com/microsoft/onnxruntime/releases/tag/v1.31.1) |
| PP-OCRv5 models | Apache-2.0 | [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR), converted to ONNX by [RapidOCR](https://github.com/RapidAI/RapidOCR) (Apache-2.0) and published on [ModelScope](https://www.modelscope.cn/models/RapidAI/RapidOCR) (Apache-2.0), tag `v3.10.0` |

The models are downloaded from ModelScope at the pinned tag; their SHA-256
values are RapidOCR's own (`default_models.yaml`) and were checked against
the downloads. RapidOCR publishes no Hugging Face mirror of v3.10.0, so
there is one URL per model.

## How it works

- **Protocol.** JSON lines on stdin and stdout. `recognize` asks Glossa
  which files are there (the `assets` host call), decodes `image_base64`,
  and answers with lines: `{"level": "line", "text", "polygon", "confidence"}`
  in the picture's own pixels ([OCR result
  format](../../../docs/modules/ocr-modules.md#the-result)). Other methods
  answer `unsupported`; a picture without text answers `not_found`.
- **Pipeline** (`src/engine.rs`, on [`oar-ocr`](https://github.com/greatv/oar-ocr)):
  a border is added (the detector misses text touching the edge); the
  detector finds the lines once; each line is cut out straight, turned
  round if the orientation model says it is upside down, and read by each
  chosen recogniser in batches of similar width. Readings below 0.5
  confidence are dropped.
- **Character lists.** RapidOCR's recognisers carry their alphabet in the
  ONNX metadata (`character`); `src/onnx_meta.rs` reads it, so a recogniser
  is one file.
- **ONNX Runtime** is loaded at run time (`ort`'s `load-dynamic`) from the
  library beside the program, or from `ORT_DYLIB_PATH`. The Glossa
  workspace excludes this crate, so building Glossa never compiles it.
- **Lifetime.** Models load on the first request, each recogniser when it
  is first needed, and stay until Glossa closes stdin.

By hand, without Glossa:

```bash
paddle-ocr --image shot.png --models <folder with det.onnx, cls.onnx, rec-*.onnx> [--lang ru]
```

prints the result and the cold and warm timings.

## Testing

```bash
cd modules/process/paddle-ocr
cargo test                      # options, metadata reader, result format, and the
                                # protocol replayed against the built binary (no models)
PADDLE_OCR_MODELS=<dir> PADDLE_OCR_IMAGE=tests/pictures/mixed.png \
ORT_DYLIB_PATH=<libonnxruntime> cargo test --release -- --ignored   # real recognition
```

`tests/basic.toml` holds the fixtures for `glossa-cli module test`. They
run the real models, so they need the module's files, recognisers
included (`rec-latin`, `rec-eslav`, `rec-ch`); without them the harness
skips the module. CI leaves this module out of its module run until the
binary is released (`.github/workflows/ci.yml`); `paddle-ocr.yml` runs
`cargo test` and a smoke test of the packed program on every platform
when this folder changes.

## Testing a local build

Until the binary is released, its `sha256` in `module.toml` is a
placeholder and the download fails its check. To try the module with your
own build (macOS or Linux):

```bash
cd modules/process/paddle-ocr
cargo build --release
# an official ONNX Runtime for your system, unpacked:
# https://github.com/microsoft/onnxruntime/releases/tag/v1.31.1
./package.sh target/release/paddle-ocr ~/Downloads/onnxruntime-osx-arm64-1.31.1 /tmp/po/paddle-ocr-dev.tar.gz
./dev-install.sh /tmp/glossa-test /tmp/po/paddle-ocr-dev.tar.gz 8765
python3 -m http.server -b 127.0.0.1 -d /tmp/po 8765 &
export GLOSSA_HOME=/tmp/glossa-test
glossa-cli assets download paddle-ocr
glossa-cli assets download paddle-ocr rec-latin.onnx rec-eslav.onnx rec-ch.onnx
glossa-cli module test --strict /tmp/glossa-test/modules/paddle-ocr
glossa-cli ocr tests/pictures/mixed.png -e paddle-ocr
```

`dev-install.sh` copies the module and its fixtures into the data folder
and points this system's `bin` asset at the local server with the
archive's real hash; the models still come from ModelScope. On Windows,
edit the copied `module.toml` the same way by hand (`url`, `sha256`,
`size` of the `windows` / `x86_64` entry, and `127.0.0.1` in `network`).

## Releasing the binary

The `bin` archives are built by
[`.github/workflows/paddle-ocr.yml`](../../../.github/workflows/paddle-ocr.yml)
and attached to a GitHub release of this repository. Publishing is the
maintainer's decision; nothing is released automatically.

1. Set `version` in `Cargo.toml` and in `module.toml` (`[module] version`)
   to the new version, and the `bin` URLs in `module.toml` to
   `…/releases/download/paddle-ocr-v<version>/paddle-ocr-<version>-<os>-<arch>.<ext>`.
   Commit.
2. Tag and push: `git tag paddle-ocr-v<version> && git push origin paddle-ocr-v<version>`.
   (A run by hand from *Actions → paddle-ocr* builds the same archives as
   artifacts, without a release.)
3. The workflow tests and builds on macOS arm64, Linux x64 and arm64 and
   Windows x64 and arm64, packs each with ONNX Runtime (`package.sh`),
   smoke-tests the archive, and creates a **draft** release with the
   archives, `SHA256SUMS` and `SIZES`.
4. Copy each archive's `sha256` and `size` from `SIZES` into its `bin`
   entry in `module.toml` and remove the `TODO(release)` note. In
   `.github/workflows/ci.yml`, drop the exclusion and fetch the three
   recognisers the fixtures use before the module run
   (`glossa-cli assets download paddle-ocr rec-latin.onnx rec-eslav.onnx rec-ch.onnx`),
   so CI runs the fixtures. Commit.
5. Publish the draft release.

The main release workflow (tags `v*`) does not build this program, and a
`paddle-ocr-v*` tag does not trigger it.
