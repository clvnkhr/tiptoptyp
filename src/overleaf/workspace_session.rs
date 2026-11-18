//! Small workspace/session metadata. Source baselines live in the offline store.
//! All filesystem operations run on explicit workers and use no-follow handles.
use super::{ProjectUrl, StoredProject, mirror};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

pub(crate) const SESSION_PATH: &str = ".tiptoptyp/overleaf-session.json";
pub(crate) const LINK_PATH: &str = ".tiptoptyp/overleaf-workspace.json";
const MAX_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceSession {
    version: u8,
    project_url: String,
    pub(crate) profile_id: [u8; 16],
    pub(crate) offline: bool,
    pub(crate) has_page_state: bool,
    pub(crate) open_documents: Vec<String>,
    pub(crate) active_document: Option<String>,
    pub(crate) preview_document: Option<String>,
}
impl WorkspaceSession {
    pub(crate) fn new(project: &ProjectUrl, profile_id: [u8; 16]) -> Self {
        Self {
            version: 1,
            project_url: project.url().into(),
            profile_id,
            offline: false,
            has_page_state: false,
            open_documents: Vec::new(),
            active_document: None,
            preview_document: None,
        }
    }
    pub(crate) fn load(root: &Path, project: &ProjectUrl) -> Result<Option<Self>, String> {
        let session: Option<Self> = read(root, SESSION_PATH)?;
        if let Some(session) = &session {
            session.validate(project)?;
        }
        Ok(session)
    }
    pub(crate) fn write(&self, root: &Path, project: &ProjectUrl) -> Result<(), String> {
        self.validate(project)?;
        write(root, SESSION_PATH, self)
    }
    fn validate(&self, project: &ProjectUrl) -> Result<(), String> {
        if self.version != 1
            || self.project_url != project.url()
            || self.profile_id == [0; 16]
            || self.open_documents.len() > 256
            || self
                .open_documents
                .iter()
                .chain(self.active_document.iter())
                .chain(self.preview_document.iter())
                .any(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
        {
            return Err("The saved Overleaf workspace session is invalid.".into());
        }
        Ok(())
    }
}

/// Opaque browser-store identity, never a credential. Persist it before use.
pub(crate) fn new_profile_id() -> [u8; 16] {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut hash = Sha256::new();
    hash.update(format!("{:?}", std::time::SystemTime::now()));
    hash.update(std::process::id().to_le_bytes());
    hash.update(NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.finalize()[..16].try_into().unwrap()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceLink {
    version: u8,
    project_url: String,
    folder: String,
}
/// An explicitly selected workspace base reopens its project-specific child.
/// The shared default cache is deliberately never linked to a single project.
pub(crate) fn bind_workspace(base: &Path, root: &Path, project: &ProjectUrl) -> Result<(), String> {
    if project.mirror_root(base) != root {
        return Err("The Overleaf project folder does not belong to this workspace.".into());
    }
    let link = WorkspaceLink {
        version: 1,
        project_url: project.url().into(),
        folder: root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid Overleaf folder.")?
            .into(),
    };
    if let Some(old) = linked_project(base)?
        && old.project != *project
    {
        return Err(
            "This workspace already stores another Overleaf project. Choose another folder.".into(),
        );
    }
    write(base, LINK_PATH, &link)
}
pub(crate) fn linked_project(base: &Path) -> Result<Option<StoredProject>, String> {
    let Some(link): Option<WorkspaceLink> = read(base, LINK_PATH)? else {
        return Ok(None);
    };
    let project = ProjectUrl::parse(&link.project_url)?;
    let root = project.mirror_root(base);
    if link.version != 1 || root.file_name().and_then(|n| n.to_str()) != Some(link.folder.as_str())
    {
        return Err("The saved Overleaf workspace link is invalid.".into());
    }
    Ok(Some(StoredProject {
        project,
        root,
        persistent: true,
    }))
}

#[cfg(unix)]
fn read<T: serde::de::DeserializeOwned>(root: &Path, path: &str) -> Result<Option<T>, String> {
    let directory = match mirror::unix::root_directory_existing(root) {
        Ok(directory) => directory,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    match mirror::unix::read(&directory, Path::new(path), MAX_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "The saved Overleaf workspace metadata is invalid.".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
#[cfg(not(unix))]
fn read<T: serde::de::DeserializeOwned>(_root: &Path, _path: &str) -> Result<Option<T>, String> {
    Err("Secure Overleaf workspace storage is unavailable.".into())
}
#[cfg(unix)]
fn write<T: Serialize>(root: &Path, path: &str, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("The Overleaf workspace session is too large.".into());
    }
    let directory = mirror::unix::root_directory(root).map_err(|e| e.to_string())?;
    mirror::unix::write(&directory, Path::new(path), &bytes).map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn write<T: Serialize>(_root: &Path, _path: &str, _value: &T) -> Result<(), String> {
    Err("Secure Overleaf workspace storage is unavailable.".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn workspace_links_and_sessions_roundtrip_without_credentials_or_cross_project_rebinding() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let project =
            ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap();
        let root = project.mirror_root(&base);
        let mut session = WorkspaceSession::new(&project, new_profile_id());
        session.offline = true;
        session.open_documents.push("doc1".into());
        session.active_document = Some("doc1".into());
        session.write(&root, &project).unwrap();
        assert_eq!(
            WorkspaceSession::load(&root, &project).unwrap(),
            Some(session)
        );
        bind_workspace(&base, &root, &project).unwrap();
        let linked = linked_project(&base).unwrap().unwrap();
        assert_eq!(linked.root, root);
        assert_eq!(linked.project, project);
        let other =
            ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567").unwrap();
        assert!(bind_workspace(&base, &other.mirror_root(&base), &other).is_err());
        assert!(WorkspaceSession::load(&root, &other).is_err());
    }
    #[test]
    fn metadata_is_bounded_and_rejects_symlinks_and_parent_traversal() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        std::fs::create_dir(base.join(".tiptoptyp")).unwrap();
        std::fs::write(base.join(LINK_PATH), br#"{"version":1,"project_url":"https://www.overleaf.com/project/0123456789abcdef01234567","folder":"../elsewhere"}"#).unwrap();
        assert!(linked_project(&base).is_err());
        std::fs::remove_file(base.join(LINK_PATH)).unwrap();
        symlink(base.join("target"), base.join(LINK_PATH)).unwrap();
        assert!(linked_project(&base).is_err());
        std::fs::remove_file(base.join(LINK_PATH)).unwrap();
        std::fs::write(base.join(LINK_PATH), vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(linked_project(&base).is_err());
    }
    #[test]
    fn independently_created_sessions_have_different_browser_profiles() {
        assert_ne!(new_profile_id(), new_profile_id());
    }

    #[test]
    fn malformed_zero_profile_is_rejected_before_native_browser_creation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project =
            ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap();
        let invalid = WorkspaceSession::new(&project, [0; 16]);
        assert!(invalid.write(&root, &project).is_err());
        assert!(!root.join(SESSION_PATH).exists());
        WorkspaceSession::new(&project, [1; 16])
            .write(&root, &project)
            .unwrap();
        std::fs::write(
            root.join(SESSION_PATH),
            serde_json::to_vec(&invalid).unwrap(),
        )
        .unwrap();
        assert!(WorkspaceSession::load(&root, &project).is_err());
    }

    #[test]
    fn relative_workspace_roots_cannot_alias_absolute_storage() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let relative = root.strip_prefix("/").unwrap();
        let project =
            ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap();
        let mut session = WorkspaceSession::new(&project, [1; 16]);
        session.write(&root, &project).unwrap();
        let original = std::fs::read(root.join(SESSION_PATH)).unwrap();

        assert!(WorkspaceSession::load(relative, &project).is_err());
        assert!(linked_project(relative).is_err());
        assert!(super::super::OfflineStore::load(relative, &project, 1).is_err());
        session.offline = true;
        assert!(session.write(relative, &project).is_err());
        assert_eq!(std::fs::read(root.join(SESSION_PATH)).unwrap(), original);
        let malformed = root.join("uncreated/../elsewhere");
        assert!(session.write(&malformed, &project).is_err());
        assert!(!root.join("uncreated").exists());
    }
}
