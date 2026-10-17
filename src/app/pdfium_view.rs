//! Persistent egui PDFium viewer. New visible regions commit as a single batch.
use super::pdfium_render_settle::{Geometry, SettleState};
use super::*;
use crate::pdfium::{Batch, Catalog, Character, LinkTarget, Reply, Request, RequestKey, Worker};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PageContextLocation {
    revision: u64,
    page: usize,
    point: Vec2,
}

impl PageContextLocation {
    fn capture(
        response: &egui::Response,
        revision: u64,
        page: usize,
        page_rect: Rect,
        zoom: f32,
    ) -> Option<Self> {
        let pointer = response.interact_pointer_pos()?;
        (zoom.is_finite() && zoom > 0.0 && page_rect.contains(pointer)).then(|| Self {
            revision,
            page,
            point: (pointer - page_rect.min) / zoom,
        })
    }

    fn current(self, revision: u64, page: usize) -> Option<Self> {
        (self.revision == revision && self.page == page).then_some(self)
    }
}

fn page_context_menu(
    response: &egui::Response,
    location: Option<PageContextLocation>,
    characters: &[Character],
) {
    response.context_menu(|ui| {
        if let Some(location) = location {
            let reveal = ui.button("Reveal in Source");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("preview.Reveal in Source", &reveal);
            if reveal.clicked() {
                let id = viewport_scoped_id(ui.ctx(), "pdf-synctex");
                ui.ctx().data_mut(|data| {
                    data.insert_temp(id, (location.page, location.point.x, location.point.y));
                });
                ui.close();
            }
        }
        if ui.button("Copy page text").clicked() {
            ui.ctx()
                .copy_text(characters.iter().map(|c| c.value).collect());
            ui.close();
        }
    });
}

fn reveal_scroll_offset(target: Pos2, viewport: Vec2) -> Vec2 {
    egui::vec2(
        (target.x - viewport.x * 0.5).max(0.0),
        (target.y - viewport.y * 0.382).max(0.0),
    )
}

#[derive(Default)]
struct SearchMatches {
    query: Arc<str>,
    ranges: Vec<std::ops::Range<usize>>,
    #[cfg(test)]
    rebuilds: usize,
}

impl SearchMatches {
    fn for_query(
        &mut self,
        characters: &[Character],
        query: &Arc<str>,
    ) -> &[std::ops::Range<usize>] {
        if !Arc::ptr_eq(&self.query, query) {
            if self.query != *query {
                self.ranges = matching_characters(characters, query);
                #[cfg(test)]
                {
                    self.rebuilds += 1;
                }
            }
            // Every resident shares the view's query instead of retaining a
            // separate copy, even for a large pasted search.
            self.query = Arc::clone(query);
        }
        &self.ranges
    }
}

struct Resident {
    texture: egui::TextureHandle,
    characters: Vec<Character>,
    links: Vec<(Rect, LinkTarget)>,
    search_matches: SearchMatches,
}

struct PdfReveal {
    page: usize,
    point: Vec2,
    expected_pdf: Option<Arc<[u8]>>,
    attention: EditorAttention,
}

impl PdfReveal {
    fn obsolete(
        &self,
        bytes: &Arc<[u8]>,
        now: Instant,
        animation: crate::settings::RevealAnimation,
    ) -> bool {
        let attention = &self.attention;
        !animation.enabled
            || attention
                .animation
                .is_some_and(|previous| previous != animation.normalized())
            || ((attention.started.is_some() || attention.fixed_elapsed.is_some())
                && self
                    .expected_pdf
                    .as_ref()
                    .is_some_and(|expected| !Arc::ptr_eq(expected, bytes) && **expected != **bytes))
            || (attention.fixed_elapsed.is_none()
                && attention.started.map_or_else(
                    || now.saturating_duration_since(attention.armed_at) >= Duration::from_secs(2),
                    |start| {
                        now.saturating_duration_since(start)
                            >= Duration::from_millis(u64::from(animation.normalized().duration_ms))
                    },
                ))
    }
}

pub(super) struct PdfiumView {
    worker: Option<Worker>,
    path: PathBuf,
    bytes: Option<Arc<[u8]>>,
    revision: u64,
    active_revision: u64,
    requested: Option<RequestKey>,
    desired: Option<RequestKey>,
    pending_reply: Option<Reply>,
    settle: SettleState,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    render_requests: u64,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    render_admissions: u64,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    settling: bool,
    catalog: Option<Arc<Catalog>>,
    residents: BTreeMap<usize, Resident>,
    active_scale: u32,
    palette: (Color32, Color32),
    zoom: f32,
    fit: bool,
    page: usize,
    goto: Option<usize>,
    pending_position: Option<(usize, f32)>,
    position_point: Option<Vec2>,
    position_pdf: Option<Arc<[u8]>>,
    reveal: Option<PdfReveal>,
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    reveal_observation: Option<serde_json::Value>,
    anchor: (usize, f32),
    restore_anchor: bool,
    query: Arc<str>,
    search_pages: Vec<usize>,
    search_pending: bool,
    find_when_ready: bool,
    selection: Option<(usize, usize, usize)>,
    context_location: Option<PageContextLocation>,
    error: Option<String>,
    layout_zoom: f32,
    tops: Vec<f32>,
    height: f32,
    page_focus: Option<egui::Id>,
    scroll_owner: Option<egui::Id>,
    controls_open: bool,
    back: Vec<(usize, f32)>,
    forward: Vec<(usize, f32)>,
}

impl Default for PdfiumView {
    fn default() -> Self {
        Self {
            worker: None,
            path: PathBuf::new(),
            bytes: None,
            revision: 0,
            active_revision: 0,
            requested: None,
            desired: None,
            pending_reply: None,
            settle: SettleState::default(),
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            render_requests: 0,
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            render_admissions: 0,
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            settling: false,
            catalog: None,
            residents: BTreeMap::new(),
            active_scale: 0,
            palette: (Color32::WHITE, Color32::BLACK),
            zoom: 1.0,
            fit: true,
            page: 0,
            goto: None,
            pending_position: None,
            position_point: None,
            position_pdf: None,
            reveal: None,
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            reveal_observation: None,
            anchor: (0, 0.0),
            restore_anchor: false,
            query: Arc::from(""),
            search_pages: Vec::new(),
            search_pending: false,
            find_when_ready: false,
            selection: None,
            context_location: None,
            error: None,
            layout_zoom: 0.0,
            tops: Vec::new(),
            height: 0.0,
            page_focus: None,
            scroll_owner: None,
            controls_open: false,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }
}

impl PdfiumView {
    #[cfg(test)]
    pub(super) fn page_focus_id_for_tests(marker: &str) -> egui::Id {
        egui::Id::new(("focused-pdfium-test-page", marker))
    }

    #[cfg(test)]
    pub(super) fn focus_for_tests(&mut self, context: &egui::Context, marker: &str) {
        let id = Self::page_focus_id_for_tests(marker);
        self.page_focus = Some(id);
        context.memory_mut(|memory| memory.request_focus(id));
    }

