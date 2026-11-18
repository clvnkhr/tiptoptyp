use super::*;
use crate::overleaf::{BrowserCommand, BrowserEvent, ModelChange};
use std::{
    os::unix::fs::{MetadataExt, symlink},
    path::PathBuf,
};

const PROJECT: &str = "https://www.overleaf.com/project/0123456789abcdef01234567";
fn project() -> ProjectUrl {
    ProjectUrl::parse(PROJECT).unwrap()
}
fn make_entry(id: &str, path: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        path: path.into(),
        kind,
    }
}
fn tree(entries: Vec<Entry>) -> Tree {
    let main = entries
        .iter()
        .find(|entry| entry.kind == EntryKind::Doc)
        .map(|entry| entry.id.clone());
    Tree::new("Offline paper".into(), main, entries).unwrap()
}
fn document(source: &str) -> Document {
    Document::new(source.into())
}
fn journal_checkpoint(mirror: &Mirror, entry: &Entry) -> [u8; 32] {
    source_hash(&fs::read(mirror.root.join(document_path(&entry.id))).unwrap())
}
fn fixture() -> (tempfile::TempDir, Mirror, Entry) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let mirror = Mirror::open(&root).unwrap();
    let entry = make_entry("doc", "main.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![entry.clone()]))
        .unwrap();
    (temporary, mirror, entry)
}
fn snapshot(epoch: u64, entry: &Entry, source: &str) -> BrowserEvent {
    BrowserEvent::Document {
        epoch,
        id: entry.id.clone(),
        path: entry.path.clone(),
        source: source.into(),
    }
}
fn remote_model(epoch: u64, entry: &Entry, source: &str) -> Model {
    let project = project();
    let mut model = Model::for_project(epoch, &project);
    model
        .apply(BrowserEvent::Project {
            epoch,
            project_id: project.id().into(),
            name: "Paper".into(),
            root_doc: Some(entry.id.clone()),
            entries: vec![entry.clone()],
        })
        .unwrap();
    model.apply(snapshot(epoch, entry, source)).unwrap();
    model
}

#[test]
fn first_adoption_of_a_different_existing_file_requires_review_without_a_common_ancestor() {
    let (_temporary, mirror, entry) = fixture();
    fs::write(mirror.root.join(&entry.path), "existing real local source").unwrap();
    let mut discovered = document("fresh Overleaf source");

    assert!(
        mirror
            .prepare_unbased_document(&entry, &mut discovered)
            .unwrap()
    );
    let receipt = mirror.sync_document(&entry, &discovered).unwrap();
    discovered.checkpoint = Some(receipt.checkpoint);
    let mut model = remote_model(1, &entry, "fresh Overleaf source");
    assert!(
        model
            .import_local_conflict(&entry.id, "fresh Overleaf source", &discovered)
            .unwrap()
    );

    let saved = model.document(&entry.id).unwrap();
    assert_eq!(saved.source(), "existing real local source");
    assert_eq!(saved.baseline, "fresh Overleaf source");
    assert!(!saved.conflict().unwrap().base_known);
    assert!(saved.conflict().unwrap().base.is_empty());
    assert!(model.publish(&entry.id).is_none());
    let review = model.conflict_review(&entry.id).unwrap();
    assert_eq!(review.local, "existing real local source");
    assert_eq!(review.remote, "fresh Overleaf source");
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "existing real local source"
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();
    assert!(
        !restored
            .document(&entry.id)
            .unwrap()
            .conflict()
            .unwrap()
            .base_known
    );
    model
        .resolve_conflict(&review, review.local.clone())
        .unwrap();
    assert!(
        matches!(model.publish(&entry.id), Some(BrowserCommand::Edit { source, .. }) if source == "existing real local source")
    );
}

#[test]
fn first_adoption_retains_an_empty_existing_file_as_an_explicit_conflict() {
    let (_temporary, mirror, entry) = fixture();
    fs::write(mirror.root.join(&entry.path), "").unwrap();
    let mut discovered = document("nonempty Overleaf source");

    assert!(
        mirror
            .prepare_unbased_document(&entry, &mut discovered)
            .unwrap()
    );
    let receipt = mirror.sync_document(&entry, &discovered).unwrap();
    discovered.checkpoint = Some(receipt.checkpoint);
    let mut model = remote_model(1, &entry, "nonempty Overleaf source");
    assert!(
        model
            .import_local_conflict(&entry.id, "nonempty Overleaf source", &discovered)
            .unwrap()
    );

    let saved = model.document(&entry.id).unwrap();
    assert_eq!(saved.draft(), Some(""));
    assert_eq!(saved.source(), "");
    assert!(!saved.conflict().unwrap().base_known);
    assert!(model.conflict_review(&entry.id).is_some());
    assert!(model.publish(&entry.id).is_none());
    assert!(fs::read(mirror.root.join(&entry.path)).unwrap().is_empty());
}

#[test]
fn fresh_remote_snapshots_never_auto_merge_an_unknown_ancestor_even_when_contents_match() {
    for local in ["", "existing local source"] {
        let (_temporary, mirror, entry) = fixture();
        fs::write(mirror.root.join(&entry.path), local).unwrap();
        let mut discovered = document("initial remote source");
        assert!(
            mirror
                .prepare_unbased_document(&entry, &mut discovered)
                .unwrap()
        );
        let mut model = remote_model(1, &entry, "initial remote source");
        assert!(
            model
                .import_local_conflict(&entry.id, "initial remote source", &discovered)
                .unwrap()
        );

        for remote in ["new remote source", local] {
            assert!(matches!(
                model.apply(snapshot(1, &entry, remote)).unwrap(),
                ModelChange::Document { conflict: true, .. }
            ));
            let saved = model.document(&entry.id).unwrap();
            assert_eq!(saved.source(), local);
            assert_eq!(saved.baseline, remote);
            assert!(!saved.conflict().unwrap().base_known);
            assert!(saved.conflict().unwrap().base.is_empty());
            assert!(model.publish(&entry.id).is_none());
        }
        // Matching bytes still require review; a subsequent remote deletion
        // cannot discard that unknown-ancestor comparison implicitly.
        model
            .apply(BrowserEvent::Project {
                epoch: 1,
                project_id: project().id().into(),
                name: "Paper".into(),
                root_doc: None,
                entries: Vec::new(),
            })
            .unwrap();
        assert!(
            !model
                .document(&entry.id)
                .unwrap()
                .conflict()
                .unwrap()
                .base_known
        );
        assert!(model.has_unsynced());
    }
}

