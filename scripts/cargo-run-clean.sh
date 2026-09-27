#!/bin/sh
# Cargo runner used by .cargo/config.toml on macOS.
set -u

if [ "$#" -lt 1 ]; then
    echo "cargo-run-clean: missing executable" >&2
    exit 64
fi

executable=$1
shift
"$executable" "$@"
status=$?

if [ "${TIPTOPTYP_KEEP_CARGO_CACHE:-0}" != "1" ]; then
    script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    repository_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
    keep=${TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS:-1}
    prune() {
        if ! python3 "$script_dir/prune_cargo_incremental.py" \
            --quiet --keep "$keep" "$@"; then
            echo "cargo-run-clean: cache pruning failed; preserving cargo status $status" >&2
        fi
    }
    prune "$repository_root/target" "$repository_root/xtask/target"
    if [ -n "${CARGO_TARGET_DIR:-}" ]; then
        case "$CARGO_TARGET_DIR" in
            /*) prune "$CARGO_TARGET_DIR" ;;
            *) prune "$(pwd)/$CARGO_TARGET_DIR" ;;
        esac
    fi
fi

exit "$status"
