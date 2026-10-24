//! Blocking mirror effects: call only from a worker. No local write is a sync ACK.
use super::*;
use std::{
    fs,
    io::{Read, Write},
};

pub(crate) struct Mirror {
    root: PathBuf,
    #[cfg(unix)]
    directory: fs::File,
}
impl Mirror {
    pub(crate) fn open(root: &Path) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("The Overleaf mirror must have an absolute path.".into());
        }
        #[cfg(unix)]
        {
            let directory = unix::root_directory(root).map_err(|e| e.to_string())?;
            Ok(Self {
                root: root.to_owned(),
                directory,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Err("Secure Overleaf mirroring is not available on this platform yet.".into())
        }
    }
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    /// Materialize only server-declared folders and retire only paths recorded
    /// in our previous server manifest. Generated artifacts are never uploaded,
    /// included in the remote tree, or recursively deleted here.
    pub(crate) fn apply_tree(&self, tree: &Tree) -> Result<(), String> {
        #[cfg(unix)]
        {
            // Reject an oversized in-memory listing before deleting old files,
            // creating folders or publishing an unreadable manifest.
            check_manifest_size(&tree.entries)?;
            let bytes = serde_json::to_vec(&tree.entries).map_err(|e| e.to_string())?;
            let manifest = Path::new(".tiptoptyp/overleaf-files.json");
            let previous = match unix::read(&self.directory, manifest, MAX_TREE_MANIFEST_BYTES) {
                Ok(bytes) => {
                    let entries: Vec<Entry> = serde_json::from_slice(&bytes)
                        .map_err(|_| "The Overleaf mirror manifest is invalid.")?;
                    Tree::new(String::new(), None, entries)?.entries
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.to_string()),
            };
            let paths: BTreeSet<&str> = tree
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect();
            for entry in &previous {
                if entry.kind != EntryKind::Folder && !paths.contains(entry.path.as_str()) {
                    unix::remove(&self.directory, &entry.relative_path()?)
                        .map_err(|e| e.to_string())?;
                }
            }
            for entry in &tree.entries {
                let relative = entry.relative_path()?;
                if entry.kind == EntryKind::Folder {
                    unix::directory(&self.directory, &relative, true).map_err(|e| e.to_string())?;
                } else if let Some(parent) = relative.parent() {
                    unix::directory(&self.directory, parent, true).map_err(|e| e.to_string())?;
                }
            }
            unix::write(&self.directory, manifest, &bytes).map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        {
            let _ = tree;
            Err("Secure Overleaf mirroring is unavailable.".into())
        }
    }
    pub(crate) fn write_document(&self, entry: &Entry, source: &str) -> Result<PathBuf, String> {
        if entry.kind != EntryKind::Doc {
            return Err("The Overleaf entry is not a document.".into());
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err("The Overleaf document is too large.".into());
        }
        self.write(entry, source.as_bytes())
    }
    pub(crate) fn write_file(&self, entry: &Entry, encoded: &str) -> Result<PathBuf, String> {
        if entry.kind != EntryKind::File {
            return Err("The Overleaf entry is not an asset.".into());
        }
        self.write(entry, &decode_base64(encoded)?)
    }
    fn write(&self, entry: &Entry, bytes: &[u8]) -> Result<PathBuf, String> {
        let relative = entry.relative_path()?;
        #[cfg(unix)]
        unix::write(&self.directory, &relative, bytes).map_err(|e| e.to_string())?;
        #[cfg(not(unix))]
        return Err("Secure Overleaf mirroring is unavailable.".into());
        #[cfg(unix)]
        Ok(self.root.join(relative))
    }
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    if input.len() > MAX_FILE_BYTES.div_ceil(3) * 4 || !input.len().is_multiple_of(4) {
        return Err("The Overleaf asset has invalid or oversized base64 data.".into());
    }
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut output = Vec::with_capacity((input.len() / 4 * 3).min(MAX_FILE_BYTES));
    let (chunks, _) = input.as_bytes().as_chunks::<4>();
    let total = chunks.len();
    for (index, chunk) in chunks.iter().enumerate() {
        let a = value(chunk[0]).ok_or("Invalid asset base64.")?;
        let b = value(chunk[1]).ok_or("Invalid asset base64.")?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            value(chunk[2]).ok_or("Invalid asset base64.")?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            value(chunk[3]).ok_or("Invalid asset base64.")?
        };
        let padding = usize::from(chunk[2] == b'=') + usize::from(chunk[3] == b'=');
        if (padding != 0 && index + 1 != total)
            || (chunk[2] == b'=' && chunk[3] != b'=')
            || (padding == 2 && b & 15 != 0)
            || (padding == 1 && c & 3 != 0)
        {
            return Err("Invalid asset base64 padding.".into());
        }
        output.push((a << 2) | (b >> 4));
        if padding < 2 {
            output.push((b << 4) | (c >> 2));
        }
        if padding == 0 {
            output.push((c << 6) | d);
        }
        if output.len() > MAX_FILE_BYTES {
            return Err("The Overleaf asset is too large.".into());
        }
    }
    Ok(output)
}

