//! Private, project-local storage for generated editor artifacts.
//!
//! Runtime scratch data must not leak into the system temporary directory or
//! appear alongside users' source files. Every helper in this module roots its
//! artifacts in `<project>/.tiptoptyp`, rejects a symlinked private directory,
//! and gives ownership of ephemeral paths to `tempfile` guards so early returns
//! and panics still clean them up.

use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

pub const PRIVATE_DIRECTORY_NAME: &str = ".tiptoptyp";
use tiptoptyp::save_transaction::WriteDurability;

/// Validated access to one project's private artifact directory.
#[derive(Debug, Clone)]
pub struct PrivateWorkspace {
    project_root: PathBuf,
    directory: PathBuf,
}

impl PrivateWorkspace {
    /// Creates (or validates) `<project_root>/.tiptoptyp`.
    pub fn open(project_root: impl AsRef<Path>) -> io::Result<Self> {
        let project_root = project_root.as_ref().canonicalize()?;
        if !project_root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private workspace root is not a directory: {}",
                    project_root.display()
                ),
            ));
        }

        let directory = project_root.join(PRIVATE_DIRECTORY_NAME);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => validate_private_directory(&directory, &metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Another session may create the same project-local directory
                // after our metadata check. That is an ordinary successful
                // initialization as long as the winner created a valid root;
                // the validation below remains authoritative for symlinks,
                // files, and other invalid replacements.
                if let Err(error) = create_private_directory(&directory)
                    && error.kind() != io::ErrorKind::AlreadyExists
                {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        // Validate again after the create/check boundary, then canonicalize.
        // This retains the original symlink-race defense while keeping the
        // validation rules in one place.
        let metadata = fs::symlink_metadata(&directory)?;
        validate_private_directory(&directory, &metadata)?;
        restrict_private_directory(&directory)?;
        let canonical_directory = directory.canonicalize()?;
        if canonical_directory.parent() != Some(project_root.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private workspace escaped project root: {}",
                    canonical_directory.display()
                ),
            ));
        }

        Ok(Self {
            project_root,
            directory: canonical_directory,
        })
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn path(&self) -> &Path {
        &self.directory
    }

    /// Creates an automatically cleaned directory below `.tiptoptyp`.
    pub fn temp_dir(&self, prefix: &str) -> io::Result<tempfile::TempDir> {
        validate_prefix(prefix)?;
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&self.directory)
    }

    /// Creates an automatically cleaned file below `.tiptoptyp`.
    pub fn temp_file(&self, prefix: &str, suffix: &str) -> io::Result<tempfile::NamedTempFile> {
        validate_prefix(prefix)?;
        validate_suffix(suffix)?;
        tempfile::Builder::new()
            .prefix(prefix)
            .suffix(suffix)
            .tempfile_in(&self.directory)
    }

    /// Creates a source file at the corresponding virtual project location.
    ///
    /// A private mirror makes relative Typst imports behave as though the
    /// unsaved buffer still lived in `source_dir`. On Unix this mirror consists
    /// of symlinks, so dependency edits remain visible to filesystem watchers.
    /// Platforms that cannot create symlinks fall back to private copies.
    pub fn mirrored_typst_document(
        &self,
        source_dir: impl AsRef<Path>,
        display_name: impl AsRef<OsStr>,
        source: &str,
    ) -> io::Result<PrivateTypstDocument> {
        PrivateTypstDocument::create(self, source_dir.as_ref(), display_name.as_ref(), source)
    }
}

fn validate_private_directory(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to use symlinked private workspace {}",
                path.display()
            ),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "private workspace path is not a directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// A project-local, automatically cleaned backing file for an unsaved Typst
/// document or a live-preview shadow.
#[derive(Debug)]
pub struct PrivateTypstDocument {
    project_root: PathBuf,
    relative_source_dir: PathBuf,
    display_name: OsString,
    session: tempfile::TempDir,
    mirror_root: PathBuf,
    path: PathBuf,
}

