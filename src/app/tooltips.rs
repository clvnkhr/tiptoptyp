//! Tooltip geometry, timing, interaction, rendering, and viewport-owned caches.
//! Callers provide anchors/content; this module has no application-state access.
use super::{
    ASSET_HOVER_CARD_MAX_IMAGE, ASSET_HOVER_ERROR_SIZE, ASSET_HOVER_LOADING_SIZE,
    RecentWorkspaceAction, approximate_char_capacity, format_rect, normalize_browser_link_target,
    tail_elide,
};
use crate::{
    asset::AssetThumbnailResult,
    child_view::{ChildViewHost, ChildViewSpec, scoped_child_viewport_id, viewport_scoped_id},
    diagnostics::DiagnosticSeverity,
    document::DocumentKind,
    generic_highlight::GenericSyntaxHighlighter,
    highlight::SyntaxHighlighter,
    screenshot::CaptureController,
    settings::DEFAULT_HOVER_DELAY_MS,
    syntax_theme::ResolvedTypstStyles,
    theme::{self, METRICS},
};
use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::{
    collections::{VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};
const TOOLTIP_HANDOFF_GRACE: Duration = Duration::from_millis(300);
#[derive(Debug, Clone)]
pub(super) struct HoverTooltipOverlay {
    pub(super) origin: Rect,
    pub(super) anchor: Pos2,
    pub(super) detail: Arc<str>,
    pub(super) opacity: f32,
}

#[derive(Debug, Clone)]
pub(super) struct AssetHoverCandidate {
    pub(super) origin: Rect,
    pub(super) anchor: Pos2,
    pub(super) placement: TooltipPlacement,
    pub(super) path: PathBuf,
    pub(super) kind: DocumentKind,
    pub(super) opacity: f32,
}

#[derive(Clone)]
pub(super) enum AssetHoverContent {
    Loading,
    Ready {
        texture: egui::TextureHandle,
        source_size: [usize; 2],
    },
    Error(String),
}

#[derive(Clone)]
pub(super) struct AssetHoverState {
    pub(super) origin: Rect,
    pub(super) anchor: Pos2,
    pub(super) placement: TooltipPlacement,
    pub(super) path: PathBuf,
    pub(super) kind: DocumentKind,
    pub(super) opacity: f32,
    pub(super) token: crate::asset::ThumbnailToken,
    pub(super) content: AssetHoverContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TooltipPlacement {
    Below,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarkdownInlineSpan {
    pub(super) text: String,
    pub(super) code: bool,
    pub(super) bold: bool,
    pub(super) italics: bool,
    pub(super) link: Option<String>,
}

/// Syntax highlighting a tooltip is independent of scrolling, so retain the
/// completed jobs in the owning viewport. This keeps wheel events from
/// reparsing and re-highlighting every code span on every frame.
#[derive(Clone, Default)]
pub(super) struct TooltipCodeCache {
    pub(super) jobs: VecDeque<TooltipCodeCacheEntry>,
}

#[derive(Clone)]
pub(super) struct TooltipCodeCacheEntry {
    pub(super) source: String,
    pub(super) token: String,
    pub(super) dark_mode: bool,
    pub(super) editor_font: egui::FontId,
    pub(super) colors: [[u8; 4]; 14],
    pub(super) job: Option<Arc<egui::text::LayoutJob>>,
}

impl TooltipCodeCacheEntry {
    pub(super) fn matches(
        &self,
        source: &str,
        token: &str,
        dark_mode: bool,
        editor_font: &egui::FontId,
        colors: &[[u8; 4]; 14],
    ) -> bool {
        self.source == source
            && self.token == token
            && self.dark_mode == dark_mode
            && self.editor_font == *editor_font
            && self.colors == *colors
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TooltipGeometry {
    pub(super) identity: u64,
    pub(super) origin: Rect,
    pub(super) card: Rect,
    /// Root-local pointer position at the moment the tooltip opened. Keep this
    /// fixed while the cursor crosses the native-view gap: moving the apex
    /// with the cursor would make the safe triangle collapse underneath it.
    pub(super) handoff_apex: Pos2,
    pub(super) pointer_inside_viewport: bool,
    pub(super) handoff_until: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TooltipInteractionState {
    pub(super) identity: u64,
    pub(super) focused: bool,
    pub(super) had_focus: bool,
    pub(super) focus_requested: bool,
    pub(super) dismissed: bool,
}

impl TooltipInteractionState {
    pub(super) const fn new(identity: u64) -> Self {
        Self {
            identity,
            focused: false,
            had_focus: false,
            focus_requested: false,
            dismissed: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HoverRuntimeConfig {
    pub(super) delay: Duration,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HoverTimingState {
    pub(super) widget: egui::Id,
    pub(super) started: f64,
}

pub(super) fn native_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-native-hover-tooltip")
}

pub(super) fn asset_hover_candidate_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-asset-hover-candidate")
}

pub(super) fn current_asset_hover_candidate(
    context: &egui::Context,
) -> Option<AssetHoverCandidate> {
    // Derive the viewport-scoped ID before entering the data lock. Calling
    // `viewport_id()` from inside `Context::data` would re-enter the same lock
    // and deadlock the first UI frame.
    let id = asset_hover_candidate_id(context);
    context.data(|data| data.get_temp::<AssetHoverCandidate>(id))
}

pub(super) fn clear_asset_hover_candidate(context: &egui::Context) {
    let id = asset_hover_candidate_id(context);
    context.data_mut(|data| data.remove::<AssetHoverCandidate>(id));
}

pub(super) fn clear_native_hover_overlay(context: &egui::Context) {
    let id = native_hover_tooltip_id(context);
    context.data_mut(|data| data.remove::<HoverTooltipOverlay>(id));
}

pub(super) fn asset_hover_timing_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("asset-hover-timing")
}

pub(super) fn tooltip_geometry_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("geometry")
}

pub(super) fn tooltip_interaction_id(context: &egui::Context) -> egui::Id {
    native_hover_tooltip_id(context).with("interaction")
}

pub(super) fn offer_asset_hover(
    response: &egui::Response,
    origin: Rect,
    path: PathBuf,
    kind: DocumentKind,
    placement: TooltipPlacement,
) {
    if native_tooltip_handoff_blocks(&response.ctx, origin) {
        return;
    }
    let Some(opacity) = hover_opacity(response, asset_hover_timing_id(&response.ctx)) else {
        return;
    };
    let anchor = match placement {
        TooltipPlacement::Below => {
            origin.left_bottom() + egui::vec2(0.0, METRICS.editor.tooltip_gap)
        }
        TooltipPlacement::Right => {
            origin.right_center() + egui::vec2(METRICS.editor.tooltip_gap, 0.0)
        }
    };
    let candidate = AssetHoverCandidate {
        origin,
        anchor,
        placement,
        path,
        kind,
        opacity,
    };
    let id = asset_hover_candidate_id(&response.ctx);
    response
        .ctx
        .data_mut(|data| data.insert_temp(id, candidate));
}

pub(super) fn asset_thumbnail_result_matches(
    hover: Option<&AssetHoverState>,
    result: &AssetThumbnailResult,
) -> bool {
    hover.is_some_and(|hover| {
        hover.token == result.token && hover.path == result.path && hover.kind == result.kind
    })
}

pub(super) fn asset_tooltip_identity(origin: Rect, path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    for coordinate in [origin.min.x, origin.min.y, origin.max.x, origin.max.y] {
        coordinate.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn fit_asset_preview_size(source: [usize; 2], bounds: Vec2) -> Vec2 {
    let source = Vec2::new(source[0].max(1) as f32, source[1].max(1) as f32);
    let bounds = Vec2::new(bounds.x.max(1.0), bounds.y.max(1.0));
    let scale = (bounds.x / source.x).min(bounds.y / source.y).min(1.0);
    (source * scale).max(Vec2::splat(1.0))
}

pub(super) fn asset_hover_card_size(
    content: &AssetHoverContent,
    viewport_size: Vec2,
    frame_margin: Vec2,
    edge: f32,
) -> Vec2 {
    let desired = match content {
        AssetHoverContent::Loading => ASSET_HOVER_LOADING_SIZE,
        AssetHoverContent::Error(_) => ASSET_HOVER_ERROR_SIZE,
        AssetHoverContent::Ready { source_size, .. } => {
            let image = fit_asset_preview_size(*source_size, ASSET_HOVER_CARD_MAX_IMAGE);
            Vec2::new(image.x + frame_margin.x, image.y + frame_margin.y)
        }
    };
    let available = (viewport_size - Vec2::splat(edge.max(0.0) * 2.0)).max(Vec2::splat(1.0));
    desired.min(available).max(Vec2::splat(1.0))
}

pub(super) fn show_asset_hover_contents(
    ui: &mut egui::Ui,
    path: &Path,
    _kind: DocumentKind,
    content: &AssetHoverContent,
    content_size: Vec2,
) {
    ui.set_min_size(content_size);
    ui.set_max_size(content_size);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match content {
        AssetHoverContent::Loading => {
            ui.vertical_centered(|ui| {
                ui.add_space((content_size.y - 44.0).max(0.0) * 0.5);
                ui.spinner();
                ui.label(format!("Loading preview for {file_name}…"));
            });
        }
        AssetHoverContent::Error(error) => {
            ui.add_sized(
                [content_size.x, METRICS.popup.tooltip_title_height],
                egui::Label::new(RichText::new(file_name).strong())
                    .selectable(true)
                    .truncate(),
            );
            ui.separator();
            ui.add(egui::Label::new(error).selectable(true).wrap());
        }
        AssetHoverContent::Ready {
            texture,
            source_size,
        } => {
            let image_size = fit_asset_preview_size(*source_size, content_size);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(image_size)
                        .alt_text(format!("Preview of {file_name}")),
                );
            });
        }
    }
}

pub(super) fn native_tooltip_handoff_active(context: &egui::Context, emit_trace: bool) -> bool {
    let geometry_id = tooltip_geometry_id(context);
    let interaction_id = tooltip_interaction_id(context);
    let pointer = context
        .pointer_hover_pos()
        .or_else(|| context.pointer_latest_pos());
    let now = context.input(|input| input.time);
    let motion_id = viewport_scoped_id(context, "tooltip-pointer-motion");
    let (active, geometry, interaction) = context.data_mut(|data| {
        let mut geometry = data.get_temp::<TooltipGeometry>(geometry_id);
        if let Some(current) = geometry {
            let previous = data
                .get_temp::<(u64, Pos2)>(motion_id)
                .filter(|(identity, _)| *identity == current.identity)
                .map(|(_, position)| position);
            if !current.pointer_inside_viewport
                && tooltip_pointer_moved_away(previous, pointer, current)
            {
                let mut dismissed = TooltipInteractionState::new(current.identity);
                dismissed.dismissed = true;
                data.insert_temp(interaction_id, dismissed);
            }
            if let Some(pointer) = pointer {
                data.insert_temp(motion_id, (current.identity, pointer));
            }
        }
        if let Some(current) = geometry {
            let current = refresh_tooltip_root_geometry(current, pointer, now);
            geometry = Some(current);
            data.insert_temp(geometry_id, current);
        }
        let interaction = tooltip_interaction_for_geometry(
            geometry,
            data.get_temp::<TooltipInteractionState>(interaction_id),
        );
        let active = tooltip_handoff_is_active(pointer, now, geometry, interaction);
        (active, geometry, interaction)
    });
    if emit_trace {
        trace_native_tooltip_handoff(pointer, now, geometry, interaction, active);
    }
    if let Some(geometry) = geometry
        && geometry.handoff_until > now
        && !geometry.pointer_inside_viewport
        && !interaction.is_some_and(|state| state.focused || state.focus_requested)
        && !pointer.is_some_and(|pointer| tooltip_region_contains(pointer, geometry))
    {
        // A pointer leaving the route may not generate another repaint. Make
        // the grace deadline self-expiring so the tooltip cannot linger
        // forever when there is no competing animation to drive the frame.
        context.request_repaint_after(Duration::from_secs_f64(
            (geometry.handoff_until - now).max(0.001),
        ));
    }
    active
}

pub(super) fn native_tooltip_handoff_blocks(
    context: &egui::Context,
    candidate_origin: Rect,
) -> bool {
    if update_hover_scroll(context) {
        return true;
    }
    let dismissed_id = viewport_scoped_id(context, "dismissed-tooltip-origin");
    if context.data(|data| {
        data.get_temp::<Rect>(dismissed_id)
            .is_some_and(|origin| origin.intersects(candidate_origin))
    }) {
        return true;
    }
    let active = native_tooltip_handoff_active(context, false);
    let geometry_id = tooltip_geometry_id(context);
    let active_origin = context.data(|data| {
        data.get_temp::<TooltipGeometry>(geometry_id)
            .map(|geometry| geometry.origin)
    });
    tooltip_handoff_blocks(active, active_origin, candidate_origin)
}

pub(super) fn tooltip_handoff_blocks(
    active: bool,
    active_origin: Option<Rect>,
    candidate_origin: Rect,
) -> bool {
    // Focus state can briefly outlive its geometry while native viewports are
    // being recreated. Without a concrete source rectangle there is no route
    // to protect, so that stale state must not suppress every future tooltip.
    active && active_origin.is_some_and(|origin| origin != candidate_origin)
}

pub(super) fn tooltip_handoff_is_active(
    pointer: Option<Pos2>,
    now: f64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> bool {
    let interaction = tooltip_interaction_for_geometry(geometry, interaction);
    if interaction.is_some_and(|state| state.dismissed) {
        return false;
    }
    if interaction.is_some_and(|state| state.focused || state.focus_requested) {
        return true;
    }
    geometry.is_some_and(|geometry| {
        geometry.handoff_until > now
            || geometry.pointer_inside_viewport
            || pointer.is_some_and(|pointer| tooltip_region_contains(pointer, geometry))
    })
}

pub(super) fn tooltip_interaction_for_geometry(
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> Option<TooltipInteractionState> {
    interaction.filter(|state| geometry.is_none_or(|geometry| geometry.identity == state.identity))
}

pub(super) fn refresh_tooltip_root_geometry(
    mut geometry: TooltipGeometry,
    pointer: Option<Pos2>,
    now: f64,
) -> TooltipGeometry {
    // Pointer ownership transfers between native viewports. Once the cursor
    // enters the child, the root reports no pointer; only the child may clear
    // `pointer_inside_viewport`. The root owns route/deadline updates only.
    if pointer.is_some_and(|pointer| tooltip_region_contains(pointer, geometry)) {
        geometry.handoff_until = tooltip_handoff_deadline(now);
    }
    geometry
}

pub(super) fn refresh_tooltip_child_geometry(
    mut geometry: TooltipGeometry,
    identity: u64,
    pointer_inside_viewport: bool,
) -> Option<TooltipGeometry> {
    // A native child can deliver its final pointer event after a competing
    // source has selected a new tooltip. Do not let that stale child mutate
    // either the replacement tooltip's geometry or its interaction state.
    if geometry.identity != identity {
        return None;
    }
    geometry.pointer_inside_viewport = pointer_inside_viewport;
    Some(geometry)
}

pub(super) fn tooltip_handoff_deadline(now: f64) -> f64 {
    now + TOOLTIP_HANDOFF_GRACE.as_secs_f64()
}

pub(super) fn tooltip_viewport_should_render(
    deterministic_scene: bool,
    root_focused: bool,
    handoff_active: bool,
    identity: u64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
) -> bool {
    deterministic_scene
        || root_focused
        || handoff_active
        || geometry.is_some_and(|geometry| {
            geometry.identity == identity && geometry.pointer_inside_viewport
        })
        || interaction.is_some_and(|state| {
            state.identity == identity && (state.focused || state.focus_requested)
        })
}

pub(super) fn trace_native_tooltip_handoff(
    pointer: Option<Pos2>,
    now: f64,
    geometry: Option<TooltipGeometry>,
    interaction: Option<TooltipInteractionState>,
    active: bool,
) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("TIPTOPTYP_UI_TRACE").is_some()) {
        return;
    }
    let pointer = pointer.map_or_else(|| "none".to_owned(), format_pos);
    let geometry = geometry.map_or_else(
        || "none".to_owned(),
        |geometry| {
            format!(
                "origin={} card={} viewport_inside={} until={:.3}",
                format_rect(geometry.origin),
                format_rect(geometry.card),
                geometry.pointer_inside_viewport,
                geometry.handoff_until,
            )
        },
    );
    let interaction = interaction.map_or_else(
        || "none".to_owned(),
        |interaction| {
            format!(
                "focused={} requested={} dismissed={}",
                interaction.focused, interaction.focus_requested, interaction.dismissed,
            )
        },
    );
    eprintln!(
        "ui.tooltip.handoff now={now:.3} pointer={pointer} {geometry} {interaction} active={active}"
    );
}

pub(super) fn format_pos(pos: Pos2) -> String {
    format!("({:.1},{:.1})", pos.x, pos.y)
}

pub(super) fn settings_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-settings-hover-tooltip")
}

pub(super) fn typst_overrides_hover_tooltip_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-typst-overrides-hover-tooltip")
}

pub(super) fn hover_runtime_config_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-hover-runtime-config")
}

/// Install the per-viewport hover timing without re-entering egui's context
/// lock. `viewport_id()` itself reads context state, so the ID must be derived
/// before `data_mut` takes the write lock. This runs during every first frame.
pub(super) fn install_hover_runtime_config(context: &egui::Context, delay: Duration) {
    let id = hover_runtime_config_id(context);
    let config = HoverRuntimeConfig { delay };
    context.data_mut(|data| data.insert_temp(id, config));
}

pub(super) fn hover_runtime_config(context: &egui::Context) -> HoverRuntimeConfig {
    let id = hover_runtime_config_id(context);
    context.data(|data| {
        data.get_temp::<HoverRuntimeConfig>(id)
            .unwrap_or(HoverRuntimeConfig {
                delay: Duration::from_millis(DEFAULT_HOVER_DELAY_MS),
            })
    })
}

pub(super) fn diagnostic_hover_timing_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "tiptoptyp-diagnostic-hover-timing")
}

