//! Ownership of compiler/LSP working copies; Overleaf remains authoritative.
//!
//! Preparation and descriptor discovery are blocking worker operations. Dropping
//! the final temporary lease schedules deletion away from the UI thread.
use super::{ProjectUrl, mirror};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

const MARKER: &str = ".tiptoptyp/overleaf-project.json";
const MAX_MARKER_BYTES: usize = 8192;
const MAX_ANCESTORS: usize = 128;
const MARKER_VERSION: u8 = 1;

#[derive(Clone, Debug)]
pub(crate) struct StorageOptions {
    pub(crate) keep_local_copy: bool,
    /// The project-specific namespace is always appended to this base folder.
    pub(crate) directory: Option<PathBuf>,
}
impl Default for StorageOptions {
    fn default() -> Self {
        Self {
            keep_local_copy: true,
            directory: None,
        }
    }
}
impl StorageOptions {
    /// Pure hint for history/routing. Temporary paths exist only after prepare.
    pub(crate) fn root_hint(
        &self,
        project: &ProjectUrl,
        default_base: &Path,
    ) -> Result<Option<PathBuf>, String> {
        if !self.keep_local_copy {
            return Ok(None);
        }
        let base = self.directory.as_deref().unwrap_or(default_base);
        validate_root(base)?;
        Ok(Some(project.mirror_root(base)))
    }
}

