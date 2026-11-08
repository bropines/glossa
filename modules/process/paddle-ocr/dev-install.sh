#!/usr/bin/env bash
# Install this module into a Glossa data folder with a locally built `bin`
# archive (from package.sh) in place of the released one, for testing
# before the binary is published.
#
#   dev-install.sh GLOSSA_HOME ARCHIVE [PORT]
#
# Copies the module to GLOSSA_HOME/modules/paddle-ocr and points this
# platform's `bin` asset at http://127.0.0.1:PORT/<archive name> with the
# archive's real hash and size. Serve the archive's folder while Glossa
# downloads it:
#
#   python3 -m http.server -b 127.0.0.1 -d "$(dirname ARCHIVE)" PORT
#   GLOSSA_HOME=... glossa-cli assets download paddle-ocr            # bin, det, cls
#   GLOSSA_HOME=... glossa-cli assets download paddle-ocr rec-eslav.onnx
set -euo pipefail

home=$1 archive=$2 port=${3:-8765}
here=$(cd "$(dirname "$0")" && pwd)
case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "Windows: edit module.toml by hand (see README.md)" >&2; exit 2 ;;
esac
arch=$(uname -m)
[ "$arch" = arm64 ] && arch=aarch64
if command -v sha256sum >/dev/null; then sum=$(sha256sum "$archive" | cut -d' ' -f1)
else sum=$(shasum -a 256 "$archive" | cut -d' ' -f1); fi
size=$(wc -c < "$archive" | tr -d ' ')
name=$(basename "$archive")

dest=$home/modules/paddle-ocr
mkdir -p "$dest"
cp "$here/module.toml" "$here/README.md" "$dest/"
# The fixtures too, so `glossa-cli module test` runs against the copy.
mkdir -p "$dest/tests/pictures"
cp "$here"/tests/*.toml "$dest/tests/"
cp "$here"/tests/pictures/*.png "$dest/tests/pictures/"
# Point this system's `bin` entry at the local archive and drop the other
# systems' entries, whose placeholder hashes Glossa refuses. Everything
# else stays as released, so the models still come from their real URLs.
python3 -I - "$dest/module.toml" "$os" "$arch" "http://127.0.0.1:$port/$name" "$sum" "$size" <<'EOF'
import re, sys
path, os_, arch, url, sha, size = sys.argv[1:]
text = open(path, encoding="utf-8").read()
blocks = re.split(r"(?m)^(?=\[\[assets\]\])", text)
out, found = [], False
for b in blocks:
    if 'name = "bin"' in b:
        if f'os = ["{os_}"]' not in b or f'arch = ["{arch}"]' not in b:
            continue
        b = re.sub(r'(?m)^url = .*$', f'url = "{url}"', b)
        b = re.sub(r'(?m)^sha256 = .*$', f'sha256 = "{sha}"', b)
        b = re.sub(r'(?m)^size = .*$', f'size = {size}', b)
        found = True
    out.append(b)
if not found:
    sys.exit(f"no bin asset for {os_}/{arch} in module.toml")
text = "".join(out).replace('network = ["github.com",', 'network = ["127.0.0.1", "github.com",')
open(path, "w", encoding="utf-8").write(text)
EOF
echo "installed into $dest (bin: $name, $size bytes)"
