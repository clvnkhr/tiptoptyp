use super::*;
use crate::asset::AssetThumbnailResult;
use crate::explorer::ExplorerPanelPhase;
use crate::settings::InterfaceTheme;

#[test]
fn preview_focus_wait_is_static_but_active_transition_animates() {
    let context = egui::Context::default();
    let paint = |waiting| {
        context.run_ui(egui::RawInput::default(), |ui| {
            show_preview_transition(ui, waiting)
        })
    };
    for _ in 0..10 {
        paint(true).drop_without_applying_deltas();
    }
    assert!(
        !context.has_requested_repaint(),
        "waiting for window focus must not animate indefinitely"
    );
    paint(false).drop_without_applying_deltas();
    assert!(
        context.has_requested_repaint(),
        "real loading keeps its progress indicator"
    );
}

fn completion_item(insert_text: &str) -> CompletionItem {
    CompletionItem {
        label: insert_text.to_owned(),
        detail: None,
        documentation: None,
        filter_text: None,
        sort_text: None,
        insert_text: insert_text.to_owned(),
        insert_text_is_snippet: false,
        text_edit: None,
        additional_text_edits: Vec::new(),
    }
}

fn run_shortcut<T>(
    modifiers: Modifiers,
    key: egui::Key,
    mut consume: impl FnMut(&mut egui::InputState) -> T,
) -> T {
    let context = egui::Context::default();
    let mut result = None;
    context
        .run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ui| {
                result = Some(ui.ctx().input_mut(&mut consume));
            },
        )
        .drop_without_applying_deltas();
    result.expect("shortcut resolver should run")
}

#[test]
fn shared_menu_geometry_contains_all_rows_and_frame_margins() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 1000.0))),
                ..Default::default()
            },
            |ui| {
                for menu in [CommandMenu::File, CommandMenu::Edit, CommandMenu::View] {
                    let size = command_popup_size(menu, ui.style());
                    let frame = theme::menu_card_frame(ui.style());
                    let margin = frame.total_margin().sum();
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::ZERO, size)),
                    );
                    let response = frame.show(&mut child, |ui| {
                        ui.set_width(size.x - margin.x);
                        show_command_popup_ui(
                            ui,
                            menu,
                            CommandAvailability {
                                can_undo: true,
                                can_redo: true,
                                saved_document: true,
                                typst_document: true,
                                typst_preview: true,
                                interactive_preview: true,
                            },
                            &ShortcutBindings::current_defaults(),
                            &mut None,
                        );
                    });
                    assert!(
                        response.response.rect.height() <= size.y,
                        "{menu:?}: {} > {}",
                        response.response.rect.height(),
                        size.y
                    );
                }
            },
        )
        .drop_without_applying_deltas();
}

#[test]
fn reused_menu_area_grows_from_file_to_edit_without_retaining_scroll_clipping() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    for (generation, menu) in [CommandMenu::File, CommandMenu::Edit]
        .into_iter()
        .enumerate()
    {
        for _ in 0..3 {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
                        ..Default::default()
                    },
                    |ui| {
                        let size = command_popup_size(menu, ui.style());
                        let frame = theme::menu_card_frame(ui.style());
                        let content_size = frame_content_size(size, frame.total_margin().sum());
                        egui::Area::new(egui::Id::new("reused-menu"))
                            .fixed_pos(Pos2::ZERO)
                            .show(ui.ctx(), |ui| {
                                frame.show(ui, |ui| {
                                    let result = show_popup_contents(
                                        ui,
                                        content_size,
                                        generation as u64,
                                        |ui| {
                                            show_command_popup_ui(
                                                ui,
                                                menu,
                                                CommandAvailability {
                                                    can_undo: true,
                                                    can_redo: true,
                                                    saved_document: true,
                                                    typst_document: true,
                                                    typst_preview: true,
                                                    interactive_preview: true,
                                                },
                                                &ShortcutBindings::current_defaults(),
                                                &mut None,
                                            );
                                        },
                                    );
                                    assert!(
                                        result.content_size.y <= result.inner_rect.height(),
                                        "{menu:?}: content {} exceeds visible {}",
                                        result.content_size.y,
                                        result.inner_rect.height()
                                    );
                                });
                            });
                    },
                )
                .drop_without_applying_deltas();
        }
    }
}

#[test]
fn context_menu_sizes_include_every_action_and_separator() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
                ..Default::default()
            },
            |ui| {
                let frame = theme::menu_card_frame(ui.style());
                for is_file in [false, true] {
                    let size = workspace_context_menu_size(is_file, ui.style());
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::ZERO, size)),
                    );
                    let result = frame.show(&mut child, |ui| {
                        show_workspace_popup_ui(
                            ui,
                            Path::new("/workspace/main.typ"),
                            is_file,
                            false,
                            &mut None,
                        );
                    });
                    assert!(result.response.rect.height() <= size.y);
                }
                for can_format in [false, true] {
                    for link in [None, Some("https://example.invalid")] {
                        let size =
                            editor_context_menu_size(link.is_some(), false, can_format, ui.style());
                        let mut child = ui.new_child(
                            egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::ZERO, size)),
                        );
                        let result = frame.show(&mut child, |ui| {
                            show_editor_context_menu_ui(
                                ui,
                                EditorContextMenuOptions {
                                    can_undo: true,
                                    can_redo: true,
                                    has_selection: true,
                                    can_format,
                                    can_sync_preview: true,
                                    link,
                                    table: None,
                                },
                                &ShortcutBindings::current_defaults(),
                                &mut None,
                            );
                        });
                        assert!(result.response.rect.height() <= size.y);
                    }
                }
            },
        )
        .drop_without_applying_deltas();
}

#[test]
fn drops_are_routed_by_visible_clipped_regions_and_folder_rows() {
    let context = egui::Context::default();
    for (pointer, expected) in [
        (
            Pos2::new(40.0, 40.0),
            Some(FileDropTarget::Folder(PathBuf::from("/root/chapter"))),
        ),
        (Pos2::new(220.0, 40.0), Some(FileDropTarget::Editor)),
        (Pos2::new(390.0, 40.0), None),
    ] {
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                },
                |ui| {
                    let id = viewport_scoped_id(ui.ctx(), "file-drop-target");
                    ui.ctx().data_mut(|data| data.remove::<FileDropTarget>(id));
                    offer_file_drop_target(
                        ui,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(150.0, 150.0)),
                        FileDropTarget::Folder(PathBuf::from("/root")),
                    );
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(Rect::from_min_max(Pos2::ZERO, Pos2::new(150.0, 150.0))),
                    );
                    row_ui.set_clip_rect(row_ui.max_rect());
                    offer_folder_row_drop(
                        &row_ui,
                        Rect::from_min_max(Pos2::new(20.0, 30.0), Pos2::new(150.0, 50.0)),
                        Path::new("/root/chapter"),
                    );
                    offer_file_drop_target(
                        ui,
                        Rect::from_min_max(Pos2::new(150.0, 0.0), Pos2::new(300.0, 150.0)),
                        FileDropTarget::Editor,
                    );
                    assert_eq!(
                        ui.ctx().data(|data| data.get_temp::<FileDropTarget>(id)),
                        expected
                    );
                },
            )
            .drop_without_applying_deltas();
    }
}

#[test]
fn every_shared_menu_command_is_present_and_routes_from_the_popup() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    for menu in [CommandMenu::File, CommandMenu::Edit, CommandMenu::View] {
        for expected in command_specs(menu) {
            let shortcuts = ShortcutBindings::current_defaults();
            let label_context = egui::Context::default();
            label_context.set_os(egui::os::OperatingSystem::Nix);
            label_context
                .run_ui(egui::RawInput::default(), |_| {})
                .drop_without_applying_deltas();
            let label = format!(
                "{} {}",
                expected.title,
                shortcuts
                    .egui(expected.shortcut_action)
                    .map(|shortcut| label_context.format_shortcut(&shortcut))
                    .unwrap_or_default()
            );
            let mut harness = Harness::builder()
                .with_size(Vec2::new(450.0, 800.0))
                .build_ui_state(
                    move |ui, action| {
                        show_command_popup_ui(
                            ui,
                            menu,
                            CommandAvailability {
                                can_undo: true,
                                can_redo: true,
                                saved_document: true,
                                typst_document: true,
                                typst_preview: true,
                                interactive_preview: true,
                            },
                            &shortcuts,
                            action,
                        );
                    },
                    None::<AppPopupAction>,
                );
            harness.run();
            harness.get_by_label(&label).click();
            harness.run();
            assert!(
                matches!(harness.state(), Some(AppPopupAction::Command(actual)) if *actual == expected.command),
                "{}",
                expected.title
            );
        }
    }
}

#[test]
fn ready_asset_card_has_only_the_image_and_exact_image_margins() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let mut harness = Harness::builder()
        .with_size(Vec2::new(400.0, 300.0))
        .build_ui(|ui| {
            let texture = ui.ctx().load_texture(
                "asset-card-test",
                ColorImage::filled([100, 60], Color32::RED),
                TextureOptions::LINEAR,
            );
            let content = AssetHoverContent::Ready {
                texture,
                source_size: [100, 60],
            };
            assert_eq!(
                asset_hover_card_size(&content, Vec2::new(800.0, 600.0), Vec2::splat(16.0), 8.0),
                Vec2::new(116.0, 76.0)
            );
            show_asset_hover_contents(
                ui,
                Path::new("picture.png"),
                DocumentKind::Image,
                &content,
                Vec2::new(100.0, 60.0),
            );
        });
    harness.run();
    assert!(harness.query_by_label("picture.png").is_none());
    assert!(harness.query_by_label_contains("100 × 60").is_none());
}

#[test]
fn completion_trigger_accepts_typing_and_plain_deletion_only() {
    assert!(completion_requested_after_events(&[egui::Event::Text(
        "hea".to_owned()
    )]));
    assert!(completion_requested_after_events(&[egui::Event::Text(
        ".".to_owned()
    )]));
    assert!(completion_requested_after_events(&[egui::Event::Text(
        " ".to_owned()
    )]));
    assert!(completion_requested_after_events(&[egui::Event::Key {
        key: egui::Key::Backspace,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]));
    assert!(!completion_requested_after_events(&[egui::Event::Key {
        key: egui::Key::Backspace,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    }]));
}

#[test]
fn child_viewports_and_input_are_owned_by_each_document_window() {
    let context = egui::Context::default();
    let first = egui::ViewportId::ROOT;
    let second = egui::ViewportId::from_hash_of("second-editor");
    for salt in ["tiptoptyp-popup-overlay", "tiptoptyp-shortcuts"] {
        let first_child = egui::ViewportId::from_hash_of((first, salt));
        let second_child = egui::ViewportId::from_hash_of((second, salt));
        assert_ne!(first_child, second_child);
        for (owner, expected) in [(first, first), (second, second_child)] {
            let mut input = egui::RawInput {
                viewport_id: owner,
                ..Default::default()
            };
            input.viewports.entry(owner).or_default().focused = Some(false);
            input.viewports.entry(second_child).or_default().focused = Some(true);
            context
                .run_ui(input, |ui| {
                    assert_eq!(focused_input_viewport(ui.ctx()), expected);
                    assert_eq!(owns_focused_input_viewport(ui.ctx()), owner == second);
                    assert_eq!(
                        scoped_child_viewport_id(ui.ctx(), salt),
                        if owner == first {
                            first_child
                        } else {
                            second_child
                        }
                    );
                })
                .drop_without_applying_deltas();
        }
    }
}

#[test]
fn focused_viewport_and_standard_text_edit_commands_are_explicit() {
    let context = egui::Context::default();
    let child = scoped_child_viewport_id(&context, "tiptoptyp-shortcuts");
    assert_ne!(
        child,
        scoped_child_viewport_id(&context, "tiptoptyp-settings")
    );
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(false);
    input.viewports.entry(child).or_default().focused = Some(true);
    context
        .run_ui(input, |ui| {
            assert_eq!(focused_input_viewport(ui.ctx()), child);
        })
        .drop_without_applying_deltas();

    let context = egui::Context::default();
    let sibling_document = egui::ViewportId::from_hash_of("other-document");
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(false);
    input.viewports.entry(sibling_document).or_default().focused = Some(true);
    context
        .run_ui(input, |ui| {
            assert_eq!(focused_input_viewport(ui.ctx()), egui::ViewportId::ROOT);
        })
        .drop_without_applying_deltas();

    assert_eq!(
        standard_text_edit_shortcut(AppCommand::Undo),
        Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z))
    );
    assert_eq!(
        standard_text_edit_shortcut(AppCommand::Redo),
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::Z,
        ))
    );
    assert_eq!(
        standard_text_edit_shortcut(AppCommand::SelectAll),
        Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::A))
    );
    assert_eq!(standard_text_edit_shortcut(AppCommand::Copy), None);
}

#[test]
fn semantic_clipboard_events_obey_effective_shortcuts_and_capture() {
    use crate::shortcuts::ShortcutOverrides;

    let defaults = ShortcutBindings::current_defaults();
    let mut copy = egui::InputState::default();
    copy.modifiers = Modifiers::COMMAND;
    copy.events = vec![egui::Event::Copy];
    assert!(!normalize_text_edit_shortcut_events(
        &mut copy, &defaults, false
    ));
    assert_eq!(copy.events, [egui::Event::Copy]);

    let mut capture = egui::InputState::default();
    capture.modifiers = Modifiers::COMMAND;
    capture.events = vec![egui::Event::Copy];
    assert_eq!(
        take_shortcut_capture_event(&mut capture),
        Some((egui::Key::C, Modifiers::COMMAND))
    );
    assert!(capture.events.is_empty());

    let mut overrides = ShortcutOverrides::default();
    overrides.set(ShortcutAction::Copy, None);
    let disabled = ShortcutBindings::current(&overrides);
    let mut ignored = egui::InputState::default();
    ignored.modifiers = Modifiers::COMMAND;
    ignored.events = vec![egui::Event::Copy];
    normalize_text_edit_shortcut_events(&mut ignored, &disabled, false);
    assert!(ignored.events.is_empty());

    let mut overrides = ShortcutOverrides::default();
    overrides.assign(
        ShortcutAction::Copy,
        ShortcutChord::primary(egui::Key::X),
        ShortcutPlatform::current(),
    );
    let rebound = ShortcutBindings::current(&overrides);
    let mut cut_chord = egui::InputState::default();
    cut_chord.modifiers = Modifiers::COMMAND;
    cut_chord.events = vec![egui::Event::Cut];
    normalize_text_edit_shortcut_events(&mut cut_chord, &rebound, false);
    assert!(matches!(
        cut_chord.events.as_slice(),
        [egui::Event::Key {
            key: egui::Key::X,
            pressed: true,
            modifiers,
            ..
        }] if *modifiers == Modifiers::COMMAND
    ));
}

#[test]
fn requested_paste_bypasses_rebound_primary_v_exactly_once() {
    use crate::shortcuts::ShortcutOverrides;

    let mut overrides = ShortcutOverrides::default();
    overrides.set(ShortcutAction::Paste, None);
    let shortcuts = ShortcutBindings::current(&overrides);
    let mut input = egui::InputState::default();
    input.events = vec![egui::Event::Paste("fresh".to_owned())];
    assert!(normalize_text_edit_shortcut_events(
        &mut input, &shortcuts, true
    ));
    assert_eq!(input.events, [egui::Event::Paste("fresh".to_owned())]);

    assert!(!normalize_text_edit_shortcut_events(
        &mut input, &shortcuts, false
    ));
    assert!(input.events.is_empty());
}

#[test]
fn requested_paste_admission_is_viewport_local_and_survives_multiple_frames() {
    let target = egui::ViewportId::from_hash_of("paste-target");
    let sibling = egui::ViewportId::from_hash_of("paste-sibling");
    let requested = PendingWidgetPaste::new(target, 10);

    assert!(requested.admits(target, 10));
    assert!(requested.admits(target, 14));
    assert!(requested.admits(target, 10 + WIDGET_PASTE_ADMISSION_FRAME_BUDGET));
    assert!(!requested.admits(sibling, 14));
    assert!(!requested.expired(10 + WIDGET_PASTE_ADMISSION_FRAME_BUDGET));
    assert!(requested.expired(11 + WIDGET_PASTE_ADMISSION_FRAME_BUDGET));
}

#[test]
fn disabled_text_edit_key_commands_are_removed_before_widgets() {
    let mut input = egui::InputState::default();
    input.events = vec![
        egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        },
        egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        },
        egui::Event::Text("kept".to_owned()),
    ];
    remove_unhandled_text_edit_builtin_events(&mut input);
    assert_eq!(input.events, [egui::Event::Text("kept".to_owned())]);
}

#[test]
fn completion_popup_prefers_below_then_flips_and_clamps() {
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0));
    let size = Vec2::new(100.0, 60.0);
    let near_top = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(1.0, 16.0));
    assert_eq!(
        completion_popup_position(near_top, size, viewport),
        Pos2::new(20.0, near_top.bottom() + theme::SPACE.tight)
    );

    let near_bottom = Rect::from_min_size(Pos2::new(290.0, 180.0), Vec2::new(1.0, 16.0));
    assert_eq!(
        completion_popup_position(near_bottom, size, viewport),
        Pos2::new(196.0, near_bottom.top() - theme::SPACE.tight - size.y)
    );
}

#[test]
fn completion_responses_require_exact_request_and_document_identity() {
    let pending = EditorCompletionState {
        generation: Generation(7),
        uri: "file:///project/main.typ".to_owned(),
        version: 12,
        request_token: 41,
        cursor: 3,
        anchor: Rect::ZERO,
        explicit: false,
        is_incomplete: false,
        selected: 0,
        items: Vec::new(),
        all_items: Vec::new(),
        source: String::new(),
        local: false,
    };
    assert!(completion_response_matches(
        &pending,
        Generation(7),
        "file:///project/main.typ",
        12,
        41,
        Some(Generation(7)),
        Some("file:///project/main.typ"),
        12,
    ));
    for (generation, uri, version, token, active_generation, active_uri, active_version) in [
        (
            Generation(8),
            "file:///project/main.typ",
            12,
            41,
            Some(Generation(8)),
            Some("file:///project/main.typ"),
            12,
        ),
        (
            Generation(7),
            "file:///project/other.typ",
            12,
            41,
            Some(Generation(7)),
            Some("file:///project/other.typ"),
            12,
        ),
        (
            Generation(7),
            "file:///project/main.typ",
            11,
            41,
            Some(Generation(7)),
            Some("file:///project/main.typ"),
            11,
        ),
        (
            Generation(7),
            "file:///project/main.typ",
            12,
            40,
            Some(Generation(7)),
            Some("file:///project/main.typ"),
            12,
        ),
        (
            Generation(7),
            "file:///project/main.typ",
            12,
            41,
            Some(Generation(7)),
            Some("file:///project/main.typ"),
            13,
        ),
    ] {
        assert!(!completion_response_matches(
            &pending,
            generation,
            uri,
            version,
            token,
            active_generation,
            active_uri,
            active_version,
        ));
    }
}

#[test]
fn completion_snippets_expand_placeholders_choices_and_cursor() {
    assert_eq!(
        expand_lsp_snippet("heading(${1:body}, ${2|red,blue|})$0").unwrap(),
        SnippetExpansion {
            text: "heading(body, red)".to_owned(),
            cursor: "heading(".chars().count(),
        }
    );
    assert_eq!(
        expand_lsp_snippet(r"\$cash ${1:value}").unwrap(),
        SnippetExpansion {
            text: "$cash value".to_owned(),
            cursor: "$cash ".chars().count(),
        }
    );
    assert_eq!(
        expand_lsp_snippet("done$0").unwrap(),
        SnippetExpansion {
            text: "done".to_owned(),
            cursor: 4,
        }
    );
    assert!(expand_lsp_snippet("${1:unfinished").is_err());
}

#[test]
fn completion_without_server_range_replaces_only_identifier_prefix() {
    let applied = prepare_completion_application("#hea", 4, &completion_item("heading")).unwrap();
    assert_eq!(
        applied,
        CompletionApplication {
            source: "#heading".to_owned(),
            cursor: 8,
        }
    );
}

#[test]
fn completion_applies_snippet_and_additional_edits_atomically() {
    let mut item = completion_item("ignored fallback");
    item.label = "bar".to_owned();
    item.insert_text_is_snippet = true;
    item.text_edit = Some(LspTextEdit {
        range: LspRange {
            start: LspPosition::new(1, 0),
            end: LspPosition::new(1, 2),
        },
        new_text: "bar(${1:x})$0".to_owned(),
    });
    item.additional_text_edits.push(LspTextEdit {
        range: LspRange {
            start: LspPosition::new(0, 0),
            end: LspPosition::new(0, 0),
        },
        new_text: "#let helper = 1\n".to_owned(),
    });

    let applied = prepare_completion_application("foo\nba", 6, &item).unwrap();
    assert_eq!(applied.source, "#let helper = 1\nfoo\nbar(x)");
    assert_eq!(
        applied.cursor,
        applied
            .source
            .chars()
            .position(|character| character == 'x')
            .unwrap()
    );
}

