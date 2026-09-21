//! Source editor widgets and completion rendering.
use super::*;

use eframe::egui::text::{ByteIndex, LayoutJob, LayoutSection};

impl EditorApp {
    pub(super) fn show_find_bar(&mut self, ui: &mut egui::Ui) {
        let source = self.tabs.current_record().document.source();
        let document_key = self.document().key();
        let actions = find_bar::show(ui, &mut self.find_bar, source, document_key);
        self.apply_find_actions(
            ui.ctx(),
            actions.previous,
            actions.next,
            actions.replace_one,
            actions.replace_all,
        );
    }

    pub(super) fn apply_find_actions(
        &mut self,
        context: &egui::Context,
        find_previous: bool,
        find_next: bool,
        replace_one: bool,
        replace_all: bool,
    ) {
        let search_revision = self.document().key();
        if find_previous {
            let source = self.tabs.current_record().document.source();
            self.pending_editor_selection = self
                .find_bar
                .search
                .previous(
                    source,
                    search_revision,
                    &self.find_bar.query,
                    self.find_bar.case_sensitive,
                    self.find_bar.regex,
                )
                .map(|matched| EditorSelection::Search(matched.char_range.clone()));
        }
        if find_next {
            let source = self.tabs.current_record().document.source();
            self.pending_editor_selection = self
                .find_bar
                .search
                .next(
                    source,
                    search_revision,
                    &self.find_bar.query,
                    self.find_bar.case_sensitive,
                    self.find_bar.regex,
                )
                .map(|matched| EditorSelection::Search(matched.char_range.clone()));
        }
        if replace_one {
            let before = self.document().source().clone();
            let snapshot = self.editor_snapshot(context);
            let find_query = &self.find_bar.query;
            let replacement = &self.find_bar.replacement;
            let find_case_sensitive = self.find_bar.case_sensitive;
            let find_regex = self.find_bar.regex;
            let search = &mut self.find_bar.search;
            let document = &mut self.tabs.current_record_mut().document;
            let replaced = document.edit(snapshot.cursor, |source| {
                search.replace_one(
                    source,
                    search_revision,
                    find_query,
                    replacement,
                    find_case_sensitive,
                    find_regex,
                )
            });
            if replaced {
                self.pending_editor_selection = self
                    .find_bar
                    .search
                    .selected()
                    .map(|matched| EditorSelection::Search(matched.char_range.clone()));
                if *self.document().source() != before {
                    self.mark_edited();
                }
            }
        }
        if replace_all {
            let before = self.document().source().clone();
            let snapshot = self.editor_snapshot(context);
            let find_query = &self.find_bar.query;
            let replacement = &self.find_bar.replacement;
            let find_case_sensitive = self.find_bar.case_sensitive;
            let find_regex = self.find_bar.regex;
            let search = &mut self.find_bar.search;
            let document = &mut self.tabs.current_record_mut().document;
            let count = document.edit(snapshot.cursor, |source| {
                search.replace_all(
                    source,
                    search_revision,
                    find_query,
                    replacement,
                    find_case_sensitive,
                    find_regex,
                )
            });
            if count > 0 && *self.document().source() != before {
                self.notice = Some(Notice {
                    message: format!("Replaced {count} matches"),
                    kind: NoticeKind::Success,
                });
                self.mark_edited();
            }
        }
    }

