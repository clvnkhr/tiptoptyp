use super::*;

#[test]
fn stage_all_and_commit_handles_new_modified_deleted_and_literal_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    initialize(root, "original\n");
    fs::write(root.join("deleted.typ"), "delete me").unwrap();
    perform(root, Operation::StageAllAndCommit("Add file".into())).unwrap();
    fs::remove_file(root.join("deleted.typ")).unwrap();
    fs::write(root.join("main.typ"), "changed\n").unwrap();
    fs::write(root.join(":(glob)*.typ"), "literal new file").unwrap();
    fs::create_dir(root.join(".tiptoptyp")).unwrap();
    fs::write(root.join(".tiptoptyp/private.typ"), "private").unwrap();
    let result = perform(root, Operation::StageAllAndCommit("All changes".into())).unwrap();
    assert!(result.committed && result.snapshot.entries.is_empty());
    assert_eq!(
        text_run(root, &["show", "HEAD:main.typ"]).unwrap(),
        "changed"
    );
    assert_eq!(
        text_run(root, &["show", "HEAD::(glob)*.typ"]).unwrap(),
        "literal new file"
    );
    assert!(text_run(root, &["show", "HEAD:deleted.typ"]).is_err());
    assert!(text_run(root, &["show", "HEAD:.tiptoptyp/private.typ"]).is_err());
    assert_eq!(
        text_run(root, &["log", "-1", "--format=%s"]).unwrap(),
        "All changes"
    );
}

#[test]
fn stage_all_and_commit_rejects_blank_messages_and_newly_staged_subsets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    initialize(root, "original\n");
    fs::write(root.join("main.typ"), "changed\n").unwrap();
    assert!(perform(root, Operation::StageAllAndCommit(" \n".into())).is_err());
    assert_eq!(snapshot(root).unwrap().entries.staged, 0);
    fs::write(root.join("selected.typ"), "selected").unwrap();
    perform(root, Operation::Stage("selected.typ".into())).unwrap();
    assert!(perform(root, Operation::StageAllAndCommit("Unsafe subset".into())).is_err());
    assert_eq!(snapshot(root).unwrap().entries.staged, 1);
    perform(root, Operation::Commit("Selected only".into())).unwrap();
    assert_eq!(
        text_run(root, &["show", "HEAD:main.typ"]).unwrap(),
        "original"
    );
    assert_eq!(
        fs::read_to_string(root.join("main.typ")).unwrap(),
        "changed\n"
    );
}

#[test]
fn revert_preserves_index_removes_only_confirmed_new_files_and_restores_deletions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    initialize(root, "original\n");
    fs::write(root.join("deleted.typ"), "restore me").unwrap();
    perform(root, Operation::StageAllAndCommit("Add file".into())).unwrap();
    fs::write(root.join("main.typ"), "staged\n").unwrap();
    fs::write(root.join("staged-new.typ"), "staged new").unwrap();
    perform(root, Operation::StageAll).unwrap();
    let index = text_run(root, &["write-tree"]).unwrap();
    fs::write(root.join("main.typ"), "unstaged\n").unwrap();
    fs::write(root.join("staged-new.typ"), "unstaged new").unwrap();
    fs::remove_file(root.join("deleted.typ")).unwrap();
    let new_path = "space and\n:(glob)*.typ";
    fs::write(root.join(new_path), "unstaged new file").unwrap();
    fs::create_dir(root.join(".tiptoptyp")).unwrap();
    fs::write(root.join(".tiptoptyp/private.typ"), "private").unwrap();
    let paths = snapshot(root)
        .unwrap()
        .entries
        .iter()
        .filter(|entry| entry.revertible())
        .map(|entry| entry.path.clone())
        .collect();
    fs::write(root.join("later.typ"), "not confirmed").unwrap();
    perform(root, Operation::Revert(paths)).unwrap();
    assert_eq!(text_run(root, &["write-tree"]).unwrap(), index);
    assert_eq!(
        fs::read_to_string(root.join("main.typ")).unwrap(),
        "staged\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("staged-new.typ")).unwrap(),
        "staged new"
    );
    assert_eq!(
        fs::read_to_string(root.join("deleted.typ")).unwrap(),
        "restore me"
    );
    assert!(!root.join(new_path).exists());
    assert!(root.join("later.typ").exists());
    assert!(root.join(".tiptoptyp/private.typ").exists());
}

#[test]
fn reverting_one_new_file_works_before_first_commit_and_cannot_delete_a_staged_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    perform(root, Operation::Init).unwrap();
    fs::write(root.join("new.typ"), "new").unwrap();
    fs::write(root.join("other.typ"), "other").unwrap();
    perform(root, Operation::Revert(vec!["new.typ".into()])).unwrap();
    assert!(!root.join("new.typ").exists());
    assert!(root.join("other.typ").exists());
    perform(root, Operation::Stage("other.typ".into())).unwrap();
    assert!(perform(root, Operation::Revert(vec!["other.typ".into()])).is_err());
    assert_eq!(fs::read_to_string(root.join("other.typ")).unwrap(), "other");
}

