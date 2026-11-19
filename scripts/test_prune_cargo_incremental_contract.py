#!/usr/bin/env python3
import os
from contextlib import contextmanager
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from unittest import mock

import prune_cargo_incremental


ROOT = Path(__file__).resolve().parents[1]
LOCKING_AVAILABLE = (
    os.name == "posix"
    and prune_cargo_incremental.fcntl is not None
    and prune_cargo_incremental.ANCHORED_FILESYSTEM
)
LOCK_CODE = """
import fcntl, sys
handle = open(sys.argv[1], 'r+b')
lock = fcntl.flock if sys.platform.startswith('linux') else fcntl.lockf
mode = fcntl.LOCK_SH if sys.argv[2] == 'shared' else fcntl.LOCK_EX
lock(handle, mode)
print('ready', flush=True)
sys.stdin.read(1)
"""
TRY_LOCK_CODE = """
import fcntl, sys
handle = open(sys.argv[1], 'r+b')
lock = fcntl.flock if sys.platform.startswith('linux') else fcntl.lockf
try:
    lock(handle, fcntl.LOCK_SH | fcntl.LOCK_NB)
except BlockingIOError:
    sys.exit(73)
"""


def base36(value, width):
    digits = "0123456789abcdefghijklmnopqrstuvwxyz"
    result = ""
    while value:
        value, remainder = divmod(value, 36)
        result = digits[remainder] + result
    return (result or "0").rjust(width, "0")


def session(crate, *, age, nonce=1, suffix=None, lock=True):
    timestamp = time.time() - age
    stem = f"s-{base36(int(timestamp * 1_000_000), 10)}-{base36(nonce, 7)}"
    path = crate / f"{stem}-{suffix or base36(1, 25)}"
    path.mkdir(parents=True)
    (path / "object.o").write_bytes(b"old")
    sibling = crate / (stem + ".lock")
    if lock:
        sibling.touch()
        os.utime(sibling, (timestamp, timestamp))
    os.utime(path, (timestamp, timestamp))
    return path, sibling


class CacheFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.target = self.root / "target"
        self.crate = self.target / "debug" / "incremental" / "crate-hash"

    def pair(self):
        old, lock = session(self.crate, age=300)
        newest, _ = session(self.crate, age=200, nonce=2)
        return old, lock, newest

    def hold_lock(self, path, mode):
        process = subprocess.Popen(
            [sys.executable, "-u", "-c", LOCK_CODE, str(path), mode],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True,
        )

        def finish():
            try:
                process.communicate("x", timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate(timeout=5)

        self.addCleanup(finish)
        self.assertTrue(select.select([process.stdout], [], [], 5)[0], "lock owner did not start")
        self.assertEqual(process.stdout.readline().strip(), "ready")
        return process


class CargoRunnerContractTests(unittest.TestCase):
    def test_normal_cargo_invocations_have_no_cleanup_runner(self):
        config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        for target in config.get("target", {}).values():
            self.assertNotIn("runner", target)
        self.assertFalse((ROOT / "scripts/cargo-run-clean.sh").exists())

    def test_active_documentation_has_no_automatic_cleanup_switch(self):
        readme = (ROOT / "README.md").read_text()
        self.assertNotIn("TIPTOPTYP_KEEP_CARGO_CACHE", readme)
        self.assertNotIn("TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS", readme)
        self.assertIn("prune_cargo_incremental.py --dry-run", readme)


@unittest.skipUnless(LOCKING_AVAILABLE, "requires Rust-compatible POSIX file locks")
class PruneCargoIncrementalTests(CacheFixture):
    def test_keeps_newest_finalized_session_and_unrelated_outputs(self):
        old, lock, newest = self.pair()
        lock_inode = lock.stat().st_ino
        unrelated = self.target / "debug/deps/application"
        unrelated.parent.mkdir(parents=True)
        unrelated.write_bytes(b"keep")
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 0))
        self.assertFalse(old.exists())
        self.assertTrue(newest.exists())
        self.assertEqual(lock.stat().st_ino, lock_inode)
        self.assertEqual(unrelated.read_bytes(), b"keep")

    def test_dry_run_takes_locks_without_deleting_or_creating_files(self):
        old, lock, newest = self.pair()
        before = sorted(path.relative_to(self.target) for path in self.target.rglob("*"))
        self.assertEqual(prune_cargo_incremental.prune_target(self.target, dry_run=True), (1, 3, 0))
        self.assertEqual(before, sorted(path.relative_to(self.target) for path in self.target.rglob("*")))
        self.assertTrue(old.exists())
        self.assertTrue(lock.exists())
        self.assertTrue(newest.exists())

    def test_actual_rust_sibling_shared_lock_protects_an_active_reader(self):
        old, lock, _ = self.pair()
        self.hold_lock(lock, "shared")
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())

    def test_actual_rust_sibling_exclusive_lock_protects_an_active_owner(self):
        old, lock, _ = self.pair()
        self.hold_lock(lock, "exclusive")
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())

    def test_exclusive_lock_is_held_during_deletion_and_inode_survives(self):
        old, lock, _ = self.pair()
        inode = lock.stat().st_ino
        session_inode = old.stat().st_ino
        real_remove = prune_cargo_incremental.remove_contents

        def remove(directory, depth=0):
            self.assertEqual(os.fstat(directory).st_ino, session_inode)
            reader = subprocess.run(
                [sys.executable, "-c", TRY_LOCK_CODE, str(lock)],
                capture_output=True, timeout=5,
            )
            self.assertEqual(reader.returncode, 73, reader.stderr)
            real_remove(directory, depth)

        with mock.patch.object(prune_cargo_incremental, "remove_contents", side_effect=remove):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 0))
        self.assertEqual(lock.stat().st_ino, inode)
        reader = subprocess.run(
            [sys.executable, "-c", TRY_LOCK_CODE, str(lock)], capture_output=True, timeout=5,
        )
        self.assertEqual(reader.returncode, 0, reader.stderr)

    def test_working_cache_never_displaces_the_retained_finalized_cache(self):
        old, _, newest = self.pair()
        working, _ = session(self.crate, age=100, nonce=3, suffix="working")
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 1))
        self.assertFalse(old.exists())
        self.assertTrue(newest.exists())
        self.assertTrue(working.exists())

    def test_recent_session_timestamp_is_protected_even_with_old_mtimes(self):
        recent, lock = session(self.crate, age=2)
        latest, _ = session(self.crate, age=1, nonce=2)
        stale = time.time() - 300
        os.utime(recent, (stale, stale))
        os.utime(lock, (stale, stale))
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(recent.exists())
        self.assertTrue(latest.exists())

    def test_fresh_directory_or_lock_is_protected(self):
        old, lock, _ = self.pair()
        for fresh in [old, lock]:
            with self.subTest(fresh=fresh.name):
                stale = time.time() - 300
                os.utime(old, (stale, stale))
                os.utime(lock, (stale, stale))
                os.utime(fresh, None)
                self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
                self.assertTrue(old.exists())

    def test_missing_lock_is_preserved_without_fabricating_a_lock(self):
        old, lock, _ = self.pair()
        lock.unlink()
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())
        self.assertFalse(lock.exists())

    def test_symlink_lock_is_preserved(self):
        old, lock, _ = self.pair()
        elsewhere = self.root / "other.lock"
        elsewhere.touch()
        lock.unlink()
        lock.symlink_to(elsewhere)
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())
        self.assertTrue(lock.is_symlink())

    def test_replaced_lock_inode_is_rejected_after_acquisition(self):
        old, lock, _ = self.pair()
        acquire = prune_cargo_incremental.lock_operation

        def replace(handle, operation):
            acquire(handle, operation)
            lock.rename(lock.with_suffix(".previous"))
            lock.touch()

        with mock.patch.object(prune_cargo_incremental, "lock_operation", side_effect=replace):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())

    def test_admitted_cache_cannot_follow_a_swapped_ancestor_outside_target(self):
        for ancestor_kind in ["target", "profile", "incremental", "crate"]:
            with self.subTest(ancestor=ancestor_kind), tempfile.TemporaryDirectory() as temporary:
                base = Path(temporary)
                target = base / "target"
                crate = target / "debug/incremental/crate-hash"
                old, _ = session(crate, age=300)
                session(crate, age=200, nonce=2)
                ancestor = {
                    "target": target,
                    "profile": target / "debug",
                    "incremental": crate.parent,
                    "crate": crate,
                }[ancestor_kind]
                outside = base / "outside"
                payload = outside / old.relative_to(ancestor) / "personal-file"
                payload.parent.mkdir(parents=True)
                payload.write_bytes(b"outside content must survive")
                saved = ancestor.with_name(ancestor.name + "-saved")
                admitted_session = prune_cargo_incremental.exclusive_session

                @contextmanager
                def swap_after_admission(*args, **kwargs):
                    with admitted_session(*args, **kwargs) as admitted:
                        if admitted:
                            ancestor.rename(saved)
                            ancestor.symlink_to(outside, target_is_directory=True)
                        yield admitted

                with mock.patch.object(prune_cargo_incremental, "exclusive_session", swap_after_admission):
                    prune_cargo_incremental.prune_target(target)
                self.assertEqual(payload.read_bytes(), b"outside content must survive")
                self.assertFalse((saved / old.relative_to(ancestor)).exists())

    def test_admitted_session_replacement_does_not_delete_the_replacement(self):
        old, _, _ = self.pair()
        outside = self.root / "outside"
        outside.mkdir()
        (outside / "personal-file").write_bytes(b"replacement must survive")
        saved = self.crate / "saved-session"
        admitted_session = prune_cargo_incremental.exclusive_session

        @contextmanager
        def swap_after_admission(*args, **kwargs):
            with admitted_session(*args, **kwargs) as admitted:
                if admitted:
                    old.rename(saved)
                    outside.rename(old)
                yield admitted

        with mock.patch.object(prune_cargo_incremental, "exclusive_session", swap_after_admission):
            prune_cargo_incremental.prune_target(self.target)
        self.assertEqual((old / "personal-file").read_bytes(), b"replacement must survive")
        self.assertFalse((saved / "object.o").exists())

    def test_no_lock_or_descriptor_support_preserves_candidates(self):
        old, _, _ = self.pair()
        with mock.patch.object(prune_cargo_incremental, "fcntl", None):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        with mock.patch.object(prune_cargo_incremental, "ANCHORED_FILESYSTEM", False):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 0))
        # Isolate the fake OS from unrelated fixtures' background Python threads.
        code = (
            "import os,sys; from pathlib import Path; import prune_cargo_incremental as p; "
            "target=Path(sys.argv[1]); os.name='nt'; "
            "assert p.prune_target(target)==(0,0,0)"
        )
        child = subprocess.run(
            [sys.executable, "-c", code, str(self.target)], cwd=ROOT / "scripts",
            capture_output=True, text=True, timeout=5,
        )
        self.assertEqual(child.returncode, 0, child.stderr)
        self.assertTrue(old.exists())

    def test_failed_deletion_releases_lock_and_preserves_its_inode(self):
        old, lock, _ = self.pair()
        inode = lock.stat().st_ino
        with mock.patch.object(prune_cargo_incremental, "remove_contents", side_effect=PermissionError("blocked")):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 1))
        self.assertTrue(old.exists())
        self.assertEqual(lock.stat().st_ino, inode)
        reader = subprocess.run(
            [sys.executable, "-c", TRY_LOCK_CODE, str(lock)], capture_output=True, timeout=5,
        )
        self.assertEqual(reader.returncode, 0, reader.stderr)

    def test_unreadable_payload_is_preserved_even_in_dry_run(self):
        old, _, _ = self.pair()

        def unreadable(*args, onerror, **kwargs):
            onerror(PermissionError("unreadable cache payload"))
            return iter(())

        with mock.patch.object(prune_cargo_incremental.os, "fwalk", side_effect=unreadable):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target, dry_run=True), (0, 0, 1))
        self.assertTrue(old.exists())

    def test_discovery_is_bounded_to_profiles_and_triple_profiles(self):
        old, _, _ = self.pair()
        triple = self.target / "aarch64-apple-darwin/profiling/incremental/crate-hash"
        triple_old, _ = session(triple, age=300)
        session(triple, age=200, nonce=2)
        ignored = self.target / "debug/build/dependency/out/target/debug/incremental/crate-hash"
        ignored_old, _ = session(ignored, age=300)
        session(ignored, age=200, nonce=2)
        with mock.patch.object(Path, "rglob", side_effect=AssertionError("whole-target recursive scan")):
            self.assertEqual(prune_cargo_incremental.prune_target(self.target), (2, 6, 0))
        self.assertFalse(old.exists())
        self.assertFalse(triple_old.exists())
        self.assertTrue(ignored_old.exists())

    def test_symlink_profiles_and_crates_are_not_followed(self):
        self.pair()
        outside = self.root / "outside/incremental/crate-hash"
        outside_old, _ = session(outside, age=300)
        session(outside, age=200, nonce=2)
        (self.target / "linked").symlink_to(outside.parent.parent, target_is_directory=True)
        (self.crate.parent / "linked-crate").symlink_to(outside, target_is_directory=True)
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 0))
        self.assertTrue(outside_old.exists())

    def test_session_size_does_not_follow_file_symlinks(self):
        old, _, _ = self.pair()
        outside = self.root / "large-file"
        outside.write_bytes(b"x" * 100)
        (old / "linked").symlink_to(outside)
        stale = time.time() - 300
        os.utime(old, (stale, stale))
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 0))
        self.assertEqual(outside.read_bytes(), b"x" * 100)

    def test_unknown_session_names_are_not_deleted(self):
        old, _, _ = self.pair()
        unknown = self.crate / "s-unrecognized"
        unknown.mkdir()
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (1, 3, 1))
        self.assertFalse(old.exists())
        self.assertTrue(unknown.exists())

    def test_keep_count_is_applied_to_finalized_sessions_in_rust_timestamp_order(self):
        oldest, _ = session(self.crate, age=400)
        middle, _ = session(self.crate, age=300, nonce=2)
        newest, _ = session(self.crate, age=200, nonce=3)
        # Old directory mtimes do not determine which source rustc will reuse.
        old_mtime = time.time() - 500
        os.utime(newest, (old_mtime, old_mtime))
        self.assertEqual(prune_cargo_incremental.prune_target(self.target, keep=2), (1, 3, 0))
        self.assertFalse(oldest.exists())
        self.assertTrue(middle.exists())
        self.assertTrue(newest.exists())

    def test_invalid_keep_and_missing_target(self):
        with self.assertRaises(ValueError):
            prune_cargo_incremental.prune_target(self.target, keep=0)
        self.assertEqual(prune_cargo_incremental.prune_target(self.target), (0, 0, 0))


if __name__ == "__main__":
    unittest.main()
