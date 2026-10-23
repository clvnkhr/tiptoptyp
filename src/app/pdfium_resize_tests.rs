//! Deterministic PDF publication tests; no native renderer or worker thread.
use super::*;

const LIGHT: (Color32, Color32) = (Color32::WHITE, Color32::BLACK);
const DARK: (Color32, Color32) = (
    Color32::from_rgb(30, 40, 50),
    Color32::from_rgb(200, 210, 220),
);

fn key(scale: u32) -> RequestKey {
    RequestKey {
        revision: 1,
        pages: vec![0],
        scale,
        palette: LIGHT,
    }
}

fn batch(key: &RequestKey) -> Batch {
    Batch {
        catalog: Arc::new(Catalog {
            sizes: vec![[200.0, 300.0], [200.0, 300.0]],
            outline: vec![],
        }),
        pages: key
            .pages
            .iter()
            .map(|&index| crate::pdfium::Page {
                index,
                image: crate::pdfium::prepared_image(
                    [2, 2],
                    [0, 64, 128, 255].repeat(4),
                    key.palette,
                ),
                characters: vec![],
                links: vec![],
            })
            .collect(),
        parsed_new_document: false,
    }
}

fn reply(key: &RequestKey) -> Reply {
    Reply {
        key: key.clone(),
        result: Ok(batch(key)),
    }
}

fn available(width: f32) -> Rect {
    Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 600.0))
}

fn frame(
    context: &egui::Context,
    view: &mut PdfiumView,
    key: &RequestKey,
    width: f32,
    resizing: bool,
    now: f64,
) -> usize {
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(available(width)),
            time: Some(now),
            ..Default::default()
        },
        |ui| view.finish_frame(ui, key.clone(), available(width), resizing),
    );
    let uploads = output.textures_delta.set.len();
    output.drop_without_applying_deltas();
    uploads
}

fn seeded() -> (egui::Context, PdfiumView, RequestKey) {
    let context = egui::Context::default();
    context.set_pixels_per_point(1.0);
    let initial = key(100);
    let mut view = PdfiumView {
        worker: Some(Worker::idle_for_tests()),
        bytes: Some(Arc::from(&b"deterministic PDF placeholder"[..])),
        revision: initial.revision,
        requested: Some(initial.clone()),
        desired: Some(initial.clone()),
        ..Default::default()
    };
    let mut prepared = Some(batch(&initial));
    context
        .run_ui(
            egui::RawInput {
                screen_rect: Some(available(1000.0)),
                time: Some(0.0),
                ..Default::default()
            },
            |ui| {
                view.accept(prepared.take().unwrap(), &initial, ui);
                view.finish_frame(ui, initial.clone(), available(1000.0), false);
            },
        )
        .drop_without_applying_deltas();
    assert!(view.ready());
    (context, view, initial)
}

