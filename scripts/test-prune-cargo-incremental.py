#!/usr/bin/env python3
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

import prune_cargo_incremental


class PruneCargoIncrementalTests(unittest.TestCase):
    def test_keeps_newest_session_and_unrelated_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target"
            incremental = target / "debug" / "incremental" / "tiptoptyp-hash"
            old = incremental / "s-old"
            newest = incremental / "s-new"
            old.mkdir(parents=True)
            newest.mkdir()
            (old / "large.bin").write_bytes(b"old")
            (newest / "current.bin").write_bytes(b"new")
            os.utime(old, (time.time() - 60, time.time() - 60))
            os.utime(newest, None)
            unrelated = target / "debug" / "deps" / "tiptoptyp"
            unrelated.parent.mkdir(parents=True)
            unrelated.write_bytes(b"keep")

            removed, size, active = prune_cargo_incremental.prune_target(target)

            self.assertEqual((removed, size, active), (1, 3, 0))
            self.assertFalse(old.exists())
            self.assertTrue(newest.exists())
            self.assertTrue(unrelated.exists())

    def test_dry_run_does_not_delete_sessions(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target"
            incremental = target / "debug" / "incremental" / "crate-hash"
            old = incremental / "s-old"
            newest = incremental / "s-new"
            old.mkdir(parents=True)
            newest.mkdir()
            os.utime(old, (time.time() - 60, time.time() - 60))

            removed, _, _ = prune_cargo_incremental.prune_target(target, dry_run=True)

            self.assertEqual(removed, 1)
            self.assertTrue(old.exists())
            self.assertTrue(newest.exists())

    @unittest.skipIf(prune_cargo_incremental.fcntl is None, "requires Unix file locks")
    def test_does_not_delete_an_active_session(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target"
            incremental = target / "debug" / "incremental" / "crate-hash"
            old = incremental / "s-old"
            newest = incremental / "s-new"
            old.mkdir(parents=True)
            newest.mkdir()
            lock = old / "session.lock"
            ready = Path(directory) / "ready"
            os.utime(old, (time.time() - 60, time.time() - 60))
            code = (
                "import fcntl, pathlib, sys, time; "
                "handle=open(sys.argv[1], 'a+b'); "
                "fcntl.flock(handle.fileno(), fcntl.LOCK_EX); "
                "pathlib.Path(sys.argv[2]).touch(); time.sleep(30)"
            )
            process = subprocess.Popen([sys.executable, "-c", code, str(lock), str(ready)])
            try:
                deadline = time.time() + 5
                while not ready.exists() and time.time() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists())
                os.utime(old, (time.time() - 60, time.time() - 60))
                removed, _, active = prune_cargo_incremental.prune_target(target)
                self.assertEqual((removed, active), (0, 1))
                self.assertTrue(old.exists())
            finally:
                process.terminate()
                process.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