    pub(super) fn show_editor(&mut self, ui: &mut egui::Ui) {
        let _span = crate::performance::span("ui.source");
        offer_file_drop_target(ui, ui.max_rect(), FileDropTarget::Editor);
        ui.set_min_width(ui.available_width());
        if self.document().reset_editor_history {
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id(ui.ctx()))
                    .unwrap_or_default();
            state.clear_undoer();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), source_editor_id(ui.ctx()));
            self.document_mut().set_history_reset(false);
        }
        self.prepare_editor_data();
        let source_metrics = self.editor_data.source_metrics();
        let line_diagnostics = self.editor_data.line_diagnostics();
        let available_width = ui.available_width();
        let line_count = source_metrics.line_count;
        let longest_line = source_metrics.longest_line_chars;
        let longest_diagnostic = self.editor_data.longest_diagnostic_chars();
        let unwrapped_editor_width = available_width
            .max(
                (longest_line as f32 * METRICS.editor.source_character_width)
                    + (longest_diagnostic as f32 * METRICS.editor.diagnostic_character_width)
                    + METRICS.editor.unwrapped_width_padding,
            )
            .max(METRICS.editor.unwrapped_minimum_width);
        let sticky_context_snapshot = self.snapshot_scene == Some(UiSnapshotScene::StickyContext);
        let folding_snapshot = self.snapshot_scene == Some(UiSnapshotScene::Folding);
        let line_wrap = sticky_context_snapshot || folding_snapshot || self.settings.line_wrap;
        let line_numbers =
            sticky_context_snapshot || folding_snapshot || self.settings.line_numbers;
        let git_gutter = self
            .git_editor
            .has_gutter(self.document().path().as_deref());
        let line_number_width =
            line_numbers.then(|| line_number_column_width(ui, line_count, &theme::editor_font()));
        let gutter_width = editor_gutter_width(line_number_width, git_gutter);
        let dark_mode = ui.visuals().dark_mode;
        let document_kind = self.document().kind();
        let contexts = if document_kind.is_typst() {
            self.editor_data.context_regions()
        } else {
            Arc::from([])
        };
        let document_key = self.document().key();
        let source_snapshot = self.editor_data.source_snapshot();
        self.folding_mut()
            .prepare(document_key, source_snapshot, &contexts);
        if !line_numbers {
            self.folding_mut().expand_all();
        }
        let fold_marker = ui.painter().layout_no_wrap(
            "...".into(),
            theme::editor_font(),
            ui.visuals().weak_text_color(),
        );
        let fold_marker_width = fold_marker.size().x + 8.0;
        self.folding_mut().set_marker_width(fold_marker_width);
        let highlight_path = self.document().path().clone();
        let asset_source_path = self.document().path().clone();
        let asset_workspace_root = self.workspace_root.clone();
        let source_preview_trigger = self.settings.source_preview_trigger;
        let preview_jump_enabled = document_kind.is_typst() && self.interactive_preview_active();
        let sticky_context_enabled = document_kind.is_typst()
            && (sticky_context_snapshot || self.settings.sticky_context_rows);
        let snapshot_scroll_offset = source_editor_snapshot_scroll_offset(self.snapshot_scene);
        let (search_highlight_matches, selected_search_match) = if self.find_bar.visible {
            let document_key = self.document().key();
            let source = self.tabs.current_record().document.source();
            let matches = self
                .find_bar
                .search
                .results(
                    source,
                    document_key,
                    &self.find_bar.query,
                    self.find_bar.case_sensitive,
                    self.find_bar.regex,
                )
                .iter()
                .map(|matched| matched.byte_range.clone())
                .collect::<Vec<_>>();
            let selected = self
                .find_bar
                .search
                .selected()
                .map(|matched| matched.byte_range.clone());
            (matches, selected)
        } else {
            (Vec::new(), None)
        };
        let completion_edit_triggered = document_kind.is_typst()
            && ui.input(|input| completion_requested_after_events(&input.events));
        if let Some(selection) = self.pending_editor_selection.clone() {
            let selection = selection.range();
            self.folding_mut().reveal(selection.start);
            self.folding_mut().reveal(selection.end);
            // Install explicit destinations before TextEdit handles input and
            // scrolls. Its old caret may lie inside a still-collapsed region;
            // revealing that stale caret after layout changes the coordinates
            // underneath the pending jump.
            let id = source_editor_id(ui.ctx());
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
            state.cursor.set_char_range(Some(CCursorRange::two(
                CCursor::new(selection.start),
                CCursor::new(selection.end),
            )));
            state.store(ui.ctx(), id);
        }
        let snapshot_before_edit = self.editor_snapshot(ui.ctx());
        self.highlighter
            .set_rainbow_brackets(self.settings.rainbow_brackets);
        let auto_pair_enabled = self.settings.auto_pair_delimiters && document_kind.is_typst();
        let auto_pair_syntax = &mut self.auto_pair_syntax;
        let highlighter = &mut self.highlighter;
        let active_tab = self
            .tabs
            .active_id()
            .expect("editor requires an active tab");
        let record = self.tabs.current_record_mut();
        let document = &mut record.document;
        let folding = &mut record.folding;
        let generic_highlighter = &mut self.generic_highlighter;
        let pending_selection = self.pending_editor_selection.take();
        let attention = self.editor_attention.and_then(|attention| {
            let elapsed = Instant::now().saturating_duration_since(attention.started);
            let progress = editor_attention_progress(elapsed);
            (progress < 1.0).then_some((attention.char_index, progress))
        });
        if attention.is_some() {
            ui.ctx()
                .request_repaint_after(METRICS.motion.animation_frame);
        } else {
            self.editor_attention = None;
        }
        let mut changed = false;
        let mut folds_changed = false;
        let mut preview_jump_char = None;
        let mut git_chunk_clicked = None;
        let mut clicked_web_link = None;
        let tooltip_request = self.tooltip_request;
        let mut hovered_semantic_token = None;
        let mut hovered_asset_literal = false;
        let mut popup_request = None;
        let mut completion_cursor: Option<usize> = None;
        let mut completion_anchor: Option<Rect> = None;
        let mut editor_has_focus = false;
        let editor_base_slot = ui.painter().add(egui::Shape::Noop);
        let editor_margin = egui::Margin {
            left: gutter_width,
            right: theme::SPACE.small as i8,
            top: theme::SPACE.tight as i8,
            bottom: theme::SPACE.tight as i8,
        };

        let scroll_area = egui::ScrollArea::new([!line_wrap, true])
            .id_salt(("source-editor-scroll", active_tab))
            .auto_shrink([false, false]);
        let scroll_area = if let Some(offset) = snapshot_scroll_offset {
            scroll_area.vertical_scroll_offset(offset)
        } else {
            scroll_area
        };
        let scroll_output = scroll_area.show(ui, |ui| {
            let editor_width = if line_wrap {
                ui.available_width()
                    .max(METRICS.editor.wrapped_minimum_width)
            } else {
                unwrapped_editor_width
            };
            let current_line_slot = ui.painter().add(egui::Shape::Noop);
            let background_slots = line_diagnostics
                .iter()
                .map(|_| ui.painter().add(egui::Shape::Noop))
                .collect::<Vec<_>>();
            let delimiter_fill_slot = ui.painter().add(egui::Shape::Noop);
            let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                let mut job = if document_kind.is_typst() {
                    highlighter.highlight(buffer.as_str(), dark_mode, generic_highlighter)
                } else {
                    generic_highlighter.highlight(
                        buffer.as_str(),
                        highlight_path.as_deref(),
                        dark_mode,
                    )
                };
                apply_search_highlights(
                    &mut job,
                    &search_highlight_matches,
                    selected_search_match.as_ref(),
                    search_match_color(ui.ctx(), false),
                    search_match_color(ui.ctx(), true),
                );
                job.wrap.max_width = if line_wrap {
                    folding.text_wrap_width(wrap_width)
                } else {
                    f32::INFINITY
                };
                folding.layout(ui.fonts_mut(|fonts| fonts.layout_job(job)))
            };
            let document_before_edit = document.key();
            let mut output = document.edit(snapshot_before_edit.cursor, |source| {
                let mut buffer = ui.input(|input| {
                    crate::auto_pairs::PairingBuffer::new(
                        source,
                        auto_pair_syntax,
                        auto_pair_enabled,
                        &input.events,
                    )
                });
                let editor = egui::TextEdit::multiline(&mut buffer)
                    .id(source_editor_id(ui.ctx()))
                    .code_editor()
                    .desired_width(editor_width)
                    .min_size(Vec2::new(editor_width, 0.0))
                    // The surface is painted across the complete viewport
                    // below. Keep TextEdit's own frame empty so its intrinsic
                    // document height cannot leave an internal border behind.
                    .frame(egui::Frame::new().inner_margin(editor_margin))
                    .layouter(&mut layouter);
                editor.show(ui)
            });
            // A closer can move the caret without modifying the document.
            // Do not invalidate completions or schedule work for that movement.
            changed = document.key() != document_before_edit;
            if changed {
                self.editor_data.prepare_source(&document.snapshot());
            }
            if !changed && let Some(mut range) = output.state.cursor.char_range() {
                let vertical = ui.input(|input| {
                    if input.modifiers.alt || input.modifiers.command || input.modifiers.ctrl {
                        return None;
                    }
                    match (
                        input.key_pressed(egui::Key::ArrowUp),
                        input.key_pressed(egui::Key::ArrowDown),
                    ) {
                        (true, false) => Some(false),
                        (false, true) => Some(true),
                        _ => None,
                    }
                });
                if let Some(down) = vertical
                    && let Some(cursor) =
                        crate::folding::skip_hidden_row(&output.galley, range.primary, down)
                {
                    range.primary = cursor;
                    if !ui.input(|input| input.modifiers.shift) {
                        range.secondary = cursor;
                    }
                    output.state.cursor.set_char_range(Some(range));
                    output.state.clone().store(ui.ctx(), output.response.id);
                    ui.scroll_to_rect(
                        output
                            .galley
                            .pos_from_cursor(cursor)
                            .translate(output.galley_pos.to_vec2()),
                        None,
                    );
                    ui.ctx().request_repaint();
                }
            }
            let mut current_char = output
                .state
                .cursor
                .char_range()
                .map(|range| range.primary.index.0);
            let line_rows = logical_line_row_ranges(&output.galley.rows);
            if current_char.is_some_and(|cursor| folding.reveal(cursor)) {
                folds_changed = true;
                ui.ctx().request_repaint();
            }

            if let Some(selection) = pending_selection {
                let range = selection.range();
                let len = document.source().chars().count();
                let range = range.start.min(len)..range.end.min(len);
                let cursor_range =
                    CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end));
                output.state.cursor.set_char_range(Some(cursor_range));
                current_char = Some(range.end);
                output.state.clone().store(ui.ctx(), output.response.id);
                if selection.takes_focus() {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if let Some(webview) = self.webview.as_ref()
                        && let Err(error) = webview.focus_parent()
                    {
                        self.notice = Some(Notice {
                            message: format!("Could not focus the source editor: {error}"),
                            kind: NoticeKind::Error,
                        });
                    }
                    // A Tinymist source jump originates in the native preview
                    // child. Move native keyboard ownership back to the root
                    // before focusing TextEdit, otherwise macOS continues to
                    // deliver Option+Arrow word navigation to the WebView.
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                    output.response.request_focus();
                }
                let cursor_rect = output
                    .galley
                    .pos_from_cursor(CCursor::new(range.start))
                    .translate(output.galley_pos.to_vec2());
                ui.scroll_to_rect(cursor_rect, Some(Align::Center));
            }

            editor_has_focus = output.response.has_focus();
            if let Some(range) = output.state.cursor.char_range()
                && range.primary.index == range.secondary.index
            {
                let cursor = range.primary.index.0;
                completion_cursor = Some(cursor);
                completion_anchor = Some(
                    output
                        .galley
                        .pos_from_cursor(CCursor::new(cursor))
                        .translate(output.galley_pos.to_vec2()),
                );
            }

            paint_editor_line_backgrounds(
                ui,
                &output,
                document.source(),
                current_char,
                attention,
                &line_rows,
                current_line_slot,
            );
            if document_kind.is_typst()
                && editor_has_focus
                && let Some(range) = output.state.cursor.char_range()
                && range.is_empty()
                && let Some(pair) = self.editor_data.matching_delimiters(range.primary.index.0)
            {
                let accent = theme::palette(ui.ctx()).accent;
                let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 48);
                let mut backgrounds = Vec::new();
                let painter = ui
                    .painter()
                    .with_clip_rect(output.response.rect.intersect(ui.clip_rect()));
                for endpoint in pair {
                    for rect in delimiter_rects(&output.galley, endpoint) {
                        let rect = rect.translate(output.galley_pos.to_vec2());
                        backgrounds.push(egui::Shape::rect_filled(rect, 2, fill));
                        painter.rect_stroke(rect, 2, Stroke::new(1.0, accent), StrokeKind::Inside);
                    }
                }
                painter.set(delimiter_fill_slot, egui::Shape::Vec(backgrounds));
            }
            if let Some(tooltip) =
                paint_line_diagnostics(ui, &output, &line_diagnostics, &line_rows, background_slots)
                && !native_tooltip_handoff_blocks(ui.ctx(), tooltip.origin)
            {
                // Keep the last diagnostic payload while the pointer is
                // crossing the root-local bridge or sitting in the
                // native child. The root is deliberately no longer
                // hovering the diagnostic row in that frame, but the
                // popup still needs the payload in order to repaint and
                // accept scroll input.
                self.diagnostic_tooltip = Some(tooltip);
            }
            if line_numbers {
                paint_line_numbers(ui, &output, &line_rows, theme::editor_font());
                let gutter_clicked =
                    paint_fold_controls(ui, &output, &line_rows, folding, git_gutter);
                let marker_clicked = paint_fold_markers(
                    ui,
                    &output,
                    &line_rows,
                    folding,
                    &fold_marker,
                    fold_marker_width,
                );
                if let Some(region) = gutter_clicked.or(marker_clicked) {
                    output.response.request_focus();
                    folding.toggle(region.line);
                    if folding.is_collapsed(region.line) {
                        output
                            .state
                            .cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(region.header))));
                        output.state.clone().store(ui.ctx(), output.response.id);
                    }
                    folds_changed = true;
                    // A discarded pass would replay this click and toggle
                    // back. Apply the new geometry on the next input frame.
                    ui.ctx().request_repaint();
                }
            }
            if git_gutter {
                let logical_rows = line_rows
                    .iter()
                    .filter_map(|rows| {
                        let first = output.galley.rows.get(rows.start)?.rect();
                        let last = output.galley.rows.get(rows.end.checked_sub(1)?)?.rect();
                        Some(first.union(last).translate(output.galley_pos.to_vec2()))
                    })
                    .collect::<Vec<_>>();
                let marker_action = crate::git::editor::view::show_markers(
                    ui,
                    &self.git_editor.hunks,
                    &logical_rows,
                    output.response.rect.left(),
                );
                // The markers may be stale in the frame that accepted an
                // edit, so keep painting them for continuity but do not open
                // a chunk from that frame's old line mapping.
                if !changed {
                    git_chunk_clicked = marker_action;
                }
            }

            let requested_caret = match tooltip_request {
                Some(TooltipRequest::Caret { key, cursor }) if key == document.key() => {
                    Some(cursor)
                }
                _ => None,
            };
            if let Some(cursor) = requested_caret {
                self.diagnostic_tooltip = None;
                let line = scalar_position_at(document.source(), ScalarOffset::new(cursor))
                    .0
                    .get() as usize
                    + 1;
                if let Some(diagnostic) = line_diagnostics
                    .iter()
                    .find(|diagnostic| diagnostic.line == line)
                {
                    let rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(cursor))
                        .translate(output.galley_pos.to_vec2());
                    self.diagnostic_tooltip = Some(DiagnosticTooltipOverlay {
                        origin: rect,
                        anchor: rect.left_bottom(),
                        detail: diagnostic.detail.clone(),
                        severity: diagnostic.severity,
                    });
                }
            }
            let hover_position = requested_caret
                .map(|cursor| {
                    let rect = output
                        .galley
                        .pos_from_cursor(CCursor::new(cursor))
                        .translate(output.galley_pos.to_vec2());
                    (cursor, rect.center())
                })
                .or_else(|| {
                    output
                        .response
                        .hovered()
                        .then(|| ui.ctx().pointer_hover_pos())
                        .flatten()
                        .map(|pointer| {
                            (
                                output
                                    .galley
                                    .cursor_from_pos(pointer - output.galley_pos)
                                    .index
                                    .0,
                                pointer,
                            )
                        })
                });
            if document_kind.is_typst()
                && let Some((char_index, pointer)) = hover_position
            {
                let asset_target = asset_source_path.as_deref().and_then(|source_path| {
                    self.editor_data.literal_asset_target_at(
                        char_index,
                        source_path,
                        &asset_workspace_root,
                    )
                });
                if let Some(target) = asset_target
                    && target.literal_range.contains(&char_index)
                {
                    let literal_rect = editor_char_range_rect(&output, &target.literal_range);
                    let origin = literal_rect.expand(theme::SPACE.tight);
                    if origin.contains(pointer) && !native_tooltip_handoff_blocks(ui.ctx(), origin)
                    {
                        let response = ui.interact(
                            origin,
                            output.response.id.with((
                                "asset-hover",
                                target.literal_range.start,
                                target.literal_range.end,
                            )),
                            Sense::hover(),
                        );
                        let kind = match target.asset_kind {
                            PreviewAssetKind::Image => DocumentKind::Image,
                            PreviewAssetKind::Pdf => DocumentKind::Pdf,
                        };
                        if requested_caret.is_some() {
                            let id = asset_hover_candidate_id(ui.ctx());
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(
                                    id,
                                    AssetHoverCandidate {
                                        origin,
                                        anchor: origin.left_bottom(),
                                        placement: TooltipPlacement::Below,
                                        path: target.resolved_path,
                                        kind,
                                        opacity: 1.0,
                                    },
                                )
                            });
                        } else {
                            offer_asset_hover(
                                &response,
                                origin,
                                target.resolved_path,
                                kind,
                                TooltipPlacement::Below,
                            );
                        }
                        hovered_asset_literal = true;
                    }
                } else if let Some(range) = self.editor_data.hover_token_range(char_index) {
                    let start = output
                        .galley
                        .pos_from_cursor(CCursor::new(range.start))
                        .translate(output.galley_pos.to_vec2());
                    let end = output
                        .galley
                        .pos_from_cursor(CCursor::new(range.end))
                        .translate(output.galley_pos.to_vec2());
                    let token_rect = Rect::from_min_max(
                        start.left_top(),
                        Pos2::new(end.left().max(start.left() + 1.0), start.bottom()),
                    );
                    if requested_caret.is_some()
                        || token_rect.expand(theme::SPACE.tight).contains(pointer)
                    {
                        hovered_semantic_token = Some((range, token_rect));
                    }
                }
            }

            if document_kind.is_typst()
                && output.response.hovered()
                && ui.input(|input| input.modifiers.command)
                && let Some(pointer) = ui.ctx().pointer_hover_pos()
            {
                let char_index = output
                    .galley
                    .cursor_from_pos(pointer - output.galley_pos)
                    .index
                    .0;
                if editor_web_link_at(&mut self.editor_data, char_index).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }

            if output.response.secondary_clicked() {
                let anchor = ui
                    .ctx()
                    .pointer_latest_pos()
                    .unwrap_or_else(|| output.response.rect.center());
                let char_index = output.response.interact_pointer_pos().map(|pointer| {
                    output
                        .galley
                        .cursor_from_pos(pointer - output.galley_pos)
                        .index
                        .0
                });
                let target =
                    char_index.and_then(|char_index| self.editor_data.font_argument_at(char_index));
                let link = char_index
                    .and_then(|char_index| editor_web_link_at(&mut self.editor_data, char_index));
                let table = (document_kind.is_typst())
                    .then(|| {
                        char_index
                            .and_then(|char_index| editable_table_at(document.source(), char_index))
                    })
                    .flatten();
                popup_request = Some(match target {
                    Some(target) => AppPopup::FontSelector { anchor, target },
                    None => AppPopup::Editor {
                        anchor,
                        link,
                        table,
                    },
                });
            }

            let clicked_link = if document_kind.is_typst() {
                output.response.interact_pointer_pos().and_then(|pointer| {
                    let char_index = output
                        .galley
                        .cursor_from_pos(pointer - output.galley_pos)
                        .index
                        .0;
                    editor_web_link_click_target(
                        &mut self.editor_data,
                        char_index,
                        output.response.clicked(),
                        ui.input(|input| input.modifiers.command),
                    )
                })
            } else {
                None
            };
            clicked_web_link = clicked_link.clone();

            let jump_gesture = source_preview_jump_gesture(
                source_preview_trigger,
                output.response.clicked(),
                output.response.double_clicked(),
                ui.input(|input| input.modifiers.command),
                clicked_link.is_some(),
            );
            if preview_jump_enabled
                && jump_gesture
                && let Some(pointer) = output.response.interact_pointer_pos()
            {
                preview_jump_char = Some(
                    output
                        .galley
                        .cursor_from_pos(pointer - output.galley_pos)
                        .index
                        .0,
                );
            }

            let visuals = *ui.style().interact(&output.response);
            let border = if output.response.has_focus() {
                ui.visuals().selection.stroke
            } else {
                visuals.bg_stroke
            };
            let sticky_context = sticky_context_enabled.then(|| {
                let scroll_lines = sticky_context_scroll_lines(
                    &line_rows,
                    |index| {
                        output
                            .galley
                            .rows
                            .get(index)
                            .map(|row| row.rect().top() + output.galley_pos.y)
                    },
                    |index| {
                        output
                            .galley
                            .rows
                            .get(index)
                            .map(|row| row.char_count_including_newline().0)
                    },
                    |index| {
                        output
                            .galley
                            .rows
                            .get(index)
                            .map(|row| row.ends_with_newline)
                    },
                )
                .unwrap_or_default();
                StickyContextEditorSnapshot {
                    galley: Arc::clone(&output.galley),
                    galley_pos: output.galley_pos,
                    line_rows,
                    scroll_lines,
                }
            });
            (
                output.response.rect,
                visuals.corner_radius,
                border,
                sticky_context,
            )
        });

        let (editor_rect, corner_radius, border, sticky_context) = scroll_output.inner;
        // Covers scrollbar drags and keyboard/programmatic scrolling too,
        // which need not deliver a wheel event to the parent viewport.
        if source_scroll_changed(ui.ctx(), scroll_output.state.offset) || folds_changed {
            self.dismiss_hover_on_scroll(ui.ctx());
            hovered_semantic_token = None;
        }
        let surface_rect = editor_surface_rect(editor_rect, scroll_output.inner_rect);
        ui.painter().set(
            editor_base_slot,
            egui::Shape::rect_filled(
                surface_rect,
                corner_radius,
                ui.visuals().text_edit_bg_color(),
            ),
        );
        ui.painter()
            .rect_stroke(surface_rect, corner_radius, border, StrokeKind::Inside);

        if hovered_asset_literal {
            self.diagnostic_tooltip = None;
            self.clear_editor_hover();
        } else if self.diagnostic_tooltip.is_none() {
            self.update_editor_hover(ui, hovered_semantic_token);
        } else {
            self.clear_editor_hover();
        }

        let filler_top = editor_rect.bottom().clamp(
            scroll_output.inner_rect.top(),
            scroll_output.inner_rect.bottom(),
        );
        if filler_top < scroll_output.inner_rect.bottom() {
            let filler_rect = Rect::from_min_max(
                Pos2::new(scroll_output.inner_rect.left(), filler_top),
                scroll_output.inner_rect.right_bottom(),
            );
            let filler = ui.interact(
                filler_rect,
                source_editor_id(ui.ctx()).with("viewport-filler"),
                Sense::click(),
            );
            if filler.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            if filler.clicked() {
                let editor_id = source_editor_id(ui.ctx());
                let mut state =
                    egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(
                        self.document().source().chars().count(),
                    ))));
                state.store(ui.ctx(), editor_id);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(editor_id));
            }
        }

        if let Some(popup) = popup_request {
            self.open_app_popup(popup);
        }

        if let Some(crate::git::editor::view::Action::OpenChunk(index)) = git_chunk_clicked
            && let Some(path) = self.document().path().clone()
        {
            self.git_editor.open_chunk(index, &path);
            if let Some(chunk) = self.git_editor.chunk.clone() {
                let anchor = ui
                    .ctx()
                    .pointer_latest_pos()
                    .unwrap_or_else(|| ui.max_rect().left_top());
                self.open_app_popup(AppPopup::GitChunk { anchor, chunk });
            }
            ui.ctx().request_repaint();
        }

        let previous_completion = if changed {
            self.editor_completion.take()
        } else {
            None
        };
        if changed {
            self.find_bar.search.clear();
            self.mark_edited();
        }
        if let (Some(cursor), Some(anchor)) = (completion_cursor, completion_anchor) {
            let key = self.document().key();
            self.last_editor_caret = Some(EditorCaretState {
                key,
                char_index: cursor,
                rect: anchor,
            });
            if changed
                && completion_edit_triggered
                && let Some(mut previous) = previous_completion
                && (previous.local || self.document().config().is_none())
                && let Some(items) = crate::completion::rebase(
                    &previous.all_items,
                    &previous.source,
                    previous.cursor,
                    self.document().source(),
                    cursor,
                )
            {
                previous.all_items = items;
                previous.items = crate::completion::filtered_for_source(
                    &previous.all_items,
                    self.document().source(),
                    cursor,
                );
                previous.source = self.document().source().clone();
                previous.cursor = cursor;
                previous.source_cursor = cursor;
                previous.version = revision_as_i32(self.document().revision());
                previous.key = self.document().key();
                previous.selected = 0;
                self.editor_completion = Some(previous);
            }
            let completion_still_current = self.editor_completion.as_ref().is_none_or(|state| {
                state.cursor == cursor
                    && state.version == revision_as_i32(self.document().revision())
                    && state.key == self.document().key()
                    && (state.local
                        || (self.tinymist_sync.generation == Some(state.generation)
                            && self.tinymist_sync.current_uri.as_deref()
                                == Some(state.uri.as_str())))
            });
            if completion_still_current {
                if let Some(state) = &mut self.editor_completion {
                    state.anchor = anchor;
                }
            } else {
                self.editor_completion = None;
            }
            if changed && completion_edit_triggered && editor_has_focus {
                self.request_editor_completion(cursor, anchor, false);
            }
        } else if editor_has_focus {
            self.last_editor_caret = None;
            self.editor_completion = None;
        }
        self.show_editor_completion_popup(ui.ctx(), scroll_output.inner_rect);
        if let Some(char_index) = preview_jump_char {
            self.jump_source_to_preview(char_index);
        }
        if let Some(target) = clicked_web_link {
            self.follow_preview_link(&target);
        }

        let sticky_context_rows = sticky_context.as_ref().and_then(|sticky_context| {
            let geometry = sticky_context_overlay_geometry(scroll_output.inner_rect, None)?;
            let query = self.editor_data.sticky_context_query();
            sticky_context_rows_for_snapshot(
                &query,
                sticky_context,
                geometry.anchor.y,
                geometry.max_height,
            )
        });
        if let (Some(sticky_context), Some(rows)) = (sticky_context, sticky_context_rows)
            && let Some(target) = show_sticky_context_overlay(
                ui.ctx(),
                scroll_output.inner_rect,
                None,
                &rows,
                &sticky_context,
                line_numbers,
            )
        {
            self.pending_editor_selection = Some(EditorSelection::Focus(target..target));
            self.editor_completion = None;
            let editor_id = source_editor_id(ui.ctx());
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(editor_id));
            ui.ctx().request_repaint();
        }

        // Paint Find/Replace after the sticky rows so it remains the topmost
        // editor overlay without reserving any layout space below it.
        if self.find_bar.visible {
            let context = ui.ctx().clone();
            let overlay_width = (scroll_output.inner_rect.width() - 4.0 * theme::SPACE.content)
                .clamp(1.0, METRICS.editor.find_overlay_max_width);
            let anchor = scroll_output.inner_rect.left_top()
                + egui::vec2(theme::SPACE.content, theme::SPACE.content);
            egui::Area::new(viewport_scoped_id(&context, "find-replace-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(anchor)
                .constrain_to(ui.clip_rect())
                .show(&context, |ui| {
                    theme::popup_card_frame(ui.style()).show(ui, |ui| {
                        ui.set_max_width(overlay_width);
                        self.show_find_bar(ui);
                    });
                });
        }
    }

    pub(super) fn show_editor_completion_popup(&mut self, context: &egui::Context, viewport: Rect) {
        use super::completion_popup::{self, CompletionAction, CompletionPopupInput};
        let Some(completion) = self.editor_completion.as_ref() else {
            return;
        };
        if completion.items.is_empty() || !viewport.is_positive() {
            return;
        }
        let selected = completion.selected.min(completion.items.len() - 1);
        let preview_family = completion
            .local
            .then(|| {
                self.font_catalog
                    .families()
                    .iter()
                    .find(|family| family.name == completion.items[selected].label)
            })
            .flatten();
        let mut pending_font_sample = false;
        let action = completion_popup::show(
            context,
            viewport,
            CompletionPopupInput {
                items: &completion.items,
                source: &completion.source,
                source_cursor: completion.source_cursor,
                selected,
                is_incomplete: completion.is_incomplete,
                anchor: completion.anchor,
                has_footer: preview_family.is_some(),
            },
            |ui| {
                if let Some(family) = preview_family {
                    pending_font_sample = !crate::font_preview::show(ui, "completion", family);
                }
            },
        );
        if pending_font_sample && self.snapshot_scene == Some(UiSnapshotScene::FontCompletion) {
            self.captures.defer_target("main");
        }
        match action {
            Some(CompletionAction::Select(index)) => {
                if let Some(completion) = &mut self.editor_completion {
                    completion.selected = index;
                }
            }
            Some(CompletionAction::Accept(index)) => self.apply_editor_completion(index, context),
            Some(CompletionAction::Dismiss) => self.editor_completion = None,
            None => {}
        }
    }
}

