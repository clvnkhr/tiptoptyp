use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

fn draft() -> Draft {
    Draft {
        name: "Ada Example".into(),
        email: "ada@example.invalid".into(),
        origin: "https://example.invalid/research/paper.git".into(),
        branch: "main".into(),
        focus: false,
    }
}

fn form(initialized: bool, busy: bool) -> Harness<'static, (Draft, Option<Action>, github::Form)> {
    Harness::builder()
        .with_size(Vec2::new(560.0, 430.0))
        .build_ui_state(
            move |ui, state: &mut (Draft, Option<Action>, github::Form)| {
                if let Some(action) = show_form(
                    ui,
                    &mut state.0,
                    &mut state.2,
                    FormInput {
                        root: Path::new("/workspace/paper"),
                        initialized,
                        busy,
                        local_copy_pending: false,
                        existing_origin: false,
                        operation: None,
                        error: None,
                    },
                ) {
                    state.1 = Some(action);
                }
            },
            (draft(), None, github::Form::default()),
        )
}

#[test]
fn one_page_form_reviews_author_remote_and_new_branch_before_explicit_create() {
    let mut harness = form(false, false);
    harness.run();
    for label in [
        "Author name",
        "Author email",
        "Initial branch",
        "Remote URL (origin, optional)",
    ] {
        assert!(!harness.get_by_label(label).accesskit_node().is_disabled());
    }
    assert_eq!(harness.state().0.name, "Ada Example");
    assert_eq!(harness.state().0.email, "ada@example.invalid");
    assert!(harness.state().1.is_none());
    harness.get_by_label("Create repository").click();
    harness.run();
    assert_eq!(harness.state().1, Some(Action::Apply));
}

#[test]
fn existing_repository_form_cannot_change_the_current_branch() {
    let mut harness = form(true, false);
    harness.run();
    assert!(harness.query_by_label("Initial branch").is_none());
    assert!(harness.query_by_label("Create repository").is_none());
    assert_eq!(
        harness.state().0.origin,
        "https://example.invalid/research/paper.git"
    );
    harness.get_by_label("Save configuration").click();
    harness.run();
    assert_eq!(harness.state().1, Some(Action::Apply));
}

#[test]
fn setup_requires_author_and_new_branch_and_cannot_double_submit() {
    let mut harness = form(false, false);
    harness.state_mut().0.name.clear();
    harness.run();
    assert!(
        harness
            .get_by_label("Create repository")
            .accesskit_node()
            .is_disabled()
    );
    harness.state_mut().0.name = "Ada Example".into();
    harness.state_mut().0.branch.clear();
    harness.run();
    assert!(
        harness
            .get_by_label("Create repository")
            .accesskit_node()
            .is_disabled()
    );
    let mut busy = form(false, true);
    busy.step();
    assert!(
        busy.get_by_label("Create repository")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        busy.get_by_label("Author name")
            .accesskit_node()
            .is_disabled()
    );
    busy.get_by_label("Close").click();
    busy.step();
    assert_eq!(busy.state().1, Some(Action::Close));
}

fn pending(id: u64, workspace: &str, phase: Phase) -> State {
    State {
        operation: SetupOperation::test_pending(Pending {
            id,
            workspace: workspace.into(),
            phase,
        }),
        dialog: Some(Dialog {
            id,
            workspace: workspace.into(),
            snapshot: None,
            draft: draft(),
            error: None,
            edit_events: Vec::new(),
            github: github::Form::default(),
        }),
        ..State::default()
    }
}

#[test]
fn exhausted_request_identity_does_not_reuse_an_id_or_spawn_a_worker() {
    let context = egui::Context::default();
    let mut state = State {
        next: u64::MAX,
        ..State::default()
    };
    state.start("/never-inspected".into(), &context);
    assert_eq!(state.next, u64::MAX);
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Restart"))
    );
    assert!(!state.visible());
    assert!(!state.operation.is_running());
    assert!(state.operation.pending().is_none());
}

