#!/usr/bin/env python3
"""Remove obsolete Cargo incremental sessions without touching build outputs."""

from __future__ import annotations

import argparse
try:
    import fcntl
except ImportError:  # pragma: no cover - only non-Unix hosts lack fcntl.
    fcntl = None
import shutil
from pathlib import Path


def lock_is_held(session: Path) -> bool:
    """Return whether another Cargo process currently owns a session lock."""

    if fcntl is None:
        return False
    for lock in session.rglob("*.lock"):
        try:
            with lock.open("a+b") as handle:
                try:
                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    return True
                finally:
                    try:
                        fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
                    except OSError:
                        pass
        except OSError:
            # A session disappearing during a concurrent build is harmless;
            # leave it alone for this pass.
            return True
    return False


def prune_target(target: Path, keep: int = 1, dry_run: bool = False) -> tuple[int, int, int]:
    """Prune old sessions below *target* and return (removed, bytes, active)."""

    if keep < 1:
        raise ValueError("keep must be at least one")
    if not target.is_dir():
        return 0, 0, 0

    removed = removed_bytes = active = 0
    for incremental in target.rglob("incremental"):
        if not incremental.is_dir() or incremental.is_symlink():
            continue
        try:
            crates = list(incremental.iterdir())
        except OSError:
            continue
        for crate in crates:
            if not crate.is_dir() or crate.is_symlink():
                continue
            try:
                sessions = [
                    path
                    for path in crate.iterdir()
                    if path.is_dir() and not path.is_symlink() and path.name.startswith("s-")
                ]
                sessions.sort(
                    key=lambda path: (path.stat().st_mtime_ns, path.name), reverse=True
                )
            except OSError:
                continue
            for session in sessions[keep:]:
                if lock_is_held(session):
                    active += 1
                    continue
                try:
                    size = sum(
                        path.stat().st_size for path in session.rglob("*") if path.is_file()
                    )
                except OSError:
                    continue
                if not dry_run:
                    try:
                        shutil.rmtree(session)
                    except OSError:
                        continue
                removed += 1
                removed_bytes += size
    return removed, removed_bytes, active


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("targets", nargs="+", type=Path)
    parser.add_argument("--keep", type=int, default=1)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    if args.keep < 1:
        parser.error("--keep must be at least one")

    totals = [0, 0, 0]
    for target in args.targets:
        result = prune_target(target.resolve(), args.keep, args.dry_run)
        totals = [left + right for left, right in zip(totals, result)]
    if not args.quiet:
        action = "would remove" if args.dry_run else "removed"
        print(
            f"cargo incremental cache: {action} {totals[0]} old session(s), "
            f"{totals[1] / 1024**2:.1f} MiB; kept {totals[2]} active session(s)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