#[test]
fn delayed_unbased_file_discovery_cannot_replace_a_newer_native_draft_or_remote_baseline() {
    let (_temporary, mirror, entry) = fixture();
    fs::write(mirror.root.join(&entry.path), "existing local source").unwrap();
    let mut discovered = document("fresh remote source");
    assert!(
        mirror
            .prepare_unbased_document(&entry, &mut discovered)
            .unwrap()
    );
    let mut model = remote_model(1, &entry, "fresh remote source");
    model
        .set_draft(&entry.id, "new native edit".into())
        .unwrap();
    assert!(
        !model
            .import_local_conflict(&entry.id, "fresh remote source", &discovered)
            .unwrap()
    );
    assert_eq!(
        model.document(&entry.id).unwrap().source(),
        "new native edit"
    );
    let mut model = remote_model(1, &entry, "newer remote source");
    assert!(
        !model
            .import_local_conflict(&entry.id, "fresh remote source", &discovered)
            .unwrap()
    );
    assert_eq!(
        model.document(&entry.id).unwrap().source(),
        "newer remote source"
    );
    assert!(model.document(&entry.id).unwrap().conflict().is_none());
}

#[test]
fn offline_restart_retains_ancestor_and_requires_fresh_server_verification() {
    let (_temporary, mirror, entry) = fixture();
    let source = "Opening.\nMiddle.\nClosing.\n";
    mirror.sync_document(&entry, &document(source)).unwrap();
    let mut first = OfflineStore::load(&mirror.root, &project(), 5)
        .unwrap()
        .unwrap();
    assert!(first.publish(&entry.id).is_none());
    first
        .set_draft(&entry.id, "Opening locally.\nMiddle.\nClosing.\n".into())
        .unwrap();
    mirror
        .sync_document(&entry, first.document(&entry.id).unwrap())
        .unwrap();
    let mut second = OfflineStore::load(&mirror.root, &project(), 9)
        .unwrap()
        .unwrap();
    assert_eq!(second.document(&entry.id).unwrap().baseline, source);
    assert!(second.has_unsynced());
    assert!(second.publish(&entry.id).is_none());
    let remote = "Opening.\nMiddle.\nClosing remotely.\n";
    assert!(matches!(
        second.apply(snapshot(9, &entry, remote)).unwrap(),
        ModelChange::Document {
            conflict: false,
            ..
        }
    ));
    let merged = "Opening locally.\nMiddle.\nClosing remotely.\n";
    assert_eq!(second.document(&entry.id).unwrap().source(), merged);
    mirror
        .sync_document(&entry, second.document(&entry.id).unwrap())
        .unwrap();
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        merged
    );
    assert!(
        matches!(second.publish(&entry.id), Some(BrowserCommand::Edit { base, source, .. }) if base == remote && source == merged)
    );
}

#[test]
fn overlapping_conflict_survives_restarts_with_original_common_ancestor() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("Title base.\n"))
        .unwrap();
    let mut model = OfflineStore::load(&mirror.root, &project(), 1)
        .unwrap()
        .unwrap();
    model.set_draft(&entry.id, "Title local.\n".into()).unwrap();
    mirror
        .sync_document(&entry, model.document(&entry.id).unwrap())
        .unwrap();
    model.record_checkpoint(&entry.id, journal_checkpoint(&mirror, &entry));
    model.apply(snapshot(1, &entry, "Title remote.\n")).unwrap();
    assert!(model.document(&entry.id).unwrap().conflict().is_some());
    mirror
        .sync_document(&entry, model.document(&entry.id).unwrap())
        .unwrap();
    let mut restored = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();
    let conflict = restored.document(&entry.id).unwrap().conflict().unwrap();
    assert_eq!(conflict.base, "Title base.\n");
    assert_eq!(conflict.local, "Title local.\n");
    assert_eq!(conflict.remote, "Title remote.\n");
    assert!(restored.conflict_review(&entry.id).is_none());
    restored
        .apply(snapshot(2, &entry, "Title remote.\n"))
        .unwrap();
    assert!(restored.conflict_review(&entry.id).is_some());
}

#[test]
fn external_text_edits_are_drafts_and_not_a_new_remote_baseline() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("base")).unwrap();
    fs::write(mirror.root.join(&entry.path), "edited while app closed").unwrap();
    let model = OfflineStore::load(&mirror.root, &project(), 10)
        .unwrap()
        .unwrap();
    let saved = model.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "base");
    assert_eq!(saved.source(), "edited while app closed");
    assert!(model.has_unsynced());
    // Loading acknowledged these exact local bytes for the next guarded write.
    mirror.sync_document(&entry, saved).unwrap();
}

#[test]
fn newer_external_edits_and_deletions_refuse_stale_worker_writes() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("base")).unwrap();
    fs::write(mirror.root.join(&entry.path), "new external edit").unwrap();
    assert!(
        mirror
            .sync_document(&entry, &document("remote"))
            .unwrap_err()
            .contains("preserved")
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "new external edit"
    );
    fs::remove_file(mirror.root.join(&entry.path)).unwrap();
    assert!(mirror.sync_document(&entry, &document("remote")).is_err());
    assert!(!mirror.root.join(&entry.path).exists());
    let restored = OfflineStore::load(&mirror.root, &project(), 1)
        .unwrap()
        .unwrap();
    assert!(restored.document(&entry.id).is_none());
}

