#!/bin/sh
# Build glossa-<version>-x86_64.AppImage from release binaries.
#
#   packaging/linux/build-appimage.sh <folder with glossa and glossa-cli> <version> [out-dir]
#
# Environment:
#   APPIMAGETOOL     appimagetool (default: appimagetool on PATH)
#   APPIMAGE_RUNTIME the type 2 runtime file to embed; without it appimagetool
#                    downloads the latest one
#
# Only Glossa's own files go in. GTK 3, ALSA and glibc come from the system,
# as for the tarball: every desktop has them, and a bundled GTK fights the
# system's themes and input methods. The glibc symbols the build used must
# exist on the target, so build on an old system (the release uses Debian 12).
#
# Inside the image the layout is the .deb's (see stage.sh), so Glossa finds
# its Python SDK in usr/share/glossa. `Glossa.AppImage cli <args>` runs
# glossa-cli.
set -eu
[ $# -ge 2 ] || { echo "usage: $0 <bin-dir> <version> [out-dir]" >&2; exit 64; }
bin=$(cd "$1" && pwd)
version=$2
out=${3:-dist}
here=$(cd "$(dirname "$0")" && pwd)
tool=${APPIMAGETOOL:-appimagetool}

mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/Glossa.AppDir"

"$here/stage.sh" "$bin" "$appdir"
cp "$appdir/usr/share/applications/io.github.bropines.Glossa.desktop" "$appdir/"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/io.github.bropines.Glossa.png" "$appdir/"
ln -s io.github.bropines.Glossa.png "$appdir/.DirIcon"
cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
if [ "${1:-}" = cli ]; then
  shift
  exec "$here/usr/bin/glossa-cli" "$@"
fi
exec "$here/usr/bin/glossa" "$@"
EOF
chmod 0755 "$appdir/AppRun"

image="$out/glossa-$version-x86_64.AppImage"
set -- --no-appstream
[ -n "${APPIMAGE_RUNTIME:-}" ] && set -- "$@" --runtime-file "$APPIMAGE_RUNTIME"
# The tool is itself an AppImage; extracting it to run needs no FUSE, which
# containers lack.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" "$@" "$appdir" "$image"
echo "$image"
