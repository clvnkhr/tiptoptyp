//! Coordinate conversion and atomic application of LSP text edits.
//!
//! LSP positions use UTF-16 columns, while the editor uses Unicode-scalar
//! offsets. Keeping that conversion here prevents UI types from leaking into
//! the formatter protocol and gives an entire edit batch one indexing pass.

use std::ops::Range;

use crate::tinymist::{LspPosition, LspRange, LspTextEdit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedTextEdits {
    pub text: String,
    pub mapped_offsets: [usize; 2],
}

pub(crate) fn range_to_char_range(source: &str, range: &LspRange) -> Range<usize> {
    let index = SourceIndex::new(source);
    let start = index.permissive_char_offset(&range.start);
    let end = index.permissive_char_offset(&range.end).max(start);
    start..end
}

pub(crate) fn apply_text_edits(
    source: &str,
    edits: &[LspTextEdit],
    offsets: [usize; 2],
) -> Result<AppliedTextEdits, String> {
    let index = SourceIndex::new(source);
    let resolved = resolve_text_edits(&index, edits)?;
    let mapped_offsets = offsets.map(|offset| map_char_offset(index.char_len, offset, &resolved));
    Ok(AppliedTextEdits {
        text: apply_resolved_text_edits(source, &resolved),
        mapped_offsets,
    })
}

/// Converts an editor scalar offset to Tinymist preview's zero-based scalar
/// line and column. This preview command deliberately differs from LSP UTF-16.
pub(crate) fn scalar_position_at_char(source: &str, char_index: usize) -> (u32, u32) {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in source.chars().take(char_index) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(1);
        }
    }
    (line, character)
}

