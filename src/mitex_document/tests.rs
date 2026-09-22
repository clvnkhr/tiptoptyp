use super::*;
use crate::mitex_projection::Error as TranslationError;

const ORIGINAL: &str = "#import \"@preview/mitex:0.2.7\": mi\n文 #mi(`\\alpha+😀`) end\n";

#[test]
fn native_tex_roundtrips_without_projection_and_keeps_undo_and_save_identity() {
    let source = "\\documentclass{article}\n\\begin{document}\n文 $\\alpha+😀$\n\\end{document}\n";
    let mut document = Document::<usize>::new(WindowSessionId::new(1), source, DocumentKind::Tex);
    assert_eq!(document.name(), "Untitled.tex");
    assert_eq!(
        document.enable(Config::default()).unwrap_err(),
        Error::NotTypst
    );
    assert!(document.config().is_none());
    assert_eq!(document.canonical_snapshot().unwrap().source(), source);
    let initial = document.key();
    let cursor = source.chars().count();
    document.edit(cursor, |text| text.push_str("% extra\n"));
    assert_ne!(document.key(), initial);
    assert!(document.is_dirty());
    document.history_step(false, cursor + 8).unwrap();
    assert_eq!(document.source(), source);
    document.history_step(true, cursor).unwrap();
    let edited = format!("{source}% extra\n");
    assert_eq!(document.source(), &edited);
    let request = document
        .prepare_save("paper.tex".into(), DocumentKind::Tex)
        .unwrap();
    assert_eq!(request.source(), edited);
    document.record_save(request.committed(17)).unwrap();
    assert!(!document.is_dirty());
    assert_eq!(document.kind(), DocumentKind::Tex);
    assert_eq!(document.canonical_snapshot().unwrap().source(), edited);
}

fn canonical_edit(
    source: &str,
    needle: &str,
    replacement: &str,
) -> tiptoptyp_core::text::LspTextEdit {
    use tiptoptyp_core::text::{LspRange, LspTextEdit, ScalarOffset, lsp_position_at_scalar};
    let byte = source.find(needle).unwrap();
    let start = source[..byte].chars().count();
    LspTextEdit {
        range: LspRange {
            start: lsp_position_at_scalar(source, ScalarOffset::new(start)),
            end: lsp_position_at_scalar(source, ScalarOffset::new(start + needle.chars().count())),
        },
        new_text: replacement.into(),
    }
}

#[test]
fn canonical_service_edits_are_projected_atomically_and_undoable() {
    use tiptoptyp_core::text::ScalarOffset;
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let key = document.key();
    let before = document.source().clone();
    let cursor = ScalarOffset::new(before.chars().count());
    let edits = [canonical_edit(ORIGINAL, " end", " longer end")];
    let applied = document
        .prepare_canonical_edits(key, &edits, [cursor; 2])
        .unwrap();
    assert_eq!(document.source(), &before, "preparation does not mutate");
    assert_eq!(applied.text, before.replace(" end", " longer end"));
    assert_eq!(
        applied.mapped_offsets,
        [ScalarOffset::new(cursor.get() + 7); 2]
    );
    document.edit(cursor.get(), |source| *source = applied.text);
    assert_eq!(
        document.canonical_snapshot().unwrap().source(),
        ORIGINAL.replace(" end", " longer end")
    );
    assert_eq!(
        document
            .prepare_canonical_edits(key, &edits, [cursor; 2])
            .unwrap_err(),
        Error::StaleSnapshot
    );
    document.history_step(false, 0).unwrap();
    assert_eq!(document.source(), &before);
    assert_eq!(document.canonical_snapshot().unwrap().source(), ORIGINAL);
    assert!(!document.is_dirty());
}

