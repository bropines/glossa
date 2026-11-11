#!/bin/sh
# Lay out Glossa under <root>/usr the way a Linux package installs it:
#
#   usr/bin/glossa, usr/bin/glossa-cli
#   usr/share/applications/io.github.bropines.Glossa.desktop
#   usr/share/icons/hicolor/256x256/apps/io.github.bropines.Glossa.png
#   usr/share/metainfo/io.github.bropines.Glossa.metainfo.xml
#                                  AppStream metadata, for software centres
#   usr/share/doc/glossa/          README, CHANGELOG, licences, NOTICE and
#                                  THIRD-PARTY-NOTICES.txt
#   usr/share/glossa/sdk/python/   glossa_module.py; Glossa looks in
#                                  <exe>/../share/glossa/sdk/python
#   usr/share/glossa/examples/modules/{js,python}/
#
# The examples are not under usr/share/glossa/modules on purpose: Glossa
# scans that folder for live modules.
#
# Used by build-deb.sh and build-appimage.sh.
#
#   packaging/linux/stage.sh <folder with glossa and glossa-cli> <root>
#
# Environment:
#   THIRD_PARTY_NOTICES  the notices for these binaries' target, written by
#                        scripts/third-party-notices.py. A package without
#                        them does not meet its dependencies' licences, so
#                        the script stops unless ALLOW_NO_NOTICES=1 says a
#                        local try-out is meant.
set -eu
[ $# -eq 2 ] || { echo "usage: $0 <bin-dir> <root>" >&2; exit 64; }
bin=$1
root=$2
repo=$(cd "$(dirname "$0")/../.." && pwd)

for f in glossa glossa-cli; do
  [ -x "$bin/$f" ] || { echo "$bin/$f is missing or not executable" >&2; exit 1; }
done

notices=${THIRD_PARTY_NOTICES:-}
if [ -z "$notices" ] && [ "${ALLOW_NO_NOTICES:-}" != 1 ]; then
  echo "THIRD_PARTY_NOTICES is not set (scripts/third-party-notices.py --target <triple> -o <file>);" >&2
  echo "set ALLOW_NO_NOTICES=1 to stage without them for a local try-out" >&2
  exit 1
fi
[ -z "$notices" ] || [ -f "$notices" ] || { echo "$notices does not exist" >&2; exit 1; }

share="$root/usr/share"
mkdir -p "$root/usr/bin" "$share/applications" "$share/icons/hicolor/256x256/apps" "$share/metainfo" \
  "$share/doc/glossa" "$share/glossa/sdk" "$share/glossa/examples/modules"

install -m 0755 "$bin/glossa" "$bin/glossa-cli" "$root/usr/bin/"
install -m 0644 "$repo/assets/linux/io.github.bropines.Glossa.desktop" "$share/applications/"
install -m 0644 "$repo/assets/icons/glossa.png" "$share/icons/hicolor/256x256/apps/io.github.bropines.Glossa.png"
install -m 0644 "$repo/README.md" "$repo/CHANGELOG.md" "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" \
  "$repo/NOTICE" "$share/doc/glossa/"
[ -z "$notices" ] || install -m 0644 "$notices" "$share/doc/glossa/THIRD-PARTY-NOTICES.txt"
install -m 0644 "$repo/assets/linux/io.github.bropines.Glossa.metainfo.xml" "$share/metainfo/"
cp -R "$repo/sdk/python" "$share/glossa/sdk/"
cp -R "$repo/modules/js" "$repo/modules/python" "$share/glossa/examples/modules/"

# No caches from local runs, and plain permissions whatever the umask or the
# checkout's file modes.
find "$root" -name __pycache__ -prune -exec rm -rf {} +
find "$root/usr" -type d -exec chmod 0755 {} +
find "$share" -type f -exec chmod 0644 {} +
