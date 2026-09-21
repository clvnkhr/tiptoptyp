#!/bin/sh
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
    echo "install-macos-app.sh requires macOS" >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
applications_dir=${TIPTOPTYP_APPLICATIONS_DIR:-/Applications}
app_name=tiptoptyp.app
source_app="$root_dir/target/release/$app_name"
installed_app="$applications_dir/$app_name"

if ! command -v cargo-packager >/dev/null 2>&1; then
    echo "cargo-packager is required; install it with: cargo install cargo-packager --locked" >&2
    exit 1
fi

echo "Building the release app bundle"
(cd "$root_dir" && cargo packager --release)

echo "Verifying the packaged app"
(cd "$root_dir" && cargo run --manifest-path xtask/Cargo.toml -- verify-package)

if [ ! -d "$source_app" ]; then
    echo "packager did not produce $source_app" >&2
    exit 1
fi

if pgrep -f -- "$installed_app/Contents/MacOS/tiptoptyp" >/dev/null 2>&1; then
    echo "Closing the previously installed tiptoptyp"
    osascript -e 'tell application id "dev.tiptoptyp.editor" to quit'
    i=0
    while pgrep -f -- "$installed_app/Contents/MacOS/tiptoptyp" >/dev/null 2>&1; do
        i=$((i + 1))
        if [ "$i" -ge 20 ]; then
            echo "the previously installed tiptoptyp did not quit" >&2
            exit 1
        fi
        sleep 0.25
    done
fi

mkdir -p "$applications_dir"
if [ -e "$installed_app" ]; then
    rm -rf -- "$installed_app"
fi
ditto --rsrc --extattr "$source_app" "$installed_app"

echo "Installed $installed_app"
open "$installed_app"

i=0
while ! pgrep -f -- "$installed_app/Contents/MacOS/tiptoptyp" >/dev/null 2>&1; do
    i=$((i + 1))
    if [ "$i" -ge 20 ]; then
        echo "tiptoptyp did not start from $installed_app" >&2
        exit 1
    fi
    sleep 0.25
done

echo "Running $installed_app"
