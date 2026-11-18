//! Reusable correction actions for offline and language-server diagnostics.
use super::*;
use crate::diagnostics::DiagnosticCorrection;

pub(super) fn show_corrections(
    ui: &mut egui::Ui,
    correction: &DiagnosticCorrection,
    current: Option<DocumentKey>,
) -> Option<usize> {
    ui.set_max_width(440.0);
    let mut selected = None;
    let enabled = current.is_some_and(|key| correction.is_current(key));
    for (index, replacement) in correction.replacements.iter().enumerate() {
        let label = if replacement.is_empty() {
            "Remove".to_owned()
        } else {
            format!("Replace with “{replacement}”")
        };
        if ui
            .add_enabled(enabled, egui::Button::new(label).wrap())
            .clicked()
        {
            selected = Some(index);
        }
    }
    if !correction.replacements.is_empty() {
        if !enabled {
            ui.weak("Waiting for refreshed suggestions");
        }
        ui.separator();
    }
    selected
}

/// An automatic fix must have one unambiguous, meaningful outcome. A no-op
/// offered beside a real change is still an alternative, not permission to
/// choose the change. Duplicate provider suggestions do not add ambiguity.
pub(super) fn automatic_index(correction: &DiagnosticCorrection) -> Option<usize> {
    let first = correction.replacements.first()?;
    (first != &correction.original
        && correction
            .replacements
            .iter()
            .all(|replacement| replacement == first))
    .then_some(0)
}

/// Cheap menu admission: inspect only the bounded spelling payload, never the
/// document. The click handler rechecks both source ownership and exact text.
pub(super) fn dictionary_word(diagnostic: &Diagnostic) -> Option<&str> {
    let correction = diagnostic.correction.as_ref()?;
    if diagnostic.provider.as_deref() != Some("Harper")
        || correction.rule.as_deref() != Some("SpellCheck")
        || correction.range.is_empty()
    {
        return None;
    }
    let word = correction.original.as_str();
    let mut count = 0;
    let mut previous_letter = false;
    for character in word.chars() {
        count += 1;
        if count > 128 {
            return None;
        }
        if character.is_alphabetic() {
            previous_letter = true;
        } else if previous_letter
            && matches!(character, '\'' | '\u{2019}' | '-' | '\u{2010}' | '\u{2011}')
        {
            previous_letter = false;
        } else {
            return None;
        }
    }
    (count > 0 && previous_letter).then_some(word)
}

#[derive(Clone, Copy)]
struct AutomaticFix<'a> {
    correction: &'a DiagnosticCorrection,
    replacement: &'a str,
}

struct PreparedFixes {
    text: String,
    cursor: CCursorRange,
    count: usize,
}

