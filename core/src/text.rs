//! Coordinate conversion and atomic application of LSP text edits.
//!
//! LSP positions use UTF-16 columns, while the editor uses Unicode-scalar
//! offsets. Keeping that conversion here prevents UI types from leaking into
//! the formatter protocol and gives an entire edit batch one indexing pass.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScalarOffset(usize);
impl ScalarOffset {
    pub const fn new(value: usize) -> Self {
        Self(value)
    }
    pub const fn get(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteOffset(usize);
impl ByteOffset {
    pub fn checked(source: &str, value: usize) -> Option<Self> {
        source.is_char_boundary(value).then_some(Self(value))
    }
    pub const fn get(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarRange(Range<usize>);
impl ScalarRange {
    pub fn start(&self) -> ScalarOffset {
        ScalarOffset(self.0.start)
    }
    pub fn end(&self) -> ScalarOffset {
        ScalarOffset(self.0.end)
    }
    /// Conversion at the UI boundary, where egui owns scalar cursor indices.
    pub fn into_range(self) -> Range<usize> {
        self.0
    }
}

use serde::{Deserialize, Serialize};

macro_rules! column_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u32);
        impl $name {
            pub const fn new(value: u32) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u32 {
                self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
column_type!(LineIndex);
column_type!(Utf16Column);
column_type!(ScalarColumn);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    /// Zero-based line number.
    pub line: LineIndex,
    /// Zero-based UTF-16 code-unit offset, as required by LSP.
    pub character: Utf16Column,
}
impl LspPosition {
    /// Decode known protocol units; editor offsets require the source index.
    pub const fn new(line: u32, utf16_column: u32) -> Self {
        Self {
            line: LineIndex::new(line),
            character: Utf16Column::new(utf16_column),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// A standard LSP text edit returned by `textDocument/formatting`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspTextEdit {
    pub range: LspRange,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedTextEdits {
    pub text: String,
    pub mapped_offsets: [ScalarOffset; 2],
}

pub fn range_to_scalar_range(source: &str, range: &LspRange) -> ScalarRange {
    let index = SourceIndex::new(source);
    let start = index.permissive_char_offset(&range.start);
    let end = index.permissive_char_offset(&range.end).max(start);
    ScalarRange(start..end)
}

pub fn apply_text_edits(
    source: &str,
    edits: &[LspTextEdit],
    offsets: [ScalarOffset; 2],
) -> Result<AppliedTextEdits, String> {
    let index = SourceIndex::new(source);
    let resolved = resolve_text_edits(&index, edits)?;
    let mapped_offsets = offsets
        .map(|offset| ScalarOffset::new(map_char_offset(index.char_len, offset.get(), &resolved)));
    Ok(AppliedTextEdits {
        text: apply_resolved_text_edits(source, &resolved),
        mapped_offsets,
    })
}

/// Converts an editor scalar offset to Tinymist preview's zero-based scalar
/// line and column. This preview command deliberately differs from LSP UTF-16.
pub fn scalar_position_at(source: &str, char_index: ScalarOffset) -> (LineIndex, ScalarColumn) {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in source.chars().take(char_index.get()) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(1);
        }
    }
    (LineIndex::new(line), ScalarColumn::new(character))
}

/// Converts an editor Unicode-scalar offset to a standard zero-based LSP
/// position. Unlike preview navigation, the column counts UTF-16 code units.
///
/// ```compile_fail
/// use tiptoptyp_core::text::{ByteOffset, lsp_position_at_scalar};
/// lsp_position_at_scalar("é", ByteOffset::checked("é", 2).unwrap());
/// ```
pub fn lsp_position_at_scalar(source: &str, char_index: ScalarOffset) -> LspPosition {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in source.chars().take(char_index.get()) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(value.len_utf16() as u32);
        }
    }
    LspPosition::new(line, character)
}

#[derive(Debug, Clone, Copy)]
struct LineStart {
    byte: usize,
    character: usize,
}

#[derive(Debug)]
struct SourceIndex<'a> {
    source: &'a str,
    lines: Vec<LineStart>,
    char_len: usize,
}

impl<'a> SourceIndex<'a> {
    fn new(source: &'a str) -> Self {
        let mut lines = vec![LineStart {
            byte: 0,
            character: 0,
        }];
        let mut char_len = 0;
        for (byte, character) in source.char_indices() {
            char_len += 1;
            if character == '\n' {
                lines.push(LineStart {
                    byte: byte + 1,
                    character: char_len,
                });
            }
        }
        Self {
            source,
            lines,
            char_len,
        }
    }

    fn line(&self, line: LineIndex, strip_carriage_return: bool) -> Option<IndexedLine<'a>> {
        let index = line.get() as usize;
        let start = *self.lines.get(index)?;
        let mut end_byte = self
            .lines
            .get(index + 1)
            .map_or(self.source.len(), |next| next.byte);
        if end_byte > start.byte && self.source.as_bytes().get(end_byte - 1) == Some(&b'\n') {
            end_byte -= 1;
        }
        if strip_carriage_return
            && end_byte > start.byte
            && self.source.as_bytes().get(end_byte - 1) == Some(&b'\r')
        {
            end_byte -= 1;
        }
        Some(IndexedLine {
            text: &self.source[start.byte..end_byte],
            start,
        })
    }

    fn permissive_char_offset(&self, position: &LspPosition) -> usize {
        // Preserve the editor's forgiving navigation behavior: an invalid line
        // targets EOF, an overlong column targets EOL, and a split surrogate
        // targets the scalar immediately before it.
        let Some(line) = self.line(position.line, false) else {
            return self.char_len;
        };
        let target_utf16 = position.character.get() as usize;
        let mut utf16 = 0;
        let mut characters = 0;
        for character in line.text.chars() {
            let next = utf16 + character.len_utf16();
            if next > target_utf16 {
                break;
            }
            utf16 = next;
            characters += 1;
        }
        line.start.character + characters
    }

    fn strict_offset(&self, position: &LspPosition) -> Result<TextOffset, String> {
        let Some(line) = self.line(position.line, true) else {
            return Err(format!("line {} is outside the document", position.line));
        };
        let target = position.character.get() as usize;
        let mut utf16 = 0;
        let mut characters = 0;
        for (byte, character) in line.text.char_indices() {
            if utf16 == target {
                return Ok(TextOffset {
                    byte: ByteOffset(line.start.byte + byte),
                    character: line.start.character + characters,
                });
            }
            let next = utf16 + character.len_utf16();
            if target < next {
                return Err(format!(
                    "UTF-16 column {} splits a surrogate pair on line {}",
                    position.character, position.line
                ));
            }
            utf16 = next;
            characters += 1;
        }
        if utf16 == target {
            Ok(TextOffset {
                byte: ByteOffset(line.start.byte + line.text.len()),
                character: line.start.character + characters,
            })
        } else {
            Err(format!(
                "UTF-16 column {} is outside line {}",
                position.character, position.line
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct IndexedLine<'a> {
    text: &'a str,
    start: LineStart,
}

#[derive(Debug, Clone, Copy)]
struct TextOffset {
    byte: ByteOffset,
    character: usize,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedTextEdit<'a> {
    start: TextOffset,
    end: TextOffset,
    replacement: &'a str,
    replacement_chars: usize,
}

fn resolve_text_edits<'a>(
    index: &SourceIndex<'_>,
    edits: &'a [LspTextEdit],
) -> Result<Vec<ResolvedTextEdit<'a>>, String> {
    let mut resolved = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = index.strict_offset(&edit.range.start)?;
        let end = index.strict_offset(&edit.range.end)?;
        if start.byte > end.byte {
            return Err("an edit range ends before it starts".to_owned());
        }
        resolved.push(ResolvedTextEdit {
            start,
            end,
            replacement: &edit.new_text,
            replacement_chars: edit.new_text.chars().count(),
        });
    }

    // Stable sorting preserves the server's order for multiple insertions at
    // one position, matching sequential reverse-range replacement.
    resolved.sort_by_key(|edit| (edit.start.byte, edit.end.byte));
    if resolved
        .windows(2)
        .any(|pair| pair[1].start.byte < pair[0].end.byte)
    {
        return Err("formatting edits overlap".to_owned());
    }
    Ok(resolved)
}

fn apply_resolved_text_edits(source: &str, edits: &[ResolvedTextEdit<'_>]) -> String {
    let removed_bytes = edits
        .iter()
        .map(|edit| edit.end.byte.get() - edit.start.byte.get())
        .sum::<usize>();
    let replacement_bytes = edits
        .iter()
        .map(|edit| edit.replacement.len())
        .sum::<usize>();
    let capacity = source
        .len()
        .saturating_sub(removed_bytes)
        .saturating_add(replacement_bytes);
    let mut formatted = String::with_capacity(capacity);
    let mut source_cursor = 0;
    for edit in edits {
        formatted.push_str(&source[source_cursor..edit.start.byte.get()]);
        formatted.push_str(edit.replacement);
        source_cursor = edit.end.byte.get();
    }
    formatted.push_str(&source[source_cursor..]);
    formatted
}

fn map_char_offset(source_len: usize, index: usize, edits: &[ResolvedTextEdit<'_>]) -> usize {
    let index = index.min(source_len);
    let mut delta = 0_isize;
    for edit in edits {
        if index < edit.start.character {
            break;
        }
        if edit.start.character == edit.end.character {
            delta += edit.replacement_chars as isize;
            continue;
        }
        let shifted_start = (edit.start.character as isize + delta).max(0) as usize;
        if index == edit.start.character {
            return shifted_start;
        }
        if index < edit.end.character {
            return shifted_start + edit.replacement_chars;
        }
        delta +=
            edit.replacement_chars as isize - (edit.end.character - edit.start.character) as isize;
    }
    (index as isize + delta).max(0) as usize
}

#[cfg(test)]
mod tests {
    #[test]
    fn unicode_positions_round_trip_at_valid_protocol_boundaries() {
        for source in ["", "a😀文e\u{301}\n", "😀\r\n文\r\n", "\n\n", "😀😀😀"] {
            let index = super::SourceIndex::new(source);
            for (scalar, byte) in source
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([source.len()])
                .enumerate()
            {
                // Between CR and LF is not a protocol column in CRLF text.
                if source[..byte].ends_with('\r') {
                    continue;
                }
                let position =
                    super::lsp_position_at_scalar(source, super::ScalarOffset::new(scalar));
                let resolved = index.strict_offset(&position).unwrap();
                assert_eq!(resolved.character, scalar, "{source:?} at {byte}");
                assert_eq!(resolved.byte.get(), byte, "{source:?} at {byte}");
            }
        }
    }
    use super::*;

    fn position_to_char(source: &str, position: &LspPosition) -> usize {
        SourceIndex::new(source).permissive_char_offset(position)
    }

    fn edit(line: u32, start: u32, end: u32, new_text: &str) -> LspTextEdit {
        LspTextEdit {
            range: LspRange {
                start: LspPosition::new(line, start),
                end: LspPosition::new(line, end),
            },
            new_text: new_text.to_owned(),
        }
    }

    #[test]
    fn utf16_positions_map_to_editor_scalar_offsets() {
        let source = "a🦀b\nsecond";
        assert_eq!(position_to_char(source, &LspPosition::new(0, 3),), 2);
        assert_eq!(position_to_char(source, &LspPosition::new(1, 3),), 7);
    }

    #[test]
    fn editor_offsets_map_to_utf16_hover_positions() {
        let source = "a🦀b\nsecond";
        assert_eq!(
            lsp_position_at_scalar(source, ScalarOffset::new(2)),
            LspPosition::new(0, 3)
        );
        assert_eq!(
            lsp_position_at_scalar(source, ScalarOffset::new(7)),
            LspPosition::new(1, 3)
        );
        assert_eq!(
            lsp_position_at_scalar(source, ScalarOffset::new(usize::MAX)),
            LspPosition::new(1, 6)
        );
    }

    #[test]
    fn formatting_edits_are_utf16_strict_atomic_and_unicode_safe() {
        let source = "a🦀b\nsecond";
        let applied = apply_text_edits(
            source,
            &[edit(0, 1, 3, "crab")],
            ([0, 0]).map(ScalarOffset::new),
        )
        .unwrap();
        assert_eq!(applied.text, "acrabb\nsecond");

        let splits_surrogate = [edit(0, 2, 3, "")];
        assert!(
            apply_text_edits(source, &splits_surrogate, ([0, 0]).map(ScalarOffset::new)).is_err()
        );
        assert_eq!(source, "a🦀b\nsecond");
    }

    #[test]
    fn formatting_edits_map_unicode_cursor_and_selection() {
        let source = "a🦀b";
        let edits = [edit(0, 1, 1, " "), edit(0, 3, 4, "bee")];
        let applied = apply_text_edits(source, &edits, ([3, 1]).map(ScalarOffset::new)).unwrap();

        assert_eq!(applied.text, "a 🦀bee");
        assert_eq!(applied.mapped_offsets.map(ScalarOffset::get), [6, 2]);
    }

    #[test]
    fn cursor_inside_replacement_moves_to_replacement_end() {
        let applied = apply_text_edits(
            "abcd",
            &[edit(0, 0, 3, "xy")],
            ([2, 2]).map(ScalarOffset::new),
        )
        .unwrap();
        assert_eq!(applied.text, "xyd");
        assert_eq!(applied.mapped_offsets.map(ScalarOffset::get), [2, 2]);
    }

    #[test]
    fn adjacent_unsorted_edits_preserve_document_and_server_order() {
        let edits = [
            edit(0, 4, 6, "EF"),
            edit(0, 1, 3, "BC"),
            edit(0, 0, 1, "A"),
            edit(0, 3, 3, "<"),
            edit(0, 3, 3, ">"),
        ];
        let applied = apply_text_edits("abcdef", &edits, ([0, 6]).map(ScalarOffset::new)).unwrap();
        assert_eq!(applied.text, "ABC<>dEF");
        assert_eq!(applied.mapped_offsets.map(ScalarOffset::get), [0, 8]);
    }

    #[test]
    fn overlapping_edits_reject_the_entire_batch() {
        let error = apply_text_edits(
            "abcdef",
            &[edit(0, 1, 4, "one"), edit(0, 3, 5, "two")],
            ([2, 2]).map(ScalarOffset::new),
        )
        .unwrap_err();
        assert_eq!(error, "formatting edits overlap");
    }

    #[test]
    fn crlf_end_of_line_positions_exclude_line_terminators() {
        let source = "a🦀\r\nline\r\n";
        let applied = apply_text_edits(
            source,
            &[edit(1, 4, 4, "!"), edit(0, 3, 3, "!")],
            ([2, 7]).map(ScalarOffset::new),
        )
        .unwrap();
        assert_eq!(applied.text, "a🦀!\r\nline!\r\n");
        assert!(
            apply_text_edits(
                source,
                &[edit(1, 5, 5, "!")],
                ([0, 0]).map(ScalarOffset::new)
            )
            .is_err()
        );
    }

    #[test]
    fn large_unsorted_batch_applies_atomically() {
        const LINES: usize = 4_096;
        let source = "x\n".repeat(LINES);
        let edits = (0..LINES)
            .rev()
            .map(|line| edit(line as u32, 0, 1, "xy"))
            .collect::<Vec<_>>();
        let applied = apply_text_edits(
            &source,
            &edits,
            ([0, source.chars().count()]).map(ScalarOffset::new),
        )
        .unwrap();
        assert_eq!(applied.text, "xy\n".repeat(LINES));
        assert_eq!(
            applied.mapped_offsets.map(ScalarOffset::get),
            [0, applied.text.chars().count()]
        );
    }

    #[test]
    fn scalar_offsets_map_to_tinymist_preview_positions() {
        let source = "a🦀b\nsecond";
        assert_eq!(
            scalar_position_at(source, ScalarOffset::new(2)),
            (LineIndex::new(0), ScalarColumn::new(2))
        );
        assert_eq!(
            scalar_position_at(source, ScalarOffset::new(7)),
            (LineIndex::new(1), ScalarColumn::new(3))
        );
        assert_eq!(
            scalar_position_at(source, ScalarOffset::new(usize::MAX)),
            (LineIndex::new(1), ScalarColumn::new(6))
        );
    }
}