impl PrivateTypstDocument {
    fn create(
        private: &PrivateWorkspace,
        source_dir: &Path,
        display_name: &OsStr,
        source: &str,
    ) -> io::Result<Self> {
        validate_file_name(display_name)?;
        let source_dir = source_dir.canonicalize()?;
        if !source_dir.is_dir() || !source_dir.starts_with(private.project_root()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Typst source directory {} is outside project root {}",
                    source_dir.display(),
                    private.project_root().display()
                ),
            ));
        }

        let session = private.temp_dir("typst-")?;
        let mirror_root = session.path().join("project");
        fs::create_dir(&mirror_root)?;
        let relative_source_dir = source_dir
            .strip_prefix(private.project_root())
            .expect("validated source directory containment");
        populate_mirror(
            private.project_root(),
            &mirror_root,
            relative_source_dir,
            display_name,
        )?;
        let shadow_dir = mirror_root.join(relative_source_dir);
        let path = shadow_dir.join(display_name);
        write_new_private_file(&path, source)?;

        Ok(Self {
            project_root: private.project_root().to_owned(),
            relative_source_dir: relative_source_dir.to_owned(),
            display_name: display_name.to_owned(),
            session,
            mirror_root,
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session_dir(&self) -> &Path {
        self.session.path()
    }

    pub fn mirror_root(&self) -> &Path {
        &self.mirror_root
    }

    /// Refreshes fallback copies and adds newly created project entries, then
    /// updates the source without replacing its inode. Keeping the inode is
    /// required by Typst's macOS filesystem watcher.
    pub fn update(&self, source: &str) -> io::Result<()> {
        populate_mirror(
            &self.project_root,
            &self.mirror_root,
            &self.relative_source_dir,
            &self.display_name,
        )?;
        write_private_source(&self.path, source, false)
    }
}

/// Finds the natural private-workspace root for a file. `typst.toml` and Git
/// roots win; otherwise the file's containing directory is used.
pub fn project_root_for_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let start = if absolute.is_dir() {
        absolute.as_path()
    } else {
        absolute.parent().unwrap_or_else(|| Path::new("."))
    };
    let start = start.canonicalize()?;
    Ok(start
        .ancestors()
        .find(|ancestor| ancestor.join("typst.toml").is_file() || ancestor.join(".git").exists())
        .unwrap_or(&start)
        .to_owned())
}

/// Destination-local writer for atomic user-file replacement.
///
/// Temporary inodes live below a private directory beside the destination,
/// which guarantees that `persist` does not cross filesystem boundaries even
/// when Save As or PDF export targets another mounted volume.
pub struct AtomicFileWriter;

impl AtomicFileWriter {
    pub fn write(destination: impl AsRef<Path>, contents: &[u8]) -> io::Result<WriteDurability> {
        Self::write_checked(destination, contents, || Ok(()))
    }

    /// Check and persist share the app-owned destination lease. External
    /// processes are not locked by this lease and may still race the write.
    pub(crate) fn write_checked(
        destination: impl AsRef<Path>,
        contents: &[u8],
        check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<WriteDurability> {
        crate::resource_lock::with_resource(destination.as_ref(), || {
            check()?;
            atomic_write_with_staging_root(
                destination.as_ref(),
                contents,
                destination_local_staging_root,
            )
        })
    }
}

fn atomic_write_with_staging_root(
    destination: &Path,
    contents: &[u8],
    select_staging_root: impl FnOnce(&Path) -> io::Result<PathBuf>,
) -> io::Result<WriteDurability> {
    let staging_root = select_staging_root(destination)?;
    let private = PrivateWorkspace::open(staging_root)?;
    let existing_permissions = fs::metadata(destination)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temporary = private.temp_file("write-", ".tmp")?;
    temporary.write_all(contents)?;
    if let Some(permissions) = existing_permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    commit_staged_file(temporary, destination, sync_parent)
}

fn commit_staged_file(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<WriteDurability> {
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    Ok(match sync(destination) {
        Ok(()) => WriteDurability::Synchronized,
        Err(error) => WriteDurability::Uncertain(error.to_string()),
    })
}

fn destination_local_staging_root(destination: &Path) -> io::Result<PathBuf> {
    let absolute = if destination.is_absolute() {
        destination.to_owned()
    } else {
        std::env::current_dir()?.join(destination)
    };
    let parent = absolute.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "atomic-write destination has no parent directory: {}",
                destination.display()
            ),
        )
    })?;
    parent.canonicalize()
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

