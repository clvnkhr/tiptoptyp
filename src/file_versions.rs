//! macOS document revisions, kept with the file by the system version store.
//!
//! Foundation objects never cross worker boundaries. Call these functions on a
//! worker: file coordination and revision copies may wait for other processes.
use std::{io, path::Path, time::SystemTime};
use tiptoptyp::save_transaction::{SaveIntent, WriteDurability};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VersionInfo {
    pub id: String,
    pub saved_at: SystemTime,
}

pub(crate) const AVAILABLE: bool = cfg!(target_os = "macos");

pub(crate) fn list(path: &Path) -> Result<Vec<VersionInfo>, String> {
    platform::list(path).map_err(|error| error.to_string())
}

pub(crate) fn read(path: &Path, id: &str) -> Result<String, String> {
    platform::read(path, id).map_err(|error| error.to_string())
}

/// Preserve the current disk contents, without modifying the user file.
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn checkpoint(path: &Path) -> Result<(), String> {
    platform::checkpoint(path).map_err(|error| error.to_string())
}

/// Preserve an unsaved buffer as a native revision without changing the file.
pub(crate) fn checkpoint_contents(path: &Path, source: &str) -> Result<(), String> {
    platform::checkpoint_contents(path, source).map_err(|error| error.to_string())
}

/// Revision errors are separate from save errors: committed source stays saved.
pub(crate) fn write_checked(
    path: &Path,
    contents: &[u8],
    intent: SaveIntent,
    check: impl FnOnce() -> io::Result<()>,
) -> io::Result<(WriteDurability, Option<String>)> {
    platform::write_checked(path, contents, intent, check)
}

