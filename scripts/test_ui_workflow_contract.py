"""Keep CI selectors aligned with the maintained native and detector lanes."""
import importlib.util
import re
from pathlib import Path
import shlex
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("workflow_desktop_ui", ROOT / "scripts/test-desktop-ui.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class UiWorkflowContract(unittest.TestCase):
    def test_native_dropdown_exposes_every_registered_choice_exactly_once(self):
        workflow = (ROOT / ".github/workflows/desktop-ui.yml").read_text()
        match = re.search(r"options:\s*(\[[^\]]*\])", workflow, re.DOTALL)
        self.assertIsNotNone(match, "native journey options must remain an inspectable flow list")
        choices = [item.strip().strip("\"'") for item in match.group(1)[1:-1].split(",")]
        self.assertEqual(len(choices), len(set(choices)), "duplicate native workflow choices")
        self.assertEqual(len(runner.JOURNEY_NAMES), len(set(runner.JOURNEY_NAMES)))
        self.assertSetEqual(set(choices), set(runner.JOURNEY_NAMES))

    def test_fast_ci_selector_collects_all_detector_contract_files(self):
        workflow = (ROOT / ".github/workflows/checks.yml").read_text()
        match = re.search(r"- name: Desktop journey detector contracts\n\s+run: ([^\n]+)", workflow)
        self.assertIsNotNone(match, "missing fast desktop detector check")
        command = shlex.split(match.group(1))
        if command[:4] == ["python3", "-m", "unittest", "discover"]:
            directory = ROOT / command[command.index("-s") + 1]
            pattern = command[command.index("-p") + 1]
            collected = {path.resolve() for path in directory.glob(pattern)}
        else:
            self.assertEqual(command[0], "python3")
            collected = {(ROOT / command[1]).resolve()}
        expected = {path.resolve() for path in (ROOT / "scripts").glob("test_*contract.py")}
        self.assertSetEqual(collected, expected,
                            "new standalone detector contracts must not become orphaned in CI")


if __name__ == "__main__":
    unittest.main()
