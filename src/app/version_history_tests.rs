use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _, by},
};

fn key() -> DocumentKey {
    DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(42), 3, 5)
}

fn fixture() -> Dialog {
    let mut dialog = Dialog::new(
        key(),
        PathBuf::from("/paper.typ"),
        "current token\n".into(),
        false,
        PathBuf::from("/"),
        Some(crate::compiler::EngineConfig::Typst(
            crate::compiler::TypstOptions {
                command: crate::tool_command::CommandCustomization::for_tool(
                    crate::toolchain::ToolKind::Typst,
                )
                .into(),
                executable: "must-not-run".into(),
                font_paths: Vec::new(),
            },
        )),
    );
    dialog.inventory = Some(Inventory {
        versions: vec![
            VersionInfo {
                id: "newer-id".into(),
                saved_at: UNIX_EPOCH + Duration::from_secs(100),
            },
            VersionInfo {
                id: "older-id".into(),
                saved_at: UNIX_EPOCH + Duration::from_secs(50),
            },
        ],
        modified: Some(UNIX_EPOCH + Duration::from_secs(200)),
        initial: 0,
    });
    dialog.choices = [Choice::Saved(0), Choice::Current];
    let old: Arc<str> = "historical token\n".into();
    dialog.comparison = Some(Comparison {
        choices: dialog.choices,
        sources: [old.clone(), dialog.current.clone()],
        diff: DocumentDiff::new(old.to_string(), &dialog.current),
    });
    dialog
}

#[test]
fn comparison_order_always_places_the_older_saved_revision_on_the_left() {
    let all = [
        Choice::Current,
        Choice::Saved(0),
        Choice::Saved(1),
        Choice::Saved(8),
    ];
    for left in all {
        for right in all {
            let pair = ordered([left, right]);
            assert!(pair[0].rank() >= pair[1].rank(), "{left:?} {right:?}");
            assert_eq!(ordered(pair), pair);
            assert!(pair == [left, right] || pair == [right, left]);
        }
    }
    assert_eq!(
        ordered([Choice::Saved(0), Choice::Saved(8)]),
        [Choice::Saved(8), Choice::Saved(0)]
    );
    assert_eq!(
        ordered([Choice::Current, Choice::Saved(0)]),
        [Choice::Saved(0), Choice::Current]
    );
}

#[test]
fn saved_time_labels_are_unambiguous_across_leap_years_and_2038() {
    for (seconds, expected) in [
        (0, "1970-01-01 00:00:00 UTC"),
        (951_782_400, "2000-02-29 00:00:00 UTC"),
        (1_709_164_800, "2024-02-29 00:00:00 UTC"),
        (2_147_483_647, "2038-01-19 03:14:07 UTC"),
        (4_107_542_400, "2100-03-01 00:00:00 UTC"),
    ] {
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(seconds)),
            expected
        );
    }
    let mut dialog = fixture();
    assert_eq!(
        dialog.label(Choice::Saved(1)),
        "Saved · 1970-01-01 00:00:50 UTC"
    );
    assert_eq!(
        dialog.label(Choice::Current),
        "Current · 1970-01-01 00:03:20 UTC"
    );
    dialog.unsaved = true;
    assert_eq!(dialog.label(Choice::Current), "Current edits · Not saved");
}

fn harness(dialog: Dialog) -> Harness<'static, (Dialog, AppSettings, Option<Action>)> {
    harness_with_width(dialog, 980.0)
}

fn harness_with_width(
    dialog: Dialog,
    width: f32,
) -> Harness<'static, (Dialog, AppSettings, Option<Action>)> {
    let mut ready = false;
    Harness::builder()
        .with_size(Vec2::new(width, 520.0))
        .build_ui_state(
            move |ui, state: &mut (Dialog, AppSettings, Option<Action>)| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    ready = true;
                    return;
                }
                if let Some(action) = show(ui, &state.0, &state.1) {
                    state.2 = Some(action);
                }
            },
            (dialog, AppSettings::default(), None),
        )
}

