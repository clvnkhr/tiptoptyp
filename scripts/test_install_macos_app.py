"""Installer sequencing and path regressions; no Rust build or real installation."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("install-macos-app.sh")
MOCK = r'''
import json
import os
from pathlib import Path
import shutil
import sys

name = Path(sys.argv[0]).name
arguments = sys.argv[1:]
installed = Path(os.environ["INSTALL_TEST_INSTALLED_APP"])
with open(os.environ["INSTALL_TEST_LOG"], "a") as log:
    log.write(json.dumps({"name": name, "arguments": arguments,
                          "cwd": os.getcwd(),
                          "target": os.environ.get("TIPTOPTYP_PACKAGE_TARGET"),
                          "old_app_present": (installed / "old-app").is_file()}) + "\n")
if name == "uname":
    print("Darwin")
elif name == "cargo":
    if arguments[:1] == ["packager"]:
        if os.environ.get("INSTALL_TEST_BUILD_FAILURE"):
            sys.exit(24)
        if not os.environ.get("INSTALL_TEST_MISSING_BUNDLE"):
            bundle = Path.cwd() / "target/release/tiptoptyp.app"
            bundle.mkdir(parents=True)
            (bundle / "new-app").write_text("verified new app")
    elif arguments[:5] == ["run", "--manifest-path", "xtask/Cargo.toml", "--", "verify-package"]:
        if os.environ.get("INSTALL_TEST_VERIFY_FAILURE"):
            sys.exit(25)
    else:
        raise AssertionError("Unexpected cargo arguments: " + repr(arguments))
elif name == "ditto":
    assert arguments[:2] == ["--rsrc", "--extattr"]
    assert len(arguments) == 4
    shutil.copytree(arguments[2], arguments[3], dirs_exist_ok=True)
else:
    raise AssertionError("Unexpected command: " + name)
'''


class MacAppInstallerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.root = self.directory / "repository with spaces"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copy2(SCRIPT, self.root / "scripts/install-macos-app.sh")
        self.commands = self.directory / "commands"
        self.commands.mkdir()
        for name in ["cargo", "cargo-packager", "uname", "ditto"]:
            command = self.commands / name
            command.write_text("#!" + sys.executable + "\n" + MOCK)
            command.chmod(0o755)
        self.applications = self.directory / "Applications with spaces 'quoted'"
        self.installed = self.applications / "tiptoptyp.app"
        self.installed.mkdir(parents=True)
        (self.installed / "old-app").write_text("keep until verified")
        self.log = self.directory / "commands.jsonl"
        self.caller = self.directory / "elsewhere"
        self.caller.mkdir()

    def run_install(self, *arguments, **environment):
        env = os.environ.copy()
        env.pop("TIPTOPTYP_PACKAGE_TARGET", None)
        env.update({
            "PATH": str(self.commands) + os.pathsep + "/usr/bin:/bin",
            "TIPTOPTYP_APPLICATIONS_DIR": str(self.applications),
            "INSTALL_TEST_LOG": str(self.log),
        })
        env.update(environment)
        applications = Path(env["TIPTOPTYP_APPLICATIONS_DIR"])
        if not applications.is_absolute():
            applications = self.caller.resolve() / applications
        env["INSTALL_TEST_INSTALLED_APP"] = str(applications / "tiptoptyp.app")
        return subprocess.run(
            [str(self.root / "scripts/install-macos-app.sh"), *arguments],
            cwd=self.caller, env=env, text=True, capture_output=True,
        )

    def events(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def cargo_events(self):
        return [event for event in self.events() if event["name"] == "cargo"]

    def test_builds_without_liquid_and_verifies_before_replacing_installed_app(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        events = self.events()
        self.assertEqual([event["name"] for event in events], ["uname", "cargo", "cargo", "ditto"])
        build, verify = self.cargo_events()
        self.assertNotIn("Preparing the bundled grammar model", result.stdout)
        self.assertEqual(build["arguments"], [
            "packager", "--config", str(self.root / "packager-prod.toml"),
            "--target", "universal-apple-darwin",
        ])
        self.assertEqual(verify["arguments"], [
            "run", "--manifest-path", "xtask/Cargo.toml", "--", "verify-package",
            "--target", "universal-apple-darwin",
        ])
        for event in [build, verify]:
            self.assertEqual(Path(event["cwd"]), self.root.resolve())
            self.assertEqual(event["target"], "universal-apple-darwin")
            self.assertTrue(event["old_app_present"])
        self.assertFalse(events[-1]["old_app_present"])
        self.assertEqual(events[-1]["arguments"], [
            "--rsrc", "--extattr", str(self.root / "target/release/tiptoptyp.app"),
            str(self.installed),
        ])
        self.assertFalse((self.installed / "old-app").exists())
        self.assertEqual((self.installed / "new-app").read_text(), "verified new app")
        self.assertIn("Installed " + str(self.installed), result.stdout)

    def test_relative_applications_path_is_resolved_against_invoking_directory(self):
        relative = Path("relative Applications 'quoted'")
        installed = self.caller.resolve() / relative / "tiptoptyp.app"
        installed.mkdir(parents=True)
        (installed / "old-app").write_text("keep until verified")
        result = self.run_install(TIPTOPTYP_APPLICATIONS_DIR=str(relative))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        build, verify = self.cargo_events()
        self.assertTrue(all(event["old_app_present"] for event in [build, verify]))
        self.assertEqual(self.events()[-1]["arguments"][-1], str(installed))
        self.assertFalse((installed / "old-app").exists())
        self.assertTrue((installed / "new-app").is_file())
        self.assertFalse((self.root / relative).exists())
        self.assertTrue((self.installed / "old-app").is_file())

    def test_first_install_does_not_require_an_existing_app_or_liquid_model(self):
        shutil.rmtree(self.installed)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        build, verify = self.cargo_events()
        self.assertEqual(build["arguments"][0], "packager")
        self.assertEqual(verify["arguments"][4], "verify-package")
        self.assertFalse(build["old_app_present"])
        self.assertTrue((self.installed / "new-app").is_file())

    def test_build_error_does_not_verify_or_replace_previous_installation(self):
        result = self.run_install(INSTALL_TEST_BUILD_FAILURE="1")
        self.assertEqual(result.returncode, 24, result.stdout + result.stderr)
        self.assertEqual(len(self.cargo_events()), 1)
        self.assertTrue((self.installed / "old-app").is_file())
        self.assertNotIn("ditto", [event["name"] for event in self.events()])

    def test_verification_error_preserves_previous_installation(self):
        result = self.run_install(INSTALL_TEST_VERIFY_FAILURE="1")
        self.assertEqual(result.returncode, 25, result.stdout + result.stderr)
        self.assertEqual(len(self.cargo_events()), 2)
        self.assertTrue((self.installed / "old-app").is_file())
        self.assertNotIn("ditto", [event["name"] for event in self.events()])

    def test_missing_packaged_bundle_preserves_previous_installation(self):
        result = self.run_install(INSTALL_TEST_MISSING_BUNDLE="1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("packager did not produce", result.stderr)
        self.assertTrue((self.installed / "old-app").is_file())
        self.assertNotIn("ditto", [event["name"] for event in self.events()])

    def test_explicit_architecture_overrides_environment_for_every_build_step(self):
        result = self.run_install(
            "--target", "x86_64-apple-darwin",
            TIPTOPTYP_PACKAGE_TARGET="aarch64-apple-darwin",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for event in self.cargo_events():
            self.assertEqual(event["target"], "x86_64-apple-darwin")
        self.assertEqual(self.cargo_events()[0]["arguments"][-2:], ["--target", "x86_64-apple-darwin"])
        self.assertEqual(self.cargo_events()[1]["arguments"][-2:], ["--target", "x86_64-apple-darwin"])

    def test_invalid_target_fails_before_any_external_command(self):
        result = self.run_install("--target", "unknown-target")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("unsupported macOS package target", result.stderr)
        self.assertEqual(self.events(), [])
        self.assertTrue((self.installed / "old-app").is_file())


if __name__ == "__main__":
    unittest.main()