pub(super) fn native_hover_text(
    response: egui::Response,
    detail: impl Into<String>,
) -> egui::Response {
    let id = native_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

pub(super) fn show_recent_workspace_row(
    ui: &mut egui::Ui,
    path: &Path,
    card_width: f32,
) -> Option<RecentWorkspaceAction> {
    let path_text = path.display().to_string();
    let max_chars = approximate_char_capacity(
        card_width - theme::SPACE.content * 2.0,
        theme::TYPE.supporting,
    );
    let response = ui.add_sized(
        [ui.available_width(), METRICS.menu.row_height],
        egui::Button::new(tail_elide(&path_text, max_chars)),
    );
    native_hover_text(response.clone(), path_text);
    let menu_was_open = response.context_menu_opened();
    let mut remove = false;
    response.context_menu(|ui| {
        if crate::app::icons::action_button(ui, "Remove from Recents").clicked() {
            remove = true;
            ui.close();
        }
    });
    if remove {
        Some(RecentWorkspaceAction::Remove(path.to_path_buf()))
    } else if response.clicked_by(egui::PointerButton::Primary)
        && !menu_was_open
        && !response.context_menu_opened()
    {
        Some(RecentWorkspaceAction::Open(path.to_path_buf()))
    } else {
        None
    }
}

pub(super) fn settings_hover_text(
    response: egui::Response,
    detail: impl Into<String>,
) -> egui::Response {
    let id = settings_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

pub(super) fn typst_overrides_hover_text(
    response: egui::Response,
    detail: impl Into<String>,
) -> egui::Response {
    let id = typst_overrides_hover_tooltip_id(&response.ctx);
    hover_text_with_id(response, detail, id)
}

pub(super) fn hover_text_with_id(
    response: egui::Response,
    detail: impl Into<String>,
    id: egui::Id,
) -> egui::Response {
    if let Some(opacity) = hover_opacity(&response, id.with("timing")) {
        let tooltip = HoverTooltipOverlay {
            origin: response.rect,
            anchor: response.rect.left_bottom() + egui::vec2(0.0, theme::SPACE.small),
            detail: Arc::from(detail.into()),
            opacity,
        };
        // Keep the currently visible native tooltip while the pointer crosses
        // another hoverable control on its way to that tooltip. Settings and
        // overrides use separate local cards and should retain their normal
        // independent behavior.
        let is_native_tooltip = id == native_hover_tooltip_id(&response.ctx);
        if !is_native_tooltip || !native_tooltip_handoff_blocks(&response.ctx, tooltip.origin) {
            response.ctx.data_mut(|data| data.insert_temp(id, tooltip));
        }
    }
    response
}

pub(super) fn hover_opacity(response: &egui::Response, timing_id: egui::Id) -> Option<f32> {
    if !response.hovered() || update_hover_scroll(&response.ctx) {
        response.ctx.data_mut(|data| {
            if data
                .get_temp::<HoverTimingState>(timing_id)
                .is_some_and(|state| state.widget == response.id)
            {
                data.remove::<HoverTimingState>(timing_id);
            }
        });
        return None;
    }
    let force_id = viewport_scoped_id(&response.ctx, "force-pointer-tooltip");
    if response
        .ctx
        .data(|data| data.get_temp::<bool>(force_id).unwrap_or(false))
    {
        return Some(1.0);
    }
    let now = response.ctx.input(|input| input.time);
    let config = hover_runtime_config(&response.ctx);
    let previous = response
        .ctx
        .data(|data| data.get_temp::<HoverTimingState>(timing_id));
    let state = hover_timing_for_widget(previous, response.id, now);
    // `response.hovered()` is the source of truth. A slow or event-driven frame
    // gap does not mean the pointer left the widget, and restarting here can
    // postpone the tooltip forever under load. The non-hovered path above
    // removes this state on an observed exit.
    response
        .ctx
        .data_mut(|data| data.insert_temp(timing_id, state));

    let elapsed = Duration::from_secs_f64((now - state.started).max(0.0));
    if elapsed < config.delay {
        response
            .ctx
            .request_repaint_after((config.delay - elapsed).min(METRICS.motion.hover_poll));
        return None;
    }
    Some(1.0)
}

pub(super) fn reset_hover_timing(context: &egui::Context, timing_id: egui::Id) {
    context.data_mut(|data| data.remove::<HoverTimingState>(timing_id));
}

pub(super) fn hover_timing_for_widget(
    previous: Option<HoverTimingState>,
    widget: egui::Id,
    now: f64,
) -> HoverTimingState {
    previous
        .filter(|state| state.widget == widget)
        .unwrap_or(HoverTimingState {
            widget,
            started: now,
        })
}

/// Scroll input belongs to a viewport. In particular, scrolling a native
/// tooltip must not dismiss it or wake the source editor. Keep hovers disarmed
/// under a stationary pointer after scrolling (including trackpad momentum).
pub(super) fn update_hover_scroll(context: &egui::Context) -> bool {
    let id = viewport_scoped_id(context, "hover-scroll-suppression");
    let (scrolling, pointer) = context.input(|input| (
        input.raw.events.iter().any(|event| matches!(event, egui::Event::MouseWheel { delta, .. } if *delta != Vec2::ZERO)) || input.smooth_scroll_delta != Vec2::ZERO,
        input.pointer.latest_pos(),
    ));
    context.data_mut(|data| {
        let previous = data.get_temp::<Option<Pos2>>(id);
        if scrolling {
            data.insert_temp(id, pointer);
            true
        } else if previous.is_some_and(|position| position == pointer) {
            true
        } else {
            data.remove::<Option<Pos2>>(id);
            false
        }
    })
}

pub(super) fn source_scroll_changed(context: &egui::Context, offset: Vec2) -> bool {
    let id = viewport_scoped_id(context, "source-hover-scroll-offset");
    let suppression_id = viewport_scoped_id(context, "hover-scroll-suppression");
    let pointer = context.pointer_latest_pos();
    context.data_mut(|data| {
        let changed = data
            .get_temp::<Vec2>(id)
            .is_some_and(|previous| previous != offset);
        data.insert_temp(id, offset);
        if changed {
            data.insert_temp(suppression_id, pointer);
        }
        changed
    })
}

pub(super) fn tooltip_pointer_moved_away(
    previous: Option<Pos2>,
    pointer: Option<Pos2>,
    geometry: TooltipGeometry,
) -> bool {
    let (Some(previous), Some(pointer)) = (previous, pointer) else {
        return false;
    };
    !tooltip_region_contains(pointer, geometry)
        && geometry.card.distance_to_pos(pointer) > geometry.card.distance_to_pos(previous) + 1.0
}

pub(super) fn hover_request_ready(
    visible: bool,
    connected: bool,
    open: bool,
    requested: bool,
) -> bool {
    visible && connected && open && !requested
}

/// Notify the parent only about ownership/actions, never ordinary popup
/// scrolling. The callback may outlive an old target, so reject stale writes.
pub(super) fn publish_tooltip_interaction(
    context: &egui::Context,
    parent: egui::ViewportId,
    geometry_id: egui::Id,
    interaction_id: egui::Id,
    mut interaction: TooltipInteractionState,
    pointer_inside_viewport: bool,
) {
    let changed = context.data_mut(|data| {
        let Some(geometry) = data.get_temp::<TooltipGeometry>(geometry_id) else {
            return false;
        };
        let Some(updated) =
            refresh_tooltip_child_geometry(geometry, interaction.identity, pointer_inside_viewport)
        else {
            return false;
        };
        if geometry.pointer_inside_viewport && !pointer_inside_viewport {
            interaction.dismissed = true;
            interaction.focused = false;
            interaction.focus_requested = false;
        }
        let changed = geometry.pointer_inside_viewport != pointer_inside_viewport
            || data.get_temp::<TooltipInteractionState>(interaction_id) != Some(interaction);
        data.insert_temp(geometry_id, updated);
        data.insert_temp(interaction_id, interaction);
        changed
    });
    if changed {
        context.request_repaint_of(parent);
    }
}

/// Registration alone does not invalidate a deferred viewport. Invalidate
/// only on changed content/style, not on each parent paint.
pub(super) fn repaint_tooltip_on_change(
    context: &egui::Context,
    salt: &'static str,
    identity: u64,
    content_revision: u64,
) {
    let id = viewport_scoped_id(context, salt);
    let style = context.style_of(context.theme());
    let changed = context.data_mut(|data| {
        let key = (identity, content_revision, style);
        let changed = data
            .get_temp::<(u64, u64, Arc<egui::Style>)>(id)
            .is_none_or(|old| old.0 != key.0 || old.1 != key.1 || !Arc::ptr_eq(&old.2, &key.2));
        data.insert_temp(id, key);
        changed
    });
    if changed {
        context.request_repaint_of(scoped_child_viewport_id(context, salt));
    }
}

const TOOLTIP_PREVIEW_CHARS: usize = 600;

fn tooltip_body_width(natural_width: f32) -> f32 {
    (natural_width + METRICS.popup.tooltip_text_padding).clamp(
        METRICS.popup.tooltip_min_width,
        METRICS.popup.tooltip_max_width,
    )
}

/// Byte boundaries are found only in the preview, never by counting
/// or parsing the entire server response during initial hover layout.
pub(super) fn tooltip_preview_end(text: &str) -> usize {
    let tail = text;
    let end = tail
        .char_indices()
        .nth(TOOLTIP_PREVIEW_CHARS)
        .map_or(tail.len(), |(i, _)| i);
    if end == tail.len() {
        return text.len();
    }
    // Prefer complete markdown lines, unless that would make a tiny page.
    tail[..end]
        .rfind('\n')
        .filter(|i| *i > end / 2)
        .map_or(end, |i| i + 1)
}

pub(super) fn show_tooltip_document(
    ui: &mut egui::Ui,
    detail: &str,
    identity: u64,
    focused: bool,
    link_sender: &mpsc::Sender<String>,
) {
    let id = ui.id().with("tooltip-engaged");
    let engaged = focused
        || ui.rect_contains_pointer(ui.max_rect())
        || ui.ctx().data(|data| data.get_temp::<(u64, bool)>(id)) == Some((identity, true));
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, (identity, engaged)));
    // Entering/focusing the popup reveals the complete response automatically.
    // Keep the same viewport/scroll identity and size: no controls and no jump
    // underneath the pointer. Brief incidental hovers only prepare a preview.
    let visible = if engaged {
        detail
    } else {
        &detail[..tooltip_preview_end(detail)]
    };
    egui::ScrollArea::vertical()
        .id_salt("tooltip-document")
        .auto_shrink([false, true])
        .max_height((ui.available_height() - theme::SPACE.small).max(1.0))
        .show(ui, |ui| {
            show_markdown(ui, visible, link_sender);
        });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_native_tooltip_card(
    context: &egui::Context,
    viewport_salt: &'static str,
    anchor: Pos2,
    origin: Rect,
    detail: Arc<str>,
    severity: Option<DiagnosticSeverity>,
    placement: TooltipPlacement,
    captures: &CaptureController,
    link_sender: &mpsc::Sender<String>,
) {
    let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
        return;
    };
    let theme = context.theme();
    let style = context.style_of(theme);
    let tooltip_frame = theme::tooltip_card_frame(&style);
    let frame_margin = tooltip_frame.total_margin().sum();
    let body_font = egui::TextStyle::Body.resolve(&style);
    let preview_end = tooltip_preview_end(&detail);
    let preview = &detail[..preview_end];
    let desired_card_width = if severity.is_some() {
        METRICS.popup.tooltip_width
    } else {
        let natural_width = context.fonts_mut(|fonts| {
            fonts
                .layout(
                    preview.to_owned(),
                    body_font.clone(),
                    style.visuals.text_color(),
                    f32::INFINITY,
                )
                .size()
                .x
        });
        tooltip_body_width(natural_width)
    };
    let available_width = (window_rect.width() - METRICS.popup.viewport_edge * 2.0).max(1.0);
    let width = (desired_card_width + frame_margin.x).min(available_width);
    let card_width = (width - frame_margin.x).max(1.0);
    let body_height = context.fonts_mut(|fonts| {
        fonts
            .layout(
                preview.to_owned(),
                body_font,
                style.visuals.text_color(),
                (card_width - METRICS.popup.tooltip_text_padding).max(1.0),
            )
            .size()
            .y
    });
    let available_height = (window_rect.height() - METRICS.popup.viewport_edge * 2.0).max(1.0);
    let height = (METRICS.popup.tooltip_title_height + body_height + frame_margin.y)
        .clamp(
            METRICS.popup.tooltip_min_height,
            METRICS.popup.tooltip_max_height,
        )
        .min(available_height);
    let root_local_card = place_native_tooltip_card(
        Rect::from_min_size(Pos2::ZERO, window_rect.size()),
        origin,
        anchor,
        Vec2::new(width, height),
        placement,
        METRICS.popup.viewport_edge,
    );
    let position = window_rect.min + root_local_card.min.to_vec2();
    let interaction_id = tooltip_interaction_id(context);
    let identity = cached_tooltip_identity(context, origin, &detail);
    let interaction = context.data(|data| {
        data.get_temp::<TooltipInteractionState>(interaction_id)
            .filter(|state| state.identity == identity)
            .unwrap_or(TooltipInteractionState::new(identity))
    });
    if interaction.dismissed {
        return;
    }
    let geometry_id = tooltip_geometry_id(context);
    let now = context.input(|input| input.time);
    let previous = context.data(|data| {
        data.get_temp::<TooltipGeometry>(geometry_id)
            .filter(|geometry| geometry.identity == identity)
    });
    let handoff_apex = previous.map_or_else(
        || {
            context
                .pointer_hover_pos()
                .or_else(|| context.pointer_latest_pos())
                .filter(|pointer| origin.contains(*pointer))
                .unwrap_or_else(|| origin.center())
        },
        |geometry| geometry.handoff_apex,
    );
    context.data_mut(|data| {
        data.insert_temp(
            geometry_id,
            TooltipGeometry {
                identity,
                origin,
                card: root_local_card,
                handoff_apex,
                // The child viewport exclusively owns this bit. The root has
                // no pointer while the cursor is over a native child and must
                // preserve the child's last observation across paint passes.
                pointer_inside_viewport: previous
                    .is_some_and(|geometry| geometry.pointer_inside_viewport),
                handoff_until: previous.map_or_else(
                    || tooltip_handoff_deadline(now),
                    |geometry| geometry.handoff_until,
                ),
            },
        );
    });
    let capture_viewport = captures.has_pending_for("diagnostic");
    let activate_viewport = capture_viewport || interaction.focus_requested || interaction.focused;

    // Keep the popup non-activating in production, while still letting it
    // receive pointer movement and wheel events for scrolling. A queued QA
    // capture temporarily activates its isolated viewport so macOS supplies
    // the repeated paint passes needed by the settling countdown.
    let spec = ChildViewSpec::tooltip(
        viewport_salt,
        "tiptoptyp",
        position,
        Vec2::new(width, height),
        activate_viewport,
        "diagnostic",
    );
    repaint_tooltip_on_change(context, viewport_salt, identity, activate_viewport as u64);
    let parent = context.viewport_id();
    let context = context.clone();
    let link_sender = link_sender.clone();
    let child_captures = captures.clone();
    ChildViewHost::show_deferred(
        &context.clone(),
        captures,
        spec,
        theme,
        &style.clone(),
        move |ui, input| {
            let interaction = context
                .data(|data| data.get_temp::<TooltipInteractionState>(interaction_id))
                .filter(|state| state.identity == identity)
                .unwrap_or(TooltipInteractionState::new(identity));
            let popup_focused = if child_captures.has_pending_for("diagnostic") {
                None
            } else {
                input.focused
            };
            if interaction.dismissed {
                return;
            }
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("tooltip-link-activated"), false));
            let frame = if interaction.focused {
                tooltip_frame.stroke(Stroke::new(
                    1.0,
                    style.visuals.widgets.active.bg_stroke.color,
                ))
            } else {
                tooltip_frame
            };
            let frame_response = frame.show(ui, |ui| {
                show_tooltip_document(
                    ui,
                    &detail,
                    identity,
                    interaction.focused || interaction.focus_requested,
                    &link_sender,
                );
            });
            let card_rect = frame_response.response.rect;
            let dismiss_requested = input.escape_pressed
                || ui.ctx().data(|data| {
                    data.get_temp::<bool>(egui::Id::new("tooltip-link-activated"))
                        .unwrap_or(false)
                });
            let pointer_inside_viewport = ui.rect_contains_pointer(ui.max_rect());
            let pointer_inside_card = ui.rect_contains_pointer(card_rect);
            let popup_interacted =
                pointer_inside_card && ui.input(|input| input.pointer.any_pressed());
            let interaction = update_tooltip_interaction_state(
                interaction,
                identity,
                popup_interacted,
                popup_focused,
            );
            let interaction = if dismiss_requested {
                TooltipInteractionState {
                    focused: false,
                    focus_requested: false,
                    dismissed: true,
                    ..interaction
                }
            } else {
                interaction
            };
            publish_tooltip_interaction(
                &context,
                parent,
                geometry_id,
                interaction_id,
                interaction,
                pointer_inside_viewport,
            );
            if popup_interacted && !dismiss_requested {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        },
    );
}

