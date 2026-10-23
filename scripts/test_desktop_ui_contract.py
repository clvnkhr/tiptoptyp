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


class TargetedSnapshotContract(unittest.TestCase):
    def snapshot(self, document_owner, requested):
        import json
        from types import SimpleNamespace
        from unittest.mock import MagicMock
        journey = object.__new__(runner.Journey)
        journey.process = SimpleNamespace(pid=42, poll=lambda: None)
        journey.directory = Path('/private/tmp/test-inspection')
        journey.sequence, journey.viewport, journey.multiple_windows = 0, None, True
        journey.record = lambda *args, **kwargs: None
        connection = MagicMock()
        connection.__enter__.return_value = connection
        connection.makefile.return_value.readline.return_value = json.dumps({
            'pid': 42, 'request': 1, 'document': {'viewport': document_owner}}).encode()
        with patch.object(runner.socket, 'socket', return_value=connection):
            result = journey.snapshot(viewport=requested)
        return result, connection

    def test_requested_owner_is_sent_over_the_read_only_socket(self):
        result, connection = self.snapshot('17F2', '17F2')
        connection.sendall.assert_called_once_with(b'snapshot 17F2\n')
        self.assertEqual(result['document']['viewport'], '17F2')

    def test_sibling_frame_is_rejected_even_in_multiwindow_journey(self):
        with self.assertRaisesRegex(AssertionError, 'another document owner'):
            self.snapshot('FFFF', '17F2')

    def test_untargeted_observation_still_uses_bare_snapshot(self):
        _, connection = self.snapshot('FFFF', None)
        connection.sendall.assert_called_once_with(b'snapshot\n')


class FixtureFingerprintContract(unittest.TestCase):
    def test_matches_rust_probe_vectors_without_logging_source(self):
        from desktop_ui_journeys import fingerprint
        self.assertEqual(fingerprint(""), "cbf29ce484222325")
        self.assertEqual(fingerprint("hello"), "a430d84680aabd0b")
        self.assertEqual(fingerprint("é🙂z"), "3a046d85bff8a56f")


class TypingParticleContract(unittest.TestCase):
    def document(self):
        return {"editor_effects": {"enabled": True, "density": "Medium", "actions": {
                    action.lower(): {"particles": "Explosion", "shake": False, "flash": False, "glow": False}
                    for action in ("Typing", "Newline", "Indent", "Delete", "Comment", "Undo", "Redo", "Copy", "Cut", "Paste")}},
                "typing_particles": {"active_count": 7, "burst_count": 1,
                    "emitted_count": 7, "max_live": 7, "pool_limit": 128,
                    "lifetime_ms": 480, "last_emission_age_ms": 100,
                    "origin": [200, 100], "clip": [10, 10, 800, 500],
                    "last_style": "Explosion", "live_styles": {"Explosion": 7},
                    "last_action": "Typing", "action_bursts": {"Typing": 1},
                    "feedback": {"burst_count": 0, "action_bursts": {}, "last_action": None,
                        "last_effects": [], "active_effects": [], "shake_offset": [0, 0],
                        "flash_alpha": 0, "glow_alpha": 0, "lifetime_ms": 480,
                        "last_emission_age_ms": None}}}

    def test_per_action_preferences_require_every_action_and_reject_legacy_or_invalid_options(self):
        from copy import deepcopy
        from desktop_ui_journeys import editor_effect_preferences
        document = self.document()
        self.assertEqual(editor_effect_preferences(document)["actions"]["typing"]["particles"], "Explosion")
        document["editor_effects"]["actions"]["copy"] = {
            "particles": None, "shake": True, "flash": False, "glow": True}
        self.assertIsNone(editor_effect_preferences(document)["actions"]["copy"]["particles"])
        mutations = [lambda d: d.pop("editor_effects"),
                     lambda d: d.update(editor_effects={"enabled": True, "density": "Medium", "style": "Explosion"}),
                     lambda d: d["editor_effects"].update(enabled=1),
                     lambda d: d["editor_effects"].update(density="Ultra"),
                     lambda d: d["editor_effects"]["actions"].pop("paste"),
                     lambda d: d["editor_effects"]["actions"].update(save={}),
                     lambda d: d["editor_effects"]["actions"]["copy"].update(particles="Off"),
                     lambda d: d["editor_effects"]["actions"]["typing"].update(particles="Unknown"),
                     lambda d: d["editor_effects"]["actions"]["typing"].update(shake="true"),
                     lambda d: d["editor_effects"]["actions"]["typing"].pop("glow")]
        for mutate in mutations:
            changed = deepcopy(document)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                editor_effect_preferences(changed)

    def test_actual_per_action_chooser_updates_only_its_row_and_records_with_real_signature(self):
        from copy import deepcopy
        from io import StringIO
        import json
        from desktop_ui_journeys import EditorJourneys
        case = self

        class FakeJourney(EditorJourneys):
            # Exercise the runner's real event writer/signature, rather than a
            # permissive **kwargs stub that can hide argument-name collisions.
            record = runner.Journey.record

            def __init__(self, mutate=None):
                self.document = case.gpu_document()
                self.document["typing_particles"].update(active_count=0, live_styles={}, last_emission_age_ms=900)
                self.log, self.record_lock = StringIO(), threading.Lock()
                self.calls, self.open, self.menu, self.mutate = [], False, None, mutate

            def snapshot(self):
                return {"document": deepcopy(self.document)}

            def settings(self):
                self.open = True

            def setting_section(self, query, title):
                case.assertTrue(self.open)
                case.assertEqual((query, title), ("editor effects", "Editor effects"))

            def wait_target(self, target):
                case.assertEqual(target, self.menu + "." + self.option)

            def wait(self, description, predicate):
                case.assertTrue(predicate(deepcopy(self.document)), description)

            def stable_state(self, predicate, frames):
                for _ in range(frames):
                    case.assertTrue(predicate(deepcopy(self.document)), "effect history changed")

            def close_settings(self):
                self.open = False

        def click(journey, target):
            case.assertTrue(journey.open)
            journey.calls.append(target)
            action, control, *option = target.split("settings.editor.effects.", 1)[1].split(".")
            row = journey.document["editor_effects"]["actions"][action]
            if control == "particles":
                if option:
                    case.assertEqual(target, journey.menu + "." + journey.option)
                    row[control] = None if option[0] == "Off" else option[0]
                    journey.menu = None
                else:
                    journey.menu = target
            else:
                row[control] = not row[control]
            if journey.mutate:
                journey.mutate(journey.document)
        FakeJourney.click = click

        for effects in ({"particles": None, "shake": True, "flash": False, "glow": True},
                        {"particles": "Plasma", "shake": True, "flash": True, "glow": False}):
            journey = FakeJourney()
            journey.option = effects["particles"] or "Off"
            before = deepcopy(journey.document)
            journey.editor_effect_choose("Copy", **effects)
            expected = deepcopy(before)
            expected["editor_effects"]["actions"]["copy"] = effects
            self.assertEqual(journey.document, expected)
            self.assertFalse(journey.open)
            event = json.loads(journey.log.getvalue())
            self.assertEqual(event["action"], "typing_particles.action_preferences")
            self.assertEqual(event["editor_action"], "Copy")
            self.assertEqual(event["effects"], effects)
            self.assertTrue(all(target.startswith("settings.editor.effects.copy.") for target in journey.calls))

        mutations = [lambda d: d["editor_effects"]["actions"]["paste"].update(glow=True),
                     lambda d: d["typing_particles"].update(burst_count=2, emitted_count=14,
                         last_action="Copy", action_bursts={"Typing": 1, "Copy": 1})]
        for mutate in mutations:
            journey = FakeJourney(mutate)
            journey.option = "Off"
            with self.subTest(mutate=mutate), self.assertRaises(AssertionError):
                journey.editor_effect_choose("Copy", particles=None)

    def test_actual_bounded_fresh_emission_and_expired_counter_history_are_distinct(self):
        from desktop_ui_journeys import typing_particle_state
        document = self.document()
        self.assertEqual(typing_particle_state(document)["active_count"], 7)
        document["typing_particles"].update(active_count=0, live_styles={}, last_emission_age_ms=900)
        self.assertEqual(typing_particle_state(document)["burst_count"], 1)

    def test_missing_unbounded_disabled_or_expired_particles_cannot_pass(self):
        from desktop_ui_journeys import typing_particle_state
        cases = [{}, {"active_count": 129}, {"max_live": 129},
                 {"pool_limit": 257}, {"lifetime_ms": 1001}, {"active_count": True},
                 {"last_emission_age_ms": 481}, {"last_emission_age_ms": True},
                 {"emitted_count": 6}, {"max_live": 6}, {"burst_count": 8},
                 {"origin": [900, 100]}, {"origin": [True, 100]},
                 {"clip": [200, 100, 200, 100]},
                 {"origin": [float("nan"), 100]}, {"last_style": None},
                 {"last_style": "Unknown"}, {"live_styles": None},
                 {"live_styles": {"Unknown": 7}}, {"live_styles": {"Explosion": 6}},
                 {"live_styles": {"Explosion": True}},
                 {"live_styles": {"Explosion": 7, "Plasma": 0}},
                 {"last_action": None}, {"last_action": "Unknown"}, {"last_action": "Copy"},
                 {"action_bursts": None}, {"action_bursts": {}},
                 {"action_bursts": {"Unknown": 1}}, {"action_bursts": {"Typing": True}},
                 {"action_bursts": {"Typing": 1, "Copy": 0}},
                 {"action_bursts": {"Typing": 2}}]
        for change in cases:
            document = self.document()
            document["typing_particles"] = ({**document["typing_particles"], **change}
                                               if change else {})
            with self.subTest(change=change), self.assertRaises(AssertionError):
                typing_particle_state(document)
        document = self.document()
        document["editor_effects"]["enabled"] = False
        with self.assertRaises(AssertionError):
            typing_particle_state(document)


    def test_birth_styles_remain_valid_when_the_selected_preference_changes(self):
        from desktop_ui_journeys import typing_particle_state
        document = self.document()
        document["editor_effects"]["actions"]["typing"]["particles"] = "Shotgun"
        self.assertEqual(typing_particle_state(document)["live_styles"], {"Explosion": 7})
        document["typing_particles"]["live_styles"] = {"Explosion": 4, "Shotgun": 3}
        document["typing_particles"]["last_style"] = "Shotgun"
        self.assertEqual(sum(typing_particle_state(document)["live_styles"].values()), 7)
        document["editor_effects"]["actions"]["typing"]["particles"] = "Unknown"
        with self.assertRaises(AssertionError):
            typing_particle_state(document)


    def gpu_document(self):
        document = self.document()
        document["typing_particles"].update(pool_limit=256, lifetime_ms=640, gpu={
            "backend": "instanced-glow", "ready": True, "draws": 3, "uploads": 1,
            "uploaded_instances": 7, "last_draw_instances": 7, "error": None})
        return document

    def test_gpu_callback_evidence_accepts_bounded_draws_and_preparation_is_distinct(self):
        from desktop_ui_journeys import typing_particle_gpu_state
        document = self.gpu_document()
        self.assertEqual(typing_particle_gpu_state(document)["draws"], 3)
        document["typing_particles"].update(active_count=0, live_styles={})
        document["typing_particles"]["gpu"].update(
            ready=False, draws=0, uploads=0, uploaded_instances=0, last_draw_instances=0)
        self.assertFalse(typing_particle_gpu_state(document)["ready"])

    def test_selected_popup_uses_current_ime_anchor_instead_of_the_old_collapsed_caret(self):
        from copy import deepcopy
        from desktop_ui_journeys import editor_particle_popup_target
        current = {"enabled": True, "age_ms": 1, "frame": 42, "viewport": "source",
                   "x": 150, "y": 250}
        state = {"document": {"cursor": [8, 24], "frame": 42, "editor_keyboard_focused": True},
                 "targets": {"editor.source": {**current, "x": 400, "y": 200, "width": 600, "height": 300},
                             "editor.ime_cursor": current,
                             "editor.caret": {**current, "frame": 35, "age_ms": 8000, "x": 300}}}
        self.assertEqual(editor_particle_popup_target(state), "editor.ime_cursor")
        for change in ({"age_ms": 501}, {"frame": 41}, {"viewport": "other"}, {"x": 1000}):
            changed = deepcopy(state)
            changed["targets"]["editor.ime_cursor"].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                editor_particle_popup_target(changed)
        state["document"]["cursor"] = [24, 24]
        state["targets"]["editor.caret"] = dict(current)
        self.assertEqual(editor_particle_popup_target(state), "editor.caret")

    def test_one_unchanged_source_copy_burst_needs_the_exact_cause_and_a_new_gpu_draw(self):
        from copy import deepcopy
        from desktop_ui_journeys import particle_action_rendered, fingerprint
        before = self.gpu_document()
        before.update(source_fingerprint=fingerprint("alpha"), source_bytes=5,
                      editor_keyboard_focused=True)
        before["editor_effects"]["actions"]["copy"]["particles"] = "Railgun"
        before["editor_effects"]["actions"]["typing"]["particles"] = "Plasma"
        actual = deepcopy(before)
        state = actual["typing_particles"]
        state.update(last_style="Railgun", live_styles={"Railgun": 7},
                     burst_count=2, emitted_count=14, last_action="Copy",
                     action_bursts={"Typing": 1, "Copy": 1})
        state["gpu"].update(draws=4, uploads=2)
        self.assertTrue(particle_action_rendered(actual, before, "Copy", "alpha"))
        cases = [lambda d: d.update(source_fingerprint=fingerprint("changed")),
                 lambda d: d.update(editor_keyboard_focused=False),
                 lambda d: d["typing_particles"]["gpu"].update(draws=3),
                 lambda d: d["typing_particles"].update(last_style="Plasma", live_styles={"Plasma": 7}),
                 lambda d: d["typing_particles"].update(last_action="Typing", action_bursts={"Typing": 2}),
                 lambda d: d["typing_particles"].update(burst_count=3, action_bursts={"Typing": 1, "Copy": 2})]
        for mutate in cases:
            changed = deepcopy(actual)
            mutate(changed)
            with self.subTest(changed=changed):
                self.assertFalse(particle_action_rendered(changed, before, "Copy", "alpha"))

    def test_document_return_requires_a_reset_trace_and_no_gpu_replay_before_rebaselining(self):
        from copy import deepcopy
        from desktop_ui_journeys import particle_document_returned_quiet, fingerprint
        prior = self.gpu_document()
        prior.update(active_tab_id=7, source_fingerprint=fingerprint("alpha"), source_bytes=5)
        returned = deepcopy(prior)
        returned["typing_particles"].update(active_count=0, burst_count=0, emitted_count=0,
            max_live=0, last_style=None, live_styles={}, last_action=None, action_bursts={})
        self.assertTrue(particle_document_returned_quiet(returned, prior, "alpha"))
        cases = [lambda d: d.update(active_tab_id=8),
                 lambda d: d.update(source_fingerprint=fingerprint("other")),
                 lambda d: d["typing_particles"]["gpu"].update(draws=4),
                 lambda d: d["typing_particles"].update(emitted_count=7),
                 lambda d: d["typing_particles"].update(max_live=7, emitted_count=7),
                 lambda d: d.update(typing_particles=deepcopy(prior["typing_particles"]))]
        for mutate in cases:
            changed = deepcopy(returned)
            mutate(changed)
            with self.subTest(changed=changed):
                self.assertFalse(particle_document_returned_quiet(changed, prior, "alpha"))

    def test_missing_cpu_only_broken_or_unbounded_gpu_evidence_is_rejected(self):
        from desktop_ui_journeys import typing_particle_gpu_state
        for gpu in (None, {}, {"backend": "cpu"}, {"ready": "yes"},
                    {"draws": True}, {"uploads": -1}, {"error": "shader compile failed"},
                    {"uploaded_instances": 257}, {"last_draw_instances": 257}):
            document = self.gpu_document()
            original = document["typing_particles"]["gpu"]
            document["typing_particles"]["gpu"] = ({**original, **gpu} if gpu else gpu)
            with self.subTest(gpu=gpu), self.assertRaises(AssertionError):
                typing_particle_gpu_state(document)


