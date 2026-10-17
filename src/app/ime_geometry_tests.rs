//! The native candidate area must follow the painted caret, not the full
//! (possibly many screens tall) text widget emitted by egui.
use super::*;
use egui_kittest::Harness;

fn source_editor(source: &str, cursor: usize, size: Vec2) -> Harness<'static, EditorApp> {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.snapshot_scene = None;
    app.settings.auto_pair_delimiters = false;
    app.settings.sticky_context_rows = false;
    app.settings.ascii_punctuation = false;
    app.settings.line_wrap = true;
    app.document_mut().replace_unprojected_untitled(source);
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(30)
        .build_ui_state(
            move |ui, app: &mut EditorApp| {
                if !fonts_ready {
                    theme::configure_test_fonts(ui.ctx());
                    fonts_ready = true;
                    return;
                }
                app.show_editor(ui);
            },
            app,
        );
    harness.run_steps(3);
    harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(cursor..cursor));
    harness.run();
    harness
}

fn assert_native_candidate_area(
    output: &egui::PlatformOutput,
    painted_caret: Rect,
    viewport: Rect,
) {
    let emitted = output
        .ime
        .as_ref()
        .expect("focused editable field emits an IME area");
    let native = eframe::window_host::native_platform_output(output.clone());
    let native = native.ime.expect("native projection retains the IME area");
    assert_eq!(native.purpose, emitted.purpose);
    assert_eq!(native.cursor_rect, emitted.cursor_rect);
    assert_eq!(
        native.should_interrupt_composition,
        emitted.should_interrupt_composition
    );
    assert!(
        viewport.expand(4.0).contains_rect(native.rect),
        "candidate area must remain at the visible caret: native={:?}, caret={painted_caret:?}, viewport={viewport:?}, full editor={:?}",
        native.rect,
        emitted.rect
    );
    // TextEdit expands the painted caret by 1.5 points for its IME rectangle.
    assert!((native.rect.center().x - painted_caret.center().x).abs() <= 2.0);
    assert!((native.rect.center().y - painted_caret.center().y).abs() <= 2.0);
    assert!(native.rect.width() <= 4.0);
    assert!(native.rect.height() <= painted_caret.height() + 4.0);
}

#[test]
fn native_ime_candidate_area_tracks_the_visible_caret_in_a_large_scrolled_document() {
    let source = (0..240)
        .map(|line| format!("Line {line}: ordinary source 中文输入。\n"))
        .collect::<String>();
    let cursor = source
        .lines()
        .take(110)
        .map(|line| line.chars().count() + 1)
        .sum::<usize>()
        + 10;
    let size = Vec2::new(610.0, 520.0);
    let harness = source_editor(&source, cursor, size);
    let caret = harness
        .state()
        .last_editor_caret
        .expect("source has a caret");
    assert_eq!(caret.char_index, cursor);
    let viewport = Rect::from_min_size(Pos2::ZERO, size);
    assert!(viewport.contains(caret.rect.center()));
    let emitted = harness.output().platform_output.ime.as_ref().unwrap();
    assert!(
        emitted.rect.height() > size.y * 3.0,
        "fixture must retain a long text widget"
    );
    assert!(
        emitted.rect.bottom() > viewport.bottom() + size.y,
        "fixture must distinguish the editor bottom from the caret"
    );
    assert_native_candidate_area(&harness.output().platform_output, caret.rect, viewport);
}

#[test]
fn native_ime_candidate_area_tracks_wrapped_cjk_text_in_the_source_panel() {
    let lead = "中文说明与数学文字 ".repeat(180);
    let cursor = lead.chars().count();
    let source = format!("{lead}正在输入的文字{}", "后续段落和数学说明 ".repeat(180));
    let size = Vec2::new(370.0, 420.0);
    let harness = source_editor(&source, cursor, size);
    let caret = harness
        .state()
        .last_editor_caret
        .expect("wrapped source has a caret");
    assert_eq!(caret.char_index, cursor);
    let viewport = Rect::from_min_size(Pos2::ZERO, size);
    assert!(viewport.contains(caret.rect.center()));
    assert!(
        harness
            .output()
            .platform_output
            .ime
            .as_ref()
            .unwrap()
            .rect
            .height()
            > size.y * 3.0
    );
    assert_native_candidate_area(&harness.output().platform_output, caret.rect, viewport);
}

#[test]
fn native_ime_candidate_area_also_tracks_shared_settings_and_snippet_code_fields() {
    for extension in ["json", "tex", "typ"] {
        let source = (0..100)
            .map(|line| format!("row{line}: \"中文输入\",\n"))
            .collect::<String>();
        let mut fonts_ready = false;
        let mut focus = true;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(540.0, 400.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                move |ui, source: &mut String| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let response = code_editor::editor(
                                ui,
                                "ime-source",
                                source,
                                extension,
                                &AppSettings::default(),
                                6,
                            );
                            if std::mem::take(&mut focus) {
                                response.request_focus();
                                ui.scroll_to_rect(
                                    Rect::from_center_size(
                                        response.rect.left_bottom() - Vec2::splat(8.0),
                                        Vec2::splat(4.0),
                                    ),
                                    Some(Align::BOTTOM),
                                );
                            }
                        });
                },
                source,
            );
        harness.run_steps(6);
        let output = &harness.output().platform_output;
        let emitted = output.ime.as_ref().expect("shared code field has focus");
        assert!(emitted.rect.height() > 1200.0, "{extension}");
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(540.0, 400.0));
        assert!(
            viewport.expand(4.0).contains(emitted.cursor_rect.center()),
            "{extension}: {:?}",
            emitted.cursor_rect
        );
        // This widget's own IME caret comes from its syntax-aware TextEdit;
        // projecting the full editor rect must not move its native candidate UI.
        assert_native_candidate_area(output, emitted.cursor_rect, viewport);
    }
}

#[test]
fn native_ime_projection_preserves_other_platform_output_and_composition_metadata() {
    let mut output = egui::PlatformOutput {
        commands: vec![egui::OutputCommand::CopyText(
            "clipboard stays untouched".into(),
        )],
        cursor_icon: egui::CursorIcon::Text,
        ..Default::default()
    };
    assert!(eframe::window_host::native_platform_output(output.clone()) == output);
    for purpose in [
        egui::IMEPurpose::Normal,
        egui::IMEPurpose::Password,
        egui::IMEPurpose::Terminal,
    ] {
        output.ime = Some(egui::output::IMEOutput {
            purpose,
            rect: Rect::from_min_size(Pos2::new(20.0, -3000.0), Vec2::new(600.0, 8000.0)),
            cursor_rect: Rect::from_min_size(Pos2::new(100.0, 200.0), Vec2::new(1.0, 18.0)),
            should_interrupt_composition: true,
        });
        let mut expected = output.clone();
        expected.ime.as_mut().unwrap().rect = output.ime.as_ref().unwrap().cursor_rect;
        assert!(eframe::window_host::native_platform_output(output.clone()) == expected);
        assert_eq!(output.ime.as_ref().unwrap().rect.bottom(), 5000.0);
    }
}