pub(super) fn show_local_tooltip_card(
    context: &egui::Context,
    anchor: Pos2,
    detail: &str,
    opacity: f32,
) -> Rect {
    let style = context.style_of(context.theme());
    // Settings hints are plain text, not document-sized markdown cards. Measure
    // the wrapped text first so even a reused Area can shrink from a long path
    // to a one-line hint without retaining its previous width or scroll height.
    let bounds = context.content_rect().shrink(theme::SPACE.small);
    let frame =
        theme::popup_card_frame(&style).inner_margin(egui::Margin::same(theme::SPACE.small as i8));
    let margin = frame.total_margin().sum();
    let max_width = METRICS
        .popup
        .tooltip_width
        .min((bounds.width() - margin.x).max(1.0));
    let galley = context.fonts_mut(|fonts| {
        fonts.layout(
            detail.to_owned(),
            egui::TextStyle::Body.resolve(&style),
            style.visuals.text_color(),
            max_width,
        )
    });
    let max_height = METRICS
        .popup
        .tooltip_max_height
        .min((bounds.height() - margin.y).max(1.0));
    let size = Vec2::new(galley.size().x.ceil(), galley.size().y.min(max_height)) + margin;
    let card = place_local_tooltip_card(anchor, size, bounds);
    egui::Area::new(viewport_scoped_id(context, "settings-tooltip-card"))
        .order(egui::Order::Foreground)
        .fixed_pos(card.min)
        .default_size(card.size())
        .constrain_to(bounds)
        // We placed this frame using its current text size, not the Area's
        // cached size from a potentially different hint on the previous frame.
        .constrain(false)
        .show(context, |ui| {
            ui.set_opacity(opacity);
            frame
                .show(ui, |ui| {
                    ui.set_width(galley.size().x.ceil());
                    egui::ScrollArea::vertical()
                        .auto_shrink([true, true])
                        .max_height(max_height)
                        .show(ui, |ui| ui.add(egui::Label::new(galley).selectable(true)));
                })
                .response
                .rect
        })
        .inner
}