#[test]
fn one_setup_completion_records_one_status_entry_and_keeps_later_frames_idle() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let root = app.workspace_root.clone();
    app.git_setup = pending(3, root.to_str().unwrap(), Phase::Apply);
    let before = app.status_log.len();
    app.git_setup
        .operation
        .job_for_test()
        .start_and_repaint("Test setup result", &context, move || {
            Ok(Reply {
                id: 3,
                workspace: root,
                phase: Phase::Apply,
                result: Err("Review the changed Git configuration".into()),
            })
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.git_setup.operation.is_running() {
        app.poll_git_setup(&context);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.status_log.len(), before + 1);
    for _ in 0..100 {
        app.poll_git_setup(&context);
        app.record_notice_transition();
    }
    assert_eq!(app.status_log.len(), before + 1);
}

#[test]
fn unrelated_reply_cannot_retire_a_pending_setup_or_change_its_form() {
    let mut state = pending(3, "/old", Phase::Inspect);
    let outcome = state.accept(
        Reply {
            id: 2,
            workspace: "/old".into(),
            phase: Phase::Inspect,
            result: Err("stale error".into()),
        },
        Path::new("/old"),
    );
    assert!(outcome.is_none());
    assert_eq!(state.operation.pending().unwrap().id, 3);
    assert!(state.dialog.as_ref().unwrap().error.is_none());
    assert_eq!(state.dialog.as_ref().unwrap().draft.name, "Ada Example");
}

#[test]
fn late_old_workspace_failure_retains_an_outcome_without_reopening_or_overwriting_a_dialog() {
    let mut state = pending(3, "/old", Phase::Apply);
    state.dialog = None; // The user closed the modal and changed workspace.
    let outcome = state
        .accept(
            Reply {
                id: 3,
                workspace: "/old".into(),
                phase: Phase::Apply,
                result: Err("configuration changed".into()),
            },
            Path::new("/new"),
        )
        .unwrap();
    assert_eq!(outcome.workspace, Path::new("/old"));
    assert_eq!(outcome.message, "configuration changed");
    assert!(outcome.failed);
    assert!(!outcome.close_dialog);
    assert!(state.dialog.is_none());
    assert!(state.error.is_none());
}

#[test]
fn setup_owns_parent_edits_and_routes_widget_commands_to_its_own_queue() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Parent source");
    app.git_setup = pending(3, directory.path().to_str().unwrap(), Phase::Inspect);
    let key = app.document().key();
    assert!(app.document_flow_busy());
    app.execute_app_command(AppCommand::ToggleComment, &context, None);
    assert_eq!(app.document().source(), "Parent source");
    assert_eq!(app.document().key(), key);
    let child = scoped_child_viewport_id(&context, SALT);
    let mut events = Vec::new();
    assert!(app.route_edit_command_to_focused_widget_into(
        AppCommand::SelectAll,
        &context,
        child,
        &mut events
    ));
    assert!(events.is_empty());
    assert_eq!(app.git_setup.dialog.as_ref().unwrap().edit_events.len(), 1);
    app.execute_app_command(AppCommand::CloseTab, &context, None);
    assert!(!app.git_setup.visible());
    assert_eq!(app.document().key(), key);
}

#[test]
fn github_creation_requires_an_explicit_choice_and_verified_account_and_defaults_private() {
    let mut harness = form(false, false);
    harness.set_size(Vec2::new(560.0, 720.0));
    harness.run();
    assert!(harness.query_by_label("Sign in to GitHub").is_none());
    harness.get_by_label("GitHub").click();
    harness.run();
    assert!(harness.state().2.private);
    assert!(harness.state().1.is_none());
    assert!(
        harness
            .get_by_label("Create on GitHub")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label("Sign in to GitHub").click();
    harness.run();
    assert_eq!(harness.state().1, Some(Action::SignInGitHub));
    harness.state_mut().1 = None;
    harness.state_mut().2.account = Some(service::Account {
        id: 42,
        login: "example".into(),
    });
    harness.state_mut().2.name = "research-paper".into();
    harness.run();
    assert!(
        !harness
            .get_by_label("Create on GitHub")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label("Create on GitHub").click();
    harness.run();
    assert_eq!(harness.state().1, Some(Action::CreateGitHub));
    assert!(harness.state().2.private);
}

#[test]
fn recorded_github_creation_offers_connection_review_and_never_repeats_create() {
    for confirmed in [false, true] {
        let mut harness = form(true, false);
        harness.set_size(Vec2::new(560.0, 780.0));
        harness.state_mut().2.selected = true;
        harness.state_mut().2.created = Some(service::CreatedRepository::fixture(confirmed));
        harness.run();
        assert!(harness.query_by_label("Create on GitHub").is_none());
        assert!(!harness.get_by_label("Done").accesskit_node().is_disabled());
        harness.get_by_label("Retry connection").click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::AttachGitHub));
        if !confirmed {
            harness.state_mut().1 = None;
            harness.get_by_label("Review new repository").click();
            harness.run();
            assert_eq!(harness.state().1, Some(Action::ForgetGitHub));
        }
    }
}

