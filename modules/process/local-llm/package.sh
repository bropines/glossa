#!/usr/bin/env bash
# Pack the module's `bin` asset: the local-llm program, its licences and
# the third-party notices of the crates compiled in. llama.cpp and the
# models are separate assets, downloaded from their own releases. The
# release workflow (.github/workflows/local-llm.yml) runs this on each
# platform; run it yourself to test the module with a local build
# (README.md, "Testing a local build").
#
#   package.sh BINARY OUT
#
# BINARY   the built local-llm (or local-llm.exe)
# OUT      the archive to write: *.tar.gz (macOS, Linux) or *.zip (Windows)
#
# Prints the archive's sha256 and size, the two values module.toml needs.
set -euo pipefail

bin=$1 out=$2
here=$(cd "$(dirname "$0")" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

case "$out" in
  *.zip) cp "$bin" "$stage/local-llm.exe" ;;
  *.tar.gz)
    cp "$bin" "$stage/local-llm"
    chmod 755 "$stage/local-llm" ;;
  *) echo "OUT must end in .zip or .tar.gz" >&2; exit 2 ;;
esac
cp "$here/README.md" "$stage/README.md"
# The notices of every Rust crate compiled in (MIT and BSD ask for them).
py=$(command -v python3 || command -v python)
"$py" "$here/third_party.py" > "$stage/THIRD-PARTY-NOTICES.txt"
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
