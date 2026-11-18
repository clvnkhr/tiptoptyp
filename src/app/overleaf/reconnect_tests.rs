//! Reconnection owns the existing working copy; storage changes are explicit.
use super::tests::{EPOCH, PROJECT, entry, fixture, park_main};
use super::*;

struct PersistentFixture {
    _directory: tempfile::TempDir,
    context: egui::Context,
    app: EditorApp,
    lease: Arc<crate::overleaf::StorageLease>,
    unused_folder: PathBuf,
}

fn persistent_fixture() -> PersistentFixture {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let lease = crate::overleaf::StorageLease::prepare(
        &project,
        &crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: Some(base.join("current")),
        },
        &base,
    )
    .unwrap();
    let root = lease.root().to_owned();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().storage = Some(Arc::clone(&lease));
    let (main, chapter) = park_main(&mut app, &root, &context);
    for (tab, entity, source) in [
        (main, "a", "Main with an unsaved draft.\n"),
        (chapter, "b", "Chapter with another unsaved draft.\n"),
    ] {
        app.tabs
            .document_mut(tab)
            .unwrap()
            .edit(CCursorRange::default(), |text| *text = source.into());
        app.overleaf.tab_entities.insert(tab, entity.into());
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .set_draft(entity, source.into())
            .unwrap();
    }
    app.settings
        .set_overleaf_storage_preference(
            project.url(),
            crate::settings::OverleafStoragePreference {
                keep_local_copy: true,
                directory: Some(base.join("current").to_string_lossy().into_owned()),
            },
        )
        .unwrap();
    app.overleaf.query = project.url().into();
    PersistentFixture {
        _directory: directory,
        context,
        app,
        lease,
        unused_folder: base.join("requested-new-folder"),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NativeTab {
    id: u64,
    key: DocumentKey,
    path: Option<PathBuf>,
    source: String,
    saved: String,
    dirty: bool,
}

fn native_tabs(app: &EditorApp) -> Vec<NativeTab> {
    app.tabs
        .ids()
        .map(|id| {
            let document = app.document_for_tab(id).unwrap();
            NativeTab {
                id,
                key: document.key(),
                path: document.path().clone(),
                source: document.source().to_owned(),
                saved: document.saved_source().to_owned(),
                dirty: document.is_dirty(),
            }
        })
        .collect()
}

fn assert_lease(app: &EditorApp, lease: &Arc<crate::overleaf::StorageLease>) {
    let connection = app.overleaf.connection.as_ref().unwrap();
    assert!(Arc::ptr_eq(connection.storage.as_ref().unwrap(), lease));
    assert_eq!(connection.root, lease.root());
    assert!(!app.overleaf.storage_job.is_running());
}

fn assert_draft_pages(app: &EditorApp, expected: &[NativeTab], workspace: &Path) {
    let actual = native_tabs(app);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.key.owner, expected.key.owner);
        assert_eq!(actual.path, expected.path);
        assert_eq!(actual.source, expected.source);
        assert_eq!(actual.saved, expected.saved);
        assert_eq!(actual.dirty, expected.dirty);
        assert_eq!(app.tab_workspace(actual.id), Some(workspace));
        let entity = &app.overleaf.tab_entities[&actual.id];
        assert_eq!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .document(entity)
                .unwrap()
                .draft(),
            Some(expected.source.as_str())
        );
    }
}

fn cancel_workspace_prompt(app: &mut EditorApp, context: &egui::Context) {
    assert!(matches!(
        app.document_workflow.take_modal(),
        Some(AppModal::Unsaved { pending, .. })
            if matches!(pending.action, DeferredDocumentAction::ApproveWorkspaceSwitch)
    ));
    app.document_workflow.clear_modal();
    app.document_workflow.cancel_continuation();
    app.cancel_workspace_switch(context);
    app.document_workflow.finish_dispatch();
    assert!(!app.document_flow_busy());
}

