use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::Duration,
};
use tiptoptyp_core::text::LspRange;

use eframe::egui;
use rfd::FileHandle;

use crate::document::DocumentKey;

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

struct EguiFutureWake {
    context: egui::Context,
    viewport: egui::ViewportId,
}

impl Wake for EguiFutureWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.context.request_repaint_of(self.viewport);
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
    let waker = Waker::from(Arc::new(EguiFutureWake {
        context: context.clone(),
        viewport: context.viewport_id(),
    }));
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
    /// When present, an already-cached artifact was known to precede a queued
    /// rebuild. The export must wait for a strictly newer compiler generation
    /// instead of materializing those stale bytes.
    pub(crate) after_artifact_generation: Option<u64>,
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
    DeleteFile {
        message: String,
        path: PathBuf,
    },
    UninstallPackage {
        message: String,
        installation: crate::package_catalog::PackageInstallation,
    },
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
    flow:
        tiptoptyp_core::workflow::Workflow<PendingDocumentAction, AppModal, PendingDocumentDialog>,
    pub(crate) modal_had_focus: bool,
    pub(crate) modal_suspended: bool,
    pub(crate) pending_export: Option<PendingExport>,
    pub(crate) pending_export_dialog: Option<PendingDialog<ExportDialogRequest>>,
    close_permit: Option<DocumentKey>,
    deferred_errors: std::collections::VecDeque<AppModal>,
}

impl DocumentWorkflow {
    pub(crate) fn complete_save(
        &mut self,
        document: &mut crate::document::DocumentSession,
        receipt: tiptoptyp_core::document::SaveReceipt,
        synchronized: bool,
    ) -> Result<Option<PendingDocumentAction>, &'static str> {
        self.flow.complete_save(document, receipt, synchronized)
    }
    pub(crate) fn allow_close_for(&mut self, key: DocumentKey) {
        self.close_permit = Some(key);
    }
    pub(crate) fn revoke_close(&mut self) {
        self.close_permit = None;
    }
    pub(crate) fn may_close(&self, key: DocumentKey) -> bool {
        self.close_permit == Some(key)
    }
    pub(crate) fn take_modal(&mut self) -> Option<AppModal> {
        self.flow.take_prompt()
    }
    pub(crate) fn modal(&self) -> Option<&AppModal> {
        self.flow.prompt()
    }
    pub(crate) fn set_modal(&mut self, modal: AppModal) {
        // An operation already owning a native dialog is not replaced by a
        // second document operation. Its completion remains authoritative.
        let _ = self.flow.show_prompt(modal);
    }
    pub(crate) fn queue_action(&mut self, action: PendingDocumentAction) {
        let _ = self.flow.queue(action);
    }
    pub(crate) fn take_action(&mut self) -> Option<PendingDocumentAction> {
        self.flow.take_action()
    }
    pub(crate) fn continue_after_save(&mut self, action: PendingDocumentAction) {
        let _ = self.flow.continue_after_save(action);
    }
    pub(crate) fn cancel_continuation(&mut self) {
        self.flow.cancel_continuation();
    }
    pub(crate) fn take_continuation(&mut self) -> Option<PendingDocumentAction> {
        self.flow.take_continuation()
    }
    pub(crate) fn has_continuation(&self) -> bool {
        self.flow.has_continuation()
    }
    pub(crate) fn has_dialog(&self) -> bool {
        self.flow.has_dialog()
    }
    pub(crate) fn start_dialog(&mut self, dialog: PendingDocumentDialog) {
        let _ = self.flow.choose(dialog);
    }
    pub(crate) fn finish_dispatch(&mut self) {
        self.flow.finish_dispatch();
        if !self.flow.is_busy()
            && let Some(modal) = self.deferred_errors.pop_front()
        {
            self.set_modal(modal);
        }
    }
    pub(crate) fn poll_document_dialog(
        &mut self,
        context: &egui::Context,
    ) -> DialogPoll<DocumentDialogRequest> {
        let mut dialog = self.flow.take_dialog();
        let result = poll_dialog(&mut dialog, context);
        if let Some(dialog) = dialog {
            self.start_dialog(dialog);
        }
        result
    }
    pub(crate) fn is_busy(&self, rename_active: bool, tool_picker_active: bool) -> bool {
        self.flow.is_busy()
            || rename_active
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
            self.queue_action(pending);
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
        self.set_modal(AppModal::Unsaved {
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
        self.flow.cancel_continuation();
        let modal = AppModal::Alert {
            title: "error".to_owned(),
            message,
            kind: NoticeKind::Error,
        };
        if let Err(modal) = self.flow.show_prompt(modal) {
            self.deferred_errors.push_back(modal);
        }
        self.modal_had_focus = false;
        self.modal_suspended = false;
    }

    pub(crate) fn clear_modal(&mut self) {
        self.flow.clear_prompt();
        self.modal_had_focus = false;
        self.modal_suspended = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: DocumentKey = DocumentKey {
        owner: tiptoptyp_core::document::WindowSessionId::new(1),
        epoch: 3,
        revision: 8,
    };

    #[test]
    fn asynchronous_dialog_wakes_its_own_window_after_focus_changes() {
        let context = egui::Context::default();
        let origin = egui::ViewportId::from_hash_of("dialog-owner");
        let other = egui::ViewportId::from_hash_of("another-editor");
        let saved = Arc::new(std::sync::Mutex::new(None::<Waker>));
        let future_waker = saved.clone();
        let mut pending = Some(PendingDialog::new(
            (),
            std::future::poll_fn(move |context| {
                *future_waker.lock().unwrap() = Some(context.waker().clone());
                Poll::Pending
            }),
        ));
        for viewport in [origin, origin, origin, other, other, other, origin, other] {
            let mut input = egui::RawInput {
                viewport_id: viewport,
                ..Default::default()
            };
            input.viewports.entry(viewport).or_default();
            let mut output = context.run_ui(input, |ui| {
                if viewport == origin {
                    assert!(matches!(
                        poll_dialog(&mut pending, ui.ctx()),
                        DialogPoll::Pending
                    ));
                }
            });
            output.textures_delta.clear();
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        context.set_request_repaint_callback(move |info| {
            sender.send(info.viewport_id).unwrap();
        });
        saved.lock().unwrap().take().unwrap().wake();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            origin
        );
    }

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
        let Some(AppModal::Unsaved { message, pending }) = workflow.modal().cloned() else {
            panic!("expected unsaved modal");
        };
        assert!(message.contains("chapter.typ"));
        assert_eq!(pending.key, KEY);
        assert!(workflow.take_action().is_none());
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
        assert_eq!(workflow.take_action().unwrap().key, KEY);
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
