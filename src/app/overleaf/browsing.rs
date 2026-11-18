//! Ordinary website browsing never grants authority over native project files.
use super::*;

impl State {
    pub(super) fn acknowledge_address_focus(&mut self, native_focused: Option<bool>) {
        self.focus = self.focus && native_focused != Some(true);
    }

    pub(super) fn storage_controls_visible(&self) -> bool {
        self.pending_storage.is_some() || ProjectUrl::parse(&self.query).is_ok()
    }

    /// Toolbar/storage focus changes keep an unsubmitted target. A real page
    /// interaction releases it, even if egui has not observed URL blur yet.
    fn address_editing(&mut self, browser_owns_keyboard: bool) -> bool {
        if browser_owns_keyboard {
            self.address_draft = false;
            return false;
        }
        self.url_editing || self.address_draft
    }

    fn accept_navigation_address(&mut self, address: &str, editing: bool) {
        if !editing {
            address.clone_into(&mut self.query);
        }
    }

    /// Opening a browser is not a reconnect and must preserve pending saves.
    pub(super) fn ensure_browsing(&mut self, context: &egui::Context) {
        if self.browser.is_some() || self.browser.creating() || self.storage_job.is_running() {
            return;
        }
        if self.events.is_none() || self.sender.is_none() {
            let inbox = crate::overleaf_inbox::Inbox::new(context);
            self.sender = Some(inbox.sender());
            self.events = Some(inbox);
        }
        self.browser_profile
            .get_or_insert_with(crate::overleaf::workspace_session::new_profile_id);
        self.browser_url = crate::overleaf_browser::LOGIN_URL.into();
        self.query.clone_from(&self.browser_url);
        self.browser.begin();
        self.show_browser = true;
    }
}

/// A stale egui focus flag cannot take the keyboard back from a clicked page.
/// Native construction can change the responder after the field was rendered.
pub(super) fn address_handoff(requested: bool, created: bool, editing: bool) -> bool {
    requested || created && editing
}

fn discovery_ready(
    navigation: &crate::overleaf_browser::Navigation,
    query: &str,
    editing: bool,
    busy: bool,
    current: Option<(&ProjectUrl, bool)>,
    awaiting: bool,
) -> bool {
    let Some(project) = &navigation.project else {
        return false;
    };
    if busy || current.is_some_and(|(current, online)| current == project && (online || awaiting)) {
        return false;
    }
    // Native callbacks cannot replace an address the user is still composing.
    !editing
        || ProjectUrl::parse(query).is_ok_and(|draft| draft == *project)
        || crate::overleaf_browser::navigation_url(query).is_ok_and(|draft| draft == navigation.url)
}

impl EditorApp {
    pub(super) fn navigate_overleaf(&mut self, context: &egui::Context) -> Result<(), String> {
        if ProjectUrl::parse(&self.overleaf.query).is_ok() {
            return self.connect_overleaf(context);
        }
        let address = crate::overleaf_browser::navigation_url(&self.overleaf.query)?;
        if let Some(browser) = self.overleaf.browser.as_ref() {
            browser.navigate(&address)?;
        } else {
            self.overleaf.ensure_browsing(context);
            self.overleaf.query.clone_from(&address);
        }
        self.overleaf.browser_url = address;
        self.overleaf.address_draft = false;
        self.overleaf.show_browser = true;
        self.overleaf.error = None;
        Ok(())
    }

