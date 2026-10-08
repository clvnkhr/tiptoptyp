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


class FailureObservationContract(unittest.TestCase):
    def test_failure_metadata_only_reads_foreground_preflight_and_windows(self):
        import json
        from types import SimpleNamespace
        actions = []
        observations = {
            "foreground": {"pid": 901, "bundle": "com.example.other-app"},
            "preflight": {"desktop": False, "locked": True, "accessibility": True, "input": True},
            "windows": {"windows": [{"AXTitle": "journey.typ — tiptoptyp", "AXFocused": True}]},
        }

        def probe(arguments, **kwargs):
            self.assertEqual(kwargs, {"capture_output": True, "text": True, "timeout": 6})
            actions.append(arguments)
            command = arguments[1]
            self.assertIn(command, observations)
            return SimpleNamespace(stdout=json.dumps(observations[command]), stderr="",
                                   returncode=2 if command == "preflight" else 0)

        journey = SimpleNamespace(driver=Path("/private/tmp/test-driver"), process=SimpleNamespace(pid=123),
                                  native=lambda *args: self.fail("failure evidence must not dispatch or recover input"),
                                  record=lambda *args, **kwargs: None)
        result = {"failure": "bottom panel did not close", "passed": False, "journeys": []}
        with patch.object(runner.subprocess, "run", side_effect=probe):
            runner.observe_failed_interaction(journey, result)
        self.assertEqual(actions, [[str(journey.driver), "foreground", "123"],
                                   [str(journey.driver), "preflight"],
                                   [str(journey.driver), "windows", "123"]])
        self.assertEqual(result["failure_foreground"], observations["foreground"])
        self.assertEqual(result["failure_preflight"], observations["preflight"])
        self.assertEqual(result["failure_native_windows"], observations["windows"])
        self.assertEqual(result["failure_observation_exits"], {"foreground": 0, "preflight": 2, "windows": 0})
        self.assertEqual(result["failure"], "bottom panel did not close")
        self.assertFalse(result["passed"])
        self.assertEqual(result["journeys"], [])

    def test_unavailable_metadata_does_not_replace_the_original_failure_or_retry(self):
        from types import SimpleNamespace
        original = AssertionError("original interaction failure")
        journey = SimpleNamespace(driver=Path("/private/tmp/test-driver"), process=SimpleNamespace(pid=123),
                                  native=lambda *args: self.fail("unexpected native dispatch"),
                                  record=lambda *args, **kwargs: None)
        result = {"failure": str(original), "passed": False}
        with patch.object(runner.subprocess, "run", side_effect=RuntimeError("observation unavailable")) as probe:
            with self.assertRaises(AssertionError) as caught:
                try:
                    raise original
                except AssertionError:
                    runner.observe_failed_interaction(journey, result)
                    raise
        self.assertIs(caught.exception, original)
        self.assertEqual(probe.call_count, 3)
        self.assertEqual(result["failure_observation_errors"], {
            "foreground": "observation unavailable", "preflight": "observation unavailable",
            "windows": "observation unavailable",
        })
        self.assertEqual(result["failure"], str(original))
        self.assertFalse(result["passed"])


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
                journey.wait_target = lambda name, focused=False: actions.append(("target", name, focused))
                journey.setting_section("backend", "Typst preview backend")
                expected = [("click", "settings.json.mode")] if initially_json else []
                expected += [("click", "settings.search"), ("target", "settings.search", True),
                             ("key", 0), ("text", "backend"),
                             ("target", "settings.result.Typst preview backend", False),
                             ("click", "settings.result.Typst preview backend")]
                self.assertEqual(actions, expected)


    def test_settings_search_without_keyboard_ownership_never_receives_typing(self):
        journey = object.__new__(runner.Journey)
        state = {"document": {"settings_json": {"visible": False}}, "targets": {
            "settings.search": {"enabled": True, "age_ms": 0, "focused": False},
            "settings.result.Typst preview backend": {"enabled": True, "age_ms": 0, "focused": False},
        }}
        actions = []
        journey.snapshot = lambda: state
        journey.click = lambda target: actions.append(("click", target))
        journey.key = lambda *args: actions.append(("key", *args))
        journey.native = lambda *args: actions.append(args)
        with patch.object(runner.time, "monotonic", side_effect=[0, 0, 7]), \
                patch.object(runner.time, "sleep"):
            with self.assertRaisesRegex(AssertionError, "target did not become visible and focused: settings.search"):
                journey.setting_section("backend", "Typst preview backend")
        self.assertEqual(actions, [("click", "settings.search")])

    def test_focus_readiness_rejects_fresh_unowned_stale_and_disabled_targets(self):
        journey = object.__new__(runner.Journey)
        targets = iter([
            {"enabled": True, "age_ms": 0, "focused": False},
            {"enabled": True, "age_ms": 600, "focused": True},
            {"enabled": False, "age_ms": 0, "focused": True},
            {"enabled": True, "age_ms": 0, "focused": True},
        ])
        journey.snapshot = lambda: {"targets": {"settings.search": next(targets)}}
        with patch.object(runner.time, "sleep"):
            journey.wait_target("settings.search", focused=True)
        self.assertEqual(list(targets), [])


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