    pub(super) fn has_focus(&self, context: &egui::Context) -> bool {
        context.input(|i| i.focused)
            && self
                .page_focus
                .is_some_and(|id| context.memory(|m| m.has_focus(id)))
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspected_palette(&self) -> [[u8; 4]; 2] {
        [self.palette.0.to_array(), self.palette.1.to_array()]
    }

    pub(super) fn command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
        force_find: bool,
    ) -> bool {
        let focused = self
            .page_focus
            .is_some_and(|id| context.memory(|m| m.focused()) == Some(id));
        if command == AppCommand::Find && (focused || force_find) {
            self.controls_open = true;
            context.request_repaint();
            return true;
        }
        if !focused {
            return false;
        }
        match command {
            AppCommand::Copy => {
                if let Some((page, a, b)) = self.selection
                    && let Some(resident) = self.residents.get(&page)
                {
                    context.copy_text(
                        resident.characters[a.min(b)..=a.max(b)]
                            .iter()
                            .map(|c| c.value)
                            .collect(),
                    );
                }
                true
            }
            AppCommand::SelectAll => {
                if let Some(resident) = self.residents.get(&self.page)
                    && !resident.characters.is_empty()
                {
                    self.selection = Some((self.page, 0, resident.characters.len() - 1));
                }
                true
            }
            _ => false,
        }
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        if let Some(error) = &self.error {
            Activity::Failed(error.clone())
        } else if self.bytes.is_none() {
            Activity::Inactive("Not in use")
        } else if !self.ready() || self.search_pending {
            Activity::Running
        } else {
            Activity::Idle
        }
    }
    pub(super) fn service_state(&self) -> ServiceState {
        if let Some(error) = &self.error {
            ServiceState::Failed(error.clone())
        } else if self.ready() {
            ServiceState::Ready("PDFium preview ready".into())
        } else {
            ServiceState::Starting("Preparing PDFium preview".into())
        }
    }

    pub(super) fn ready_for(&self, bytes: Option<&Arc<[u8]>>) -> bool {
        self.ready()
            && bytes
                .zip(self.bytes.as_ref())
                .is_some_and(|(expected, displayed)| Arc::ptr_eq(expected, displayed))
    }

    pub(super) fn ready(&self) -> bool {
        self.bytes.is_some()
            && self
                .desired
                .as_ref()
                .is_some_and(|key| self.reusable(key) && key.scale == self.active_scale)
    }

