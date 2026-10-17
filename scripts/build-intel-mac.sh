#!/bin/sh
# Cross-compile tiptoptyp for Intel Macs (x86_64-apple-darwin) from any Mac.
#
# Usage: scripts/build-intel-mac.sh [--release]
#
# Verifies that the workspace compiles for Intel Macs and that the produced
# binary is actually x86_64. This covers compilation only. To build a complete
# Intel app instead, use scripts/install-macos-app.sh --target x86_64-apple-darwin.
# The installer defaults to a universal app when no target is supplied.
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
    echo "build-intel-mac.sh requires macOS (it cross-compiles with the Xcode SDK)" >&2
    exit 1
fi

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [--release]" >&2
    exit 2
fi

mode="debug"
if [ "${1:-}" = "--release" ]; then
    mode="release"
elif [ -n "${1:-}" ]; then
    echo "usage: $0 [--release]" >&2
    exit 2
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
target_dir="${CARGO_TARGET_DIR:-$root_dir/target}"
# Cargo runs from the repository even when this script is invoked elsewhere.
# Use that same absolute destination for compilation and verification.
case "$target_dir" in
    /*) ;;
    *) target_dir="$root_dir/$target_dir" ;;
esac

# The repository pins its Rust toolchain; Homebrew Cargo cannot see
# rust-toolchain.toml targets, so drive the build through rustup with the
# pinned toolchain, which carries the x86_64 standard library.
toolchain=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$root_dir/rust-toolchain.toml" | head -n 1)
if [ -z "$toolchain" ]; then
    toolchain=$(rustup show active-toolchain 2>/dev/null | awk '{print $1}')
fi
if [ -z "$toolchain" ]; then
    echo "could not determine the Rust toolchain" >&2
    exit 1
fi

if ! rustup target list --toolchain "$toolchain" --installed 2>/dev/null | grep -qx "x86_64-apple-darwin"; then
    echo "Installing the x86_64-apple-darwin target for $toolchain"
    rustup target add --toolchain "$toolchain" x86_64-apple-darwin
fi

# Cargo resolves rustc from PATH (Homebrew here, which lacks the Intel
# target); the RUSTC_WRAPPER cache forwards whatever it is given, so point
# RUSTC at the toolchain compiler explicitly.
export RUSTC
RUSTC=$(rustup which --toolchain "$toolchain" rustc)

echo "Compiling tiptoptyp for x86_64-apple-darwin ($mode)"
if [ "$mode" = "release" ]; then
    (cd "$root_dir" && rustup run "$toolchain" cargo build --locked --target-dir "$target_dir" --release --target x86_64-apple-darwin)
    binary="$target_dir/x86_64-apple-darwin/release/tiptoptyp"
else
    (cd "$root_dir" && rustup run "$toolchain" cargo build --locked --target-dir "$target_dir" --target x86_64-apple-darwin)
    binary="$target_dir/x86_64-apple-darwin/debug/tiptoptyp"
fi

if [ ! -x "$binary" ]; then
    echo "expected binary missing: $binary" >&2
    exit 1
fi
arch=$(lipo -info "$binary" 2>/dev/null || file -b "$binary")
echo "Built: $binary"
echo "Architecture: $arch"
case "$arch" in
    *x86_64*)
        ;;
    *)
        echo "binary is not x86_64: $arch" >&2
        exit 1
        ;;
esac

if arch -x86_64 true 2>/dev/null; then
    echo "Running the Intel binary under Rosetta"
    arch -x86_64 "$binary" --version
else
    echo "Rosetta unavailable; skipping the execution smoke test" >&2
fi

cat >&2 <<'EOF'
NOTE: compilation only. For a complete Intel app with the matching helpers
and PDFium, use scripts/install-macos-app.sh --target x86_64-apple-darwin.
The default installer builds a universal app for Apple Silicon and Intel.
EOF
