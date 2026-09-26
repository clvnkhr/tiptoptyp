//! Read-only observations for externally driven desktop journeys.
use super::*;
impl EditorApp {
    pub(super) fn publish_desktop_inspection(&self, context: &egui::Context) {
        if !crate::desktop_test::requested() {
            return;
        }
        if let Some(webview) = &self.webview
            && crate::desktop_test::request_renderer()
            && webview
                .evaluate_script_with_callback(
                    include_str!("desktop_preview_probe.js"),
                    crate::desktop_test::renderer_observed,
                )
                .is_err()
        {
            crate::desktop_test::renderer_observed("null".into());
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
        let palette = self.preview_palette(context, self.preview.dark);
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
            "page_dark": self.preview.dark,
            "expected_palette": [palette.0.to_array(), palette.1.to_array()],
            "webview_palette": self.webview_palette.map(|p| [p.0.to_array(), p.1.to_array()]),
            "pdfium_palette": self.pdfium_preview.inspected_palette(),
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
