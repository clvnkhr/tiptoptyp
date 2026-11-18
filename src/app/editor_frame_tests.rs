//! Source layout must stay cached through real animation frames and invalidate safely.
use super::*;
use crate::settings::{ActionEffectSettings, EditorAction, ParticleStyle};
use egui_kittest::{Harness, kittest::Queryable as _};

fn fixture() -> Harness<'static, EditorApp> {
    let mut harness = editor_particles::tests::editor(false);
    let source = format!(
        "prefix \n#let amplitude(x) = {{\n  let y = x + 1\n  y * 2\n}}\n\nneedle {} tail\n",
        "ordinary prose with a sufficiently long wrapped line ".repeat(30),
    );
    let app = harness.state_mut();
    app.settings.line_wrap = true;
    app.settings.line_numbers = true;
    app.settings.sticky_context_rows = true;
    for action in EditorAction::ALL {
        *app.settings.editor_effects.actions.get_mut(action) = ActionEffectSettings {
            lingering: None,
            particles: None,
            shake: false,
            flash: false,
            glow: false,
        };
    }
    app.settings.editor_effects.actions.typing.particles = Some(ParticleStyle::Plasma);
    app.document_mut().replace_unprojected_untitled(&source);
    app.pending_editor_selection = Some(EditorSelection::Focus(7..7));
    harness.run_steps(3);
    assert_current_text(&harness);
    harness
}

fn snapshot(harness: &Harness<'_, EditorApp>) -> editor_scroll::Snapshot {
    let id = source_editor_id(&harness.ctx).with("resize-anchor");
    harness
        .ctx
        .data(|data| data.get_temp::<editor_scroll::Snapshot>(id))
        .expect("production source editor publishes its painted galley")
}

fn assert_current_text(harness: &Harness<'_, EditorApp>) {
    let painted = snapshot(harness);
    let source = harness.state().document().source();
    assert_eq!(painted.galley.job.text, *source);
    assert_eq!(painted.galley.end().index.0, source.chars().count());
}

#[test]
fn matching_delimiters_paint_background_without_outlines_in_both_languages() {
    fn rectangles<'a>(shapes: &'a [egui::Shape], out: &mut Vec<&'a egui::epaint::RectShape>) {
        for shape in shapes {
            match shape {
                egui::Shape::Rect(rect) => out.push(rect),
                egui::Shape::Vec(children) => rectangles(children, out),
                _ => {}
            }
        }
    }
    for (kind, source) in [
        (DocumentKind::Typst, "#let value = (1, 2)"),
        (DocumentKind::Tex, "\\[x + y\\]"),
    ] {
        let mut harness = editor_particles::tests::editor(false);
        let app = harness.state_mut();
        app.settings.editor_effects.enabled = false;
        app.document_mut().replace_untitled_kind(kind);
        app.document_mut()
            .edit(CCursorRange::default(), |text| *text = source.into());
        let at = source
            .find(if kind == DocumentKind::Tex { '\\' } else { '(' })
            .unwrap();
        app.pending_editor_selection = Some(EditorSelection::Focus(at..at));
        harness.run_steps(3);
        let accent = theme::palette(&harness.ctx).accent;
        let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 48);
        for phase in ["endpoint", "body", "typing"] {
            if phase == "body" {
                let inside = source
                    .find(if kind == DocumentKind::Tex { 'x' } else { '1' })
                    .unwrap();
                harness.state_mut().pending_editor_selection =
                    Some(EditorSelection::Focus(inside..inside));
                harness.step();
            } else if phase == "typing" {
                harness
                    .input_mut()
                    .events
                    .push(egui::Event::Text("3".into()));
                harness.step();
                assert_ne!(harness.state().document().source(), source);
                assert_current_text(&harness);
            }
            let mut rects = Vec::new();
            for shape in &harness.output().shapes {
                rectangles(std::slice::from_ref(&shape.shape), &mut rects);
            }
            let backgrounds: Vec<_> = rects.iter().filter(|rect| rect.fill == fill).collect();
            assert_eq!(
                backgrounds.len(),
                2,
                "{kind:?}/{phase}: both endpoints have a background"
            );
            for background in backgrounds {
                assert!(
                    background.stroke.is_empty(),
                    "{kind:?}: background has no outline"
                );
                assert!(
                    !rects
                        .iter()
                        .any(|rect| rect.rect == background.rect && !rect.stroke.is_empty()),
                    "{kind:?}: no separate outline is painted over the endpoint"
                );
            }
        }
    }
}

