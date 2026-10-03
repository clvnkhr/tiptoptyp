//! Tracks confirmed user-visible PDF writes, separately from preview caches.
//! This is an in-memory receipt; observing it never stats or reads the output.
use std::path::{Path, PathBuf};

use crate::{compiler::ArtifactKey, document::DocumentKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PdfOutputStatus {
    /// No successful PDF write has been observed for this document session.
    Unverified,
    Saved,
    Stale,
    Building,
    Failed,
}

#[derive(Debug)]
struct SavedPdf {
    source: DocumentKey,
    artifact: ArtifactKey,
    path: PathBuf,
}

#[derive(Debug)]
struct FailedWrite {
    source: DocumentKey,
    error: String,
}

#[derive(Debug, Default)]
pub(super) struct PdfOutput {
    saved: Option<SavedPdf>,
    invalidated: bool,
    failure: Option<FailedWrite>,
}

fn same_document(left: DocumentKey, right: DocumentKey) -> bool {
    left.owner == right.owner && left.epoch == right.epoch
}

impl PdfOutput {
    /// Call only after a successful, durable write to the chosen destination.
    /// An artifact produced for the preview alone is not a saved PDF receipt.
    pub(super) fn saved(&mut self, source: DocumentKey, artifact: ArtifactKey, path: PathBuf) {
        self.saved = Some(SavedPdf {
            source,
            artifact,
            path,
        });
        self.invalidated = false;
        self.failure = None;
    }

    /// Source edits, dependency changes, and build-configuration changes can
    /// invalidate a PDF even before a new preview artifact has arrived.
    pub(super) fn invalidate(&mut self) {
        self.invalidated = true;
    }

    /// Tab activation rebases service identities without changing its source.
    /// Preserve a current receipt through that rebase and preserve staleness
    /// when the buffer already differed from the last saved PDF.
    pub(super) fn rekey(&mut self, old: DocumentKey, new: DocumentKey) {
        if let Some(saved) = self
            .saved
            .as_mut()
            .filter(|saved| same_document(saved.source, old))
        {
            self.invalidated |= saved.source != old || saved.artifact.revision != old.revision;
            saved.source = new;
            saved.artifact.revision = new.revision;
        }
        if let Some(failure) = self
            .failure
            .as_mut()
            .filter(|failure| failure.source == old)
        {
            failure.source = new;
        }
    }

    /// Preserve the previous successful destination while reporting a failed
    /// attempt. The source identity prevents errors leaking across replacements.
    pub(super) fn failed(&mut self, source: DocumentKey, error: String) {
        self.failure = Some(FailedWrite { source, error });
    }

    pub(super) fn status(
        &self,
        source: DocumentKey,
        current_artifact: Option<ArtifactKey>,
        pending_write: bool,
    ) -> PdfOutputStatus {
        if pending_write {
            return PdfOutputStatus::Building;
        }
        if self.error(source).is_some() {
            return PdfOutputStatus::Failed;
        }
        let Some(saved) = self
            .saved
            .as_ref()
            .filter(|saved| same_document(saved.source, source))
        else {
            return PdfOutputStatus::Unverified;
        };
        if self.invalidated
            || saved.source != source
            || saved.artifact.revision != source.revision
            || current_artifact.is_some_and(|artifact| {
                artifact.revision == source.revision
                    && artifact.generation != saved.artifact.generation
            })
        {
            PdfOutputStatus::Stale
        } else {
            PdfOutputStatus::Saved
        }
    }

    /// Remains available for stale output and a failed rewrite, but never for
    /// another document owner or replacement epoch.
    pub(super) fn path(&self, source: DocumentKey) -> Option<&Path> {
        self.saved
            .as_ref()
            .filter(|saved| same_document(saved.source, source))
            .map(|saved| saved.path.as_path())
    }

    pub(super) fn error(&self, source: DocumentKey) -> Option<&str> {
        self.failure
            .as_ref()
            .filter(|failure| failure.source == source)
            .map(|failure| failure.error.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiptoptyp_core::document::WindowSessionId;

    fn source() -> DocumentKey {
        DocumentKey::new(WindowSessionId::new(1), 2, 3)
    }

    fn artifact() -> ArtifactKey {
        ArtifactKey {
            revision: 3,
            generation: 4,
        }
    }

    fn saved_output() -> PdfOutput {
        let mut output = PdfOutput::default();
        output.saved(source(), artifact(), PathBuf::from("/project/paper.pdf"));
        output
    }

    #[test]
    fn cached_preview_alone_does_not_verify_a_saved_pdf() {
        let output = PdfOutput::default();
        assert_eq!(
            output.status(source(), Some(artifact()), false),
            PdfOutputStatus::Unverified
        );
        assert!(output.path(source()).is_none());
    }

    #[test]
    fn successful_write_can_remain_current_without_a_pdf_preview_cache() {
        let output = saved_output();
        assert_eq!(output.status(source(), None, false), PdfOutputStatus::Saved);
        assert_eq!(
            output.status(source(), Some(artifact()), false),
            PdfOutputStatus::Saved
        );
        assert_eq!(output.path(source()), Some(Path::new("/project/paper.pdf")));
    }

    #[test]
    fn source_edits_and_dependency_rebuilds_make_previous_output_stale() {
        let output = saved_output();
        assert_eq!(
            output.status(source().after_edit(), None, false),
            PdfOutputStatus::Stale
        );
        assert_eq!(
            output.status(
                source(),
                Some(ArtifactKey {
                    generation: 5,
                    ..artifact()
                }),
                false,
            ),
            PdfOutputStatus::Stale
        );
        assert!(output.path(source().after_edit()).is_some());
    }

    #[test]
    fn explicit_invalidation_survives_old_cache_until_a_successful_write() {
        let mut output = saved_output();
        output.invalidate();
        assert_eq!(
            output.status(source(), Some(artifact()), false),
            PdfOutputStatus::Stale
        );
        assert_eq!(
            output.status(source(), Some(artifact()), true),
            PdfOutputStatus::Building
        );
        output.saved(source(), artifact(), PathBuf::from("/project/new.pdf"));
        assert_eq!(output.status(source(), None, false), PdfOutputStatus::Saved);
        assert_eq!(output.path(source()), Some(Path::new("/project/new.pdf")));
    }

    #[test]
    fn replacement_and_other_owner_never_inherit_output_state() {
        let mut output = saved_output();
        output.failed(source(), "Permission denied".into());
        for replacement in [
            DocumentKey {
                epoch: 8,
                ..source()
            },
            DocumentKey {
                owner: WindowSessionId::new(9),
                ..source()
            },
        ] {
            assert_eq!(
                output.status(replacement, Some(artifact()), false),
                PdfOutputStatus::Unverified
            );
            assert!(output.path(replacement).is_none());
            assert!(output.error(replacement).is_none());
        }
    }

    #[test]
    fn failed_rewrite_keeps_the_previous_destination_and_retry_takes_precedence() {
        let mut output = saved_output();
        output.failed(source(), "Disk full".into());
        assert_eq!(
            output.status(source(), None, false),
            PdfOutputStatus::Failed
        );
        assert_eq!(output.error(source()), Some("Disk full"));
        assert_eq!(output.path(source()), Some(Path::new("/project/paper.pdf")));
        output.invalidate();
        assert_eq!(
            output.status(source(), None, false),
            PdfOutputStatus::Failed,
            "an unrelated preview refresh cannot erase a failed disk write"
        );
        assert_eq!(
            output.status(source(), None, true),
            PdfOutputStatus::Building
        );
        output.saved(source(), artifact(), PathBuf::from("/project/paper.pdf"));
        assert_eq!(output.status(source(), None, false), PdfOutputStatus::Saved);
        assert!(output.error(source()).is_none());
    }

    #[test]
    fn failed_first_write_is_scoped_and_editing_allows_a_fresh_attempt() {
        let mut output = PdfOutput::default();
        output.failed(source(), "No write access".into());
        assert_eq!(
            output.status(source(), None, false),
            PdfOutputStatus::Failed
        );
        assert!(output.path(source()).is_none());
        assert_eq!(
            output.status(
                DocumentKey {
                    epoch: 8,
                    ..source()
                },
                None,
                false
            ),
            PdfOutputStatus::Unverified
        );
        output.invalidate();
        assert_eq!(
            output.status(source(), None, false),
            PdfOutputStatus::Failed
        );
        assert_eq!(
            output.status(source().after_edit(), None, false),
            PdfOutputStatus::Unverified
        );
        assert!(output.error(source().after_edit()).is_none());
    }

    #[test]
    fn unrelated_tabs_have_independent_output_receipts() {
        let first = saved_output();
        let second = PdfOutput::default();
        assert_eq!(first.status(source(), None, false), PdfOutputStatus::Saved);
        assert_eq!(
            second.status(source(), Some(artifact()), false),
            PdfOutputStatus::Unverified
        );
    }

    #[test]
    fn a_mismatched_artifact_revision_cannot_claim_fresh_output() {
        let mut output = PdfOutput::default();
        output.saved(
            source().after_edit(),
            artifact(),
            PathBuf::from("/project/paper.pdf"),
        );
        assert_eq!(
            output.status(source().after_edit(), None, false),
            PdfOutputStatus::Stale
        );
    }

    #[test]
    fn pure_tab_activation_preserves_receipt_and_ignores_superseded_cache() {
        let mut output = saved_output();
        let reactivated = DocumentKey {
            epoch: 9,
            revision: 10,
            ..source()
        };
        output.rekey(source(), reactivated);
        assert_eq!(
            output.status(reactivated, Some(artifact()), false),
            PdfOutputStatus::Saved,
            "retained preview cache has an old service revision, not a source edit"
        );
        assert!(output.path(reactivated).is_some());
        assert_eq!(
            output.status(
                reactivated,
                Some(ArtifactKey {
                    revision: reactivated.revision,
                    generation: 5,
                }),
                false,
            ),
            PdfOutputStatus::Stale,
            "a new artifact for the current identity can reveal dependency changes"
        );
    }

    #[test]
    fn activation_does_not_make_an_edited_or_invalidated_pdf_current() {
        let edited = source().after_edit();
        let reactivated = DocumentKey {
            epoch: 9,
            revision: 10,
            ..edited
        };
        let mut output = saved_output();
        output.rekey(edited, reactivated);
        assert_eq!(
            output.status(reactivated, None, false),
            PdfOutputStatus::Stale
        );
        let mut output = saved_output();
        output.invalidate();
        output.rekey(source(), reactivated);
        assert_eq!(
            output.status(reactivated, None, false),
            PdfOutputStatus::Stale
        );
    }

    #[test]
    fn activation_preserves_current_write_failure_but_new_edit_does_not() {
        let mut output = saved_output();
        output.failed(source(), "Disk full".into());
        let reactivated = DocumentKey {
            epoch: 9,
            revision: 10,
            ..source()
        };
        output.rekey(source(), reactivated);
        assert_eq!(
            output.status(reactivated, None, false),
            PdfOutputStatus::Failed
        );
        assert_eq!(output.error(reactivated), Some("Disk full"));
        assert_eq!(
            output.status(reactivated.after_edit(), None, false),
            PdfOutputStatus::Stale
        );
        assert!(output.error(reactivated.after_edit()).is_none());
    }
}
