#!/bin/sh
# Put Glossa in this user's application menu: the menu entry and its icon
# (desktops and the portal's shortcut settings use them to name the app).
# Run from the unpacked release folder, or pass the folder holding `glossa`.
set -e
DIR=$(cd "${1:-$(dirname "$0")}" && pwd)
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
mkdir -p "$APPS" "$ICONS"
sed "s|^Exec=glossa|Exec=\"$DIR/glossa\"|" "$DIR/io.github.bropines.Glossa.desktop" > "$APPS/io.github.bropines.Glossa.desktop"
cp "$DIR/glossa.png" "$ICONS/io.github.bropines.Glossa.png"
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
echo "Glossa is in the application menu now ($APPS/io.github.bropines.Glossa.desktop)."
