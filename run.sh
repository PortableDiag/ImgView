#!/usr/bin/env bash
# Dev run. Keeps the build output in $HOME because /media/veracrypt1 is exfat
# (no symlinks / exec bits), which cargo's target/ dir needs.
set -euo pipefail
. "$HOME/.cargo/env"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/imgview-rs-target}"
cd "$(dirname "${BASH_SOURCE[0]}")"
exec cargo run --release -- "$@"