/// Resolve all scalar endpoints in one source sweep. Overlapping alternatives
/// are omitted as a whole, including conflicting insertions at a boundary.
/// After that, exact-text validation touches disjoint source slices only.
fn prepare_automatic_fixes(
    source: &str,
    mut fixes: Vec<AutomaticFix<'_>>,
    cursor: CCursorRange,
) -> Result<Option<PreparedFixes>, String> {
    if fixes.is_empty() {
        return Ok(None);
    }
    if fixes
        .iter()
        .any(|fix| fix.correction.range.start > fix.correction.range.end)
    {
        return Err("A correction has an invalid source range.".into());
    }
    fixes.sort_unstable_by(|left, right| {
        let left_range = &left.correction.range;
        let right_range = &right.correction.range;
        (left_range.start, left_range.end, left.replacement).cmp(&(
            right_range.start,
            right_range.end,
            right.replacement,
        ))
    });
    fixes.dedup_by(|right, left| {
        right.correction.range == left.correction.range
            && right.replacement == left.replacement
            && right.correction.original == left.correction.original
    });
    let mut endpoints = fixes
        .iter()
        .flat_map(|fix| [fix.correction.range.start, fix.correction.range.end])
        .collect::<Vec<_>>();
    endpoints.sort_unstable();
    endpoints.dedup();
    let mut byte_offsets = Vec::with_capacity(endpoints.len());
    let mut source_len = 0;
    for (scalar, byte) in source
        .char_indices()
        .map(|(byte, _)| byte)
        .chain([source.len()])
        .enumerate()
    {
        source_len = scalar;
        if endpoints.get(byte_offsets.len()) == Some(&scalar) {
            byte_offsets.push(byte);
        }
    }
    if byte_offsets.len() != endpoints.len() {
        return Err("A correction is outside the document.".into());
    }
    let mut kept = Vec::with_capacity(fixes.len());
    let mut first = 0;
    while first < fixes.len() {
        let mut end = first + 1;
        let mut last_scalar = fixes[first].correction.range.end;
        let mut insertion_at_end = fixes[first].correction.range.is_empty();
        while let Some(next) = fixes.get(end) {
            let range = &next.correction.range;
            if range.start > last_scalar
                || (range.start == last_scalar && !range.is_empty() && !insertion_at_end)
            {
                break;
            }
            if range.end > last_scalar {
                last_scalar = range.end;
                insertion_at_end = range.is_empty();
            } else if range.end == last_scalar {
                insertion_at_end |= range.is_empty();
            }
            end += 1;
        }
        if end == first + 1 {
            kept.push(fixes[first]);
        }
        first = end;
    }
    if kept.is_empty() {
        return Ok(None);
    }
    let byte_at = |scalar| byte_offsets[endpoints.binary_search(&scalar).unwrap()];
    for fix in &kept {
        let range = &fix.correction.range;
        if source.get(byte_at(range.start)..byte_at(range.end))
            != Some(fix.correction.original.as_str())
        {
            return Err(
                "The source no longer matches a suggestion; wait for refreshed suggestions.".into(),
            );
        }
    }
    let mut text = String::with_capacity(source.len());
    let mut previous = 0;
    for fix in &kept {
        let range = &fix.correction.range;
        let start = byte_at(range.start);
        let end = byte_at(range.end);
        text.push_str(&source[previous..start]);
        text.push_str(fix.replacement);
        previous = end;
    }
    text.push_str(&source[previous..]);
    if text == source {
        return Ok(None);
    }
    let map = |index: usize| {
        let index = index.min(source_len);
        let mut delta = 0_isize;
        for fix in &kept {
            let range = &fix.correction.range;
            if index < range.start {
                break;
            }
            let added = fix.replacement.chars().count();
            if !range.is_empty() {
                let start = (range.start as isize + delta) as usize;
                if index == range.start {
                    return start;
                }
                if index < range.end {
                    return start + added;
                }
            }
            delta += added as isize - range.len() as isize;
        }
        (index as isize + delta) as usize
    };
    Ok(Some(PreparedFixes {
        text,
        cursor: CCursorRange {
            primary: CCursor {
                index: CCursor::new(map(cursor.primary.index.0)).index,
                ..cursor.primary
            },
            secondary: CCursor {
                index: CCursor::new(map(cursor.secondary.index.0)).index,
                ..cursor.secondary
            },
            h_pos: cursor.h_pos,
        },
        count: kept.len(),
    }))
}

impl EditorApp {
    pub(super) fn add_diagnostic_word(
        &mut self,
        diagnostic: &Diagnostic,
        context: &egui::Context,
    ) -> Result<(), String> {
        if self.tabs.is_empty() || !self.diagnostic_targets_current_document(diagnostic) {
            return Err("Open the source document and request fresh suggestions.".into());
        }
        let word = dictionary_word(diagnostic)
            .ok_or("Only a single Harper spelling word can be added to the dictionary.")?;
        let correction = diagnostic.correction.as_ref().unwrap();
        let bytes =
            correction.checked_byte_range(self.document().source(), self.document().key())?;
        let part_of_word = |character: char| character.is_alphanumeric() || character == '_';
        if self.document().source()[..bytes.start]
            .chars()
            .next_back()
            .is_some_and(part_of_word)
            || self.document().source()[bytes.end..]
                .chars()
                .next()
                .is_some_and(part_of_word)
        {
            return Err(
                "This suggestion is part of a larger word; request fresh suggestions.".into(),
            );
        }
        let mut settings = self.settings_snapshot();
        crate::writing::validate_dictionary(&settings.harper_dictionary)?;
        if settings
            .harper_dictionary
            .lines()
            .any(|entry| entry.trim() == word)
        {
            return Ok(());
        }
        if !settings.harper_dictionary.is_empty() && !settings.harper_dictionary.ends_with('\n') {
            settings.harper_dictionary.push('\n');
        }
        settings.harper_dictionary.push_str(word);
        crate::writing::validate_dictionary(&settings.harper_dictionary)?;
        self.queue_settings(settings, context);
        Ok(())
    }

