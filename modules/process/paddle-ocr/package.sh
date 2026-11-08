#!/usr/bin/env bash
# Pack the module's `bin` asset: the paddle-ocr binary, the ONNX Runtime
# library beside it, and their licences. The release workflow
# (.github/workflows/paddle-ocr.yml) runs this on each platform; run it
# yourself to test the module with a local build (README.md, "Testing a
# local build").
#
#   package.sh BINARY ORT_DIR OUT
#
# BINARY   the built paddle-ocr (or paddle-ocr.exe)
# ORT_DIR  an unpacked official ONNX Runtime release
#          (onnxruntime-<os>-<arch>-<version>, with lib/ and LICENSE)
# OUT      the archive to write: *.tar.gz (macOS, Linux) or *.zip (Windows)
#
# Prints the archive's sha256 and size, the two values module.toml needs.
set -euo pipefail

bin=$1 ort=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

case "$out" in
  *.zip)
    cp "$bin" "$stage/paddle-ocr.exe"
    cp "$ort/lib/onnxruntime.dll" "$stage/" ;;
  *.tar.gz)
    cp "$bin" "$stage/paddle-ocr"
    chmod 755 "$stage/paddle-ocr"
    # The versioned file itself, not a symlink: Glossa's unpacker keeps
    # plain files only. The binary opens it by this name.
    if [ -f "$ort/lib/libonnxruntime.dylib" ]; then
      cp -L "$ort/lib/libonnxruntime.dylib" "$stage/libonnxruntime.dylib"
    else
      cp -L "$ort/lib/libonnxruntime.so" "$stage/libonnxruntime.so"
    fi ;;
  *) echo "OUT must end in .zip or .tar.gz" >&2; exit 2 ;;
esac
cp "$ort/LICENSE" "$stage/LICENSE-onnxruntime.txt"
cp "$ort/ThirdPartyNotices.txt" "$stage/ThirdPartyNotices-onnxruntime.txt"
cp "$here/README.md" "$stage/README.md"
cp "$here/../../../LICENSE-MIT" "$stage/LICENSE-MIT.txt"
cp "$here/../../../LICENSE-APACHE" "$stage/LICENSE-APACHE.txt"

mkdir -p "$(dirname "$out")"
out_abs=$(cd "$(dirname "$out")" && pwd)/$(basename "$out")
rm -f "$out_abs"
case "$out" in
  *.zip)
    if command -v 7z >/dev/null; then
      (cd "$stage" && 7z a -tzip -mx=9 "$out_abs" . >/dev/null)
    else
      (cd "$stage" && zip -9 -q -r "$out_abs" .)
    fi ;;
  *.tar.gz) (cd "$stage" && tar -czf "$out_abs" *) ;;
esac

if command -v sha256sum >/dev/null; then sum=$(sha256sum "$out_abs" | cut -d' ' -f1)
else sum=$(shasum -a 256 "$out_abs" | cut -d' ' -f1); fi
size=$(wc -c < "$out_abs" | tr -d ' ')
echo "$(basename "$out_abs") sha256=$sum size=$size"