class EditorFeedbackContract(unittest.TestCase):
    def feedback_document(self, effect="Shake"):
        document = TypingParticleContract().gpu_document()
        document["typing_particles"].update(active_count=0, live_styles={})
        document["editor_effects"]["actions"]["typing"] = {
            "particles": None, "shake": effect == "Shake", "flash": effect == "Flash", "glow": effect == "Glow"}
        return document

    def emitted_document(self, effect="Shake"):
        document = self.feedback_document(effect)
        document["typing_particles"]["feedback"].update(
            burst_count=1, action_bursts={"Typing": 1}, last_action="Typing", last_effects=[effect],
            active_effects=[effect], shake_offset=([2.5, -1] if effect == "Shake" else [0, 0]),
            flash_alpha=(0.2 if effect == "Flash" else 0), glow_alpha=(0.3 if effect == "Glow" else 0),
            last_emission_age_ms=50)
        return document

    def test_feedback_requires_actual_amplitude_and_own_cause_without_particle_gpu_work(self):
        from copy import deepcopy
        from desktop_ui_journeys import editor_feedback_action_rendered, fingerprint
        for effect in ("Shake", "Flash", "Glow"):
            before = self.feedback_document(effect)
            before.update(source_fingerprint=fingerprint("a"), source_bytes=1, editor_keyboard_focused=True)
            actual = self.emitted_document(effect)
            actual.update(source_fingerprint=fingerprint("ax"), source_bytes=2, editor_keyboard_focused=True)
            self.assertTrue(editor_feedback_action_rendered(actual, before, "Typing", effect, "ax"))
            mutations = [lambda d: d.update(editor_keyboard_focused=False),
                         lambda d: d.update(source_fingerprint=fingerprint("wrong")),
                         lambda d: d["typing_particles"]["feedback"].update(last_action="Copy", action_bursts={"Copy": 1}),
                         lambda d: d["typing_particles"]["feedback"].update(burst_count=2, action_bursts={"Typing": 2}),
                         lambda d: d["typing_particles"]["feedback"].update(active_effects=[], shake_offset=[0, 0], flash_alpha=0, glow_alpha=0),
                         lambda d: d["typing_particles"]["feedback"].update(shake_offset=[0, 0], flash_alpha=0, glow_alpha=0),
                         lambda d: d["typing_particles"]["gpu"].update(draws=4),
                         lambda d: d["typing_particles"]["gpu"].update(uploads=2),
                         lambda d: d["typing_particles"].update(burst_count=2, emitted_count=14, action_bursts={"Typing": 2})]
            for mutate in mutations:
                changed = deepcopy(actual)
                mutate(changed)
                with self.subTest(effect=effect, changed=changed):
                    self.assertFalse(editor_feedback_action_rendered(changed, before, "Typing", effect, "ax"))

    def test_missing_invalid_expired_or_disabled_feedback_cannot_pass(self):
        from copy import deepcopy
        from desktop_ui_journeys import editor_feedback_state
        document = self.emitted_document()
        self.assertEqual(editor_feedback_state(document)["active_effects"], ["Shake"])
        mutations = [lambda d: d["typing_particles"].pop("feedback"),
                     lambda d: d["editor_effects"].update(enabled=False),
                     lambda d: d["typing_particles"]["feedback"].update(burst_count=True),
                     lambda d: d["typing_particles"]["feedback"].update(action_bursts={"Copy": 2}),
                     lambda d: d["typing_particles"]["feedback"].update(last_action="Missing"),
                     lambda d: d["typing_particles"]["feedback"].update(lifetime_ms=1001),
                     lambda d: d["typing_particles"]["feedback"].update(last_effects=["Shake", "Shake"]),
                     lambda d: d["typing_particles"]["feedback"].update(active_effects=["Flash"]),
                     lambda d: d["typing_particles"]["feedback"].update(active_effects=None),
                     lambda d: d["typing_particles"]["feedback"].update(shake_offset=[float("nan"), 0]),
                     lambda d: d["typing_particles"]["feedback"].update(shake_offset=[33, 0]),
                     lambda d: d["typing_particles"]["feedback"].update(flash_alpha=True),
                     lambda d: d["typing_particles"]["feedback"].update(glow_alpha=1.1),
                     lambda d: d["typing_particles"]["feedback"].update(last_emission_age_ms=None),
                     lambda d: d["typing_particles"]["feedback"].update(last_emission_age_ms=480),
                     lambda d: d["typing_particles"]["feedback"].update(active_effects=[])]
        for mutate in mutations:
            changed = deepcopy(document)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                editor_feedback_state(changed)
        expired = deepcopy(document)
        expired["typing_particles"]["feedback"].update(active_effects=[], shake_offset=[0, 0], last_emission_age_ms=900)
        self.assertEqual(editor_feedback_state(expired)["burst_count"], 1)
        expired["editor_effects"]["enabled"] = False
        self.assertEqual(editor_feedback_state(expired)["active_effects"], [])


class PartialFixtureCleanupContract(unittest.TestCase):
    def test_mru_selection_never_closes_an_original_tab(self):
        from desktop_ui_partial_journeys import PartialPreviewJourneys
        class Session(PartialPreviewJourneys):
            def __init__(self):
                self.tabs = [10, 20, 30, 40]
                self.active = 30
                self.preview = 30
                self.backend = "InteractivePartial"
                self.closed = []
            def focus_document_window(self):
                pass
            def snapshot(self):
                return {"document": {"tabs": len(self.tabs), "preview_requested_backend": self.backend}}
            def click(self, name):
                if name.startswith("tab."):
                    self.active = int(name.split(".")[1])
                    assert self.active in self.tabs
            def key(self, code, flags):
                self.preview = self.active
            def close_scratch(self, count):
                self.closed.append(self.active)
                self.tabs.remove(self.active)
                # Deliberately return to the original MRU, not the next fixture.
                self.active = 10
                assert len(self.tabs) == count
            def choose_backend(self, backend, save_pdf):
                assert self.preview == 20
                self.backend = backend
        session = Session()
        before = {"active_tab_id": 10, "preview_tab_id": 20, "preview_requested_backend": "Interactive"}
        session.restore_partial_fixture_tabs(before, [30, 40])
        self.assertEqual(session.closed, [40, 30])
        self.assertEqual(session.tabs, [10, 20])
        self.assertEqual((session.active, session.preview, session.backend), (10, 20, "Interactive"))


