use std::{sync::Arc, time::Duration};

use eframe::egui::{TextureHandle, Vec2};

use crate::{
    compiler::{ArtifactKey, PREVIEW_DPI, PreviewLink},
    diagnostics::Diagnostic,
    settings::PreviewPreference,
    theme::METRICS,
};

pub const PDF_POINTS_PER_PREVIEW_PIXEL: f32 = 72.0 / PREVIEW_DPI;
pub const PAGE_MARGIN: f32 = METRICS.preview.page_margin;
pub const PAGE_GAP: f32 = METRICS.preview.page_gap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewStatus {
    Waiting,
    Compiling,
    Ready(Duration),
    Error,
}

pub(crate) struct PreviewTexture {
    /// Logical layout size. Images may deliberately differ from PDF raster
    /// pixels so one image pixel maps to one UI point at 100%.
    pub(crate) size: [usize; 2],
    pub(crate) raster_size: [usize; 2],
    pub(crate) rgba: Vec<u8>,
    pub(crate) links: Vec<PreviewLink>,
    pub(crate) texture: TextureHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterContentFreshness {
    Current,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceState {
    Disabled(String),
    Starting(String),
    Ready(String),
    Degraded(String),
    Failed(String),
    Unsupported(String),
}

impl ServiceState {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Disabled(_) => "Disabled",
            Self::Starting(_) => "Starting",
            Self::Ready(_) => "Ready",
            Self::Degraded(_) => "Degraded",
            Self::Failed(_) => "Failed",
            Self::Unsupported(_) => "Unsupported",
        }
    }

    pub(crate) fn detail(&self) -> &str {
        match self {
            Self::Disabled(detail)
            | Self::Starting(detail)
            | Self::Ready(detail)
            | Self::Degraded(detail)
            | Self::Failed(detail)
            | Self::Unsupported(detail) => detail,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewBackend {
    Interactive,
    Raster,
}

pub(crate) fn preview_fallback_reason_for(
    preference: PreviewPreference,
    interactive_active: bool,
    preview_url_available: bool,
    tinymist_state: &ServiceState,
    webview_state: &ServiceState,
) -> Option<String> {
    if preference != PreviewPreference::Interactive || interactive_active {
        return None;
    }
    let state = if preview_url_available {
        webview_state
    } else {
        tinymist_state
    };
    Some(format!("{}: {}", state.label(), state.detail()))
}

#[cfg(test)]
pub(crate) fn preview_backend_label_for(
    preference: PreviewPreference,
    interactive_active: bool,
) -> &'static str {
    if interactive_active {
        "Interactive"
    } else if preference == PreviewPreference::Interactive {
        "Rasterised PDF · fallback"
    } else {
        "Rasterised PDF"
    }
}

pub(crate) fn raster_result_matches_artifact(
    raster_key: ArtifactKey,
    document_revision: u64,
    artifact_key: Option<ArtifactKey>,
) -> bool {
    raster_key.revision == document_revision && artifact_key == Some(raster_key)
}

pub(crate) fn raster_content_freshness(
    has_pages: bool,
    raster_key: Option<ArtifactKey>,
    document_revision: u64,
    artifact_key: Option<ArtifactKey>,
) -> Option<RasterContentFreshness> {
    if !has_pages {
        return None;
    }
    Some(
        if raster_key
            .is_some_and(|key| raster_result_matches_artifact(key, document_revision, artifact_key))
        {
            RasterContentFreshness::Current
        } else {
            RasterContentFreshness::Stale
        },
    )
}

/// Owns preview artifacts, generation-aware raster state, diagnostics, and
/// navigation requests. Artifact/raster comparisons always use the complete
/// `ArtifactKey`, so a previous dependency build with the same editor revision
/// cannot become current again.
pub(crate) struct PreviewController {
    pub(crate) requested_backend: PreviewPreference,
    pub(crate) tinymist_preview_enabled: bool,
    pub(crate) tinymist_lsp_ready: bool,
    pub(crate) interactive_url: Option<String>,
    pub(crate) tinymist_state: ServiceState,
    pub(crate) webview_state: ServiceState,
    pub(crate) status: PreviewStatus,
    pub(crate) raw_diagnostics: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) tinymist_diagnostics: Vec<Diagnostic>,
    pub(crate) diagnostics_generation: u64,
    pub(crate) artifact_key: Option<ArtifactKey>,
    pub(crate) pdf: Option<Arc<[u8]>>,
    pub(crate) raster_key: Option<ArtifactKey>,
    pub(crate) raster_error: Option<String>,
    pub(crate) pages: Vec<PreviewTexture>,
    pub(crate) visible_page: usize,
    pub(crate) zoom: f32,
    pub(crate) fit_width: bool,
    pub(crate) requested_zoom: Option<f32>,
    pub(crate) requested_page: Option<usize>,
    pub(crate) dark: bool,
    pub(crate) was_visible: bool,
}

impl PreviewController {
    pub(crate) fn new(dark: bool, requested_backend: PreviewPreference) -> Self {
        Self {
            requested_backend,
            tinymist_preview_enabled: false,
            tinymist_lsp_ready: false,
            interactive_url: None,
            tinymist_state: ServiceState::Starting("Launching Tinymist LSP".to_owned()),
            webview_state: ServiceState::Starting(
                "Waiting for Tinymist's preview server".to_owned(),
            ),
            status: PreviewStatus::Waiting,
            raw_diagnostics: String::new(),
            diagnostics: Vec::new(),
            tinymist_diagnostics: Vec::new(),
            diagnostics_generation: 0,
            artifact_key: None,
            pdf: None,
            raster_key: None,
            raster_error: None,
            pages: Vec::new(),
            visible_page: 0,
            zoom: 1.0,
            fit_width: true,
            requested_zoom: None,
            requested_page: None,
            dark,
            was_visible: false,
        }
    }