    fn handle_overleaf_sign_in(
        &mut self,
        navigation: &crate::overleaf_browser::Navigation,
        context: &egui::Context,
    ) {
        use crate::overleaf_browser::SignIn;
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        let epoch = connection.model.epoch();
        if navigation.sign_in == SignIn::None {
            return;
        }
        if navigation.sign_in == SignIn::ResumeProject {
            // Keep the retry action pointed at the bound project even when
            // automatic return stops on the dashboard or sign-in page.
            self.overleaf.query = connection.project.url().into();
        }
        self.overleaf.authentication.set_required(
            navigation.sign_in != SignIn::ResumeProject
                || self.overleaf.authentication.return_attempted(),
        );
        self.overleaf.await_project();
        if self.overleaf.authentication.required() {
            self.overleaf.show_browser = true;
        }
        let (state, message) = if self.overleaf.authentication.required() {
            (
                "signed-out",
                "Sign in to Overleaf to continue. Your local draft is retained.",
            )
        } else {
            ("loading", "Reopening the Overleaf project after sign-in…")
        };
        self.apply_overleaf_events(
            [Ok(BrowserEvent::Status {
                epoch,
                state: state.into(),
                message: Some(message.into()),
            })],
            context,
        );
        match navigation.sign_in {
            SignIn::ResumeProject if !self.overleaf.authentication.return_attempted() => {
                // A new native view reuses browser-owned session storage while
                // retiring every pre-login inbox message and mirror receipt.
                self.overleaf.authentication.mark_return_attempted();
                if let Err(error) = self.resume_overleaf(context) {
                    self.overleaf.authentication.set_required(true);
                    self.overleaf.cancel_adoption();
                    self.overleaf.show_browser = true;
                    self.overleaf.error = Some(error);
                }
            }
            SignIn::ResumeProject => {
                self.overleaf.cancel_adoption();
                self.overleaf.error = Some("Overleaf did not reopen this project after sign-in. Check project access, then Reconnect.".into());
            }
            SignIn::Required | SignIn::None => {}
        }
    }