#[cfg(unix)]
pub(super) mod unix {
    use super::*;
    use nix::libc;
    use std::{
        ffi::CString,
        io,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
        sync::atomic::{AtomicU64, Ordering},
    };
    fn name(value: &std::ffi::OsStr) -> io::Result<CString> {
        CString::new(value.as_bytes()).map_err(|_| io::Error::other("Invalid mirror name"))
    }
    fn open_directory(
        parent: &fs::File,
        component: &std::ffi::OsStr,
        create: bool,
    ) -> io::Result<fs::File> {
        let component = name(component)?;
        if create {
            // SAFETY: a live directory descriptor and terminated component.
            let result = unsafe { libc::mkdirat(parent.as_raw_fd(), component.as_ptr(), 0o700) };
            if result != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(io::Error::last_os_error());
            }
        }
        // O_NOFOLLOW on every component prevents swapped symlinks escaping
        // the retained root, unlike canonicalize-then-write path checks.
        // SAFETY: valid descriptor, C string and no creation mode needed.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a newly owned descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
    pub(in crate::overleaf) fn root_directory(root: &Path) -> io::Result<fs::File> {
        root_directory_with(root, true)
    }
    pub(in crate::overleaf) fn root_directory_existing(root: &Path) -> io::Result<fs::File> {
        root_directory_with(root, false)
    }
    fn root_directory_with(root: &Path, create: bool) -> io::Result<fs::File> {
        let mut result = fs::File::open("/")?;
        for component in root.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(part) => result = open_directory(&result, part, create)?,
                _ => return Err(io::Error::other("Mirror path must be normalized")),
            }
        }
        Ok(result)
    }
    pub(super) fn directory(
        root: &fs::File,
        relative: &Path,
        create: bool,
    ) -> io::Result<fs::File> {
        let mut result = root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return Err(io::Error::other("Invalid mirror directory"));
            };
            result = open_directory(&result, part, create)?;
        }
        Ok(result)
    }
    fn parent(root: &fs::File, path: &Path, create: bool) -> io::Result<(fs::File, CString)> {
        let parent = directory(root, path.parent().unwrap_or(Path::new("")), create)?;
        let file = name(
            path.file_name()
                .ok_or_else(|| io::Error::other("Missing mirror file name"))?,
        )?;
        Ok((parent, file))
    }
    pub(in crate::overleaf) fn read(
        root: &fs::File,
        path: &Path,
        cap: usize,
    ) -> io::Result<Vec<u8>> {
        let (parent, file) = parent(root, path, false)?;
        // SAFETY: validated terminated filename under retained parent.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                file.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: new descriptor returned by openat.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("Mirror file is not regular"));
        }
        let mut bytes = Vec::new();
        file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > cap {
            return Err(io::Error::other("Mirror manifest is too large"));
        }
        Ok(bytes)
    }
    pub(super) fn remove(root: &fs::File, path: &Path) -> io::Result<()> {
        let (parent, file) = match parent(root, path, false) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        // unlinkat removes a link itself; it never follows its target.
        // SAFETY: retained parent descriptor and terminated name.
        let result = unsafe { libc::unlinkat(parent.as_raw_fd(), file.as_ptr(), 0) };
        if result == 0 || io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    pub(in crate::overleaf) fn write(root: &fs::File, path: &Path, bytes: &[u8]) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let (parent, file) = parent(root, path, true)?;
        // An existing symlink is rejected explicitly; renameat also never
        // follows a target swapped in after this check.
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: valid pointers and a writeable stat result.
        let existing = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                file.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if existing == 0 {
            // SAFETY: successful fstatat initialized the output.
            let stat = unsafe { stat.assume_init() };
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err(io::Error::other("Mirror target is not a regular file"));
            }
        } else if io::Error::last_os_error().kind() != io::ErrorKind::NotFound {
            return Err(io::Error::last_os_error());
        }
        let temporary = CString::new(format!(
            ".ttt-overleaf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
        .expect("ASCII temporary filename");
        // SAFETY: a retained parent, terminated name and explicit create mode.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: newly owned descriptor.
        let mut output = unsafe { fs::File::from_raw_fd(fd) };
        let result = (|| {
            output.write_all(bytes)?;
            output.sync_all()?;
            // SAFETY: two filenames in the same retained parent directory.
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    temporary.as_ptr(),
                    parent.as_raw_fd(),
                    file.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            parent.sync_all()
        })();
        if result.is_err() {
            // SAFETY: best-effort removal of our own temporary filename.
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result
    }
}

#[cfg(all(test, unix))]
mod manifest_tests {
    use super::*;
    fn long_listing(count: usize) -> Vec<Entry> {
        // Shared real-world directory nesting: components stay under ordinary
        // filesystem filename limits, but the aggregate listing exceeds 8 MiB.
        let prefix = (0..14)
            .map(|index| format!("{index:02}{}", "x".repeat(198)))
            .collect::<Vec<_>>()
            .join("/");
        (0..count)
            .map(|index| Entry {
                id: format!("doc{index}"),
                path: format!("{prefix}/file{index}.tex"),
                kind: EntryKind::Doc,
            })
            .collect()
    }
    #[test]
    fn large_server_manifest_is_readable_on_the_next_mirror_update() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mirror = Mirror::open(&root).unwrap();
        let tree = Tree::new("Large project".into(), None, long_listing(3100)).unwrap();
        let encoded = serde_json::to_vec(tree.entries()).unwrap();
        assert!(encoded.len() > 8 * 1024 * 1024);
        assert!(encoded.len() < MAX_TREE_MANIFEST_BYTES);
        // Simulate the retained listing from an earlier background session;
        // remote files may legitimately not be hydrated yet.
        unix::write(
            &mirror.directory,
            Path::new(".tiptoptyp/overleaf-files.json"),
            &encoded,
        )
        .unwrap();
        fs::write(root.join("generated.pdf"), b"preserve compiler output").unwrap();
        let next = Mirror::open(&root).unwrap();
        next.apply_tree(&Tree::new("Empty project".into(), None, Vec::new()).unwrap())
            .unwrap();
        assert_eq!(
            fs::read(root.join(".tiptoptyp/overleaf-files.json")).unwrap(),
            b"[]"
        );
        assert_eq!(
            fs::read(root.join("generated.pdf")).unwrap(),
            b"preserve compiler output"
        );
    }
    #[test]
    fn oversized_serialized_listings_are_rejected_before_mirror_side_effects() {
        let entries = long_listing(6200);
        assert!(serde_json::to_vec(&entries).unwrap().len() > MAX_TREE_MANIFEST_BYTES);
        assert!(
            Tree::new("Too large".into(), None, entries.clone())
                .unwrap_err()
                .contains("16 MiB")
        );
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mirror = Mirror::open(&root).unwrap();
        let retained = Entry {
            id: "old".into(),
            path: "retained.tex".into(),
            kind: EntryKind::Doc,
        };
        mirror
            .apply_tree(&Tree::new("Old project".into(), None, vec![retained.clone()]).unwrap())
            .unwrap();
        mirror.write_document(&retained, "retained draft").unwrap();
        let manifest = fs::read(root.join(".tiptoptyp/overleaf-files.json")).unwrap();
        // The writer boundary checks again even if a caller mutates its local
        // presentation copy after initial Tree validation.
        let mut oversized = Tree::new("Too large".into(), None, Vec::new()).unwrap();
        oversized.entries = entries;
        assert!(
            mirror
                .apply_tree(&oversized)
                .unwrap_err()
                .contains("16 MiB")
        );
        assert_eq!(
            fs::read(root.join("retained.tex")).unwrap(),
            b"retained draft"
        );
        assert_eq!(
            fs::read(root.join(".tiptoptyp/overleaf-files.json")).unwrap(),
            manifest
        );
    }
}
