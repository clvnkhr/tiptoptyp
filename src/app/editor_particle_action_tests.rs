//! Successful application commands share source/diff particle admission.
use super::*;
use editor_particles::Action;

fn fixture(source: &str, diff: bool) -> (tempfile::TempDir, egui::Context, EditorApp) {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.settings.editor_particles.enabled = true;
    app.document_mut().replace_unprojected_untitled(source);
    app.document_mut().set_history_reset(false);
    if diff {
        app.document_diff = Some(crate::git::editor::document_diff::DocumentDiff::new(
            source.into(),
            source,
        ));
    }
    (directory, context, app)
}

fn queued(context: &egui::Context) -> Option<Action> {
    editor_particles::queued_action(context, source_editor_id(context))
}

#[test]
fn editor_action_particles_accept_successful_copy_cut_and_comment_in_both_views() {
    for diff in [false, true] {
        for action in [Action::Copy, Action::Cut, Action::Comment] {
            let (_directory, context, mut app) = fixture("alpha beta", diff);
            app.store_editor_cursor(
                &context,
                CCursorRange::two(CCursor::new(0), CCursor::new(5)),
            );
            match action {
                Action::Copy => app.copy_editor_selection(&context),
                Action::Cut => app.cut_editor_selection(&context),
                Action::Comment => app.toggle_comments(&context),
                _ => unreachable!(),
            }
            assert_eq!(queued(&context), Some(action), "{diff:?} {action:?}");
            assert_eq!(
                app.document().source().as_str(),
                match action {
                    Action::Copy => "alpha beta",
                    Action::Cut => " beta",
                    Action::Comment => "// alpha beta",
                    _ => unreachable!(),
                }
            );
        }
    }
}

#[test]
fn editor_action_particles_require_a_changed_undo_or_redo() {
    for diff in [false, true] {
        for redo in [false, true] {
            let (_directory, context, mut app) = fixture("alpha", diff);
            app.document_mut()
                .edit(CCursorRange::default(), |source| source.push_str(" beta"));
            if redo {
                assert!(
                    app.document_mut()
                        .history_step(false, CCursorRange::default())
                        .is_some()
                );
            }
            app.undo_editor(&context, redo);
            assert_eq!(
                queued(&context),
                Some(if redo { Action::Redo } else { Action::Undo })
            );
            assert_eq!(
                app.document().source().as_str(),
                if redo { "alpha beta" } else { "alpha" }
            );
        }
        for redo in [false, true] {
            let (_directory, context, mut app) = fixture("alpha", diff);
            app.undo_editor(&context, redo);
            assert_eq!(queued(&context), None, "empty history must not emit");
        }
    }
}

#[test]
fn editor_action_particles_reject_empty_selection_disabled_effects_and_paste_requests() {
    for cut in [false, true] {
        let (_directory, context, mut app) = fixture("alpha", false);
        app.store_editor_cursor(&context, CCursorRange::default());
        if cut {
            app.cut_editor_selection(&context);
        } else {
            app.copy_editor_selection(&context);
        }
        assert_eq!(
            queued(&context),
            None,
            "an empty clipboard selection must not emit"
        );
    }
    let (_directory, context, mut app) = fixture("alpha", false);
    app.settings.editor_particles.enabled = false;
    app.store_editor_cursor(
        &context,
        CCursorRange::two(CCursor::new(0), CCursor::new(5)),
    );
    app.cut_editor_selection(&context);
    assert_eq!(app.document().source().as_str(), "");
    assert_eq!(
        queued(&context),
        None,
        "disabling effects must not disable editing"
    );

    let (_directory, context, mut app) = fixture("alpha", false);
    app.request_editor_paste(&context);
    assert!(app.pending_widget_paste.is_some());
    assert_eq!(
        queued(&context),
        None,
        "clipboard delivery, not the request, owns paste feedback"
    );
}

#[test]
fn editor_action_particles_do_not_leak_between_windows_or_readonly_table_sources() {
    let (_directory, context, mut app) = fixture("#table([alpha])", false);
    let table = TableTarget::at(app.document().source(), 2, app.document().kind()).unwrap();
    app.begin_table_target(table);
    app.store_editor_cursor(
        &context,
        CCursorRange::two(CCursor::new(8), CCursor::new(13)),
    );
    app.copy_editor_selection(&context);
    assert_eq!(
        queued(&context),
        None,
        "a table-locked source must not emit"
    );

    let (_directory, context, mut app) = fixture("alpha", false);
    app.store_editor_cursor(
        &context,
        CCursorRange::two(CCursor::new(0), CCursor::new(5)),
    );
    app.copy_editor_selection(&context);
    assert_eq!(queued(&context), Some(Action::Copy));
    let other = egui::Context::default();
    assert_eq!(
        queued(&other),
        None,
        "another editor window cannot inherit a receipt"
    );

    let (_directory, context, mut app) = fixture("asset", false);
    app.document_mut().replace_loaded_unprojected(
        "asset".into(),
        PathBuf::from("asset.pdf"),
        DocumentKind::Pdf,
        None,
    );
    app.store_editor_cursor(
        &context,
        CCursorRange::two(CCursor::new(0), CCursor::new(5)),
    );
    app.copy_editor_selection(&context);
    assert_eq!(
        queued(&context),
        None,
        "preview-only assets must not emit editor feedback"
    );
}