class CachedEdgeWheelContract(unittest.TestCase):
    def run_journey(self, baseline=None, after=None):
        from desktop_ui_partial_journeys import PartialPreviewJourneys
        class Session(PartialPreviewJourneys):
            def __init__(self):
                self.now = 0.0
                self.wheel_at = None
                self.records = []
            def click(self, name):
                assert name == "preview.page"
            def key(self, code):
                assert code == 115
            def wait(self, description, predicate):
                assert predicate({"preview_page": 0})
            def snapshot(self):
                state = {"document": {"preview_zoom_overlay": {
                    "cached": True, "active": False, "capturing": False,
                    "prepared": False, "deferred": False, "pending": None,
                    "in_flight": None, "token": "image:23", "scale": 1,
                    "view_size": [558, 829]}},
                    "renderer": {"age_ms": 0, "value": {"ready": True,
                        "scroll": [0, 0], "zoom": 1, "zoom_snapshot": {
                            "enabled": True, "cached": True, "active": False,
                            "capturing": False, "admitting": False,
                            "handoff_pending": False, "request": None, "frame_ready": True}}},
                    "targets": {"preview.page": {"x": 300, "y": 400}}}
                mutate = after if self.wheel_at is not None else baseline
                if mutate:
                    mutate(state, self.now)
                return state
            def native(self, command, *args):
                if command == "wheel":
                    self.wheel_at = self.now
            def record(self, name, **details):
                self.records.append((name, details))
            def sleep(self, seconds):
                self.now += seconds
        session = Session()
        with patch("desktop_ui_partial_journeys.time.monotonic", side_effect=lambda: session.now), \
                patch("desktop_ui_partial_journeys.time.sleep", side_effect=session.sleep):
            session.cached_edge_wheel("partial")
        return session

    def test_capture_replacement_finishes_before_token_identity_is_baselined(self):
        def refreshing(state, now):
            if now < .1:
                state["document"]["preview_zoom_overlay"].update(
                    token="image:21", capturing=True)
        session = self.run_journey(baseline=refreshing)
        self.assertGreaterEqual(session.wheel_at, .35)
        self.assertEqual(session.records[0][1]["before"]["token"], "image:23")
        self.assertEqual(session.records[0][1]["after"]["token"], "image:23")

    def test_busy_or_stale_samples_reset_the_quiet_baseline(self):
        cases = [{"cached": False}, {"active": True}, {"capturing": True},
                 {"prepared": True}, {"deferred": True}, {"pending": 7},
                 {"in_flight": 7}, {"token": None}, {"age_ms": 501},
                 {"ready": False}, {"scroll": [0, 4]}, {"scale": 1.1},
                 {"view_size": [600, 829]}]
        for change in cases:
            def transient(state, now):
                if .1 <= now < .2:
                    native = state["document"]["preview_zoom_overlay"]
                    renderer = state["renderer"]
                    if "age_ms" in change:
                        renderer.update(change)
                    elif "ready" in change or "scroll" in change:
                        renderer["value"].update(change)
                    else:
                        native.update(change)
            with self.subTest(change=change):
                session = self.run_journey(baseline=transient)
                self.assertGreaterEqual(session.wheel_at, .45)

    def test_native_idle_does_not_hide_the_prior_javascript_handoff(self):
        # run-0hd6r980/event892: native was idle but the last JS observation
        # still owned revision 25 and had its two-frame handoff pending.
        def finishing(state, now):
            if now < .4:
                state["renderer"]["value"]["zoom_snapshot"].update(
                    active=True, handoff_pending=True, request={"revision": 25})
        session = self.run_journey(baseline=finishing)
        self.assertGreaterEqual(session.wheel_at, .65)

    def test_baseline_requires_explicit_quiet_javascript_snapshot(self):
        invalid = {"enabled": False, "cached": False, "active": True,
                   "capturing": True, "admitting": True, "handoff_pending": True,
                   "request": {"revision": 25}, "frame_ready": False}
        for missing in (False, True):
            for field, value in invalid.items():
                def transient(state, now):
                    if .1 <= now < .2:
                        snapshot = state["renderer"]["value"]["zoom_snapshot"]
                        if missing:
                            snapshot.pop(field)
                        else:
                            snapshot[field] = value
                with self.subTest(missing=missing, field=field):
                    self.assertGreaterEqual(self.run_journey(baseline=transient).wheel_at, .45)

    def test_post_wheel_requires_explicit_idle_javascript_owner(self):
        invalid = {"enabled": False, "cached": False, "active": True,
                   "capturing": None, "admitting": True, "handoff_pending": True,
                   "request": {"revision": 25}, "frame_ready": None}
        for missing in (False, True):
            for field, value in invalid.items():
                def unfinished(state, now):
                    snapshot = state["renderer"]["value"]["zoom_snapshot"]
                    if missing:
                        snapshot.pop(field)
                    else:
                        snapshot[field] = value
                with self.subTest(missing=missing, field=field), self.assertRaisesRegex(
                        AssertionError, "did not retain an idle top-edge preview"):
                    self.run_journey(after=unfinished)

    def test_actual_cache_loss_or_replacement_after_idle_wheel_still_fails(self):
        for change in ({"cached": False}, {"token": "replacement:99"}):
            def lost(state, now):
                state["document"]["preview_zoom_overlay"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(
                    AssertionError, "discarded idle preview pixels"):
                self.run_journey(after=lost)

    def test_post_wheel_native_gesture_or_missing_state_cannot_pass(self):
        for change in ({"active": True}, {"prepared": True}, {"deferred": True},
                       {"pending": 7}, {"in_flight": 7}, {"capturing": None},
                       {"active": None}):
            def unfinished(state, now):
                state["document"]["preview_zoom_overlay"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(
                    AssertionError, "did not retain an idle top-edge preview"):
                self.run_journey(after=unfinished)

    def test_post_wheel_stale_unready_or_unclamped_renderer_cannot_pass(self):
        cases = [("age_ms", value) for value in (501, -1, True, float("nan"))]
        cases += [("ready", False), ("scroll", None), ("scroll", [0]),
                  ("scroll", [0, 200]), ("scroll", [0, -1]),
                  ("scroll", [0, True]), ("scroll", [0, float("nan")])]
        for field, value in cases:
            def invalid(state, now):
                renderer = state["renderer"]
                (renderer if field == "age_ms" else renderer["value"])[field] = value
            with self.subTest(field=field, value=value), self.assertRaisesRegex(
                    AssertionError, "did not retain an idle top-edge preview"):
                self.run_journey(after=invalid)

    def test_post_wheel_retains_valid_pixels_during_background_capture(self):
        # run-gu27t4hv: full mode started a background refresh after the wheel,
        # while the same token remained usable and the ready renderer stayed at 0.
        def refreshing(state, now):
            state["document"]["preview_zoom_overlay"]["capturing"] = True
            state["renderer"]["value"]["zoom_snapshot"]["frame_ready"] = False
        session = self.run_journey(after=refreshing)
        self.assertEqual(session.records[0][1]["before"]["token"], "image:23")
        self.assertEqual(session.records[0][1]["after"]["token"], "image:23")
        self.assertTrue(session.records[0][1]["after"]["capturing"])

    def test_permanently_busy_preview_exhausts_the_bounded_wait(self):
        def busy(state, now):
            state["document"]["preview_zoom_overlay"]["capturing"] = True
        with self.assertRaisesRegex(AssertionError, "no idle cached preview"):
            self.run_journey(baseline=busy)


class PreviewAnchorContract(unittest.TestCase):
    def test_original_source_point_detects_a_shared_center_anchor_error(self):
        from desktop_ui_partial_journeys import source_point,project_source_point
        before={"paper_scale":2,"paper_bounds":[0,-40,600,1200],"page_bounds":[[0,-40,600,1200]]}
        point=source_point(before,100,200)
        correct={"paper_scale":3,"paper_bounds":[-50,-160,900,1800],"page_bounds":[[-50,-160,900,1800]]}
        centered={"paper_scale":3,"paper_bounds":[-50,-165,900,1800],"page_bounds":[[-50,-165,900,1800]]}
        self.assertEqual(project_source_point(correct,point),[100,200])
        self.assertEqual(project_source_point(centered,point),[100,195])


    def test_recorded_full_preview_fractional_inset_maps_the_exact_visible_top(self):
        from desktop_ui_partial_journeys import source_point, project_source_point
        # run-_ov9ukop/event926: the first page's SVG transform starts 0.429px
        # below the viewport despite scrollTop=0; this is valid leading padding.
        before = {"paper_scale": .9362416107382551,
                  "paper_bounds": [0, -1, 558, 38310],
                  "viewport_bounds": [0, -1, 558, 829],
                  "page_bounds": [[0, -.5707712769508362, 558, 788.3154362416108]]}
        point = source_point(before, 0, -1)
        self.assertEqual(point["page"], 0)
        self.assertLess(point["local"], 0)
        self.assertEqual(project_source_point(before, point), [0, -1])
        scale = 1.2
        top = -1 - point["local"] * scale
        correct = {"paper_scale": scale, "paper_bounds": [0, -1, 715, 49102],
                   "page_bounds": [[0, top, 715, 1010]]}
        self.assertEqual(project_source_point(correct, point), [0, -1])
        incorrect = {**correct, "page_bounds": [[0, top + 4, 715, 1010]]}
        self.assertEqual(project_source_point(incorrect, point), [0, 3])

    def test_outer_page_padding_and_interpage_gap_keep_distinct_coordinates(self):
        from desktop_ui_partial_journeys import source_point, project_source_point
        before = {"paper_scale": 2, "paper_bounds": [0, 0, 600, 220],
                  "page_bounds": [[0, 10, 600, 100], [0, 130, 600, 80]]}
        for y, expected in ((-5, {"page": 0, "local": -7.5}),
                            (120, {"before": 0, "after": 1, "fraction": .5}),
                            (220, {"page": 1, "local": 45})):
            with self.subTest(y=y):
                point = source_point(before, 60, y)
                self.assertEqual(point, {"x": 30, **expected})
                self.assertEqual(project_source_point(before, point), [60, y])
        for pages, y in (([], -1), ([None], -1),
                         ([None, [0, 10, 600, 100]], -1),
                         ([[0, 10, 600, 100], None], 220)):
            with self.subTest(pages=pages), self.assertRaises(AssertionError):
                source_point({**before, "page_bounds": pages}, 0, y)


class PartialPreviewCoverageContract(unittest.TestCase):
    def observation(self):
        return {"age_ms": 0, "value": {"ready": True, "partial": True,
            "pages": 48, "populated": 3, "canvases": 0,
            "visible": [{"index": 0, "dummy": False, "ink": True}]}}

    def test_accepts_actual_bounded_svg_coverage(self):
        from desktop_ui_partial_journeys import assert_partial_coverage
        self.assertEqual(assert_partial_coverage(self.observation())["populated"], 3)

    def test_rejects_missing_stale_wrong_mode_and_skeleton_only_coverage(self):
        from desktop_ui_partial_journeys import assert_partial_coverage
        cases = [None, {**self.observation(), "age_ms": 501}]
        for changed in ({"partial": False}, {"pages": 3}, {"visible": []},
                        {"visible": [{"index": 0, "dummy": True, "ink": False}]}):
            cases.append({"age_ms": 0, "value": {**self.observation()["value"], **changed}})
        for case in cases:
            with self.subTest(case=case), self.assertRaises(AssertionError):
                assert_partial_coverage(case)

    def test_rejects_full_document_or_offscreen_canvas_work(self):
        from desktop_ui_partial_journeys import assert_partial_coverage
        for changed in ({"populated": 48}, {"canvases": 47}):
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                assert_partial_coverage({"age_ms": 0,
                    "value": {**self.observation()["value"], **changed}})

    def test_missing_or_malformed_current_coverage_cannot_pass(self):
        from desktop_ui_partial_journeys import assert_partial_coverage
        original = self.observation()["value"]
        cases = []
        for missing in ("populated", "canvases"):
            cases.append({key: value for key, value in original.items() if key != missing})
        for field in ("populated", "canvases"):
            for value in (None, True, False, -1, 1.5, float("nan")):
                cases.append({**original, field: value})
        cases.extend(({**original, "populated": 0},
                      {**original, "populated": 1, "visible": [
                          {"index": 0, "dummy": False, "ink": True},
                          {"index": 1, "dummy": False, "ink": True}]}))
        for value in cases:
            with self.subTest(value=value), self.assertRaises(AssertionError):
                assert_partial_coverage({"age_ms": 0, "value": value})

    def test_visible_pages_need_unique_in_range_indices_and_real_ink_flags(self):
        from desktop_ui_partial_journeys import assert_partial_coverage
        original = self.observation()["value"]
        page = original["visible"][0]
        cases = [[{key: value for key, value in page.items() if key != missing}]
                 for missing in ("index", "dummy", "ink")]
        cases.extend([[{**page, "index": value}] for value in (None, True, -1, 48, 0.5)])
        cases.extend(([page, page], [{**page, "dummy": 0}], [{**page, "ink": 1}]))
        for visible in cases:
            with self.subTest(visible=visible), self.assertRaises(AssertionError):
                assert_partial_coverage({"age_ms": 0, "value": {**original, "visible": visible}})

    def test_fresh_default_requires_selected_and_actual_partial_renderer(self):
        from desktop_ui_partial_journeys import assert_partial_default
        document = {"preview_requested_backend": "InteractivePartial",
                    "preview_backend": "Tinymist partial"}
        renderer = self.observation()
        assert_partial_default(document, renderer)
        for changed in ({"preview_requested_backend": "Interactive"},
                        {"preview_requested_backend": "Pdfium"},
                        {"preview_backend": "PDFium"}):
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                assert_partial_default({**document, **changed}, renderer)
        for wrong in (None, {**renderer, "age_ms": 501},
                      {"age_ms": 0, "value": {"ready": True, "partial": False}},
                      {"age_ms": 0, "value": {"ready": False, "partial": True}}):
            with self.subTest(renderer=wrong), self.assertRaises(AssertionError):
                assert_partial_default(document, wrong)

    def test_zoom_checks_rendered_scale_and_accumulation_not_just_percentage(self):
        from desktop_ui_partial_journeys import assert_zoom_geometry
        baseline = {"zoom": 1, "paper_scale": 1.5}
        renderer = self.observation()
        renderer["value"].update(zoom=1.1, paper_scale=1.65)
        assert_zoom_geometry(renderer, baseline, 1.1)
        for changed in ({"paper_scale": 1.501}, {"zoom": 1.001},
                        {"paper_scale": None}, {"paper_scale": float("nan")},
                        {"paper_scale": True}, {"ready": False}):
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                assert_zoom_geometry({"age_ms": 0, "value": {**renderer["value"], **changed}}, baseline, 1.1)
        with self.assertRaises(AssertionError):
            assert_zoom_geometry({**renderer, "age_ms": 501}, baseline, 1.1)


class ZoomBurstCoalescingContract(unittest.TestCase):
    def observation(self, inputs=10, hits=8):
        return {"inputs": inputs, "hits": hits, "deferred": False,
                "active": False, "in_flight": None, "pending": None}

    def test_requires_all_twenty_cached_inputs_and_one_final_vector_request(self):
        from desktop_ui_partial_journeys import assert_zoom_burst
        before, after = self.observation(), self.observation(inputs=30, hits=28)
        assert_zoom_burst(before, after, 1)
        for requests in (0, 2, True, None):
            with self.subTest(requests=requests), self.assertRaises(AssertionError):
                assert_zoom_burst(before, after, requests)
        for mutation in ({"inputs": 29}, {"hits": 27}, {"hits": 29},
                         {"deferred": True}, {"active": True},
                         {"pending": 5}, {"in_flight": 5}):
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                assert_zoom_burst(before, {**after, **mutation}, 1)

    def test_missing_legacy_marker_or_current_counters_cannot_bypass_coalescing(self):
        from desktop_ui_partial_journeys import assert_zoom_burst
        before, after = self.observation(), self.observation(inputs=30, hits=28)
        for field in before:
            for baseline in (True, False):
                earlier = {key: value for key, value in before.items() if key != field} if baseline else before
                later = after if baseline else {key: value for key, value in after.items() if key != field}
                with self.subTest(field=field, baseline=baseline), self.assertRaises(AssertionError):
                    assert_zoom_burst(earlier, later, 1)
        for field in ("inputs", "hits"):
            for value in (None, True, -1, 1.5):
                with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                    assert_zoom_burst({**before, field: value}, after, 1)
        for field in ("deferred", "active"):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_zoom_burst({**before, field: 0}, after, 1)


class HarperMathProseContract(unittest.TestCase):
    def observation(self, locations=None):
        from desktop_ui_journeys import fingerprint
        locations = [] if locations is None else locations
        source = "Correct mathematical prose.\n"
        path = Path("/private/tmp/harper-fixture/prose.typ")
        return source, path, {
            "path": str(path), "source_fingerprint": fingerprint(source),
            "source_bytes": len(source.encode("utf-8")), "writing_current": True,
            "writing_pending": False, "writing_count": len(locations),
            "writing_locations": locations, "writing_lines": [value[0] for value in locations],
            "writing_paths": [str(path)] * len(locations), "writing_fingerprint": "published",
        }

    def test_journey_is_registered_and_uses_both_checked_in_grammatical_fixtures(self):
        from desktop_ui_tex_journeys import TEX_JOURNEYS, TexJourneys
        self.assertIn("harper_math_prose", TEX_JOURNEYS)
        self.assertTrue(callable(TexJourneys.harper_math_prose))
        for suffix in ("typ", "tex"):
            source = (Path(__file__).resolve().parent.parent / "manual-tests" /
                      ("harper-math-prose." + suffix)).read_text()
            self.assertIn("well-posedness", source)
            self.assertIn("ill-posedness", source)
            self.assertIn("nonautonomous", source)
            self.assertIn("x = y,", source)
            self.assertIn("z = x;", source)
            self.assertNotIn("misspelld", source)
            self.assertNotIn("This is the the", source)

    def test_harper_settings_journey_requires_applied_form_preferences(self):
        from desktop_ui_journeys import fingerprint
        from desktop_ui_tex_journeys import TEX_JOURNEYS, TexJourneys, harper_settings_applied
        self.assertIn("harper_settings", TEX_JOURNEYS)
        self.assertTrue(callable(TexJourneys.harper_settings))
        document = {"harper_dictionary_fingerprint": fingerprint("qzorthal"),
                    "harper_ignore_capitalized_names": True}
        self.assertTrue(harper_settings_applied(document, "qzorthal", True))
        self.assertFalse(harper_settings_applied(document, "", True))
        self.assertFalse(harper_settings_applied(document, "qzorthal", False))
        self.assertFalse(harper_settings_applied({}, "qzorthal", True))
        self.assertFalse(harper_settings_applied(
            {**document, "harper_ignore_capitalized_names": 1}, "qzorthal", True))

    def test_harper_rule_journey_rejects_stale_or_destructive_filter_results(self):
        from desktop_ui_journeys import fingerprint
        from desktop_ui_tex_journeys import harper_rule_result
        source, path = "Short sentence.  Next sentence.", Path("/tmp/rule-test.typ")
        locations = [[1, 12], [2, 12], [3, 26], [4, 9]]
        document = {"path": str(path), "source_fingerprint": fingerprint(source),
                    "writing_current": True, "writing_pending": False,
                    "harper_rule_overrides": {"NoFrenchSpaces": True},
                    "writing_rules": ["SpellCheck", "SpellCheck", "SpellCheck", "RepeatedWords", "NoFrenchSpaces"],
                    "writing_count": 5, "writing_locations": [*locations, [5, 26]],
                    "writing_paths": [str(path)] * 5}
        self.assertTrue(harper_rule_result(document, source, path, locations, True))
        for field, wrong in (("writing_current", False), ("writing_pending", True),
                             ("source_fingerprint", "stale"), ("writing_count", 1),
                             ("writing_locations", [[5, 26]]),
                             ("writing_locations", [*locations, [5, 25]]),
                             ("writing_paths", ["/tmp/generated.typ"] * 5),
                             ("writing_rules", ["SpellCheck"]),
                             ("harper_rule_overrides", {"NoFrenchSpaces": 1})):
            with self.subTest(field=field):
                self.assertFalse(harper_rule_result({**document, field: wrong}, source, path, locations, True))

    def test_quiet_badness_journey_requires_visible_correctness_reports(self):
        from desktop_ui_tex_journeys import quiet_badness_result
        document = {"tex_diagnostics_current": [True, True], "badness_visible_codes": ["undefined-label"]}
        self.assertTrue(quiet_badness_result(document))
        for value in ([], ["redundant-script-braces"], ['"redundant-script-braces"'], [None]):
            self.assertFalse(quiet_badness_result({**document, "badness_visible_codes": value}))
        self.assertFalse(quiet_badness_result({**document, "tex_diagnostics_current": [True, False]}))

    def test_an_idle_old_writing_result_cannot_pass_freshness(self):
        from desktop_ui_tex_journeys import harper_fresh
        source, path, document = self.observation()
        self.assertTrue(harper_fresh(document, source, path, []))
        for field, wrong in (("writing_current", False), ("writing_pending", True),
                             ("source_fingerprint", "old-source"), ("source_bytes", 0),
                             ("path", str(path.parent))):
            with self.subTest(field=field):
                self.assertFalse(harper_fresh({**document, field: wrong}, source, path, []))

    def test_false_positive_wrong_column_or_generated_source_cannot_pass(self):
        from desktop_ui_tex_journeys import harper_fresh
        locations = [[34, 26], [35, 9]]
        source, path, document = self.observation(locations)
        self.assertTrue(harper_fresh(document, source, path, locations))
        for field, wrong in (("writing_count", 3), ("writing_lines", [34, 34]),
                             ("writing_locations", [[34, 25], [35, 9]]),
                             ("writing_paths", [str(path), "/private/tmp/.tiptoptyp/generated.typ"])):
            with self.subTest(field=field):
                self.assertFalse(harper_fresh({**document, field: wrong}, source, path, locations))

    def test_clearing_or_changing_any_stale_diagnostic_field_is_rejected(self):
        from desktop_ui_tex_journeys import assert_harper_stale
        _, _, previous = self.observation([[34, 26], [35, 9]])
        stale = {**previous, "writing_pending": True, "writing_current": False}
        assert_harper_stale(stale, previous)
        for field, wrong in (("writing_count", 0), ("writing_fingerprint", "empty"),
                             ("writing_lines", []), ("writing_locations", []),
                             ("writing_paths", []), ("writing_current", True),
                             ("writing_pending", False)):
            with self.subTest(field=field):
                with self.assertRaises(AssertionError):
                    assert_harper_stale({**stale, field: wrong}, previous)

    def test_open_uses_the_real_file_path_and_requires_source_keyboard_ownership(self):
        from desktop_ui_tex_journeys import TexJourneys
        journey = TexJourneys()
        path = Path("/private/tmp/harper-fixture/prose.tex")
        actions = []
        journey.focus_document_window = lambda: actions.append(("focus",))
        journey.key = lambda *args: actions.append(("key", *args))
        def native(*args):
            actions.append(args)
            if args[0] == "ax-button-title":
                return dict(requested_title=args[1], count=1, complete=True,
                            role="AXButton", title=args[1], enabled=True)
            if args[0] == "ax-dialog-state":
                return dict(complete=True, focused_ancestry=[],
                            focused_window=dict(role="AXSheet", identifier="GoToWindow"))
        journey.native = native
        journey.click = lambda *args: actions.append(("click", *args))

        def wait(description, predicate, **kwargs):
            if description == "named Harper fixture opened":
                self.assertFalse(predicate({"path": str(path.parent)}))
                self.assertTrue(predicate({"path": str(path)}))
            else:
                self.assertEqual(description, "Harper fixture owns keyboard")
                self.assertFalse(predicate({"editor_keyboard_focused": False}))
                self.assertTrue(predicate({"editor_keyboard_focused": True}))
            actions.append(("verified", description))

        journey.wait = wait
        with patch("time.sleep"):
            journey.open_harper_fixture(path)
        self.assertEqual(actions, [
            ("focus",), ("key", 31), ("ax-button-title", "Open"),
            ("key", 5, "cmd+shift"), ("ax-dialog-state",), ("key", 0),
            ("text", str(path)), ("key", 36, "none"),
            ("ax-button-title", "Open"), ("ax-press-title", "Open"),
            ("verified", "named Harper fixture opened"), ("click", "editor.source"),
            ("verified", "Harper fixture owns keyboard"),
        ])


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


class PdfiumResizeContract(unittest.TestCase):
    def document(self):
        return {"preview_backend": "PDFium", "preview_pdfium_ready": True,
                "pdfium_render": {"requested_scale": 300, "desired_scale": 300,
                    "active_scale": 300, "resident_count": 1, "requests": 5,
                    "admissions": 5, "settling": False}}

    def test_held_resize_keeps_the_sharp_batch_and_allows_only_desired_scale_to_change(self):
        from desktop_ui_journeys import assert_pdfium_resize_held
        baseline = self.document()["pdfium_render"]
        document = self.document()
        document["preview_pdfium_ready"] = False
        document["pdfium_render"].update(desired_scale=280, settling=True)
        self.assertEqual(assert_pdfium_resize_held(document, baseline)["desired_scale"], 280)
        for field, wrong in (("requests", 6), ("admissions", 6), ("active_scale", 280),
                             ("resident_count", 0), ("settling", False)):
            changed = {**document, "pdfium_render": {**document["pdfium_render"], field: wrong}}
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_pdfium_resize_held(changed, baseline)

    def test_missing_wrong_backend_or_malformed_counters_cannot_pass(self):
        from desktop_ui_journeys import pdfium_render_state
        pdfium_render_state(self.document())
        for document in ({}, {**self.document(), "preview_backend": "Tinymist"},
                         {**self.document(), "pdfium_render": None}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                pdfium_render_state(document)
        for field in self.document()["pdfium_render"]:
            wrong = None if field == "settling" else True
            document = self.document()
            document["pdfium_render"][field] = wrong
            with self.subTest(field=field), self.assertRaises(AssertionError):
                pdfium_render_state(document)

    def test_ready_flag_or_old_active_scale_cannot_pass_final_settling(self):
        from desktop_ui_journeys import pdfium_resize_settled
        self.assertTrue(pdfium_resize_settled(self.document()))
        # TeX uses PDFium even when the saved Typst preference selects Tinymist.
        self.assertTrue(pdfium_resize_settled({**self.document(),
                                              "preview_requested_backend": "InteractivePartial"}))
        for field, wrong in (("requested_scale", 280), ("desired_scale", 280),
                             ("active_scale", 280), ("resident_count", 0),
                             ("settling", True), ("desired_scale", True)):
            document = self.document()
            document["pdfium_render"][field] = wrong
            with self.subTest(field=field):
                self.assertFalse(pdfium_resize_settled(document))
        for field, wrong in (("preview_backend", "Tinymist"), ("preview_pdfium_ready", False),
                             ("pdfium_render", None)):
            with self.subTest(field=field):
                self.assertFalse(pdfium_resize_settled({**self.document(), field: wrong}))


class BackgroundFadesContract(unittest.TestCase):
    def document(self):
        def background(identity):
            return {"translucent": True, "blur_fade": "right", "blur_radius": 8, "blur_end_radius": 0,
                    "blur": {"method": "variable-blur", "applied_fade": "right",
                             "applied_radius": 8, "identity": identity, "applications": 3}}
        return {"window_background": {"enabled": True, "blur": True, "opacity_percent": 80,
                    "blur_radius": 8, "blur_end_radius": 0, "opacity_end_percent": 100,
                    "blur_fade": "right", "transparency_fade": "down"},
                "native_background": background(101), "settings_native_background": background(202)}

    def test_independent_directions_need_actual_native_acceptance_on_both_owners(self):
        from desktop_ui_journeys import background_fades_applied
        self.assertTrue(background_fades_applied(self.document(), "right", "down"))
        for key in ("native_background", "settings_native_background"):
            for field, wrong in (("method", "window-server"), ("applied_fade", "none"),
                                 ("applied_radius", 0), ("identity", True), ("applications", 0)):
                document = self.document()
                document[key]["blur"][field] = wrong
                with self.subTest(key=key, field=field):
                    self.assertFalse(background_fades_applied(document, "right", "down"))
            document = self.document()
            document[key] = None
            self.assertFalse(background_fades_applied(document, "right", "down"))
        document = self.document()
        document["settings_native_background"]["blur"]["identity"] = 101
        self.assertFalse(background_fades_applied(document, "right", "down"))

    def test_wrong_preferences_or_fullscreen_opaque_surface_cannot_pass(self):
        from desktop_ui_journeys import background_fades_applied
        for field, wrong in (("blur_fade", "left"), ("transparency_fade", "right"),
                             ("enabled", False), ("blur", False), ("opacity_percent", 100),
                             ("opacity_percent", True), ("blur_radius", 0)):
            document = self.document()
            document["window_background"][field] = wrong
            with self.subTest(field=field):
                self.assertFalse(background_fades_applied(document, "right", "down"))
        document = self.document()
        document["native_background"]["translucent"] = False
        document["native_background"]["blur"] = None
        self.assertFalse(background_fades_applied(document, "right", "down"))

    def test_inverted_endpoints_need_the_actual_native_maximum(self):
        from desktop_ui_journeys import background_fades_applied
        document = self.document()
        document["window_background"].update(blur_radius=0, blur_end_radius=20,
                                            opacity_percent=100, opacity_end_percent=40)
        for key in ("native_background", "settings_native_background"):
            document[key].update(blur_radius=0, blur_end_radius=20)
            document[key]["blur"]["applied_radius"] = 20
        self.assertTrue(background_fades_applied(document, "right", "down"))
        document["native_background"]["blur_end_radius"] = 0
        self.assertFalse(background_fades_applied(document, "right", "down"))

    def test_focus_and_transparency_changes_cannot_reapply_or_replace_blur(self):
        from desktop_ui_journeys import background_blur_unchanged
        self.assertTrue(background_blur_unchanged(self.document(), self.document()))
        document = self.document()
        document["window_background"]["transparency_fade"] = "left"
        self.assertTrue(background_blur_unchanged(document, self.document()))
        for key in ("native_background", "settings_native_background"):
            for field, wrong in (("identity", 303), ("applications", 4),
                                 ("applied_fade", "up"), ("applied_radius", 20)):
                document = self.document()
                document[key]["blur"][field] = wrong
                with self.subTest(key=key, field=field):
                    self.assertFalse(background_blur_unchanged(document, self.document()))
        self.assertFalse(background_blur_unchanged({}, {}))


class BackendSelectionContract(unittest.TestCase):
    def test_selected_preference_is_acknowledged_before_settings_closes_without_retrying(self):
        from desktop_ui_journeys import EditorJourneys
        class Session(EditorJourneys):
            def __init__(self):
                self.open = False
                self.accepted = False
                self.clicks = []
                self.state = {"settings_json": {"visible": False}, "settings_visible": False,
                    "preview_requested_backend": "InteractivePartial", "preview_backend": "Tinymist partial",
                    "preview_pdfium_ready": False}
            def settings(self):
                self.open = True
                self.state["settings_visible"] = True
            def snapshot(self):
                return {"document": self.state}
            def setting_section(self, query, title):
                pass
            def wait_target(self, name):
                pass
            def click(self, name):
                self.clicks.append(name)
            def wait(self, description, predicate, timeout=6):
                if "preference applies" in description:
                    assert self.open
                    assert not predicate(self.state)
                    self.state["preview_requested_backend"] = "Pdfium"
                    self.accepted = True
                assert predicate(self.state)
            def close_settings(self):
                assert self.accepted, "closed Settings before its deferred preference was consumed"
                self.open = False
                self.state["settings_visible"] = False
            def key(self, code):
                assert not self.open
                self.state.update(preview_backend="PDFium", preview_pdfium_ready=True)
        session = Session()
        session.choose_backend("Pdfium")
        self.assertEqual(session.clicks, ["settings.backend.Pdfium"])


class NativeSizeContract(unittest.TestCase):
    def test_native_restore_accepts_one_point_rounding_in_either_dimension(self):
        from desktop_ui_journeys import native_size_matches
        for size in ([1200, 720], [1200.5, 719.5], [1199, 721]):
            self.assertTrue(native_size_matches(size, [1200, 720]))
        self.assertFalse(native_size_matches([1202, 720], [1200, 720]))
        self.assertFalse(native_size_matches([1200, 718], [1200, 720]))

    def test_native_restore_rejects_missing_or_nonfinite_geometry(self):
        from desktop_ui_journeys import native_size_matches
        for size in (None, [], [1200], [1200, math.nan], [math.inf, 720], [True, 720], [0, 720]):
            with self.subTest(size=size):
                self.assertFalse(native_size_matches(size, [1200, 720]))
                self.assertFalse(native_size_matches([1200, 720], size))


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

    def test_both_svg_modes_bypass_only_the_exact_identity_palette(self):
        for partial in (False, True):
            with self.subTest(partial=partial):
                state = {"age_ms": 0, "value": {"ready": True, "partial": partial,
                    "filters": ["none"], "background": "rgb(255, 255, 255)",
                    "slopes": [1, 1, 1], "intercepts": [0, 0, 0]}}
                runner.assert_tinymist_palette(state,
                    [[255, 255, 255, 255], [0, 0, 0, 255]])
                for palette in ([[254, 255, 255, 255], [0, 0, 0, 255]],
                                [[255, 255, 255, 255], [0, 0, 1, 255]],
                                [[0, 0, 0, 255], [255, 255, 255, 255]]):
                    with self.assertRaisesRegex(AssertionError, "appropriate"):
                        runner.assert_tinymist_palette(state, palette)

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
        wrong_paper = self.observation()
        wrong_paper["value"]["filters"] = ["none"]
        with self.assertRaisesRegex(AssertionError, "paper"):
            runner.assert_tinymist_palette(wrong_paper, [[255, 255, 255, 255], [0, 0, 0, 255]])


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


class SettingsHelpToolOptionsContract(unittest.TestCase):
    def journey(self, source="unchanged"):
        from desktop_ui_journeys import EditorJourneys
        journey = EditorJourneys()
        actions = []
        journey.click = lambda target: actions.append(("click", target))
        journey.help_target = lambda target, scroll_down=False: actions.append(("visible", target))
        state = {"settings_help_visible": True, "settings_visible": True,
                 "source_fingerprint": source}
        journey.wait = lambda description, predicate: self.assertTrue(predicate(state), description)
        return journey, actions

    def test_each_reference_is_opened_by_a_real_click_and_requires_its_option_label(self):
        journey, actions = self.journey()
        journey.help_tool_options("unchanged")
        for tool in ("typst", "tinymist", "tectonic", "texlab", "badness", "tex-fmt"):
            header = "settings.help.tool." + tool
            self.assertEqual(actions.count(("click", header)), 2)
            self.assertIn(("visible", "settings.help.option." + tool + ".0"), actions)
            self.assertIn(("visible", "settings.help.option." + tool + ".9"), actions)
        self.assertEqual(actions[0], ("visible", "settings.help.tools"))
        self.assertEqual(actions[1], ("click", "settings.help.tools"))
        self.assertEqual(actions[-1], ("click", "settings.help.tools"))

    def test_help_visibility_rejects_old_clipped_and_disabled_observations(self):
        from desktop_ui_journeys import help_target_visible
        name = "settings.help.option.tex-fmt.0"
        state = {"scrolls": {"settings.help": {"frame": 10}},
                 "targets": {name: {"frame": 10, "age_ms": 0, "enabled": True}}}
        self.assertTrue(help_target_visible(state, name))
        for field, value in (("frame", 9), ("age_ms", 501), ("enabled", False)):
            with self.subTest(field=field):
                changed = {**state, "targets": {name: {**state["targets"][name], field: value}}}
                self.assertFalse(help_target_visible(changed, name))
        self.assertFalse(help_target_visible({**state, "targets": {}}, name))
        self.assertFalse(help_target_visible({**state, "scrolls": {}}, name))

    def test_an_offscreen_option_requires_a_real_wheel_before_it_can_pass(self):
        from desktop_ui_journeys import EditorJourneys
        journey = EditorJourneys()
        name = "settings.help.option.tex-fmt.0"
        state = {"scrolls": {"settings.help": {"frame": 10, "x": 250, "y": 280}},
                 "targets": {name: {"frame": 9, "age_ms": 0, "enabled": True}}}
        actions = []
        journey.snapshot = lambda: state

        def native(*args):
            actions.append(args)
            if args[0] == "wheel":
                state["scrolls"]["settings.help"]["frame"] = 11
                state["targets"][name]["frame"] = 11

        journey.native = native
        with patch("time.sleep"):
            journey.help_target(name, scroll_down=True)
        self.assertEqual(actions, [("move", 250, 280), ("wheel", 250, 280, -4)])

    def test_opening_a_reference_cannot_pass_if_it_changes_source(self):
        journey, _ = self.journey("edited")
        with self.assertRaises(AssertionError):
            journey.help_tool_options("unchanged")


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


class SettingsJsonEditingContract(unittest.TestCase):
    def state(self, selection):
        return {"document": {"settings_json": {"visible": True, "fingerprint": "draft",
                                             "valid": True}},
                "targets": {"settings.json": {"enabled": True, "focused": True,
                                               "age_ms": 0, "selection": selection}}}

    def test_selection_requires_fresh_focused_unchanged_json_draft(self):
        from desktop_ui_journeys import settings_json_editor_selection
        for selection in ([0, 10], [10, 0], [10, 10]):
            self.assertEqual(settings_json_editor_selection(self.state(selection), "draft"), selection)
        for change in ({"selection": [True, 10]}, {"selection": [-1, 0]},
                       {"selection": [0]}, {"selection": None}, {"focused": False},
                       {"enabled": False}, {"age_ms": -1}, {"age_ms": 501},
                       {"age_ms": True}, {"age_ms": float("nan")}):
            state = self.state([0, 10])
            state["targets"]["settings.json"].update(change)
            with self.subTest(change=change):
                self.assertIsNone(settings_json_editor_selection(state, "draft"))
        state = self.state([0, 10])
        self.assertIsNone(settings_json_editor_selection(state, "earlier"))
        state["document"]["settings_json"]["visible"] = False
        self.assertIsNone(settings_json_editor_selection(state, "draft"))

    def test_whitespace_edit_waits_for_both_keys_and_rejects_old_valid_state(self):
        from copy import deepcopy
        journey = object.__new__(runner.Journey)
        state = self.state([6, 6])
        events, pending = [], []

        def snapshot():
            if pending:
                phase, count = pending[0]
                pending[0] = (phase, count + 1)
                if count == 1:
                    if phase == "select":
                        state["targets"]["settings.json"]["selection"] = [10, 0]
                    elif phase == "collapse":
                        state["targets"]["settings.json"]["selection"] = [10, 10]
                    else:
                        state["document"]["settings_json"]["fingerprint"] = "changed"
                    events.append(phase + ".acknowledged")
                    pending.clear()
            return deepcopy(state)

        def key(code, flags="cmd"):
            if code == 0:
                phase = "select"
            elif code == 124 and flags == "none":
                self.assertEqual(events[-1], "select.acknowledged")
                self.assertEqual(state["targets"]["settings.json"]["selection"], [10, 0])
                phase = "collapse"
            else:
                raise AssertionError((code, flags))
            events.append(phase)
            pending.append((phase, 0))

        def native(command, text):
            self.assertEqual((command, text), ("text", " "))
            self.assertEqual(events[-1], "collapse.acknowledged")
            self.assertEqual(state["targets"]["settings.json"]["selection"], [10, 10])
            events.append("text")
            pending.append(("edit", 0))

        def wait(description, predicate):
            self.assertFalse(predicate(snapshot()["document"]), "old valid draft must not acknowledge typing")
            self.assertTrue(predicate(snapshot()["document"]), description)

        journey.snapshot, journey.key, journey.native, journey.wait = snapshot, key, native, wait
        journey.wait_target = lambda name, focused=False: None
        with patch("time.sleep"):
            journey.settings_json_append_whitespace()
        self.assertEqual(events, ["select", "select.acknowledged", "collapse",
                                  "collapse.acknowledged", "text", "edit.acknowledged"])

    def test_missing_right_acknowledgement_never_types_over_selected_json(self):
        journey = object.__new__(runner.Journey)
        state = self.state([10, 0])
        journey.snapshot = lambda: state
        journey.wait_target = lambda name, focused=False: None
        journey.key = lambda *args: None
        journey.native = lambda *args: self.fail("text preceded selection collapse")
        with patch("time.monotonic", side_effect=[0, 0, 7]), patch("time.sleep"):
            with self.assertRaisesRegex(AssertionError, "Right did not collapse"):
                journey.settings_json_append_whitespace()


class SettingsNavigationContract(unittest.TestCase):
    def test_rail_or_open_selector_requires_current_settings_targets(self):
        from desktop_ui_journeys import settings_category_target_visible
        target = {"enabled": True, "age_ms": 0, "frame": 12, "viewport": "Settings"}
        state = {"targets": {"settings.category.Editor": dict(target), "settings.search": dict(target)}}
        self.assertTrue(settings_category_target_visible(state, "Editor"))
        for field, value in (("enabled", False), ("age_ms", 501), ("frame", 11), ("viewport", "main")):
            state["targets"]["settings.category.Editor"] = {**target, field: value}
            with self.subTest(field=field):
                self.assertFalse(settings_category_target_visible(state, "Editor"))
        self.assertFalse(settings_category_target_visible(state, "Tools"))

    def test_navigation_requires_category_query_form_mode_and_unchanged_source(self):
        from desktop_ui_journeys import settings_navigation_matches, fingerprint
        valid = {"settings_visible": True, "settings_json": {"visible": False},
                 "settings_help_visible": False, "settings_category": "Editor",
                 "settings_query_fingerprint": fingerprint("wrap"), "source_fingerprint": "original"}
        self.assertTrue(settings_navigation_matches(valid, "Editor", "wrap", "original"))
        for field, value in (("settings_visible", False), ("settings_json", {"visible": True}),
                             ("settings_help_visible", True), ("settings_category", "Appearance"),
                             ("settings_query_fingerprint", fingerprint("")), ("source_fingerprint", "edited")):
            with self.subTest(field=field):
                self.assertFalse(settings_navigation_matches({**valid, field: value}, "Editor", "wrap", "original"))

    def test_native_size_is_required_instead_of_assuming_a_missing_window_is_stable(self):
        from desktop_ui_journeys import settings_window_size
        settings = {"AXTitle": "tiptoptyp Settings", "size": [850, 700]}
        self.assertEqual(settings_window_size([{"AXTitle": "main"}, settings]), (850, 700))
        for windows in ([], [settings, settings], [{**settings, "size": None}],
                        [{**settings, "size": [0, 700]}], [{**settings, "size": [float("nan"), 700]}]):
            with self.subTest(windows=windows), self.assertRaises(AssertionError):
                settings_window_size(windows)


class CopyIsolationContract(unittest.TestCase):
    def test_copy_acknowledges_fixture_match_and_releases_adapter_on_success_or_failure(self):
        import io
        from types import SimpleNamespace
        for matches, click_fails, observer_fails in [
                (True, False, False), (False, False, False),
                (True, True, False), (True, False, True)]:
            with self.subTest(matches=matches, click_fails=click_fails, observer_fails=observer_fails):
                journey = object.__new__(runner.Journey)
                journey.driver = Path("private-driver")
                journey.process = SimpleNamespace(pid=123)
                events, restored = [], []
                child = SimpleNamespace(
                    stdin=io.StringIO(),
                    stdout=io.StringIO(runner.json.dumps({"ready": "copy"}) + "\n"
                                      + (runner.json.dumps({"matches": matches}) + "\n") * 2),
                    returncode=None,
                    poll=lambda: child.returncode,
                )
                def communicate(value, timeout):
                    restored.append(value)
                    child.returncode = 0
                    return "", ""
                child.communicate = communicate
                def click(name):
                    events.append(name)
                    if click_fails:
                        raise RuntimeError("click failed")
                journey.click = click
                journey.record = lambda *args, **details: events.append((args, details))
                with patch("subprocess.Popen", return_value=child), \
                     patch("select.select", return_value=([child.stdout], [], [])), \
                     patch("time.monotonic", side_effect=[0, 7]):
                    if observer_fails:
                        def observer():
                            raise AssertionError("particle observation failed")
                        with self.assertRaisesRegex(AssertionError, "particle observation failed"):
                            journey.copy_with_receipt("private fixture text", lambda: click("menu.Copy"), observer)
                        self.assertEqual(child.stdin.getvalue(), "check\n")
                    elif click_fails:
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
                if matches and not click_fails and not observer_fails:
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
                                          + (runner.json.dumps({"matches": True}) + "\n") * 2),
                        returncode=None,
                        poll=lambda: child.returncode,
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
                         "page_dark": False, "cursor": [0, 0], "problems": {"query": ""}}
                actions = []
                handoffs = []
                search_focused, search_selection = [False], [None]
                journey.snapshot = lambda: {"document": dict(state), "targets": {"problems.search": {
                    "enabled": True, "focused": search_focused[0], "age_ms": 0,
                    "selection": search_selection[0]}}}
                journey.newest_preview_document = lambda: None
                journey.choose_backend = lambda backend: actions.append(("backend", backend))

                def panel(tab):
                    search_focused[0] = False
                    remembered[0] = tab
                    state["panel"] = tab
                    state["problems_keyboard_focused"] = tab == "Problems"
                    state["bottom_panel_keyboard_focused"] = True
                    state["preview_keyboard_focused"] = False
                    state["editor_keyboard_focused"] = False

                def click(target):
                    actions.append(("click", target))
                    if target == "preview.page":
                        search_focused[0] = False
                        state["preview_keyboard_focused"] = True
                        state["problems_keyboard_focused"] = False
                        state["editor_keyboard_focused"] = False
                    elif target == "panel.close":
                        search_focused[0] = False
                        state["panel"] = None
                        state["problems_keyboard_focused"] = False
                    elif target == "panel.problems":
                        panel("Problems")
                    elif target == "panel.activity":
                        panel("Activity")
                    elif target == "problems.input":
                        self.assertEqual(state["panel"], "Problems")
                    elif target == "editor.source":
                        search_focused[0] = False
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
                    elif code == 3 and state["panel"] == "Problems":
                        search_focused[0] = True
                    elif code == 0 and search_focused[0]:
                        search_selection[0] = [0, len(state["problems"]["query"])]
                    elif code == 51 and search_focused[0]:
                        state["problems"]["query"] = ""

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
                def native(command, text):
                    if search_focused[0]:
                        self.assertEqual(command, "text")
                        state["problems"]["query"] = text
                    elif state["editor_keyboard_focused"]:
                        state["source_fingerprint"] = "edited"

                def wait_target(name, focused=False):
                    self.assertEqual(name, "problems.search")
                    self.assertTrue(focused and search_focused[0])

                journey.native = native
                journey.wait_target = wait_target
                journey.stable_state = lambda predicate, **kwargs: self.assertTrue(predicate(state))
                journey.paste_source = lambda text, check: check()
                journey.preview_terminal_handoff = terminal_handoff
                # This contract isolates backend focus preparation. The real
                # mixed-provider filtering phase has its own observation contracts.
                journey.problems_filters = lambda: actions.append(("filter-journey",))
                journey.problems_focus()
                self.assertEqual(handoffs, ["Problems", "Problems"])
                self.assertEqual(state["problems"]["query"], "")
                self.assertEqual(actions[-1], ("filter-journey",))


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


class ArxivTableFixtureContract(unittest.TestCase):
    def test_real_attributed_excerpts_and_unique_edits_use_only_lossless_grids(self):
        import hashlib
        import json
        from desktop_ui_newest_journeys import arxiv_table_cases
        directory = Path(__file__).resolve().parents[1] / "tests/fixtures/arxiv-tables"
        metadata = {entry["file"]: entry for entry in json.loads((directory / "provenance.json").read_text())}
        cases = arxiv_table_cases()
        self.assertEqual([(case["file"], case["cell"]) for case in cases], [
            ("scipy-roadmap.tex", (1, 1)), ("scipy-optimizers.tex", (1, 0)),
            ("platonic-metrics.tex", (3, 5))])
        for case in cases:
            with self.subTest(fixture=case["file"]):
                self.assertEqual(case["source"].encode("utf-8"), (directory / case["file"]).read_bytes())
                self.assertEqual(hashlib.sha256(case["table"].encode("utf-8")).hexdigest(),
                                 metadata[case["file"]]["table_sha256"])
                self.assertIn(r"\begin{tabular}", case["source"].splitlines()[case["line"] - 1])
                self.assertEqual(case["table"].count(case["cell_source"]), 1)
                self.assertEqual(case["cell_source"].count(case["editor_text"]), 1)
                self.assertEqual(case["editor_text"].replace(case["edit_before"], case["edit_after"], 1),
                                 case["edited_editor_text"])
                edited_raw_cell = case["cell_source"].replace(
                    case["editor_text"], case["edited_editor_text"], 1)
                self.assertEqual(edited_raw_cell, case["cell_source"].replace(
                    case["edit_before"], case["edit_after"], 1))
                self.assertEqual(case["source"].replace(case["cell_source"], edited_raw_cell, 1),
                                 case["source"].replace(case["edit_before"], case["edit_after"], 1))
                changed = case["source"].replace(case["edit_before"], case["edit_after"], 1)
                self.assertNotEqual(changed, case["source"])
                self.assertEqual(changed.replace(case["edit_after"], case["edit_before"], 1), case["source"])

    def test_platonic_edits_only_the_projected_body_and_preserves_exact_raw_wrappers(self):
        from desktop_ui_newest_journeys import arxiv_table_cases
        case = next(case for case in arxiv_table_cases() if case["file"] == "platonic-metrics.tex")
        body = (r"Unbiased estimator of CKA that corrects for sample bias in "
                r"HSIC~\cite{song2012feature}.")
        self.assertEqual(case["editor_text"], body)
        self.assertEqual(case["cell_source"], r"{\vspace{-0.5em}" + body + "}")
        self.assertEqual(case["edited_editor_text"], body.replace(
            "Unbiased estimator of CKA", "Bias-corrected estimator of CKA", 1))
        self.assertNotIn(r"\vspace", case["edited_editor_text"])
        self.assertTrue(case["edited_editor_text"].endswith(r"~\cite{song2012feature}."))

    def test_optimizer_last_column_requires_scroll_then_applies_and_undoes_exactly(self):
        from desktop_ui_newest_journeys import NewestJourneys, arxiv_table_cases
        from desktop_ui_journeys import fingerprint
        case = next(case for case in arxiv_table_cases() if case["file"] == "scipy-optimizers.tex")
        state = GridOnlyTableContract().state()
        state["document"]["table_draft"].update(rows=13, columns=14)
        state["document"]["source_fingerprint"] = fingerprint(case["source"])
        journey, actions, sources, records = NewestJourneys(), [], [], []
        journey.capture_review = False
        journey.snapshot = lambda: state
        journey.source_is = sources.append
        journey.open_table_workbench = lambda **kwargs: {"table.cell.1.13": 8}
        journey.scroll_table_grid_to_top = lambda *args, **kwargs: None
        def reveal(name, previous_frame, require_offscreen):
            self.assertEqual((name, previous_frame, require_offscreen), ("table.cell.1.13", 8, True))
            actions.append(("revealed", name))
        journey.reveal_table_cell = reveal
        journey.click = lambda name: actions.append(("click", name))
        journey.key = lambda *args: actions.append(("key", *args))
        journey.native = lambda *args: actions.append(args)
        journey.table_target = lambda name: actions.append(("ready", name))
        journey.stable_state = lambda predicate, frames: self.assertTrue(predicate(state["document"]))
        journey.wait = lambda description, predicate: self.assertTrue(predicate({"table_editor_open": False}))
        journey.record = lambda *args, **detail: records.append((args, detail))
        journey.arxiv_optimizer_last_column(case)
        original = case["source"]
        self.assertEqual(sources, [original, original,
                                  original.replace("0.19 & 1.0", "0.19 & 1.1", 1), original])
        self.assertEqual(actions, [("revealed", "table.cell.1.13"), ("click", "table.cell.1.13"),
                                  ("key", 0), ("text", "1.1"), ("ready", "table.apply"),
                                  ("click", "table.apply"), ("click", "editor.source"), ("key", 6)])
        self.assertEqual(records[0][1]["cell"], [1, 13])
        self.assertTrue(records[0][1]["exact_undo"])

    def test_registered_lane_runs_without_the_full_synthetic_table_suite(self):
        from desktop_ui_newest_journeys import NEWEST_JOURNEYS, NewestJourneys
        self.assertIn("arxiv_tables", NEWEST_JOURNEYS)
        self.assertTrue(callable(NewestJourneys.arxiv_tables))



class MathGridJourneyContract(unittest.TestCase):
    def state_and_case(self):
        from desktop_ui_math_grid_journeys import math_grid_fixtures, math_grid_fingerprint
        case = math_grid_fixtures()[1]["cases"][0]
        state = CompactTableGeometryContract().state()
        state["document"].update(
            table_draft={"rows": 2, "columns": 2, "valid": True, "math": True,
                         "kind": "mat", "fingerprint": math_grid_fingerprint(case["cells"])},
            table_samples={"pending": False, "failed": False, "locked": []})
        return state, case

    def test_fixture_lane_is_registered_and_uses_exact_edits_in_both_languages(self):
        from desktop_ui_math_grid_journeys import math_grid_fixtures, MathGridJourneys
        from desktop_ui_newest_journeys import NEWEST_JOURNEYS
        self.assertIn("math_grids", NEWEST_JOURNEYS)
        self.assertTrue(callable(runner.Journey.math_grids))
        self.assertTrue(callable(MathGridJourneys.math_grids))
        fixtures = math_grid_fixtures()
        self.assertEqual([fixture["filename"] for fixture in fixtures],
                         ["math-grid-editor.tex", "math-grid-editor.typ"])
        self.assertEqual([[case["kind"] for case in fixture["cases"]] for fixture in fixtures],
                         [["array", "pmatrix", "cases"], ["mat", "vec", "cases"]])
        for fixture in fixtures:
            for case in fixture["cases"]:
                with self.subTest(fixture=fixture["filename"], kind=case["kind"]):
                    self.assertNotEqual(case["applied"], fixture["source"])
                    self.assertTrue(fixture["source"].splitlines()[case["line"]].lstrip()
                                    .startswith(case["cells"][0][0]))

    def test_metadata_and_raw_formula_fingerprint_must_identify_the_math_grid(self):
        from desktop_ui_math_grid_journeys import assert_math_grid, math_grid_fingerprint
        state, case = self.state_and_case()
        assert_math_grid(state, case)
        wrong_cells = [row.copy() for row in case["cells"]]
        wrong_cells[0][0] = "$x$"
        for change in ({"math": False}, {"kind": "table"}, {"rows": 3},
                       {"fingerprint": math_grid_fingerprint(wrong_cells)}):
            state, case = self.state_and_case()
            state["document"]["table_draft"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, "metadata/formulas"):
                assert_math_grid(state, case)

    def test_formula_sampling_or_readonly_locks_cannot_pass_as_math_editing(self):
        from desktop_ui_math_grid_journeys import assert_math_grid
        for change in ({"pending": True}, {"failed": True}, {"locked": [[0, 0]]}):
            state, case = self.state_and_case()
            state["document"]["table_samples"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, "sampled or locked"):
                assert_math_grid(state, case)


class TableJourneyRecordingContract(unittest.TestCase):
    def test_record_details_cannot_collide_with_the_event_action_argument(self):
        import ast
        source = Path(__file__).with_name("desktop_ui_newest_journeys.py")
        tree = ast.parse(source.read_text(), filename=str(source))
        collisions = [call.lineno for call in ast.walk(tree)
                      if isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)
                      and isinstance(call.func.value, ast.Name) and call.func.value.id == "self"
                      and call.func.attr == "record"
                      and any(keyword.arg == "action" for keyword in call.keywords)]
        self.assertEqual(collisions, [], "record(action, ...) already owns the action parameter")