#[test]
fn canonical_service_edits_reject_hidden_native_invalid_and_overlapping_changes() {
    use tiptoptyp_core::text::ScalarOffset;
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let key = document.key();
    let before = document.source().clone();
    for edits in [
        vec![canonical_edit(ORIGINAL, "`\\alpha+😀`", "\"\\\\alpha+😀\"")],
        vec![canonical_edit(ORIGINAL, " end", " $native$ end")],
        vec![canonical_edit(ORIGINAL, " end", " #let x = ( end")],
        vec![
            canonical_edit(ORIGINAL, " end", " replacement"),
            canonical_edit(ORIGINAL, "end", "overlap"),
        ],
    ] {
        assert!(
            document
                .prepare_canonical_edits(key, &edits, [ScalarOffset::new(0); 2])
                .is_err()
        );
        assert_eq!(document.key(), key);
        assert_eq!(document.source(), &before);
        assert_eq!(document.history_availability(), (false, false));
        assert!(!document.is_dirty());
    }
}

#[test]
fn canonical_no_op_does_not_surface_an_implicit_import() {
    use tiptoptyp_core::text::ScalarOffset;
    let mut document = Document::<usize>::new(WindowSessionId::new(1), "", DocumentKind::Typst);
    document.enable(Config::default()).unwrap();
    document.edit(0, |source| source.push_str("$\\alpha$ tail"));
    let key = document.key();
    let cursor = ScalarOffset::new(document.source().chars().count());
    let applied = document
        .prepare_canonical_edits(key, &[], [cursor; 2])
        .unwrap();
    assert_eq!(applied.text, *document.source());
    assert_eq!(applied.mapped_offsets, [cursor; 2]);
    let source = document.canonical_snapshot().unwrap();
    let edits = [canonical_edit(source.source(), "tail", "next")];
    let applied = document
        .prepare_canonical_edits(key, &edits, [cursor; 2])
        .unwrap();
    document.edit(0, |source| *source = applied.text);
    assert_eq!(
        document.canonical_snapshot().unwrap().source(),
        source.source().replace("tail", "next")
    );
    document.history_step(false, 0).unwrap();
    assert_eq!(document.source(), "$\\alpha$ tail");
}

#[test]
fn rename_preflight_and_new_document_keep_receipt_boundaries() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let key = document.key();
    assert_eq!(
        document.rename("paper.txt".into(), DocumentKind::Text),
        Err(Error::NotTypst)
    );
    assert_eq!(document.key(), key);
    let old = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    document
        .rename("new.typ".into(), DocumentKind::Typst)
        .unwrap();
    assert!(document.record_save(old.committed(9)).is_err());
    assert!(!document.is_dirty());
    document.edit(0, |source| source.push('$'));
    document.replace_unprojected_untitled("$native math$");
    assert!(document.config().is_none());
    assert!(document.path().is_none());
    assert_eq!(
        document.canonical_snapshot().unwrap().source(),
        "$native math$"
    );
    assert!(!document.is_dirty());
}

fn loaded() -> Document<usize> {
    let mut document = Document::new(WindowSessionId::new(1), "", DocumentKind::Typst);
    document
        .replace_loaded(
            ORIGINAL.into(),
            "paper.typ".into(),
            DocumentKind::Typst,
            Some(42),
        )
        .unwrap();
    document
}

#[test]
fn ordinary_documents_use_the_editor_snapshot_without_translation() {
    let mut document = loaded();
    let key = document.editor().key();
    let snapshot = document.canonical_snapshot().unwrap();
    assert_eq!(snapshot.key(), key);
    assert_eq!(snapshot.source(), ORIGINAL);
    assert!(snapshot.translation.is_none());
    assert_eq!(snapshot.editor_to_canonical(0), Some(0));
    let unicode = ORIGINAL.find('文').unwrap();
    assert_eq!(snapshot.canonical_to_editor(unicode), Some(unicode));
    assert_eq!(snapshot.canonical_to_editor(unicode + 1), None);
    assert_eq!(snapshot.editor_to_canonical(ORIGINAL.len() + 1), None);
    document.edit(0, |source| source.push('!'));
    let request = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    assert!(request.source().ends_with('!'));
    document.record_save(request.committed(43)).unwrap();
    assert!(!document.is_dirty());
    assert_eq!(document.encodes.get(), 0);
}

