//! A batch close consumes one answer per window and commits only as a whole.
use crate::document::DocumentKey;
#[derive(Default)]
pub struct CloseCoordinator {
    batch: Option<Batch>,
}
struct Batch {
    documents: Vec<DocumentKey>,
    next: usize,
}
impl CloseCoordinator {
    pub fn begin(&mut self, documents: Vec<DocumentKey>) -> bool {
        if self.batch.is_some() || documents.is_empty() {
            return false;
        }
        self.batch = Some(Batch { documents, next: 0 });
        true
    }
    pub fn is_active(&self) -> bool {
        self.batch.is_some()
    }
    pub fn current(&self) -> Option<DocumentKey> {
        self.batch
            .as_ref()
            .and_then(|batch| batch.documents.get(batch.next))
            .copied()
    }
    pub fn accept(&mut self, requested: DocumentKey, approved: DocumentKey) -> bool {
        if self.current() != Some(requested) || requested.owner != approved.owner {
            return false;
        }
        let batch = self.batch.as_mut().unwrap();
        batch.documents[batch.next] = approved;
        batch.next += 1;
        true
    }
    pub fn ready(&self, current: &[DocumentKey]) -> bool {
        self.batch
            .as_ref()
            .is_some_and(|batch| batch.next == batch.documents.len() && batch.documents == current)
    }
    pub fn can_exit(&self, current: &[DocumentKey], operations_pending: bool) -> bool {
        !operations_pending && self.ready(current)
    }
    pub fn cancel(&mut self) {
        self.batch = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::WindowSessionId;
    fn key(owner: u64, revision: u64) -> DocumentKey {
        DocumentKey::new(WindowSessionId::new(owner), 0, revision)
    }
    #[test]
    fn one_cancel_preserves_all_windows_and_consumes_old_approvals() {
        let keys = vec![key(1, 0), key(2, 0), key(3, 0)];
        let mut close = CloseCoordinator::default();
        assert!(close.begin(keys.clone()));
        assert!(close.accept(keys[0], keys[0]));
        assert!(!close.accept(keys[0], keys[0]));
        close.cancel();
        assert!(!close.ready(&keys));
        assert!(!close.accept(keys[1], keys[1]));
        assert!(close.begin(keys.clone()));
        assert_eq!(close.current(), Some(keys[0]));
    }
    #[test]
    fn saved_versions_are_approved_but_later_edits_or_new_windows_abort_commit() {
        let mut close = CloseCoordinator::default();
        close.begin(vec![key(1, 0), key(2, 0)]);
        assert!(!close.accept(key(1, 0), key(2, 0)));
        assert!(close.accept(key(1, 0), key(1, 1)));
        assert!(close.accept(key(2, 0), key(2, 0)));
        assert!(close.ready(&[key(1, 1), key(2, 0)]));
        assert!(!close.can_exit(&[key(1, 1), key(2, 0)], true));
        assert!(close.can_exit(&[key(1, 1), key(2, 0)], false));
        assert!(!close.ready(&[key(1, 2), key(2, 0)]));
        assert!(!close.ready(&[key(1, 1), key(2, 0), key(3, 0)]));
    }
}
