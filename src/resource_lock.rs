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
}