#[test]
fn disabled_mode_edits_dirty_checks_and_saves_never_build_projection_state() {
    let mut document = loaded();
    for _ in 0..100 {
        document.edit(0, |source| source.push('x'));
        assert!(document.is_dirty());
        let request = document
            .prepare_save("paper.typ".into(), DocumentKind::Typst)
            .unwrap();
        assert!(request.canonical.is_none());
        document.record_save(request.committed(42)).unwrap();
        assert!(!document.is_dirty());
        assert!(document.canonical.borrow().is_none());
    }
    assert_eq!(document.encodes.get(), 0);
}

#[test]
fn removing_native_math_allows_enabling_and_revert_restores_native_source() {
    let mut document =
        Document::<usize>::new(WindowSessionId::new(1), "$ native $", DocumentKind::Typst);
    document.edit(0, String::clear);
    document.enable(Config::default()).unwrap();
    assert!(document.is_dirty());
    document.restore_saved_source().unwrap();
    assert_eq!(document.source(), "$ native $");
    assert!(document.config().is_none());
    assert!(!document.is_dirty());
}

#[test]
fn inserted_import_and_block_calls_map_gutter_lines_to_view() {
    let mut document = Document::<usize>::new(WindowSessionId::new(1), "", DocumentKind::Typst);
    document.enable(Config::default()).unwrap();
    document.edit(0, |source| *source = "$x$\n$\n  \\alpha\n$\n= End\n".into());
    let snapshot = document.canonical_snapshot().unwrap();
    assert_eq!(snapshot.editor_lines(0..1), 0..1);
    assert_eq!(snapshot.editor_lines(1..2), 0..1);
    assert_eq!(snapshot.editor_lines(2..5), 1..4);
    assert_eq!(snapshot.editor_lines(5..6), 4..5);
    assert_eq!(snapshot.editor_lines(5..5), 4..4);
}

#[test]
fn toggles_preserve_disk_identity_dirty_work_and_exact_spelling() {
    let mut document = loaded();
    document.edit(3, |source| source.push_str("Unsaved"));
    let canonical = document.editor().source().clone();
    let old = document.editor().key();
    let old_save = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    let map = document.enable(Config::default()).unwrap().unwrap();
    assert_eq!(map.input(), canonical);
    assert!(document.editor().source().contains("$\\alpha+😀$"));
    assert!(document.is_dirty());
    assert_eq!(document.editor().disk_fingerprint(), Some(42));
    assert_eq!(
        document.editor().path().as_deref(),
        Some(std::path::Path::new("paper.typ"))
    );
    assert_ne!(document.editor().key().epoch, old.epoch);
    assert_eq!(document.editor().history_availability(), (false, false));
    assert!(document.take_history_reset());
    assert!(!document.take_history_reset());
    assert!(document.take_edit().is_some());
    assert!(document.take_edit().is_none());
    assert!(document.record_save(old_save.committed(99)).is_err());
    document.disable().unwrap();
    assert_eq!(document.editor().source(), &canonical);
    assert_eq!(document.editor().saved_source(), ORIGINAL);
    assert!(document.is_dirty());
    assert_eq!(document.editor().disk_fingerprint(), Some(42));
}

#[test]
fn clean_toggle_and_repeated_enable_do_not_dirty_or_reset_history() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    assert!(!document.is_dirty());
    document.edit(0, |source| source.push('!'));
    let key = document.editor().key();
    assert!(document.enable(Config::default()).unwrap().is_none());
    assert_eq!(document.editor().key(), key);
    assert_eq!(document.editor().history_availability(), (true, false));
    document.history_step(false, 0).unwrap();
    assert!(!document.is_dirty());
    document.disable().unwrap();
    assert_eq!(document.editor().source(), ORIGINAL);
    assert!(!document.is_dirty());
    assert!(document.disable().unwrap().is_none());
}

