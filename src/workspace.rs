use std::{
    borrow::Cow,
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

/// The kind of an entry in the project tree.
///
/// Symlinks are surfaced so the filesystem panel does not silently hide them,
/// but they are never followed. Callers should only open `File` nodes as
/// project files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
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
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            snapshot: WorkspaceSnapshot::scan(root)?,
            generation: 0,
        })
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

    /// Rescan the root, retaining the old cache if scanning fails. Returns
    /// whether the visible tree changed.
    pub fn refresh(&mut self) -> io::Result<bool> {
        let next = WorkspaceSnapshot::scan(&self.snapshot.root)?;
        if next == self.snapshot {
            return Ok(false);
        }

        self.snapshot = next;
        self.generation = self.generation.wrapping_add(1);
        Ok(true)
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

    nodes.sort_by(|left, right| {
        kind_order(left.kind)
            .cmp(&kind_order(right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(nodes)
}

fn kind_order(kind: WorkspaceNodeKind) -> u8 {
    match kind {
        WorkspaceNodeKind::Directory => 0,
        WorkspaceNodeKind::File => 1,
        WorkspaceNodeKind::Symlink => 2,
    }
}

fn is_excluded(name: &OsStr) -> bool {
    if name == OsStr::new(".git") || name == OsStr::new("target") {
        return true;
    }

    let name = name.to_string_lossy();
    name.starts_with(".mytypst-preview-")
        || name.starts_with("mytypst-preview-")
        || name.starts_with(".mytypst-write-")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mytypst-workspace-{label}-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn directory(&self, relative: impl AsRef<Path>) {
            fs::create_dir_all(self.path.join(relative)).unwrap();
        }

        fn file(&self, relative: impl AsRef<Path>, contents: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn names(nodes: &[WorkspaceNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| node.display_name().into_owned())
            .collect()
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
        project.file(".mytypst-preview-123.typ", "preview");
        project.file("mytypst-preview-124.typ", "preview");
        project.file("nested/.mytypst-preview-456.typ", "preview");
        project.file(".mytypst-write-document.typ", "write");
        project.file("nested/.mytypst-write-789", "write");
        project.file("target.typ", "useful");
        project.file(".mytypst-preview", "useful");

        let snapshot = WorkspaceSnapshot::scan(project.path()).unwrap();
        assert!(snapshot.find(".git").is_none());
        assert!(snapshot.find("target").is_none());
        assert!(snapshot.find("nested/target").is_none());
        assert!(snapshot.find(".mytypst-preview-123.typ").is_none());
        assert!(snapshot.find("mytypst-preview-124.typ").is_none());
        assert!(snapshot.find("nested/.mytypst-preview-456.typ").is_none());
        assert!(snapshot.find(".mytypst-write-document.typ").is_none());
        assert!(snapshot.find("nested/.mytypst-write-789").is_none());
        assert!(snapshot.find("target.typ").is_some());
        assert!(snapshot.find(".mytypst-preview").is_some());
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
        let mut tree = WorkspaceTree::new(project.path()).unwrap();
        assert_eq!(tree.generation(), 0);

        project.file("chapter.typ", "chapter");
        assert!(tree.snapshot().find("chapter.typ").is_none());
        assert!(tree.refresh().unwrap());
        assert!(tree.snapshot().find("chapter.typ").is_some());
        assert_eq!(tree.generation(), 1);

        assert!(!tree.refresh().unwrap());
        assert_eq!(tree.generation(), 1);

        // Contents do not affect the filesystem panel's structural snapshot.
        project.file("main.typ", "second");
        assert!(!tree.refresh().unwrap());
        assert_eq!(tree.generation(), 1);
    }

    #[test]
    fn paths_are_relative_to_the_canonical_root() {
        let project = TempProject::new("paths");
        project.file("chapters/one.typ", "one");

        let tree = WorkspaceTree::new(project.path()).unwrap();
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