#[test]
fn source_animation_frames_reuse_row_indexes_and_keep_paragraph_layouts_warm() {
    let mut harness = fixture();
    let before = snapshot(&harness);
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("x".into()));
    harness.step();
    assert!(
        harness
            .state()
            .document()
            .source()
            .starts_with("prefix x\n")
    );
    assert_current_text(&harness);
    assert!(!Arc::ptr_eq(&before.galley, &snapshot(&harness).galley));
    let effects = editor_particles::inspected(
        &harness.ctx,
        source_editor_id(&harness.ctx),
        harness.state().document().key(),
    );
    assert_eq!(effects["burst_count"], 1);
    assert!(effects["active_count"].as_u64().unwrap() > 0);
    // Settle any layout follow-up before measuring the still-live animation.
    harness.run_steps(2);
    let prepared = snapshot(&harness);
    let rows = harness.state_mut().source_rows.rows(&prepared.galley);
    harness.run_steps(12);
    assert_current_text(&harness);
    let current = snapshot(&harness);
    assert!(Arc::ptr_eq(&prepared.galley, &current.galley));
    let reused = harness.state_mut().source_rows.rows(&current.galley);
    assert!(Arc::ptr_eq(&rows.logical, &reused.logical));
    assert!(Arc::ptr_eq(&rows.sticky, &reused.sticky));
    let effects = editor_particles::inspected(
        &harness.ctx,
        source_editor_id(&harness.ctx),
        harness.state().document().key(),
    );
    assert_eq!(effects["burst_count"], 1);
    assert!(effects["active_count"].as_u64().unwrap() > 0);
    // A later edit must still reuse egui's unchanged paragraph layouts.
    let first_row = Arc::clone(&current.galley.rows[0].row);
    let end = harness.state().document().source().chars().count();
    harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(end..end));
    harness.step();
    harness
        .input_mut()
        .events
        .push(egui::Event::Text("y".into()));
    harness.step();
    assert_current_text(&harness);
    assert!(Arc::ptr_eq(
        &first_row,
        &snapshot(&harness).galley.rows[0].row
    ));
}

#[test]
fn source_layout_refreshes_after_a_font_atlas_reset_without_editor_settings_changes() {
    let mut harness = fixture();
    let old = snapshot(&harness);
    let font_data = harness.ctx.fonts(|fonts| {
        fonts
            .definitions()
            .font_data
            .values()
            .next()
            .expect("fixture has a valid font")
            .as_ref()
            .clone()
    });
    // An unused valid font resets egui's atlas without changing the selected
    // code font, source, wrap width, palette, or application settings.
    harness.ctx.add_font(egui::epaint::text::FontInsert::new(
        "source-layout-atlas-reset",
        font_data,
        Vec::new(),
    ));
    harness.step();
    assert_current_text(&harness);
    assert!(
        !Arc::ptr_eq(&old.galley, &snapshot(&harness).galley),
        "retained galleys must not keep texture coordinates from a reset atlas"
    );
}

#[test]
fn source_layout_refreshes_for_wrapping_search_rainbow_and_fold_geometry() {
    let mut harness = fixture();
    let wrapped = snapshot(&harness);
    harness.state_mut().settings.line_wrap = false;
    harness.step();
    let unwrapped = snapshot(&harness);
    assert!(unwrapped.galley.rows.len() < wrapped.galley.rows.len());
    assert_current_text(&harness);

    harness.state_mut().settings.line_wrap = true;
    harness.set_size(egui::vec2(410.0, 500.0));
    harness.step();
    assert!(snapshot(&harness).galley.rows.len() > wrapped.galley.rows.len());
    assert_current_text(&harness);

    let plain = snapshot(&harness);
    harness.state_mut().find_bar.visible = true;
    harness.state_mut().find_bar.query = "needle".into();
    harness.step();
    let searched = snapshot(&harness);
    assert_ne!(plain.galley.job.sections, searched.galley.job.sections);
    assert_current_text(&harness);
    harness.state_mut().find_bar.visible = false;
    harness.step();
    assert_ne!(
        searched.galley.job.sections,
        snapshot(&harness).galley.job.sections
    );

    let before_rainbow = snapshot(&harness);
    let old = harness.state().settings.rainbow_brackets.enabled;
    harness.state_mut().settings.rainbow_brackets.enabled = !old;
    harness.step();
    assert_ne!(
        before_rainbow.galley.job.sections,
        snapshot(&harness).galley.job.sections
    );
    assert_current_text(&harness);

    assert!(!harness.state_mut().folding_mut().regions.is_empty());
    harness.state_mut().folding_mut().collapse_all();
    harness.step();
    assert!(
        snapshot(&harness)
            .galley
            .rows
            .iter()
            .any(|row| row.size.y == 0.0)
    );
    assert_current_text(&harness);
    harness.state_mut().folding_mut().expand_all();
    harness.step();
    assert!(
        snapshot(&harness)
            .galley
            .rows
            .iter()
            .all(|row| row.size.y > 0.0)
    );
    assert_current_text(&harness);
}