#[test]
fn completion_rejects_invalid_and_overlapping_server_edits() {
    let mut item = completion_item("replacement");
    item.text_edit = Some(LspTextEdit {
        range: LspRange {
            start: LspPosition::new(0, 1),
            end: LspPosition::new(0, 3),
        },
        new_text: "ok".to_owned(),
    });
    item.additional_text_edits.push(LspTextEdit {
        range: LspRange {
            start: LspPosition::new(0, 2),
            end: LspPosition::new(0, 2),
        },
        new_text: "overlap".to_owned(),
    });
    assert!(
        prepare_completion_application("abcd", 3, &item)
            .unwrap_err()
            .contains("overlap")
    );

    item.additional_text_edits.clear();
    item.text_edit.as_mut().unwrap().range.start.character =
        tiptoptyp_core::text::Utf16Column::new(99);
    item.text_edit.as_mut().unwrap().range.end.character =
        tiptoptyp_core::text::Utf16Column::new(100);
    assert!(prepare_completion_application("abcd", 3, &item).is_err());
    assert!(prepare_completion_application("abcd", 99, &completion_item("x")).is_err());
}

#[test]
fn first_frame_hover_state_installs_without_reentering_the_context_lock() {
    let context = egui::Context::default();
    let delay = Duration::from_millis(275);

    context
        .run_ui(egui::RawInput::default(), |ui| {
            // This is the same first-frame operation that once prevented
            // the native window from ever painting. epaint detects a
            // nested context lock as a deadlock, so completing this call
            // is the regression assertion.
            install_hover_runtime_config(ui.ctx(), delay);
            clear_asset_hover_candidate(ui.ctx());
            clear_native_hover_overlay(ui.ctx());
            assert!(current_asset_hover_candidate(ui.ctx()).is_none());
        })
        .drop_without_applying_deltas();

    let id = hover_runtime_config_id(&context);
    let installed = context
        .data(|data| data.get_temp::<HoverRuntimeConfig>(id))
        .expect("the first frame should publish hover timing state");
    assert_eq!(installed.delay, delay);
}

#[test]
fn designated_typst_entry_remains_visible_while_editing_other_file_kinds() {
    for document_kind in [
        DocumentKind::Typst,
        DocumentKind::Text,
        DocumentKind::Pdf,
        DocumentKind::Image,
    ] {
        assert!(typst_preview_available_for(document_kind, true));
        assert!(preview_visible_for(document_kind, ViewMode::Split, true));
    }

    assert!(!typst_preview_available_for(DocumentKind::Text, false));
    assert!(!preview_visible_for(
        DocumentKind::Text,
        ViewMode::Split,
        false
    ));
    assert!(!preview_visible_for(
        DocumentKind::Text,
        ViewMode::Code,
        true
    ));
    assert!(preview_visible_for(
        DocumentKind::Pdf,
        ViewMode::Code,
        false
    ));
}

#[test]
fn interactive_preview_does_not_duplicate_raster_compilation() {
    assert!(!raster_preview_required_for(true, false, false));
    assert!(raster_preview_required_for(true, true, false));
    assert!(raster_preview_required_for(true, false, true));
    assert!(raster_preview_required_for(false, false, false));
}

#[test]
fn local_webview_failure_schedules_the_first_raster_fallback() {
    let raster_was_required = raster_preview_required_for(true, false, false);
    let raster_is_required = raster_preview_required_for(true, true, false);

    assert!(raster_fallback_compile_needed(
        true,
        true,
        raster_was_required,
        raster_is_required,
    ));
    assert!(!raster_fallback_compile_needed(
        false,
        true,
        raster_was_required,
        raster_is_required,
    ));
    assert!(!raster_fallback_compile_needed(
        true,
        false,
        raster_was_required,
        raster_is_required,
    ));
    assert!(!raster_fallback_compile_needed(true, true, true, true));
}

#[test]
fn raster_results_require_the_matching_current_artifact() {
    let old = ArtifactKey {
        revision: 9,
        generation: 41,
    };
    let newer_same_revision = ArtifactKey {
        revision: 9,
        generation: 42,
    };
    assert!(raster_result_matches_artifact(
        newer_same_revision,
        9,
        Some(newer_same_revision)
    ));
    assert!(!raster_result_matches_artifact(
        old,
        9,
        Some(newer_same_revision)
    ));
    assert!(!raster_result_matches_artifact(old, 10, Some(old)));
    assert!(!raster_result_matches_artifact(old, 9, None));
}

#[test]
fn same_revision_raster_from_an_older_artifact_is_stale_for_ui_actions() {
    let old = ArtifactKey {
        revision: 9,
        generation: 41,
    };
    let newer_same_revision = ArtifactKey {
        revision: 9,
        generation: 42,
    };

    assert_eq!(
        raster_content_freshness(true, Some(old), 9, Some(newer_same_revision)),
        Some(RasterContentFreshness::Stale)
    );
    assert_eq!(
        raster_content_freshness(
            true,
            Some(newer_same_revision),
            9,
            Some(newer_same_revision)
        ),
        Some(RasterContentFreshness::Current)
    );
    assert_eq!(
        raster_content_freshness(false, Some(old), 9, Some(newer_same_revision)),
        None
    );
}

#[test]
fn egui_color_conversion_preserves_unmultiplied_alpha_channels() {
    // Use channels that survive Color32's quantized premultiplied storage exactly.
    let color = Color32::from_rgba_unmultiplied(255, 34, 68, 128);
    assert_eq!(rgba_from_color(color), Rgba::from_rgba(255, 34, 68, 128));
    assert_eq!(color_from_rgba(rgba_from_color(color)), color);
}

#[test]
fn pausing_blocks_automatic_builds_but_not_explicit_pdf_or_capture_work() {
    assert!(compilation_run_allowed(false, false, false));
    assert!(!compilation_run_allowed(true, false, false));
    assert!(compilation_run_allowed(true, true, false));
    assert!(compilation_run_allowed(true, false, true));
    assert_eq!(tinymist_preview_refresh(false), PreviewRefresh::OnType);
    assert_eq!(tinymist_preview_refresh(true), PreviewRefresh::OnSave);
    assert!(tinymist_language_features_ready(
        DocumentKind::Typst,
        true,
        true
    ));
    assert!(!tinymist_language_features_ready(
        DocumentKind::Typst,
        false,
        true
    ));
    assert!(!tinymist_language_features_ready(
        DocumentKind::Typst,
        true,
        false
    ));
    assert!(!tinymist_language_features_ready(
        DocumentKind::Text,
        true,
        true
    ));
    assert_eq!(
        compilation_toggle_copy(false),
        (
            "Pause",
            "Pause automatic preview updates; Compile PDF stays available"
        )
    );
    assert_eq!(
        compilation_toggle_copy(true),
        ("Resume", "Resume automatic preview updates")
    );
    assert_eq!(
        compilation_notice(true),
        ("Automatic preview updates paused", NoticeKind::Info)
    );
    assert_eq!(
        compilation_notice(false),
        ("Automatic preview updates resumed", NoticeKind::Success)
    );
}

#[test]
fn compile_writes_beside_the_effective_saved_typst_entry() {
    assert_eq!(
        default_compile_pdf_path(
            None,
            DocumentKind::Typst,
            Some(Path::new("/project/chapters/main.typ")),
        ),
        Some(PathBuf::from("/project/chapters/main.pdf"))
    );
    assert_eq!(
        default_compile_pdf_path(
            Some(Path::new("/project/book.typ")),
            DocumentKind::Text,
            Some(Path::new("/project/metadata.toml")),
        ),
        Some(PathBuf::from("/project/book.pdf"))
    );
    assert_eq!(
        default_compile_pdf_path(None, DocumentKind::Typst, None),
        None,
        "an unsaved document must ask the user for an output path"
    );
    assert_eq!(
        default_compile_pdf_path(
            None,
            DocumentKind::Text,
            Some(Path::new("/project/notes.txt")),
        ),
        None
    );
}

#[test]
fn compiling_snapshot_hides_pages_without_destroying_shared_raster_state() {
    assert!(snapshot_scene_hides_preview_pages(Some(
        UiSnapshotScene::PreviewCompiling
    )));
    assert!(!snapshot_scene_hides_preview_pages(Some(
        UiSnapshotScene::ProblemsPanel
    )));
    assert!(!snapshot_scene_hides_preview_pages(None));
}

#[test]
fn raster_gated_snapshot_scenes_do_not_clobber_an_in_flight_build() {
    for scene in [
        UiSnapshotScene::Main,
        UiSnapshotScene::ProblemsPanel,
        UiSnapshotScene::FindReplace,
    ] {
        assert_eq!(settled_snapshot_preview_status(scene, false), None);
    }
    assert_eq!(
        settled_snapshot_preview_status(UiSnapshotScene::Main, true),
        Some(PreviewStatus::Ready(Duration::ZERO))
    );
    assert_eq!(
        settled_snapshot_preview_status(UiSnapshotScene::ProblemsPanel, true),
        Some(PreviewStatus::Error)
    );
}

#[test]
fn pending_main_capture_queues_only_one_build_while_waiting_for_its_raster() {
    assert!(capture_preview_build_needed(
        true,
        false,
        false,
        PreviewStatus::Waiting,
        false,
    ));
    assert!(!capture_preview_build_needed(
        true,
        false,
        true,
        PreviewStatus::Waiting,
        false,
    ));
    assert!(!capture_preview_build_needed(
        true,
        false,
        false,
        PreviewStatus::Compiling,
        false,
    ));
    assert!(!capture_preview_build_needed(
        true,
        false,
        false,
        PreviewStatus::Ready(Duration::ZERO),
        true,
    ));
    assert!(!capture_preview_build_needed(
        true,
        false,
        false,
        PreviewStatus::Error,
        false,
    ));
}

#[test]
fn pinned_restart_retains_exactly_one_existing_interactive_surface() {
    assert!(retain_preview_surface_for_restart(true, true, true, true));
    assert!(!retain_preview_surface_for_restart(false, true, true, true));
    assert!(!retain_preview_surface_for_restart(true, false, true, true));
    assert!(!retain_preview_surface_for_restart(true, true, false, true));
    assert!(!retain_preview_surface_for_restart(true, true, true, false));
}

#[test]
fn replacement_preview_server_forces_navigation_even_when_its_url_is_reused() {
    let url = "http://127.0.0.1:4173/preview";
    assert!(!webview_navigation_required(Some(url), url, false));
    assert!(webview_navigation_required(Some(url), url, true));
    assert!(webview_navigation_required(
        Some(url),
        "http://127.0.0.1:4174/preview",
        false
    ));
}

#[test]
fn command_clicking_a_link_takes_priority_over_source_preview_sync() {
    assert!(!source_preview_jump_gesture(
        SourcePreviewTrigger::ModifierClick,
        true,
        false,
        true,
        true,
    ));
    assert!(source_preview_jump_gesture(
        SourcePreviewTrigger::ModifierClick,
        true,
        false,
        true,
        false,
    ));
    assert!(source_preview_jump_gesture(
        SourcePreviewTrigger::DoubleClick,
        false,
        true,
        false,
        false,
    ));
}

#[test]
fn cursor_coordinates_are_one_based_and_unicode_scalar_aware() {
    let source = "a🦀b\nsecond";
    assert_eq!(line_column_at_char(source, 0), (1, 1));
    assert_eq!(line_column_at_char(source, 2), (1, 3));
    assert_eq!(line_column_at_char(source, 4), (2, 1));
    assert_eq!(line_column_at_char(source, usize::MAX), (2, 7));
}

#[test]
fn semantic_hover_tracks_identifiers_and_nearby_call_parentheses() {
    let source = "#text(fill: blue)[hello]";
    let mut data = hover_test_data(source);
    assert_eq!(data.hover_token_range(2), Some(1..5));
    assert_eq!(data.hover_token_range(5), Some(1..5));
    assert_eq!(data.hover_token_range(8), Some(6..10));
    assert_eq!(data.hover_token_range(0), None);
    assert_eq!(hover_test_data("").hover_token_range(0), None);
}

fn hover_test_data(source: &str) -> crate::editor_data::EditorDerivedData {
    let mut data = crate::editor_data::EditorDerivedData::default();
    data.prepare_source(&crate::document::DocumentSnapshot::fixture(
        DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1),
        source,
    ));
    data
}

#[test]
fn semantic_hover_respects_math_operator_boundaries_not_symbol_names() {
    for (source, target) in [
        ("$ v^L in C^oo([epsilon,t_*];H_sigma^k) $", "sigma"),
        ("$ v^L in C^oo([epsilon,t_*];epsilon^k) $", "epsilon"),
        ("$ A_beta + delta-theta + x_gamma.alt $", "beta"),
        ("$ A_beta + delta-theta + x_gamma.alt $", "theta"),
        ("$ A_beta + delta-theta + x_gamma.alt $", "gamma"),
        ("$ A_beta + delta-theta + x_gamma.alt $", "alt"),
        ("$ A_gamma + sigma-epsilon $", "epsilon"),
        ("$ A_gamma + sigma-epsilon $", "sigma"),
        ("$ alpha^beta_(gamma/delta) $", "alpha"),
        ("$ alpha^beta_(gamma/delta) $", "beta"),
        ("$ alpha^beta_(gamma/delta) $", "gamma"),
        ("$ alpha^beta_(gamma/delta) $", "delta"),
        ("🦀 café $ H_sigma^k $", "sigma"),
        ("#let code_name-long = 1", "code_name-long"),
        ("#sym.arrow.r", "arrow"),
        ("#sym.alpha.r", "r"),
    ] {
        let byte = source.find(target).unwrap();
        let start = source[..byte].chars().count();
        let mut data = hover_test_data(source);
        // Include the insertion position after the final glyph, where egui
        // places a pointer on that glyph's right half, whatever follows it.
        for cursor in start..=start + target.chars().count() {
            assert_eq!(
                data.hover_token_range(cursor),
                Some(start..start + target.chars().count()),
                "source={source:?} cursor={cursor}"
            );
        }
    }
}

#[test]
#[ignore = "requires a real Tinymist executable; set TIPTOPTYP_TEST_TINYMIST to override discovery"]
fn real_tinymist_hover_targets_respect_math_syntax_boundaries() {
    fn wait_for(
        sidecar: &TinymistSidecar,
        accept: impl Fn(&TinymistEvent) -> bool,
    ) -> TinymistEvent {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            while let Some(event) = sidecar.try_recv() {
                assert!(
                    !matches!(&event, TinymistEvent::Error { fatal: true, .. }),
                    "Tinymist failed: {event:?}"
                );
                if accept(&event) {
                    return event;
                }
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for Tinymist hover probe"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let program = std::env::var_os("TIPTOPTYP_TEST_TINYMIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_tool(ToolKind::Tinymist, &ToolPreference::default()).program);
    let project = tempfile::tempdir().unwrap();
    let source = "$ v^L in C^oo([epsilon,t_*];H_sigma^k) $\n$ v^L in C^oo([epsilon,t_*];epsilon^k) $\n$ A_beta + delta-theta $";
    let backing =
        UnsavedTextDocument::create(project.path(), project.path(), "Hover.typ", source).unwrap();
    let sidecar = TinymistSidecar::new(crate::worker::RepaintTarget::test());
    let mut config = TinymistConfig::new(project.path()).with_executable(program);
    config.start_preview = false;
    let generation = sidecar.start_workspace(config).unwrap();
    wait_for(&sidecar, |event| {
        matches!(event, TinymistEvent::Initialized { .. })
    });
    let document = backing.text_document(1, source);
    let uri = document.uri.clone();
    sidecar.did_open(generation, document).unwrap();
    let mut data = hover_test_data(source);
    let mut token = 0;
    for name in ["epsilon", "sigma", "beta", "theta"] {
        for (byte, _) in source.match_indices(name) {
            token += 1;
            let start = source[..byte].chars().count();
            let range = data.hover_token_range(start + 1).unwrap();
            let position = lsp_position_at_scalar(source, ScalarOffset::new(range.start));
            sidecar
                .hover_document(generation, uri.clone(), 1, position, token)
                .unwrap();
            let event = wait_for(
                &sidecar,
                |event| matches!(event, TinymistEvent::Hovered { request_token, .. } if *request_token == token),
            );
            let TinymistEvent::Hovered { contents, .. } = event else {
                unreachable!()
            };
            assert!(
                contents.is_some_and(|text| !text.trim().is_empty()),
                "No hover at {name} byte {byte}"
            );
        }
    }
    sidecar.did_close(generation, uri).unwrap();
    sidecar.stop_workspace(generation).unwrap();
    wait_for(&sidecar, |event| {
        matches!(event, TinymistEvent::Stopped { .. })
    });
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn web_navigation_keeps_preview_routes_embedded_but_rejects_other_origins() {
    let base = "http://127.0.0.1:4173/preview/index.html";
    assert!(same_web_origin(
        base,
        "http://127.0.0.1:4173/assets/page.svg"
    ));
    assert!(same_web_origin(base, "http://127.0.0.1:4173/#page=12"));
    assert!(!same_web_origin(base, "https://typst.app/docs"));
    assert!(!same_web_origin(base, "http://127.0.0.1:4174/preview"));
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn reused_preview_webview_routes_against_the_current_server_origin() {
    let mut context = PreviewNavigationContext {
        base_url: "http://127.0.0.1:4173/preview".to_owned(),
        project_root: PathBuf::from("/tmp/project"),
        source_dir: Some(PathBuf::from("/tmp/project/chapters")),
    };
    let replacement = "http://127.0.0.1:4174/preview";
    assert_eq!(
        preview_navigation_action(&context, replacement),
        PreviewNavigationAction::Dispatch(replacement.to_owned())
    );

    context.base_url = replacement.to_owned();
    assert_eq!(
        preview_navigation_action(&context, replacement),
        PreviewNavigationAction::Embed
    );
    assert_eq!(
        preview_navigation_action(&context, "https://example.com/docs"),
        PreviewNavigationAction::Dispatch("https://example.com/docs".to_owned())
    );
    assert_eq!(
        preview_new_window_target(&context, "https://example.com/docs"),
        Some("https://example.com/docs".to_owned())
    );
}

#[test]
fn linked_pdf_pages_are_converted_from_one_based_urls() {
    assert_eq!(
        pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf#page=7").unwrap()),
        Some(6)
    );
    assert_eq!(
        pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf?page=3").unwrap()),
        Some(2)
    );
    assert_eq!(
        pdf_page_from_url(&url::Url::parse("file:///tmp/paper.pdf#section").unwrap()),
        None
    );
    assert_eq!(internal_pdf_page_target("#page=9"), Some(8));
}

#[test]
fn image_texture_rebuild_uses_raster_not_logical_dimensions() {
    let rgba = vec![255; 2 * 3 * 4];
    let image = preview_color_image([2, 3], &rgba);
    assert_eq!(image.size, [2, 3]);
    assert_eq!(image.pixels.len(), 6);
}

#[test]
fn queued_exports_follow_edits_but_never_cross_documents() {
    let mut pending = Some(PendingExport {
        path: PathBuf::from("old-document.pdf"),
        document_epoch: 7,
        intent: PdfWriteIntent::Export,
        after_artifact_generation: None,
    });
    assert_eq!(
        take_ready_export(
            &mut pending,
            8,
            12,
            Some(ArtifactKey {
                revision: 12,
                generation: 41,
            }),
        ),
        None
    );
    assert!(pending.is_none());

    let expected = PendingExport {
        path: PathBuf::from("current-document.pdf"),
        document_epoch: 8,
        intent: PdfWriteIntent::Compile,
        after_artifact_generation: None,
    };
    let mut pending = Some(expected.clone());
    assert_eq!(
        take_ready_export(
            &mut pending,
            8,
            13,
            Some(ArtifactKey {
                revision: 13,
                generation: 42,
            }),
        ),
        Some(expected)
    );
}

#[test]
fn queued_export_waits_for_a_strictly_newer_current_artifact() {
    let expected = PendingExport {
        path: PathBuf::from("rebuilt-document.pdf"),
        document_epoch: 8,
        intent: PdfWriteIntent::Export,
        after_artifact_generation: Some(41),
    };
    let mut pending = Some(expected.clone());

    assert_eq!(
        take_ready_export(
            &mut pending,
            8,
            13,
            Some(ArtifactKey {
                revision: 13,
                generation: 41,
            }),
        ),
        None,
        "the cached same-revision artifact must not satisfy the export"
    );
    assert!(pending.is_some());
    assert_eq!(
        take_ready_export(
            &mut pending,
            8,
            13,
            Some(ArtifactKey {
                revision: 12,
                generation: 42,
            }),
        ),
        None,
        "a newer artifact for a stale editor revision must not satisfy it"
    );
    assert!(pending.is_some());
    assert_eq!(
        take_ready_export(
            &mut pending,
            8,
            13,
            Some(ArtifactKey {
                revision: 13,
                generation: 42,
            }),
        ),
        Some(expected)
    );
}

#[test]
fn pdf_output_reuses_only_a_stable_current_artifact() {
    let current = Some(ArtifactKey {
        revision: 13,
        generation: 41,
    });

    let stable =
        pdf_output_requires_new_artifact(true, false, false, PreviewStatus::Ready(Duration::ZERO));
    assert!(!stable);
    assert!(pdf_artifact_reusable_for_output(current, true, 13, stable));

    for requires_new in [
        pdf_output_requires_new_artifact(true, true, false, PreviewStatus::Ready(Duration::ZERO)),
        pdf_output_requires_new_artifact(true, false, true, PreviewStatus::Ready(Duration::ZERO)),
        pdf_output_requires_new_artifact(true, false, false, PreviewStatus::Waiting),
        pdf_output_requires_new_artifact(true, false, false, PreviewStatus::Compiling),
        pdf_output_requires_new_artifact(true, false, false, PreviewStatus::Error),
    ] {
        assert!(requires_new);
        assert!(!pdf_artifact_reusable_for_output(
            current,
            true,
            13,
            requires_new
        ));
    }

    assert!(!pdf_artifact_reusable_for_output(current, false, 13, false));
    assert!(!pdf_artifact_reusable_for_output(current, true, 14, false));
}

#[test]
fn typst_link_fragments_map_to_unicode_source_positions() {
    let url = url::Url::parse("file:///tmp/chapter.typ#line=2:2").unwrap();
    assert_eq!(source_position_from_url(&url), Some((2, 2)));
    assert_eq!(char_index_at_line_column("one\n🦀two\n", 2, 2), 5);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn preview_server_document_links_route_back_to_project_files() {
    let project = tempfile::tempdir().unwrap();
    let source_dir = project.path().join("chapters");
    let linked = project.path().join("assets/other paper.pdf");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::fs::write(&linked, b"%PDF-test").unwrap();

    let routed = preview_document_file_url(
        "http://127.0.0.1:4173/assets/other%20paper.pdf#page=4",
        project.path(),
        Some(&source_dir),
    )
    .unwrap();
    let routed = url::Url::parse(&routed).unwrap();
    assert_eq!(
        routed.to_file_path().unwrap(),
        linked.canonicalize().unwrap()
    );
    assert_eq!(routed.fragment(), Some("page=4"));
    assert!(
        preview_document_file_url(
            "http://127.0.0.1:4173/assets/viewer.js",
            project.path(),
            Some(&source_dir),
        )
        .is_none()
    );
}

#[test]
fn atomic_write_replaces_a_file_with_exact_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.pdf");
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"%PDF-exact\0bytes").unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"%PDF-exact\0bytes");
}

#[test]
fn autosave_requires_a_known_disk_fingerprint() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.typ");
    std::fs::write(&path, "= Original").unwrap();
    assert!(!disk_matches_fingerprint(&path, None));
    assert!(disk_matches_fingerprint(
        &path,
        Some(fingerprint(b"= Original"))
    ));
    assert!(!disk_matches_fingerprint(
        &path,
        Some(fingerprint(b"= Different"))
    ));
}