#[test]
fn missing_cached_document_keeps_its_journal_and_does_not_block_other_offline_pages() {
    let (_temporary, mirror, missing) = fixture();
    let available = make_entry("available", "chapter.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![missing.clone(), available.clone()]))
        .unwrap();
    let mut saved_missing = document("missing baseline");
    saved_missing.edit("recoverable saved local draft".into(), false);
    mirror.sync_document(&missing, &saved_missing).unwrap();
    mirror
        .sync_document(&available, &document("available source"))
        .unwrap();
    let journal_path = mirror.root.join(document_path(&missing.id));
    let journal = fs::read(&journal_path).unwrap();
    fs::remove_file(mirror.root.join(&missing.path)).unwrap();

    let mut restored = OfflineStore::load(&mirror.root, &project(), 8)
        .unwrap()
        .unwrap();

    assert!(restored.document(&missing.id).is_none());
    assert!(restored.tree().unwrap().entry(&missing.id).is_some());
    assert_eq!(
        restored.document(&available.id).unwrap().source(),
        "available source"
    );
    assert!(restored.publish(&available.id).is_none());
    assert!(!mirror.root.join(&missing.path).exists());
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
    assert!(mirror.sync_document(&missing, &saved_missing).is_err());
    assert!(!mirror.root.join(&missing.path).exists());
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
}

#[test]
fn untracked_existing_text_and_assets_are_never_overwritten() {
    let (_temporary, mirror, entry) = fixture();
    fs::write(mirror.root.join(&entry.path), "untracked real file").unwrap();
    assert!(mirror.sync_document(&entry, &document("remote")).is_err());
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "untracked real file"
    );
    let asset = make_entry("asset", "figure.png", EntryKind::File);
    fs::write(mirror.root.join(&asset.path), b"real image").unwrap();
    assert!(mirror.sync_file(&asset, "YWJj").is_err());
    assert_eq!(
        fs::read(mirror.root.join(&asset.path)).unwrap(),
        b"real image"
    );
}

#[test]
fn removing_remote_document_keeps_saved_local_draft_discoverable_after_restart() {
    let (_temporary, mirror, entry) = fixture();
    let mut saved = document("base");
    let receipt = mirror.sync_document(&entry, &saved).unwrap();
    saved.checkpoint = Some(receipt.checkpoint);
    saved.edit("saved offline edit".into(), false);
    mirror.sync_document(&entry, &saved).unwrap();
    let error = mirror.sync_tree(&project(), &tree(Vec::new())).unwrap_err();
    assert!(error.contains("local copies are retained"));
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "saved offline edit"
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.document(&entry.id).unwrap().source(),
        "saved offline edit"
    );
    assert!(restored.tree().unwrap().entry(&entry.id).is_some());
}

#[test]
fn first_checkpoint_clean_rename_keeps_source_until_the_new_path_commits() {
    for reopen in [false, true] {
        let (_temporary, mirror, original) = fixture();
        let source = "A newly duplicated real document.\n";
        let mut saved = document(source);
        saved.checkpoint = Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
        let renamed = make_entry(&original.id, "notes/renamed.txt", EntryKind::Doc);
        let journal = fs::read(mirror.root.join(document_path(&original.id))).unwrap();
        mirror
            .sync_tree(&project(), &tree(vec![renamed.clone()]))
            .unwrap();
        assert_eq!(
            fs::read_to_string(mirror.root.join(&original.path)).unwrap(),
            source,
            "tree metadata cannot remove the only bytes of a committed first journal"
        );
        assert_eq!(
            fs::read(mirror.root.join(document_path(&original.id))).unwrap(),
            journal
        );
        if reopen {
            let restored = OfflineStore::load(&mirror.root, &project(), 3)
                .unwrap()
                .unwrap();
            saved = restored.document(&original.id).unwrap().clone();
            assert_eq!(saved.source(), source);
        }
        saved.checkpoint = Some(mirror.sync_document(&renamed, &saved).unwrap().checkpoint);
        assert_eq!(
            fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
            source
        );
        assert!(!mirror.root.join(&original.path).exists());
        // A later folder move uses the same durable predecessor chain.
        let moved = make_entry(&original.id, "moved/again.tex", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![moved.clone()]))
            .unwrap();
        mirror.sync_document(&moved, &saved).unwrap();
        assert_eq!(
            fs::read_to_string(mirror.root.join(&moved.path)).unwrap(),
            source
        );
        assert!(!mirror.root.join(&renamed.path).exists());
        mirror.sync_tree(&project(), &tree(Vec::new())).unwrap();
        assert!(!mirror.root.join(&moved.path).exists());
    }
}

fn has_case_aliases(root: &Path) -> bool {
    let lower = root.join("case-sensitivity-probe");
    let upper = root.join("CASE-SENSITIVITY-PROBE");
    fs::write(&lower, "filesystem probe").unwrap();
    let aliases = upper.exists();
    fs::remove_file(lower).unwrap();
    if !aliases {
        eprintln!("case-alias regression is unavailable on this case-sensitive filesystem");
    }
    aliases
}

#[cfg(target_os = "macos")]
fn assert_stored_name(parent: &Path, expected: &str) {
    assert!(
        fs::read_dir(parent)
            .unwrap()
            .any(|entry| entry.unwrap().file_name() == expected),
        "the stored filename must match the project spelling: {expected}"
    );
}

