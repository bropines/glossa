#!/bin/sh
# The numbers-only forms of a Glossa version, for the fields that cannot
# hold "0.1.0-beta.2" (see packaging/AGENTS.md, "Versions").
#
#   scripts/version-numbers.sh [VERSION]     default: the workspace version
#
# prints, for 0.1.0-beta.2:
#
#   version=0.1.0-beta.2
#   core=0.1.0            major.minor.patch
#   build=2002            grows within one major.minor.patch: 1000+n for
#                         -alpha.n, 2000+n for -beta.n, 3000+n for -rc.n,
#                         9000 for the release, n for any other tag
#   windows=0.1.0.2002    a Windows file version (FILEVERSION, Inno Setup's
#                         VersionInfoVersion)
#   macos=1002002         CFBundleVersion: one number that grows with every
#                         release, (major*10000 + minor*100 + patch)*10000
#                         + build
#
# The same rule is in packaging/windows/version_resource.rs for the build
# scripts; crates/glossa-cli/tests/version_numbers.rs keeps the two equal.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
version=${1:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)}
[ -n "$version" ] || { echo "no version given and none found in Cargo.toml" >&2; exit 1; }

core=${version%%[-+]*}
case "$core" in
  *[!0-9.]* | *.*.*.* | .* | *. | *..*) echo "$version is not major.minor.patch[-tag.n]" >&2; exit 1 ;;
  *.*.*) ;;
  *) echo "$version is not major.minor.patch[-tag.n]" >&2; exit 1 ;;
esac
major=${core%%.*}
rest=${core#*.}
minor=${rest%%.*}
patch=${rest#*.}

case "$version" in
  *-*)
    pre=${version#*-}
    pre=${pre%%+*}
    tag=${pre%%.*}
    case "$pre" in *.*) n=${pre#*.} ;; *) n=0 ;; esac
    case "$n" in '' | *[!0-9]*) n=0 ;; esac
    [ "$n" -le 999 ] || n=999
    case "$tag" in
      alpha) stage=1000 ;;
      beta) stage=2000 ;;
      rc) stage=3000 ;;
      *) stage=0 ;;
    esac
    build=$((stage + n))
    ;;
  *) build=9000 ;;
esac

echo "version=$version"
echo "core=$core"
echo "build=$build"
echo "windows=$core.$build"
echo "macos=$(( (major * 10000 + minor * 100 + patch) * 10000 + build ))"
