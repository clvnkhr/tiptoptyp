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


class PreviewAnchorContract(unittest.TestCase):
    def test_original_source_point_detects_a_shared_center_anchor_error(self):
        from desktop_ui_partial_journeys import source_point,project_source_point
        before={"paper_scale":2,"paper_bounds":[0,-40,600,1200],"page_bounds":[[0,-40,600,1200]]}
        point=source_point(before,100,200)
        correct={"paper_scale":3,"paper_bounds":[-50,-160,900,1800],"page_bounds":[[-50,-160,900,1800]]}
        centered={"paper_scale":3,"paper_bounds":[-50,-165,900,1800],"page_bounds":[[-50,-165,900,1800]]}
        self.assertEqual(project_source_point(correct,point),[100,200])
        self.assertEqual(project_source_point(centered,point),[100,195])


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
        journey.native = lambda *args: actions.append(args)
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
            ("focus",), ("key", 31), ("key", 5, "cmd+shift"), ("key", 0),
            ("text", str(path)), ("key", 36, "none"), ("ax-press-title", "Open"),
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


if __name__ == "__main__":
    unittest.main()
