use super::{
    ImportResult, MAX_BIB_BYTES,
    bibtex::{self, Entry},
    check_cancel,
};
use std::{
    fs::{self, File, Metadata, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

/// The destination as reviewed, including its exact bytes and file identity.
/// Read and import on workers. A modified/replaced destination requires a fresh review.
#[derive(Clone, Debug)]
pub(crate) struct BibliographySnapshot {
    pub(crate) path: PathBuf,
    pub(crate) exists: bool,
    bytes: Vec<u8>,
    metadata: Option<Metadata>,
}

impl BibliographySnapshot {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        validate_path(path)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(
                        "Choose a regular .bib file, not a directory or symbolic link.".into(),
                    );
                }
                let mut file = open_existing(path, false)?;
                let opened = file
                    .metadata()
                    .map_err(|_| "The bibliography could not be inspected.")?;
                if !same_file(&metadata, &opened) {
                    return Err(changed());
                }
                let bytes = read_bounded(&mut file)?;
                bibtex::parse_database(text(&bytes)?)?;
                let after = fs::symlink_metadata(path).map_err(|_| changed())?;
                if !same_file(&opened, &after) || after.len() != bytes.len() as u64 {
                    return Err(changed());
                }
                Ok(Self {
                    path: path.to_owned(),
                    exists: true,
                    bytes,
                    metadata: Some(opened),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !path.parent().is_some_and(Path::is_dir) {
                    return Err("Choose an existing folder for the bibliography.".into());
                }
                Ok(Self {
                    path: path.to_owned(),
                    exists: false,
                    bytes: Vec::new(),
                    metadata: None,
                })
            }
            Err(_) => Err("The bibliography could not be read. Check its permissions.".into()),
        }
    }

    pub(crate) fn import(
        &self,
        reviewed_bibtex: &str,
        cancel: &AtomicBool,
    ) -> Result<ImportResult, String> {
        check_cancel(cancel)?;
        let incoming = bibtex::parse_entry(reviewed_bibtex)?;
        crate::resource_lock::with_resource(&self.path, || self.import_locked(incoming, cancel))
    }

    fn import_locked(&self, incoming: Entry, cancel: &AtomicBool) -> Result<ImportResult, String> {
        check_cancel(cancel)?;
        let suffix = format!(
            "{}{}\n",
            if self.bytes.is_empty() || self.bytes.ends_with(b"\n\n") {
                ""
            } else if self.bytes.ends_with(b"\n") {
                "\n"
            } else {
                "\n\n"
            },
            incoming.bibtex
        );
        if self.bytes.len() + suffix.len() > MAX_BIB_BYTES {
            return Err("Import would exceed the 16 MiB bibliography limit.".into());
        }
        if self.exists {
            let mut file = open_existing(&self.path, true)?;
            lock(&file, cancel)?;
            let metadata = file.metadata().map_err(|_| changed())?;
            if !self
                .metadata
                .as_ref()
                .is_some_and(|old| same_file(old, &metadata))
            {
                return Err(changed());
            }
            let current = read_bounded(&mut file)?;
            let path_metadata = fs::symlink_metadata(&self.path).map_err(|_| changed())?;
            if current != self.bytes
                || !same_file(&metadata, &path_metadata)
                || path_metadata.file_type().is_symlink()
            {
                return Err(changed());
            }
            if let Some(existing) = duplicate(&bibtex::parse_database(text(&current)?)?, &incoming)?
            {
                bibtex::validate_key(&existing.key)?;
                return Ok(ImportResult {
                    key: existing.key,
                    bibtex: existing.bibtex,
                    added: false,
                });
            }
            check_cancel(cancel)?;
            // O_APPEND preserves all pre-existing bytes, including an external writer
            // that does not participate in advisory locking. Never truncate/replace.
            file.write_all(suffix.as_bytes()).map_err(|_| "The citation could not be appended. Check available disk space and reopen the bibliography.")?;
            file.sync_all()
                .map_err(|_| "The bibliography could not be saved to disk.")?;
            let after = fs::symlink_metadata(&self.path).map_err(|_| changed())?;
            if !same_file(&metadata, &after) {
                return Err(changed());
            }
        } else {
            if fs::symlink_metadata(&self.path).is_ok() {
                return Err(changed());
            }
            let parent = self.path.parent().ok_or("Choose a bibliography folder.")?;
            let mut file = tempfile::NamedTempFile::new_in(parent)
                .map_err(|_| "The bibliography could not be created in that folder.")?;
            file.write_all(suffix.as_bytes())
                .and_then(|()| file.as_file().sync_all())
                .map_err(|_| "The bibliography could not be saved to disk.")?;
            check_cancel(cancel)?;
            file.persist_noclobber(&self.path).map_err(|_| "The bibliography already exists or could not be created. Review the destination again.")?;
        }
        Ok(ImportResult {
            key: incoming.key,
            bibtex: incoming.bibtex,
            added: true,
        })
    }
}