#[test]
fn project_root_uses_nearest_marker() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    let source = project.join("chapters");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    assert_eq!(discover_project_root(&source), project);
}

#[test]
fn opening_a_file_only_invalidates_the_tree_when_the_workspace_changes() {
    let root = Path::new("/project");
    assert!(preserve_workspace_snapshot_for_open(
        Some(root),
        root,
        Path::new("/project/chapters/intro.typ")
    ));
    assert!(!preserve_workspace_snapshot_for_open(
        Some(root),
        root,
        Path::new("/another-project/main.typ")
    ));
    assert!(!preserve_workspace_snapshot_for_open(
        None,
        root,
        Path::new("/project/main.typ")
    ));
}

#[test]
fn first_workspace_switch_rejects_the_previous_roots_snapshot() {
    let previous_root = Path::new("/workspaces/mytypst");
    let selected_root = Path::new("/workspaces/log-illposed-ns");
    let remembered_document = selected_root.join("main-notation-aligned.typ");

    assert!(!preserve_workspace_snapshot_for_open(
        Some(previous_root),
        selected_root,
        &remembered_document,
    ));
}

#[test]
fn workspace_font_refresh_matches_catalog_directory_exclusions() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("assets/fonts")).unwrap();
    std::fs::create_dir_all(directory.path().join("node_modules/package")).unwrap();
    std::fs::write(directory.path().join("assets/fonts/local.otf"), b"font").unwrap();
    std::fs::write(
        directory.path().join("node_modules/package/ignored.ttf"),
        b"font",
    )
    .unwrap();
    std::fs::write(directory.path().join("notes.txt"), b"not a font").unwrap();

    let snapshot = WorkspaceSnapshot::scan(directory.path()).unwrap();
    let files = workspace_snapshot_font_files(&snapshot);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("assets/fonts/local.otf"));
}

#[test]
fn compact_paths_keep_the_tail_and_unicode_character_budget() {
    let path = "/Users/example/Documents/文稿/chapters/introduction.typ";
    let compact = tail_elide(path, 18);
    assert!(compact.starts_with('…'));
    assert!(compact.ends_with("introduction.typ"));
    assert_eq!(compact.chars().count(), 18);
}

#[test]
fn workspace_copy_actions_produce_name_absolute_and_root_relative_text() {
    let root = Path::new("/project");
    let path = root.join("docs/guide.typ");

    assert_eq!(
        workspace_copy_text(&path, root, WorkspaceCopyKind::FileName).as_deref(),
        Some("guide.typ")
    );
    assert_eq!(
        workspace_copy_text(&path, root, WorkspaceCopyKind::FilePath).as_deref(),
        Some("/project/docs/guide.typ")
    );
    assert_eq!(
        workspace_copy_text(&path, root, WorkspaceCopyKind::RelativePath).as_deref(),
        Some("docs/guide.typ")
    );
    assert_eq!(
        workspace_copy_text(
            Path::new("/elsewhere/guide.typ"),
            root,
            WorkspaceCopyKind::RelativePath,
        ),
        None
    );
}

#[test]
fn file_context_menu_exposes_each_copy_contract() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let path = PathBuf::from("/project/docs/guide.typ");
    let expected_path = path.clone();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(320.0, 360.0))
        .build_ui_state(
            move |ui, action| {
                show_workspace_popup_ui(ui, &path, true, false, action);
            },
            None::<AppPopupAction>,
        );
    harness.run();

    for label in ["Copy File Name", "Copy File Path", "Copy Relative Path"] {
        assert!(
            harness.query_by_label_contains(label).is_some(),
            "missing {label}"
        );
    }
    assert!(harness.query_by_label_contains(reveal_label()).is_some());
    harness.get_by_label_contains("Copy Relative Path").click();
    harness.run();

    match harness.state() {
        Some(AppPopupAction::Workspace(WorkspaceMenuAction::Copy {
            path,
            kind: WorkspaceCopyKind::RelativePath,
        })) => assert_eq!(path, &expected_path),
        other => panic!("unexpected context-menu action: {other:?}"),
    }
    assert_eq!(
        workspace_context_menu_size(true, &egui::Style::default()).y,
        workspace_context_menu_size(false, &egui::Style::default()).y
            + (METRICS.menu.row_height + theme::SPACE.small) * 2.0
    );
}

#[test]
fn recent_workspace_context_menu_exposes_removal_without_selecting_the_row() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let path = PathBuf::from("/project/recent");
    let mut harness = Harness::builder()
        .with_size(Vec2::new(360.0, 180.0))
        .build_ui_state(
            move |ui, action| {
                if let Some(next) = show_recent_workspace_row(ui, &path, 340.0) {
                    *action = Some(next);
                }
            },
            None::<RecentWorkspaceAction>,
        );
    harness.run();
    harness
        .get_by_label_contains("/project/recent")
        .click_secondary();
    harness.step();
    assert_eq!(harness.state(), &None);
    assert!(harness.query_by_label("Remove from Recents").is_some());
}

#[test]
fn title_bar_file_and_view_popups_expose_their_dynamic_actions() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let shortcuts = ShortcutBindings::current_defaults();
    let path = PathBuf::from("/project/main.typ");
    let file_shortcuts = shortcuts.clone();
    let mut file_harness = Harness::builder()
        .with_size(Vec2::new(320.0, 360.0))
        .build_ui_state(
            move |ui, action| {
                show_file_popup_ui(ui, true, Some(&path), &file_shortcuts, action);
            },
            None::<AppPopupAction>,
        );
    file_harness.run();
    file_harness.get_by_label_contains("Rename").click();
    file_harness.run();
    assert!(matches!(
        file_harness.state(),
        Some(AppPopupAction::Command(AppCommand::Rename))
    ));

    let mut view_harness = Harness::builder()
        .with_size(Vec2::new(320.0, 360.0))
        .build_ui_state(
            move |ui, action| show_view_popup_ui(ui, true, &shortcuts, action),
            None::<AppPopupAction>,
        );
    view_harness.run();
    view_harness.get_by_label_contains("Packages").click();
    view_harness.run();
    assert!(matches!(
        view_harness.state(),
        Some(AppPopupAction::Command(AppCommand::Packages))
    ));
}

#[test]
fn popup_anchor_clamps_inside_even_tiny_viewports() {
    assert_eq!(
        clamp_popup_anchor(
            Pos2::new(900.0, 700.0),
            Vec2::new(280.0, 320.0),
            Vec2::new(220.0, 160.0),
        ),
        Pos2::new(4.0, 4.0)
    );
    assert_eq!(
        clamp_popup_anchor(
            Pos2::new(900.0, 700.0),
            Vec2::new(200.0, 100.0),
            Vec2::new(640.0, 480.0),
        ),
        Pos2::new(436.0, 376.0)
    );
}

#[test]
fn status_log_popup_hugs_its_bottom_anchor_and_only_grows_for_visible_rows() {
    let size = status_log_popup_size(4);
    assert_eq!(size.x, METRICS.menu.status_log_size.x);
    assert!(size.y < METRICS.menu.status_log_size.y);
    assert_eq!(status_log_popup_size(100).y, METRICS.menu.status_log_size.y);

    let anchor = clamp_popup_above_anchor(Pos2::new(500.0, 700.0), size, Vec2::new(1_000.0, 800.0));
    assert_eq!(anchor.y + size.y, 700.0);
}

#[test]
fn popup_blur_requires_sustained_focus_loss_and_refocus_cancels_it() {
    let now = Instant::now();
    let mut had_focus = false;
    let mut blur_started = None;
    assert!(!popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(false),
        now,
    ));
    assert!(!popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(true),
        now,
    ));
    assert!(had_focus);
    assert!(!popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(false),
        now,
    ));
    assert!(!popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(true),
        now + POPUP_BLUR_GRACE / 2,
    ));
    assert_eq!(blur_started, None);
    assert!(!popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(false),
        now + POPUP_BLUR_GRACE,
    ));
    assert!(popup_focus_should_close(
        &mut had_focus,
        &mut blur_started,
        Some(false),
        now + POPUP_BLUR_GRACE * 2,
    ));
}

#[test]
fn each_popup_open_gets_independent_scroll_memory() {
    assert_ne!(app_popup_scroll_id(1), app_popup_scroll_id(2));
}

#[test]
fn fallback_menu_rows_keep_their_caption_left_aligned() {
    let context = egui::Context::default();
    context
        .run_ui(Default::default(), |ui| {
            for shortcut in [
                None,
                Some(KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S)),
            ] {
                let button = menu_button(ui, "Save", shortcut);
                let atoms = button.atoms();
                assert!(
                    atoms.iter().skip(1).any(|atom| atom.grow),
                    "a growing atom after the caption pins it to the left: {atoms:?}"
                );
            }
        })
        .drop_without_applying_deltas();
}

#[test]
fn menu_availability_distinguishes_the_document_from_a_pinned_typst_preview() {
    let text_with_pinned_preview = CommandAvailability {
        can_undo: false,
        can_redo: false,
        saved_document: false,
        typst_document: false,
        typst_preview: true,
        interactive_preview: false,
    };
    assert!(text_with_pinned_preview.allows(command_spec(AppCommand::ExportPdf).requirement));
    assert!(!text_with_pinned_preview.allows(command_spec(AppCommand::Format).requirement));
}

#[test]
fn settings_font_dropdown_opens_and_selects_a_catalog_family() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let catalog = FontCatalog::snapshot_fixture();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(520.0, 460.0))
        .build_ui_state(
            move |ui, selection| {
                if let Some(next) = show_font_family_picker(
                    ui,
                    "tested-font-picker",
                    &catalog,
                    None,
                    None,
                    "System UI",
                    true,
                ) {
                    *selection = Some(next);
                }
            },
            None::<FontPickerSelection>,
        );
    harness.run();
    harness.get_by_value("System UI").click();
    harness.run();
    assert!(harness.query_by_label("Project Sans").is_some());
    harness.get_by_label("Project Sans").click();
    harness.run();
    assert!(matches!(
        harness.state(),
        Some(FontPickerSelection::Family { name, .. }) if name == "Project Sans"
    ));
}

#[test]
fn tooltip_markdown_link_dispatches_its_normalized_target() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let (sender, receiver) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(420.0, 160.0))
        .build_ui(move |ui| {
            show_markdown(ui, "Read [the guide](www\\.example.com/guide)", &sender);
        });
    harness.run();
    assert!(harness.query_by_label("www.example.com/guide").is_none());
    harness.get_by_label("the guide").click();
    harness.run();
    assert_eq!(
        receiver.try_recv(),
        Ok("https://www.example.com/guide".to_owned())
    );
}

#[test]
fn tooltip_code_jobs_are_cached_while_a_popup_scrolls() {
    let context = egui::Context::default();
    let highlighter = GenericSyntaxHighlighter::default();
    let mut typst_highlighter = SyntaxHighlighter::default();
    let palette = theme::default_syntax_palette(true);
    for _ in 0..2 {
        let _ = cached_tooltip_code_job(
            &context,
            &highlighter,
            &mut typst_highlighter,
            "let value = 1",
            "typc",
            true,
            palette,
        );
    }

    let cache_id = viewport_scoped_id(&context, "tooltip-code-cache");
    let cache = context.data(|data| data.get_temp::<TooltipCodeCache>(cache_id));
    assert_eq!(cache.as_ref().map(|cache| cache.jobs.len()), Some(1));

    for index in 0..33 {
        let source = format!("let value = {index}");
        let _ = cached_tooltip_code_job(
            &context,
            &highlighter,
            &mut typst_highlighter,
            &source,
            "typ",
            true,
            palette,
        );
    }
    let cache = context
        .data(|data| data.get_temp::<TooltipCodeCache>(cache_id))
        .unwrap();
    assert_eq!(cache.jobs.len(), 32);
    assert!(
        cache
            .jobs
            .iter()
            .all(|entry| entry.source != "let value = 0")
    );
    assert!(
        cache
            .jobs
            .iter()
            .any(|entry| entry.source == "let value = 32")
    );
}

#[test]
fn editor_context_menu_exposes_the_link_under_the_pointer() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let mut harness = Harness::builder()
        .with_size(Vec2::new(320.0, 360.0))
        .build_ui_state(
            |ui, action| {
                show_editor_context_menu_ui(
                    ui,
                    EditorContextMenuOptions {
                        can_undo: false,
                        can_redo: false,
                        has_selection: false,
                        can_format: true,
                        can_sync_preview: true,
                        link: Some("https://example.com/docs"),
                        table: None,
                    },
                    &ShortcutBindings::current_defaults(),
                    action,
                );
            },
            None::<EditorMenuAction>,
        );
    harness.run();
    harness
        .get_by_label_contains("Open Link in Browser")
        .click();
    harness.run();
    assert_eq!(
        harness.state(),
        &Some(EditorMenuAction::OpenLink(
            "https://example.com/docs".to_owned()
        ))
    );
    assert!(
        editor_context_menu_size(true, false, true, &egui::Style::default()).y
            > editor_context_menu_size(false, false, true, &egui::Style::default()).y
    );
}

#[test]
fn editor_context_menu_offers_the_static_table_under_the_pointer() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let source = "#table(columns: 2, [Name], [Value])";
    let cursor = source[..source.find("Name").unwrap()].chars().count();
    let table = editable_table_at(source, cursor).expect("static table");
    let expected = table.clone();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(320.0, 400.0))
        .build_ui_state(
            move |ui, action| {
                show_editor_context_menu_ui(
                    ui,
                    EditorContextMenuOptions {
                        can_undo: false,
                        can_redo: false,
                        has_selection: false,
                        can_format: true,
                        can_sync_preview: true,
                        link: None,
                        table: Some(&table),
                    },
                    &ShortcutBindings::current_defaults(),
                    action,
                );
            },
            None::<EditorMenuAction>,
        );
    harness.run();
    harness.get_by_label_contains("Edit Table…").click();
    harness.run();

    assert_eq!(
        harness.state(),
        &Some(EditorMenuAction::EditTable(expected))
    );
    assert!(
        editor_context_menu_size(false, true, true, &egui::Style::default()).y
            > editor_context_menu_size(false, false, true, &egui::Style::default()).y
    );
}

#[test]
fn table_editor_controls_keep_rows_and_columns_rectangular() {
    use egui_kittest::{
        Harness,
        kittest::{Queryable as _, by},
    };

    let source = "#table(columns: 2, [A], [B])";
    let cursor = source[..source.find("[A]").unwrap()].chars().count();
    let table = editable_table_at(source, cursor).unwrap();
    let original_call = char_range_slice(source, table.source_range.clone())
        .unwrap()
        .to_owned();
    let dialog = TableEditorDialog {
        table,
        original_call,
        document_key: DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch: 3,
            revision: 7,
        },
        focus_first_cell: false,
        error: None,
    };
    let mut harness = Harness::builder()
        .with_size(Vec2::new(760.0, 520.0))
        .build_ui_state(
            |ui, state| {
                if let Some(action) = show_table_editor_ui(ui, &mut state.0, 700.0, 330.0) {
                    state.1 = Some(action);
                }
            },
            (dialog, None::<TableEditorUiAction>),
        );
    harness.run();
    harness
        .get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value("A"),
        )
        .focus();
    harness.run();
    harness
        .get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value("A"),
        )
        .type_text("Edited ");
    harness.run();
    assert!(harness.state().0.table.cells[0][0].contains("Edited"));
    harness.get_by_label("Add row").click();
    harness.run();
    harness.get_by_label("Add column").click();
    harness.run();

    assert_eq!(harness.state().0.table.row_count(), 2);
    assert_eq!(harness.state().0.table.columns, 3);
    assert!(
        harness
            .state()
            .0
            .table
            .cells
            .iter()
            .all(|row| row.len() == 3)
    );
    harness.get_by_label("Apply").click();
    harness.run();
    assert_eq!(harness.state().1, Some(TableEditorUiAction::Apply));
}

#[test]
fn prepared_table_edit_is_one_unicode_safe_replacement_and_rejects_stale_source() {
    let source = "Préface\n#table(columns: 2, [Nom], [Valeur])\nFin";
    let cursor = source[..source.find("Nom").unwrap()].chars().count();
    let mut table = editable_table_at(source, cursor).unwrap();
    let original_call = char_range_slice(source, table.source_range.clone())
        .unwrap()
        .to_owned();
    table.cells[0][1] = "Édité".to_owned();
    let key = DocumentKey {
        owner: tiptoptyp_core::document::WindowSessionId::new(1),
        epoch: 4,
        revision: 9,
    };
    let dialog = TableEditorDialog {
        table,
        original_call,
        document_key: key,
        focus_first_cell: false,
        error: None,
    };

    let edit = prepare_table_source_edit(source, key, &dialog).unwrap();
    let mut applied = source.to_owned();
    applied.replace_range(edit.byte_range.clone(), &edit.replacement);
    assert!(applied.contains("[Édité]"));
    assert_eq!(
        edit.cursor,
        source[..edit.byte_range.start].chars().count() + edit.replacement.chars().count()
    );

    let stale = prepare_table_source_edit(
        source,
        DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch: 4,
            revision: 10,
        },
        &dialog,
    )
    .unwrap_err();
    assert!(stale.contains("document changed"));

    let changed_source = source.replacen("[Nom]", "[Other]", 1);
    let stale = prepare_table_source_edit(&changed_source, key, &dialog).unwrap_err();
    assert!(stale.contains("table source changed"));
}

#[test]
fn prepared_table_edit_refuses_dynamic_cell_source() {
    let source = "#table([Safe])";
    let mut table = editable_table_at(source, 2).unwrap();
    let original_call = char_range_slice(source, table.source_range.clone())
        .unwrap()
        .to_owned();
    table.cells[0][0] = "#unsafe-expression".to_owned();
    let key = DocumentKey {
        owner: tiptoptyp_core::document::WindowSessionId::new(1),
        epoch: 0,
        revision: 0,
    };
    let dialog = TableEditorDialog {
        table,
        original_call,
        document_key: key,
        focus_first_cell: false,
        error: None,
    };

    let error = prepare_table_source_edit(source, key, &dialog).unwrap_err();
    assert!(error.contains("not static Typst markup"));
}

#[test]
fn typst_override_grid_centers_headers_labels_samples_and_actions() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let mut fonts_configured = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1_600.0, 900.0))
        .build_ui(move |ui| {
            if !fonts_configured {
                let _ = theme::configure_editor_fonts(
                    ui.ctx(),
                    theme::FontRequest::default(),
                    theme::FontRequest::default(),
                    false,
                    theme::FONT_WEIGHT_NORMAL,
                    theme::FONT_WEIGHT_NORMAL,
                );
                fonts_configured = true;
                return;
            }
            show_typst_override_editor(
                ui,
                &mut TypstStyleOverrides::default(),
                theme::default_syntax_palette(false),
                &syntect::highlighting::Theme::default(),
                &theme::FontWeightSupport::Discrete {
                    values: theme::EDITOR_FONT_WEIGHTS.to_vec(),
                    default: theme::FONT_WEIGHT_NORMAL,
                },
            );
        });
    harness.run();

    let syntax = harness
        .get_all_by_label("Syntax")
        .next()
        .unwrap()
        .rect()
        .center()
        .y;
    let sample_header = harness
        .get_all_by_label("Live sample")
        .next()
        .unwrap()
        .rect()
        .center()
        .y;
    assert!((syntax - sample_header).abs() <= 0.5);

    let role = harness.get_by_label("Plain text").rect().center().y;
    let sample = harness.get_by_label("Document text").rect().center().y;
    let reset = harness
        .get_all_by_label("Reset")
        .next()
        .unwrap()
        .rect()
        .center()
        .y;
    assert!((role - sample).abs() <= 0.5);
    assert!((role - reset).abs() <= 0.5);
}