#[test]
fn history_comparison_is_read_only_and_restore_targets_the_chosen_side() {
    let mut harness = harness(fixture());
    harness.run();
    assert_eq!(
        harness
            .query_all(by().label("Older version contents"))
            .count(),
        1
    );
    assert_eq!(
        harness
            .query_all(by().label("Newer version contents"))
            .count(),
        1
    );
    assert_eq!(
        harness
            .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
            .count(),
        0
    );
    let restores: Vec<_> = harness.get_all_by_label("Restore version").collect();
    assert_eq!(restores.len(), 2);
    assert!(!restores[0].accesskit_node().is_disabled());
    assert!(
        restores[1].accesskit_node().is_disabled(),
        "Current edits are already current"
    );
    restores[0].click();
    harness.run();
    assert_eq!(harness.state().2, Some(Action::Restore(0)));
    assert_eq!(harness.state().0.current.as_ref(), "current token\n");
    let comparison = harness.state().0.comparison.as_ref().unwrap();
    assert_eq!(comparison.sources[0].as_ref(), "historical token\n");
    assert_eq!(comparison.diff.before, "historical token\n");
    assert_eq!(comparison.diff.current, "current token\n");
    assert_eq!(comparison.diff.builds, 1);
    harness.run_steps(5);
    assert_eq!(
        harness.state().0.comparison.as_ref().unwrap().diff.builds,
        1,
        "Idle history paints do not recalculate the diff"
    );
}

#[test]
fn stale_comparison_cannot_be_mislabeled_rendered_or_restored_after_a_selection_change() {
    let mut dialog = fixture();
    dialog.choices = [Choice::Saved(1), Choice::Current];
    assert!(!dialog.loaded());
    let mut harness = harness(dialog);
    harness.run();
    for label in ["Preview PDF", "Restore version"] {
        let buttons: Vec<_> = harness.get_all_by_label(label).collect();
        assert_eq!(buttons.len(), 2);
        assert!(
            buttons
                .iter()
                .all(|button| button.accesskit_node().is_disabled())
        );
    }
    assert_eq!(
        harness
            .query_all(by().label("Older version contents"))
            .count(),
        0,
        "The old diff must not appear under the newly chosen timestamp"
    );
    assert_eq!(
        harness
            .query_all(by().label("Newer version contents"))
            .count(),
        0
    );
}

#[test]
fn obsolete_comparison_success_or_failure_resolves_to_the_latest_choices() {
    for failed in [false, true] {
        let context = egui::Context::default();
        let mut dialog = fixture();
        let obsolete = dialog.comparison.take().unwrap();
        dialog.comparison_requested = Some(obsolete.choices);
        dialog
            .comparison_job
            .start_and_repaint("obsolete comparison", &context, move || {
                if failed {
                    Err("obsolete failure".into())
                } else {
                    Ok(obsolete)
                }
            })
            .unwrap();
        // Current/current uses only the frozen buffer, so following the queued
        // selection never reads real native revisions or creates a compiler.
        dialog.choices = [Choice::Current; 2];
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dialog.loaded() {
            assert!(dialog.poll(&context).is_none());
            assert!(
                Instant::now() < deadline,
                "Latest comparison was stranded (failed={failed})"
            );
            std::thread::yield_now();
        }
        let comparison = dialog.comparison.as_ref().unwrap();
        assert_eq!(comparison.choices, [Choice::Current; 2]);
        assert_eq!(
            comparison.sources,
            [dialog.current.clone(), dialog.current.clone()]
        );
        assert!(comparison.diff.hunks.is_empty());
        assert!(
            dialog.error.is_none(),
            "An obsolete error must not replace the latest comparison"
        );
    }
}

#[test]
fn empty_history_shows_a_message_without_restore_or_preview_actions() {
    let mut dialog = fixture();
    dialog.inventory.as_mut().unwrap().versions.clear();
    dialog.comparison = None;
    let mut harness = harness(dialog);
    harness.run();
    assert_eq!(
        harness
            .query_all(by().label("No saved versions yet."))
            .count(),
        1
    );
    assert_eq!(harness.query_all(by().label("Restore version")).count(), 0);
    assert_eq!(harness.query_all(by().label("Preview PDF")).count(), 0);
    assert_eq!(
        harness
            .query_all(by().label("Older version contents"))
            .count(),
        0
    );
}