#[test]
fn github_device_code_can_be_copied_during_sign_in_without_enabling_create() {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(560.0, 780.0))
        .build_ui_state(
            |ui, state: &mut (Draft, github::Form, Option<Action>)| {
                if let Some(action) = show_form(
                    ui,
                    &mut state.0,
                    &mut state.1,
                    FormInput {
                        root: Path::new("/workspace/paper"),
                        initialized: false,
                        busy: true,
                        local_copy_pending: false,
                        existing_origin: false,
                        operation: Some(Phase::SignIn),
                        error: None,
                    },
                ) {
                    state.2 = Some(action);
                }
            },
            (
                draft(),
                github::Form {
                    selected: true,
                    sign_in: github::SignInPhase::DeviceCode {
                        code: "ABCD-1234".into(),
                        uri: "https://github.com/login/device".into(),
                    },
                    ..Default::default()
                },
                None,
            ),
        );
    harness.run();
    assert!(
        !harness
            .get_by_label("Copy code")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !harness
            .get_by_label("Open GitHub")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        harness
            .get_by_label("Create on GitHub")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label("Copy code").click();
    harness.step();
    assert!(harness.output().platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "ABCD-1234")
    ));
    assert!(harness.state().2.is_none());
}

#[test]
fn github_progress_is_bounded_and_bound_to_the_current_request_and_workspace() {
    let mut state = pending(4, "/working-copy", Phase::SignIn);
    state.dialog.as_mut().unwrap().github.begin_sign_in();
    let (sender, receiver) = mpsc::sync_channel(8);
    state.operation.set_progress_for_test(receiver);
    sender
        .try_send((
            3,
            service::Progress::DeviceCode {
                code: "OLD-CODE".into(),
                verification_uri: "https://github.com/login/device".into(),
            },
        ))
        .unwrap();
    state.poll_progress(Path::new("/working-copy"));
    assert!(state.dialog.as_ref().unwrap().github.code().is_none());
    sender
        .try_send((
            4,
            service::Progress::DeviceCode {
                code: "ABCD-1234".into(),
                verification_uri: "https://github.com/login/device".into(),
            },
        ))
        .unwrap();
    state.poll_progress(Path::new("/another-workspace"));
    assert!(state.dialog.as_ref().unwrap().github.code().is_none());
    sender
        .try_send((
            4,
            service::Progress::DeviceCode {
                code: "ABCD-1234".into(),
                verification_uri: "https://github.com/login/device".into(),
            },
        ))
        .unwrap();
    state.poll_progress(Path::new("/working-copy"));
    assert_eq!(
        state.dialog.as_ref().unwrap().github.code().unwrap().0,
        "ABCD-1234"
    );
    state.cancel_github();
    assert!(
        state
            .operation
            .cancellation_for_test()
            .load(Ordering::Acquire)
    );
    assert!(state.dialog.as_ref().unwrap().github.code().is_none());
}

