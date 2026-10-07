"""Build-script regressions using a fake compiler; no Rust install or GUI needed."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build-intel-mac.sh")


class IntelBuildScriptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.root = self.directory / "repository"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy2(SCRIPT, self.root / "scripts/build-intel-mac.sh")
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.98.1"\n')
        self.commands = self.directory / "commands"
        self.commands.mkdir()
        self.stub("uname", 'printf "Darwin\\n"')
        self.stub("arch", "exit 1")
        self.stub("lipo", 'printf "Non-fat file: %s is architecture: x86_64\\n" "$2"')
        self.stub("rustup", r'''
case "$1" in
    target) printf 'x86_64-apple-darwin\n' ;;
    which) printf '/usr/bin/true\n' ;;
    run)
        shift 3
        mode=debug
        output="${CARGO_TARGET_DIR:-target}"
        while [ "$#" -gt 0 ]; do
            case "$1" in
                --release) mode=release ;;
                --target-dir) shift; output="$1" ;;
            esac
            shift
        done
        destination="$output/x86_64-apple-darwin/$mode/tiptoptyp"
        mkdir -p "$(dirname "$destination")"
        printf '#!/bin/sh\nexit 0\n' > "$destination"
        chmod +x "$destination"
        ;;
    *) exit 99 ;;
esac
''')
        self.caller = self.directory / "elsewhere"
        self.caller.mkdir()

    def stub(self, name, source):
        path = self.commands / name
        path.write_text("#!/bin/sh\nset -eu\n" + source + "\n")
        path.chmod(0o755)

    def run_build(self, *arguments, target_dir=None):
        env = os.environ.copy()
        env["PATH"] = str(self.commands) + os.pathsep + "/usr/bin:/bin"
        env.pop("CARGO_TARGET_DIR", None)
        if target_dir is not None:
            env["CARGO_TARGET_DIR"] = str(target_dir)
        return subprocess.run(
            [str(self.root / "scripts/build-intel-mac.sh"), *arguments],
            cwd=self.caller, env=env, text=True, capture_output=True,
        )

    def test_relative_target_directory_is_relative_to_repository_not_caller(self):
        result = self.run_build(target_dir="custom-target")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        binary = self.root / "custom-target/x86_64-apple-darwin/debug/tiptoptyp"
        self.assertTrue(binary.is_file())
        self.assertIn(f"Built: {binary}", result.stdout)
        self.assertFalse((self.caller / "custom-target").exists())

    def test_absolute_target_directory_and_release_are_preserved(self):
        target = self.directory / "absolute target"
        result = self.run_build("--release", target_dir=target)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((target / "x86_64-apple-darwin/release/tiptoptyp").is_file())

    def test_extra_arguments_are_rejected_before_compiling(self):
        result = self.run_build("--release", "--bogus")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("usage:", result.stderr)
        self.assertFalse((self.root / "target").exists())


if __name__ == "__main__":
    unittest.main()
