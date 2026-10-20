"""Failure-detector checks for the native version journey; no desktop claimed."""
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from copy import deepcopy

sys.dont_write_bytecode = True
from desktop_ui_journeys import fingerprint
from desktop_ui_version_journeys import (
    docked_history, version_history, version_pair, version_pdf_ready, version_restored,
)


class VersionJourneyContract(unittest.TestCase):
    def document(self):
        return {"version_history": {"visible": True, "loading": False, "count": 3,
                "left": fingerprint("older"), "right": fingerprint("newer"),
                "selected": [1, None], "can_restore": [True, False], "error": None,
                "rendering": False, "preview": None}}

    def test_requires_chronological_actual_sources_and_current_on_right(self):
        document = self.document()
        self.assertTrue(version_pair(document, "older", "newer"))
        self.assertFalse(version_pair(document, "newer", "older"))
        for mutate in (lambda h: h.update(loading=True),
                       lambda h: h.update(left=fingerprint("unrelated")),
                       lambda h: h.update(right=fingerprint("unrelated")),
                       lambda h: h.update(selected=[None, 1]),
                       lambda h: h.update(selected=[1, 0])):
            changed = deepcopy(document)
            mutate(changed["version_history"])
            with self.subTest(changed=changed):
                self.assertFalse(version_pair(changed, "older", "newer"))

    def test_rejects_missing_malformed_and_error_states_instead_of_passing(self):
        for document in ({}, {"version_history": None}, {"version_history": {}}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                version_history(document)
        for key in self.document()["version_history"]:
            document = self.document()
            document["version_history"].pop(key)
            with self.subTest(missing=key), self.assertRaises(AssertionError):
                version_history(document)
        for change in ({"visible": 1}, {"loading": 0}, {"count": True}, {"count": -1},
                       {"left": "raw source text"}, {"right": "000"}, {"right": True},
                       {"selected": [3, None]}, {"selected": [True, None]},
                       {"selected": [1]}, {"can_restore": [1, False]},
                       {"can_restore": [True]}, {"error": "revision unavailable"},
                       {"rendering": 1}, {"preview": {}},
                       {"preview": {"side": True, "source_fingerprint": fingerprint("older"), "pages": 1}}):
            document = self.document()
            document["version_history"].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                version_history(document)

    def test_closed_browser_cannot_keep_loading_revisions_or_restore_actions(self):
        closed = {"visible": False, "loading": False, "count": 0, "left": None,
                  "right": None, "selected": [None, None], "can_restore": [False, False],
                  "error": None, "rendering": False, "preview": None}
        self.assertFalse(version_history({"version_history": closed})["visible"])
        for change in ({"loading": True}, {"count": 1}, {"left": fingerprint("old")},
                       {"can_restore": [True, False]}, {"rendering": True},
                       {"preview": {"side": 0, "source_fingerprint": fingerprint("older"), "pages": 1}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                version_history({"version_history": {**closed, **change}})

    def test_restore_requires_buffer_disk_tab_identity_and_no_dirty_state(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "version.typ"
            path.write_text("older")
            document = {**self.document(), "path": str(path), "tabs": 2,
                        "active_tab_id": 7, "source_fingerprint": fingerprint("older"),
                        "dirty": False, "view_mode": "Split"}
            document["version_history"] = {"visible": False, "loading": False, "count": 0,
                "left": None, "right": None, "selected": [None, None],
                "can_restore": [False, False], "error": None, "rendering": False, "preview": None}
            self.assertTrue(version_restored(document, path, "older", 2, 7))
            for change in ({"dirty": True}, {"tabs": 1}, {"active_tab_id": 8}, {"view_mode": "Code"},
                           {"path": str(path) + ".other"},
                           {"source_fingerprint": fingerprint("newer")}):
                with self.subTest(change=change):
                    self.assertFalse(version_restored({**document, **change}, path, "older", 2, 7))
            path.write_text("newer")
            self.assertFalse(version_restored(document, path, "older", 2, 7))

    def test_historical_pdf_requires_completed_matching_side_and_visible_pdfium_pages(self):
        document = self.document()
        document.update(preview_pages=1, preview_surface_visible=False)
        document["version_history"]["preview"] = {
            "side": 0, "source_fingerprint": fingerprint("older"), "pages": 1}
        self.assertTrue(version_pdf_ready(document, 0, "older", 1))
        self.assertFalse(version_pdf_ready(document, 1, "older", 1))
        self.assertFalse(version_pdf_ready(document, 0, "newer", 1))
        self.assertFalse(version_pdf_ready(document, 0, "older", 2))
        for change in ({"preview_pages": 0}, {"preview_surface_visible": True}):
            self.assertFalse(version_pdf_ready({**document, **change}, 0, "older", 1))
        for change in ({"rendering": True}, {"preview": None}):
            changed = deepcopy(document)
            changed["version_history"].update(change)
            self.assertFalse(version_pdf_ready(changed, 0, "older", 1))

    def geometry(self):
        document = self.document()
        document.update(view_mode="Split", viewport="root", frame=20)
        def target(x, y, width, height):
            return dict(x=x, y=y, width=width, height=height,
                        age_ms=0, viewport="root", frame=20)
        return {"document": document, "targets": {
            "versions.panel": target(300, 400, 500, 700),
            "versions.diff": target(300, 450, 490, 580),
            "preview.page": target(900, 400, 650, 700)}}

    def test_comparison_must_stay_inside_code_pane_beside_a_current_preview(self):
        state = self.geometry()
        self.assertTrue(docked_history(state))
        mutations = [lambda s: s["document"].update(view_mode="Code"),
                     lambda s: s["targets"].pop("preview.page"),
                     lambda s: s["targets"]["versions.panel"].update(viewport="child"),
                     lambda s: s["targets"]["versions.diff"].update(frame=19),
                     lambda s: s["targets"]["preview.page"].update(age_ms=501),
                     lambda s: s["targets"]["versions.diff"].update(width=510),
                     lambda s: s["targets"]["versions.diff"].update(height=1000),
                     lambda s: s["targets"]["preview.page"].update(x=500)]
        for mutate in mutations:
            changed = deepcopy(state)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                docked_history(changed)
        for invalid in (float("nan"), float("inf"), True, None):
            changed = deepcopy(state)
            changed["targets"]["versions.panel"]["x"] = invalid
            with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                docked_history(changed)

    def test_runner_exposes_the_native_journey(self):
        spec = importlib.util.spec_from_file_location("desktop_runner_versions",
            Path(__file__).with_name("test-desktop-ui.py"))
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        self.assertIn("file_versions", runner.JOURNEY_NAMES)
        self.assertTrue(callable(getattr(runner.Journey, "file_versions", None)))


if __name__ == "__main__":
    unittest.main()
