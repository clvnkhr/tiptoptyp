//! Save requests keep their exact source owner through the correction phase.
use super::*;
use crate::tex::autofix::Rule;

const ORIGINAL: &str = "α Figure  \\ref{fig:a} and equation \n  \\eqref{eq:b}.";
const CORRECTED: &str = "α Figure~\\ref{fig:a} and equation~\\eqref{eq:b}.";

fn fixture(context: &egui::Context, root: &Path) -> (EditorApp, u64, PathBuf) {
    let path = root.join("paper.tex");
    fs::write(&path, ORIGINAL).unwrap();
    let mut app = EditorApp::dormant_for_tests(context, root.into());
    app.snapshot_scene = None;
    app.settings.auto_save = false;
    app.settings.autofix_on_save = true;
    app.settings.autofix.enabled = [Rule::ReferenceTies].into();
    app.document_mut().replace_loaded_unprojected(
        ORIGINAL.into(),
        path.clone(),
        DocumentKind::Tex,
        Some(fingerprint(ORIGINAL.as_bytes())),
    );
    app.document_mut().set_history_reset(false);
    let tab = app.tabs.active_id().unwrap();
    (app, tab, path)
}

fn prepare_ready_save(
    app: &mut EditorApp,
    tab: u64,
    path: &Path,
    intent: SaveIntent,
    context: &egui::Context,
) {
    assert!(app.prepare_autofix_save(tab, path.into(), intent, false, context));
    app.autofix.wait_until_ready_for_test().unwrap();
    assert!(!app.local_save.is_running());
}

#[test]
fn corrected_explicit_and_auto_saves_are_durable_with_one_undo_step() {
    for intent in [SaveIntent::Explicit, SaveIntent::Auto] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, tab, path) = fixture(&context, root.path());
        let original_key = app.document().key();
        prepare_ready_save(&mut app, tab, &path, intent, &context);
        assert_eq!(app.document().key(), original_key);
        assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);

        app.update_autofix(&context);
        assert_eq!(app.document().source(), CORRECTED);
        assert!(app.local_save.is_running());
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(&path).unwrap(), CORRECTED);
        assert!(!app.document().is_dirty());
        assert!(!app.document_workflow.save_in_flight());
        assert!(matches!(app.autofix, State::Idle));
        let saved_key = app.document().key();
        app.update_autofix(&context);
        app.poll_save(&context);
        assert_eq!(app.document().key(), saved_key);
        assert!(!app.local_save.is_running());

        app.document_mut()
            .history_step(false, CCursorRange::default())
            .expect("one correction transaction is undoable");
        assert_eq!(app.document().source(), ORIGINAL);
        assert!(app.document().is_dirty());
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), CORRECTED);
    }
}

#[test]
fn on_save_preference_rule_or_tool_changes_cancel_a_ready_scan_without_writing() {
    for change in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, tab, path) = fixture(&context, root.path());
        prepare_ready_save(&mut app, tab, &path, SaveIntent::Explicit, &context);
        match change {
            0 => app.settings.autofix_on_save = false,
            1 => Rule::ReferenceTies.set_enabled(&mut app.settings.autofix, false),
            2 => {
                app.tex_tools.badness.origin = crate::toolchain::ToolOrigin::Custom;
                app.tex_tools.badness.command.environment =
                    r#"{"TTT_AUTOFIX_TEST":"changed"}"#.into();
            }
            _ => unreachable!(),
        }
        let key = app.document().key();
        app.update_autofix(&context);
        app.poll_save(&context);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), ORIGINAL);
        assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
        assert!(!app.local_save.is_running());
        assert!(!app.document_workflow.save_in_flight());
        assert!(matches!(app.autofix, State::Idle));
    }
}

#[test]
fn source_changes_during_a_scan_cannot_write_the_old_or_new_source() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, tab, path) = fixture(&context, root.path());
    prepare_ready_save(&mut app, tab, &path, SaveIntent::Auto, &context);
    app.document_mut()
        .edit(CCursorRange::default(), |source| source.push_str(" newer"));
    let key = app.document().key();
    app.update_autofix(&context);
    app.poll_save(&context);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), &format!("{ORIGINAL} newer"));
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert!(!app.local_save.is_running());
    assert!(matches!(app.autofix, State::Idle));
    assert!(app.tabs.record_mut(tab).unwrap().autosave.is_some());
}

