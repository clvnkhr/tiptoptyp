//! Real Git results drive the same tab and buffer that the docked editor edits.
use super::*;
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    git(&root, &["init", "--quiet"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    fs::write(
        root.join("chapter.typ"),
        "= Heading\nOriginal paragraph.\nEnd.\n",
    )
    .unwrap();
    git(&root, &["add", "chapter.typ"]);
    git(&root, &["commit", "--quiet", "-m", "Initial"]);
    (directory, root)
}

fn finish_panel(app: &mut EditorApp, context: &egui::Context, root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.git.poll(context, root);
        if app.git.activity() == crate::activity::Activity::Idle {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Git panel did not finish: {:?}",
            app.git.activity()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn finish_document(app: &mut EditorApp, context: &egui::Context, root: &Path) {
    app.tick_git_file_diff(context);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending_git_file_diff.is_some() {
        let document = &app.tabs.current_record().document;
        app.git_editor.tick(
            context,
            root,
            document.path().as_deref(),
            document.key(),
            document.source(),
            None,
        );
        app.tick_git_file_diff(context);
        assert!(
            Instant::now() < deadline,
            "Git document scan did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn modified_staged_and_added_files_open_complete_editable_diffs() {
    for mode in [
        "modified",
        "staged",
        "added",
        "untracked",
        "conflict",
        "renamed",
        "renamed-edited",
    ] {
        let (_directory, root) = repository();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let renamed = mode.starts_with("renamed");
        if renamed {
            git(&root, &["mv", "chapter.typ", "renamed.typ"]);
        }
        // The real snapshot deliberately disables rename detection, exposing
        // a staged move as deleted + added rather than an unsupported R row.
        let added = matches!(mode, "added" | "untracked") || renamed;
        let relative = if renamed {
            "renamed.typ"
        } else if added {
            "new.typ"
        } else {
            "chapter.typ"
        };
        let path = root.join(relative);
        let current = if mode == "conflict" {
            "= Heading\n<<<<<<< HEAD\nOurs.\n=======\nTheirs.\n>>>>>>> branch\nEnd.\n"
        } else if mode == "renamed" {
            "= Heading\nOriginal paragraph.\nEnd.\n"
        } else {
            "= Heading\nChanged paragraph.\nEnd.\n"
        };
        fs::write(&path, current).unwrap();
        if matches!(mode, "staged" | "added") {
            git(&root, &["add", relative]);
        }
        app.git.open(&context, &root);
        finish_panel(&mut app, &context, &root);
        app.request_git_file_diff(&context, relative.into());
        finish_panel(&mut app, &context, &root);
        finish_document(&mut app, &context, &root);
        let diff = app.document_diff.as_ref().expect(mode);
        assert_eq!(app.document().path().as_deref(), Some(path.as_path()));
        assert_eq!(diff.current, current);
        assert_eq!(
            diff.before,
            if added {
                ""
            } else {
                "= Heading\nOriginal paragraph.\nEnd.\n"
            }
        );
        assert!(!diff.hunks.is_empty());
        assert_eq!(fs::read_to_string(path).unwrap(), current);
        assert!(!app.document().is_dirty());
        assert!(app.document_workflow.modal().is_none());
    }
}

#[test]
fn file_diff_reuses_the_unsaved_tab_and_keeps_its_buffer() {
    let (_directory, root) = repository();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    let path = root.join("chapter.typ");
    fs::write(&path, "= Heading\nStaged disk paragraph.\nEnd.\n").unwrap();
    git(&root, &["add", "chapter.typ"]);
    assert!(app.open_tab_path(path.clone(), &context));
    let dirty = "= Heading\nUnsaved paragraph.\nEnd.\n";
    app.document_mut()
        .edit(CCursorRange::default(), |text| *text = dirty.into());
    let key = app.document().key();
    let tabs = app.tabs.len();
    app.git.open(&context, &root);
    finish_panel(&mut app, &context, &root);
    app.request_git_file_diff(&context, "chapter.typ".into());
    finish_panel(&mut app, &context, &root);
    finish_document(&mut app, &context, &root);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.tabs.len(), tabs);
    assert_eq!(app.document_diff.as_ref().unwrap().current, dirty);
    assert_eq!(
        app.document_diff.as_ref().unwrap().before,
        "= Heading\nOriginal paragraph.\nEnd.\n"
    );
    assert!(app.document().is_dirty());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "= Heading\nStaged disk paragraph.\nEnd.\n"
    );
    assert!(app.document_workflow.modal().is_none());
}

#[test]
fn removed_and_binary_rows_retain_the_read_only_diff_without_opening_a_tab() {
    let modes = ["removed", "binary", "binary-text-extension", "private"]
        .into_iter()
        .chain(cfg!(unix).then_some("symlink"))
        .chain(cfg!(unix).then_some("directory-symlink"));
    for mode in modes {
        let (_directory, root) = repository();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let relative = match mode {
            "binary" => "blob.bin",
            "private" => ".tiptoptyp/private.typ",
            "symlink" => "alias.typ",
            "directory-symlink" => "nested/child.typ",
            _ => "chapter.typ",
        };
        #[cfg(unix)]
        let external = tempfile::tempdir().unwrap();
        match mode {
            "removed" => fs::remove_file(root.join(relative)).unwrap(),
            "private" => {
                fs::create_dir(root.join(".tiptoptyp")).unwrap();
                fs::write(root.join(relative), "Generated source.").unwrap();
                git(&root, &["add", "-f", relative]);
            }
            #[cfg(unix)]
            "symlink" => {
                std::os::unix::fs::symlink("chapter.typ", root.join(relative)).unwrap();
                git(&root, &["add", relative]);
            }
            #[cfg(unix)]
            "directory-symlink" => {
                fs::create_dir(root.join("nested")).unwrap();
                fs::write(root.join(relative), "Original tracked child.").unwrap();
                git(&root, &["add", relative]);
                git(&root, &["commit", "--quiet", "-m", "Add nested file"]);
                fs::remove_file(root.join(relative)).unwrap();
                fs::remove_dir(root.join("nested")).unwrap();
                fs::write(
                    external.path().join("child.typ"),
                    "Different external source.",
                )
                .unwrap();
                // lstat of the final child is regular despite its symlinked
                // parent. Git correctly treats the tracked child as deleted.
                std::os::unix::fs::symlink(external.path(), root.join("nested")).unwrap();
            }
            _ => fs::write(root.join(relative), b"\0\x01\x02binary").unwrap(),
        }
        let key = app.document().key();
        let tabs = app.tabs.len();
        app.git.open(&context, &root);
        finish_panel(&mut app, &context, &root);
        app.request_git_file_diff(&context, relative.into());
        finish_panel(&mut app, &context, &root);
        finish_document(&mut app, &context, &root);
        assert!(app.document_diff.is_none());
        assert_eq!(app.document().key(), key);
        assert_eq!(app.tabs.len(), tabs);
        assert!(app.document_workflow.modal().is_none());
        assert!(app.encoding_import.is_none());
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 800.0))
            .build_ui(|ui| {
                let output = app.git.show(
                    ui,
                    false,
                    crate::settings::GitDiffStyle::Unified,
                    crate::settings::ToolbarStyle::Icons,
                );
                app.git.apply_view_output(ui.ctx(), output);
            });
        harness.run();
        let _ = harness.get_by_label(if matches!(mode, "private" | "symlink") {
            "Staged changes"
        } else {
            "Unstaged changes"
        });
    }
}

#[test]
fn delayed_file_diff_cannot_steal_an_edited_or_newly_selected_document() {
    for switch_tab in [false, true] {
        let (_directory, root) = repository();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        fs::write(
            root.join("chapter.typ"),
            "= Heading\nChanged paragraph.\nEnd.\n",
        )
        .unwrap();
        let original = root.join("other.txt");
        fs::write(&original, "Original active source.").unwrap();
        assert!(app.open_tab_path(original, &context));
        app.git.open(&context, &root);
        finish_panel(&mut app, &context, &root);
        app.request_git_file_diff(&context, "chapter.typ".into());
        let origin = app.document().key();
        assert_eq!(app.pending_git_file_origin, Some(origin));
        // Hold application admission until after a real worker reply exists.
        finish_panel(&mut app, &context, &root);
        if switch_tab {
            let next = root.join("next.txt");
            fs::write(&next, "Newly selected source.").unwrap();
            assert!(app.open_tab_path(next, &context));
        } else {
            app.document_mut().edit(CCursorRange::default(), |text| {
                text.push_str(" New unsaved edit.");
            });
        }
        let key = app.document().key();
        let source = app.document().source().to_owned();
        let tabs = app.tabs.len();
        assert_ne!(origin, key);
        app.tick_git_file_diff(&context);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.tabs.len(), tabs);
        assert!(app.pending_git_file_origin.is_none());
        assert!(app.pending_git_file_diff.is_none());
        assert!(app.document_diff.is_none());
        assert!(app.document_workflow.modal().is_none());
    }
}

