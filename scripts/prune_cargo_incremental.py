#!/usr/bin/env python3
"""Prune old finalized Rust caches, retaining protected sessions."""

from __future__ import annotations

import argparse
from contextlib import closing, contextmanager
import os
from pathlib import Path
import re
import stat
import sys
import time

try:
    import fcntl
except ImportError:  # pragma: no cover - non-POSIX hosts retain every session.
    fcntl = None


# Rust 1.98.1's cache protocol uses a timestamp, nonce and finalized SVH.
# https://github.com/rust-lang/rust/blob/1.98.1/compiler/rustc_incremental/src/persist/fs.rs
FINALIZED_SESSION = re.compile(r"s-([0-9a-z]{10})-([0-9a-z]{7})-([0-9a-z]{25})\Z")
MIN_AGE_SECONDS = 10
MAX_PAYLOAD_DEPTH = 64
TARGET_TRIPLE = re.compile(r"[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}\Z")
CARGO_CACHE_SIGNATURE = b"Signature: 8a477f597d28d172789f06886806bc55\n"
ANCHORED_FILESYSTEM = (
    os.name == "posix"
    and hasattr(os, "O_NOFOLLOW")
    and hasattr(os, "O_DIRECTORY")
    and hasattr(os, "fwalk")
    and os.listdir in os.supports_fd
    and all(operation in os.supports_dir_fd for operation in (os.open, os.stat, os.unlink, os.rmdir))
    and os.stat in os.supports_follow_symlinks
)


def same_object(before: os.stat_result, after: os.stat_result) -> bool:
    return (before.st_dev, before.st_ino, stat.S_IFMT(before.st_mode)) == (
        after.st_dev, after.st_ino, stat.S_IFMT(after.st_mode)
    )


def unchanged(before: os.stat_result, after: os.stat_result) -> bool:
    return same_object(before, after) and before.st_ctime_ns == after.st_ctime_ns


def directory_flags() -> int:
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)


@contextmanager
def open_directory(name, parent: int | None = None, expected=None):
    """Retain one verified directory object, never following its final symlink."""
    handle = None
    admitted = False
    try:
        before = os.stat(name, dir_fd=parent, follow_symlinks=False)
        if not stat.S_ISDIR(before.st_mode) or (expected is not None and not unchanged(expected, before)):
            raise OSError("directory was replaced")
        handle = os.open(name, directory_flags(), dir_fd=parent)
        admitted = unchanged(before, os.fstat(handle))
    except OSError:
        pass
    try:
        yield handle if admitted else None
    finally:
        if handle is not None:
            os.close(handle)


def entries(directory: int) -> list[str]:
    try:
        return os.listdir(directory)
    except OSError:
        return []


def cargo_target_candidates(executable: Path) -> tuple[Path, ...]:
    """Consider only profile[/deps|examples]/artifact, with an optional triple."""
    try:
        executable = Path(os.path.abspath(executable))
        if not stat.S_ISREG(os.stat(executable, follow_symlinks=False).st_mode):
            return ()
    except OSError:
        return ()
    profiles = [executable.parent]
    if executable.parent.name in {"deps", "examples"}:
        # Those names can also be user-defined profiles, so consider both
        # interpretations and let the target's held cache marker admit it.
        profiles.append(executable.parent.parent)
    candidates = []
    for profile in profiles:
        target = profile.parent
        candidates.append(target)
        if TARGET_TRIPLE.fullmatch(target.name):
            candidates.append(target.parent)
    return tuple(dict.fromkeys(candidates))


def cargo_cache_marker(target: int) -> bool:
    """Verify a bounded, no-follow marker read in the retained target object."""
    handle = None
    try:
        handle = os.open(
            "CACHEDIR.TAG", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            | getattr(os, "O_CLOEXEC", 0), dir_fd=target,
        )
        return (
            stat.S_ISREG(os.fstat(handle).st_mode)
            and os.read(handle, len(CARGO_CACHE_SIGNATURE)) == CARGO_CACHE_SIGNATURE
        )
    except OSError:
        return False
    finally:
        if handle is not None:
            os.close(handle)


