#[path = "../build_support/build_identity.rs"]
mod build_identity;

use std::{
    fs::{self, File, FileTimes},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

struct Repository {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Repository {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let repository = Self {
            _directory: directory,
            root,
        };
        repository.git(&["init", "--quiet", "--initial-branch=main"]);
        for (key, value) in [
            ("user.name", "Build identity fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.fsmonitor", "false"),
            ("core.untrackedCache", "false"),
            ("core.hooksPath", "disabled-hooks"),
            ("core.excludesFile", "empty-global-excludes"),
            ("core.attributesFile", "empty-global-attributes"),
        ] {
            repository.git(&["config", "--local", key, value]);
        }
        repository.write("src/fixture.rs", "fn fixture() {}\n");
        repository.write("Cargo.toml", "# build identity fixture\n");
        repository.write("README.md", "Ordinary documentation.\n");
        repository.git(&["add", "."]);
        repository.commit("Initial fixture");
        repository
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn git(&self, arguments: &[&str]) -> String {
        git(self.root(), arguments)
    }

    fn commit(&self, message: &str) {
        self.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
    }

    fn identity(&self) -> String {
        build_identity::build_id(self.root(), 17)
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        build_identity::watch_paths(self.root())
    }
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("missing-global-config"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("Git is required for build provenance tests");
    assert!(
        output.status.success(),
        "Git fixture command {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

#[test]
fn observes_current_head_and_relevant_working_tree_changes() {
    let repository = Repository::new();
    let revision = repository.git(&["rev-parse", "--short=12", "HEAD"]);
    assert_eq!(repository.identity(), format!("{revision}.17"));

    repository.write("src/fixture.rs", "fn changed() {}\n");
    assert_eq!(repository.identity(), format!("{revision}-dirty.17"));
    repository.git(&["add", "src/fixture.rs"]);
    assert_eq!(repository.identity(), format!("{revision}-dirty.17"));
    repository.commit("Changed source");
    let next = repository.git(&["rev-parse", "--short=12", "HEAD"]);
    assert_ne!(next, revision);
    assert_eq!(repository.identity(), format!("{next}.17"));
}

#[test]
fn staging_different_bytes_does_not_mislabel_the_actual_compiled_source() {
    let repository = Repository::new();
    let clean = repository.identity();
    repository.write("src/fixture.rs", "fn staged_but_not_compiled() {}\n");
    repository.git(&["add", "src/fixture.rs"]);
    repository.write("src/fixture.rs", "fn fixture() {}\n");
    assert_eq!(repository.identity(), clean);
}

#[test]
fn current_branch_metadata_moves_identity_without_source_changes() {
    let repository = Repository::new();
    let before = repository.identity();
    let paths = repository.watch_paths();
    repository.commit("Move current branch without editing inputs");
    assert_ne!(repository.identity(), before);
    assert_eq!(repository.watch_paths(), paths);
    assert!(paths.contains(&repository.root().join(".git/refs/heads/main")));
}

#[test]
fn unrelated_documents_staging_and_other_branches_are_not_inputs() {
    let repository = Repository::new();
    let clean = repository.identity();
    let paths = repository.watch_paths();
    let index = repository.root().join(".git/index");
    let refs = repository.root().join(".git/refs");
    assert!(!paths.contains(&index));
    assert!(!paths.contains(&refs));
    assert!(paths.contains(&repository.root().join(".git/HEAD")));
    assert!(paths.contains(&repository.root().join(".git/refs/heads/main")));

    repository.write("README.md", "Changed documentation.\n");
    repository.write("personal.typ", "Personal document.\n");
    repository.git(&["add", "README.md", "personal.typ"]);
    repository.git(&["branch", "other"]);
    let other = repository.git(&[
        "commit-tree",
        "HEAD^{tree}",
        "-p",
        "HEAD",
        "-m",
        "Other branch only",
    ]);
    repository.git(&["update-ref", "refs/heads/other", &other]);
    assert_eq!(repository.identity(), clean);
    assert_eq!(repository.watch_paths(), paths);
    assert!(!paths.contains(&repository.root().join(".git/refs/heads/other")));
}

#[test]
fn new_untracked_build_inputs_are_dirty_and_ignored_build_outputs_are_not() {
    let repository = Repository::new();
    let clean = repository.identity();
    repository.write("src/new.rs", "fn newly_added() {}\n");
    assert!(repository.identity().contains("-dirty.17"));
    fs::remove_file(repository.root().join("src/new.rs")).unwrap();
    repository.write(".gitignore", "src/generated/\n");
    repository.write("src/generated/temporary.rs", "ignored build output\n");
    assert_eq!(repository.identity(), clean);
}

#[test]
fn embedded_resources_and_vendored_source_participate_in_provenance() {
    for path in [
        "assets/fixture.bin",
        "resources/fixture.json",
        "vendor/library/src/lib.rs",
        "toolchain/manifest.tsv",
        "manual-tests/templates/article.tex",
        "docs/ui-snapshots/gallery-manifest.tsv",
        "scripts/fixtures/table-editor.typ",
        "tests/fixtures/arxiv-tables/platonic-metrics.tex",
    ] {
        let repository = Repository::new();
        repository.write(path, "Build input.\n");
        assert!(repository.identity().contains("-dirty.17"), "{path}");
        assert!(
            repository
                .watch_paths()
                .iter()
                .any(|watch| { repository.root().join(path).starts_with(watch) }),
            "{path}"
        );
    }
}

#[test]
fn packed_refs_use_existing_fallbacks_then_follow_the_new_loose_ref() {
    let repository = Repository::new();
    repository.git(&["branch", "other"]);
    repository.git(&["pack-refs", "--all", "--prune"]);
    let reference = repository.root().join(".git/refs/heads/main");
    assert!(!reference.exists());
    let paths = repository.watch_paths();
    assert!(paths.contains(&repository.root().join(".git/packed-refs")));
    assert!(paths.contains(&repository.root().join(".git/refs/heads")));
    assert!(!paths.contains(&reference));
    assert!(
        paths
            .iter()
            .filter(|path| path.starts_with(repository.root().join(".git")))
            .all(|path| path.exists())
    );

    let previous = repository.identity();
    repository.commit("Move packed branch");
    assert_ne!(repository.identity(), previous);
    let paths = repository.watch_paths();
    assert!(paths.contains(&reference));
    assert!(!paths.contains(&repository.root().join(".git/packed-refs")));
    assert!(!paths.contains(&repository.root().join(".git/refs/heads")));
}

#[test]
fn detached_head_does_not_watch_branch_activity() {
    let repository = Repository::new();
    repository.git(&["checkout", "--quiet", "--detach"]);
    let paths = repository.watch_paths();
    assert!(paths.contains(&repository.root().join(".git/HEAD")));
    assert!(
        !paths
            .iter()
            .any(|path| path.starts_with(repository.root().join(".git/refs")))
    );
    let previous = repository.identity();
    repository.commit("Move detached HEAD");
    assert_ne!(repository.identity(), previous);
    assert_eq!(repository.watch_paths(), paths);
}

#[test]
fn an_unborn_branch_never_emits_a_missing_git_watch_path() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=unborn"]);
    let metadata = root.join(".git");
    let paths = build_identity::watch_paths(&root);
    assert!(paths.contains(&metadata.join("HEAD")));
    assert!(paths.contains(&metadata.join("refs/heads")));
    assert!(!paths.contains(&metadata.join("refs/heads/unborn")));
    assert!(
        paths
            .iter()
            .filter(|path| path.starts_with(&metadata))
            .all(|path| path.exists())
    );
    assert_eq!(
        build_identity::build_id(&root, 23),
        "unknown-revision-unknown-state.23"
    );
}

#[test]
fn linked_worktrees_watch_their_own_head_and_the_shared_current_branch_ref() {
    let repository = Repository::new();
    let directory = tempfile::tempdir().unwrap();
    let linked = directory.path().join("linked worktree");
    let output = Command::new("git")
        .current_dir(repository.root())
        .args(["worktree", "add", "--quiet", "-b", "linked"])
        .arg(&linked)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(linked.join(".git").is_file());
    let paths = build_identity::watch_paths(&linked);
    let head = PathBuf::from(git(
        &linked,
        &["rev-parse", "--path-format=absolute", "--git-path", "HEAD"],
    ));
    assert!(paths.contains(&head));
    assert!(paths.contains(&repository.root().join(".git/refs/heads/linked")));
    assert!(!paths.contains(&repository.root().join(".git/HEAD")));
    assert!(!paths.contains(&repository.root().join(".git/refs/heads/main")));
    let before = build_identity::build_id(&linked, 17);
    git(
        &linked,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Move linked branch",
        ],
    );
    assert_ne!(build_identity::build_id(&linked, 17), before);
    assert_eq!(repository.identity(), before);
}

#[test]
fn provenance_observation_cannot_refresh_the_index() {
    let repository = Repository::new();
    let index = repository.root().join(".git/index");
    let before = fs::read(&index).unwrap();
    let modified = fs::metadata(&index).unwrap().modified().unwrap();
    let file = File::options()
        .write(true)
        .open(repository.root().join("src/fixture.rs"))
        .unwrap();
    file.set_times(FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(60)))
        .unwrap();
    assert!(!repository.identity().contains("-dirty"));
    repository.watch_paths();
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), modified);
}

#[cfg(unix)]
#[test]
fn executable_mode_changes_and_deleted_source_are_dirty_without_index_writes() {
    use std::os::unix::fs::PermissionsExt;
    let repository = Repository::new();
    let clean = repository.identity();
    let index = repository.root().join(".git/index");
    let before = fs::read(&index).unwrap();
    let source = repository.root().join("src/fixture.rs");
    let permissions = fs::metadata(&source).unwrap().permissions();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(repository.identity().contains("-dirty.17"));
    fs::set_permissions(&source, permissions).unwrap();
    assert_eq!(repository.identity(), clean);
    fs::remove_file(source).unwrap();
    assert!(repository.identity().contains("-dirty.17"));
    assert_eq!(fs::read(index).unwrap(), before);
}

#[test]
fn source_archives_without_git_have_explicit_unknown_provenance() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        build_identity::build_id(directory.path(), 29),
        "unknown-revision-unknown-state.29"
    );
    assert!(
        !build_identity::watch_paths(directory.path())
            .iter()
            .any(|path| path.to_string_lossy().contains(".git"))
    );
}