#[test]
fn a_pending_render_disables_duplicate_mutations_and_keeps_the_existing_preview() {
    let context = egui::Context::default();
    let mut dialog = fixture();
    let frozen = dialog.comparison.as_ref().unwrap().sources[0].clone();
    let (release, wait) = std::sync::mpsc::channel();
    dialog
        .pdf_job
        .start_and_repaint("pending history render", &context, move || {
            wait.recv().map_err(|error| error.to_string())?;
            assert_eq!(frozen.as_ref(), "historical token\n");
            Err("Render cancelled by test".into())
        })
        .unwrap();
    let mut harness = harness(dialog);
    // An active render intentionally keeps its spinner animating. Inspect a
    // fixed number of frames instead of waiting for an idle repaint state.
    harness.run_steps(3);
    assert!(harness.state().0.busy());
    assert!(harness.state().0.preview_pdf().is_none());
    for label in ["Preview PDF", "Restore version"] {
        assert!(
            harness
                .get_all_by_label(label)
                .all(|button| button.accesskit_node().is_disabled())
        );
    }
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.state().0.pdf_job.is_running() {
        assert!(harness.state_mut().0.poll(&context).is_none());
        assert!(Instant::now() < deadline, "Render did not finish");
        std::thread::yield_now();
    }
    assert!(harness.state().0.preview_pdf().is_none());
    assert_eq!(harness.state().0.current.as_ref(), "current token\n");
}

#[test]
fn unsupported_or_disabled_build_engine_keeps_history_restoration_available() {
    let mut dialog = fixture();
    dialog.engine = None;
    let mut harness = harness(dialog);
    harness.run();
    assert!(
        harness
            .get_all_by_label("Preview PDF")
            .all(|button| button.accesskit_node().is_disabled())
    );
    assert!(
        !harness
            .get_all_by_label("Restore version")
            .next()
            .unwrap()
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn restore_completion_cannot_overwrite_a_newer_edit_or_different_document() {
    for replace_document in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        let path = directory.path().join("paper.typ");
        fs::write(&path, "current token\n").unwrap();
        app.document_mut().replace_loaded_unprojected(
            "current token\n".into(),
            path.clone(),
            DocumentKind::Typst,
            None,
        );
        let mut dialog = Dialog::new(
            app.document().key(),
            path.clone(),
            "current token\n".into(),
            false,
            directory.path().into(),
            None,
        );
        dialog
            .backup_job
            .start_and_repaint("ready backup for stale restore", &context, || {
                Ok(RestoreReady {
                    source: Arc::from("historical token\n"),
                })
            })
            .unwrap();
        app.version_history = Some(dialog);
        if replace_document {
            app.document_mut().replace_loaded_unprojected(
                "another document".into(),
                directory.path().join("another.typ"),
                DocumentKind::Typst,
                None,
            );
        } else {
            app.document_mut().edit(CCursorRange::default(), |source| {
                source.push_str("newer edit")
            });
        }
        let key = app.document().key();
        let source = app.document().source().clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .version_history
            .as_ref()
            .unwrap()
            .backup_job
            .is_running()
        {
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    app.poll_version_history(ui.ctx())
                })
                .drop_without_applying_deltas();
            assert!(
                Instant::now() < deadline,
                "Restore completion did not arrive"
            );
            std::thread::yield_now();
        }
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), &source);
        assert!(!app.save_job.is_running());
        assert_eq!(fs::read_to_string(path).unwrap(), "current token\n");
        assert!(
            app.version_history
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("changed")
        );
    }
}

#[test]
fn closing_history_cancels_rendering_without_mutating_the_document() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("original document");
    let key = app.document().key();
    let dialog = fixture();
    let cancel = dialog.cancel.clone();
    app.version_history = Some(dialog);
    app.close_version_history(&context);
    assert!(cancel.load(Ordering::Acquire));
    assert!(app.version_history.is_none());
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "original document");
}

