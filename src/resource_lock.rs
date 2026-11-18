//! App-owned writes to one canonical resource are serial across windows.
//! Callers re-read preconditions inside the lock. This is not an OS file lock.
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock, Weak},
};

type LockMap = BTreeMap<std::path::PathBuf, Weak<Mutex<()>>>;
static LOCKS: OnceLock<Mutex<LockMap>> = OnceLock::new();

fn resource_lock(path: &Path) -> Arc<Mutex<()>> {
    let key = path.canonicalize().unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| parent.canonicalize().ok())
            .zip(path.file_name())
            .map_or_else(|| path.to_owned(), |(parent, name)| parent.join(name))
    });
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    locks.retain(|_, weak| weak.strong_count() != 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub(crate) fn with_resource<R>(path: &Path, operation: impl FnOnce() -> R) -> R {
    let lock = resource_lock(path);
    // The mutex owns no data: after a panic the adapter re-reads the resource.
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    operation()
}

/// Interactive filesystem mutations must neither race an active save nor wait
/// for a worker on the UI thread. The caller can display the busy error and retry.
pub(crate) fn try_with_resource<R>(
    path: &Path,
    operation: impl FnOnce() -> std::io::Result<R>,
) -> std::io::Result<R> {
    let lock = resource_lock(path);
    let _guard = match lock.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "The file is busy in another operation. Try again after it finishes.",
            ));
        }
    };
    operation()
}

#[cfg(test)]
pub(crate) fn is_locked_for_test(path: &Path) -> bool {
    matches!(
        resource_lock(path).try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_share_a_lock_but_other_resources_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let first = resource_lock(&dir.path().join("note.txt"));
        let second = resource_lock(&dir.path().join("./note.txt"));
        assert!(Arc::ptr_eq(&first, &second));
        let _guard = first.lock().unwrap();
        assert!(matches!(
            second.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let other = resource_lock(&dir.path().join("other.txt"));
        assert!(other.try_lock().is_ok());
    }

    #[test]
    fn interactive_mutations_refuse_an_owned_resource_without_running_or_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.txt");
        with_resource(&path, || {
            let error = try_with_resource::<()>(&directory.path().join("./note.txt"), || {
                panic!("a busy mutation must not execute")
            })
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
            assert!(error.to_string().contains("Try again"));
            assert!(try_with_resource(&directory.path().join("other.txt"), || Ok(())).is_ok());
        });
        assert_eq!(try_with_resource(&path, || Ok(7)).unwrap(), 7);
    }
}
