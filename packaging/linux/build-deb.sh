#!/bin/sh
# Build glossa_<version>_<arch>.deb from release binaries.
#
#   packaging/linux/build-deb.sh <folder with glossa and glossa-cli> <version> [out-dir]
#
# e.g. packaging/linux/build-deb.sh target/x86_64-unknown-linux-gnu/release 0.1.0 dist
#
# Needs dpkg-deb, and dpkg-shlibdeps (package dpkg-dev, part of
# build-essential) to work out the library dependencies. Build on the oldest
# system the package should install on (the release builds on Debian 12).
#
# Environment:
#   DEB_ARCH        the package architecture (default: dpkg --print-architecture)
#   DEB_MAINTAINER  the Maintainer field
set -eu
[ $# -ge 2 ] || { echo "usage: $0 <bin-dir> <version> [out-dir]" >&2; exit 64; }
bin=$(cd "$1" && pwd)
version=$2
out=${3:-dist}
here=$(cd "$(dirname "$0")" && pwd)
arch=${DEB_ARCH:-$(dpkg --print-architecture)}
maintainer=${DEB_MAINTAINER:-"Bropines <bropines@users.noreply.github.com>"}

# 1.2.3-rc.1 -> 1.2.3~rc.1, so a pre-release sorts before its release.
deb_version=$(printf '%s' "$version" | sed 's/-/~/')

mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# debian/glossa, the folder dpkg-shlibdeps expects a package tree in.
root="$work/debian/glossa"

"$here/stage.sh" "$bin" "$root"

# Debian wants a copyright file in the doc folder; ours points at the
# licence texts installed beside it.
cat > "$root/usr/share/doc/glossa/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Glossa
Source: https://github.com/bropines/glossa

Files: *
Copyright: Glossa contributors
License: MIT or Apache-2.0
 The full texts are in /usr/share/doc/glossa/LICENSE-MIT and
 /usr/share/doc/glossa/LICENSE-APACHE. Third-party notices are in
 /usr/share/doc/glossa/NOTICE.
EOF
chmod 0644 "$root/usr/share/doc/glossa/copyright"

# Libraries the binaries link: dpkg-shlibdeps reads them off the ELF files
# and names the packages that provide them on this system. It insists on a
# debian/control in the current folder, and finds the package tree by its
# DEBIAN folder.
mkdir -p "$root/DEBIAN"
printf 'Source: glossa\n\nPackage: glossa\nArchitecture: any\n' > "$work/debian/control"
linked=$(cd "$work" && dpkg-shlibdeps -O debian/glossa/usr/bin/glossa debian/glossa/usr/bin/glossa-cli |
  sed -n 's/^shlibs:Depends=//p')
[ -n "$linked" ] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
# Opened at run time with dlopen, so invisible to dpkg-shlibdeps: OpenGL
# (EGL or GLX) for the windows, and xkbcommon-x11 for the keyboard on X11.
# X11, Wayland and xkbcommon themselves come with GTK.
depends="$linked, libegl1 | libgl1, libxkbcommon-x11-0"

installed_size=$(du -sk "$root/usr" | cut -f1)
cat > "$root/DEBIAN/control" <<EOF
Package: glossa
Version: $deb_version
Architecture: $arch
Maintainer: $maintainer
Installed-Size: $installed_size
Depends: $depends
Recommends: tesseract-ocr, libayatana-appindicator3-1 | libappindicator3-1
Suggests: python3, tesseract-ocr-rus
Section: utils
Priority: optional
Homepage: https://github.com/bropines/glossa
Description: popup translator with OCR and dictionaries
 Glossa translates the selected text in any program in a popup by the
 pointer, recognises text in a region of the screen (Google Lens,
 Tesseract or any command) and looks words up in online and offline
 dictionaries. It sits in the tray and works with many translation
 services, including any OpenAI-compatible API. A clean-room successor to
 QTranslate.
 .
 glossa is the desktop program; glossa-cli does the same from a terminal.
EOF
(cd "$root" && find usr -type f -exec md5sum {} + | sort -k2) > "$root/DEBIAN/md5sums"
chmod 0644 "$root/DEBIAN/control" "$root/DEBIAN/md5sums"

# The file keeps the tag's spelling: GitHub renames release assets with a ~.
deb="$out/glossa_${version}_${arch}.deb"
dpkg-deb --root-owner-group -Zxz --build "$root" "$deb"
echo "$deb"
