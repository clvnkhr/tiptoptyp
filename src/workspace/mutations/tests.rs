use super::*;

#[test]
fn new_file_is_empty_and_never_overwrites_existing_entries() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let folder = project.path().join("chapters");
    fs::create_dir(&folder).unwrap();
    let target = root.directory(&folder).unwrap();
    let created = target.create_file(OsStr::new("new.tex")).unwrap();
    assert_eq!(created, folder.canonicalize().unwrap().join("new.tex"));
    assert_eq!(fs::read(&created).unwrap(), b"");
    fs::write(&created, b"keep").unwrap();
    assert_eq!(
        target
            .create_file(OsStr::new("new.tex"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&created).unwrap(), b"keep");
    fs::create_dir(folder.join("existing")).unwrap();
    assert!(target.create_file(OsStr::new("existing")).is_err());
    assert!(folder.join("existing").is_dir());
}

#[test]
fn new_file_rejects_paths_reserved_storage_and_control_characters() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let target = root.directory(project.path()).unwrap();
    for name in [
        "",
        ".",
        "..",
        "../outside.tex",
        "nested/file.tex",
        "nested\\file.tex",
        "/absolute.tex",
        ".tiptoptyp",
        "bad\nname.tex",
        "bad\0name",
    ] {
        assert!(target.create_file(OsStr::new(name)).is_err(), "{name:?}");
    }
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 0);
    assert!(root.directory(project.path().parent().unwrap()).is_err());
}

#[test]
fn duplicate_file_keeps_extension_dotfiles_unicode_and_original_contents() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    for (source, expected) in [
        ("paper.tex", "paper copy.tex"),
        ("report.v2.typ", "report.v2 copy.typ"),
        (".env", ".env copy"),
        ("LICENSE", "LICENSE copy"),
        ("测𝛼.tex", "测𝛼 copy.tex"),
    ] {
        let source = project.path().join(source);
        fs::write(&source, b"contents").unwrap();
        let copy = root.duplicate(&source).unwrap();
        assert_eq!(copy.file_name(), Some(OsStr::new(expected)));
        assert_eq!(fs::read(copy).unwrap(), b"contents");
        assert_eq!(fs::read(source).unwrap(), b"contents");
    }
}

#[test]
fn duplicates_choose_unused_sibling_names_without_overwriting() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let source = project.path().join("paper.tex");
    fs::write(&source, b"source").unwrap();
    fs::write(project.path().join("paper copy.tex"), b"first").unwrap();
    fs::create_dir(project.path().join("paper copy 2.tex")).unwrap();
    let copy = root.duplicate(&source).unwrap();
    assert_eq!(copy.file_name(), Some(OsStr::new("paper copy 3.tex")));
    assert_eq!(
        fs::read(project.path().join("paper copy.tex")).unwrap(),
        b"first"
    );
    assert!(project.path().join("paper copy 2.tex").is_dir());
}

#[test]
fn duplicate_folders_copy_structure_and_hidden_files_but_skip_generated_storage() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let source = project.path().join("figures.v2");
    fs::create_dir_all(source.join("nested/.hidden")).unwrap();
    fs::create_dir_all(source.join(".tiptoptyp/cache")).unwrap();
    fs::create_dir_all(source.join("nested/.tiptoptyp/cache")).unwrap();
    fs::write(source.join(".env"), b"setting").unwrap();
    fs::write(source.join("nested/.hidden/image.dat"), b"pixels").unwrap();
    fs::write(source.join(".tiptoptyp/cache/generated.pdf"), b"runtime").unwrap();
    fs::write(
        source.join("nested/.tiptoptyp/cache/generated.pdf"),
        b"runtime",
    )
    .unwrap();
    let copy = root.duplicate(&source).unwrap();
    assert_eq!(copy.file_name(), Some(OsStr::new("figures.v2 copy")));
    assert_eq!(fs::read(copy.join(".env")).unwrap(), b"setting");
    assert_eq!(
        fs::read(copy.join("nested/.hidden/image.dat")).unwrap(),
        b"pixels"
    );
    assert!(!copy.join(".tiptoptyp").exists());
    assert!(!copy.join("nested/.tiptoptyp").exists());
    fs::write(copy.join(".env"), b"changed").unwrap();
    assert_eq!(fs::read(source.join(".env")).unwrap(), b"setting");
    assert!(
        root.duplicate(project.path()).is_err(),
        "a root cannot copy into itself"
    );
}