#[derive(Debug)]
pub(crate) struct StorageLease {
    root: PathBuf,
    temporary: Option<OwnedTemporary>,
}
impl StorageLease {
    /// Call on the mirror worker, never while drawing or handling input.
    pub(crate) fn prepare(
        project: &ProjectUrl,
        options: &StorageOptions,
        default_base: &Path,
    ) -> Result<Arc<Self>, String> {
        let (root, temporary) = if let Some(root) = options.root_hint(project, default_base)? {
            (root, None)
        } else {
            let temporary = tempfile::Builder::new()
                .prefix("tiptoptyp-overleaf-")
                .tempdir()
                .map_err(|error| error.to_string())?;
            // macOS's temporary-directory spelling can contain /var, a system
            // alias. Resolve this newly created private directory exactly once;
            // subsequent operations use no-follow directory descriptors.
            let root = temporary
                .path()
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let identity = Identity::read(&root).map_err(|error| error.to_string())?;
            let _ = temporary.keep();
            let tracker = Arc::clone(cleanup_tracker());
            tracker.started();
            let owned = OwnedTemporary {
                tracker,
                root: root.clone(),
                identity,
                #[cfg(test)]
                completed: None,
            };
            (root, Some(owned))
        };
        let lease = Arc::new(Self { root, temporary });
        lease.bind(project)?;
        Ok(lease)
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn persistent(&self) -> bool {
        self.temporary.is_none()
    }
    fn bind(&self, project: &ProjectUrl) -> Result<(), String> {
        #[cfg(unix)]
        {
            let directory = mirror::unix::root_directory(&self.root).map_err(|e| e.to_string())?;
            if let Some(stored) = read_marker(&directory).map_err(|e| e.to_string())? {
                if stored.project()? != *project || stored.persistent != self.persistent() {
                    return Err("This working-copy folder belongs to a different Overleaf project or storage mode.".into());
                }
                return Ok(());
            }
            let metadata = Marker {
                version: MARKER_VERSION,
                project_url: project.url().to_owned(),
                persistent: self.persistent(),
            };
            let bytes = serde_json::to_vec(&metadata).map_err(|e| e.to_string())?;
            if bytes.len() > MAX_MARKER_BYTES {
                return Err(
                    "The Overleaf project URL is too long to save its working-copy descriptor."
                        .into(),
                );
            }
            mirror::unix::write(&directory, Path::new(MARKER), &bytes).map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        {
            let _ = project;
            Err("Secure Overleaf storage is not available on this platform yet.".into())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredProject {
    pub(crate) project: ProjectUrl,
    pub(crate) root: PathBuf,
    pub(crate) persistent: bool,
}
impl StoredProject {
    /// Discover an existing working copy from its root or a contained file.
    /// This is bounded, performs no writes, and rejects symlinked metadata.
    /// Call on an explicit open/reopen worker, not during a frame or file guard.
    pub(crate) fn from_path(path: &Path) -> Result<Option<Self>, String> {
        validate_root(path)?;
        #[cfg(unix)]
        {
            for root in path.ancestors().take(MAX_ANCESTORS) {
                let directory = match mirror::unix::root_directory_existing(root) {
                    Ok(directory) => directory,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error.to_string()),
                };
                if let Some(marker) = read_marker(&directory).map_err(|error| error.to_string())? {
                    return Ok(Some(Self {
                        project: marker.project()?,
                        root: root.to_owned(),
                        persistent: marker.persistent,
                    }));
                }
            }
            if path.ancestors().count() > MAX_ANCESTORS {
                return Err("The Overleaf working-copy path is nested too deeply.".into());
            }
            Ok(None)
        }
        #[cfg(not(unix))]
        Err("Secure Overleaf storage is not available on this platform yet.".into())
    }
}

fn validate_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(
            "Choose an absolute Overleaf working-copy folder without parent-directory components."
                .into(),
        );
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u8,
    project_url: String,
    persistent: bool,
}
impl Marker {
    fn project(&self) -> Result<ProjectUrl, String> {
        if self.version != MARKER_VERSION {
            return Err("This Overleaf working-copy descriptor has an unsupported version.".into());
        }
        ProjectUrl::parse(&self.project_url)
    }
}
#[cfg(unix)]
fn read_marker(directory: &fs::File) -> io::Result<Option<Marker>> {
    match mirror::unix::read(directory, Path::new(MARKER), MAX_MARKER_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| io::Error::other("The Overleaf working-copy descriptor is invalid.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Identity {
    fn read(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "The temporary Overleaf workspace is not a directory.",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }
    fn matches(&self, path: &Path) -> io::Result<bool> {
        let other = Self::read(path)?;
        #[cfg(unix)]
        {
            Ok(self.device == other.device && self.inode == other.inode)
        }
        #[cfg(not(unix))]
        {
            let _ = other;
            Ok(true)
        }
    }
}

#[derive(Debug)]
struct OwnedTemporary {
    tracker: Arc<CleanupTracker>,
    root: PathBuf,
    identity: Identity,
    #[cfg(test)]
    completed: Option<std::sync::mpsc::Sender<bool>>,
}
impl Drop for OwnedTemporary {
    fn drop(&mut self) {
        let root = std::mem::take(&mut self.root);
        let tracker = Arc::clone(&self.tracker);
        let identity = Identity {
            #[cfg(unix)]
            device: self.identity.device,
            #[cfg(unix)]
            inode: self.identity.inode,
        };
        #[cfg(test)]
        let completed = self.completed.take();
        let result = std::thread::Builder::new()
            .name("overleaf-workspace-cleanup".into())
            .spawn(move || {
                // Only remove the unique directory we created, never a selected
                // base folder or a replacement that now occupies the same path.
                let result = match identity.matches(&root) {
                    Ok(true) => fs::remove_dir_all(&root).is_ok(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                    _ => false,
                };
                tracker.completed(result);
                #[cfg(test)]
                if let Some(completed) = completed {
                    let _ = completed.send(result);
                }
            });
        if result.is_err() {
            // Do not turn a rare thread-spawn failure into blocking disk I/O on
            // the input thread. Shutdown reports incomplete cleanup to its caller.
            self.tracker.completed(false);
        }
    }
}

#[derive(Debug, Default)]
struct CleanupState {
    active: usize,
    failed: bool,
}
#[derive(Debug, Default)]
struct CleanupTracker {
    state: Mutex<CleanupState>,
    idle: Condvar,
}
impl CleanupTracker {
    fn started(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).active += 1;
    }
    fn completed(&self, success: bool) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active -= 1;
        state.failed |= !success;
        self.idle.notify_all();
    }
    fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.active != 0 {
            let Some(remaining) =
                deadline.and_then(|end| end.checked_duration_since(Instant::now()))
            else {
                return false;
            };
            let (next, result) = self
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(|p| p.into_inner());
            state = next;
            if result.timed_out() && state.active != 0 {
                return false;
            }
        }
        !state.failed
    }
}
fn cleanup_tracker() -> &'static Arc<CleanupTracker> {
    static TRACKER: OnceLock<Arc<CleanupTracker>> = OnceLock::new();
    TRACKER.get_or_init(|| Arc::new(CleanupTracker::default()))
}
/// After the app and its worker leases are dropped, wait a bounded time for
/// temporary deletion. No project paths or document contents are kept globally.
pub(crate) fn finish_temporary_cleanup(timeout: Duration) -> bool {
    cleanup_tracker().wait(timeout)
}

#[cfg(all(test, unix))]
#[path = "storage/tests.rs"]
mod tests;
