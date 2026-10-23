use super::*;
use crate::diagnostics::DiagnosticCorrection;
use egui_kittest::{Harness, kittest::Queryable as _};

fn app_with_problems() -> EditorApp {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, std::env::temp_dir());
    app.snapshot_scene = None;
    app.document_mut()
        .replace_unprojected_untitled("first\nteh\nthird\nfourth");
    let mut correction = DiagnosticCorrection::new(
        6..9,
        "teh".into(),
        Some("SpellCheck".into()),
        Some("Spelling".into()),
        vec!["the".into()],
    );
    correction.document = Some(app.document().key());
    app.preview.diagnostics = vec![
        diagnostic("Tinymist", DiagnosticSeverity::Error, "Unknown variable", 1),
        Diagnostic {
            details: vec![
                "Code: redundant-script-braces".into(),
                "Hint: remove special braces".into(),
            ],
            ..diagnostic(
                "Badness",
                DiagnosticSeverity::Warning,
                "Redundant braces",
                2,
            )
        },
        Diagnostic {
            provider: None,
            source: DiagnosticSource::Global,
            location: None,
            ..diagnostic(
                "",
                DiagnosticSeverity::Unknown,
                "Unstructured tool output",
                1,
            )
        },
    ];
    app.preview.editor_diagnostics = vec![
        Diagnostic {
            correction: Some(correction),
            ..diagnostic("Harper", DiagnosticSeverity::Help, "Check spelling", 2)
        },
        Diagnostic {
            source: DiagnosticSource::File("/project/appendix.tex".into()),
            details: vec!["Hint: add an article".into()],
            correction: Some(DiagnosticCorrection::new(
                0..0,
                String::new(),
                Some("MISSING_ARTICLE".into()),
                Some("Grammar".into()),
                Vec::new(),
            )),
            ..diagnostic("LTeX+", DiagnosticSeverity::Warning, "Missing article", 3)
        },
        diagnostic("Harper", DiagnosticSeverity::Warning, "Repeated word", 4),
    ];
    app.mark_diagnostics_changed();
    app
}

fn diagnostic(
    provider: &str,
    severity: DiagnosticSeverity,
    message: &str,
    line: usize,
) -> Diagnostic {
    Diagnostic {
        correction: None,
        provider: Some(provider.into()),
        severity,
        source: DiagnosticSource::Main,
        location: Some(DiagnosticLocation { line, column: 1 }),
        message: message.into(),
        details: Vec::new(),
    }
}

fn harness_with_size(app: EditorApp, size: Vec2) -> Harness<'static, EditorApp> {
    let mut harness = Harness::builder()
        .with_size(size)
        .with_step_dt(0.01)
        .build_ui_state(|ui, app: &mut EditorApp| app.show_problems(ui), app);
    harness.run();
    harness
}

fn harness() -> Harness<'static, EditorApp> {
    harness_with_size(app_with_problems(), Vec2::new(900.0, 600.0))
}

fn wheel(harness: &mut Harness<'_, EditorApp>, delta: f32) {
    harness.hover_at(harness.state().problems.scroll_rect.center());
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(12);
}

fn many_problems() -> EditorApp {
    let mut app = app_with_problems();
    app.preview.diagnostics = (0..100)
        .map(|index| {
            diagnostic(
                "Tinymist",
                DiagnosticSeverity::Warning,
                &format!("Problem {index}"),
                index + 1,
            )
        })
        .collect();
    app.preview.editor_diagnostics.clear();
    app.mark_diagnostics_changed();
    app
}

fn workspace_harness(mut app: EditorApp) -> Harness<'static, EditorApp> {
    app.bottom_panel.select(PanelTab::Problems);
    let mut ready = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(900.0, 1000.0))
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, app: &mut EditorApp| {
                if !ready {
                    theme::configure_test_fonts(ui.ctx());
                    let panel = crate::child_view::viewport_scoped_id(ui.ctx(), "bottom-panel");
                    ui.ctx().data_mut(|data| {
                        data.insert_persisted(
                            panel,
                            egui::PanelState {
                                outer_rect: Rect::from_min_size(
                                    Pos2::ZERO,
                                    Vec2::new(900.0, 500.0),
                                ),
                            },
                        );
                    });
                    ready = true;
                    return;
                }
                if !app.show_bottom_panel_container(ui) {
                    app.show_editor(ui);
                }
            },
            app,
        );
    harness.run();
    harness
}

