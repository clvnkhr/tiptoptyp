//! Exercise the shared controls through widgets, the real PDF worker and compiler.
use super::*;
use crate::settings::ToolPreference;
use egui_kittest::{Harness, kittest::Queryable as _};

fn compile_fixture(root: &Path, last: &str) -> Arc<[u8]> {
    let source = format!(
        "#set page(width: 400pt, height: 600pt)\n= First\nFirst page.\n#pagebreak()\n= Second\nSecond page.\n#pagebreak()\n= Third\n{last}\n"
    );
    let file = root.join("main.typ");
    std::fs::write(&file, source).unwrap();
    let executable = std::env::var_os("TIPTOPTYP_TEST_TYPST")
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_tool(ToolKind::Typst, &ToolPreference::default()).program);
    let result = std::process::Command::new(executable)
        .arg("compile")
        .arg(&file)
        .arg(root.join("main.pdf"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read(root.join("main.pdf")).unwrap().into()
}

fn fixture(root: &Path, bytes: Arc<[u8]>) -> Harness<'static, EditorApp> {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.into());
    app.settings.preview_preference = PreviewPreference::Pdfium;
    app.preview = PreviewController::new(false, PreviewPreference::Pdfium);
    app.preview
        .accept_artifact(ArtifactKey::unversioned(1), bytes);
    app.view_mode = ViewMode::Split;
    app.settings.titlebar_menus = false;
    app.preview_controls.position = Some(egui::pos2(620.0, 80.0));
    Harness::builder()
        .with_size(egui::vec2(1100.0, 650.0))
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                app.handle_shortcuts(ui.ctx(), None);
                app.process_native_menu_commands(ui.ctx(), None);
                egui::Panel::top("test-toolbar").show(ui, |ui| app.show_toolbar(ui, None));
                if app.preview_controls.popout.is_some() {
                    app.show_preview_window(ui.ctx());
                } else {
                    app.show_pdfium_view(ui, false);
                    app.show_preview_controls(ui.ctx());
                }
            },
            app,
        )
}

