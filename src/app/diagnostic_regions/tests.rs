use super::*;
use crate::diagnostics::{DiagnosticCorrection, DiagnosticLocation};
use egui_kittest::{Harness, kittest::Queryable as _};

fn source(kind: DocumentKind, enabled: bool) -> String {
    let prefix = if kind == DocumentKind::Tex { "%" } else { "//" };
    let switch = if enabled { "on" } else { "disabled" };
    format!("outside\n{prefix} ttt: {switch}\nteh\n{prefix} ttt: off\noutside")
}

fn app_with_source(root: &Path, kind: DocumentKind, text: &str) -> EditorApp {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.into());
    let extension = if kind == DocumentKind::Tex {
        "tex"
    } else {
        "typ"
    };
    let path = root.join(format!("main.{extension}"));
    std::fs::write(&path, text).unwrap();
    app.document_mut()
        .replace_loaded_unprojected(text.into(), path, kind, None);
    app
}

fn diagnostic(provider: &str, severity: DiagnosticSeverity, line: usize) -> Diagnostic {
    Diagnostic {
        correction: None,
        provider: Some(provider.into()),
        severity,
        source: DiagnosticSource::Main,
        location: Some(DiagnosticLocation { line, column: 1 }),
        message: format!("{provider} finding"),
        details: vec![format!("{provider} detail")],
    }
}

fn problems_harness(mut app: EditorApp) -> Harness<'static, EditorApp> {
    app.bottom_panel.select(PanelTab::Problems);
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1000.0, 700.0))
        .with_step_dt(0.01)
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                app.show_bottom_panel(ui);
                app.prepare_editor_data();
            },
            app,
        );
    harness.run();
    harness
}

#[test]
fn every_provider_and_severity_share_problems_badge_facets_and_inline_suppression() {
    for kind in [DocumentKind::Tex, DocumentKind::Typst] {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with_source(root.path(), kind, &source(kind, true));
        let providers = [
            ("Compiler", DiagnosticSeverity::Error),
            ("Tinymist", DiagnosticSeverity::Warning),
            ("TexLab", DiagnosticSeverity::Note),
            ("Badness", DiagnosticSeverity::Help),
            ("Harper", DiagnosticSeverity::Help),
            ("LTeX+", DiagnosticSeverity::Warning),
            ("Unicode", DiagnosticSeverity::Warning),
            ("Future provider", DiagnosticSeverity::Unknown),
        ];
        app.preview.diagnostics = providers[..2]
            .iter()
            .map(|&(provider, severity)| diagnostic(provider, severity, 3))
            .collect();
        app.preview.editor_diagnostics = providers[2..]
            .iter()
            .map(|&(provider, severity)| diagnostic(provider, severity, 3))
            .chain([diagnostic("Visible", DiagnosticSeverity::Warning, 5)])
            .collect();
        app.mark_diagnostics_changed();
        let original = (
            app.preview.diagnostics.clone(),
            app.preview.editor_diagnostics.clone(),
        );
        let mut harness = problems_harness(app);
        harness.get_by_label("Problems (1)");
        harness.get_by_label("Visible finding");
        for (provider, _) in providers {
            assert!(
                harness
                    .query_by_label(&format!("{provider} finding"))
                    .is_none()
            );
        }
        let app = harness.state();
        assert_eq!(app.diagnostic_regions.visible_count(), 1);
        assert_eq!(app.problems.model.indices(), [8]);
        assert_eq!(app.problems.model.total_count(), 1);
        assert_eq!(app.problems.model.providers().len(), 1);
        assert_eq!(app.problems.model.providers()[0].value, "Visible");
        assert_eq!(app.problems.model.severities().len(), 1);
        assert_eq!(app.problems.model.severities()[0].count, 1);
        let lines = app.editor_data.line_diagnostics();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line, 5);
        assert_eq!(lines[0].summary, "Visible finding");
        assert!(!lines[0].detail.contains("Harper"));
        assert_eq!(
            (&app.preview.diagnostics, &app.preview.editor_diagnostics),
            (&original.0, &original.1),
        );
        harness.state_mut().problems.filter.provider = Some("Harper".into());
        harness.run();
        harness.get_by_label("No matching problems");
        assert!(harness.state().problems.model.indices().is_empty());
    }
}

