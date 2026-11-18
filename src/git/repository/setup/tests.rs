use super::*;

fn request(workspace: &Path) -> SetupRequest {
    let expected = inspect(workspace).unwrap();
    SetupRequest {
        origin: expected.origin.clone(),
        expected,
        initial_branch: "backup-main".into(),
        author_name: "Project Author".into(),
        author_email: "author@example.invalid".into(),
    }
}

#[test]
fn setup_initializes_only_the_chosen_workspace_and_keeps_editor_data_local() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    fs::create_dir(root.join(".tiptoptyp")).unwrap();
    fs::write(root.join(".tiptoptyp/session.json"), b"private session").unwrap();
    for (name, content) in [
        ("main.tex", "source"),
        ("figure.pdf", "PDF asset"),
        ("references.bib", "references"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    let before = inspect(root).unwrap();
    assert!(!before.initialized);
    assert!(!root.join(".git").exists(), "inspection must be read-only");
    let ready = apply(root, request(root)).unwrap();
    assert!(ready.initialized);
    assert_eq!(ready.root, root.canonicalize().unwrap());
    assert_eq!(ready.branch, "backup-main");
    assert_eq!(ready.author_name, "Project Author");
    assert_eq!(ready.author_email, "author@example.invalid");
    assert!(text_run(root, &["rev-parse", "--verify", "HEAD"]).is_err());
    assert!(
        text_run(root, &["diff", "--cached", "--name-only"])
            .unwrap()
            .is_empty()
    );
    assert!(!root.join(".gitignore").exists());
    let status = text_run(root, &["status", "--porcelain", "--untracked-files=all"]).unwrap();
    assert!(!status.contains(".tiptoptyp"));
    assert!(
        status.contains("figure.pdf"),
        "PDF project assets stay visible to Git"
    );
    let configuration = text_run(root, &["config", "--local", "--get", "user.name"]).unwrap();
    assert_eq!(configuration, "Project Author");
}

#[test]
fn invalid_setup_fields_never_create_a_repository() {
    for (name, email, branch, origin) in [
        ("Name\nInjected", "a@example.invalid", "main", ""),
        ("Name", "not-an-email", "main", ""),
        ("Name", "a@example.invalid", "bad..branch", ""),
        ("Name", "a@example.invalid", "main", "ext::sh -c echo bad"),
        (
            "Name",
            "a@example.invalid",
            "main",
            "https://secret@host/owner/repo.git",
        ),
        (
            "Name",
            "a@example.invalid",
            "main",
            "https://host/owner/repo.git?token=secret",
        ),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let mut setup = request(workspace.path());
        setup.author_name = name.into();
        setup.author_email = email.into();
        setup.initial_branch = branch.into();
        setup.origin = origin.into();
        assert!(apply(workspace.path(), setup).is_err());
        assert!(
            !workspace.path().join(".git").exists(),
            "{name} {branch} {origin}"
        );
    }
}

#[test]
fn remote_validation_accepts_normal_git_transports_and_absolute_local_backups() {
    for origin in [
        "https://host.example/owner/repo.git",
        "ssh://git@host.example/owner/repo.git",
        "git@host.example:owner/repo.git",
        "host.example:owner/repo.git",
        "/Volumes/Backup Disk/project.git",
        "file:///tmp/project.git",
    ] {
        assert!(validate_origin(origin).is_ok(), "{origin}");
    }
    for origin in [
        "-command",
        "ext::anything",
        "file://remote.example/project.git",
        "https://host.example",
        "https://u:p@host.example/repo.git",
        "ssh://u:p@host.example/repo.git",
        "git@host.example:",
        "git@host.example:repo\ninjected",
    ] {
        assert!(validate_origin(origin).is_err(), "{origin}");
    }
}

#[test]
fn setup_uses_the_existing_parent_repository_and_does_not_change_its_branch() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    apply(root, request(root)).unwrap();
    fs::create_dir(root.join("paper")).unwrap();
    let selected = root.join("paper");
    let mut update = request(&selected);
    assert_eq!(update.expected.root, root.canonicalize().unwrap());
    update.author_name = "Local Paper Author".into();
    update.initial_branch = "must-not-switch".into();
    let ready = apply(&selected, update).unwrap();
    assert_eq!(ready.root, root.canonicalize().unwrap());
    assert_eq!(ready.branch, "backup-main");
    assert!(!selected.join(".git").exists());
    assert_eq!(
        text_run(root, &["config", "--local", "user.name"]).unwrap(),
        "Local Paper Author"
    );
}