def incremental_directories(target: int):
    """Hold the profile/triple ancestors while yielding incremental handles."""
    for name in entries(target):
        with open_directory(name, target) as directory:
            if directory is None:
                continue
            with open_directory("incremental", directory) as direct:
                if direct is not None:
                    yield direct
            if not TARGET_TRIPLE.fullmatch(name):
                continue
            for profile_name in entries(directory):
                with open_directory(profile_name, directory) as profile:
                    if profile is None:
                        continue
                    with open_directory("incremental", profile) as incremental:
                        if incremental is not None:
                            yield incremental


def finalized_timestamp(name: str) -> float | None:
    matched = FINALIZED_SESSION.fullmatch(name)
    if matched is None:
        return None
    timestamp, nonce, fingerprint = (int(value, 36) for value in matched.groups())
    if timestamp > 2**64 - 1 or nonce > 2**32 - 1 or fingerprint > 2**128 - 1:
        return None
    return timestamp / 1_000_000


def sibling_lock(name: str) -> str:
    return name.rsplit("-", 1)[0] + ".lock"


def old_enough(metadata: os.stat_result, created: float, now: float) -> bool:
    cutoff = now - MIN_AGE_SECONDS
    return created < cutoff and metadata.st_mtime < cutoff


def lock_operation(handle: int, operation: int) -> None:
    # rustc uses flock on Linux; its other Unix targets use POSIX record locks.
    # Those lock families are independent on some hosts, so match rustc exactly.
    # https://github.com/rust-lang/rust/tree/1.98.1/compiler/rustc_data_structures/src/flock
    if sys.platform.startswith("linux"):
        fcntl.flock(handle, operation)
    else:
        fcntl.lockf(handle, operation)