#[test]
fn refusing_mode_or_reload_leaves_all_current_state_intact() {
    let mut document = loaded();
    document.edit(0, |source| source.push_str("$x$"));
    let key = document.editor().key();
    assert!(matches!(
        document.enable(Config::default()),
        Err(Error::Translation(TranslationError::NativeMath { .. }))
    ));
    assert_eq!(document.editor().key(), key);
    assert!(document.config().is_none());
    assert_eq!(document.editor().history_availability(), (true, false));
    document.history_step(false, 0).unwrap();
    document.enable(Config::default()).unwrap();
    document.edit(0, |source| source.push_str("pending"));
    let before = document.editor().snapshot();
    assert!(
        document
            .replace_loaded(
                "$native$".into(),
                "other.typ".into(),
                DocumentKind::Typst,
                Some(99)
            )
            .is_err()
    );
    assert_eq!(document.editor().key(), before.key());
    assert_eq!(document.editor().source(), before.source());
    assert_eq!(document.editor().disk_fingerprint(), Some(42));
    assert!(document.config().is_some());
    assert!(document.is_dirty());
}

#[test]
fn unfinished_input_is_editable_but_cannot_save_disable_or_supply_service_source() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let old = document.canonical_snapshot().unwrap();
    document.edit(0, |source| source.push_str("$unfinished"));
    let key = document.editor().key();
    for _ in 0..100 {
        assert!(document.canonical_snapshot().is_err());
        assert!(
            document
                .prepare_save("paper.typ".into(), DocumentKind::Typst)
                .is_err()
        );
        assert!(document.disable().is_err());
        assert!(document.is_dirty());
    }
    assert_eq!(document.encodes.get(), 2, "cache failed translations too");
    assert_eq!(document.editor().key(), key);
    assert_ne!(old.key(), key);
    assert!(document.config().is_some());
    assert_eq!(document.editor().disk_fingerprint(), Some(42));
    document.history_step(false, 0).unwrap();
    assert_eq!(document.canonical_snapshot().unwrap().source(), ORIGINAL);
    assert!(!document.is_dirty());
    document.history_step(true, 0).unwrap();
    assert!(document.canonical_snapshot().is_err());
    document.edit(0, |source| source.push('$'));
    assert!(
        document
            .canonical_snapshot()
            .unwrap()
            .source()
            .contains("#mi(\"unfinished\")")
    );
}

#[test]
fn save_as_writes_canonical_bytes_but_acknowledges_the_displayed_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new.typ");
    let mut document = Document::new(WindowSessionId::new(1), "", DocumentKind::Typst);
    document.enable(Config::default()).unwrap();
    document.edit(0, |source| source.push_str("Unicode $\\alpha+😀$"));
    let before = document.editor().key();
    let request = document
        .prepare_save(path.clone(), DocumentKind::Typst)
        .unwrap();
    let persisted = request.source().to_owned();
    std::fs::write(request.path(), request.source()).unwrap();
    document.record_save(request.committed(88)).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), persisted);
    assert_eq!(persisted.matches("#import").count(), 1);
    assert!(persisted.contains("#mi(\"\\\\alpha+😀\")"));
    assert_eq!(document.editor().source(), "Unicode $\\alpha+😀$");
    assert_eq!(document.editor().source(), document.editor().saved_source());
    assert!(!document.is_dirty());
    assert_ne!(document.editor().key().epoch, before.epoch);
    assert_eq!(document.editor().path(), &Some(path.clone()));
    assert_eq!(document.editor().disk_fingerprint(), Some(88));
    let request = document
        .prepare_save(path.clone(), DocumentKind::Typst)
        .unwrap();
    assert_eq!(request.source(), persisted);
    document
        .replace_loaded(persisted.clone(), path, DocumentKind::Typst, Some(88))
        .unwrap();
    assert!(!document.is_dirty());
    assert_eq!(document.canonical_snapshot().unwrap().source(), persisted);
    assert_eq!(document.editor().history_availability(), (false, false));
}