#[test]
fn revert_rejects_stale_conflicted_private_and_directory_targets_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    initialize(root, "original\n");
    fs::write(root.join("main.typ"), "changed\n").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    text_run(&root.join("nested"), &["init"]).unwrap();
    fs::write(root.join("nested/new.typ"), "nested").unwrap();
    for bad in [
        "missing.typ",
        "../outside.typ",
        ".tiptoptyp/private.typ",
        "nested/",
    ] {
        assert!(
            perform(root, Operation::Revert(vec!["main.typ".into(), bad.into()])).is_err(),
            "{bad}"
        );
        assert_eq!(
            fs::read_to_string(root.join("main.typ")).unwrap(),
            "changed\n"
        );
    }
    for (index, worktree) in [('U', 'U'), ('A', 'A'), ('D', 'D'), ('A', 'U'), ('U', 'D')] {
        assert!(
            !Entry {
                path: "main.typ".into(),
                index,
                worktree
            }
            .revertible()
        );
    }
}

#[cfg(unix)]
#[test]
fn reverting_an_untracked_symlink_does_not_follow_it() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path();
    initialize(root, "original\n");
    fs::write(outside.path().join("keep.typ"), "keep").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
    perform(root, Operation::Revert(vec!["link".into()])).unwrap();
    assert!(fs::symlink_metadata(root.join("link")).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("keep.typ")).unwrap(),
        "keep"
    );
}

pub(super) fn initialize(root: &Path, source: &str) {
    text_run(root, &["init"]).unwrap();
    for (name, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        text_run(root, &["config", name, value]).unwrap();
    }
    fs::write(root.join("main.typ"), source).unwrap();
    text_run(root, &["add", "--", "main.typ"]).unwrap();
    text_run(root, &["commit", "-m", "Initial"]).unwrap();
}

#[test]
fn handle_construction_is_borrowed_and_does_not_touch_the_filesystem() {
    let missing = Path::new("/missing-repository-no-probe-on-construction");
    let repository = Repository::new(missing);
    assert!(std::ptr::eq(repository.workspace, missing));
    assert_eq!(
        std::mem::size_of_val(&repository),
        std::mem::size_of::<&Path>()
    );
}

#[test]
fn nested_workspace_results_keep_the_request_identity_and_canonical_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    initialize(&root, "initial\n");
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let alias = nested.join(".");
    let repository = Repository::new(&alias);
    fs::write(root.join("new.typ"), "new\n").unwrap();
    for operation in [
        Operation::Refresh,
        Operation::Stage("new.typ".into()),
        Operation::Diff("new.typ".into(), DiffKind::Staged),
        Operation::Unstage("new.typ".into()),
    ] {
        let result = repository.execute(operation).unwrap();
        assert_eq!(result.workspace, alias);
        assert_eq!(result.snapshot.root, root);
    }
}

#[test]
fn service_preserves_new_file_crlf_and_missing_final_newline_through_hunk_actions() {
    use hunks::Action;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    initialize(&root, "unrelated\n");
    text_run(&root, &["config", "core.autocrlf", "false"]).unwrap();
    text_run(&root, &["config", "core.safecrlf", "false"]).unwrap();
    #[cfg(unix)]
    let relative = Path::new("quoted \"文稿\" [x].typ");
    #[cfg(not(unix))]
    let relative = Path::new("space 文稿 [x].typ");
    let path = root.join(relative);
    let original = "first\r\nlast";
    let changed = "first\r\nchanged 文稿";
    fs::write(&path, original).unwrap();
    let repository = Repository::new(&root);
    let BufferStatus { snapshot, hunks } = repository.scan_buffer(Some(&path), original).unwrap();
    assert_eq!(snapshot.entries[0].path, relative);
    assert_eq!(snapshot.entries[0].index, '?');
    let new_hunk = hunks.unwrap().remove(0);
    assert!(new_hunk.text.contains("+first\r\n"));
    assert!(new_hunk.text.contains("\\ No newline at end of file"));
    repository
        .change_index(&path, original, &new_hunk, Action::Stage)
        .unwrap();
    repository
        .execute(Operation::Commit("New file".into()))
        .unwrap();

    let hunk = repository
        .scan_buffer(Some(&path), changed)
        .unwrap()
        .hunks
        .unwrap()
        .remove(0);
    assert_eq!(hunks::revert(changed, &hunk).unwrap(), original);
    repository
        .change_index(&path, changed, &hunk, Action::Stage)
        .unwrap();
    let mut object = OsString::from(":");
    object.push(relative);
    let read_index = || run(&root, &["cat-file".into(), "blob".into(), object.clone()]).unwrap();
    assert_eq!(read_index(), changed.as_bytes());
    let staged = repository
        .execute(Operation::Diff(relative.into(), DiffKind::Staged))
        .unwrap();
    let content = staged.diff.unwrap().content.unwrap();
    assert!(content.contains(" first\r\n"));
    assert!(content.contains("+changed 文稿\n\\ No newline at end of file"));
    repository
        .change_index(&path, changed, &hunk, Action::Unstage)
        .unwrap();
    assert_eq!(read_index(), original.as_bytes());
    assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    assert_eq!(fs::read(root.join("main.typ")).unwrap(), b"unrelated\n");
}
