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
        let controls = self.preview_controls_snapshot();
        let preview = self.preview_status_snapshot();
        let palette = self.preview_palette(context, self.preview.dark);
        let (
            settings_query_fingerprint,
            settings_text_input_focused,
            settings_json,
            settings_category,
        ) = {
            let settings_window = self.settings_window.lock().unwrap();
            (
                crate::desktop_test::fingerprint(&settings_window.ui.query),
                settings_window.text_input_focused,
                serde_json::json!({"fingerprint": settings_window.ui.json_draft.as_ref().map(|draft| crate::desktop_test::fingerprint(&draft.text)), "visible": settings_window.ui.json_mode, "query": settings_window.ui.json_query, "valid": settings_window.ui.json_draft.as_ref().is_some_and(|draft| draft.validation.is_ok()), "saved": settings_window.ui.json_draft.as_ref().is_some_and(|draft| draft.saved)}),
                settings_window.ui.category.title(),
            )
        };
        let snippets = self.settings.snippets.iter().map(|s| serde_json::json!({"prefix": s.prefix, "language": s.language, "scope": s.scope, "line_start": s.line_start, "enabled": s.enabled, "placeholder_open": s.placeholder_open, "placeholder_close": s.placeholder_close, "description": crate::desktop_test::fingerprint(&s.description), "body": crate::desktop_test::fingerprint(&s.body)})).collect::<Vec<_>>();
        let mut observation = serde_json::json!({
            "viewport": format!("{:?}", context.viewport_id()),
            "native_size": eframe::window_host::window(context, context.viewport_id()).map(|w| { let s = w.inner_size().to_logical::<f64>(w.scale_factor()); [s.width, s.height] }),
            "frame": context.cumulative_frame_nr(),
            "focused": context.input(|input| input.viewport().focused),
            "minimized": context.input(|input| input.viewport().minimized),
            "tabs": self.tabs.len(),
            "tab_order": self.tabs.ids().collect::<Vec<_>>(),
            "active_tab_id": self.tabs.active_id(),
            "preview_tab_id": self.tabs.preview_id(),
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
            "comfy": self.settings.comfy_background && self.settings.comfy_text,
            "page_dark": self.preview.dark,
            "dark_images": self.settings.dark_images,
            "asset_preview_dark": self.asset_preview.dark,
            "asset_image_ready": self.document().kind() == DocumentKind::Image && matches!(self.asset_preview.status, PreviewStatus::Ready(_)) && self.asset_preview.has_resident_pages(),
            "asset_image_pixels": self.asset_preview.content.pages().first().and_then(|page| page.resident.as_ref()).filter(|resident| resident.is_usable()).map(|resident| resident.rgba.iter().take(16).copied().collect::<Vec<_>>()),
            "expected_palette": [palette.0.to_array(), palette.1.to_array()],
            "webview_palette": self.webview_style.map(|style| [style.palette.0.to_array(), style.palette.1.to_array()]),
            "pdfium_palette": self.pdfium_preview.inspected_palette(),
            "panel": self.bottom_panel.selected().map(|tab| format!("{tab:?}")),
            "panel_rect": panel,
            "panel_maximized": self.bottom_panel.is_maximized(),
            "find_visible": self.find_bar.visible,
            "find_focused": self.find_bar.child_focused,
            "replace_visible": self.find_bar.replace_visible,
            "settings_visible": self.settings_visible,
            "table_editor_open": self.table_editor.is_some(),
            "settings_help_visible": self.settings_window.lock().unwrap().ui.help_mode,
            "settings_json": settings_json,
            "writing_language": self.settings.writing_language,
            "settings_query_fingerprint": settings_query_fingerprint,
            "settings_text_input_focused": settings_text_input_focused,
        });
        observation["synctex_available"] = self
            .preview
            .content
            .pdf()
            .is_some_and(|bytes| {
                self.pdfium_source_jump_available(false, &self.preview_document_path(), bytes)
            })
            .into();
        observation["palette_visible"] = self.command_palette_visible().into();
        observation["palette_query_fingerprint"] =
            crate::desktop_test::fingerprint(&self.command_palette.query).into();
        observation["palette_selected"] = self
            .command_palette
            .selected_action()
            .map(ShortcutAction::id)
            .into();
        observation["palette_results"] = self
            .command_palette
            .result_actions()
            .map(ShortcutAction::id)
            .collect::<Vec<_>>()
            .into();
        observation["palette_enabled"] = self
            .command_palette
            .enabled_actions()
            .map(ShortcutAction::id)
            .collect::<Vec<_>>()
            .into();
        observation["interface_theme"] =
            serde_json::to_value(self.settings.interface_theme).unwrap();
        observation["editor_effects"] = serde_json::to_value(self.settings.editor_effects).unwrap();
        observation["typing_particles"] =
            editor_particles::inspected(context, source_editor_id(context), self.document().key());
        observation["reveal_animation_options"] = serde_json::json!({
            "source": self.settings.source_reveal_animation,
            "preview": self.settings.preview_reveal_animation,
        });
        observation["settings_category"] = settings_category.into();
        observation["compact_layout_controls"] = self.settings.compact_layout_controls.into();
        observation["pdfium_render"] = self.pdfium_preview.inspected_render();
        observation["fullscreen"] = context
            .input(|input| input.viewport().fullscreen.unwrap_or(false))
            .into();
        observation["window_background"] =
            serde_json::to_value(self.settings.window_background).unwrap();
        observation["native_background"] =
            crate::window_background::inspection(context, context.viewport_id());
        observation["settings_native_background"] = crate::window_background::inspection(
            context,
            scoped_child_viewport_id(context, "tiptoptyp-settings"),
        );
        observation["mitex_enabled"] = self.document().config().is_some().into();
        observation["ascii_punctuation"] = self.settings.ascii_punctuation.into();
        observation["native_ime_area"] = crate::native_window::for_viewport(
            context,
            context.viewport_id(),
        )
        .and_then(|view| view.ime_cursor_area())
        .and_then(|(local, first_responder)| {
            let origin = context.input(|input| input.viewport().inner_rect)?.min;
            Some(serde_json::json!({
                "rect": crate::native_window::ime_screen_rect(origin, local, context.zoom_factor()),
                "local_rect": local,
                "first_responder": first_responder,
                "source": "NSTextInputClient.firstRectForCharacterRange",
            }))
        })
        .into();
        {
            let snippets = self.snippet_window.lock().unwrap();
            observation["snippet_editor_visible"] = self.snippets_visible.into();
            observation["snippet_editor_query_fingerprint"] =
                crate::desktop_test::fingerprint(snippets.search_query()).into();
            observation["snippet_editor_fields"] = snippets.field_fingerprints();
            observation["snippet_editor_focused"] = snippets.focused.into();
            observation["snippet_editor_text_focused"] = snippets.text_input_focused.into();
        }
        observation["snippets"] = snippets.into();
        observation["snippet_presets"] = self.settings.snippet_presets.iter().map(|s| serde_json::json!({"prefix": s.prefix, "language": s.language, "scope": s.scope, "line_start": s.line_start, "enabled": s.enabled, "placeholder_open": s.placeholder_open, "placeholder_close": s.placeholder_close, "description": crate::desktop_test::fingerprint(&s.description), "body": crate::desktop_test::fingerprint(&s.body)})).collect::<Vec<_>>().into();
        let find_viewport = scoped_child_viewport_id(context, find_bar::VIEWPORT_SALT);
        let controls_owner = if self.preview_controls_in_popout() {
            scoped_child_viewport_id(context, "preview-window")
        } else {
            context.viewport_id()
        };
        let controls_viewport =
            crate::child_view::child_viewport_id(controls_owner, preview_controls::SALT);
        let hover_tooltip_id = native_hover_tooltip_id(context);
        let mut interaction = serde_json::json!({
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
            "preview_controls_position": context.input(|input| input.raw.viewports.get(&controls_viewport).and_then(|viewport| viewport.outer_rect)).map(|rect| [rect.min.x, rect.min.y]),
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
            "diagnostic_count": self.preview.diagnostics.len() + self.preview.editor_diagnostics.len(),
            "completion_visible": self.editor_completion.as_ref().is_some_and(|completion| !completion.items.is_empty()),
            "completion_count": self.editor_completion.as_ref().map_or(0, |completion| completion.items.len()),
            "completion_selected": self.editor_completion.as_ref().map(|completion| completion.selected),
        });

        let font_name = self.editor_completion.as_ref().and_then(|completion| {
            let item = completion.items.get(completion.selected)?;
            (item.origin == crate::lsp::protocol::CompletionOrigin::Font)
                .then_some((completion.selected, item.label.as_str()))
        });
        interaction["completion_font_name"] = font_name.map(|(_, name)| name).into();
        interaction["completion_font_name_ready"] = font_name
            .is_some_and(|(index, name)| crate::font_preview::name_is_ready(context, index, name))
            .into();
        interaction["editor_lsp_ready"] = self.editor_lsp_ready().into();
        interaction["completion_current"] = self
            .editor_completion
            .as_ref()
            .is_some_and(|completion| {
                let (generation, uri) = self.editor_lsp_identity();
                completion.key == self.document().key()
                    && completion.version == revision_as_i32(self.document().revision())
                    && completion.provenance.is_current(generation, uri)
            })
            .into();
        interaction["completion_notice"] = self
            .notice
            .as_ref()
            .map(|notice| notice.message.as_str())
            .into();
        interaction["completion_labels"] = self
            .editor_completion
            .as_ref()
            .map(|completion| {
                completion
                    .items
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
            .into();
        interaction["completion_origins"] = serde_json::json!(
            self.editor_completion
                .as_ref()
                .map(|completion| completion
                    .items
                    .iter()
                    .map(|item| item.origin.label())
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        );
        interaction.as_object_mut().unwrap().extend(
            serde_json::json!({
            "texlab_ready": self.tex_service.texlab_ready,
            "badness_ready": self.tex_service.badness_ready,
            "tex_diagnostic_counts": self.tex_diagnostics.iter().map(|diagnostics| diagnostics.len()).collect::<Vec<_>>(),
            "tex_diagnostics_current": self.activity.diagnostics[1..].iter().map(|key| *key == Some(self.document().key())).collect::<Vec<_>>(),
            "completion_pending": self.activity.completion.is_some(),
            "writing_count": self.writing.diagnostics.len(),
            "writing_lines": self.writing.diagnostics.iter().filter_map(|diagnostic| diagnostic.line()).collect::<Vec<_>>(),
            "writing_fingerprint": crate::desktop_test::fingerprint(&format!("{:?}", self.writing.diagnostics)),
            "writing_pending": matches!(self.writing.activity(), crate::activity::Activity::Pending(_) | crate::activity::Activity::Running),
            "pdf_output_state": format!("{:?}", self.pdf_output_status()),
            "pdf_output_pending": self.document_workflow.pending_export.is_some(),
            "pdf_output_revision": self.pdf_output_key().revision,
            "compile_indicator_spinning": self.compile_indicator_spinning(),
            "preview_generation": format!("{:?}", self.tinymist_sync.generation),
            }).as_object().unwrap().clone(),
        );
        interaction["version_history"] = self
            .version_history
            .as_ref()
            .map(|dialog| dialog.inspect(&self.pdfium_preview))
            .unwrap_or_else(|| {
                serde_json::json!({
                    "visible": false, "loading": false, "count": 0, "left": null, "right": null,
                    "selected": [null, null], "can_restore": [false, false], "rendering": false, "preview": null, "error": null,
                })
            });
        interaction["harper_dictionary_fingerprint"] = serde_json::json!(
            crate::desktop_test::fingerprint(&self.settings.harper_dictionary)
        );
        interaction["harper_ignore_capitalized_names"] =
            serde_json::json!(self.settings.harper_ignore_capitalized_names);
        interaction["writing_current"] = serde_json::json!(
            !self.tabs.is_empty()
                && self.writing.current(
                    self.document().key(),
                    self.settings.english_grammar,
                    self.settings.unicode_warnings,
                    self.settings.writing_language
                )
        );
        interaction["writing_locations"] = serde_json::json!(
            self.writing
                .diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic
                    .location
                    .map(|location| [location.line, location.column]))
                .collect::<Vec<_>>()
        );
        interaction["writing_paths"] = serde_json::json!(
            self.writing
                .diagnostics
                .iter()
                .filter_map(|diagnostic| match &diagnostic.source {
                    DiagnosticSource::File(path) => Some(path),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
        interaction["format_after_math_conversion"] =
            serde_json::json!(self.settings.format_after_math_conversion);
        interaction["explorer_panel_width"] = serde_json::json!(
            egui::PanelState::load(context, explorer_panel_id(context)).map(|state| state.size().x)
        );
        interaction["table_draft"] = self.table_editor.as_ref().map_or(
            serde_json::Value::Null,
            table_workbench::TableEditorDialog::desktop_inspection,
        );
        interaction["table_samples"] = self.table_samples.inspect();
        interaction["compilation_paused"] = serde_json::json!(self.compilation_paused);
        interaction["compiler_diagnostic_count"] =
            serde_json::json!(self.preview.diagnostics.len());
        interaction["terminal_off"] = serde_json::json!(self.terminal.is_off());
        interaction["terminal_status"] = serde_json::json!(self.terminal.status_text());
        interaction["terminal_count"] = serde_json::json!(self.terminal.len());
        interaction["terminal_active"] = serde_json::json!(self.terminal.active_index());
        interaction["terminal_find"] = self.terminal.inspect_find(context);
        interaction["terminal_content_revision"] =
            serde_json::json!(self.terminal.content_revision_for_test());
        interaction["independent_main_dividers"] =
            serde_json::json!(self.settings.independent_main_dividers);
        interaction["indentation_guides"] = serde_json::json!(self.settings.indentation_guides);
        interaction["highlight_local_uses"] = serde_json::json!(self.settings.highlight_local_uses);
        interaction["highlight_rules"] = serde_json::json!(self.settings.highlight_rules.len());
        let hover_viewport = scoped_child_viewport_id(context, "diagnostic-tooltip-overlay");
        interaction["native_hover_window"] =
            serde_json::json!(eframe::window_host::window(context, hover_viewport).is_some());
        let hover_geometry_id = tooltip_geometry_id(context);
        interaction["hover_tooltip_rect"] = context
            .data(|data| data.get_temp::<TooltipGeometry>(hover_geometry_id))
            .map_or(serde_json::Value::Null, |geometry| {
                serde_json::json!([
                    geometry.card.min.x,
                    geometry.card.min.y,
                    geometry.card.width(),
                    geometry.card.height()
                ])
            });

        interaction["source_reveal"] = reveal::editor_observation(context)
            .map(|state| serde_json::json!({
                "active": state.active, "center": state.center, "target": state.target,
                "viewport": state.viewport, "elapsed_ms": state.elapsed_ms, "char_index": state.char_index,
            }))
            .unwrap_or(serde_json::Value::Null);
        interaction["preview_reveal"] = self.pdfium_preview.inspected_reveal();
        interaction["git_overview"] =
            crate::git::editor::overview::inspection(context).unwrap_or(serde_json::Value::Null);
        interaction["git_hunk_count"] = serde_json::json!(self.git_editor.hunks.len());
        interaction["git_hunk_popup_line"] = serde_json::json!(if let Some(diff) =
            &self.document_diff
        {
            let at = self.last_editor_caret.map_or(0, |c| c.char_index);
            let line = self
                .document()
                .source()
                .chars()
                .take(at)
                .filter(|&c| c == '\n')
                .count();
            diff.hunks
                .iter()
                .flat_map(|h| &h.changes)
                .find(|c| c.lines.contains(&line) || (c.lines.is_empty() && c.lines.start == line))
                .map(|c| c.lines.start)
        } else {
            None
        });
        interaction["git_diff_open"] = serde_json::json!(self.document_diff.is_some());
        interaction["git_diff_lines"] = serde_json::json!(
            self.document_diff
                .as_ref()
                .map(|diff| [diff.before.lines().count(), diff.current.lines().count()])
        );
        interaction["git_diff_builds"] =
            serde_json::json!(self.document_diff.as_ref().map(|diff| diff.builds));
        interaction["git_diff_changed_pairs"] =
            serde_json::json!(self.document_diff.as_ref().map(|diff| {
                diff.pairs
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.changed)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>()
            }));
        interaction["git_diff_hunks"] =
            serde_json::json!(self.document_diff.as_ref().map(|diff| {
                diff.hunks
                    .iter()
                    .filter_map(|h| h.changes.first().map(|c| c.lines.start))
                    .collect::<Vec<_>>()
            }));
        interaction["editor_panel_width"] = serde_json::json!(
            egui::PanelState::load(context, editor_panel_id(context)).map(|p| p.size().x)
        );
        interaction["editor_panel_rect"] = serde_json::json!(
            egui::PanelState::load(context, editor_panel_id(context)).map(|panel| {
                let rect = panel.outer_rect;
                [rect.left(), rect.top(), rect.right(), rect.bottom()]
            })
        );
        interaction["explorer_focused"] = serde_json::json!(
            self.explorer
                .focused_section()
                .map(|section| section.title())
        );
        #[cfg(target_os = "macos")]
        {
            interaction["preview_zoom_overlay"] = self.webview_snapshot.inspect();
        }
        interaction["preview_keyboard_focused"] =
            serde_json::json!(if self.pdfium_preview_requested() {
                self.pdfium_preview.has_focus(context)
            } else {
                self.preview_controls.web.focused
                    && self.webview_applied.is_some_and(|state| state.visible)
            });
        interaction["preview_controls_target"] =
            serde_json::json!(match self.preview_controls.target {
                preview_controls::Target::Source => "source",
                preview_controls::Target::Asset(_) => "asset",
            });
        interaction["preview_query_fingerprint"] = serde_json::json!(
            crate::desktop_test::fingerprint(&self.preview_controls.query)
        );
        interaction["preview_controls_local_position"] = serde_json::json!(
            self.preview_controls
                .position
                .map(|point| [point.x, point.y])
        );
        interaction["interface_scale"] = serde_json::json!(context.zoom_factor());
        let editor_id = source_editor_id(context);
        interaction["bottom_panel_keyboard_focused"] = serde_json::json!(
            self.bottom_panel.is_visible()
                && context.input(|input| input.focused)
                && egui::PanelState::load(context, panel_id)
                    .is_some_and(|state| { theme::focus_is_inside(context, state.outer_rect) })
        );
        interaction["problems_keyboard_focused"] =
            serde_json::json!(self.problems_focused(context, context.viewport_id()));
        interaction["editor_keyboard_focused"] =
            serde_json::json!(context.memory(|m| m.has_focus(editor_id)));
        interaction["terminal_keyboard_focused"] =
            serde_json::json!(self.terminal_focused(context, context.viewport_id()));
        interaction["drawing"] = self.symbol_drawing.inspection(
            self.document().kind() == DocumentKind::Tex || self.document().config().is_some(),
            self.settings.handwriting,
        );
        observation
            .as_object_mut()
            .unwrap()
            .extend(interaction.as_object().unwrap().clone());
        crate::desktop_test::publish(observation);
    }
}
