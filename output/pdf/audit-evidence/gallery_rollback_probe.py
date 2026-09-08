#!/usr/bin/env python3
"""Reproduce the audited gallery rollback failure using temporary files only.

Run from any directory with Python 3. The probe extracts the current transaction
functions from the repository script, never invokes its top-level runner, and
injects a failure on the second move. It does not touch maintained gallery PNGs.
"""

import hashlib
from pathlib import Path
import subprocess
import tempfile


def main() -> None:
    repository = Path(__file__).resolve().parents[3]
    source_path = repository / "scripts/capture-theme-gallery.sh"
    source_bytes = source_path.read_bytes()
    source = source_bytes.decode("utf-8")
    start = source.index('backup_directory=""\nrestore_outputs_on_exit=0\n')
    stop = source.index('\napp_binary=""\n', start)
    transaction_functions = source[start:stop]
    first_line = source[:start].count("\n") + 1
    last_line = source[:stop].count("\n")
    print("Input: scripts/capture-theme-gallery.sh")
    print("Input SHA-256:", hashlib.sha256(source_bytes).hexdigest())
    print(f"Exact extracted transaction region: lines {first_line}-{last_line}")
    print("Fault injection: fail the second mv before executing it")

    with tempfile.TemporaryDirectory(prefix="tiptoptyp-audit-gallery-") as temporary:
        root = Path(temporary)
        gallery = root / "gallery"
        backup = root / "backup"
        gallery.mkdir()
        backup.mkdir()
        (gallery / "first.png").write_bytes(b"original-first")
        (gallery / "second.png").write_bytes(b"original-second")

        shell = "set -euo pipefail\n" + transaction_functions + r'''
latest_directory="$1/gallery"
backup_directory="$1/backup"
expected_outputs=(first.png second.png)
move_count=0
mv() {
  move_count=$((move_count + 1))
  if (( move_count == 2 )); then
    echo 'injected second-move failure' >&2
    return 1
  fi
  command mv "$@"
}
trap cleanup EXIT
backup_requested_outputs
'''
        result = subprocess.run(
            ["bash", "-c", shell, "audit", str(root)],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        print("Script exit:", result.returncode)
        print("stderr:", result.stderr.strip())
        first_exists = (gallery / "first.png").exists()
        second_exists = (gallery / "second.png").exists()
        backup_exists = backup.exists()
        print("first original exists:", first_exists)
        print("second original exists:", second_exists)
        print("backup exists:", backup_exists)
        reproduced = (
            result.returncode != 0
            and not first_exists
            and second_exists
            and not backup_exists
        )
        print("Audited original-file-loss failure reproduced:", reproduced)
        if not reproduced:
            raise SystemExit("Current behavior no longer matches the audited failure")
    print("Maintained gallery untouched; temporary fixtures removed")


if __name__ == "__main__":
    main()