fn reveal_filters(harness: &mut Harness<'_, EditorApp>) {
    let context = harness.ctx.clone();
    harness.state_mut().focus_problems_search(&context);
    harness.run();
    assert!(harness.get_by_label("Search problems").is_focused());
}

fn enable_grouping(harness: &mut Harness<'_, EditorApp>) {
    reveal_filters(harness);
    harness.get_by_label("Group problems").click();
    harness.run();
}

fn assert_search_owns_keyboard(harness: &Harness<'_, EditorApp>) {
    assert!(harness.get_by_label("Search problems").is_focused());
    assert!(
        harness
            .state()
            .problems_focused(&harness.ctx, harness.ctx.viewport_id())
    );
    let source = source_editor_id(&harness.ctx);
    assert!(!harness.ctx.memory(|memory| memory.has_focus(source)));
}

#[test]
fn default_problems_rows_are_flat_and_start_at_the_top() {
    let harness = harness();
    let first = harness.get_by_label("Unknown variable").rect();
    let second = harness.get_by_label("Redundant braces").rect();
    let state = &harness.state().problems;
    assert!(!state.grouped);
    assert!(!state.controls_visible);
    assert!(!state.sticky_filters);
    assert!(
        state.controls_rect.bottom() <= state.scroll_rect.top() + 1.0,
        "empty filters occupy the list: {:?}",
        state.controls_rect
    );
    assert!(first.top() <= state.scroll_rect.top() + 4.0);
    assert!(second.top() > first.top());
    assert!(
        second.top() - first.top() <= first.height() + 12.0,
        "plain one-line diagnostics acquired vertical gaps: {first:?}, {second:?}"
    );
    for group in [
        "Tinymist · Error (1)",
        "Badness · Warning (1)",
        "Harper · Help (1)",
    ] {
        assert!(harness.query_by_label(group).is_none(), "{group}");
    }
    harness.get_by_label("Check spelling");
    harness.get_by_label("Repeated word");
}

#[test]
fn scrolling_up_reveals_empty_filters_and_scrolling_down_hides_them() {
    // A short list still has enough scroll range to uncover its search row.
    let mut harness = harness();
    let top = harness.state().problems.scroll_rect.top();
    wheel(&mut harness, 160.0);
    let revealed = harness.get_by_label("Search problems").rect();
    assert!(harness.state().problems.controls_visible);
    assert!(!harness.state().problems.sticky_filters);
    assert!(revealed.top() >= top - 1.0, "{revealed:?}");
    assert!(revealed.bottom() < harness.get_by_label("Unknown variable").rect().top());
    harness.run_steps(12);
    assert_eq!(harness.get_by_label("Search problems").rect(), revealed);

    wheel(&mut harness, -160.0);
    assert!(!harness.state().problems.controls_visible);
    assert!(harness.state().problems.controls_rect.bottom() <= top + 1.0);
    assert!(harness.get_by_label("Unknown variable").rect().top() <= top + 4.0);
    harness.run_steps(12);
    assert!(!harness.state().problems.controls_visible);
}

#[test]
fn wheel_scrolling_does_not_fight_back_with_empty_or_active_filters() {
    let mut harness = harness_with_size(many_problems(), Vec2::new(900.0, 260.0));
    let initial = harness.state().problems.scroll_offset;
    wheel(&mut harness, -160.0);
    let scrolled = harness.state().problems.scroll_offset;
    assert!(scrolled > initial + 100.0);
    harness.run_steps(12);
    assert!(harness.state().problems.scroll_offset >= scrolled - 1.0);

    reveal_filters(&mut harness);
    harness.get_by_label("Search problems").type_text("Problem");
    harness.run();
    let pinned = harness.get_by_label("Search problems").rect();
    let initial = harness.state().problems.scroll_offset;
    wheel(&mut harness, -160.0);
    let scrolled = harness.state().problems.scroll_offset;
    assert!(scrolled > initial + 100.0);
    assert!(harness.state().problems.sticky_filters);
    assert_eq!(harness.get_by_label("Search problems").rect(), pinned);
    harness.run_steps(12);
    assert!(harness.state().problems.scroll_offset >= scrolled - 1.0);
    assert_eq!(harness.get_by_label("Search problems").rect(), pinned);
}

