//! Source editor widgets and completion rendering.
use super::*;

impl EditorApp {
    pub(super) fn show_find_bar(&mut self, ui: &mut egui::Ui) {
        let mut find_next = false;
        let mut find_previous = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let search_revision = self.document.key();
        let search_results = self.search.results(
            self.document.source(),
            search_revision,
            &self.find_query,
            self.find_case_sensitive,
            self.find_regex,
        );
        let match_status = search_results.error().map_or_else(
            || format!("{} matches", search_results.len()),
            |_| "Invalid pattern".to_owned(),
        );

        ui.horizontal_wrapped(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.find_query)
                    .id_salt("find-query")
                    .hint_text("Find")
                    .desired_width(METRICS.editor.find_field_width),
            );
            if self.focus_find {
                response.request_focus();
                self.focus_find = false;
            }
            if response.changed() {
                self.search.clear();
            }
            if response.lost_focus()
                && let Some(step) = ui.input(|input| {
                    find_step_for_enter(input.key_pressed(egui::Key::Enter), input.modifiers.shift)
                })
            {
                match step {
                    FindStep::Next => find_next = true,
                    FindStep::Previous => find_previous = true,
                }
                // Single-line TextEdit relinquishes focus on Enter. Return it
                // immediately so repeated Enter/Shift+Enter keeps navigating.
                response.request_focus();
            }
            ui.label(
                RichText::new(match_status)
                    .size(theme::TYPE.supporting)
                    .weak(),
            );
            let case_label = if self.find_case_sensitive { "Aa" } else { "aa" };
            if native_hover_text(
                ui.selectable_label(self.find_case_sensitive, case_label),
                if self.find_case_sensitive {
                    "Case-sensitive matching"
                } else {
                    "Case-insensitive matching"
                },
            )
            .clicked()
            {
                self.find_case_sensitive = !self.find_case_sensitive;
                self.search.clear();
            }
            let regex_button = native_hover_text(
                ui.selectable_label(self.find_regex, ".*"),
                "Regular expression mode. Supports ., *, +, ?, [], ^, $, \\d, \\w, and \\s.",
            );
            regex_button.context_menu(|ui| {
                ui.label(RichText::new("Regular expressions").strong());
                ui.label(".  any character");
                ui.label("*  zero or more · +  one or more");
                ui.label("?  optional · ^  start · $  end");
                ui.label("[abc] [a-z] [^0-9]  character classes");
                ui.label("\\d digit · \\w word · \\s whitespace");
                ui.label("Replacement text is literal; capture expansion is not supported.");
            });
            if regex_button.clicked() {
                self.find_regex = !self.find_regex;
                self.search.clear();
            }
            if ui
                .selectable_label(self.replace_visible, "Replace")
                .on_hover_text("Show or hide replace fields")
                .clicked()
            {
                self.replace_visible = !self.replace_visible;
            }
            find_previous |= icon_button(ui, UiIcon::Up, "Previous match · Shift+Enter").clicked();
            find_next |= icon_button(ui, UiIcon::Down, "Next match · Enter").clicked();
            if icon_button(ui, UiIcon::Close, "Close · Esc").clicked() {
                self.find_visible = false;
                self.replace_visible = false;
                self.search.clear();
            }
        });

        if self.replace_visible {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .hint_text("Replace")
                        .desired_width(METRICS.editor.find_field_width),
                );
                replace_one |= ui.button("Replace").clicked();
                replace_all |= ui.button("All").clicked();
            });
        }

        if find_previous {
            self.pending_editor_selection = self
                .search
                .previous(
                    self.document.source(),
                    search_revision,
                    &self.find_query,
                    self.find_case_sensitive,
                    self.find_regex,
                )
                .map(|matched| matched.char_range.clone());
        }
        if find_next {
            self.pending_editor_selection = self
                .search
                .next(
                    self.document.source(),
                    search_revision,
                    &self.find_query,
                    self.find_case_sensitive,
                    self.find_regex,
                )
                .map(|matched| matched.char_range.clone());
        }
        if replace_one {
            let before = self.document.source().clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let replaced = self.document.edit(snapshot.cursor, |source| {
                self.search.replace_one(
                    source,
                    search_revision,
                    &self.find_query,
                    &self.replacement,
                    self.find_case_sensitive,
                    self.find_regex,
                )
            });
            if replaced {
                self.pending_editor_selection = self
                    .search
                    .selected()
                    .map(|matched| matched.char_range.clone());
                if *self.document.source() != before {
                    self.mark_edited();
                }
            }
        }
        if replace_all {
            let before = self.document.source().clone();
            let snapshot = self.editor_snapshot(ui.ctx());
            let count = self.document.edit(snapshot.cursor, |source| {
                self.search.replace_all(
                    source,
                    search_revision,
                    &self.find_query,
                    &self.replacement,
                    self.find_case_sensitive,
                    self.find_regex,
                )
            });
            if count > 0 && *self.document.source() != before {
                self.notice = Some(Notice {
                    message: format!("Replaced {count} matches"),
                    kind: NoticeKind::Success,
                });
                self.mark_edited();
            }
        }
    }

    pub(super) fn show_editor(&mut self, ui: &mut egui::Ui) {
        offer_file_drop_target(ui, ui.max_rect(), FileDropTarget::Editor);
        ui.set_min_width(ui.available_width());
        if self.document.reset_editor_history {
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), source_editor_id(ui.ctx()))
                    .unwrap_or_default();
            state.clear_undoer();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(0))));
            state.store(ui.ctx(), source_editor_id(ui.ctx()));
            self.document.reset_editor_history = false;
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
        let line_wrap = sticky_context_snapshot || self.settings.line_wrap;
        let line_numbers = sticky_context_snapshot || self.settings.line_numbers;
        let git_gutter = self.git_editor.has_gutter(self.document.path().as_deref());
        let gutter_width = line_number_gutter_width(line_count, line_numbers)
            + if git_gutter {
                crate::git::editor::GUTTER_WIDTH
            } else {
                0
            };
        let dark_mode = ui.visuals().dark_mode;
        let document_kind = self.document.kind();
        let highlight_path = self.document.path().clone();
        let asset_source_path = self.document.path().clone();
        let asset_workspace_root = self.workspace_root.clone();
        let source_preview_trigger = self.settings.source_preview_trigger;
        let preview_jump_enabled = document_kind.is_typst() && self.interactive_preview_active();
        let sticky_context_enabled = document_kind.is_typst()
            && (sticky_context_snapshot || self.settings.sticky_context_rows);
        let snapshot_scroll_offset = source_editor_snapshot_scroll_offset(self.snapshot_scene);
        let completion_edit_triggered = document_kind.is_typst()
            && ui.input(|input| completion_requested_after_events(&input.events));
        let snapshot_before_edit = self.editor_snapshot(ui.ctx());
        let highlighter = &mut self.highlighter;
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
            .id_salt("source-editor-scroll")
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
                job.wrap.max_width = if line_wrap { wrap_width } else { f32::INFINITY };
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };
            let mut output = self.document.edit(snapshot_before_edit.cursor, |source| {
                let editor = egui::TextEdit::multiline(source)
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
            changed = output.response.changed();
            if changed {
                self.editor_data.prepare_source(&self.document.snapshot());
            }
            let mut current_char = output
                .state
                .cursor
                .char_range()
                .map(|range| range.primary.index.0);
            let line_rows = logical_line_row_ranges(&output.galley.rows);

            if let Some(range) = pending_selection {
                let len = self.document.source().chars().count();
                let range = range.start.min(len)..range.end.min(len);
                let cursor_range =
                    CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end));
                output.state.cursor.set_char_range(Some(cursor_range));
                current_char = Some(range.end);
                output.state.clone().store(ui.ctx(), output.response.id);
                if !self.find_visible {
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
                self.document.source(),
                current_char,
                attention,
                &line_rows,
                current_line_slot,
            );
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
                paint_line_numbers(ui, &output, &line_rows);
            }
            if !changed && git_gutter {
                let logical_rows = line_rows
                    .iter()
                    .filter_map(|rows| {
                        let first = output.galley.rows.get(rows.start)?.rect();
                        let last = output.galley.rows.get(rows.end.checked_sub(1)?)?.rect();
                        Some(first.union(last).translate(output.galley_pos.to_vec2()))
                    })
                    .collect::<Vec<_>>();
                git_chunk_clicked = crate::git::editor::show_markers(
                    ui,
                    &self.git_editor.hunks,
                    &logical_rows,
                    output.response.rect.left(),
                );
            }

            let requested_caret = match tooltip_request {
                Some(TooltipRequest::Caret { key, cursor }) if key == self.document.key() => {
                    Some(cursor)
                }
                _ => None,
            };
            if let Some(cursor) = requested_caret {
                self.diagnostic_tooltip = None;
                let line = scalar_position_at(self.document.source(), ScalarOffset::new(cursor))
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
                        opacity: 1.0,
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
                    literal_asset_target_at(
                        self.document.source(),
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
                } else if let Some(range) =
                    typst_hover_token_range(self.document.source(), char_index)
                {
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
                        char_index.and_then(|char_index| {
                            editable_table_at(self.document.source(), char_index)
                        })
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
            self.editor_hover = None;
        } else if self.diagnostic_tooltip.is_none() {
            self.update_editor_hover(ui, hovered_semantic_token);
        } else {
            self.editor_hover = None;
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
                        self.document.source().chars().count(),
                    ))));
                state.store(ui.ctx(), editor_id);
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(editor_id));
            }
        }

        if let Some(popup) = popup_request {
            self.open_app_popup(popup);
        }

        if let Some(index) = git_chunk_clicked
            && let Some(path) = &self.document.path()
        {
            self.git_editor.open_chunk(index, path);
            ui.ctx().request_repaint();
        }

        let previous_completion = if changed {
            self.editor_completion.take()
        } else {
            None
        };
        if changed {
            self.search.clear();
            self.mark_edited();
        }
        if let (Some(cursor), Some(anchor)) = (completion_cursor, completion_anchor) {
            let key = self.document.key();
            self.last_editor_caret = Some(EditorCaretState {
                key,
                char_index: cursor,
                rect: anchor,
            });
            if changed
                && completion_edit_triggered
                && let Some(mut previous) = previous_completion
                && let Some(items) = crate::completion::rebase(
                    &previous.all_items,
                    &previous.source,
                    previous.cursor,
                    self.document.source(),
                    cursor,
                )
            {
                previous.all_items = items;
                previous.items = crate::completion::filtered_for_source(
                    &previous.all_items,
                    self.document.source(),
                    cursor,
                );
                previous.source = self.document.source().clone();
                previous.cursor = cursor;
                previous.version = revision_as_i32(self.document.revision());
                previous.selected = 0;
                self.editor_completion = Some(previous);
            }
            let completion_still_current = self.editor_completion.as_ref().is_none_or(|state| {
                state.cursor == cursor
                    && state.version == revision_as_i32(self.document.revision())
                    && (state.local
                        || (self.tinymist_generation == Some(state.generation)
                            && self.tinymist_uri.as_deref() == Some(state.uri.as_str())))
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
            self.pending_editor_selection = Some(target..target);
            self.editor_completion = None;
            let editor_id = source_editor_id(ui.ctx());
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(editor_id));
            ui.ctx().request_repaint();
        }

        // Paint Find/Replace after the sticky rows so it remains the topmost
        // editor overlay without reserving any layout space below it.
        if self.find_visible {
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
        let Some(completion) = self.editor_completion.as_ref() else {
            return;
        };
        if completion.items.is_empty() || !viewport.is_positive() {
            return;
        }

        let items = completion.items.clone();
        let selected = completion.selected.min(items.len().saturating_sub(1));
        let is_incomplete = completion.is_incomplete;
        let anchor = completion.anchor;
        let popup_width = COMPLETION_POPUP_WIDTH.min((viewport.width() - 8.0).max(1.0));
        let frame = theme::popup_card_frame(&context.style_of(context.theme()));
        let margin = frame.total_margin().sum();
        let inner_width = (popup_width - margin.x).max(1.0);
        let list_height = (items.len() as f32 * (COMPLETION_ROW_HEIGHT + theme::SPACE.small)
            - theme::SPACE.small)
            .clamp(COMPLETION_ROW_HEIGHT, COMPLETION_POPUP_MAX_HEIGHT);
        let preview_family = completion
            .local
            .then(|| {
                self.font_catalog
                    .families()
                    .iter()
                    .find(|family| family.name == items[selected].label)
            })
            .flatten()
            .cloned();
        let footer_height =
            f32::from(preview_family.is_some()) * 36.0 + f32::from(is_incomplete) * 40.0;
        let desired_size = Vec2::new(popup_width, list_height + footer_height + margin.y);
        let position = completion_popup_position(anchor, desired_size, viewport);
        let mut clicked = None;
        let mut hovered = None;

        let popup = egui::Area::new(viewport_scoped_id(context, "editor-completion-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .constrain_to(viewport)
            .show(context, |ui| {
                frame.show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = theme::SPACE.small;
                    ui.set_min_width(inner_width);
                    ui.set_max_width(inner_width);
                    ui.set_height(list_height + footer_height);
                    egui::ScrollArea::vertical()
                        .id_salt("editor-completion-items")
                        .max_height(list_height)
                        .min_scrolled_height(0.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (index, item) in items.iter().enumerate() {
                                let label = crate::completion::display_label(
                                    item,
                                    self.document.source(),
                                    completion.cursor,
                                );
                                let mut response = ui.add_sized(
                                    [ui.available_width(), COMPLETION_ROW_HEIGHT],
                                    egui::Button::selectable(
                                        index == selected,
                                        RichText::new(label).monospace(),
                                    )
                                    .right_text("")
                                    .truncate(),
                                );
                                if let Some(documentation) = item.documentation.as_deref() {
                                    response = response.on_hover_text(documentation);
                                }
                                if response.hovered() {
                                    hovered = Some(index);
                                }
                                if response.clicked() {
                                    clicked = Some(index);
                                }
                                if index == selected {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                    if let Some(family) = &preview_family
                        && !crate::font_preview::show(ui, "completion", family)
                        && self.snapshot_scene == Some(UiSnapshotScene::FontCompletion)
                    {
                        self.captures.defer_target("main");
                    }
                    if is_incomplete {
                        ui.separator();
                        ui.label(
                            RichText::new("Keep typing for more suggestions")
                                .size(theme::TYPE.supporting)
                                .weak(),
                        );
                    }
                });
            });

        if let Some(index) = hovered
            && let Some(completion) = &mut self.editor_completion
        {
            completion.selected = index;
        }
        if let Some(index) = clicked {
            self.apply_editor_completion(index, context);
            return;
        }

        let clicked_outside = context.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pointer| !popup.response.rect.contains(pointer))
        });
        if clicked_outside {
            self.editor_completion = None;
        }
    }
}
