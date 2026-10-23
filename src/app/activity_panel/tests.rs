use super::filter::{Group, Status};
use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

struct PanelFixture {
    view: State,
    rows: Vec<Indicator>,
}

fn harness_with_size(size: Vec2) -> Harness<'static, PanelFixture> {
    let mut harness = Harness::builder()
        .with_size(size)
        .with_step_dt(0.01)
        .build_ui_state(
            |ui, fixture: &mut PanelFixture| {
                show_indicators(ui, &mut fixture.view, &fixture.rows);
            },
            PanelFixture {
                view: State::default(),
                rows: fixture_indicators(),
            },
        );
    harness.run();
    harness
}

fn reveal_filters(harness: &mut Harness<'_, PanelFixture>) {
    harness.state_mut().view.focus_search = true;
    harness.run();
    assert!(harness.get_by_label("Search activity").is_focused());
}

fn wheel(harness: &mut Harness<'_, PanelFixture>, delta: f32) {
    harness.hover_at(harness.state().view.scroll_rect.center());
    harness.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(12);
}

#[test]
fn compact_grouped_chips_wrap_without_horizontal_overflow() {
    for width in [320.0, 800.0, 1400.0] {
        let harness = harness_with_size(Vec2::new(width, 600.0));
        assert!(harness.state().view.grouped);
        assert!(!harness.state().view.controls_visible);
        for row in &harness.state().rows {
            let rect = harness.get_by_label(&format!("● {}", row.name)).rect();
            assert!(
                rect.min.x >= 0.0 && rect.max.x <= width,
                "{} overflows {width}: {rect:?}",
                row.name
            );
        }
        let last = harness.get_by_label("● Terminal").rect();
        if width >= 800.0 {
            assert!(
                last.max.y < 220.0,
                "groups acquired oversized rows: {last:?}"
            );
        }
        let state = &harness.state().view;
        assert!(
            state.controls_rect.bottom() <= state.scroll_rect.top() + 1.0,
            "inactive filters stole body space: {:?}",
            state.controls_rect
        );
    }
}

#[test]
fn activity_state_changes_cannot_move_unfiltered_chips() {
    let mut harness = harness_with_size(Vec2::new(800.0, 600.0));
    let before: Vec<_> = harness
        .state()
        .rows
        .iter()
        .map(|row| harness.get_by_label(&format!("● {}", row.name)).rect())
        .collect();
    for state in [
        Activity::Idle,
        Activity::Running,
        Activity::Pending("Queued"),
        Activity::Failed("failure".into()),
        Activity::Inactive("Disabled"),
    ] {
        for row in &mut harness.state_mut().rows {
            row.state = state.clone();
        }
        harness.run();
        for (row, rect) in harness.state().rows.iter().zip(&before) {
            assert_eq!(
                harness.get_by_label(&format!("● {}", row.name)).rect(),
                *rect
            );
        }
    }
}

#[test]
fn group_status_and_case_insensitive_error_search_combine_without_changing_sources() {
    let mut harness = harness_with_size(Vec2::new(800.0, 600.0));
    let original: Vec<_> = harness
        .state()
        .rows
        .iter()
        .map(|row| row.state.clone())
        .collect();
    {
        let filter = &mut harness.state_mut().view.filter;
        filter.group = Some(Group::Git);
        filter.status = Some(Status::Failed);
        filter.query = "EXECUTABLE UNAVAILABLE".into();
    }
    harness.run();
    harness.get_by_label("● Git status");
    for name in ["Git hunks", "Git mutation", "Save", "Typst", "Harper"] {
        assert!(harness.query_by_label(&format!("● {name}")).is_none());
    }
    assert!(harness.state().view.sticky_filters);
    assert!(harness.state().view.controls_visible);
    assert_eq!(
        harness
            .state()
            .rows
            .iter()
            .map(|row| row.state.clone())
            .collect::<Vec<_>>(),
        original
    );
    harness.state_mut().view.filter.query = "no-activity-can-match-this".into();
    harness.run();
    assert!(harness.query_by_label("● Git status").is_none());
    assert_eq!(
        harness
            .state()
            .rows
            .iter()
            .map(|row| row.state.clone())
            .collect::<Vec<_>>(),
        original
    );
}