    // Reuse at any scale: resizing is GPU geometry until the final sharp batch.
    fn reusable(&self, key: &RequestKey) -> bool {
        self.active_revision == key.revision
            && self.palette == key.palette
            && !self.residents.is_empty()
            && key.pages.iter().all(|p| {
                self.residents.contains_key(p)
                    || self.catalog.as_ref().is_some_and(|c| *p >= c.sizes.len())
            })
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspected_render(&self) -> serde_json::Value {
        serde_json::json!({
            "requested_scale": self.requested.as_ref().map(|key| key.scale),
            "desired_scale": self.desired.as_ref().map(|key| key.scale),
            "active_scale": self.active_scale,
            "resident_count": self.residents.len(),
            "requests": self.render_requests,
            "admissions": self.render_admissions,
            "settling": self.settling,
        })
    }
    #[cfg(test)]
    pub(super) fn requested_page(&self) -> Option<usize> {
        self.goto
    }

    pub(super) fn reveal_position(&mut self, page: usize, x: f32, y: f32, explicit: bool) {
        self.reveal_position_for_pdf(page, x, y, explicit, self.bytes.clone());
    }

    pub(super) fn reveal_position_for_pdf(
        &mut self,
        page: usize,
        x: f32,
        y: f32,
        explicit: bool,
        expected_pdf: Option<Arc<[u8]>>,
    ) {
        self.clear_reveal();
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.go_to_position(page, y);
        self.position_point = explicit.then_some(egui::vec2(x, y));
        self.position_pdf = expected_pdf.clone();
        if explicit {
            self.reveal = Some(PdfReveal {
                page,
                point: egui::vec2(x, y),
                expected_pdf,
                attention: EditorAttention::arm(0),
            });
        }
    }

    pub(super) fn snapshot_reveal(&mut self, page: usize, x: f32, y: f32, elapsed: Duration) {
        self.reveal_position(page, x, y, true);
        if let Some(reveal) = self.reveal.as_mut() {
            reveal.attention = EditorAttention::snapshot(0, elapsed);
        }
    }

    pub(super) fn clear_reveal(&mut self) {
        self.reveal = None;
        self.pending_position = None;
        self.position_pdf = None;
        self.position_point = None;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            self.reveal_observation = None;
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspected_reveal(&self) -> serde_json::Value {
        self.reveal_observation.clone().unwrap_or_else(|| {
            serde_json::json!({
                "active": false, "pending": self.reveal.is_some(),
            })
        })
    }

    pub(super) fn go_to_position(&mut self, page: usize, y: f32) {
        self.clear_reveal();
        // The new map may arrive before its PDF has been inspected. Resolve
        // point coordinates only against that PDF's admitted page dimensions.
        self.pending_position = Some((page, y));
        self.position_point = None;
        self.position_pdf = None;
    }
    fn apply_pending_position(&mut self) {
        if self.active_revision != self.revision
            || self.position_pdf.as_ref().is_some_and(|expected| {
                self.bytes
                    .as_ref()
                    .is_none_or(|bytes| !Arc::ptr_eq(expected, bytes) && **expected != **bytes)
            })
        {
            return;
        }
        if let Some((page, y)) = self.pending_position.take() {
            self.position_pdf = None;
            if let Some(size) = self.catalog.as_ref().and_then(|c| c.sizes.get(page)) {
                push_history(&mut self.back, self.anchor);
                self.forward.clear();
                self.anchor = (page, (y / size[1]).clamp(0.0, 1.0));
                self.goto = None;
                self.restore_anchor = true;
            } else {
                self.goto = Some(page);
            }
        }
    }
    pub(super) fn go_to_page(&mut self, page: usize) {
        self.clear_reveal();
        self.goto = Some(page);
    }

    pub(super) fn zoom(&mut self, action: PreviewZoomAction) {
        match action {
            PreviewZoomAction::Reset => self.fit = true,
            PreviewZoomAction::In => {
                self.zoom = (self.zoom * 1.2).min(8.0);
                self.fit = false;
            }
            PreviewZoomAction::Out => {
                self.zoom = (self.zoom / 1.2).max(0.15);
                self.fit = false;
            }
        }
        self.restore_anchor = true;
    }

    fn accept(&mut self, batch: Batch, key: &RequestKey, ui: &egui::Ui) {
        let changed = self.active_revision != key.revision;
        if changed || self.active_scale != key.scale {
            self.residents.clear();
        }
        if changed {
            self.selection = None;
            self.restore_anchor = true;
            self.layout_zoom = 0.0;
        }
        for page in batch.pages {
            let texture = ui.ctx().load_texture(
                format!("pdfium-{}-{}", key.revision, page.index),
                egui::ImageData::Color(page.image),
                egui::TextureOptions::LINEAR,
            );
            self.residents.insert(
                page.index,
                Resident {
                    texture,
                    characters: page.characters,
                    links: page.links,
                    search_matches: SearchMatches::default(),
                },
            );
        }
        // A batch is the entire demanded region; this bounds retained GPU memory.
        self.residents.retain(|index, _| {
            key.pages.contains(index)
                || *index == batch.catalog.sizes.len() - 1
                    && key.pages.iter().any(|p| *p >= batch.catalog.sizes.len())
        });
        self.catalog = Some(batch.catalog);
        self.active_revision = key.revision;
        self.active_scale = key.scale;
        self.palette = key.palette;
        self.error = None;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            self.render_admissions += 1;
        }
    }

    fn prepare_path(&mut self, path: PathBuf, bytes: &Arc<[u8]>) {
        if self.bytes.is_some() && self.path != path {
            // A source jump may open a previously hidden preview. Preserve only
            // navigation bound to the incoming PDF; retire the former document.
            let matches = self
                .position_pdf
                .as_ref()
                .is_some_and(|expected| Arc::ptr_eq(expected, bytes) || **expected == **bytes);
            let position = matches.then(|| self.pending_position.take()).flatten();
            let position_pdf = matches.then(|| self.position_pdf.take()).flatten();
            let reveal = matches.then(|| self.reveal.take()).flatten();
            let position_point = matches.then_some(self.position_point).flatten();
            *self = Self {
                pending_position: position,
                position_pdf,
                reveal,
                position_point,
                ..Default::default()
            };
        }
        self.path = path;
    }

    fn update_reveal_options(
        &mut self,
        bytes: &Arc<[u8]>,
        now: Instant,
        animation: crate::settings::RevealAnimation,
    ) {
        let Some(reveal) = &self.reveal else {
            return;
        };
        if !animation.enabled
            || reveal
                .attention
                .animation
                .is_some_and(|previous| previous != animation.normalized())
        {
            // Effects preferences control feedback, never the pending SyncTeX jump.
            self.reveal = None;
        } else if reveal.obsolete(bytes, now, animation) {
            self.clear_reveal();
        }
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        path: PathBuf,
        bytes: Arc<[u8]>,
        palette: (Color32, Color32),
        can_reveal_source: bool,
        animation: crate::settings::RevealAnimation,
    ) -> Option<String> {
        self.prepare_path(path, &bytes);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            self.reveal_observation = None;
        }
        self.update_reveal_options(&bytes, Instant::now(), animation);
        if self
            .bytes
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &bytes) && **old != *bytes)
        {
            self.revision += 1;
            self.bytes = Some(bytes.clone());
            self.requested = None;
            self.desired = None;
            self.pending_reply = None;
            self.error = None;
            self.search_pages.clear();
            self.context_location = None;
        }
        // Adopt equal-byte Arcs so repeated frames take the pointer fast path.
        self.bytes = Some(bytes);
        if self.worker.is_none() {
            self.worker = Some(Worker::start(crate::worker::RepaintTarget::current(
                ui.ctx(),
            )));
        }
        let (reply, search) = self.worker.as_ref().unwrap().poll();
        if let Some(reply) = reply
            && self.requested.as_ref() == Some(&reply.key)
        {
            self.pending_reply = Some(reply);
        }
        // Catalog metadata is small and needed to calculate the current fit
        // scale. Pixels wait until AFTER layout, so an old width never uploads.
        if let Some(Ok(batch)) = self.pending_reply.as_ref().map(|reply| &reply.result)
            && self
                .catalog
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &batch.catalog))
        {
            self.catalog = Some(batch.catalog.clone());
            self.layout_zoom = 0.0;
            self.restore_anchor = true;
        }
        self.apply_pending_position();
        if let Some((key, pages)) = search
            && key.revision == self.revision
            && key.query == self.query.as_ref()
        {
            self.search_pages = pages;
            self.search_pending = false;
            if std::mem::take(&mut self.find_when_ready) {
                self.find_next();
            }
        }
        // Stale pixels may remain visible while a replacement PDF is being
        // rendered. Their coordinates must never be queried against its map.
        let can_reveal_source = can_reveal_source && self.active_revision == self.revision;
        if !can_reveal_source {
            self.context_location = None;
        }
        let mut link = None;
        let available = ui.available_rect_before_wrap();
        let mut demand = vec![self.page];
        if let Some(catalog) = self.catalog.clone() {
            let widest = catalog.sizes.iter().map(|s| s[0]).fold(1.0f32, f32::max);
            let old_zoom = self.zoom;
            if self.fit {
                self.zoom = fit_width_zoom(available.width(), widest);
                if self.zoom != old_zoom {
                    self.restore_anchor = true;
                }
            }
            let pinch = ui.ctx().input(|i| i.zoom_delta());
            let pointer = ui.ctx().pointer_latest_pos();
            let scroll_id = ui.make_persistent_id("pdfium-scroll");
            if self.scroll_owner != Some(scroll_id) {
                self.scroll_owner = Some(scroll_id);
                self.restore_anchor = true;
            }
            if pointer.is_some_and(|p| available.contains(p)) && (pinch - 1.0).abs() > 0.001 {
                self.fit = false;
                self.zoom = (self.zoom * pinch).clamp(0.15, 8.0);
                let mut state =
                    egui::scroll_area::State::load(ui.ctx(), scroll_id).unwrap_or_default();
                state.offset = zoom_anchored_offset(
                    state.offset,
                    pointer.unwrap() - available.min,
                    old_zoom,
                    self.zoom,
                );
                state.store(ui.ctx(), scroll_id);
                // Pointer anchoring already supplies the new offset; a page
                // anchor from a prior layout must not overwrite that gesture.
                self.restore_anchor = false;
            }
            if self.layout_zoom != self.zoom {
                self.tops = page_tops(&catalog.sizes, self.zoom);
                self.height = self.tops.last().copied().unwrap_or(0.0)
                    + catalog.sizes.last().unwrap()[1] * self.zoom
                    + 12.0;
                self.layout_zoom = self.zoom;
            }
            let width = (widest * self.zoom).max(available.width());
            let mut scroll = egui::ScrollArea::both()
                .id_salt("pdfium-scroll")
                .auto_shrink([false, false]);
            if let Some(page) = self.goto.take() {
                scroll = scroll.vertical_scroll_offset(self.tops[page.min(self.tops.len() - 1)]);
                self.position_point = None;
                self.restore_anchor = false;
            } else if self.restore_anchor {
                let (page, fraction) = self.anchor;
                let page = page.min(catalog.sizes.len() - 1);
                let target_y = self.tops[page] + fraction * catalog.sizes[page][1] * self.zoom;
                let offset = if let Some(point) = self.position_point.take() {
                    let page_width = catalog.sizes[page][0] * self.zoom;
                    let target =
                        Pos2::new((width - page_width) / 2.0 + point.x * self.zoom, target_y);
                    let offset = reveal_scroll_offset(target, available.size());
                    scroll = scroll.horizontal_scroll_offset(offset.x);
                    offset.y
                } else {
                    target_y
                };
                scroll = scroll.vertical_scroll_offset(offset);
                self.restore_anchor = false;
            }
            let interaction_bounds =
                super::panel_handles::preview_interaction_bounds(ui, available);
            scroll.show_viewport(ui, |ui, viewport| {
                ui.set_min_size(egui::vec2(width, self.height));
                let range = visible_pages(
                    &self.tops,
                    &catalog.sizes,
                    self.zoom,
                    viewport.top(),
                    viewport.bottom(),
                );
                demand = range.clone().collect();
                // The first intersecting page can be only a sliver, especially
                // when the last page cannot scroll all the way to the top.
                // Keep the scroll anchor separate from the displayed page.
                self.page = most_visible_page(
                    range.clone(),
                    &self.tops,
                    &catalog.sizes,
                    self.zoom,
                    viewport.top(),
                    viewport.bottom(),
                );
                self.anchor = (
                    range.start,
                    (viewport.top() - self.tops[range.start])
                        / (catalog.sizes[range.start][1] * self.zoom),
                );
                for index in range {
                    let size =
                        egui::vec2(catalog.sizes[index][0], catalog.sizes[index][1]) * self.zoom;
                    let rect = Rect::from_min_size(
                        ui.min_rect().min + egui::vec2((width - size.x) / 2.0, self.tops[index]),
                        size,
                    );
                    ui.painter().rect_filled(rect, 0.0, palette.0);
                    let Some(resident) = self.residents.get_mut(&index) else {
                        continue;
                    };
                    ui.painter().image(
                        resident.texture.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    let clip = ui.clip_rect();
                    ui.set_clip_rect(clip.intersect(interaction_bounds));
                    let response = ui.interact(
                        rect,
                        ui.id().with(("pdfium-page", index)),
                        Sense::click_and_drag(),
                    );
                    ui.set_clip_rect(clip);
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    if index == self.page {
                        crate::desktop_test::observe("preview.page", &response);
                    }
                    if response.has_focus() || response.clicked() || response.drag_started() {
                        self.page_focus = Some(response.id);
                    }
                    let map = |normalized: Rect| {
                        Rect::from_min_max(
                            rect.min + normalized.min.to_vec2() * rect.size(),
                            rect.min + normalized.max.to_vec2() * rect.size(),
                        )
                    };
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    if can_reveal_source && index == self.page {
                        let visible = clip.intersect(interaction_bounds);
                        if let Some(bounds) = resident
                            .characters
                            .iter()
                            .filter(|character| {
                                !character.value.is_whitespace() && character.rect.is_finite()
                            })
                            .map(|character| map(character.rect))
                            .find(|bounds| {
                                bounds.is_positive() && visible.contains(bounds.center())
                            })
                        {
                            let mut ink = response.clone();
                            ink.interact_rect = bounds.intersect(visible);
                            crate::desktop_test::observe("preview.source_ink", &ink);
                        }
                    }
                    if let Some(pointer) = response.interact_pointer_pos() {
                        if can_reveal_source
                            && response.clicked()
                            && ui.input(|i| i.modifiers.command)
                        {
                            let point = (pointer - rect.min) / self.zoom;
                            let id = viewport_scoped_id(ui.ctx(), "pdf-synctex");
                            ui.ctx()
                                .data_mut(|data| data.insert_temp(id, (index, point.x, point.y)));
                        }
                        if response.drag_started()
                            && let Some(char_index) = nearest_character(
                                &resident.characters,
                                (pointer - rect.min) / rect.size(),
                            )
                        {
                            self.selection = Some((index, char_index, char_index));
                            response.request_focus();
                        }
                        if response.dragged()
                            && let Some((page, _, end)) = &mut self.selection
                            && *page == index
                            && let Some(char_index) = nearest_character(
                                &resident.characters,
                                (pointer - rect.min) / rect.size(),
                            )
                        {
                            *end = char_index;
                        }
                        if response.clicked() {
                            response.request_focus();
                            self.selection = None;
                            for (bounds, target) in &resident.links {
                                if map(*bounds).contains(pointer) {
                                    match target {
                                        LinkTarget::Page(page) => {
                                            self.reveal = None;
                                            self.pending_position = None;
                                            self.position_pdf = None;
                                            self.position_point = None;
                                            push_history(&mut self.back, self.anchor);
                                            self.forward.clear();
                                            self.goto = Some(*page);
                                        }
                                        LinkTarget::Url(url) => link = Some(url.clone()),
                                    }
                                }
                            }
                        }
                    }
                    for range in resident.search_matches.for_query(&resident.characters, &self.query) {
                        for character in &resident.characters[range.clone()] {
                            if character.rect.is_finite() {
                                ui.painter().rect_filled(
                                    map(character.rect),
                                    0.0,
                                    Color32::from_rgba_unmultiplied(240, 190, 30, 85),
                                );
                            }
                        }
                    }
                    let selected = self
                        .selection
                        .filter(|(page, _, _)| *page == index)
                        .map(|(_, a, b)| a.min(b)..=a.max(b));
                    if let Some(selected) = selected.clone() {
                        for character in &resident.characters[selected] {
                            if character.rect.is_finite() {
                                ui.painter().rect_filled(
                                    map(character.rect),
                                    0.0,
                                    Color32::from_rgba_unmultiplied(60, 130, 230, 75),
                                );
                            }
                        }
                    }
                    if self.active_revision == self.revision
                        && let Some(reveal) = self.reveal.as_mut().filter(|r| r.page == index
                            && r.expected_pdf.as_ref().is_none_or(|expected| self.bytes.as_ref()
                                .is_some_and(|bytes| Arc::ptr_eq(expected, bytes) || **expected == **bytes)))
                    {
                        let point = rect.min + reveal.point * self.zoom;
                        let target = Rect::from_center_size(point, egui::vec2(2.0, 18.0));
                        let visible = clip.intersect(interaction_bounds);
                        let active = reveal.attention.paint(ui, visible, target, animation);
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if active && visible.contains(point)
                            && let Some(elapsed) = reveal.attention.fixed_elapsed.or_else(||
                                reveal.attention.started.map(|start| start.elapsed()))
                        {
                            self.reveal_observation = Some(serde_json::json!({
                                "active": true, "pending": false, "page": index,
                                "center": [point.x,point.y],
                                "target": [target.left(),target.top(),target.right(),target.bottom()],
                                "viewport": [visible.left(),visible.top(),visible.right(),visible.bottom()],
                                "elapsed_ms": elapsed.as_millis() as u64,
                            }));
                        }
                        if !active { self.reveal = None; }
                    }
                    if response.secondary_clicked() {
                        self.context_location = can_reveal_source
                            .then(|| {
                                PageContextLocation::capture(
                                    &response,
                                    self.revision,
                                    index,
                                    rect,
                                    self.zoom,
                                )
                            })
                            .flatten();
                    }
                    page_context_menu(
                        &response,
                        self.context_location
                            .and_then(|location| location.current(self.revision, index)),
                        &resident.characters,
                    );
                    if response.has_focus()
                        && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, egui::Key::C))
                        && let Some(selected) = selected
                    {
                        ui.ctx().copy_text(
                            resident
                                .characters
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| selected.contains(i))
                                .map(|(_, c)| c.value)
                                .collect(),
                        );
                    }
                }
            });
        } else {
            show_centered_preview_message(ui, "Preparing PDF preview…", self.error.is_none());
        }

        if let Some(reveal) = &self.reveal
            && reveal.attention.fixed_elapsed.is_none()
        {
            let remaining = reveal.attention.started.map_or_else(
                || Duration::from_secs(2).saturating_sub(reveal.attention.armed_at.elapsed()),
                |started| {
                    Duration::from_millis(u64::from(animation.normalized().duration_ms))
                        .saturating_sub(started.elapsed())
                },
            );
            ui.ctx().request_repaint_after(remaining);
        }
        let key = RequestKey {
            revision: self.revision,
            pages: demand,
            scale: (self.zoom * ui.ctx().pixels_per_point() * 100.0)
                .round()
                .max(1.0) as u32,
            query: self.query.to_string(),
            palette,
        };
        self.finish_frame(
            ui,
            key,
            available,
            super::panel_handles::main_panel_resizing(ui.ctx()),
        );
        link
    }

    fn finish_frame(&mut self, ui: &egui::Ui, key: RequestKey, available: Rect, resizing: bool) {
        let now = ui.input(|input| input.time);
        let physical = available.size() * ui.ctx().pixels_per_point();
        self.settle.observe(
            Geometry {
                size: [
                    physical.x.round().max(0.0) as u32,
                    physical.y.round().max(0.0) as u32,
                ],
                scale: key.scale,
            },
            resizing,
            now,
        );
        // New documents, palette changes and newly visible missing pages cannot
        // wait behind a gesture. Otherwise keep the exact same GPU textures.
        let deferred = self.reusable(&key) && self.settle.deferred(now);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            self.settling = deferred;
        }
        if deferred {
            if let Some(delay) = self.settle.wake_after(now) {
                ui.ctx().request_repaint_after(delay);
            }
        } else if let Some(reply) = self.pending_reply.take()
            && reply.key == key
        {
            match reply.result {
                Ok(batch) => {
                    self.accept(batch, &reply.key, ui);
                    self.apply_pending_position();
                    // This frame painted the retained image. Present its sharp
                    // replacement on the next ordinary frame.
                    ui.ctx().request_repaint();
                }
                Err(error) => self.error = Some(error),
            }
        }
        if !deferred && self.requested.as_ref() != Some(&key) {
            self.worker.as_ref().unwrap().request(Request {
                key: key.clone(),
                bytes: self.bytes.as_ref().unwrap().clone(),
            });
            self.requested = Some(key.clone());
            self.search_pending = !self.query.is_empty();
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            {
                self.render_requests += 1;
            }
        }
        self.desired = Some(key);
    }

    fn find_next(&mut self) {
        if let Some(target) = self
            .search_pages
            .iter()
            .copied()
            .find(|p| *p > self.page)
            .or_else(|| self.search_pages.first().copied())
        {
            self.navigate(target);
        }
    }

    fn navigate(&mut self, page: usize) {
        self.clear_reveal();
        push_history(&mut self.back, self.anchor);
        self.forward.clear();
        self.goto = Some(page);
    }

    pub(super) fn controls_snapshot(&self) -> super::preview_controls::Snapshot {
        super::preview_controls::Snapshot {
            focused: false,
            page: self.page,
            count: self.catalog.as_ref().map_or(0, |c| c.sizes.len()),
            zoom: self.zoom,
            back: !self.back.is_empty(),
            forward: !self.forward.is_empty(),
            outline: self
                .catalog
                .as_ref()
                .map_or_else(Vec::new, |c| c.outline.clone()),
        }
    }
    pub(super) fn controls_action(&mut self, action: super::preview_controls::Action) {
        use super::preview_controls::Action;
        match action {
            Action::Back => {
                self.clear_reveal();
                if let Some(target) = self.back.pop() {
                    push_history(&mut self.forward, self.anchor);
                    self.anchor = target;
                    self.restore_anchor = true;
                }
            }
            Action::Forward => {
                self.clear_reveal();
                if let Some(target) = self.forward.pop() {
                    push_history(&mut self.back, self.anchor);
                    self.anchor = target;
                    self.restore_anchor = true;
                }
            }
            Action::Page(page) => self.navigate(page),
            Action::ZoomIn => self.zoom(PreviewZoomAction::In),
            Action::ZoomOut => self.zoom(PreviewZoomAction::Out),
            Action::Fit => self.zoom(PreviewZoomAction::Reset),
            Action::Outline(index) => {
                if let Some((_, page)) = self.catalog.as_ref().and_then(|c| c.outline.get(index)) {
                    self.navigate(*page);
                }
            }
            Action::Find(query) => {
                if self.query.as_ref() != query {
                    self.query = query.into();
                    self.search_pages.clear();
                    self.search_pending = !self.query.is_empty();
                    self.find_when_ready = self.search_pending;
                } else if self.search_pending {
                    self.find_when_ready = true;
                } else {
                    self.find_next();
                }
            }
            Action::Location(location) => {
                self.reveal_position(location.page, location.x, location.y, true)
            }
            Action::PopOut => {}
        }
    }
}