class ProblemsPanelPreparationContract(unittest.TestCase):
    def test_shortcut_handoff_is_tested_after_preparing_the_remembered_tab(self):
        from desktop_ui_newest_journeys import NewestJourneys
        for initial_tab in ("Problems", "Terminal", "Activity"):
            with self.subTest(initial_tab=initial_tab):
                journey = NewestJourneys()
                remembered = [initial_tab]
                state = {"preview_controls_open": False, "panel": None, "find_visible": False,
                         "preview_keyboard_focused": False, "problems_keyboard_focused": False,
                         "editor_keyboard_focused": False, "bottom_panel_keyboard_focused": False,
                         "explorer_focused": None,
                         "source_fingerprint": "fixture", "source_bytes": 7,
                         "page_dark": False, "cursor": [0, 0]}
                actions = []
                handoffs = []
                journey.snapshot = lambda: {"document": dict(state)}
                journey.newest_preview_document = lambda: None
                journey.choose_backend = lambda backend: actions.append(("backend", backend))

                def panel(tab):
                    remembered[0] = tab
                    state["panel"] = tab
                    state["problems_keyboard_focused"] = tab == "Problems"
                    state["bottom_panel_keyboard_focused"] = True
                    state["preview_keyboard_focused"] = False
                    state["editor_keyboard_focused"] = False

                def click(target):
                    actions.append(("click", target))
                    if target == "preview.page":
                        state["preview_keyboard_focused"] = True
                        state["problems_keyboard_focused"] = False
                        state["editor_keyboard_focused"] = False
                    elif target == "panel.close":
                        state["panel"] = None
                        state["problems_keyboard_focused"] = False
                    elif target == "panel.problems":
                        panel("Problems")
                    elif target == "panel.activity":
                        panel("Activity")
                    elif target == "problems.input":
                        self.assertEqual(state["panel"], "Problems")
                    elif target == "editor.source":
                        state["editor_keyboard_focused"] = True
                        state["bottom_panel_keyboard_focused"] = False
                    else:
                        self.fail("unexpected setup click: " + target)

                def key(code, modifiers="cmd"):
                    actions.append(("key", code, modifiers))
                    if code == 23:
                        if state["panel"] is None:
                            panel(remembered[0])
                        else:
                            state["panel"] = None
                            state["problems_keyboard_focused"] = False
                    elif code == 6 and state["editor_keyboard_focused"]:
                        state["source_fingerprint"] = "fixture"

                def wait(description, predicate):
                    self.assertTrue(predicate(state), description)
                    if description == "Problems claims keyboard from preview":
                        # Preparation must precede preview focus: clicking the
                        # tab after the shortcut would mask a failed handoff.
                        self.assertEqual(actions[-1], ("key", 23, "cmd"))
                        self.assertEqual(actions[-2], ("click", "preview.page"))
                        handoffs.append(remembered[0])

                def terminal_handoff(backend, unchanged):
                    self.assertTrue(unchanged(state))
                    remembered[0] = "Terminal"
                    state["panel"] = None
                    state["preview_keyboard_focused"] = False
                    state["problems_keyboard_focused"] = False
                    state["editor_keyboard_focused"] = False

                journey.click = click
                journey.key = key
                journey.wait = wait
                journey.native = lambda *args: state.update(source_fingerprint="edited") \
                    if state["editor_keyboard_focused"] else None
                journey.stable_state = lambda predicate, **kwargs: self.assertTrue(predicate(state))
                journey.paste_source = lambda text, check: check()
                journey.preview_terminal_handoff = terminal_handoff
                journey.problems_focus()
                self.assertEqual(handoffs, ["Problems", "Problems"])