#[test]
fn a_parked_save_corrects_only_its_owner_and_preserves_active_editor_focus() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, parked, path) = fixture(&context, root.path());
    app.lifecycle = DocumentLifecycle::Active;
    app.new_tab(&context);
    app.lifecycle = DocumentLifecycle::Dormant;
    let active = app.tabs.active_id().unwrap();
    assert_ne!(parked, active);
    app.document_mut().replace_loaded_unprojected(
        "Active text stays untouched.".into(),
        root.path().join("active.txt"),
        DocumentKind::Text,
        None,
    );
    app.pending_editor_selection = None;
    let active_cursor = CCursorRange::one(CCursor::new(5));
    let mut active_editor = egui::text_edit::TextEditState::default();
    active_editor.cursor.set_char_range(Some(active_cursor));
    active_editor.store(&context, source_editor_id(&context));
    let field = egui::Id::new("autofix-save-search-field");
    context.memory_mut(|memory| memory.request_focus(field));

    let parked_cursor = CCursorRange::one(CCursor::new(ORIGINAL.chars().count()));
    let mut parked_editor = egui::text_edit::TextEditState::default();
    parked_editor.cursor.set_char_range(Some(parked_cursor));
    app.tabs.record_mut(parked).unwrap().editor = Some(parked_editor);
    prepare_ready_save(&mut app, parked, &path, SaveIntent::Explicit, &context);
    app.document_mut()
        .edit(active_cursor, |source| source.push('!'));
    app.mark_edited();
    assert!(
        matches!(app.autofix, State::Running(_)),
        "another tab's edit must not cancel this save"
    );
    let active_source = app.document().source().clone();
    let active_key = app.document().key();
    app.update_autofix(&context);
    app.finish_save_for_test(&context);

    assert_eq!(app.tabs.active_id(), Some(active));
    assert_eq!(app.document().key(), active_key);
    assert_eq!(app.document().source(), &active_source);
    assert!(context.memory(|memory| memory.has_focus(field)));
    assert_eq!(
        egui::text_edit::TextEditState::load(&context, source_editor_id(&context))
            .unwrap()
            .cursor
            .char_range(),
        Some(active_cursor)
    );
    assert!(app.pending_editor_selection.is_none());
    let record = app.tabs.record_mut(parked).unwrap();
    assert_eq!(record.document.source(), CORRECTED);
    assert!(!record.document.is_dirty());
    assert_eq!(
        record.editor.as_ref().unwrap().cursor.char_range(),
        Some(CCursorRange::one(CCursor::new(CORRECTED.chars().count())))
    );
    record
        .document
        .history_step(false, parked_cursor)
        .expect("the parked correction also remains undoable");
    assert_eq!(record.document.source(), ORIGINAL);
    assert_eq!(fs::read_to_string(path).unwrap(), CORRECTED);
}

#[test]
fn a_durable_corrected_save_releases_one_close_for_the_saved_owner() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, tab, path) = fixture(&context, root.path());
    app.document_workflow
        .continue_after_save(PendingDocumentAction {
            action: DeferredDocumentAction::CloseTab,
            key: app.document().key(),
            allow_discard: false,
            description: "closing the corrected document".into(),
        });
    prepare_ready_save(&mut app, tab, &path, SaveIntent::Explicit, &context);
    assert!(app.document_workflow.take_action().is_none());
    app.update_autofix(&context);
    app.finish_save_for_test(&context);
    assert_eq!(fs::read_to_string(path).unwrap(), CORRECTED);
    let close = app.document_workflow.take_action().unwrap();
    assert!(matches!(close.action, DeferredDocumentAction::CloseTab));
    assert_eq!(close.key, app.document().key());
    assert!(!close.allow_discard);
    assert!(!app.document().is_dirty());
    assert!(app.document_workflow.take_action().is_none());
}

#[test]
fn a_new_close_request_cannot_be_released_by_an_older_autofix_scan() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, tab, path) = fixture(&context, root.path());
    let action = PendingDocumentAction {
        action: DeferredDocumentAction::CloseTab,
        key: app.document().key(),
        allow_discard: false,
        description: "closing".into(),
    };
    app.document_workflow.continue_after_save(action.clone());
    let first_token = app.document_workflow.continuation_token();
    prepare_ready_save(&mut app, tab, &path, SaveIntent::Explicit, &context);
    app.document_workflow.cancel_continuation();
    app.document_workflow.continue_after_save(action);
    let newer_token = app.document_workflow.continuation_token();
    assert_ne!(first_token, newer_token);

    app.update_autofix(&context);
    assert_eq!(app.document_workflow.continuation_token(), newer_token);
    assert!(app.document_workflow.take_action().is_none());
    assert_eq!(app.document().source(), ORIGINAL);
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert!(!app.local_save.is_running());
    assert!(matches!(app.autofix, State::Idle));
}