#[cfg(unix)]
fn rendered_preview(directory: &Path, side: usize, source: &str) -> pdf::Preview {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(directory).unwrap();
    let original = directory.join("paper.typ");
    fs::write(&original, "current token\n").unwrap();
    fs::write(directory.join("paper.pdf"), b"existing PDF").unwrap();
    fs::write(directory.join("asset.txt"), "relative asset").unwrap();
    let compiler = directory.join("version-compiler");
    fs::write(
        &compiler,
        r#"#!/bin/sh
set -eu
previous=''; source=''; output=''
for arg do source=$previous; output=$arg; previous=$arg; done
printf 'compiling ...\n' >&2
printf '%%PDF-1.7\n' > "$output"
cat "$source" >> "$output"
cat "${source%/*}/asset.txt" >> "$output"
printf '\ncompiled successfully\n' >&2
exec sleep 60
"#,
    )
    .unwrap();
    fs::set_permissions(&compiler, fs::Permissions::from_mode(0o700)).unwrap();
    pdf::render(
        crate::version_render::Request {
            original,
            source: source.into(),
            project_root: directory.into(),
            engine: crate::compiler::EngineConfig::Typst(crate::compiler::TypstOptions {
                command: crate::tool_command::CommandCustomization::for_tool(
                    crate::toolchain::ToolKind::Typst,
                )
                .into(),
                executable: compiler,
                font_paths: Vec::new(),
            }),
        },
        side,
        Arc::from(source),
        "Saved · 2026-10-20 09:00:00 UTC".into(),
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn historical_preview_owns_immutable_bytes_and_private_pdf_without_exporting_over_source() {
    let directory = tempfile::tempdir().unwrap();
    let preview = rendered_preview(directory.path(), 0, "historical token\n");
    assert_eq!(preview.side, 0);
    assert_eq!(preview.source.as_ref(), "historical token\n");
    assert_eq!(preview.label, "Saved · 2026-10-20 09:00:00 UTC");
    let path = preview.path().to_owned();
    let bytes = preview.bytes().clone();
    assert!(path.starts_with(directory.path().canonicalize().unwrap().join(".tiptoptyp")));
    assert_eq!(fs::read(&path).unwrap(), bytes.as_ref());
    assert_eq!(
        bytes.as_ref(),
        b"%PDF-1.7\nhistorical token\nrelative asset"
    );
    assert!(Arc::ptr_eq(preview.bytes(), &bytes));
    assert_eq!(
        fs::read_to_string(directory.path().join("paper.typ")).unwrap(),
        "current token\n"
    );
    assert_eq!(
        fs::read(directory.path().join("paper.pdf")).unwrap(),
        b"existing PDF"
    );
    fs::write(directory.path().join("asset.txt"), "new asset").unwrap();
    assert_eq!(
        preview.bytes().as_ref(),
        b"%PDF-1.7\nhistorical token\nrelative asset"
    );
    drop(preview);
    assert!(!path.exists());
    // PDFium requests can retain these bytes after the history owner closes;
    // they do not need the private file or block its deletion.
    assert_eq!(
        bytes.as_ref(),
        b"%PDF-1.7\nhistorical token\nrelative asset"
    );
}

#[cfg(unix)]
#[test]
fn publishing_and_closing_a_historical_pdf_preserves_source_tab_and_live_preview() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let original = directory.path().join("paper.typ");
    app.document_mut().replace_loaded_unprojected(
        "current token\n".into(),
        original.clone(),
        DocumentKind::Typst,
        None,
    );
    let key = app.document().key();
    let live_bytes: Arc<[u8]> = crate::pdf::test_pdf().into();
    app.preview
        .accept_artifact(ArtifactKey::unversioned(key.revision), live_bytes.clone());
    let requested = app.preview.requested_backend;
    let tabs = app.tabs.len();
    let mut dialog = Dialog::new(
        key,
        original,
        "current token\n".into(),
        false,
        directory.path().into(),
        None,
    );
    dialog.pdf_preview = Some(rendered_preview(directory.path(), 0, "historical token\n"));
    let historical = dialog.preview_pdf().unwrap().1.clone();
    app.version_history = Some(dialog);
    assert!(app.pdfium_preview_requested());
    assert!(!app.pdfium_source_jump_available(false, &app.preview_document_path(), &historical));
    assert!(!app.interactive_preview_active());
    assert!(Arc::ptr_eq(app.preview.content.pdf().unwrap(), &live_bytes));
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "current token\n");
    assert_eq!(app.tabs.len(), tabs);
    app.close_version_history(&context);
    assert!(app.version_history.is_none());
    assert!(Arc::ptr_eq(app.preview.content.pdf().unwrap(), &live_bytes));
    assert_eq!(app.preview.requested_backend, requested);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.tabs.len(), tabs);
}