    pub(super) fn apply_automatic_diagnostic_fixes(
        &mut self,
        diagnostics: &[Diagnostic],
        context: &egui::Context,
    ) -> Result<usize, String> {
        if self.reject_read_only_edit(context) {
            return Err("The source is read-only; corrections were not applied.".into());
        }
        if self.tabs.is_empty() || !self.document().kind().is_editable() {
            return Err("Open the source document and request fresh suggestions.".into());
        }
        let key = self.document().key();
        let mut ownership = std::collections::HashMap::new();
        let fixes = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                let correction = diagnostic.correction.as_ref()?;
                if !correction.is_current(key) {
                    return None;
                }
                let index = automatic_index(correction)?;
                let targets_source = *ownership
                    .entry(&diagnostic.source)
                    .or_insert_with(|| self.diagnostic_targets_current_document(diagnostic));
                targets_source.then_some(AutomaticFix {
                    correction,
                    replacement: &correction.replacements[index],
                })
            })
            .collect();
        let cursor = self.editor_snapshot(context).cursor;
        let Some(prepared) = prepare_automatic_fixes(self.document().source(), fixes, cursor)?
        else {
            return Ok(0);
        };
        self.document_mut()
            .edit(cursor, |source| *source = prepared.text);
        self.store_editor_cursor(context, prepared.cursor);
        self.document_mut().set_history_reset(false);
        self.pending_editor_selection = None;
        self.find_bar.search.clear();
        self.mark_edited();
        Ok(prepared.count)
    }

    pub(super) fn apply_diagnostic_correction(
        &mut self,
        diagnostic: &Diagnostic,
        index: usize,
        context: &egui::Context,
    ) -> Result<(), String> {
        if self.reject_read_only_edit(context) {
            return Err(if self.overleaf_source_locked() {
                "Wait for Overleaf to finish syncing before applying corrections."
            } else {
                "Close the active table editor or review before applying corrections."
            }
            .into());
        }
        if self.tabs.is_empty()
            || !self.document().kind().is_editable()
            || !self.diagnostic_targets_current_document(diagnostic)
        {
            return Err("Open the source document and request fresh suggestions.".into());
        }
        let correction = diagnostic
            .correction
            .as_ref()
            .ok_or("This diagnostic has no suggested correction.")?;
        let replacement = correction
            .replacements
            .get(index)
            .ok_or("This correction is no longer available.")?;
        let bytes =
            correction.checked_byte_range(self.document().source(), self.document().key())?;
        let cursor = CCursorRange::two(
            CCursor::new(correction.range.start),
            CCursor::new(correction.range.end),
        );
        self.document_mut().edit(cursor, |source| {
            source.replace_range(bytes, replacement);
        });
        let end = correction.range.start + replacement.chars().count();
        self.pending_editor_selection = Some(EditorSelection::Focus(correction.range.start..end));
        self.find_bar.search.clear();
        self.mark_edited();
        self.focus_source_editor(context);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    fn diagnostic(app: &EditorApp, range: Range<usize>, replacement: &str) -> Diagnostic {
        let original = app
            .document()
            .source()
            .chars()
            .skip(range.start)
            .take(range.len())
            .collect();
        Diagnostic {
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: Some(crate::diagnostics::DiagnosticLocation {
                line: 1,
                column: range.start + 1,
            }),
            message: "Spelling".into(),
            details: Vec::new(),
            correction: Some(DiagnosticCorrection {
                range,
                original,
                rule: Some("SpellCheck".into()),
                category: Some("Spelling".into()),
                replacements: vec![replacement.into()],
                document: Some(app.document().key()),
            }),
        }
    }

    #[test]
    fn corrections_replace_unicode_scalar_span_in_one_undo_transaction() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("é 😀 teh result.");
        let correction = diagnostic(&app, 4..7, "the");
        let before = app.document().key();
        app.apply_diagnostic_correction(&correction, 0, &context)
            .unwrap();
        assert_eq!(app.document().source(), "é 😀 the result.");
        assert_ne!(app.document().key(), before);
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
        assert_eq!(app.document().source(), "é 😀 the result.");
        let restored = app
            .document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(restored.cursor.primary.index.0, 7);
        assert_eq!(restored.cursor.secondary.index.0, 4);
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
        assert_eq!(app.document().source(), "é 😀 teh result.");
    }

    #[test]
    fn stale_or_other_document_suggestions_never_edit_even_matching_text() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("teh result.");
        let correction = diagnostic(&app, 0..3, "the");
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push(' '));
        let before = app.document().snapshot();
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
        assert_eq!(app.document().source(), before.source());
        assert_eq!(app.document().key(), before.key());
        app.document_mut()
            .replace_unprojected_untitled("teh result.");
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
    }

    #[test]
    fn correction_controls_select_replacements_and_disable_stale_actions() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(5), 1, 1);
        let mut correction = DiagnosticCorrection::new(
            0..3,
            "teh".into(),
            Some("SpellCheck".into()),
            None,
            vec!["the".into(), String::new()],
        );
        correction.document = Some(key);
        let mut harness = Harness::builder().build_ui_state(
            |ui, state: &mut (DiagnosticCorrection, Option<DocumentKey>, Option<usize>)| {
                if let Some(index) = show_corrections(ui, &state.0, state.1) {
                    state.2 = Some(index);
                }
            },
            (correction, Some(key), None),
        );
        harness.get_by_label("Replace with “the”").click();
        harness.run();
        assert_eq!(harness.state().2, Some(0));
        harness.state_mut().1 = Some(key.after_edit());
        harness.state_mut().2 = None;
        harness.run();
        assert!(
            harness
                .query_by_label("Waiting for refreshed suggestions")
                .is_some()
        );
        assert!(
            harness
                .get_by_label("Replace with “the”")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("Remove")
                .accesskit_node()
                .is_disabled()
        );
    }

    fn correction(range: Range<usize>, original: &str, replacement: &str) -> DiagnosticCorrection {
        DiagnosticCorrection::new(range, original.into(), None, None, vec![replacement.into()])
    }

    fn prepare(
        source: &str,
        corrections: &[DiagnosticCorrection],
        cursor: CCursorRange,
    ) -> Result<Option<PreparedFixes>, String> {
        prepare_automatic_fixes(
            source,
            corrections
                .iter()
                .filter_map(|correction| {
                    automatic_index(correction).map(|index| AutomaticFix {
                        correction,
                        replacement: &correction.replacements[index],
                    })
                })
                .collect(),
            cursor,
        )
    }

    #[test]
    fn automatic_fixes_require_one_distinct_changed_outcome() {
        for (replacements, expected) in [
            (vec![], None),
            (vec!["teh"], None),
            (vec!["the"], Some(0)),
            (vec!["the", "the"], Some(0)),
            (vec!["the", "ten"], None),
            (vec!["teh", "the"], None),
            (vec!["the", "teh"], None),
            (vec![""], Some(0)),
        ] {
            let mut correction = correction(0..3, "teh", "the");
            correction.replacements = replacements.into_iter().map(str::to_owned).collect();
            assert_eq!(automatic_index(&correction), expected, "{correction:?}");
        }
    }

    #[test]
    fn overlapping_clusters_are_all_skipped_while_duplicate_providers_apply_once() {
        let fixes = [
            correction(0..3, "abc", "ABC"),
            correction(2..6, "cdef", "CDEF"),
            correction(0..3, "abc", "ABC"),
            correction(7..11, "ghij", "GH"),
            correction(7..11, "ghij", "GH"),
        ];
        let result = prepare(
            "abcdef ghij",
            &fixes,
            CCursorRange::two(CCursor::new(7), CCursor::new(11)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.text, "abcdef GH");
        assert_eq!(result.count, 1);
        assert_eq!(result.cursor.primary.index.0, 9);
        assert_eq!(result.cursor.secondary.index.0, 7);
        for fixes in [
            vec![correction(1..1, "", "X"), correction(1..1, "", "Y")],
            vec![
                correction(0..1, "a", "Z"),
                correction(1..1, "", "X"),
                correction(1..2, "b", "Q"),
            ],
        ] {
            assert!(
                prepare("ab", &fixes, CCursorRange::default())
                    .unwrap()
                    .is_none()
            );
        }
        let adjacent = [correction(0..1, "a", "Z"), correction(1..2, "b", "Q")];
        assert_eq!(
            prepare("ab", &adjacent, CCursorRange::default())
                .unwrap()
                .unwrap()
                .text,
            "ZQ"
        );
        let cancelling = [correction(0..1, "a", ""), correction(1..2, "b", "ab")];
        assert!(
            prepare("ab", &cancelling, CCursorRange::default())
                .unwrap()
                .is_none()
        );
        let duplicate_insert = [correction(1..1, "", "X"), correction(1..1, "", "X")];
        let inserted = prepare("ab", &duplicate_insert, CCursorRange::default())
            .unwrap()
            .unwrap();
        assert_eq!(inserted.count, 1);
        assert_eq!(inserted.text, "aXb");
    }

    #[test]
    fn generated_unicode_splices_preserve_scalar_positions_and_selection_direction() {
        let chars = "a😀éz".chars().collect::<Vec<_>>();
        for start in 0..=chars.len() {
            for end in start..=chars.len() {
                let original = chars[start..end].iter().collect::<String>();
                for replacement in ["", "文", "Ω😀"] {
                    if replacement == original {
                        continue;
                    }
                    let fix = correction(start..end, &original, replacement);
                    let mut expected = chars.clone();
                    expected.splice(start..end, replacement.chars());
                    let expected = expected.into_iter().collect::<String>();
                    let map = |position| {
                        if position < start || (position == start && start != end) {
                            position
                        } else if position >= end {
                            position - (end - start) + replacement.chars().count()
                        } else {
                            start + replacement.chars().count()
                        }
                    };
                    for primary in 0..=chars.len() {
                        let secondary = chars.len() - primary;
                        let cursor = CCursorRange {
                            primary: CCursor {
                                prefer_next_row: true,
                                ..CCursor::new(primary)
                            },
                            secondary: CCursor::new(secondary),
                            h_pos: Some(12.0),
                        };
                        let result = prepare("a😀éz", std::slice::from_ref(&fix), cursor)
                            .unwrap()
                            .unwrap();
                        assert_eq!(result.text, expected);
                        assert_eq!(result.cursor.primary.index.0, map(primary));
                        assert_eq!(result.cursor.secondary.index.0, map(secondary));
                        assert_eq!(result.cursor.h_pos, cursor.h_pos);
                        assert_eq!(
                            result.cursor.primary.prefer_next_row,
                            cursor.primary.prefer_next_row
                        );
                        assert_eq!(
                            result.cursor.secondary.prefer_next_row,
                            cursor.secondary.prefer_next_row
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn batch_applies_current_active_file_only_and_preserves_selection_in_one_undo() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let original = "é 😀 teh badd result.";
        app.document_mut().replace_unprojected_untitled(original);
        let cursor = CCursorRange {
            primary: CCursor::new(original.chars().count()),
            secondary: CCursor::new(2),
            h_pos: Some(8.0),
        };
        app.store_editor_cursor(&context, cursor);
        let first = diagnostic(&app, 4..7, "the");
        let mut duplicate = first.clone();
        duplicate.provider = Some("LTeX+".into());
        let second = diagnostic(&app, 8..12, "bad");
        let mut stale = diagnostic(&app, 13..19, "wrong");
        stale.correction.as_mut().unwrap().document = Some(app.document().key().after_edit());
        let mut foreign = diagnostic(&app, 13..19, "wrong");
        foreign.source = DiagnosticSource::File(root.path().join("another.typ"));
        let other_focus = egui::Id::new("problems-control");
        context.memory_mut(|memory| memory.request_focus(other_focus));
        let focus = context.memory(|memory| memory.focused());
        assert_eq!(
            app.apply_automatic_diagnostic_fixes(
                &[first, duplicate, second, stale, foreign],
                &context
            )
            .unwrap(),
            2
        );
        assert_eq!(app.document().source(), "é 😀 the bad result.");
        let mapped = app.editor_snapshot(&context).cursor;
        assert_eq!(mapped.primary.index.0, cursor.primary.index.0 - 1);
        assert_eq!(mapped.secondary.index, cursor.secondary.index);
        assert_eq!(mapped.h_pos, cursor.h_pos);
        assert_eq!(context.memory(|memory| memory.focused()), focus);
        let restored = app.document_mut().history_step(false, mapped).unwrap();
        assert_eq!(restored.cursor, cursor);
        assert_eq!(app.document().source(), original);
        assert!(app.document_mut().history_step(false, cursor).is_none());
        let redone = app.document_mut().history_step(true, cursor).unwrap();
        assert_eq!(redone.cursor, mapped);
        assert_eq!(app.document().source(), "é 😀 the bad result.");
    }

    #[test]
    fn invalid_batch_is_atomic_and_noop_or_conflicting_batches_do_not_make_history() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_unprojected_untitled("teh badd");
        let first = diagnostic(&app, 0..3, "the");
        for invalid in [
            correction(4..8, "WRONG", "bad"),
            correction(4..90, "badd", "bad"),
            correction(Range { start: 7, end: 4 }, "badd", "bad"),
        ] {
            let mut bad = diagnostic(&app, 4..8, "bad");
            bad.correction = Some(DiagnosticCorrection {
                document: Some(app.document().key()),
                ..invalid
            });
            let before = app.document().key();
            assert!(
                app.apply_automatic_diagnostic_fixes(&[first.clone(), bad], &context)
                    .is_err()
            );
            assert_eq!(app.document().key(), before);
            assert_eq!(app.document().source(), "teh badd");
        }
        let no_op = diagnostic(&app, 0..3, "teh");
        let conflict = diagnostic(&app, 0..3, "ten");
        let before = app.document().key();
        assert_eq!(
            app.apply_automatic_diagnostic_fixes(&[no_op], &context)
                .unwrap(),
            0
        );
        assert_eq!(
            app.apply_automatic_diagnostic_fixes(&[first, conflict], &context)
                .unwrap(),
            0
        );
        assert_eq!(app.document().key(), before);
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
    }

    #[test]
    fn dictionary_action_is_bounded_harper_spelling_only_and_revalidates_on_click() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("Ézulverton’s result.");
        let word = "Ézulverton’s";
        let finding = diagnostic(&app, 0..word.chars().count(), "alternative");
        assert_eq!(dictionary_word(&finding), Some(word));
        for invalid in [
            "two words",
            "\\command",
            "#code",
            "x1",
            "-word",
            "word-",
            "two--words",
            "",
        ] {
            let mut rejected = finding.clone();
            rejected.correction.as_mut().unwrap().original = invalid.into();
            assert!(dictionary_word(&rejected).is_none(), "{invalid}");
        }
        let mut long = finding.clone();
        long.correction.as_mut().unwrap().original = "é".repeat(129);
        assert!(dictionary_word(&long).is_none());
        let mut wrong_provider = finding.clone();
        wrong_provider.provider = Some("LTeX+".into());
        assert!(dictionary_word(&wrong_provider).is_none());
        let mut wrong_rule = finding.clone();
        wrong_rule.correction.as_mut().unwrap().rule = Some("RepeatedWords".into());
        assert!(dictionary_word(&wrong_rule).is_none());
        let mut wrong_text = finding.clone();
        wrong_text.correction.as_mut().unwrap().original = "Zulverton".into();
        assert!(app.add_diagnostic_word(&wrong_text, &context).is_err());
        let mut fragment = finding.clone();
        fragment.correction.as_mut().unwrap().range = 0..2;
        fragment.correction.as_mut().unwrap().original = "Éz".into();
        assert!(app.add_diagnostic_word(&fragment, &context).is_err());
        let mut outside = finding.clone();
        outside.source = DiagnosticSource::Global;
        assert!(app.add_diagnostic_word(&outside, &context).is_err());
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push(' '));
        assert!(app.add_diagnostic_word(&finding, &context).is_err());
        assert!(app.settings_snapshot().harper_dictionary.is_empty());
    }

    #[test]
    fn dictionary_action_preserves_pending_settings_deduplicates_and_is_allowed_read_only() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("qzorthal #table([A])");
        let finding = diagnostic(&app, 0..8, "orthal");
        let mut settings = app.settings_snapshot();
        settings.auto_save = !settings.auto_save;
        let auto_save = settings.auto_save;
        settings.harper_dictionary = " Existing \nExisting\n".into();
        app.queue_settings(settings, &context);
        let table = crate::editor_features::editable_table_at(app.document().source(), 12).unwrap();
        app.begin_table_editor(table);
        assert!(app.source_edit_blocked());
        let key = app.document().key();
        let cursor = app.editor_snapshot(&context).cursor;
        assert!(
            app.apply_automatic_diagnostic_fixes(std::slice::from_ref(&finding), &context)
                .is_err()
        );
        app.add_diagnostic_word(&finding, &context).unwrap();
        app.add_diagnostic_word(&finding, &context).unwrap();
        assert_eq!(
            app.settings_snapshot().harper_dictionary,
            " Existing \nExisting\nqzorthal"
        );
        assert_eq!(app.settings_snapshot().auto_save, auto_save);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.editor_snapshot(&context).cursor, cursor);
        assert!(app.table_editor.is_some());
    }
}