/// Converts an editor Unicode-scalar offset to a standard zero-based LSP
/// position. Unlike preview navigation, the column counts UTF-16 code units.
pub(crate) fn lsp_position_at_char(source: &str, char_index: usize) -> LspPosition {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for value in source.chars().take(char_index) {
        if value == '\n' {
            line = line.saturating_add(1);
            character = 0;
        } else {
            character = character.saturating_add(value.len_utf16() as u32);
        }
    }
    LspPosition { line, character }
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

    fn line(&self, line: u32, strip_carriage_return: bool) -> Option<IndexedLine<'a>> {
        let index = line as usize;
        let start = *self.lines.get(index)?;
        let mut end_byte = self
            .lines
            .get(index + 1)
            .map_or(self.source.len(), |next| next.byte);
        if self.source.as_bytes().get(end_byte.wrapping_sub(1)) == Some(&b'\n') {
            end_byte -= 1;
        }
        if strip_carriage_return
            && self.source.as_bytes().get(end_byte.wrapping_sub(1)) == Some(&b'\r')
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
        let target_utf16 = position.character as usize;
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
        let target = position.character as usize;
        let mut utf16 = 0;
        let mut characters = 0;
        for (byte, character) in line.text.char_indices() {
            if utf16 == target {
                return Ok(TextOffset {
                    byte: line.start.byte + byte,
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
                byte: line.start.byte + line.text.len(),
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
    byte: usize,
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
        .map(|edit| edit.end.byte - edit.start.byte)
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
        formatted.push_str(&source[source_cursor..edit.start.byte]);
        formatted.push_str(edit.replacement);
        source_cursor = edit.end.byte;
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
    use super::*;

    fn position_to_char(source: &str, position: &LspPosition) -> usize {
        SourceIndex::new(source).permissive_char_offset(position)
    }

    fn edit(line: u32, start: u32, end: u32, new_text: &str) -> LspTextEdit {
        LspTextEdit {
            range: LspRange {
                start: LspPosition {
                    line,
                    character: start,
                },
                end: LspPosition {
                    line,
                    character: end,
                },
            },
            new_text: new_text.to_owned(),
        }
    }

    #[test]
    fn utf16_positions_map_to_editor_scalar_offsets() {
        let source = "a🦀b\nsecond";
        assert_eq!(
            position_to_char(
                source,
                &LspPosition {
                    line: 0,
                    character: 3,
                },
            ),
            2
        );
        assert_eq!(
            position_to_char(
                source,
                &LspPosition {
                    line: 1,
                    character: 3,
                },
            ),
            7
        );
    }

    #[test]
    fn editor_offsets_map_to_utf16_hover_positions() {
        let source = "a🦀b\nsecond";
        assert_eq!(
            lsp_position_at_char(source, 2),
            LspPosition {
                line: 0,
                character: 3,
            }
        );
        assert_eq!(
            lsp_position_at_char(source, 7),
            LspPosition {
                line: 1,
                character: 3,
            }
        );
        assert_eq!(
            lsp_position_at_char(source, usize::MAX),
            LspPosition {
                line: 1,
                character: 6,
            }
        );
    }

    #[test]
    fn formatting_edits_are_utf16_strict_atomic_and_unicode_safe() {
        let source = "a🦀b\nsecond";
        let applied = apply_text_edits(source, &[edit(0, 1, 3, "crab")], [0, 0]).unwrap();
        assert_eq!(applied.text, "acrabb\nsecond");

        let splits_surrogate = [edit(0, 2, 3, "")];
        assert!(apply_text_edits(source, &splits_surrogate, [0, 0]).is_err());
        assert_eq!(source, "a🦀b\nsecond");
    }

    #[test]
    fn formatting_edits_map_unicode_cursor_and_selection() {
        let source = "a🦀b";
        let edits = [edit(0, 1, 1, " "), edit(0, 3, 4, "bee")];
        let applied = apply_text_edits(source, &edits, [3, 1]).unwrap();

        assert_eq!(applied.text, "a 🦀bee");
        assert_eq!(applied.mapped_offsets, [6, 2]);
    }

    #[test]
    fn cursor_inside_replacement_moves_to_replacement_end() {
        let applied = apply_text_edits("abcd", &[edit(0, 0, 3, "xy")], [2, 2]).unwrap();
        assert_eq!(applied.text, "xyd");
        assert_eq!(applied.mapped_offsets, [2, 2]);
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
        let applied = apply_text_edits("abcdef", &edits, [0, 6]).unwrap();
        assert_eq!(applied.text, "ABC<>dEF");
        assert_eq!(applied.mapped_offsets, [0, 8]);
    }

    #[test]
    fn overlapping_edits_reject_the_entire_batch() {
        let error = apply_text_edits(
            "abcdef",
            &[edit(0, 1, 4, "one"), edit(0, 3, 5, "two")],
            [2, 2],
        )
        .unwrap_err();
        assert_eq!(error, "formatting edits overlap");
    }

    #[test]
    fn crlf_end_of_line_positions_exclude_line_terminators() {
        let source = "a🦀\r\nline\r\n";
        let applied =
            apply_text_edits(source, &[edit(1, 4, 4, "!"), edit(0, 3, 3, "!")], [2, 7]).unwrap();
        assert_eq!(applied.text, "a🦀!\r\nline!\r\n");
        assert!(apply_text_edits(source, &[edit(1, 5, 5, "!")], [0, 0]).is_err());
    }

    #[test]
    fn large_unsorted_batch_applies_atomically() {
        const LINES: usize = 4_096;
        let source = "x\n".repeat(LINES);
        let edits = (0..LINES)
            .rev()
            .map(|line| edit(line as u32, 0, 1, "xy"))
            .collect::<Vec<_>>();
        let applied = apply_text_edits(&source, &edits, [0, source.chars().count()]).unwrap();
        assert_eq!(applied.text, "xy\n".repeat(LINES));
        assert_eq!(applied.mapped_offsets, [0, applied.text.chars().count()]);
    }

    #[test]
    fn scalar_offsets_map_to_tinymist_preview_positions() {
        let source = "a🦀b\nsecond";
        assert_eq!(scalar_position_at_char(source, 2), (0, 2));
        assert_eq!(scalar_position_at_char(source, 7), (1, 3));
        assert_eq!(scalar_position_at_char(source, usize::MAX), (1, 6));
    }
}
