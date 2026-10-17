"""Failure-detector tests for reveal observations; no native interaction claimed."""
import copy
import sys
import unittest

sys.dont_write_bytecode = True
from desktop_ui_jump_journeys import assert_reveal, reveal_retired


class PreviewRevealContract(unittest.TestCase):
    def visible(self):
        return {"active": True, "pending": False, "page": 2, "center": [200, 150],
                "viewport": [20, 40, 420, 600], "elapsed_ms": 300}

    def test_accepts_actual_tinymist_and_source_pdfium_observations(self):
        self.assertIs(assert_reveal(value := self.visible(), page=2), value)
        value.update(target=[199, 141, 201, 159], char_index=25)
        self.assertIs(assert_reveal(value, region=(4, 20, 40)), value)
        value["target"] = [200, 141, 200, 159]  # Actual text caret may have zero width.
        assert_reveal(value, region=(4, 20, 40))

    def test_rejects_pending_missing_expired_or_wrong_destination(self):
        for value in (None, {}, {**self.visible(), "pending": True},
                      {**self.visible(), "active": False},
                      {**self.visible(), "elapsed_ms": 900}):
            with self.subTest(value=value), self.assertRaises(AssertionError):
                assert_reveal(value)
        with self.assertRaisesRegex(AssertionError, "another paper"):
            assert_reveal(self.visible(), page=0)
        for at in (None, True, 19, 40):
            with self.subTest(at=at), self.assertRaisesRegex(AssertionError, "source location"):
                assert_reveal({**self.visible(), "char_index": at}, region=(4, 20, 40))

    def test_rejects_offscreen_nonfinite_or_miscentered_feedback(self):
        mutations = [
            {"center": [0, 0]}, {"center": [float("nan"), 150]},
            {"viewport": [20, 40, 20, 600]}, {"elapsed_ms": True},
            {"target": [200, 100, 220, 120]}, {"target": [200, 150, 201, 149]},
            {"target": [False, 141, 201, 159]},
        ]
        for patch in mutations:
            with self.subTest(patch=patch), self.assertRaises(AssertionError):
                assert_reveal({**self.visible(), **copy.deepcopy(patch)})

    def test_retirement_rejects_pending_and_active_feedback(self):
        self.assertTrue(reveal_retired(None))
        self.assertTrue(reveal_retired({"active": False, "pending": False}))
        self.assertFalse(reveal_retired(self.visible()))
        self.assertFalse(reveal_retired({"active": False, "pending": True}))
        self.assertFalse(reveal_retired({}))


if __name__ == "__main__":
    unittest.main()