#[test]
fn same_project_reconnect_resumes_its_lease_despite_unapplied_storage_choices() {
    for choice in [
        "new folder",
        "temporary",
        "invalid folder",
        "pending preference",
    ] {
        let PersistentFixture {
            _directory,
            context,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        let project = app.overleaf.connection.as_ref().unwrap().project.clone();
        let mut preference = crate::settings::OverleafStoragePreference {
            keep_local_copy: choice != "temporary",
            directory: Some(unused_folder.to_string_lossy().into_owned()),
        };
        if choice == "invalid folder" {
            app.overleaf.storage_draft = Some(storage_ui::StorageDraft {
                project_url: Some(project.url().into()),
                keep_local_copy: true,
                directory: "unfinished/relative/folder".into(),
            });
        } else if choice == "pending preference" {
            let mut pending = app.settings.clone();
            pending
                .set_overleaf_storage_preference(project.url(), preference)
                .unwrap();
            app.pending_settings = Some(pending);
        } else {
            if choice == "temporary" {
                preference.directory = None;
            }
            app.overleaf.storage_draft =
                Some(storage_ui::StorageDraft::new(project.url(), &preference));
        }
        let tabs = native_tabs(&app);
        let active = app.tabs.active_id();
        let preview = app.tabs.preview_id();
        let workspace = app.workspace_root.clone();
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        app.overleaf.error = Some("An earlier bridge session failed".into());
        let old_inbox = crate::overleaf_inbox::Inbox::new(&context);
        let old_sender = old_inbox.sender();
        old_sender
            .send(format!(r#"{{"event":"document","epoch":{EPOCH},"id":"a","path":"main.tex","source":"Stale queued source"}}"#))
            .unwrap();
        app.overleaf.sender = Some(old_sender.clone());
        app.overleaf.events = Some(old_inbox);
        app.overleaf.tree_pending = app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .tree()
            .cloned();
        app.overleaf.queued.push_back(MirrorWork {
            checkpoint: false,
            entry: entry("a", "main.tex", EntryKind::Doc),
            data: MirrorData::Document("Stale queued mirror".into()),
            save: None,
        });
        app.connect_overleaf(&context).unwrap();
        assert_lease(&app, &lease);
        assert_eq!(native_tabs(&app), tabs, "{choice}");
        assert_eq!(app.tabs.active_id(), active);
        assert_eq!(app.tabs.preview_id(), preview);
        assert_eq!(app.workspace_root, workspace);
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert!(!unused_folder.exists());
        assert_eq!(app.overleaf.epoch, EPOCH + 1);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert_eq!(connection.model.epoch(), EPOCH + 1);
        assert!(!connection.online());
        assert!(connection.hydration.pending());
        assert!(app.overleaf_source_locked());
        assert!(app.overleaf.error.is_none());
        assert!(app.overleaf.queued.is_empty());
        assert!(app.overleaf.tree_pending.is_none());
        assert!(
            old_sender
                .send(r#"{"event":"status","epoch":7,"state":"connected"}"#.into())
                .is_err()
        );
        app.apply_overleaf_events(
            [Ok(BrowserEvent::Document {
                epoch: EPOCH,
                id: "a".into(),
                path: "main.tex".into(),
                source: "Stale publication after reconnect".into(),
            })],
            &context,
        );
        assert_eq!(native_tabs(&app), tabs);
        assert!(app.overleaf.queued.is_empty());
        for (entity, source) in [
            ("a", "Main with an unsaved draft.\n"),
            ("b", "Chapter with another unsaved draft.\n"),
        ] {
            assert_eq!(
                app.overleaf
                    .connection
                    .as_ref()
                    .unwrap()
                    .model
                    .document(entity)
                    .unwrap()
                    .draft(),
                Some(source)
            );
        }
    }
}

#[test]
fn applying_changed_storage_prompts_and_cancel_preserves_project_drafts_and_lease() {
    for keep_local_copy in [true, false] {
        let PersistentFixture {
            _directory,
            context,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        let project = app.overleaf.connection.as_ref().unwrap().project.clone();
        let preference = crate::settings::OverleafStoragePreference {
            keep_local_copy,
            directory: Some(unused_folder.to_string_lossy().into_owned()),
        };
        app.overleaf.storage_draft =
            Some(storage_ui::StorageDraft::new(project.url(), &preference));
        let tabs = native_tabs(&app);
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        let workspace = app.workspace_root.clone();
        let active = app.tabs.active_id();
        let preview = app.tabs.preview_id();
        app.apply_overleaf_storage(&context).unwrap();
        assert_draft_pages(&app, &tabs, &workspace);
        assert_lease(&app, &lease);
        cancel_workspace_prompt(&mut app, &context);
        assert_lease(&app, &lease);
        assert_draft_pages(&app, &tabs, &workspace);
        assert_eq!(app.tabs.active_id(), active);
        assert_eq!(app.tabs.preview_id(), preview);
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert_eq!(app.workspace_root, workspace);
        assert_eq!(app.overleaf.epoch, EPOCH);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert_eq!(connection.model.epoch(), EPOCH);
        assert!(connection.online());
        assert!(!connection.hydration.pending());
        assert!(!app.overleaf.browser.creating());
        assert!(!unused_folder.exists());
    }
}

#[test]
fn another_project_connect_or_storage_apply_prompts_and_cancel_preserves_open_drafts() {
    for apply in [false, true] {
        let PersistentFixture {
            _directory,
            context,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        app.overleaf.query = "https://www.overleaf.com/project/ffffffffffffffffffffffff".into();
        app.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(
            &app.overleaf.query,
            &crate::settings::OverleafStoragePreference {
                keep_local_copy: true,
                directory: Some(unused_folder.to_string_lossy().into_owned()),
            },
        ));
        let tabs = native_tabs(&app);
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        let workspace = app.workspace_root.clone();
        let active = app.tabs.active_id();
        let preview = app.tabs.preview_id();
        let result = if apply {
            app.apply_overleaf_storage(&context)
        } else {
            app.connect_overleaf(&context)
        };
        result.unwrap();
        assert_draft_pages(&app, &tabs, &workspace);
        assert_lease(&app, &lease);
        cancel_workspace_prompt(&mut app, &context);
        assert_lease(&app, &lease);
        assert_draft_pages(&app, &tabs, &workspace);
        assert_eq!(app.tabs.active_id(), active);
        assert_eq!(app.tabs.preview_id(), preview);
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert_eq!(app.workspace_root, workspace);
        assert_eq!(app.overleaf.epoch, EPOCH);
        assert!(app.overleaf.connection.as_ref().unwrap().online());
        assert!(!app.overleaf.browser.creating());
        assert!(!unused_folder.exists());
    }
}

#[test]
fn reset_storage_uses_the_active_lease_without_persisting_drifted_preferences_or_hiding_errors() {
    for error in [
        None,
        Some(STORAGE_CHANGE_OPEN_TABS),
        Some("Transport failed; reconnect required"),
    ] {
        let PersistentFixture {
            _directory,
            context: _,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        let project = app.overleaf.connection.as_ref().unwrap().project.clone();
        let drifted = crate::settings::OverleafStoragePreference {
            keep_local_copy: false,
            directory: Some(unused_folder.to_string_lossy().into_owned()),
        };
        let mut pending = app.settings.clone();
        pending
            .set_overleaf_storage_preference(project.url(), drifted.clone())
            .unwrap();
        app.pending_settings = Some(pending);
        app.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(project.url(), &drifted));
        app.overleaf.error = error.map(str::to_owned);
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        let tabs = native_tabs(&app);
        let default_base = Path::new("/unused/application-support");
        assert!(app.overleaf.storage_changes_pending(Some(default_base)));
        app.overleaf.reset_storage_draft(Some(default_base));
        let preference = app
            .overleaf
            .storage_draft
            .as_ref()
            .unwrap()
            .preference()
            .unwrap();
        assert!(preference.keep_local_copy);
        assert_eq!(
            crate::overleaf::StorageOptions {
                keep_local_copy: preference.keep_local_copy,
                directory: preference.directory.map(PathBuf::from),
            }
            .root_hint(&project, default_base)
            .unwrap()
            .as_deref(),
            Some(lease.root())
        );
        assert!(!app.overleaf.storage_changes_pending(Some(default_base)));
        assert_eq!(
            app.overleaf.error.as_deref(),
            error.filter(|value| *value != STORAGE_CHANGE_OPEN_TABS)
        );
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert_eq!(native_tabs(&app), tabs);
        assert_eq!(app.overleaf.epoch, EPOCH);
        assert_lease(&app, &lease);
        assert!(!unused_folder.exists());
    }
}

#[test]
fn reset_storage_for_another_project_or_nonproject_address_leaves_the_draft_and_error_untouched() {
    for query in [
        "https://www.overleaf.com/project/ffffffffffffffffffffffff",
        "https://www.overleaf.com/login",
    ] {
        let PersistentFixture {
            _directory,
            context: _,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        let draft_url = "https://www.overleaf.com/project/ffffffffffffffffffffffff";
        app.overleaf.query = query.into();
        app.overleaf.storage_draft = Some(storage_ui::StorageDraft {
            project_url: Some(draft_url.into()),
            keep_local_copy: false,
            directory: "unfinished/new-project/folder".into(),
        });
        app.overleaf.error = Some(STORAGE_CHANGE_OPEN_TABS.into());
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        let tabs = native_tabs(&app);
        app.overleaf
            .reset_storage_draft(Some(Path::new("/unused/application-support")));
        let draft = app.overleaf.storage_draft.as_ref().unwrap();
        assert_eq!(draft.project_url.as_deref(), Some(draft_url));
        assert!(!draft.keep_local_copy);
        assert_eq!(draft.directory, "unfinished/new-project/folder");
        assert_eq!(
            app.overleaf.error.as_deref(),
            Some(STORAGE_CHANGE_OPEN_TABS)
        );
        assert_eq!(app.overleaf.query, query);
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert_eq!(native_tabs(&app), tabs);
        assert_eq!(app.overleaf.epoch, EPOCH);
        assert_lease(&app, &lease);
        assert!(!unused_folder.exists());
    }
}

#[test]
fn unsynced_model_work_without_native_tabs_blocks_relocation_and_project_replacement() {
    for switch_project in [false, true] {
        let PersistentFixture {
            _directory,
            context,
            mut app,
            lease,
            unused_folder,
        } = persistent_fixture();
        let project = app.overleaf.connection.as_ref().unwrap().project.clone();
        // A publish belongs to the authoritative model until acknowledged,
        // even if there are no longer any native tab bindings to its source.
        assert!(
            app.overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .publish("a")
                .is_some()
        );
        app.tabs = super::super::tabs::Tabs::default();
        app.overleaf.tab_entities.clear();
        assert_eq!(app.tabs.ids().count(), 0);
        assert!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .has_unsynced()
        );
        if switch_project {
            app.overleaf.query = "https://www.overleaf.com/project/ffffffffffffffffffffffff".into();
            app.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(
                &app.overleaf.query,
                &crate::settings::OverleafStoragePreference {
                    keep_local_copy: true,
                    directory: Some(unused_folder.to_string_lossy().into_owned()),
                },
            ));
        } else {
            app.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(
                project.url(),
                &crate::settings::OverleafStoragePreference {
                    keep_local_copy: true,
                    directory: Some(unused_folder.to_string_lossy().into_owned()),
                },
            ));
        }
        let settings = app.settings.clone();
        let pending_settings = app.pending_settings.clone();
        let result = if switch_project {
            app.connect_overleaf(&context)
        } else {
            app.apply_overleaf_storage(&context)
        };
        assert!(
            result.is_err(),
            "unacknowledged model work must not be discarded"
        );
        assert_lease(&app, &lease);
        assert_eq!(app.settings, settings);
        assert_eq!(app.pending_settings, pending_settings);
        assert_eq!(app.overleaf.epoch, EPOCH);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(connection.model.has_unsynced());
        assert!(connection.model.in_flight().is_some());
        assert_eq!(
            connection.model.document("a").unwrap().draft(),
            Some("Main with an unsaved draft.\n")
        );
        assert!(connection.online());
        assert!(!app.overleaf.browser.creating());
        assert!(!unused_folder.exists());
    }
}
