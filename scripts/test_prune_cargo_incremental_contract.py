#!/usr/bin/env python3
import os
from contextlib import contextmanager
import json
from pathlib import Path
import select
import shlex
import shutil
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
    def test_macos_cargo_runner_enables_post_run_cleanup(self):
        config = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        self.assertEqual(
            config["target"]['cfg(target_os = "macos")']["runner"],
            "scripts/cargo-run-clean.sh",
        )
        self.assertTrue(os.access(ROOT / "scripts/cargo-run-clean.sh", os.X_OK))

    def test_documentation_explains_automatic_cleanup_and_overrides(self):
        readme = (ROOT / "README.md").read_text()
        self.assertIn("After the launched process exits", readme)
        self.assertIn("TIPTOPTYP_KEEP_CARGO_CACHE=1", readme)
        self.assertIn("TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS", readme)
        self.assertIn("prune_cargo_incremental.py --dry-run", readme)


@unittest.skipUnless(LOCKING_AVAILABLE, "requires Rust-compatible POSIX file locks")
class CargoRunnerExecutionTests(unittest.TestCase):
    """Run private copies only; repository build caches are never candidates."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.repository = self.directory / "repository with spaces"
        scripts = self.repository / "scripts"
        scripts.mkdir(parents=True)
        for name in ["cargo-run-clean.sh", "prune_cargo_incremental.py"]:
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        self.runner = scripts / "cargo-run-clean.sh"
        self.commands = self.directory / "commands"
        self.commands.mkdir()
        self.python_command("exec " + shlex.quote(sys.executable) + ' "$@"')
        self.caller = self.directory / "unrelated caller"
        self.caller.mkdir()
        self.receipt = self.directory / "child.json"
        self.child = self.directory / "child.py"
        self.child.write_text("""
import json, os, pathlib, sys
paths = json.loads(os.environ['RUNNER_EXPECT_SESSIONS'])
def check():
    assert all(pathlib.Path(path).is_dir() for path in paths), 'cleanup ran before child exit'
check()
if os.environ.get('RUNNER_WAIT') == '1':
    print('ready', flush=True)
    assert sys.stdin.readline().strip() == 'finish'
    check()