fn restrict_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn validate_prefix(prefix: &str) -> io::Result<()> {
    if !is_single_normal_component(OsStr::new(prefix)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private artifact prefix must be one non-empty path component",
        ));
    }
    Ok(())
}

fn validate_suffix(suffix: &str) -> io::Result<()> {
    if !suffix.is_empty() && !is_single_normal_component(OsStr::new(suffix)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private artifact suffix must not contain path separators",
        ));
    }
    Ok(())
}

fn validate_file_name(name: &OsStr) -> io::Result<()> {
    if !is_single_normal_component(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private Typst document name must be one ordinary path component",
        ));
    }
    Ok(())
}

fn is_single_normal_component(value: &OsStr) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn write_new_private_file(path: &Path, source: &str) -> io::Result<()> {
    write_private_source(path, source, true)
}

fn write_private_source(path: &Path, source: &str, create_new: bool) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(source.as_bytes())?;
    file.flush()?;
    file.sync_data()
}

fn populate_mirror(
    project_root: &Path,
    mirror_root: &Path,
    relative_source_dir: &Path,
    display_name: &OsStr,
) -> io::Result<()> {
    populate_mirror_with_mode(
        project_root,
        mirror_root,
        relative_source_dir,
        display_name,
        MirrorMode::PreferSymlink,
    )
}

fn populate_mirror_with_mode(
    project_root: &Path,
    mirror_root: &Path,
    relative_source_dir: &Path,
    display_name: &OsStr,
    mode: MirrorMode,
) -> io::Result<()> {
    if relative_source_dir
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source directory is not relative to the project root",
        ));
    }
    populate_mirror_level(
        project_root,
        mirror_root,
        relative_source_dir,
        display_name,
        true,
        mode,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MirrorMode {
    PreferSymlink,
    #[cfg(test)]
    Copy,
}

fn populate_mirror_level(
    source: &Path,
    mirror: &Path,
    remaining_source_dir: &Path,
    display_name: &OsStr,
    at_project_root: bool,
    mode: MirrorMode,
) -> io::Result<()> {
    fs::create_dir_all(mirror)?;
    let mut expected_entries = HashSet::new();
    if remaining_source_dir.as_os_str().is_empty() {
        // The live private source may have no on-disk counterpart yet.
        expected_entries.insert(display_name.to_owned());
    }
    let mut remaining = remaining_source_dir.components();
    let next_directory = remaining.next().and_then(|component| match component {
        Component::Normal(name) => Some(name),
        _ => None,
    });
    let following_directories = remaining.as_path();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if at_project_root && name == OsStr::new(PRIVATE_DIRECTORY_NAME) {
            continue;
        }
        expected_entries.insert(name.clone());
        if remaining_source_dir.as_os_str().is_empty() && name == display_name {
            // This is the live buffer's private replacement.
            continue;
        }
        let source_path = entry.path();
        let mirror_path = mirror.join(&name);
        if next_directory == Some(name.as_os_str()) {
            if !entry.file_type()?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "source path component is not a directory: {}",
                        source_path.display()
                    ),
                ));
            }
            ensure_directory(&mirror_path)?;
            populate_mirror_level(
                &source_path,
                &mirror_path,
                following_directories,
                display_name,
                false,
                mode,
            )?;
        } else {
            mirror_entry(&source_path, &mirror_path, entry.file_type()?, mode)?;
        }
    }

    // A copied fallback is a mirror, not an accumulating cache. Remove entries
    // deleted from the source while preserving the private replacement for the
    // live document itself.
    for entry in fs::read_dir(mirror)? {
        let entry = entry?;
        if !expected_entries.contains(&entry.file_name()) {
            remove_entry(&entry.path())?;
        }
    }

    if let Some(next) = next_directory {
        let mirror_path = mirror.join(next);
        if !mirror_path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "source directory disappeared while mirroring: {}",
                    source.display()
                ),
            ));
        }
    }
    Ok(())
}