#[test]
fn related_groups_collapse_and_expand_without_losing_members() {
    let mut harness = harness_with_size(Vec2::new(800.0, 600.0));
    let label = format!("{} (3)", Group::Git.label());
    harness.get_by_label(&label).click();
    harness.run();
    assert!(harness.state().view.collapsed.contains(&Group::Git));
    for name in ["Git status", "Git hunks", "Git mutation"] {
        assert!(harness.query_by_label(&format!("● {name}")).is_none());
    }
    harness.get_by_label("● Harper");
    harness.get_by_label(&label).click();
    harness.run();
    for name in ["Git status", "Git hunks", "Git mutation"] {
        harness.get_by_label(&format!("● {name}"));
    }
    assert!(!harness.state().view.collapsed.contains(&Group::Git));
}

#[test]
fn filters_are_editable_and_reset_together_while_flat_grouping_is_optional() {
    let mut harness = harness_with_size(Vec2::new(800.0, 600.0));
    reveal_filters(&mut harness);
    harness.get_by_label("Group").click();
    harness.run();
    harness.get_by_label(Group::Git.label()).click();
    harness.run();
    assert_eq!(harness.state().view.filter.group, Some(Group::Git));
    harness.get_by_label("State").click();
    harness.run();
    harness.get_by_label(Status::Failed.label()).click();
    harness.run();
    assert_eq!(harness.state().view.filter.status, Some(Status::Failed));
    harness.get_by_label("Search activity").type_text("Git");
    harness.run();
    assert_eq!(harness.state().view.filter.query, "Git");
    harness.get_by_label("Group related activity").click();
    harness.run();
    assert!(!harness.state().view.grouped);
    harness.get_by_label("● Git status");
    harness.get_by_label("Reset activity filters").click();
    harness.run();
    assert_eq!(harness.state().view.filter, Default::default());
    assert!(
        !harness.state().view.grouped,
        "resetting filters changed presentation"
    );
    harness.get_by_label("● Git mutation");
    harness.get_by_label("● Harper");
    assert!(harness.get_by_label("Search activity").is_focused());
}

#[test]
fn inactive_filters_scroll_away_and_active_filters_remain_pinned() {
    let mut harness = harness_with_size(Vec2::new(800.0, 120.0));
    let top = harness.state().view.scroll_rect.top();
    wheel(&mut harness, 160.0);
    assert!(harness.state().view.controls_visible);
    assert!(!harness.state().view.sticky_filters);
    assert!(harness.get_by_label("Search activity").rect().top() >= top - 1.0);
    wheel(&mut harness, -160.0);
    assert!(!harness.state().view.controls_visible);
    let offset = harness.state().view.scroll_offset;
    harness.run_steps(12);
    assert_eq!(
        harness.state().view.scroll_offset,
        offset,
        "scroll reset on an ordinary frame"
    );
    reveal_filters(&mut harness);
    harness.get_by_label("Search activity").type_text("i");
    harness.run();
    assert!(harness.state().view.sticky_filters);
    let search = harness.get_by_label("Search activity").rect();
    wheel(&mut harness, -160.0);
    assert!(
        harness.state().view.scroll_offset > 0.0,
        "fixture did not overflow"
    );
    assert_eq!(harness.get_by_label("Search activity").rect(), search);
    let offset = harness.state().view.scroll_offset;
    harness.run_steps(12);
    assert_eq!(
        harness.state().view.scroll_offset,
        offset,
        "sticky filters fought the wheel"
    );
}

fn keyboard_harness() -> Harness<'static, EditorApp> {
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, std::env::temp_dir());
    app.snapshot_scene = None;
    app.document_mut()
        .replace_unprojected_untitled("source must stay unchanged");
    app.bottom_panel.select(PanelTab::Activity);
    let mut configured = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(900.0, 600.0))
        .with_step_dt(0.01)
        .build_ui_state(
            move |ui, app: &mut EditorApp| {
                if !configured {
                    theme::configure_test_fonts(ui.ctx());
                    configured = true;
                    return;
                }
                app.handle_shortcuts(ui.ctx(), None);
                if !app.show_bottom_panel_container(ui) {
                    app.show_editor(ui);
                }
            },
            app,
        );
    harness.run();
    harness
}

