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
    pub(crate) fields: Vec<(u32, Range<usize>)>,
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
            fields: application.fields,
            applied: AppliedTextEdits {
                text: application.source,
                mapped_offsets: [ScalarOffset::new(application.cursor); 2],
            },
        })
    }

    pub(crate) fn add_tex_imports(
        &mut self,
        required: &crate::tex::imports::Requirements,
    ) -> Result<(), String> {
        let mut offsets: Vec<_> = self
            .applied
            .mapped_offsets
            .iter()
            .map(|o| o.get())
            .collect();
        offsets.extend(
            self.fields
                .iter()
                .flat_map(|(_, range)| [range.start, range.end]),
        );
        crate::tex::imports::ensure(&mut self.applied.text, &mut offsets, required)?;
        self.applied.mapped_offsets =
            [ScalarOffset::new(offsets[0]), ScalarOffset::new(offsets[1])];
        for ((_, range), offsets) in self.fields.iter_mut().zip(offsets[2..].as_chunks::<2>().0) {
            *range = offsets[0]..offsets[1];
        }
        Ok(())
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
    pub(crate) fields: Vec<(u32, Range<usize>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnippetExpansion {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) fields: Vec<(u32, Range<usize>)>,
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
    let mut expansion = if item.insert_text_is_snippet {
        expand_lsp_snippet(&main_edit.new_text)?
    } else {
        SnippetExpansion {
            cursor: main_edit.new_text.chars().count(),
            text: main_edit.new_text.clone(),
            fields: vec![],
        }
    };
    let main_range = range_to_scalar_range(source, &main_edit.range).into_range();
    if item.origin == crate::lsp::protocol::CompletionOrigin::Snippet {
        reindent_snippet(source, main_range.start, &mut expansion);
    }
    main_edit.new_text = expansion.text.clone();

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
        fields: expansion
            .fields
            .into_iter()
            .map(|(id, range)| {
                let start = applied.mapped_offsets[0].get().saturating_sub(inserted_len);
                (id, range.start + start..range.end + start)
            })
            .collect(),
    })
}

/// Preserve relative body indentation and add only the surrounding line's
/// leading whitespace. Offset mapping happens before the validated edit batch.
fn reindent_snippet(source: &str, start: usize, expansion: &mut SnippetExpansion) {
    let byte = source
        .char_indices()
        .nth(start)
        .map_or(source.len(), |(at, _)| at);
    let line = source[..byte].rsplit('\n').next().unwrap_or_default();
    let indent: String = line
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();
    if indent.is_empty() || !expansion.text.contains('\n') {
        return;
    }
    let mut boundaries = vec![0];
    let mut text = String::with_capacity(expansion.text.len());
    let mut position = 0;
    for c in expansion.text.chars() {
        text.push(c);
        position += 1;
        if c == '\n' {
            text.push_str(&indent);
            position += indent.chars().count();
        }
        boundaries.push(position);
    }
    expansion.cursor = boundaries[expansion.cursor];
    for (_, range) in &mut expansion.fields {
        *range = boundaries[range.start]..boundaries[range.end];
    }
    expansion.text = text;
}

#[derive(Default)]
struct SnippetCursorTracker {
    first_tabstop: Option<(u32, usize)>,
    final_tabstop: Option<usize>,
    fields: Vec<(u32, Range<usize>)>,
    values: std::collections::BTreeMap<u32, String>,
    defaults: std::collections::BTreeMap<u32, Vec<char>>,
    resolving: std::collections::BTreeSet<u32>,
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
    collect_snippet_defaults(&characters, &mut tracker.defaults)?;
    expand_lsp_snippet_fragment(&characters, &mut output, &mut tracker)?;
    let cursor = tracker
        .first_tabstop
        .map(|(_, cursor)| cursor)
        .or(tracker.final_tabstop)
        .unwrap_or_else(|| output.chars().count());
    if !tracker.fields.is_empty() && tracker.final_tabstop.is_none() {
        let end = output.chars().count();
        tracker.fields.push((0, end..end));
    }
    Ok(SnippetExpansion {
        text: output,
        cursor,
        fields: tracker.fields,
    })
}

