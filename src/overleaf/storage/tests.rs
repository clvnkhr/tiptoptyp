use super::*;
use std::{os::unix::fs::symlink, sync::mpsc};

fn project() -> ProjectUrl {
    ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap()
}
fn base() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
fn temporary() -> (Arc<StorageLease>, mpsc::Receiver<bool>) {
    let options = StorageOptions {
        keep_local_copy: false,
        directory: None,
    };
    let mut lease = StorageLease::prepare(&project(), &options, Path::new("/unused")).unwrap();
    let (send, receive) = mpsc::channel();
    let owned = Arc::get_mut(&mut lease)
        .unwrap()
        .temporary
        .as_mut()
        .unwrap();
    // An intentionally refused replacement cleanup must not poison the global
    // shutdown tracker used by other tests running concurrently.
    owned.tracker.completed(true);
    owned.tracker = Arc::new(CleanupTracker::default());
    owned.tracker.started();
    owned.completed = Some(send);
    (lease, receive)
}

#[test]
fn persistent_copy_defaults_to_a_stable_project_namespace_and_survives_release() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let options = StorageOptions::default();
    let first = StorageLease::prepare(&project(), &options, &base).unwrap();
    let root = first.root().to_owned();
    assert!(first.persistent());
    assert_eq!(root, project().mirror_root(&base));
    assert_ne!(root, base);
    fs::write(root.join("main.tex"), b"draft").unwrap();
    drop(first);
    let second = StorageLease::prepare(&project(), &options, &base).unwrap();
    assert_eq!(second.root(), root);
    assert_eq!(fs::read(root.join("main.tex")).unwrap(), b"draft");
    drop(second);
    assert!(root.join(MARKER).is_file());
}

#[test]
fn selected_folder_is_a_base_not_a_deletion_or_project_root() {
    let selected = base();
    let selected = selected.path().canonicalize().unwrap();
    fs::write(selected.join("unrelated.txt"), b"keep").unwrap();
    let options = StorageOptions {
        keep_local_copy: true,
        directory: Some(selected.clone()),
    };
    let lease = StorageLease::prepare(&project(), &options, Path::new("/unused")).unwrap();
    assert_eq!(lease.root().parent(), Some(selected.as_path()));
    let other =
        ProjectUrl::parse("https://latex.example.org/project/0123456789abcdef01234567").unwrap();
    assert_ne!(
        options
            .root_hint(&other, Path::new("/unused"))
            .unwrap()
            .unwrap(),
        lease.root()
    );
    drop(lease);
    assert_eq!(fs::read(selected.join("unrelated.txt")).unwrap(), b"keep");
    assert!(
        selected
            .join(project().mirror_root(Path::new("")).file_name().unwrap())
            .is_dir()
    );
}

#[test]
fn temporary_copy_is_unique_and_kept_until_the_last_worker_lease_is_released() {
    let (lease, done) = temporary();
    let (other, other_done) = temporary();
    let root = lease.root().to_owned();
    assert!(!lease.persistent());
    assert_ne!(lease.root(), other.root());
    fs::write(root.join("draft.tex"), b"unsaved").unwrap();
    let worker = Arc::clone(&lease);
    drop(lease);
    assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert_eq!(fs::read(root.join("draft.tex")).unwrap(), b"unsaved");
    drop(worker);
    assert!(done.recv_timeout(Duration::from_secs(5)).unwrap());
    assert!(!root.exists());
    drop(other);
    assert!(other_done.recv_timeout(Duration::from_secs(5)).unwrap());
}

#[test]
fn temporary_cleanup_does_not_follow_a_link_inside_its_owned_root() {
    let elsewhere = base();
    let elsewhere = elsewhere.path().canonicalize().unwrap();
    fs::write(elsewhere.join("unrelated.tex"), b"keep").unwrap();
    let (lease, done) = temporary();
    symlink(&elsewhere, lease.root().join("foreign")).unwrap();
    drop(lease);
    assert!(done.recv_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(fs::read(elsewhere.join("unrelated.tex")).unwrap(), b"keep");
}

#[test]
fn temporary_cleanup_refuses_a_replacement_at_its_original_path() {
    let elsewhere = base();
    let elsewhere = elsewhere.path().canonicalize().unwrap();
    fs::write(elsewhere.join("unrelated.tex"), b"keep").unwrap();
    let (lease, done) = temporary();
    let root = lease.root().to_owned();
    let moved = root.with_extension("moved");
    fs::rename(&root, &moved).unwrap();
    symlink(&elsewhere, &root).unwrap();
    drop(lease);
    assert!(!done.recv_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(fs::read(elsewhere.join("unrelated.tex")).unwrap(), b"keep");
    fs::remove_file(root).unwrap();
    fs::remove_dir_all(moved).unwrap();
}

#[test]
fn descriptor_discovery_finds_project_from_root_or_nested_existing_and_new_paths() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let lease = StorageLease::prepare(&project(), &StorageOptions::default(), &base).unwrap();
    fs::create_dir(lease.root().join("chapters")).unwrap();
    fs::write(lease.root().join("chapters/main.tex"), b"text").unwrap();
    for path in [
        lease.root().to_owned(),
        lease.root().join("chapters/main.tex"),
        lease.root().join("chapters/new.tex"),
    ] {
        let stored = StoredProject::from_path(&path).unwrap().unwrap();
        assert_eq!(stored.project, project());
        assert_eq!(stored.root, lease.root());
        assert!(stored.persistent);
    }
    assert!(StoredProject::from_path(&base).unwrap().is_none());
    let (temporary, done) = temporary();
    assert!(
        !StoredProject::from_path(temporary.root())
            .unwrap()
            .unwrap()
            .persistent
    );
    drop(temporary);
    assert!(done.recv_timeout(Duration::from_secs(5)).unwrap());
}

#[test]
fn discovery_is_read_only_and_does_not_create_missing_folders() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let missing = base.join("not-created/deep/main.tex");
    assert!(StoredProject::from_path(&missing).unwrap().is_none());
    assert!(!base.join("not-created").exists());
    assert!(
        StorageOptions::default()
            .root_hint(&project(), Path::new("relative"))
            .is_err()
    );
    assert!(StoredProject::from_path(Path::new("relative/main.tex")).is_err());
    assert!(StoredProject::from_path(&base.join("../escape")).is_err());
}