fn fit_width_zoom(available_width: f32, page_width: f32) -> f32 {
    (available_width / page_width).clamp(0.15, 8.0)
}

fn push_history(history: &mut Vec<(usize, f32)>, position: (usize, f32)) {
    if history.len() == 256 {
        history.remove(0);
    }
    history.push(position);
}

fn page_tops(sizes: &[[f32; 2]], zoom: f32) -> Vec<f32> {
    let mut top = 12.0;
    sizes
        .iter()
        .map(|s| {
            let result = top;
            top += s[1] * zoom + 12.0;
            result
        })
        .collect()
}

fn visible_pages(
    tops: &[f32],
    sizes: &[[f32; 2]],
    zoom: f32,
    top: f32,
    bottom: f32,
) -> std::ops::Range<usize> {
    let first = tops
        .partition_point(|p| *p < top)
        .saturating_sub(1)
        .min(tops.len() - 1);
    let first = if tops[first] + sizes[first][1] * zoom < top {
        (first + 1).min(tops.len() - 1)
    } else {
        first
    };
    first
        ..tops
            .partition_point(|p| *p <= bottom)
            .max(first + 1)
            .min(tops.len())
}

fn most_visible_page(
    range: std::ops::Range<usize>,
    tops: &[f32],
    sizes: &[[f32; 2]],
    zoom: f32,
    top: f32,
    bottom: f32,
) -> usize {
    let mut page = range.start;
    let mut largest = 0.0;
    for index in range {
        let visible =
            (bottom.min(tops[index] + sizes[index][1] * zoom) - top.max(tops[index])).max(0.0);
        if visible > largest {
            page = index;
            largest = visible;
        }
    }
    page
}