#[test]
fn case_only_document_and_parent_renames_preserve_clean_and_dirty_saved_sources() {
    let (temporary, _, _) = fixture();
    if !has_case_aliases(temporary.path()) {
        return;
    }
    for renamed_path in ["notes/MAIN.tex", "Notes/main.tex", "Notes/MAIN.tex"] {
        for dirty in [false, true] {
            for reopen in [false, true] {
                let (_temporary, mirror, entry) = fixture();
                let original = make_entry(&entry.id, "notes/main.tex", EntryKind::Doc);
                let folder = make_entry("folder", "notes", EntryKind::Folder);
                mirror
                    .sync_tree(&project(), &tree(vec![folder, original.clone()]))
                    .unwrap();
                let mut saved = document("Real remote baseline\n");
                if dirty {
                    saved.edit("Saved offline draft\n".into(), false);
                }
                saved.checkpoint =
                    Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
                let file_inode = fs::metadata(mirror.root.join(&original.path))
                    .unwrap()
                    .ino();
                let folder_inode = fs::metadata(mirror.root.join("notes")).unwrap().ino();
                fs::write(
                    mirror.root.join("notes/untracked.txt"),
                    "Keep local bytes\n",
                )
                .unwrap();
                let renamed = make_entry(&original.id, renamed_path, EntryKind::Doc);
                let folder = make_entry(
                    "folder",
                    renamed_path.split_once('/').unwrap().0,
                    EntryKind::Folder,
                );
                mirror
                    .sync_tree(&project(), &tree(vec![folder, renamed.clone()]))
                    .unwrap();
                assert!(
                    mirror::unix::same_file(
                        &mirror.directory,
                        &original.relative_path().unwrap(),
                        &renamed.relative_path().unwrap(),
                    )
                    .unwrap()
                );
                if reopen {
                    let restored = OfflineStore::load(&mirror.root, &project(), 4)
                        .unwrap()
                        .unwrap();
                    saved = restored.document(&original.id).unwrap().clone();
                    assert_eq!(saved.is_dirty(), dirty);
                    assert_eq!(
                        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
                        saved.source(),
                        "reopening cannot unlink the same file through its old spelling"
                    );
                }
                saved.checkpoint = Some(mirror.sync_document(&renamed, &saved).unwrap().checkpoint);
                assert_eq!(
                    fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
                    saved.source(),
                    "a successful case-only checkpoint must keep its committed target"
                );
                assert_eq!(
                    fs::metadata(mirror.root.join(&renamed.path)).unwrap().ino(),
                    file_inode,
                    "a metadata-only alias rename must not replace source bytes"
                );
                let (parent, _) = renamed_path.split_once('/').unwrap();
                assert_eq!(
                    fs::metadata(mirror.root.join(parent)).unwrap().ino(),
                    folder_inode
                );
                assert_eq!(
                    fs::read_to_string(mirror.root.join(parent).join("untracked.txt")).unwrap(),
                    "Keep local bytes\n"
                );
                #[cfg(target_os = "macos")]
                {
                    let (_, basename) = renamed_path.split_once('/').unwrap();
                    assert_stored_name(&mirror.root, parent);
                    assert_stored_name(&mirror.root.join(parent), basename);
                }
                let restored = OfflineStore::load(&mirror.root, &project(), 5)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    restored.document(&original.id).unwrap().source(),
                    saved.source()
                );
                saved.edit("A subsequent saved draft\n".into(), false);
                mirror.sync_document(&renamed, &saved).unwrap();
                assert_eq!(
                    fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
                    saved.source()
                );
            }
        }
    }
}

#[test]
fn case_only_rename_retains_external_edits_and_checkpoint_authority() {
    let (_temporary, mirror, original) = fixture();
    if !has_case_aliases(&mirror.root) {
        return;
    }
    let mut saved = document("Original baseline\n");
    saved.checkpoint = Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
    let renamed = make_entry(&original.id, "MAIN.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![renamed.clone()]))
        .unwrap();
    let journal = fs::read(mirror.root.join(document_path(&original.id))).unwrap();
    fs::write(mirror.root.join(&original.path), "External editor draft\n").unwrap();
    assert!(mirror.sync_document(&renamed, &saved).is_err());
    assert_eq!(
        fs::read(mirror.root.join(document_path(&original.id))).unwrap(),
        journal
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 4)
        .unwrap()
        .unwrap();
    let current = restored.document(&original.id).unwrap();
    assert_eq!(current.baseline, saved.baseline);
    assert_eq!(current.draft(), Some("External editor draft\n"));
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        current.source()
    );
    #[cfg(target_os = "macos")]
    assert_stored_name(&mirror.root, &renamed.path);
    assert!(mirror.sync_document(&renamed, &saved).is_err());
    mirror.sync_document(&renamed, current).unwrap();
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        current.source()
    );
}

#[test]
fn renamed_hard_links_keep_both_names_and_later_writes_do_not_change_the_old_link() {
    for reopen in [false, true] {
        let (_temporary, mirror, original) = fixture();
        let mut saved = document("Original linked source\n");
        saved.checkpoint = Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
        let renamed = make_entry(&original.id, "notes/linked.tex", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![renamed.clone()]))
            .unwrap();
        fs::hard_link(
            mirror.root.join(&original.path),
            mirror.root.join(&renamed.path),
        )
        .unwrap();
        #[cfg(target_os = "macos")]
        assert!(
            !mirror::unix::rename_alias(
                &mirror.directory,
                &original.relative_path().unwrap(),
                &renamed.relative_path().unwrap(),
                EntryKind::Doc,
            )
            .unwrap()
        );
        if reopen {
            saved = OfflineStore::load(&mirror.root, &project(), 4)
                .unwrap()
                .unwrap()
                .document(&original.id)
                .unwrap()
                .clone();
        }
        saved.checkpoint = Some(mirror.sync_document(&renamed, &saved).unwrap().checkpoint);
        assert!(mirror.root.join(&original.path).exists());
        assert!(mirror.root.join(&renamed.path).exists());
        saved.edit("New independently saved source\n".into(), false);
        mirror.sync_document(&renamed, &saved).unwrap();
        assert_eq!(
            fs::read_to_string(mirror.root.join(&original.path)).unwrap(),
            "Original linked source\n"
        );
        assert_eq!(
            fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
            saved.source()
        );
    }
}