class TableTypingBudgetContract(unittest.TestCase):
    def test_long_cell_typing_keeps_the_full_fixture_inside_each_native_call_budget(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        posted = []
        journey.native = lambda command, text: posted.append((command, text))
        text = "Long table cell input preserves its complete content. " * 20
        journey.type_table_cell_text(text)
        self.assertGreater(len(posted), 1)
        self.assertTrue(all(command == "text" and 0 < len(chunk) <= 256
                            for command, chunk in posted))
        self.assertEqual("".join(chunk for _, chunk in posted), text)


class TableNativeResizeContract(unittest.TestCase):
    def windows(self):
        return [{"AXTitle": "journey.typ — tiptoptyp", "AXFocused": False, "size": [1400, 883]},
                {"AXTitle": "tiptoptyp Table editor", "AXFocused": True,
                 "AXMinimized": False, "size": [620, 448]}]

    def test_actual_focused_child_size_accepts_only_small_native_chrome_tolerance(self):
        from desktop_ui_newest_journeys import table_native_window, table_parent_sizes
        windows = self.windows()
        self.assertEqual(table_native_window(windows, [620, 420])["size"], [620, 448])
        self.assertEqual(table_parent_sizes(windows), {"journey.typ — tiptoptyp": [1400, 883]})
        for change in ({"AXFocused": False}, {"AXMinimized": True},
                       {"size": [860, 560]}, {"size": [620, 560]}, {"size": [620, 380]},
                       {"size": [700, 420]}, {"size": [620, float("nan")]}, {"size": [True, 420]}):
            windows = self.windows()
            windows[1].update(change)
            with self.subTest(change=change):
                self.assertIsNone(table_native_window(windows, [620, 420]))

    def test_a_focused_or_resized_parent_cannot_stand_in_for_the_table(self):
        from desktop_ui_newest_journeys import table_native_window, table_parent_sizes
        windows = self.windows()
        original_parents = table_parent_sizes(windows)
        windows[0].update(AXFocused=True, size=[620, 420])
        windows[1]["AXFocused"] = False
        self.assertIsNone(table_native_window(windows, [620, 420]))
        self.assertNotEqual(table_parent_sizes(windows), original_parents)
        self.assertIsNone(table_native_window([windows[0]], [620, 420]))
        self.assertIsNone(table_native_window(self.windows() + [self.windows()[1]], [620, 420]))


class CompactTableGeometryContract(unittest.TestCase):
    def state(self):
        button = {"enabled": True, "age_ms": 0, "frame": 10,
                  "viewport": "table-child", "width": 60, "height": 24, "y": 430}
        return {"document": {"table_editor_open": True, "interface_scale": 1},
                "scrolls": {"table.grid": {"x": 300, "y": 250, "frame": 10,
                    "offset": 0, "content_height": 1320, "viewport_height": 300}},
                "targets": {"table.apply": {**button, "x": 550},
                            "table.cancel": {**button, "x": 470},
                            "table.cell.1.0": {**button, "x": 100, "y": 160},
                            "table.cell.2.0": {**button, "x": 100, "y": 196}}}

    def test_reopened_scrolled_grid_is_wheeled_to_a_stable_top_before_cell_queries(self):
        from desktop_ui_newest_journeys import NewestJourneys
        journey = NewestJourneys()
        frames = []
        for offset in (320, 160, 0, 0, 0):
            state = self.state()
            state["scrolls"]["table.grid"]["offset"] = offset
            frames.append(state)
        states = iter(frames)
        journey.snapshot = lambda: next(states)
        native, records = [], []
        journey.native = lambda *args: native.append(args)
        journey.record = lambda *args, **detail: records.append((args, detail))
        with patch("desktop_ui_newest_journeys.time.sleep"):
            journey.scroll_table_grid_to_top("tex", "apply")
        self.assertEqual(native, [("move", 300, 250), ("wheel", 300, 250, 20)])
        self.assertEqual(list(states), [])
        self.assertEqual(records[0][1]["before"], 320)
        self.assertEqual(records[0][1]["after"], 0)
        self.assertEqual(records[0][1]["stable_samples"], 3)

    def test_small_real_tables_keep_geometry_checks_without_requiring_stress_overflow(self):
        from desktop_ui_newest_journeys import table_grid_geometry
        state = self.state()
        state["scrolls"]["table.grid"]["content_height"] = 280
        with self.assertRaises(AssertionError):
            table_grid_geometry(state)
        self.assertEqual(table_grid_geometry(state, require_overflow=False)[0]["offset"], 0)
        state["targets"]["table.cancel"]["frame"] = 9
        with self.assertRaises(AssertionError):
            table_grid_geometry(state, require_overflow=False)

    def test_scroll_offset_may_change_but_the_child_footer_must_stay_fixed(self):
        from desktop_ui_newest_journeys import table_grid_geometry, assert_compact_table_rows
        state = self.state()
        scroll, footer = table_grid_geometry(state)
        self.assertEqual(scroll["offset"], 0)
        self.assertEqual(assert_compact_table_rows(state), 36)
        state["scrolls"]["table.grid"]["offset"] = 144
        self.assertEqual(table_grid_geometry(state, footer)[0]["offset"], 144)
        state["targets"]["table.apply"]["y"] += 3
        with self.assertRaisesRegex(AssertionError, "moved"):
            table_grid_geometry(state, footer)

    def test_closed_stale_unscrollable_or_overlapping_geometry_is_not_evidence(self):
        from desktop_ui_newest_journeys import table_grid_geometry
        mutations = [lambda s: s["document"].update(table_editor_open=False),
                     lambda s: s["targets"]["table.cancel"].update(age_ms=800),
                     lambda s: s["targets"]["table.cancel"].update(frame=9),
                     lambda s: s["targets"]["table.cancel"].update(viewport="other-child"),
                     lambda s: s["targets"]["table.apply"].update(y=380),
                     lambda s: s["scrolls"]["table.grid"].update(content_height=320),
                     lambda s: s["scrolls"]["table.grid"].update(offset=float("nan"))]
        for mutate in mutations:
            state = self.state()
            mutate(state)
            with self.subTest(state=state), self.assertRaises(AssertionError):
                table_grid_geometry(state)

    def test_old_tall_row_pitch_and_mixed_frame_rows_cannot_pass_compactness(self):
        from desktop_ui_newest_journeys import assert_compact_table_rows
        for change in ({"y": 234}, {"frame": 9}, {"viewport": "other-child"}, {"age_ms": 800}):
            state = self.state()
            state["targets"]["table.cell.2.0"].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                assert_compact_table_rows(state)


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
            return {"document": {"table_editor_open": opened}, "targets": {"table.cell.0.0": {
                "enabled": enabled, "age_ms": 0, "frame": frame}}}
        states = iter([sample(False, 10, True), sample(True, 10, True),
                       sample(True, 11, False), sample(True, 12, True)])
        journey.snapshot = lambda: next(states)
        with patch("desktop_ui_newest_journeys.time.sleep"):
            target = journey.table_target("table.cell.0.0", previous_frame=10)
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


class GridOnlyTableContract(unittest.TestCase):
    def state(self):
        return {"document": {"table_editor_open": True,
                    "table_draft": {"rows": 10, "columns": 6, "valid": True}},
                "targets": {"table.apply": {"enabled": True, "age_ms": 1,
                    "frame": 12, "viewport": "table-child"}},
                "scrolls": {"table.grid": {"frame": 12}}}

    def test_grid_requires_current_footer_and_rejects_source_controls_in_the_same_child(self):
        from copy import deepcopy
        from desktop_ui_newest_journeys import assert_table_grid_only
        state = self.state()
        assert_table_grid_only(state)
        for target in ("table.source", "table.source-mode"):
            changed = deepcopy(state)
            changed["targets"][target] = dict(changed["targets"]["table.apply"])
            with self.subTest(target=target), self.assertRaisesRegex(AssertionError, "source editor"):
                assert_table_grid_only(changed)
            changed["targets"][target]["frame"] = 11
            assert_table_grid_only(changed)
        for apply_change in ({"age_ms": 501}, {"enabled": False}):
            changed = deepcopy(state)
            changed["targets"]["table.apply"].update(apply_change)
            with self.subTest(change=apply_change), self.assertRaises(AssertionError):
                assert_table_grid_only(changed)
        state["scrolls"]["table.grid"]["frame"] = 11
        with self.assertRaisesRegex(AssertionError, "no grid"):
            assert_table_grid_only(state)

    def test_grid_metadata_must_match_validation_and_remain_bounded(self):
        from copy import deepcopy
        from desktop_ui_newest_journeys import assert_table_grid_only
        for change in ({"valid": False}, {"rows": 0}, {"rows": True},
                       {"columns": None}, {"rows": 4097}, {"rows": 100, "columns": 100}):
            state = deepcopy(self.state())
            state["document"]["table_draft"].update(change)
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, "metadata"):
                assert_table_grid_only(state)

    def test_invalid_cells_stay_in_the_grid_with_disabled_apply(self):
        from desktop_ui_newest_journeys import assert_table_grid_only
        state = self.state()
        state["targets"]["table.apply"]["enabled"] = False
        state["document"]["table_draft"]["valid"] = False
        assert_table_grid_only(state, valid=False)

    def test_cell_reveal_ignores_stale_offscreen_slivers_and_requires_matching_owner(self):
        from copy import deepcopy
        from desktop_ui_newest_journeys import visible_table_cell
        state = self.state()
        cell = {**state["targets"]["table.apply"], "x": 300, "y": 180,
                "width": 150, "height": 26}
        state["targets"]["table.cell.3.5"] = cell
        self.assertEqual(visible_table_cell(state, "table.cell.3.5"), cell)
        self.assertIsNone(visible_table_cell(state, "table.cell.3.5", 12))
        for change in ({"frame": 11}, {"viewport": "root"}, {"enabled": False},
                       {"age_ms": 501}, {"width": 3}, {"height": 0}, {"x": float("nan")}):
            altered = deepcopy(state)
            altered["targets"]["table.cell.3.5"].update(change)
            with self.subTest(change=change):
                self.assertIsNone(visible_table_cell(altered, "table.cell.3.5"))

    def test_native_horizontal_input_reveals_a_virtualized_column_before_editing(self):
        from desktop_ui_newest_journeys import NewestJourneys
        state = CompactTableGeometryContract().state()
        name = "table.cell.3.5"
        journey = NewestJourneys()
        def snapshot():
            frame = state["scrolls"]["table.grid"]["frame"] + 1
            state["scrolls"]["table.grid"]["frame"] = frame
            for target in state["targets"].values():
                target["frame"] = frame
            return state
        journey.snapshot = snapshot
        actions, records = [], []
        def native(*args):
            actions.append(args)
            if args[0] == "horizontal-wheel" and args[-1] < 0:
                state["targets"][name] = {**state["targets"]["table.apply"],
                    "x": 450, "y": 220, "width": 150, "height": 26}
        journey.native = native
        journey.record = lambda *args, **detail: records.append((args, detail))
        clock = iter(index / 10 for index in range(100))
        with patch("desktop_ui_newest_journeys.time.sleep"), \
                patch("desktop_ui_newest_journeys.time.monotonic", side_effect=lambda: next(clock)):
            target = journey.reveal_table_cell(name, require_offscreen=True)
        self.assertEqual(target["x"], 450)
        self.assertEqual(actions, [("move", 300, 250), ("horizontal-wheel", 300, 250, 100),
                                  ("move", 300, 250), ("horizontal-wheel", 300, 250, -8)])
        self.assertEqual(records[0][1]["stable_samples"], 3)
        self.assertEqual(records[0][1]["horizontal_steps"], 1)
        self.assertTrue(records[0][1]["was_offscreen"])

    def test_horizontal_coverage_cannot_pass_with_an_already_visible_cell(self):
        from desktop_ui_newest_journeys import NewestJourneys
        state = CompactTableGeometryContract().state()
        name = "table.cell.1.13"
        state["targets"][name] = {**state["targets"]["table.apply"],
            "x": 450, "y": 220, "width": 150, "height": 26}
        journey = NewestJourneys()
        journey.snapshot = lambda: state
        journey.native = lambda *args: self.fail("visible cell cannot stand in for scroll coverage")
        with self.assertRaisesRegex(AssertionError, "must begin offscreen"):
            journey.reveal_table_cell(name, require_offscreen=True)

    def test_cell_position_stability_cannot_hide_cumulative_scroll_drift(self):
        from desktop_ui_newest_journeys import NewestJourneys
        states = []
        name = "table.cell.3.5"
        for frame, x in enumerate((400, 400, 400.75, 401.5, 401.5, 401.5), start=10):
            state = CompactTableGeometryContract().state()
            state["scrolls"]["table.grid"]["frame"] = frame
            for target in state["targets"].values():
                target["frame"] = frame
            state["targets"][name] = {**state["targets"]["table.apply"],
                "x": x, "y": 220, "width": 150, "height": 26}
            states.append(state)
        samples = iter(states)
        journey = NewestJourneys()
        journey.snapshot = lambda: next(samples)
        journey.native = lambda *args: self.fail("visible cells do not need scroll input")
        journey.record = lambda *args, **details: None
        with patch("desktop_ui_newest_journeys.time.sleep"):
            self.assertEqual(journey.reveal_table_cell(name)["x"], 401.5)
        self.assertEqual(list(samples), [])

    def test_horizontal_reveal_requires_advancing_cell_frames(self):
        from desktop_ui_newest_journeys import NewestJourneys
        name, states = "table.cell.3.5", []
        for frame in (10, 10, 10, 10, 11, 12):
            state = CompactTableGeometryContract().state()
            state["scrolls"]["table.grid"]["frame"] = frame
            for target in state["targets"].values():
                target["frame"] = frame
            state["targets"][name] = {**state["targets"]["table.apply"],
                "x": 400, "y": 220, "width": 150, "height": 26}
            states.append(state)
        samples = iter(states)
        journey = NewestJourneys()
        journey.snapshot = lambda: next(samples)
        journey.native = lambda *args: self.fail("visible cells do not need scroll input")
        journey.record = lambda *args, **details: None
        with patch("desktop_ui_newest_journeys.time.sleep"):
            journey.reveal_table_cell(name)
        self.assertEqual(list(samples), [], "one retained cell paint is not three current observations")

    def test_unreachable_table_column_exhausts_a_bounded_native_scroll_budget(self):
        from desktop_ui_newest_journeys import NewestJourneys
        state = CompactTableGeometryContract().state()
        journey = NewestJourneys()
        journey.snapshot = lambda: state
        actions = []
        journey.native = lambda *args: actions.append(args)
        clock = iter(index / 10 for index in range(300))
        with patch("desktop_ui_newest_journeys.time.sleep"), \
                patch("desktop_ui_newest_journeys.time.monotonic", side_effect=lambda: next(clock)):
            with self.assertRaisesRegex(AssertionError, "stably visible"):
                journey.reveal_table_cell("table.cell.3.5")
        rightward = [args for args in actions if args[0] == "horizontal-wheel" and args[-1] < 0]
        self.assertEqual(len(rightward), 8)

    def test_negative_detection_requires_three_advancing_menu_frames(self):
        from desktop_ui_newest_journeys import NewestJourneys
        states = iter({"document": {"table_editor_open": False}, "targets": {
            "menu.Select All": {"enabled": True, "frame": frame,
                                "age_ms": 1, "viewport": "popup"}}}
                      for frame in (5, 12, 12, 12, 13, 14))
        journey, actions, records = NewestJourneys(), [], []
        journey.snapshot = lambda: next(states)
        journey.click = lambda name: actions.append(("click", name))
        journey.key = lambda *args: actions.append(("key", *args))
        journey.wait_target = lambda *args: None
        from desktop_ui_journeys import fingerprint
        source = "#table(columns: widths, ..rows)"
        journey.wait = lambda description, predicate: self.assertTrue(predicate({
            "cursor": [2, 2], "source_fingerprint": fingerprint(source)}))
        journey.right_click = lambda name: actions.append(("right-click", name))
        journey.record = lambda *args, **detail: records.append((args, detail))
        with patch("desktop_ui_newest_journeys.time.sleep"):
            journey.reject_table_workbench("unsupported", source)
        self.assertEqual(list(states), [], "repeated observations of one menu paint cannot prove stability")
        self.assertEqual(records[0][1]["stable_samples"], 3)
        self.assertEqual(actions[-1], ("key", 53, "none"))

    def test_negative_caret_guard_uses_character_offsets_and_rejects_intercepted_navigation(self):
        from desktop_ui_newest_journeys import NewestJourneys
        from desktop_ui_journeys import fingerprint
        source = "// Ω 🧮\n#table(columns: widths, ..rows)"
        prefix = source.split("#table", 1)[0]
        expected = len(prefix) + 2
        self.assertGreater(len(prefix.encode("utf-8")) + 2, expected)
        for cursor in ([expected, expected], [0, 0],
                       [len(prefix.encode("utf-8")) + 2] * 2):
            journey, right_clicks = NewestJourneys(), []
            journey.click = lambda name: None
            journey.key = lambda *args: None
            journey.wait_target = lambda *args: None
            journey.right_click = lambda name: right_clicks.append(name)
            def wait(description, predicate):
                self.assertEqual(description, "caret reaches the unsupported table fixture")
                if not predicate({"cursor": cursor, "source_fingerprint": fingerprint(source)}):
                    raise AssertionError("caret location did not reach unsupported table")
            journey.wait = wait
            if cursor == [expected, expected]:
                # Stop immediately after the guard: the next observation opens
                # the separately tested native popup protocol.
                journey.snapshot = lambda: {"targets": {}}
                def stop_after_guard(name):
                    raise RuntimeError("guard passed")
                journey.right_click = stop_after_guard
                with self.assertRaisesRegex(RuntimeError, "guard passed"):
                    journey.reject_table_workbench("unicode", source, line=1)
            else:
                journey.snapshot = lambda: self.fail("wrong caret must stop before menu observation")
                with self.assertRaisesRegex(AssertionError, "caret location"):
                    journey.reject_table_workbench("unicode", source, line=1)
                self.assertEqual(right_clicks, [])

    def test_unsupported_detection_requires_a_current_context_menu(self):
        from copy import deepcopy
        from desktop_ui_newest_journeys import table_offer_rejected
        state = {"document": {"table_editor_open": False}, "targets": {
            "menu.Select All": {"enabled": True, "frame": 12, "age_ms": 1, "viewport": "menu"}}}
        self.assertTrue(table_offer_rejected(state, 11))
        self.assertFalse(table_offer_rejected(state, 12))
        for change in ({"age_ms": 501}, {"enabled": False}):
            stale = deepcopy(state)
            stale["targets"]["menu.Select All"].update(change)
            self.assertFalse(table_offer_rejected(stale))
        for enabled in (False, True):
            offered = deepcopy(state)
            offered["targets"]["menu.Edit Table…"] = {
                **offered["targets"]["menu.Select All"], "enabled": enabled}
            with self.subTest(enabled=enabled), self.assertRaisesRegex(AssertionError, "offered"):
                table_offer_rejected(offered, 11)
            offered["targets"]["menu.Edit Table…"]["frame"] = 10
            self.assertTrue(table_offer_rejected(offered, 11))
        state["document"]["table_editor_open"] = True
        with self.assertRaisesRegex(AssertionError, "unexpectedly opened"):
            table_offer_rejected(state, 11)


