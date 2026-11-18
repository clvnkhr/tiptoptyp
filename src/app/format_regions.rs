//! Preserve comment-controlled source while accepting formatter changes elsewhere.
//! Whole-document replies are aligned with the same bounded line diff as Git.
use crate::{
    comment_controls::{Feature, Regions, Settings},
    document::DocumentKind,
    git::editor::document_diff::align_line_slices,
};
use std::{borrow::Cow, ops::Range};
use tiptoptyp_core::text::{
    AppliedTextEdits, LspPosition, LspRange, LspTextEdit, ScalarOffset, apply_text_edits,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINES: usize = 50_000;
const LIMIT_ERROR: &str =
    "Formatting comment controls support up to 8 MiB and 50,000 lines per document";

impl super::EditorApp {
    /// Guard canonical source before translating a formatter reply back to the
    /// editor's optional TeX-math representation.
    pub(super) fn prepare_protected_format_edits(
        &self,
        edits: &[LspTextEdit],
        cursors: [ScalarOffset; 2],
    ) -> Result<AppliedTextEdits, String> {
        let snapshot = self
            .document()
            .canonical_snapshot()
            .map_err(|error| error.to_string())?;
        let edits = protect(
            snapshot.source(),
            self.document().kind(),
            &self.settings.comment_controls,
            edits,
        )?;
        self.document()
            .prepare_canonical_edits(self.document().key(), &edits, cursors)
            .map_err(|error| error.to_string())
    }
}

/// Validate the original reply before projecting its changes onto enabled lines.
/// The common marker-free path borrows the provider's edit batch unchanged.
pub(super) fn protect<'a>(
    source: &str,
    kind: DocumentKind,
    settings: &Settings,
    edits: &'a [LspTextEdit],
) -> Result<Cow<'a, [LspTextEdit]>, String> {
    if !settings.occurs_in(source) {
        return Ok(Cow::Borrowed(edits));
    }
    check_size(source)?;
    if edits.len() > MAX_LINES
        || edits
            .iter()
            .try_fold(0usize, |size, edit| size.checked_add(edit.new_text.len()))
            .is_none_or(|size| size > MAX_BYTES)
    {
        return Err(LIMIT_ERROR.into());
    }
    let regions = Regions::parse(source, kind, settings, Feature::Formatting);
    if regions.is_empty() {
        return Ok(Cow::Borrowed(edits));
    }
    let formatted = apply_text_edits(source, edits, [ScalarOffset::new(0); 2])?.text;
    check_size(&formatted)?;
    if formatted == source {
        return Ok(Cow::Owned(Vec::new()));
    }
    let before: Vec<_> = source.split_inclusive('\n').collect();
    let after: Vec<_> = formatted.split_inclusive('\n').collect();
    let old_starts = line_starts(&before);
    let new_starts = line_starts(&after);
    let pairs = align_line_slices(&before, &after, |a, b| equivalent(a, b, kind));
    let mut changes = Vec::new();
    let (mut old, mut new, mut row) = (0, 0, 0);
    while row < pairs.len() {
        let (old_start, new_start) = (old, new);
        if pairs[row].changed {
            while row < pairs.len() && pairs[row].changed {
                old += usize::from(pairs[row].old.is_some());
                new += usize::from(pairs[row].new.is_some());
                row += 1;
            }
        } else {
            old += 1;
            new += 1;
            row += 1;
        }
        let range = old_starts[old_start]..old_starts[old];
        let replacement = &formatted[new_starts[new_start]..new_starts[new]];
        if let Some(change) = minimal_change(source, range, replacement)
            && !intersects_protection(&change, regions.protected_ranges(), source.len())
        {
            changes.push(change);
        }
    }
    Ok(Cow::Owned(lsp_edits(source, changes)))
}

fn check_size(source: &str) -> Result<(), String> {
    if source.len() > MAX_BYTES
        || source
            .bytes()
            .filter(|&byte| byte == b'\n')
            .take(MAX_LINES)
            .count()
            >= MAX_LINES
    {
        return Err(LIMIT_ERROR.into());
    }
    Ok(())
}

fn line_starts(lines: &[&str]) -> Vec<usize> {
    let mut offset = 0;
    std::iter::once(0)
        .chain(lines.iter().map(|line| {
            offset += line.len();
            offset
        }))
        .collect()
}

