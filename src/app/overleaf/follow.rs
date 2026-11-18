//! Explicit collaborator navigation; ordinary presence remains paint-only.
use super::*;
use crate::app::overleaf_presence::BadgeAction;

#[derive(Default)]
pub(super) struct CursorNavigation {
    pub(super) target: Option<Target>,
    epoch: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationMode {
    Seek,
    Follow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CameraCue {
    Pending,
    Applied,
}
enum Position {
    Waiting,
    Opening(String),
    At {
        key: DocumentKey,
        head: usize,
        cue: CameraCue,
    },
}
pub(super) struct Target {
    pub(super) peer: String,
    mode: NavigationMode,
    pub(super) owner: Option<u64>,
    position: Position,
}
impl Target {
    pub(super) fn following(&self) -> bool {
        self.mode == NavigationMode::Follow
    }
    pub(super) fn opening(&self) -> Option<&str> {
        match &self.position {
            Position::Opening(id) => Some(id),
            _ => None,
        }
    }
    pub(super) fn last(&self) -> Option<(DocumentKey, usize)> {
        match self.position {
            Position::At { key, head, .. } => Some((key, head)),
            _ => None,
        }
    }
    pub(super) fn start_open(&mut self, id: String) {
        self.position = Position::Opening(id);
    }
    fn wait(&mut self) {
        self.position = Position::Waiting;
    }
}
impl CursorNavigation {
    pub(super) fn following_peer(&self) -> Option<&str> {
        self.target
            .as_ref()
            .filter(|target| target.following())
            .map(|target| target.peer.as_str())
    }
    pub(super) fn reveal(&self) -> Option<usize> {
        match self.target.as_ref()?.position {
            Position::At {
                head,
                cue: CameraCue::Pending,
                ..
            } => Some(head),
            _ => None,
        }
    }
    pub(super) fn clear_reveal(&mut self) {
        if let Some(Target {
            position: Position::At { cue, .. },
            ..
        }) = &mut self.target
        {
            *cue = CameraCue::Applied;
        }
    }
    pub(super) fn take_reveal(&mut self) -> Option<usize> {
        let head = self.reveal()?;
        self.clear_reveal();
        Some(head)
    }
}
impl EditorApp {
    pub(in crate::app) fn overleaf_following_peer(&self) -> Option<&str> {
        self.overleaf.cursor_navigation.following_peer()
    }
    pub(in crate::app) fn stop_overleaf_cursor(&mut self) {
        if let Some(opening) = self
            .overleaf
            .cursor_navigation
            .target
            .as_ref()
            .and_then(|t| t.opening())
            && self.overleaf.open_after.as_deref() == Some(opening)
        {
            self.overleaf.open_after = None;
            self.overleaf.navigation = None;
        }
        self.overleaf.cursor_navigation = CursorNavigation::default();
    }
    fn suspend_overleaf_cursor(&mut self) {
        if let Some(target) = &mut self.overleaf.cursor_navigation.target
            && !matches!(target.position, Position::Opening(_))
        {
            target.wait();
        }
    }
    /// A save may finish after an already-requested cursor open reaches disk.
    /// Discard that exact navigation receipt and retry through a fresh Open
    /// after the save, retaining the followed peer but never a stale viewport.
    pub(super) fn defer_overleaf_cursor_open(&mut self, id: &str) {
        if !self
            .overleaf
            .cursor_navigation
            .target
            .as_ref()
            .is_some_and(|target| target.opening() == Some(id))
        {
            return;
        }
        if self.overleaf.open_after.as_deref() != Some(id) {
            // A newer manual navigation keeps ownership of its own request.
            self.stop_overleaf_cursor();
            return;
        }
        self.overleaf.open_after = None;
        self.overleaf.navigation = None;
        self.overleaf
            .cursor_navigation
            .target
            .as_mut()
            .unwrap()
            .wait();
        self.suspend_overleaf_cursor();
    }
    pub(in crate::app) fn overleaf_presence_action(
        &mut self,
        action: BadgeAction,
        context: &egui::Context,
    ) {
        let (peer, following) = match action {
            BadgeAction::StopFollowing => {
                self.stop_overleaf_cursor();
                context.request_repaint();
                return;
            }
            BadgeAction::Seek(peer) => {
                // A single click recenters an existing follow of the same
                // collaborator; the explicit triple-click gesture stops it.
                let following = self.overleaf_following_peer() == Some(peer.as_str());
                (peer, following)
            }
            BadgeAction::Follow(peer) => (peer, true),
        };
        // A table draft (or another source transaction) owns the document
        // until it is applied/cancelled. Presence must never bypass that owner.
        if self.document_transaction_busy() {
            self.stop_overleaf_cursor();
            return;
        }
        if !self.overleaf_collaborators().iter().any(|p| p.id == peer) {
            return;
        }
        self.stop_overleaf_cursor();
        self.overleaf.cursor_navigation = CursorNavigation {
            target: Some(Target {
                peer,
                mode: if following {
                    NavigationMode::Follow
                } else {
                    NavigationMode::Seek
                },
                owner: self.tabs.active_id(),
                position: Position::Waiting,
            }),
            epoch: self.overleaf.epoch,
        };
        if self.view_mode == ViewMode::Preview {
            self.view_mode = ViewMode::Split;
        }
        self.advance_overleaf_cursor(context);
        context.request_repaint();
    }
    /// One pass after inbox/mirror processing. No polling loop or source scans
    /// are added to idle frames; revision-owned presence reuses its index.
    pub(in crate::app) fn advance_overleaf_cursor(&mut self, context: &egui::Context) {
        let Some(target) = self.overleaf.cursor_navigation.target.as_ref() else {
            return;
        };
        let editor_id = source_editor_id(context);
        let focused = context.memory(|m| m.focused());
        // An asynchronous file switch can leave no focused widget. Root
        // navigation still ends following; another input keeps its ownership.
        let source_controls_input = focused.is_none() || focused == Some(editor_id);
        let cancel_input = context.input(|input| {
            input.key_pressed(egui::Key::Escape)
                || (source_controls_input
                    && input.events.iter().any(|event| match event {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            // Global commands (Save, Copy, Compile, screenshots)
                            // do not navigate or edit the source camera.
                            matches!(
                                key,
                                egui::Key::ArrowUp
                                    | egui::Key::ArrowDown
                                    | egui::Key::ArrowLeft
                                    | egui::Key::ArrowRight
                                    | egui::Key::Home
                                    | egui::Key::End
                                    | egui::Key::PageUp
                                    | egui::Key::PageDown
                                    | egui::Key::Enter
                                    | egui::Key::Tab
                                    | egui::Key::Backspace
                                    | egui::Key::Delete
                            ) || (modifiers.command && *key == egui::Key::A)
                        }
                        egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Cut
                        | egui::Event::Ime(_) => true,
                        _ => false,
                    }))
        });
        if cancel_input
            || self.document_transaction_busy()
            || self.overleaf.cursor_navigation.epoch != self.overleaf.epoch
            || target.owner != self.tabs.active_id()
            || self.pending_editor_selection.is_some()
        {
            self.stop_overleaf_cursor();
            return;
        }
        let Some(connection) = self.overleaf.connection.as_ref().filter(|c| c.online()) else {
            self.stop_overleaf_cursor();
            return;
        };
        let peers = connection.model.presence().collaborators();
        let Some(peer) = peers.iter().find(|p| p.id == target.peer) else {
            self.stop_overleaf_cursor();
            return;
        };
        if self.document_workflow.save_in_flight() {
            self.suspend_overleaf_cursor();
            return;
        }
        if connection.hydration.pending() {
            return;
        }
        // A collaborator can return to the current file before an earlier
        // asynchronous open finishes. Retire that owned request immediately;
        // its late mirror receipt must not pull us into the abandoned file.
        let obsolete = target
            .opening()
            .filter(|opening| Some(*opening) != peer.doc_id.as_deref())
            .map(str::to_owned);
        if let Some(opening) = obsolete {
            if self.overleaf.open_after.as_ref() == Some(&opening) {
                self.overleaf.open_after = None;
                self.overleaf.navigation = None;
            }
            let target = self.overleaf.cursor_navigation.target.as_mut().unwrap();
            target.wait();
        }
        let Some(id) = peer.doc_id.as_deref() else {
            self.overleaf.cursor_navigation.clear_reveal();
            return;
        };
        let active_id = self
            .tabs
            .active_id()
            .and_then(|tab| self.overleaf_entry_for_tab(tab))
            .map(|entry| entry.id.as_str());
        if active_id != Some(id) {
            if self
                .overleaf
                .cursor_navigation
                .target
                .as_ref()
                .is_some_and(|target| target.opening() == Some(id))
            {
                // A manual navigation may have replaced/cancelled this open.
                if self.overleaf.open_after.as_deref() != Some(id) {
                    self.stop_overleaf_cursor();
                }
                return;
            }
            let Some(entry) = connection
                .model
                .tree()
                .and_then(|t| t.entry(id))
                .filter(|e| e.kind == EntryKind::Doc)
            else {
                return;
            };
            let path = connection.root.join(&entry.path);
            let id = id.to_owned();
            self.overleaf.cursor_navigation.clear_reveal();
            if self.open_overleaf_path(&path) && self.overleaf.open_after.as_deref() == Some(&id) {
                self.overleaf
                    .cursor_navigation
                    .target
                    .as_mut()
                    .unwrap()
                    .start_open(id);
            } else {
                self.stop_overleaf_cursor();
            }
            return;
        }
        if self.document().is_dirty() {
            self.stop_overleaf_cursor();
            self.notice = Some(Notice {
                message: "Save this draft before seeking a collaborator.".into(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let cursors = self.overleaf_presence_for_active();
        let Some(cursor) = cursors.iter().find(|c| c.id == peer.id) else {
            self.overleaf.cursor_navigation.clear_reveal();
            self.overleaf
                .cursor_navigation
                .target
                .as_mut()
                .unwrap()
                .wait();
            return;
        };
        let position = (self.document().key(), cursor.head);
        let target = self.overleaf.cursor_navigation.target.as_mut().unwrap();
        if target.last() == Some(position) {
            return;
        }
        target.position = Position::At {
            key: position.0,
            head: position.1,
            cue: CameraCue::Pending,
        };
        if !target.following() {
            self.pending_editor_selection = Some(EditorSelection::Focus(cursor.head..cursor.head));
            self.editor_attention = Some(EditorAttention::arm(cursor.head));
            self.stop_overleaf_cursor();
        }
    }
    /// Called only after the normal fresh-server open has succeeded.
    pub(super) fn finish_overleaf_cursor_open(&mut self, id: &str) {
        if let Some(target) = &mut self.overleaf.cursor_navigation.target
            && target.opening() == Some(id)
        {
            target.wait();
            target.owner = self.tabs.active_id();
        }
    }
    /// User input wins over a queued camera movement. This does not consume any
    /// input, change the selection, or take focus away from another control.
    pub(in crate::app) fn take_overleaf_reveal(&mut self, ui: &egui::Ui) -> Option<usize> {
        self.overleaf_following_peer()?;
        if self.document_transaction_busy()
            || self.overleaf.cursor_navigation.epoch != self.overleaf.epoch
            || self
                .overleaf
                .cursor_navigation
                .target
                .as_ref()
                .is_some_and(|t| t.owner != self.tabs.active_id())
            || self
                .overleaf
                .connection
                .as_ref()
                .is_none_or(|c| !c.online())
        {
            self.stop_overleaf_cursor();
            return None;
        }
        let bounds = ui.available_rect_before_wrap();
        let cancel_pointer = ui.input(|i| {
            i.pointer.interact_pos().is_some_and(|p| bounds.contains(p))
                && (i.pointer.any_pressed()
                    || i.events
                        .iter()
                        .any(|event| matches!(event, egui::Event::MouseWheel { .. })))
        });
        if cancel_pointer || self.pending_editor_selection.is_some() {
            self.stop_overleaf_cursor();
            return None;
        }
        if self.document_workflow.save_in_flight() {
            self.suspend_overleaf_cursor();
            return None;
        }
        let at = self.overleaf.cursor_navigation.take_reveal()?;
        self.overleaf
            .cursor_navigation
            .target
            .as_ref()
            .is_some_and(|target| target.last() == Some((self.document().key(), at)))
            .then_some(at)
    }
}

#[cfg(test)]
mod phase_tests {
    use super::*;
    #[test]
    fn camera_cue_is_consumed_once_and_cannot_survive_opening_another_document() {
        let key = tiptoptyp_core::document::DocumentKey::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            2,
            3,
        );
        let mut navigation = CursorNavigation {
            target: Some(Target {
                peer: "peer".into(),
                mode: NavigationMode::Follow,
                owner: Some(1),
                position: Position::At {
                    key,
                    head: 7,
                    cue: CameraCue::Pending,
                },
            }),
            epoch: 4,
        };
        assert_eq!(navigation.take_reveal(), Some(7));
        assert_eq!(navigation.take_reveal(), None);
        navigation
            .target
            .as_mut()
            .unwrap()
            .start_open("other".into());
        assert_eq!(navigation.target.as_ref().unwrap().opening(), Some("other"));
        assert_eq!(navigation.target.as_ref().unwrap().last(), None);
        assert_eq!(navigation.reveal(), None);
        assert_eq!(navigation.following_peer(), Some("peer"));
    }
}