fn search_match_color(context: &egui::Context, selected: bool) -> Color32 {
    let accent = theme::palette(context).accent;
    let alpha = if selected { 96 } else { 48 };
    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha)
}

/// Use glyph advances rather than a cursor rectangle spanning a soft wrap.
/// Raw delimiters may contain multiple backticks and can cross visual rows.
pub(super) fn delimiter_rects(galley: &egui::Galley, characters: Range<usize>) -> Vec<Rect> {
    let mut rectangles: Vec<Rect> = Vec::new();
    for character in characters {
        let mut cursor = CCursor::new(character);
        cursor.prefer_next_row = true;
        let position = galley.layout_from_cursor(cursor);
        let Some(row) = galley.rows.get(position.row) else {
            continue;
        };
        if row.size.y == 0.0 || position.column.0 >= row.glyphs.len() {
            continue;
        }
        let start = row.pos.x + row.x_offset(position.column);
        let end = row.pos.x + row.x_offset(egui::text::CharIndex(position.column.0 + 1));
        let rect = Rect::from_min_max(
            Pos2::new(start, row.min_y()),
            Pos2::new(end.max(start + 1.0), row.max_y()),
        );
        if let Some(previous) = rectangles.last_mut()
            && previous.top() == rect.top()
        {
            *previous = previous.union(rect);
        } else {
            rectangles.push(rect);
        }
    }
    rectangles
}

