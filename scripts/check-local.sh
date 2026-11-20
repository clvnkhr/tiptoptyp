#!/bin/sh
# Canonical local checks. CI mirrors these commands; it is not a prerequisite.
set -eu

usage() {
    printf '%s\n' 'Usage: scripts/check-local.sh [fast|rust|features|all]' \
        '  fast      Script/browser contracts; no Rust build, models or desktop (default).' \
        '  rust      Required formatting, strict lint, full Rust and packaging tests.' \
        '  features  Supplemental profiling/desktop compilation and owned contracts.' \
        '  all       Run fast, rust and features locally, stopping on first failure.'
}

mode=${1-fast}
if [ "$#" -gt 1 ]; then usage >&2; exit 64; fi
case "$mode" in
    fast|rust|features|all) ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 64 ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$script_dir/.."

fast() {
    printf '%s\n' 'Running fast local checks.'
    python3 -m unittest discover -s scripts -p 'test_*.py'
    node --test scripts/test_overleaf_bridge.cjs scripts/test-preview-navigation.mjs \
        scripts/test-preview-snapshot-buffer.mjs scripts/test-preview-zoom-snapshot.mjs \
        scripts/test-desktop-preview-probe.mjs scripts/test-preview-native-probe.mjs
    node scripts/test-preview-e2e.cjs --large --self-test
    python3 scripts/test-preview-partial-native.py --self-test
}

rust() {
    printf '%s\n' 'Running required local Rust checks.'
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked --no-fail-fast
    cargo test --locked --manifest-path xtask/Cargo.toml
}

feature_tests() {
    feature_set=$1
    shift
    if feature_output=$(cargo test --locked --features "$feature_set" --bin tiptoptyp \
        -- --format pretty --color never "$@" 2>&1); then
        printf '%s\n' "$feature_output"
    else
        feature_status=$?
        printf '%s\n' "$feature_output" >&2
        return "$feature_status"
    fi
    # libtest accepts a stale filter as a successful zero-test run. Every
    # requested owner must actually pass a test, including after refactors.
    for feature_owner do
        if ! printf '%s\n' "$feature_output" | grep -E "^test $feature_owner.* \.\.\. ok$" >/dev/null; then
            printf 'No passing feature tests for owner: %s\n' "$feature_owner" >&2
            return 1
        fi
    done
}

features() {
    printf '%s\n' 'Running local feature contracts.'
    cargo fmt --manifest-path xtask/Cargo.toml -- --check
    if [ "$(uname -s)" = Darwin ]; then
        cargo clippy --locked --all-targets --features profiling,desktop-ui-tests -- -D warnings
        feature_tests profiling,desktop-ui-tests performance:: desktop_test::tests
    else
        cargo clippy --locked --all-targets --features profiling -- -D warnings
        feature_tests profiling performance::
    fi
}

case "$mode" in
    fast) fast ;;
    rust) rust ;;
    features) features ;;
    all) fast; rust; features ;;
esac