fn wait(harness: &mut Harness<'_, EditorApp>, label: &str, ready: impl Fn(&EditorApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        harness.run_steps(2);
        if ready(harness.state()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {:?}",
            harness.state().pdfium_preview.activity()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires pinned Typst and bundled PDFium"]
fn e2e_pdf_controls_navigate_search_edit_query_and_restore_history() {
    let directory = tempfile::tempdir().unwrap();
    let pdf = compile_fixture(directory.path(), "Unique needle on the final page.");
    let mut harness = fixture(directory.path(), pdf);
    wait(&mut harness, "initial PDF", |app| {
        app.pdfium_preview.ready()
    });
    assert_eq!(harness.state().pdfium_preview.controls_snapshot().count, 3);
    harness.get_by_label("Preview controls").click();
    harness.run_steps(3);
    harness.get_by_label("Outline").click();
    harness.run_steps(3);
    harness.get_by_label("Second").click();
    wait(&mut harness, "outline navigation", |app| {
        app.pdfium_preview.controls_snapshot().page == 1
    });
    harness.get_by_label("Back").click();
    wait(&mut harness, "back", |app| {
        app.pdfium_preview.controls_snapshot().page == 0
    });
    harness.get_by_label("Forward").click();
    wait(&mut harness, "forward", |app| {
        app.pdfium_preview.controls_snapshot().page == 1
    });
    harness.get_by_label("Find in preview").click();
    harness.run_steps(2);
    harness.get_by_label("Find in preview").type_text("needle");
    harness.run_steps(2);
    harness.get_by_label("Find next").click();
    wait(
        &mut harness,
        "first search jumps without a second click",
        |app| app.pdfium_preview.controls_snapshot().page == 2,
    );
    let source = harness.state().document().source().to_owned();
    harness.get_by_label("Find in preview").click();
    harness.run_steps(2);
    harness
        .state_mut()
        .enqueue_native_menu_command(AppCommand::SelectAll);
    harness.run_steps(2);
    harness.get_by_label("Find in preview").type_text("Second");
    harness.run_steps(2);
    assert_eq!(harness.state().preview_controls.query, "Second");
    assert_eq!(harness.state().document().source(), &source);
    harness.get_by_label("Find next").click();
    wait(&mut harness, "replaced search", |app| {
        app.pdfium_preview.controls_snapshot().page == 1
    });
    harness.get_by_label("Fit page width").click();
    wait(&mut harness, "fit", |app| app.pdfium_preview.ready());
    let fit = harness.state().pdfium_preview.controls_snapshot().zoom;
    harness.get_by_label("Zoom in").click();
    wait(&mut harness, "zoom", |app| {
        app.pdfium_preview.ready() && app.pdfium_preview.controls_snapshot().zoom > fit
    });
    harness.get_by_label("Fit page width").click();
    wait(&mut harness, "fit again", |app| {
        app.pdfium_preview.ready() && app.pdfium_preview.controls_snapshot().zoom == fit
    });
    harness.get_by_label("Minimize controls").click();
    harness.run_steps(2);
    assert!(!harness.state().preview_controls.open);
    harness.get_by_label("Preview controls").click();
    harness.run_steps(2);
    assert_eq!(harness.state().preview_controls.query, "Second");
    harness.set_size(egui::vec2(500.0, 650.0));
    wait(&mut harness, "fit after narrowing", |app| {
        app.pdfium_preview.ready()
    });
    assert_eq!(
        harness.state().pdfium_preview.controls_snapshot().page,
        1,
        "resize must preserve the reading position"
    );
    harness.set_size(egui::vec2(1100.0, 650.0));
    wait(&mut harness, "fit after widening", |app| {
        app.pdfium_preview.ready()
    });
    let document = harness.state().document().key();
    harness.get_by_label("Pop out").click();
    harness.run_steps(3);
    assert!(harness.state().preview_controls.popout.is_some());
    harness.get_by_label("Return to document").click();
    harness.run_steps(3);
    assert!(harness.state().preview_controls.popout.is_none());
    assert_eq!(harness.state().view_mode, ViewMode::Split);
    assert_eq!(harness.state().document().key(), document);
    assert_eq!(harness.state().pdfium_preview.controls_snapshot().page, 1);
}

#[test]
#[ignore = "requires pinned Typst and bundled PDFium"]
fn e2e_pdf_search_recovers_after_recompile_and_invalid_pdf() {
    let directory = tempfile::tempdir().unwrap();
    let first = compile_fixture(directory.path(), "First version with a needle.");
    let second = compile_fixture(directory.path(), "Recompiled version with a needle.");
    let mut harness = fixture(directory.path(), first);
    wait(&mut harness, "initial PDF", |app| {
        app.pdfium_preview.ready()
    });
    harness.get_by_label("Preview controls").click();
    harness.run_steps(3);
    harness.get_by_label("Find in preview").click();
    harness.run_steps(2);
    harness.get_by_label("Find in preview").type_text("needle");
    harness.run_steps(2);
    harness.get_by_label("Find next").click();
    wait(&mut harness, "search", |app| {
        app.pdfium_preview.controls_snapshot().page == 2
    });
    harness
        .state_mut()
        .preview
        .accept_artifact(ArtifactKey::unversioned(2), Arc::from(&b"invalid PDF"[..]));
    wait(&mut harness, "invalid PDF", |app| {
        matches!(
            app.pdfium_preview.activity(),
            crate::activity::Activity::Failed(_)
        )
    });
    harness
        .state_mut()
        .preview
        .accept_artifact(ArtifactKey::unversioned(3), second);
    wait(&mut harness, "recovered PDF and search", |app| {
        matches!(
            app.pdfium_preview.activity(),
            crate::activity::Activity::Idle
        )
    });
    harness.get_by_label("Back").click();
    wait(&mut harness, "back after replacement", |app| {
        app.pdfium_preview.controls_snapshot().page == 0
    });
    harness.get_by_label("Find next").click();
    wait(&mut harness, "same query on replacement", |app| {
        app.pdfium_preview.controls_snapshot().page == 2
    });
}
