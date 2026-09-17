//! Validated, versioned completion edits. A consumed transaction creates one undo entry.
use crate::tinymist::CompletionItem;
use std::ops::Range;
use tiptoptyp_core::{
    document::DocumentKey,
    text::{
        AppliedTextEdits, LspRange, LspTextEdit, ScalarOffset, apply_text_edits,
        lsp_position_at_scalar, range_to_scalar_range,
    },
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum CompletionCoordinates {
    Display,
    Canonical,
}

/// Owns one validated edit batch and its resulting selection, not another source snapshot.
/// Construction performs core range validation; commit rechecks identity and miTeX preflight.
pub(crate) struct CompletionTransaction {
    key: DocumentKey,
    coordinates: CompletionCoordinates,
    applied: AppliedTextEdits,
}
impl CompletionTransaction {
    pub(crate) fn prepare(
        key: DocumentKey,
        source: &str,
        cursor: usize,
        item: &CompletionItem,
        coordinates: CompletionCoordinates,
    ) -> Result<Self, String> {
        let application = prepare_completion_application(source, cursor, item)?;
        Ok(Self {
            key,
            coordinates,
            applied: AppliedTextEdits {
                text: application.source,
                mapped_offsets: [ScalarOffset::new(application.cursor); 2],
            },
        })
    }

    /// The transaction itself is the single undo intent: never apply edits piecemeal.
    pub(crate) fn commit<C>(
        self,
        document: &mut tiptoptyp::mitex_document::Document<C>,
        before: C,
    ) -> Result<Range<usize>, String> {
        if document.key() != self.key {
            return Err("the completion belongs to an outdated document".to_owned());
        }
        let applied = match self.coordinates {
            CompletionCoordinates::Display => self.applied,
            CompletionCoordinates::Canonical => document
                .project_canonical_change(self.key, self.applied)
                .map_err(|error| error.to_string())?,
        };
        let selection = applied.mapped_offsets[0].get()..applied.mapped_offsets[1].get();
        document.edit(before, |source| *source = applied.text);
        Ok(selection)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionApplication {
    pub(crate) source: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnippetExpansion {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

fn completion_prefix_range(source: &str, cursor: usize) -> Range<usize> {
    let cursor = cursor.min(source.chars().count());
    let prefix = source.chars().take(cursor).collect::<Vec<_>>();
    let start = prefix
        .iter()
        .rev()
        .take_while(|&&character| character.is_alphanumeric() || matches!(character, '_' | '-'))
        .count();
    cursor.saturating_sub(start)..cursor
}

fn completion_ranges_conflict(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() && right.is_empty() {
        return left.start == right.start;
    }
    if left.is_empty() {
        return right.start <= left.start && left.start <= right.end;
    }
    if right.is_empty() {
        return left.start <= right.start && right.start <= left.end;
    }
    left.start < right.end && right.start < left.end
}

pub(crate) fn prepare_completion_application(
    source: &str,
    request_cursor: usize,
    item: &CompletionItem,
) -> Result<CompletionApplication, String> {
    let source_len = source.chars().count();
    if request_cursor > source_len {
        return Err("the completion cursor is outside the document".to_owned());
    }

    let mut main_edit = item.text_edit.clone().unwrap_or_else(|| {
        let range = completion_prefix_range(source, request_cursor);
        LspTextEdit {
            range: LspRange {
                start: lsp_position_at_scalar(source, ScalarOffset::new(range.start)),
                end: lsp_position_at_scalar(source, ScalarOffset::new(range.end)),
            },
            new_text: item.insert_text.clone(),
        }
    });
    let expansion = if item.insert_text_is_snippet {
        expand_lsp_snippet(&main_edit.new_text)?
    } else {
        SnippetExpansion {
            cursor: main_edit.new_text.chars().count(),
            text: main_edit.new_text.clone(),
        }
    };
    main_edit.new_text = expansion.text.clone();
    let main_range = range_to_scalar_range(source, &main_edit.range).into_range();

    for additional in &item.additional_text_edits {
        let additional_range = range_to_scalar_range(source, &additional.range).into_range();
        if completion_ranges_conflict(&main_range, &additional_range) {
            return Err("the completion's main and additional edits overlap".to_owned());
        }
    }

    let mut edits = Vec::with_capacity(1 + item.additional_text_edits.len());
    edits.push(main_edit);
    edits.extend(item.additional_text_edits.iter().cloned());
    let applied = apply_text_edits(
        source,
        &edits,
        ([main_range.end, main_range.end]).map(ScalarOffset::new),
    )?;
    let inserted_len = expansion.text.chars().count();
    let cursor = applied.mapped_offsets[0]
        .get()
        .saturating_sub(inserted_len)
        .saturating_add(expansion.cursor.min(inserted_len));
    Ok(CompletionApplication {
        source: applied.text,
        cursor,
    })
}

#[derive(Default)]
struct SnippetCursorTracker {
    first_tabstop: Option<(u32, usize)>,
    final_tabstop: Option<usize>,
}

impl SnippetCursorTracker {
    fn record(&mut self, tabstop: u32, cursor: usize) {
        if tabstop == 0 {
            self.final_tabstop.get_or_insert(cursor);
        } else if self
            .first_tabstop
            .is_none_or(|(current, _)| tabstop < current)
        {
            self.first_tabstop = Some((tabstop, cursor));
        }
    }
}

pub(crate) fn expand_lsp_snippet(snippet: &str) -> Result<SnippetExpansion, String> {
    let characters = snippet.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(snippet.len());
    let mut tracker = SnippetCursorTracker::default();
    expand_lsp_snippet_fragment(&characters, &mut output, &mut tracker)?;
    let cursor = tracker
        .first_tabstop
        .map(|(_, cursor)| cursor)
        .or(tracker.final_tabstop)
        .unwrap_or_else(|| output.chars().count());
    Ok(SnippetExpansion {
        text: output,
        cursor,
    })
}

fn expand_lsp_snippet_fragment(
    characters: &[char],
    output: &mut String,
    tracker: &mut SnippetCursorTracker,
) -> Result<(), String> {
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '\\' if index + 1 < characters.len()
                && matches!(characters[index + 1], '$' | '}' | '\\') =>
            {
                output.push(characters[index + 1]);
                index += 2;
            }
            '$' if index + 1 < characters.len() && characters[index + 1].is_ascii_digit() => {
                let (tabstop, next) = parse_snippet_number(characters, index + 1);
                tracker.record(tabstop, output.chars().count());
                index = next;
            }
            '$' if index + 1 < characters.len() && characters[index + 1] == '{' => {
                let close = snippet_closing_brace(characters, index + 2)
                    .ok_or_else(|| "an LSP snippet placeholder is not closed".to_owned())?;
                expand_braced_snippet(&characters[index + 2..close], output, tracker)?;
                index = close + 1;
            }
            '$' if index + 1 < characters.len()
                && (characters[index + 1].is_ascii_alphabetic()
                    || characters[index + 1] == '_') =>
            {
                index += 2;
                while index < characters.len()
                    && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
                {
                    index += 1;
                }
            }
            character => {
                output.push(character);
                index += 1;
            }
        }
    }
    Ok(())
}

fn parse_snippet_number(characters: &[char], start: usize) -> (u32, usize) {
    let mut value = 0_u32;
    let mut index = start;
    while index < characters.len() && characters[index].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(characters[index].to_digit(10).unwrap_or(0));
        index += 1;
    }
    (value, index)
}

fn snippet_closing_brace(characters: &[char], start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut escaped = false;
    for (index, character) in characters.iter().copied().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
        } else if character == '{' {
            depth += 1;
        } else if character == '}' {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
}

fn expand_braced_snippet(
    body: &[char],
    output: &mut String,
    tracker: &mut SnippetCursorTracker,
) -> Result<(), String> {
    if body
        .first()
        .is_some_and(|character| character.is_ascii_digit())
    {
        let (tabstop, after_number) = parse_snippet_number(body, 0);
        let cursor = output.chars().count();
        tracker.record(tabstop, cursor);
        match body.get(after_number) {
            None => {}
            Some(':') => {
                expand_lsp_snippet_fragment(&body[after_number + 1..], output, tracker)?;
            }
            Some('|') if body.last() == Some(&'|') => {
                let choice = first_snippet_choice(&body[after_number + 1..body.len() - 1]);
                output.extend(choice);
            }
            _ => return Err("an LSP snippet tabstop has an unsupported form".to_owned()),
        }
        return Ok(());
    }

    let separator = body.iter().position(|character| *character == ':');
    if let Some(separator) = separator {
        expand_lsp_snippet_fragment(&body[separator + 1..], output, tracker)?;
    } else if body
        .iter()
        .all(|character| character.is_ascii_alphanumeric() || *character == '_')
    {
        // Unknown variables have an empty value, as required by the snippet
        // fallback rules when no default is supplied.
    } else {
        return Err("an LSP snippet variable has an unsupported form".to_owned());
    }
    Ok(())
}

fn first_snippet_choice(characters: &[char]) -> Vec<char> {
    let mut choice = Vec::new();
    let mut escaped = false;
    for character in characters.iter().copied() {
        if escaped {
            choice.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ',' {
            break;
        } else {
            choice.push(character);
        }
    }
    if escaped {
        choice.push('\\');
    }
    choice
}
