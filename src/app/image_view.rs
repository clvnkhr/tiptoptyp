//! Image zoom and painting. PDF documents are exclusively handled by PDFium.
use super::*;

pub(super) fn request_zoom(preview: &mut PreviewController, action: PreviewZoomAction) {
    let zoom = match action {
        PreviewZoomAction::In => preview.zoom * METRICS.preview.zoom_step,
        PreviewZoomAction::Out => preview.zoom / METRICS.preview.zoom_step,
        PreviewZoomAction::Reset => 1.0,
    };
    preview.requested_zoom = Some(zoom.clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM));
    preview.fit_width = false;
}

pub(super) fn show_controls(
    ui: &mut egui::Ui,
    preview: &mut PreviewController,
    dark_images: bool,
) -> Option<bool> {
    if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
        request_zoom(preview, PreviewZoomAction::Out);
    }
    if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
        request_zoom(preview, PreviewZoomAction::In);
    }
    ui.label(format!("{:.0}%", preview.zoom * 100.0));
    if icon_button(ui, UiIcon::FitWidth, "Fit image width").clicked() {
        preview.fit_width = !preview.fit_width;
    }
    let dark = native_hover_text(
        icons::toolbar_button(
            ui,
            true,
            dark_images,
            UiIcon::Contrast,
            "Dark images",
            crate::settings::ToolbarStyle::Icons,
            false,
        ),
        "Dark images — follow the page appearance setting. Light pages keep the original colors.",
    );
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.is_rect_visible(dark.rect) {
        crate::desktop_test::observe("image.dark", &dark);
    }
    dark.clicked().then_some(!dark_images)
}