class SnippetManagerContract(unittest.TestCase):
    def document(self):
        from desktop_ui_journeys import fingerprint
        entry = {"language": "typst", "prefix": "eq", "body": fingerprint("$ {{1:x}} $"),
                 "enabled": True, "placeholder_open": "{{", "placeholder_close": "}}"}
        return {"snippet_presets": [entry], "snippets": []}

    def test_saved_match_requires_exact_body_options_origin_and_language(self):
        from desktop_ui_snippet_journeys import snippet_matches
        document = self.document()
        self.assertTrue(snippet_matches(document, "preset", "typst", "eq", "$ {{1:x}} $", True, "{{", "}}"))
        for changed in ({"body": "old"}, {"enabled": False}, {"enabled": 1},
                        {"placeholder_open": "<<"}, {"placeholder_close": ">>"}, {"language": "tex"}, {"prefix": "other"}):
            with self.subTest(changed=changed):
                wrong = {**document, "snippet_presets": [{**document["snippet_presets"][0], **changed}]}
                self.assertFalse(snippet_matches(wrong, "preset", "typst", "eq", "$ {{1:x}} $", True, "{{", "}}"))
        self.assertFalse(snippet_matches(document, "custom", "typst", "eq", "$ {{1:x}} $", True, "{{", "}}"))

    def test_duplicate_entries_cannot_pass_saved_or_removed_assertions(self):
        from desktop_ui_snippet_journeys import snippet_absent, snippet_matches
        document = self.document()
        document["snippet_presets"] *= 2
        self.assertFalse(snippet_matches(document, "preset", "typst", "eq", "$ {{1:x}} $", True, "{{", "}}"))
        self.assertFalse(snippet_absent(document, "preset", "typst", "eq"))
        self.assertTrue(snippet_absent(document, "custom", "typst", "eq"))

    def test_completion_requires_current_local_provider_at_the_same_index(self):
        from desktop_ui_snippet_journeys import snippet_completion
        document = {"completion_visible": True, "completion_current": True,
                    "completion_labels": ["eq", "eq"],
                    "completion_origins": ["Language server", "Snippet"]}
        self.assertEqual(snippet_completion(document, "eq"), 1)
        for changed in ({"completion_visible": False}, {"completion_current": False},
                        {"completion_origins": ["Language server", "Language server"]},
                        {"completion_labels": ["eq", "other"]}):
            with self.subTest(changed=changed):
                self.assertIsNone(snippet_completion({**document, **changed}, "eq"))

    def test_filters_open_the_real_control_before_selecting_an_option(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        journey = SnippetJourneys()
        actions = []
        journey.click = lambda target: actions.append(("click", target))
        journey.wait_target = lambda target: actions.append(("wait", target))
        journey.filter_snippets("status", "disabled")
        self.assertEqual(actions, [
            ("click", "settings.snippets.filter.status"),
            ("wait", "settings.snippets.filter.status.disabled"),
            ("click", "settings.snippets.filter.status.disabled"),
        ])

    def test_draft_ack_requires_the_exact_field_value_in_a_visible_editor(self):
        from desktop_ui_snippet_journeys import snippet_field_matches
        from desktop_ui_journeys import fingerprint
        document = {"snippet_editor_visible": True, "snippet_editor_fields": {
            "source": fingerprint("$ {{1:x}} $"), "prefix": fingerprint("eq")}}
        self.assertTrue(snippet_field_matches(document, "source", "$ {{1:x}} $"))
        self.assertFalse(snippet_field_matches(document, "source", "old source"))
        self.assertFalse(snippet_field_matches(document, "opening", "{{"))
        self.assertFalse(snippet_field_matches({**document, "snippet_editor_visible": False},
                                               "source", "$ {{1:x}} $"))

    def test_paste_adapter_stays_open_until_the_exact_deferred_draft_arrives(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        from desktop_ui_journeys import fingerprint
        journey = SnippetJourneys()
        actions = []
        journey.click = lambda target: actions.append(("click", target))
        journey.key = lambda *args: actions.append(("key", *args))
        old = {"snippet_editor_visible": True, "snippet_editor_fields": {"source": fingerprint("old")}}
        new = {**old, "snippet_editor_fields": {"source": fingerprint("new")}}
        def wait(description, predicate):
            self.assertFalse(predicate(old), "already-enabled Save cannot acknowledge stale text")
            self.assertTrue(predicate(new))
            actions.append(("draft-consumed",))
        def paste(text, check):
            actions.append(("clipboard-open", text))
            check()
            actions.append(("clipboard-restored",))
        journey.wait = wait
        journey.paste_source = paste
        journey.wait_target = lambda target: actions.append(("ready", target))
        journey.edit_snippet_text("source", "new")
        self.assertEqual(actions, [
            ("click", "settings.snippets.source"), ("key", 0),
            ("clipboard-open", "new"), ("draft-consumed",), ("clipboard-restored",),
            ("ready", "settings.snippets.save"),
        ])

    def test_empty_query_uses_delete_because_empty_paste_does_not_clear_a_field(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        from desktop_ui_journeys import fingerprint
        journey = SnippetJourneys()
        actions = []
        journey.click = lambda target: actions.append(("click", target))
        journey.key = lambda *args: actions.append(("key", *args))
        def wait(description, predicate):
            self.assertTrue(predicate({"snippet_editor_visible": True,
                                      "snippet_editor_fields": {"search": fingerprint("")}}))
        journey.wait = wait
        journey.paste_source = lambda *_: self.fail("empty paste cannot delete selected text")
        journey.edit_snippet_text("search", "", dirty=False)
        self.assertEqual(actions, [("click", "settings.snippets.search"),
                                   ("key", 0), ("key", 51, "none")])

    def test_delimiter_change_requires_explicit_rewrite_or_literal_source_choice(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        for keep, action in ((False, "rewrite"), (True, "keep-source")):
            with self.subTest(keep=keep):
                journey = SnippetJourneys()
                actions = []
                journey.edit_snippet_text = lambda *args, **kwargs: actions.append(("edit", args, kwargs))
                journey.wait_target = lambda target: actions.append(("wait", target))
                journey.click = lambda target: actions.append(("click", target))
                journey.change_snippet_delimiters("<<", ">>", keep_source=keep)
                self.assertEqual(actions, [
                    ("edit", ("opening", "<<"), {"dirty": False}),
                    ("edit", ("closing", ">>"), {"dirty": False}),
                    ("wait", "settings.snippets.delimiters." + action),
                    ("click", "settings.snippets.delimiters." + action),
                    ("wait", "settings.snippets.save"),
                ])

    def test_snippet_window_cannot_change_or_switch_its_document_owner(self):
        from desktop_ui_snippet_journeys import snippet_owner_unchanged
        before = {"tabs": 2, "active_tab_id": 7, "path": "/fixture.typ",
                  "source_bytes": 4, "source_fingerprint": "same"}
        self.assertTrue(snippet_owner_unchanged({**before, "snippet_editor_visible": True}, before))
        for field, value in (("tabs", 1), ("active_tab_id", 8), ("path", None),
                             ("source_bytes", 5), ("source_fingerprint", "edited")):
            with self.subTest(field=field):
                self.assertFalse(snippet_owner_unchanged({**before, field: value}, before))

    def test_snippet_cmd_w_must_leave_settings_and_the_document_intact(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        journey = SnippetJourneys()
        before = {"tabs": 2, "active_tab_id": 7, "path": "/fixture.typ",
                  "source_bytes": 4, "source_fingerprint": "same",
                  "settings_visible": True, "snippet_editor_visible": True}
        after = {**before, "snippet_editor_visible": False}
        actions = []
        journey.snapshot = lambda: {"document": before}
        journey.key = lambda *args: actions.append(("key", *args))
        def wait(description, predicate):
            self.assertTrue(predicate(after))
            for change in ({"settings_visible": False}, {"snippet_editor_visible": True},
                           {"tabs": 1}, {"source_fingerprint": "edited"}):
                self.assertFalse(predicate({**after, **change}))
        def native_wait(description, predicate):
            settings = {"AXTitle": "tiptoptyp Settings", "AXFocused": True}
            hidden = {"AXTitle": "tiptoptyp Snippets", "AXFocused": False}
            self.assertTrue(predicate([settings, hidden]))
            self.assertFalse(predicate([{**settings, "AXFocused": False}, hidden]))
            self.assertFalse(predicate([settings, {**hidden, "AXFocused": True}]))
        journey.wait = wait
        journey.native_wait = native_wait
        journey.close_settings = lambda: actions.append(("close-settings",))
        journey.close_snippet_manager()
        self.assertEqual(actions, [("key", 13), ("close-settings",)])

    def test_revert_requires_fresh_disabled_save_in_an_open_snippet_window(self):
        from desktop_ui_snippet_journeys import SnippetJourneys
        journey = SnippetJourneys()
        def state(visible=True, enabled=False, age=0):
            return {"document": {"snippet_editor_visible": visible}, "targets": {
                "settings.snippets.save": {"enabled": enabled, "age_ms": age}}}
        observations = iter([state(enabled=True), state(age=501), state(visible=False), state()])
        journey.snapshot = lambda: next(observations)
        with patch("time.sleep"):
            journey.wait_snippet_save_disabled()
        self.assertEqual(list(observations), [])


class NativeImeContract(unittest.TestCase):
    def state(self, scale=1.0):
        return {"document": {"frame": 10, "editor_keyboard_focused": True,
                "interface_scale": scale, "native_ime_area": {
                    "first_responder": True,
                    "source": "NSTextInputClient.firstRectForCharacterRange",
                    "rect": [100.0, 200.0, 3.0 * scale, 25.0 * scale]}},
                "targets": {"editor.caret": {"frame": 10, "age_ms": 0,
                    "x": 100.0 + 1.5 * scale, "y": 200.0 + 12.5 * scale},
                    "editor.source": {"frame": 10, "age_ms": 0,
                        "x": 300.0, "y": 400.0, "width": 500.0, "height": 700.0}}}

    def test_native_readback_requires_the_caret_instead_of_the_editor_rectangle(self):
        from desktop_ui_ime_journeys import assert_native_ime_at_caret
        state = self.state()
        self.assertEqual(assert_native_ime_at_caret(state), [100.0, 200.0, 3.0, 25.0])
        for rect in ([30, -2000, 800, 6000], [100, 200, 0, 5000],
                     [100, 200, 0, -25], [100, 200, 0, float("nan")],
                     [400, 200, 0, 25], [100, 600, 0, 25]):
            with self.subTest(rect=rect):
                state["document"]["native_ime_area"]["rect"] = rect
                with self.assertRaises(AssertionError):
                    assert_native_ime_at_caret(state)

    def test_native_anchor_rejects_stale_echoes_and_the_wrong_input_owner(self):
        from desktop_ui_ime_journeys import assert_native_ime_at_caret
        for field, value in (("first_responder", False), ("source", "egui cached output")):
            state = self.state()
            state["document"]["native_ime_area"][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_native_ime_at_caret(state)
        for field, value in (("frame", 9), ("age_ms", 501)):
            state = self.state()
            state["targets"]["editor.caret"][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_native_ime_at_caret(state)
        state = self.state()
        state["document"]["editor_keyboard_focused"] = False
        with self.assertRaises(AssertionError):
            assert_native_ime_at_caret(state)

    def test_composition_accepts_a_collapsed_preedit_but_requires_fresh_ime_caret(self):
        from desktop_ui_ime_journeys import assert_native_ime_at_caret
        state = self.state()
        state["document"]["cursor"] = [45, 45]
        state["targets"]["editor.ime_cursor"] = state["targets"].pop("editor.caret")
        self.assertEqual(assert_native_ime_at_caret(state, composing=True), [100.0, 200.0, 3.0, 25.0])
        state["targets"]["editor.caret"] = state["targets"].pop("editor.ime_cursor")
        with self.assertRaisesRegex(AssertionError, "fresh source caret"):
            assert_native_ime_at_caret(state, composing=True)

    def test_phonetic_phase_cannot_pass_the_required_chinese_conversion(self):
        from desktop_ui_ime_journeys import pinyin_source_matches
        from desktop_ui_journeys import fingerprint
        before, after = "L039.\n", "L040.\n"
        for text, preedit, committed in (("nihao", True, False), ("ni hao", True, False),
                                         ("ni'hao", True, False), ("nihao ", False, False),
                                         ("你好", False, True), ("你好 ", False, False)):
            expected = before + text + after
            document = {"source_fingerprint": fingerprint(expected),
                        "source_bytes": len(expected.encode("utf-8")), "cursor": [11, 11]}
            with self.subTest(text=text):
                self.assertEqual(pinyin_source_matches(document, before, after, False), preedit)
                self.assertEqual(pinyin_source_matches(document, before, after, True), committed)
        document["source_bytes"] += 1
        self.assertFalse(pinyin_source_matches(document, before, after, True))

    def test_matching_native_hint_and_caret_must_still_be_visible(self):
        from desktop_ui_ime_journeys import assert_native_ime_at_caret
        state = self.state()
        state["document"]["native_ime_area"]["rect"][1] = -100
        state["targets"]["editor.caret"]["y"] = -87.5
        with self.assertRaisesRegex(AssertionError, "outside the visible"):
            assert_native_ime_at_caret(state)
        for field, value in (("frame", 9), ("age_ms", 501)):
            state = self.state()
            state["targets"]["editor.source"][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "fresh visible"):
                assert_native_ime_at_caret(state)
        state = self.state()
        del state["targets"]["editor.source"]
        with self.assertRaisesRegex(AssertionError, "fresh visible"):
            assert_native_ime_at_caret(state)

    def test_scaled_caret_uses_its_center_and_scaled_native_width(self):
        from desktop_ui_ime_journeys import assert_native_ime_at_caret
        state = self.state(scale=1.5)
        state["targets"]["editor.caret"]["y"] = 218.75
        self.assertEqual(assert_native_ime_at_caret(state), [100.0, 200.0, 4.5, 37.5])

    def test_source_cleanup_requires_an_exact_successful_restoration_receipt(self):
        from desktop_ui_ime_journeys import finish_input_source_scope
        from types import SimpleNamespace
        import json
        valid = {"restored": True, "original": "British", "current": "British"}
        calls = []
        child = SimpleNamespace(returncode=0, communicate=lambda value, timeout:
                                (calls.append((value, timeout)) or json.dumps(valid), ""))
        self.assertEqual(finish_input_source_scope(child), valid)
        self.assertEqual(calls, [("restore\n", 6)])
        for receipt in ({"restored": True}, {**valid, "restored": 1},
                        {**valid, "restored": False}, {**valid, "current": "External choice"}, []):
            child.communicate = lambda *_args, **_kwargs: (json.dumps(receipt), "")
            with self.subTest(receipt=receipt), self.assertRaises(RuntimeError):
                finish_input_source_scope(child)

    def test_source_cleanup_timeout_is_unavailable_and_reaps_the_adapter(self):
        from desktop_ui_ime_journeys import finish_input_source_scope
        from types import SimpleNamespace
        import subprocess
        calls = []
        def communicate(value=None, timeout=None):
            calls.append(("communicate", value, timeout))
            if value is not None:
                raise subprocess.TimeoutExpired("fixture-driver", timeout)
            return "", ""
        child = SimpleNamespace(communicate=communicate,
                                terminate=lambda: calls.append(("terminate",)))
        with self.assertRaisesRegex(RuntimeError, "restoration could not be verified"):
            finish_input_source_scope(child)
        self.assertEqual(calls, [("communicate", "restore\n", 6), ("terminate",),
                                 ("communicate", None, 2)])

    def test_composition_failure_still_restores_the_original_source(self):
        from desktop_ui_ime_journeys import ImeJourneys, PINYIN
        from types import SimpleNamespace
        from io import StringIO
        import json
        journey = ImeJourneys()
        journey.driver, journey.process = Path("/private/tmp/fixture-driver"), SimpleNamespace(pid=123)
        journey.focus_document_window = lambda: None
        journey.record = lambda *_args, **_kwargs: None
        journey.native = lambda command: {"selected": "British", "sources": [
            {"id": PINYIN, "enabled": True, "selectable": True}]}
        restored = []
        child = SimpleNamespace(returncode=0,
            stdout=StringIO(json.dumps({"selected": PINYIN, "original": "British"}) + "\n"),
            communicate=lambda value, timeout: (restored.append(value) or json.dumps({
                "restored": True, "original": "British", "current": "British"}), ""))
        with patch("desktop_ui_ime_journeys.subprocess.Popen", return_value=child), \
                patch("desktop_ui_ime_journeys.select.select", return_value=([child.stdout], [], [])):
            with self.assertRaisesRegex(ValueError, "composition failed"):
                with journey.pinyin_input():
                    raise ValueError("composition failed")
        self.assertEqual(restored, ["restore\n"])

    def test_unavailable_pinyin_never_attempts_to_enable_or_select_a_source(self):
        from desktop_ui_ime_journeys import ImeJourneys
        journey = ImeJourneys()
        journey.focus_document_window = lambda: None
        journey.native = lambda command: {"selected": "British", "sources": []}
        with patch("desktop_ui_ime_journeys.subprocess.Popen") as process:
            with self.assertRaisesRegex(RuntimeError, "Pinyin.*not already enabled"):
                with journey.pinyin_input():
                    self.fail("unavailable input mode was treated as a native pass")
        process.assert_not_called()

    def test_native_ime_journey_is_registered_on_the_actual_runner(self):
        self.assertTrue(callable(runner.Journey.ime))


class PreviewDividerInputScopeContract(unittest.TestCase):
    def test_divider_journey_covers_both_modes_without_reserved_zoom_input(self):
        from desktop_ui_partial_journeys import PartialPreviewJourneys
        class Fake(PartialPreviewJourneys):
            def __init__(self): self.actions=[]
            def partial_preview_default(self): self.actions.append("default")
            def choose_backend(self, backend, save_pdf): self.actions.append((backend,save_pdf))
            def wait(self, *args, **kwargs): pass
            def cached_divider_resize(self, mode): self.actions.append(mode)
            def native(self, *args): raise AssertionError("reserved input was not requested")
        fake=Fake(); fake.partial_dividers()
        self.assertEqual(fake.actions,["default","partial",("Interactive",False),"full"])

    def test_system_scroll_zoom_is_unavailable_without_posting_input(self):
        from desktop_ui_partial_journeys import PartialPreviewJourneys
        class Fake(PartialPreviewJourneys):
            def __init__(self): self.calls=[]
            def native(self, command):
                self.calls.append(command)
                return {"system_scroll_zoom":True}
            def record(self, *args, **kwargs): pass
            def click(self, *args): raise AssertionError("input posted despite system zoom")
        fake=Fake()
        with self.assertRaisesRegex(RuntimeError,"unavailable"):
            fake.cursor_wheel_zoom("full")
        self.assertEqual(fake.calls,["preflight"])


if __name__ == "__main__":
    unittest.main()
