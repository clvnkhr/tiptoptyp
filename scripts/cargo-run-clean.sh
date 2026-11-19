#!/bin/sh
# Cargo's macOS runner: preserve the child result, then safely trim old caches.
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
    if script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) &&
       repository_root=$(CDPATH= cd -- "$script_dir/.." && pwd); then
        set -- "$repository_root/target" "$repository_root/xtask/target"
        # Cargo changes test working directories, and --target-dir can override
        # CARGO_TARGET_DIR. Let the pruner validate the actual artifact's root.
        if ! python3 "$script_dir/prune_cargo_incremental.py" \
            --quiet --keep "${TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS:-1}" \
            --cargo-executable "$executable" -- "$@"; then
            echo "cargo-run-clean: cache pruning failed; preserving cargo status $status" >&2
        fi
    else
        echo "cargo-run-clean: cache pruning failed to locate repository; preserving cargo status $status" >&2
    fi
fi

exit "$status"