#[test]
fn descriptor_project_and_version_are_checked_before_reusing_a_folder() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let lease = StorageLease::prepare(&project(), &StorageOptions::default(), &base).unwrap();
    let marker = lease.root().join(MARKER);
    let invalid = [
        r#"{"version":1,"project_url":"https://www.overleaf.com/project/ffffffffffffffffffffffff","persistent":true}"#,
        r#"{"version":2,"project_url":"https://www.overleaf.com/project/0123456789abcdef01234567","persistent":true}"#,
        r#"{"version":1,"project_url":"https://user:secret@www.overleaf.com/project/0123456789abcdef01234567","persistent":true}"#,
        r#"{"version":1,"project_url":"https://www.overleaf.com/project/0123456789abcdef01234567","persistent":false}"#,
        "{broken",
    ];
    for value in invalid {
        fs::write(&marker, value).unwrap();
        assert!(StorageLease::prepare(&project(), &StorageOptions::default(), &base).is_err());
        assert_eq!(fs::read_to_string(&marker).unwrap(), value);
    }
    fs::write(&marker, vec![b' '; MAX_MARKER_BYTES + 1]).unwrap();
    assert!(StoredProject::from_path(lease.root()).is_err());
}

#[test]
fn descriptor_discovery_and_preparation_reject_symlinked_metadata() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let lease = StorageLease::prepare(&project(), &StorageOptions::default(), &base).unwrap();
    let marker = lease.root().join(MARKER);
    let copied = base.join("copied.json");
    fs::rename(&marker, &copied).unwrap();
    symlink(&copied, &marker).unwrap();
    assert!(StoredProject::from_path(lease.root()).is_err());
    assert!(StorageLease::prepare(&project(), &StorageOptions::default(), &base).is_err());
    fs::remove_file(&marker).unwrap();
    fs::rename(&copied, &marker).unwrap();
    let hidden = lease.root().join(".tiptoptyp");
    let moved = base.join("moved-metadata");
    fs::rename(&hidden, &moved).unwrap();
    symlink(&moved, &hidden).unwrap();
    assert!(StoredProject::from_path(lease.root()).is_err());
    assert!(StorageLease::prepare(&project(), &StorageOptions::default(), &base).is_err());
}

#[test]
fn shutdown_waits_for_held_work_and_reports_timeout_or_cleanup_failure() {
    let tracker = Arc::new(CleanupTracker::default());
    tracker.started();
    assert!(!tracker.wait(Duration::ZERO));
    let release = Arc::clone(&tracker);
    std::thread::spawn(move || release.completed(true))
        .join()
        .unwrap();
    assert!(tracker.wait(Duration::from_secs(1)));
    tracker.started();
    tracker.completed(false);
    assert!(!tracker.wait(Duration::from_secs(1)));
}

#[test]
fn selected_storage_base_cannot_follow_a_directory_link() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let target = base.join("real");
    fs::create_dir(&target).unwrap();
    let alias = base.join("alias");
    symlink(&target, &alias).unwrap();
    let options = StorageOptions {
        keep_local_copy: true,
        directory: Some(alias),
    };
    assert!(StorageLease::prepare(&project(), &options, &base).is_err());
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn descriptor_discovery_is_bounded_for_pathological_nesting() {
    let base = base();
    let base = base.path().canonicalize().unwrap();
    let deep = (0..MAX_ANCESTORS + 1).fold(base.clone(), |path, _| path.join("x"));
    assert!(
        StoredProject::from_path(&deep)
            .unwrap_err()
            .contains("deeply")
    );
    assert_eq!(fs::read_dir(base).unwrap().count(), 0);
}