class TerminalHandoffContract(unittest.TestCase):
    def test_repeated_backend_handoffs_prepare_a_pane_before_the_focus_shortcut(self):
        from desktop_ui_newest_journeys import NewestJourneys
        import shlex
        import tempfile
        for initially_off in (False, True):
            with self.subTest(initially_off=initially_off), tempfile.TemporaryDirectory() as directory:
                journey = NewestJourneys()
                journey.directory = Path(directory)
                state = {"panel": None, "terminal_off": initially_off,
                         "terminal_keyboard_focused": False, "preview_keyboard_focused": False,
                         "editor_keyboard_focused": False, "explorer_focused": None}
                actions = []
                journey.snapshot = lambda: {"document": state.copy()}

                def click(target):
                    actions.append(("click", target))
                    if target == "preview.page":
                        state["preview_keyboard_focused"] = True
                        state["terminal_keyboard_focused"] = False
                    elif target == "terminal.new":
                        self.assertEqual(state["panel"], "Terminal")
                        state["terminal_off"] = False
                    elif target == "terminal.off":
                        state["terminal_off"] = True
                        state["terminal_keyboard_focused"] = False
                    elif target == "panel.close":
                        state["panel"] = None
                        state["terminal_keyboard_focused"] = False
                    else:
                        self.fail(f"unexpected compensating click: {target}")

                def key(code, modifiers):
                    actions.append(("key", code, modifiers))
                    if code == 50:
                        state["panel"] = "Terminal"
                        state["preview_keyboard_focused"] = False
                        state["terminal_keyboard_focused"] = not state["terminal_off"]
                    elif code == 17:
                        self.assertTrue(state["terminal_keyboard_focused"])

                def native(operation, command):
                    self.assertEqual(operation, "text")
                    self.assertTrue(state["terminal_keyboard_focused"])
                    parts = shlex.split(command)
                    Path(parts[-1]).write_text(parts[2])

                journey.click = click
                journey.key = key
                journey.native = native
                journey.wait = lambda description, predicate: self.assertTrue(predicate(state), description)
                journey.wait_target = lambda _: self.assertTrue(state["terminal_keyboard_focused"])
                journey.stable_state = lambda predicate, **_: self.assertTrue(predicate(state))
                journey.record = lambda *_, **__: None
                for backend in ("Pdfium", "Interactive"):
                    journey.preview_terminal_handoff(backend, lambda _: True)
                self.assertTrue(state["terminal_off"])
                self.assertEqual(actions.count(("click", "terminal.new")), 2 if initially_off else 1)


class InitialReviewCaptureContract(unittest.TestCase):
    def test_native_window_focus_does_not_replace_explicit_source_keyboard_focus(self):
        from desktop_ui_journeys import EditorJourneys
        journey = EditorJourneys()
        state = {"focused": True, "editor_keyboard_focused": False}
        actions = []
        journey.focus_document_window = lambda: actions.append("document window")
        journey.wait_target = lambda target: actions.append(("ready", target))

        def click(target):
            self.assertEqual(target, "editor.source")
            state["editor_keyboard_focused"] = True
            actions.append(("click", target))

        def wait(description, predicate):
            self.assertTrue(predicate(state), description)
            actions.append("source focus verified")

        def capture(target):
            self.assertTrue(state["editor_keyboard_focused"], "native focus alone cannot authorize the initial capture")
            actions.append(("capture", target))

        journey.click = click
        journey.wait = wait
        journey.capture_viewport = capture
        journey.capture_initial_review()
        self.assertEqual(actions, ["document window", ("ready", "editor.source"),
                                  ("click", "editor.source"), "source focus verified", ("capture", "main")])

    def test_initial_capture_waits_for_the_click_to_claim_the_keyboard(self):
        from desktop_ui_journeys import EditorJourneys
        journey = EditorJourneys()
        state = {"focused": True, "editor_keyboard_focused": False}
        captured = []
        journey.focus_document_window = lambda: None
        journey.wait_target = lambda target: None
        journey.click = lambda target: None
        journey.wait = lambda description, predicate: self.assertTrue(predicate(state), description)
        journey.capture_viewport = lambda target: captured.append(target)
        with self.assertRaisesRegex(AssertionError, "source owns keyboard before initial review capture"):
            journey.capture_initial_review()
        self.assertEqual(captured, [])