/// Automatic saving may run on every edit; native snapshots are coalesced to
/// one per minute. Explicit saves and the first write always preserve history.
#[cfg(any(target_os = "macos", test))]
fn checkpoint_due(intent: SaveIntent, latest: Option<SystemTime>, now: SystemTime) -> bool {
    intent != SaveIntent::Auto
        || latest.is_none_or(|last| {
            now.duration_since(last)
                .is_ok_and(|elapsed| elapsed >= std::time::Duration::from_secs(60))
        })
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    fn unavailable() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Local file versions require macOS",
        )
    }
    pub(super) fn list(_: &Path) -> io::Result<Vec<VersionInfo>> {
        Err(unavailable())
    }
    pub(super) fn read(_: &Path, _: &str) -> io::Result<String> {
        Err(unavailable())
    }
    pub(super) fn checkpoint_contents(_: &Path, _: &str) -> io::Result<()> {
        Err(unavailable())
    }
    pub(super) fn write_checked(
        path: &Path,
        contents: &[u8],
        _intent: SaveIntent,
        check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<(WriteDurability, Option<String>)> {
        crate::private_workspace::AtomicFileWriter::write_checked(path, contents, check)
            .map(|durability| (durability, None))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use block2::RcBlock;
    use objc2::{rc::autoreleasepool, runtime::AnyObject};
    use objc2_foundation::{
        NSFileCoordinator, NSFileCoordinatorReadingOptions, NSFileCoordinatorWritingOptions,
        NSFileManager, NSFileManagerItemReplacementOptions, NSFileVersion,
        NSFileVersionAddingOptions, NSKeyedArchiver, NSURL,
    };
    use std::{
        cell::RefCell,
        collections::VecDeque,
        fs::{self, File},
        io::{BufReader, Read, Write},
        os::unix::fs::MetadataExt,
        path::PathBuf,
        ptr::NonNull,
        sync::{Mutex, OnceLock},
        time::{Duration, UNIX_EPOCH},
    };

    const RECENT_SAVES: usize = 128;
    type DiskStamp = (u64, u64, u64, i64, i64);
    type SavedFiles = VecDeque<(PathBuf, DiskStamp)>;
    static SAVED_FILES: OnceLock<Mutex<SavedFiles>> = OnceLock::new();

    fn disk_stamp(metadata: &fs::Metadata) -> DiskStamp {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
        )
    }

    fn saved_by_app(path: &Path, metadata: &fs::Metadata) -> bool {
        SAVED_FILES.get().is_some_and(|saved| {
            saved
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .any(|(saved_path, stamp)| saved_path == path && *stamp == disk_stamp(metadata))
        })
    }

    fn note_saved(path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let mut saved = SAVED_FILES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        remember_saved(&mut saved, path, disk_stamp(&metadata));
    }

    fn remember_saved(saved: &mut SavedFiles, path: &Path, stamp: DiskStamp) {
        saved.retain(|(saved_path, _)| saved_path != path);
        if saved.len() == RECENT_SAVES {
            saved.pop_front();
        }
        saved.push_back((path.to_owned(), stamp));
    }

    fn file_url(path: &Path) -> io::Result<objc2::rc::Retained<NSURL>> {
        NSURL::from_file_path(path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid file path"))
    }

    fn at_requested_path<T>(
        requested: &Path,
        accessor: &Path,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        if requested != accessor {
            return Err(io::Error::other(
                "The file moved while waiting for access. Reopen the document and try again.",
            ));
        }
        operation()
    }

    fn coordinated<T>(
        path: &Path,
        writing: bool,
        operation: impl FnOnce(&Path, &NSURL) -> io::Result<T>,
    ) -> io::Result<T> {
        crate::resource_lock::with_resource(path, || {
            autoreleasepool(|_| {
                let url = file_url(path)?;
                // Capture Foundation's normalized request before another process
                // can move it. The lease and save receipt belong to this path.
                let requested_path = url.to_file_path().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Invalid file path")
                })?;
                let coordinator = NSFileCoordinator::new();
                let operation = RefCell::new(Some(operation));
                let result = RefCell::new(None);
                let accessor = RcBlock::new(|url: NonNull<NSURL>| {
                    // Foundation invokes this accessor synchronously and keeps
                    // its coordinated URL alive until the block returns.
                    let url = unsafe { url.as_ref() };
                    let value = url
                        .to_file_path()
                        .ok_or_else(|| io::Error::other("The coordinated file is unavailable"))
                        .and_then(|path| {
                            at_requested_path(&requested_path, &path, || {
                                operation.borrow_mut().take().ok_or_else(|| {
                                    io::Error::other("File coordinator repeated a transaction")
                                })?(&path, url)
                            })
                        });
                    *result.borrow_mut() = Some(value);
                });
                let mut error = None;
                if writing {
                    coordinator.coordinateWritingItemAtURL_options_error_byAccessor(
                        &url,
                        NSFileCoordinatorWritingOptions::empty(),
                        Some(&mut error),
                        &accessor,
                    );
                } else {
                    coordinator.coordinateReadingItemAtURL_options_error_byAccessor(
                        &url,
                        NSFileCoordinatorReadingOptions::empty(),
                        Some(&mut error),
                        &accessor,
                    );
                }
                drop(accessor);
                // An accessor result takes precedence: after a committed save,
                // a late coordination error must not turn success into failure.
                result.into_inner().unwrap_or_else(|| {
                    Err(io::Error::other(error.map_or_else(
                        || "File coordination did not run".into(),
                        |error| error.localizedDescription().to_string(),
                    )))
                })
            })
        })
    }

    fn version_time(version: &NSFileVersion) -> Option<SystemTime> {
        let seconds = version.modificationDate()?.timeIntervalSince1970();
        if !seconds.is_finite() {
            return None;
        }
        let duration = Duration::try_from_secs_f64(seconds.abs()).ok()?;
        if seconds >= 0.0 {
            UNIX_EPOCH.checked_add(duration)
        } else {
            UNIX_EPOCH.checked_sub(duration)
        }
    }

    fn version_id(version: &NSFileVersion) -> io::Result<String> {
        let identifier = version.persistentIdentifier();
        // The native identifier conforms to NSCoding. We only encode and match
        // these values; untrusted strings are never decoded as Objective-C objects.
        let object: &AnyObject = identifier.as_ref();
        let data = unsafe {
            NSKeyedArchiver::archivedDataWithRootObject_requiringSecureCoding_error(object, true)
        }
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))?;
        let bytes = data.to_vec();
        let mut id = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(id, "{byte:02x}").expect("writing a String cannot fail");
        }
        Ok(id)
    }

    fn versions(url: &NSURL) -> Vec<objc2::rc::Retained<NSFileVersion>> {
        NSFileVersion::otherVersionsOfItemAtURL(url)
            .map_or_else(Vec::new, |versions| versions.to_vec())
    }

    pub(super) fn list(path: &Path) -> io::Result<Vec<VersionInfo>> {
        coordinated(path, false, |path, url| {
            fs::metadata(path)?;
            let mut result = Vec::new();
            for version in versions(url) {
                if let Some(saved_at) = version_time(&version) {
                    result.push(VersionInfo {
                        id: version_id(&version)?,
                        saved_at,
                    });
                }
            }
            result.sort_by_key(|version| std::cmp::Reverse(version.saved_at));
            Ok(result)
        })
    }

    pub(super) fn read(path: &Path, id: &str) -> io::Result<String> {
        coordinated(path, false, |_, url| {
            for version in versions(url) {
                if version_id(&version)? == id {
                    let path = version.URL().to_file_path().ok_or_else(|| {
                        io::Error::other("The saved version is no longer available locally")
                    })?;
                    return fs::read_to_string(path);
                }
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The saved version is no longer available",
            ))
        })
    }

    fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
        let left = File::open(left)?;
        let right = File::open(right)?;
        if left.metadata()?.len() != right.metadata()?.len() {
            return Ok(false);
        }
        // File size does not affect comparison memory or temporary-copy count.
        let mut left = BufReader::new(left);
        let mut right = BufReader::new(right);
        let mut a = [0_u8; 16 * 1024];
        let mut b = [0_u8; 16 * 1024];
        loop {
            let count = left.read(&mut a)?;
            right.read_exact(&mut b[..count])?;
            if a[..count] != b[..count] {
                return Ok(false);
            }
            if count == 0 {
                return Ok(true);
            }
        }
    }

    fn contents_equal(path: &Path, contents: &[u8]) -> io::Result<bool> {
        let mut file = File::open(path)?;
        if file.metadata()?.len() != contents.len() as u64 {
            return Ok(false);
        }
        let mut offset = 0;
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                return Ok(offset == contents.len());
            }
            if contents.get(offset..offset + count) != Some(&buffer[..count]) {
                return Ok(false);
            }
            offset += count;
        }
    }

    fn latest_version(url: &NSURL) -> Option<(SystemTime, objc2::rc::Retained<NSFileVersion>)> {
        versions(url)
            .into_iter()
            .filter_map(|version| version_time(&version).map(|time| (time, version)))
            .max_by_key(|(time, _)| *time)
    }

    fn checkpoint_coordinated(path: &Path, url: &NSURL) -> io::Result<()> {
        // Only compare the most recent revision. Returning to earlier text is
        // still a meaningful new revision rather than a global deduplication.
        if let Some(latest) =
            latest_version(url).and_then(|(_, version)| version.URL().to_file_path())
            && files_equal(path, &latest)?
        {
            return Ok(());
        }
        let contents_url = file_url(path)?;
        NSFileVersion::addVersionOfItemAtURL_withContentsOfURL_options_error(
            url,
            &contents_url,
            NSFileVersionAddingOptions::empty(),
        )
        .map(|_| ())
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
    }

    #[cfg(test)]
    pub(super) fn checkpoint(path: &Path) -> io::Result<()> {
        coordinated(path, true, checkpoint_coordinated)
    }

    pub(super) fn checkpoint_contents(path: &Path, source: &str) -> io::Result<()> {
        coordinated(path, true, |path, url| {
            fs::metadata(path)?;
            let private = crate::private_workspace::PrivateWorkspace::open(
                path.parent()
                    .ok_or_else(|| io::Error::other("File has no parent directory"))?,
            )?;
            let mut temporary = private.temp_file("version-", ".tmp")?;
            temporary.write_all(source.as_bytes())?;
            temporary.as_file().sync_all()?;
            checkpoint_coordinated(temporary.path(), url)
        })
    }

    pub(super) fn write_checked(
        path: &Path,
        contents: &[u8],
        intent: SaveIntent,
        check: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<(WriteDurability, Option<String>)> {
        coordinated(path, true, |path, url| {
            // Validation precedes all revision-store mutations. Rejected saves
            // neither change the source nor manufacture a backup of stale input.
            check()?;
            let existing = match fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => Some(metadata),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Save destination is not a regular file",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            let mut warning = None;
            let latest = latest_version(url).map(|(time, _)| time);
            let due = checkpoint_due(intent, latest, SystemTime::now());
            // Process bookkeeping contains no source or revision data. It only
            // recognizes our last successful write so the first overwrite (or
            // an external inode/mtime change) preserves the exact disk baseline.
            let preserve_original = existing
                .as_ref()
                .is_some_and(|metadata| !saved_by_app(path, metadata))
                || intent != SaveIntent::Auto;
            if existing.is_some() && contents_equal(path, contents)? {
                if due || preserve_original {
                    warning = checkpoint_coordinated(path, url)
                        .err()
                        .map(|error| error.to_string());
                }
                let durability = match File::open(path).and_then(|file| file.sync_all()) {
                    Ok(()) => WriteDurability::Synchronized,
                    Err(error) => WriteDurability::Uncertain(error.to_string()),
                };
                note_saved(path);
                return Ok((durability, warning));
            }
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "File has no parent directory")
            })?;
            let private = crate::private_workspace::PrivateWorkspace::open(parent)?;
            let mut temporary = private.temp_file("write-", ".tmp")?;
            temporary.write_all(contents)?;
            if let Some(metadata) = &existing {
                temporary
                    .as_file()
                    .set_permissions(metadata.permissions())?;
            }
            temporary.as_file().sync_all()?;
            // The first overwrite preserves the original; later autosaves
            // coalesce into one new version per minute rather than two.
            if existing.is_some() && preserve_original {
                warning = checkpoint_coordinated(path, url)
                    .err()
                    .map(|error| error.to_string());
            }
            if existing.is_some() {
                let staged_url = file_url(temporary.path())?;
                NSFileManager::defaultManager()
                    .replaceItemAtURL_withItemAtURL_backupItemName_options_resultingItemURL_error(
                        url,
                        &staged_url,
                        None,
                        NSFileManagerItemReplacementOptions::empty(),
                        None,
                    )
                    .map_err(|error| io::Error::other(error.localizedDescription().to_string()))?;
            } else {
                temporary
                    .persist_noclobber(path)
                    .map_err(|error| error.error)?;
            }
            let durability = match File::open(parent).and_then(|file| file.sync_all()) {
                Ok(()) => WriteDurability::Synchronized,
                Err(error) => WriteDurability::Uncertain(error.to_string()),
            };
            if due && let Err(error) = checkpoint_coordinated(path, url) {
                warning.get_or_insert_with(|| error.to_string());
            }
            note_saved(path);
            Ok((durability, warning))
        })
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn coordinated_path_guard_accepts_the_original_normalized_url() {
            let requested = file_url(Path::new("/tmp/project/./main.typ"))
                .unwrap()
                .to_file_path()
                .unwrap();
            let accessor = file_url(Path::new("/tmp/project/main.typ"))
                .unwrap()
                .to_file_path()
                .unwrap();
            let result = at_requested_path(&requested, &accessor, || Ok("ran"));
            assert_eq!(result.unwrap(), "ran");
        }

        #[test]
        fn coordinated_path_guard_rejects_moved_urls_before_any_operation() {
            let ran = std::cell::Cell::new(false);
            let result = at_requested_path(
                Path::new("/tmp/project/main.typ"),
                Path::new("/tmp/elsewhere/main.typ"),
                || {
                    ran.set(true);
                    Ok(())
                },
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("Reopen the document")
            );
            assert!(!ran.get());
        }

        #[test]
        fn recent_save_bookkeeping_is_bounded_and_replaces_old_disk_ownership() {
            let mut saved = SavedFiles::new();
            for index in 0..RECENT_SAVES + 10 {
                remember_saved(&mut saved, Path::new(&index.to_string()), (1, 2, 3, 4, 5));
            }
            assert_eq!(saved.len(), RECENT_SAVES);
            assert_eq!(saved.front().unwrap().0, Path::new("10"));
            remember_saved(&mut saved, Path::new("10"), (6, 7, 8, 9, 10));
            assert_eq!(saved.len(), RECENT_SAVES);
            assert_eq!(saved.back().unwrap().1, (6, 7, 8, 9, 10));
            assert_eq!(
                saved
                    .iter()
                    .filter(|(path, _)| path == Path::new("10"))
                    .count(),
                1
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_checkpoints_are_coalesced_but_explicit_saves_are_immediate() {
        use std::time::{Duration, UNIX_EPOCH};
        let last = UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(checkpoint_due(SaveIntent::Auto, None, last));
        assert!(!checkpoint_due(
            SaveIntent::Auto,
            Some(last),
            last + Duration::from_secs(59)
        ));
        assert!(checkpoint_due(
            SaveIntent::Auto,
            Some(last),
            last + Duration::from_secs(60)
        ));
        assert!(checkpoint_due(SaveIntent::Explicit, Some(last), last));
        assert!(checkpoint_due(
            SaveIntent::ExplicitConfirmed { observed: None },
            Some(last),
            last
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dirty_buffer_checkpoint_preserves_both_disk_and_unsaved_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native-versions.typ");
        std::fs::write(&path, "disk").unwrap();
        checkpoint_contents(&path, "unsaved buffer").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "disk");
        let versions = list(&path).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(read(&path, &versions[0].id).unwrap(), "unsaved buffer");
        checkpoint_contents(&path, "unsaved buffer").unwrap();
        assert_eq!(list(&path).unwrap().len(), 1);
        let (_, warning) =
            write_checked(&path, b"restored", SaveIntent::Explicit, || Ok(())).unwrap();
        assert!(warning.is_none(), "{warning:?}");
        let contents: Vec<_> = list(&path)
            .unwrap()
            .iter()
            .map(|v| read(&path, &v.id).unwrap())
            .collect();
        for text in ["disk", "unsaved buffer", "restored"] {
            assert!(
                contents.iter().any(|content| content == text),
                "{contents:?}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn first_overwrite_preserves_external_baseline_even_with_a_recent_native_version() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native-versions.typ");
        std::fs::write(&path, "previous native version").unwrap();
        checkpoint(&path).unwrap();
        std::fs::write(&path, "external baseline").unwrap();
        let (_, warning) =
            write_checked(&path, b"edited in app", SaveIntent::Auto, || Ok(())).unwrap();
        assert!(warning.is_none(), "{warning:?}");
        let contents: Vec<_> = list(&path)
            .unwrap()
            .iter()
            .map(|v| read(&path, &v.id).unwrap())
            .collect();
        assert_eq!(contents.len(), 2, "{contents:?}");
        assert!(
            contents.iter().any(|source| source == "external baseline"),
            "{contents:?}"
        );
        let (_, warning) =
            write_checked(&path, b"further typing", SaveIntent::Auto, || Ok(())).unwrap();
        assert!(warning.is_none(), "{warning:?}");
        assert_eq!(list(&path).unwrap().len(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn automatic_saves_keep_initial_source_without_a_revision_for_every_keystroke() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native-versions.typ");
        std::fs::write(&path, "original").unwrap();
        for text in ["a", "ab", "abc"] {
            let (_, warning) =
                write_checked(&path, text.as_bytes(), SaveIntent::Auto, || Ok(())).unwrap();
            assert!(warning.is_none(), "{warning:?}");
        }
        let versions = list(&path).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abc");
        assert!(
            versions
                .iter()
                .any(|v| read(&path, &v.id).unwrap() == "original")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_history_survives_atomic_saves_and_deduplicates_unchanged_saves() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native-versions.typ");
        std::fs::write(&path, "original").unwrap();
        for text in ["second", "third", "third"] {
            let (_, warning) =
                write_checked(&path, text.as_bytes(), SaveIntent::Explicit, || Ok(())).unwrap();
            assert!(warning.is_none(), "{warning:?}");
        }
        let versions = list(&path).unwrap();
        let contents: Vec<_> = versions
            .iter()
            .map(|v| read(&path, &v.id).unwrap())
            .collect();
        assert_eq!(contents.len(), 3, "{contents:?}");
        for text in ["original", "second", "third"] {
            assert!(
                contents.iter().any(|content| content == text),
                "{contents:?}"
            );
        }
        checkpoint(&path).unwrap();
        assert_eq!(list(&path).unwrap().len(), 3);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "third");
        assert!(read(&path, "not-a-native-version").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directory_save_target_fails_without_replacing_or_deleting_its_contents() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("directory.typ");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep.txt"), "must survive").unwrap();
        assert!(write_checked(&path, b"not a directory", SaveIntent::Explicit, || Ok(())).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("keep.txt")).unwrap(),
            "must survive"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejected_transaction_does_not_create_a_version_or_change_source() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native-versions.typ");
        std::fs::write(&path, "original").unwrap();
        let before = list(&path).unwrap();
        let result = write_checked(&path, b"rejected", SaveIntent::Auto, || {
            Err(io::Error::other("conflict"))
        });
        assert_eq!(result.unwrap_err().to_string(), "conflict");
        assert_eq!(list(&path).unwrap(), before);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn history_is_explicitly_unavailable_without_a_fake_fallback() {
        assert!(list(Path::new("file.typ")).unwrap_err().contains("macOS"));
    }
}