#[test]
fn case_only_owned_folder_renames_preserve_empty_directories_and_untracked_children() {
    let (_temporary, mirror, _) = fixture();
    if !has_case_aliases(&mirror.root) {
        return;
    }
    let original = make_entry("parent", "notes", EntryKind::Folder);
    let nested = make_entry("nested", "notes/empty", EntryKind::Folder);
    mirror
        .sync_tree(&project(), &tree(vec![original.clone(), nested.clone()]))
        .unwrap();
    let parent_inode = fs::metadata(mirror.root.join(&original.path))
        .unwrap()
        .ino();
    let nested_inode = fs::metadata(mirror.root.join(&nested.path)).unwrap().ino();
    fs::write(mirror.root.join("notes/local.txt"), "Untracked contents\n").unwrap();
    let current = make_entry(&original.id, "Notes", EntryKind::Folder);
    let nested_current = make_entry(&nested.id, "Notes/Empty", EntryKind::Folder);
    mirror
        .sync_tree(
            &project(),
            &tree(vec![current.clone(), nested_current.clone()]),
        )
        .unwrap();
    assert_eq!(
        fs::metadata(mirror.root.join(&current.path)).unwrap().ino(),
        parent_inode
    );
    assert_eq!(
        fs::metadata(mirror.root.join(&nested_current.path))
            .unwrap()
            .ino(),
        nested_inode,
        "the retired old spelling cannot remove and recreate the same empty directory"
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join("Notes/local.txt")).unwrap(),
        "Untracked contents\n"
    );
    #[cfg(target_os = "macos")]
    {
        assert_stored_name(&mirror.root, "Notes");
        assert_stored_name(&mirror.root.join("Notes"), "Empty");
    }
}

#[test]
fn case_only_asset_rename_keeps_its_bytes_until_refresh_and_repairs_stored_spelling() {
    let (_temporary, mirror, _) = fixture();
    if !has_case_aliases(&mirror.root) {
        return;
    }
    let original = make_entry("asset", "figure.bin", EntryKind::File);
    mirror
        .sync_tree(&project(), &tree(vec![original.clone()]))
        .unwrap();
    mirror.sync_file(&original, "YWJj").unwrap();
    let inode = fs::metadata(mirror.root.join(&original.path))
        .unwrap()
        .ino();
    let renamed = make_entry(&original.id, "Figure.bin", EntryKind::File);
    mirror
        .sync_tree(&project(), &tree(vec![renamed.clone()]))
        .unwrap();
    assert_eq!(fs::read(mirror.root.join(&renamed.path)).unwrap(), b"abc");
    assert_eq!(
        fs::metadata(mirror.root.join(&renamed.path)).unwrap().ino(),
        inode
    );
    mirror.sync_file(&renamed, "YWJj").unwrap();
    #[cfg(target_os = "macos")]
    assert_stored_name(&mirror.root, &renamed.path);
    fs::write(mirror.root.join(&renamed.path), "Externally changed asset").unwrap();
    let changed = make_entry(&original.id, "FIGURE.bin", EntryKind::File);
    assert!(mirror.sync_tree(&project(), &tree(vec![changed])).is_err());
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        "Externally changed asset"
    );
}

#[test]
fn an_unverifiable_asset_rename_destination_cannot_retire_the_saved_source() {
    let (_temporary, mirror, _) = fixture();
    let original = make_entry("asset", "figure.bin", EntryKind::File);
    mirror
        .sync_tree(&project(), &tree(vec![original.clone()]))
        .unwrap();
    mirror.sync_file(&original, "YWJj").unwrap();
    fs::write(mirror.root.join("untracked.bin"), "Unrelated local bytes").unwrap();
    symlink("untracked.bin", mirror.root.join("renamed.bin")).unwrap();
    let renamed = make_entry(&original.id, "renamed.bin", EntryKind::File);
    assert!(mirror.sync_tree(&project(), &tree(vec![renamed])).is_err());
    assert_eq!(fs::read(mirror.root.join(&original.path)).unwrap(), b"abc");
    assert_eq!(
        fs::read_to_string(mirror.root.join("untracked.bin")).unwrap(),
        "Unrelated local bytes"
    );
    assert!(mirror.root.join("renamed.bin").is_symlink());
}

#[test]
fn physical_file_identity_rejects_symlinks_and_non_regular_files() {
    let (_temporary, mirror, original) = fixture();
    fs::write(mirror.root.join(&original.path), "Real contents\n").unwrap();
    fs::write(mirror.root.join("distinct.tex"), "Real contents\n").unwrap();
    assert!(
        !mirror::unix::same_file(
            &mirror.directory,
            Path::new(&original.path),
            Path::new("distinct.tex"),
        )
        .unwrap()
    );
    #[cfg(target_os = "macos")]
    assert!(
        !mirror::unix::rename_alias(
            &mirror.directory,
            Path::new(&original.path),
            Path::new("distinct.tex"),
            EntryKind::Doc,
        )
        .unwrap()
    );
    symlink(&original.path, mirror.root.join("alias.tex")).unwrap();
    assert!(
        mirror::unix::same_file(
            &mirror.directory,
            Path::new(&original.path),
            Path::new("alias.tex"),
        )
        .is_err()
    );
    fs::create_dir(mirror.root.join("real-directory")).unwrap();
    symlink(".", mirror.root.join("linked-parent")).unwrap();
    for invalid in ["real-directory", "linked-parent/main.tex", "missing.tex"] {
        assert!(
            mirror::unix::same_file(
                &mirror.directory,
                Path::new(&original.path),
                Path::new(invalid),
            )
            .is_err()
        );
    }
    #[cfg(target_os = "macos")]
    for invalid in ["alias.tex", "real-directory", "linked-parent/main.tex"] {
        assert!(
            mirror::unix::rename_alias(
                &mirror.directory,
                Path::new(&original.path),
                Path::new(invalid),
                EntryKind::Doc,
            )
            .is_err()
        );
    }
}