#[test]
fn double_clicking_sticky_search_cannot_navigate_a_clipped_diagnostic_under_it() {
    let mut harness = workspace_harness(many_problems());
    reveal_filters(&mut harness);
    harness.get_by_label("Search problems").type_text("Problem");
    harness.run();
    wheel(&mut harness, -160.0);
    assert!(harness.state().problems.scroll_offset > 100.0);
    let search = harness.get_by_label("Search problems").rect();
    assert!(search.bottom() < harness.state().problems.scroll_rect.top());
    for _ in 0..2 {
        harness.get_by_label("Search problems").click();
        harness.step();
    }
    assert!(
        harness.state().pending_editor_selection.is_none(),
        "a double-click in the search field navigated an offscreen row beneath it"
    );
    assert_search_owns_keyboard(&harness);
}

#[test]
fn find_reveals_hidden_filters_and_keeps_typing_out_of_source() {
    let mut harness = harness();
    assert!(!harness.state().problems.controls_visible);
    reveal_filters(&mut harness);
    let search = harness.get_by_label("Search problems").rect();
    assert!(harness.state().problems.controls_visible);
    assert!(harness.state().problems.sticky_filters);
    assert!(search.top() < harness.state().problems.scroll_rect.top());
    harness
        .get_by_label("Search problems")
        .type_text("spelling");
    harness.run();
    harness.get_by_label("Check spelling");
    assert_eq!(harness.get_by_label("Search problems").rect(), search);
    assert!(harness.query_by_label("Unknown variable").is_none());
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
}

#[test]
fn source_type_and_typed_search_combine_and_reset_together() {
    let mut harness = workspace_harness(app_with_problems());
    reveal_filters(&mut harness);
    harness.get_by_label("Source").click();
    harness.run();
    harness.get_by_label("Harper (2)").click();
    harness.run();
    assert_search_owns_keyboard(&harness);
    assert_eq!(
        harness.state().problems.filter.provider.as_deref(),
        Some("Harper")
    );
    assert!(harness.query_by_label("Unknown variable").is_none());
    harness.get_by_label("Check spelling");
    harness.get_by_label("Repeated word");

    harness.get_by_label("Type").click();
    harness.run();
    harness.get_by_label("Help (1)").click();
    harness.run();
    assert_search_owns_keyboard(&harness);
    assert_eq!(
        harness.state().problems.filter.severity,
        Some(DiagnosticSeverity::Help)
    );
    assert!(harness.query_by_label("Repeated word").is_none());

    harness.get_by_label("Category").click();
    harness.run();
    harness.get_by_label("Spelling (1)").click();
    harness.run();
    assert_search_owns_keyboard(&harness);
    assert_eq!(
        harness.state().problems.filter.category.as_deref(),
        Some("Spelling")
    );

    harness
        .get_by_label("Search problems")
        .type_text("SpellCheck");
    harness.run();
    harness.get_by_label("Check spelling");
    assert_eq!(harness.state().problems.filter.query, "SpellCheck");
    harness.get_by_label("Reset problem filters").click();
    harness.run();
    assert_search_owns_keyboard(&harness);
    assert!(harness.state().problems.filter.query.is_empty());
    assert!(harness.state().problems.filter.provider.is_none());
    assert!(harness.state().problems.filter.severity.is_none());
    assert!(harness.state().problems.filter.category.is_none());
    harness.get_by_label("Unknown variable");
    harness.get_by_label("Repeated word");
    harness
        .get_by_label("Search problems")
        .type_text("Redundant braces");
    harness.run();
    harness.get_by_label("Redundant braces");
    assert!(harness.query_by_label("Check spelling").is_none());
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
}

