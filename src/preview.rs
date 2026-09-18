use std::{
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};

use eframe::egui::{TextureHandle, Vec2};

use crate::{
    compiler::ArtifactKey,
    diagnostics::Diagnostic,
    pdf::{PREVIEW_DPI, PdfDocumentCatalog, PdfPageMetadata, PreviewLink},
    pdf_pages::{RasterPageKey, RasterPageRequestKey},
    pdf_residency::ResidencyLease,
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

pub(crate) struct ResidentPreviewTexture {
    pub(crate) key: RasterPageKey,
    pub(crate) raster_size: [usize; 2],
    pub(crate) rgba: Arc<[u8]>,
    pub(crate) texture: TextureHandle,
    pub(crate) lease: ResidencyLease,
}

impl ResidentPreviewTexture {
    pub(crate) fn is_usable(&self) -> bool {
        self.lease.is_resident()
            && self
                .raster_size
                .iter()
                .copied()
                .try_fold(4_usize, usize::checked_mul)
                == Some(self.rgba.len())
    }
}

pub(crate) struct PreviewTexture {
    /// Logical layout size. Images may deliberately differ from PDF raster
    /// pixels so one image pixel maps to one UI point at 100%.
    pub(crate) size: [usize; 2],
    pub(crate) links: Vec<PreviewLink>,
    pub(crate) resident: Option<ResidentPreviewTexture>,
}

impl PreviewTexture {
    fn from_metadata(page: PdfPageMetadata) -> Self {
        Self {
            size: page.size,
            links: page.links,
            resident: None,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewStatusSnapshot<'a> {
    pub(crate) requested_backend: PreviewPreference,
    pub(crate) effective_backend: PreviewBackend,
    pub(crate) interactive_requested: bool,
    pub(crate) should_attempt_native: bool,
    pub(crate) native_ready: bool,
    pub(crate) canonical_artifact_available: bool,
    pub(crate) interactive_transitioning: bool,
    fallback_state: Option<&'a ServiceState>,
}

impl PreviewStatusSnapshot<'_> {
    pub(crate) fn backend_label(&self) -> &'static str {
        match self.effective_backend {
            PreviewBackend::Interactive => "Interactive",
            PreviewBackend::Raster if self.requested_backend == PreviewPreference::Interactive => {
                "Rasterised PDF · fallback"
            }
            PreviewBackend::Raster => "Rasterised PDF",
        }
    }

    pub(crate) fn fallback_reason(&self) -> Option<String> {
        self.fallback_state
            .map(|state| format!("{}: {}", state.label(), state.detail()))
    }
}

#[derive(Debug)]
pub(crate) enum PreviewTransitionEvent<'a> {
    Failure(&'a crate::tinymist::TinymistEvent, Instant),
    RecoveryTick(Instant),
    Restart {
        preserve_surface: bool,
    },
    PreviewEntryChanged,
    Stop,
    PauseChanged {
        generation: Option<crate::tinymist::Generation>,
        paused: bool,
    },
    RenderRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreviewEffect {
    StopAttempt(crate::tinymist::Generation),
    StopService,
    RestartService {
        preserve_surface: bool,
    },
    SetRefresh {
        generation: crate::tinymist::Generation,
        refresh: crate::tinymist::PreviewRefresh,
    },
    ScheduleRaster,
    RepaintAfter(Duration),
    DiscardLanguageRequests,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewTransition {
    pub(crate) handled: bool,
    pub(crate) effects: Vec<PreviewEffect>,
}

#[cfg(test)]
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
    pub(crate) recovery:
        tiptoptyp_core::recovery::Recovery<crate::tinymist::Generation, std::time::Instant>,
    pub(crate) requested_backend: PreviewPreference,
    pub(crate) tinymist_preview_enabled: bool,
    pub(crate) connection:
        tiptoptyp_core::connection::Connection<crate::tinymist::Generation, url::Url>,
    pub(crate) tinymist_state: ServiceState,
    pub(crate) webview_state: ServiceState,
    pub(crate) status: PreviewStatus,
    compile_started: Option<(crate::tinymist::Generation, String, std::time::Instant)>,
    pub(crate) raw_diagnostics: String,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) tinymist_diagnostics: Vec<Diagnostic>,
    pub(crate) diagnostics_generation: u64,
    pub(crate) content: tiptoptyp_core::preview::PreviewContent<PreviewTexture>,
    pub(crate) visible_page: usize,
    pub(crate) zoom: f32,
    pub(crate) fit_width: bool,
    pub(crate) requested_zoom: Option<f32>,
    pub(crate) requested_page: Option<usize>,
    pub(crate) dark: bool,
    was_visible: bool,
    pending_catalog: Option<(ArtifactKey, PdfDocumentCatalog)>,
    page_demand: Option<RangeInclusive<usize>>,
    last_page_request: Option<RasterPageRequestKey>,
    appearance_revision: u64,
}

impl PreviewController {
    /// Readiness is admitted by both the attempt and connection owners before
    /// it may reset recovery or change user-visible status.
    pub(crate) fn initialized(&mut self, generation: crate::tinymist::Generation) -> bool {
        if !self.recovery.accepts(generation) || !self.connection.initialized(generation) {
            return false;
        }
        self.tinymist_state = if self.tinymist_preview_enabled {
            ServiceState::Starting("Starting Tinymist preview server".to_owned())
        } else {
            self.recovery.recovered(generation);
            ServiceState::Ready("Tinymist LSP is ready".to_owned())
        };
        true
    }

    pub(crate) fn preview_ready(
        &mut self,
        generation: crate::tinymist::Generation,
        url: &str,
        reusing_webview: bool,
    ) -> Result<bool, &'static str> {
        if !self.tinymist_preview_enabled
            || !self.recovery.accepts(generation)
            || !self.connection.is_ready_for(generation)
        {
            return Ok(false);
        }
        let endpoint = url::Url::parse(url).map_err(|_| "Invalid preview endpoint")?;
        if !self.connection.connect(generation, endpoint) {
            return Ok(false);
        }
        self.recovery.recovered(generation);
        self.tinymist_state = ServiceState::Ready("LSP and preview server are ready".to_owned());
        if matches!(self.status, PreviewStatus::Waiting) {
            self.status = PreviewStatus::Ready(Duration::ZERO);
        }
        self.webview_state = ServiceState::Starting(if reusing_webview {
            "Loading the pinned entry in the existing preview".to_owned()
        } else {
            "Embedding the vector preview".to_owned()
        });
        Ok(true)
    }

    pub(crate) fn visibility_changed(
        &mut self,
        visible: bool,
        requested: bool,
    ) -> PreviewTransition {
        let became_visible = !std::mem::replace(&mut self.was_visible, visible) && visible;
        let mut transition = if self.tinymist_preview_enabled != requested {
            self.transition(PreviewTransitionEvent::Restart {
                preserve_surface: false,
            })
        } else {
            PreviewTransition {
                handled: true,
                effects: Vec::new(),
            }
        };
        if became_visible {
            transition.effects.push(PreviewEffect::ScheduleRaster);
        }
        transition
    }

    pub(crate) fn suspend_document(&mut self, reason: &str) {
        self.tinymist_preview_enabled = false;
        self.was_visible = false;
        self.recovery.reset();
        self.connection.suspend(false);
        self.tinymist_state = ServiceState::Disabled(reason.to_owned());
        self.webview_state = ServiceState::Disabled(reason.to_owned());
    }

    pub(crate) fn transition(&mut self, event: PreviewTransitionEvent<'_>) -> PreviewTransition {
        use tiptoptyp_core::recovery::Failure;
        let effects = match event {
            PreviewTransitionEvent::Failure(event, now) => {
                let Some(outcome) = self.receive_tinymist_failure(event, now) else {
                    return PreviewTransition {
                        handled: false,
                        effects: Vec::new(),
                    };
                };
                match outcome {
                    Failure::Ignored => Vec::new(),
                    Failure::Waiting { deadline, .. } => vec![
                        PreviewEffect::StopAttempt(event.generation()),
                        PreviewEffect::DiscardLanguageRequests,
                        PreviewEffect::RepaintAfter(deadline.saturating_duration_since(now)),
                    ],
                    Failure::Exhausted => vec![
                        PreviewEffect::StopAttempt(event.generation()),
                        PreviewEffect::DiscardLanguageRequests,
                        PreviewEffect::ScheduleRaster,
                    ],
                }
            }
            PreviewTransitionEvent::RecoveryTick(now) => {
                if self.recovery.take_retry(now) {
                    vec![PreviewEffect::RestartService {
                        preserve_surface: true,
                    }]
                } else if let Some(deadline) = self.recovery.deadline() {
                    vec![PreviewEffect::RepaintAfter(
                        deadline.saturating_duration_since(now),
                    )]
                } else {
                    Vec::new()
                }
            }
            PreviewTransitionEvent::Restart { preserve_surface } => {
                self.recovery.reset();
                vec![PreviewEffect::RestartService { preserve_surface }]
            }
            PreviewTransitionEvent::PreviewEntryChanged => {
                self.recovery.reset();
                vec![PreviewEffect::RestartService {
                    preserve_surface: true,
                }]
            }
            PreviewTransitionEvent::Stop => vec![PreviewEffect::StopService],
            PreviewTransitionEvent::PauseChanged { generation, paused } => generation
                .map(|generation| PreviewEffect::SetRefresh {
                    generation,
                    refresh: if paused {
                        crate::tinymist::PreviewRefresh::OnSave
                    } else {
                        crate::tinymist::PreviewRefresh::OnType
                    },
                })
                .into_iter()
                .collect(),
            PreviewTransitionEvent::RenderRequested => vec![PreviewEffect::ScheduleRaster],
        };
        PreviewTransition {
            handled: true,
            effects,
        }
    }

    /// Measure server notification intervals, not UI queue delay. Repeated
    /// word-count/status reports must not restart or erase a completed timing.
    pub(crate) fn compile_status(
        &mut self,
        generation: crate::tinymist::Generation,
        path: String,
        status: crate::tinymist::CompileStatus,
        received: std::time::Instant,
    ) {
        use crate::tinymist::CompileStatus;
        match status {
            CompileStatus::Compiling => {
                if !self
                    .compile_started
                    .as_ref()
                    .is_some_and(|(g, p, _)| *g == generation && *p == path)
                {
                    self.compile_started = Some((generation, path, received));
                }
                self.status = PreviewStatus::Compiling;
            }
            CompileStatus::CompileSuccess | CompileStatus::CompileError => {
                let elapsed = self
                    .compile_started
                    .take()
                    .filter(|(g, p, _)| *g == generation && *p == path)
                    .map(|(_, _, start)| received.saturating_duration_since(start));
                if status == CompileStatus::CompileError {
                    self.status = PreviewStatus::Error;
                } else if let Some(elapsed) = elapsed {
                    self.status = PreviewStatus::Ready(elapsed);
                }
            }
        }
    }

    /// Translate protocol failures into one recovery transition. Formatting,
    /// navigation, and hover errors do not restart an otherwise healthy server.
    pub(crate) fn receive_tinymist_failure(
        &mut self,
        event: &crate::tinymist::TinymistEvent,
        now: std::time::Instant,
    ) -> Option<tiptoptyp_core::recovery::Failure<std::time::Instant>> {
        use crate::tinymist::TinymistEvent;
        use tiptoptyp_core::recovery::Failure;
        let detail = match event {
            TinymistEvent::Error {
                stage,
                message,
                fatal,
                ..
            } if *fatal || *stage == "preview" => format!("{stage}: {message}"),
            TinymistEvent::Stopped { reason, .. } => reason.clone(),
            _ => return None,
        };
        let outcome = self.recovery.failed(event.generation(), now);
        match outcome {
            Failure::Ignored => {}
            Failure::Waiting { failures, .. } => {
                self.connection.suspend(true);
                self.tinymist_state = ServiceState::Starting(format!(
                    "Attempt {failures}/5 failed: {detail}. Retrying in 1 second"
                ));
                if self.tinymist_preview_enabled
                    && !(self.connection.endpoint().is_some() && self.webview_state.is_ready())
                {
                    self.webview_state =
                        ServiceState::Starting("Waiting to restart Tinymist".to_owned());
                }
            }
            Failure::Exhausted => {
                self.connection.stop();
                self.tinymist_state =
                    ServiceState::Failed(format!("Five consecutive failures: {detail}"));
                self.webview_state =
                    ServiceState::Failed("Tinymist retry limit reached".to_owned());
            }
        }
        Some(outcome)
    }

    pub(crate) fn new(dark: bool, requested_backend: PreviewPreference) -> Self {
        Self {
            recovery: Default::default(),
            requested_backend,
            tinymist_preview_enabled: false,
            connection: Default::default(),
            tinymist_state: ServiceState::Starting("Launching Tinymist LSP".to_owned()),
            webview_state: ServiceState::Starting(
                "Waiting for Tinymist's preview server".to_owned(),
            ),
            status: PreviewStatus::Waiting,
            compile_started: None,
            raw_diagnostics: String::new(),
            diagnostics: Vec::new(),
            tinymist_diagnostics: Vec::new(),
            diagnostics_generation: 0,
            content: Default::default(),
            visible_page: 0,
            zoom: 1.0,
            fit_width: true,
            requested_zoom: None,
            requested_page: None,
            dark,
            was_visible: false,
            pending_catalog: None,
            page_demand: None,
            last_page_request: None,
            appearance_revision: 1,
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
        self.content.accept_artifact(key, pdf);
        self.pending_catalog = None;
        self.last_page_request = None;
    }

    pub(crate) fn accepts_raster(&self, key: ArtifactKey, document_revision: u64) -> bool {
        raster_result_matches_artifact(key, document_revision, self.content.artifact_key())
    }

    pub(crate) fn accept_catalog(&mut self, key: ArtifactKey, catalog: PdfDocumentCatalog) -> bool {
        if self.content.artifact_key() != Some(key) || catalog.pages.is_empty() {
            return false;
        }
        let page_count = catalog.pages.len();
        self.visible_page = self.visible_page.min(page_count.saturating_sub(1));
        self.page_demand = Some(self.visible_page..=self.visible_page);
        self.last_page_request = None;
        if self.content.pages().is_empty() {
            self.content.accept_raster(
                key,
                catalog
                    .pages
                    .into_iter()
                    .map(PreviewTexture::from_metadata)
                    .collect(),
            );
        } else {
            self.pending_catalog = Some((key, catalog));
        }
        true
    }

    pub(crate) fn replace_asset(
        &mut self,
        key: ArtifactKey,
        pdf: Option<Arc<[u8]>>,
        pages: Vec<PreviewTexture>,
    ) {
        self.content.replace_asset(key, pdf, pages);
        self.visible_page = self
            .visible_page
            .min(self.content.pages().len().saturating_sub(1));
        self.status = PreviewStatus::Ready(Duration::ZERO);
        self.page_demand = (!self.content.pages().is_empty()).then_some(0..=0);
        self.last_page_request = None;
    }

    pub(crate) fn replace_pdf_asset(
        &mut self,
        key: ArtifactKey,
        pdf: Arc<[u8]>,
        catalog: PdfDocumentCatalog,
    ) {
        let pages = catalog
            .pages
            .into_iter()
            .map(PreviewTexture::from_metadata)
            .collect::<Vec<_>>();
        self.content.replace_asset(key, Some(pdf), pages);
        self.visible_page = self
            .visible_page
            .min(self.content.pages().len().saturating_sub(1));
        self.page_demand = Some(self.visible_page..=self.visible_page);
        self.last_page_request = None;
        self.pending_catalog = None;
        self.status = PreviewStatus::Ready(Duration::ZERO);
    }

    pub(crate) fn set_page_demand(&mut self, demand: RangeInclusive<usize>) {
        if self.page_demand.as_ref() != Some(&demand) {
            self.page_demand = Some(demand);
            self.last_page_request = None;
        }
    }

    pub(crate) fn page_is_demanded(&self, page: usize) -> bool {
        self.page_demand
            .as_ref()
            .is_some_and(|demand| demand.contains(&page))
    }

    pub(crate) fn raster_request_key(&self) -> Option<RasterPageRequestKey> {
        self.content.pdf()?;
        let artifact = self.content.artifact_key()?;
        let page_count = self
            .pending_catalog
            .as_ref()
            .filter(|(key, _)| *key == artifact)
            .map(|(_, catalog)| catalog.pages.len())
            .unwrap_or_else(|| self.content.pages().len());
        let demand = self.page_demand.clone()?;
        let range = crate::pdf_pages::bounded_prefetch_range(demand, page_count)?;
        let dpi = (PREVIEW_DPI * self.zoom.clamp(1.0, 2.0)).round() as u32;
        let key = RasterPageRequestKey {
            artifact,
            first: *range.start(),
            last: *range.end(),
            dpi,
            appearance_revision: self.appearance_revision,
        };
        if self.last_page_request == Some(key) || self.range_is_resident(key) {
            None
        } else {
            Some(key)
        }
    }

    fn range_is_resident(&self, key: RasterPageRequestKey) -> bool {
        self.content.raster_key() == Some(key.artifact)
            && (key.first..=key.last).all(|page| {
                self.content
                    .pages()
                    .get(page)
                    .and_then(|page| page.resident.as_ref())
                    .is_some_and(|resident| {
                        resident.key == key.page_key(page) && resident.lease.is_resident()
                    })
            })
    }

    pub(crate) fn record_page_request(&mut self, key: RasterPageRequestKey) {
        self.last_page_request = Some(key);
    }

    pub(crate) fn accepts_page_key(&self, key: RasterPageRequestKey) -> bool {
        self.content.artifact_key() == Some(key.artifact)
            && key.appearance_revision == self.appearance_revision
    }

    pub(crate) fn accept_page_residents(
        &mut self,
        key: RasterPageRequestKey,
        pages: Vec<(usize, ResidentPreviewTexture)>,
    ) -> bool {
        if self.content.artifact_key() != Some(key.artifact)
            || key.appearance_revision != self.appearance_revision
        {
            return false;
        }
        if self
            .pending_catalog
            .as_ref()
            .is_some_and(|(pending, _)| *pending == key.artifact)
        {
            let (_, catalog) = self.pending_catalog.take().unwrap();
            if !self.content.accept_raster(
                key.artifact,
                catalog
                    .pages
                    .into_iter()
                    .map(PreviewTexture::from_metadata)
                    .collect(),
            ) {
                return false;
            }
        }
        if self.content.raster_key() != Some(key.artifact) {
            return false;
        }
        for (index, resident) in pages {
            if resident.key == key.page_key(index)
                && let Some(page) = self.content.pages_mut().get_mut(index)
            {
                page.resident = Some(resident);
            }
        }
        true
    }

    pub(crate) fn bump_appearance(&mut self, dark: bool) {
        self.dark = dark;
        self.appearance_revision = self.appearance_revision.wrapping_add(1).max(1);
        self.last_page_request = None;
        if !self.content.pages().is_empty() {
            self.page_demand = Some(self.visible_page..=self.visible_page);
        }
    }

    pub(crate) fn prune_evicted_pages(&mut self) {
        let mut demand_evicted = false;
        for (index, page) in self.content.pages_mut().iter_mut().enumerate() {
            if page
                .resident
                .as_ref()
                .is_some_and(|resident| !resident.lease.is_resident())
            {
                page.resident = None;
                demand_evicted |= self
                    .page_demand
                    .as_ref()
                    .is_some_and(|range| range.contains(&index));
            }
        }
        if demand_evicted {
            self.last_page_request = None;
        }
    }

    pub(crate) fn visible_residency_ids(&self) -> Vec<u64> {
        let Some(range) = &self.page_demand else {
            return Vec::new();
        };
        range
            .clone()
            .filter_map(|index| {
                self.content
                    .pages()
                    .get(index)
                    .and_then(|page| page.resident.as_ref())
                    .filter(|resident| resident.lease.is_resident())
                    .map(|resident| resident.lease.id())
            })
            .collect()
    }

    pub(crate) fn has_resident_pages(&self) -> bool {
        self.content.pages().iter().any(|page| {
            page.resident
                .as_ref()
                .is_some_and(ResidentPreviewTexture::is_usable)
        })
    }

    pub(crate) fn clear_for_document(
        &mut self,
        document_revision: u64,
        preserve_designated_preview: bool,
    ) {
        if preserve_designated_preview {
            // Both halves retain the same generation while being rebound to
            // the newly active child document's revision.
            self.content.rebind_revision(document_revision);
            return;
        }

        self.content.clear();
        self.pending_catalog = None;
        self.page_demand = None;
        self.last_page_request = None;
        self.compile_started = None;
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
            !self.content.pages().is_empty(),
            self.content.raster_key(),
            document_revision,
            self.content.artifact_key(),
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

    pub(crate) fn interactive_active(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
            && self.connection.endpoint().is_some()
            && self.webview_state.is_ready()
    }

    pub(crate) fn should_attempt_interactive(
        &self,
        typst_preview_available: bool,
        platform_supported: bool,
    ) -> bool {
        self.interactive_requested(typst_preview_available, platform_supported)
            && self.connection.endpoint().is_some()
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

    pub(crate) fn status_snapshot(
        &self,
        typst_preview_available: bool,
        preview_visible: bool,
        platform_supported: bool,
        document_revision: u64,
    ) -> PreviewStatusSnapshot<'_> {
        let native_ready = self.interactive_active(typst_preview_available, platform_supported);
        let interactive_requested =
            self.interactive_requested(typst_preview_available, platform_supported);
        PreviewStatusSnapshot {
            requested_backend: self.requested_backend,
            effective_backend: if native_ready {
                PreviewBackend::Interactive
            } else {
                PreviewBackend::Raster
            },
            interactive_requested,
            should_attempt_native: self
                .should_attempt_interactive(typst_preview_available, platform_supported),
            native_ready,
            canonical_artifact_available: self.content.pdf().is_some()
                && self
                    .content
                    .artifact_key()
                    .is_some_and(|key| key.revision == document_revision),
            interactive_transitioning: self
                .interactive_transitioning(typst_preview_available, platform_supported),
            fallback_state: if typst_preview_available
                && preview_visible
                && self.requested_backend == PreviewPreference::Interactive
                && !native_ready
            {
                Some(if self.connection.endpoint().is_some() {
                    &self.webview_state
                } else {
                    &self.tinymist_state
                })
            } else {
                None
            },
        }
    }

    pub(crate) fn raster_required(
        &self,
        interactive_requested: bool,
        screenshot_pending: bool,
    ) -> bool {
        let interactive_unavailable = (self.connection.endpoint().is_none()
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

pub fn visible_page_range(
    pages: &[PageGeometry],
    scroll_y: f32,
    viewport_height: f32,
) -> Option<RangeInclusive<usize>> {
    let bottom = scroll_y + viewport_height.max(0.0);
    let first = pages
        .iter()
        .find(|page| page.bottom() >= scroll_y)
        .map(|page| page.index)?;
    let last = pages
        .iter()
        .rev()
        .find(|page| page.top <= bottom)
        .map(|page| page.index)?;
    Some(first..=last.max(first))
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
    fn rejected_ready_notifications_never_reset_the_five_attempt_budget() {
        use crate::tinymist::{Generation, TinymistEvent};
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_preview_enabled = true;
        let mut now = Instant::now();
        for attempt in 1..=5 {
            let generation = Generation(attempt);
            preview.recovery.started(generation);
            preview.connection.start(generation);
            assert_eq!(
                preview.preview_ready(generation, "http://127.0.0.1:1234", false),
                Ok(false),
                "not initialized"
            );
            assert!(preview.initialized(generation));
            assert!(!preview.initialized(generation), "duplicate initialization");
            assert_eq!(
                preview.preview_ready(Generation(0), "not a URL", false),
                Ok(false),
                "stale event"
            );
            let message = preview
                .preview_ready(generation, "not a URL", false)
                .unwrap_err();
            assert!(preview.connection.endpoint().is_none());
            let failure = TinymistEvent::Error {
                generation,
                stage: "preview",
                message: message.into(),
                fatal: false,
            };
            let transition = preview.transition(PreviewTransitionEvent::Failure(&failure, now));
            assert_eq!(
                transition.effects.contains(&PreviewEffect::ScheduleRaster),
                attempt == 5
            );
            assert!(
                preview
                    .transition(PreviewTransitionEvent::Failure(&failure, now))
                    .effects
                    .is_empty()
            );
            if attempt < 5 {
                now += tiptoptyp_core::recovery::RETRY_DELAY;
                assert_eq!(
                    preview
                        .transition(PreviewTransitionEvent::RecoveryTick(now))
                        .effects,
                    vec![PreviewEffect::RestartService {
                        preserve_surface: true
                    }]
                );
            }
        }
        assert!(
            preview
                .tinymist_state
                .detail()
                .contains("Five consecutive failures")
        );
    }

    #[test]
    fn readiness_is_generation_owned_and_lsp_only_never_embeds_a_preview() {
        use crate::tinymist::Generation;
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_preview_enabled = true;
        preview.recovery.started(Generation(1));
        preview.connection.start(Generation(1));
        assert!(preview.initialized(Generation(1)));
        assert_eq!(
            preview.preview_ready(Generation(1), "http://127.0.0.1:1234", false),
            Ok(true)
        );
        let endpoint = preview.connection.endpoint().cloned();
        preview.connection.suspend(true);
        preview.recovery.started(Generation(2));
        preview.connection.start(Generation(2));
        assert_eq!(preview.connection.endpoint(), endpoint.as_ref());
        assert_eq!(
            preview.preview_ready(Generation(1), "http://127.0.0.1:4321", true),
            Ok(false)
        );
        assert!(!preview.initialized(Generation(1)));
        assert!(preview.initialized(Generation(2)));
        // Equal URL does not mean equal server: this still admits a handoff.
        assert_eq!(
            preview.preview_ready(Generation(2), "http://127.0.0.1:1234", true),
            Ok(true)
        );
        assert!(preview.webview_state.detail().contains("existing preview"));
        preview.suspend_document("No tab is open");
        assert!(preview.connection.endpoint().is_none());
        assert_eq!(
            preview.preview_ready(Generation(2), "http://127.0.0.1:1234", true),
            Ok(false)
        );
        preview.tinymist_preview_enabled = false;
        preview.recovery.started(Generation(3));
        preview.connection.start(Generation(3));
        assert!(preview.initialized(Generation(3)));
        assert_eq!(
            preview.preview_ready(Generation(3), "http://127.0.0.1:1234", false),
            Ok(false)
        );
        assert_eq!(preview.tinymist_state.detail(), "Tinymist LSP is ready");
        assert!(preview.connection.endpoint().is_none());
    }

    #[test]
    fn unchanged_visibility_has_no_effects_or_repaint_requests() {
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        assert!(preview.visibility_changed(false, false).effects.is_empty());
        assert_eq!(
            preview.visibility_changed(true, true).effects,
            vec![
                PreviewEffect::RestartService {
                    preserve_surface: false
                },
                PreviewEffect::ScheduleRaster,
            ]
        );
        // The restart adapter installs the requested service configuration.
        preview.tinymist_preview_enabled = true;
        for _ in 0..100 {
            assert!(preview.visibility_changed(true, true).effects.is_empty());
        }
        assert_eq!(
            preview.visibility_changed(false, false).effects,
            vec![PreviewEffect::RestartService {
                preserve_surface: false
            }]
        );
        preview.tinymist_preview_enabled = false;
        for _ in 0..100 {
            assert!(preview.visibility_changed(false, false).effects.is_empty());
        }
        preview.tinymist_preview_enabled = true;
        preview.suspend_document("No document window is open");
        for _ in 0..100 {
            assert!(
                preview.visibility_changed(false, false).effects.is_empty(),
                "a suspended document must not repeatedly request restarts"
            );
        }
    }

    #[test]
    fn interactive_compile_timing_uses_one_matching_cycle_and_ignores_duplicate_reports() {
        use crate::tinymist::{CompileStatus::*, Generation};
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        let start = std::time::Instant::now();
        preview.compile_status(Generation(1), "/main.typ".into(), CompileSuccess, start);
        assert_eq!(
            preview.status,
            PreviewStatus::Waiting,
            "no fabricated startup timing"
        );
        preview.compile_status(Generation(1), "/main.typ".into(), Compiling, start);
        preview.compile_status(
            Generation(1),
            "/main.typ".into(),
            Compiling,
            start + Duration::from_millis(5),
        );
        preview.compile_status(
            Generation(1),
            "/main.typ".into(),
            CompileSuccess,
            start + Duration::from_millis(18),
        );
        assert_eq!(
            preview.status,
            PreviewStatus::Ready(Duration::from_millis(18))
        );
        preview.compile_status(
            Generation(1),
            "/main.typ".into(),
            CompileSuccess,
            start + Duration::from_secs(1),
        );
        assert_eq!(
            preview.status,
            PreviewStatus::Ready(Duration::from_millis(18))
        );
        preview.compile_status(Generation(1), "/main.typ".into(), Compiling, start);
        preview.compile_status(
            Generation(2),
            "/main.typ".into(),
            CompileSuccess,
            start + Duration::from_secs(1),
        );
        assert!(
            !matches!(preview.status, PreviewStatus::Ready(_)),
            "never time across server generations"
        );
        preview.compile_status(Generation(2), "/other.typ".into(), Compiling, start);
        preview.compile_status(Generation(2), "/other.typ".into(), CompileError, start);
        assert_eq!(preview.status, PreviewStatus::Error);
    }

    #[test]
    fn protocol_failures_wait_and_only_the_fifth_selects_fallback() {
        use crate::tinymist::{Generation, TinymistEvent};
        use tiptoptyp_core::recovery::{Failure, RETRY_DELAY};
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_preview_enabled = true;
        let mut now = std::time::Instant::now();
        for attempt in 1..=5 {
            let generation = Generation(attempt);
            preview.recovery.started(generation);
            preview.connection.start(generation);
            let failure = TinymistEvent::Error {
                generation,
                stage: if attempt % 2 == 0 {
                    "initialize"
                } else {
                    "preview"
                },
                message: "test failure".to_owned(),
                fatal: attempt % 2 == 0,
            };
            let result = preview.receive_tinymist_failure(&failure, now).unwrap();
            assert_eq!(
                preview.receive_tinymist_failure(
                    &TinymistEvent::Stopped {
                        generation,
                        code: None,
                        reason: "same failed process exited".to_owned(),
                    },
                    now
                ),
                Some(Failure::Ignored)
            );
            if attempt < 5 {
                assert!(matches!(result, Failure::Waiting { .. }));
                assert!(preview.interactive_transitioning(true, true));
                assert!(!preview.recovery.take_retry(now));
                now += RETRY_DELAY;
                assert!(preview.recovery.take_retry(now));
                assert!(!preview.recovery.take_retry(now));
            } else {
                assert_eq!(result, Failure::Exhausted);
                assert!(!preview.interactive_transitioning(true, true));
                assert!(!preview.interactive_active(true, true));
                assert!(matches!(preview.tinymist_state, ServiceState::Failed(_)));
            }
        }
    }

    #[test]
    fn retry_keeps_existing_display_but_rejects_readiness_and_unrelated_errors() {
        use crate::tinymist::{Generation, TinymistEvent};
        use tiptoptyp_core::recovery::Failure;
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        let generation = Generation(1);
        let now = std::time::Instant::now();
        preview.tinymist_preview_enabled = true;
        preview.recovery.started(generation);
        preview.connection.start(generation);
        preview.connection.initialized(generation);
        preview.connection.connect(
            generation,
            url::Url::parse("http://127.0.0.1:1234").unwrap(),
        );
        preview.webview_state = ServiceState::Ready("display".into());
        for stage in ["formatting", "navigation", "hover"] {
            assert_eq!(
                preview.receive_tinymist_failure(
                    &TinymistEvent::Error {
                        generation,
                        stage,
                        message: "optional feature failed".into(),
                        fatal: false,
                    },
                    now
                ),
                None
            );
        }
        assert!(matches!(
            preview.receive_tinymist_failure(
                &TinymistEvent::Stopped {
                    generation,
                    code: None,
                    reason: "process exited".into(),
                },
                now
            ),
            Some(Failure::Waiting { failures: 1, .. })
        ));
        assert!(!preview.connection.is_ready());
        assert!(preview.interactive_active(true, true));
        assert!(!preview.recovery.recovered(generation));
    }

    #[test]
    fn transitions_emit_each_recovery_render_and_repaint_effect_once() {
        use crate::tinymist::{Generation, TinymistEvent};
        use tiptoptyp_core::recovery::RETRY_DELAY;
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_preview_enabled = true;
        let mut now = Instant::now();

        for attempt in 1..=5 {
            let generation = Generation(attempt);
            preview.recovery.started(generation);
            let failure = TinymistEvent::Stopped {
                generation,
                code: None,
                reason: "test failure".into(),
            };
            let transition = preview.transition(PreviewTransitionEvent::Failure(&failure, now));
            assert!(transition.handled);
            assert_eq!(
                transition
                    .effects
                    .iter()
                    .filter(|effect| matches!(effect, PreviewEffect::StopAttempt(_)))
                    .count(),
                1
            );
            assert_eq!(
                transition
                    .effects
                    .iter()
                    .filter(|effect| matches!(effect, PreviewEffect::DiscardLanguageRequests))
                    .count(),
                1
            );
            let duplicate = preview.transition(PreviewTransitionEvent::Failure(&failure, now));
            assert!(duplicate.handled);
            assert!(duplicate.effects.is_empty());
            if attempt < 5 {
                assert_eq!(
                    transition
                        .effects
                        .iter()
                        .filter(|effect| matches!(effect, PreviewEffect::RepaintAfter(_)))
                        .count(),
                    1
                );
                assert!(
                    !transition
                        .effects
                        .iter()
                        .any(|effect| matches!(effect, PreviewEffect::ScheduleRaster))
                );
                let idle = preview.transition(PreviewTransitionEvent::RecoveryTick(now));
                assert!(matches!(
                    idle.effects.as_slice(),
                    [PreviewEffect::RepaintAfter(_)]
                ));
                now += RETRY_DELAY;
                let retry = preview.transition(PreviewTransitionEvent::RecoveryTick(now));
                assert_eq!(
                    retry.effects,
                    vec![PreviewEffect::RestartService {
                        preserve_surface: true
                    }]
                );
            } else {
                assert_eq!(
                    transition
                        .effects
                        .iter()
                        .filter(|effect| matches!(effect, PreviewEffect::ScheduleRaster))
                        .count(),
                    1
                );
                assert!(
                    !transition
                        .effects
                        .iter()
                        .any(|effect| matches!(effect, PreviewEffect::RepaintAfter(_)))
                );
                assert!(
                    preview
                        .transition(PreviewTransitionEvent::RecoveryTick(now))
                        .effects
                        .is_empty(),
                    "exhausted recovery must not leave an idle repaint loop"
                );
            }
        }

        assert_eq!(
            preview
                .transition(PreviewTransitionEvent::PreviewEntryChanged)
                .effects,
            vec![PreviewEffect::RestartService {
                preserve_surface: true
            }]
        );
        assert_eq!(
            preview
                .transition(PreviewTransitionEvent::PauseChanged {
                    generation: Some(Generation(8)),
                    paused: true,
                })
                .effects,
            vec![PreviewEffect::SetRefresh {
                generation: Generation(8),
                refresh: crate::tinymist::PreviewRefresh::OnSave,
            }]
        );
        assert_eq!(
            preview
                .transition(PreviewTransitionEvent::RenderRequested)
                .effects,
            vec![PreviewEffect::ScheduleRaster]
        );
        preview.recovery.started(Generation(9));
        let recovering_export = preview.transition(PreviewTransitionEvent::RenderRequested);
        assert_eq!(
            recovering_export.effects,
            vec![PreviewEffect::ScheduleRaster],
            "an export render remains one bounded request during recovery"
        );
        assert_eq!(
            preview
                .transition(PreviewTransitionEvent::Restart {
                    preserve_surface: false,
                })
                .effects,
            vec![PreviewEffect::RestartService {
                preserve_surface: false,
            }]
        );
        assert_eq!(
            preview.transition(PreviewTransitionEvent::Stop).effects,
            vec![PreviewEffect::StopService]
        );
    }

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
    fn visible_range_reports_every_intersecting_page() {
        let pages = page_stack_geometry([[100, 100], [100, 100], [100, 100]], 1.0);
        assert_eq!(visible_page_range(&pages, 0.0, 60.0), Some(0..=0));
        assert_eq!(
            visible_page_range(&pages, pages[0].bottom() - 1.0, PAGE_GAP + 2.0),
            Some(0..=1)
        );
        assert_eq!(visible_page_range(&[], 0.0, 100.0), None);
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
        preview
            .content
            .accept_artifact(key(9, 3), Arc::from(&b"old"[..]));
        preview.content.accept_raster(key(9, 3), vec![]);
        preview
            .content
            .accept_artifact(key(9, 4), Arc::from(&b"new"[..]));
        assert!(!preview.accepts_raster(key(9, 3), 9));
    }

    #[test]
    fn catalog_replacement_retains_old_layout_and_rejects_stale_artifact_and_appearance() {
        let catalog = |size| PdfDocumentCatalog {
            pages: vec![PdfPageMetadata {
                size,
                links: Vec::new(),
            }],
        };
        let mut preview = PreviewController::new(false, PreviewPreference::Native);
        let old = key(9, 3);
        let new = key(9, 4);
        preview.accept_artifact(old, Arc::from(&b"old"[..]));
        assert!(preview.accept_catalog(old, catalog([100, 200])));
        assert_eq!(preview.content.raster_key(), Some(old));
        assert_eq!(preview.content.pages()[0].size, [100, 200]);

        preview.accept_artifact(new, Arc::from(&b"new"[..]));
        assert!(preview.accept_catalog(new, catalog([300, 400])));
        assert_eq!(preview.content.raster_key(), Some(old));
        assert_eq!(preview.content.pages()[0].size, [100, 200]);
        let request = preview.raster_request_key().unwrap();
        assert_eq!(request.artifact, new);
        assert!(!preview.accepts_page_key(RasterPageRequestKey {
            artifact: old,
            ..request
        }));

        preview.bump_appearance(true);
        assert!(!preview.accepts_page_key(request));
        let replacement = preview.raster_request_key().unwrap();
        assert_eq!(
            replacement.appearance_revision,
            request.appearance_revision + 1
        );
        assert!(preview.accepts_page_key(replacement));
        assert_eq!(preview.content.pages()[0].size, [100, 200]);
    }

    #[test]
    fn rebinding_a_designated_preview_does_not_promote_an_old_generation() {
        let mut preview = PreviewController::new(false, PreviewPreference::Native);
        preview
            .content
            .accept_artifact(key(4, 11), Arc::from(&b"old"[..]));
        preview.content.accept_raster(key(4, 11), vec![]);
        preview
            .content
            .accept_artifact(key(4, 12), Arc::from(&b"new"[..]));

        preview.clear_for_document(5, true);

        assert_eq!(preview.content.artifact_key(), Some(key(5, 12)));
        assert_eq!(preview.content.raster_key(), Some(key(5, 11)));
        assert!(!preview.accepts_raster(key(5, 11), 5));
    }

    #[test]
    fn requested_and_effective_backends_expose_fallback_state() {
        let mut preview = PreviewController::new(false, PreviewPreference::Interactive);
        preview.tinymist_state = ServiceState::Failed("server stopped".to_owned());
        preview.accept_artifact(key(7, 1), Arc::from(&b"pdf"[..]));
        let status = preview.status_snapshot(true, true, true, 0);
        assert_eq!(status.effective_backend, PreviewBackend::Raster);
        assert!(!status.canonical_artifact_available);
        assert_eq!(status.backend_label(), "Rasterised PDF · fallback");
        assert!(
            status
                .fallback_reason()
                .is_some_and(|reason| reason.contains("server stopped"))
        );

        preview.connection.start(crate::tinymist::Generation(1));
        preview
            .connection
            .initialized(crate::tinymist::Generation(1));
        preview.connection.connect(
            crate::tinymist::Generation(1),
            url::Url::parse("http://127.0.0.1:23625").unwrap(),
        );
        let status = preview.status_snapshot(true, true, true, 7);
        assert!(status.canonical_artifact_available);
        preview.webview_state = ServiceState::Ready("loaded".to_owned());
        let status = preview.status_snapshot(true, true, true, 0);
        assert_eq!(status.effective_backend, PreviewBackend::Interactive);
        assert!(status.fallback_reason().is_none());

        preview.recovery.started(crate::tinymist::Generation(2));
        let failure = crate::tinymist::TinymistEvent::Stopped {
            generation: crate::tinymist::Generation(2),
            code: None,
            reason: "restart".to_owned(),
        };
        let now = Instant::now();
        preview.transition(PreviewTransitionEvent::Failure(&failure, now));
        let recovering = preview.status_snapshot(true, true, true, 7);
        assert!(
            recovering.native_ready,
            "the retained surface remains usable"
        );
        assert!(recovering.interactive_transitioning);
        assert_eq!(recovering.effective_backend, PreviewBackend::Interactive);
    }
}
