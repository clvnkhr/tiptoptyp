//! Persistent egui PDFium viewer. New visible regions commit as a single batch.
use super::*;
use crate::pdfium::{Batch, Catalog, Character, LinkTarget, Request, RequestKey, Worker};
use std::collections::BTreeMap;

struct Resident {
    texture: egui::TextureHandle,
    original: Vec<u8>,
    size: [usize; 2],
    characters: Vec<Character>,
    links: Vec<(Rect, LinkTarget)>,
}

pub(super) struct PdfiumView {
    worker: Option<Worker>,
    path: PathBuf,
    bytes: Option<Arc<[u8]>>,
    revision: u64,
    active_revision: u64,
    requested: Option<RequestKey>,
    catalog: Option<Arc<Catalog>>,
    residents: BTreeMap<usize, Resident>,
    active_scale: u32,
    palette: (Color32, Color32),
    zoom: f32,
    fit: bool,
    page: usize,
    goto: Option<usize>,
    pending_position: Option<(usize, f32)>,
    anchor: (usize, f32),
    restore_anchor: bool,
    query: String,
    search_pages: Vec<usize>,
    search_pending: bool,
    find_when_ready: bool,
    selection: Option<(usize, usize, usize)>,
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
            catalog: None,
            residents: BTreeMap::new(),
            active_scale: 0,
            palette: (Color32::WHITE, Color32::BLACK),
            zoom: 1.0,
            fit: true,
            page: 0,
            goto: None,
            pending_position: None,
            anchor: (0, 0.0),
            restore_anchor: false,
            query: String::new(),
            search_pages: Vec::new(),
            search_pending: false,
            find_when_ready: false,
            selection: None,
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
            && self.active_revision == self.revision
            && !self.residents.is_empty()
            && self.requested.as_ref().is_some_and(|key| {
                key.scale == self.active_scale
                    && key.pages.iter().all(|p| {
                        self.residents.contains_key(p)
                            || self.catalog.as_ref().is_some_and(|c| *p >= c.sizes.len())
                    })
            })
    }
    #[cfg(test)]
    pub(super) fn requested_page(&self) -> Option<usize> {
        self.goto
    }

    pub(super) fn go_to_position(&mut self, page: usize, y: f32) {
        // The new map may arrive before its PDF has been inspected. Resolve
        // point coordinates only against that PDF's admitted page dimensions.
        self.pending_position = Some((page, y));
    }
    fn apply_pending_position(&mut self) {
        if self.active_revision != self.revision {
            return;
        }
        if let Some((page, y)) = self.pending_position.take() {
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

    fn accept(
        &mut self,
        batch: Batch,
        key: &RequestKey,
        ui: &egui::Ui,
        palette: (Color32, Color32),
    ) {
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
            let original = page.rgba.clone();
            let mut rgba = page.rgba;
            super::preview_palette::recolor(&mut rgba, palette.0, palette.1);
            let texture = ui.ctx().load_texture(
                format!("pdfium-{}-{}", key.revision, page.index),
                egui::ColorImage::from_rgba_unmultiplied(page.size, &rgba),
                egui::TextureOptions::LINEAR,
            );
            self.residents.insert(
                page.index,
                Resident {
                    texture,
                    original,
                    size: page.size,
                    characters: page.characters,
                    links: page.links,
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
        self.palette = palette;
        self.error = None;
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        path: PathBuf,
        bytes: Arc<[u8]>,
        palette: (Color32, Color32),
    ) -> Option<String> {
        if self.bytes.is_some() && self.path != path {
            *self = Self::default();
        }
        self.path = path;
        if self
            .bytes
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &bytes) && **old != *bytes)
        {
            self.revision += 1;
            self.bytes = Some(bytes.clone());
            self.requested = None;
            self.error = None;
            self.search_pages.clear();
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
            match reply.result {
                Ok(batch) => self.accept(batch, &reply.key, ui, palette),
                Err(error) => self.error = Some(error),
            }
        }
        self.apply_pending_position();
        if let Some((key, pages)) = search
            && key.revision == self.revision
            && key.query == self.query
        {
            self.search_pages = pages;
            self.search_pending = false;
            if std::mem::take(&mut self.find_when_ready) {
                self.find_next();
            }
        }
        if self.palette != palette {
            for resident in self.residents.values_mut() {
                let mut rgba = resident.original.clone();
                super::preview_palette::recolor(&mut rgba, palette.0, palette.1);
                resident.texture.set(
                    egui::ColorImage::from_rgba_unmultiplied(resident.size, &rgba),
                    egui::TextureOptions::LINEAR,
                );
            }
            self.palette = palette;
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
                self.restore_anchor = false;
            } else if self.restore_anchor {
                let (page, fraction) = self.anchor;
                let page = page.min(catalog.sizes.len() - 1);
                scroll = scroll.vertical_scroll_offset(
                    self.tops[page] + fraction * catalog.sizes[page][1] * self.zoom,
                );
                self.restore_anchor = false;
            }
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
                self.page = range.start;
                self.anchor = (
                    self.page,
                    (viewport.top() - self.tops[self.page])
                        / (catalog.sizes[self.page][1] * self.zoom),
                );
                for index in range {
                    let size =
                        egui::vec2(catalog.sizes[index][0], catalog.sizes[index][1]) * self.zoom;
                    let rect = Rect::from_min_size(
                        ui.min_rect().min + egui::vec2((width - size.x) / 2.0, self.tops[index]),
                        size,
                    );
                    ui.painter().rect_filled(rect, 0.0, palette.0);
                    let Some(resident) = self.residents.get(&index) else {
                        continue;
                    };
                    ui.painter().image(
                        resident.texture.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    let response = ui.interact(
                        rect,
                        ui.id().with(("pdfium-page", index)),
                        Sense::click_and_drag(),
                    );
                    if response.has_focus() || response.clicked() || response.drag_started() {
                        self.page_focus = Some(response.id);
                    }
                    let map = |normalized: Rect| {
                        Rect::from_min_max(
                            rect.min + normalized.min.to_vec2() * rect.size(),
                            rect.min + normalized.max.to_vec2() * rect.size(),
                        )
                    };
                    if let Some(pointer) = response.interact_pointer_pos() {
                        if response.clicked() && ui.input(|i| i.modifiers.command) {
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
                    for range in matching_characters(&resident.characters, &self.query) {
                        for character in &resident.characters[range] {
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
                    for (i, character) in resident.characters.iter().enumerate() {
                        if selected.as_ref().is_some_and(|r| r.contains(&i))
                            && character.rect.is_finite()
                        {
                            ui.painter().rect_filled(
                                map(character.rect),
                                0.0,
                                Color32::from_rgba_unmultiplied(60, 130, 230, 75),
                            );
                        }
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy page text").clicked() {
                            ui.ctx()
                                .copy_text(resident.characters.iter().map(|c| c.value).collect());
                            ui.close();
                        }
                    });
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

        let key = RequestKey {
            revision: self.revision,
            pages: demand,
            scale: (self.zoom * ui.ctx().pixels_per_point() * 100.0)
                .round()
                .max(1.0) as u32,
            query: self.query.clone(),
        };
        if self.requested.as_ref() != Some(&key) {
            self.palette = palette;
            self.worker.as_ref().unwrap().request(Request {
                key: key.clone(),
                bytes: self.bytes.as_ref().unwrap().clone(),
            });
            self.requested = Some(key);
            self.search_pending = !self.query.is_empty();
        }
        link
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
        push_history(&mut self.back, self.anchor);
        self.forward.clear();
        self.goto = Some(page);
    }

    pub(super) fn controls_snapshot(&self) -> super::preview_controls::Snapshot {
        super::preview_controls::Snapshot {
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
                if let Some(target) = self.back.pop() {
                    push_history(&mut self.forward, self.anchor);
                    self.anchor = target;
                    self.restore_anchor = true;
                }
            }
            Action::Forward => {
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
                if self.query != query {
                    self.query = query;
                    self.search_pages.clear();
                    self.search_pending = !self.query.is_empty();
                    self.find_when_ready = self.search_pending;
                } else if self.search_pending {
                    self.find_when_ready = true;
                } else {
                    self.find_next();
                }
            }
            Action::Location(location) => self.go_to_position(location.page, location.y),
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
    pub(super) fn show_pdfium_view(&mut self, ui: &mut egui::Ui, asset: bool) {
        self.preview_controls.available = Some(ui.available_rect_before_wrap());
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
                "Waiting for a PDF…",
                preview.status != PreviewStatus::Error,
            );
            return;
        };
        let palette = self.preview_palette(ui.ctx(), preview.render_dark());
        let view = if asset {
            &mut self.pdfium_asset
        } else {
            &mut self.pdfium_preview
        };
        if std::mem::take(&mut view.controls_open) {
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
                |ui| view.show(ui, path, bytes, palette),
            )
            .inner
            && crate::pdfium::safe_url(&link)
        {
            let _ = self.web_link_sender.send(link);
        }
        let id = viewport_scoped_id(ui.ctx(), "pdf-synctex");
        if !asset
            && let Some((page, x, y)) = ui
                .ctx()
                .data_mut(|data| data.remove_temp::<(usize, f32, f32)>(id))
        {
            self.request_synctex(crate::synctex::Query::Page { page, x, y }, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