fn source_selection(harness: &Harness<'_, EditorApp>) -> CCursorRange {
    egui::text_edit::TextEditState::load(&harness.ctx, source_editor_id(&harness.ctx))
        .expect("production source editor state")
        .cursor
        .char_range()
        .expect("production source editor selection")
}

#[test]
fn source_selection_survives_keyboard_handoff_native_blur_and_return() {
    for reversed in [false, true] {
        let mut harness = fixture();
        let editor = source_editor_id(&harness.ctx);
        let expected = CCursorRange {
            primary: CCursor::new(if reversed { 2 } else { 6 }),
            secondary: CCursor::new(if reversed { 6 } else { 2 }),
            ..Default::default()
        };
        harness.state().store_editor_cursor(&harness.ctx, expected);
        let source = harness.state().document().source().clone();
        let key = harness.state().document().key();
        // An outside click surrenders TextEdit's widget focus before a native
        // child (such as the Overleaf workspace chooser) takes window focus.
        harness
            .ctx
            .memory_mut(|memory| memory.surrender_focus(editor));
        harness.input_mut().focused = false;
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        for _ in 0..3 {
            harness.step();
            let actual = source_selection(&harness);
            assert_eq!(actual.primary.index, expected.primary.index);
            assert_eq!(actual.secondary.index, expected.secondary.index);
            assert!(!harness.ctx.memory(|memory| memory.has_focus(editor)));
            assert_eq!(harness.state().document().source(), &source);
            assert_eq!(harness.state().document().key(), key);
            assert!(harness.state().pending_editor_selection.is_none());
        }
        harness.input_mut().focused = true;
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        harness.run_steps(3);
        let actual = source_selection(&harness);
        assert_eq!(actual.primary.index, expected.primary.index);
        assert_eq!(actual.secondary.index, expected.secondary.index);
        assert!(!harness.ctx.memory(|memory| memory.has_focus(editor)));
        // Returning keyboard ownership must use the original selected text,
        // rather than silently inserting beside a collapsed caret.
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(editor));
        harness
            .input_mut()
            .events
            .push(egui::Event::Text("x".into()));
        harness.step();
        assert_eq!(
            harness.state().document().source(),
            &format!("{}x{}", &source[..2], &source[6..])
        );
    }
}

#[test]
fn deliberate_source_pointer_selection_replaces_a_retained_unfocused_selection() {
    let mut harness = fixture();
    let editor = source_editor_id(&harness.ctx);
    let expected = CCursorRange::two(CCursor::new(2), CCursor::new(6));
    harness.state().store_editor_cursor(&harness.ctx, expected);
    harness
        .ctx
        .memory_mut(|memory| memory.surrender_focus(editor));
    harness.step();
    let painted = snapshot(&harness);
    let caret = painted.galley.pos_from_cursor(CCursor::new(0));
    let position = painted.galley_pos + caret.center().to_vec2() + egui::vec2(0.5, 0.0);
    harness.event(egui::Event::PointerMoved(position));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    let actual = source_selection(&harness);
    assert_eq!(actual.primary.index.0, 0);
    assert_eq!(actual.secondary.index.0, 0);
    assert!(harness.ctx.memory(|memory| memory.has_focus(editor)));
}

fn locked_source_fixture() -> Harness<'static, EditorApp> {
    let mut harness = fixture();
    let context = harness.ctx.clone();
    harness.state_mut().begin_new_table(&context);
    assert!(harness.state().table_editor.is_some());
    harness.run_steps(2);
    harness
}

