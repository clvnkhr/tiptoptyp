use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

fn snippet_click_case(projected: bool, server_backed: bool) {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.snapshot_scene = None;
    app.settings.auto_pair_delimiters = false;
    let (language, source, cursor, body, expected, selection) = if projected {
        app.document_mut().replace_unprojected_untitled("");
        let mut settings = app.settings.clone();
        settings.mitex_auto_enable = true;
        app.apply_shared_settings(settings, &context);
        assert!(app.document().config().is_some());
        (
            crate::snippets::Language::Tex,
            "$ frac $",
            6,
            r"\frac{{{1:left}}}{{{2:right}}}{{0}}",
            r"$ \frac{left}{right} $",
            8..12,
        )
    } else {
        (
            crate::snippets::Language::Typst,
            "$frac",
            5,
            "frac({{1:left}}, {{2:right}}){{0}}",
            "$frac(left, right)",
            6..10,
        )
    };
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = source.into();
    });
    let fraction = app
        .settings
        .snippet_presets
        .iter_mut()
        .find(|snippet| snippet.prefix == "frac" && snippet.language == language)
        .unwrap();
    fraction.body = body.into();
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(30)
        .with_size(Vec2::new(610.0, 520.0))
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
    harness.run();
    harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(cursor..cursor));
    harness.run();
    let caret = harness.state().last_editor_caret.unwrap();
    harness
        .state_mut()
        .request_editor_completion(cursor, caret.rect, true);
    if server_backed {
        let app = harness.state_mut();
        let generation = Generation(7);
        let uri = "file:///fixture.typ".to_owned();
        app.tinymist_sync.generation = Some(generation);
        app.tinymist_sync.current_uri = Some(uri.clone());
        app.editor_completion.as_mut().unwrap().provenance = CompletionProvenance::Server {
            generation,
            uri,
            request_token: 19,
        };
    }
    harness.run();
    assert!(harness.state().editor_completion.is_some());
    let row = harness.get_by_label_contains("· Snippet").rect();
    harness.event(egui::Event::PointerMoved(row.center()));
    harness.run();
    harness.event(egui::Event::PointerButton {
        pos: row.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    // Native down/up arrive in separate event-loop turns and can include a
    // little pointer jitter while remaining inside the same completion row.
    let release = row.center() + Vec2::new(-1.25, -1.5);
    harness.event(egui::Event::PointerMoved(release));
    harness.run();
    harness.event(egui::Event::PointerButton {
        pos: release,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run();
    assert_eq!(harness.state().document().source(), expected);
    assert_eq!(
        harness
            .state()
            .snippet_session
            .as_ref()
            .unwrap()
            .selection(),
        selection
    );
}

#[test]
fn edited_snippet_mouse_acceptance_survives_the_source_editor_pass() {
    snippet_click_case(false, false);
}

#[test]
fn server_batch_keeps_local_snippet_mouse_acceptance_in_display_coordinates() {
    snippet_click_case(false, true);
}

#[test]
fn projected_math_accepts_the_tex_snippet_with_split_mouse_events() {
    snippet_click_case(true, false);
}
