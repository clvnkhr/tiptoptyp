//! Lazy, revision-owned line indices. Queries scan at most the affected lines,
//! never rebuild a document-wide index for each diagnostic or hover position.
use super::CanonicalSnapshot;
use std::ops::Range;
use tiptoptyp_core::text::{LineIndex, LspPosition, LspRange, ScalarColumn, ScalarOffset};

#[derive(Debug)]
pub(super) struct Coordinates {
    editor: Lines,
    canonical: Lines,
}

impl CanonicalSnapshot {
    /// Canonical zero-based line interval -> displayed gutter interval. Include
    /// the final touched line, not the line following a trailing newline.
    pub fn editor_lines(&self, lines: Range<usize>) -> Range<usize> {
        let indices = self.coordinates();
        let byte_at = |line: usize| {
            indices
                .canonical
                .starts
                .get(line)
                .map_or(self.source().len(), |line| line.byte)
        };
        let start = byte_at(lines.start);
        let end = byte_at(lines.end);
        let touched_end = if start < end && self.source().as_bytes().get(end - 1) == Some(&b'\n') {
            end - 1
        } else {
            end
        };
        let mapped_line = |byte| {
            self.canonical_to_editor(byte)
                .and_then(|byte| indices.editor.scalar_position(self.editor_source(), byte))
                .map_or(0, |(line, _)| line)
        };
        let first = mapped_line(start);
        first..if lines.is_empty() {
            first
        } else {
            mapped_line(touched_end).max(first) + 1
        }
    }
    pub fn canonical_scalar_cursor(&self, cursor: ScalarOffset) -> Option<ScalarOffset> {
        let indices = self.coordinates();
        let byte = indices
            .editor
            .byte_at_scalar(self.editor_source(), cursor.get())?;
        let byte = self.editor_to_canonical(byte)?;
        indices
            .canonical
            .scalar_at_byte(self.source(), byte)
            .map(ScalarOffset::new)
    }
    pub fn editor_scalar_cursor(&self, cursor: ScalarOffset) -> Option<ScalarOffset> {
        let indices = self.coordinates();
        let byte = indices
            .canonical
            .byte_at_scalar(self.source(), cursor.get())?;
        let byte = self.canonical_to_editor(byte)?;
        indices
            .editor
            .scalar_at_byte(self.editor_source(), byte)
            .map(ScalarOffset::new)
    }
    fn coordinates(&self) -> &Coordinates {
        self.coordinates.get_or_init(|| Coordinates {
            editor: Lines::new(self.editor_source()),
            canonical: Lines::new(self.source()),
        })
    }

    /// Editor scalar cursor -> canonical LSP UTF-16 position.
    pub fn canonical_lsp_position(&self, cursor: ScalarOffset) -> Option<LspPosition> {
        let indices = self.coordinates();
        let byte = indices
            .editor
            .byte_at_scalar(self.editor_source(), cursor.get())?;
        let byte = self.editor_to_canonical(byte)?;
        indices.canonical.lsp_at_byte(self.source(), byte)
    }

    /// Editor scalar cursor -> canonical preview scalar line/column. Preview
    /// navigation does not use LSP's UTF-16 columns.
    pub fn canonical_preview_position(
        &self,
        cursor: ScalarOffset,
    ) -> Option<(LineIndex, ScalarColumn)> {
        let indices = self.coordinates();
        let byte = indices
            .editor
            .byte_at_scalar(self.editor_source(), cursor.get())?;
        let byte = self.editor_to_canonical(byte)?;
        let (line, column) = indices.canonical.scalar_position(self.source(), byte)?;
        Some((
            LineIndex::new(line.try_into().ok()?),
            ScalarColumn::new(column.try_into().ok()?),
        ))
    }

    /// Canonical LSP range -> editor scalar range for navigation/diagnostics.
    /// Reject invalid positions, reversed ranges and split surrogate pairs.
    /// Generated wrapper/import spans may collapse to a boundary; this is NOT
    /// an API for rewriting editor text with canonical LSP edit payloads.
    pub fn editor_range(&self, range: LspRange) -> Option<Range<usize>> {
        let indices = self.coordinates();
        let start = indices.canonical.byte_at_lsp(self.source(), range.start)?;
        let end = indices.canonical.byte_at_lsp(self.source(), range.end)?;
        if end < start {
            return None;
        }
        let start = self.canonical_to_editor(start)?;
        let end = self.canonical_to_editor(end)?;
        Some(
            indices.editor.scalar_at_byte(self.editor_source(), start)?
                ..indices.editor.scalar_at_byte(self.editor_source(), end)?,
        )
    }
}