#[test]
fn search_finds_collapsed_details_codes_categories_and_other_files() {
    let mut harness = harness();
    for (query, expected) in [
        ("SPECIAL BRACES", "Redundant braces"),
        ("redundant-script-braces", "Redundant braces"),
        ("appendix.tex", "Missing article"),
        ("MISSING_ARTICLE", "Missing article"),
        ("Grammar", "Missing article"),
        ("Spelling", "Check spelling"),
    ] {
        harness.state_mut().problems.filter.query = query.into();
        harness.run();
        harness.get_by_label(expected);
        assert!(
            harness.query_by_label("Unknown variable").is_none(),
            "{query}"
        );
    }
    // Matching an undisclosed detail must not expand the row as a side effect.
    harness.state_mut().problems.filter.query = "SPECIAL BRACES".into();
    harness.run();
    assert!(
        harness
            .query_by_label("Hint: remove special braces")
            .is_none()
    );
    harness.get_by_label("Show diagnostic details");
}

#[test]
fn refresh_retains_filters_and_distinguishes_no_matches_from_no_problems() {
    let mut harness = harness();
    harness.state_mut().problems.filter.provider = Some("Harper".into());
    harness.state_mut().problems.filter.query = "spelling".into();
    harness.run();
    harness.get_by_label("Check spelling");
    harness.state_mut().preview.editor_diagnostics.clear();
    harness.state_mut().mark_diagnostics_changed();
    harness.run();
    harness.get_by_label("No matching problems");
    assert_eq!(
        harness.state().problems.filter.provider.as_deref(),
        Some("Harper")
    );
    assert_eq!(harness.state().problems.filter.query, "spelling");
    // A retained source selection remains resettable after that provider disappears.
    harness.get_by_label("Reset problem filters").click();
    harness.run();
    harness.get_by_label("Unknown variable");
    assert!(harness.query_by_label("No matching problems").is_none());

    harness.state_mut().preview.diagnostics.clear();
    harness.state_mut().mark_diagnostics_changed();
    harness.run();
    harness.get_by_label("No problems");
    assert!(harness.query_by_label("No matching problems").is_none());
}

#[test]
fn filtering_and_refresh_preserve_detail_expansion_without_navigating() {
    let mut harness = harness();
    harness.state_mut().problems.filter.provider = Some("Badness".into());
    harness.run();
    harness.get_by_label("Show diagnostic details").click();
    harness.run();
    harness.get_by_label("Hint: remove special braces");
    harness.state_mut().problems.filter.query = "no-match".into();
    harness.run();
    harness.get_by_label("No matching problems");
    harness.state_mut().problems.filter.query.clear();
    harness.state_mut().mark_diagnostics_changed();
    harness.run();
    harness.get_by_label("Hint: remove special braces");
    harness.get_by_label("Hide diagnostic details");
    assert!(harness.state().pending_editor_selection.is_none());
}

#[test]
fn source_type_groups_collapse_independently_and_remember_their_state() {
    let mut harness = harness();
    enable_grouping(&mut harness);
    harness.get_by_label("Harper · Help (1)").click();
    harness.run();
    assert!(harness.get_by_label("Harper · Help (1)").is_focused());
    assert!(harness.query_by_label("Check spelling").is_none());
    harness.get_by_label("Repeated word");
    harness.get_by_label("Unknown variable");

    harness.state_mut().problems.filter.provider = Some("Tinymist".into());
    harness.run();
    harness.get_by_label("Unknown variable");
    harness.state_mut().problems.filter.provider = Some("Harper".into());
    harness.state_mut().mark_diagnostics_changed();
    harness.run();
    assert!(harness.query_by_label("Check spelling").is_none());
    harness.get_by_label("Repeated word");
    harness.get_by_label("Harper · Help (1)").click();
    harness.run();
    harness.get_by_label("Check spelling");
    assert!(harness.state().pending_editor_selection.is_none());
}