#[cfg(unix)]
#[test]
fn historical_preview_temporarily_disables_the_live_tex_source_map() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let path = directory.path().join("paper.tex");
    app.document_mut().replace_loaded_unprojected(
        "current TeX".into(),
        path.clone(),
        DocumentKind::Tex,
        None,
    );
    let key = ArtifactKey::unversioned(app.document().revision());
    let bytes: Arc<[u8]> = crate::pdf::test_pdf().into();
    app.preview.accept_artifact(key, bytes.clone());
    app.tex_tools.synctex = Some("must-not-execute".into());
    app.synctex.artifact = Some(super::super::synctex::VersionedMap {
        path: path.clone(),
        key,
        pdf: bytes.clone(),
        map: Arc::new(crate::synctex::Artifact {
            data: Arc::from([]),
            compressed: false,
            root: directory.path().into(),
            mirror: directory.path().into(),
        }),
    });
    assert!(app.pdfium_source_jump_available(false, &path, &bytes));
    let mut dialog = Dialog::new(
        app.document().key(),
        path.clone(),
        "current TeX".into(),
        false,
        directory.path().into(),
        None,
    );
    dialog.pdf_preview = Some(rendered_preview(directory.path(), 1, "historical token\n"));
    app.version_history = Some(dialog);
    assert!(
        !app.pdfium_source_jump_available(false, &path, &bytes),
        "The current TeX map cannot navigate historical PDF content"
    );
    app.close_version_history(&context);
    assert!(app.pdfium_source_jump_available(false, &path, &bytes));
}

#[test]
fn mismatched_render_metadata_is_rejected_before_starting_a_compiler_or_creating_files() {
    let directory = tempfile::tempdir().unwrap();
    let request = crate::version_render::Request {
        original: directory.path().join("absent.typ"),
        source: "requested source".into(),
        project_root: directory.path().into(),
        engine: fixture().engine.clone().unwrap(),
    };
    for (side, source) in [(0, "different source"), (2, "requested source")] {
        let result = pdf::render(
            request.clone(),
            side,
            Arc::from(source),
            "stale".into(),
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn docked_history_controls_fit_narrow_source_panes_and_remain_read_only() {
    for width in [320.0, 540.0] {
        let mut harness = harness_with_width(fixture(), width);
        harness.run();
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::ComboBox))
                .count(),
            2
        );
        let controls = harness
            .query_all(by().role(egui::accesskit::Role::ComboBox))
            .chain(harness.get_all_by_label("Preview PDF"))
            .chain(harness.get_all_by_label("Restore version"))
            .chain(harness.get_all_by_label("Close"));
        for control in controls {
            let rect = control.rect();
            assert!(
                rect.left() >= 0.0
                    && rect.right() <= width
                    && rect.top() >= 0.0
                    && rect.bottom() <= 520.0,
                "Control outside {width}pt code pane: {rect:?}"
            );
        }
        assert_eq!(
            harness
                .query_all(by().role(egui::accesskit::Role::MultilineTextInput))
                .count(),
            0
        );
        assert_eq!(harness.state().0.current.as_ref(), "current token\n");
    }
}

#[cfg(unix)]
#[test]
fn closing_historical_pdf_restores_live_zoom_and_pending_page_without_rebuilding_its_view() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let original = directory.path().join("paper.typ");
    app.document_mut().replace_loaded_unprojected(
        "current token\n".into(),
        original.clone(),
        DocumentKind::Typst,
        None,
    );
    for _ in 0..3 {
        app.pdfium_preview.zoom(PreviewZoomAction::In);
    }
    app.pdfium_preview.go_to_page(7);
    let live_zoom = app.pdfium_preview.controls_snapshot().zoom;
    let historical = rendered_preview(directory.path(), 0, "historical token\n");
    let mut dialog = Dialog::new(
        app.document().key(),
        original,
        "current token\n".into(),
        false,
        directory.path().into(),
        None,
    );
    dialog
        .pdf_job
        .start_and_repaint("ready historical preview", &context, move || Ok(historical))
        .unwrap();
    app.version_history = Some(dialog);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.version_pdf_active() {
        app.poll_version_history(&context);
        assert!(
            Instant::now() < deadline,
            "Historical PDF was not published"
        );
        std::thread::yield_now();
    }
    assert_eq!(app.pdfium_preview.controls_snapshot().zoom, 1.0);
    let retained = app
        .version_history
        .as_ref()
        .unwrap()
        .live_pdfium_view
        .as_ref()
        .unwrap();
    assert_eq!(retained.controls_snapshot().zoom, live_zoom);
    assert_eq!(retained.requested_page(), Some(7));
    app.pdfium_preview.zoom(PreviewZoomAction::Out);
    app.pdfium_preview.go_to_page(2);
    app.close_version_history(&context);
    assert_eq!(app.pdfium_preview.controls_snapshot().zoom, live_zoom);
    assert_eq!(app.pdfium_preview.requested_page(), Some(7));
}

