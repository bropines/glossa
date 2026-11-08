# Local engines worth offering as modules

Which offline engines Glossa should offer as modules: text recognition,
translation, speech, language detection and dictionaries. For each, this
covers model and runtime size, how it would ship, its speed, its quality for
the languages that matter most here (Russian, English, Japanese, Chinese,
Korean, German), its licences, and its support on each OS. The document ends
with a ranked shortlist and a rough plan for each pick.

Research only: nothing here is implemented. Figures were checked in November
2026 against the projects' own release pages, model cards and file listings
(linked inline). Speeds are from published benchmarks unless marked
*estimate*. Benchmark on real screenshots before quoting any speed in the
user docs.

## Contents

- [What Glossa already has](#what-glossa-already-has)
- [Constraints that decide the shape](#constraints-that-decide-the-shape)
- [Text recognition](#text-recognition)
- [Offline translation](#offline-translation)
- [Text to speech](#text-to-speech)
- [Language detection](#language-detection)
- [Dictionaries and spelling](#dictionaries-and-spelling)
- [Using module assets](#using-module-assets)
- [Shortlist](#shortlist)
- [Open questions](#open-questions)

## What Glossa already has

Some of the candidates are already covered. A module only adds something
where it fills a gap.

| area | already there | gap |
|---|---|---|
| OCR | Apple Vision (macOS), Windows OCR, OneOCR (Windows 11), Tesseract through the installed binary, Google Lens, AI vision endpoints ([ocr.md](../ocr.md#engines)) | Linux has nothing good offline. Windows OCR needs a language pack per language. Tesseract is weak on CJK and screenshots, and the user has to install it |
| translation | web translators; LibreTranslate (Argos inside) as a self-hosted service; OpenAI-compatible endpoints with **Ollama** and **LM Studio** presets (`crates/glossa-services/src/openai.rs`) | nothing that works offline out of the box |
| speech | the system voice (`glossa-services/src/system_tts`) | quality varies, and Linux often has only espeak |
| language detection | Bing and Yandex, online only | nothing offline: `auto` fails without a network |
| dictionaries | StarDict, DSL, XDXF and TSV readers (`crates/glossa-dict`) | no dictionary data ships or downloads |
| spelling | Hunspell through `spellbook`, using the system's dictionaries (`glossa-platform/src/spell`) | Windows and macOS have no Hunspell dictionaries of their own |

So **local LLM translation already works** through the Ollama preset, as
long as the user installs Ollama. A module would let it run without Ollama.

## Constraints that decide the shape

- **Resident memory is single megabytes.** Every engine below needs tens to
  hundreds of MB once a model is loaded, so the model must never stay
  loaded while the app sits in the tray.
- **Process modules suit heavy engines; native ones do not.** A process
  module starts on its first call and stops after `idle_timeout_secs`
  ([process-modules.md](../modules/process-modules.md#lifetime)), so it
  costs nothing while idle. A native module stays loaded for the whole run:
  the registry cannot unload it, and a model freed inside it still leaves
  the runtime library mapped. A crash in it also takes Glossa down. So:
  - every model-backed engine below is a **process module**: a small Rust
    (or C++) binary that speaks JSON lines on stdio, built per OS by CI;
  - **native** (or built in) only for things measured in hundreds of
    kilobytes, such as a trigram language detector.
- **Downloads go through module assets.** The hash-checked `[[assets]]`
  entries the module-assets work adds to `module.toml` (url, sha256, size,
  unpack, os/arch) fetch the engine binary and its models. Nothing large
  goes into the Glossa download. See [Using module assets](#using-module-assets).
- **Clean licences only for anything Glossa fetches for the user.** The
  allowed licences are:
  - code: MIT, Apache-2.0, BSD, MPL-2.0;
  - models and data: CC0, CC-BY, CC-BY-SA, Apache-2.0, MIT.

  Excluded:
  - non-commercial terms (CC-BY-NC*);
  - "research only" model terms;
  - terms with regional exclusions;
  - revenue-gated licences such as OpenRAIL-M variants with company-size
    limits.

  GPL code is only acceptable in a **separately downloaded process
  module**, a separate program talking over pipes, and only with the
  licence stated on its options page. It never goes into Glossa's MIT/Apache
  binaries.
- **Windows builds are cross-compiled in WSL** (`x86_64-pc-windows-gnu`).
  ONNX Runtime and most C++ engines ship MSVC binaries. A process module's
  binary is its own program, so CI can build it on a Windows MSVC runner
  without touching how Glossa itself is built.

## Text recognition

| engine | models | runtime | languages here | licence (code / models) | verdict |
|---|---|---|---|---|---|
| **PaddleOCR PP-OCRv5 mobile** (ONNX) | det 4.8 MB; rec 8–17 MB per script; cls 1 MB | ONNX Runtime: 17 MB (win-x64 DLL), 45 MB (macOS arm64 dylib), 29 MB (linux-x64 .so) | all six (see below) | Apache-2.0 / Apache-2.0 | **build it** |
| PP-OCRv6 tiny/small (June 2026) | tiny det 1.8 MB + rec 4.5 MB; small 9.9 + 21 MB | ONNX Runtime | zh, en, ja and Latin in one model; no Cyrillic or Korean mentioned | Apache-2.0 | use for CJK and Latin once ports catch up; keep v5 for ru and ko |
| Tesseract 5.5 | tessdata_fast: eng 4.1, rus 3.9, jpn 2.5, chi_sim 2.5, kor 1.7, deu 1.5 MB | libtesseract plus leptonica, a few MB | all, weak on CJK and stylised text | Apache-2.0 / Apache-2.0 | already built in |
| ocrs (Rust, RTen) | det 2.5 + rec 9.7 MB | pure Rust | Latin only, "early preview" | MIT/Apache / unstated (trained on CC-BY-SA data) | no: no Cyrillic or CJK |
| EasyOCR | ~100 MB of weights | PyTorch, 130–540 MB | all | Apache-2.0 | no: too heavy, last release 2024 |
| Surya 2 | 1.37 GB | PyTorch | 90+ | Apache-2.0 / modified OpenRAIL-M, revenue-gated | no: licence and size |
| manga-ocr | 444 MB (ONNX conversions 140–460 MB) | PyTorch or ONNX | Japanese only, vertical manga text | Apache-2.0 / Apache-2.0 | later, as an opt-in Japanese module |
| PaddleOCR-VL 1.6, GOT-OCR 2, Florence-2, dots.ocr, DeepSeek-OCR | 0.9–6.7 GB | llama.cpp / PyTorch | many | Apache-2.0 or MIT | no: too big and slow on CPU; reachable today through an AI endpoint |

Sources:
- [PaddleOCR releases](https://github.com/PaddlePaddle/PaddleOCR/releases/tag/v3.7.0)
- [PP-OCRv5 languages](https://www.paddleocr.ai/latest/en/version3.x/algorithm/PP-OCRv5/PP-OCRv5_multi_languages.html)
- [RapidOCR model list](https://github.com/RapidAI/RapidOCR/blob/main/python/rapidocr/default_models.yaml)
- [ONNX Runtime 1.31.1](https://github.com/microsoft/onnxruntime/releases/tag/v1.31.1)
- [tessdata_fast](https://github.com/tesseract-ocr/tessdata_fast)
- [ocrs](https://github.com/robertknight/ocrs)
- [Surya](https://huggingface.co/datalab-to/surya-ocr-2)
- [manga-ocr](https://huggingface.co/kha-white/manga-ocr-base)

### PaddleOCR in detail

**Recognition models** (PP-OCRv5 mobile, as RapidOCR 3.10 publishes them on
ModelScope):

| rec model | size | covers |
|---|---|---|
| `ch_PP-OCRv5_rec_mobile` | 16.6 MB | Simplified and Traditional Chinese, English, Japanese |
| `eslav_PP-OCRv5_rec_mobile` | 7.9 MB | Russian, Ukrainian, Belarusian, English |
| `cyrillic_PP-OCRv5_rec_mobile` | 8.1 MB | all Cyrillic (bg, sr, kk…), English |
| `korean_PP-OCRv5_rec_mobile` | 13.5 MB | Korean, English |
| `latin_PP-OCRv5_rec_mobile` | 7.9 MB | German, French, Spanish and the rest |
| `en_PP-OCRv5_rec_mobile` | 7.9 MB | English |

The detector (4.8 MB) is shared by every script. A useful full set (det,
cls, ch, eslav, korean, latin) is about 52 MB. Most users need det plus one
or two recognisers, which is 15–30 MB.

**Speed.**
- On an M2, CPU provider: detection takes 163 ms per image and recognition
  18.5 ms per line. The CoreML provider was much slower, 1.1 s for
  detection ([RapidOCR blog](https://rapidai.github.io/RapidOCRDocs/main/blog/2026/02/28/coreml-vs-cpu-provider-onnxruntime-rapidocr)).
- *Estimate:* 150–400 ms for a typical screen region on a laptop CPU, plus
  a few hundred ms to load the session when the process starts cold.
- That is slower than Apple Vision, about the same as Windows OCR, and
  more accurate than Tesseract on screenshots and CJK. Small published
  tests favour Paddle (character error rate 0.20 against Tesseract's 0.33
  on receipts). No controlled screenshot benchmark exists.

**Rust route.**
- [`oar-ocr`](https://github.com/greatv/oar-ocr) 0.10 (Apache-2.0) runs
  PP-OCRv4, v5 and v6 on the [`ort`](https://crates.io/crates/ort) crate,
  every script above included.
- `ort` is 2.0.0-rc.15, still pre-1.0. Build it with `load-dynamic`, so
  the module binary loads the official ONNX Runtime library shipped as an
  asset rather than pyke's prebuilt one, which may not suit every target.
- Turn off `oar-ocr`'s `auto-download`. Models come from module assets.

**Platforms.**
- Windows x64 and arm64, Linux x64 and aarch64, and macOS arm64 all have
  official ONNX Runtime builds.
- **ONNX Runtime 1.31 dropped the macOS x64 and universal2 packages.** An
  Intel Mac needs an older ONNX Runtime (1.20 still ships x64) or a custom
  build. Intel Macs already have Apple Vision, so the module could simply
  not offer macOS x64.

**Where it helps most:** Linux (no OS OCR at all), and Windows without the
OCR language pack for the text in front of the user.

## Offline translation

| engine | model per direction | runtime | languages here | licence (code / models) | verdict |
|---|---|---|---|---|---|
| **Bergamot** (Firefox Translations) | int8, 17–60 MB | bergamot-translator (marian fork, C++), a few MB | all six, both directions with English | MPL-2.0 / MPL-2.0 (assumed, see below) | **build it** |
| Argos Translate | 74–196 MB (`.argosmodel`) | CTranslate2 plus Python (Stanza, SentencePiece) | all, through English | MIT or CC0 / per package, CC-BY-4.0 for the OPUS-derived ones | no: bigger and weaker than Bergamot; LibreTranslate already covers it |
| Opus-MT via CTranslate2 | ~300 MB fp32, ~75–80 MB int8 (*estimate*) | CTranslate2 (MIT; 1.5 MB on macOS arm64, 20–43 MB on x86 with MKL) through `ct2rs` | ru, zh, ja→en, ko→en, de good; en→ja is Bible-trained, en→ko only the 418 MB tc-big | MIT / CC-BY-4.0 or Apache-2.0 per model | fallback only, for pairs Bergamot lacks |
| NLLB-200 distilled 600M | 623 MB (CT2 int8) | CTranslate2 | 200 languages, direct pairs | MIT / **CC-BY-NC-4.0**, "not for production" | no: non-commercial licence |
| M2M100 418M | ~500 MB int8 (*estimate*) | CTranslate2 (not listed by `ct2rs`) | 100, direct | MIT / MIT | no: big, below Bergamot |
| **Hy-MT2 1.8B** (Tencent, May 2026) | Q4_K_M GGUF 1.13 GB | llama.cpp | 33 languages incl. all six, direct | Apache-2.0 / **Apache-2.0** | **build it**, as the LLM tier |
| Qwen3 0.6B / 1.7B | 0.64 GB (Q8) / 1.1 GB (Q4) | llama.cpp | general model; fair at ru/zh/ja | Apache-2.0 | alternative LLM |
| TranslateGemma 4B, Gemma 3 1B/4B | 1.0–2.5 GB | llama.cpp | good | Gemma terms (use restrictions) | offer only as user-chosen downloads, not a default |
| HY-MT1.5, Hunyuan-MT 7B | 1.1 GB / ~4.5 GB | llama.cpp | good | Tencent licence: **not valid in the EU, UK or South Korea** | no |
| TowerInstruct, Seed-X 7B | 7B | llama.cpp | good | CC-BY-NC / custom | no |

Sources:
- [Mozilla model registry](https://storage.googleapis.com/moz-fx-translations-data--303e-prod-translations-data/db/models.json)
- [Firefox models dashboard](https://mozilla.github.io/translations/firefox-models/)
- [mozilla/translations inference](https://github.com/mozilla/translations/tree/main/inference)
- [Argos index](https://github.com/argosopentech/argospm-index)
- [CTranslate2](https://github.com/OpenNMT/CTranslate2)
- [ct2rs](https://github.com/jkawamoto/ctranslate2-rs)
- [Opus-MT ru-en](https://huggingface.co/Helsinki-NLP/opus-mt-ru-en)
- [NLLB card](https://huggingface.co/facebook/nllb-200-distilled-600M)
- [Hy-MT2-1.8B](https://huggingface.co/tencent/Hy-MT2-1.8B)
- [HY-MT1.5 licence](https://huggingface.co/tencent/HY-MT1.5-1.8B/raw/main/License.txt)
- [Qwen2.5-3B licence](https://huggingface.co/Qwen/Qwen2.5-3B-Instruct)

### Bergamot in detail

**Where things live now.**
- Mozilla maintains the runtime in
  [mozilla/translations/inference](https://github.com/mozilla/translations/tree/main/inference).
  `browsermt/bergamot-translator` has been idle since 2024.
- The old `mozilla/firefox-translations-models` repository was archived
  in December 2025. The models are now listed in a registry JSON on Google
  Cloud Storage, regenerated daily.
- 108 directions are released, all to or from English. Each model comes
  with a lexical shortlist and a SentencePiece vocabulary.

**Directions that matter here** (size of the int8 model, COMET22 score):

| direction | variant | size | COMET22 |
|---|---|---|---|
| ru→en | base-memory / tiny | 31.6 / 17.1 MB | 0.85 / 0.84 |
| en→ru | base | 43.0 MB | 0.88 |
| ja, zh, ko→en | base | 59.5 MB | ~0.86 |
| en→ja, en→zh | base-memory | 43.8 MB | 0.90 / 0.86 |
| en→ko | base | 59.5 MB | 0.87 |
| de↔en | base-memory | 31.6 MB | 0.88 / 0.87 |

- A user reading English, Japanese and Chinese into Russian downloads about
  200 MB. Most need one or two directions, 50–100 MB.
- A pair without English (ja→ru) runs as two passes through English. That
  doubles the time and costs some quality, but stays well under a second
  for popup-sized text.

**Speed.** 1,000–6,500 words per second for int8 on 2012–2016 CPUs
([translateLocally paper](https://arxiv.org/abs/2109.10194)), so one
sentence is effectively instant. Loading the model is the cost, about a
second on a cold start.

**Licence.** The code is MPL-2.0: file-level copyleft, fine in a separate
binary. The archived models repository was MPL-2.0. The files on Google
Cloud Storage carry no separate statement, so confirm with Mozilla before
Glossa points downloads at them.

**Build.**
- There is no maintained Rust crate, so the module is a small C++ binary,
  or a thin C shim called from a Rust binary, built with CMake per OS.
- x86 wants intgemm, which needs no MKL for int8 inference. Apple Silicon
  uses ruy or Accelerate. Avoid OpenBLAS, which translateLocally reports as
  up to 100× slower.
- [translateLocally](https://github.com/XapaJIaMnu/translateLocally) (MIT)
  proves the native build works on Windows, macOS (including M1) and Linux.
  Its CLI mode could even serve as a first prototype.

### The LLM tier in detail

- **Speed.** A 1.5–1.8B Q4 model generates about 20–40 tokens/s on a
  laptop CPU and 60–100 on Apple Silicon with Metal (*estimate*,
  extrapolated from [llama.cpp benchmarks](https://github.com/ggml-org/llama.cpp/discussions/4167)).
  A popup sentence takes 1–2 seconds, plus 1–3 s to load the model cold.
- **Memory.** About 1.5 GB while loaded. That is acceptable only because
  the process stops when idle.
- **Quality.** Better than Bergamot on idiom and context, worse on speed
  and size.
- **Model choice.** Hy-MT2 is the only small *dedicated* translation model
  under a clean licence. Its 1.25-bit GGUF (462 MB) needs a llama.cpp
  kernel that is not merged yet; Q4 works with stock llama.cpp.
- **Runtime.** [llama.cpp release](https://github.com/ggml-org/llama.cpp/releases)
  archives (MIT) are 12–25 MB for CPU builds and 17 MB for macOS arm64
  with Metal. Each archive carries every tool; `llama-server` and its
  libraries alone are smaller.

## Text to speech

| engine | voice size | runtime | languages here | licence (code / voices) | verdict |
|---|---|---|---|---|---|
| **Piper** | x_low 21 MB, low/medium 63–77 MB, high 114 MB | piper1-gpl with espeak-ng compiled in; ONNX Runtime | ru (CC0: denis, dmitri), de (CC0: thorsten), zh (CC0: chaowen), en (CC-BY: alba); ja and ko voices are CC-BY-NC-SA only | **GPL-3.0** since v1.3 (moved from MIT `rhasspy/piper`) / per voice | **build it**, as a separate GPL process module with a curated voice list |
| sherpa-onnx | runs Piper/VITS, Kokoro, Matcha, KittenTTS, Supertonic voices | C API; 8–10 MB libraries plus ONNX Runtime; official Rust crate `sherpa-onnx` 2.0 | depends on the voice | Apache-2.0, but Piper and Kokoro voices need espeak-ng (GPL) | the best *runtime* for Piper voices if a non-espeak path appears |
| Kokoro 82M | 86–92 MB quantized ONNX | ONNX Runtime plus misaki G2P (espeak-ng fallback) | en, ja, zh; **no Russian or German** | Apache-2.0 / Apache-2.0 | later, for ja and zh voices |
| KittenTTS nano | 25 MB int8 | ONNX | English only for the small models | Apache-2.0 | no |
| Supertonic 3 | ~99M params | ONNX, Rust example | 31 incl. ru, de | MIT / OpenRAIL-M; repo archived | no |
| RHVoice | small statistical voices | C++ | very good Russian, uk, en | GPL-2.0 per its repo (older releases LGPL) | maybe, for Russian on Linux; voice licences unclear |

Sources:
- [rhasspy/piper archive note](https://github.com/rhasspy/piper)
- [piper1-gpl releases](https://github.com/OHF-Voice/piper1-gpl/releases)
- [Piper voices](https://huggingface.co/rhasspy/piper-voices/raw/main/voices.json) (each voice's `MODEL_CARD` gives the dataset licence)
- [Kokoro voices](https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md)
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
- [KittenTTS](https://github.com/KittenML/KittenTTS)
- [Supertonic](https://github.com/supertone-inc/supertonic)
- [RHVoice](https://github.com/RHVoice/RHVoice)

### Piper in detail

**Speed.** A real-time factor of about 0.15–0.2 for medium voices on a
laptop CPU or a Raspberry Pi 5 (secondary sources, not an official
benchmark). A sentence is ready in well under a second after the voice
loads.

**The licence is the catch.**
- Piper is GPL-3.0 now, and phonemisation goes through espeak-ng (GPL-3.0)
  in every Piper build, old or new.
- That is fine for a **process module downloaded separately**, a separate
  program talking JSON over pipes. It is not fine linked into Glossa.
- The module's `license` field and options page must say GPL-3.0 and link
  the source.

**Voices.**
- The default list should hold only CC0 and CC-BY voices: ru denis or
  dmitri, de thorsten, en alba, zh chaowen.
- Leave out the non-commercial ones: ruslan (ru), and the only ja and ko
  voices.
- `lessac` (en_US) carries the Blizzard 2013 licence, which needs checking
  before it is offered.

**The voice the module returns** is WAV (`audio/wav`), which every
platform plays ([protocol.md](../modules/protocol.md#speak)).

**Platforms.**
- PyPI wheels exist for Windows x64, macOS arm64 and x64, and Linux x64
  and aarch64, but there are no standalone native binaries.
- So the module is either:
  - a small Rust binary on `ort` with an espeak-ng phonemiser (GPL as a
    whole);
  - or sherpa-onnx's VITS path, also with espeak-ng.

  Either way CI builds it per OS.

## Language detection

| library | languages | size | short-text accuracy | licence | verdict |
|---|---|---|---|---|---|
| **whatlang** 0.18 | 70 | trigram tables, a few hundred KB in the binary | ru single words 52%, sentences 84%; tells ja/zh/ko apart by script | MIT | **build it in** (not a module) |
| lingua-rs 1.8 | 75, per-language cargo features | all models ~300 MB on disk; FST models since 1.8, "a few dozen MB" in memory | best on short text: ru single words 76% (high accuracy), sentences 98% | Apache-2.0 | optional module (high accuracy, lazily loaded) |
| whichlang | 16, **no uk or bg** | tiny, no dependencies, ~10× faster than whatlang | anything Cyrillic comes out as Russian | MIT | no: misses Ukrainian |
| fastText lid.176 `.ftz` | 176 | 917 KB | good on sentences | CC-BY-SA-3.0 (model) | possible; the Rust crate's `.ftz` support is unconfirmed |
| CLD3 | 107 | C++ and protobuf | fair | Apache-2.0, archived 2024 | no |
| GlotLID v4 | 2,000+ | 35 MB small model | research grade | Apache-2.0 | no |

Accuracy figures are from [lingua's tables](https://github.com/pemistahl/lingua-rs/tree/main/tables).
Sources: [whatlang](https://github.com/greyblake/whatlang-rs),
[whichlang](https://github.com/quickwit-oss/whichlang),
[fastText LID](https://fasttext.cc/docs/en/language-identification.html),
[lingua release notes](https://raw.githubusercontent.com/pemistahl/lingua-rs/main/RELEASE_NOTES.md).

Detection is tiny enough, and needed often enough (every `auto`
translation offline), that it belongs **in glossa-engine** rather than in a
module:

- whatlang becomes the offline fallback when no online detector answers;
- Glossa's own language list narrows its guesses: the user's source
  languages, plus whatever script the text is in.

Russian against Ukrainian or Bulgarian on one or two words stays
unreliable for every library. CJK is solved by script alone. Check
whatlang's binary-size cost with `cargo bloat` before committing to it.
Lingua with only the user's languages is the upgrade if whatlang's short-text
accuracy disappoints.

## Dictionaries and spelling

The readers exist; what is missing is data. All of it is CC-BY-SA, which
fits a download Glossa offers but not something bundled in the binary.
Show the attribution on the dictionary's options page.

| data | coverage | licence | notes |
|---|---|---|---|
| [Wiktionary dictionaries](https://github.com/Vuizur/Wiktionary-Dictionaries) (kaikki/wiktextract) | X→English for nearly every language, in StarDict format | CC-BY-SA-3.0 and GFDL | ready as StarDict; ru→en, ja→en, zh→en, ko→en, de→en |
| [JMdict](https://www.edrdg.org/edrdg/licence.html) | Japanese–English (and others) | CC-BY-SA-4.0 | needs an acknowledgement in About and regular updates |
| [CC-CEDICT](https://cc-cedict.org/wiki/) | Chinese–English | CC-BY-SA-4.0 | |
| [kengdic](https://github.com/garfieldnate/kengdic) | Korean–English, about 21k entries | MPL-2.0 or LGPL-2.0+ | small and rough |
| [LibreOffice Hunspell dictionaries](https://github.com/LibreOffice/dictionaries) | ru, en, de | ru: BSD-style; en (SCOWL): permissive; **de: GPL-2/3** | download on demand on Windows and macOS rather than bundling |

These need no engine. Each is a **data-only module**: a `module.toml`
whose assets are the dictionary files, with the offline dictionary provider
reading them. That needs one small addition: either the dictionary
provider scans `<models>/<module-id>/`, or the module declares a
`dictionary_dir`. This is cheap and very visible to users.

## Using module assets

The module-assets work (branch `claude/module-assets-d79472`, in progress)
adds `[[assets]]` entries to `module.toml`. Each entry has:

- `name`;
- `url`, one URL or a list of mirrors;
- `sha256` and `size`;
- `unpack`: `none`, `zip` or `tar.gz`;
- optional `os` and `arch` selectors.

Files land in `<models>/<module-id>/<name>`. A module whose assets are not
ready is listed but not loaded until they are downloaded, either on request
or on first call. That fits the engine binaries and the shared models well:

```toml
[module]
id = "paddle-ocr"
name = "PaddleOCR (offline)"
runtime = "process"
command.windows = ["${MODULE_ASSETS}/bin/paddle-ocr.exe"]   # see open questions
command.unix = ["${MODULE_ASSETS}/bin/paddle-ocr"]
idle_timeout_secs = 120
license = "Apache-2.0"

[[assets]]                   # the module binary plus onnxruntime, per platform
name = "bin"
url = "https://github.com/bropines/glossa-modules/releases/download/paddle-ocr-0.1.0/paddle-ocr-windows-x86_64.zip"
sha256 = "…"
size = 9500000
unpack = "zip"
os = ["windows"]
arch = ["x86_64"]

[[assets]]                   # the shared detector, every platform
name = "PP-OCRv5_det_mobile.onnx"
url = ["https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.10.0/onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx",
       "https://huggingface.co/…/ch_PP-OCRv5_det_mobile.onnx"]
sha256 = "…"
size = 4800000
```

**Gap: optional assets.** As written, every asset that matches the OS and
architecture is required before the module loads. The engines here need
the opposite:

- Bergamot has 108 directions and a user wants two;
- PaddleOCR has six recognisers and a user wants one or two;
- Piper has one voice per language.

Downloading all of them (several GB for Bergamot) is not acceptable. The
manifest needs **optional assets chosen by an option**, for example:

```toml
[[assets]]
name = "ruen"
url = "…/ruen/model.ruen.intgemm.alphas.bin.gz"
sha256 = "…"
size = 17000000
optional = true                      # not needed to load the module
when = { option = "pairs", has = "ru-en" }   # fetched when this option includes it
```

Supporting that also needs:

- the options page lists the optional assets with their sizes, as
  checkboxes, and shows download progress;
- the module, through the `assets` host call, learns which assets are
  present;
- the module answers `Unsupported` (or a clear "download the ru→en model"
  error) for a pair it lacks.

The fallback, without this feature, is one module per language pair or per
voice. That works with the current design but clutters the services list.
This is left as a note for the module-assets session.

Other points the engines here raise:

- **Pinned URLs.** Point at a release tag or commit (ModelScope
  `resolve/v3.10.0`, a Hugging Face commit hash, a GitHub release). Never
  use `main` or `latest`: the sha256 would break on the next upload.
- **Mirrors matter.** ModelScope is fast in China and slow elsewhere;
  Hugging Face is the reverse. Both are listed in the example.
- **Compressed models.** Bergamot serves models gzipped. A `gz` unpack
  mode (a single file, not a tar) would save the module from decompressing
  them itself.
- **Binaries from assets.** A process module's `command` must be able to
  name a file inside its assets folder. The asset also needs the executable
  bit on Unix, and on macOS it must not be quarantined. A notarised binary
  or an ad-hoc signature avoids Gatekeeper prompts. Glossa's own downloader
  sets no quarantine attribute, but the binary must still be signed for
  arm64.

## Shortlist

Ranked by value to users against the cost of building and keeping them up
to date.

### 1. Offline language detection (built in, not a module)

The cheapest change with the widest effect: `auto` stops failing offline,
and every offline translator below depends on it.

- **Runtime:** the `whatlang` crate in `glossa-engine`, as the detector
  used when the chosen online one fails or the network is off. Narrow the
  candidates to the user's languages.
- **Size:** a few hundred KB in the binary (measure it). No download.
- **Options page:** nothing new; perhaps an "offline detector" entry in the
  detector list.
- **Later:** a `lingua` process module for high-accuracy short-text
  detection, if users ask.

### 2. PaddleOCR (process module)

Fills the biggest gap: Linux has no offline OCR, and Windows needs a
language pack per language. Beats Tesseract on screenshots and CJK.

**Built:** [`modules/process/paddle-ocr`](../../modules/process/paddle-ocr/README.md),
with measured speed and memory in its README. Differences from the plan
below: the detector runs at PaddleOCR's own scale (short side at least
64 px), not RapidOCR's 736, which doubled memory and split one-line
captures; ONNX Runtime ships inside the per-platform `bin` archive rather
than as separate assets (the official Windows zip is 83 MB for a 17 MB
DLL); recognisers are optional assets and *auto* reads each line with every
downloaded one, keeping the most confident reading.

- **Runtime:** a Rust binary, `paddle-ocr`, on `oar-ocr` + `ort` with
  `load-dynamic`. It speaks the [OCR module protocol](../modules/ocr-modules.md)
  and returns line boxes, so in-place translation works.
- **Assets:**
  - per OS and architecture: the binary plus ONNX Runtime, 17–45 MB;
  - always: the det and cls models (6 MB);
  - by option: one recogniser per script (8–17 MB).

  A typical install is 35–70 MB.
- **Models:** PP-OCRv5 mobile from RapidOCR's ModelScope release, mirrored
  on Hugging Face. Move CJK and Latin to PP-OCRv6 tiny or small once
  `oar-ocr` supports it and its quality is checked.
- **Platforms:** Windows x64/arm64, Linux x64/aarch64, macOS arm64. Skip
  macOS x64, which has Apple Vision.
- **Options page:** scripts as checkboxes (Chinese and Japanese, Russian,
  Korean, Latin) with sizes; *download now*; thread count; the maximum
  image side for speed.
- **Idle cost:** none. The process stops after `idle_timeout_secs`.

### 3. Bergamot (process module)

Real offline translation at 17–60 MB per direction, fast on any CPU, all
six languages paired with English.

- **Runtime:** a C++ binary from `mozilla/translations/inference`, built
  with CMake in CI per OS (intgemm on x86, ruy on arm64). It reads JSON
  lines and keeps loaded models until the process stops.
- **Pivoting:** through English for pairs like ja→ru.
- **Assets:**
  - per OS: the binary, a few MB;
  - by option: per direction, the model, the lexical shortlist and the
    vocabulary (gzipped, from Mozilla's registry).

  Needs optional assets, or one module per direction until they exist.
- **Licence check first:** confirm with Mozilla that the models on Google
  Cloud Storage are MPL-2.0 (or anything else redistributable) before
  pointing Glossa at them.
- **Options page:**
  - directions as checkboxes with sizes, offered in pairs ("Russian ↔
    English" downloads both);
  - tiny or base where both exist;
  - threads.
- **Capabilities:** `translate`, with `languages` limited to the downloaded
  directions plus whatever pivots through English.

### 4. Local LLM translation without Ollama (process module)

**Built:** [`modules/process/local-llm`](../../modules/process/local-llm/README.md),
with measured speed, memory and quality in its README. Differences from
the plan below: Qwen3 comes from the llama.cpp team's Q4_K_M conversion
(1.28 GB; Qwen publishes only Q8_0); the module does not offer language
detection, because Hy-MT2 misnames languages when asked; and Glossa's
unpacker drops symbolic links, so the module recreates the version links
the macOS and Linux llama.cpp builds load their libraries through.

Better context and idiom than Bergamot. It costs a second or two per popup
and a 1.1 GB download. It reuses the existing OpenAI-compatible provider
code.

- **Runtime:** a small Rust wrapper binary. It starts the `llama-server`
  from the llama.cpp release on a random localhost port with the model, and
  translates JSON-line requests into its OpenAI-style HTTP API. Linking
  llama.cpp in through the `llama-cpp-2` crate is the alternative; it
  avoids the second process but means building llama.cpp per OS.
- **Lifetime:** the wrapper stops `llama-server` when it exits, so idle
  memory returns to zero.
- **Assets:**
  - per OS: the llama.cpp build, 12–25 MB (Metal on macOS arm64, CPU or
    Vulkan elsewhere);
  - one model, selectable: Hy-MT2-1.8B Q4_K_M (1.13 GB, Apache-2.0) by
    default; Qwen3-1.7B Q4 (1.1 GB, Apache-2.0) as the alternative.

  Gemma-based models only if the user picks them, with their terms linked.
- **Options page:** the model (with sizes and licences), threads, context
  length, GPU offload on or off, and a "use my Ollama instead" hint
  pointing at the existing preset.
- **Why fourth:** Ollama users already have this today, and it is the
  heaviest download by far.

### 5. Next in line

- **Piper voices** (process module, GPL-3.0, separate download): CC0 voices
  for ru, de and zh and CC-BY for en, 21–77 MB each. A clear upgrade on
  Linux, where the system voice is often espeak. Ranked below the others
  because of the licence handling and because Windows and macOS already
  have decent system voices.
- **Dictionary data modules:** Wiktionary StarDict sets, JMdict and
  CC-CEDICT as asset-only modules feeding the existing readers. They need
  the small `dictionary_dir` addition above; very little code.
- **manga-ocr** (Japanese manga, ~440 MB) and **Kokoro** (ja and zh voices,
  ~90 MB) as opt-in extras.

## Open questions

- **Bergamot model licence.** The files on Google Cloud Storage have no
  stated licence. Ask Mozilla, or read the Remote Settings records Firefox
  downloads them from.
- **Optional assets.** Does the module-assets design take an `optional` /
  `when` selector, or should each pair be its own module?
- **Asset placeholders.** Can `command` name a file in the assets folder
  (`${MODULE_ASSETS}` or similar)? The binary also needs the executable bit
  and a signature on macOS.
- **ONNX Runtime on Intel Macs.** Drop that target, or pin an older ONNX
  Runtime?
- **Where module binaries are built and hosted.** A `glossa-modules`
  release per module, built by a CI matrix (MSVC on Windows for ONNX
  Runtime and C++ engines), is the simplest. That matrix is new CI work.
- **Measured speed.** Benchmark PaddleOCR (cold start and warm) against
  Windows OCR, Apple Vision and Tesseract on real screenshots before
  quoting numbers to users.