#[test]
fn clean_rename_does_not_override_changes_at_either_path_or_a_deleted_original() {
    for changed in 0..3 {
        let (_temporary, mirror, original) = fixture();
        let mut saved = document("A newly cached clean document.\n");
        saved.checkpoint = Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
        let renamed = make_entry(&original.id, "notes/renamed.txt", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![renamed.clone()]))
            .unwrap();
        let journal = fs::read(mirror.root.join(document_path(&original.id))).unwrap();
        match changed {
            0 => fs::write(
                mirror.root.join(&original.path),
                "An external original edit",
            )
            .unwrap(),
            1 => fs::write(
                mirror.root.join(&renamed.path),
                "An unrelated destination file",
            )
            .unwrap(),
            _ => fs::remove_file(mirror.root.join(&original.path)).unwrap(),
        }
        assert!(mirror.sync_document(&renamed, &saved).is_err());
        assert_eq!(
            fs::read(mirror.root.join(document_path(&original.id))).unwrap(),
            journal
        );
        match changed {
            0 => assert_eq!(
                fs::read_to_string(mirror.root.join(&original.path)).unwrap(),
                "An external original edit"
            ),
            1 => assert_eq!(
                fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
                "An unrelated destination file"
            ),
            _ => assert!(!mirror.root.join(&original.path).exists()),
        }
    }
}

#[test]
fn a_failed_renamed_journal_keeps_original_source_and_reopenable_lineage() {
    let (_temporary, mirror, original) = fixture();
    let mut saved = document("A recoverable original.\n");
    saved.checkpoint = Some(mirror.sync_document(&original, &saved).unwrap().checkpoint);
    let renamed = make_entry(&original.id, "notes/renamed.txt", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![renamed.clone()]))
        .unwrap();
    let journal_path = mirror.root.join(document_path(&original.id));
    let journal = fs::read(&journal_path).unwrap();
    assert!(
        mirror
            .sync_document_with(&renamed, &saved, || {
                fs::remove_file(&journal_path).unwrap();
                fs::create_dir(&journal_path).unwrap();
            })
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&original.path)).unwrap(),
        saved.source()
    );
    assert!(!mirror.root.join(&renamed.path).exists());
    fs::remove_dir(&journal_path).unwrap();
    fs::write(&journal_path, journal).unwrap();
    let restored = OfflineStore::load(&mirror.root, &project(), 3)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.document(&original.id).unwrap().source(),
        saved.source()
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        saved.source()
    );
}

#[test]
fn folder_rename_move_and_removal_preserve_nested_work_and_untracked_children() {
    for dirty in [false, true] {
        let (_temporary, mirror, original) = fixture();
        let folder = make_entry("folder", "notes", EntryKind::Folder);
        let nested = make_entry(&original.id, "notes/nested.tex", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![folder.clone(), nested.clone()]))
            .unwrap();
        let mut saved = document("Nested baseline\n");
        if dirty {
            saved.edit("Saved nested draft\n".into(), false);
        }
        saved.checkpoint = Some(mirror.sync_document(&nested, &saved).unwrap().checkpoint);
        fs::write(
            mirror.root.join("notes/untracked.txt"),
            "Untracked real file",
        )
        .unwrap();
        let renamed_folder = make_entry("folder", "archive", EntryKind::Folder);
        let renamed = make_entry(&nested.id, "archive/nested.tex", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![renamed_folder, renamed.clone()]))
            .unwrap();
        assert_eq!(
            fs::read_to_string(mirror.root.join(&nested.path)).unwrap(),
            saved.source()
        );
        saved.checkpoint = Some(mirror.sync_document(&renamed, &saved).unwrap().checkpoint);
        assert!(!mirror.root.join(&nested.path).exists());
        let parent = make_entry("parent", "papers", EntryKind::Folder);
        let moved_folder = make_entry("folder", "papers/archive", EntryKind::Folder);
        let moved = make_entry(&nested.id, "papers/archive/nested.tex", EntryKind::Doc);
        mirror
            .sync_tree(&project(), &tree(vec![parent, moved_folder, moved.clone()]))
            .unwrap();
        assert_eq!(
            fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
            saved.source()
        );
        saved.checkpoint = Some(mirror.sync_document(&moved, &saved).unwrap().checkpoint);
        assert!(!mirror.root.join(&renamed.path).exists());
        if dirty {
            assert!(mirror.sync_tree(&project(), &tree(Vec::new())).is_err());
            assert_eq!(
                fs::read_to_string(mirror.root.join(&moved.path)).unwrap(),
                saved.source()
            );
        } else {
            mirror.sync_tree(&project(), &tree(Vec::new())).unwrap();
            assert!(!mirror.root.join(&moved.path).exists());
        }
        assert_eq!(
            fs::read_to_string(mirror.root.join("notes/untracked.txt")).unwrap(),
            "Untracked real file"
        );
    }
}

#[test]
fn clean_remote_removal_and_dirty_rename_preserve_the_right_bytes() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("base")).unwrap();
    let mut changed = document("base");
    changed.checkpoint = Some(journal_checkpoint(&mirror, &entry));
    changed.edit("offline draft".into(), false);
    mirror.sync_document(&entry, &changed).unwrap();
    let renamed = make_entry(&entry.id, "chapters/renamed.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![renamed.clone()]))
        .unwrap();
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "offline draft"
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 3)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.document(&entry.id).unwrap().source(),
        "offline draft"
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        "offline draft"
    );
    let receipt = mirror
        .sync_document(&renamed, restored.document(&entry.id).unwrap())
        .unwrap();
    let mut clean = document("offline draft");
    clean.checkpoint = Some(receipt.checkpoint);
    mirror.sync_document(&renamed, &clean).unwrap();
    mirror.sync_tree(&project(), &tree(Vec::new())).unwrap();
    assert!(!mirror.root.join(&renamed.path).exists());
}

