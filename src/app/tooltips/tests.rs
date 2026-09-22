use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

#[test]
fn unrelated_controls_cannot_reset_the_hovered_controls_shared_timer() {
    let timing = egui::Id::new("shared-hover-timing");
    let mut harness = Harness::builder().build_ui(|ui| {
        install_hover_runtime_config(ui.ctx(), Duration::ZERO);
        let target = ui.button("Target");
        hover_opacity(&target, timing);
        let other = ui.button("Other");
        hover_opacity(&other, timing);
        if target.hovered() {
            assert_eq!(
                ui.ctx().data(|data| {
                    data.get_temp::<HoverTimingState>(timing)
                        .map(|state| state.widget)
                }),
                Some(target.id)
            );
        }
    });
    harness.get_by_label("Target").hover();
    harness.run();
}

#[test]
fn tooltip_server_request_waits_for_delay_and_is_sent_once() {
    assert!(!hover_request_ready(false, true, true, false));
    assert!(hover_request_ready(true, true, true, false));
    assert!(!hover_request_ready(true, true, true, true));
    assert!(!hover_request_ready(true, false, true, false));
    assert!(!hover_request_ready(true, true, false, false));
}

#[test]
fn tooltip_scroll_suppression_lasts_until_pointer_moves_without_animation() {
    let context = egui::Context::default();
    let mut input = egui::RawInput::default();
    let pointer = Pos2::new(20.0, 20.0);
    input.events = vec![
        egui::Event::PointerMoved(pointer),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -20.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    context
        .run_ui(input, |ui| assert!(update_hover_scroll(ui.ctx())))
        .drop_without_applying_deltas();
    for _ in 0..60 {
        context
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(update_hover_scroll(ui.ctx()))
            })
            .drop_without_applying_deltas();
    }
    let mut input = egui::RawInput::default();
    input
        .events
        .push(egui::Event::PointerMoved(pointer + Vec2::new(8.0, 0.0)));
    context
        .run_ui(input, |ui| assert!(!update_hover_scroll(ui.ctx())))
        .drop_without_applying_deltas();
    assert!(!source_scroll_changed(&context, Vec2::ZERO));
    assert!(source_scroll_changed(&context, Vec2::new(0.0, 90.0)));
    assert!(update_hover_scroll(&context));
}

#[test]
fn tooltip_delay_finishes_at_full_opacity_and_settles() {
    let mut harness = Harness::builder().build_ui(|ui| {
        install_hover_runtime_config(ui.ctx(), Duration::ZERO);
        let response = ui.button("Hover target");
        let opacity = hover_opacity(&response, egui::Id::new("test-timing"));
        if response.hovered() {
            assert_eq!(opacity, Some(1.0));
        }
    });
    harness.get_by_label("Hover target").hover();
    harness.run(); // fails if animations keep requesting paints
}

#[test]
fn tooltip_pointer_away_dismisses_but_toward_inside_and_transient_gaps_do_not() {
    let origin = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(20.0));
    let card = Rect::from_min_size(Pos2::new(10.0, 50.0), Vec2::new(200.0, 100.0));
    let geometry = TooltipGeometry {
        identity: 1,
        origin,
        card,
        handoff_apex: Pos2::new(20.0, 20.0),
        pointer_inside_viewport: false,
        handoff_until: 0.0,
    };
    let point = |x, y| Some(Pos2::new(x, y));
    assert!(tooltip_pointer_moved_away(
        point(20.0, 20.0),
        point(20.0, 0.0),
        geometry,
    ));
    assert!(!tooltip_pointer_moved_away(
        point(20.0, 45.0),
        point(20.0, 35.0),
        geometry,
    ));
    assert!(!tooltip_pointer_moved_away(
        point(20.0, 20.0),
        point(20.0, 40.0),
        geometry,
    ));
    assert!(!tooltip_pointer_moved_away(
        point(20.0, 40.0),
        point(20.0, 60.0),
        geometry,
    ));
    assert!(!tooltip_pointer_moved_away(
        point(20.0, 60.0),
        point(150.0, 90.0),
        geometry,
    ));
    assert!(!tooltip_pointer_moved_away(
        point(20.0, 20.0),
        None,
        geometry,
    ));
}

