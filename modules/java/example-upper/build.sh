#!/usr/bin/env sh
# Build module.jar: this module's sources and the Java SDK's, compiled
# together into one runnable jar (java -jar module.jar). Needs a JDK 17+.
#
# GLOSSA_JAVA_SDK points at the SDK's folder (the one holding src/) when this
# module lives outside the Glossa repository.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk="${GLOSSA_JAVA_SDK:-$here/../../../sdk/java}"
bin=""
if [ -n "${JAVA_HOME:-}" ]; then bin="$JAVA_HOME/bin/"; fi
if [ ! -d "$sdk/src" ]; then
    echo "the Java SDK was not found at $sdk; set GLOSSA_JAVA_SDK" >&2
    exit 1
fi

rm -rf "$here/build"
mkdir -p "$here/build/classes"
find "$here/src" "$sdk/src" -name '*.java' | sed 's/.*/"&"/' > "$here/build/sources.txt"
"${bin}javac" --release 17 -encoding UTF-8 -d "$here/build/classes" @"$here/build/sources.txt"
"${bin}jar" --create --file "$here/module.jar" --main-class example.ExampleUpper -C "$here/build/classes" .
echo "built $here/module.jar"
