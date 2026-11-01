#!/usr/bin/env sh
# Build the Glossa module SDK for Java with nothing but a JDK (17 or newer).
#
#   ./build.sh          -> build/glossa-module-sdk.jar
#   ./build.sh test     -> the jar, then the SDK's own tests
#
# Uses $JAVA_HOME/bin when JAVA_HOME is set, else javac/jar/java from PATH.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/build"
bin=""
if [ -n "${JAVA_HOME:-}" ]; then bin="$JAVA_HOME/bin/"; fi

rm -rf "$out/classes" "$out/test-classes"
mkdir -p "$out/classes"
# An argument file keeps long source lists and paths with spaces working.
find "$here/src" -name '*.java' | sed 's/.*/"&"/' > "$out/sources.txt"
"${bin}javac" --release 17 -encoding UTF-8 -Xlint:all -d "$out/classes" @"$out/sources.txt"
"${bin}jar" --create --file "$out/glossa-module-sdk.jar" -C "$out/classes" .
echo "built $out/glossa-module-sdk.jar"

if [ "${1:-}" = "test" ]; then
    mkdir -p "$out/test-classes"
    find "$here/test" -name '*.java' | sed 's/.*/"&"/' > "$out/test-sources.txt"
    "${bin}javac" --release 17 -encoding UTF-8 -cp "$out/classes" -d "$out/test-classes" @"$out/test-sources.txt"
    sep=":"
    case "$(uname -s 2>/dev/null)" in MINGW*|MSYS*|CYGWIN*) sep=";" ;; esac
    "${bin}java" -cp "$out/classes$sep$out/test-classes" io.github.bropines.glossa.SdkTest
fi
