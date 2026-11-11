#!/bin/sh
# Bundle a local release build into "Glossa Dev.app", for trying it on this
# Mac.
#
#   cargo build --release -p glossa-ui-egui -p glossa-cli
#   scripts/macos-bundle.sh [OUT_DIR]        # default: ~/Desktop/Glossa
#
# The development copy has its own name and bundle identifier
# (io.github.bropines.glossa.dev), so it sits next to a release Glossa.app
# from GitHub without the two fighting over the permissions macOS stores
# per identifier: each keeps its own. Both use the same settings folder, so
# run one at a time (a second copy only brings the first one forward).
#
# Signs with the "Glossa Dev" identity when it exists (see
# scripts/macos-dev-identity.sh), so the app keeps its permissions across
# rebuilds; otherwise signs ad hoc and clears the permissions macOS stored
# for the previous build, which no longer match, so the app can ask again.
set -e
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$HOME/Desktop/Glossa"}
APP="$OUT/Glossa Dev.app"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release"
ID=io.github.bropines.glossa.dev

# A development Glossa.app from before the split carried the release's
# identifier. It is not removed here: a release Glossa.app may just as well
# sit in the same folder, and nothing tells the two apart for certain.
if [ -d "$OUT/Glossa.app" ]; then
  echo "Note: $OUT/Glossa.app exists. If it is an old development build, delete it" >&2
  echo "      yourself, so Spotlight and Launchpad show one copy of each." >&2
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/glossa" "$APP/Contents/MacOS/glossa"
cp "$BIN/glossa-cli" "$OUT/glossa-cli"
VERSION=$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')
# CFBundleVersion takes numbers only (see scripts/version-numbers.sh).
BUILD=$("$ROOT/scripts/version-numbers.sh" "$VERSION" | sed -n 's/^macos=//p')
sed -e "s/@VERSION@/$VERSION/g" \
    -e "s/@BUILD@/$BUILD/g" \
    -e "s|<string>io.github.bropines.glossa</string>|<string>$ID</string>|" \
    -e "s|<string>Glossa</string>|<string>Glossa Dev</string>|g" \
    "$ROOT/assets/macos/Info.plist" > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
# The licences, and the third-party notices for this Mac's build, where the
# About window looks for them. The generator needs the crates' sources in
# cargo's registry, which a build has just put there.
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE" "$APP/Contents/Resources/"
if ! python3 "$ROOT/scripts/third-party-notices.py" -o "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt"; then
  echo "Note: no THIRD-PARTY-NOTICES.txt in this bundle (see the message above)." >&2
  rm -f "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt"
fi
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
  if ! codesign --force --deep -s "Glossa Dev" "$APP"; then
    # errSecInternalComponent: the keychain will not hand the key to a
    # session that cannot show its prompt, such as one over SSH.
    echo "Signing failed. Over SSH the keychain refuses the key: run this script in Terminal on the Mac." >&2
    exit 1
  fi
  echo "Signed with \"Glossa Dev\"."
else
  codesign --force --deep -s - "$APP"
  for s in Accessibility ScreenCapture PostEvent ListenEvent; do
    tccutil reset "$s" "$ID" >/dev/null 2>&1 || true
  done
  echo "Signed ad hoc; permissions reset (run scripts/macos-dev-identity.sh once to keep them)."
fi
echo "$APP"
