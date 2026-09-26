//! Read-only observations for externally driven desktop journeys.
use super::*;
impl EditorApp {
    pub(super) fn publish_desktop_inspection(&self, context: &egui::Context) {
        if !crate::desktop_test::requested() {
            return;
        }
        let panel_id = crate::child_view::viewport_scoped_id(
            context,
            if self.bottom_panel.is_maximized() {
                "bottom-panel-maximized"
            } else {
                "bottom-panel"
            },
        );
        let panel = egui::PanelState::load(context, panel_id).map(|state| {
            let rect = state.outer_rect;
            [rect.left(), rect.top(), rect.right(), rect.bottom()]
        });
        let preview = self.preview_status_snapshot();
        crate::desktop_test::publish(serde_json::json!({
            "viewport": format!("{:?}", context.viewport_id()),
            "frame": context.cumulative_frame_nr(),
            "focused": context.input(|input| input.viewport().focused),
            "minimized": context.input(|input| input.viewport().minimized),
            "tabs": self.tabs.len(),
            "path": self.document().path(),
            "source_bytes": self.document().source().len(),
            "revision": format!("{:?}", self.document().revision()),
            "preview_path": self.preview_document_path(),
            "preview_backend": preview.backend_label(),
            "preview_requested_backend": format!("{:?}", preview.requested_backend),
            "preview_pdfium_ready": self.pdfium_preview.ready_for(self.preview.content.pdf()),
            "preview_native_ready": preview.native_ready,
            "preview_failure": preview.failure_reason(),
            "dark": context.theme() == egui::Theme::Dark,
            "comfy": self.settings.comfy_preview,
            "panel": self.bottom_panel.selected().map(|tab| format!("{tab:?}")),
            "panel_rect": panel,
            "panel_maximized": self.bottom_panel.is_maximized(),
            "find_visible": self.find_bar.visible,
            "find_focused": self.find_bar.child_focused,
            "replace_visible": self.find_bar.replace_visible,
            "settings_visible": self.settings_visible,
        }));
    }
}
