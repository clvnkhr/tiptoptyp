"""Fast checks for the desktop runner's failure detectors; no desktop claimed."""
import importlib.util
import math
import threading
from pathlib import Path
import unittest
from unittest.mock import patch
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("desktop_ui", Path(__file__).with_name("test-desktop-ui.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class FixtureFingerprintContract(unittest.TestCase):
    def test_matches_rust_probe_vectors_without_logging_source(self):
        from desktop_ui_journeys import fingerprint
        self.assertEqual(fingerprint(""), "cbf29ce484222325")
        self.assertEqual(fingerprint("hello"), "a430d84680aabd0b")
        self.assertEqual(fingerprint("é🙂z"), "3a046d85bff8a56f")


class ClickReadinessContract(unittest.TestCase):
    def test_settles_moving_geometry_before_one_click(self):
        journey = object.__new__(runner.Journey)
        samples = iter([10, 11, 11])
        journey.snapshot = lambda: {"document": {}, "targets": {"button": {
            "x": 20, "y": next(samples), "enabled": True, "age_ms": 0}}}
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.record = lambda *args, **kwargs: None
        journey.click("button")
        self.assertEqual(actions, [("move", 20, 10), ("move", 20, 11), ("click", 20, 11)])

    def test_unsettled_target_times_out_without_clicking(self):
        journey = object.__new__(runner.Journey)
        samples = iter([10, 11])
        journey.snapshot = lambda: {"document": {}, "targets": {"button": {
            "x": 20, "y": next(samples), "enabled": True, "age_ms": 0}}}
        actions = []
        journey.native = lambda *args: actions.append(args)
        with patch.object(runner.time, "monotonic", side_effect=[0, 4]):
            with self.assertRaisesRegex(AssertionError, "did not settle"):
                journey.click("button")
        self.assertEqual(actions, [("move", 20, 10)])

    def test_disappearing_target_never_receives_a_click(self):
        journey = object.__new__(runner.Journey)
        samples = iter([{"button": {"x": 20, "y": 10, "enabled": True, "age_ms": 0}}, {}])
        journey.snapshot = lambda: {"document": {}, "targets": next(samples)}
        actions = []
        journey.native = lambda *args: actions.append(args)
        with self.assertRaisesRegex(AssertionError, "missing"):
            journey.click("button")
        self.assertEqual(actions, [("move", 20, 10)])


class SaveIsolationContract(unittest.TestCase):
    def test_native_save_chooses_private_directory_before_confirming(self):
        journey = object.__new__(runner.Journey)
        actions = []
        journey.key = lambda *args: actions.append(("key", *args))
        journey.native = lambda *args: actions.append(args)
        destination = Path("/private/tmp/ttt-ui-fixture/saved.tex")
        with patch.object(runner.time, "sleep"):
            journey.save_to_fixture(destination)
        self.assertIn(("key", 5, "cmd+shift"), actions)
        folder = actions.index(("text", str(destination.parent)))
        filename = actions.index(("ax-set-text", destination.name))
        confirm = actions.index(("ax-press-title", "Save"))
        self.assertLess(folder, filename)
        self.assertLess(filename, confirm)


class ExplorerFrameContract(unittest.TestCase):
    def state(self, visible=False, explorer_frame=9, editor_frame=10, edge=700):
        return {"document": {"frame": 10, "explorer_visible": visible}, "targets": {
            "divider.explorer": {"frame": explorer_frame},
            "divider.editor": {"frame": editor_frame, "x": edge}}}

    def test_closed_panel_accepts_stale_targets_only(self):
        from desktop_ui_journeys import assert_explorer_frame
        self.assertEqual(assert_explorer_frame(self.state(), True, 700), 10)
        with self.assertRaisesRegex(AssertionError, "empty panel"):
            assert_explorer_frame(self.state(explorer_frame=10), True, 700)

    def test_open_panel_and_coupled_resize_are_allowed(self):
        from desktop_ui_journeys import assert_explorer_frame
        assert_explorer_frame(self.state(visible=True, explorer_frame=10), True, 700)
        assert_explorer_frame(self.state(edge=740), False, 700)

    def test_independent_movement_cannot_pass(self):
        from desktop_ui_journeys import assert_explorer_frame
        with self.assertRaisesRegex(AssertionError, "edge moved"):
            assert_explorer_frame(self.state(edge=740), True, 700)

    def test_nonfinite_independent_geometry_cannot_pass(self):
        from desktop_ui_journeys import assert_explorer_frame
        for edge, baseline in [(math.nan, 700), (math.inf, 700), (700, math.nan)]:
            with self.assertRaisesRegex(AssertionError, "invalid independent"):
                assert_explorer_frame(self.state(edge=edge), True, baseline)

    def test_stale_independent_edge_cannot_pass(self):
        from desktop_ui_journeys import assert_explorer_frame
        with self.assertRaisesRegex(AssertionError, "missing fresh"):
            assert_explorer_frame(self.state(editor_frame=9), True, 700)


class HeightContract(unittest.TestCase):
    def test_accepts_constant_height_when_window_moves(self):
        for top in [0, 100, -200]:
            runner.assert_stable_height(220, {"panel_rect": [0, top, 800, top + 220]})

    def test_rejects_the_observed_three_point_per_frame_regression(self):
        for frame in range(1, 31):
            with self.assertRaisesRegex(AssertionError, "panel drift"):
                runner.assert_stable_height(232, {"panel_rect": [0, 0, 800, 232 + 3 * frame]})

    def test_missing_or_nonfinite_geometry_cannot_pass(self):
        for rectangle in [None, [0, 0, 800, math.nan], [0, 0, 800, math.inf]]:
            with self.assertRaises(AssertionError):
                runner.assert_stable_height(220, {"panel_rect": rectangle})


class PreviewContract(unittest.TestCase):
    def observation(self):
        return {"age_ms": 0, "value": {"ready": True, "filters": ["url(#tiptoptyp-palette)"],
                "background": "rgb(0, 0, 0)", "slopes": [-1, -1, -1], "intercepts": [1, 1, 1]}}

    def test_single_dark_transform(self):
        runner.assert_tinymist_palette(self.observation(), [[0, 0, 0, 255], [255, 255, 255, 255]])

    def test_double_inversion_cannot_pass(self):
        state = self.observation()
        state["value"]["filters"].append("invert(1)")
        with self.assertRaisesRegex(AssertionError, "exactly one"):
            runner.assert_tinymist_palette(state, [[0, 0, 0, 255], [255, 255, 255, 255]])

    def test_stale_or_wrong_palette_cannot_pass(self):
        state = self.observation()
        state["age_ms"] = 501
        with self.assertRaisesRegex(AssertionError, "stale"):
            runner.assert_tinymist_palette(state, [[0, 0, 0, 255], [255, 255, 255, 255]])
        with self.assertRaisesRegex(AssertionError, "paper"):
            runner.assert_tinymist_palette(self.observation(), [[255, 255, 255, 255], [0, 0, 0, 255]])


class ConcurrentRecordingContract(unittest.TestCase):
    def test_native_and_snapshot_records_are_complete_atomic_lines(self):
        first_write = threading.Event()
        release_first = threading.Event()
        second_attempt = threading.Event()
        operations = []
        failures = []

        class ContendedLock:
            def __init__(self):
                self.lock = threading.Lock()
                self.guard = threading.Lock()
                self.attempts = 0

            def __enter__(self):
                with self.guard:
                    self.attempts += 1
                    if self.attempts == 2:
                        second_attempt.set()
                self.lock.acquire()

            def __exit__(self, *_):
                self.lock.release()

        class InterruptedWriter:
            def write(self, line):
                self.action = runner.json.loads(line)["action"]
                operations.append(("write", self.action))
                if self.action == "native":
                    first_write.set()
                    if not release_first.wait(2):
                        raise AssertionError("first record was not released")

            def flush(self):
                operations.append(("flush", self.action))

        journey = object.__new__(runner.Journey)
        journey.log = InterruptedWriter()
        journey.record_lock = ContendedLock()

        def record(action):
            try:
                journey.record(action, value="x" * 16384)
            except BaseException as error:
                failures.append(error)

        first = threading.Thread(target=record, args=("native",))
        second = threading.Thread(target=record, args=("snapshot",))
        first.start()
        try:
            self.assertTrue(first_write.wait(1), "first record did not start")
            second.start()
            self.assertTrue(second_attempt.wait(1), "concurrent record bypassed serialization")
            self.assertEqual(operations, [("write", "native")])
        finally:
            release_first.set()
            first.join(2)
            if second.ident is not None:
                second.join(2)
        self.assertFalse(first.is_alive() or second.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(operations, [("write", "native"), ("flush", "native"),
                                      ("write", "snapshot"), ("flush", "snapshot")])


class SettingsSectionContract(unittest.TestCase):
    def test_form_search_remains_available_after_json_settings_edits(self):
        for initially_json in (False, True):
            with self.subTest(json_mode=initially_json):
                journey = object.__new__(runner.Journey)
                state = {"settings_json": {"visible": initially_json}}
                actions = []
                journey.snapshot = lambda: {"document": state}

                def click(name):
                    actions.append(("click", name))
                    if name == "settings.json.mode":
                        state["settings_json"]["visible"] = False
                    elif name == "settings.search" and state["settings_json"]["visible"]:
                        raise AssertionError("form search is unavailable in JSON mode")

                def wait(description, predicate):
                    self.assertTrue(predicate(state), description)

                journey.click = click
                journey.wait = wait
                journey.key = lambda *args: actions.append(("key", *args))
                journey.native = lambda *args: actions.append(args)
                journey.wait_target = lambda name: actions.append(("target", name))
                journey.setting_section("backend", "Typst preview backend")
                expected = [("click", "settings.json.mode")] if initially_json else []
                expected += [("click", "settings.search"), ("key", 0), ("text", "backend"),
                             ("target", "settings.result.Typst preview backend"),
                             ("click", "settings.result.Typst preview backend")]
                self.assertEqual(actions, expected)


class CopyIsolationContract(unittest.TestCase):
    def test_copy_acknowledges_fixture_match_and_releases_adapter_on_success_or_failure(self):
        import io
        from types import SimpleNamespace
        for matches, click_fails in [(True, False), (False, False), (True, True)]:
            with self.subTest(matches=matches, click_fails=click_fails):
                journey = object.__new__(runner.Journey)
                journey.driver = Path("private-driver")
                journey.process = SimpleNamespace(pid=123)
                events, restored = [], []
                child = SimpleNamespace(
                    stdin=io.StringIO(),
                    stdout=io.StringIO(runner.json.dumps({"ready": "copy"}) + "\n"
                                      + runner.json.dumps({"matches": matches}) + "\n"),
                    returncode=0,
                    communicate=lambda value, timeout: (restored.append(value) or "", ""),
                )
                def click(name):
                    events.append(name)
                    if click_fails:
                        raise RuntimeError("click failed")
                journey.click = click
                journey.record = lambda *args, **details: events.append((args, details))
                with patch("subprocess.Popen", return_value=child), \
                     patch("select.select", return_value=([child.stdout], [], [])), \
                     patch("time.monotonic", side_effect=[0, 7]):
                    if click_fails:
                        with self.assertRaisesRegex(RuntimeError, "click failed"):
                            journey.copy_from_menu("private fixture text")
                    elif matches:
                        journey.copy_from_menu("private fixture text")
                    else:
                        with self.assertRaisesRegex(AssertionError, "selected fixture"):
                            journey.copy_from_menu("private fixture text")
                self.assertEqual(restored, ["restore\n"])
                self.assertEqual(events[0], "menu.Copy")
                self.assertNotIn("private fixture text", repr(events))
                if matches and not click_fails:
                    self.assertEqual(events[1][0], ("clipboard.copy",))


class ClipboardAdapterLifetimeContract(unittest.TestCase):
    def test_copy_and_paste_timeout_terminate_then_kill_and_reap_if_needed(self):
        import io
        from types import SimpleNamespace
        for operation in ("copy", "paste"):
            for needs_kill in (False, True):
                with self.subTest(operation=operation, needs_kill=needs_kill):
                    journey = object.__new__(runner.Journey)
                    journey.driver = Path("private-driver")
                    journey.process = SimpleNamespace(pid=123)
                    journey.click = lambda name: None
                    journey.source_is = lambda text: None
                    journey.record = lambda *args, **details: None
                    calls = []
                    attempts = 0
                    first = {"ready": "copy"} if operation == "copy" else {"posted": "paste"}
                    child = SimpleNamespace(
                        stdin=io.StringIO(),
                        stdout=io.StringIO(runner.json.dumps(first) + "\n"
                                          + runner.json.dumps({"matches": True}) + "\n"),
                        returncode=None,
                        terminate=lambda: calls.append(("terminate",)),
                        kill=lambda: calls.append(("kill",)),
                    )

                    def communicate(value=None, timeout=None):
                        nonlocal attempts
                        attempts += 1
                        calls.append(("communicate", value, timeout))
                        if attempts == 1 or (attempts == 2 and needs_kill):
                            raise runner.subprocess.TimeoutExpired("private-driver", timeout)
                        child.returncode = -9 if needs_kill else -15
                        return "", ""

                    child.communicate = communicate
                    with patch("subprocess.Popen", return_value=child), \
                         patch("select.select", return_value=([child.stdout], [], [])):
                        with self.assertRaisesRegex(RuntimeError, "restoration could not be verified"):
                            if operation == "copy":
                                journey.copy_from_menu("private fixture text")
                            else:
                                journey.paste_source("private fixture text")
                    expected = [("communicate", "restore\n", 6), ("terminate",),
                                ("communicate", None, 2)]
                    if needs_kill:
                        expected += [("kill",), ("communicate", None, 2)]
                    self.assertEqual(calls, expected)
                    self.assertIsNotNone(child.returncode, "adapter was not reaped")


class PreviewDragContract(unittest.TestCase):
    def test_complete_displacement_passes(self):
        from desktop_ui_newest_journeys import assert_preview_drag_position
        assert_preview_drag_position({"preview_controls_position": [124, 112]}, [100, 100], [24, 12])

    def test_half_distance_and_stationary_drift_fail(self):
        from desktop_ui_newest_journeys import assert_preview_drag_position
        for actual in ([112, 106], [132, 112], [124, 119]):
            with self.assertRaisesRegex(AssertionError, "drifted"):
                assert_preview_drag_position({"preview_controls_position": actual}, [100, 100], [24, 12])

    def test_missing_or_nonfinite_geometry_fails(self):
        from desktop_ui_newest_journeys import assert_preview_drag_position
        for actual in (None, [1], [math.nan, 112], [124, math.inf]):
            with self.assertRaisesRegex(AssertionError, "invalid"):
                assert_preview_drag_position({"preview_controls_position": actual}, [100, 100], [24, 12])

    def test_held_drag_always_releases_on_failure(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        journey.wait_target = lambda _: None
        document = {"preview_controls_position": [100, 100], "preview_controls_local_position": [100, 100],
                    "preview_controls_size": [360, 108], "native_size": [1200, 800], "interface_scale": 1}
        journey.snapshot = lambda: {"document": document, "targets": {"preview.drag_handle": {"x": 110, "y": 110}}}
        actions = []
        journey.native = lambda *args: actions.append(args)
        journey.wait = lambda *args: (_ for _ in ()).throw(AssertionError("deliberate failure"))
        with self.assertRaisesRegex(AssertionError, "deliberate failure"):
            journey.stationary_preview_drag()
        self.assertEqual(actions[0], ("drag-start", 110, 110))
        self.assertEqual(actions[-1], ("drag-end", 134, 122))


    def test_missing_mouse_down_receipt_still_releases(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        journey.wait_target = lambda _: None
        document = {"preview_controls_position": [100, 100], "preview_controls_local_position": [100, 100],
                    "preview_controls_size": [360, 108], "native_size": [1200, 800], "interface_scale": 1}
        journey.snapshot = lambda: {"document": document, "targets": {"preview.drag_handle": {"x": 110, "y": 110}}}
        actions = []

        def native(*args):
            actions.append(args)
            if args[0] == "drag-start":
                raise RuntimeError("missing input receipt")

        journey.native = native
        with self.assertRaisesRegex(RuntimeError, "missing input receipt"):
            journey.stationary_preview_drag()
        self.assertEqual(actions, [("drag-start", 110, 110), ("drag-end", 134, 122)])

    def test_query_focus_wait_observes_the_next_fresh_frame(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        states = iter([
            {"targets": {"preview.query": {"enabled": True, "age_ms": 0, "focused": True, "frame": 7}},
             "document": {"find_visible": False}},
            {"targets": {"preview.query": {"enabled": True, "age_ms": 0, "focused": False, "frame": 8}},
             "document": {"find_visible": False}},
            {"targets": {"preview.query": {"enabled": True, "age_ms": 0, "focused": True, "frame": 9}},
             "document": {"find_visible": False}},
        ])
        journey.snapshot = lambda: next(states)
        with patch("desktop_ui_newest_journeys.time.sleep"):
            journey.wait_preview_query_focus()
        self.assertEqual(list(states), [])



class TableDraftContract(unittest.TestCase):
    def test_table_journey_is_registered_with_the_newest_native_group(self):
        from desktop_ui_newest_journeys import NEWEST_JOURNEYS, NewestJourneys
        self.assertIn("tables", NEWEST_JOURNEYS)
        self.assertTrue(callable(NewestJourneys.tables))

    def test_a_closed_or_previously_painted_table_widget_cannot_pass_readiness(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        def sample(opened, frame, enabled):
            return {"document": {"table_editor_open": opened}, "targets": {"table.source": {
                "enabled": enabled, "age_ms": 0, "frame": frame}}}
        states = iter([sample(False, 10, True), sample(True, 10, True),
                       sample(True, 11, False), sample(True, 12, True)])
        journey.snapshot = lambda: next(states)
        with patch("desktop_ui_newest_journeys.time.sleep"):
            target = journey.table_target("table.source", previous_frame=10)
        self.assertEqual(target["frame"], 12)
        self.assertEqual(list(states), [])

    def test_invalid_draft_waits_for_disabled_apply_in_a_fresh_open_dialog(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        states = iter([
            {"document": {"table_editor_open": True}, "targets": {"table.apply": {
                "enabled": True, "age_ms": 0, "frame": 10}}},
            {"document": {"table_editor_open": True}, "targets": {"table.apply": {
                "enabled": False, "age_ms": 800, "frame": 11}}},
            {"document": {"table_editor_open": True}, "targets": {"table.apply": {
                "enabled": False, "age_ms": 0, "frame": 12}}},
        ])
        journey.snapshot = lambda: next(states)
        with patch("desktop_ui_newest_journeys.time.sleep"):
            target = journey.table_target("table.apply", enabled=False)
        self.assertEqual(target["frame"], 12)
        self.assertEqual(list(states), [])


if __name__ == "__main__":
    unittest.main()