#[test]
fn changed_binary_assets_and_remote_replacements_need_explicit_attention() {
    let (_temporary, mirror, original) = fixture();
    mirror.sync_document(&original, &document("base")).unwrap();
    let asset = make_entry("asset", "image.png", EntryKind::File);
    mirror
        .sync_tree(&project(), &tree(vec![original.clone(), asset.clone()]))
        .unwrap();
    mirror.sync_file(&asset, "YWJj").unwrap();
    fs::write(mirror.root.join(&asset.path), b"local binary edit").unwrap();
    assert!(
        mirror
            .sync_file(&asset, "ZGVm")
            .unwrap_err()
            .contains("local asset changes")
    );
    assert!(
        mirror
            .sync_tree(&project(), &tree(vec![original.clone()]))
            .is_err()
    );
    assert_eq!(
        fs::read(mirror.root.join(&asset.path)).unwrap(),
        b"local binary edit"
    );
    fs::write(mirror.root.join(&original.path), "local text edit").unwrap();
    let replacement = make_entry("new-id", &original.path, EntryKind::Doc);
    assert!(
        mirror
            .sync_tree(&project(), &tree(vec![replacement]))
            .unwrap_err()
            .contains("replaced")
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&original.path)).unwrap(),
        "local text edit"
    );
}

#[test]
fn corrupted_journals_wrong_project_and_symlinks_fail_without_mutating_real_files() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("retained")).unwrap();
    let wrong =
        ProjectUrl::parse("https://other.example/project/0123456789abcdef01234567").unwrap();
    assert!(OfflineStore::load(&mirror.root, &wrong, 1).is_err());
    let journal = mirror.root.join(document_path(&entry.id));
    let before = fs::read(&journal).unwrap();
    fs::write(&journal, b"{\"version\":").unwrap();
    assert!(OfflineStore::load(&mirror.root, &project(), 1).is_err());
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "retained"
    );
    fs::remove_file(&journal).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("journal.json");
    fs::write(&external, &before).unwrap();
    symlink(&external, &journal).unwrap();
    assert!(OfflineStore::load(&mirror.root, &project(), 1).is_err());
    assert!(mirror.sync_document(&entry, &document("new")).is_err());
    assert_eq!(fs::read(&external).unwrap(), before);
}

#[test]
fn restore_budget_and_source_limits_reject_before_writing_sources() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("safe")).unwrap();
    let too_large = document(&"x".repeat(super::super::MAX_SOURCE_BYTES + 1));
    assert!(mirror.sync_document(&entry, &too_large).is_err());
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "safe"
    );
    let mut remaining = 3;
    assert!(
        read_optional_budget(
            &mirror.directory,
            Path::new(&entry.path),
            100,
            &mut remaining
        )
        .is_err()
    );
    assert_eq!(remaining, 3);
    // Malicious IDs never become path components.
    assert_eq!(document_path("../../etc/passwd").components().count(), 4);
    assert!(
        !document_path("../../etc/passwd")
            .to_string_lossy()
            .contains("..")
    );
    let no_session = tempfile::tempdir().unwrap();
    let root: PathBuf = no_session.path().canonicalize().unwrap();
    assert!(OfflineStore::load(&root, &project(), 0).unwrap().is_none());
}

#[test]
fn one_document_save_does_not_rewrite_other_documents_or_the_tree() {
    use std::os::unix::fs::MetadataExt;
    let (_temporary, mirror, first) = fixture();
    let second = make_entry("second", "chapter.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![first.clone(), second.clone()]))
        .unwrap();
    mirror.sync_document(&first, &document("one")).unwrap();
    mirror.sync_document(&second, &document("two")).unwrap();
    let paths = [
        Path::new(TREE_PATH).to_owned(),
        document_path(&second.id),
        second.relative_path().unwrap(),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| {
            let path = mirror.root.join(path);
            (fs::metadata(&path).unwrap().ino(), fs::read(path).unwrap())
        })
        .collect();
    let mut changed = document("one");
    changed.checkpoint = Some(journal_checkpoint(&mirror, &first));
    changed.edit("one saved offline".into(), false);
    mirror.sync_document(&first, &changed).unwrap();
    let after: Vec<_> = paths
        .iter()
        .map(|path| {
            let path = mirror.root.join(path);
            (fs::metadata(&path).unwrap().ino(), fs::read(path).unwrap())
        })
        .collect();
    assert_eq!(before, after);
}

#[test]
fn stale_window_cannot_overwrite_another_windows_saved_source_or_ancestor() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("shared baseline"))
        .unwrap();
    let mut a = OfflineStore::load(&mirror.root, &project(), 1)
        .unwrap()
        .unwrap();
    let mut b = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();
    a.set_draft(&entry.id, "stale window A".into()).unwrap();
    b.set_draft(&entry.id, "saved window B".into()).unwrap();
    mirror
        .sync_document(&entry, b.document(&entry.id).unwrap())
        .unwrap();
    let journal = fs::read(mirror.root.join(document_path(&entry.id))).unwrap();

    assert!(
        mirror
            .sync_document(&entry, a.document(&entry.id).unwrap())
            .is_err()
    );

    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "saved window B"
    );
    assert_eq!(
        fs::read(mirror.root.join(document_path(&entry.id))).unwrap(),
        journal
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 3)
        .unwrap()
        .unwrap();
    let saved = restored.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "shared baseline");
    assert_eq!(saved.source(), "saved window B");
}