#[cfg(target_os = "macos")]
#[test]
fn historical_pdf_override_does_not_activate_the_live_pdf_compiler_for_native_tinymist() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let original = directory.path().join("paper.typ");
    app.document_mut().replace_loaded_unprojected(
        "current token\n".into(),
        original.clone(),
        DocumentKind::Typst,
        None,
    );
    app.preview = PreviewController::new(false, PreviewPreference::InteractivePartial);
    app.preview.tinymist_preview_enabled = true;
    app.preview.tinymist_state = ServiceState::Ready("Tinymist ready".into());
    app.preview.webview_state = ServiceState::Ready("WebView ready".into());
    let generation = crate::tinymist::Generation(77);
    app.preview.connection.start(generation);
    app.preview.connection.initialized(generation);
    app.preview.connection.connect(
        generation,
        url::Url::parse("http://127.0.0.1:23625").unwrap(),
    );
    app.preview.status = PreviewStatus::Ready(Duration::ZERO);
    app.view_mode = ViewMode::Split;
    assert!(app.interactive_preview_active());
    assert!(!app.preview_processing_enabled());
    let historical = rendered_preview(directory.path(), 1, "historical token\n");
    let mut dialog = Dialog::new(
        app.document().key(),
        original,
        "current token\n".into(),
        false,
        directory.path().into(),
        None,
    );
    dialog
        .pdf_job
        .start_and_repaint("ready historical preview", &context, move || Ok(historical))
        .unwrap();
    app.version_history = Some(dialog);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.version_pdf_active() {
        app.poll_version_history(&context);
        assert!(
            Instant::now() < deadline,
            "Historical PDF was not published"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        app.preview_status_snapshot().effective_backend,
        crate::preview::PreviewBackend::Pdfium
    );
    assert_eq!(
        app.live_preview_status_snapshot().effective_backend,
        crate::preview::PreviewBackend::Interactive
    );
    assert!(app.live_preview_status_snapshot().native_ready);
    assert!(app.interactive_preview_requested());
    assert!(!app.interactive_preview_active());
    assert!(
        !app.preview_processing_enabled(),
        "Historical PDF must not admit unrelated live compiler work"
    );
    app.compile_deadline = Some(Instant::now());
    app.tick_compile(&context);
    assert!(app.compile_deadline.is_none());
    assert_eq!(app.preview.status, PreviewStatus::Ready(Duration::ZERO));
    app.close_version_history(&context);
    assert!(app.interactive_preview_active());
    assert!(!app.preview_processing_enabled());
}

#[cfg(target_os = "macos")]
#[test]
fn real_history_open_from_focused_code_only_releases_source_focus_without_a_child_window() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    context.set_embed_viewports(false);
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    theme::configure_test_fonts(&context);
    app.snapshot_scene = None;
    let path = directory.path().join("saved.typ");
    let source = "= Saved document\nOriginal source remains unchanged.\n";
    fs::write(&path, source).unwrap();
    app.document_mut().replace_loaded_unprojected(
        source.into(),
        path.clone(),
        DocumentKind::Typst,
        None,
    );
    app.view_mode = ViewMode::Code;
    let key = app.document().key();
    let tabs = app.tabs.len();
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
        ..Default::default()
    };
    let root = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
    root.focused = Some(true);
    root.inner_rect = input.screen_rect;
    let output = context.run_ui(input, |ui| {
        // Register the real source TextEdit before invoking the File action.
        app.show_editor(ui);
        let editor = source_editor_id(ui.ctx());
        ui.memory_mut(|memory| memory.request_focus(editor));
        assert!(ui.memory(|memory| memory.has_focus(editor)));
        assert!(app.native_command_enabled(AppCommand::RevertTo));
        app.begin_version_history(ui.ctx());
        assert!(
            app.version_history.is_some(),
            "The actual history admission path must run"
        );
        assert_eq!(app.view_mode, ViewMode::Split);
        assert!(
            !ui.memory(|memory| memory.has_focus(editor)),
            "Source typing must stop when its pane becomes read-only history"
        );
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), source);
        app.close_version_history(ui.ctx());
        assert!(app.version_history.is_none());
        assert!(ui.memory(|memory| memory.has_focus(editor)));
    });
    assert!(
        output
            .viewport_output
            .keys()
            .all(|id| *id == egui::ViewportId::ROOT),
        "Opening docked history must not create a second native viewport"
    );
    output.drop_without_applying_deltas();
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().path().as_ref(), Some(&path));
    assert_eq!(app.document().source(), source);
    assert_eq!(app.tabs.len(), tabs);
    assert_eq!(fs::read_to_string(path).unwrap(), source);
}