pub(super) fn apply_search_highlights(
    job: &mut LayoutJob,
    matches: &[Range<usize>],
    selected: Option<&Range<usize>>,
    match_color: Color32,
    selected_color: Color32,
) {
    if matches.is_empty() || job.sections.is_empty() {
        return;
    }

    let sections = std::mem::take(&mut job.sections);
    let mut highlighted = Vec::with_capacity(sections.len() + matches.len());
    let mut first_match = 0;
    for section in sections {
        let section_start = section.byte_range.start.0;
        let section_end = section.byte_range.end.0;
        while first_match < matches.len() && matches[first_match].end <= section_start {
            first_match += 1;
        }
        let mut cursor = section_start;
        for matched in matches[first_match..]
            .iter()
            .take_while(|matched| matched.start < section_end)
        {
            let match_start = matched.start.max(section_start).max(cursor);
            let match_end = matched.end.min(section_end);
            if match_start >= match_end {
                continue;
            }
            if cursor < match_start {
                highlighted.push(LayoutSection {
                    leading_space: if cursor == section_start {
                        section.leading_space
                    } else {
                        0.0
                    },
                    byte_range: ByteIndex(cursor)..ByteIndex(match_start),
                    format: section.format.clone(),
                });
            }
            let mut format = section.format.clone();
            format.background = if selected.is_some_and(|current| current == matched) {
                selected_color
            } else {
                match_color
            };
            highlighted.push(LayoutSection {
                leading_space: if cursor == section_start && cursor == match_start {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(match_start)..ByteIndex(match_end),
                format,
            });
            cursor = match_end;
        }
        if cursor < section_end {
            highlighted.push(LayoutSection {
                leading_space: if cursor == section_start {
                    section.leading_space
                } else {
                    0.0
                },
                byte_range: ByteIndex(cursor)..ByteIndex(section_end),
                format: section.format,
            });
        }
    }
    job.sections = highlighted;
}