pub(super) fn place_local_tooltip_card(anchor: Pos2, size: Vec2, bounds: Rect) -> Rect {
    let size = size.min(bounds.size());
    Rect::from_min_size(
        Pos2::new(
            anchor.x.clamp(bounds.left(), bounds.right() - size.x),
            anchor.y.clamp(bounds.top(), bounds.bottom() - size.y),
        ),
        size,
    )
}

pub(super) fn frame_content_size(viewport_size: Vec2, frame_margin: Vec2) -> Vec2 {
    Vec2::new(
        (viewport_size.x - frame_margin.x).max(1.0),
        (viewport_size.y - frame_margin.y).max(1.0),
    )
}

#[derive(Clone)]
enum MarkdownBlock {
    Space,
    Separator,
    Code {
        source: String,
        token: String,
    },
    Line {
        prefix: &'static str,
        spans: Vec<MarkdownInlineSpan>,
        scale: f32,
    },
}

#[derive(Clone)]
struct ParsedTooltipMarkdown {
    source: String,
    blocks: Vec<MarkdownBlock>,
}

fn cached_tooltip_markdown(context: &egui::Context, markdown: &str) -> Arc<ParsedTooltipMarkdown> {
    let id = viewport_scoped_id(context, "tooltip-markdown");
    if let Some(cached) = context.data(|data| data.get_temp::<Arc<ParsedTooltipMarkdown>>(id))
        && cached.source == markdown
    {
        crate::performance::counter("tooltip.markdown.cache_hit");
        return cached;
    }
    crate::performance::counter("tooltip.markdown.cache_miss");
    let _span = crate::performance::span("tooltip.markdown.parse");
    let parsed = Arc::new(ParsedTooltipMarkdown {
        source: markdown.to_owned(),
        blocks: parse_tooltip_markdown(markdown),
    });
    context.data_mut(|data| data.insert_temp(id, parsed.clone()));
    parsed
}