#[test]
fn status_log_keeps_the_newest_one_hundred_entries() {
    let mut entries = VecDeque::new();
    for index in 0..105 {
        push_status_log_entry(
            &mut entries,
            StatusLogEntry {
                timestamp: format!("{index}Z"),
                detail: format!("entry {index}"),
                kind: NoticeKind::Info,
            },
        );
    }
    assert_eq!(entries.len(), STATUS_LOG_LIMIT);
    assert_eq!(entries.front().unwrap().detail, "entry 104");
    assert_eq!(entries.back().unwrap().detail, "entry 5");
}

#[test]
fn continuous_font_weight_is_committed_only_after_pointer_release() {
    let mut committed = 400;
    let mut staged = None;
    update_staged_font_weight(&mut committed, &mut staged, 535, true, true, false);
    assert_eq!(committed, 400);
    assert_eq!(staged, Some(535));

    update_staged_font_weight(&mut committed, &mut staged, 560, true, false, true);
    assert_eq!(committed, 560);
    assert_eq!(staged, None);

    update_staged_font_weight(&mut committed, &mut staged, 600, true, false, false);
    assert_eq!(committed, 600, "keyboard changes commit immediately");
}

#[test]
fn font_argument_selector_targets_only_set_text_string_values() {
    let source = "= 文稿\n#set text(font: \"Inter\", size: 11pt)\n#text(font: \"Wrong\")[body]";
    let font_byte = source.find("font").unwrap();
    let font_char = source[..font_byte].chars().count();
    let target = typst_font_argument_at(source, font_char).unwrap();
    assert_eq!(&source[target.value_range], "Inter");

    let wrong_byte = source.rfind("font").unwrap();
    let wrong_char = source[..wrong_byte].chars().count();
    assert_eq!(typst_font_argument_at(source, wrong_char), None);
    assert_eq!(typst_font_argument_at("#set par(justify: true)", 8), None);
}

#[test]
fn typst_web_links_are_detected_without_claiming_normal_editor_clicks() {
    let source = "= 文稿\n#link(\"www.example.com/docs?q=1\")[documentation]";
    for needle in ["link", "www.example.com", "documentation"] {
        let byte = source.find(needle).expect("fixture contains target");
        let character = source[..byte].chars().count();
        assert_eq!(
            typst_web_link_at(source, character),
            Some("https://www.example.com/docs?q=1".to_owned()),
            "pointer over {needle}"
        );
    }

    assert_eq!(typst_web_link_at(source, 0), None);
    let link_char = source[..source.find("link").unwrap()].chars().count();
    assert_eq!(
        typst_web_link_click_target(source, link_char, true, false),
        None,
        "an ordinary click must remain an editor action"
    );
    assert_eq!(
        typst_web_link_click_target(source, link_char, true, true),
        Some("https://www.example.com/docs?q=1".to_owned())
    );
    assert_eq!(typst_web_link_at("ordinary text", 0), None);
    assert_eq!(typst_web_link_at("#link(target)[dynamic]", 3), None);
    assert_eq!(typst_web_link_at("#link(\"file.typ\")[local]", 3), None);
    assert_eq!(
        typst_web_link_at("#other(\"https://example.com\")", 3),
        None
    );
}

#[test]
fn browser_link_targets_accept_http_and_www_but_reject_unsafe_schemes() {
    assert_eq!(
        normalize_browser_link_target("www.example.com/path"),
        Some("https://www.example.com/path".to_owned())
    );
    assert_eq!(
        normalize_browser_link_target("https://example.com/path#part"),
        Some("https://example.com/path#part".to_owned())
    );
    assert_eq!(normalize_browser_link_target("javascript:alert(1)"), None);
    assert_eq!(normalize_browser_link_target("notes.typ"), None);
}

#[test]
fn external_browser_links_report_a_success_notice() {
    assert_eq!(
        external_link_opened_notice("https://example.com"),
        Notice {
            message: "Opened https://example.com in the system browser".to_owned(),
            kind: NoticeKind::Success,
        }
    );
}

#[test]
fn refresh_arrow_geometry_stays_outside_the_arc() {
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(14.0, 12.0));
    let geometry = refresh_icon_geometry(rect);
    let center = rect.center();
    let radius = geometry.arc[0].distance(center);

    assert_eq!(geometry.arc.last().copied(), Some(geometry.shaft[0]));
    assert_eq!(geometry.shaft[1], geometry.wing[0]);
    assert!(geometry.shaft[1].distance(center) > radius);

    let [a, b] = geometry.wing;
    let segment = b - a;
    let progress = ((center - a).dot(segment) / segment.length_sq()).clamp(0.0, 1.0);
    let closest = a + segment * progress;
    assert!(
        closest.distance(center) > radius,
        "the arrow wing must not cross back over the circular body"
    );
}

#[test]
fn preview_eye_uses_symmetric_curves_instead_of_straight_facets() {
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(14.0));
    let geometry = eye_icon_geometry(rect);
    let center = rect.center();

    assert_eq!(geometry.upper[0], geometry.lower[0]);
    assert_eq!(geometry.upper[3], geometry.lower[3]);
    assert!(geometry.upper[1..3].iter().all(|point| point.y < center.y));
    assert!(geometry.lower[1..3].iter().all(|point| point.y > center.y));
    assert!(geometry.pupil_radius > METRICS.icon.stroke_width * 0.5);
}

#[test]
fn popup_cards_never_paint_dark_corner_shadows() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    let style = context.style_of(context.theme());
    let frame = theme::popup_card_frame(&style);
    assert_eq!(frame.shadow, egui::epaint::Shadow::NONE);
    assert_eq!(frame.corner_radius, egui::CornerRadius::same(8));
    assert_eq!(style.visuals.popup_shadow, egui::epaint::Shadow::NONE);
    assert_eq!(
        style.visuals.menu_corner_radius,
        egui::CornerRadius::same(8)
    );

    let viewport = theme::popup_viewport_builder("test popup");
    assert_eq!(viewport.transparent, Some(true));
    assert_eq!(viewport.decorations, Some(false));
    assert_eq!(viewport.has_shadow, Some(false));
    assert_eq!(viewport.taskbar, Some(false));
    assert_eq!(
        viewport.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );
}

#[test]
fn panel_headers_have_exact_and_gapless_geometry_for_varied_controls() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    let style = context.style_of(context.theme());
    let frame = theme::content_panel_frame(&style);
    assert_eq!(frame.inner_margin.left, 8);
    assert_eq!(frame.inner_margin.right, 8);
    assert_eq!(frame.inner_margin.top, 0);
    assert_eq!(frame.inner_margin.bottom, 0);

    let mut measurements = Vec::new();
    for (id, content) in [
        ("geometry-workspace-header", 0),
        ("geometry-compact-control-header", 1),
        ("geometry-preview-header", 2),
    ] {
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 180.0))),
                    ..Default::default()
                },
                |ui| {
                    let top = ui.available_rect_before_wrap().top();
                    let mut control_center = 0.0;
                    let header = theme::panel_header(ui, id, |ui| {
                        let response = match content {
                            0 => ui.add_sized([180.0, 20.0], egui::Label::new("…/project")),
                            1 => ui.label(RichText::new("chapter.typ").strong()),
                            _ => ui.selectable_label(true, "Interactive"),
                        };
                        control_center = response.rect.center().y;
                    });
                    measurements.push((
                        top,
                        header,
                        ui.available_rect_before_wrap().top(),
                        control_center,
                    ));
                },
            )
            .drop_without_applying_deltas();
    }

    for (top, header, body_top, control_center) in &measurements {
        assert!((header.top() - top).abs() < 0.01, "{measurements:?}");
        assert!(
            (header.height() - METRICS.chrome.panel_header_height).abs() < 0.01,
            "{measurements:?}"
        );
        assert!(
            (body_top - header.bottom()).abs() < 0.01,
            "{measurements:?}"
        );
        assert!(
            (control_center - header.center().y).abs() <= 0.51,
            "{measurements:?}"
        );
    }
    assert!(
        measurements.windows(2).all(|pair| {
            (pair[0].1.bottom() - pair[1].1.bottom()).abs() < 0.01
                && (pair[0].3 - pair[1].3).abs() < 0.01
        }),
        "{measurements:?}"
    );
}

#[test]
fn native_preview_bounds_stay_inside_the_panel_clip() {
    let available = Rect::from_min_max(Pos2::new(96.0, 30.0), Pos2::new(420.0, 260.0));
    let clip = Rect::from_min_max(Pos2::new(120.0, 48.0), Pos2::new(400.0, 220.0));

    assert_eq!(
        clipped_preview_rect(available, clip),
        Rect::from_min_max(Pos2::new(120.0, 48.0), Pos2::new(400.0, 220.0))
    );
}

#[test]
fn native_preview_bounds_follow_egui_zoom_without_moving_the_viewport_origin() {
    assert_eq!(
        scale_rect_from_egui_to_native(
            Rect::from_min_max(Pos2::new(100.0, 50.0), Pos2::new(300.0, 250.0)),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0)),
            1.15,
        )
        .map(|rect| [rect.left(), rect.top(), rect.right(), rect.bottom()]),
        Some([115.0, 57.5, 345.0, 287.5])
    );
}

#[test]
fn wide_explorer_header_and_body_cannot_grow_the_resized_panel() {
    let context = egui::Context::default();
    let mut widths = Vec::new();
    let mut header_geometry = Vec::new();
    for _ in 0..4 {
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                    ..Default::default()
                },
                |ui| {
                    let response = egui::Panel::left("narrow-explorer-test")
                        .default_size(42.0)
                        .min_size(0.0)
                        .max_size(300.0)
                        .show(ui, |ui| {
                            let header =
                                theme::panel_header(ui, "narrow-explorer-header-test", |ui| {
                                    let _ = ui.add_sized(
                                        [500.0, 60.0],
                                        egui::Label::new("an overflowing explorer header"),
                                    );
                                });
                            header_geometry.push((header, ui.available_rect_before_wrap().top()));
                            let mut content =
                                clipped_panel_content_ui(ui, "narrow-explorer-content-test");
                            content.add_sized(
                                [500.0, 20.0],
                                egui::Label::new("a deliberately very wide tree row"),
                            );
                        });
                    widths.push(response.response.rect.width());
                },
            )
            .drop_without_applying_deltas();
    }
    assert!(widths.iter().all(|width| *width <= 43.0), "{widths:?}");
    assert!(
        header_geometry.iter().all(|(header, body_top)| {
            (header.height() - METRICS.chrome.panel_header_height).abs() < 0.01
                && (body_top - header.bottom()).abs() < 0.01
        }),
        "{header_geometry:?}"
    );
    assert!(
        header_geometry
            .iter()
            .zip(&widths)
            .all(|((header, _), panel_width)| header.width() <= *panel_width + 0.01),
        "headers={header_geometry:?}, panels={widths:?}"
    );
}

#[test]
fn explorer_reopen_restores_the_last_open_width() {
    let previous = Rect::from_min_size(Pos2::new(0.0, 24.0), Vec2::new(18.0, 300.0));
    assert_eq!(
        explorer_width_restored_rect(previous, 236.0),
        Rect::from_min_size(Pos2::new(0.0, 24.0), Vec2::new(236.0, 300.0))
    );
}

#[test]
fn explorer_sections_split_the_body_budget_without_hiding_headers() {
    let available = 500.0;
    let frame_height = 2.0;
    for open_sections in 1..=ExplorerSection::ALL.len() {
        let body = available_explorer_section_body_height(available, open_sections, frame_height);
        let headers = ExplorerSection::ALL.len() as f32
            * (METRICS.explorer.section_header_height + frame_height);
        let gaps = (ExplorerSection::ALL.len() - 1) as f32 * METRICS.explorer.section_gap;
        assert!((headers + gaps + body * open_sections as f32 - available).abs() < 0.01);
    }
    assert_eq!(
        available_explorer_section_body_height(10.0, 2, frame_height),
        0.0
    );
    assert_eq!(
        available_explorer_section_body_height(available, 0, frame_height),
        0.0
    );
}

#[test]
fn explorer_section_resize_moves_only_the_adjacent_open_split() {
    let open = [true, false, true, true, false, false, false, false];
    let mut layout = ExplorerSectionLayout::default();
    let before = layout.body_heights(open, 300.0);
    assert!((before[0] - 100.0).abs() < 0.01);
    assert!((before[2] - 100.0).abs() < 0.01);
    assert!((before[3] - 100.0).abs() < 0.01);

    assert!(layout.resize_after(
        open,
        300.0,
        ExplorerOrder::default(),
        ExplorerSection::Files,
        30.0
    ));
    let after = layout.body_heights(open, 300.0);
    assert!((after[0] - 130.0).abs() < 0.01, "{after:?}");
    assert!((after[2] - 70.0).abs() < 0.01, "{after:?}");
    assert!((after[3] - 100.0).abs() < 0.01, "{after:?}");
    assert!((after.iter().sum::<f32>() - 300.0).abs() < 0.01);
}

#[test]
fn explorer_search_is_unicode_case_insensitive_and_retains_ancestors() {
    let leaf = WorkspaceNode {
        name: std::ffi::OsString::from("Résumé.TYP"),
        path: PathBuf::from("/workspace/chapters/Résumé.TYP"),
        relative_path: PathBuf::from("chapters/Résumé.TYP"),
        kind: crate::workspace::WorkspaceNodeKind::File,
        children: Vec::new(),
    };
    let directory = WorkspaceNode {
        name: std::ffi::OsString::from("chapters"),
        path: PathBuf::from("/workspace/chapters"),
        relative_path: PathBuf::from("chapters"),
        kind: crate::workspace::WorkspaceNodeKind::Directory,
        children: vec![leaf.clone()],
    };
    let unrelated = WorkspaceNode {
        name: std::ffi::OsString::from("notes.txt"),
        path: PathBuf::from("/workspace/notes.txt"),
        relative_path: PathBuf::from("notes.txt"),
        kind: crate::workspace::WorkspaceNodeKind::File,
        children: Vec::new(),
    };
    let query = normalize_explorer_query("  RÉSUMÉ  ");

    assert!(workspace_node_matches_query(&leaf, &query));
    assert!(
        workspace_node_matches_query(&directory, &query),
        "the parent directory must remain visible for a matching descendant"
    );
    assert!(!workspace_node_matches_query(&unrelated, &query));
    assert!(workspace_node_matches_query(&unrelated, ""));
}

#[test]
fn explorer_search_covers_every_project_index_section() {
    let root = PathBuf::from("/workspace");
    let snapshot = WorkspaceSnapshot {
        root: root.clone(),
        nodes: Vec::new(),
    };
    let index = ProjectIndex {
        outline: vec![crate::project_index::OutlineEntry {
            path: root.join("paper.typ"),
            line: 7,
            level: 1,
            title: "Introduction".to_owned(),
        }],
        subfiles: vec![root.join("appendix.typ")],
        symbols: vec![crate::project_index::SymbolEntry {
            path: root.join("paper.typ"),
            line: 12,
            name: "accent-color".to_owned(),
            kind: crate::project_index::SymbolKind::Definition,
        }],
        packages: vec!["@preview/cetz:0.4.2".to_owned()],
        tags: vec![crate::project_index::ReferenceEntry {
            path: root.join("paper.typ"),
            line: 19,
            label: "<tag:overview>".to_owned(),
        }],
        references: vec![crate::project_index::ReferenceEntry {
            path: root.join("paper.typ"),
            line: 20,
            label: "@fig:overview".to_owned(),
        }],
        ..ProjectIndex::default()
    };

    for (query, section) in [
        ("introduction", 2),
        ("appendix", 3),
        ("ACCENT-COLOR", 4),
        ("cetz", 5),
        ("fig:overview", 7),
        ("tag:overview", 6),
    ] {
        let query = normalize_explorer_query(query);
        let matches = explorer_section_query_matches(Some(&snapshot), &index, &query);
        assert!(matches[section], "query {query:?}: {matches:?}");
    }

    assert_eq!(
        explorer_section_query_matches(Some(&snapshot), &index, "does-not-exist"),
        [true, false, false, false, false, false, false, false],
        "an empty result keeps the Files surface open for its empty-state message"
    );
}

#[test]
fn explorer_section_resize_clamps_to_a_usable_minimum() {
    let open = [true, true, false, false, false, false, false, false];
    let mut layout = ExplorerSectionLayout::default();
    assert!(layout.resize_after(
        open,
        200.0,
        ExplorerOrder::default(),
        ExplorerSection::Files,
        1_000.0
    ));
    let heights = layout.body_heights(open, 200.0);
    assert!((heights[0] - 156.0).abs() < 0.01, "{heights:?}");
    assert!((heights[1] - EXPLORER_SECTION_MIN_BODY_HEIGHT).abs() < 0.01);

    let tiny = layout.body_heights(open, 40.0);
    assert_eq!(tiny, [20.0, 20.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert!(!layout.resize_after(
        open,
        40.0,
        ExplorerOrder::default(),
        ExplorerSection::Files,
        5.0
    ));
}

#[test]
fn explorer_close_hides_contents_one_frame_before_the_panel() {
    let hiding = ExplorerPanelPhase::Open.toggle();
    assert_eq!(hiding, ExplorerPanelPhase::HideContents);
    assert!(hiding.panel_visible());
    assert!(!hiding.contents_visible());

    let closed = hiding.finish_frame();
    assert_eq!(closed, ExplorerPanelPhase::Closed);
    assert!(!closed.panel_visible());
    assert_eq!(closed.toggle(), ExplorerPanelPhase::Open);
    assert_eq!(hiding.toggle(), ExplorerPanelPhase::Open);
}

#[test]
fn explorer_file_rows_expose_git_status_badges() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = Path::new("/project");
    let path = root.join("main.typ");
    let git =
        crate::git::editor::GitEditorState::snapshot_fixture(root, &path, "one\ntwo\nthree", false);
    let nodes = ["main.typ", "references.bib", "clean.typ"].map(|name| WorkspaceNode {
        name: name.into(),
        path: root.join(name),
        relative_path: name.into(),
        kind: crate::workspace::WorkspaceNodeKind::File,
        children: Vec::new(),
    });
    let mut harness = Harness::builder()
        .with_size(Vec2::new(280.0, 240.0))
        .build_ui(move |ui| {
            let mut state = TreeViewState::default();
            let context = ui.ctx().clone();
            TreeView::new(ui.id().with("git-files")).show_state(ui, &mut state, |builder| {
                add_workspace_nodes(builder, &nodes, None, None, &context, "", &git.statuses);
            });
        });
    harness.run();
    for (file, status) in [("main.typ", "M"), ("references.bib", "A")] {
        let file = harness.get_by_label(file).rect();
        let badge = harness.get_by_label(status).rect();
        assert!((file.center().y - badge.center().y).abs() < 1.0);
        assert!(badge.left() > file.right() && badge.right() <= 280.0);
    }
    harness.get_by_label("clean.typ");
}

#[test]
fn explorer_order_controls_move_panels_and_reset_with_aligned_buttons() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let mut harness = Harness::builder()
        .with_size(Vec2::new(340.0, 400.0))
        .build_ui_state(
            |ui, order| {
                install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                settings_panel::show_explorer_order_controls(ui, order);
            },
            ExplorerOrder::default(),
        );
    harness.run();
    let right = harness.get_by_label("Move Files down").rect().right();
    for section in ExplorerSection::ALL {
        let down = harness
            .get_by_label(&format!("Move {} down", section.title()))
            .rect();
        let up = harness
            .get_by_label(&format!("Move {} up", section.title()))
            .rect();
        assert_eq!(down.right(), right);
        assert!(up.right() < down.left());
    }
    harness.get_by_label("Move Files down").click();
    harness.run();
    assert_eq!(
        harness.state().sections()[..2],
        [ExplorerSection::Git, ExplorerSection::Files]
    );
    assert!(harness.get_by_label("Git").rect().top() < harness.get_by_label("Files").rect().top());
    harness.get_by_label("Move References up").click();
    harness.run();
    assert_eq!(harness.state().sections()[6], ExplorerSection::References);
    harness.get_by_label("Reset panel order").click();
    harness.run();
    assert_eq!(*harness.state(), ExplorerOrder::default());
}

#[test]
fn explorer_order_controls_stay_visible_after_an_oversized_settings_row() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let mut harness = Harness::builder()
        .with_size(Vec2::new(360.0, 500.0))
        .build_ui_state(
            |ui, order| {
                ui.allocate_space(Vec2::new(800.0, 20.0));
                settings_panel::show_explorer_order_controls(ui, order);
            },
            ExplorerOrder::default(),
        );
    harness.run();
    for section in ExplorerSection::ALL {
        let down = harness
            .get_by_label(&format!("Move {} down", section.title()))
            .rect();
        assert!(down.right() <= 360.0, "{down:?}");
    }
    harness.get_by_label("Move Files down").click();
    harness.run_steps(2);
    assert_eq!(harness.state().sections()[0], ExplorerSection::Git);
}