#[test]
fn a_hundred_held_widths_keep_the_same_texture_and_dispatch_only_on_release() {
    let (context, mut view, initial) = seeded();
    let texture = view.residents[&0].texture.id();
    view.pending_reply = Some(reply(&initial));
    for step in 1..=100 {
        let desired = key(100 + step);
        assert_eq!(
            frame(
                &context,
                &mut view,
                &desired,
                1000.0 + step as f32,
                true,
                f64::from(step) / 1000.0,
            ),
            0,
            "held divider changes must not allocate or upload textures"
        );
        assert_eq!(view.requested.as_ref(), Some(&initial));
        assert_eq!(view.desired.as_ref(), Some(&desired));
        assert_eq!(view.active_scale, initial.scale);
        assert_eq!(view.residents[&0].texture.id(), texture);
        assert!(view.pending_reply.is_some());
        assert!(!view.ready());
    }
    let final_key = key(200);
    assert_eq!(
        frame(&context, &mut view, &final_key, 1100.0, false, 0.101),
        0
    );
    assert_eq!(view.requested.as_ref(), Some(&final_key));
    assert!(view.pending_reply.is_none(), "the old-width reply is stale");
    assert_eq!(view.residents[&0].texture.id(), texture);
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    assert_eq!((view.render_requests, view.render_admissions), (1, 1));

    view.pending_reply = Some(reply(&final_key));
    assert_eq!(
        frame(&context, &mut view, &final_key, 1100.0, false, 0.102),
        1
    );
    assert!(view.ready());
    assert_ne!(view.residents[&0].texture.id(), texture);
    let admitted = view.residents[&0].texture.id();
    for step in 1..=10 {
        assert_eq!(
            frame(
                &context,
                &mut view,
                &final_key,
                1100.0,
                false,
                0.102 + f64::from(step)
            ),
            0
        );
        assert_eq!(view.residents[&0].texture.id(), admitted);
        assert_eq!(view.requested.as_ref(), Some(&final_key));
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    assert_eq!((view.render_requests, view.render_admissions), (1, 2));
}

#[test]
fn wrong_width_palette_and_revision_replies_never_replace_resident_pixels() {
    for mismatch in 0..3 {
        let (context, mut view, initial) = seeded();
        let texture = view.residents[&0].texture.id();
        let desired = key(200);
        frame(&context, &mut view, &desired, 1100.0, true, 0.01);
        let mut stale = desired.clone();
        match mismatch {
            0 => stale.scale -= 1,
            1 => stale.palette = DARK,
            _ => stale.revision += 1,
        }
        view.pending_reply = Some(reply(&stale));
        assert_eq!(frame(&context, &mut view, &desired, 1100.0, false, 0.02), 0);
        assert_eq!(view.residents[&0].texture.id(), texture);
        assert_eq!(view.active_revision, initial.revision);
        assert_eq!(view.active_scale, initial.scale);
        assert_eq!(view.palette, initial.palette);
        assert_eq!(view.requested.as_ref(), Some(&desired));
        assert!(view.pending_reply.is_none());
        assert!(!view.ready());
    }
}

#[test]
fn a_matching_reply_survives_a_stationary_held_drag_until_release() {
    let (context, mut view, _) = seeded();
    let texture = view.residents[&0].texture.id();
    let desired = key(200);
    // This render was dispatched before the user grabbed the divider.
    view.requested = Some(desired.clone());
    view.pending_reply = Some(reply(&desired));
    for now in [0.01, 0.1, 1.0, 600.0] {
        assert_eq!(frame(&context, &mut view, &desired, 1100.0, true, now), 0);
        assert_eq!(view.pending_reply.as_ref().unwrap().key, desired);
        assert_eq!(view.residents[&0].texture.id(), texture);
        assert!(!view.ready());
    }
    assert_eq!(
        frame(&context, &mut view, &desired, 1100.0, false, 600.01),
        1
    );
    assert!(view.pending_reply.is_none());
    assert!(view.ready());
    assert_ne!(view.residents[&0].texture.id(), texture);
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    assert_eq!(
        view.render_requests, 0,
        "the retained reply needs no duplicate render"
    );
}

#[test]
fn missing_pages_new_revisions_and_new_palettes_bypass_dispatch_and_admission_delays() {
    for change in 0..3 {
        let (context, mut view, initial) = seeded();
        let texture = view.residents[&0].texture.id();
        let mut desired = key(200);
        match change {
            0 => desired.pages.push(1),
            1 => {
                desired.revision += 1;
                view.revision = desired.revision;
            }
            _ => desired.palette = DARK,
        }
        assert_eq!(frame(&context, &mut view, &desired, 1100.0, true, 0.01), 0);
        assert_eq!(view.requested.as_ref(), Some(&desired));
        assert_eq!(view.residents[&0].texture.id(), texture);
        assert_eq!(
            view.palette, initial.palette,
            "dispatch cannot claim new colors are displayed"
        );
        assert!(!view.ready());
        view.pending_reply = Some(reply(&desired));
        assert_eq!(
            frame(&context, &mut view, &desired, 1100.0, true, 0.02),
            desired.pages.len(),
            "first usable pixels must appear even if the divider remains held"
        );
        assert!(view.ready());
        assert_ne!(view.residents[&0].texture.id(), texture);
        assert_eq!(view.palette, desired.palette);
        assert_eq!(view.active_revision, desired.revision);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        assert_eq!(view.render_requests, 1);
    }
}

#[test]
fn readiness_follows_desired_pixels_instead_of_the_last_dispatched_request() {
    let (_, mut view, initial) = seeded();
    assert!(view.ready());
    for change in 0..4 {
        let mut desired = initial.clone();
        match change {
            0 => desired.scale += 1,
            1 => desired.palette = DARK,
            2 => desired.revision += 1,
            _ => desired.pages.push(1),
        }
        view.desired = Some(desired);
        assert_eq!(view.requested.as_ref(), Some(&initial));
        assert!(
            !view.ready(),
            "the previous request cannot claim changed desired pixels are ready"
        );
    }
    view.desired = Some(initial);
    assert!(view.ready());
}

#[test]
fn publishing_replacement_page_text_retires_its_search_matches() {
    let (context, mut view, key) = seeded();
    let characters = |text: &str| {
        text.chars()
            .map(|value| Character {
                value,
                rect: Rect::NOTHING,
            })
            .collect::<Vec<_>>()
    };
    let mut first = batch(&key);
    first.pages[0].characters = characters("É");
    let mut first = Some(first);
    context
        .run_ui(egui::RawInput::default(), |ui| {
            view.accept(first.take().unwrap(), &key, ui);
        })
        .drop_without_applying_deltas();
    let resident = view.residents.get_mut(&0).unwrap();
    assert_eq!(
        resident
            .search_matches
            .for_query(&resident.characters, &Arc::from("é")),
        std::slice::from_ref(&(0..1))
    );
    let mut replacement = batch(&key);
    replacement.pages[0].characters = characters("other");
    let mut replacement = Some(replacement);
    context
        .run_ui(egui::RawInput::default(), |ui| {
            view.accept(replacement.take().unwrap(), &key, ui);
        })
        .drop_without_applying_deltas();
    let resident = view.residents.get_mut(&0).unwrap();
    assert!(
        resident
            .search_matches
            .for_query(&resident.characters, &Arc::from("é"))
            .is_empty()
    );
    assert_eq!(resident.search_matches.rebuilds, 1);
}

#[test]
fn find_edits_and_clear_keep_ready_pixels_and_the_same_texture() {
    let (context, mut view, initial) = seeded();
    let texture = view.residents[&0].texture.id();
    for (step, query) in ["p", "preview", ""].into_iter().enumerate() {
        view.controls_action(super::super::preview_controls::Action::Find(
            query.to_owned(),
        ));
        let desired = initial.clone();
        assert_eq!(
            frame(
                &context,
                &mut view,
                &desired,
                1000.0,
                false,
                1.0 + step as f64
            ),
            0
        );
        assert_eq!(
            view.requested.as_ref(),
            Some(&initial),
            "Find edits must not dispatch a replacement raster"
        );
        assert_eq!(view.residents[&0].texture.id(), texture);
        assert!(view.ready(), "Find must not change pixel readiness");
    }
}

#[test]
fn pinch_keeps_the_page_point_at_the_cursor_across_fixed_gaps_and_centering() {
    let sizes = vec![[600.0, 800.0]; 100];
    let cursor = egui::vec2(550.0, 200.0);
    for page in [0, 7, 99] {
        let old_top = page_tops(&sizes, 1.0)[page];
        let new_top = page_tops(&sizes, 2.0)[page];
        // At zoom 1 this 600pt page is centered in a 1000pt viewport. The
        // cursor is over PDF coordinate (350, 300), away from scroll clamps.
        let old = egui::vec2(0.0, old_top + 300.0 - cursor.y);
        let offset = PageZoomAnchor::capture(
            &sizes,
            ([&page_tops(&sizes, 1.0), &page_tops(&sizes, 2.0)])[0],
            ([1000.0, 1200.0])[0],
            old,
            cursor,
            1.0,
        )
        .and_then(|anchor| {
            anchor.offset(
                &sizes,
                ([&page_tops(&sizes, 1.0), &page_tops(&sizes, 2.0)])[1],
                ([1000.0, 1200.0])[1],
                cursor,
                2.0,
            )
        })
        .unwrap_or(old);
        let after = egui::vec2(350.0 * 2.0 - offset.x, new_top + 300.0 * 2.0 - offset.y);
        assert!(
            (after - cursor).length() < 0.001,
            "page {page}: fixed gutters and centered paper must not move the PDF point under the cursor: {after:?}"
        );
    }
}

#[test]
fn search_results_survive_viewport_changes_but_never_old_queries_or_documents() {
    let (context, mut view, initial) = seeded();
    view.controls_action(super::super::preview_controls::Action::Find(
        "norm".to_owned(),
    ));
    frame(&context, &mut view, &initial, 1000.0, false, 1.0);
    let search = view.search_requested.clone().unwrap();
    for stale in [
        SearchKey {
            query: "old".into(),
            ..search.clone()
        },
        SearchKey {
            revision: search.revision + 1,
            ..search.clone()
        },
    ] {
        view.accept_search(stale, vec![0]);
        assert!(view.search_pending);
        assert!(view.search_pages.is_empty());
        assert!(view.goto.is_none());
    }
    let desired = RequestKey {
        pages: vec![1],
        ..initial
    };
    frame(&context, &mut view, &desired, 1000.0, false, 2.0);
    assert_eq!(view.search_requested.as_ref(), Some(&search));
    view.accept_search(search.clone(), vec![1]);
    assert!(!view.search_pending);
    assert_eq!(view.search_pages, [1]);
    assert_eq!(view.goto, Some(1));
    view.goto = None;
    view.controls_action(super::super::preview_controls::Action::Find(String::new()));
    view.accept_search(search, vec![0]);
    assert!(view.search_pages.is_empty());
    assert!(!view.search_pending);
    assert!(view.goto.is_none());
}

#[test]
fn a_find_edit_does_not_reject_an_already_completed_pixel_replacement() {
    let (context, mut view, _) = seeded();
    let desired = key(200);
    view.requested = Some(desired.clone());
    view.pending_reply = Some(reply(&desired));
    assert_eq!(frame(&context, &mut view, &desired, 1100.0, true, 0.01), 0);
    view.controls_action(super::super::preview_controls::Action::Find(
        "norm".to_owned(),
    ));
    assert_eq!(frame(&context, &mut view, &desired, 1100.0, false, 0.02), 1);
    assert!(view.ready());
    assert!(view.search_pending);
    assert_eq!(view.active_scale, desired.scale);
}

#[test]
fn pinch_keeps_fixed_page_gutters_and_mixed_page_geometry() {
    let sizes = [[600.0, 800.0], [300.0, 400.0], [500.0, 700.0]];
    let old_tops = page_tops(&sizes, 1.0);
    let new_tops = page_tops(&sizes, 1.5);
    let cursor = egui::vec2(350.0, 200.0);
    let old = egui::vec2(100.0, old_tops[1] + 400.0 + 6.0 - cursor.y);
    let offset = PageZoomAnchor::capture(
        &sizes,
        ([&old_tops, &new_tops])[0],
        ([600.0, 900.0])[0],
        old,
        cursor,
        1.0,
    )
    .and_then(|anchor| {
        anchor.offset(
            &sizes,
            ([&old_tops, &new_tops])[1],
            ([600.0, 900.0])[1],
            cursor,
            1.5,
        )
    })
    .unwrap_or(old);
    assert!((new_tops[1] + 400.0 * 1.5 + 6.0 - offset.y - cursor.y).abs() < 0.001);
    assert!((225.0 + 300.0 * 1.5 - offset.x - cursor.x).abs() < 0.001);
    let before_top_margin = egui::vec2(100.0, 2.0);
    let offset = PageZoomAnchor::capture(
        &sizes,
        ([&old_tops, &new_tops])[0],
        ([600.0, 900.0])[0],
        before_top_margin,
        egui::vec2(350.0, 4.0),
        1.0,
    )
    .and_then(|anchor| {
        anchor.offset(
            &sizes,
            ([&old_tops, &new_tops])[1],
            ([600.0, 900.0])[1],
            egui::vec2(350.0, 4.0),
            1.5,
        )
    })
    .unwrap_or(before_top_margin);
    assert_eq!(
        offset.y, before_top_margin.y,
        "top margin is fixed CSS space"
    );
    for (sizes, tops, zoom) in [
        (&sizes[..0], &old_tops[..0], 1.0),
        (&sizes[..], &old_tops[..0], 1.0),
        (&sizes[..], &old_tops[..], 0.0),
    ] {
        assert_eq!(
            PageZoomAnchor::capture(
                sizes,
                ([tops, &new_tops])[0],
                ([600.0, 900.0])[0],
                old,
                cursor,
                zoom
            )
            .and_then(|anchor| anchor.offset(
                sizes,
                ([tops, &new_tops])[1],
                ([600.0, 900.0])[1],
                cursor,
                1.5
            ))
            .unwrap_or(old),
            old
        );
    }
}

#[test]
fn distant_fractional_page_zoom_uses_the_exact_new_layout_tops() {
    let sizes = vec![[600.0, 841.8898]; 10_000];
    let old_tops = page_tops(&sizes, 0.73);
    let new_tops = page_tops(&sizes, 1.11);
    let cursor = egui::vec2(250.0, 200.0);
    for page in [7, 99, 999, 9999] {
        let old = egui::vec2(0.0, old_tops[page] + 300.0 * 0.73 - cursor.y);
        let offset = PageZoomAnchor::capture(
            &sizes,
            ([&old_tops, &new_tops])[0],
            ([500.0, 666.0])[0],
            old,
            cursor,
            0.73,
        )
        .and_then(|anchor| {
            anchor.offset(
                &sizes,
                ([&old_tops, &new_tops])[1],
                ([500.0, 666.0])[1],
                cursor,
                1.11,
            )
        })
        .unwrap_or(old);
        let actual_y = (new_tops[page] + 300.0 * 1.11) - offset.y;
        assert!(
            (actual_y - cursor.y).abs() <= 1.0,
            "page {page}: actual rounded page geometry moved the cursor by {}pt",
            actual_y - cursor.y
        );
    }
}

#[test]
fn pinch_scroll_owner_matches_the_real_scroll_area_state() {
    let context = egui::Context::default();
    context.set_pixels_per_point(1.0);
    for (frame, desired) in [500.0, 900.0].into_iter().enumerate() {
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(available(1000.0)),
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |ui| {
                    let owner = pdfium_scroll_id(ui);
                    let mut scroll = egui::ScrollArea::both()
                        .id_salt("pdfium-scroll")
                        .auto_shrink([false, false]);
                    if frame == 0 {
                        scroll = scroll.vertical_scroll_offset(desired);
                    } else {
                        let mut state = egui::scroll_area::State::load(ui.ctx(), owner).expect(
                            "the owning state must exist after the real scroll area painted",
                        );
                        assert_eq!(state.offset.y, 500.0);
                        state.offset.y = desired;
                        state.store(ui.ctx(), owner);
                    }
                    let output = scroll.show_viewport(ui, |ui, viewport| {
                        ui.set_min_size(egui::vec2(1000.0, 4000.0));
                        viewport
                    });
                    assert_eq!(
                        output.id, owner,
                        "pinch must update the same persisted state which ScrollArea uses"
                    );
                    assert_eq!(output.inner.top(), desired);
                    assert_eq!(output.state.offset.y, desired);
                },
            )
            .drop_without_applying_deltas();
    }
}