fn parse_tooltip_markdown(markdown: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut fenced = false;
    let mut fence_len = 0;
    let mut skipping_doc_error = false;
    let mut fence_token = String::new();
    let mut fence_lines = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if skipping_doc_error {
            if trimmed == "---" {
                skipping_doc_error = false;
                blocks.push(MarkdownBlock::Separator);
            }
            continue;
        }
        if let Some((length, token)) = tooltip_markdown_fence(trimmed) {
            if fenced {
                let is_closing = length >= fence_len && token.is_empty();
                if is_closing {
                    push_tooltip_code_block(
                        &mut blocks,
                        fence_lines.join("\n"),
                        std::mem::take(&mut fence_token),
                    );
                    fence_lines.clear();
                    fence_token.clear();
                    fence_len = 0;
                    fenced = false;
                } else {
                    // A shorter fence, such as the nested ```typ examples in
                    // Tinymist's four-backtick documentation block, is data.
                    fence_lines.push(line.to_owned());
                }
            } else {
                fence_len = length;
                fence_token = token.to_owned();
                fenced = true;
            }
            continue;
        }
        if fenced {
            fence_lines.push(line.to_owned());
            continue;
        }
        // Tinymist joins multiple hover sections with a Markdown thematic
        // break. Keep the divider semantic instead of displaying raw `---`.
        if trimmed == "---" {
            blocks.push(MarkdownBlock::Separator);
            continue;
        }
        // A documentation compiler failure is an implementation detail of
        // the server, not useful hover documentation. It can include paths,
        // source excerpts, and many lines of carets; omit that whole section
        // while retaining the useful signature or preceding documentation.
        if trimmed.starts_with("failed to parse docs:") {
            skipping_doc_error = true;
            continue;
        }
        if trimmed.is_empty() {
            blocks.push(MarkdownBlock::Space);
            continue;
        }

        let (prefix, content, heading) = if let Some(content) = trimmed.strip_prefix("### ") {
            ("", content, 1.05)
        } else if let Some(content) = trimmed.strip_prefix("## ") {
            ("", content, 1.1)
        } else if let Some(content) = trimmed.strip_prefix("# ") {
            ("", content, 1.15)
        } else if let Some(content) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ("• ", content, 1.0)
        } else {
            ("", trimmed, 1.0)
        };
        blocks.push(MarkdownBlock::Line {
            prefix,
            spans: markdown_inline_spans(content),
            scale: heading,
        });
    }
    if fenced {
        push_tooltip_code_block(&mut blocks, fence_lines.join("\n"), fence_token);
    }
    while matches!(
        blocks.last(),
        Some(MarkdownBlock::Separator | MarkdownBlock::Space)
    ) {
        blocks.pop();
    }
    blocks
}

