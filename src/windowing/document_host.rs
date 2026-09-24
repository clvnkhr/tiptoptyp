//! Deferred callbacks address UI-thread-owned documents by a non-reused token.
//!
//! EditorApp contains native webviews/handles and must not become Send. eframe
//! invokes viewport callbacks on its event-loop thread. Only a token crosses
//! egui's Send + Sync callback boundary; no pointer or unsafe Send implementation
//! does. Closing a host removes its weak registration, making late callbacks inert.
use std::{
    cell::{Ref, RefCell, RefMut},
    collections::HashMap,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, ThreadId},
};

use crate::app::EditorApp;
use eframe::egui;

thread_local! {
    static DOCUMENTS: RefCell<HashMap<u64, Weak<RefCell<DocumentState>>>> = RefCell::default();
}
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(super) struct DocumentHost {
    editor: Rc<RefCell<DocumentState>>,
    token: DocumentToken,
}

struct DocumentState {
    editor: EditorApp,
    focused: bool,
    closed: bool,
    settings: Option<crate::settings::AppSettings>,
    #[cfg(test)]
    paints: usize,
}

#[derive(Clone, Copy)]
pub(super) struct DocumentToken {
    id: u64,
    thread: ThreadId,
}

impl DocumentHost {
    pub(super) fn new(editor: EditorApp) -> Self {
        let editor = Rc::new(RefCell::new(DocumentState {
            editor,
            focused: false,
            closed: false,
            settings: None,
            #[cfg(test)]
            paints: 0,
        }));
        let token = DocumentToken {
            id: NEXT_TOKEN.fetch_add(1, Ordering::Relaxed),
            thread: thread::current().id(),
        };
        DOCUMENTS.with_borrow_mut(|documents| {
            documents.insert(token.id, Rc::downgrade(&editor));
        });
        Self { editor, token }
    }

    pub(super) fn borrow(&self) -> Ref<'_, EditorApp> {
        Ref::map(self.editor.borrow(), |state| &state.editor)
    }

    pub(super) fn borrow_mut(&self) -> RefMut<'_, EditorApp> {
        RefMut::map(self.editor.borrow_mut(), |state| &mut state.editor)
    }

    pub(super) fn token(&self) -> DocumentToken {
        self.token
    }

    pub(super) fn closed(&self) -> bool {
        self.editor.borrow().closed
    }

    pub(super) fn queue_settings(&self, settings: crate::settings::AppSettings) {
        self.editor.borrow_mut().settings = Some(settings);
    }

    pub(super) fn apply_settings(&self, context: &egui::Context) {
        self.editor.borrow_mut().apply_settings(context);
    }

    pub(super) fn forget_workspaces(&self, roots: &[std::path::PathBuf]) {
        let mut state = self.editor.borrow_mut();
        state.editor.apply_workspace_history_removals(roots);
        if let Some(settings) = &mut state.settings {
            for root in roots {
                settings.forget_workspace(root);
            }
        }
    }
}

impl DocumentState {
    fn apply_settings(&mut self, context: &egui::Context) {
        if let Some(settings) = self.settings.take() {
            self.editor.apply_shared_settings(settings, context);
        }
    }
}

impl Drop for DocumentHost {
    fn drop(&mut self) {
        DOCUMENTS.with_borrow_mut(|documents| {
            documents.remove(&self.token.id);
        });
    }
}

impl DocumentToken {
    fn with_state<R>(self, f: impl FnOnce(&mut DocumentState) -> R) -> Option<R> {
        assert_eq!(
            self.thread,
            thread::current().id(),
            "document UI must stay on its event-loop thread"
        );
        let editor =
            DOCUMENTS.with_borrow(|documents| documents.get(&self.id).and_then(Weak::upgrade))?;
        Some(f(&mut editor.borrow_mut()))
    }