pub(super) fn show_pages(ui: &mut egui::Ui, preview: &mut PreviewController) {
    let viewport_rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(viewport_rect, 0.0, preview_background(ui));
    if preview.content.pages().is_empty() {
        let failed = preview.status == PreviewStatus::Error;
        show_centered_preview_message(
            ui,
            if failed {
                "Could not load image"
            } else {
                "Loading image…"
            },
            !failed,
        );
        return;
    }

    let widest_page = preview
        .content
        .pages()
        .iter()
        .map(|page| page.size[0] as f32 * PDF_POINTS_PER_PREVIEW_PIXEL)
        .fold(1.0_f32, f32::max);
    if preview.fit_width {
        preview.zoom = ((viewport_rect.width() - PAGE_MARGIN * 2.0) / widest_page)
            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
    }

    let scroll_id = ui.make_persistent_id(egui::IdSalt::new("image-preview-scroll"));
    let pointer = ui.ctx().input(|input| input.pointer.latest_pos());
    let pinch = ui.ctx().input(|input| input.zoom_delta());
    let pinch_active = pointer.is_some_and(|pointer| viewport_rect.contains(pointer))
        && (pinch - 1.0).abs() > 0.001;
    let old_zoom = preview.zoom;
    let new_zoom = if pinch_active {
        preview.fit_width = false;
        (preview.zoom * pinch).clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM)
    } else {
        preview.requested_zoom.take().unwrap_or(preview.zoom)
    };
    let zoom_changed = (new_zoom - old_zoom).abs() > f32::EPSILON;
    let geometries = page_stack_geometry(
        preview.content.pages().iter().map(|page| page.size),
        if zoom_changed { new_zoom } else { old_zoom },
    );
    let content_width = page_canvas_width(
        viewport_rect.width(),
        geometries
            .iter()
            .map(|page| page.size.x)
            .fold(0.0, f32::max),
    );
    if zoom_changed {
        let old_pages = page_stack_geometry(
            preview.content.pages().iter().map(|page| page.size),
            old_zoom,
        );
        let old_width = page_canvas_width(viewport_rect.width(), widest_page * old_zoom);
        let anchor = pointer
            .filter(|pointer| viewport_rect.contains(*pointer))
            .unwrap_or_else(|| viewport_rect.center());
        let margin = ui.spacing().scroll.content_margin;
        let content_origin = viewport_rect.min + egui::vec2(margin.leftf(), margin.topf());
        let mut state = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
        let origin_id = scroll_id.with("image-zoom-origin");
        let mut origin = ui.ctx().data_mut(|data| {
            data.get_temp::<crate::preview::ZoomOrigin<Option<ArtifactKey>>>(origin_id)
                .unwrap_or_default()
        });
        let input = crate::preview::ZoomOriginState {
            document: preview.content.raster_key(),
            viewport: viewport_rect,
            pointer: anchor - content_origin,
            pixels_per_point: ui.pixels_per_point(),
            zoom: old_zoom,
            offset: state.offset,
        };
        state.offset = image_zoom_anchored_offset(
            [&old_pages, &geometries],
            [old_width, content_width],
            origin.basis(&input),
            input.pointer,
        );
        origin.record(crate::preview::ZoomOriginState {
            zoom: new_zoom,
            offset: state.offset,
            ..input
        });
        ui.ctx()
            .data_mut(|data| data.insert_temp(origin_id, origin));
        state.store(ui.ctx(), scroll_id);
        preview.zoom = new_zoom;
        preview.fit_width = false;
    }

    let content_height = stack_height(&geometries);
    let requested_page = preview.requested_page.take();

    let output = egui::ScrollArea::both()
        .id_salt("image-preview-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(content_width, content_height));
            let page_theme = theme::preview_palette(preview.render_dark());
            for (page, geometry) in preview.content.pages().iter().zip(&geometries) {
                let left = ((content_width - geometry.size.x) * 0.5).max(PAGE_MARGIN);
                let rect = Rect::from_min_size(
                    Pos2::new(
                        ui.min_rect().left() + left,
                        ui.min_rect().top() + geometry.top,
                    ),
                    geometry.size,
                );
                if !rect.expand(PAGE_GAP).intersects(ui.clip_rect())
                    && requested_page != Some(geometry.index)
                {
                    continue;
                }
                let shadow_rect = rect
                    .translate(Vec2::new(0.0, METRICS.preview.shadow_offset_y))
                    .expand(METRICS.preview.shadow_expand);
                ui.painter().rect_filled(
                    shadow_rect,
                    METRICS.preview.shadow_radius,
                    page_theme.shadow,
                );
                ui.painter()
                    .rect_filled(rect, METRICS.preview.page_radius, page_theme.page_fill);
                ui.painter().rect_stroke(
                    rect,
                    METRICS.preview.page_radius,
                    Stroke::new(METRICS.preview.page_border_width, page_theme.border),
                    StrokeKind::Outside,
                );
                let resident = page
                    .resident
                    .as_ref()
                    .filter(|resident| resident.is_usable());
                if let Some(resident) = resident {
                    ui.put(
                        rect,
                        egui::Image::new(&resident.texture)
                            .fit_to_exact_size(geometry.size)
                            .alt_text("Image"),
                    );
                } else {
                    show_page_status_indicator(
                        ui,
                        rect,
                        geometry.index,
                        preview.status == PreviewStatus::Error,
                    );
                }
                if requested_page == Some(geometry.index) {
                    ui.scroll_to_rect(rect, Some(Align::Min));
                }
            }
        });
    preview.visible_page = visible_page(
        &geometries,
        output.state.offset.y,
        output.inner_rect.height(),
    );
}