class MathConversionPreparationContract(unittest.TestCase):
    def test_exact_source_journeys_disable_and_restore_formatting_for_both_defaults(self):
        from desktop_ui_journeys import EditorJourneys
        for name in ("git_hunks", "context_menu"):
            for initial in (False, True):
                for fail in (False, True):
                    with self.subTest(journey=name, initial=initial, fail=fail):
                        journey = EditorJourneys()
                        state = {"format_after_math_conversion": initial}
                        actions = []
                        journey.snapshot = lambda: {"document": dict(state)}
                        journey.settings = lambda: actions.append("settings")
                        journey.setting_section = lambda query, title: actions.append((query, title))

                        def click(target):
                            self.assertEqual(target, "settings.editor.math_format")
                            state["format_after_math_conversion"] = not state["format_after_math_conversion"]
                            actions.append(target)

                        def wait(description, predicate):
                            self.assertTrue(predicate(state), description)
                            actions.append("verified")

                        def body():
                            self.assertFalse(state["format_after_math_conversion"])
                            actions.append("exact source and single undo")
                            if fail:
                                raise RuntimeError("fixture assertion failed")

                        journey.click = click
                        journey.wait = wait
                        journey.close_settings = lambda: actions.append("close settings")
                        setattr(journey, "_" + name, body)
                        if fail:
                            with self.assertRaisesRegex(RuntimeError, "fixture assertion failed"):
                                getattr(journey, name)()
                        else:
                            getattr(journey, name)()
                        self.assertIs(state["format_after_math_conversion"], initial)
                        toggle = ["settings", ("format after math conversion", "Format after math conversion"),
                                  "settings.editor.math_format", "verified", "close settings"]
                        expected = toggle + ["exact source and single undo"] + toggle if initial else ["exact source and single undo"]
                        self.assertEqual(actions, expected)


    def test_clicking_the_toggle_is_not_proof_the_effective_preference_changed(self):
        from desktop_ui_journeys import EditorJourneys
        journey = EditorJourneys()
        state = {"format_after_math_conversion": True}
        actions = []
        journey.snapshot = lambda: {"document": dict(state)}
        journey.settings = lambda: None
        journey.setting_section = lambda *args: None
        journey.click = lambda target: actions.append(target)
        journey.wait = lambda description, predicate: self.assertTrue(predicate(state), description)
        journey.close_settings = lambda: actions.append("close settings")
        with self.assertRaisesRegex(AssertionError, "math conversion formatting disabled"):
            journey.set_math_conversion_formatting(False)
        self.assertEqual(actions, ["settings.editor.math_format"])


class TableDraftContract(unittest.TestCase):
    def test_table_journey_is_registered_with_the_newest_native_group(self):
        from desktop_ui_newest_journeys import NEWEST_JOURNEYS, NewestJourneys
        self.assertIn("tables", NEWEST_JOURNEYS)
        self.assertTrue(callable(NewestJourneys.tables))

    def test_native_tex_open_selects_the_file_without_relaxing_owner_checks(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        journey.viewport = '"FFFF"'
        journey.multiple_windows = False
        path = Path("/private/tmp/private-table-fixture/table-workbench.tex")
        actions = []
        journey.key = lambda *args: actions.append(("key", *args))
        journey.native = lambda *args: actions.append(args)

        def wait(description, predicate, timeout):
            self.assertEqual(description, "private TeX table opens")
            self.assertEqual(timeout, 15)
            self.assertFalse(predicate({"path": None}))
            self.assertFalse(predicate({"path": str(path.parent)}))
            self.assertTrue(predicate({"path": str(path)}))
            actions.append(("opened",))

        journey.wait = wait
        with patch("desktop_ui_newest_journeys.time.sleep"):
            journey.open_table_fixture(path)
        self.assertEqual(actions, [
            ("key", 31), ("key", 5, "cmd+shift"), ("key", 0),
            ("text", str(path)), ("key", 36, "none"),
            ("ax-press-title", "Open"), ("opened",),
        ])
        self.assertEqual(journey.viewport, '"FFFF"')
        self.assertFalse(journey.multiple_windows)

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