#[test]
fn activity_find_and_edit_shortcuts_own_typing_and_release_it_on_panel_handoff() {
    let mut harness = keyboard_harness();
    harness.get_by_label("Activity").click();
    harness.run();
    let bindings = harness.state().settings.effective_shortcuts();
    let find = bindings.egui(ShortcutAction::Find).unwrap();
    harness.key_press_modifiers(find.modifiers, find.logical_key);
    harness.run();
    assert!(harness.get_by_label("Search activity").is_focused());
    assert!(
        harness
            .state()
            .activity_focused(&harness.ctx, harness.ctx.viewport_id())
    );
    let editor = source_editor_id(&harness.ctx);
    assert!(!harness.ctx.memory(|memory| memory.has_focus(editor)));
    harness.event(egui::Event::Text("first query".into()));
    harness.run();
    let select_all = standard_text_edit_shortcut(AppCommand::SelectAll).unwrap();
    harness.key_press_modifiers(select_all.modifiers, select_all.logical_key);
    harness.run();
    harness.event(egui::Event::Text("second query".into()));
    harness.run();
    assert_eq!(harness.state().activity.view.filter.query, "second query");
    let replace = bindings.egui(ShortcutAction::FindReplace).unwrap();
    harness.key_press_modifiers(replace.modifiers, replace.logical_key);
    harness.run();
    assert!(!harness.state().find_bar.visible);
    let comment = bindings.egui(ShortcutAction::ToggleComment).unwrap();
    harness.key_press_modifiers(comment.modifiers, comment.logical_key);
    harness.run();
    assert_eq!(
        harness.state().document().source(),
        "source must stay unchanged"
    );
    let field = search_id(&harness.ctx);
    harness.get_by_label("Problems").click();
    harness.run();
    assert!(!harness.ctx.memory(|memory| memory.has_focus(field)));
    assert!(
        !harness
            .state()
            .activity_focused(&harness.ctx, harness.ctx.viewport_id())
    );
    harness.get_by_label("Activity").click();
    harness.run();
    assert_eq!(harness.state().activity.view.filter.query, "second query");
    assert_eq!(
        harness.state().document().source(),
        "source must stay unchanged"
    );
    harness.get_by_label("Close panel").click();
    harness.run();
    assert!(!harness.ctx.memory(|memory| memory.has_focus(field)));
    assert!(
        !harness
            .state()
            .activity_focused(&harness.ctx, harness.ctx.viewport_id())
    );
}

#[test]
fn maximizing_and_restoring_filter_panels_preserves_search_focus_and_typing() {
    for tab in [PanelTab::Activity, PanelTab::Problems] {
        let mut harness = keyboard_harness();
        let context = harness.ctx.clone();
        let (label, field) = match tab {
            PanelTab::Activity => {
                harness.state_mut().focus_activity_search(&context);
                ("Search activity", search_id(&context))
            }
            PanelTab::Problems => {
                harness.get_by_label("Problems").click();
                harness.run();
                harness.state_mut().focus_problems_search(&context);
                ("Search problems", problems::search_id(&context))
            }
            PanelTab::Terminal => unreachable!(),
        };
        harness.run();
        harness.get_by_label(label).type_text("Git");
        harness.run();
        let mut expected = "Git".to_owned();
        for (button, maximized, suffix) in [
            ("Maximize panel", true, " status"),
            ("Restore panel size", false, " unavailable"),
        ] {
            harness.get_by_label(button).click();
            harness.run();
            assert_eq!(harness.state().bottom_panel.is_maximized(), maximized);
            assert!(
                harness.ctx.memory(|memory| memory.has_focus(field)),
                "{button} stole {tab:?} search keyboard focus"
            );
            harness.event(egui::Event::Text(suffix.into()));
            harness.run();
            expected.push_str(suffix);
            let actual = match tab {
                PanelTab::Activity => &harness.state().activity.view.filter.query,
                PanelTab::Problems => &harness.state().problems.filter.query,
                PanelTab::Terminal => unreachable!(),
            };
            assert_eq!(actual, &expected, "{tab:?}");
            assert_eq!(
                harness.state().document().source(),
                "source must stay unchanged"
            );
        }
        let inactive_query = match tab {
            PanelTab::Activity => &harness.state().problems.filter.query,
            PanelTab::Problems => &harness.state().activity.view.filter.query,
            PanelTab::Terminal => unreachable!(),
        };
        assert!(
            inactive_query.is_empty(),
            "typing reached the inactive panel"
        );
    }
}