#[test]
fn editing_only_a_directive_restores_stored_corrections_and_retires_hidden_hover() {
    let root = tempfile::tempdir().unwrap();
    let kind = DocumentKind::Typst;
    let mut app = app_with_source(root.path(), kind, &source(kind, false));
    let text = app.document().source();
    let start = text[..text.find("teh").unwrap()].chars().count();
    let mut finding = diagnostic("Harper", DiagnosticSeverity::Help, 3);
    finding.correction = Some(DiagnosticCorrection {
        document: Some(app.document().key()),
        ..DiagnosticCorrection::new(
            start..start + 3,
            "teh".into(),
            Some("SpellCheck".into()),
            Some("Spelling".into()),
            vec!["the".into()],
        )
    });
    app.preview.editor_diagnostics = vec![finding.clone()];
    app.mark_diagnostics_changed();
    let published = app.preview.diagnostics_generation;
    let mut harness = problems_harness(app);
    harness.get_by_label("Harper finding");
    let initial_generation = harness.state().diagnostic_regions.generation;
    let origin = Rect::from_min_size(Pos2::ZERO, Vec2::splat(20.0));
    let app = harness.state_mut();
    app.diagnostic_tooltip = Some(DiagnosticTooltipOverlay {
        source: app.diagnostic_presentation(),
        line: 3,
        origin,
        anchor: origin.left_bottom(),
        severity: finding.severity,
        detail: finding.full_message(),
    });
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = text.replace("ttt: disabled", "ttt: on");
    });
    harness.run();
    harness.get_by_label("Problems");
    harness.get_by_label("No problems");
    assert!(harness.query_by_label("Harper finding").is_none());
    let context = harness.ctx.clone();
    harness.state_mut().refresh_diagnostic_tooltip(&context);
    assert!(harness.state().diagnostic_tooltip.is_none());
    assert!(harness.state().editor_data.line_diagnostics().is_empty());
    assert!(harness.state().diagnostic_regions.generation > initial_generation);
    assert_eq!(harness.state().preview.diagnostics_generation, published);

    harness
        .state_mut()
        .document_mut()
        .edit(CCursorRange::default(), |text| {
            *text = text.replace("ttt: on", "ttt: disabled");
        });
    harness.run();
    harness.get_by_label("Problems (1)");
    harness.get_by_label("Harper finding");
    let app = harness.state();
    assert_eq!(app.preview.diagnostics_generation, published);
    assert_eq!(app.preview.editor_diagnostics, [finding.clone()]);
    assert_eq!(
        app.editor_data.line_diagnostics()[0].detail,
        finding.full_message()
    );
    assert_eq!(app.problems.model.indices(), [0]);
    assert_eq!(app.problems.model.categories()[0].value, "Spelling");
    assert!(
        harness
            .state_mut()
            .apply_diagnostic_correction(&finding, 0, &context)
            .is_err(),
        "restoring a stale suggestion must not authorize editing a newer revision"
    );
}

#[test]
fn named_relative_and_generated_sources_resolve_without_hiding_unknown_locations() {
    use crate::private_workspace::PrivateWorkspace;
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_source(
        root.path(),
        DocumentKind::Typst,
        &source(DocumentKind::Typst, true),
    );
    let path = app.document().path().clone().unwrap();
    let mirror = PrivateWorkspace::open(root.path())
        .unwrap()
        .mirrored_source(
            root.path(),
            "main.typ",
            "an intentionally different saved mirror",
        )
        .unwrap();
    let mut findings = vec![diagnostic("Main", DiagnosticSeverity::Error, 3)];
    for (provider, path) in [
        ("Absolute", path),
        ("Relative", PathBuf::from("main.typ")),
        ("Generated", mirror.path().to_owned()),
        ("Missing include", root.path().join("missing.typ")),
    ] {
        findings.push(Diagnostic {
            source: DiagnosticSource::File(path),
            ..diagnostic(provider, DiagnosticSeverity::Warning, 3)
        });
    }
    findings.push(Diagnostic {
        source: DiagnosticSource::Global,
        ..diagnostic("Global", DiagnosticSeverity::Error, 3)
    });
    findings.push(Diagnostic {
        location: None,
        ..diagnostic("Unlocated", DiagnosticSeverity::Unknown, 3)
    });
    findings.push(diagnostic("Outside", DiagnosticSeverity::Note, 5));
    app.preview.diagnostics = findings.clone();
    app.mark_diagnostics_changed();
    app.prepare_editor_data();
    assert_eq!(
        app.diagnostic_regions.suppressed,
        [true, true, true, true, false, false, false, false]
    );
    assert!(
        !app.diagnostic_regions.external_job.is_running(),
        "preparing editor data cannot synchronously start file reads without a repaint owner"
    );
    assert_eq!(app.preview.diagnostics, findings);
    assert_eq!(app.editor_data.line_diagnostics().len(), 1);
    assert_eq!(
        app.editor_data.line_diagnostics()[0].summary,
        "Outside finding"
    );
}

