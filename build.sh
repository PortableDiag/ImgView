#!/usr/bin/env bash
# Build the optimized single binary and copy it next to this script.
# Result: ./imgview  (copy it to any Linux box with the usual GL/X11/Wayland
# libs present — no Rust, no runtime needed).
set -euo pipefail
. "$HOME/.cargo/env"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/imgview-rs-target}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

cargo build --release
# Copy out of the exfat-unfriendly target dir so the binary sits with the source.
cp "$CARGO_TARGET_DIR/release/imgview" "$DIR/imgview"
chmod +x "$DIR/imgview"
echo
echo "Binary: $DIR/imgview   ($(du -h "$DIR/imgview" | cut -f1))"
