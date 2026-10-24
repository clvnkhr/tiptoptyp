use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

#[test]
fn typst_tex_parity_explorer_can_preview_either_source_language() {
    for extension in ["typ", "tex", "TEX"] {
        let path = PathBuf::from(format!("/workspace/main.{extension}"));
        let expected = path.clone();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 650.0))
            .build_ui_state(
                move |ui, action| {
                    show_workspace_popup_ui(ui, &path, true, false, true, action);
                },
                None::<AppPopupAction>,
            );
        harness.run();
        let preview = harness.get_by_label_contains("Use for preview");
        assert!(
            !preview.accesskit_node().is_disabled(),
            "{extension} must be a preview source"
        );
        preview.click();
        harness.run();
        assert!(
            matches!(harness.state(), Some(AppPopupAction::Workspace(WorkspaceMenuAction::TogglePreview(path))) if path == &expected)
        );
    }
}

#[test]
fn typst_tex_parity_new_table_is_available_for_both_languages() {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, std::env::temp_dir());
    for kind in [DocumentKind::Typst, DocumentKind::Tex] {
        app.document_mut().replace_untitled_kind(kind);
        assert!(app.native_command_enabled(AppCommand::NewTable), "{kind:?}");
    }
    app.document_mut().replace_untitled_kind(DocumentKind::Text);
    assert!(!app.native_command_enabled(AppCommand::NewTable));
}

#[test]
fn typst_tex_parity_typed_untitled_documents_keep_language_in_save_and_preview_paths() {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, std::env::temp_dir());
    let tab = app.tabs.active_id().unwrap();
    for (kind, extension) in [(DocumentKind::Tex, "tex"), (DocumentKind::Typst, "typ")] {
        app.document_mut().replace_untitled_kind(kind);
        let (save_kind, name) = crate::workflow::document_save_defaults(app.document());
        assert_eq!(save_kind, kind);
        assert_eq!(name, format!("Untitled.{extension}"));
        assert_eq!(
            crate::workflow::source_save_path("paper".into(), save_kind),
            PathBuf::from(format!("paper.{extension}"))
        );
        assert_eq!(
            crate::workflow::source_save_path("paper.txt".into(), save_kind),
            PathBuf::from("paper.txt")
        );
        assert_eq!(app.untitled_tab_path(tab).extension().unwrap(), extension);
        assert_eq!(app.preview_document_path().extension().unwrap(), extension);
        assert!(app.document().path().is_none());
        assert!(!app.document().is_dirty());
    }
    app.document_mut().replace_untitled_kind(DocumentKind::Text);
    assert_eq!(
        crate::workflow::document_save_defaults(app.document()),
        (DocumentKind::Text, "Untitled".into())
    );
}

#[test]
fn typst_tex_parity_tex_packages_open_documentation_and_typst_keeps_its_catalog() {
    for kind in [DocumentKind::Tex, DocumentKind::Typst] {
        let index = ProjectIndex {
            packages: vec!["amsmath".into()],
            ..Default::default()
        };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(400.0, 300.0))
            .build_ui_state(
                move |ui, action| {
                    let outcome = explorer_view::show_project_index_section(
                        ui,
                        ExplorerSection::Packages,
                        Path::new("/project"),
                        &index,
                        kind,
                        "",
                    );
                    if outcome.open_package_manager || outcome.tex_documentation.is_some() {
                        *action = outcome;
                    }
                },
                explorer_view::ExplorerProjectSectionOutcome::default(),
            );
        harness.run();
        let browse = if kind == DocumentKind::Tex {
            "TeXdoc…"
        } else {
            "Browse Typst packages…"
        };
        harness.get_by_label(browse).click();
        harness.run();
        assert_eq!(
            harness.state().open_package_manager,
            kind == DocumentKind::Typst
        );
        assert_eq!(
            harness.state().tex_documentation.as_deref(),
            (kind == DocumentKind::Tex).then_some("")
        );
        if kind == DocumentKind::Tex {
            harness.get_by_label("amsmath").click();
            harness.run();
            assert_eq!(
                harness.state().tex_documentation.as_deref(),
                Some("amsmath")
            );
            assert!(!harness.state().open_package_manager);
        }
    }
}

#[test]
fn typst_tex_parity_full_explorer_keeps_texdoc_action_across_later_sections() {
    let index = ProjectIndex {
        packages: vec!["amsmath".into()],
        ..Default::default()
    };
    let statuses = crate::git::editor::FileStatuses::default();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(450.0, 1200.0))
        .build_ui_state(
            |ui, state: &mut (ExplorerPanelState, Option<String>)| {
                let output = explorer_view::show(
                    ui,
                    explorer_view::Input {
                        root: Path::new("/project"),
                        label: "/project",
                        snapshot: None,
                        index: &index,
                        project_kind: DocumentKind::Tex,
                        active: None,
                        preview: None,
                        statuses: &statuses,
                        error: None,
                        order: ExplorerOrder::default(),
                        git_visible: false,
                        read_only: false,
                        tex: true,
                        handwriting: Default::default(),
                    },
                    &mut state.0,
                    &mut crate::handwriting::Drawing::default(),
                    |_| panic!("hidden Git must not be painted"),
                );
                if output.tex_documentation.is_some() {
                    state.1 = output.tex_documentation;
                }
            },
            (ExplorerPanelState::default(), None),
        );
    harness.run();
    for label in ["Packages", "Labels", "References"] {
        harness.get_by_label(label).click();
        harness.run();
    }
    harness.get_by_label("TeXdoc…").click();
    harness.run();
    assert_eq!(harness.state().1.as_deref(), Some(""));
    harness.get_by_label("amsmath").click();
    harness.run();
    assert_eq!(harness.state().1.as_deref(), Some("amsmath"));
}

#[test]
fn typst_tex_parity_new_document_follows_active_typesetting_language() {
    for kind in [DocumentKind::Typst, DocumentKind::Tex] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.lifecycle = DocumentLifecycle::Active;
        app.document_mut().replace_untitled_kind(kind);
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str("keep me"));
        let first = app.tabs.active_id().unwrap();
        app.new_tab(&context);
        assert_ne!(app.tabs.active_id(), Some(first));
        assert_eq!(app.document().kind(), kind);
        assert_eq!(app.document().source(), "");
        assert!(!app.document().is_dirty());
        assert_eq!(app.document_for_tab(first).unwrap().source(), "keep me");
        assert_eq!(app.tabs.preview_id(), Some(first));
    }
}

#[test]
fn typst_tex_parity_tex_index_uses_edits_before_save_and_redebounces() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
    app.lifecycle = DocumentLifecycle::Active;
    app.settings.auto_save = false;
    let path = root.path().join("main.tex");
    app.document_mut().replace_loaded_unprojected(
        "\\section{Disk title}".into(),
        path.clone(),
        DocumentKind::Tex,
        None,
    );
    for title in ["Unsaved title", "Changed again"] {
        app.project_index_deadline.clear();
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = format!("\\section{{{title}}}");
        });
        app.mark_edited();
        assert!(app.project_index_deadline.is_pending());
        let mut overrides = BTreeMap::new();
        app.collect_tab_sources(&mut overrides).unwrap();
        assert_eq!(overrides.get(&path), Some(app.document().source()));
        let index = crate::project_index::analyze_project(root.path(), &path, &overrides);
        assert_eq!(index.outline[0].title, title);
        assert!(!path.exists(), "indexing must not save the document");
    }
}