    pub(super) fn paint(self, ui: &mut egui::Ui) {
        crate::performance::secondary_repaints(ui.ctx());
        self.with_state(|state| {
            if state.closed {
                return;
            }
            #[cfg(test)]
            {
                state.paints += 1;
            }
            state.apply_settings(ui.ctx());
            let focused = crate::app::owns_focused_input_viewport(ui.ctx());
            let focus_changed = state.focused != focused;
            state.focused = focused;
            let editor = &mut state.editor;
            let before = editor.shell_signal();
            let close_requested = ui.ctx().input(|input| input.viewport().close_requested())
                || crate::window_host::close_requested(ui.ctx(), ui.ctx().viewport_id());
            editor.ui_in_window(ui, None);
            state.closed = !editor.process_close_pending()
                && (editor.close_accepted() || (close_requested && !editor.is_dirty_for_close()));
            if state.closed {
                crate::window_host::acknowledge_close(ui.ctx(), ui.ctx().viewport_id());
                crate::window_host::transition(
                    ui.ctx(),
                    ui.ctx().viewport_id(),
                    crate::window_host::Lifecycle::DurablyClosed,
                );
            }
            if state.closed
                || focus_changed
                || editor.shell_signal() != before
                || editor.has_shell_work()
            {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_callbacks_are_inert_and_documents_never_cross_threads() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let host = DocumentHost::new(EditorApp::dormant_for_tests(&context, root.path().into()));
        let token = host.token();
        assert!(token.with_state(|_| ()).is_some());
        assert!(
            std::thread::spawn(move || token.with_state(|_| ()))
                .join()
                .is_err()
        );
        drop(host);
        assert!(
            token
                .with_state(|_| panic!("closed editor accessed"))
                .is_none()
        );
        assert!(!DOCUMENTS.with_borrow(|documents| documents.contains_key(&token.id)));
    }

    #[test]
    fn queued_broadcasts_cannot_resurrect_removed_workspace_history() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let host = DocumentHost::new(EditorApp::dormant_for_tests(&context, root.path().into()));
        let mut settings = host.borrow().settings_snapshot();
        settings.remember_workspace(root.path());
        host.queue_settings(settings);
        host.forget_workspaces(&[root.path().into()]);
        host.apply_settings(&context);
        assert!(
            !host
                .borrow()
                .settings_snapshot()
                .recent_workspaces
                .contains(&root.path().to_string_lossy().into_owned())
        );
    }

    #[test]
    fn deferred_document_wheel_frames_do_not_paint_parent_or_siblings() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let ids = [
            super::super::document_viewport_id(1),
            super::super::document_viewport_id(2),
        ];
        let hosts = ids.map(|id| {
            DocumentHost::new(EditorApp::dormant_window_for_tests(
                &context,
                root.path().into(),
                id,
            ))
        });
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 700.0),
            )),
            ..Default::default()
        };
        for id in ids {
            raw.viewports.insert(
                id,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(false),
                    ..Default::default()
                },
            );
        }
        let register = |ui: &mut egui::Ui| {
            for (id, host) in ids.iter().zip(&hosts) {
                let token = host.token();
                crate::viewport_fonts::show_deferred(
                    ui.ctx(),
                    *id,
                    super::super::document_viewport_builder("fixture".into(), false),
                    move |ui, _| token.paint(ui),
                );
            }
        };
        let output = context.run_ui(raw.clone(), register);
        let callbacks = ids.map(|id| {
            assert!(output.viewport_output[&id].class == egui::ViewportClass::Deferred);
            output.viewport_output[&id].viewport_ui_cb.clone().unwrap()
        });
        output.drop_without_applying_deltas();
        for host in &hosts {
            assert_eq!(host.editor.borrow().paints, 0);
        }
        for (id, callback) in ids.iter().zip(&callbacks) {
            raw.viewport_id = *id;
            context
                .run_ui(raw.clone(), |ui| callback(ui))
                .drop_without_applying_deltas();
        }
        let before = hosts.each_ref().map(|host| host.editor.borrow().paints);
        let root_pass = context.cumulative_pass_nr_for(egui::ViewportId::ROOT);
        raw.viewport_id = ids[0];
        for _ in 0..10 {
            raw.events = vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -20.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            }];
            context
                .run_ui(raw.clone(), |ui| callbacks[0](ui))
                .drop_without_applying_deltas();
        }
        assert!(hosts[0].editor.borrow().paints >= before[0] + 10);
        assert_eq!(hosts[1].editor.borrow().paints, before[1]);
        assert_eq!(
            context.cumulative_pass_nr_for(egui::ViewportId::ROOT),
            root_pass
        );
        let before = hosts.each_ref().map(|host| host.editor.borrow().paints);
        raw.viewport_id = egui::ViewportId::ROOT;
        raw.events.clear();
        for _ in 0..10 {
            context
                .run_ui(raw.clone(), register)
                .drop_without_applying_deltas();
        }
        assert_eq!(
            hosts.each_ref().map(|host| host.editor.borrow().paints),
            before
        );

        let mut settings = hosts[0].borrow().settings_snapshot();
        settings.line_numbers = !settings.line_numbers;
        hosts[0].queue_settings(settings.clone());
        assert_ne!(
            hosts[0].borrow().settings_snapshot().line_numbers,
            settings.line_numbers
        );
        raw.viewport_id = ids[0];
        context
            .run_ui(raw.clone(), |ui| callbacks[0](ui))
            .drop_without_applying_deltas();
        assert_eq!(
            hosts[0].borrow().settings_snapshot().line_numbers,
            settings.line_numbers
        );
        assert_ne!(
            hosts[1].borrow().settings_snapshot().line_numbers,
            settings.line_numbers
        );
        assert!(
            !hosts[0].borrow().has_settings_update(),
            "broadcasts must not echo back as edits"
        );

        raw.viewports
            .get_mut(&ids[0])
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        context
            .run_ui(raw.clone(), |ui| callbacks[0](ui))
            .drop_without_applying_deltas();
        assert!(hosts[0].closed());
        assert!(!hosts[1].closed());
        let painted = hosts[0].editor.borrow().paints;
        context
            .run_ui(raw, |ui| callbacks[0](ui))
            .drop_without_applying_deltas();
        assert_eq!(hosts[0].editor.borrow().paints, painted);
    }
}