#[test]
fn cancelling_a_close_save_releases_its_own_continuation_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, tab, path) = fixture(&context, root.path());
    app.document_workflow
        .continue_after_save(PendingDocumentAction {
            action: DeferredDocumentAction::CloseTab,
            key: app.document().key(),
            allow_discard: false,
            description: "closing".into(),
        });
    prepare_ready_save(&mut app, tab, &path, SaveIntent::Explicit, &context);
    app.settings.autofix_on_save = false;
    app.update_autofix(&context);
    assert!(!app.document_workflow.has_continuation());
    assert!(app.document_workflow.take_action().is_none());
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert!(!app.document_flow_busy());
}

fn dispatch_save_then_close(app: &mut EditorApp, context: &egui::Context) {
    let key = app.document().key();
    app.document_workflow.queue_action(PendingDocumentAction {
        action: DeferredDocumentAction::SaveThen(Box::new(PendingDocumentAction {
            action: DeferredDocumentAction::CloseTab,
            key,
            allow_discard: false,
            description: "closing after the corrected save".into(),
        })),
        key,
        allow_discard: false,
        description: "saving before closing".into(),
    });
    app.execute_pending_document_action(context, None);
}

#[test]
fn save_then_dispatch_and_intermediate_poll_preserve_the_preparing_close_continuation() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, _, path) = fixture(&context, root.path());
    dispatch_save_then_close(&mut app, &context);
    assert!(matches!(app.autofix, State::Running(_)));
    let token = app.document_workflow.continuation_token();
    assert!(token.is_some());
    assert!(!app.local_save.is_running());
    assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);

    // Production polls the ordinary save before receiving autofix completion.
    // That dispatch boundary must retain this exact preparing-save owner.
    app.poll_save(&context);
    assert_eq!(app.document_workflow.continuation_token(), token);
    app.autofix.wait_until_ready_for_test().unwrap();
    app.update_autofix(&context);
    app.finish_save_for_test(&context);
    assert_eq!(fs::read_to_string(path).unwrap(), CORRECTED);
    let close = app.document_workflow.take_action().unwrap();
    assert!(matches!(close.action, DeferredDocumentAction::CloseTab));
    assert_eq!(close.key, app.document().key());
    assert!(!app.document().is_dirty());
    assert!(app.document_workflow.take_action().is_none());
}

#[test]
fn a_close_preparation_cannot_become_a_parked_save_or_close_another_owner() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let (mut app, saving, path) = fixture(&context, root.path());
    app.lifecycle = DocumentLifecycle::Active;
    app.new_tab(&context);
    app.lifecycle = DocumentLifecycle::Dormant;
    let other = app.tabs.active_id().unwrap();
    app.document_mut().replace_loaded_unprojected(
        "Other owner remains open.".into(),
        root.path().join("other.txt"),
        DocumentKind::Text,
        None,
    );
    app.tabs.set_active_for_test(saving);
    dispatch_save_then_close(&mut app, &context);
    assert!(app.document_workflow.has_continuation());
    app.autofix.wait_until_ready_for_test().unwrap();

    // This directly exercises ownership even though ordinary tab controls are
    // disabled while the close continuation owns the document transaction.
    app.activate_tab(other, &context);
    let other_key = app.document().key();
    app.update_autofix(&context);
    app.poll_save(&context);
    assert_eq!(app.tabs.active_id(), Some(other));
    assert_eq!(app.document().key(), other_key);
    assert_eq!(app.document().source(), "Other owner remains open.");
    assert_eq!(app.document_for_tab(saving).unwrap().source(), ORIGINAL);
    assert_eq!(fs::read_to_string(path).unwrap(), ORIGINAL);
    assert!(!app.document_workflow.has_continuation());
    assert!(app.document_workflow.take_action().is_none());
    assert!(!app.local_save.is_running());
    assert!(!app.document_flow_busy());
    assert!(matches!(app.autofix, State::Idle));
}