fn tooltip_markdown_fence(line: &str) -> Option<(usize, &str)> {
    let length = line.bytes().take_while(|byte| *byte == b'`').count();
    (length >= 3).then(|| (length, line[length..].trim()))
}

fn push_tooltip_code_block(blocks: &mut Vec<MarkdownBlock>, source: String, token: String) {
    let source = strip_tooltip_doc_parse_failure(&source);
    if source.trim().is_empty() {
        return;
    }
    blocks.push(MarkdownBlock::Code { source, token });
}

fn strip_tooltip_doc_parse_failure(source: &str) -> String {
    let mut lines = source.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    if !first.trim_start().starts_with("failed to parse docs:") {
        return source.to_owned();
    }

    // Tinymist places the useful documentation after a blank line following
    // the compiler excerpt. Drop only that diagnostic prefix; keep the docs,
    // including any shorter nested fences, intact.
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            return lines.collect::<Vec<_>>().join("\n").trim_start().to_owned();
        }
    }
    String::new()
}

pub(super) fn show_markdown(ui: &mut egui::Ui, markdown: &str, link_sender: &mpsc::Sender<String>) {
    show_markdown_with_culling(ui, markdown, link_sender, true);
}

struct TooltipMarkdownLayout {
    parsed: Arc<ParsedTooltipMarkdown>,
    width: f32,
    style: Arc<egui::Style>,
    font_witness: Arc<egui::Galley>,
    editor_font: egui::FontId,
    heights: Vec<Option<f32>>,
    #[cfg(test)]
    rendered_blocks: usize,
}

fn show_markdown_with_culling(
    ui: &mut egui::Ui,
    markdown: &str,
    link_sender: &mpsc::Sender<String>,
    cull: bool,
) {
    let _span = crate::performance::span("tooltip.markdown.paint");
    let parsed = cached_tooltip_markdown(ui.ctx(), markdown);
    let id = viewport_scoped_id(ui.ctx(), "tooltip-markdown-layout");
    let width = ui.available_width();
    let style = ui.style().clone();
    let font_witness = ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(String::new(), egui::FontId::default(), egui::Color32::WHITE)
    });
    let editor_font = theme::editor_font();
    let layout = ui.ctx().data_mut(|data| {
        if let Some(cached) = data.get_temp::<Arc<Mutex<TooltipMarkdownLayout>>>(id) {
            let matches = {
                let previous = cached.lock().unwrap();
                Arc::ptr_eq(&parsed, &previous.parsed)
                    && previous.width == width
                    && previous.style == style
                    && Arc::ptr_eq(&font_witness, &previous.font_witness)
                    && previous.editor_font == editor_font
            };
            if matches {
                return cached;
            }
        }
        let layout = Arc::new(Mutex::new(TooltipMarkdownLayout {
            #[cfg(test)]
            rendered_blocks: 0,
            heights: vec![None; parsed.blocks.len()],
            parsed: parsed.clone(),
            width,
            style,
            font_witness,
            editor_font,
        }));
        data.insert_temp(id, layout.clone());
        layout
    });
    // Keep geometry for only the current document/style. Scrolling skips
    // offscreen blocks before highlighting or constructing their widgets.
    let mut layout = layout.lock().unwrap();
    #[cfg(test)]
    {
        layout.rendered_blocks = 0;
    }
    let highlighter = GenericSyntaxHighlighter::default();
    let mut typst_highlighter = SyntaxHighlighter::default();
    for (index, block) in parsed.blocks.iter().enumerate() {
        if cull
            && let Some(height) = layout.heights[index]
            && !ui.is_rect_visible(Rect::from_min_size(
                ui.next_widget_position(),
                Vec2::new(width, height),
            ))
        {
            ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
            continue;
        }
        let response = ui.push_id(index, |ui| match block {
            MarkdownBlock::Space => ui.add_space(theme::SPACE.small),
            MarkdownBlock::Separator => {
                ui.separator();
            }
            MarkdownBlock::Code { source, token } => {
                show_markdown_code_block(ui, &highlighter, &mut typst_highlighter, source, token)
            }
            MarkdownBlock::Line {
                prefix,
                spans,
                scale,
            } => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if !prefix.is_empty() {
                        ui.add(egui::Label::new(*prefix).selectable(true));
                    }
                    show_markdown_inline(
                        ui,
                        spans,
                        *scale,
                        &highlighter,
                        &mut typst_highlighter,
                        link_sender,
                    );
                });
            }
        });
        #[cfg(test)]
        {
            layout.rendered_blocks += 1;
        }
        layout.heights[index] = Some(response.response.rect.height());
    }
}