#[test]
fn in_flight_save_preserves_newer_edits_and_older_receipts_cannot_regress_disk_state() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let older = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    document.edit(0, |source| source.push_str(" new $x$"));
    let newer = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    let canonical = newer.source().to_owned();
    document.edit(0, |source| source.push_str(" newest"));
    assert_eq!(
        document.record_save(newer.committed(50)).unwrap(),
        SaveStatus::Applied
    );
    assert!(document.is_dirty());
    assert_eq!(
        document.record_save(older.committed(49)).unwrap(),
        SaveStatus::Stale
    );
    assert_eq!(document.editor().disk_fingerprint(), Some(50));
    document.history_step(false, 0).unwrap();
    assert_eq!(document.canonical_snapshot().unwrap().source(), canonical);
    assert!(!document.is_dirty());
    document.history_step(false, 0).unwrap();
    assert!(
        document.is_dirty(),
        "undoing past the saved revision is dirty"
    );
    document.restore_saved_source().unwrap();
    assert_eq!(document.canonical_snapshot().unwrap().source(), canonical);
    assert!(!document.is_dirty());
}

#[test]
fn identical_views_do_not_hide_unsaved_literal_spelling_changes() {
    let mut document = loaded();
    document.edit(0, |source| {
        *source = source.replace("`\\alpha+😀`", "\"\\\\alpha+😀\"")
    });
    let changed = document.editor().source().clone();
    assert_ne!(changed, ORIGINAL);
    document.enable(Config::default()).unwrap();
    assert_eq!(document.editor().source(), document.editor().saved_source());
    assert!(document.is_dirty(), "dirty state compares canonical bytes");
    assert_eq!(document.canonical_snapshot().unwrap().source(), changed);
    document.restore_saved_source().unwrap();
    assert_eq!(document.canonical_snapshot().unwrap().source(), ORIGINAL);
    assert!(!document.is_dirty());
}

#[test]
fn replacement_rejects_old_receipts_and_non_typst_opens_exit_the_mode() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let request = document
        .prepare_save("paper.typ".into(), DocumentKind::Typst)
        .unwrap();
    document
        .replace_loaded(
            "plain $text$".into(),
            "notes.txt".into(),
            DocumentKind::Text,
            Some(22),
        )
        .unwrap();
    assert!(document.config().is_none());
    assert_eq!(document.editor().source(), "plain $text$");
    assert!(!document.is_dirty());
    assert!(document.record_save(request.committed(33)).is_err());
    assert_eq!(document.editor().disk_fingerprint(), Some(22));
    assert_eq!(
        document.enable(Config::default()).unwrap_err(),
        Error::NotTypst
    );
    document.replace_untitled(String::new()).unwrap();
    assert!(document.editor().path().is_none());
    document.enable(Config::default()).unwrap();
    assert_eq!(
        document
            .prepare_save("notes.txt".into(), DocumentKind::Text)
            .unwrap_err(),
        Error::NotTypst
    );
}

#[test]
fn cache_is_revision_scoped_shared_and_survives_no_op_edits() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let first = document.canonical_snapshot().unwrap();
    for _ in 0..1000 {
        document.edit(0, |_| {});
        let next = document.canonical_snapshot().unwrap();
        assert_eq!(next.key(), first.key());
        assert!(Arc::ptr_eq(
            next.translation.as_ref().unwrap(),
            first.translation.as_ref().unwrap()
        ));
        assert!(!document.is_dirty());
    }
    assert_eq!(document.encodes.get(), 1);
    document.edit(0, |source| source.push('!'));
    assert_ne!(document.canonical_snapshot().unwrap().key(), first.key());
    assert_eq!(document.encodes.get(), 2);
    document.history_step(false, 0).unwrap();
    document.canonical_snapshot().unwrap();
    assert_eq!(document.encodes.get(), 3);
}

