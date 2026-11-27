#!/bin/sh
# Build glossa-<version>-<arch>.AppImage from release binaries.
#
#   packaging/linux/build-appimage.sh <folder with glossa and glossa-cli> <version> [out-dir]
#
# Environment:
#   APPIMAGETOOL     appimagetool (default: appimagetool on PATH)
#   APPIMAGE_RUNTIME the type 2 runtime file to embed, for APPIMAGE_ARCH;
#                    without it appimagetool downloads the latest one
#   APPIMAGE_ARCH    the binaries' architecture: x86_64 (default) or aarch64.
#                    appimagetool itself runs on the build machine, so an
#                    aarch64 image can be made on x86-64
#   APPIMAGE_UPDATE_INFO  update information to embed, e.g.
#                    gh-releases-zsync|bropines|glossa|latest|glossa-*-x86_64.AppImage.zsync;
#                    then <image>.zsync is written beside the image, for
#                    AppImageUpdate and the like to fetch only what changed.
#                    Needs zsyncmake (Debian: zsync); without it the script
#                    stops rather than embed information nothing answers
#   APPIMAGE_LIBDIR  where this machine keeps the target's libraries
#                    (default: /usr/lib/<arch>-linux-gnu) and
#   APPIMAGE_DOCDIR  their packages' copyright files (default:
#                    /usr/share/doc), for the fallback libraries below
#   THIRD_PARTY_NOTICES, ALLOW_NO_NOTICES  see stage.sh
#
# GTK 3, ALSA and glibc come from the system, as for the tarball: every
# desktop has them, and a bundled GTK fights the system's themes and input
# methods. The glibc symbols the build used must exist on the target, so
# build on an old system (the release uses Debian 12).
#
# One library is not on every desktop: libxkbcommon-x11, which the window
# toolkit opens at run time for the keyboard on X11 and without which no
# window opens (a plain Ubuntu 22.04 lacks it). The image carries it, and
# libxcb-xkb, which only it needs, in usr/lib/glossa-fallback. AppRun adds
# that folder only where the system has no copy: the system's one matches
# the system's libxkbcommon, which GTK loads either way.
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
arch=${APPIMAGE_ARCH:-x86_64}

mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/Glossa.AppDir"

"$here/stage.sh" "$bin" "$appdir"
cp "$appdir/usr/share/applications/io.github.bropines.Glossa.desktop" "$appdir/"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/io.github.bropines.Glossa.png" "$appdir/"
ln -s io.github.bropines.Glossa.png "$appdir/.DirIcon"

triplet=$arch-linux-gnu
libdir=${APPIMAGE_LIBDIR:-/usr/lib/$triplet}
docdir=${APPIMAGE_DOCDIR:-/usr/share/doc}
fallback="$appdir/usr/lib/glossa-fallback"
mkdir -p "$fallback"
for lib in libxkbcommon-x11.so.0:libxkbcommon-x11-0 libxcb-xkb.so.1:libxcb-xkb1; do
  file=${lib%%:*}
  package=${lib##*:}
  [ -e "$libdir/$file" ] || {
    echo "$libdir/$file is missing: install $package for $arch, or point APPIMAGE_LIBDIR at the target's libraries" >&2
    exit 1
  }
  # -L: the file the versioned name points to, under the name that is opened.
  cp -L "$libdir/$file" "$fallback/$file"
  chmod 0644 "$fallback/$file"
  # Both are under MIT-style licences, which ask for the notice to travel
  # with the copy.
  [ -f "$docdir/$package/copyright" ] || { echo "$docdir/$package/copyright is missing" >&2; exit 1; }
  install -m 0644 "$docdir/$package/copyright" "$appdir/usr/share/doc/glossa/copyright-$package"
done

# The triplet is the one thing written into AppRun; the rest is literal.
printf '#!/bin/sh\ntriplet=%s\n' "$triplet" > "$appdir/AppRun"
cat >> "$appdir/AppRun" <<'EOF'
here=$(dirname "$(readlink -f "$0")")
if [ "${1:-}" = cli ]; then
  shift
  exec "$here/usr/bin/glossa-cli" "$@"
fi
# The carried libxkbcommon-x11, only where the system has none (see
# build-appimage.sh).
system_copy=
for dir in "/usr/lib/$triplet" "/lib/$triplet" /usr/lib64 /lib64 /usr/lib /lib /usr/local/lib; do
  if [ -e "$dir/libxkbcommon-x11.so.0" ]; then
    system_copy=$dir
    break
  fi
done
if [ -z "$system_copy" ]; then
  LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$here/usr/lib/glossa-fallback"
  export LD_LIBRARY_PATH
fi
exec "$here/usr/bin/glossa" "$@"
EOF
chmod 0755 "$appdir/AppRun"

image="$out/glossa-$version-$arch.AppImage"
set -- --no-appstream
[ -n "${APPIMAGE_RUNTIME:-}" ] && set -- "$@" --runtime-file "$APPIMAGE_RUNTIME"
update=${APPIMAGE_UPDATE_INFO:-}
if [ -n "$update" ]; then
  # appimagetool leaves the .zsync to zsyncmake and only warns when it is
  # missing; an image that points at a .zsync the release lacks would make
  # updaters fail, so check first.
  command -v zsyncmake >/dev/null || { echo "APPIMAGE_UPDATE_INFO needs zsyncmake (Debian: zsync)" >&2; exit 1; }
  set -- "$@" -u "$update"
fi
# The tool is itself an AppImage; extracting it to run needs no FUSE, which
# containers lack. Run in the output folder: zsyncmake writes the .zsync to
# the current one, and names the image inside it by its bare file name, so
# the two must sit side by side in the release.
(cd "$out" && APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch "$tool" "$@" "$appdir" "$image")
if [ -n "$update" ]; then
  [ -s "$image.zsync" ] || { echo "$image.zsync was not written" >&2; exit 1; }
  echo "$image.zsync"
fi
echo "$image"