#[test]
fn reordered_explorer_keeps_body_identity_and_collapsed_state() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    #[derive(Default)]
    struct State {
        order: ExplorerOrder,
        body_ids: [Option<egui::Id>; 8],
    }
    let mut harness = Harness::builder()
        .with_size(Vec2::new(360.0, 800.0))
        .build_ui_state(
            |ui, state: &mut State| {
                if ui.button("Reorder").clicked() {
                    state.order.move_to(ExplorerSection::References, 0);
                    state.order.move_to(ExplorerSection::Files, 7);
                }
                state.body_ids.fill(None);
                let defaults = [true; 8];
                let open = explorer_section_open_states(ui, false, defaults);
                show_explorer_sections(
                    ui,
                    ExplorerSectionsSpec {
                        order: state.order,
                        defaults,
                        open,
                        heights: [44.0; 8],
                        filtered: false,
                        git_visible: true,
                    },
                    |ui, section| {
                        state.body_ids[section.index()] = Some(ui.id());
                        ui.label(format!("{} body", section.title()));
                    },
                );
            },
            State::default(),
        );
    harness.run();
    let identities = harness.state().body_ids;
    assert!(identities.iter().all(Option::is_some));
    harness.get_by_label("Tags").click();
    harness.run();
    assert!(harness.query_by_label("Tags body").is_none());
    harness.get_by_label("Reorder").click();
    harness.run();
    assert!(harness.query_by_label("Tags body").is_none());
    assert!(
        harness.get_by_label("References").rect().top() < harness.get_by_label("Git").rect().top()
    );
    assert!(harness.get_by_label("Files").rect().top() > harness.get_by_label("Tags").rect().top());
    for section in ExplorerSection::ALL {
        if section != ExplorerSection::Tags {
            assert_eq!(
                harness.state().body_ids[section.index()],
                identities[section.index()],
                "{section:?} lost body identity"
            );
        }
    }
}

#[test]
fn tag_and_reference_panels_have_independent_search_and_navigation() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = Path::new("/workspace");
    let main = root.join("main.typ");
    let index = analyze_project(
        root,
        &main,
        &BTreeMap::from([(
            main.clone(),
            "= Chapter <chapter>\nSee @chapter.\n".to_owned(),
        )]),
    );
    for (section, label, other, line) in [
        (ExplorerSection::Tags, "<chapter>", "@chapter", 1),
        (ExplorerSection::References, "@chapter", "<chapter>", 2),
    ] {
        let matches = explorer_section_query_matches(None, &index, label);
        assert!(matches[section.index()]);
        let other_section = if section == ExplorerSection::Tags {
            ExplorerSection::References
        } else {
            ExplorerSection::Tags
        };
        assert!(!matches[other_section.index()]);
        let mut harness = Harness::builder()
            .with_size(Vec2::new(360.0, 200.0))
            .build_ui_state(
                |ui, target| {
                    install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                    let outcome = show_project_index_section(ui, section, root, &index, "");
                    if outcome.target.is_some() {
                        *target = outcome.target;
                    }
                },
                None::<(PathBuf, usize)>,
            );
        harness.run();
        assert!(harness.query_by_label(other).is_none());
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(*harness.state(), Some((main.clone(), line)));
    }
}

#[test]
fn reordered_explorer_resizes_the_visible_neighbor_and_preserves_other_heights() {
    let mut order = ExplorerOrder::default();
    order.move_to(ExplorerSection::References, 0);
    order.move_to(ExplorerSection::Contents, 1);
    let mut open = [false; 8];
    for section in [
        ExplorerSection::References,
        ExplorerSection::Files,
        ExplorerSection::Tags,
    ] {
        open[section.index()] = true;
    }
    let mut layout = ExplorerSectionLayout::default();
    assert!(layout.resize_after(open, 300.0, order, ExplorerSection::References, 30.0));
    let heights = layout.body_heights(open, 300.0);
    assert!((heights[ExplorerSection::References.index()] - 130.0).abs() < 0.01);
    assert!((heights[ExplorerSection::Files.index()] - 70.0).abs() < 0.01);
    assert!((heights[ExplorerSection::Tags.index()] - 100.0).abs() < 0.01);
}

#[test]
fn gutter_marker_receives_clicks_beside_the_actual_text_editor() {
    use crate::git::editor::{ChangeKind, Hunk, LineChange};
    use egui_kittest::{Harness, kittest::Queryable as _};
    for (source, change, label) in [
        (
            "first\nafter\nthird",
            LineChange {
                lines: 1..2,
                kind: ChangeKind::Modified,
                old_line_count: 1,
                new_line_count: 1,
            },
            "Git change: modified lines 2–2",
        ),
        (
            "",
            LineChange {
                lines: 0..0,
                kind: ChangeKind::Deleted,
                old_line_count: 1,
                new_line_count: 0,
            },
            "Git change: deleted lines at line 1",
        ),
    ] {
        let hunk = Hunk {
            text: "@@ -2 +2 @@\n-before\n+after\n".into(),
            changes: vec![change],
        };
        let mut source = source.to_owned();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 260.0))
            .build_ui_state(
                move |ui, clicked| {
                    let output = egui::TextEdit::multiline(&mut source)
                        .code_editor()
                        .frame(egui::Frame::new().inner_margin(egui::Margin {
                            left: 40,
                            right: 4,
                            top: 2,
                            bottom: 2,
                        }))
                        .desired_width(460.0)
                        .show(ui);
                    let line_rows = logical_line_row_ranges(&output.galley.rows);
                    paint_line_numbers(ui, &output, &line_rows);
                    let rows = line_rows
                        .iter()
                        .map(|rows| {
                            output.galley.rows[rows.start]
                                .rect()
                                .union(output.galley.rows[rows.end - 1].rect())
                                .translate(output.galley_pos.to_vec2())
                        })
                        .collect::<Vec<_>>();
                    if let Some(index) = crate::git::editor::show_markers(
                        ui,
                        std::slice::from_ref(&hunk),
                        &rows,
                        output.response.rect.left(),
                    ) {
                        *clicked = Some(index);
                    }
                },
                None::<usize>,
            );
        harness.run();
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(*harness.state(), Some(0));
    }
}

#[test]
fn workspace_tree_state_survives_a_filesystem_refresh() {
    let context = egui::Context::default();
    let root = PathBuf::from("/workspace/project");
    let directory = root.join("chapters");

    context
        .run_ui(Default::default(), |ui| {
            let id = workspace_tree_state_id(ui, &root, false);
            let mut state = TreeViewState::load(ui, id).unwrap_or_default();
            state.set_openness(directory.clone(), true);
            state.store(ui, id);
        })
        .drop_without_applying_deltas();

    // A later filesystem generation renders through the same stable UI
    // identity. Loading it must recover the user's expansion state.
    context
        .run_ui(Default::default(), |ui| {
            let id = workspace_tree_state_id(ui, &root, false);
            let state = TreeViewState::load(ui, id).expect("tree state from prior scan");
            assert_eq!(state.is_open(&directory), Some(true));
        })
        .drop_without_applying_deltas();
}

#[test]
fn explorer_section_bodies_start_at_the_section_root() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    let mut expected_left = None;
    let mut body_left = None;
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 180.0))),
                ..Default::default()
            },
            |ui| {
                expected_left = Some(
                    ui.available_rect_before_wrap().left()
                        + theme::explorer_section_frame(ui.style())
                            .total_margin()
                            .left,
                );
                explorer_section(ui, "unindented-section-test", "Files", true, 60.0, |ui| {
                    body_left = Some(ui.available_rect_before_wrap().left())
                });
            },
        )
        .drop_without_applying_deltas();
    assert_eq!(body_left, expected_left);
}

#[test]
fn explorer_section_frames_fit_the_available_height() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    let mut used_height = 0.0;
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 500.0))),
                ..Default::default()
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = METRICS.explorer.section_gap;
                let top = ui.available_rect_before_wrap().top();
                let body_height = explorer_section_body_height(ui);
                for section in ExplorerSection::ALL {
                    explorer_section(
                        ui,
                        section.id(),
                        section.title(),
                        section.default_open(),
                        body_height,
                        |_| {},
                    );
                }
                used_height = ui.min_rect().bottom() - top;
            },
        )
        .drop_without_applying_deltas();
    assert!(used_height <= 500.01, "used {used_height} points");
    assert!(
        used_height >= 490.0,
        "left too much unused space: {used_height}"
    );
}

#[test]
fn view_modes_have_the_requested_panel_matrix() {
    assert!(ViewMode::Code.shows_code());
    assert!(!ViewMode::Code.shows_preview());
    assert!(ViewMode::Split.shows_code());
    assert!(ViewMode::Split.shows_preview());
    assert!(!ViewMode::Preview.shows_code());
    assert!(ViewMode::Preview.shows_preview());
}

#[test]
fn view_mode_controls_are_only_enabled_for_typst_documents() {
    assert!(view_mode_controls_enabled(DocumentKind::Typst));
    for kind in [DocumentKind::Text, DocumentKind::Image, DocumentKind::Pdf] {
        assert!(!view_mode_controls_enabled(kind));
    }
}

#[test]
fn tooltip_bridge_keeps_pointer_transitively_connected_to_the_card() {
    let origin = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0));
    let card = Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0));
    assert!(tooltip_region_contains(origin.center(), origin, card));
    assert!(tooltip_region_contains(Pos2::new(20.0, 0.0), origin, card));
    assert!(tooltip_region_contains(Pos2::new(20.0, 10.0), origin, card));
    assert!(tooltip_region_contains(Pos2::new(45.0, 15.0), origin, card));
    assert!(!tooltip_region_contains(
        Pos2::new(90.0, 15.0),
        origin,
        card
    ));
    assert!(!tooltip_region_contains(
        Pos2::new(20.0, 25.0),
        origin,
        card
    ));
}

#[test]
fn tooltip_bridge_ends_at_the_bottom_edge_of_a_lower_card() {
    let origin = Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(50.0, 10.0));
    let card = Rect::from_min_max(Pos2::new(0.0, 30.0), Pos2::new(80.0, 60.0));

    assert!(tooltip_region_contains(Pos2::new(40.0, 20.0), origin, card));
    assert!(tooltip_region_contains(Pos2::new(40.0, 45.0), origin, card));
    assert!(!tooltip_region_contains(
        Pos2::new(40.0, 60.1),
        origin,
        card
    ));
}

#[test]
fn tooltip_bridge_converts_child_position_to_root_coordinates() {
    let root = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
    let origin = Rect::from_min_size(Pos2::new(320.0, 130.0), Vec2::new(40.0, 14.0));
    let card = place_native_tooltip_card(
        Rect::from_min_size(Pos2::ZERO, root.size()),
        origin,
        Pos2::new(320.0, 150.0),
        Vec2::new(240.0, 120.0),
        TooltipPlacement::Below,
        8.0,
    );
    let child_position = root.min + card.min.to_vec2();
    assert_eq!(child_position, Pos2::new(420.0, 200.0));
    assert_eq!(card.min, Pos2::new(320.0, 150.0));
    assert!(card.contains(Pos2::new(400.0, 240.0)));
}

#[test]
fn tooltip_below_flips_above_instead_of_covering_a_bottom_edge_source() {
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let origin = Rect::from_min_max(Pos2::new(300.0, 540.0), Pos2::new(340.0, 560.0));
    let card = place_native_tooltip_card(
        viewport,
        origin,
        Pos2::new(300.0, 566.0),
        Vec2::new(240.0, 120.0),
        TooltipPlacement::Below,
        8.0,
    );

    assert_eq!(card.min, Pos2::new(300.0, 414.0));
    assert_eq!(card.bottom(), origin.top() - 6.0);
    assert!(!card.intersects(origin));
}

#[test]
fn tooltip_right_flips_left_at_the_viewport_edge() {
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let origin = Rect::from_min_max(Pos2::new(750.0, 200.0), Pos2::new(770.0, 220.0));
    let card = place_native_tooltip_card(
        viewport,
        origin,
        Pos2::new(776.0, 200.0),
        Vec2::new(240.0, 120.0),
        TooltipPlacement::Right,
        8.0,
    );

    assert_eq!(card.min, Pos2::new(504.0, 200.0));
    assert_eq!(card.right(), origin.left() - 6.0);
    assert!(!card.intersects(origin));
}

#[test]
fn asset_preview_sizes_are_bounded_and_never_upscaled() {
    let landscape = fit_asset_preview_size([800, 400], Vec2::new(420.0, 300.0));
    assert!((landscape.x - 420.0).abs() < 0.01);
    assert!((landscape.y - 210.0).abs() < 0.01);
    let portrait = fit_asset_preview_size([400, 800], Vec2::new(420.0, 300.0));
    assert!((portrait.x - 150.0).abs() < 0.01);
    assert!((portrait.y - 300.0).abs() < 0.01);
    assert_eq!(
        fit_asset_preview_size([40, 20], Vec2::new(420.0, 300.0)),
        Vec2::new(40.0, 20.0)
    );

    let card = asset_hover_card_size(
        &AssetHoverContent::Loading,
        Vec2::new(180.0, 90.0),
        Vec2::new(16.0, 16.0),
        8.0,
    );
    assert!(card.x <= 164.0);
    assert!(card.y <= 74.0);
}

#[test]
fn stale_asset_thumbnail_results_cannot_replace_the_active_hover() {
    let token = |count| {
        let mut token = crate::asset::ThumbnailToken::default();
        for _ in 0..count {
            token.advance();
        }
        token
    };
    let active = AssetHoverState {
        origin: Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0)),
        anchor: Pos2::new(0.0, 12.0),
        placement: TooltipPlacement::Below,
        path: PathBuf::from("current.png"),
        kind: DocumentKind::Image,
        opacity: 1.0,
        token: token(9),
        content: AssetHoverContent::Loading,
    };
    let result = |count, path: &str, kind| AssetThumbnailResult {
        token: token(count),
        path: PathBuf::from(path),
        kind,
        output: Err("unused".to_owned()),
    };

    assert!(asset_thumbnail_result_matches(
        Some(&active),
        &result(9, "current.png", DocumentKind::Image)
    ));
    assert!(!asset_thumbnail_result_matches(
        Some(&active),
        &result(8, "current.png", DocumentKind::Image)
    ));
    assert!(!asset_thumbnail_result_matches(
        Some(&active),
        &result(9, "old.png", DocumentKind::Image)
    ));
    assert!(!asset_thumbnail_result_matches(
        None,
        &result(9, "current.png", DocumentKind::Image)
    ));
}

#[test]
fn multiline_editor_asset_range_uses_a_connected_hover_region() {
    let editor = Rect::from_min_max(Pos2::ZERO, Pos2::new(320.0, 180.0));
    let start = Rect::from_min_size(Pos2::new(80.0, 20.0), Vec2::new(1.0, 18.0));
    let end = Rect::from_min_size(Pos2::new(40.0, 56.0), Vec2::new(1.0, 18.0));

    let range = editor_range_rect(editor, start, end);

    assert_eq!(range.left(), editor.left());
    assert_eq!(range.right(), editor.right());
    assert_eq!(range.top(), start.top());
    assert_eq!(range.bottom(), end.bottom());
}

#[test]
fn asset_hover_error_card_keeps_file_and_failure_visible() {
    use egui_kittest::{Harness, kittest::Queryable as _};

    let mut harness = Harness::builder()
        .with_size(Vec2::new(360.0, 144.0))
        .build_ui(|ui| {
            show_asset_hover_contents(
                ui,
                Path::new("assets/missing diagram.pdf"),
                DocumentKind::Pdf,
                &AssetHoverContent::Error("Could not read the PDF".to_owned()),
                Vec2::new(340.0, 124.0),
            );
        });
    harness.run();

    assert!(
        harness
            .query_by_label_contains("missing diagram.pdf")
            .is_some()
    );
    assert!(
        harness
            .query_by_label_contains("Could not read the PDF")
            .is_some()
    );
}

#[test]
fn hovered_asset_row_publishes_a_preview_candidate_after_its_delay() {
    let context = egui::Context::default();
    install_hover_runtime_config(&context, Duration::ZERO);
    let row_rect = std::cell::Cell::new(Rect::NOTHING);
    let draw = |input| {
        context
            .run_ui(input, |ui| {
                let response = ui.add(
                    egui::Label::new("diagram.png")
                        .selectable(false)
                        .sense(Sense::hover()),
                );
                row_rect.set(response.rect);
                offer_asset_hover(
                    &response,
                    response.rect,
                    PathBuf::from("assets/diagram.png"),
                    DocumentKind::Image,
                    TooltipPlacement::Right,
                );
            })
            .drop_without_applying_deltas();
    };
    draw(egui::RawInput::default());
    clear_asset_hover_candidate(&context);
    draw(egui::RawInput {
        events: vec![egui::Event::PointerMoved(row_rect.get().center())],
        ..Default::default()
    });

    let candidate = current_asset_hover_candidate(&context)
        .expect("the hovered asset row should publish a preview candidate");
    assert_eq!(candidate.path, PathBuf::from("assets/diagram.png"));
    assert_eq!(candidate.kind, DocumentKind::Image);
    assert_eq!(candidate.placement, TooltipPlacement::Right);
    assert!(candidate.anchor.x > candidate.origin.right());
    assert_eq!(candidate.opacity, 1.0);
}

#[test]
fn native_popup_content_budget_leaves_room_for_the_rendered_frame() {
    let style = egui::Style::default();
    let frame_margin = theme::tooltip_card_frame(&style).total_margin().sum();
    let content = frame_content_size(Vec2::new(200.0, 100.0), frame_margin);
    assert!(content.x + frame_margin.x <= 200.0);
    assert!(content.y + frame_margin.y <= 100.0);
    assert_eq!(
        frame_content_size(Vec2::splat(1.0), Vec2::splat(4.0)),
        Vec2::splat(1.0)
    );
}

#[test]
fn tooltip_handoff_blocks_competing_hover_targets_until_focus_changes() {
    let geometry = TooltipGeometry {
        identity: 1,
        origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
        card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
        pointer_inside_viewport: false,
        handoff_until: 0.0,
    };
    assert!(tooltip_handoff_is_active(
        Some(Pos2::new(20.0, 0.0)),
        0.0,
        Some(geometry),
        None,
    ));
    assert!(!tooltip_handoff_blocks(
        true,
        Some(geometry.origin),
        geometry.origin,
    ));
    assert!(tooltip_handoff_blocks(
        true,
        Some(geometry.origin),
        Rect::from_min_max(Pos2::new(100.0, 0.0), Pos2::new(110.0, 10.0)),
    ));

    let mut focused = TooltipInteractionState::new(7);
    focused.focused = true;
    assert!(tooltip_handoff_is_active(
        Some(Pos2::new(500.0, 500.0)),
        0.0,
        None,
        Some(focused),
    ));
    assert!(!tooltip_handoff_blocks(
        true,
        None,
        Rect::from_min_max(Pos2::new(490.0, 490.0), Pos2::new(510.0, 510.0)),
    ));

    let mut dismissed = focused;
    dismissed.dismissed = true;
    assert!(!tooltip_handoff_is_active(
        Some(Pos2::new(500.0, 500.0)),
        0.0,
        Some(geometry),
        Some(dismissed),
    ));
}

#[test]
fn tooltip_handoff_keeps_competing_targets_blocked_across_the_child_viewport() {
    let origin = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(40.0, 30.0));
    let geometry = TooltipGeometry {
        identity: 2,
        origin,
        // The handoff envelope includes the transparent native viewport;
        // its painted card can be smaller after content-aware shrinking.
        card: Rect::from_min_max(Pos2::new(10.0, 50.0), Pos2::new(220.0, 140.0)),
        pointer_inside_viewport: false,
        handoff_until: 0.0,
    };
    let pointer = Pos2::new(100.0, 40.0);
    let active = tooltip_handoff_is_active(Some(pointer), 0.0, Some(geometry), None);
    assert!(active);
    assert!(tooltip_handoff_blocks(
        active,
        Some(origin),
        Rect::from_min_max(Pos2::new(90.0, 35.0), Pos2::new(120.0, 55.0)),
    ));
}

#[test]
fn tooltip_handoff_grace_survives_a_transient_pointer_gap() {
    let geometry = TooltipGeometry {
        identity: 3,
        origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
        card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
        pointer_inside_viewport: false,
        handoff_until: 1.3,
    };
    let pointer = Some(Pos2::new(400.0, 400.0));
    assert!(tooltip_handoff_is_active(
        pointer,
        1.2,
        Some(geometry),
        None
    ));
    assert!(!tooltip_handoff_is_active(
        pointer,
        1.3,
        Some(geometry),
        None
    ));
}

#[test]
fn tooltip_child_pointer_ownership_survives_missing_root_pointer_events() {
    let geometry = TooltipGeometry {
        identity: 4,
        origin: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)),
        card: Rect::from_min_max(Pos2::new(30.0, 0.0), Pos2::new(80.0, 30.0)),
        pointer_inside_viewport: true,
        handoff_until: 0.5,
    };
    let refreshed = refresh_tooltip_root_geometry(geometry, None, 5.0);

    assert!(refreshed.pointer_inside_viewport);
    assert!(tooltip_handoff_is_active(None, 5.0, Some(refreshed), None));
    assert!(tooltip_viewport_should_render(
        false,
        false,
        false,
        geometry.identity,
        Some(refreshed),
        None,
    ));

    let replacement = TooltipGeometry {
        identity: 5,
        pointer_inside_viewport: false,
        ..geometry
    };
    assert!(refresh_tooltip_child_geometry(replacement, 4, true).is_none());
    let mut stale_focus = TooltipInteractionState::new(4);
    stale_focus.focused = true;
    assert!(!tooltip_handoff_is_active(
        None,
        5.0,
        Some(replacement),
        Some(stale_focus),
    ));
    assert!(tooltip_viewport_should_render(
        false,
        false,
        true,
        geometry.identity,
        None,
        None,
    ));
}

