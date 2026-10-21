"""Conservative source edits for the developer-only Harper/ML comparison.

All offsets are Python/Rust Unicode scalar indices, never UTF-8 byte offsets.
A projection admits literal source characters and normalized whitespace only;
synthetic opaque placeholders are immutable. This is not an app edit API.
"""
from __future__ import annotations

from difflib import SequenceMatcher
from typing import Any


class _Unsafe(ValueError):
    pass


def map_correction(source: str, projection: dict[str, Any], corrected: str) -> dict[str, Any]:
    """Map localized model changes, atomically rejecting an unsafe proposal.

    ``edits`` retains safe candidate edits for inspection even if another edit
    is rejected; ``source_output`` changes only when *all* edits are safe.
    """
    edits: list[dict[str, Any]] = []
    rejected: list[dict[str, Any]] = []
    try:
        if not isinstance(source, str) or not isinstance(corrected, str):
            raise _Unsafe("Source and correction must be strings")
        if not isinstance(projection, dict) or not isinstance(projection.get("text"), str):
            raise _Unsafe("Projection must contain text")
        text = projection["text"]
        mapping = projection.get("source_map")
        protected_values = projection.get("protected_scalars", [])
        if not isinstance(mapping, list) or len(mapping) != len(text):
            raise _Unsafe("Projection map must have one entry per Unicode scalar")
        if not isinstance(protected_values, list) or any(
            type(index) is not int or not 0 <= index < len(text) for index in protected_values
        ) or len(set(protected_values)) != len(protected_values):
            raise _Unsafe("Protected scalar indices must be unique and in bounds")
        protected = set(protected_values)
        previous = -1
        for index, offset in enumerate(mapping):
            if offset is None:
                if index not in protected and not text[index].isspace():
                    raise _Unsafe("Unmapped non-whitespace must be protected")
            else:
                if type(offset) is not int or not 0 <= offset < len(source):
                    raise _Unsafe("Source scalar offset is out of bounds")
                if offset <= previous:
                    raise _Unsafe("Source mapping must be strictly increasing")
                if text[index] != source[offset]:
                    raise _Unsafe("Mapped projection character differs from source")
                if index in protected:
                    raise _Unsafe("Protected synthetic scalars cannot map to source")
                previous = offset
        if any(0xD800 <= ord(char) <= 0xDFFF for value in (source, text, corrected) for char in value):
            raise _Unsafe("Inputs must contain Unicode scalars, not surrogate code points")
    except _Unsafe as error:
        return {"edits": [], "rejected": [{"reason": str(error)}],
                "source_output": source, "all_safe": False}

    def whitespace_gap(left: int, right: int) -> tuple[int, int]:
        """Resolve a complete normalized run between two admitted characters."""
        if left < 0 or right >= len(text) or mapping[left] is None or mapping[right] is None:
            raise _Unsafe("Normalized whitespace has no unique source boundaries")
        if any(index in protected or not text[index].isspace() for index in range(left + 1, right)):
            raise _Unsafe("Whitespace boundary touches an opaque placeholder")
        start, end = mapping[left] + 1, mapping[right]
        if start >= end or not source[start:end].isspace():
            raise _Unsafe("Normalized whitespace crosses omitted source syntax")
        return start, end

    def run_bounds(index: int) -> tuple[int, int]:
        left, right = index - 1, index
        while left >= 0 and mapping[left] is None and text[left].isspace() and left not in protected:
            left -= 1
        while right < len(text) and mapping[right] is None and text[right].isspace() and right not in protected:
            right += 1
        return left, right

    def boundary(index: int, start: bool) -> int:
        adjacent = index if start else index - 1
        if 0 <= adjacent < len(text) and mapping[adjacent] is not None:
            return mapping[adjacent] if start else mapping[adjacent] + 1
        left, right = run_bounds(index)
        if (start and index != left + 1) or (not start and index != right):
            raise _Unsafe("Edit splits a normalized whitespace run")
        gap_start, gap_end = whitespace_gap(left, right)
        return gap_start if start else gap_end

    # A word artificially joined across omitted syntax is not an editable word.
    # Character diffing can otherwise disguise a cross-wrapper rewrite as two
    # individually local edits ("bad\\emph{word}" -> "good\\emph{}").
    split_words: list[tuple[int, int]] = []
    for index in range(1, len(text)):
        left, right = mapping[index - 1], mapping[index]
        if (left is not None and right is not None and right != left + 1
                and text[index - 1].isalnum() and text[index].isalnum()):
            begin, end = index - 1, index
            while begin > 0 and text[begin - 1].isalnum():
                begin -= 1
            while end < len(text) and text[end].isalnum():
                end += 1
            split_words.append((begin, end))

    def insertion(index: int, replacement: str) -> int:
        # Ignore intervening normalized whitespace when testing adjacency to
        # math: a word inserted beside synthetic x has no unique source side.
        for direction, cursor in ((-1, index - 1), (1, index)):
            # SequenceMatcher can place insertion "the " inside the preceding
            # word when letters repeat. Its apparent position is not evidence
            # that a word-level insertion is away from a math boundary.
            if any(char.isspace() for char in replacement):
                while 0 <= cursor < len(text) and text[cursor].isalnum() and cursor not in protected:
                    cursor += direction
            while 0 <= cursor < len(text) and text[cursor].isspace() and cursor not in protected:
                cursor += direction
            if cursor in protected:
                raise _Unsafe("Insertion is adjacent to an opaque placeholder")
        if index == 0:
            if text and mapping[0] == 0:
                return 0
            raise _Unsafe("Insertion at a fragment start has ambiguous source adjacency")
        if index == len(text):
            if text and mapping[-1] == len(source) - 1:
                return len(source)
            raise _Unsafe("Insertion at a fragment end has ambiguous source adjacency")
        left, right = mapping[index - 1], mapping[index]
        if left is not None and right is not None:
            if left + 1 == right:
                return right
            raise _Unsafe("Insertion crosses omitted source syntax")
        run_left, run_right = run_bounds(index)
        gap_start, gap_end = whitespace_gap(run_left, run_right)
        if index == run_left + 1:
            return gap_start
        if index == run_right:
            return gap_end
        raise _Unsafe("Insertion inside normalized whitespace has no unique source position")

    for operation, begin, end, new_begin, new_end in SequenceMatcher(
        None, text, corrected, autojunk=False
    ).get_opcodes():
        if operation == "equal":
            continue
        replacement = corrected[new_begin:new_end]
        try:
            if any(begin < word_end and end > word_start
                   or begin == end and word_start <= begin <= word_end
                   for word_start, word_end in split_words):
                raise _Unsafe("Edit touches a word joined across omitted source syntax")
            if any(index in protected for index in range(begin, end)):
                raise _Unsafe("Edit changes an opaque placeholder")
            if begin == end:
                source_start = source_end = insertion(begin, replacement)
            else:
                source_start, source_end = boundary(begin, True), boundary(end, False)
                mapped = [(index, mapping[index]) for index in range(begin, end)
                          if mapping[index] is not None]
                for (left_index, left), (right_index, right) in zip(mapped, mapped[1:]):
                    if right != left + 1:
                        if not text[left_index + 1:right_index].isspace():
                            raise _Unsafe("Edit crosses noncontiguous source characters")
                        whitespace_gap(left_index, right_index)
                if not 0 <= source_start <= source_end <= len(source):
                    raise _Unsafe("Mapped edit is outside source bounds")
            edits.append({"start": source_start, "end": source_end, "text": replacement})
        except _Unsafe as error:
            rejected.append({"start": begin, "end": end, "text": replacement,
                             "reason": str(error)})

    edits.sort(key=lambda edit: (edit["start"], edit["end"]))
    for left, right in zip(edits, edits[1:]):
        if left["end"] > right["start"] or left["start"] == right["start"]:
            rejected.append({"reason": "Mapped edits overlap or share an ambiguous source boundary"})
            break
    result = source
    if not rejected:
        for edit in reversed(edits):
            result = result[:edit["start"]] + edit["text"] + result[edit["end"]:]
    return {"edits": edits, "rejected": rejected, "source_output": result,
            "all_safe": not rejected}