#[test]
fn grouped_header_keeps_its_hit_target_while_search_focus_changes() {
    let mut harness = harness();
    enable_grouping(&mut harness);
    // With no filters, only search focus keeps the toolbar above the list.
    // Pressing a group must not move its eventual release target as that focus
    // transfers away from the search field.
    reveal_filters(&mut harness);
    assert!(!harness.state().problems.filter.is_active());
    assert!(harness.state().problems.sticky_filters);
    let label = "Harper · Help (1)";
    let initial = harness.get_by_label(label).rect();
    let position = initial.center();
    harness.hover_at(position);
    harness.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(8);
    assert_eq!(harness.get_by_label(label).rect(), initial);
    harness.get_by_label("Check spelling");

    // Keep raw coordinates fixed: resolving the widget again for the release
    // would conceal any intervening toolbar-induced layout shift.
    harness.event(egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(8);
    assert!(harness.query_by_label("Check spelling").is_none());
    assert!(harness.get_by_label(label).is_focused());
    assert!(harness.state().pending_editor_selection.is_none());
}

fn seed_problem_hover(harness: &Harness<'_, EditorApp>, origin: Rect, detail: String) {
    let overlay = native_hover_tooltip_id(&harness.ctx);
    let geometry = tooltip_geometry_id(&harness.ctx);
    let interaction = tooltip_interaction_id(&harness.ctx);
    harness.ctx.data_mut(|data| {
        data.insert_temp(
            overlay,
            HoverTooltipOverlay {
                origin,
                anchor: origin.left_bottom(),
                detail: Arc::from(detail),
                opacity: 1.0,
                git_diff: false,
            },
        );
        data.insert_temp(
            geometry,
            TooltipGeometry {
                identity: 7,
                origin,
                card: Rect::from_min_size(origin.left_bottom(), Vec2::new(500.0, 180.0)),
                handoff_apex: origin.center(),
                pointer_inside_viewport: true,
                handoff_until: f64::MAX,
            },
        );
        data.insert_temp(interaction, TooltipInteractionState::new(7));
    });
}

fn assert_no_problem_hover(harness: &Harness<'_, EditorApp>) {
    let overlay = native_hover_tooltip_id(&harness.ctx);
    let geometry = tooltip_geometry_id(&harness.ctx);
    let interaction = tooltip_interaction_id(&harness.ctx);
    assert!(harness.ctx.data(|data| {
        data.get_temp::<HoverTooltipOverlay>(overlay).is_none()
            && data.get_temp::<TooltipGeometry>(geometry).is_none()
            && data
                .get_temp::<TooltipInteractionState>(interaction)
                .is_none()
    }));
}

#[test]
fn filtering_retires_tooltip_handoff_and_problem_rows_do_not_recreate_it() {
    let mut harness = workspace_harness(app_with_problems());
    reveal_filters(&mut harness);
    let origin = harness.get_by_label("Redundant braces").rect();
    seed_problem_hover(
        &harness,
        origin,
        harness.state().preview.diagnostics[1].full_message(),
    );
    harness
        .get_by_label("Search problems")
        .type_text("Harper SpellCheck");
    harness.run();
    harness.get_by_label("Check spelling");
    assert_no_problem_hover(&harness);

    // Rows already expose their full text and an explicit details disclosure;
    // hovering them must not leave another native surface over moved groups.
    tooltips::install_hover_runtime_config(&harness.ctx, Duration::ZERO);
    harness.get_by_label("Check spelling").hover();
    harness.run_steps(6);
    assert_no_problem_hover(&harness);
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
}

#[test]
fn publication_and_group_toggles_retire_tooltips_anchored_to_previous_rows() {
    let mut harness = workspace_harness(app_with_problems());
    enable_grouping(&mut harness);
    let origin = harness.get_by_label("Redundant braces").rect();
    let detail = harness.state().preview.diagnostics[1].full_message();
    seed_problem_hover(&harness, origin, detail.clone());
    // Publication alone can move rows. Neither query nor filter changes here.
    harness.state_mut().preview.diagnostics.insert(
        0,
        diagnostic(
            "A new provider",
            DiagnosticSeverity::Note,
            "Newly published diagnostic",
            3,
        ),
    );
    harness.state_mut().mark_diagnostics_changed();
    harness.run();
    assert_no_problem_hover(&harness);
    harness.get_by_label("Newly published diagnostic");

    let origin = harness.get_by_label("Redundant braces").rect();
    seed_problem_hover(&harness, origin, detail.clone());
    harness.get_by_label("Badness · Warning (1)").click();
    harness.run();
    assert_no_problem_hover(&harness);
    assert!(harness.query_by_label("Redundant braces").is_none());

    // A retained/late tooltip for that old row must also retire when reopening.
    seed_problem_hover(&harness, origin, detail);
    harness.get_by_label("Badness · Warning (1)").click();
    harness.run();
    assert_no_problem_hover(&harness);
    harness.get_by_label("Redundant braces");
    assert!(harness.state().pending_editor_selection.is_none());
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
}

fn click_context_action(harness: &mut Harness<'_, EditorApp>, message: &str, action: &str) {
    harness.get_by_label(message).click_secondary();
    harness.run();
    let position = harness.get_by_label(action).rect().center();
    harness.event(egui::Event::PointerMoved(position));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
}

#[test]
fn filtered_row_copy_and_correction_use_the_original_editor_diagnostic() {
    let mut app = app_with_problems();
    app.problems.filter.provider = Some("Harper".into());
    app.problems.filter.severity = Some(DiagnosticSeverity::Help);
    // Applying a correction focuses the actual source editor. Register that
    // sibling just as the real workspace does, rather than inventing a focus
    // target that cannot exist in this isolated Problems-only harness.
    let mut harness = workspace_harness(app);
    let expected = harness.state().preview.editor_diagnostics[0].full_message();
    click_context_action(&mut harness, "Check spelling", "Copy diagnostic");
    assert!(
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
            })
    );
    let search = search_id(&harness.ctx);
    assert!(harness.ctx.memory(|memory| memory.has_focus(search)));
    assert!(
        harness
            .state()
            .problems_focused(&harness.ctx, harness.ctx.viewport_id())
    );
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
    click_context_action(&mut harness, "Check spelling", "Replace with “the”");
    assert_eq!(
        harness.state().document().source(),
        "first\nthe\nthird\nfourth"
    );
    let context = harness.ctx.clone();
    let cursor = harness
        .state_mut()
        .editor_snapshot(&context)
        .cursor
        .as_sorted_char_range();
    assert_eq!(cursor.start.0..cursor.end.0, 6..9);
    let editor = source_editor_id(&harness.ctx);
    assert!(harness.ctx.memory(|memory| memory.has_focus(editor)));
}

