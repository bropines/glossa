#!/usr/bin/env bash
# Screenshot smoke test: render every window `glossa --screenshot` knows,
# in a few themes, each with a throwaway data folder, and fail if any run
# crashes, hangs, draws a blank picture or shows a string with no text.
#
#   scripts/screenshot-smoke.sh path/to/glossa [OUT_DIR]
#
# Pictures land in OUT_DIR (default target/screenshots), one folder per
# theme. Nothing is compared pixel for pixel: fonts differ between systems.
# A debug build is best on Windows, where a release build has no console
# and its messages are lost (the exit codes still count).
#
# Offline: every service is `scripts/smoke-module`, a JavaScript module
# with canned answers, and the proxy points at a closed local port, so a
# request that slips through fails at once instead of reaching a service.
#
# GLOSSA_SMOKE_THEMES overrides the themes, separated by commas;
# GLOSSA_SMOKE_ONLY limits the windows to those whose names contain it.
set -u

bin=${1:?usage: screenshot-smoke.sh path/to/glossa [OUT_DIR]}
out=${2:-target/screenshots}
IFS=',' read -r -a themes <<< "${GLOSSA_SMOKE_THEMES:-Glossa Dark,Glossa Light,macOS Light}"

module=$(cd "$(dirname "$0")" && pwd)/smoke-module
targets=$("$bin" --screenshot-list | tr -d '\r') || { echo "cannot list the windows from $bin" >&2; exit 1; }
[ -n "$targets" ] || { echo "$bin listed no windows" >&2; exit 1; }

scratch=$(mktemp -d 2>/dev/null || mktemp -d -t glossa-smoke)
trap 'rm -rf "$scratch"' EXIT

describe() {
    case $1 in
        0) echo ok ;;
        2) echo "blank picture" ;;
        3) echo "timed out" ;;
        4) echo "string with no text in any language" ;;
        5) echo "could not write the picture" ;;
        6) echo "unknown window" ;;
        *) echo "crashed (exit $1)" ;;
    esac
}

# One theme's windows, one after another. Prints a line per run and
# appends each failure to $scratch/failed.
run_theme() {
    local theme=$1 lang=$2 slug home t file start code
    slug=$(printf '%s' "$theme" | tr 'A-Z ' 'a-z-')
    home="$scratch/$slug"
    mkdir -p "$home/modules" "$out/$slug"
    cp -R "$module" "$home/modules/smoke"
    cat > "$home/config.toml" <<EOF
[general]
ui_language = "$lang"
show_tray_hint = false
show_welcome = false

[appearance]
theme = "$theme"
follow_system_theme = false

[services]
order = ["smoke"]
enabled = ["smoke"]
active = "smoke"
dictionary_order = ["smoke"]
dictionary_enabled = ["smoke"]
dictionary_active = "smoke"
fail_over = false

[network]
max_retries = 0
timeout_secs = 3

[network.proxy]
mode = "manual"
scheme = "http"
host = "127.0.0.1"
port = 9

[updates]
check_automatically = false

[server]
enabled = false
EOF
    for t in $targets; do
        case $t in *"${GLOSSA_SMOKE_ONLY:-}"*) ;; *) continue ;; esac
        file="$out/$slug/${t//:/-}.png"
        start=$(date +%s)
        "$bin" --home "$home" --screenshot "$t" "$file" 2> "$home/err.txt"
        code=$?
        echo run >> "$scratch/runs"
        printf '%-14s %-24s %4ss  %s\n' "$slug" "$t" "$(( $(date +%s) - start ))" "$(describe $code)"
        if [ $code -ne 0 ]; then
            sed 's/^/    /' "$home/err.txt" | tail -n 20
            # Panics and warnings go to the log, not to stderr.
            [ -f "$home/cache/glossa.log" ] && grep -E "ERROR|WARN|panic" "$home/cache/glossa.log" | tail -n 15 | sed 's/^/    log: /'
            echo "$slug/$t: $(describe $code)" >> "$scratch/failed"
        elif [ ! -s "$file" ]; then
            echo "$slug/$t: no picture written" >> "$scratch/failed"
        fi
    done
}

# The themes run side by side: each run is a process of its own with its
# own data folder, and the picture is read from the window's own buffer,
# so windows overlapping on screen do not spoil it.
: > "$scratch/runs"
: > "$scratch/failed"
for i in "${!themes[@]}"; do
    # The second theme also shows the interface in Russian, so Cyrillic
    # text and the Russian pack's longer strings get looked at too.
    lang=en
    [ "$i" -eq 1 ] && lang=ru
    run_theme "${themes[$i]}" "$lang" > "$scratch/log-$i" 2>&1 &
done
wait
cat "$scratch"/log-*
runs=$(wc -l < "$scratch/runs" | tr -d ' ')

echo
if [ -s "$scratch/failed" ]; then
    echo "$(wc -l < "$scratch/failed" | tr -d ' ') of $runs runs failed:"
    sed 's/^/  /' "$scratch/failed"
    exit 1
fi
[ "$runs" -gt 0 ] || { echo "no window matched" >&2; exit 1; }
echo "all $runs runs passed; pictures in $out"