    pub(crate) fn mark_diagnostics_changed(&mut self) {
        self.diagnostics_generation = self.diagnostics_generation.wrapping_add(1);
    }

    pub(crate) fn set_diagnostics(&mut self, raw: String, diagnostics: Vec<Diagnostic>) {
        self.raw_diagnostics = raw;
        self.diagnostics = diagnostics;
        self.mark_diagnostics_changed();
    }

    pub(crate) fn accept_artifact(&mut self, key: ArtifactKey, pdf: Arc<[u8]>) {
        self.artifact_key = Some(key);
        self.pdf = Some(pdf);
        self.raster_error = None;
    }

    pub(crate) fn accepts_raster(&self, key: ArtifactKey, document_revision: u64) -> bool {
        raster_result_matches_artifact(key, document_revision, self.artifact_key)
    }

    pub(crate) fn replace_raster(&mut self, key: ArtifactKey, pages: Vec<PreviewTexture>) {
        self.pages = pages;
        self.visible_page = self.visible_page.min(self.pages.len().saturating_sub(1));
        self.raster_key = Some(key);
        self.raster_error = None;
    }

    pub(crate) fn replace_asset(
        &mut self,
        key: ArtifactKey,
        pdf: Option<Arc<[u8]>>,
        pages: Vec<PreviewTexture>,
    ) {
        self.pages = pages;
        self.pdf = pdf;
        self.artifact_key = Some(key);
        self.raster_key = Some(key);
        self.raster_error = None;
        self.visible_page = self.visible_page.min(self.pages.len().saturating_sub(1));
        self.status = PreviewStatus::Ready(Duration::ZERO);
    }

    pub(crate) fn clear_for_document(
        &mut self,
        document_revision: u64,
        preserve_designated_preview: bool,
    ) {
        if preserve_designated_preview {
            // Both halves retain the same generation while being rebound to
            // the newly active child document's revision.
            if let Some(key) = &mut self.artifact_key {
                key.revision = document_revision;
            }
            if let Some(key) = &mut self.raster_key {
                key.revision = document_revision;
            }
            return;
        }

        self.artifact_key = None;
        self.pdf = None;
        self.raster_key = None;
        self.raster_error = None;
        self.pages.clear();
        self.visible_page = 0;
        self.raw_diagnostics.clear();
        self.diagnostics.clear();
        self.tinymist_diagnostics.clear();
        self.mark_diagnostics_changed();
    }

    pub(crate) fn raster_freshness(
        &self,
        document_revision: u64,
    ) -> Option<RasterContentFreshness> {
        raster_content_freshness(
            !self.pages.is_empty(),
            self.raster_key,
            document_revision,
            self.artifact_key,
        )
    }