#[test]
fn tooltip_click_requests_focus_and_defocus_dismisses_it() {
    let identity = 7;
    let state = TooltipInteractionState::new(identity);
    let state = update_tooltip_interaction_state(state, identity, true, Some(false));
    assert!(state.focus_requested);
    assert!(!state.dismissed);

    let state = update_tooltip_interaction_state(state, identity, false, Some(true));
    assert!(state.focused);
    assert!(state.had_focus);

    let state = update_tooltip_interaction_state(state, identity, false, Some(false));
    assert!(state.dismissed);
    assert!(!state.focus_requested);
}

#[test]
fn tooltip_focus_shortcut_is_stable() {
    assert_eq!(
        ShortcutBindings::current_defaults().egui(ShortcutAction::PointerTooltip),
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::Space,
        ))
    );
    assert_eq!(
        ShortcutBindings::current_defaults().egui(ShortcutAction::CaretTooltip),
        Some(KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::K,
        ))
    );
}

#[test]
fn dismissed_tooltip_source_stays_blocked_until_explicitly_rearmed() {
    let context = egui::Context::default();
    context
        .run_ui(Default::default(), |ui| {
            let origin = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(30.0));
            let id = viewport_scoped_id(ui.ctx(), "dismissed-tooltip-origin");
            ui.ctx().data_mut(|data| data.insert_temp(id, origin));
            assert!(native_tooltip_handoff_blocks(ui.ctx(), origin));
            assert!(!native_tooltip_handoff_blocks(
                ui.ctx(),
                origin.translate(Vec2::splat(100.0))
            ));
            ui.ctx().data_mut(|data| data.remove::<Rect>(id));
            assert!(!native_tooltip_handoff_blocks(ui.ctx(), origin));
        })
        .drop_without_applying_deltas();
}

#[test]
fn tooltip_typst_fences_respect_source_and_code_modes() {
    let generic = GenericSyntaxHighlighter::default();
    let mut typst = SyntaxHighlighter::default();
    let keyword = theme::default_syntax_palette(true).keyword;
    let color_at = |job: &egui::text::LayoutJob, byte| {
        job.sections
            .iter()
            .find(|section| section.byte_range.start.0 <= byte && byte < section.byte_range.end.0)
            .expect("highlighted byte should have a layout section")
            .format
            .color
    };

    let source = "#let value = 1";
    let source_job = tooltip_code_job(
        &generic,
        &mut typst,
        source,
        "typst",
        true,
        theme::default_syntax_palette(true),
    )
    .unwrap();
    assert_eq!(source_job.text, source);
    assert_eq!(color_at(&source_job, source.find("let").unwrap()), keyword);

    let code = "let value = 1";
    let code_job = tooltip_code_job(
        &generic,
        &mut typst,
        code,
        "typc",
        true,
        theme::default_syntax_palette(true),
    )
    .unwrap();
    assert_eq!(code_job.text, code);
    assert_eq!(color_at(&code_job, 0), keyword);
}

#[test]
fn tooltip_inline_code_closes_without_leaking_its_style() {
    let span = |text: &str, code| MarkdownInlineSpan {
        text: text.to_owned(),
        code,
        bold: false,
        italics: false,
        link: None,
    };
    assert_eq!(
        markdown_inline_spans("The character `#` is invalid"),
        vec![
            span("The character ", false),
            span("#", true),
            span(" is invalid", false),
        ]
    );
    assert_eq!(
        markdown_inline_spans("An `unclosed marker"),
        vec![span("An `unclosed marker", false)]
    );
    assert_eq!(
        markdown_inline_spans("`stars * stay literal`"),
        vec![span("stars * stay literal", true)]
    );
}

#[test]
fn tooltip_markdown_links_hide_destinations_and_preserve_code_literals() {
    assert_eq!(
        markdown_inline_spans("See [stuff](www\\.here.com/path) now"),
        vec![
            MarkdownInlineSpan {
                text: "See ".to_owned(),
                code: false,
                bold: false,
                italics: false,
                link: None,
            },
            MarkdownInlineSpan {
                text: "stuff".to_owned(),
                code: false,
                bold: false,
                italics: false,
                link: Some("https://www.here.com/path".to_owned()),
            },
            MarkdownInlineSpan {
                text: " now".to_owned(),
                code: false,
                bold: false,
                italics: false,
                link: None,
            },
        ]
    );
    assert_eq!(
        markdown_inline_spans("`[literal](https://example.com)`"),
        vec![MarkdownInlineSpan {
            text: "[literal](https://example.com)".to_owned(),
            code: true,
            bold: false,
            italics: false,
            link: None,
        }]
    );
}

#[test]
fn typst_override_toggle_hints_are_stateful_and_compact() {
    assert_eq!(typst_override_state(None), "inherit");
    assert_eq!(typst_override_state(Some(true)), "on");
    assert_eq!(typst_override_state(Some(false)), "off");
    assert_eq!(next_typst_override_state(None), Some(true));
    assert_eq!(next_typst_override_state(Some(true)), Some(false));
    assert_eq!(next_typst_override_state(Some(false)), None);

    let hint = typst_override_toggle_tooltip("B", None);
    assert_eq!(hint, "B: inherit; click to cycle");
    assert!(hint.chars().count() <= 32);
}

#[test]
fn remembered_documents_must_stay_inside_the_canonical_project() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let outside = directory.path().join("outside.typ");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("main.typ"), "= Main").unwrap();
    std::fs::write(&outside, "= Outside").unwrap();

    let root = root.canonicalize().unwrap();
    let mut settings = AppSettings::default();
    settings.last_opened_files.insert(
        root.to_str().unwrap().to_owned(),
        root.join("main.typ").display().to_string(),
    );
    assert_eq!(
        remembered_document(&settings, &root),
        Some(root.join("main.typ").canonicalize().unwrap())
    );

    settings.last_opened_files.insert(
        root.to_str().unwrap().to_owned(),
        outside.display().to_string(),
    );
    assert_eq!(remembered_document(&settings, &root), None);
}

#[test]
fn targetless_launch_restores_the_newest_existing_workspace_and_document() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    let fallback = directory.path().join("fallback");
    for root in [&first, &second, &fallback] {
        std::fs::create_dir(root).unwrap();
    }
    std::fs::write(first.join("first.typ"), "= First").unwrap();
    std::fs::write(second.join("second.typ"), "= Second").unwrap();
    let first = first.canonicalize().unwrap();
    let second = second.canonicalize().unwrap();
    let mut settings = AppSettings::default();
    settings.remember_workspace(&first);
    settings.remember_workspace(&second);
    settings.last_opened_files.insert(
        second.to_str().unwrap().to_owned(),
        second.join("second.typ").display().to_string(),
    );

    assert_eq!(
        resolve_initial_workspace(&settings, None, &fallback),
        InitialWorkspace {
            root: second.clone(),
            document: Some(second.join("second.typ").canonicalize().unwrap()),
        }
    );
}

#[test]
fn targetless_launch_skips_missing_recents_then_falls_back_to_the_cwd_project() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    let nested = project.join("chapters");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(project.join(".git")).unwrap();
    let existing = directory.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    let existing = existing.canonicalize().unwrap();

    let mut settings = AppSettings {
        recent_workspaces: vec![
            directory.path().join("missing").display().to_string(),
            existing.display().to_string(),
        ],
        ..AppSettings::default()
    };
    assert_eq!(
        resolve_initial_workspace(&settings, None, &nested).root,
        existing
    );

    settings.recent_workspaces.clear();
    assert_eq!(
        resolve_initial_workspace(&settings, None, &nested).root,
        project.canonicalize().unwrap()
    );
}

#[test]
fn explicit_launch_targets_override_workspace_history() {
    let directory = tempfile::tempdir().unwrap();
    let remembered = directory.path().join("remembered");
    let explicit = directory.path().join("explicit");
    let nested = explicit.join("chapters");
    std::fs::create_dir(&remembered).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(explicit.join(".git")).unwrap();
    let main = explicit.join("main.typ");
    let chapter = nested.join("one.typ");
    std::fs::write(&main, "= Main").unwrap();
    std::fs::write(&chapter, "= One").unwrap();
    let remembered = remembered.canonicalize().unwrap();
    let explicit = explicit.canonicalize().unwrap();
    let mut settings = AppSettings::default();
    settings.remember_workspace(&remembered);
    settings.last_opened_files.insert(
        explicit.to_str().unwrap().to_owned(),
        main.display().to_string(),
    );

    assert_eq!(
        resolve_initial_workspace(&settings, Some(&explicit), directory.path()),
        InitialWorkspace {
            root: explicit.clone(),
            document: Some(main.canonicalize().unwrap()),
        }
    );
    assert_eq!(
        resolve_initial_workspace(&settings, Some(&chapter), directory.path()),
        InitialWorkspace {
            root: explicit,
            document: Some(chapter.canonicalize().unwrap()),
        }
    );
}

#[test]
fn revision_versions_saturate_for_lsp() {
    assert_eq!(revision_as_i32(42), 42);
    assert_eq!(revision_as_i32(u64::MAX), i32::MAX);
}

#[test]
fn logical_line_count_preserves_empty_and_trailing_lines() {
    assert_eq!(logical_line_count(""), 1);
    assert_eq!(logical_line_count("a\n"), 2);
    assert_eq!(logical_line_count("a\n\n🙂"), 3);
    assert_eq!(line_index_at_char("first\nsecond", 5), 0);
    assert_eq!(line_index_at_char("first\nsecond", 6), 1);
}

#[test]
fn ui_scale_is_clamped_to_the_supported_interface_range() {
    assert_eq!(ui_scale_factor(75), 0.75);
    assert_eq!(ui_scale_factor(DEFAULT_UI_SCALE_PERCENT), 1.0);
    assert_eq!(ui_scale_factor(150), 1.5);
    assert_eq!(ui_scale_factor(0), 0.75);
    assert_eq!(ui_scale_factor(u16::MAX), 1.5);
}

#[test]
fn find_enter_navigation_honours_shift() {
    assert_eq!(find_step_for_enter(true, false), Some(FindStep::Next));
    assert_eq!(find_step_for_enter(true, true), Some(FindStep::Previous));
    assert_eq!(find_step_for_enter(false, true), None);
}

#[test]
fn preview_status_does_not_report_zero_millisecond_startup_timing() {
    assert_eq!(
        preview_timing_label(DocumentKind::Typst, Duration::ZERO),
        None
    );
    assert_eq!(
        preview_timing_label(DocumentKind::Typst, Duration::from_millis(18)),
        Some("18 ms".to_owned())
    );
    assert_eq!(
        preview_timing_label(DocumentKind::Pdf, Duration::from_millis(18)),
        None
    );
}

#[test]
fn opening_git_from_a_normal_window_reveals_the_explorer() {
    assert!(git_command_opens_explorer(false));
    assert!(!git_command_opens_explorer(true));
}

#[test]
fn hidden_git_section_clears_a_persisted_open_state() {
    let context = egui::Context::default();
    context
        .run_ui(Default::default(), |ui| {
            let id = explorer_section_state_id(ui, "workspace-git", false);
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                false,
            );
            state.set_open(true);
            state.store(ui.ctx());
            set_explorer_section_open(ui, "workspace-git", false);
            assert!(
                !egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    false
                )
                .is_open()
            );
        })
        .drop_without_applying_deltas();
}

#[test]
fn app_popup_guard_only_blocks_root_owned_overlays() {
    assert!(!app_popup_blocked_by_root_overlay(
        false, false, false, false, false
    ));
    assert!(app_popup_blocked_by_root_overlay(
        true, false, false, false, false
    ));
    assert!(app_popup_blocked_by_root_overlay(
        false, true, false, false, false
    ));
    assert!(app_popup_blocked_by_root_overlay(
        false, false, true, false, false
    ));
    assert!(app_popup_blocked_by_root_overlay(
        false, false, false, true, false
    ));
    assert!(app_popup_blocked_by_root_overlay(
        false, false, false, false, true
    ));
}

#[test]
fn explorer_asset_hover_ends_at_the_visible_panel_edge() {
    let row = Rect::from_min_max(Pos2::new(24.0, 40.0), Pos2::new(420.0, 64.0));
    let panel = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(300.0, 200.0));
    let hover = workspace_asset_hover_rect(row, panel);

    assert_eq!(hover.left(), row.left());
    assert_eq!(hover.right(), panel.right());
    assert_eq!(hover.top(), row.top());
    assert_eq!(hover.bottom(), row.bottom());
}

#[test]
fn active_explorer_entry_keeps_the_shared_content_font_size() {
    assert_eq!(theme::strong_ui_font().size, theme::TYPE.content);
}

#[test]
fn search_highlights_cover_every_match_and_distinguish_the_selected_one() {
    let mut job = egui::text::LayoutJob::simple(
        "alpha beta alpha".to_owned(),
        theme::editor_font(),
        Color32::WHITE,
        f32::INFINITY,
    );
    let normal = Color32::from_rgba_unmultiplied(1, 2, 3, 48);
    let selected = Color32::from_rgba_unmultiplied(1, 2, 3, 96);
    editor_view::apply_search_highlights(
        &mut job,
        &[0..5, 11..16],
        Some(&(11..16)),
        normal,
        selected,
    );
    job.debug_sanity_check();

    assert_eq!(job.text, "alpha beta alpha");
    assert_eq!(
        job.sections
            .iter()
            .filter(|section| section.format.background == normal)
            .map(|section| section.byte_range.end.0 - section.byte_range.start.0)
            .sum::<usize>(),
        5
    );
    assert_eq!(
        job.sections
            .iter()
            .filter(|section| section.format.background == selected)
            .map(|section| section.byte_range.end.0 - section.byte_range.start.0)
            .sum::<usize>(),
        5
    );
}

#[test]
fn delimiter_highlights_use_character_advances_at_soft_wrap_boundaries() {
    let context = egui::Context::default();
    context
        .run_ui(Default::default(), |ui| {
            let galley = ui.painter().layout(
                "éééééééééééééééééééé()```".into(),
                egui::FontId::monospace(16.0),
                Color32::WHITE,
                40.0,
            );
            assert!(galley.rows.len() > 1);
            for character in 0..galley.text().chars().count() {
                let rectangles = editor_view::delimiter_rects(&galley, character..character + 1);
                assert_eq!(rectangles.len(), 1);
                assert!(
                    rectangles[0].width() > 0.0 && rectangles[0].width() < 20.0,
                    "{rectangles:?}"
                );
                assert!(rectangles[0].height() < 30.0);
            }
            let len = galley.text().chars().count();
            let fences = editor_view::delimiter_rects(&galley, len - 3..len);
            assert!(!fences.is_empty());
            assert!(
                fences
                    .iter()
                    .all(|rect| rect.width() <= 40.0 && rect.height() < 30.0)
            );
        })
        .drop_without_applying_deltas();
}

#[test]
fn settings_search_indexes_every_visible_setting_label() {
    for target in SettingsTarget::ALL {
        assert!(
            settings_search_results(target.label()).contains(&target),
            "missing searchable label {:?}: {}",
            target,
            target.label()
        );
    }
    assert_eq!(
        settings_search_results("auto save delay"),
        vec![SettingsTarget::AutoSaveDelay]
    );
    assert!(settings_search_results("autosave").contains(&SettingsTarget::AutoSave));
    assert_eq!(
        settings_search_results("titlebar"),
        vec![SettingsTarget::TitleBarMenus]
    );
    assert_eq!(
        settings_search_results("custom compiler"),
        vec![SettingsTarget::TypstCompiler]
    );
    assert_eq!(
        settings_search_results("raster pdf"),
        vec![SettingsTarget::PreviewBackend]
    );
    assert_eq!(
        settings_search_results("output directory"),
        vec![SettingsTarget::UiScreenshots]
    );
    assert!(settings_search_results("missing setting").is_empty());
}

#[test]
fn settings_search_routes_to_the_exact_individual_target() {
    let mut pending = Some(SettingsTarget::CodeFontWeight);
    assert!(!take_settings_scroll_target(
        &mut pending,
        SettingsTarget::UiFontWeight
    ));
    assert_eq!(pending, Some(SettingsTarget::CodeFontWeight));
    assert!(take_settings_scroll_target(
        &mut pending,
        SettingsTarget::CodeFontWeight
    ));
    assert_eq!(pending, None);
}

#[test]
fn save_as_format_handoff_waits_for_the_matching_ready_document() {
    let key = DocumentKey {
        owner: tiptoptyp_core::document::WindowSessionId::new(1),
        epoch: 3,
        revision: 7,
    };
    // The destination kind wins: Text -> .typ formats, while Typst ->
    // .txt does not carry the source document's formatting policy across.
    assert_eq!(
        save_as_format_handoff(true, DocumentKind::Typst, key),
        Some(key)
    );
    assert_eq!(
        save_as_format_handoff(false, DocumentKind::Typst, key),
        None
    );
    assert_eq!(save_as_format_handoff(true, DocumentKind::Text, key), None);

    let mut pending = Some(key);
    assert!(!take_ready_format_handoff(&mut pending, key, false));
    assert_eq!(pending, Some(key));
    assert!(!take_ready_format_handoff(
        &mut pending,
        DocumentKey {
            owner: tiptoptyp_core::document::WindowSessionId::new(1),
            epoch: 4,
            revision: 7,
        },
        true,
    ));
    assert_eq!(pending, Some(key));
    assert!(take_ready_format_handoff(&mut pending, key, true));
    assert_eq!(pending, None);
}

#[test]
fn toolbar_shortcut_copy_uses_effective_configured_binding() {
    let mut overrides = crate::shortcuts::ShortcutOverrides::default();
    overrides.set(
        ShortcutAction::Compile,
        Some(ShortcutChord::parse("Primary+Shift+B").unwrap()),
    );
    let bindings = ShortcutBindings::current(&overrides);
    assert_eq!(
        shortcut_tooltip("Compile", &bindings, ShortcutAction::Compile),
        if cfg!(target_os = "macos") {
            "Compile · Cmd+Shift+B"
        } else {
            "Compile · Ctrl+Shift+B"
        }
    );
}

#[test]
fn modified_shortcuts_win_over_their_generic_variants() {
    let shortcuts = ShortcutBindings::current_defaults();
    assert_eq!(
        run_shortcut(Modifiers::COMMAND | Modifiers::ALT, egui::Key::O, |input| {
            consume_shortcut(input, &shortcuts, |command| {
                command_spec(command).menu == CommandMenu::File
            })
        },),
        Some(AppCommand::OpenInNewWindow)
    );
    assert_eq!(
        run_shortcut(
            Modifiers::COMMAND | Modifiers::SHIFT,
            egui::Key::O,
            |input| consume_shortcut(input, &shortcuts, |command| command_spec(command).menu
                == CommandMenu::File),
        ),
        Some(AppCommand::ChangeWorkspaceRoot)
    );
    let replace = shortcuts
        .egui(command_spec(AppCommand::FindReplace).shortcut_action)
        .expect("find/replace has a shortcut");
    assert_eq!(
        run_shortcut(replace.modifiers, replace.logical_key, |input| {
            consume_shortcut(input, &shortcuts, |command| {
                matches!(command, AppCommand::Find | AppCommand::FindReplace)
            })
        },),
        Some(AppCommand::FindReplace)
    );
}

#[test]
fn preview_zoom_and_interface_scale_have_distinct_shortcuts() {
    let shortcuts = ShortcutBindings::current_defaults();
    let preview_modifiers = Modifiers::COMMAND | Modifiers::ALT;
    assert_eq!(
        run_shortcut(preview_modifiers, egui::Key::Plus, |input| {
            consume_preview_zoom_shortcut(input, &shortcuts)
        },),
        Some(PreviewZoomAction::In)
    );
    assert_eq!(
        run_shortcut(Modifiers::COMMAND, egui::Key::Plus, |input| {
            consume_preview_zoom_shortcut(input, &shortcuts)
        },),
        None
    );
    assert_eq!(
        run_shortcut(Modifiers::COMMAND, egui::Key::Plus, |input| {
            consume_ui_scale_shortcut(input, &shortcuts)
        },),
        Some(5)
    );
}

#[test]
fn workspace_colors_use_the_same_extension_policy_as_document_detection() {
    fn light_context() -> egui::Context {
        let context = egui::Context::default();
        context.set_theme(egui::Theme::Light);
        context
    }
    assert_eq!(
        workspace_entry_color(Path::new("main.typ"), false, false, &light_context()),
        workspace_entry_color(Path::new("main.TYP"), false, false, &light_context())
    );
    assert_eq!(
        workspace_entry_color(Path::new("paper.pdf"), false, false, &light_context()),
        workspace_entry_color(Path::new("paper.PDF"), false, false, &light_context())
    );
    assert_eq!(
        workspace_entry_color(Path::new("notes.jsonc"), false, false, &light_context()),
        theme::default_syntax_palette(false).plain
    );
    assert_eq!(
        workspace_entry_color(Path::new("favicon.ico"), false, false, &light_context()),
        theme::default_palette(false).info
    );
    let category = Color32::from_rgb(8, 20, 36);
    let strong = Color32::from_rgb(245, 244, 240);
    assert_eq!(
        workspace_entry_resolved_color(category, false, strong),
        category
    );
    assert_eq!(
        workspace_entry_resolved_color(category, true, strong),
        strong
    );
}