#[test]
fn edit_unwind_cannot_reuse_a_stale_translation() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let first = document.canonical_snapshot().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        document.edit(0, |source| {
            source.push_str("$unfinished");
            panic!("injected edit failure");
        });
    }));
    assert!(result.is_err());
    assert_ne!(first.key(), document.editor().key());
    assert!(document.canonical_snapshot().is_err());
    assert_eq!(document.encodes.get(), 2);
}

#[test]
fn snapshot_maps_stay_with_their_source_after_new_edits() {
    let mut document = loaded();
    document.enable(Config::default()).unwrap();
    let snapshot = document.canonical_snapshot().unwrap();
    let editor_byte = snapshot.editor_source().find('😀').unwrap();
    let canonical_byte = ORIGINAL.find('😀').unwrap();
    assert_eq!(
        snapshot.editor_to_canonical(editor_byte),
        Some(canonical_byte)
    );
    assert_eq!(
        snapshot.canonical_to_editor(canonical_byte),
        Some(editor_byte)
    );
    assert_eq!(snapshot.editor_to_canonical(editor_byte + 1), None);
    document.edit(0, |source| source.insert_str(0, "new prefix\n"));
    assert_ne!(document.editor().key(), snapshot.key());
    assert_eq!(
        snapshot.canonical_to_editor(canonical_byte),
        Some(editor_byte)
    );
    let current = document.canonical_snapshot().unwrap();
    assert_eq!(
        current.canonical_to_editor(canonical_byte + 11),
        Some(editor_byte + 11)
    );
}

#[test]
fn service_positions_use_the_correct_unicode_units_and_cached_line_indices() {
    use tiptoptyp_core::text::{LspPosition, LspRange, ScalarOffset};
    let mut document = Document::new(WindowSessionId::new(1), "", DocumentKind::Typst);
    document.enable(Config::default()).unwrap();
    document.edit(0, |source| source.push_str("文 $\\alpha+😀$ tail\r\nnext"));
    let snapshot = document.canonical_snapshot().unwrap();
    assert!(snapshot.coordinates.get().is_none());
    let editor_byte = snapshot.editor_source().find(" tail").unwrap();
    let canonical_byte = snapshot.source().find(" tail").unwrap();
    let editor_scalar = snapshot.editor_source()[..editor_byte].chars().count();
    let line_start = snapshot.source()[..canonical_byte].rfind('\n').unwrap() + 1;
    let expected = LspPosition::new(
        1,
        snapshot.source()[line_start..canonical_byte]
            .encode_utf16()
            .count() as u32,
    );
    let position = snapshot
        .canonical_lsp_position(ScalarOffset::new(editor_scalar))
        .unwrap();
    assert_eq!(
        position, expected,
        "generated import shifts the line, escaping shifts the column"
    );
    let (line, column) = snapshot
        .canonical_preview_position(ScalarOffset::new(editor_scalar))
        .unwrap();
    assert_eq!(line, position.line);
    assert_eq!(
        column.get() + 1,
        position.character.get(),
        "emoji takes two UTF-16 units, one scalar"
    );
    assert_eq!(
        snapshot.editor_range(LspRange {
            start: position,
            end: position
        }),
        Some(editor_scalar..editor_scalar)
    );
    let emoji_byte = snapshot.source().find('😀').unwrap();
    let emoji_col = snapshot.source()[line_start..emoji_byte]
        .encode_utf16()
        .count() as u32;
    let split = LspPosition::new(1, emoji_col + 1);
    assert_eq!(
        snapshot.editor_range(LspRange {
            start: split,
            end: split
        }),
        None
    );
    assert_eq!(
        snapshot.editor_range(LspRange {
            start: position,
            end: LspPosition::new(0, 0)
        }),
        None
    );
    assert_eq!(
        snapshot.canonical_lsp_position(ScalarOffset::new(usize::MAX)),
        None
    );
    let other = document.canonical_snapshot().unwrap();
    assert!(Arc::ptr_eq(&snapshot.coordinates, &other.coordinates));
    assert!(other.coordinates.get().is_some());
}

