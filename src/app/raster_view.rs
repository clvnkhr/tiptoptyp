//! Shared raster controls and painting; each pane supplies independent state.
use super::*;

pub(super) fn show_controls(
    ui: &mut egui::Ui,
    preview: &mut PreviewController,
    raster_is_current: bool,
    hide_pages: bool,
) {
    let header_width = ui.available_width();
    {
        let page_count = if hide_pages || !raster_is_current {
            0
        } else {
            preview.content.pages().len()
        };
        if header_width >= METRICS.preview.header_pages_min_width {
            if icon_button_enabled(
                ui,
                page_count > 0 && preview.visible_page > 0,
                UiIcon::Previous,
                "Previous page",
            )
            .clicked()
            {
                preview.requested_page = Some(preview.visible_page - 1);
            }
            ui.label(if page_count == 0 {
                "–/–".to_owned()
            } else {
                format!("{}/{page_count}", preview.visible_page + 1)
            });
            if icon_button_enabled(
                ui,
                preview.visible_page + 1 < page_count,
                UiIcon::Next,
                "Next page",
            )
            .clicked()
            {
                preview.requested_page = Some(preview.visible_page + 1);
            }
        }
        if header_width >= METRICS.preview.header_zoom_min_width {
            ui.separator();
            if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                preview.requested_zoom = Some(
                    (preview.zoom / METRICS.preview.zoom_step)
                        .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                );
                preview.fit_width = false;
            }
            if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                preview.requested_zoom = Some(
                    (preview.zoom * METRICS.preview.zoom_step)
                        .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM),
                );
                preview.fit_width = false;
            }
            if header_width >= METRICS.preview.header_percent_min_width {
                ui.label(format!("{:.0}%", preview.zoom * 100.0));
            }
            if icon_button(
                ui,
                UiIcon::FitWidth,
                if preview.fit_width {
                    "Fit page width (on)"
                } else {
                    "Fit page width"
                },
            )
            .clicked()
            {
                preview.fit_width = !preview.fit_width;
            }
        }
    }
}

pub(super) fn show_pages(
    ui: &mut egui::Ui,
    preview: &mut PreviewController,
    raster_is_current: bool,
    hide_pages: bool,
) -> Option<String> {
    let viewport_rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(viewport_rect, 0.0, preview_background(ui));
    if preview.content.pages().is_empty() || hide_pages {
        let failed = preview.status == PreviewStatus::Error;
        show_centered_preview_message(
            ui,
            if failed {
                "Fix the diagnostics to render a preview"
            } else {
                "Building preview…"
            },
            !failed,
        );
        return None;
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

    let scroll_id = ui.make_persistent_id("pdf-preview-scroll");
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
    if (new_zoom - old_zoom).abs() > f32::EPSILON {
        let anchor = pointer
            .filter(|pointer| viewport_rect.contains(*pointer))
            .unwrap_or_else(|| viewport_rect.center());
        let mut state = egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
        state.offset =
            zoom_anchored_offset(state.offset, anchor - viewport_rect.min, old_zoom, new_zoom);
        state.store(ui.ctx(), scroll_id);
        preview.zoom = new_zoom;
        preview.fit_width = false;
    }

    let geometries = page_stack_geometry(
        preview.content.pages().iter().map(|page| page.size),
        preview.zoom,
    );
    let content_width = page_canvas_width(
        viewport_rect.width(),
        geometries
            .iter()
            .map(|page| page.size.x)
            .fold(0.0, f32::max),
    );
    let content_height = stack_height(&geometries);
    let requested_page = preview.requested_page.take();
    let mut clicked_link = None;

    let output = egui::ScrollArea::both()
        .id_salt("pdf-preview-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_size(Vec2::new(content_width, content_height));
            let page_theme = theme::preview_palette(preview.dark);
            for (page, geometry) in preview.content.pages().iter().zip(&geometries) {
                let left = ((content_width - geometry.size.x) * 0.5).max(PAGE_MARGIN);
                let rect = Rect::from_min_size(
                    Pos2::new(
                        ui.min_rect().left() + left,
                        ui.min_rect().top() + geometry.top,
                    ),
                    geometry.size,
                );
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
                ui.put(
                    rect,
                    egui::Image::new(&page.texture)
                        .fit_to_exact_size(geometry.size)
                        .alt_text(format!("PDF page {}", geometry.index + 1)),
                );
                for (link_index, link) in page.links.iter().enumerate() {
                    if !raster_is_current {
                        continue;
                    }
                    let [left, top, right, bottom] = link.rect;
                    let link_rect = Rect::from_min_max(
                        Pos2::new(
                            rect.left() + left * rect.width(),
                            rect.top() + top * rect.height(),
                        ),
                        Pos2::new(
                            rect.left() + right * rect.width(),
                            rect.top() + bottom * rect.height(),
                        ),
                    )
                    .expand(theme::SPACE.tight)
                    .intersect(rect);
                    if !link_rect.is_positive() {
                        continue;
                    }
                    let response = ui.interact(
                        link_rect,
                        ui.id().with(("pdf-link", geometry.index, link_index)),
                        Sense::click(),
                    );
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        clicked_link = Some(link.target.clone());
                    }
                    native_hover_text(response, &link.target);
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
    clicked_link
}

fn page_canvas_width(viewport_width: f32, widest_page: f32) -> f32 {
    viewport_width.max(widest_page + PAGE_MARGIN * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