fn mutation_key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn locked_source_edit_attempts_preserve_exact_selection_and_source() {
    for reversed in [false, true] {
        let mut harness = locked_source_fixture();
        let expected = CCursorRange {
            primary: CCursor::new(if reversed { 2 } else { 6 }),
            secondary: CCursor::new(if reversed { 6 } else { 2 }),
            ..Default::default()
        };
        harness.state().store_editor_cursor(&harness.ctx, expected);
        let editor = source_editor_id(&harness.ctx);
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(editor));
        let source = harness.state().document().source().clone();
        let key = harness.state().document().key();
        let mut attempts = vec![
            egui::Event::Text("MUST REMAIN LOCKED".into()),
            egui::Event::Paste("MUST REMAIN LOCKED".into()),
            egui::Event::Cut,
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "日本".into(),
                active_range_chars: Some(0..2),
            }),
            egui::Event::Ime(egui::ImeEvent::Commit("日本。".into())),
            egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
                before_chars: 2,
                after_chars: 1,
            }),
        ];
        for key in [
            egui::Key::Backspace,
            egui::Key::Delete,
            egui::Key::Enter,
            egui::Key::Tab,
        ] {
            attempts.push(mutation_key(key, egui::Modifiers::NONE));
        }
        for key in [egui::Key::H, egui::Key::K, egui::Key::U, egui::Key::W] {
            attempts.push(mutation_key(key, egui::Modifiers::CTRL));
        }
        for key in [egui::Key::Z, egui::Key::Y] {
            attempts.push(mutation_key(key, egui::Modifiers::COMMAND));
        }
        for event in attempts {
            harness.input_mut().events.push(event.clone());
            harness.step();
            let actual = source_selection(&harness);
            assert_eq!(actual.primary.index, expected.primary.index, "{event:?}");
            assert_eq!(
                actual.secondary.index, expected.secondary.index,
                "{event:?}"
            );
            assert_eq!(harness.state().document().source(), &source);
            assert_eq!(harness.state().document().key(), key);
            harness.get_by_label(editor_read_only::MESSAGE);
            assert!(
                !harness.ctx.input(|input| input.events.contains(&event)),
                "rejected source edits must not reach a later widget"
            );
        }
    }
}

#[test]
fn locked_source_keeps_navigation_copy_and_pointer_selection_after_ignored_typing() {
    let mut harness = locked_source_fixture();
    let editor = source_editor_id(&harness.ctx);
    harness.state().store_editor_cursor(
        &harness.ctx,
        CCursorRange::two(CCursor::new(2), CCursor::new(6)),
    );
    harness
        .ctx
        .memory_mut(|memory| memory.request_focus(editor));
    let source = harness.state().document().source().clone();
    harness.input_mut().events.extend([
        egui::Event::Text("ignored".into()),
        mutation_key(egui::Key::ArrowRight, egui::Modifiers::SHIFT),
        egui::Event::Paste("also ignored".into()),
        egui::Event::Copy,
    ]);
    harness.step();
    let actual = source_selection(&harness);
    assert_eq!(actual.primary.index.0, 7);
    assert_eq!(actual.secondary.index.0, 2);
    assert!(
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == &source[2..7])
            })
    );
    let painted = snapshot(&harness);
    let caret = painted.galley.pos_from_cursor(CCursor::new(0));
    let position = painted.galley_pos + caret.center().to_vec2() + egui::vec2(0.5, 0.0);
    harness.event(egui::Event::PointerMoved(position));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
    }
    let actual = source_selection(&harness);
    assert_eq!(actual.primary.index.0, 0);
    assert_eq!(actual.secondary.index.0, 0);
    assert_eq!(harness.state().document().source(), &source);
    assert!(harness.ctx.memory(|memory| memory.has_focus(editor)));
}

#[test]
fn locking_source_retires_an_existing_ime_composition_before_unlock() {
    let mut harness = fixture();
    harness
        .state()
        .store_editor_cursor(&harness.ctx, CCursorRange::one(CCursor::new(2)));
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "日本".into(),
        active_range_chars: Some(0..2),
    }));
    harness.step();
    let source = harness.state().document().source().clone();
    assert!(source.starts_with("pr日本efix"));
    let selection = source_selection(&harness);
    let context = harness.ctx.clone();
    harness.state_mut().begin_new_table(&context);
    assert!(harness.state().table_editor.is_some());
    harness.step();
    assert_eq!(harness.state().document().source(), &source);
    assert_eq!(source_selection(&harness), selection);
    harness.state_mut().table_editor = None;
    harness.event(egui::Event::Ime(egui::ImeEvent::Commit(String::new())));
    harness.step();
    assert_eq!(harness.state().document().source(), &source);
    assert_eq!(source_selection(&harness), selection);
}

#[test]
fn read_only_edit_attempt_is_acknowledged_visibly_without_changing_source_or_selection() {
    let mut harness = locked_source_fixture();
    let editor = source_editor_id(&harness.ctx);
    let selection = CCursorRange::two(CCursor::new(2), CCursor::new(6));
    harness.state().store_editor_cursor(&harness.ctx, selection);
    harness
        .ctx
        .memory_mut(|memory| memory.request_focus(editor));
    let source = harness.state().document().source().clone();
    let key = harness.state().document().key();
    harness.event(egui::Event::Text("Rejected text".into()));
    harness.step();
    assert_eq!(harness.state().document().source(), &source);
    assert_eq!(harness.state().document().key(), key);
    assert_eq!(source_selection(&harness), selection);
    harness.get_by_label("Read-only · edit not applied");
}