#[test]
#[ignore = "opt-in optimized translation cache microbenchmark; no wall-time assertion"]
fn disabled_projection_measurement() {
    use std::{hint::black_box, time::Instant};
    let source = "// Unicode 文 😀 representative source for projection caching\n".repeat(5000);
    let mut core = document::DocumentSession::<usize>::new(
        WindowSessionId::new(1),
        source.clone(),
        DocumentKind::Typst,
    );
    let mut disabled =
        Document::<usize>::new(WindowSessionId::new(1), source.clone(), DocumentKind::Typst);
    let toggle = |source: &mut String| {
        if source.ends_with('!') {
            source.pop();
        } else {
            source.push('!');
        }
    };
    for _ in 0..10 {
        core.edit(0, toggle);
        disabled.edit(0, toggle);
    }
    for round in 0..5 {
        let mut elapsed = [std::time::Duration::ZERO; 2];
        for which in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = Instant::now();
            for _ in 0..200 {
                if which == 0 {
                    core.edit(0, toggle);
                    black_box(core.is_dirty());
                    black_box(core.prepare_save("paper.typ".into(), DocumentKind::Typst));
                    core.clear_history();
                } else {
                    disabled.edit(0, toggle);
                    black_box(disabled.is_dirty());
                    black_box(
                        disabled
                            .prepare_save("paper.typ".into(), DocumentKind::Typst)
                            .unwrap(),
                    );
                    disabled.clear_history();
                }
            }
            elapsed[which] = start.elapsed();
        }
        eprintln!(
            "disabled_mode fixture_bytes={} warmup=10 iterations=200 round={round} core_us={} adapter_us={}",
            source.len(),
            elapsed[0].as_micros(),
            elapsed[1].as_micros()
        );
    }
    assert_eq!(disabled.encodes.get(), 0);
    assert!(disabled.canonical.borrow().is_none());
}

#[test]
#[ignore = "opt-in optimized translation cache microbenchmark; no wall-time assertion"]
fn projection_cache_measurement() {
    use std::{hint::black_box, time::Instant};
    use tiptoptyp_core::text::ScalarOffset;
    let source = format!(
        "{}{}",
        "// Unicode 文 😀 representative source for projection caching\n".repeat(5000),
        ORIGINAL
    );
    let projection = Projection::open(&source, Config::default()).unwrap();
    let mut document =
        Document::<usize>::new(WindowSessionId::new(1), source.clone(), DocumentKind::Typst);
    document.enable(Config::default()).unwrap();
    let view = document.editor().source().clone();
    let cursor = ScalarOffset::new(view.chars().count());
    for _ in 0..10 {
        black_box(projection.encode(black_box(&view)).unwrap());
        black_box(
            document
                .canonical_snapshot()
                .unwrap()
                .canonical_lsp_position(cursor),
        );
    }
    let iterations = 200;
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(projection.encode(black_box(&view)).unwrap());
    }
    let uncached = start.elapsed();
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(document.canonical_snapshot().unwrap());
    }
    let cached = start.elapsed();
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(
            document
                .canonical_snapshot()
                .unwrap()
                .canonical_lsp_position(cursor),
        );
    }
    let coordinates = start.elapsed();
    let start = Instant::now();
    for _ in 0..iterations {
        document.edit(0, |source| {
            if source.ends_with('!') {
                source.pop();
            } else {
                source.push('!');
            }
        });
        black_box(document.canonical_snapshot().unwrap());
    }
    let edits = start.elapsed();
    eprintln!(
        "fixture_bytes={} warmup=10 iterations={iterations} unchanged_uncached_us={} unchanged_cached_us={} cached_position_us={} alternating_edit_miss_us={} encodes={}",
        source.len(),
        uncached.as_micros(),
        cached.as_micros(),
        coordinates.as_micros(),
        edits.as_micros(),
        document.encodes.get()
    );
}