fn nearest_character(chars: &[Character], point: Vec2) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .filter(|(_, c)| c.rect.is_finite())
        .min_by(|(_, a), (_, b)| {
            a.rect
                .distance_to_pos(point.to_pos2())
                .total_cmp(&b.rect.distance_to_pos(point.to_pos2()))
        })
        .map(|(index, _)| index)
}

fn matching_characters(chars: &[Character], query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut text = String::new();
    let mut indices = Vec::new();
    for (index, c) in chars.iter().enumerate() {
        for lower in c.value.to_lowercase() {
            text.push(lower);
            indices.extend(std::iter::repeat_n(index, lower.len_utf8()));
        }
    }
    text.match_indices(&query.to_lowercase())
        .map(|(offset, matched)| indices[offset]..indices[offset + matched.len() - 1] + 1)
        .collect()
}

impl EditorApp {
    pub(super) fn clear_pdf_views(&mut self) {
        self.pdfium_preview = Default::default();
        self.pdfium_asset = Default::default();
    }

    pub(super) fn pdfium_preview_requested(&self) -> bool {
        self.preview_status_snapshot().effective_backend == crate::preview::PreviewBackend::Pdfium
    }
    pub(super) fn pdfium_asset_requested(&self) -> bool {
        self.document().kind() == DocumentKind::Pdf
    }
    pub(super) fn pdfium_source_jump_available(
        &self,
        asset: bool,
        path: &Path,
        bytes: &Arc<[u8]>,
    ) -> bool {
        !asset
            && self.preview_document_kind() == DocumentKind::Tex
            && self.tex_tools.synctex.is_some()
            && self.synctex.artifact.as_ref().is_some_and(|artifact| {
                artifact.path == path
                    && Some(artifact.key) == self.preview.content.artifact_key()
                    && (Arc::ptr_eq(&artifact.pdf, bytes)
                        || artifact.pdf.as_ref() == bytes.as_ref())
            })
    }