#[test]
fn docked_diff_cannot_edit_source_while_a_table_locks_it() {
    let mut harness = fixture();
    let source = harness.state().document().source().clone();
    harness.state_mut().document_diff = Some(crate::git::editor::document_diff::DocumentDiff::new(
        source.clone(),
        &source,
    ));
    harness.run_steps(2);
    let context = harness.ctx.clone();
    harness.state_mut().begin_new_table(&context);
    assert!(harness.state().table_editor.is_some());
    harness.run_steps(2);
    let editor = source_editor_id(&harness.ctx);
    harness
        .state()
        .store_editor_cursor(&harness.ctx, CCursorRange::one(CCursor::new(2)));
    harness
        .ctx
        .memory_mut(|memory| memory.request_focus(editor));
    let key = harness.state().document().key();
    harness.event(egui::Event::Text("Rejected text".into()));
    harness.step();
    assert_eq!(harness.state().document().source(), &source);
    assert_eq!(harness.state().document().key(), key);
    harness.get_by_label("Read-only · edit not applied");
}

#[test]
fn read_only_feedback_expires_without_relayout_and_does_not_leak_after_unlock() {
    let mut harness = locked_source_fixture();
    let context = harness.ctx.clone();
    let editor = source_editor_id(&context);
    context.memory_mut(|memory| memory.request_focus(editor));
    harness.event(egui::Event::Text("Rejected".into()));
    harness.step();
    harness.get_by_label(editor_read_only::MESSAGE);
    let before = snapshot(&harness);
    let source = harness.state().document().source().clone();
    let key = harness.state().document().key();
    harness.run_steps(110);
    assert!(harness.query_by_label(editor_read_only::MESSAGE).is_none());
    let after = snapshot(&harness);
    assert!(Arc::ptr_eq(&before.galley, &after.galley));
    assert_eq!(before.galley_pos, after.galley_pos);
    assert_eq!(harness.state().document().source(), &source);
    assert_eq!(harness.state().document().key(), key);
    assert!(context.memory(|memory| memory.has_focus(editor)));
    harness.event(egui::Event::Text("Also rejected".into()));
    harness.step();
    harness.get_by_label(editor_read_only::MESSAGE);
    harness.state_mut().table_editor = None;
    harness.step();
    assert!(harness.query_by_label(editor_read_only::MESSAGE).is_none());
    harness.event(egui::Event::Text("Allowed".into()));
    harness.step();
    assert_ne!(harness.state().document().key(), key);
    assert!(harness.query_by_label(editor_read_only::MESSAGE).is_none());
}

#[test]
fn read_only_navigation_copy_and_passive_ime_cancellation_do_not_report_edits() {
    let mut harness = locked_source_fixture();
    let context = harness.ctx.clone();
    let editor = source_editor_id(&context);
    harness.state().store_editor_cursor(
        &context,
        CCursorRange::two(CCursor::new(2), CCursor::new(6)),
    );
    context.memory_mut(|memory| memory.request_focus(editor));
    let key = harness.state().document().key();
    harness.input_mut().events.extend([
        egui::Event::Copy,
        mutation_key(egui::Key::ArrowRight, egui::Modifiers::SHIFT),
        egui::Event::Ime(egui::ImeEvent::Preedit {
            text: String::new(),
            active_range_chars: None,
        }),
        egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
        egui::Event::Ime(egui::ImeEvent::DeleteSurrounding {
            before_chars: 0,
            after_chars: 0,
        }),
    ]);
    harness.step();
    assert_eq!(harness.state().document().key(), key);
    assert_eq!(source_selection(&harness).primary.index.0, 7);
    assert!(harness.query_by_label(editor_read_only::MESSAGE).is_none());
    assert!(
        harness.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "efix")
        )
    );
}

#[test]
fn locked_source_rejects_replacements_but_allows_find_navigation() {
    let mut harness = locked_source_fixture();
    let context = harness.ctx.clone();
    let key = harness.state().document().key();
    harness.state_mut().find_bar.query = "needle".into();
    harness.state_mut().find_bar.replacement = "MUST NOT REPLACE".into();
    harness
        .state_mut()
        .apply_find_actions(&context, false, true, false, false);
    assert!(harness.state().pending_editor_selection.is_some());
    for (one, all) in [(true, false), (false, true)] {
        harness
            .state_mut()
            .apply_find_actions(&context, false, false, one, all);
        harness.step();
        harness.get_by_label(editor_read_only::MESSAGE);
        assert_eq!(harness.state().document().key(), key);
    }
}
