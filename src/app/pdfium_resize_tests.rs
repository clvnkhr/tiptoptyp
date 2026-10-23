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