#[test]
fn managed_ancestors_and_nested_overleaf_copies_are_rejected_at_execution() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let managed = project.path().join("managed");
    fs::create_dir_all(managed.join("sub")).unwrap();
    fs::write(managed.join("sub/main.tex"), b"server source").unwrap();
    let target = root.directory(&managed.join("sub")).unwrap();
    fs::create_dir(managed.join(".tiptoptyp")).unwrap();
    fs::write(
        managed.join(".tiptoptyp/overleaf-project.json"),
        b"descriptor",
    )
    .unwrap();
    assert!(target.create_file(OsStr::new("new.tex")).is_err());
    assert!(root.duplicate(&managed.join("sub/main.tex")).is_err());
    assert!(root.duplicate(&managed).is_err());
    assert!(!project.path().join("managed copy").exists());
    let outer = project.path().join("outer");
    fs::create_dir(&outer).unwrap();
    fs::rename(&managed, outer.join("managed")).unwrap();
    fs::write(outer.join("a.txt"), b"copied before nested rejection").unwrap();
    assert!(root.duplicate(&outer).is_err());
    assert!(!project.path().join("outer copy").exists());
    assert_eq!(
        fs::read(outer.join("managed/sub/main.tex")).unwrap(),
        b"server source"
    );
}

#[cfg(unix)]
#[test]
fn symlink_sources_and_nested_links_are_rejected_and_partial_copies_are_removed() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("private.txt"), b"keep").unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    symlink(
        external.path().join("private.txt"),
        project.path().join("linked.txt"),
    )
    .unwrap();
    assert!(root.duplicate(&project.path().join("linked.txt")).is_err());
    let source = project.path().join("folder");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"first").unwrap();
    symlink(external.path(), source.join("z-link")).unwrap();
    fs::create_dir(project.path().join("folder copy")).unwrap();
    fs::write(project.path().join("folder copy/keep.txt"), b"existing").unwrap();
    assert!(root.duplicate(&source).is_err());
    assert!(!project.path().join("folder copy 2").exists());
    assert_eq!(
        fs::read(project.path().join("folder copy/keep.txt")).unwrap(),
        b"existing"
    );
    assert_eq!(
        fs::read(external.path().join("private.txt")).unwrap(),
        b"keep"
    );
}