#[test]
fn problems_row_double_click_dispatches_only_located_diagnostics() {
    let located = Diagnostic {
        severity: DiagnosticSeverity::Error,
        source: DiagnosticSource::File(PathBuf::from("chapter.typ")),
        location: Some(DiagnosticLocation {
            line: 17,
            column: 4,
        }),
        message: "expected expression".to_owned(),
        details: Vec::new(),
    };
    assert_eq!(problem_row_jump_target(&located, false), None);
    assert_eq!(problem_row_jump_target(&located, true), Some(located));

    let unlocated = Diagnostic {
        severity: DiagnosticSeverity::Error,
        source: DiagnosticSource::Global,
        location: None,
        message: "compiler unavailable".to_owned(),
        details: Vec::new(),
    };
    assert_eq!(problem_row_jump_target(&unlocated, true), None);
}

#[test]
fn problems_row_detects_a_real_pointer_double_click_without_covering_its_text() {
    let context = egui::Context::default();
    let jumped = std::cell::Cell::new(false);
    let row_rect = std::cell::Cell::new(Rect::NOTHING);
    let run_frame = |time: f64, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 100.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let response =
                ui.add(egui::Label::new("selectable diagnostic message").selectable(true));
            row_rect.set(response.rect);
            jumped.set(jumped.get() || problem_row_double_clicked(&response));
        });
        output.textures_delta.clear();
    };

    run_frame(0.0, Vec::new());
    let position = row_rect.get().center();
    for (time, pressed) in [(0.01, true), (0.02, false), (0.03, true), (0.04, false)] {
        run_frame(
            time,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }],
        );
    }

    assert!(jumped.get());
}

#[test]
fn status_timestamps_are_explicitly_utc_and_wrap_at_midnight() {
    assert_eq!(utc_timestamp_from_unix_seconds(0), "00:00:00Z");
    assert_eq!(utc_timestamp_from_unix_seconds(3_661), "01:01:01Z");
    assert_eq!(utc_timestamp_from_unix_seconds(86_401), "00:00:01Z");
}

#[test]
fn interface_scale_preserves_native_display_density() {
    for density in [1.0, 2.0] {
        let context = egui::Context::default();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(density);
        context
            .run_ui(input.clone(), |ui| apply_ui_scale(ui.ctx(), 125))
            .textures_delta
            .clear();
        context
            .run_ui(input, |ui| {
                assert_eq!(ui.ctx().zoom_factor(), 1.25);
                assert_eq!(ui.ctx().pixels_per_point(), density * 1.25);
            })
            .textures_delta
            .clear();
    }
}

#[test]
fn toggle_line_comments_handles_selected_lines_and_round_trips() {
    let source = "  alpha\n\tbeta\n\n  gamma";
    let selected = 2..source.chars().count();
    let (commented, mapped) = toggle_line_comments(source, selected.clone(), "// ");
    assert_eq!(commented, "//   alpha\n// \tbeta\n\n//   gamma");
    assert_eq!(mapped, 5..commented.chars().count());

    let (restored, restored_range) = toggle_line_comments(&commented, mapped, "// ");
    assert_eq!(restored, source);
    assert_eq!(restored_range, selected);
}

#[test]
fn toggle_line_comments_uses_the_cursor_line_for_an_empty_selection() {
    let source = "one\ntwo\nthree";
    let (commented, cursor) = toggle_line_comments(source, 5..5, "// ");
    assert_eq!(commented, "one\n// two\nthree");
    assert_eq!(cursor, 8..8);
}

#[test]
fn editor_attention_progress_is_short_and_monotonic() {
    assert_eq!(editor_attention_progress(Duration::ZERO), 0.0);
    assert!(editor_attention_progress(Duration::from_millis(210)) > 0.45);
    assert_eq!(
        editor_attention_progress(METRICS.motion.editor_attention),
        1.0
    );
}

#[test]
fn short_editor_surface_fills_tall_and_narrow_viewports() {
    for viewport in [
        Rect::from_min_size(Pos2::new(12.0, 18.0), Vec2::new(640.0, 900.0)),
        Rect::from_min_size(Pos2::new(3.0, 7.0), Vec2::new(42.0, 511.0)),
    ] {
        let short_document = Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), 84.0));
        let surface = editor_surface_rect(short_document, viewport);

        assert_eq!(surface.left(), viewport.left());
        assert_eq!(surface.right(), viewport.right());
        assert_eq!(surface.top(), viewport.top());
        assert_eq!(surface.bottom(), viewport.bottom());
    }
}

#[test]
fn sticky_context_geometry_stays_at_the_editor_top() {
    let viewport = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(600.0, 400.0));
    let without_find = sticky_context_overlay_geometry(viewport, None).unwrap();
    assert_eq!(without_find.anchor, viewport.left_top());
    assert_eq!(without_find.width, viewport.width());
    assert_eq!(without_find.max_height, 180.0);

    let find = Rect::from_min_size(Pos2::new(18.0, 28.0), Vec2::new(360.0, 76.0));
    let at_editor_top = sticky_context_overlay_geometry(viewport, Some(find)).unwrap();
    assert_eq!(at_editor_top.anchor, viewport.left_top());
    assert_eq!(at_editor_top.width, viewport.width());

    let unrelated = Rect::from_min_size(Pos2::new(800.0, 28.0), Vec2::new(100.0, 76.0));
    assert_eq!(
        sticky_context_overlay_geometry(viewport, Some(unrelated)),
        Some(without_find)
    );
}

#[test]
fn sticky_context_uses_the_editor_gutter_and_a_bottom_only_shadow() {
    let galley_x = 132.0;
    assert_eq!(
        editor_gutter_geometry(galley_x),
        EditorGutterGeometry {
            line_number_right: galley_x - METRICS.editor.line_number_right_gap + 8.0,
            separator_x: galley_x - 0.25,
        }
    );

    let viewport = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(600.0, 400.0));
    let overlay = Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), 24.0));
    let clip = sticky_context_shadow_clip(overlay, viewport);
    assert_eq!(clip.top(), overlay.bottom());
    assert_eq!(clip.left(), viewport.left());
    assert_eq!(clip.right_bottom(), viewport.right_bottom());
    let cover = sticky_context_opaque_cover(overlay, viewport);
    assert_eq!(cover.min, overlay.min);
    assert_eq!(cover.right(), overlay.right());
    assert_eq!(
        cover.bottom(),
        overlay.bottom() + STICKY_CONTEXT_BOTTOM_COVER
    );

    for dark_mode in [false, true] {
        let shadow = sticky_context_shadow(dark_mode);
        assert_eq!(shadow.offset, [0, 2]);
        assert_eq!(shadow.spread, 0);
        assert!(shadow.blur > 0);
        assert!(shadow.color.a() > 0);
    }
}

#[test]
fn sticky_context_rows_activate_at_successive_stack_boundaries() {
    let row_tops = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0, 120.0];
    let anchor_at_boundary = |boundary: f32| row_tops.iter().rposition(|top| *top <= boundary);
    let stack = |anchor| match anchor {
        0..=1 => StickyContextStackProbe {
            signature: vec![(1, 0)],
            height: 20.0,
        },
        2..=3 => StickyContextStackProbe {
            signature: vec![(1, 0), (3, 20)],
            height: 40.0,
        },
        _ => StickyContextStackProbe {
            signature: vec![(1, 0), (3, 20), (5, 40)],
            height: 60.0,
        },
    };
    let anchor_at = |viewport_top| {
        sticky_context_stacked_scroll_anchor(viewport_top, 180.0, anchor_at_boundary, stack)
    };

    assert_eq!(anchor_at(-0.5), None);
    assert_eq!(anchor_at(0.0), Some(1));
    assert_eq!(anchor_at(19.5), Some(1));
    assert_eq!(
        anchor_at(20.0),
        Some(3),
        "row two must join when it reaches the bottom of sticky row one"
    );
    assert_eq!(
        anchor_at(40.0),
        Some(5),
        "row three must join at the bottom of the first two sticky rows"
    );

    assert!(!sticky_context_row_reached_boundary(40.0, 20.0, 19.5));
    assert!(sticky_context_row_reached_boundary(40.0, 20.0, 20.0));
    assert!(sticky_context_row_reached_boundary(40.0, 20.5, 20.0));
}

#[test]
fn sticky_context_resolver_retains_a_multiline_scalar_definition() {
    let source = "#let a(\nb,\nc,\n) = 2\nordinary";
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let line_char_counts = lines
        .iter()
        .map(|line| line.chars().count())
        .collect::<Vec<_>>();
    let logical_lines = (0..line_char_counts.len())
        .map(|line| line..line + 1)
        .collect::<Vec<_>>();
    let line_tops = (0..line_char_counts.len())
        .map(|line| line as f32 * 20.0)
        .collect::<Vec<_>>();
    let scroll_lines = sticky_context_scroll_lines(
        &logical_lines,
        |row| line_tops.get(row).copied(),
        |row| line_char_counts.get(row).copied(),
        |row| lines.get(row).map(|line| line.ends_with('\n')),
    )
    .unwrap();
    let ordinary_anchor = sticky_context_scroll_anchor(&scroll_lines, 80.0).unwrap();
    assert!(sticky_context_rows(source, ordinary_anchor).is_empty());

    let resolved = sticky_context_stacked_scroll_anchor(
        0.0,
        180.0,
        |boundary| sticky_context_scroll_anchor(&scroll_lines, boundary),
        |anchor| {
            let rows = sticky_context_rows(source, anchor);
            StickyContextStackProbe {
                signature: rows.iter().map(|row| (row.line, row.char_index)).collect(),
                height: rows.len() as f32 * 20.0,
            }
        },
    )
    .unwrap();
    assert_eq!(
        sticky_context_rows(source, resolved)
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["#let a(", "b,", "c,", ") = 2"]
    );
}

#[test]
fn sticky_context_resolver_adopts_a_shallower_sibling_path() {
    let source = "= Old\n== Nested\nold body\n= New\nnew body";
    let old_body = source[..source.find("old body").unwrap()].chars().count();
    let new_heading = source[..source.find("= New").unwrap()].chars().count() + 4;
    let old_rows = sticky_context_rows(source, old_body);
    let new_rows = sticky_context_rows(source, new_heading);
    assert_eq!(
        old_rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["= Old", "== Nested"]
    );
    assert_eq!(
        new_rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["= New"]
    );

    let resolved = sticky_context_stacked_scroll_anchor(
        0.0,
        180.0,
        |boundary| Some(usize::from(boundary >= 40.0)),
        |anchor| {
            let rows = if anchor == 0 { &old_rows } else { &new_rows };
            StickyContextStackProbe {
                signature: rows.iter().map(|row| (row.line, row.char_index)).collect(),
                height: rows.len() as f32 * 20.0,
            }
        },
    );
    assert_eq!(resolved, Some(1));
}

#[test]
fn sticky_context_snapshot_fixture_scrolls_past_its_line_six_heading() {
    let lines = STICKY_CONTEXT_SNAPSHOT_SOURCE.lines().collect::<Vec<_>>();
    assert_eq!(lines.get(5), Some(&"= Running todo list"));
    assert_eq!(lines.get(6), Some(&"== Active subsection"));
    assert_eq!(lines.get(7), Some(&"#let review("));
    assert_eq!(lines.get(10), Some(&") = 2"));
    assert!(
        lines.len() as f32 * theme::TYPE.content
            > METRICS.chrome.main_size.y + STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET,
        "the fixture must remain tall enough for the forced offset"
    );
    const {
        assert!(
            STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET > 11.0 * theme::TYPE.content,
            "the multiline definition must have crossed the viewport top"
        );
    }
    let initializer_byte = STICKY_CONTEXT_SNAPSHOT_SOURCE.find(") = 2").unwrap() + ") = ".len();
    let initializer_char = STICKY_CONTEXT_SNAPSHOT_SOURCE[..initializer_byte]
        .chars()
        .count();
    assert_eq!(
        sticky_context_rows(STICKY_CONTEXT_SNAPSHOT_SOURCE, initializer_char)
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        [
            "= Running todo list",
            "== Active subsection",
            "#let review(",
            "task,",
            "state,",
            ") = 2",
        ]
    );
    assert_eq!(
        source_editor_snapshot_scroll_offset(Some(UiSnapshotScene::StickyContext)),
        Some(STICKY_CONTEXT_SNAPSHOT_SCROLL_OFFSET)
    );
    assert_eq!(
        source_editor_snapshot_scroll_offset(Some(UiSnapshotScene::Main)),
        None
    );
    assert_eq!(source_editor_snapshot_scroll_offset(None), None);

    let mut document = DocumentSession::new(
        tiptoptyp_core::document::WindowSessionId::new(1),
        "old source",
        DocumentKind::Typst,
    );
    document.reset_editor_history = false;
    let revision = document.revision();
    assert!(prepare_sticky_context_snapshot_document(&mut document));
    assert_eq!(document.source(), STICKY_CONTEXT_SNAPSHOT_SOURCE);
    assert_eq!(document.revision(), revision + 1);
    assert!(
        document.reset_editor_history,
        "show_editor must reset the deterministic scene caret to character zero"
    );
    assert!(!prepare_sticky_context_snapshot_document(&mut document));
}

#[test]
fn snapshot_capture_close_is_not_blocked_by_ephemeral_scene_edits() {
    assert!(!close_request_requires_confirmation(
        true, true, false, true
    ));
    assert!(close_request_requires_confirmation(
        true, true, false, false
    ));
    assert!(!close_request_requires_confirmation(
        false, true, false, false
    ));
    assert!(!close_request_requires_confirmation(
        true, false, false, false
    ));
    assert!(!close_request_requires_confirmation(
        true, true, true, false
    ));
}

#[test]
fn sticky_context_click_targets_the_rows_exact_source_character() {
    let row = StickyContextRow {
        kind: StickyContextKind::Function,
        line: 12,
        char_index: 137,
        end_line: 20,
        text: "#let render(body) = {".to_owned(),
    };
    assert_eq!(sticky_context_jump_target(&row, false), None);
    assert_eq!(sticky_context_jump_target(&row, true), Some(137));
}

#[test]
fn sticky_context_activates_at_the_scroll_boundary_not_the_caret() {
    let source = "intro\n= Section\nbody\n";
    let logical_lines = [0..1, 1..2, 2..3];
    let visual_tops = [8.0, 32.0, 56.0];
    let visual_char_counts = [6, 10, 5];
    let visual_newlines = [true, true, true];
    let scroll_lines = sticky_context_scroll_lines(
        &logical_lines,
        |row| visual_tops.get(row).copied(),
        |row| visual_char_counts.get(row).copied(),
        |row| visual_newlines.get(row).copied(),
    )
    .unwrap();
    let anchor_at = |viewport_top| sticky_context_scroll_anchor(&scroll_lines, viewport_top);

    assert_eq!(anchor_at(7.5), None);
    assert_eq!(anchor_at(8.0), Some(4));
    assert!(sticky_context_rows(source, anchor_at(31.5).unwrap()).is_empty());

    let section_anchor = anchor_at(32.0).unwrap();
    assert_eq!(section_anchor, "intro\n= Section".chars().count() - 1);
    assert_eq!(
        sticky_context_rows(source, section_anchor)
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["= Section"]
    );
    assert_eq!(
        sticky_context_rows(source, anchor_at(80.0).unwrap())
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["= Section"]
    );
}

#[test]
fn sticky_context_is_pushed_out_continuously_at_its_ending_boundary() {
    assert_eq!(sticky_context_push_offset(64.0, 20.0, 40.0), 0.0);
    assert_eq!(sticky_context_push_offset(55.0, 20.0, 40.0), -5.0);
    assert_eq!(sticky_context_push_offset(40.0, 20.0, 40.0), -20.0);
    assert_eq!(sticky_context_push_offset(20.0, 20.0, 40.0), -40.0);
}

#[test]
fn later_sticky_context_slides_under_the_rows_that_remain() {
    let rows = [
        StickyContextMotionRow {
            end_line: 10,
            height: 20.0,
        },
        StickyContextMotionRow {
            end_line: 5,
            height: 20.0,
        },
    ];
    let layout = sticky_context_motion_layout(&rows, 0.0, |line| match line {
        10 => Some(100.0),
        5 => Some(30.0),
        _ => None,
    });

    assert_eq!(
        layout.rows,
        [
            StickyContextMotion {
                offset: 0.0,
                clip_top: 0.0,
            },
            StickyContextMotion {
                offset: -10.0,
                clip_top: 20.0,
            },
        ]
    );
    assert_eq!(layout.visible_bottom, 30.0);
}

#[test]
fn sticky_contexts_with_one_ending_boundary_slide_away_as_a_cohort() {
    let rows = [
        StickyContextMotionRow {
            end_line: 5,
            height: 20.0,
        },
        StickyContextMotionRow {
            end_line: 5,
            height: 20.0,
        },
    ];
    let layout = sticky_context_motion_layout(&rows, 0.0, |_| Some(30.0));

    assert_eq!(
        layout.rows,
        [
            StickyContextMotion {
                offset: -10.0,
                clip_top: 0.0,
            },
            StickyContextMotion {
                offset: -10.0,
                clip_top: 0.0,
            },
        ]
    );
    assert_eq!(layout.visible_bottom, 30.0);
}

#[test]
fn sticky_context_scroll_anchor_uses_the_first_row_of_a_wrapped_line() {
    let source = "intro\n= A deliberately long section\nbody\n";
    let section_chars = "= A deliberately long section\n".chars().count();
    let logical_lines = [0..1, 1..3, 3..4];
    let visual_tops = [8.0, 32.0, 52.0, 72.0];
    let visual_char_counts = [6, 12, section_chars - 12, 5];
    let visual_newlines = [true, false, true, true];
    let scroll_lines = sticky_context_scroll_lines(
        &logical_lines,
        |row| visual_tops.get(row).copied(),
        |row| visual_char_counts.get(row).copied(),
        |row| visual_newlines.get(row).copied(),
    )
    .unwrap();
    let anchor_at = |viewport_top| sticky_context_scroll_anchor(&scroll_lines, viewport_top);

    assert_eq!(anchor_at(31.5), Some(4));
    assert_eq!(
        anchor_at(32.0),
        Some("intro\n= A deliberately long section".chars().count() - 1)
    );
    assert_eq!(anchor_at(52.0), anchor_at(32.0));
    assert_eq!(
        anchor_at(72.0),
        Some("intro\n= A deliberately long section\nbody".chars().count() - 1)
    );
    assert_eq!(
        sticky_context_rows(source, anchor_at(52.0).unwrap())
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["= A deliberately long section"]
    );
}

#[test]
fn sticky_context_scroll_anchor_builds_parser_derived_ancestor_stacks() {
    let source = "= Outer\nintro\n== Inner\n#let render(body) = {\n  body\n}\n";
    let logical_lines = [0..1, 1..2, 2..3, 3..4, 4..5, 5..6];
    let visual_tops = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0];
    let visual_char_counts = source
        .split_inclusive('\n')
        .map(str::chars)
        .map(Iterator::count)
        .collect::<Vec<_>>();
    let visual_newlines = [true; 6];
    let scroll_lines = sticky_context_scroll_lines(
        &logical_lines,
        |row| visual_tops.get(row).copied(),
        |row| visual_char_counts.get(row).copied(),
        |row| visual_newlines.get(row).copied(),
    )
    .unwrap();
    let anchor_at =
        |viewport_top| sticky_context_scroll_anchor(&scroll_lines, viewport_top).unwrap();
    let row_text_at = |viewport_top| {
        sticky_context_rows(source, anchor_at(viewport_top))
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
    };

    assert_eq!(row_text_at(20.0), ["= Outer"]);
    assert_eq!(row_text_at(40.0), ["= Outer", "== Inner"]);
    assert_eq!(
        row_text_at(60.0),
        ["= Outer", "== Inner", "#let render(body) = {"]
    );
}

#[test]
fn wrapped_visual_rows_map_back_to_logical_lines() {
    assert_eq!(
        logical_line_row_ranges_from_breaks([false, false, true, false, true, false]),
        vec![0..3, 3..5, 5..6]
    );
    assert_eq!(logical_line_row_ranges_from_breaks([false]), vec![0..1]);
    assert!(logical_line_row_ranges_from_breaks([]).is_empty());
}

#[test]
fn text_edit_wrap_keeps_the_exact_unicode_source_mapping() {
    fn layout(wrap: bool) -> (usize, String, usize) {
        let context = egui::Context::default();
        let _ = theme::configure_editor_fonts(
            &context,
            theme::FontRequest::default(),
            theme::FontRequest::default(),
            false,
            theme::FONT_WEIGHT_NORMAL,
            theme::FONT_WEIGHT_NORMAL,
        );
        let mut source =
            "A deliberately long Typst markup line with 🦀 unicode that must wrap cleanly.\n#let x = 1"
                .to_owned();
        let source_chars = source.chars().count();
        let mut rows = 0;
        let mut laid_out = String::new();
        let mut end = 0;
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(180.0, 300.0))),
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(180.0);
                    let mut layouter =
                        |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
                            let mut job = egui::text::LayoutJob::simple(
                                buffer.as_str().to_owned(),
                                theme::editor_font(),
                                Color32::WHITE,
                                if wrap { wrap_width } else { f32::INFINITY },
                            );
                            job.break_on_newline = true;
                            ui.fonts_mut(|fonts| fonts.layout_job(job))
                        };
                    let output = egui::TextEdit::multiline(&mut source)
                        .desired_width(180.0)
                        .layouter(&mut layouter)
                        .show(ui);
                    rows = output.galley.rows.len();
                    laid_out = output.galley.text().to_owned();
                    end = output.galley.end().index.0;
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(end, source_chars);
        (rows, laid_out, source_chars)
    }

    let (wrapped_rows, wrapped_text, _) = layout(true);
    let (unwrapped_rows, unwrapped_text, _) = layout(false);
    assert!(wrapped_rows > logical_line_count(&wrapped_text));
    assert_eq!(unwrapped_rows, logical_line_count(&unwrapped_text));
    assert_eq!(wrapped_text, unwrapped_text);
}