fn mirror_entry(
    source: &Path,
    destination: &Path,
    file_type: fs::FileType,
    mode: MirrorMode,
) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        // Symlinks always reflect replacement at the source path. Forced-copy
        // mode removes them so tests and restricted platforms exercise the
        // complete synchronization path independently of host privileges.
        if metadata.file_type().is_symlink() && mode == MirrorMode::PreferSymlink {
            return Ok(());
        }
        if metadata.file_type().is_symlink() {
            fs::remove_file(destination)?;
        }
    }

    if mode == MirrorMode::PreferSymlink && !destination.exists() {
        match create_symlink(source, destination, file_type) {
            Ok(()) => return Ok(()),
            Err(error) if !file_type.is_file() && !file_type.is_dir() => return Err(error),
            Err(_) => {}
        }
    }
    sync_copied_entry(source, destination, file_type)
}

#[cfg(unix)]
fn create_symlink(source: &Path, destination: &Path, _file_type: fs::FileType) -> io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn create_symlink(source: &Path, destination: &Path, file_type: fs::FileType) -> io::Result<()> {
    if file_type.is_dir() {
        std::os::windows::fs::symlink_dir(source, destination)
    } else {
        std::os::windows::fs::symlink_file(source, destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_source: &Path, _destination: &Path, _file_type: fs::FileType) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filesystem symlinks are unavailable",
    ))
}

fn sync_copied_entry(source: &Path, destination: &Path, file_type: fs::FileType) -> io::Result<()> {
    if file_type.is_file() {
        if let Ok(metadata) = fs::symlink_metadata(destination)
            && !metadata.is_file()
        {
            remove_entry(destination)?;
        }
        fs::copy(source, destination).map(|_| ())
    } else if file_type.is_dir() {
        ensure_directory(destination)?;
        sync_copied_directory(source, destination)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("cannot copy unsupported mirror entry {}", source.display()),
        ))
    }
}

fn sync_copied_directory(source: &Path, destination: &Path) -> io::Result<()> {
    ensure_directory(destination)?;
    let mut expected_entries = HashSet::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(PRIVATE_DIRECTORY_NAME) {
            continue;
        }
        expected_entries.insert(name.clone());
        let source_path = entry.path();
        let destination_path = destination.join(name);
        let file_type = entry.file_type()?;
        sync_copied_entry(&source_path, &destination_path, file_type)?;
    }
    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        if !expected_entries.contains(&entry.file_name()) {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        remove_entry(path)?;
    }
    fs::create_dir_all(path)
}