pub(super) fn show_markdown_code_block(
    ui: &mut egui::Ui,
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    source: &str,
    token: &str,
) {
    let dark_mode = ui.visuals().dark_mode;
    let job = cached_tooltip_code_job(
        ui.ctx(),
        highlighter,
        typst_highlighter,
        source,
        token,
        dark_mode,
        theme::syntax_palette(ui.ctx()),
    );
    if let Some(job) = job {
        ui.add(egui::Label::new(job).selectable(true).wrap());
    } else {
        let color = theme::syntax_palette(ui.ctx()).plain;
        ui.add(
            egui::Label::new(RichText::new(source).monospace().color(color))
                .selectable(true)
                .wrap(),
        );
    }
}

pub(super) fn cached_tooltip_code_job(
    context: &egui::Context,
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    source: &str,
    token: &str,
    dark_mode: bool,
    palette: theme::SyntaxPalette,
) -> Option<Arc<egui::text::LayoutJob>> {
    let token = normalize_tooltip_code_token(token);
    let cache_id = viewport_scoped_id(context, "tooltip-code-cache");
    let editor_font = theme::editor_font();
    let colors = tooltip_code_cache_colors(palette);
    if let Some(job) = context.data_mut(|data| {
        data.get_temp_mut_or_default::<TooltipCodeCache>(cache_id)
            .jobs
            .iter()
            .find(|entry| entry.matches(source, &token, dark_mode, &editor_font, &colors))
            .map(|entry| entry.job.clone())
    }) {
        crate::performance::counter("tooltip.code.cache_hit");
        return job;
    }
    crate::performance::counter("tooltip.code.cache_miss");
    let job = tooltip_code_job(
        highlighter,
        typst_highlighter,
        source,
        &token,
        dark_mode,
        palette,
    )
    .map(Arc::new);
    context.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<TooltipCodeCache>(cache_id);
        // Keep exact entries bounded without flushing all recently rendered
        // tooltips when one more diagnostic appears.
        if cache.jobs.len() >= 32 {
            cache.jobs.pop_front();
        }
        cache.jobs.push_back(TooltipCodeCacheEntry {
            source: source.to_owned(),
            token,
            dark_mode,
            editor_font,
            colors,
            job: job.clone(),
        });
    });
    job
}

pub(super) fn tooltip_code_cache_colors(palette: theme::SyntaxPalette) -> [[u8; 4]; 14] {
    [
        palette.plain,
        palette.comment,
        palette.operator,
        palette.number,
        palette.emphasis,
        palette.link,
        palette.string,
        palette.label,
        palette.heading,
        palette.keyword,
        palette.interpolated,
        palette.error,
        palette.error_background,
        palette.editor_background,
    ]
    .map(|color| color.to_array())
}

pub(super) fn tooltip_code_job(
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    source: &str,
    token: &str,
    dark_mode: bool,
    palette: theme::SyntaxPalette,
) -> Option<egui::text::LayoutJob> {
    let token = normalize_tooltip_code_token(token);
    let mode = tooltip_code_mode(&token);
    if matches!(
        mode,
        TooltipCodeMode::TypstSource | TooltipCodeMode::TypstCode
    ) {
        typst_highlighter.set_styles(ResolvedTypstStyles::resolve(
            palette,
            None,
            &Default::default(),
        ));
        return Some(if mode == TooltipCodeMode::TypstCode {
            typst_highlighter.highlight_code(source, dark_mode, highlighter)
        } else {
            typst_highlighter.highlight(source, dark_mode, highlighter)
        });
    }
    highlighter.highlight_token(source, &token, dark_mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TooltipCodeMode {
    TypstSource,
    TypstCode,
    Generic,
}

fn normalize_tooltip_code_token(token: &str) -> String {
    token.trim().to_ascii_lowercase()
}

fn tooltip_code_mode(token: &str) -> TooltipCodeMode {
    match token {
        "typ" | "typst" => TooltipCodeMode::TypstSource,
        // `typc` is Tinymist's marked-string language for a Typst code
        // snippet. Accept the descriptive spellings too so other LSP clients
        // can feed the same popup without silently falling back to plaintext.
        "typc" | "typst-code" | "typst_code" | "typstcode" => TooltipCodeMode::TypstCode,
        _ => TooltipCodeMode::Generic,
    }
}

pub(super) fn show_markdown_inline(
    ui: &mut egui::Ui,
    spans: &[MarkdownInlineSpan],
    scale: f32,
    highlighter: &GenericSyntaxHighlighter,
    typst_highlighter: &mut SyntaxHighlighter,
    link_sender: &mpsc::Sender<String>,
) {
    let dark_mode = ui.visuals().dark_mode;
    for span in spans {
        if span.code {
            let job = cached_tooltip_code_job(
                ui.ctx(),
                highlighter,
                typst_highlighter,
                &span.text,
                "typc",
                dark_mode,
                theme::syntax_palette(ui.ctx()),
            );
            if let Some(job) = job {
                ui.add(egui::Label::new(job).selectable(true).wrap());
                continue;
            }
        }
        let mut rich = RichText::new(&span.text);
        if span.bold {
            rich = rich.strong();
        }
        if span.italics {
            rich = rich.italics();
        }
        if scale != 1.0 {
            rich = rich.size(theme::TYPE.content * scale);
        }
        if let Some(target) = &span.link {
            let response = ui
                .add(
                    egui::Label::new(rich.color(ui.visuals().hyperlink_color).underline())
                        .selectable(true)
                        .sense(Sense::click_and_drag())
                        .wrap(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() && link_sender.send(target.clone()).is_ok() {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new("tooltip-link-activated"), true)
                });
                let parent = ui
                    .input(|input| input.viewport().parent)
                    .unwrap_or(ui.ctx().viewport_id());
                ui.ctx().request_repaint_of(parent);
            }
        } else {
            ui.add(egui::Label::new(rich).selectable(true).wrap());
        }
    }
}

pub(super) fn markdown_inline_spans(text: &str) -> Vec<MarkdownInlineSpan> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut code = false;
    let mut bold = false;
    let mut italics = false;
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        if code {
            if chars[index] == '`' {
                if !current.is_empty() {
                    push_markdown_span(&mut spans, &mut current, true, bold, italics, None);
                }
                code = false;
            } else {
                current.push(chars[index]);
            }
            index += 1;
            continue;
        }
        if chars[index] == '['
            && let Some((label, target, next_index)) = markdown_link_at(&chars, index)
        {
            push_markdown_span(&mut spans, &mut current, false, bold, italics, None);
            let link = normalize_browser_link_target(&target);
            spans.push(MarkdownInlineSpan {
                text: label,
                code: false,
                bold,
                italics,
                link,
            });
            index = next_index;
            continue;
        }
        let (kind, marker, marker_len) = if chars[index] == '`' {
            (Some(0), '`', 1)
        } else if chars[index] == '*' && chars.get(index + 1) == Some(&'*') {
            (Some(1), '*', 2)
        } else if chars[index] == '_' && chars.get(index + 1) == Some(&'_') {
            (Some(1), '_', 2)
        } else if chars[index] == '*' {
            (Some(2), '*', 1)
        } else if chars[index] == '_' {
            (Some(2), '_', 1)
        } else {
            (None, '\0', 0)
        };
        let active = match kind {
            Some(1) => bold,
            Some(2) => italics,
            _ => false,
        };
        if let Some(kind) = kind
            && (active || marker_is_closed(&chars, index + marker_len, marker, marker_len))
        {
            push_markdown_span(&mut spans, &mut current, code, bold, italics, None);
            match kind {
                0 => code = !code,
                1 => bold = !bold,
                _ => italics = !italics,
            }
            index += marker_len;
        } else {
            current.extend(chars[index..index + marker_len.max(1)].iter().copied());
            index += marker_len.max(1);
        }
    }
    push_markdown_span(&mut spans, &mut current, code, bold, italics, None);
    spans
}