fn collect_snippet_defaults(
    characters: &[char],
    defaults: &mut std::collections::BTreeMap<u32, Vec<char>>,
) -> Result<(), String> {
    let mut i = 0;
    while i < characters.len() {
        if characters[i] == '\\' {
            i += 2;
            continue;
        }
        if characters[i] == '$' && characters.get(i + 1) == Some(&'{') {
            let end = snippet_closing_brace(characters, i + 2)
                .ok_or_else(|| "an LSP snippet placeholder is not closed".to_owned())?;
            let body = &characters[i + 2..end];
            if body.first().is_some_and(char::is_ascii_digit) {
                let (id, after) = parse_snippet_number(body, 0);
                if body.get(after) == Some(&':') || body.get(after) == Some(&'|') {
                    defaults.entry(id).or_insert_with(|| body[after..].to_vec());
                }
            }
            collect_snippet_defaults(body, defaults)?;
            i = end + 1;
        } else {
            i += 1;
        }
    }
    Ok(())
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
                let (_, next) = parse_snippet_number(characters, index + 1);
                expand_braced_snippet(&characters[index + 1..next], output, tracker)?;
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
        } else if character == '$' && characters.get(index + 1) == Some(&'{') {
            // A literal opening brace is allowed in an LSP default. Only a
            // nested placeholder opens another snippet delimiter; its literal
            // closing braces are escaped by the source snippet adapter.
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
        if let Some(value) = tracker.values.get(&tabstop) {
            output.push_str(value);
            tracker
                .fields
                .push((tabstop, cursor..output.chars().count()));
            return Ok(());
        }
        if !tracker.resolving.insert(tabstop) {
            tracker.fields.push((tabstop, cursor..cursor));
            return Ok(());
        }
        let default = tracker.defaults.get(&tabstop).cloned();
        let form = default.as_deref().unwrap_or(&body[after_number..]);
        match form.first() {
            None => {}
            Some(':') => {
                expand_lsp_snippet_fragment(&form[1..], output, tracker)?;
            }
            Some('|') if form.last() == Some(&'|') => {
                let choice = first_snippet_choice(&form[1..form.len() - 1]);
                output.extend(choice);
            }
            _ => return Err("an LSP snippet tabstop has an unsupported form".to_owned()),
        }
        tracker.resolving.remove(&tabstop);
        tracker
            .values
            .insert(tabstop, output.chars().skip(cursor).collect());
        tracker
            .fields
            .push((tabstop, cursor..output.chars().count()));
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

#[cfg(test)]
mod tex_import_tests {
    use super::*;
    #[test]
    fn tex_imports_completion_keeps_linked_fields_and_has_one_undo() {
        let source = "\\documentclass{article}\n\\begin{document}é eq\\end{document}";
        let mut document = crate::document::DocumentSession::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            source,
            crate::document::DocumentKind::Tex,
        );
        let cursor = source[..source.find("eq").unwrap()].chars().count() + 2;
        let body = "\\begin{align}${1:α} &= $1$0\\end{align}";
        let item = CompletionItem {
            origin: crate::lsp::protocol::CompletionOrigin::LanguageServer,
            label: "equation".into(),
            detail: None,
            documentation: None,
            filter_text: None,
            sort_text: None,
            insert_text: body.into(),
            insert_text_is_snippet: true,
            text_edit: None,
            additional_text_edits: vec![],
        };
        let mut transaction = CompletionTransaction::prepare(
            document.key(),
            source,
            cursor,
            &item,
            CompletionCoordinates::Display,
        )
        .unwrap();
        let expansion = expand_lsp_snippet(body).unwrap();
        transaction
            .add_tex_imports(&crate::tex::imports::Requirements::fragment(
                &expansion.text,
            ))
            .unwrap();
        let fields = transaction.fields.clone();
        let before = eframe::egui::text::CCursorRange::default();
        transaction.commit(&mut document, before).unwrap();
        let applied = document.source().clone();
        assert!(applied.contains("\\usepackage{amsmath}\n"));
        for (_, range) in fields.iter().filter(|(id, _)| *id == 1) {
            assert_eq!(
                applied
                    .chars()
                    .skip(range.start)
                    .take(range.len())
                    .collect::<String>(),
                "α"
            );
        }
        assert_eq!(fields.iter().filter(|(id, _)| *id == 1).count(), 2);
        document.history_step(false, before);
        assert_eq!(document.source(), source);
    }
}