#[test]
fn double_clicking_a_filtered_editor_diagnostic_reveals_its_own_range() {
    let mut harness = harness();
    harness.state_mut().problems.filter.provider = Some("Harper".into());
    harness.state_mut().problems.filter.severity = Some(DiagnosticSeverity::Help);
    harness.run();
    for _ in 0..2 {
        harness.get_by_label("Check spelling").click();
        harness.step();
    }
    assert_eq!(
        harness.state().pending_editor_selection,
        Some(EditorSelection::Focus(6..9))
    );
    assert_eq!(
        harness.state().document().source(),
        "first\nteh\nthird\nfourth"
    );
}

#[test]
fn filtering_in_a_narrow_bottom_panel_never_changes_the_users_height() {
    let mut app = app_with_problems();
    app.bottom_panel.select(PanelTab::Problems);
    let mut harness = Harness::builder()
        .with_size(Vec2::new(360.0, 600.0))
        .with_step_dt(0.01)
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                app.show_bottom_panel_container(ui);
            },
            app,
        );
    harness.run();
    let id = crate::child_view::viewport_scoped_id(&harness.ctx, "bottom-panel");
    let height = |harness: &mut Harness<'_, EditorApp>| {
        harness.ctx.data_mut(|data| {
            data.get_persisted::<egui::PanelState>(id)
                .unwrap()
                .outer_rect
                .height()
        })
    };
    let initial = height(&mut harness);
    reveal_filters(&mut harness);
    harness
        .get_by_label("Search problems")
        .type_text("nothing matches this");
    harness.run();
    harness.get_by_label("No matching problems");
    for width in [280.0, 360.0, 600.0, 360.0] {
        harness.set_size(Vec2::new(width, 600.0));
        harness.run_steps(4);
        assert_eq!(height(&mut harness), initial, "panel grew at width {width}");
        let search = harness.get_by_label("Search problems").rect();
        assert!(search.left() >= 0.0 && search.right() <= width);
    }
    harness.get_by_label("Reset problem filters").click();
    harness.run_steps(6);
    assert_eq!(height(&mut harness), initial);
}