#[test]
fn git_marker_lane_has_constant_width_and_spacing_at_every_digit_count() {
    let editor_left = 12.0;
    let git_width = f32::from(crate::git::editor::GUTTER_WIDTH);

    for rendered_number_width in [7.0, 14.0, 21.0, 28.0, 42.0, 84.0] {
        let number_gutter = editor_gutter_width(Some(rendered_number_width), false);
        let combined_gutter = editor_gutter_width(Some(rendered_number_width), true);
        let galley_x = editor_left + f32::from(combined_gutter);
        let number_slot_left =
            editor_gutter_geometry(galley_x).line_number_right - rendered_number_width;

        assert!((number_slot_left - (editor_left + git_width + 8.0)).abs() < 1.0);
        assert_eq!(
            combined_gutter - number_gutter,
            crate::git::editor::GUTTER_WIDTH
        );
    }

    assert_eq!(editor_gutter_width(None, false), 4);
    assert_eq!(editor_gutter_width(None, true), 10);
    assert_eq!(editor_gutter_width(Some(f32::NAN), true), 15);
    assert_eq!(editor_gutter_width(Some(f32::INFINITY), true), 15);
    assert_eq!(editor_gutter_width(Some(f32::MAX), false), 121);
    assert_eq!(editor_gutter_width(Some(f32::MAX), true), 127);
}

#[test]
fn folding_gutter_toggles_from_number_and_arrow_without_overlapping_git() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let source = "#let f(x) = {\n  αβ\n}\nVisible after fold".to_owned();
    let mut folding = crate::folding::Folding::default();
    folding.prepare(
        DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
        Arc::from(source.as_str()),
        &crate::editor_features::context_regions(
            &typst_syntax::Source::detached(source.clone()),
            &source,
        ),
    );
    let mut harness = Harness::builder()
        .with_size(Vec2::new(500.0, 300.0))
        .build_ui_state(
            |ui, (source, folding): &mut (String, crate::folding::Folding)| {
                let marker = ui.painter().layout_no_wrap(
                    "...".into(),
                    egui::FontId::monospace(14.0),
                    Color32::WHITE,
                );
                let marker_width = marker.size().x + 8.0;
                folding.set_marker_width(marker_width);
                let mut layout = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
                    folding.layout(ui.painter().layout(
                        text.as_str().into(),
                        egui::FontId::monospace(14.0),
                        Color32::WHITE,
                        width,
                    ))
                };
                let mut output = egui::TextEdit::multiline(source)
                    .code_editor()
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        left: editor_gutter_width(Some(7.0), true),
                        right: 4,
                        top: 2,
                        bottom: 2,
                    }))
                    .layouter(&mut layout)
                    .show(ui);
                let rows = logical_line_row_ranges(&output.galley.rows);
                paint_line_numbers(ui, &output, &rows);
                let gutter_clicked = paint_fold_controls(ui, &output, &rows, folding, true);
                let marker_clicked =
                    paint_fold_markers(ui, &output, &rows, folding, &marker, marker_width);
                if let Some(region) = gutter_clicked.or(marker_clicked) {
                    output.response.request_focus();
                    folding.toggle(region.line);
                    output
                        .state
                        .cursor
                        .set_char_range(Some(CCursorRange::one(CCursor::new(region.header))));
                    output.state.clone().store(ui.ctx(), output.response.id);
                    ui.ctx().request_repaint();
                }
            },
            (source.clone(), folding),
        );
    harness.run();
    harness.get_by_label("Collapse line 1").click();
    harness.run();
    assert!(harness.state().1.is_collapsed(0));
    assert_eq!(harness.state().0, source);
    let arrow = harness.get_by_label("Expand line 1").rect().left_center() + Vec2::new(2.5, 0.0);
    harness.event(egui::Event::PointerMoved(arrow));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: arrow,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.run();
    assert!(!harness.state().1.is_collapsed(0));
    assert_eq!(harness.state().0, source);
    harness.get_by_label("Collapse line 1").click();
    harness.run();
    harness.get_by_label("Expand folded line 1").click();
    harness.run();
    assert!(!harness.state().1.is_collapsed(0));
    harness.get_by_label("Collapse line 1").click();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run();
    harness.event(egui::Event::Copy);
    harness.step();
    assert!(
        harness.output().platform_output.commands.iter().any(
            |command| matches!(command, egui::OutputCommand::CopyText(text) if text == &source)
        )
    );
}

#[test]
fn production_theme_preference_releases_system_override_but_keeps_explicit_modes() {
    assert_eq!(
        active_theme_preference(InterfaceTheme::System, false, false),
        egui::ThemePreference::System
    );
    assert_eq!(
        active_theme_preference(InterfaceTheme::System, false, true),
        egui::ThemePreference::System
    );
    assert_eq!(
        active_theme_preference(InterfaceTheme::Light, false, false),
        egui::ThemePreference::Light
    );
    assert_eq!(
        active_theme_preference(InterfaceTheme::Dark, false, true),
        egui::ThemePreference::Dark
    );
    assert_eq!(
        active_theme_preference(InterfaceTheme::System, true, false),
        egui::ThemePreference::Light
    );
    assert_eq!(
        active_theme_preference(InterfaceTheme::System, true, true),
        egui::ThemePreference::Dark
    );

    let context = egui::Context::default();
    context.set_theme(active_theme_preference(
        InterfaceTheme::System,
        false,
        false,
    ));
    let output = context.run_ui(
        egui::RawInput {
            system_theme: Some(egui::Theme::Light),
            ..Default::default()
        },
        |_| {},
    );
    assert!(output.viewport_output.values().any(|viewport| {
        viewport.commands.iter().any(|command| {
            matches!(
                command,
                egui::ViewportCommand::SetTheme(egui::SystemTheme::SystemDefault)
            )
        })
    }));
    output.drop_without_applying_deltas();
    assert_eq!(context.theme(), egui::Theme::Light);

    for system_theme in [egui::Theme::Dark, egui::Theme::Light] {
        context
            .run_ui(
                egui::RawInput {
                    system_theme: Some(system_theme),
                    ..Default::default()
                },
                |_| {},
            )
            .drop_without_applying_deltas();
        assert_eq!(context.theme(), system_theme);
    }

    context.set_theme(active_theme_preference(InterfaceTheme::Light, false, false));
    context
        .run_ui(
            egui::RawInput {
                system_theme: Some(egui::Theme::Dark),
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
    assert_eq!(context.theme(), egui::Theme::Light);

    context.set_theme(active_theme_preference(InterfaceTheme::Dark, false, true));
    context
        .run_ui(
            egui::RawInput {
                system_theme: Some(egui::Theme::Light),
                ..Default::default()
            },
            |_| {},
        )
        .drop_without_applying_deltas();
    assert_eq!(context.theme(), egui::Theme::Dark);
}

#[test]
fn system_appearance_changes_select_the_configured_theme_pair_in_both_directions() {
    let mut settings = AppSettings {
        interface_theme: InterfaceTheme::System,
        light_theme: ColorThemeChoice::builtin("paper-light"),
        dark_theme: ColorThemeChoice::builtin("catppuccin-mocha"),
        ..AppSettings::default()
    };
    let light = active_theme_request(&settings, Some(egui::Theme::Light), None);
    let dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
    let light_again = active_theme_request(&settings, Some(egui::Theme::Light), None);
    assert_eq!(
        light.source,
        ThemeSourceRequest::Builtin("paper-light".to_owned())
    );
    assert_eq!(
        dark.source,
        ThemeSourceRequest::Builtin("catppuccin-mocha".to_owned())
    );
    assert_ne!(light, dark);
    assert_eq!(light, light_again);

    settings.interface_theme = InterfaceTheme::Light;
    let fixed_light = active_theme_request(&settings, Some(egui::Theme::Light), None);
    assert_eq!(
        fixed_light,
        active_theme_request(&settings, Some(egui::Theme::Dark), None)
    );
    assert_eq!(fixed_light.source, light.source);

    settings.interface_theme = InterfaceTheme::Dark;
    let fixed_dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
    assert_eq!(
        fixed_dark,
        active_theme_request(&settings, Some(egui::Theme::Light), None)
    );
    assert_eq!(fixed_dark.source, dark.source);
}

#[test]
fn theme_resolution_uses_the_slot_for_effective_appearance() {
    let mut settings = AppSettings {
        light_theme: ColorThemeChoice::builtin("paper-light"),
        dark_theme: ColorThemeChoice::sublime("/themes/custom.sublime-color-scheme"),
        theme_invert: true,
        theme_hue_shift_degrees: -20,
        ..AppSettings::default()
    };
    let override_profile = CaptureThemeProfile {
        name: "catppuccin-mocha".to_owned(),
        invert: false,
        hue_shift_degrees: 45,
    };

    let launch = active_theme_request(&settings, Some(egui::Theme::Light), Some(&override_profile));
    assert_eq!(
        launch.source,
        ThemeSourceRequest::Builtin("catppuccin-mocha".to_owned())
    );
    assert!(!launch.invert);
    assert_eq!(launch.hue_shift_degrees, 45);

    let imported = active_theme_request(&settings, Some(egui::Theme::Dark), None);
    assert_eq!(
        imported.source,
        ThemeSourceRequest::Sublime(PathBuf::from("/themes/custom.sublime-color-scheme"))
    );
    assert!(imported.invert);
    assert_eq!(imported.hue_shift_degrees, -20);

    let builtin = active_theme_request(&settings, Some(egui::Theme::Light), None);
    assert_eq!(
        builtin.source,
        ThemeSourceRequest::Builtin("paper-light".to_owned())
    );

    settings.interface_theme = InterfaceTheme::Light;
    let explicit_light = active_theme_request(&settings, Some(egui::Theme::Dark), None);
    assert_eq!(explicit_light.source, builtin.source);

    settings.interface_theme = InterfaceTheme::Dark;
    let explicit_dark = active_theme_request(&settings, Some(egui::Theme::Light), None);
    assert_eq!(explicit_dark.source, imported.source);
}

#[test]
fn system_theme_selects_a_light_or_dark_tiptop_palette() {
    let settings = AppSettings::default();
    let light = active_theme_request(&settings, Some(egui::Theme::Light), None);
    let dark = active_theme_request(&settings, Some(egui::Theme::Dark), None);
    assert_eq!(
        light.source,
        ThemeSourceRequest::Builtin("tiptop-light".to_owned())
    );
    assert_eq!(
        dark.source,
        ThemeSourceRequest::Builtin("tiptop-dark".to_owned())
    );
}

#[test]
fn document_theme_maps_to_tinymist_with_effective_preview_appearance() {
    assert_eq!(
        tinymist_invert_colors(DocumentTheme::FollowInterface, false),
        InvertColors::Never
    );
    assert_eq!(
        tinymist_invert_colors(DocumentTheme::Light, true),
        InvertColors::Never
    );
    assert_eq!(
        tinymist_invert_colors(DocumentTheme::Dark, false),
        InvertColors::Always
    );
    assert_eq!(
        tinymist_invert_colors(DocumentTheme::FollowInterface, true),
        InvertColors::Always
    );
}

#[test]
fn resolved_system_appearance_change_restarts_tinymist_preview() {
    assert!(!tinymist_restart_required(
        false, false, false, false, false
    ));
    assert!(tinymist_restart_required(true, false, false, false, false));
    assert!(tinymist_restart_required(false, true, false, false, false));
}

#[test]
fn activation_sensitive_webview_creation_waits_only_for_explicit_blur() {
    assert!(!may_create_embedded_webview(true, Some(false)));
    assert!(may_create_embedded_webview(true, Some(true)));
    assert!(may_create_embedded_webview(true, None));
    assert!(may_create_embedded_webview(false, Some(false)));
}

#[test]
fn every_secondary_window_can_embed_after_its_native_parent_is_focused() {
    assert!(!may_create_window_webview(
        EditorWindowHost::Secondary,
        true,
        None,
    ));
    assert!(!may_create_window_webview(
        EditorWindowHost::Secondary,
        false,
        Some(false),
    ));
    assert!(may_create_window_webview(
        EditorWindowHost::Secondary,
        true,
        Some(true),
    ));
    assert!(may_create_window_webview(
        EditorWindowHost::Root,
        false,
        Some(false),
    ));
}

#[test]
fn sublime_theme_accepts_either_slot_before_color_transforms() {
    let temp = tempfile::tempdir().expect("create temporary theme directory");
    let path = temp.path().join("Dark.sublime-color-scheme");
    fs::write(
        &path,
        r##"{
            "name": "Test Dark",
            "globals": {
                "background": "#151820",
                "foreground": "#edf1f7"
            }
        }"##,
    )
    .expect("write dark Sublime theme");

    let mismatched = ActiveThemeRequest {
        source: ThemeSourceRequest::Sublime(path.clone()),
        invert: true,
        hue_shift_degrees: 30,
        colors: Default::default(),
        fallback_dark: false,
    };
    let opposite = load_active_theme(&mismatched).expect("light slot accepts a dark file");

    let matching = ActiveThemeRequest {
        fallback_dark: true,
        ..mismatched
    };
    let transformed = load_active_theme(&matching).expect("dark slot accepts the dark file");
    assert!(!transformed.dark_mode, "inversion runs after loading");
    assert_eq!(opposite.palette, transformed.palette);
}

#[test]
fn active_theme_transform_is_ordered_and_non_cumulative() {
    let request = ActiveThemeRequest {
        source: ThemeSourceRequest::Builtin("catppuccin-latte".to_owned()),
        invert: true,
        hue_shift_degrees: 30,
        colors: crate::theme_transform::ThemeColorAdjustments {
            luminosity: 15,
            brightness: -5,
            contrast: 110,
            saturation: 90,
        },
        fallback_dark: false,
    };
    let original = builtin_themes::find("catppuccin-latte").unwrap();
    let expected = request.transform().apply_rgba(original.palette.background);
    let first = load_active_theme(&request).unwrap();
    let second = load_active_theme(&request).unwrap();

    assert_eq!(first.palette.background, expected);
    assert_eq!(first.palette, second.palette);
    assert_eq!(first.syntect_theme, second.syntect_theme);
    assert!(first.dark_mode);
}

#[test]
fn unknown_themes_report_an_error_and_use_the_paired_fallback() {
    let request = ActiveThemeRequest {
        source: ThemeSourceRequest::Builtin("does-not-exist".to_owned()),
        invert: false,
        hue_shift_degrees: 0,
        colors: Default::default(),
        fallback_dark: true,
    };
    let (fallback, error) = load_active_theme_or_fallback(&request);
    assert!(error.unwrap().contains("does-not-exist"));
    assert_eq!(fallback.name.as_deref(), Some("Tiptop Dark"));
    assert!(fallback.dark_mode);
}

#[test]
fn light_and_dark_styles_have_identical_layout_geometry() {
    let context = egui::Context::default();
    theme::configure_styles(&context);
    let dark = context.style_of(egui::Theme::Dark);
    let light = context.style_of(egui::Theme::Light);

    assert_eq!(dark.spacing, light.spacing);
    assert_eq!(dark.text_styles, light.text_styles);
    assert_eq!(dark.spacing.item_spacing, egui::vec2(6.0, 4.0));
    assert_eq!(dark.spacing.button_padding, egui::vec2(7.0, 3.0));
    assert_eq!(
        dark.text_styles.get(&egui::TextStyle::Monospace),
        Some(&theme::editor_font())
    );
}

#[test]
fn automatic_preview_fallback_preserves_and_reports_user_intent() {
    let preference = PreviewPreference::Interactive;
    let tinymist = ServiceState::Failed("tinymist executable was not found".to_owned());
    let webview = ServiceState::Starting("waiting".to_owned());

    assert_eq!(
        preview_fallback_reason_for(preference, false, false, &tinymist, &webview),
        Some("Failed: tinymist executable was not found".to_owned())
    );
    assert_eq!(
        preview_backend_label_for(preference, false),
        "Rasterised PDF · fallback"
    );
    assert_eq!(preference, PreviewPreference::Interactive);
}

#[test]
fn explicitly_selected_native_preview_is_not_a_fallback() {
    let tinymist = ServiceState::Failed("unavailable".to_owned());
    let webview = ServiceState::Failed("unavailable".to_owned());

    assert_eq!(
        preview_fallback_reason_for(PreviewPreference::Native, false, false, &tinymist, &webview,),
        None
    );
    assert_eq!(
        preview_backend_label_for(PreviewPreference::Native, false),
        "Rasterised PDF"
    );
}

#[test]
fn embedded_viewer_failure_is_reported_after_preview_server_startup() {
    let tinymist = ServiceState::Ready("server ready".to_owned());
    let webview = ServiceState::Failed("child webview could not load".to_owned());

    assert_eq!(
        preview_fallback_reason_for(
            PreviewPreference::Interactive,
            false,
            true,
            &tinymist,
            &webview,
        ),
        Some("Failed: child webview could not load".to_owned())
    );
}

#[test]
fn bottom_status_collects_every_non_preview_fallback() {
    let settings = AppSettings::default();
    let typst = ToolResolution {
        kind: ToolKind::Typst,
        program: PathBuf::from("typst"),
        origin: ToolOrigin::Path,
        fallback_reason: Some("bundled Typst is missing; using PATH".to_owned()),
    };
    let tinymist = ToolResolution {
        kind: ToolKind::Tinymist,
        program: PathBuf::from("tinymist"),
        origin: ToolOrigin::Missing,
        fallback_reason: Some("Tinymist is unavailable".to_owned()),
    };

    let details = non_preview_fallback_details_for(&settings, None, &typst, &tinymist);
    assert_eq!(details.len(), 3);
    assert!(details[0].starts_with("Appearance:"));
    assert_eq!(details[1], "Typst: bundled Typst is missing; using PATH");
    assert_eq!(details[2], "Tinymist: Tinymist is unavailable");
}

#[test]
fn git_line_change_summary_keeps_all_change_categories_visible() {
    let summary = EditorApp::git_line_change_summary(crate::git::editor::LineChangeCounts {
        added: 12,
        modified: 3,
        deleted: 7,
    });

    assert_eq!(summary, "Git: +12 added · ~3 modified · −7 deleted");
}

#[test]
fn tinymist_diagnostics_become_one_based_inline_diagnostics() {
    let converted = tinymist_diagnostic(
        TinymistDiagnostic {
            range: LspRange {
                start: LspPosition::new(4, 7),
                end: LspPosition::new(4, 11),
            },
            severity: Some(TinymistDiagnosticSeverity::Warning),
            code: Some(serde_json::json!("deprecated")),
            source: Some("tinymist".to_owned()),
            message: "old syntax".to_owned(),
            raw: serde_json::json!({}),
        },
        DiagnosticSource::Main,
    );

    assert_eq!(converted.severity, DiagnosticSeverity::Warning);
    assert_eq!(converted.location.unwrap().line, 5);
    assert_eq!(converted.location.unwrap().column, 8);
    assert_eq!(converted.full_message(), "old syntax\ncode: \"deprecated\"");
}

#[test]
fn capture_scene_replacement_does_not_replace_the_batch_fixture() {
    let mut document = DocumentSession::new(
        tiptoptyp_core::document::WindowSessionId::new(1),
        "initial",
        DocumentKind::Typst,
    );
    document.replace_loaded(
        "original fixture".to_owned(),
        PathBuf::from("fixture.typ"),
        DocumentKind::Typst,
        Some(7),
    );
    let fixture = SceneDocument::capture(&document);
    let before = document.key();
    document.replace_untitled(r#"#set text(font: "")"#);
    fixture.restore(&mut document);
    assert_eq!(document.source(), "original fixture");
    assert_eq!(document.path(), &Some(PathBuf::from("fixture.typ")));
    assert_eq!(document.disk_fingerprint(), Some(7));
    assert!(!document.is_dirty());
    assert_ne!(document.key(), before);
    assert_eq!(document.key().owner, before.owner);
}

#[test]
fn bracket_settings_controls_change_each_palette_and_can_be_disabled() {
    use crate::rainbow::{BracketFamily, BracketPalette, RainbowBrackets};
    use egui_kittest::{Harness, kittest::Queryable as _};
    let mut harness = Harness::builder()
        .with_size(Vec2::new(420.0, 360.0))
        .build_ui_state(
            settings_panel::show_bracket_controls,
            RainbowBrackets::default(),
        );
    harness.run();
    for family in BracketFamily::ALL {
        let picker = harness.get_by_role_and_label(egui::accesskit::Role::ComboBox, family.label());
        assert!(picker.rect().right() <= 420.0);
    }
    harness
        .get_by_role_and_label(
            egui::accesskit::Role::ComboBox,
            BracketFamily::Round.label(),
        )
        .click();
    harness.run();
    harness
        .get_by_role_and_label(egui::accesskit::Role::Button, "Orchid")
        .click();
    harness.run();
    assert_eq!(
        harness.state().palettes,
        [
            BracketPalette::Orchid,
            BracketPalette::Forest,
            BracketPalette::Sunset,
            BracketPalette::Orchid
        ]
    );
    harness.get_by_label("Rainbow brackets").click();
    harness.run();
    assert!(!harness.state().enabled);
    harness.get_by_label("Rainbow brackets").click();
    harness.run();
    assert!(harness.state().enabled);
    assert_eq!(harness.state().palettes[0], BracketPalette::Orchid);
}
