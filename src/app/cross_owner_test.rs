//! Test-only document state for the bounded cross-window ownership scenario.

use super::*;

impl EditorApp {
    /// Build the document-local half of the bounded cross-owner architecture
    /// regression without exposing test orchestration in the production API.
    pub(crate) fn prepare_cross_owner_test_state(
        &mut self,
        context: &egui::Context,
        save_path: PathBuf,
    ) {
        self.snapshot_scene = None;
        let preview_id = self.tabs.preview_id().expect("fixture has a preview tab");
        self.prepare_tabs_fixture(context);
        self.document_mut()
            .replace_unprojected_untitled("pending save from owner A");
        assert!(self.save_to(save_path, context));
        assert!(
            self.save_job.is_running(),
            "the cross-owner fixture must retain an in-flight save"
        );

        let stale_key = self.document().key();
        let stale_uri = "file:///cross-owner.typ".to_owned();
        let stale_generation = Generation(17);
        let stale_version = revision_as_i32(self.document().revision());
        let stale_token = 91;
        self.tinymist_sync.generation = Some(stale_generation);
        self.tinymist_sync.current_uri = Some(stale_uri.clone());
        self.editor_completion = Some(EditorCompletionState {
            key: stale_key,
            provenance: CompletionProvenance::Server {
                generation: stale_generation,
                uri: stale_uri,
                request_token: stale_token,
            },
            version: stale_version,
            cursor: 0,
            source_cursor: 0,
            anchor: Rect::ZERO,
            explicit: true,
            is_incomplete: false,
            selected: 0,
            items: vec![CompletionItem {
                label: "stale completion".to_owned(),
                detail: None,
                documentation: None,
                filter_text: None,
                sort_text: None,
                insert_text: "stale completion".to_owned(),
                insert_text_is_snippet: false,
                text_edit: None,
                additional_text_edits: Vec::new(),
            }],
            all_items: Vec::new(),
            source: self.document().source().clone(),
        });

        // Switch the active tab without starting a service in this dormant,
        // deterministic fixture. The reply still carries the old tab key and
        // must be rejected by the normal document-identity checks.
        self.tabs.set_active_for_test(preview_id);
        assert_eq!(self.tabs.preview_id(), Some(preview_id));
        assert!(self.tabs.uses_designated_preview());
    }

    /// Deliver the reply prepared above after the owner has switched tabs.
    /// This calls the real reply adapter, not a parallel test-only path.
    pub(crate) fn deliver_cross_owner_test_reply(&mut self, context: &egui::Context) {
        let pending = self
            .editor_completion
            .clone()
            .expect("cross-owner fixture has a pending reply");
        let CompletionProvenance::Server {
            generation,
            uri,
            request_token,
        } = pending.provenance
        else {
            panic!("cross-owner fixture must use server completion provenance");
        };
        let before = self.document().source().clone();
        self.receive_editor_completions(
            EditorCompletionResponse {
                generation,
                uri,
                version: pending.version,
                request_token,
                is_incomplete: false,
                items: vec![CompletionItem {
                    label: "late completion".to_owned(),
                    detail: None,
                    documentation: None,
                    filter_text: None,
                    sort_text: None,
                    insert_text: "late completion".to_owned(),
                    insert_text_is_snippet: false,
                    text_edit: None,
                    additional_text_edits: Vec::new(),
                }],
            },
            context,
        );
        assert_eq!(self.document().source(), &before);
        assert_eq!(
            self.editor_completion
                .as_ref()
                .map(|completion| completion.items[0].label.as_str()),
            Some("stale completion")
        );
    }
}