#[test]
fn a_pinned_preview_and_other_active_tab_use_their_own_unsaved_directives() {
    let context = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_source(
        root.path(),
        DocumentKind::Typst,
        &source(DocumentKind::Typst, true),
    );
    let first = app.tabs.active_id().unwrap();
    let first_path = app.document().path().clone().unwrap();
    app.snapshot_scene = None;
    app.select_preview_tab(first, &context);
    app.lifecycle = DocumentLifecycle::Active;
    app.new_tab(&context);
    let second = app.tabs.active_id().unwrap();
    let second_path = root.path().join("other.typ");
    // Disk has a suppression, but this tab's newer unsaved source does not.
    std::fs::write(&second_path, source(DocumentKind::Typst, true)).unwrap();
    app.document_mut().replace_loaded_unprojected(
        source(DocumentKind::Typst, false),
        second_path.clone(),
        DocumentKind::Typst,
        None,
    );
    app.preview.diagnostics = vec![diagnostic("Main", DiagnosticSeverity::Error, 3)];
    app.preview.editor_diagnostics = [first_path, second_path]
        .into_iter()
        .enumerate()
        .map(|(index, path)| Diagnostic {
            source: DiagnosticSource::File(path),
            ..diagnostic(&format!("Tab {index}"), DiagnosticSeverity::Warning, 3)
        })
        .collect();
    app.mark_diagnostics_changed();
    app.prepare_editor_data();
    assert_eq!(app.tabs.preview_id(), Some(first));
    assert_eq!(app.tabs.active_id(), Some(second));
    assert_eq!(app.diagnostic_regions.suppressed, [true, true, false]);
    assert_eq!(
        app.editor_data.line_diagnostics()[0].summary,
        "Tab 1 finding"
    );
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = source(DocumentKind::Typst, true);
    });
    app.prepare_editor_data();
    assert_eq!(app.diagnostic_regions.suppressed, [true, true, true]);
    assert!(app.editor_data.line_diagnostics().is_empty());
    assert!(!app.diagnostic_regions.external_job.is_running());
}

#[test]
fn unicode_markers_use_scalar_positions_and_idle_frames_reuse_the_mask() {
    let root = tempfile::tempdir().unwrap();
    let text = "文😀\n// ttt: on\nα\u{200b}😀\n// ttt: off\n中\u{200b}";
    let mut app = app_with_source(root.path(), DocumentKind::Typst, text);
    app.preview.editor_diagnostics = vec![diagnostic("Unicode", DiagnosticSeverity::Warning, 3)];
    app.mark_diagnostics_changed();
    app.prepare_diagnostic_visibility(None);
    let tab = app.tabs.active_id();
    let hidden = text[..text.find('\u{200b}').unwrap()].chars().count();
    let visible = text[..text.rfind('\u{200b}').unwrap()].chars().count();
    let opening = text[..text.find("// ttt: on").unwrap()].chars().count();
    let closing = text[..text.find("// ttt: off").unwrap()].chars().count();
    assert!(app.diagnostic_regions.suppresses_char(tab, hidden));
    assert!(!app.diagnostic_regions.suppresses_char(tab, visible));
    assert!(!app.diagnostic_regions.suppresses_char(tab, opening));
    assert!(!app.diagnostic_regions.suppresses_char(tab, closing));
    assert!(!app.diagnostic_regions.suppresses_char(None, hidden));
    let generation = app.diagnostic_regions.generation;
    let rebuilds = app.diagnostic_regions.rebuilds;
    let mask = app.diagnostic_regions.suppressed.as_ptr();
    for _ in 0..40 {
        app.prepare_diagnostic_visibility(None);
    }
    assert_eq!(app.diagnostic_regions.generation, generation);
    assert_eq!(app.diagnostic_regions.rebuilds, rebuilds);
    assert_eq!(app.diagnostic_regions.suppressed.as_ptr(), mask);
}

#[test]
fn unopened_includes_are_read_asynchronously_and_missing_files_stay_visible() {
    let context = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_source(root.path(), DocumentKind::Typst, "outside\nplain\nthird");
    let included = root.path().join("included.tex");
    let missing = root.path().join("missing.typ");
    std::fs::write(&included, source(DocumentKind::Tex, true)).unwrap();
    app.preview.diagnostics = [included.clone(), missing]
        .into_iter()
        .enumerate()
        .map(|(index, path)| Diagnostic {
            source: DiagnosticSource::File(path),
            ..diagnostic(&format!("External {index}"), DiagnosticSeverity::Warning, 3)
        })
        .collect();
    app.mark_diagnostics_changed();
    let original = app.preview.diagnostics.clone();
    app.prepare_diagnostic_visibility(None);
    assert_eq!(app.diagnostic_regions.suppressed, [false, false]);
    assert!(!app.diagnostic_regions.external_job.is_running());
    app.prepare_diagnostic_visibility(Some(&context));
    assert!(app.diagnostic_regions.external_job.is_running());
    app.diagnostic_regions
        .external_job
        .wait_until_ready_for_test()
        .unwrap();
    app.prepare_diagnostic_visibility(Some(&context));
    assert_eq!(app.diagnostic_regions.suppressed, [true, false]);
    assert!(!app.diagnostic_regions.external_job.is_running());
    let generation = app.diagnostic_regions.generation;
    for _ in 0..20 {
        app.prepare_diagnostic_visibility(Some(&context));
    }
    assert_eq!(app.diagnostic_regions.generation, generation);
    assert!(!app.diagnostic_regions.external_job.is_running());

    std::fs::write(included, source(DocumentKind::Tex, false)).unwrap();
    app.diagnostic_regions.invalidate_files();
    app.prepare_diagnostic_visibility(Some(&context));
    app.diagnostic_regions
        .external_job
        .wait_until_ready_for_test()
        .unwrap();
    app.prepare_diagnostic_visibility(Some(&context));
    assert_eq!(app.diagnostic_regions.suppressed, [false, false]);
    assert_eq!(app.preview.diagnostics, original);
}