fn duplicate(entries: &[Entry], incoming: &Entry) -> Result<Option<Entry>, String> {
    let doi = incoming.doi();
    let arxiv = incoming.arxiv();
    if let Some(entry) = entries.iter().find(|entry| {
        doi.as_ref()
            .is_some_and(|doi| entry.doi().as_ref() == Some(doi))
            || arxiv
                .as_ref()
                .is_some_and(|arxiv| entry.arxiv().as_ref() == Some(arxiv))
    }) {
        return Ok(Some(entry.clone()));
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.key.eq_ignore_ascii_case(&incoming.key))
    {
        if entry.fields == incoming.fields {
            return Ok(Some(entry.clone()));
        }
        return Err(format!(
            "Citation key “{}” is already used by a different entry. Choose another key.",
            incoming.key
        ));
    }
    Ok(None)
}

fn validate_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bib"))
    {
        return Err("Choose an absolute path ending in .bib.".into());
    }
    Ok(())
}
fn text(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| {
        "The bibliography is not UTF-8. Convert it to UTF-8 before importing.".to_owned()
    })
}
fn read_bounded(file: &mut File) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take((MAX_BIB_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "The bibliography could not be read.")?;
    if bytes.len() > MAX_BIB_BYTES {
        return Err("The bibliography exceeds the 16 MiB limit.".into());
    }
    Ok(bytes)
}
fn open_existing(path: &Path, append: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).append(append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(
        |_| "The bibliography could not be opened. It may have changed or be read-only.",
    )?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("Choose a regular .bib file.".into());
    }
    Ok(file)
}
fn same_file(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.created().ok() == right.created().ok() && left.is_file() == right.is_file()
    }
}
fn changed() -> String {
    "The bibliography changed since review. Look up the citation again before importing.".into()
}
fn lock(file: &File, cancel: &AtomicBool) -> Result<(), String> {
    lock_with_timeout(file, cancel, Duration::from_secs(5))
}
fn lock_with_timeout(file: &File, cancel: &AtomicBool, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        check_cancel(cancel)?;
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(TryLockError::WouldBlock) => {
                return Err(
                    "The bibliography is busy in another application. Try again shortly.".into(),
                );
            }
            Err(TryLockError::Error(_)) => {
                return Err("The bibliography could not be locked for a safe import.".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_file_lock_times_out_observes_cancel_and_recovers_without_wall_time_assertions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locked.bib");
        fs::write(&path, "").unwrap();
        let held = File::open(&path).unwrap();
        held.lock().unwrap();
        let contender = File::open(&path).unwrap();
        let error =
            lock_with_timeout(&contender, &AtomicBool::new(false), Duration::ZERO).unwrap_err();
        assert!(error.contains("busy in another application"));
        let error =
            lock_with_timeout(&contender, &AtomicBool::new(true), Duration::ZERO).unwrap_err();
        assert!(error.contains("cancelled"));
        drop(held);
        lock_with_timeout(&contender, &AtomicBool::new(false), Duration::ZERO).unwrap();
    }
}