#[test]
fn github_device_code_belongs_only_to_an_active_sign_in_phase() {
    let progress = || service::Progress::DeviceCode {
        code: "ABCD-1234".into(),
        verification_uri: "https://github.com/login/device".into(),
    };
    let mut form = github::Form::default();
    form.progress(progress());
    assert!(
        form.code().is_none(),
        "unowned progress cannot create a sign-in challenge"
    );
    form.begin_sign_in();
    assert_eq!(form.stage(), "preparing");
    form.progress(progress());
    form.progress(service::Progress::WaitingForBrowser);
    assert_eq!(form.stage(), "waiting");
    assert_eq!(form.code().unwrap().0, "ABCD-1234");
    form.cancel_sign_in();
    assert_eq!(form.stage(), "cancelling");
    assert!(form.code().is_none());
    form.progress(progress());
    assert!(
        form.code().is_none(),
        "late progress cannot revive a cancelled challenge"
    );
    form.finish_sign_in();
    assert_eq!(form.stage(), "idle");
    assert!(form.code().is_none());
}

#[test]
fn closing_github_cancels_its_worker_but_local_configuration_can_finish() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    for phase in [
        Phase::Apply,
        Phase::SignIn,
        Phase::CreateGitHub,
        Phase::AttachGitHub,
    ] {
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let key = app.document().key();
        app.git_setup = pending(3, directory.path().to_str().unwrap(), phase);
        let cancel = app.git_setup.operation.cancellation_for_test().clone();
        app.close_git_setup(&context);
        assert_eq!(cancel.load(Ordering::Acquire), phase != Phase::Apply);
        assert!(!app.git_setup.visible());
        assert_eq!(app.document().key(), key);
    }
}

#[test]
fn github_attachment_refreshes_existing_remote_fields_instead_of_later_clearing_origin() {
    let directory = tempfile::tempdir().unwrap();
    let mut snapshot = setup::inspect(directory.path()).unwrap();
    let root = snapshot.workspace.clone();
    snapshot.initialized = true;
    snapshot.author_name = "Ada Example".into();
    snapshot.author_email = "ada@example.invalid".into();
    snapshot.branch = "paper-backup".into();
    snapshot.origin = "https://github.com/example/paper.git".into();
    let mut state = pending(3, root.to_str().unwrap(), Phase::AttachGitHub);
    state.dialog.as_mut().unwrap().draft.origin.clear();
    let outcome = state
        .accept(
            Reply {
                id: 3,
                workspace: root.clone(),
                phase: Phase::AttachGitHub,
                result: Ok(Outcome::AttachedGitHub(
                    service::AttachedRepository::fixture(snapshot.clone()),
                )),
            },
            &root,
        )
        .unwrap();
    assert!(!outcome.failed);
    let dialog = state.dialog.as_mut().unwrap();
    dialog.github.selected = false;
    assert_eq!(dialog.draft.origin, snapshot.origin);
    assert_eq!(dialog.draft.branch, snapshot.branch);
    assert_eq!(
        state.request().unwrap().origin,
        "https://github.com/example/paper.git"
    );
}

#[test]
fn reopening_connected_or_reconfigured_repository_preserves_the_current_remote_workflow() {
    let directory = tempfile::tempdir().unwrap();
    for connected in [true, false] {
        let mut snapshot = setup::inspect(directory.path()).unwrap();
        let root = snapshot.workspace.clone();
        let created = service::CreatedRepository::fixture(true);
        snapshot.initialized = true;
        snapshot.origin = if connected {
            created.clone_url.clone()
        } else {
            "https://example.invalid/new-origin.git".into()
        };
        let current_origin = snapshot.origin.clone();
        let mut state = pending(3, root.to_str().unwrap(), Phase::Inspect);
        assert!(
            state
                .accept(
                    Reply {
                        id: 3,
                        workspace: root.clone(),
                        phase: Phase::Inspect,
                        result: Ok(Outcome::Inspected {
                            snapshot,
                            created: Some(created)
                        }),
                    },
                    &root
                )
                .is_none()
        );
        let dialog = state.dialog.as_ref().unwrap();
        assert!(
            !dialog.github.selected,
            "an existing destination stays in the usual remote configuration workflow"
        );
        assert_eq!(dialog.draft.origin, current_origin);
        assert_eq!(
            dialog.github.created.as_ref().unwrap().snapshot.is_ok(),
            connected
        );
        assert!(dialog.error.is_none());
    }
}