#[test]
fn an_open_source_edit_cannot_drop_a_queued_external_refresh_behind_an_old_reader() {
    let context = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let mut app = app_with_source(root.path(), DocumentKind::Typst, "outside\nplain\nthird");
    let included = root.path().join("included.tex");
    std::fs::write(&included, source(DocumentKind::Tex, true)).unwrap();
    app.preview.diagnostics = vec![Diagnostic {
        source: DiagnosticSource::File(included.clone()),
        ..diagnostic("External", DiagnosticSeverity::Warning, 3)
    }];
    app.mark_diagnostics_changed();
    app.prepare_diagnostic_visibility(Some(&context));
    app.diagnostic_regions
        .external_job
        .wait_until_ready_for_test()
        .unwrap();
    app.prepare_diagnostic_visibility(Some(&context));
    assert_eq!(app.diagnostic_regions.suppressed, [true]);

    // Hold a prior reader at an exact boundary instead of relying on the
    // scheduler, file size or a sleep to overlap it with a new publication.
    let old_epoch = app.diagnostic_regions.external_epoch;
    let old_results = app.diagnostic_regions.external.clone();
    let (release, held) = std::sync::mpsc::channel();
    app.diagnostic_regions
        .external_job
        .start("held-diagnostic-regions-test", move || {
            held.recv().map_err(|error| error.to_string())?;
            Ok((old_epoch, old_results))
        })
        .unwrap();
    std::fs::write(&included, source(DocumentKind::Tex, false)).unwrap();
    app.mark_diagnostics_changed();
    app.prepare_diagnostic_visibility(None);
    assert_ne!(app.diagnostic_regions.external_epoch, old_epoch);
    assert!(app.diagnostic_regions.external_needed.contains(&included));

    // A local edit rebuilds the projection while the obsolete reader remains
    // busy. Its known cache entry must not erase the already queued reread.
    app.document_mut()
        .edit(CCursorRange::default(), |text| text.push('!'));
    app.prepare_diagnostic_visibility(None);
    release.send(()).unwrap();
    app.diagnostic_regions
        .external_job
        .wait_until_ready_for_test()
        .unwrap();
    app.prepare_diagnostic_visibility(Some(&context));
    assert!(
        app.diagnostic_regions.external_job.is_running(),
        "the obsolete reader cannot satisfy or discard the queued current-epoch refresh"
    );
    app.diagnostic_regions
        .external_job
        .wait_until_ready_for_test()
        .unwrap();
    app.prepare_diagnostic_visibility(Some(&context));
    assert_eq!(app.diagnostic_regions.suppressed, [false]);
    assert!(!app.diagnostic_regions.external_job.is_running());
}

#[test]
fn tex_include_comment_syntax_does_not_change_when_the_file_is_opened() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("custom.sty");
    let text = source(DocumentKind::Tex, true);
    std::fs::write(&path, &text).unwrap();
    let external = read_external(vec![path.clone()]);
    assert!(external[&path].as_ref().unwrap().contains_line(3));
    let mut app = app_with_source(root.path(), DocumentKind::Tex, "outside");
    app.document_mut()
        .replace_loaded_unprojected(text, path.clone(), DocumentKind::Text, None);
    app.preview.diagnostics = vec![Diagnostic {
        source: DiagnosticSource::File(path),
        ..diagnostic("Include", DiagnosticSeverity::Warning, 3)
    }];
    app.mark_diagnostics_changed();
    app.prepare_diagnostic_visibility(None);
    assert_eq!(app.diagnostic_regions.suppressed, [true]);
    app.document_mut().edit(CCursorRange::default(), |source| {
        *source = source.replace("ttt: on", "ttt: disabled")
    });
    app.prepare_diagnostic_visibility(None);
    assert_eq!(app.diagnostic_regions.suppressed, [false]);
}