#[test]
fn hover_timing_survives_a_slow_frame_while_the_same_widget_remains_hovered() {
    let widget = egui::Id::new("hover-target");
    let state = HoverTimingState {
        widget,
        started: 1.0,
    };
    let retained = hover_timing_for_widget(Some(state), widget, 2.0);
    assert_eq!(retained.started, 1.0);
    assert_eq!(
        hover_timing_for_widget(Some(state), egui::Id::new("other"), 2.0).started,
        2.0
    );
}

#[test]
fn hover_timing_starts_fresh_after_an_observed_exit() {
    let context = egui::Context::default();
    let timing_id = egui::Id::new("semantic-hover-timing");
    context.data_mut(|data| {
        data.insert_temp(
            timing_id,
            HoverTimingState {
                widget: egui::Id::new("target"),
                started: 1.0,
            },
        );
    });
    reset_hover_timing(&context, timing_id);
    assert!(
        context
            .data(|data| data.get_temp::<HoverTimingState>(timing_id))
            .is_none()
    );
}

#[test]
fn tooltip_preview_is_bounded_utf8_safe_and_keeps_short_content_whole() {
    let small = "```typc\ntext(body)\n```\nShort description.";
    assert_eq!(tooltip_preview_end(small), small.len());
    let long = format!("{}\n{}", "é🙂字".repeat(10_000), "last line");
    let end = tooltip_preview_end(&long);
    assert_eq!(long[..end].chars().count(), TOOLTIP_PREVIEW_CHARS);
    assert!(long.ends_with("last line"));
}

#[test]
fn tooltip_body_width_keeps_short_definitions_readable_and_caps_long_docs() {
    assert_eq!(tooltip_body_width(1.0), METRICS.popup.tooltip_min_width,);
    assert_eq!(
        tooltip_body_width(METRICS.popup.tooltip_max_width * 2.0),
        METRICS.popup.tooltip_max_width,
    );
    assert!(tooltip_body_width(320.0) > METRICS.popup.tooltip_min_width);
}

#[test]
fn tooltip_expands_on_pointer_entry_without_buttons_and_resets_for_new_content() {
    let (sender, _receiver) = mpsc::channel();
    let content = format!("Preview\n{}\nHidden continuation", "preview ".repeat(74));
    let mut harness = Harness::builder()
        .with_size(Vec2::new(500.0, 400.0))
        .build_ui_state(
            |ui, content| {
                show_tooltip_document(
                    ui,
                    content,
                    tooltip_identity(Rect::ZERO, content),
                    false,
                    &sender,
                );
            },
            content,
        );
    assert!(harness.query_by_label("Hidden continuation").is_none());
    assert!(harness.query_by_label("Read more").is_none());
    harness.get_by_label("Preview").hover();
    harness.run();
    assert!(harness.query_by_label("Hidden continuation").is_some());
    *harness.state_mut() = "Replacement".to_owned();
    harness.run();
    assert!(harness.query_by_label("Replacement").is_some());
    assert!(harness.query_by_label("Read more").is_none());
}

/// Diagnostic probe, not a timing gate. Run optimized with --nocapture on a
/// quiet machine; excludes GPU/native buffer swaps and server response time.
#[test]
fn tooltip_document_render_cost_probe() {
    let long: String = (0..100).map(|i| format!("## Parameter {i}\nThe parameter `value_{i}` accepts lengths and colors. This documentation explains how it changes the output.\n```typc\ntext(size: {i}pt, fill: blue)[Example]\n```\n\n")).collect();
    let short = "```typc\ntext(body, size: length = 1em, fill: color = black)\n```\nDisplays content as text with the selected size and fill.";
    for (name, detail, cull) in [
        ("short-full", short, true),
        ("long-preview", &long[..tooltip_preview_end(&long)], true),
        ("long-full-unculled", long.as_str(), false),
        ("long-full", long.as_str(), true),
    ] {
        let context = egui::Context::default();
        // Bind the same named editor families used by tooltip highlighting.
        theme::configure_editor_fonts(
            &context,
            theme::FontRequest::default(),
            theme::FontRequest::default(),
            false,
            theme::FONT_WEIGHT_NORMAL,
            theme::FONT_WEIGHT_NORMAL,
            None,
        );
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let _ = GenericSyntaxHighlighter::default();
        let (sender, _receiver) = mpsc::channel();
        let render = || {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(480.0, 240.0))),
                ..Default::default()
            };
            context
                .run_ui(input, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        show_markdown_with_culling(ui, detail, &sender, cull)
                    });
                })
                .drop_without_applying_deltas();
        };
        let started = std::time::Instant::now();
        render();
        let cold = started.elapsed();
        for _ in 0..3 {
            render();
        }
        let started = std::time::Instant::now();
        for _ in 0..20 {
            render();
        }
        eprintln!(
            "tooltip-probe {name} bytes={} cold_ms={:.3} warm_mean_ms={:.3}",
            detail.len(),
            cold.as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 50.0
        );
    }
}

