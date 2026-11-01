#!/bin/sh
# Bundle a local release build into Glossa.app, for trying it on this Mac.
#
#   cargo build --release -p glossa-ui-egui -p glossa-cli
#   scripts/macos-bundle.sh [OUT_DIR]        # default: ~/Desktop/Glossa
#
# Signs with the "Glossa Dev" identity when it exists (see
# scripts/macos-dev-identity.sh), so the app keeps its permissions across
# rebuilds; otherwise signs ad hoc and clears the permissions macOS stored
# for the previous build, which no longer match, so the app can ask again.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$HOME/Desktop/Glossa"}
APP="$OUT/Glossa.app"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release"
ID=io.github.bropines.glossa

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/glossa" "$APP/Contents/MacOS/glossa"
cp "$BIN/glossa-cli" "$OUT/glossa-cli"
VERSION=$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')
sed "s/@VERSION@/$VERSION/g" "$ROOT/assets/macos/Info.plist" > "$APP/Contents/Info.plist"
# The bundled Python SDK lets Python modules start from a Finder launch.
if [ -f "$ROOT/sdk/python/glossa_module.py" ]; then
  cp "$ROOT/sdk/python/glossa_module.py" "$APP/Contents/Resources/"
fi

ICONSET=$(mktemp -d)/glossa.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$ROOT/assets/icons/glossa.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$ROOT/assets/icons/glossa.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/glossa.icns"

if security find-identity -p codesigning 2>/dev/null | grep -q '"Glossa Dev"'; then
  codesign --force --deep -s "Glossa Dev" "$APP"
  echo "Signed with \"Glossa Dev\"."
else
  codesign --force --deep -s - "$APP"
  for s in Accessibility ScreenCapture PostEvent ListenEvent; do
    tccutil reset "$s" "$ID" >/dev/null 2>&1 || true
  done
  echo "Signed ad hoc; permissions reset (run scripts/macos-dev-identity.sh once to keep them)."
fi
echo "$APP"