    pub(super) fn show_pdfium_view(&mut self, ui: &mut egui::Ui, asset: bool) {
        let viewport = ui.available_rect_before_wrap();
        let target = if asset {
            let key = self.document().key();
            self.preview_controls.asset_available = Some((key, viewport));
            preview_controls::Target::Asset(key)
        } else {
            self.preview_controls.available = Some(viewport);
            preview_controls::Target::Source
        };
        let path = if asset {
            self.document().path().clone().unwrap_or_default()
        } else {
            self.preview_document_path()
        };
        let preview = if asset {
            &self.asset_preview
        } else {
            &self.preview
        };
        let Some(bytes) = preview.content.pdf().cloned() else {
            show_centered_preview_message(
                ui,
                if self.compilation_paused {
                    "Preview paused"
                } else {
                    "Waiting for a PDF…"
                },
                !self.compilation_paused
                    && preview.status != PreviewStatus::Error
                    && self.compile_indicator_spinning(),
            );
            return;
        };
        let palette = self.preview_palette(ui.ctx(), preview.render_dark());
        let can_reveal_source = self.pdfium_source_jump_available(asset, &path, &bytes);
        let source_popped_out = self.preview_source_popped_out();
        let animation = self.settings.preview_reveal_animation;
        let view = if asset {
            &mut self.pdfium_asset
        } else {
            &mut self.pdfium_preview
        };
        if std::mem::take(&mut view.controls_open) {
            self.preview_controls.select(target);
            self.preview_controls.open = true;
            self.preview_controls.focus_find = true;
        }
        if let Some(link) = ui
            .push_id(
                if asset {
                    "pdfium-asset"
                } else {
                    "pdfium-preview"
                },
                |ui| view.show(ui, path, bytes, palette, can_reveal_source, animation),
            )
            .inner
            && crate::pdfium::safe_url(&link)
        {
            let _ = self.web_link_sender.send(link);
        }
        if view.has_focus(ui.ctx()) {
            self.explorer.set_focused_section(None);
        }
        if let Some(stroke) = theme::panel_interaction_stroke(
            ui.visuals(),
            ui.rect_contains_pointer(viewport),
            view.has_focus(ui.ctx()),
        ) {
            ui.painter()
                .rect_stroke(viewport, 0.0, stroke, egui::StrokeKind::Inside);
        }
        // egui clears widget focus at the beginning of an Escape frame.
        // Consult the previous owner as well, before routing Escape to source.
        if !asset
            && ui.input(|i| i.focused)
            && view
                .page_focus
                .is_some_and(|id| ui.memory(|m| m.has_focus(id) || m.had_focus_last_frame(id)))
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, egui::Key::Escape))
        {
            // Resolve the source editor in its document callback, including
            // when this viewer is hosted in the detached preview window.
            self.preview_controls.return_to_editor = true;
            let owner = if source_popped_out {
                ui.ctx().parent_viewport_id()
            } else {
                ui.ctx().viewport_id()
            };
            ui.ctx().request_repaint_of(owner);
        }
        let id = viewport_scoped_id(ui.ctx(), "pdf-synctex");
        let location = ui
            .ctx()
            .data_mut(|data| data.remove_temp::<(usize, f32, f32)>(id));
        if can_reveal_source && let Some((page, x, y)) = location {
            self.request_synctex(crate::synctex::Query::Page { page, x, y }, false);
        }
    }
}

#[cfg(test)]
#[path = "pdfium_view_perf.rs"]
mod perf;

