//! Immutable save handoff. The IO adapter owns disk checks and execution timing;
//! document identity and receipt validation remain in the existing document model.
use crate::mitex_document::{SaveReceipt, SaveRequest};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
};
use tiptoptyp_core::workflow::SaveContinuationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveIntent {
    Explicit,
    ExplicitConfirmed { observed: Option<u64> },
    Auto,
}

/// What the admission adapter knew when constructing the request. Unchecked is
/// explicit: a Save As destination or a document without a known disk baseline
/// must not be confused with a file known to be missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedDiskState {
    Unchecked,
    Fingerprint(u64),
    Missing,
}

/// A failed persist has no receipt. Once persist succeeded, failed durability
/// confirmation is a committed outcome, never a retryable pre-commit failure.
#[derive(Debug)]
pub enum WriteDurability {
    Synchronized,
    Uncertain(String),
}

#[derive(Debug)]
pub struct SaveInput {
    request: SaveRequest,
    expected_disk: ExpectedDiskState,
    intent: SaveIntent,
    continuation: Option<SaveContinuationToken>,
}

impl SaveInput {
    pub fn new(
        request: SaveRequest,
        expected_disk: ExpectedDiskState,
        intent: SaveIntent,
        continuation: Option<SaveContinuationToken>,
    ) -> Self {
        Self {
            request,
            expected_disk,
            intent,
            continuation,
        }
    }
    pub fn path(&self) -> &Path {
        self.request.path()
    }
    pub fn bytes(&self) -> &[u8] {
        self.request.source().as_bytes()
    }
    pub fn expected_disk(&self) -> ExpectedDiskState {
        self.expected_disk
    }

    /// Consume the original request exactly once. Does not clone its canonical
    /// bytes, invent another document key, spawn work or touch the filesystem.
    /// The writer adapter must preserve pre-commit vs committed/uncertain errors.
    pub fn execute_with(
        self,
        writer: impl FnOnce(&Self) -> Result<WriteDurability, String>,
    ) -> SaveCompletion {
        let written_fingerprint = fingerprint(self.bytes());
        let result = writer(&self).map(|durability| CommittedSave {
            receipt: self.request.committed(written_fingerprint),
            durability,
        });
        SaveCompletion {
            intent: self.intent,
            expected_disk: self.expected_disk,
            continuation: self.continuation,
            result,
        }
    }
}

#[derive(Debug)]
pub struct CommittedSave {
    pub receipt: SaveReceipt,
    pub durability: WriteDurability,
}

#[derive(Debug)]
pub struct SaveCompletion {
    pub intent: SaveIntent,
    pub expected_disk: ExpectedDiskState,
    pub continuation: Option<SaveContinuationToken>,
    pub result: Result<CommittedSave, String>,
}

pub fn fingerprint(contents: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mitex_document::Document, mitex_projection::Config};
    use tiptoptyp_core::{
        document::{DocumentKind, WindowSessionId},
        workflow::Workflow,
    };

    const CANONICAL: &str = "#import \"@preview/mitex:0.2.7\": mi\n文 #mi(`\\alpha+😀`) end\n";

    #[test]
    fn immutable_handoff_borrows_canonical_bytes_and_preserves_receipt_identity() {
        for projected in [false, true] {
            let mut document =
                Document::new(WindowSessionId::new(1), CANONICAL, DocumentKind::Typst);
            if projected {
                document.enable(Config::default()).unwrap();
            }
            let request = document
                .prepare_save("paper.typ".into(), DocumentKind::Typst)
                .unwrap();
            let source_pointer = request.source().as_ptr();
            let mut workflow = Workflow::<&str, (), ()>::default();
            workflow.continue_after_save("close").unwrap();
            let token = workflow.continuation_token();
            let input = SaveInput::new(
                request,
                ExpectedDiskState::Fingerprint(17),
                SaveIntent::Explicit,
                token,
            );
            document.edit(0, |source| source.push('!'));
            let completion = input.execute_with(|input| {
                assert_eq!(
                    input.bytes().as_ptr(),
                    source_pointer,
                    "handoff must not copy source"
                );
                assert_eq!(input.bytes(), CANONICAL.as_bytes());
                assert_eq!(input.path(), Path::new("paper.typ"));
                assert_eq!(input.expected_disk(), ExpectedDiskState::Fingerprint(17));
                Ok(WriteDurability::Synchronized)
            });
            assert_eq!(completion.intent, SaveIntent::Explicit);
            assert_eq!(completion.expected_disk, ExpectedDiskState::Fingerprint(17));
            assert_eq!(completion.continuation, token);
            let committed = completion.result.unwrap();
            assert!(matches!(
                committed.durability,
                WriteDurability::Synchronized
            ));
            let status = document.record_save(committed.receipt);
            assert_eq!(
                workflow
                    .complete_save(token, status, document.is_dirty(), true)
                    .unwrap(),
                None
            );
            assert!(document.is_dirty());
            assert_eq!(
                document.disk_fingerprint(),
                Some(fingerprint(CANONICAL.as_bytes()))
            );
        }
    }

    #[test]
    fn failures_have_no_receipt_but_uncertain_durability_keeps_committed_bytes() {
        for failed in [false, true] {
            let mut document =
                Document::new(WindowSessionId::new(1), "original", DocumentKind::Text);
            document.edit(0, |source| source.push_str(" edited"));
            let request = document
                .prepare_save("notes.txt".into(), DocumentKind::Text)
                .unwrap();
            let input = SaveInput::new(request, ExpectedDiskState::Missing, SaveIntent::Auto, None);
            let result = input.execute_with(|_| {
                if failed {
                    Err("persist failed".into())
                } else {
                    Ok(WriteDurability::Uncertain("directory sync failed".into()))
                }
            });
            assert_eq!(result.intent, SaveIntent::Auto);
            assert_eq!(result.expected_disk, ExpectedDiskState::Missing);
            assert!(result.continuation.is_none());
            if failed {
                assert_eq!(result.result.unwrap_err(), "persist failed");
                assert!(document.is_dirty());
                assert_eq!(document.saved_source(), "original");
            } else {
                let committed = result.result.unwrap();
                assert!(
                    matches!(committed.durability, WriteDurability::Uncertain(ref error) if error == "directory sync failed")
                );
                document.record_save(committed.receipt).unwrap();
                assert_eq!(document.saved_source(), "original edited");
                assert!(!document.is_dirty());
            }
        }
    }

    #[test]
    fn handoff_does_not_replace_the_documents_receipt_validation() {
        for replace_owner in [false, true] {
            let mut document =
                Document::<usize>::new(WindowSessionId::new(1), "old", DocumentKind::Text);
            let request = document
                .prepare_save("notes.txt".into(), DocumentKind::Text)
                .unwrap();
            let input = SaveInput::new(
                request,
                ExpectedDiskState::Unchecked,
                SaveIntent::ExplicitConfirmed { observed: None },
                None,
            );
            if replace_owner {
                document = Document::new(WindowSessionId::new(2), "new", DocumentKind::Text);
            } else {
                document.replace_unprojected_untitled("new");
            }
            let result = input.execute_with(|_| Ok(WriteDurability::Synchronized));
            assert!(
                document
                    .record_save(result.result.unwrap().receipt)
                    .is_err()
            );
            assert_eq!(document.source(), "new");
        }
    }

    #[test]
    fn requests_and_completions_can_cross_a_worker_boundary() {
        fn send<T: Send>() {}
        send::<SaveInput>();
        send::<SaveCompletion>();
    }
}
