use super::*;

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