#[cfg(test)]
#[path = "pdfium_resize_tests.rs"]
mod resize_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_jump_requires_the_matching_compiled_tex_map_and_binary() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.tex");
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_loaded_unprojected(
            "Text".into(),
            path.clone(),
            DocumentKind::Tex,
            None,
        );
        let bytes: Arc<[u8]> = Arc::from(&b"compiled PDF"[..]);
        let key = ArtifactKey {
            revision: 3,
            generation: 5,
        };
        app.preview.accept_artifact(key, bytes.clone());
        app.tex_tools.synctex = Some(directory.path().join("synctex"));
        assert!(!app.pdfium_source_jump_available(false, &path, &bytes));
        app.synctex.artifact = Some(super::super::synctex::VersionedMap {
            path: path.clone(),
            key,
            map: Arc::new(crate::synctex::Artifact {
                data: Arc::from(&b"source map"[..]),
                compressed: false,
                root: directory.path().into(),
                mirror: directory.path().join("mirror"),
            }),
            pdf: bytes.clone(),
        });
        assert!(app.pdfium_source_jump_available(false, &path, &bytes));
        assert!(app.pdfium_source_jump_available(false, &path, &Arc::from(bytes.as_ref())));
        assert!(!app.pdfium_source_jump_available(true, &path, &bytes));
        assert!(!app.pdfium_source_jump_available(false, &path.with_extension("pdf"), &bytes));
        assert!(!app.pdfium_source_jump_available(false, &path, &Arc::from(&b"old PDF"[..])));
        app.tex_tools.synctex = None;
        assert!(!app.pdfium_source_jump_available(false, &path, &bytes));
        app.tex_tools.synctex = Some(directory.path().join("synctex"));
        app.preview.accept_artifact(
            ArtifactKey {
                generation: 6,
                ..key
            },
            bytes.clone(),
        );
        assert!(!app.pdfium_source_jump_available(false, &path, &bytes));
        app.preview.accept_artifact(key, bytes.clone());
        app.document_mut().replace_loaded_unprojected(
            "Text".into(),
            path.with_extension("typ"),
            DocumentKind::Typst,
            None,
        );
        assert!(!app.pdfium_source_jump_available(false, &path, &bytes));
    }

    #[test]
    fn page_context_locations_reject_other_pages_and_replaced_pdfs() {
        let location = PageContextLocation {
            revision: 7,
            page: 2,
            point: egui::vec2(20.0, 30.0),
        };
        assert_eq!(location.current(7, 2), Some(location));
        assert_eq!(location.current(8, 2), None);
        assert_eq!(location.current(7, 3), None);
    }

    #[test]
    fn pending_pdf_or_lost_source_capability_flushes_the_context_location() {
        let context = egui::Context::default();
        for (active_revision, allowed) in [(1, true), (2, false)] {
            let bytes: Arc<[u8]> = Arc::from(&b"prepared PDF"[..]);
            let path = PathBuf::from("source.tex");
            let mut view = PdfiumView {
                worker: Some(Worker::idle_for_tests()),
                path: path.clone(),
                bytes: Some(bytes.clone()),
                revision: 2,
                active_revision,
                context_location: Some(PageContextLocation {
                    revision: active_revision,
                    page: 0,
                    point: egui::vec2(20.0, 30.0),
                }),
                ..Default::default()
            };
            context
                .run_ui(Default::default(), |ui| {
                    view.show(
                        ui,
                        path.clone(),
                        bytes.clone(),
                        (Color32::WHITE, Color32::BLACK),
                        allowed,
                        crate::settings::RevealAnimation::default(),
                    );
                })
                .drop_without_applying_deltas();
            assert!(view.context_location.is_none());
        }
    }

    #[test]
    fn reveal_in_source_uses_the_original_right_click_after_the_pointer_moves() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        struct Menu {
            location: Option<PageContextLocation>,
            page: Rect,
        }
        let mut harness = Harness::builder()
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(30)
            .with_size(egui::vec2(600.0, 500.0))
            .build_ui_state(
                |ui, state: &mut Menu| {
                    let response =
                        ui.allocate_response(egui::vec2(400.0, 360.0), Sense::click_and_drag());
                    state.page = response.rect;
                    if response.secondary_clicked() {
                        state.location =
                            PageContextLocation::capture(&response, 7, 2, response.rect, 2.0);
                    }
                    page_context_menu(
                        &response,
                        state.location.and_then(|location| location.current(7, 2)),
                        &[],
                    );
                },
                Menu {
                    location: None,
                    page: Rect::NOTHING,
                },
            );
        harness.run();
        let point = harness.state().page.min + egui::vec2(80.0, 120.0);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerMoved(point));
            harness.event(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run();
        }
        assert_eq!(
            harness.state().location.unwrap().point,
            egui::vec2(40.0, 60.0)
        );
        let _ = harness.get_by_label("Copy page text");
        let row = harness.get_by_label("Reveal in Source").rect().center();
        assert_ne!(row, point);
        harness.event(egui::Event::PointerMoved(row));
        harness.run();
        harness.get_by_label("Reveal in Source").click();
        harness.run();
        let id = viewport_scoped_id(&harness.ctx, "pdf-synctex");
        assert_eq!(
            harness
                .ctx
                .data_mut(|data| data.remove_temp::<(usize, f32, f32)>(id)),
            Some((2, 40.0, 60.0)),
        );
    }

    #[test]
    fn page_context_menu_without_a_source_map_still_allows_copy() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness = Harness::builder()
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(30)
            .with_size(egui::vec2(600.0, 500.0))
            .build_ui(|ui| {
                let response =
                    ui.allocate_response(egui::vec2(400.0, 360.0), Sense::click_and_drag());
                page_context_menu(&response, None, &[]);
            });
        harness.run();
        let point = egui::pos2(80.0, 120.0);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerMoved(point));
            harness.event(egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run();
        }
        let _ = harness.get_by_label("Copy page text");
        assert!(harness.query_by_label("Reveal in Source").is_none());
    }

    #[test]
    fn source_position_waits_for_the_new_pdfs_dimensions() {
        let mut view = PdfiumView {
            revision: 2,
            active_revision: 1,
            catalog: Some(Arc::new(Catalog {
                sizes: vec![[400.0, 100.0]],
                outline: vec![],
            })),
            ..Default::default()
        };
        view.go_to_position(0, 50.0);
        view.apply_pending_position();
        assert!(view.pending_position.is_some());
        assert_eq!(view.anchor, (0, 0.0));
        view.active_revision = 2;
        view.catalog = Some(Arc::new(Catalog {
            sizes: vec![[400.0, 200.0]],
            outline: vec![],
        }));
        view.apply_pending_position();
        assert!(view.pending_position.is_none());
        assert_eq!(view.anchor, (0, 0.25));
        assert!(view.restore_anchor);
    }

    #[test]
    fn explicit_pdf_reveal_keeps_both_coordinates_and_automatic_follow_is_silent() {
        let mut view = PdfiumView::default();
        view.reveal_position(2, 125.0, 360.0, true);
        let reveal = view.reveal.as_ref().unwrap();
        assert_eq!(reveal.page, 2);
        assert_eq!(reveal.point, egui::vec2(125.0, 360.0));
        assert_eq!(view.position_point, Some(reveal.point));
        assert!(reveal.attention.started.is_none());
        view.reveal_position(2, 200.0, 400.0, false);
        assert!(view.reveal.is_none());
        assert!(view.position_point.is_none());
        assert_eq!(view.pending_position, Some((2, 400.0)));
        view.reveal_position(0, f32::NAN, 10.0, true);
        assert!(view.reveal.is_none());
    }

    #[test]
    fn pending_synctex_position_waits_for_its_exact_pdf_before_using_dimensions() {
        let expected: Arc<[u8]> = Arc::from([2u8]);
        let mut view = PdfiumView {
            bytes: Some(Arc::from([1u8])),
            revision: 1,
            active_revision: 1,
            catalog: Some(Arc::new(Catalog {
                sizes: vec![[400.0, 100.0]],
                outline: vec![],
            })),
            ..Default::default()
        };
        for explicit in [true, false] {
            view.reveal_position_for_pdf(0, 80.0, 50.0, explicit, Some(expected.clone()));
            view.apply_pending_position();
            assert_eq!(view.pending_position, Some((0, 50.0)));
            assert_eq!(view.anchor, (0, 0.0));
        }
        view.bytes = Some(expected);
        view.catalog = Some(Arc::new(Catalog {
            sizes: vec![[400.0, 200.0]],
            outline: vec![],
        }));
        view.apply_pending_position();
        assert_eq!(view.anchor, (0, 0.25));
        assert!(view.position_pdf.is_none());
    }

    #[test]
    fn pdf_beacons_expire_without_their_page_being_painted_and_frozen_targets_cannot_cross_pdfs() {
        let bytes: Arc<[u8]> = Arc::from([1u8]);
        let other: Arc<[u8]> = Arc::from([2u8]);
        let now = Instant::now();
        let mut reveal = PdfReveal {
            page: 0,
            point: Vec2::ZERO,
            expected_pdf: Some(bytes.clone()),
            attention: EditorAttention::arm(0),
        };
        assert!(
            !reveal.obsolete(&other, now, crate::settings::RevealAnimation::default()),
            "wait for the intended PDF to arrive"
        );
        reveal.attention.started = Some(now);
        assert!(!reveal.obsolete(
            &bytes,
            now + Duration::from_millis(899),
            crate::settings::RevealAnimation::default()
        ));
        assert!(reveal.obsolete(
            &bytes,
            now + super::super::reveal::DURATION,
            crate::settings::RevealAnimation::default()
        ));
        assert!(reveal.obsolete(&other, now, crate::settings::RevealAnimation::default()));
        reveal.attention = EditorAttention::snapshot(0, Duration::from_millis(300));
        assert!(!reveal.obsolete(
            &bytes,
            now + Duration::from_secs(10),
            crate::settings::RevealAnimation::default()
        ));
        assert!(reveal.obsolete(&other, now, crate::settings::RevealAnimation::default()));
    }

    #[test]
    fn reveal_scroll_offsets_keep_the_destination_visible_at_page_edges() {
        let viewport = Vec2::new(400.0, 300.0);
        assert_eq!(
            reveal_scroll_offset(Pos2::new(10.0, 20.0), viewport),
            Vec2::ZERO
        );
        let offset = reveal_scroll_offset(Pos2::new(640.0, 750.0), viewport);
        assert_eq!(offset.x, 440.0);
        assert!((offset.y - 635.4).abs() < 0.001);
    }

    #[test]
    fn disabled_or_changed_pdf_feedback_keeps_the_pending_navigation() {
        let bytes: Arc<[u8]> = Arc::from([1u8]);
        for animation in [
            crate::settings::RevealAnimation {
                enabled: false,
                ..Default::default()
            },
            crate::settings::RevealAnimation {
                style: crate::settings::RevealStyle::Pulse,
                ..Default::default()
            },
        ] {
            let mut view = PdfiumView {
                bytes: Some(bytes.clone()),
                ..Default::default()
            };
            view.reveal_position(2, 640.0, 750.0, true);
            let point = view.position_point.unwrap();
            let viewport = Vec2::new(400.0, 300.0);
            let expected_offset =
                reveal_scroll_offset(Pos2::new(point.x, 2000.0 + point.y), viewport);
            view.reveal.as_mut().unwrap().attention.animation = Some(Default::default());
            view.update_reveal_options(&bytes, Instant::now(), animation);
            assert!(view.reveal.is_none());
            assert_eq!(view.pending_position, Some((2, 750.0)));
            assert!(view.position_pdf.is_some());
            assert_eq!(view.position_point, Some(point));
            view.catalog = Some(Arc::new(Catalog {
                sizes: vec![[800.0, 1000.0]; 3],
                outline: vec![],
            }));
            view.apply_pending_position();
            assert_eq!(view.anchor, (2, 0.75));
            let retained = view.position_point.unwrap();
            let actual_offset = reveal_scroll_offset(
                Pos2::new(retained.x, 2000.0 + view.anchor.1 * 1000.0),
                viewport,
            );
            assert_eq!(actual_offset, expected_offset);
            assert_eq!(actual_offset, egui::vec2(440.0, 2635.4));
        }
    }

    #[test]
    fn configured_pdf_duration_expires_offscreen_and_cancels_stale_navigation() {
        let bytes: Arc<[u8]> = Arc::from([1u8]);
        let mut view = PdfiumView {
            bytes: Some(bytes.clone()),
            ..Default::default()
        };
        view.reveal_position(2, 40.0, 50.0, true);
        let now = Instant::now();
        view.reveal.as_mut().unwrap().attention.started = Some(now);
        let animation = crate::settings::RevealAnimation {
            duration_ms: 1500,
            ..Default::default()
        };
        view.update_reveal_options(&bytes, now + Duration::from_millis(1499), animation);
        assert!(view.reveal.is_some());
        view.update_reveal_options(&bytes, now + Duration::from_millis(1500), animation);
        assert!(view.reveal.is_none());
        assert!(view.pending_position.is_none());
    }

    #[test]
    fn latest_navigation_cancels_pending_reveal_coordinates_and_animation() {
        let mut view = PdfiumView::default();
        for history in [true, false] {
            view.reveal_position(2, 30.0, 40.0, true);
            if history {
                view.navigate(5);
            } else {
                view.go_to_page(5);
            }
            assert!(view.reveal.is_none());
            assert!(view.pending_position.is_none());
            assert!(view.position_point.is_none());
            view.apply_pending_position();
            assert_eq!(view.goto, Some(5));
        }
        view.reveal_position(2, 30.0, 40.0, true);
        view.clear_reveal();
        assert!(view.pending_position.is_none());
    }

    #[test]
    fn first_frame_of_a_new_pdf_preserves_only_a_jump_bound_to_that_pdf() {
        for matching in [true, false] {
            let next: Arc<[u8]> = Arc::from([2u8]);
            let mut view = PdfiumView {
                path: PathBuf::from("a.pdf"),
                bytes: Some(Arc::from([1u8])),
                ..Default::default()
            };
            view.reveal_position_for_pdf(
                2,
                40.0,
                50.0,
                true,
                Some(if matching {
                    next.clone()
                } else {
                    Arc::from([3u8])
                }),
            );
            view.prepare_path(PathBuf::from("b.pdf"), &next);
            assert_eq!(view.reveal.is_some(), matching);
            assert_eq!(view.pending_position.is_some(), matching);
            assert_eq!(
                view.position_point,
                matching.then_some(egui::vec2(40.0, 50.0))
            );
            assert!(view.bytes.is_none());
        }
    }

    #[test]
    fn fit_width_uses_the_whole_preview_width() {
        assert_eq!(fit_width_zoom(600.0, 400.0) * 400.0, 600.0);
    }

    #[test]
    fn page_navigation_retains_a_back_stack() {
        let mut view = PdfiumView {
            anchor: (2, 0.4),
            ..Default::default()
        };
        view.navigate(6);
        assert_eq!(view.back, vec![(2, 0.4)]);
        assert_eq!(view.goto, Some(6));
        assert!(view.forward.is_empty());
    }

    #[test]
    fn navigation_history_is_bounded() {
        let mut history = Vec::new();
        for page in 0..300 {
            push_history(&mut history, (page, 0.0));
        }
        assert_eq!(history.len(), 256);
        assert_eq!(history.first().unwrap().0, 44);
        assert_eq!(history.last().unwrap().0, 299);
    }
    #[test]
    fn demand_only_intersects_visible_region_and_clamps_after_shrink() {
        let sizes = vec![[420.0, 550.0]; 240];
        let tops = page_tops(&sizes, 1.0);
        assert_eq!(
            visible_pages(&tops, &sizes, 1.0, tops[120] + 10.0, tops[120] + 700.0),
            120..122
        );
        assert_eq!(
            visible_pages(&tops[..1], &sizes[..1], 1.0, 90000.0, 90700.0),
            0..1
        );
    }
    #[test]
    fn current_page_ignores_a_previous_page_sliver_at_the_document_end() {
        let sizes = vec![[200.0, 200.0]; 3];
        let tops = page_tops(&sizes, 1.0);
        assert_eq!(most_visible_page(1..3, &tops, &sizes, 1.0, 400.0, 648.0), 2);
        assert_eq!(most_visible_page(0..2, &tops, &sizes, 1.0, 12.0, 250.0), 0);
        // Ties choose the earlier page and page sizing need not be uniform.
        assert_eq!(most_visible_page(0..2, &tops, &sizes, 1.0, 12.0, 424.0), 0);
        let mixed = [[200.0, 100.0], [200.0, 300.0]];
        let tops = page_tops(&mixed, 2.0);
        assert_eq!(most_visible_page(0..2, &tops, &mixed, 2.0, 180.0, 600.0), 1);
    }

    #[test]
    fn text_hit_testing_ignores_missing_bounds() {
        let chars = vec![
            Character {
                value: '\n',
                rect: Rect::NOTHING,
            },
            Character {
                value: 'x',
                rect: Rect::from_min_max(Pos2::ZERO, egui::pos2(0.1, 0.1)),
            },
        ];
        assert_eq!(nearest_character(&chars, Vec2::ZERO), Some(1));
    }
    #[test]
    fn unchanged_pdf_search_repaints_reuse_match_ranges() {
        let characters = "É İ xyz É"
            .chars()
            .map(|value| Character {
                value,
                rect: Rect::NOTHING,
            })
            .collect::<Vec<_>>();
        let mut matches = SearchMatches::default();
        assert!(matches.for_query(&characters, &Arc::from("")).is_empty());
        assert_eq!(matches.rebuilds, 0, "closed Find needs no page scan");
        for _ in 0..1000 {
            assert_eq!(
                matches.for_query(&characters, &Arc::from("é")),
                &[0..1, 8..9]
            );
        }
        assert_eq!(
            matches.rebuilds, 1,
            "animation and scroll frames must reuse unchanged page matches"
        );
        let shared_query: Arc<str> = Arc::from("é");
        assert_eq!(matches.for_query(&characters, &shared_query), &[0..1, 8..9]);
        assert!(
            Arc::ptr_eq(&matches.query, &shared_query),
            "resident pages must share query storage"
        );
        assert_eq!(matches.for_query(&characters, &Arc::from("i")), &[2..3]);
        assert_eq!(matches.rebuilds, 2);
        assert!(matches.for_query(&characters, &Arc::from("")).is_empty());
        assert_eq!(matches.rebuilds, 3);
        let mut replacement = SearchMatches::default();
        assert!(
            replacement.for_query(&[], &Arc::from("i")).is_empty(),
            "new resident pages must not inherit the former text's matches"
        );
    }

    #[test]
    fn search_handles_unicode_case_expansion_and_byte_indices() {
        let chars = "É İ xyz É"
            .chars()
            .map(|value| Character {
                value,
                rect: Rect::NOTHING,
            })
            .collect::<Vec<_>>();
        assert_eq!(matching_characters(&chars, "é"), vec![0..1, 8..9]);
        assert_eq!(matching_characters(&chars, "i"), vec![2..3]);
        assert!(matching_characters(&chars, "").is_empty());
    }

    #[test]
    #[ignore = "requires the bundled PDFium library"]
    fn native_view_retains_pixels_during_updates_and_errors() {
        let context = egui::Context::default();
        let mut view = PdfiumView::default();
        let red: Arc<[u8]> = crate::pdf::test_pdf().into();
        let green: Arc<[u8]> = String::from_utf8(red.to_vec())
            .unwrap()
            .replace("1 0 0 rg", "0 1 0 rg")
            .into_bytes()
            .into();
        let draw = |view: &mut PdfiumView, bytes: Arc<[u8]>| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(900.0, 700.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        view.show(
                            ui,
                            PathBuf::from("fixture.pdf"),
                            bytes.clone(),
                            (Color32::WHITE, Color32::BLACK),
                            false,
                            crate::settings::RevealAnimation::default(),
                        );
                    },
                )
                .drop_without_applying_deltas();
        };
        draw(&mut view, red);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !view.ready() || view.active_scale != view.requested.as_ref().unwrap().scale {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
            let bytes = view.bytes.clone().unwrap();
            draw(&mut view, bytes);
        }
        let old_id = view.residents[&0].texture.id();
        assert!(view.ready_for(view.bytes.as_ref()));
        assert!(
            !view.ready_for(Some(&green)),
            "new artifacts must wait before capture"
        );
        draw(&mut view, green.clone());
        assert_eq!(view.residents[&0].texture.id(), old_id);
        while !view.ready() {
            assert!(Instant::now() < deadline);
            assert!(!view.residents.is_empty());
            std::thread::sleep(Duration::from_millis(2));
            draw(&mut view, green.clone());
        }
        assert!(view.ready_for(Some(&green)));
        let good_id = view.residents[&0].texture.id();
        assert_ne!(good_id, old_id);
        let bad: Arc<[u8]> = Arc::from(&b"not a PDF"[..]);
        draw(&mut view, bad.clone());
        while view.error.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
            draw(&mut view, bad.clone());
        }
        assert_eq!(view.residents[&0].texture.id(), good_id);
    }
}
