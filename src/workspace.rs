use std::{
    borrow::Cow,
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

/// Workspace mutations require a target resolved through its owning root.
pub(crate) struct WorkspaceRoot(PathBuf);
pub(crate) struct WorkspaceDirectory {
    root: PathBuf,
    path: PathBuf,
}
pub(crate) struct WorkspaceFile {
    root: PathBuf,
    path: PathBuf,
}
impl WorkspaceRoot {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let root = path.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::other("workspace root is not a directory"));
        }
        Ok(Self(root))
    }
    pub(crate) fn directory(&self, path: &Path) -> io::Result<WorkspaceDirectory> {
        let path = path.canonicalize()?;
        if !path.starts_with(&self.0) || !path.is_dir() {
            return Err(io::Error::other("the destination is outside the workspace"));
        }
        Ok(WorkspaceDirectory {
            root: self.0.clone(),
            path,
        })
    }
    pub(crate) fn file(&self, path: &Path) -> io::Result<WorkspaceFile> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?
            .canonicalize()?;
        if !parent.starts_with(&self.0) || !fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(io::Error::other(
                "only regular workspace files can be deleted",
            ));
        }
        Ok(WorkspaceFile {
            root: self.0.clone(),
            path: path.to_owned(),
        })
    }
}
impl WorkspaceDirectory {
    pub(crate) fn import(&self, source: &Path) -> io::Result<PathBuf> {
        import_file(&self.root, &self.path, source)
    }
}
impl WorkspaceFile {
    pub(crate) fn delete(self) -> io::Result<()> {
        delete_file(&self.root, &self.path)
    }
}

/// Revalidate at execution; a validated target is not protection against an
/// external filesystem race between validation and the OS operation itself.
fn import_file(root: &Path, directory: &Path, source: &Path) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let directory = directory.canonicalize()?;
    if !directory.starts_with(&root) || !directory.is_dir() {
        return Err(io::Error::other("the destination is outside the workspace"));
    }
    if !fs::symlink_metadata(source)?.file_type().is_file() {
        return Err(io::Error::other("only regular files can be imported"));
    }
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::other("missing file name"))?;
    let destination = directory.join(name);
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    let mut copy = || -> io::Result<()> {
        io::copy(&mut input, &mut output)?;
        output.set_permissions(input.metadata()?.permissions())?;
        output.sync_all()
    };
    if let Err(error) = copy() {
        drop(output);
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    Ok(destination)
}

fn delete_file(root: &Path, path: &Path) -> io::Result<()> {
    let root = root.canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?
        .canonicalize()?;
    if !parent.starts_with(root) || !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::other(
            "only regular workspace files can be deleted",
        ));
    }
    fs::remove_file(path)
}

/// The kind of an entry in the project tree.
///
/// Symlinks are surfaced so the filesystem panel does not silently hide them,
/// but they are never followed. Callers should only open `File` nodes as
/// project files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkspaceNodeKind {
    Directory,
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceNode {
    /// The original filesystem name. Keeping this as `OsString` avoids losing
    /// otherwise valid project files whose names are not UTF-8.
    pub name: OsString,
    /// Absolute path below the canonical project root. Symlink paths are not
    /// canonicalized, so this value itself never points outside the root.
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub kind: WorkspaceNodeKind,
    /// Sorted children. This is always empty for files and symlinks.
    pub children: Vec<WorkspaceNode>,
}

impl WorkspaceNode {
    pub fn display_name(&self) -> Cow<'_, str> {
        self.name.to_string_lossy()
    }

    pub fn is_directory(&self) -> bool {
        self.kind == WorkspaceNodeKind::Directory
    }

    pub fn is_file(&self) -> bool {
        self.kind == WorkspaceNodeKind::File
    }

    pub fn is_symlink(&self) -> bool {
        self.kind == WorkspaceNodeKind::Symlink
    }

    pub fn find(&self, relative_path: &Path) -> Option<&WorkspaceNode> {
        if self.relative_path == relative_path {
            return Some(self);
        }
        if !relative_path.starts_with(&self.relative_path) {
            return None;
        }
        self.children
            .iter()
            .find_map(|child| child.find(relative_path))
    }
}

/// An immutable, deterministic view of a project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub root: PathBuf,
    pub nodes: Vec<WorkspaceNode>,
}