#[derive(Debug)]
struct Line {
    byte: usize,
    scalar: usize,
}
#[derive(Debug)]
struct Lines {
    starts: Vec<Line>,
    scalar_len: usize,
}
impl Lines {
    fn new(source: &str) -> Self {
        let mut starts = vec![Line { byte: 0, scalar: 0 }];
        let mut scalar_len = 0;
        for (byte, ch) in source.char_indices() {
            scalar_len += 1;
            if ch == '\n' {
                starts.push(Line {
                    byte: byte + 1,
                    scalar: scalar_len,
                });
            }
        }
        Self { starts, scalar_len }
    }
    fn byte_at_scalar(&self, source: &str, scalar: usize) -> Option<usize> {
        if scalar == self.scalar_len {
            return Some(source.len());
        }
        if scalar > self.scalar_len {
            return None;
        }
        let line = &self.starts[self.starts.partition_point(|line| line.scalar <= scalar) - 1];
        source[line.byte..]
            .char_indices()
            .nth(scalar - line.scalar)
            .map(|(byte, _)| line.byte + byte)
    }
    fn scalar_at_byte(&self, source: &str, byte: usize) -> Option<usize> {
        if !source.is_char_boundary(byte) {
            return None;
        }
        let line = &self.starts[self.starts.partition_point(|line| line.byte <= byte) - 1];
        Some(line.scalar + source[line.byte..byte].chars().count())
    }
    fn scalar_position(&self, source: &str, byte: usize) -> Option<(usize, usize)> {
        if !source.is_char_boundary(byte) {
            return None;
        }
        let index = self.starts.partition_point(|line| line.byte <= byte) - 1;
        Some((index, source[self.starts[index].byte..byte].chars().count()))
    }
    fn lsp_at_byte(&self, source: &str, byte: usize) -> Option<LspPosition> {
        if !source.is_char_boundary(byte) {
            return None;
        }
        let index = self.starts.partition_point(|line| line.byte <= byte) - 1;
        let column = source[self.starts[index].byte..byte].encode_utf16().count();
        Some(LspPosition::new(
            index.try_into().ok()?,
            column.try_into().ok()?,
        ))
    }
    fn byte_at_lsp(&self, source: &str, position: LspPosition) -> Option<usize> {
        let index = position.line.get() as usize;
        let start = self.starts.get(index)?.byte;
        let end = self
            .starts
            .get(index + 1)
            .map_or(source.len(), |line| line.byte);
        let line = &source[start..end];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let target = position.character.get() as usize;
        let mut utf16 = 0;
        for (byte, ch) in line.char_indices() {
            if utf16 == target {
                return Some(start + byte);
            }
            utf16 += ch.len_utf16();
            if utf16 > target {
                return None;
            }
        }
        (utf16 == target).then_some(start + line.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_indices_round_trip_unicode_crlf_and_empty_lines() {
        for text in ["", "\n", "😀\r\n文e\u{301}\n\n", "a😀b\r\n🦀"] {
            let lines = Lines::new(text);
            for (scalar, byte) in text
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([text.len()])
                .enumerate()
            {
                assert_eq!(lines.byte_at_scalar(text, scalar), Some(byte));
                assert_eq!(lines.scalar_at_byte(text, byte), Some(scalar));
                if !text[..byte].ends_with('\r') {
                    let position = lines.lsp_at_byte(text, byte).unwrap();
                    assert_eq!(lines.byte_at_lsp(text, position), Some(byte));
                }
            }
            assert_eq!(lines.byte_at_scalar(text, lines.scalar_len + 1), None);
            assert_eq!(lines.scalar_at_byte(text, text.len() + 1), None);
            assert_eq!(lines.byte_at_lsp(text, LspPosition::new(99, 0)), None);
            assert_eq!(lines.byte_at_lsp(text, LspPosition::new(0, 99)), None);
        }
    }
}
