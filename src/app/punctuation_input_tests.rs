use super::*;

#[test]
fn cjk_typing_and_ime_commits_follow_the_explicit_ascii_punctuation_preference() {
    for ascii in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.settings.ascii_punctuation = ascii;
        app.settings.auto_pair_delimiters = false;
        app.document_mut().replace_unprojected_untitled("prefix ");
        let frame = |app: &mut EditorApp, events| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| app.show_editor(ui),
                )
                .drop_without_applying_deltas();
        };
        frame(&mut app, Vec::new());
        app.pending_editor_selection = Some(EditorSelection::Focus(7..7));
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "日本。！".into(),
                active_range_chars: Some(0..2),
            })],
        );
        assert_eq!(app.document().source(), "prefix 日本。！");
        frame(
            &mut app,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("日本。！".into()))],
        );
        let committed = if ascii {
            "prefix 日本.!"
        } else {
            "prefix 日本。！"
        };
        assert_eq!(app.document().source(), committed);
        let pasted = format!("{committed}。、，！");
        frame(&mut app, vec![egui::Event::Paste("。、，！".into())]);
        assert_eq!(app.document().source(), &pasted);
        frame(&mut app, vec![egui::Event::Text("。！".into())]);
        let expected = format!("{pasted}{}", if ascii { ".!" } else { "。！" });
        assert_eq!(app.document().source(), &expected);
    }
}