impl WorkspaceSnapshot {
    pub fn scan(root: impl AsRef<Path>) -> io::Result<Self> {
        Self::scan_canonical(root.as_ref().canonicalize()?)
    }

    fn scan_canonical(root: PathBuf) -> io::Result<Self> {
        if !fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("workspace root is not a directory: {}", root.display()),
            ));
        }

        let mut visited = HashSet::from([root.clone()]);
        let nodes = scan_directory(&root, &root, Path::new(""), &mut visited)?;
        Ok(Self { root, nodes })
    }

    pub fn find(&self, relative_path: impl AsRef<Path>) -> Option<&WorkspaceNode> {
        let relative_path = relative_path.as_ref();
        self.nodes.iter().find_map(|node| node.find(relative_path))
    }
}

/// A project tree whose last successful snapshot remains cached until an
/// explicit refresh. This keeps filesystem traversal out of egui's frame loop.
#[derive(Debug, Clone)]
pub struct WorkspaceTree {
    snapshot: WorkspaceSnapshot,
    generation: u64,
}

impl WorkspaceTree {
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            snapshot,
            generation: 0,
        }
    }

    pub fn root(&self) -> &Path {
        &self.snapshot.root
    }

    pub fn snapshot(&self) -> &WorkspaceSnapshot {
        &self.snapshot
    }

    /// Increments only when the visible tree structure changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Apply a completed background scan, incrementing the visible generation
    /// only when the tree structure changed.
    pub fn apply_snapshot(&mut self, next: WorkspaceSnapshot) -> bool {
        if next.root != self.snapshot.root || next.nodes == self.snapshot.nodes {
            return false;
        }

        self.snapshot = next;
        self.generation = self.generation.wrapping_add(1);
        true
    }
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    relative_directory: &Path,
    visited: &mut HashSet<PathBuf>,
) -> io::Result<Vec<WorkspaceNode>> {
    let mut nodes = Vec::new();

    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_excluded(&name) {
            continue;
        }

        let file_type = entry.file_type()?;
        let relative_path = relative_directory.join(&name);
        let path = root.join(&relative_path);

        let (kind, children) = if file_type.is_symlink() {
            (WorkspaceNodeKind::Symlink, Vec::new())
        } else if file_type.is_dir() {
            // The file type check prevents following ordinary symlinks. The
            // canonical containment check additionally protects against
            // platform-specific reparse points and unusual filesystem mounts.
            let canonical = match path.canonicalize() {
                Ok(canonical) if canonical.starts_with(root) => canonical,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let children = if visited.insert(canonical.clone()) {
                scan_directory(root, &canonical, &relative_path, visited)?
            } else {
                Vec::new()
            };
            (WorkspaceNodeKind::Directory, children)
        } else if file_type.is_file() {
            (WorkspaceNodeKind::File, Vec::new())
        } else {
            // Sockets, devices, and other special files are not useful editor
            // project entries and may not be safe to open as ordinary files.
            continue;
        };

        nodes.push(WorkspaceNode {
            name,
            path,
            relative_path,
            kind,
            children,
        });
    }

    nodes.sort_unstable_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(nodes)
}