#[test]
fn switching_activity_to_problems_assigns_find_and_typing_to_the_new_panel() {
    let mut harness = keyboard_harness();
    let context = harness.ctx.clone();
    harness.state_mut().focus_activity_search(&context);
    harness.run();
    harness.get_by_label("Search activity").type_text("Git");
    harness.run();
    let activity_search = search_id(&harness.ctx);
    harness.get_by_label("Problems").click();
    harness.run();
    assert!(
        harness
            .state()
            .problems_focused(&harness.ctx, harness.ctx.viewport_id()),
        "cleanup of Activity surrendered the newly focused Problems panel"
    );
    assert!(
        !harness
            .ctx
            .memory(|memory| memory.has_focus(activity_search))
    );
    let find = harness
        .state()
        .settings
        .effective_shortcuts()
        .egui(ShortcutAction::Find)
        .unwrap();
    harness.key_press_modifiers(find.modifiers, find.logical_key);
    harness.run();
    assert!(harness.get_by_label("Search problems").is_focused());
    harness.event(egui::Event::Text("problem query".into()));
    harness.run();
    assert_eq!(harness.state().problems.filter.query, "problem query");
    assert_eq!(harness.state().activity.view.filter.query, "Git");
    assert!(!harness.state().find_bar.visible);
    assert_eq!(
        harness.state().document().source(),
        "source must stay unchanged"
    );
    harness.get_by_label("Activity").click();
    harness.run();
    harness.key_press_modifiers(find.modifiers, find.logical_key);
    harness.run();
    assert!(harness.get_by_label("Search activity").is_focused());
    harness.event(egui::Event::Text(" status".into()));
    harness.run();
    assert_eq!(harness.state().activity.view.filter.query, "Git status");
    assert_eq!(harness.state().problems.filter.query, "problem query");
    assert_eq!(
        harness.state().document().source(),
        "source must stay unchanged"
    );
}

#[test]
fn filtering_a_narrow_activity_panel_never_changes_its_height() {
    let mut harness = keyboard_harness();
    harness.set_size(Vec2::new(360.0, 600.0));
    harness.run();
    let panel = crate::child_view::viewport_scoped_id(&harness.ctx, "bottom-panel");
    let height = harness.ctx.data_mut(|data| {
        data.get_persisted::<egui::PanelState>(panel)
            .unwrap()
            .outer_rect
            .height()
    });
    let context = harness.ctx.clone();
    harness.state_mut().focus_activity_search(&context);
    harness.run();
    harness
        .get_by_label("Search activity")
        .type_text("no-activity-can-match-this");
    harness.run();
    for width in [280.0, 360.0, 600.0, 360.0] {
        harness.set_size(Vec2::new(width, 600.0));
        harness.run_steps(4);
        let actual = harness.ctx.data_mut(|data| {
            data.get_persisted::<egui::PanelState>(panel)
                .unwrap()
                .outer_rect
                .height()
        });
        assert_eq!(actual, height, "filters grew the panel at width {width}");
        let search = harness.get_by_label("Search activity").rect();
        assert!(search.left() >= 0.0 && search.right() <= width);
    }
}
#[test]
fn freshness_requires_the_exact_document_revision_and_owner() {
    let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 2, 3);
    assert_eq!(freshness(Some(key), key), Activity::Idle);
    for other in [
        None,
        Some(DocumentKey { revision: 2, ..key }),
        Some(DocumentKey { epoch: 1, ..key }),
        Some(DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(2),
            ..key
        }),
    ] {
        assert_eq!(freshness(other, key), Activity::Pending("Stale"));
    }
}
#[test]
fn activity_diagnostics_ignore_old_replies_after_typing() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app =
        EditorApp::dormant_window_for_tests(&context, root.path().into(), egui::ViewportId::ROOT);
    let uri = "file:///fixture.typ";
    app.tinymist_sync.current_uri = Some(uri.into());
    let old = revision_as_i32(app.document().revision());
    app.receive_editor_diagnostics(uri, Some(old), vec![]);
    assert_eq!(
        freshness(app.activity.diagnostics[0], app.document().key()),
        Activity::Idle
    );
    app.document_mut()
        .edit(CCursorRange::one(CCursor::new(0)), |source| {
            source.push_str("edited")
        });
    app.receive_editor_diagnostics(uri, Some(old), vec![]);
    assert_eq!(
        freshness(app.activity.diagnostics[0], app.document().key()),
        Activity::Pending("Stale")
    );
    let current = revision_as_i32(app.document().revision());
    app.receive_editor_diagnostics("file:///other.typ", Some(current), vec![]);
    assert_eq!(
        freshness(app.activity.diagnostics[0], app.document().key()),
        Activity::Pending("Stale")
    );
    app.receive_editor_diagnostics(uri, Some(current), vec![]);
    assert_eq!(
        freshness(app.activity.diagnostics[0], app.document().key()),
        Activity::Idle
    );
}

#[test]
fn a_new_request_supersedes_failure_but_observing_idle_does_not() {
    let failed = Activity::Failed("disk full".into());
    assert_eq!(queued(failed.clone(), false, "Queued"), failed);
    assert_eq!(queued(failed, true, "Queued"), Activity::Pending("Queued"));
    assert_eq!(queued(Activity::Running, true, "Queued"), Activity::Running);
}