#[test]
fn identical_working_bytes_do_not_authorize_overwriting_a_newer_remote_ancestor() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("shared working bytes"))
        .unwrap();
    let a = OfflineStore::load(&mirror.root, &project(), 1)
        .unwrap()
        .unwrap();
    let mut b = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();
    b.apply(snapshot(2, &entry, "new remote ancestor")).unwrap();
    b.set_draft(&entry.id, "shared working bytes".into())
        .unwrap();
    mirror
        .sync_document(&entry, b.document(&entry.id).unwrap())
        .unwrap();
    let journal = fs::read(mirror.root.join(document_path(&entry.id))).unwrap();

    assert!(
        mirror
            .sync_document(&entry, a.document(&entry.id).unwrap())
            .is_err()
    );
    assert!(
        mirror
            .sync_document(&entry, &document("shared working bytes"))
            .is_err()
    );

    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "shared working bytes"
    );
    assert_eq!(
        fs::read(mirror.root.join(document_path(&entry.id))).unwrap(),
        journal
    );
    let restored = OfflineStore::load(&mirror.root, &project(), 3)
        .unwrap()
        .unwrap();
    let saved = restored.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "new remote ancestor");
    assert_eq!(saved.source(), "shared working bytes");
    assert!(saved.is_dirty());
}

#[test]
fn failed_journal_preparation_cannot_change_the_source_file() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("saved baseline"))
        .unwrap();
    let mut model = OfflineStore::load(&mirror.root, &project(), 1)
        .unwrap()
        .unwrap();
    model
        .set_draft(&entry.id, "typing whose save must fail".into())
        .unwrap();
    let journal = mirror.root.join(document_path(&entry.id));
    let result = mirror.sync_document_with(&entry, model.document(&entry.id).unwrap(), || {
        // Fail the actual journal replacement after its old contents have
        // been read. The former file-first ordering changed main.tex here.
        fs::remove_file(&journal).unwrap();
        fs::create_dir(&journal).unwrap();
    });

    assert!(result.is_err());

    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "saved baseline"
    );
    assert!(journal.is_dir());
}

fn pending_save(entry: &Entry, previous: Document, desired: Document) -> SavedDocument {
    SavedDocument {
        version: DOCUMENT_VERSION,
        entry: entry.clone(),
        document: desired,
        pending: Some(PendingWrite {
            previous: Some(PreviousDocument {
                entry: entry.clone(),
                document: previous,
            }),
        }),
    }
}

#[test]
fn pending_journal_recovers_previous_ancestor_when_source_write_never_happened() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("previous remote ancestor"))
        .unwrap();
    let mut desired = document("new remote ancestor");
    desired.edit("new local working bytes".into(), false);
    let pending = pending_save(&entry, document("previous remote ancestor"), desired);
    write_json(
        &mirror.directory,
        &document_path(&entry.id),
        &pending,
        MAX_JOURNAL_BYTES,
    )
    .unwrap();

    let restored = OfflineStore::load(&mirror.root, &project(), 2)
        .unwrap()
        .unwrap();

    let saved = restored.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "previous remote ancestor");
    assert_eq!(saved.source(), "previous remote ancestor");
    assert_eq!(saved.checkpoint, Some(journal_checkpoint(&mirror, &entry)));
    assert_eq!(
        fs::read_to_string(mirror.root.join(&entry.path)).unwrap(),
        "previous remote ancestor"
    );
}

#[test]
fn pending_journal_selects_desired_ancestor_after_source_write_and_before_finalization() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("previous ancestor"))
        .unwrap();
    let mut desired = document("new remote ancestor");
    desired.edit("saved new local bytes".into(), false);
    let pending = pending_save(&entry, document("previous ancestor"), desired);
    write_json(
        &mirror.directory,
        &document_path(&entry.id),
        &pending,
        MAX_JOURNAL_BYTES,
    )
    .unwrap();
    mirror::unix::write(
        &mirror.directory,
        &entry.relative_path().unwrap(),
        b"saved new local bytes",
    )
    .unwrap();

    let mut restored = OfflineStore::load(&mirror.root, &project(), 3)
        .unwrap()
        .unwrap();

    let saved = restored.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "new remote ancestor");
    assert_eq!(saved.source(), "saved new local bytes");
    assert!(restored.publish(&entry.id).is_none());
}

#[test]
fn external_change_during_pending_save_uses_the_prior_ancestor() {
    let (_temporary, mirror, entry) = fixture();
    mirror
        .sync_document(&entry, &document("prior ancestor"))
        .unwrap();
    let pending = pending_save(
        &entry,
        document("prior ancestor"),
        document("uncommitted new ancestor"),
    );
    write_json(
        &mirror.directory,
        &document_path(&entry.id),
        &pending,
        MAX_JOURNAL_BYTES,
    )
    .unwrap();
    fs::write(mirror.root.join(&entry.path), "independent external edit").unwrap();

    let restored = OfflineStore::load(&mirror.root, &project(), 4)
        .unwrap()
        .unwrap();

    let saved = restored.document(&entry.id).unwrap();
    assert_eq!(saved.baseline, "prior ancestor");
    assert_eq!(saved.source(), "independent external edit");
    assert_eq!(saved.checkpoint, Some(journal_checkpoint(&mirror, &entry)));
}

#[test]
fn external_edit_of_old_path_survives_a_renamed_tree_checkpoint() {
    let (_temporary, mirror, entry) = fixture();
    mirror.sync_document(&entry, &document("baseline")).unwrap();
    let mut saved = document("baseline");
    saved.checkpoint = Some(journal_checkpoint(&mirror, &entry));
    saved.edit("saved before rename".into(), false);
    mirror.sync_document(&entry, &saved).unwrap();
    let renamed = make_entry(&entry.id, "new-name.tex", EntryKind::Doc);
    mirror
        .sync_tree(&project(), &tree(vec![renamed.clone()]))
        .unwrap();
    fs::write(
        mirror.root.join(&entry.path),
        "external after rename checkpoint",
    )
    .unwrap();

    let restored = OfflineStore::load(&mirror.root, &project(), 5)
        .unwrap()
        .unwrap();

    assert_eq!(
        restored.document(&entry.id).unwrap().source(),
        "external after rename checkpoint"
    );
    assert_eq!(
        fs::read_to_string(mirror.root.join(&renamed.path)).unwrap(),
        "external after rename checkpoint"
    );
    assert!(!mirror.root.join(&entry.path).exists());
}