/// Anchor image coordinates, keeping canvas margins and page gaps unscaled.
fn image_zoom_anchored_offset(
    pages: [&[crate::preview::PageGeometry]; 2],
    widths: [f32; 2],
    old_offset: Vec2,
    pointer: Vec2,
) -> Vec2 {
    let [old, new] = pages;
    if old.is_empty() || old.len() != new.len() {
        return old_offset;
    }
    let point = old_offset + pointer;
    let index = old
        .partition_point(|page| page.top <= point.y)
        .saturating_sub(1);
    let (old, new) = (old[index], new[index]);
    let local_y = point.y - old.top;
    let y = if point.y < pages[0][0].top {
        point.y
    } else if local_y <= old.size.y {
        new.top + local_y * (new.size.y / old.size.y)
    } else {
        new.bottom() + (local_y - old.size.y)
    };
    let left = |width: f32, page_width: f32| ((width - page_width) * 0.5).max(PAGE_MARGIN);
    let x = left(widths[1], new.size.x)
        + (point.x - left(widths[0], old.size.x)) * (new.size.x / old.size.x);
    (egui::vec2(x, y) - pointer).max(Vec2::ZERO)
}

const PAGE_LOADING_INDICATOR_WIDTH: f32 = 128.0;
const PAGE_LOADING_INDICATOR_HEIGHT: f32 = 48.0;

fn page_loading_indicator_rect(page: Rect) -> Rect {
    Rect::from_center_size(
        page.center(),
        Vec2::new(
            page.width().min(PAGE_LOADING_INDICATOR_WIDTH),
            page.height().min(PAGE_LOADING_INDICATOR_HEIGHT),
        ),
    )
}

fn show_page_status_indicator(ui: &mut egui::Ui, page: Rect, page_index: usize, failed: bool) {
    let indicator = page_loading_indicator_rect(page);
    ui.scope_builder(egui::UiBuilder::new().max_rect(indicator), |ui| {
        ui.with_layout(
            Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.vertical_centered(|ui| {
                    if failed {
                        ui.label(RichText::new("Image unavailable").weak());
                    } else {
                        ui.spinner();
                        ui.label(RichText::new(format!("Loading page {}…", page_index + 1)).weak());
                    }
                });
            },
        );
    });
}