#[test]
fn tooltip_caches_share_jobs_and_parse_once_without_copying_the_cache() {
    let context = egui::Context::default();
    let markdown = "A `text` value with [help](https://example.com)";
    let first = cached_tooltip_markdown(&context, markdown);
    assert!(Arc::ptr_eq(
        &first,
        &cached_tooltip_markdown(&context, markdown)
    ));
    assert!(!Arc::ptr_eq(
        &first,
        &cached_tooltip_markdown(&context, "other")
    ));
    let highlighter = GenericSyntaxHighlighter::default();
    let mut typst = SyntaxHighlighter::default();
    let palette = theme::default_syntax_palette(true);
    let mut get = |dark, palette| {
        cached_tooltip_code_job(
            &context,
            &highlighter,
            &mut typst,
            "text",
            "typc",
            dark,
            palette,
        )
        .unwrap()
    };
    let first = get(true, palette);
    assert!(Arc::ptr_eq(&first, &get(true, palette)));
    assert!(!Arc::ptr_eq(
        &first,
        &get(false, theme::default_syntax_palette(false))
    ));
    let source: Arc<str> = Arc::from(markdown);
    assert_eq!(
        cached_tooltip_identity(&context, Rect::ZERO, &source),
        tooltip_identity(Rect::ZERO, markdown)
    );
    assert_ne!(
        cached_tooltip_identity(&context, Rect::ZERO, &Arc::from("changed")),
        tooltip_identity(Rect::ZERO, markdown)
    );
}

#[test]
fn tooltip_culling_keeps_scroll_geometry_and_invalidates_for_width_and_style() {
    let context = egui::Context::default();
    let markdown = (0..80)
        .map(|i| format!("Paragraph {i} with enough text to wrap at narrow widths.\n"))
        .collect::<String>();
    let (sender, _receiver) = mpsc::channel();
    let render = |width, offset| {
        let mut content_size = Vec2::ZERO;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 180.0))),
                    ..Default::default()
                },
                |ui| {
                    content_size = egui::ScrollArea::vertical()
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| show_markdown(ui, &markdown, &sender))
                        .content_size;
                },
            )
            .drop_without_applying_deltas();
        let id = viewport_scoped_id(&context, "tooltip-markdown-layout");
        let layout = context
            .data(|data| data.get_temp::<Arc<Mutex<TooltipMarkdownLayout>>>(id))
            .unwrap();
        (content_size, layout)
    };
    // The scrollbar may take a settling pass before its width is known.
    render(400.0, 0.0);
    let (original, initial) = render(400.0, 0.0);
    let (settled, cached) = render(400.0, 0.0);
    assert_eq!(original.y, settled.y);
    assert!(Arc::ptr_eq(&initial, &cached));
    assert!(cached.lock().unwrap().rendered_blocks < 20);
    let (scrolled, _) = render(400.0, 850.0);
    assert_eq!(scrolled.y, original.y);
    let (narrow, changed) = render(220.0, 0.0);
    assert!(!Arc::ptr_eq(&cached, &changed));
    assert!(narrow.y > original.y);
    context.set_theme(egui::Theme::Light);
    let (_, themed) = render(220.0, 0.0);
    assert!(!Arc::ptr_eq(&changed, &themed));
}

