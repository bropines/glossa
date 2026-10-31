#!/usr/bin/env bash
# Run cargo inside WSL with the target directory on the Linux filesystem.
# Building through /mnt/<drive> is several times slower, so only the sources
# live on the Windows side.
#
#   scripts/wsl-cargo.sh test -p glossa-core
#   scripts/wsl-cargo.sh build --release
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/glossa-target}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-never}"
cd "$here"
exec cargo "$@"