fn page_canvas_width(viewport_width: f32, widest_page: f32) -> f32 {
    viewport_width.max(widest_page + PAGE_MARGIN * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_frame(
        context: &egui::Context,
        preview: &mut PreviewController,
        events: Vec<egui::Event>,
        offset: Option<Vec2>,
        margin: i8,
        pixels_per_point: f32,
    ) -> (Rect, Rect, Vec2) {
        let mut viewport = Rect::NOTHING;
        let mut actual_offset = Vec2::ZERO;
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700.0, 500.0))),
            events,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(pixels_per_point);
        let output = context.run_ui(input, |ui| {
            ui.style_mut().animation_time = 0.0;
            ui.spacing_mut().scroll.content_margin = egui::Margin::same(margin);
            viewport = ui.available_rect_before_wrap();
            let id = ui.make_persistent_id(egui::IdSalt::new("image-preview-scroll"));
            if let Some(offset) = offset {
                let mut state = egui::scroll_area::State::load(context, id).unwrap_or_default();
                state.offset = offset;
                state.store(context, id);
            }
            show_pages(ui, preview);
            actual_offset = egui::scroll_area::State::load(context, id).unwrap().offset;
        });
        let size = page_stack_geometry(
            preview.content.pages().iter().map(|page| page.size),
            preview.zoom,
        )[0]
        .size;
        fn page_rect(shape: &egui::Shape, size: Vec2) -> Option<Rect> {
            match shape {
                egui::Shape::Rect(rect) if (rect.rect.size() - size).length() < 0.001 => {
                    Some(rect.rect)
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| page_rect(shape, size)),
                _ => None,
            }
        }
        let page = output
            .shapes
            .iter()
            .find_map(|shape| page_rect(&shape.shape, size))
            .expect("the real image widget painted its page");
        output.drop_without_applying_deltas();
        (page, viewport, actual_offset)
    }

    #[test]
    fn real_image_scroll_widget_keeps_the_cursor_point_for_pinch_and_toolbar_zoom() {
        for pixels_per_point in [1.0, 1.5, 2.0] {
            for margin in [0, 3] {
                for initial_zoom in [0.4, 1.0] {
                    for pinch in [false, true] {
                        let context = egui::Context::default();
                        let mut preview =
                            PreviewController::new(false, PreviewPreference::default());
                        preview.replace_asset(
                            ArtifactKey::unversioned(0),
                            None,
                            vec![crate::preview::PreviewTexture {
                                size: [4800, 4800],
                                resident: None,
                            }],
                        );
                        preview.fit_width = false;
                        preview.zoom = initial_zoom;
                        image_frame(
                            &context,
                            &mut preview,
                            vec![],
                            Some(egui::vec2(150.3, 100.4)),
                            margin,
                            pixels_per_point,
                        );
                        let (mut before, viewport, mut old_offset) = image_frame(
                            &context,
                            &mut preview,
                            vec![],
                            None,
                            margin,
                            pixels_per_point,
                        );
                        let mut pointer = viewport.center();
                        // Each phase establishes a new painted source point. The second
                        // genuinely scrolls, and the third moves the cursor without scrolling.
                        for phase in 0..3 {
                            if phase == 1 {
                                (before, _, old_offset) = image_frame(
                                    &context,
                                    &mut preview,
                                    vec![],
                                    Some(old_offset + egui::vec2(11.3, 29.7)),
                                    margin,
                                    pixels_per_point,
                                );
                            } else if phase == 2 {
                                pointer += egui::vec2(27.0, -19.0);
                            }
                            let relative = (pointer - before.min) / before.size();
                            assert!(relative.x > 0.0 && relative.x < 1.0);
                            assert!(relative.y > 0.0 && relative.y < 1.0);
                            for step in 0..100 {
                                let factor = if phase == 1 { 1.0 / 1.003 } else { 1.003 };
                                let mut events = vec![egui::Event::PointerMoved(pointer)];
                                if pinch {
                                    events.push(egui::Event::Zoom(factor));
                                } else {
                                    preview.requested_zoom = Some(preview.zoom * factor);
                                }
                                let (after, _, offset) = image_frame(
                                    &context,
                                    &mut preview,
                                    events,
                                    None,
                                    margin,
                                    pixels_per_point,
                                );
                                let error = (after.min + relative * after.size() - pointer).abs();
                                // ScrollArea rounds the painted content origin to a physical
                                // pixel (egui 0.36.2 scroll_area.rs), leaving saved offsets
                                // fractional. Only that final rounding is allowed; errors
                                // must not accumulate across these 100 small zoom events.
                                // At fractional monitor scales, round_ui additionally
                                // quantizes to the pinned 1/32-point GUI grid.
                                let gui_rounding = if pixels_per_point == 1.5 {
                                    1.0 / 64.0
                                } else {
                                    0.0
                                };
                                assert!(
                                    error.max_elem()
                                        <= 0.5 / pixels_per_point + gui_rounding + 0.003,
                                    "ppp={pixels_per_point} margin={margin} initial_zoom={initial_zoom} pinch={pinch} phase={phase} step={step}: error={error:?}; scroll {old_offset:?} -> {offset:?}"
                                );
                                before = after;
                                old_offset = offset;
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn image_zoom_preserves_fixed_page_gaps_and_each_pages_center() {
        let old = page_stack_geometry([[1200, 1600], [600, 800]], 1.0);
        let new = page_stack_geometry([[1200, 1600], [600, 800]], 1.5);
        let pointer = egui::vec2(350.0, 200.0);
        let old_offset = egui::vec2(100.0, old[1].bottom() + 6.0 - pointer.y);
        let offset =
            image_zoom_anchored_offset([&old, &new], [1000.0, 1400.0], old_offset, pointer);
        assert!((new[1].bottom() + 6.0 - offset.y - pointer.y).abs() < 0.001);
        assert_eq!(
            image_zoom_anchored_offset([&[], &[]], [1000.0, 1400.0], old_offset, pointer),
            old_offset
        );
    }

    #[test]
    fn failed_retained_image_without_pixels_reports_unavailable_instead_of_loading() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut preview = PreviewController::new(false, PreviewPreference::default());
        preview.replace_asset(
            ArtifactKey::unversioned(0),
            None,
            vec![crate::preview::PreviewTexture {
                size: [640, 480],
                resident: None,
            }],
        );
        preview.status = PreviewStatus::Error;
        preview.fit_width = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(500.0, 400.0))
            .build_ui_state(show_pages, preview);
        harness.run();
        assert!(harness.query_by_label("Image unavailable").is_some());
        assert!(harness.query_by_label("Loading page 1…").is_none());
    }

    #[test]
    fn dark_image_control_toggles_only_the_shared_preference() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness = Harness::builder()
            .with_size(egui::vec2(500.0, 80.0))
            .build_ui_state(
                |ui, state: &mut (PreviewController, bool)| {
                    ui.horizontal(|ui| {
                        if let Some(next) = show_controls(ui, &mut state.0, state.1) {
                            state.1 = next;
                        }
                    });
                },
                (
                    PreviewController::new(false, PreviewPreference::default()),
                    false,
                ),
            );
        harness.run();
        harness.get_by_label("Dark images").click();
        harness.run();
        assert!(harness.state().1);
        assert_eq!(harness.state().0.zoom, 1.0);
        assert!(
            !harness.state().0.render_dark(),
            "runtime owns applying appearance"
        );
        harness.get_by_label("Dark images").click();
        harness.run();
        assert!(!harness.state().1);
    }

    #[test]
    fn zoom_actions_clamp_and_leave_fit_mode_without_applying_early() {
        let mut preview = PreviewController::new(false, PreviewPreference::default());
        for (zoom, action, expected) in [
            (MAX_PREVIEW_ZOOM, PreviewZoomAction::In, MAX_PREVIEW_ZOOM),
            (MIN_PREVIEW_ZOOM, PreviewZoomAction::Out, MIN_PREVIEW_ZOOM),
            (2.0, PreviewZoomAction::Reset, 1.0),
            (1.0, PreviewZoomAction::In, METRICS.preview.zoom_step),
            (1.0, PreviewZoomAction::Out, 1.0 / METRICS.preview.zoom_step),
        ] {
            preview.zoom = zoom;
            preview.fit_width = true;
            preview.requested_zoom = Some(3.0);
            request_zoom(&mut preview, action);
            assert_eq!(preview.requested_zoom, Some(expected));
            assert!(!preview.fit_width);
            assert_eq!(preview.zoom, zoom, "painting still owns zoom application");
        }
    }

    #[test]
    fn fitted_page_preserves_both_margins_without_horizontal_overflow() {
        for viewport in [240.0, 500.0, 1000.0] {
            let page = viewport - PAGE_MARGIN * 2.0;
            let canvas = page_canvas_width(viewport, page);
            assert_eq!(canvas, viewport);
            assert_eq!((canvas - page) * 0.5, PAGE_MARGIN);
        }
    }

    #[test]
    fn zoomed_pages_can_scroll_with_margins_and_small_pages_stay_centered() {
        assert_eq!(page_canvas_width(400.0, 800.0), 800.0 + PAGE_MARGIN * 2.0);
        assert_eq!(page_canvas_width(800.0, 400.0), 800.0);
    }

    #[test]
    fn page_loading_indicator_stays_centered_and_inside_page() {
        let page = Rect::from_min_size(Pos2::new(32.0, 64.0), Vec2::new(500.0, 700.0));
        let indicator = page_loading_indicator_rect(page);

        assert_eq!(indicator.center(), page.center());
        assert!(page.contains(indicator.min));
        assert!(page.contains(indicator.max));

        let small_page = Rect::from_min_size(Pos2::ZERO, Vec2::new(40.0, 30.0));
        let small_indicator = page_loading_indicator_rect(small_page);
        assert_eq!(small_indicator.size(), small_page.size());
    }
}