#[test]
fn tooltip_child_scroll_does_not_wake_parent_but_leaving_dismisses() {
    let context = egui::Context::default();
    let geometry_id = tooltip_geometry_id(&context);
    let interaction_id = tooltip_interaction_id(&context);
    let state = TooltipInteractionState::new(42);
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = requests.clone();
    context.set_request_repaint_callback(move |request| {
        recorded.lock().unwrap().push(request.viewport_id)
    });
    context.data_mut(|data| {
        data.insert_temp(
            geometry_id,
            TooltipGeometry {
                identity: 42,
                origin: Rect::ZERO,
                card: Rect::EVERYTHING,
                handoff_apex: Pos2::ZERO,
                pointer_inside_viewport: true,
                handoff_until: 0.0,
            },
        );
        data.insert_temp(interaction_id, state);
    });
    for _ in 0..20 {
        publish_tooltip_interaction(
            &context,
            egui::ViewportId::ROOT,
            geometry_id,
            interaction_id,
            state,
            true,
        );
    }
    assert!(requests.lock().unwrap().is_empty());
    publish_tooltip_interaction(
        &context,
        egui::ViewportId::ROOT,
        geometry_id,
        interaction_id,
        state,
        false,
    );
    assert!(context.data(|data| {
        data.get_temp::<TooltipInteractionState>(interaction_id)
            .unwrap()
            .dismissed
    }));
    assert!(!requests.lock().unwrap().is_empty());
    publish_tooltip_interaction(
        &context,
        egui::ViewportId::ROOT,
        geometry_id,
        interaction_id,
        TooltipInteractionState::new(41),
        true,
    );
    assert_eq!(
        context.data(|data| data
            .get_temp::<TooltipGeometry>(geometry_id)
            .unwrap()
            .identity),
        42
    );
}

#[test]
fn tooltip_code_language_tags_keep_typst_source_and_code_modes_distinct() {
    assert_eq!(
        tooltip_code_mode(&normalize_tooltip_code_token(" TypSt ")),
        TooltipCodeMode::TypstSource
    );
    for token in ["typc", " typst-code ", "TYPST_CODE", "typstcode"] {
        assert_eq!(
            tooltip_code_mode(&normalize_tooltip_code_token(token)),
            TooltipCodeMode::TypstCode
        );
    }
    assert_eq!(
        tooltip_code_mode(&normalize_tooltip_code_token("rust")),
        TooltipCodeMode::Generic
    );
}

#[test]
fn tooltip_markdown_treats_tinymist_separators_and_doc_failures_as_structure() {
    let blocks = parse_tooltip_markdown(
        "```typc\nlet theorem()\n```\n\n---\n\nfailed to parse docs: error:\nunexpected argument: scale-preview\n/path/to/wrapper.typ:17:5\n^^^^^^^^^^^^^^^^^^^^\n",
    );
    assert!(matches!(blocks.first(), Some(MarkdownBlock::Code { .. })));
    assert!(!blocks.iter().any(|block| {
        matches!(
            block,
            MarkdownBlock::Line { spans, .. }
                if spans.iter().any(|span| span.text.contains("failed to parse docs"))
        )
    }));
    assert!(
        !blocks
            .iter()
            .any(|block| matches!(block, MarkdownBlock::Separator))
    );

    let blocks = parse_tooltip_markdown("Signature\n\n---\n\nDocumentation.");
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block, MarkdownBlock::Separator))
    );

    let blocks = parse_tooltip_markdown(
        "````typ\nfailed to parse docs: error: unexpected argument: scale-preview\n   ┌─ /path/wrapper.typ:17:5\n17 │ ````, scale-preview: 90%)\n   │ ^^^^^^^^^^^^^^^^^^\n\nTheorem Environment\n\n#example(```typ\n#theorem[Body]\n```, scale-preview: 90%)\n````",
    );
    let Some(MarkdownBlock::Code { source, token }) = blocks.first() else {
        panic!("nested documentation fence should remain one code block");
    };
    assert_eq!(token, "typ");
    assert!(!source.contains("failed to parse docs"));
    assert!(source.contains("Theorem Environment"));
    assert!(source.contains("#example(```typ"));
}
