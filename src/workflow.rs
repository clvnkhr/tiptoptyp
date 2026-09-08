use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::Duration,
};

use eframe::egui;
use rfd::FileHandle;

use crate::{document::DocumentKey, tinymist::LspRange};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentDialogTarget {
    OpenFile,
    OpenFileInNewWindow,
    OpenFolder,
    SaveAs { typst: bool },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DocumentDialogRequest {
    pub(crate) target: DocumentDialogTarget,
    pub(crate) key: DocumentKey,
}

pub(crate) type PendingDocumentDialog = PendingDialog<DocumentDialogRequest>;

pub(crate) struct PendingDialog<T> {
    request: T,
    future: Pin<Box<dyn Future<Output = Option<FileHandle>>>>,
}

impl<T> PendingDialog<T> {
    pub(crate) fn new(
        request: T,
        future: impl Future<Output = Option<FileHandle>> + 'static,
    ) -> Self {
        Self {
            request,
            future: Box::pin(future),
        }
    }
}

pub(crate) enum DialogPoll<T> {
    Idle,
    Pending,
    Ready {
        request: T,
        selection: Option<FileHandle>,
    },
}

struct EguiFutureWake(egui::Context);

impl Wake for EguiFutureWake {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}

/// Polls one native async file dialog and consumes it exactly once on
/// completion. All document, export, and presentation pickers share this
/// waker/repaint behavior rather than maintaining separate polling loops.
pub(crate) fn poll_dialog<T>(
    pending: &mut Option<PendingDialog<T>>,
    context: &egui::Context,
) -> DialogPoll<T> {
    let Some(dialog) = pending.as_mut() else {
        return DialogPoll::Idle;
    };
    let waker = Waker::from(Arc::new(EguiFutureWake(context.clone())));
    let mut task_context = TaskContext::from_waker(&waker);
    let Poll::Ready(selection) = dialog.future.as_mut().poll(&mut task_context) else {
        context.request_repaint_after(Duration::from_millis(50));
        return DialogPoll::Pending;
    };
    let dialog = pending.take().expect("completed dialog remains installed");
    DialogPoll::Ready {
        request: dialog.request,
        selection,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfWriteIntent {
    Compile,
    Export,
}

impl PdfWriteIntent {
    pub(crate) const fn dialog_title(self) -> &'static str {
        match self {
            Self::Compile => "Compile PDF",
            Self::Export => "Export PDF",
        }
    }

    pub(crate) const fn completed_verb(self) -> &'static str {
        match self {
            Self::Compile => "Compiled",
            Self::Export => "Exported",
        }
    }

    pub(crate) const fn queued_message(self) -> &'static str {
        match self {
            Self::Compile => "Compile queued for the next successful build",
            Self::Export => "Export queued for the next successful build",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingExport {
    pub(crate) path: PathBuf,
    pub(crate) document_epoch: u64,
    pub(crate) intent: PdfWriteIntent,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExportDialogRequest {
    pub(crate) document_epoch: u64,
    pub(crate) intent: PdfWriteIntent,
}

#[derive(Debug, Clone)]
pub(crate) enum DeferredDocumentAction {
    New,
    OpenFileDialog,
    OpenFolderDialog,
    CloseWindow,
    LoadPath(PathBuf),
    OpenFolder(PathBuf),
    FollowFileLink {
        path: PathBuf,
        page: Option<usize>,
        source_position: Option<(usize, usize)>,
    },
    FollowTinymistLocation {
        path: PathBuf,
        selection: Option<LspRange>,
    },
    SaveThen(Box<PendingDocumentAction>),
    ForceSave {
        path: PathBuf,
        key: DocumentKey,
        expected_disk_fingerprint: Option<u64>,
        observed_disk_fingerprint: Option<u64>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PendingDocumentAction {
    pub(crate) action: DeferredDocumentAction,
    pub(crate) key: DocumentKey,
    pub(crate) allow_discard: bool,
    pub(crate) description: String,
}

#[derive(Debug, Clone)]
pub(crate) enum AppModal {
    Alert {
        title: String,
        message: String,
        kind: NoticeKind,
    },
    Unsaved {
        message: String,
        pending: PendingDocumentAction,
    },
    Overwrite {
        message: String,
        path: PathBuf,
        key: DocumentKey,
        expected_disk_fingerprint: Option<u64>,
        observed_disk_fingerprint: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppModalChoice {
    Primary,
    Secondary,
    Cancel,
}

/// Owns document replacement, save continuation, modal, and file-dialog
/// lifecycles. Every asynchronous action carries a `DocumentKey`, preventing
/// it from applying to a replacement document or a newer revision.
#[derive(Default)]
pub(crate) struct DocumentWorkflow {
    pub(crate) modal: Option<AppModal>,
    pub(crate) modal_had_focus: bool,
    pub(crate) modal_suspended: bool,
    pub(crate) pending_action: Option<PendingDocumentAction>,
    pub(crate) post_save_action: Option<PendingDocumentAction>,
    pub(crate) pending_dialog: Option<PendingDocumentDialog>,
    pub(crate) pending_export: Option<PendingExport>,
    pub(crate) pending_export_dialog: Option<PendingDialog<ExportDialogRequest>>,
    pub(crate) allow_close: bool,
}

impl DocumentWorkflow {
    pub(crate) fn is_busy(&self, rename_active: bool, tool_picker_active: bool) -> bool {
        self.modal.is_some()
            || rename_active
            || self.pending_action.is_some()
            || self.post_save_action.is_some()
            || self.pending_dialog.is_some()
            || self.pending_export_dialog.is_some()
            || tool_picker_active
    }

    pub(crate) fn queue_replacement(
        &mut self,
        key: DocumentKey,
        dirty: bool,
        document_name: &str,
        action: DeferredDocumentAction,
        description: &str,
    ) -> bool {
        if self.is_busy(false, false) {
            return false;
        }
        let pending = PendingDocumentAction {
            action,
            key,
            allow_discard: false,
            description: description.to_owned(),
        };
        if dirty {
            self.present_unsaved(key, document_name, pending);
        } else {
            self.pending_action = Some(pending);
        }
        true
    }

    pub(crate) fn present_unsaved(
        &mut self,
        key: DocumentKey,
        document_name: &str,
        mut pending: PendingDocumentAction,
    ) {
        pending.key = key;
        pending.allow_discard = false;
        self.modal = Some(AppModal::Unsaved {
            message: format!(
                "Save changes to {document_name} before {}?",
                pending.description
            ),
            pending,
        });
        self.modal_had_focus = false;
        self.modal_suspended = false;
    }

    pub(crate) fn present_error(&mut self, message: String) {
        self.modal = Some(AppModal::Alert {
            title: "error".to_owned(),
            message,
            kind: NoticeKind::Error,
        });
        self.modal_had_focus = false;
        self.modal_suspended = false;
    }

    pub(crate) fn clear_modal(&mut self) {
        self.modal = None;
        self.modal_had_focus = false;
        self.modal_suspended = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: DocumentKey = DocumentKey {
        epoch: 3,
        revision: 8,
    };

    #[test]
    fn dirty_replacement_becomes_a_keyed_unsaved_prompt() {
        let mut workflow = DocumentWorkflow::default();
        assert!(workflow.queue_replacement(
            KEY,
            true,
            "chapter.typ",
            DeferredDocumentAction::New,
            "creating a new document",
        ));
        let Some(AppModal::Unsaved { message, pending }) = workflow.modal else {
            panic!("expected unsaved modal");
        };
        assert!(message.contains("chapter.typ"));
        assert_eq!(pending.key, KEY);
        assert!(workflow.pending_action.is_none());
    }

    #[test]
    fn clean_replacement_is_queued_once() {
        let mut workflow = DocumentWorkflow::default();
        assert!(workflow.queue_replacement(
            KEY,
            false,
            "chapter.typ",
            DeferredDocumentAction::CloseWindow,
            "closing tiptoptyp",
        ));
        assert!(!workflow.queue_replacement(
            KEY,
            false,
            "chapter.typ",
            DeferredDocumentAction::New,
            "creating a new document",
        ));
        assert_eq!(workflow.pending_action.unwrap().key, KEY);
    }

    #[test]
    fn dialog_poller_consumes_ready_future_once() {
        let context = egui::Context::default();
        let mut dialog = Some(PendingDialog::new(17, std::future::ready(None)));
        match poll_dialog(&mut dialog, &context) {
            DialogPoll::Ready { request, selection } => {
                assert_eq!(request, 17);
                assert!(selection.is_none());
            }
            DialogPoll::Idle | DialogPoll::Pending => panic!("ready dialog was not completed"),
        }
        assert!(dialog.is_none());
        assert!(matches!(
            poll_dialog(&mut dialog, &context),
            DialogPoll::Idle
        ));
    }
}
