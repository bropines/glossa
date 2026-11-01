#!/bin/sh
# Fill the winget and Scoop templates for a release, from the files the
# release uploads.
#
#   packaging/fill-manifests.sh <version> <dist-dir> [out-dir]
#
# Reads <dist-dir>/glossa-<version>-{x86_64,arm64}-windows-setup.exe and
# glossa-<version>-{x86_64,arm64}-windows-portable.zip, writes Bropines.Glossa*.yaml and
# glossa.json to out-dir (default: dist-dir). The URLs point at the GitHub
# release of tag v<version> in $GITHUB_REPOSITORY (default bropines/glossa),
# so they only work once that release is published in a public repository.
set -eu
[ $# -ge 2 ] || { echo "usage: $0 <version> <dist-dir> [out-dir]" >&2; exit 64; }
version=$1
dist=$2
out=${3:-$dist}
here=$(cd "$(dirname "$0")" && pwd)
repo=${GITHUB_REPOSITORY:-bropines/glossa}
base="https://github.com/$repo/releases/download/v$version"

setup="glossa-$version-x86_64-windows-setup.exe"
zip="glossa-$version-x86_64-windows-portable.zip"
arm_setup="glossa-$version-arm64-windows-setup.exe"
arm_zip="glossa-$version-arm64-windows-portable.zip"
sha() { sha256sum "$dist/$1" | cut -d' ' -f1; }
setup_sha=$(sha "$setup")
zip_sha=$(sha "$zip")
arm_setup_sha=$(sha "$arm_setup")
arm_zip_sha=$(sha "$arm_zip")

mkdir -p "$out"
fill() { # template, output
  sed -e "s|@VERSION@|$version|g" -e "s|@REPO@|$repo|g" \
    -e "s|@SETUP_URL@|$base/$setup|g" -e "s|@SETUP_SHA256@|$setup_sha|g" \
    -e "s|@ZIP_URL@|$base/$zip|g" -e "s|@ZIP_SHA256@|$zip_sha|g" \
    -e "s|@ARM_SETUP_URL@|$base/$arm_setup|g" -e "s|@ARM_SETUP_SHA256@|$arm_setup_sha|g" \
    -e "s|@ARM_ZIP_URL@|$base/$arm_zip|g" -e "s|@ARM_ZIP_SHA256@|$arm_zip_sha|g" "$1" > "$2"
  if grep -n '@[A-Z0-9_]*@' "$2"; then
    echo "$2: placeholders left" >&2
    exit 1
  fi
  echo "$2"
}
for f in Bropines.Glossa.yaml Bropines.Glossa.installer.yaml Bropines.Glossa.locale.en-US.yaml; do
  fill "$here/winget/$f" "$out/$f"
done
fill "$here/scoop/glossa.json" "$out/glossa.json"
