use eframe::egui::Vec2;

use crate::{compiler::PREVIEW_DPI, theme::METRICS};

pub const PDF_POINTS_PER_PREVIEW_PIXEL: f32 = 72.0 / PREVIEW_DPI;
pub const PAGE_MARGIN: f32 = METRICS.preview.page_margin;
pub const PAGE_GAP: f32 = METRICS.preview.page_gap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    pub index: usize,
    pub top: f32,
    pub size: Vec2,
}

impl PageGeometry {
    pub fn bottom(self) -> f32 {
        self.top + self.size.y
    }
}

/// Computes a stable, continuous vertical stack for the native preview.
///
/// Pages are centred independently so mixed page sizes work as expected. The
/// `top` values never depend on build status or diagnostics, which lets the UI
/// preserve its scroll offset when a new successful build replaces the pages.
pub fn page_stack_geometry(
    raster_sizes: impl IntoIterator<Item = [usize; 2]>,
    zoom: f32,
) -> Vec<PageGeometry> {
    let scale = zoom * PDF_POINTS_PER_PREVIEW_PIXEL;
    let mut top = PAGE_MARGIN;
    raster_sizes
        .into_iter()
        .enumerate()
        .map(|(index, [width, height])| {
            let size = Vec2::new(width as f32 * scale, height as f32 * scale);
            let geometry = PageGeometry { index, top, size };
            top += size.y + PAGE_GAP;
            geometry
        })
        .collect()
}

pub fn stack_height(pages: &[PageGeometry]) -> f32 {
    pages
        .last()
        .map_or(PAGE_MARGIN * 2.0, |page| page.bottom() + PAGE_MARGIN)
}

pub fn visible_page(pages: &[PageGeometry], scroll_y: f32, viewport_height: f32) -> usize {
    if pages.is_empty() {
        return 0;
    }
    let viewport_center = scroll_y + viewport_height * 0.5;
    pages
        .iter()
        .min_by(|left, right| {
            let left_distance = (left.top + left.size.y * 0.5 - viewport_center).abs();
            let right_distance = (right.top + right.size.y * 0.5 - viewport_center).abs();
            left_distance.total_cmp(&right_distance)
        })
        .map_or(0, |page| page.index)
}

/// Keeps the content point under the pointer fixed while the scale changes.
pub fn zoom_anchored_offset(
    old_offset: Vec2,
    pointer_in_viewport: Vec2,
    old_zoom: f32,
    new_zoom: f32,
) -> Vec2 {
    if old_zoom <= 0.0 || !old_zoom.is_finite() || !new_zoom.is_finite() {
        return old_offset;
    }
    let ratio = new_zoom / old_zoom;
    ((old_offset + pointer_in_viewport) * ratio - pointer_in_viewport).max(Vec2::ZERO)
}

/// A preview-only dark transform. The original pixels and exported PDF remain
/// untouched, so switching mode is lossless.
pub fn dark_preview_rgba(rgba: &[u8]) -> Vec<u8> {
    let [red_percent, green_percent, blue_percent] = METRICS.preview.dark_transform_rgb_percent;
    rgba.chunks_exact(4)
        .flat_map(|pixel| {
            // A slightly blue-black inversion is more comfortable than a raw
            // photographic negative for predominantly black-on-white pages.
            let red = (255_u16.saturating_sub(pixel[0] as u16) * red_percent / 100) as u8;
            let green = (255_u16.saturating_sub(pixel[1] as u16) * green_percent / 100) as u8;
            let blue = (255_u16.saturating_sub(pixel[2] as u16) * blue_percent / 100) as u8;
            [red, green, blue, pixel[3]]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_form_a_continuous_monotonic_stack() {
        let pages = page_stack_geometry([[100, 200], [120, 250], [90, 180]], 1.0);
        assert_eq!(pages.len(), 3);
        for pair in pages.windows(2) {
            assert_eq!(pair[1].top, pair[0].bottom() + PAGE_GAP);
            assert!(pair[1].top > pair[0].top);
        }
        assert_eq!(stack_height(&pages), pages[2].bottom() + PAGE_MARGIN);
    }

    #[test]
    fn visible_page_uses_viewport_centre() {
        let pages = page_stack_geometry([[100, 100], [100, 100], [100, 100]], 1.0);
        assert_eq!(visible_page(&pages, 0.0, 60.0), 0);
        assert_eq!(visible_page(&pages, pages[1].top, 60.0), 1);
        assert_eq!(visible_page(&pages, pages[2].top, 60.0), 2);
    }

    #[test]
    fn anchored_zoom_preserves_pointer_content_point() {
        let pointer = Vec2::new(75.0, 120.0);
        let old_offset = Vec2::new(30.0, 500.0);
        let new_offset = zoom_anchored_offset(old_offset, pointer, 1.0, 2.0);
        let before = (old_offset + pointer) / 1.0;
        let after = (new_offset + pointer) / 2.0;
        assert!((before - after).length() < 0.001);
    }

    #[test]
    fn dark_transform_preserves_alpha_and_is_reversible_from_original() {
        let source = [255, 255, 255, 17, 0, 0, 0, 255];
        let dark = dark_preview_rgba(&source);
        assert_eq!(dark[3], 17);
        assert_eq!(dark[7], 255);
        assert!(dark[0] < 16);
        assert!(dark[4] > 220);
        assert_eq!(source, [255, 255, 255, 17, 0, 0, 0, 255]);
    }
}
