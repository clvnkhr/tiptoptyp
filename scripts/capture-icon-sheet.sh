#!/usr/bin/env bash
# Use the production icon painters and the native framebuffer capture path.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked
binary="${CARGO_TARGET_DIR:-target}/release/tiptoptyp"
mkdir -p .tiptoptyp/screenshots/icon-audit docs/icons
log=$(mktemp)
trap 'rm -f "$log"' EXIT
LC_ALL=C perl -e 'alarm 45; exec @ARGV; die "Cannot launch icon capture: $!\n";' \
  "$binary" --ui-theme catppuccin-latte --ui-snapshot-scene icons \
  --ui-screenshot-subdir screenshots/icon-audit --ui-screenshot-settle 12 \
  --ui-screenshot-exit docs/ui-snapshots/theme-fixture.typ 2>&1 | tee "$log"
image=$(sed -n 's/^UI screenshot saved to //p' "$log" | tail -n 1)
test -n "$image" && test -s "$image"
cp "$image" docs/icons/contact-sheet.png
printf 'Updated docs/icons/contact-sheet.png\n'