#[cfg(unix)]
#[test]
fn replaced_directory_targets_and_collision_symlinks_never_redirect_writes() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let folder = project.path().join("folder");
    fs::create_dir(&folder).unwrap();
    let target = root.directory(&folder).unwrap();
    fs::remove_dir(&folder).unwrap();
    symlink(external.path(), &folder).unwrap();
    assert!(target.create_file(OsStr::new("new.txt")).is_err());
    assert!(!external.path().join("new.txt").exists());
    fs::write(project.path().join("note.tex"), b"source").unwrap();
    fs::write(external.path().join("keep.tex"), b"keep").unwrap();
    symlink(
        external.path().join("keep.tex"),
        project.path().join("note copy.tex"),
    )
    .unwrap();
    let copy = root.duplicate(&project.path().join("note.tex")).unwrap();
    assert_eq!(copy.file_name(), Some(OsStr::new("note copy 2.tex")));
    assert_eq!(fs::read(external.path().join("keep.tex")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn retained_directory_handles_do_not_follow_a_later_path_replacement() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    let canonical = project.path().canonicalize().unwrap();
    let external = tempfile::tempdir().unwrap();
    let folder = canonical.join("folder");
    fs::create_dir(&folder).unwrap();
    let handle = MutationDirectory::workspace(&canonical, &folder).unwrap();
    fs::rename(&folder, canonical.join("moved")).unwrap();
    symlink(external.path(), &folder).unwrap();
    handle.create_file(OsStr::new("local.txt")).unwrap();
    assert!(canonical.join("moved/local.txt").is_file());
    assert!(!external.path().join("local.txt").exists());
}

#[cfg(unix)]
#[test]
fn failed_copy_cleanup_never_follows_a_replaced_parent_path() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    let canonical = project.path().canonicalize().unwrap();
    let external = tempfile::tempdir().unwrap();
    let folder = canonical.join("folder");
    fs::create_dir(&folder).unwrap();
    let handle = MutationDirectory::workspace(&canonical, &folder).unwrap();
    let partial_file = handle.create_file(OsStr::new("partial.txt")).unwrap();
    let file_identity = CreatedIdentity::from_metadata(&partial_file.metadata().unwrap());
    let partial = handle
        .create_directory(OsStr::new("partial-folder"))
        .unwrap();
    let folder_identity = partial.identity().unwrap();
    partial.create_file(OsStr::new("own.txt")).unwrap();
    fs::rename(&folder, canonical.join("moved")).unwrap();
    symlink(external.path(), &folder).unwrap();
    fs::write(external.path().join("partial.txt"), b"keep").unwrap();
    fs::create_dir(external.path().join("partial-folder")).unwrap();
    fs::write(external.path().join("partial-folder/keep.txt"), b"keep").unwrap();
    assert!(
        cleanup_failed_copy(
            &handle,
            OsStr::new("partial.txt"),
            &file_identity,
            false,
            io::Error::other("injected copy failure")
        )
        .is_err()
    );
    assert!(
        cleanup_failed_copy(
            &handle,
            OsStr::new("partial-folder"),
            &folder_identity,
            true,
            io::Error::other("injected copy failure")
        )
        .is_err()
    );
    assert!(!canonical.join("moved/partial.txt").exists());
    assert!(!canonical.join("moved/partial-folder").exists());
    assert_eq!(
        fs::read(external.path().join("partial.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        fs::read(external.path().join("partial-folder/keep.txt")).unwrap(),
        b"keep"
    );
}

#[cfg(unix)]
#[test]
fn duplicated_scripts_preserve_executable_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().unwrap();
    let source = project.path().join("build.sh");
    fs::write(&source, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
    let copy = WorkspaceRoot::open(project.path())
        .unwrap()
        .duplicate(&source)
        .unwrap();
    assert_eq!(
        fs::metadata(copy).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_duplicate_names_preserve_raw_bytes_without_a_filesystem() {
    use std::os::unix::ffi::OsStringExt;
    let name = OsString::from_vec(vec![0xff, b'.', b't', b'e', b'x']);
    assert!(validate_entry_name(&name).is_ok());
    assert_eq!(
        duplicate_name(&name, false, 1).into_vec(),
        vec![0xff, b' ', b'c', b'o', b'p', b'y', b'.', b't', b'e', b'x']
    );
    assert_eq!(
        duplicate_name(&name, false, 2).into_vec(),
        vec![
            0xff, b' ', b'c', b'o', b'p', b'y', b' ', b'2', b'.', b't', b'e', b'x'
        ]
    );
    let mut folder = name.clone().into_vec();
    folder.extend_from_slice(b" copy");
    assert_eq!(duplicate_name(&name, true, 1).into_vec(), folder);
}

// APFS requires valid UTF-8 names, so its raw-byte coverage is the pure test
// above. Linux's ordinary ext4/tmpfs test filesystem also covers actual copying.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_file_names_are_duplicated_without_loss() {
    use std::os::unix::ffi::OsStringExt;
    let project = tempfile::tempdir().unwrap();
    let name = OsString::from_vec(vec![0xff, b'.', b't', b'e', b'x']);
    let source = project.path().join(&name);
    fs::write(&source, b"contents").unwrap();
    let copy = WorkspaceRoot::open(project.path())
        .unwrap()
        .duplicate(&source)
        .unwrap();
    assert_eq!(
        copy.file_name(),
        Some(
            OsString::from_vec(vec![
                0xff, b' ', b'c', b'o', b'p', b'y', b'.', b't', b'e', b'x'
            ])
            .as_os_str()
        )
    );
    assert_eq!(fs::read(copy).unwrap(), b"contents");
}

#[test]
fn choosing_a_subdirectory_does_not_bypass_a_managed_overleaf_ancestor() {
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".tiptoptyp")).unwrap();
    fs::create_dir_all(project.path().join("subdir/chapters")).unwrap();
    fs::write(
        project.path().join(".tiptoptyp/overleaf-project.json"),
        b"descriptor",
    )
    .unwrap();
    let selected = project.path().join("subdir");
    let root = WorkspaceRoot::open(&selected).unwrap();
    let source = selected.join("main.tex");
    fs::write(&source, b"server contents").unwrap();
    let target = root.directory(&selected.join("chapters")).unwrap();
    assert!(target.create_file(OsStr::new("local.tex")).is_err());
    assert!(root.duplicate(&source).is_err());
    assert!(root.rename(&source, OsStr::new("renamed.tex")).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"server contents");
    assert!(!selected.join("renamed.tex").exists());
    assert!(!selected.join("chapters/local.tex").exists());
}

#[cfg(unix)]
#[test]
fn failed_copy_cleanup_preserves_file_and_directory_replacements() {
    let project = tempfile::tempdir().unwrap();
    let canonical = project.path().canonicalize().unwrap();
    let directory = MutationDirectory::workspace(&canonical, &canonical).unwrap();
    let original = directory.create_file(OsStr::new("copy.txt")).unwrap();
    let identity = CreatedIdentity::from_metadata(&original.metadata().unwrap());
    fs::write(canonical.join("copy.txt"), b"original copy").unwrap();
    fs::rename(canonical.join("copy.txt"), canonical.join("moved.txt")).unwrap();
    fs::write(canonical.join("copy.txt"), b"replacement user file").unwrap();
    assert!(
        cleanup_failed_copy(
            &directory,
            OsStr::new("copy.txt"),
            &identity,
            false,
            io::Error::other("injected")
        )
        .is_err()
    );
    assert_eq!(
        fs::read(canonical.join("copy.txt")).unwrap(),
        b"replacement user file"
    );
    assert_eq!(
        fs::read(canonical.join("moved.txt")).unwrap(),
        b"original copy"
    );
    let original = directory
        .create_directory(OsStr::new("copy-folder"))
        .unwrap();
    let identity = original.identity().unwrap();
    fs::write(canonical.join("copy-folder/own.txt"), b"original copy").unwrap();
    fs::rename(
        canonical.join("copy-folder"),
        canonical.join("moved-folder"),
    )
    .unwrap();
    fs::create_dir(canonical.join("copy-folder")).unwrap();
    fs::write(
        canonical.join("copy-folder/user.txt"),
        b"replacement user folder",
    )
    .unwrap();
    assert!(
        cleanup_failed_copy(
            &directory,
            OsStr::new("copy-folder"),
            &identity,
            true,
            io::Error::other("injected")
        )
        .is_err()
    );
    assert_eq!(
        fs::read(canonical.join("copy-folder/user.txt")).unwrap(),
        b"replacement user folder"
    );
    assert_eq!(
        fs::read(canonical.join("moved-folder/own.txt")).unwrap(),
        b"original copy"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn atomic_rename_preserves_sources_and_both_kinds_of_existing_destination() {
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let file = project.path().join("main.tex");
    let folder = project.path().join("chapters");
    fs::write(&file, b"source").unwrap();
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("part.tex"), b"chapter").unwrap();
    fs::write(project.path().join("existing.txt"), b"keep").unwrap();
    fs::create_dir(project.path().join("existing-folder")).unwrap();
    for source in [&file, &folder] {
        for name in ["existing.txt", "existing-folder"] {
            assert!(root.rename(source, OsStr::new(name)).is_err());
            assert!(source.exists());
        }
        for name in ["", "..", "../escape", ".tiptoptyp", "nested/name"] {
            assert!(root.rename(source, OsStr::new(name)).is_err());
            assert!(source.exists());
        }
    }
    assert_eq!(fs::read(&file).unwrap(), b"source");
    assert_eq!(fs::read(folder.join("part.tex")).unwrap(), b"chapter");
    assert_eq!(
        fs::read(project.path().join("existing.txt")).unwrap(),
        b"keep"
    );
    let renamed = root.rename(&file, OsStr::new("renamed.typ")).unwrap();
    assert_eq!(renamed.file_name(), Some(OsStr::new("renamed.typ")));
    assert_eq!(fs::read(&renamed).unwrap(), b"source");
    assert!(!file.exists());
    let renamed_folder = root.rename(&folder, OsStr::new("renamed-folder")).unwrap();
    assert_eq!(
        fs::read(renamed_folder.join("part.tex")).unwrap(),
        b"chapter"
    );
    assert!(!folder.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn atomic_case_only_rename_preserves_the_original_file_identity() {
    use std::os::unix::fs::MetadataExt;
    let project = tempfile::tempdir().unwrap();
    let root = WorkspaceRoot::open(project.path()).unwrap();
    let source = project.path().join("paper.tex");
    fs::write(&source, b"paper").unwrap();
    let identity = fs::metadata(&source).unwrap().ino();
    let renamed = root.rename(&source, OsStr::new("PAPER.tex")).unwrap();
    assert_eq!(fs::metadata(&renamed).unwrap().ino(), identity);
    assert_eq!(fs::read(&renamed).unwrap(), b"paper");
    let names = fs::read_dir(project.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [OsString::from("PAPER.tex")]);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn concurrent_renames_cannot_overwrite_the_winning_destination() {
    use std::sync::Barrier;
    let project = tempfile::tempdir().unwrap();
    let root = Arc::new(WorkspaceRoot::open(project.path()).unwrap());
    let canonical = project.path().canonicalize().unwrap();
    fs::write(canonical.join("a.tex"), b"A").unwrap();
    fs::write(canonical.join("b.tex"), b"B").unwrap();
    let start = Arc::new(Barrier::new(2));
    let jobs = ["a.tex", "b.tex"].map(|name| {
        let root = Arc::clone(&root);
        let start = Arc::clone(&start);
        let source = canonical.join(name);
        std::thread::spawn(move || {
            start.wait();
            (
                source.clone(),
                root.rename(&source, OsStr::new("winner.tex")),
            )
        })
    });
    let results = jobs.map(|job| job.join().unwrap());
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    for (source, result) in results {
        if result.is_ok() {
            assert!(!source.exists());
        } else {
            assert!(source.exists());
        }
    }
    assert!(
        [b"A".as_slice(), b"B".as_slice()]
            .contains(&fs::read(canonical.join("winner.tex")).unwrap().as_slice())
    );
}