    pub(super) fn poll_overleaf_browsing(&mut self, context: &egui::Context) {
        let Some(browser) = self.overleaf.browser.as_ref() else {
            return;
        };
        let browser_owns_keyboard = browser.owns_keyboard();
        let navigation = browser.take_navigation();
        let editing = self.overleaf.address_editing(browser_owns_keyboard);
        let Some(navigation) = navigation else {
            return;
        };
        let sign_in = navigation.sign_in;
        if sign_in == crate::overleaf_browser::SignIn::ResumeProject
            && editing
            && self.overleaf.connection.as_ref().is_some_and(|connection| {
                ProjectUrl::parse(&self.overleaf.query).as_ref().ok() != Some(&connection.project)
            })
        {
            if let Some(browser) = self.overleaf.browser.as_ref() {
                browser.cancel_sign_in_return();
            }
            return;
        }
        if sign_in != crate::overleaf_browser::SignIn::None {
            self.handle_overleaf_sign_in(&navigation, context);
            if sign_in != crate::overleaf_browser::SignIn::Required {
                return;
            }
        }
        let connect = discovery_ready(
            &navigation,
            &self.overleaf.query,
            editing,
            self.overleaf.storage_job.is_running(),
            self.overleaf.connection.as_ref().map(|connection| {
                (
                    &connection.project,
                    connection.online() && !self.overleaf.authentication.required(),
                )
            }),
            self.overleaf.awaiting_project(),
        );
        self.overleaf
            .accept_navigation_address(&navigation.url, editing);
        if connect {
            let project = navigation.project.unwrap();
            self.overleaf.query = project.url().into();
            if let Err(error) = self.connect_overleaf(context) {
                self.overleaf.error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn navigation() -> crate::overleaf_browser::Navigation {
        let project =
            ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap();
        crate::overleaf_browser::Navigation {
            url: project.url().into(),
            project: Some(project),
            sign_in: crate::overleaf_browser::SignIn::None,
        }
    }

    #[test]
    fn delayed_login_after_url_blur_preserves_the_project_and_storage_draft() {
        let project = navigation().url;
        let mut state = State {
            query: project.clone(),
            url_editing: true,
            address_draft: true,
            storage_draft: Some(storage_ui::StorageDraft::new(
                &project,
                &crate::settings::OverleafStoragePreference {
                    keep_local_copy: true,
                    directory: Some("/projects/local-copies".into()),
                },
            )),
            ..State::default()
        };
        // Clicking storage controls blurs the URL while native input stays
        // with the parent. The initial login commit can arrive on either
        // control's next frame without cancelling the unsubmitted project.
        state.url_editing = false;
        for _ in 0..3 {
            let editing = state.address_editing(false);
            state.accept_navigation_address(crate::overleaf_browser::LOGIN_URL, editing);
            assert!(editing);
            assert_eq!(state.query, project);
            assert!(state.storage_controls_visible());
            let storage = state.storage_draft.as_ref().unwrap();
            assert_eq!(storage.project_url.as_deref(), Some(project.as_str()));
            assert!(storage.keep_local_copy);
            assert_eq!(storage.directory, "/projects/local-copies");
        }
    }

    #[test]
    fn real_browser_input_releases_the_address_draft_without_native_focus_repair() {
        let mut state = State {
            query: navigation().url,
            url_editing: true, // egui focus can lag native page input.
            address_draft: true,
            ..State::default()
        };
        let editing = state.address_editing(true);
        assert!(!editing);
        assert!(!state.address_draft);
        state.accept_navigation_address(crate::overleaf_browser::LOGIN_URL, editing);
        assert_eq!(state.query, crate::overleaf_browser::LOGIN_URL);
        assert!(!address_handoff(false, false, editing));
        state.url_editing = false;
        assert!(!state.address_editing(false));
    }

    #[test]
    fn staged_workspace_target_survives_browser_startup_and_storage_focus() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let project = navigation().url;
        app.stage_overleaf_workspace(directory.path(), &context);
        app.overleaf.query = project.clone();
        app.overleaf
            .ensure_storage_draft(Some(&ProjectUrl::parse(&project).unwrap()), &app.settings)
            .unwrap();
        app.overleaf.address_draft = true;
        assert!(app.overleaf.address_draft);
        app.overleaf.url_editing = false;
        let editing = app.overleaf.address_editing(false);
        app.overleaf
            .accept_navigation_address(crate::overleaf_browser::LOGIN_URL, editing);
        assert_eq!(app.overleaf.query, project);
        assert_eq!(
            app.overleaf.storage_draft.as_ref().unwrap().directory,
            directory.path().to_string_lossy()
        );
        assert!(app.overleaf.connection.is_none());
    }

    #[test]
    fn address_open_request_waits_for_native_focus_acknowledgement_then_stays_retired() {
        let mut state = State {
            focus: true,
            ..State::default()
        };
        for native in [None, Some(false), None, Some(false)] {
            state.acknowledge_address_focus(native);
            assert!(
                state.focus,
                "dispatching a native focus request is not an acknowledgement"
            );
        }
        state.acknowledge_address_focus(Some(true));
        assert!(!state.focus);
        for native in [None, Some(false), Some(true)] {
            state.acknowledge_address_focus(native);
            assert!(
                !state.focus,
                "idle focus changes must not restart a completed open request"
            );
        }
    }

    #[test]
    fn native_address_focus_is_explicit_and_is_restored_after_browser_construction() {
        for editing in [false, true] {
            assert!(address_handoff(true, false, editing));
            assert!(address_handoff(true, true, editing));
            assert!(
                !address_handoff(false, false, editing),
                "idle egui focus cannot steal native browser typing"
            );
        }
        assert!(address_handoff(false, true, true));
        assert!(!address_handoff(false, true, false));
    }

    #[test]
    fn standalone_login_does_not_connect_or_change_save_ownership() {
        let context = egui::Context::default();
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        let operation = crate::workflow::DocumentWorkflow::default()
            .begin_save(key, crate::workflow::SaveDomain::Overleaf)
            .unwrap();
        let mut state = State {
            save: SavePhase::WritingMirror {
                entity: "retained-save".into(),
                operation,
            },
            epoch: 7,
            query: navigation().url,
            ..State::default()
        };
        state.ensure_browsing(&context);
        assert!(state.browser.creating() && state.show_browser);
        assert_eq!(state.browser_url, crate::overleaf_browser::LOGIN_URL);
        assert_eq!(
            state.query, state.browser_url,
            "a fresh browser cannot show an old project address over the login page"
        );
        assert!(state.connection.is_none());
        assert!(!state.awaiting_project());
        assert_eq!(state.epoch, 7);
        assert!(state.save.active());
        assert_eq!(state.save.entity(), Some("retained-save"));
        let sender = state.sender.clone().unwrap();
        state.ensure_browsing(&context);
        assert!(
            sender
                .send(serde_json::json!({"event":"status","epoch":7,"state":"loading"}).to_string())
                .is_ok()
        );
        assert!(
            state.events.is_some(),
            "reopening does not detach the existing inbox"
        );
    }

    #[test]
    fn explicit_go_without_a_browser_retains_its_accepted_address() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.overleaf.query = "http://localhost:43210/login".into();
        app.overleaf.address_draft = true;
        app.navigate_overleaf(&context).unwrap();
        assert_eq!(app.overleaf.query, "http://localhost:43210/login");
        assert_eq!(app.overleaf.browser_url, app.overleaf.query);
        assert!(app.overleaf.browser.creating());
        assert!(app.overleaf.connection.is_none());
        assert_eq!(app.overleaf.epoch, 0);
        assert!(!app.overleaf.awaiting_project());
        assert!(!app.overleaf.address_draft);
    }

    #[test]
    fn storage_controls_only_belong_to_canonical_project_addresses() {
        let mut state = State::default();
        for address in [
            crate::overleaf_browser::LOGIN_URL,
            "https://example.org",
            "javascript:alert(1)",
            "https://www.overleaf.com/project/0123456789abcdef01234567?doc=abcdef0123456789abcdef01",
        ] {
            state.query = address.into();
            assert!(!state.storage_controls_visible(), "{address}");
        }
        state.query = navigation().url;
        assert!(state.storage_controls_visible());
    }

    #[test]
    fn project_discovery_is_not_a_reconnect_loop_or_a_typed_address_override() {
        let navigation = navigation();
        let project = navigation.project.as_ref().unwrap();
        assert!(discovery_ready(
            &navigation,
            crate::overleaf_browser::LOGIN_URL,
            false,
            false,
            None,
            false
        ));
        assert!(!discovery_ready(
            &navigation,
            crate::overleaf_browser::LOGIN_URL,
            true,
            false,
            None,
            false
        ));
        assert!(discovery_ready(
            &navigation,
            project.url(),
            true,
            false,
            None,
            false
        ));
        assert!(!discovery_ready(
            &navigation,
            project.url(),
            false,
            true,
            None,
            false
        ));
        assert!(!discovery_ready(
            &navigation,
            project.url(),
            false,
            false,
            Some((project, true)),
            false
        ));
        assert!(!discovery_ready(
            &navigation,
            project.url(),
            false,
            false,
            Some((project, false)),
            true
        ));
        assert!(discovery_ready(
            &navigation,
            project.url(),
            false,
            false,
            Some((project, false)),
            false
        ));
        let ordinary = crate::overleaf_browser::Navigation {
            url: crate::overleaf_browser::LOGIN_URL.into(),
            project: None,
            sign_in: crate::overleaf_browser::SignIn::None,
        };
        assert!(!discovery_ready(
            &ordinary,
            project.url(),
            false,
            false,
            None,
            false
        ));
    }

    #[test]
    fn first_open_uses_login_instead_of_recent_project_and_never_starts_storage() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.settings.overleaf_projects = vec![navigation().url];
        app.begin_overleaf(&context);
        assert_eq!(app.overleaf.query, crate::overleaf_browser::LOGIN_URL);
        assert_eq!(app.overleaf.browser_url, crate::overleaf_browser::LOGIN_URL);
        assert_eq!(app.overleaf.epoch, 0);
        assert!(app.overleaf.browser.creating());
        assert!(app.overleaf.connection.is_none());
        assert!(!app.overleaf.storage_job.is_running());
    }

