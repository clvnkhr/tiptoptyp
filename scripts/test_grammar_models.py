"""Adapter boundaries that do not require model weights or optional ML packages."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).parent / "grammar_compare"))
from models import (
    InputTooLongError, MODEL_SPECS, normalize_word_apostrophes,
    require_token_limit, restore_word_apostrophes, refine_checked,
)


class AdapterBoundaryTests(unittest.TestCase):
    def test_token_boundary_accepts_exact_limit_and_rejects_longer(self):
        require_token_limit(128, 128, "Liquid")
        with self.assertRaisesRegex(InputTooLongError, "129.*128.*no text was truncated"):
            require_token_limit(129, 128, "Liquid")

    def test_only_word_apostrophes_are_normalized(self):
        self.assertEqual(normalize_word_apostrophes("Tao’s and dʼAlembert’s ‘proof’."),
                         "Tao's and d'Alembert's ‘proof’.")
        self.assertEqual(normalize_word_apostrophes("'MATH' — x-y"), "'MATH' — x-y")

    def test_apostrophe_style_is_restored_without_restoring_other_edits(self):
        self.assertEqual(restore_word_apostrophes("Tao's proof is valid.", "Tao’s proof are valid."),
                         "Tao’s proof is valid.")
        self.assertEqual(restore_word_apostrophes("Taos proof is valid.", "Tao’s proof are valid."),
                         "Taos proof is valid.")

    def test_apostrophe_restore_does_not_replace_a_substring_of_a_word(self):
        self.assertEqual(restore_word_apostrophes("NotTao's proof", "Tao’s proof"), "NotTao's proof")

    def test_refinement_rejects_growth_before_a_second_call_can_truncate_it(self):
        calls = []
        def once(value):
            calls.append(value)
            return "x" * 129
        with self.assertRaises(InputTooLongError):
            refine_checked("ok", len, once, limit=128, max_iter=3, name="Liquid")
        self.assertEqual(calls, ["ok"])

    def test_refinement_stops_unchanged_cycles_and_at_the_iteration_cap(self):
        calls = []
        def once(value):
            calls.append(value)
            return {"a": "b", "b": "a"}[value]
        self.assertEqual(refine_checked("a", len, once, limit=128, max_iter=3, name="Liquid"), "a")
        self.assertEqual(calls, ["a", "b"])
        self.assertEqual(refine_checked("a", len, lambda value: value + "b", limit=128,
                                        max_iter=3, name="Liquid"), "abbb")
        self.assertEqual(refine_checked("a", len, lambda value: value, limit=128,
                                        max_iter=3, name="Liquid"), "a")

    def test_model_weights_and_auxiliary_revisions_are_pinned(self):
        self.assertEqual(set(MODEL_SPECS), {"liquid", "gector", "coedit"})
        for spec in MODEL_SPECS.values():
            self.assertNotEqual(spec.revision, "main")
            self.assertGreater(spec.artifact_bytes, 0)
        self.assertRegex(MODEL_SPECS["gector"].revision, r"^sha256:[a-f0-9]{64}$")


if __name__ == "__main__":
    unittest.main()