pub(super) fn push_markdown_span(
    spans: &mut Vec<MarkdownInlineSpan>,
    current: &mut String,
    code: bool,
    bold: bool,
    italics: bool,
    link: Option<String>,
) {
    if !current.is_empty() {
        spans.push(MarkdownInlineSpan {
            text: std::mem::take(current),
            code,
            bold,
            italics,
            link,
        });
    }
}

pub(super) fn markdown_link_at(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let label_end = find_unescaped_char(chars, start + 1, ']')?;
    if chars.get(label_end + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 1_usize;
    let mut index = label_end + 2;
    while index < chars.len() {
        if chars[index] == '\\' {
            index = (index + 2).min(chars.len());
            continue;
        }
        match chars[index] {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let label = unescape_markdown(&chars[start + 1..label_end]);
                    let target = unescape_markdown(&chars[label_end + 2..index]);
                    if target.trim().is_empty() {
                        return None;
                    }
                    return Some((label, target.trim().to_owned(), index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

pub(super) fn find_unescaped_char(chars: &[char], start: usize, needle: char) -> Option<usize> {
    let mut index = start;
    while index < chars.len() {
        if chars[index] == '\\' {
            index = (index + 2).min(chars.len());
            continue;
        }
        if chars[index] == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

pub(super) fn unescape_markdown(chars: &[char]) -> String {
    let mut output = String::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\'
            && let Some(character) = chars.get(index + 1)
        {
            output.push(*character);
            index += 2;
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    output
}

pub(super) fn marker_is_closed(chars: &[char], start: usize, marker: char, length: usize) -> bool {
    chars.get(start..).is_some_and(|rest| {
        rest.windows(length)
            .any(|window| window.iter().all(|character| *character == marker))
    })
}

pub(super) fn tooltip_region_contains(pointer: Pos2, geometry: TooltipGeometry) -> bool {
    let TooltipGeometry {
        origin,
        card,
        handoff_apex,
        ..
    } = geometry;
    if origin.contains(pointer) || card.contains(pointer) {
        return true;
    }

    // Protect every straight path from the pointer that opened the popup to
    // the complete facing edge of the card. This is deliberately a triangle,
    // not a narrow center line or a trapezoid based on the token bounds: the
    // user's intent starts at their cursor and may target either corner.
    let (first, second) = if card.left() >= origin.right() {
        (card.left_top(), card.left_bottom())
    } else if card.right() <= origin.left() {
        (card.right_top(), card.right_bottom())
    } else if card.top() >= origin.bottom() {
        (card.left_top(), card.right_top())
    } else if card.bottom() <= origin.top() {
        (card.left_bottom(), card.right_bottom())
    } else {
        return false;
    };
    point_in_triangle(pointer, handoff_apex, first, second)
}

fn point_in_triangle(point: Pos2, first: Pos2, second: Pos2, third: Pos2) -> bool {
    let cross = |start: Pos2, end: Pos2, point: Pos2| {
        let edge = end - start;
        let offset = point - start;
        edge.x * offset.y - edge.y * offset.x
    };
    let signs = [
        cross(first, second, point),
        cross(second, third, point),
        cross(third, first, point),
    ];
    let epsilon = 0.01;
    let has_negative = signs.iter().any(|value| *value < -epsilon);
    let has_positive = signs.iter().any(|value| *value > epsilon);
    !(has_negative && has_positive)
}

pub(super) fn tooltip_identity(origin: Rect, detail: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    detail.hash(&mut hasher);
    tooltip_identity_from_hash(origin, hasher.finish())
}

fn tooltip_identity_from_hash(origin: Rect, content_hash: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    content_hash.hash(&mut hasher);
    for coordinate in [origin.min.x, origin.min.y, origin.max.x, origin.max.y] {
        coordinate.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn cached_tooltip_identity(
    context: &egui::Context,
    origin: Rect,
    detail: &Arc<str>,
) -> u64 {
    let id = viewport_scoped_id(context, "tooltip-content-identity");
    let content_hash = context.data_mut(|data| {
        if let Some((source, hash)) = data.get_temp::<(Arc<str>, u64)>(id)
            && Arc::ptr_eq(&source, detail)
        {
            return hash;
        }
        let mut hasher = DefaultHasher::new();
        detail.hash(&mut hasher);
        let hash = hasher.finish();
        data.insert_temp(id, (detail.clone(), hash));
        hash
    });
    tooltip_identity_from_hash(origin, content_hash)
}

pub(super) fn place_native_tooltip_card(
    viewport: Rect,
    origin: Rect,
    anchor: Pos2,
    size: Vec2,
    placement: TooltipPlacement,
    edge: f32,
) -> Rect {
    let edge = edge.max(0.0);
    let min_x = (viewport.left() + edge).min(viewport.center().x);
    let min_y = (viewport.top() + edge).min(viewport.center().y);
    let max_x = (viewport.right() - edge - size.x).max(min_x);
    let max_y = (viewport.bottom() - edge - size.y).max(min_y);

    let (x, y) = match placement {
        TooltipPlacement::Below => {
            let gap = (anchor.y - origin.bottom()).max(0.0);
            let below = anchor.y;
            let above = origin.top() - gap - size.y;
            let y = if (min_y..=max_y).contains(&below) {
                below
            } else if (min_y..=max_y).contains(&above) {
                above
            } else {
                let room_below = (viewport.bottom() - edge - origin.bottom() - gap).max(0.0);
                let room_above = (origin.top() - gap - viewport.top() - edge).max(0.0);
                if room_above > room_below {
                    above.clamp(min_y, max_y)
                } else {
                    below.clamp(min_y, max_y)
                }
            };
            (anchor.x.clamp(min_x, max_x), y)
        }
        TooltipPlacement::Right => {
            let gap = (anchor.x - origin.right()).max(0.0);
            let right = anchor.x;
            let left = origin.left() - gap - size.x;
            let x = if (min_x..=max_x).contains(&right) {
                right
            } else if (min_x..=max_x).contains(&left) {
                left
            } else {
                let room_right = (viewport.right() - edge - origin.right() - gap).max(0.0);
                let room_left = (origin.left() - gap - viewport.left() - edge).max(0.0);
                if room_left > room_right {
                    left.clamp(min_x, max_x)
                } else {
                    right.clamp(min_x, max_x)
                }
            };
            (x, anchor.y.clamp(min_y, max_y))
        }
    };
    Rect::from_min_size(Pos2::new(x, y), size)
}

pub(super) fn update_tooltip_interaction_state(
    mut state: TooltipInteractionState,
    identity: u64,
    popup_interacted: bool,
    popup_focused: Option<bool>,
) -> TooltipInteractionState {
    if state.identity != identity {
        state = TooltipInteractionState::new(identity);
    }
    if popup_interacted {
        state.focus_requested = true;
    }
    match popup_focused {
        Some(true) => {
            state.focused = true;
            state.had_focus = true;
            state.dismissed = false;
        }
        Some(false) if state.had_focus => {
            state.focused = false;
            state.focus_requested = false;
            state.dismissed = true;
        }
        _ => {}
    }
    state
}

#[cfg(test)]
mod tests;
