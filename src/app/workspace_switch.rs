//! Workspace navigation approves every owned draft before retiring any tab.
use super::*;

#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

pub(super) enum Destination {
    Local(PathBuf),
    Overleaf {
        root: PathBuf,
        epoch: u64,
    },
    ConnectOverleaf {
        url: String,
        preference: crate::settings::OverleafStoragePreference,
    },
}

struct Pending {
    origin: PathBuf,
    active: Option<u64>,
    destination: Destination,
    waiting: Option<u64>,
}

impl State {
    pub(super) fn active(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn keeps_save_workspace(&self, tab: u64, workspace: &Path) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.waiting == Some(tab) && pending.origin == workspace)
    }
    pub(super) fn adopting_overleaf(&self, epoch: u64) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            matches!(&pending.destination, Destination::Overleaf { epoch: expected, .. } if *expected == epoch)
        })
    }
}

impl EditorApp {
    pub(super) fn request_workspace_switch(
        &mut self,
        destination: Destination,
        context: &egui::Context,
    ) {
        if self.workspace_switch.active() {
            return;
        }
        self.tabs.approved.clear();
        self.workspace_switch.pending = Some(Pending {
            origin: self.workspace_root.clone(),
            active: self.tabs.active_id(),
            destination,
            waiting: None,
        });
        self.advance_workspace_switch(context);
        context.request_repaint();
    }

    pub(super) fn cancel_workspace_switch(&mut self, context: &egui::Context) {
        let Some(pending) = self.workspace_switch.pending.take() else {
            return;
        };
        if let Destination::Overleaf { epoch, .. } = pending.destination {
            self.cancel_overleaf_workspace_open(epoch);
        }
        self.tabs.approved.clear();
        if self.workspace_root == pending.origin
            && let Some(active) = pending.active
            && self.tabs.index_of(active).is_some()
        {
            self.activate_tab(active, context);
        }
        context.request_repaint();
    }

    pub(super) fn approve_workspace_switch_tab(&mut self) {
        let Some(tab) = self.tabs.active_id() else {
            return;
        };
        let Some(pending) = &mut self.workspace_switch.pending else {
            return;
        };
        if pending.waiting != Some(tab) {
            return;
        }
        self.tabs.approved.push((tab, self.document().key()));
        self.workspace_switch.pending.as_mut().unwrap().waiting = None;
    }

    pub(super) fn advance_workspace_switch(&mut self, context: &egui::Context) {
        let Some(pending) = self.workspace_switch.pending.as_ref() else {
            return;
        };
        if self.document_workflow.save_in_flight()
            || self.other_document_transaction_busy()
            || self.local_save.is_running()
        {
            return;
        }
        // A cancelled chooser, rejected save, or stale continuation returned
        // control without approving its tab. No page has been removed yet.
        if pending.waiting.is_some() || self.workspace_root != pending.origin {
            self.cancel_workspace_switch(context);
            return;
        }
        let closing = self.tabs_owned_by_workspace(&pending.origin);
        let dirty = closing.iter().copied().find(|&id| {
            self.document_for_tab(id).is_some_and(|document| {
                document.is_dirty() && !self.tabs.approved.contains(&(id, document.key()))
            })
        });
        if let Some(tab) = dirty {
            self.activate_tab(tab, context);
            self.workspace_switch.pending.as_mut().unwrap().waiting = Some(tab);
            self.document_workflow.queue_replacement(
                self.document().key(),
                true,
                &self.document().name(),
                DeferredDocumentAction::ApproveWorkspaceSwitch,
                "changing workspace",
            );
            return;
        }
        let valid = match &pending.destination {
            Destination::Local(root) => root.is_dir(),
            Destination::Overleaf { root, epoch } => self.overleaf_workspace_matches(root, *epoch),
            Destination::ConnectOverleaf { .. } => true,
        };
        if !valid {
            self.cancel_workspace_switch(context);
            self.show_file_error("The selected workspace is no longer available.".into());
            return;
        }
        // Capture pages before closing them, and finish the tiny metadata
        // checkpoint before a different project replaces the connection.
        if self.prepare_overleaf_workspace_departure(&closing, context)
            || self.wait_for_overleaf_session(context)
        {
            return;
        }
        let pending = self.workspace_switch.pending.take().unwrap();
        self.depart_overleaf_workspace(
            &pending.origin,
            matches!(pending.destination, Destination::ConnectOverleaf { .. }),
        );
        self.close_workspace_tabs(&closing, context);
        self.tabs.approved.clear();
        self.prune_overleaf_bindings();
        match pending.destination {
            Destination::Local(root) => self.commit_local_workspace(root, context),
            Destination::Overleaf { root, .. } => {
                self.workspace_root = root;
                self.resume_overleaf_workspace_open(context);
            }
            Destination::ConnectOverleaf { url, preference } => {
                self.resume_workspace_overleaf_connection(&url, preference, context);
            }
        }
        context.request_repaint();
    }
}