#[test]
fn pending_file_diff_cannot_take_focus_from_settings_or_an_inactive_document() {
    for phase in ["worker-reply", "document-scan"] {
        for owner in ["settings", "opening-settings", "inactive-document"] {
            let (_directory, root) = repository();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.clone());
            fs::write(
                root.join("chapter.typ"),
                "= Heading\nChanged paragraph.\nEnd.\n",
            )
            .unwrap();
            let original = root.join("other.txt");
            fs::write(&original, "Original active source.").unwrap();
            assert!(app.open_tab_path(original, &context));
            app.git.open(&context, &root);
            finish_panel(&mut app, &context, &root);
            app.request_git_file_diff(&context, "chapter.typ".into());
            finish_panel(&mut app, &context, &root);
            if phase == "document-scan" {
                // Opening a tab was legitimate while the document was active;
                // completing its delayed scan must respect a new input owner.
                app.tick_git_file_diff(&context);
                assert!(app.pending_git_file_diff.is_some());
            }
            let key = app.document().key();
            let source = app.document().source().to_owned();
            let tabs = app.tabs.len();
            match owner {
                "settings" => app.settings_visible = true,
                "opening-settings" => app.settings_open_requested = true,
                "inactive-document" => {}
                _ => unreachable!(),
            }
            let mut input = egui::RawInput::default();
            if owner == "inactive-document" {
                input
                    .viewports
                    .entry(egui::ViewportId::ROOT)
                    .or_default()
                    .focused = Some(false);
            }
            context
                .run_ui(input, |ui| {
                    let focus = egui::Id::new(("file-diff-input-owner", phase, owner));
                    ui.memory_mut(|memory| memory.request_focus(focus));
                    app.tick_git_file_diff(ui.ctx());
                    assert_eq!(app.document().key(), key, "{phase}: {owner} lost its tab");
                    assert_eq!(app.document().source(), &source);
                    assert_eq!(app.tabs.len(), tabs);
                    assert!(
                        app.document_diff.is_none(),
                        "{phase}: {owner} opened a docked diff"
                    );
                    assert!(app.pending_git_file_diff.is_none());
                    assert!(app.pending_git_file_origin.is_none());
                    assert_eq!(
                        ui.memory(|memory| memory.focused()),
                        Some(focus),
                        "{phase}: {owner} lost keyboard focus"
                    );
                    assert!(app.document_workflow.modal().is_none());
                })
                .drop_without_applying_deltas();
        }
    }
}