fn is_excluded(name: &OsStr) -> bool {
    if matches!(name.to_str(), Some(".git" | "target"))
        || name == OsStr::new(crate::private_workspace::PRIVATE_DIRECTORY_NAME)
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    #[cfg(unix)]
    #[test]
    fn validated_delete_rechecks_a_target_replaced_by_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("external.txt");
        fs::write(&external, "keep").unwrap();
        let path = root.path().join("note.txt");
        fs::write(&path, "initial").unwrap();
        let workspace = WorkspaceRoot::open(root.path()).unwrap();
        let target = workspace.file(&path).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&external, &path).unwrap();
        assert!(target.delete().is_err());
        assert_eq!(fs::read_to_string(&external).unwrap(), "keep");
        assert!(workspace.directory(outside.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn importing_a_script_preserves_its_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let source = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let script = source.path().join("build.sh");
        fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o750)).unwrap();
        let imported = import_file(project.path(), project.path(), &script).unwrap();
        assert_eq!(
            fs::metadata(imported).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    struct TempProject {
        directory: tempfile::TempDir,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            let prefix = format!("tiptoptyp-workspace-{label}-");
            let directory = tempfile::Builder::new().prefix(&prefix).tempdir().unwrap();
            Self { directory }
        }

        fn path(&self) -> &Path {
            self.directory.path()
        }

        fn directory(&self, relative: impl AsRef<Path>) {
            fs::create_dir_all(self.path().join(relative)).unwrap();
        }

        fn file(&self, relative: impl AsRef<Path>, contents: &str) {
            let path = self.path().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }
    }

    fn workspace_tree(root: &Path) -> WorkspaceTree {
        WorkspaceTree::from_snapshot(WorkspaceSnapshot::scan(root).unwrap())
    }

    fn refresh_workspace_tree(tree: &mut WorkspaceTree) -> io::Result<bool> {
        // The cached root is already canonical, matching the background scan
        // that production applies through `apply_snapshot`.
        let next = WorkspaceSnapshot::scan_canonical(tree.snapshot.root.clone())?;
        Ok(tree.apply_snapshot(next))
    }

    fn names(nodes: &[WorkspaceNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| node.display_name().into_owned())
            .collect()
    }

    #[test]
    fn import_preserves_sources_and_rejects_collisions_and_outside_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let folder = root.join("chapters");
        fs::create_dir_all(&folder).unwrap();
        let source = temp.path().join("sample.typ");
        fs::write(&source, "hello").unwrap();
        let imported = import_file(&root, &folder, &source).unwrap();
        assert_eq!(fs::read_to_string(&imported).unwrap(), "hello");
        fs::write(&source, "changed").unwrap();
        assert!(import_file(&root, &folder, &source).is_err());
        assert_eq!(fs::read_to_string(&imported).unwrap(), "hello");
        assert!(source.exists());
        assert!(import_file(&root, temp.path(), &source).is_err());
        assert!(import_file(&root, &folder, &root).is_err());
        assert!(delete_file(&root, &source).is_err());
        assert!(delete_file(&root, &folder).is_err());
        delete_file(&root, &imported).unwrap();
        assert!(!imported.exists());
    }

    #[cfg(unix)]
    #[test]
    fn file_operations_do_not_follow_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let source = temp.path().join("private.typ");
        fs::write(&source, "preserve").unwrap();
        let link = root.join("link.typ");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(import_file(&root, &root, &link).is_err());
        assert!(delete_file(&root, &link).is_err());
        let outside = root.join("outside");
        std::os::unix::fs::symlink(temp.path(), &outside).unwrap();
        assert!(import_file(&root, &outside, &source).is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "preserve");
    }

    #[test]
    fn sorts_directories_before_files_then_by_name() {
        let project = TempProject::new("sorting");
        project.directory("z-dir");
        project.directory("a-dir");
        project.file("b.typ", "b");
        project.file("A.typ", "a");

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert_eq!(
            names(&snapshot.nodes),
            vec!["a-dir", "z-dir", "A.typ", "b.typ"]
        );
    }

    #[test]
    fn sorting_is_deterministic_at_every_depth() {
        let project = TempProject::new("nested-sorting");
        project.file("chapters/z.typ", "z");
        project.file("chapters/a.typ", "a");
        project.directory("chapters/assets-z");
        project.directory("chapters/assets-a");

        let first = WorkspaceSnapshot::scan(project.path()).unwrap();
        let second = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            names(&first.find("chapters").unwrap().children),
            vec!["assets-a", "assets-z", "a.typ", "z.typ"]
        );
    }

    #[test]
    fn excludes_repository_build_and_live_preview_artifacts() {
        let project = TempProject::new("exclusions");
        project.file(".git/config", "git");
        project.file("target/debug/output", "build");
        project.file("nested/target/output", "build");
        project.file(".tiptoptyp/private/preview.typ", "private");
        project.file("nested/.tiptoptyp/private/preview.typ", "private");
        project.file("target.typ", "useful");

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert!(snapshot.find(".git").is_none());
        assert!(snapshot.find("target").is_none());
        assert!(snapshot.find("nested/target").is_none());
        assert!(snapshot.find(".tiptoptyp").is_none());
        assert!(snapshot.find("nested/.tiptoptyp").is_none());
        assert!(snapshot.find("target.typ").is_some());
    }

    #[test]
    fn includes_typst_sources_assets_data_and_other_regular_files() {
        let project = TempProject::new("useful-files");
        for file in [
            "main.typ",
            "figure.png",
            "references.bib",
            "data.csv",
            "theme.toml",
            ".editorconfig",
            "LICENSE",
        ] {
            project.file(file, "contents");
        }

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        for file in [
            "main.typ",
            "figure.png",
            "references.bib",
            "data.csv",
            "theme.toml",
            ".editorconfig",
            "LICENSE",
        ] {
            let node = snapshot.find(file).unwrap();
            assert!(node.is_file(), "{file} should be a regular file node");
            assert!(node.children.is_empty());
        }
    }

    #[test]
    fn caches_until_refresh_and_tracks_structural_generations() {
        let project = TempProject::new("cache");
        project.file("main.typ", "first");
        let mut tree = workspace_tree(project.path());
        assert_eq!(tree.generation(), 0);

        project.file("chapter.typ", "chapter");
        assert!(tree.snapshot().find("chapter.typ").is_none());
        assert!(refresh_workspace_tree(&mut tree).unwrap());
        assert!(tree.snapshot().find("chapter.typ").is_some());
        assert_eq!(tree.generation(), 1);

        assert!(!refresh_workspace_tree(&mut tree).unwrap());
        assert_eq!(tree.generation(), 1);

        // Contents do not affect the filesystem panel's structural snapshot.
        project.file("main.typ", "second");
        assert!(!refresh_workspace_tree(&mut tree).unwrap());
        assert_eq!(tree.generation(), 1);
    }

    #[test]
    fn paths_are_relative_to_the_canonical_root() {
        let project = TempProject::new("paths");
        project.file("chapters/one.typ", "one");

        let tree = workspace_tree(project.path());
        let node = tree.snapshot().find("chapters/one.typ").unwrap();
        assert_eq!(tree.root(), project.path().canonicalize().unwrap());
        assert_eq!(node.relative_path, PathBuf::from("chapters/one.typ"));
        assert_eq!(node.path, tree.root().join("chapters/one.typ"));
    }

    #[test]
    fn rejects_a_file_as_the_workspace_root() {
        let project = TempProject::new("file-root");
        project.file("main.typ", "main");
        let error = WorkspaceSnapshot::scan(project.path().join("main.typ")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn find_distinguishes_nested_paths_with_similar_prefixes() {
        let project = TempProject::new("find-prefixes");
        project.file("a/deep/one.typ", "one");
        project.file("ab/deep/two.typ", "two");

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert_eq!(
            snapshot.find("a/deep/one.typ").unwrap().display_name(),
            "one.typ"
        );
        assert_eq!(
            snapshot.find("ab/deep/two.typ").unwrap().display_name(),
            "two.typ"
        );
        assert!(snapshot.find("a/deep/two.typ").is_none());
        assert!(snapshot.find("").is_none());
        assert!(
            snapshot
                .find(project.path().join("a/deep/one.typ"))
                .is_none()
        );
    }

    #[test]
    fn a_failed_refresh_preserves_the_cached_snapshot_and_generation() {
        let project = TempProject::new("failed-refresh");
        project.file("main.typ", "main");
        let mut tree = workspace_tree(project.path());
        let cached = tree.snapshot().clone();

        fs::remove_dir_all(project.path()).unwrap();
        assert_eq!(
            refresh_workspace_tree(&mut tree).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(tree.snapshot(), &cached);
        assert_eq!(tree.generation(), 0);
    }

    #[test]
    fn private_directory_exclusion_is_exact() {
        let project = TempProject::new("private-exclusion");
        project.file(".tiptoptyp/hidden.typ", "hidden");
        project.file(".tiptoptyp-notes/visible.typ", "visible");

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert!(snapshot.find(".tiptoptyp").is_none());
        assert!(snapshot.find(".tiptoptyp-notes/visible.typ").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn represents_but_never_follows_symlinks_inside_or_outside_root() {
        use std::os::unix::fs::symlink;

        let project = TempProject::new("symlinks");
        let outside = TempProject::new("outside");
        project.file("real/inside.typ", "inside");
        outside.file("secret.typ", "outside");
        symlink(
            project.path().join("real"),
            project.path().join("inside-link"),
        )
        .unwrap();
        symlink(outside.path(), project.path().join("escape-link")).unwrap();

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        let inside = snapshot.find("inside-link").unwrap();
        let escape = snapshot.find("escape-link").unwrap();
        assert!(inside.is_symlink());
        assert!(escape.is_symlink());
        assert!(inside.children.is_empty());
        assert!(escape.children.is_empty());
        assert!(snapshot.find("inside-link/inside.typ").is_none());
        assert!(snapshot.find("escape-link/secret.typ").is_none());
    }
}
