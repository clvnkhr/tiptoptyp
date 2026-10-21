"""Source-mapping safety tests, without optional model dependencies."""
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).parent / "grammar_compare"))
from harper_mapping import map_correction


def literal(text, start=0):
    return {"text": text, "source_map": list(range(start, start + len(text))),
            "protected_scalars": []}


def projected(source, text, offsets, protected=()):
    return {"text": text, "source_map": offsets, "protected_scalars": list(protected)}


class SourceMappingTests(unittest.TestCase):
    def safe(self, source, projection, corrected, expected):
        result = map_correction(source, projection, corrected)
        self.assertTrue(result["all_safe"], result)
        self.assertEqual(result["source_output"], expected)
        self.assertEqual(result["rejected"], [])
        return result

    def unsafe(self, source, projection, corrected):
        result = map_correction(source, projection, corrected)
        self.assertFalse(result["all_safe"], result)
        self.assertEqual(result["source_output"], source)
        self.assertTrue(result["rejected"])
        return result

    def test_literal_replacement_and_unchanged(self):
        self.safe("This are valid.", literal("This are valid."), "This is valid.", "This is valid.")
        self.safe("Already valid.", literal("Already valid."), "Already valid.", "Already valid.")

    def test_unicode_scalar_offsets_not_utf8_bytes(self):
        source = "😀 Évariste’s proof are valid."
        result = self.safe(source, literal(source), source.replace(" are ", " is "),
                           "😀 Évariste’s proof is valid.")
        self.assertEqual(result["edits"][0]["start"], source.index("are"))

    def test_repeat_deletion_preserves_unmodified_original_whitespace(self):
        source = "The  the\nproof holds."
        text = "The the proof holds."
        offsets = [0, 1, 2, None, 5, 6, 7, None, *range(9, len(source))]
        self.safe(source, projected(source, text, offsets), "The proof holds.", "The\nproof holds.")

    def test_changed_whitespace_maps_only_the_whitespace_gap(self):
        source = "First\n\nsecond."
        projection = projected(source, "First second.", [*range(5), None, *range(7, len(source))])
        self.safe(source, projection, "First, second.", "First,\n\nsecond.")
        self.safe(source, projection, "Firstsecond.", "Firstsecond.")

    def test_hidden_math_and_code_survive_safe_typo(self):
        source = "This are $x^2$ and #code(42)."
        text = "This are x and x."
        offsets = [0, 1, 2, 3, None, 5, 6, 7, None, None, None, 15, 16, 17, None, None, 28]
        self.safe(source, projected(source, text, offsets, [9, 15]),
                  "This is x and x.", "This is $x^2$ and #code(42).")

    def test_placeholder_deletion_and_reordering_are_rejected(self):
        source = "We use $x$ here."
        text = "We use x here."
        projection = projected(source, text, [0, 1, None, 3, 4, 5, None, None, None,
                                             *range(11, len(source))], [7])
        self.unsafe(source, projection, "We use here.")
        self.unsafe(source, projection, "x We use here.")

    def test_insertion_next_to_opaque_math_is_rejected(self):
        source = "We use $x$ here."
        text = "We use x here."
        projection = projected(source, text, [0, 1, None, 3, 4, 5, None, None, None,
                                             *range(11, len(source))], [7])
        self.unsafe(source, projection, "We use the x here.")
        self.unsafe(source, projection, "We use x exactly here.")

    def test_safe_typo_inside_macro_content(self):
        source = "A \\emph{estmate} holds."
        text = "A estmate holds."
        offsets = [0, None, *range(8, 15), None, *range(17, len(source))]
        self.safe(source, projected(source, text, offsets), "A estimate holds.",
                  "A \\emph{estimate} holds.")

    def test_replacement_across_macro_boundary_is_rejected(self):
        source = "bad\\emph{word}"
        text = "badword"
        self.unsafe(source, projected(source, text, [0, 1, 2, *range(9, 13)]), "good")

    def test_whitespace_only_change_cannot_delete_macro_syntax(self):
        source = "a\\emph{b}"
        projection = projected(source, "a b", [0, None, 7])
        self.unsafe(source, projection, "ab")
        self.unsafe(source, projection, "a  b")

    def test_fragment_boundary_insertion_cannot_enter_macro(self):
        source = "\\emph{word}"
        projection = literal("word", 6)
        self.unsafe(source, projection, "a word")
        self.unsafe(source, projection, "word indeed")

    def test_physical_source_edges_allow_insertions(self):
        self.safe("proof", literal("proof"), "A proof.", "A proof.")

    def test_partial_normalized_paragraph_run_is_ambiguous(self):
        source = "a\n\n\nb"
        projection = projected(source, "a\n\nb", [0, None, None, 4])
        self.unsafe(source, projection, "a\nb")
        self.unsafe(source, projection, "a\nX\nb")

    def test_one_unsafe_edit_rejects_complete_proposal(self):
        source = "This are valid. We use $x$."
        begin, end = source.index("$"), source.rindex("$") + 1
        text = source[:begin] + "x" + source[end:]
        projection = projected(source, text,
                               [*range(begin), None, *range(end, len(source))], [begin])
        result = self.unsafe(source, projection, "This is valid. We use y.")
        self.assertTrue(result["edits"], result)

    def test_invalid_mapping_is_rejected_even_when_output_unchanged(self):
        for offsets in ([1, 0], [0, 0], [0, 9], [-1, 1], [True, 1], [0, 1.0], [0]):
            with self.subTest(offsets=offsets):
                self.unsafe("aa", projected("aa", "aa", offsets), "aa")
        self.unsafe("ab", projected("ab", "aa", [0, 1]), "aa")
        self.unsafe("ab", projected("ab", "ax", [0, None]), "ax")

    def test_invalid_protection_indices(self):
        for protected in ([2], [-1], [True], [0, 0], [0]):
            with self.subTest(protected=protected):
                self.unsafe("ab", projected("ab", "ab", [0, 1], protected), "ab")

    def test_empty_projection_cannot_create_source_text(self):
        self.safe("$x$", projected("$x$", "", []), "", "$x$")
        self.unsafe("$x$", projected("$x$", "", []), "new")


if __name__ == "__main__":
    unittest.main()