fn remove_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn sync_parent(destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = destination.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn artifacts_are_project_local_and_guards_clean_ephemeral_entries() {
        let project = tempfile::tempdir().unwrap();
        let private = PrivateWorkspace::open(project.path()).unwrap();
        assert_eq!(
            private.path(),
            project
                .path()
                .canonicalize()
                .unwrap()
                .join(PRIVATE_DIRECTORY_NAME)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(private.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        let file_path = {
            let file = private.temp_file("test-", ".tmp").unwrap();
            assert!(file.path().starts_with(private.path()));
            file.path().to_owned()
        };
        assert!(!file_path.exists());

        let directory_path = {
            let directory = private.temp_dir("test-").unwrap();
            assert!(directory.path().starts_with(private.path()));
            directory.path().to_owned()
        };
        assert!(!directory_path.exists());
        assert!(private.path().is_dir());
    }

    #[test]
    fn rejects_a_symlinked_private_directory() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), project.path().join(PRIVATE_DIRECTORY_NAME))
            .unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(
            outside.path(),
            project.path().join(PRIVATE_DIRECTORY_NAME),
        )
        .is_err()
        {
            return;
        }

        let error = PrivateWorkspace::open(project.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn concurrent_valid_opens_share_the_created_private_root() {
        let project = tempfile::tempdir().unwrap();
        let project_path = Arc::new(project.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(16));
        let workers = (0..16)
            .map(|_| {
                let project_path = project_path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    PrivateWorkspace::open(project_path.as_path())
                })
            })
            .collect::<Vec<_>>();

        let opened = workers
            .into_iter()
            .map(|worker| worker.join().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert!(
            opened
                .iter()
                .all(|private| private.path() == opened[0].path())
        );
    }

    #[test]
    fn mirrored_document_keeps_relative_project_layout_and_cleans_up() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("chapters")).unwrap();
        fs::write(project.path().join("shared.typ"), "#let shared = 1").unwrap();
        fs::write(project.path().join("chapters/sibling.typ"), "sibling").unwrap();
        fs::write(project.path().join("chapters/main.typ"), "on disk").unwrap();
        let private = PrivateWorkspace::open(project.path()).unwrap();

        let session_path = {
            let document = private
                .mirrored_typst_document(
                    project.path().join("chapters"),
                    "main.typ",
                    "unsaved source",
                )
                .unwrap();
            let session = document.session_dir().to_owned();
            assert!(document.path().starts_with(&session));
            assert_eq!(
                fs::read_to_string(document.path()).unwrap(),
                "unsaved source"
            );
            assert_eq!(
                fs::read_to_string(document.path().parent().unwrap().join("sibling.typ")).unwrap(),
                "sibling"
            );
            assert_eq!(
                fs::read_to_string(document.path().parent().unwrap().join("../shared.typ"))
                    .unwrap(),
                "#let shared = 1"
            );
            document.update("new unsaved source").unwrap();
            assert_eq!(
                fs::read_to_string(document.path()).unwrap(),
                "new unsaved source"
            );
            assert_eq!(
                fs::read_to_string(project.path().join("chapters/main.typ")).unwrap(),
                "on disk"
            );
            session
        };
        assert!(!session_path.exists());
    }

    #[test]
    fn artifact_names_are_single_components_and_cannot_escape_private_storage() {
        let project = tempfile::tempdir().unwrap();
        let private = PrivateWorkspace::open(project.path()).unwrap();

        assert_eq!(
            private.temp_dir("../escape").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            private.temp_dir(".").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            private.temp_file("safe-", "../escape").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(private.temp_file("safe-", "").is_ok());
        assert_eq!(
            private
                .mirrored_typst_document(project.path(), "../escape.typ", "text")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!project.path().parent().unwrap().join("escape.typ").exists());
    }

    #[test]
    fn updating_a_mirror_preserves_the_source_inode_and_adds_new_siblings() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("chapters")).unwrap();
        let private = PrivateWorkspace::open(project.path()).unwrap();
        let document = private
            .mirrored_typst_document(project.path().join("chapters"), "main.typ", "first")
            .unwrap();
        #[cfg(unix)]
        let original_inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(document.path()).unwrap().ino()
        };

        fs::write(project.path().join("chapters/new.typ"), "new sibling").unwrap();
        document.update("second").unwrap();

        assert_eq!(fs::read_to_string(document.path()).unwrap(), "second");
        assert_eq!(
            fs::read_to_string(document.path().parent().unwrap().join("new.typ")).unwrap(),
            "new sibling"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(document.path()).unwrap().ino(), original_inode);
        }
    }

    #[test]
    fn forced_copy_mirror_synchronizes_nested_changes_and_type_replacements() {
        let project = tempfile::tempdir().unwrap();
        let mirror_owner = tempfile::tempdir().unwrap();
        let mirror = mirror_owner.path().join("mirror");
        let assets = project.path().join("assets");
        fs::create_dir_all(assets.join("nested")).unwrap();
        fs::write(assets.join("nested/changed.txt"), "old").unwrap();
        fs::write(assets.join("nested/deleted.txt"), "delete me").unwrap();
        fs::write(assets.join("becomes-directory"), "file").unwrap();
        fs::create_dir(assets.join("becomes-file")).unwrap();
        fs::write(assets.join("becomes-file/old.txt"), "old").unwrap();

        populate_mirror_with_mode(
            project.path(),
            &mirror,
            Path::new(""),
            OsStr::new("main.typ"),
            MirrorMode::Copy,
        )
        .unwrap();

        fs::write(assets.join("nested/changed.txt"), "new").unwrap();
        fs::remove_file(assets.join("nested/deleted.txt")).unwrap();
        fs::write(assets.join("nested/added.txt"), "added").unwrap();
        fs::remove_file(assets.join("becomes-directory")).unwrap();
        fs::create_dir(assets.join("becomes-directory")).unwrap();
        fs::write(assets.join("becomes-directory/new.txt"), "directory").unwrap();
        fs::remove_dir_all(assets.join("becomes-file")).unwrap();
        fs::write(assets.join("becomes-file"), "replacement file").unwrap();

        populate_mirror_with_mode(
            project.path(),
            &mirror,
            Path::new(""),
            OsStr::new("main.typ"),
            MirrorMode::Copy,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(mirror.join("assets/nested/changed.txt")).unwrap(),
            "new"
        );
        assert!(!mirror.join("assets/nested/deleted.txt").exists());
        assert_eq!(
            fs::read_to_string(mirror.join("assets/nested/added.txt")).unwrap(),
            "added"
        );
        assert_eq!(
            fs::read_to_string(mirror.join("assets/becomes-directory/new.txt")).unwrap(),
            "directory"
        );
        assert_eq!(
            fs::read_to_string(mirror.join("assets/becomes-file")).unwrap(),
            "replacement file"
        );
    }

    #[test]
    fn atomic_write_stages_privately_and_preserves_permissions() {
        let project = tempfile::tempdir().unwrap();
        let destination = project.path().join("main.typ");
        fs::write(&destination, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
        }

        AtomicFileWriter::write(&destination, b"new").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        let private = PrivateWorkspace::open(project.path()).unwrap();
        assert_eq!(fs::read_dir(private.path()).unwrap().count(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(destination).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }

    #[test]
    fn failed_directory_sync_reports_committed_bytes_without_a_retryable_error() {
        let project = tempfile::tempdir().unwrap();
        let destination = project.path().join("saved.typ");
        fs::write(&destination, "old").unwrap();
        let mut temporary = tempfile::NamedTempFile::new_in(project.path()).unwrap();
        temporary.write_all(b"new").unwrap();
        let outcome = commit_staged_file(temporary, &destination, |_| {
            Err(io::Error::other("injected sync failure"))
        })
        .unwrap();
        assert!(matches!(outcome, WriteDurability::Uncertain(error) if error.contains("injected")));
        assert_eq!(fs::read(&destination).unwrap(), b"new");

        let directory = project.path().join("directory");
        fs::create_dir(&directory).unwrap();
        let temporary = tempfile::NamedTempFile::new_in(project.path()).unwrap();
        let called = std::cell::Cell::new(false);
        assert!(
            commit_staged_file(temporary, &directory, |_| {
                called.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(
            !called.get(),
            "post-commit sync cannot run after persist failed"
        );
        assert!(directory.is_dir());
    }

    #[test]
    fn atomic_writer_selects_staging_from_the_destination_parent() {
        let destination_root = tempfile::tempdir().unwrap();
        let destination = destination_root.path().join("export.pdf");
        assert_eq!(
            destination_local_staging_root(&destination).unwrap(),
            destination_root.path().canonicalize().unwrap()
        );

        let injected_root = tempfile::tempdir().unwrap();
        let selected_for = std::cell::RefCell::new(None);
        atomic_write_with_staging_root(&destination, b"pdf bytes", |path| {
            selected_for.replace(Some(path.to_owned()));
            Ok(injected_root.path().to_path_buf())
        })
        .unwrap();
        assert_eq!(
            selected_for.into_inner().as_deref(),
            Some(destination.as_path())
        );
        assert_eq!(fs::read(destination).unwrap(), b"pdf bytes");
    }

    #[test]
    fn root_discovery_prefers_nearest_project_marker() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(".git")).unwrap();
        fs::create_dir_all(project.path().join("chapters/deep")).unwrap();
        fs::write(project.path().join("chapters/deep/main.typ"), "text").unwrap();
        assert_eq!(
            project_root_for_path(&project.path().join("chapters/deep/main.typ")).unwrap(),
            project.path().canonicalize().unwrap()
        );
    }
}