    struct DirtyAuthFixture {
        _directory: tempfile::TempDir,
        context: egui::Context,
        app: EditorApp,
        selection: CCursorRange,
    }

    fn dirty_auth_fixture() -> DirtyAuthFixture {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = super::super::tests::fixture(&root, &context);
        app.document_mut().set_history_reset(false);
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = "Main locally edited\n".into();
        });
        let source = app.document().source().clone();
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.model.set_draft("a", source).unwrap();
        assert!(connection.model.publish("a").is_some());
        let selection = CCursorRange {
            primary: CCursor::new(7),
            secondary: CCursor::new(2),
            ..Default::default()
        };
        app.store_editor_cursor(&context, selection);
        assert!(!app.overleaf_source_locked());
        DirtyAuthFixture {
            _directory: directory,
            context,
            app,
            selection,
        }
    }

    #[test]
    fn required_sign_in_locks_the_existing_dirty_project_without_replacing_its_owner() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            selection,
        } = dirty_auth_fixture();
        let source = app.document().source().clone();
        let saved = app.document().saved_source().to_owned();
        let path = app.document().path().clone();
        let key = app.document().key();
        let tab = app.tabs.active_id();
        let workspace = app.workspace_root.clone();
        let connection = app.overleaf.connection.as_ref().unwrap();
        let root = connection.root.clone();
        let tree = connection.model.tree().cloned();
        let draft = connection.model.document("a").cloned();
        let in_flight = connection.model.in_flight().cloned();
        let epoch = connection.model.epoch();
        app.overleaf.visible = false;
        app.overleaf.show_browser = false;
        let editor = source_editor_id(&context);
        context.memory_mut(|memory| memory.request_focus(editor));
        app.handle_overleaf_sign_in(
            &crate::overleaf_browser::Navigation {
                url: "https://www.overleaf.com/login".into(),
                project: None,
                sign_in: crate::overleaf_browser::SignIn::Required,
            },
            &context,
        );
        assert!(app.overleaf.authentication.required());
        assert!(
            app.overleaf.show_browser,
            "reopening the manager must expose the login page"
        );
        assert!(app.overleaf.awaiting_project());
        assert!(app.overleaf_source_locked());
        assert!(context.memory(|memory| memory.has_focus(editor)));
        assert!(
            !app.overleaf.focus,
            "sign-in notification must not steal native address focus"
        );
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(!connection.online());
        assert!(connection.hydration.pending());
        assert_eq!(connection.root, root);
        assert_eq!(connection.model.epoch(), epoch);
        assert_eq!(connection.model.tree(), tree.as_ref());
        assert_eq!(connection.model.document("a"), draft.as_ref());
        assert_eq!(connection.model.in_flight(), in_flight.as_ref());
        assert_eq!(app.workspace_root, workspace);
        assert_eq!(app.tabs.active_id(), tab);
        assert_eq!(app.document().path(), &path);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().saved_source(), &saved);
        assert_eq!(app.document().key(), key);
        assert!(app.document().is_dirty());
        let cursor = app.editor_snapshot(&context).cursor;
        assert_eq!(cursor.primary.index, selection.primary.index);
        assert_eq!(cursor.secondary.index, selection.secondary.index);
        assert!(matches!(
            app.overleaf.browser_status,
            activity::BrowserStatus::SignedOut
        ));
        assert!(!app.overleaf.storage_job.is_running());
        assert!(!app.overleaf.browser.creating());
        assert!(app.overleaf.browser.is_none());
    }

    #[test]
    fn failed_sign_in_return_is_bounded_and_retains_the_dirty_model_and_editor() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            selection,
        } = dirty_auth_fixture();
        let source = app.document().source().clone();
        let key = app.document().key();
        let root = app.overleaf.connection.as_ref().unwrap().root.clone();
        let model = app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .cloned();
        let epoch = app.overleaf.epoch;
        app.overleaf.authentication.mark_return_attempted();
        let navigation = crate::overleaf_browser::Navigation {
            url: "https://www.overleaf.com/project".into(),
            project: None,
            sign_in: crate::overleaf_browser::SignIn::ResumeProject,
        };
        let mut message = None;
        for _ in 0..3 {
            app.handle_overleaf_sign_in(&navigation, &context);
            let error = app
                .overleaf
                .error
                .as_ref()
                .expect("failed return stays visible");
            assert!(error.contains("Check project access"));
            assert!(error.len() < 256);
            if let Some(message) = &message {
                assert_eq!(error, message);
            } else {
                message = Some(error.clone());
            }
            assert!(!app.overleaf.awaiting_project());
            assert!(app.overleaf_source_locked());
            assert_eq!(
                app.overleaf.query,
                app.overleaf.connection.as_ref().unwrap().project.url()
            );
            assert!(
                app.overleaf.storage_controls_visible(),
                "manual Reconnect remains available"
            );
            assert!(!app.overleaf.browser.creating());
            assert!(!app.overleaf.storage_job.is_running());
            assert!(!app.overleaf.io.is_running());
            assert_eq!(app.overleaf.epoch, epoch);
            let connection = app.overleaf.connection.as_ref().unwrap();
            assert_eq!(connection.root, root);
            assert_eq!(connection.model.document("a"), model.as_ref());
            assert_eq!(app.document().source(), &source);
            assert_eq!(app.document().key(), key);
            let cursor = app.editor_snapshot(&context).cursor;
            assert_eq!(cursor.primary.index, selection.primary.index);
            assert_eq!(cursor.secondary.index, selection.secondary.index);
        }
        // A failed return did not establish a new epoch. Delayed readiness from
        // the old page must not remove the independent authentication barrier.
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.transport = Transport::Online;
        connection.hydration = Hydration::default();
        assert!(app.overleaf_source_locked());
    }

    #[test]
    fn sign_in_requirement_blocks_source_edits_even_with_a_stale_online_ready_snapshot() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            ..
        } = dirty_auth_fixture();
        app.overleaf.authentication.set_required(true);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(connection.online());
        assert!(!connection.hydration.pending());
        assert!(app.overleaf_source_locked());
        let source = app.document().source().clone();
        let key = app.document().key();
        app.insert_editor_text(&context, "must not replace selected text");
        app.find_bar.query = "Main".into();
        app.find_bar.replacement = "Changed".into();
        app.apply_find_actions(&context, false, false, true, true);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().key(), key);
        assert!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .has_unsynced()
        );
    }

    #[test]
    fn sign_in_requirement_cannot_be_cleared_by_stale_completed_hydration() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            ..
        } = dirty_auth_fixture();
        app.overleaf.authentication.set_required(true);
        let connection = app.overleaf.connection.as_mut().unwrap();
        let tree = connection.model.tree().unwrap().clone();
        connection.hydration = Hydration::waiting();
        connection.hydration.reset(&tree);
        connection.hydration.accept_tree(&tree);
        for entry in tree.entries() {
            connection.hydration.accept_file(entry);
        }
        assert!(connection.online(), "simulate delayed pre-login readiness");
        app.finish_overleaf_hydration(&context);
        assert!(app.overleaf.authentication.required());
        assert!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .hydration
                .pending()
        );
        assert!(app.overleaf_source_locked());
        // The same fully written tree remains sufficient once fresh sign-in
        // ownership has removed the independent authentication barrier.
        app.overleaf.authentication.set_required(false);
        app.finish_overleaf_hydration(&context);
        assert!(
            !app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .hydration
                .pending()
        );
        assert!(!app.overleaf_source_locked());
    }

    #[test]
    fn automatic_login_return_failure_keeps_authentication_barrier() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            selection,
        } = dirty_auth_fixture();
        let source = app.document().source().clone();
        let key = app.document().key();
        app.overleaf.epoch = u64::MAX;
        app.handle_overleaf_sign_in(
            &crate::overleaf_browser::Navigation {
                url: "https://www.overleaf.com/project".into(),
                project: None,
                sign_in: crate::overleaf_browser::SignIn::ResumeProject,
            },
            &context,
        );
        assert!(
            app.overleaf
                .error
                .as_ref()
                .unwrap()
                .contains("generation exhausted")
        );
        assert!(app.overleaf.authentication.return_attempted());
        assert!(app.overleaf.authentication.required());
        assert!(!app.overleaf.awaiting_project());
        assert!(!app.overleaf.browser.creating());
        assert_eq!(app.overleaf.epoch, u64::MAX);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().key(), key);
        let cursor = app.editor_snapshot(&context).cursor;
        assert_eq!(cursor.primary.index, selection.primary.index);
        assert_eq!(cursor.secondary.index, selection.secondary.index);
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.transport = Transport::Online;
        connection.hydration = Hydration::default();
        assert!(app.overleaf_source_locked());
    }

    #[test]
    fn ordinary_browser_go_does_not_clear_a_project_authentication_barrier() {
        let DirtyAuthFixture {
            _directory,
            context,
            mut app,
            ..
        } = dirty_auth_fixture();
        app.overleaf.authentication.set_required(true);
        app.overleaf.query = "https://www.overleaf.com/login#continue".into();
        let epoch = app.overleaf.epoch;
        let source = app.document().source().clone();
        app.navigate_overleaf(&context).unwrap();
        assert!(app.overleaf.authentication.required());
        assert!(app.overleaf_source_locked());
        assert_eq!(app.overleaf.epoch, epoch);
        assert_eq!(app.document().source(), &source);
        assert!(app.overleaf.connection.as_ref().unwrap().online());
        assert!(
            !app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .hydration
                .pending()
        );
    }
}