#[test]
fn setup_refuses_stale_workspace_configuration_and_newly_appearing_repositories() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stale = request(root);
    text_run(root, &["init"]).unwrap();
    assert!(apply(root, stale).unwrap_err().contains("changed"));
    let mut stale = request(root);
    stale.origin = "https://host.example/new.git".into();
    text_run(root, &["config", "--local", "user.name", "Someone Else"]).unwrap();
    let before = fs::read(root.join(".git/config")).unwrap();
    assert!(apply(root, stale).unwrap_err().contains("changed"));
    assert_eq!(fs::read(root.join(".git/config")).unwrap(), before);
    let other = tempfile::tempdir().unwrap();
    assert!(
        apply(other.path(), request(root))
            .unwrap_err()
            .contains("workspace changed")
    );
    assert!(!other.path().join(".git").exists());
}

#[test]
fn setup_preserves_unrelated_local_configuration_and_exclude_bytes() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    text_run(root, &["init"]).unwrap();
    text_run(root, &["config", "--local", "example.keep", "untouched"]).unwrap();
    let exclude = root.join(".git/info/exclude");
    let original = b"# personal rules\n*.scratch\nno-newline";
    fs::write(&exclude, original).unwrap();
    let mut setup = request(root);
    setup.origin = "https://host.example/owner/repo.git".into();
    apply(root, setup).unwrap();
    assert_eq!(
        text_run(root, &["config", "--local", "example.keep"]).unwrap(),
        "untouched"
    );
    assert!(fs::read(&exclude).unwrap().starts_with(original));
    let expected = fs::read(&exclude).unwrap();
    apply(root, request(root)).unwrap();
    assert_eq!(
        fs::read(&exclude).unwrap(),
        expected,
        "the ignore rule is idempotent"
    );
    assert_eq!(
        text_run(root, &["remote", "get-url", "origin"]).unwrap(),
        "https://host.example/owner/repo.git"
    );
    let mut cleared = request(root);
    cleared.origin.clear();
    apply(root, cleared).unwrap();
    assert!(
        text_run(root, &["remote", "get-url", "origin"]).is_err(),
        "explicitly clearing the form removes origin"
    );
}

#[test]
fn setup_refuses_flattening_multiple_remotes_or_separate_push_urls() {
    for separate_push in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        apply(root, request(root)).unwrap();
        text_run(
            root,
            &["remote", "add", "origin", "https://host.example/first.git"],
        )
        .unwrap();
        text_run(
            root,
            &[
                "config",
                "--add",
                if separate_push {
                    "remote.origin.pushurl"
                } else {
                    "remote.origin.url"
                },
                "https://host.example/second.git",
            ],
        )
        .unwrap();
        let mut setup = request(root);
        setup.origin = "https://host.example/replacement.git".into();
        let before = fs::read(root.join(".git/config")).unwrap();
        assert!(
            apply(root, setup)
                .unwrap_err()
                .contains("multiple URLs or a separate push URL")
        );
        assert_eq!(fs::read(root.join(".git/config")).unwrap(), before);
        assert!(!root.join(".git/config.lock").exists());
    }
}

#[test]
fn existing_url_credentials_are_hidden_and_preserved_when_not_changed() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    apply(root, request(root)).unwrap();
    let secret = "https://account:secret@host.example/owner/repo.git?token=private";
    text_run(root, &["remote", "add", "origin", secret]).unwrap();
    let mut setup = request(root);
    assert_eq!(setup.expected.origin, "https://host.example/owner/repo.git");
    assert!(!format!("{:?}", setup.expected).contains("secret"));
    setup.origin = setup.expected.origin.clone();
    apply(root, setup).unwrap();
    assert_eq!(
        text_run(root, &["config", "--local", "remote.origin.url"]).unwrap(),
        secret
    );
}

#[test]
fn setup_does_not_replace_busy_config_or_linked_ignore_files() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    apply(root, request(root)).unwrap();
    let expected = fs::read(root.join(".git/config")).unwrap();
    fs::write(root.join(".git/config.lock"), b"other git process").unwrap();
    assert!(apply(root, request(root)).unwrap_err().contains("busy"));
    assert_eq!(fs::read(root.join(".git/config")).unwrap(), expected);
    assert_eq!(
        fs::read(root.join(".git/config.lock")).unwrap(),
        b"other git process"
    );
    #[cfg(unix)]
    {
        fs::remove_file(root.join(".git/config.lock")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let private = outside.path().join("exclude");
        fs::write(&private, b"preserve").unwrap();
        fs::remove_file(root.join(".git/info/exclude")).unwrap();
        std::os::unix::fs::symlink(&private, root.join(".git/info/exclude")).unwrap();
        assert!(apply(root, request(root)).is_err());
        assert_eq!(fs::read(&private).unwrap(), b"preserve");
    }
}