pathlib.Path(os.environ['RUNNER_RECEIPT']).write_text(json.dumps({
    'argv': sys.argv[1:], 'cwd': os.getcwd(), 'sessions_before_exit': paths,
}))
sys.exit(int(os.environ.get('RUNNER_EXIT', '0')))
""")
        self.env = os.environ.copy()
        for name in ["CARGO_TARGET_DIR", "TIPTOPTYP_KEEP_CARGO_CACHE", "TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS"]:
            self.env.pop(name, None)
        self.env.update({
            "PATH": str(self.commands) + os.pathsep + "/usr/bin:/bin",
            "RUNNER_RECEIPT": str(self.receipt),
            "RUNNER_EXPECT_SESSIONS": "[]",
        })

    def python_command(self, body):
        command = self.commands / "python3"
        command.write_text("#!/bin/sh\n" + body + "\n")
        command.chmod(0o755)

    def pair(self, target, profile="debug"):
        crate = target / profile / "incremental/crate-hash"
        old, lock = session(crate, age=300)
        newest, _ = session(crate, age=200, nonce=2)
        return old, lock, newest

    def artifact(self, target, layout="debug", marker=True):
        executable = target / layout / "fixture-artifact"
        executable.parent.mkdir(parents=True, exist_ok=True)
        executable.write_text("#!" + sys.executable + "\n" + self.child.read_text())
        executable.chmod(0o755)
        if marker:
            (target / "CACHEDIR.TAG").write_bytes(
                b"Signature: 8a477f597d28d172789f06886806bc55\n# Cargo cache\n"
            )
        return executable

    def invoke(self, *arguments, executable=None, **environment):
        child = [str(executable)] if executable is not None else [sys.executable, str(self.child)]
        return subprocess.run(
            [str(self.runner), *child, *arguments],
            cwd=self.caller, env={**self.env, **environment},
            text=True, capture_output=True, timeout=10,
        )

    def test_default_cleans_both_repository_roots_and_preserves_arguments_and_status(self):
        pairs = [self.pair(self.repository / relative) for relative in ["target", "xtask/target"]]
        arguments = ["argument with spaces", "", "--flag", "literal;$()[]*", "line\nbreak"]
        result = self.invoke(
            *arguments, RUNNER_EXIT="37",
            RUNNER_EXPECT_SESSIONS=json.dumps([str(old) for old, _, _ in pairs]),
        )
        self.assertEqual(result.returncode, 37, result.stderr)
        receipt = json.loads(self.receipt.read_text())
        self.assertEqual(receipt["argv"], arguments)
        self.assertEqual(Path(receipt["cwd"]), self.caller.resolve())
        for old, lock, newest in pairs:
            self.assertFalse(old.exists())
            self.assertTrue(lock.exists())
            self.assertTrue(newest.exists())

    def test_configured_target_supports_absolute_and_caller_relative_paths(self):
        for configured in ["relative target", str(self.directory / "absolute target")]:
            with self.subTest(configured=configured):
                target = Path(configured)
                if not target.is_absolute():
                    target = self.caller / target
                old, lock, newest = self.pair(target)
                result = self.invoke(executable=self.artifact(target), CARGO_TARGET_DIR=configured)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(old.exists())
                self.assertTrue(lock.exists())
                self.assertTrue(newest.exists())

    def test_member_working_directory_does_not_redirect_relative_target_cleanup(self):
        target = self.repository / "custom"
        old, _, newest = self.pair(target)
        executable = self.artifact(target, "debug/deps")
        self.caller = self.repository / "core"
        self.caller.mkdir()
        decoy, _, _ = self.pair(self.caller / "custom")
        result = self.invoke(executable=executable, CARGO_TARGET_DIR="custom")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            (old.exists(), decoy.exists()), (False, True),
            "cleanup must prune the artifact's target and preserve the member-relative decoy",
        )
        self.assertTrue(newest.exists())
        self.assertTrue(decoy.exists(), "relative env was resolved against the member directory")

    def test_actual_artifact_selects_custom_run_test_example_and_triple_targets(self):
        layouts = [
            ("debug", "debug"), ("debug/deps", "debug"),
            ("release/examples", "release"), ("fast-edit", "fast-edit"),
            ("deps", "deps"), ("examples", "examples"),
            ("deps/examples", "deps"), ("examples/deps", "examples"),
            ("aarch64-apple-darwin/debug", "aarch64-apple-darwin/debug"),
            ("aarch64-apple-darwin/debug/deps", "aarch64-apple-darwin/debug"),
            ("aarch64-apple-darwin/deps", "aarch64-apple-darwin/deps"),
            ("aarch64-apple-darwin/examples/deps", "aarch64-apple-darwin/examples"),
            ("x86_64-unknown-linux-gnu/release/examples", "x86_64-unknown-linux-gnu/release"),
        ]
        for index, (layout, profile) in enumerate(layouts):
            with self.subTest(layout=layout):
                target = self.directory / ("custom-target-" + str(index))
                old, lock, newest = self.pair(target, profile)
                executable = self.artifact(target, layout)
                decoy, _, _ = self.pair(self.directory / ("unused-env-" + str(index)))
                result = self.invoke(executable=executable, CARGO_TARGET_DIR=str(decoy.parents[3]))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(old.exists())
                self.assertTrue(lock.exists())
                self.assertTrue(newest.exists())
                self.assertTrue(decoy.exists(), "an overridden environment target was pruned")

    def test_closest_marked_root_excludes_other_marked_ancestors(self):
        parent = self.directory / "parent-target"
        parent_old, _, _ = self.pair(parent)
        (parent / "CACHEDIR.TAG").write_bytes(prune_cargo_incremental.CARGO_CACHE_SIGNATURE)
        target = parent / "debug"
        old, _, newest = self.pair(target)
        executable = self.artifact(target, "deps")
        result = self.invoke(executable=executable)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(old.exists())
        self.assertTrue(newest.exists())
        self.assertTrue(parent_old.exists(), "a marked ancestor was also pruned")

    def test_empty_closest_marked_root_still_excludes_other_marked_ancestors(self):
        parent = self.directory / "parent-target"
        parent_old, _, _ = self.pair(parent)
        (parent / "CACHEDIR.TAG").write_bytes(prune_cargo_incremental.CARGO_CACHE_SIGNATURE)
        executable = self.artifact(parent / "debug", "examples")
        result = self.invoke(executable=executable)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(parent_old.exists())

    def test_ambiguous_artifacts_or_unverified_markers_never_authorize_cleanup(self):
        for kind in ["missing", "invalid", "symlink", "directory", "non-cargo layout"]:
            with self.subTest(kind=kind):
                target = self.directory / kind
                old, _, _ = self.pair(target)
                layout = "debug/build/package/out" if kind == "non-cargo layout" else "debug/deps"
                executable = self.artifact(target, layout, marker=False)
                tag = target / "CACHEDIR.TAG"
                if kind == "invalid":
                    tag.write_bytes(b"not a Cargo cache signature\n")
                elif kind == "symlink":
                    valid_tag = self.directory / "valid-tag"
                    valid_tag.write_bytes(b"Signature: 8a477f597d28d172789f06886806bc55\n")
                    tag.symlink_to(valid_tag)
                elif kind == "directory":
                    tag.mkdir()
                elif kind == "non-cargo layout":
                    tag.write_bytes(b"Signature: 8a477f597d28d172789f06886806bc55\n")
                result = self.invoke(executable=executable, CARGO_TARGET_DIR=str(target))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(old.exists())

    def test_cleanup_waits_until_child_has_exited(self):
        old, _, newest = self.pair(self.repository / "target")
        process = subprocess.Popen(
            [str(self.runner), sys.executable, str(self.child)],
            cwd=self.caller,
            env={**self.env, "RUNNER_WAIT": "1", "RUNNER_EXPECT_SESSIONS": json.dumps([str(old)])},
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )

        def finish():
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=5)

        self.addCleanup(finish)
        self.assertTrue(select.select([process.stdout], [], [], 5)[0], "child did not start")
        self.assertEqual(process.stdout.readline().strip(), "ready")
        self.assertIsNone(process.poll())
        self.assertTrue(old.exists())
        _, stderr = process.communicate("finish\n", timeout=10)
        self.assertEqual(process.returncode, 0, stderr)
        self.assertFalse(old.exists())
        self.assertTrue(newest.exists())

    def test_opt_out_skips_python_and_preserves_all_sessions(self):
        old, _, newest = self.pair(self.repository / "target")
        invoked = self.directory / "python-was-invoked"
        self.python_command("touch " + shlex.quote(str(invoked)) + "\nexit 99")
        result = self.invoke(TIPTOPTYP_KEEP_CARGO_CACHE="1", RUNNER_EXIT="23")
        self.assertEqual(result.returncode, 23, result.stderr)
        self.assertTrue(old.exists())
        self.assertTrue(newest.exists())
        self.assertFalse(invoked.exists())

    def test_retention_override_keeps_requested_newest_sessions(self):
        old, _, newest = self.pair(self.repository / "target")
        oldest, _ = session(old.parent, age=400, nonce=3)
        result = self.invoke(TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS="2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(oldest.exists())
        self.assertTrue(old.exists())
        self.assertTrue(newest.exists())

    def test_cleanup_failures_preserve_sessions_and_child_exit_status(self):
        for child_status in ["0", "31"]:
            for failure in ["python failure", "missing python", "missing pruner", "invalid retention", "zero retention"]:
                with self.subTest(failure=failure, child_status=child_status):
                    target = self.repository / (failure + child_status)
                    old, _, newest = self.pair(target)
                    executable = self.artifact(target)
                    environment = {"CARGO_TARGET_DIR": str(target), "RUNNER_EXIT": child_status}
                    script = self.repository / "scripts/prune_cargo_incremental.py"
                    original = script.read_bytes()
                    if failure == "python failure":
                        self.python_command("exit 42")
                    elif failure == "missing python":
                        (self.commands / "dirname").symlink_to(shutil.which("dirname"))
                        (self.commands / "python3").unlink()
                        environment["PATH"] = str(self.commands)
                    elif failure == "missing pruner":
                        script.unlink()
                    elif failure == "invalid retention":
                        environment["TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS"] = "not-an-integer"
                    else:
                        environment["TIPTOPTYP_CARGO_CACHE_KEEP_SESSIONS"] = "0"
                    try:
                        result = self.invoke(executable=executable, **environment)
                    finally:
                        script.write_bytes(original)
                        self.python_command("exec " + shlex.quote(sys.executable) + ' "$@"')
                        (self.commands / "dirname").unlink(missing_ok=True)
                    self.assertEqual(result.returncode, int(child_status), result.stderr)
                    self.assertIn("cache pruning failed", result.stderr)
                    self.assertTrue(old.exists())
                    self.assertTrue(newest.exists())

    def test_missing_executable_is_rejected_without_cleanup(self):
        old, _, _ = self.pair(self.repository / "target")
        result = subprocess.run(
            [str(self.runner)], cwd=self.caller, env=self.env,
            text=True, capture_output=True, timeout=5,
        )
        self.assertEqual(result.returncode, 64, result.stderr)
        self.assertIn("missing executable", result.stderr)
        self.assertTrue(old.exists())


@unittest.skipUnless(LOCKING_AVAILABLE, "requires Rust-compatible POSIX file locks")
class PruneCargoIncrementalTests(CacheFixture):
    def test_marker_validation_and_cleanup_retain_the_same_target_directory(self):
        old, _, newest = self.pair()
        (self.target / "CACHEDIR.TAG").write_bytes(prune_cargo_incremental.CARGO_CACHE_SIGNATURE)
        executable = self.target / "debug/fixture"
        executable.touch()
        outside = self.root / "outside"
        outside_old, _ = session(outside / "debug/incremental/crate-hash", age=300)
        session(outside_old.parent, age=200, nonce=2)
        saved = self.root / "saved-target"
        marker = prune_cargo_incremental.cargo_cache_marker

        def swap_after_verification(directory):
            admitted = marker(directory)
            if admitted:
                self.target.rename(saved)
                self.target.symlink_to(outside, target_is_directory=True)
            return admitted

        with mock.patch.object(prune_cargo_incremental, "cargo_cache_marker", side_effect=swap_after_verification):
            self.assertEqual(prune_cargo_incremental.prune_artifact_target(executable), (1, 3, 0))
        self.assertFalse((saved / old.relative_to(self.target)).exists())
        self.assertTrue((saved / newest.relative_to(self.target)).exists())
        self.assertTrue(outside_old.exists())

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
