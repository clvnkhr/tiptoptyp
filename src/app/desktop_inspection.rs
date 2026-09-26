//! Read-only observations for externally driven desktop journeys.
use super::*;
impl EditorApp {
    pub(super) fn publish_desktop_inspection(&self, context: &egui::Context) {
        if !crate::desktop_test::requested() {
            return;
        }
        if let Some(webview) = &self.webview
            && let Some(epoch) = crate::desktop_test::request_renderer()
            && webview
                .evaluate_script_with_callback(
                    include_str!("desktop_preview_probe.js"),
                    move |value| crate::desktop_test::renderer_observed(epoch, value),
                )
                .is_err()
        {
            crate::desktop_test::renderer_observed(epoch, "null".into());
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
        let cursor = egui::text_edit::TextEditState::load(context, source_editor_id(context))
            .and_then(|state| state.cursor.char_range())
            .map(|range| [range.primary.index.0, range.secondary.index.0]);
        let folding = &self.tabs.current_record().folding;
        let controls = if self.interactive_preview_active() {
            self.preview_controls.web.clone()
        } else {
            self.pdfium_preview.controls_snapshot()
        };
        let preview = self.preview_status_snapshot();
        let palette = self.preview_palette(context, self.preview.dark);
        let mut observation = serde_json::json!({
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
        });
        let find_viewport = scoped_child_viewport_id(context, find_bar::VIEWPORT_SALT);
        let controls_viewport = scoped_child_viewport_id(context, "preview-controls");
        let hover_tooltip_id = native_hover_tooltip_id(context);
        let interaction = serde_json::json!({
            "source_fingerprint": crate::desktop_test::fingerprint(self.document().source()),
            "cursor": cursor,
            "pointer": context.input(|input| input.pointer.hover_pos()).map(|position| [position.x, position.y]),
            "dirty": self.document().is_dirty(),
            "modal_open": self.document_workflow.modal().is_some(),
            "view_mode": format!("{:?}", self.view_mode),
            "explorer_visible": self.explorer.panel_visible(),
            "explorer_maximized": self.explorer.maximized_section(true).map(|section| section.title()),
            "line_wrap": self.settings.line_wrap,
            "line_numbers": self.settings.line_numbers,
            "sticky_context": self.settings.sticky_context_rows,
            "fold_headers": folding.regions.iter().map(|region| region.line).collect::<Vec<_>>(),
            "fold_collapsed": folding.regions.iter().filter(|region| folding.is_collapsed(region.line)).map(|region| region.line).collect::<Vec<_>>(),
            "find_query_fingerprint": crate::desktop_test::fingerprint(&self.find_bar.query),
            "find_case": self.find_bar.case_sensitive,
            "find_regex": self.find_bar.regex,
            "find_native_height": context.input(|input| input.raw.viewports.get(&find_viewport).and_then(|viewport| viewport.inner_rect)).map(|rect| rect.height()),
            "find_measured_height": self.find_bar.native_height,
            "find_selected": self.find_bar.search.selected_ordinal(),
            "preview_surface_exists": self.webview.is_some(),
            "preview_surface_visible": self.webview_applied.is_some_and(|state| state.visible),
            "settings_highlight": settings_highlight(context, scoped_child_viewport_id(context, "tiptoptyp-settings")).map(|target| target.label()),
            "preview_controls_visible": eframe::window_host::window(context, controls_viewport).and_then(|window| window.is_visible()),
            "preview_controls_measured_size": self.preview_controls.inspected_size().map(|size| [size.x, size.y]),
            "preview_controls_size": context.input(|input| input.raw.viewports.get(&controls_viewport).and_then(|viewport| viewport.inner_rect)).map(|rect| [rect.width(), rect.height()]),
            "hover_tooltip_open": context.data(|data| data.get_temp::<HoverTooltipOverlay>(hover_tooltip_id).is_some()),
            "preview_controls_open": self.preview_controls.open,
            "preview_popout": self.preview_controls.popout.is_some(),
            "preview_outline": self.preview_controls.outline,
            "preview_page": controls.page,
            "preview_search_offset": self.web_search_offset,
            "preview_search_query_fingerprint": crate::desktop_test::fingerprint(&self.web_search_query),
            "preview_pages": controls.count,
            "preview_zoom": controls.zoom,
            "preview_back": controls.back,
            "preview_forward": controls.forward,
        });
        observation
            .as_object_mut()
            .unwrap()
            .extend(interaction.as_object().unwrap().clone());
        crate::desktop_test::publish(observation);
    }
}