    pub(crate) fn set_requested_backend(&mut self, preference: PreviewPreference) {
        self.requested_backend = preference;
    }

    pub(crate) fn interactive_requested(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        typst_preview_available
            && self.requested_backend == PreviewPreference::Interactive
            && platform_supported
    }

    pub(crate) fn session_requested(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
    }

    pub(crate) fn interactive_active(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
            && self.interactive_url.is_some()
            && self.webview_state.is_ready()
    }

    pub(crate) fn should_attempt_interactive(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
            && self.interactive_url.is_some()
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
    }

    pub(crate) fn interactive_transitioning(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
            && matches!(
                self.tinymist_state,
                ServiceState::Starting(_) | ServiceState::Ready(_)
            )
            && !matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            )
    }

    pub(crate) fn effective_backend(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> PreviewBackend {
        if self.interactive_active(typst_preview_available, platform_supported) {
            PreviewBackend::Interactive
        } else {
            PreviewBackend::Raster
        }
    }

    pub(crate) fn fallback_reason(
        &self,
        typst_preview_available: bool,
        preview_visible: bool,
        platform_supported: bool,
    ) -> Option<String> {
        if !typst_preview_available || !preview_visible {
            return None;
        }
        preview_fallback_reason_for(
            self.requested_backend,
            self.interactive_active(typst_preview_available, platform_supported),
            self.interactive_url.is_some(),
            &self.tinymist_state,
            &self.webview_state,
        )
    }

    pub(crate) fn backend_label(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> &'static str {
        match self.effective_backend(typst_preview_available, platform_supported) {
            PreviewBackend::Interactive => "Interactive",
            PreviewBackend::Raster if self.requested_backend == PreviewPreference::Interactive => {
                "Rasterised PDF · fallback"
            }
            PreviewBackend::Raster => "Rasterised PDF",
        }
    }

    pub(crate) fn raster_required(
        &self,
        interactive_requested: bool,
        screenshot_pending: bool,
    ) -> bool {
        let interactive_unavailable = (self.interactive_url.is_none()
            && matches!(
                self.tinymist_state,
                ServiceState::Disabled(_)
                    | ServiceState::Degraded(_)
                    | ServiceState::Failed(_)
                    | ServiceState::Unsupported(_)
            ))
            || matches!(
                self.webview_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            );
        screenshot_pending || !interactive_requested || interactive_unavailable
    }
}

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

    fn key(revision: u64, generation: u64) -> ArtifactKey {
        ArtifactKey {
            revision,
            generation,
        }
    }

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

    #[test]
    fn same_revision_raster_from_an_older_artifact_is_stale() {
        let mut preview = PreviewController::new(false, PreviewPreference::Native);
        preview.artifact_key = Some(key(9, 4));
        preview.raster_key = Some(key(9, 3));
        assert!(!preview.accepts_raster(key(9, 3), 9));
    }

    #[test]
    fn rebinding_a_designated_preview_does_not_promote_an_old_generation() {
        let mut preview = PreviewController::new(false, PreviewPreference::Native);
        preview.artifact_key = Some(key(4, 12));
        preview.raster_key = Some(key(4, 11));

        preview.clear_for_document(5, true);

        assert_eq!(preview.artifact_key, Some(key(5, 12)));
        assert_eq!(preview.raster_key, Some(key(5, 11)));
        assert!(!preview.accepts_raster(key(5, 11), 5));
    }

    #[test]
    fn requested_and_effective_backends_expose_fallback_state() {
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_state = ServiceState::Failed("server stopped".to_owned());
        assert_eq!(
            preview.effective_backend(true, true),
            PreviewBackend::Raster
        );
        assert_eq!(
            preview.backend_label(true, true),
            "Rasterised PDF · fallback"
        );
        assert!(
            preview
                .fallback_reason(true, true, true)
                .is_some_and(|reason| reason.contains("server stopped"))
        );

        preview.interactive_url = Some("http://127.0.0.1:23625".to_owned());
        preview.webview_state = ServiceState::Ready("loaded".to_owned());
        assert_eq!(
            preview.effective_backend(true, true),
            PreviewBackend::Interactive
        );
        assert!(preview.fallback_reason(true, true, true).is_none());
    }
}