@contextmanager
def exclusive_session(crate: int, name: str, created: float, now: float):
    """Hold the sibling lock and original session object through deletion."""
    if fcntl is None:
        yield None
        return
    lock_handle = session_handle = None
    admitted = False
    try:
        before = os.stat(name, dir_fd=crate, follow_symlinks=False)
        lock_name = sibling_lock(name)
        lock_before = os.stat(lock_name, dir_fd=crate, follow_symlinks=False)
        if (
            not stat.S_ISDIR(before.st_mode)
            or not stat.S_ISREG(lock_before.st_mode)
            or not old_enough(before, created, now)
            or not old_enough(lock_before, created, now)
        ):
            raise OSError("session is not an old finalized cache with its original lock")
        lock_handle = os.open(
            lock_name,
            os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0),
            dir_fd=crate,
        )
        lock_operation(lock_handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        session_handle = os.open(name, directory_flags(), dir_fd=crate)
        admitted = (
            unchanged(lock_before, os.fstat(lock_handle))
            and unchanged(lock_before, os.stat(lock_name, dir_fd=crate, follow_symlinks=False))
            and unchanged(before, os.fstat(session_handle))
            and unchanged(before, os.stat(name, dir_fd=crate, follow_symlinks=False))
        )
    except OSError:
        pass
    try:
        yield session_handle if admitted else None
    finally:
        if session_handle is not None:
            os.close(session_handle)
        if lock_handle is not None:
            os.close(lock_handle)
        # Closing releases the lock. Never unlink its inode: a compiler may
        # already have it open while trying to acquire a read lock.


def session_bytes(session: int) -> int:
    def unreadable(error: OSError) -> None:
        raise error

    size = 0
    for _, _, filenames, directory in os.fwalk(
        ".", dir_fd=session, follow_symlinks=False, onerror=unreadable
    ):
        for name in filenames:
            metadata = os.stat(name, dir_fd=directory, follow_symlinks=False)
            if stat.S_ISREG(metadata.st_mode):
                size += metadata.st_size
    return size


def remove_contents(directory: int, depth: int = 0) -> None:
    """Delete only children reached from the held original session handle."""
    if depth >= MAX_PAYLOAD_DEPTH:
        raise OSError("cache payload nesting is unsupported")
    for name in os.listdir(directory):
        before = os.stat(name, dir_fd=directory, follow_symlinks=False)
        if not stat.S_ISDIR(before.st_mode):
            os.unlink(name, dir_fd=directory)
            continue
        with open_directory(name, directory, before) as child:
            if child is None:
                raise OSError("cache payload directory was replaced")
            remove_contents(child, depth + 1)
            current = os.stat(name, dir_fd=directory, follow_symlinks=False)
            if not same_object(current, os.fstat(child)):
                raise OSError("cache payload directory was replaced")
            os.rmdir(name, dir_fd=directory)


def prune_crates(incremental: int, keep: int, dry_run: bool, now: float) -> tuple[int, int, int]:
    removed = removed_bytes = protected = 0
    for crate_name in entries(incremental):
        with open_directory(crate_name, incremental) as crate:
            if crate is None:
                continue
            sessions = []
            for name in entries(crate):
                try:
                    metadata = os.stat(name, dir_fd=crate, follow_symlinks=False)
                except OSError:
                    continue
                if not stat.S_ISDIR(metadata.st_mode) or not name.startswith("s-"):
                    continue
                created = finalized_timestamp(name)
                if created is None:
                    protected += 1
                else:
                    sessions.append((created, name))
            sessions.sort(reverse=True)
            for created, name in sessions[keep:]:
                with exclusive_session(crate, name, created, now) as session:
                    if session is None:
                        protected += 1
                        continue
                    try:
                        size = session_bytes(session)
                        if not dry_run:
                            remove_contents(session)
                            current = os.stat(name, dir_fd=crate, follow_symlinks=False)
                            if not same_object(current, os.fstat(session)):
                                raise OSError("finalized cache directory was replaced")
                            os.rmdir(name, dir_fd=crate)
                    except OSError:
                        protected += 1
                        continue
                    removed += 1
                    removed_bytes += size
    return removed, removed_bytes, protected


def prune_directory(root: int, keep: int, dry_run: bool) -> tuple[int, int, int]:
    totals = [0, 0, 0]
    now = time.time()
    with closing(incremental_directories(root)) as incremental:
        for directory in incremental:
            result = prune_crates(directory, keep, dry_run, now)
            totals = [left + right for left, right in zip(totals, result)]
    return tuple(totals)


def prune_target(target: Path, keep: int = 1, dry_run: bool = False) -> tuple[int, int, int]:
    """Return (removed sessions, logical file bytes, protected sessions)."""
    if keep < 1:
        raise ValueError("keep must be at least one")
    if os.name != "posix" or not ANCHORED_FILESYSTEM:
        return 0, 0, 0
    with open_directory(target) as root:
        return (0, 0, 0) if root is None else prune_directory(root, keep, dry_run)


def prune_artifact_target(
    executable: Path, keep: int = 1, dry_run: bool = False, known_targets=(),
) -> tuple[int, int, int]:
    """Admit only the closest marked Cargo root, never other marked ancestors."""
    if keep < 1:
        raise ValueError("keep must be at least one")
    if os.name != "posix" or not ANCHORED_FILESYSTEM:
        return 0, 0, 0
    for target in cargo_target_candidates(executable):
        if target in known_targets:
            return 0, 0, 0  # This actual root was already pruned explicitly.
        with open_directory(target) as root:
            if root is not None and cargo_cache_marker(root):
                return prune_directory(root, keep, dry_run)
    return 0, 0, 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("targets", nargs="+", type=Path)
    parser.add_argument("--keep", type=int, default=1)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--quiet", action="store_true")
    parser.add_argument("--cargo-executable", type=Path)
    args = parser.parse_args()
    if args.keep < 1:
        parser.error("--keep must be at least one")

    totals = [0, 0, 0]
    targets = {Path(os.path.abspath(target)) for target in args.targets}
    for target in targets:
        result = prune_target(target, args.keep, args.dry_run)
        totals = [left + right for left, right in zip(totals, result)]
    if args.cargo_executable is not None:
        result = prune_artifact_target(args.cargo_executable, args.keep, args.dry_run, targets)
        totals = [left + right for left, right in zip(totals, result)]
    if not args.quiet:
        action = "would remove" if args.dry_run else "removed"
        print(
            f"cargo incremental cache: {action} {totals[0]} old session(s), "
            f"{totals[1] / 1024**2:.1f} MiB of session files; "
            f"kept {totals[2]} protected session(s)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