fn equivalent(a: &str, b: &str, kind: DocumentKind) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a == b {
        return true;
    }
    let prefix = if kind == DocumentKind::Tex { "%" } else { "//" };
    // Formatters may change indentation and whitespace inside a marker body.
    // Share the parser's matching rule; original protected bytes still win.
    a.strip_prefix(prefix)
        .zip(b.strip_prefix(prefix))
        .is_some_and(|(a, b)| crate::comment_controls::marker_matches(a, b))
}

struct Change {
    range: Range<usize>,
    text: String,
}

fn minimal_change(source: &str, range: Range<usize>, replacement: &str) -> Option<Change> {
    let original = &source[range.clone()];
    if original == replacement {
        return None;
    }
    let prefix: usize = original
        .chars()
        .zip(replacement.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum();
    let suffix: usize = original[prefix..]
        .chars()
        .rev()
        .zip(replacement[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum();
    let mut start = range.start + prefix;
    let mut end = range.end - suffix;
    let mut text = replacement[prefix..replacement.len() - suffix].to_owned();
    // LSP columns do not address the middle of a CRLF terminator. Widen an
    // edit that changes only CR to consume/reinsert the adjacent LF as well.
    if splits_crlf(source, start) {
        start -= 1;
        text.insert(0, '\r');
    }
    if splits_crlf(source, end) {
        end += 1;
        text.push('\n');
    }
    Some(Change {
        range: start..end,
        text,
    })
}

fn splits_crlf(source: &str, at: usize) -> bool {
    at > 0
        && source.as_bytes().get(at - 1) == Some(&b'\r')
        && source.as_bytes().get(at) == Some(&b'\n')
}

fn intersects_protection(change: &Change, protected: &[Range<usize>], length: usize) -> bool {
    if change.range.is_empty() {
        let at = change.range.start;
        let after = protected.partition_point(|range| range.start <= at);
        if after == 0 {
            return false;
        }
        let range = &protected[after - 1];
        return if at == range.start {
            // New complete lines before a marker leave it standalone. Added
            // indentation or token text on the marker itself must not land.
            !change.text.ends_with('\n')
        } else {
            at < range.end || (at == range.end && at == length)
        };
    }
    let after = protected.partition_point(|range| range.end <= change.range.start);
    protected
        .get(after)
        .is_some_and(|range| range.start < change.range.end)
}

fn lsp_edits(source: &str, changes: Vec<Change>) -> Vec<LspTextEdit> {
    let (mut scanned, mut line, mut column) = (0, 0, 0);
    let mut position = |to| {
        for ch in source[scanned..to].chars() {
            if ch == '\n' {
                line += 1;
                column = 0;
            } else {
                column += ch.len_utf16() as u32;
            }
        }
        scanned = to;
        LspPosition::new(line, column)
    };
    changes
        .into_iter()
        .map(|change| LspTextEdit {
            range: LspRange {
                start: position(change.range.start),
                end: position(change.range.end),
            },
            new_text: change.text,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comment_controls::Markers;
    use tiptoptyp_core::text::lsp_position_at_scalar;

    pub(super) fn whole(source: &str, formatted: &str) -> Vec<LspTextEdit> {
        vec![LspTextEdit {
            range: LspRange {
                start: LspPosition::new(0, 0),
                end: lsp_position_at_scalar(source, ScalarOffset::new(source.chars().count())),
            },
            new_text: formatted.into(),
        }]
    }

    fn apply(source: &str, formatted: &str, kind: DocumentKind, settings: Settings) -> String {
        let raw = whole(source, formatted);
        let edits = protect(source, kind, &settings, &raw).unwrap();
        apply_text_edits(source, &edits, [ScalarOffset::new(0); 2])
            .unwrap()
            .text
    }

    #[test]
    fn whole_document_replacements_format_both_sides_and_leave_disabled_bytes_exact() {
        for (kind, prefix) in [(DocumentKind::Typst, "//"), (DocumentKind::Tex, "%")] {
            let source = format!(
                "before  = 1\n{prefix}ttt: off\n  keep  😀 = 2\n{prefix}ttt: on\nafter  = 3\n"
            );
            let formatted = format!(
                "before = 1\n  {prefix} ttt: off\nkeep 😀 = 2\n  {prefix} ttt: on\nafter = 3\n"
            );
            let expected = source
                .replace("before  = 1", "before = 1")
                .replace("after  = 3", "after = 3");
            assert_eq!(
                apply(&source, &formatted, kind, Settings::default()),
                expected
            );
        }
    }

    #[test]
    fn marker_whitespace_changes_keep_protected_bytes_and_format_both_sides() {
        for (kind, prefix) in [(DocumentKind::Typst, "//"), (DocumentKind::Tex, "%")] {
            for (off, on, formatted_off, formatted_on) in [
                ("ttt: off", "ttt: on", "ttt:off", "ttt:on"),
                ("ttt:off", "ttt:on", "t t t :  o f f", "t t t :  o n"),
                (
                    "t\u{2003}tt:\u{a0}o\tf f",
                    "t\ttt: o\u{2002}n",
                    "ttt:off",
                    "ttt:on",
                ),
            ] {
                let source = format!(
                    "before  = 1\n{prefix}{off}\n  keep  😀 = 2\n{prefix}{on}\nafter  = 3\n"
                );
                let formatted = format!(
                    "before = 1\n  {prefix} {formatted_off}\nkeep 😀 = 2\n  {prefix} {formatted_on}\nafter = 3\n"
                );
                let expected = source
                    .replace("before  = 1", "before = 1")
                    .replace("after  = 3", "after = 3");
                assert_eq!(
                    apply(&source, &formatted, kind, Settings::default()),
                    expected,
                    "{kind:?}: {off:?} / {on:?}"
                );
            }
        }
    }

    #[test]
    fn custom_unicode_markers_crlf_and_unmatched_off_keep_their_original_bytes() {
        let markers = Markers {
            off: "整形しない".into(),
            on: "整形する".into(),
        };
        let source = "outside  text\r\n% 整形しない\r\n keep  😀\r\n";
        let formatted = "outside text\n% 整形しない\nkeep 😀\n";
        assert_eq!(
            apply(
                source,
                formatted,
                DocumentKind::Tex,
                Settings {
                    both: markers,
                    ..Settings::default()
                }
            ),
            "outside text\n% 整形しない\r\n keep  😀\r\n"
        );
        let source = "%ttt: off\n keep";
        assert_eq!(
            apply(
                source,
                "% ttt: off\nkeep\n",
                DocumentKind::Tex,
                Settings::default()
            ),
            source
        );
    }

    #[test]
    fn markers_inside_literals_do_not_protect_formatter_changes() {
        let source = "```typ\n//ttt: off\n```\n#let x=(1,2)\n";
        let raw = whole(source, "```typ\n//ttt: off\n```\n#let x = (1, 2)\n");
        assert!(matches!(
            protect(source, DocumentKind::Typst, &Settings::default(), &raw).unwrap(),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn invalid_provider_edits_are_not_hidden_by_the_protected_region() {
        let source = "%ttt: off\n😀\n%ttt: on\n";
        let invalid = [LspTextEdit {
            range: LspRange {
                start: LspPosition::new(1, 1),
                end: LspPosition::new(1, 2),
            },
            new_text: "x".into(),
        }];
        assert!(protect(source, DocumentKind::Tex, &Settings::default(), &invalid).is_err());
    }

    #[test]
    fn protected_cursor_maps_through_enabled_changes_and_multiple_blocks() {
        let source = "before  x\n//ttt: off\n keep 😀\n//ttt: on\nnext  y\n//ttt: off\n keep z\n//ttt: on\nafter  q\n";
        let formatted = source.replace("  ", " ").replace(" keep", "keep");
        let raw = whole(source, &formatted);
        let edits = protect(source, DocumentKind::Typst, &Settings::default(), &raw).unwrap();
        let byte = source.find('😀').unwrap();
        let at = ScalarOffset::new(source[..byte].chars().count());
        let applied = apply_text_edits(source, &edits, [at; 2]).unwrap();
        let expected = source
            .replace("before  x", "before x")
            .replace("next  y", "next y")
            .replace("after  q", "after q");
        assert_eq!(applied.text, expected);
        assert_eq!(
            applied.mapped_offsets[0].get(),
            expected[..expected.find('😀').unwrap()].chars().count()
        );
    }

    #[test]
    fn ambiguous_large_replacements_are_bounded_and_cannot_delete_a_marker() {
        let source = format!("{}//ttt: off\n keep\n//ttt: on\n", "old\n".repeat(10_000));
        let formatted = "new\n".repeat(10_000);
        assert_eq!(
            apply(
                &source,
                &formatted,
                DocumentKind::Typst,
                Settings::default()
            ),
            source
        );
    }

    #[test]
    fn feature_specific_markers_and_flat_both_overrides_do_not_block_other_formatting() {
        let source = "//diag: off\nformat  diagnostics-hidden\n//diag: on\n//ttt: off\n keep  both\n//fmt: on\nformat  resumed\n//fmt: off\n keep  formatting\n//ttt: on\nformat  end\n";
        let formatted = source.replace("  ", " ").replace(" keep", "keep");
        assert_eq!(
            apply(source, &formatted, DocumentKind::Typst, Settings::default()),
            source
                .replace("format  diagnostics-hidden", "format diagnostics-hidden")
                .replace("format  resumed", "format resumed")
                .replace("format  end", "format end")
        );
    }

    #[test]
    fn work_limits_precede_parsing_and_line_materialization_without_affecting_unmarked_source() {
        let marked = format!("//fmt: off\n{}", "line\n".repeat(MAX_LINES));
        assert!(protect(&marked, DocumentKind::Typst, &Settings::default(), &[]).is_err());
        let unmarked = "line\n".repeat(MAX_LINES);
        assert!(matches!(
            protect(&unmarked, DocumentKind::Typst, &Settings::default(), &[]).unwrap(),
            Cow::Borrowed(_)
        ));
        let marked = "//fmt: off\nkeep\n//fmt: on\n";
        let reply = whole(marked, &"x".repeat(MAX_BYTES + 1));
        assert!(protect(marked, DocumentKind::Typst, &Settings::default(), &reply).is_err());
    }
}

#[cfg(test)]
mod app_tests {
    use super::super::*;
    use super::tests::whole;

    fn fixture(kind: DocumentKind, source: &str) -> (tempfile::TempDir, EditorApp, egui::Context) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let path = root.join(if kind == DocumentKind::Tex {
            "main.tex"
        } else {
            "main.typ"
        });
        fs::write(&path, source).unwrap();
        let mut app = EditorApp::new_session(
            &context,
            egui::ViewportId::ROOT,
            Some(path),
            CaptureController::disabled_for_tests(),
            None,
            None,
            AppSettings {
                auto_save: false,
                ..AppSettings::default()
            },
            EditorWindowHost::Root,
            DocumentLifecycle::Dormant,
        );
        assert_eq!(app.document().kind(), kind);
        assert_eq!(
            app.document().disk_fingerprint(),
            Some(fingerprint(source.as_bytes()))
        );
        app.document_mut().set_history_reset(false);
        (directory, app, context)
    }

    fn arm(app: &mut EditorApp) -> (Generation, String, DocumentKey) {
        let key = app.document().key();
        let path = app.document().path().clone().unwrap();
        let uri = url::Url::from_file_path(path).unwrap().to_string();
        let generation = if app.document().kind() == DocumentKind::Tex {
            let settings = crate::tex::settings::TexSettings {
                texlab_enabled: false,
                lint: false,
                formatter: crate::tex::settings::Formatter::Disabled,
                ..Default::default()
            };
            app.tex_service.synchronize(
                crate::tex::Snapshot {
                    generation: Generation(0),
                    key,
                    uri: uri.clone(),
                    source: Arc::from(app.document().source().as_str()),
                    root: app.workspace_root.clone(),
                    tools: app.tex_tools.clone(),
                    settings,
                },
                crate::worker::RepaintTarget::test(),
            );
            app.tex_service.identity().unwrap().generation
        } else {
            app.tinymist_sync
                .fixture_session(Generation(73), Some(uri.clone()), None);
            Generation(73)
        };
        app.formatting.request(key, formatting::Intent::Edit);
        (generation, uri, key)
    }

    #[test]
    fn an_older_formatting_reply_cannot_retire_a_same_document_replacement() {
        for kind in [DocumentKind::Tex, DocumentKind::Typst] {
            let source = "Already formatted.\n";
            let (_directory, mut app, context) = fixture(kind, source);
            let (generation, uri, key) = arm(&mut app);
            let old_token = app.formatting.request_token().unwrap();
            let current_token = app.formatting.request(key, formatting::Intent::Save);
            let current = app.formatting;
            app.notice = None;
            app.receive_formatting_failure(
                generation,
                &uri,
                revision_as_i32(key.revision),
                old_token,
                "old request failed".into(),
            );
            app.receive_formatted_document(
                &context,
                generation,
                &uri,
                revision_as_i32(key.revision),
                old_token,
                Vec::new(),
            );
            assert_eq!(app.formatting, current);
            assert!(app.activity.format_error.is_none());
            assert!(app.notice.is_none());
            assert_eq!(app.document().source(), source);
            app.receive_formatting_failure(
                generation,
                &uri,
                revision_as_i32(key.revision),
                current_token,
                "current request failed".into(),
            );
            assert_eq!(app.formatting, formatting::State::Idle);
            assert_eq!(
                app.activity.format_error.as_deref(),
                Some("current request failed")
            );
            let token = app.formatting.request(key, formatting::Intent::Edit);
            app.receive_formatted_document(
                &context,
                generation,
                &uri,
                revision_as_i32(key.revision),
                current_token,
                Vec::new(),
            );
            assert_eq!(app.formatting.request_token(), Some(token));
            assert_eq!(
                app.notice.as_ref().unwrap().message,
                "Formatting failed: current request failed"
            );
        }
    }

    #[test]
    fn formatting_failures_require_the_current_source_and_service_identity() {
        for kind in [DocumentKind::Tex, DocumentKind::Typst] {
            let (_directory, mut app, _context) = fixture(kind, "Original source.\n");
            let (old_generation, old_uri, old_key) = arm(&mut app);
            let old_token = app.formatting.request_token().unwrap();
            app.document_mut().edit(CCursorRange::default(), |text| {
                text.push_str("New source.\n")
            });
            let (generation, uri, key) = arm(&mut app);
            let token = app.formatting.request_token().unwrap();
            let current = app.formatting;
            app.notice = None;
            for (generation, uri, version, token) in [
                (
                    old_generation,
                    old_uri,
                    revision_as_i32(old_key.revision),
                    old_token,
                ),
                (
                    Generation(generation.0 + 1),
                    uri.clone(),
                    revision_as_i32(key.revision),
                    token,
                ),
                (
                    generation,
                    "file:///another-document.typ".into(),
                    revision_as_i32(key.revision),
                    token,
                ),
                (
                    generation,
                    uri.clone(),
                    revision_as_i32(key.revision) - 1,
                    token,
                ),
            ] {
                app.receive_formatting_failure(generation, &uri, version, token, "stale".into());
                assert_eq!(app.formatting, current);
                assert!(app.activity.format_error.is_none());
                assert!(app.notice.is_none());
            }
            app.receive_formatting_failure(
                generation,
                &uri,
                revision_as_i32(key.revision),
                token,
                "current".into(),
            );
            assert_eq!(app.formatting, formatting::State::Idle);
            assert_eq!(app.activity.format_error.as_deref(), Some("current"));
        }
    }

    #[test]
    fn successful_null_or_empty_formatting_is_a_noop_for_tex_and_typst() {
        for kind in [DocumentKind::Tex, DocumentKind::Typst] {
            for wire in [serde_json::Value::Null, serde_json::json!([])] {
                for intent in [formatting::Intent::Edit, formatting::Intent::Save] {
                    let source = "Already formatted α🙂.\n";
                    let (_directory, mut app, context) = fixture(kind, source);
                    let (generation, uri, key) = arm(&mut app);
                    app.formatting.request(key, intent);
                    let cursor = CCursorRange::one(CCursor::new(6));
                    app.store_editor_cursor(&context, cursor);
                    let edits =
                        crate::lsp::protocol::parse_format_document_result(wire.clone()).unwrap();
                    app.receive_formatted_document(
                        &context,
                        generation,
                        &uri,
                        revision_as_i32(key.revision),
                        app.formatting.request_token().unwrap_or(0),
                        edits,
                    );
                    assert_eq!(app.document().key(), key);
                    assert_eq!(app.document().source(), source);
                    assert_eq!(app.editor_snapshot(&context).cursor, cursor);
                    assert_eq!(app.formatting, formatting::State::Idle);
                    assert!(app.activity.format_error.is_none());
                    assert!(app.local_save.pending().is_none());
                    assert!(!app.document_workflow.save_in_flight());
                    let notice = app.notice.as_ref().unwrap();
                    assert_eq!(notice.kind, NoticeKind::Success);
                    assert_eq!(notice.message, "No formatting changes");
                }
            }
        }
    }

    #[test]
    fn typst_and_tex_replies_preserve_regions_and_cursors_with_one_undo_and_stale_rejection() {
        for (kind, prefix) in [(DocumentKind::Typst, "//"), (DocumentKind::Tex, "%")] {
            let source =
                format!("before  x\n{prefix}ttt: off\n keep  😀\n{prefix}ttt: on\nafter  y\n");
            let formatted = source.replace("  ", " ").replace(" keep", "keep");
            let expected = source
                .replace("before  x", "before x")
                .replace("after  y", "after y");
            let (_directory, mut app, context) = fixture(kind, &source);
            let cursor = source[..source.find('😀').unwrap()].chars().count();
            app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(cursor)));
            let (generation, uri, key) = arm(&mut app);
            let edits = whole(&source, &formatted);
            app.receive_formatted_document(
                &context,
                generation,
                &uri,
                revision_as_i32(key.revision) - 1,
                app.formatting.request_token().unwrap_or(0),
                edits.clone(),
            );
            assert_eq!(app.document().source(), &source);
            app.receive_formatted_document(
                &context,
                generation,
                &uri,
                revision_as_i32(key.revision),
                app.formatting.request_token().unwrap_or(0),
                edits.clone(),
            );
            assert_eq!(app.document().source(), &expected);
            assert_eq!(
                app.editor_snapshot(&context).cursor.primary.index.0,
                expected[..expected.find('😀').unwrap()].chars().count()
            );
            assert!(app.local_save.pending().is_none());
            let current = app.document().key();
            app.formatting.request(key, formatting::Intent::Edit);
            app.receive_formatted_document(
                &context,
                generation,
                &uri,
                revision_as_i32(key.revision),
                app.formatting.request_token().unwrap_or(0),
                edits,
            );
            assert_eq!(app.document().key(), current);
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .unwrap();
            assert_eq!(app.document().source(), &source);
        }
    }

    #[test]
    fn format_on_save_publishes_only_enabled_changes() {
        let source = "before  x\n//fmt: off\n keep  😀\n//fmt: on\nafter  y\n";
        let expected = source
            .replace("before  x", "before x")
            .replace("after  y", "after y");
        let (_directory, mut app, context) = fixture(DocumentKind::Typst, source);
        let (generation, uri, key) = arm(&mut app);
        app.formatting.request(key, formatting::Intent::Save);
        app.receive_formatted_document(
            &context,
            generation,
            &uri,
            revision_as_i32(key.revision),
            app.formatting.request_token().unwrap_or(0),
            whole(source, &source.replace("  ", " ")),
        );
        assert!(
            app.local_save.pending().is_some() && app.local_save.is_running(),
            "format-on-save must admit a real write: {:?}",
            app.notice
        );
        app.finish_save_for_test(&context);
        assert_eq!(app.document().source(), &expected);
        assert_eq!(
            fs::read_to_string(app.document().path().as_ref().unwrap()).unwrap(),
            expected
        );
        assert!(app.local_save.pending().is_none());
        assert!(app.activity.save_error.is_none());
        assert!(!app.document().is_dirty());
    }

    #[test]
    fn canonical_formatting_is_guarded_before_tex_math_projection() {
        let source = "#import \"@preview/mitex:0.2.7\": mi\n#let before=(1,2)\n//fmt: off\n#mi(`\\alpha+x`)\n//fmt: on\n#let after=(3,4)\n";
        let (_directory, mut app, _context) = fixture(DocumentKind::Typst, source);
        app.document_mut().enable(Default::default()).unwrap();
        let before = app.document().source().clone();
        let formatted = source
            .replace("before=(1,2)", "before = (1, 2)")
            .replace("after=(3,4)", "after = (3, 4)")
            .replace("\\alpha+x", "\\alpha + x");
        let applied = app
            .prepare_protected_format_edits(&whole(source, &formatted), [ScalarOffset::new(0); 2])
            .unwrap();
        app.document_mut()
            .edit(CCursorRange::default(), |text| *text = applied.text);
        assert_eq!(
            app.document().canonical_snapshot().unwrap().source(),
            source
                .replace("before=(1,2)", "before = (1, 2)")
                .replace("after=(3,4)", "after = (3, 4)")
        );
        assert!(app.document().source().contains("\\alpha+x"));
        app.document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(app.document().source(), &before);
        assert!(app.local_save.pending().is_none());
    }
}