fn finish_scan_without_admitting_diff(app: &mut EditorApp, context: &egui::Context, root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let document = &app.tabs.current_record().document;
        app.git_editor.tick(
            context,
            root,
            document.path().as_deref(),
            document.key(),
            document.source(),
            None,
        );
        if app
            .git_editor
            .current_hunks(document.key(), document.path().as_deref().unwrap())
            .is_some()
        {
            return;
        }
        assert!(Instant::now() < deadline, "Git scan did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn delayed_file_diff_respects_new_terminal_focus_within_the_document_window() {
    assert_delayed_file_diff_respects_new_owner("terminal");
}

#[test]
fn delayed_file_diff_respects_new_preview_focus_within_the_document_window() {
    for owner in ["pdfium", "tinymist"] {
        assert_delayed_file_diff_respects_new_owner(owner);
    }
}

fn assert_delayed_file_diff_respects_new_owner(owner: &str) {
    for phase in ["worker-reply", "document-scan"] {
        let (_directory, root) = repository();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        fs::write(
            root.join("chapter.typ"),
            "= Heading\nChanged paragraph.\nEnd.\n",
        )
        .unwrap();
        let original = root.join("other.typ");
        fs::write(&original, "Original active source.").unwrap();
        assert!(app.open_tab_path(original, &context));
        app.git.open(&context, &root);
        finish_panel(&mut app, &context, &root);
        app.request_git_file_diff(&context, "chapter.typ".into());
        finish_panel(&mut app, &context, &root);
        if phase == "document-scan" {
            app.tick_git_file_diff(&context);
            assert!(app.pending_git_file_diff.is_some());
            finish_scan_without_admitting_diff(&mut app, &context, &root);
        }
        let key = app.document().key();
        let tabs = app.tabs.len();
        let source = app.document().source().to_owned();
        let mut input = egui::RawInput {
            focused: true,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        context
            .run_ui(input, |ui| {
                let context = ui.ctx();
                match owner {
                    "terminal" => {
                        app.terminal.prepare_fixture(b"Terminal owns input", &root);
                        app.bottom_panel.select(PanelTab::Terminal);
                        let terminal = terminal_id(context);
                        context.memory_mut(|memory| memory.request_focus(terminal));
                        assert!(app.terminal_focused(context, egui::ViewportId::ROOT));
                    }
                    "pdfium" => {
                        app.pdfium_preview.focus_for_tests(context, "newer-preview");
                        assert!(app.pdfium_preview.has_focus(context));
                    }
                    "tinymist" => {
                        app.view_mode = ViewMode::Split;
                        app.preview = PreviewController::new(false, PreviewPreference::Interactive);
                        app.preview_controls.web.focused = true;
                        app.claim_native_preview_keyboard(context);
                        assert_eq!(
                            app.focused_preview_target(context),
                            Some(preview_controls::Target::Source)
                        );
                    }
                    _ => unreachable!(),
                }
                let focused = context.memory(|memory| memory.focused());
                app.tick_git_file_diff(context);
                assert_eq!(
                    app.document().key(),
                    key,
                    "{phase}: delayed diff switched the {owner} user's tab"
                );
                assert_eq!(app.document().source(), &source);
                assert_eq!(app.tabs.len(), tabs);
                assert!(
                    app.document_diff.is_none(),
                    "{phase}: delayed diff opened over the {owner} user's editor"
                );
                assert_eq!(
                    context.memory(|memory| memory.focused()),
                    focused,
                    "{phase}: delayed diff stole the {owner} keyboard"
                );
                if owner == "tinymist" {
                    assert!(app.preview_controls.web.focused);
                }
                assert!(app.pending_git_file_diff.is_none());
                assert!(app.pending_git_file_origin.is_none());
            })
            .drop_without_applying_deltas();
    }
}

#[test]
fn worker_file_eligibility_cannot_open_a_removed_binary_or_symlink_replacement() {
    let modes = ["removed", "binary", "invalid-utf8"]
        .into_iter()
        .chain(cfg!(unix).then_some("symlink"));
    for replacement in modes {
        let (_directory, root) = repository();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let path = root.join("chapter.typ");
        fs::write(&path, "= Heading\nChanged paragraph.\nEnd.\n").unwrap();
        let original = root.join("other.txt");
        fs::write(&original, "Original active source.").unwrap();
        assert!(app.open_tab_path(original, &context));
        app.git.open(&context, &root);
        finish_panel(&mut app, &context, &root);
        app.request_git_file_diff(&context, "chapter.typ".into());
        finish_panel(&mut app, &context, &root);
        let key = app.document().key();
        let source = app.document().source().to_owned();
        let tabs = app.tabs.len();
        // This happens after the real Git worker classified a regular UTF-8
        // file but before its result is allowed to activate a document tab.
        match replacement {
            "removed" => fs::remove_file(&path).unwrap(),
            "binary" => fs::write(&path, b"\0\x01replacement").unwrap(),
            "invalid-utf8" => fs::write(&path, b"\xffreplacement").unwrap(),
            #[cfg(unix)]
            "symlink" => {
                let target = root.join("replacement.typ");
                fs::write(&target, "A different file.").unwrap();
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(target, &path).unwrap();
            }
            _ => unreachable!(),
        }
        app.tick_git_file_diff(&context);
        assert_eq!(
            app.document().key(),
            key,
            "{replacement}: stale eligibility activated a replacement"
        );
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.tabs.len(), tabs);
        assert!(app.document_diff.is_none());
        assert!(
            app.document_workflow.modal().is_none(),
            "{replacement}: stale eligibility opened an error modal"
        );
        assert!(
            app.encoding_import.is_none(),
            "{replacement}: stale eligibility opened an encoding import"
        );
        assert!(app.pending_git_file_diff.is_none());
        assert!(app.pending_git_file_origin.is_none());
    }
}

#[test]
fn read_only_file_diff_fallback_survives_a_background_status_refresh() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let (_directory, root) = repository();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    fs::write(root.join("blob.bin"), b"\0binary").unwrap();
    app.git.open(&context, &root);
    finish_panel(&mut app, &context, &root);
    app.request_git_file_diff(&context, "blob.bin".into());
    finish_panel(&mut app, &context, &root);
    app.tick_git_file_diff(&context);
    for refreshed in [false, true] {
        if refreshed {
            app.git.request_refresh();
            finish_panel(&mut app, &context, &root);
        }
        let mut harness = Harness::builder()
            .with_size(egui::vec2(560.0, 800.0))
            .build_ui(|ui| {
                let output = app.git.show(
                    ui,
                    false,
                    crate::settings::GitDiffStyle::Unified,
                    crate::settings::ToolbarStyle::Icons,
                );
                app.git.apply_view_output(ui.ctx(), output);
            });
        harness.run();
        let _ = harness.get_by_label("Unstaged changes");
    }
    assert!(app.document_diff.is_none());
    assert!(app.document_workflow.modal().is_none());
}