fn painted_widget_frame(
    context: &egui::Context,
    view: &mut PdfiumView,
    texture: egui::TextureId,
    pointer: Pos2,
    zoom: Option<f32>,
    time: f64,
    pixels_per_point: f32,
) -> (Rect, Rect) {
    let bytes = view.bytes.as_ref().unwrap().clone();
    let mut events = vec![egui::Event::PointerMoved(pointer)];
    if let Some(zoom) = zoom {
        events.push(egui::Event::Zoom(zoom));
    }
    let mut input = egui::RawInput {
        screen_rect: Some(available(1000.0)),
        time: Some(time),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(pixels_per_point);
    let mut viewport = Rect::NOTHING;
    let output = context.run_ui(input, |ui| {
        viewport = ui.available_rect_before_wrap();
        view.show(
            ui,
            PathBuf::new(),
            bytes.clone(),
            LIGHT,
            false,
            crate::settings::RevealAnimation::default(),
        );
    });
    let rect = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                Some(mesh.calc_bounds())
            }
            _ => None,
        })
        .expect("the same resident page must remain painted at the pointer after pinch");
    output.drop_without_applying_deltas();
    (rect, viewport)
}

#[test]
fn real_pdfium_widget_keeps_its_painted_page_point_under_pinch() {
    for (pixels_per_point, margin) in [
        (1.0, egui::Margin::ZERO),
        (
            1.5,
            egui::Margin {
                left: 7,
                right: 11,
                top: 9,
                bottom: 13,
            },
        ),
        (2.0, egui::Margin::ZERO),
    ] {
        for page in [7, 99] {
            let (context, mut view, _) = seeded();
            context.all_styles_mut(|style| style.spacing.scroll.content_margin = margin);
            let mut initial = key(100);
            initial.pages = vec![page];
            let mut prepared = batch(&initial);
            prepared.catalog = Arc::new(Catalog {
                sizes: vec![[600.0, 800.0]; 100],
                outline: vec![],
            });
            let mut prepared = Some(prepared);
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(available(1000.0)),
                        ..Default::default()
                    },
                    |ui| {
                        view.accept(prepared.take().unwrap(), &initial, ui);
                    },
                )
                .drop_without_applying_deltas();
            view.zoom = 2.7;
            view.fit = false;
            view.goto = Some(page);
            let texture = view.residents[&page].texture.id();
            let mut pointer = egui::pos2(550.0, 200.0);
            painted_widget_frame(
                &context,
                &mut view,
                texture,
                pointer,
                None,
                10.0,
                pixels_per_point,
            );
            let owner = view.scroll_owner.unwrap();
            let mut state = egui::scroll_area::State::load(&context, owner).unwrap();
            state.offset = egui::vec2(203.7, view.tops[page] + 203.7);
            state.store(&context, owner);
            let mut time = 11.0;
            for interruption in 0..4 {
                if interruption == 1 {
                    let mut state = egui::scroll_area::State::load(&context, owner).unwrap();
                    state.offset += egui::vec2(25.3, 17.7);
                    state.store(&context, owner);
                } else if interruption == 2 {
                    pointer += egui::vec2(-37.3, 13.7);
                } else if interruption == 3 {
                    // Force the horizontal document border, then reverse the
                    // gesture. The next burst must use the clamped painted
                    // point rather than the previous off-document request.
                    let (_, viewport) = painted_widget_frame(
                        &context,
                        &mut view,
                        texture,
                        pointer,
                        Some(0.4),
                        time,
                        pixels_per_point,
                    );
                    let offset = egui::scroll_area::State::load(&context, owner)
                        .unwrap()
                        .offset;
                    assert_eq!(
                        offset.x, 0.0,
                        "the edge case must genuinely reach the document border"
                    );
                    assert!(
                        !view
                            .zoom_origin
                            .continues(&crate::preview::ZoomOriginState {
                                document: view.revision,
                                viewport,
                                pointer: pointer
                                    - viewport.min
                                    - egui::vec2(margin.leftf(), margin.topf()),
                                pixels_per_point,
                                zoom: view.zoom,
                                offset,
                            }),
                        "a clamped request must re-anchor from the next painted point"
                    );
                    time += 1.0;
                }
                let (rect, _) = painted_widget_frame(
                    &context,
                    &mut view,
                    texture,
                    pointer,
                    None,
                    time,
                    pixels_per_point,
                );
                let source_point = [
                    (f64::from(pointer.x) - f64::from(rect.min.x)) / f64::from(view.zoom),
                    (f64::from(pointer.y) - f64::from(rect.min.y)) / f64::from(view.zoom),
                ];
                for step in 0..200 {
                    time += 1.0 / 60.0;
                    let factor = if interruption == 3 && step == 0 {
                        // Leave the zero-horizontal-range fit before asserting
                        // a fixed anchor: its ordinary clamp can otherwise
                        // legitimately move the point on every small zoom.
                        3.0
                    } else if step < 100 {
                        1.003
                    } else {
                        1.0 / 1.003
                    };
                    let (rect, viewport) = painted_widget_frame(
                        &context,
                        &mut view,
                        texture,
                        pointer,
                        Some(factor),
                        time,
                        pixels_per_point,
                    );
                    assert_eq!(view.page, page, "pinch must not reveal a different page");
                    let offset = egui::scroll_area::State::load(&context, owner)
                        .unwrap()
                        .offset;
                    let width = (600.0 * view.zoom).max(viewport.width());
                    let left = (width - 600.0 * view.zoom) * 0.5;
                    let origin = viewport.min + egui::vec2(margin.leftf(), margin.topf());
                    let ideal = [
                        f64::from(origin.x)
                            + f64::from(left)
                            + source_point[0] * f64::from(view.zoom)
                            - f64::from(pointer.x),
                        f64::from(origin.y)
                            + f64::from(view.tops[page])
                            + source_point[1] * f64::from(view.zoom)
                            - f64::from(pointer.y),
                    ];
                    for (axis, ideal) in ideal.into_iter().enumerate() {
                        let half_ulp = 0.5
                            * f64::from(
                                f32::from_bits(offset[axis].abs().to_bits() + 1)
                                    - offset[axis].abs(),
                            );
                        // Page-local coordinates must survive the entire burst:
                        // only the final f64 -> f32 scroll-state store may round.
                        let logical_error = (f64::from(offset[axis]) - ideal).abs();
                        assert!(
                            logical_error <= half_ulp + 0.001,
                            "page {page} ppp{pixels_per_point} interruption{interruption} step{step} axis{axis}: logical drift {logical_error}, half-ULP {half_ulp}"
                        );
                        // Pinned ScrollArea first rounds to a physical pixel,
                        // then to emath's 1/32pt GUI grid. At distant pages the
                        // one final f32 offset store also has a measurable ULP.
                        let gui_rounding = if pixels_per_point == 1.5 {
                            1.0 / 64.0
                        } else {
                            0.0
                        };
                        let limit =
                            0.5 / f64::from(pixels_per_point) + gui_rounding + half_ulp + 0.003;
                        let painted_error = (f64::from(rect.min[axis])
                            + source_point[axis] * f64::from(view.zoom)
                            - f64::from(pointer[axis]))
                        .abs();
                        assert!(
                            painted_error <= limit,
                            "page {page} ppp{pixels_per_point} interruption{interruption} step{step} axis{axis}: painted drift {painted_error}, bound {limit}"
                        );
                    }
                }
                time += 1.0;
            }
        }
    }
}
