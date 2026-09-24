//! Persistent egui PDFium viewer. New visible regions commit as a single batch.
use super::*;
use crate::pdfium::{Batch, Catalog, Character, LinkTarget, Request, RequestKey, Worker};
use std::collections::BTreeMap;

struct Resident {
    texture: egui::TextureHandle,
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
    dark: bool,
    zoom: f32,
    fit: bool,
    page: usize,
    goto: Option<usize>,
    anchor: (usize, f32),
    restore_anchor: bool,
    query: String,
    search_pages: Vec<usize>,
    search_pending: bool,
    selection: Option<(usize, usize, usize)>,
    error: Option<String>,
    layout_zoom: f32,
    tops: Vec<f32>,
    height: f32,
    page_focus: Option<egui::Id>,
    focus_find: bool,
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
            dark: false,
            zoom: 1.0,
            fit: true,
            page: 0,
            goto: None,
            anchor: (0, 0.0),
            restore_anchor: false,
            query: String::new(),
            search_pages: Vec::new(),
            search_pending: false,
            selection: None,
            error: None,
            layout_zoom: 0.0,
            tops: Vec::new(),
            height: 0.0,
            page_focus: None,
            focus_find: false,
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
            self.focus_find = true;
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

    fn accept(&mut self, batch: Batch, key: &RequestKey, ui: &egui::Ui, dark: bool) {
        let changed = self.active_revision != key.revision;
        if changed || self.active_scale != key.scale || self.dark != dark {
            self.residents.clear();
        }
        if changed {
            self.selection = None;
            self.restore_anchor = true;
            self.layout_zoom = 0.0;
        }
        for page in batch.pages {
            let mut rgba = page.rgba;
            if dark {
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    // Source colors remain untouched when the caller disables dark rendering.
                    for channel in &mut pixel[..3] {
                        *channel = 255 - *channel;
                    }
                }
            }
            let texture = ui.ctx().load_texture(
                format!("pdfium-{}-{}", key.revision, page.index),
                egui::ColorImage::from_rgba_unmultiplied(page.size, &rgba),
                egui::TextureOptions::LINEAR,
            );
            self.residents.insert(
                page.index,
                Resident {
                    texture,
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
        self.dark = dark;
        self.error = None;
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        path: PathBuf,
        bytes: Arc<[u8]>,
        dark: bool,
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
                Ok(batch) => self.accept(batch, &reply.key, ui, dark),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((key, pages)) = search
            && key.revision == self.revision
            && key.query == self.query
        {
            self.search_pages = pages;
            self.search_pending = false;
        }
        let mut link = None;
        let available = ui.available_rect_before_wrap();
        let mut demand = vec![self.page];
        if let Some(catalog) = self.catalog.clone() {
            let widest = catalog.sizes.iter().map(|s| s[0]).fold(1.0f32, f32::max);
            let old_zoom = self.zoom;
            if self.fit {
                self.zoom = fit_width_zoom(available.width(), widest);
            }
            let pinch = ui.ctx().input(|i| i.zoom_delta());
            let pointer = ui.ctx().pointer_latest_pos();
            let scroll_id = ui.make_persistent_id("pdfium-scroll");
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
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        if dark {
                            Color32::from_gray(25)
                        } else {
                            Color32::WHITE
                        },
                    );
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
        let was_open = self.controls_open;
        let controls_response = egui::Area::new(ui.make_persistent_id("pdfium-controls"))
            .order(egui::Order::Foreground)
            .default_pos(available.min + egui::vec2(8.0, 8.0))
            .movable(true)
            .constrain_to(available)
            .sense(egui::Sense::click_and_drag())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| self.show_controls(ui));
            });
        if !was_open && controls_response.response.clicked() {
            self.controls_open = true;
        }
        let key = RequestKey {
            revision: self.revision,
            pages: demand,
            scale: (self.zoom * ui.ctx().pixels_per_point() * 100.0)
                .round()
                .max(1.0) as u32,
            query: self.query.clone(),
        };
        if self.requested.as_ref() != Some(&key)
            || self.dark != dark
                && self.error.is_none()
                && self.active_revision == self.revision
                && self.requested.as_ref().is_some_and(|r| r == &key)
        {
            self.dark = dark;
            self.worker.as_ref().unwrap().request(Request {
                key: key.clone(),
                bytes: self.bytes.as_ref().unwrap().clone(),
            });
            self.requested = Some(key);
            self.search_pending = !self.query.is_empty();
        }
        link
    }

    fn navigate(&mut self, page: usize) {
        push_history(&mut self.back, self.anchor);
        self.forward.clear();
        self.goto = Some(page);
    }

    fn show_controls(&mut self, ui: &mut egui::Ui) {
        if !self.controls_open {
            ui.allocate_ui_with_layout(
                egui::vec2(18.0, 18.0),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.label("☰").on_hover_text("Preview controls");
                },
            );
            return;
        }
        let mut outline_target = None;
        ui.horizontal(|ui| {
            if ui
                .small_button("−")
                .on_hover_text("Minimize controls")
                .clicked()
            {
                self.controls_open = false;
            }
            ui.label("Preview");
            if let Some(catalog) = &self.catalog
                && !catalog.outline.is_empty()
            {
                ui.menu_button("Outline", |ui| {
                    for (title, page) in &catalog.outline {
                        if ui.button(title).clicked() {
                            outline_target = Some(*page);
                            ui.close();
                        }
                    }
                });
            }
        });
        if let Some(page) = outline_target {
            self.navigate(page);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.back.is_empty(), egui::Button::new("←"))
                .on_hover_text("Back")
                .clicked()
                && let Some(target) = self.back.pop()
            {
                push_history(&mut self.forward, self.anchor);
                self.anchor = target;
                self.restore_anchor = true;
            }
            if ui
                .add_enabled(!self.forward.is_empty(), egui::Button::new("→"))
                .on_hover_text("Forward")
                .clicked()
                && let Some(target) = self.forward.pop()
            {
                push_history(&mut self.back, self.anchor);
                self.anchor = target;
                self.restore_anchor = true;
            }
            if icon_button(ui, UiIcon::Previous, "Previous page").clicked() {
                self.navigate(self.page.saturating_sub(1));
            }
            let mut page = self.page + 1;
            let count = self.catalog.as_ref().map_or(1, |c| c.sizes.len());
            if ui
                .add(egui::DragValue::new(&mut page).range(1..=count))
                .changed()
            {
                self.navigate(page - 1);
            }
            ui.label(format!("/ {count}"));
            if icon_button(ui, UiIcon::Next, "Next page").clicked() {
                self.navigate((self.page + 1).min(count - 1));
            }
            if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                self.zoom(PreviewZoomAction::Out);
            }
            if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                self.zoom(PreviewZoomAction::In);
            }
            if icon_button(ui, UiIcon::FitWidth, "Fit page width").clicked() {
                self.zoom(PreviewZoomAction::Reset);
            }
            ui.label(format!("{:.0}%", self.zoom * 100.0));
        });
        ui.horizontal(|ui| {
            ui.label("Find");
            let find = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .desired_width(150.0)
                    .hint_text("Search PDF"),
            );
            if self.focus_find {
                find.request_focus();
                self.focus_find = false;
            }
            if find.changed() {
                self.search_pages.clear();
                self.search_pending = !self.query.is_empty();
            }
            if !self.query.is_empty() {
                ui.label(if self.search_pending {
                    "Searching…".into()
                } else {
                    format!("{} pages", self.search_pages.len())
                });
                if icon_button(ui, UiIcon::Next, "Next matching page").clicked() {
                    let target = self
                        .search_pages
                        .iter()
                        .copied()
                        .find(|p| *p > self.page)
                        .or_else(|| self.search_pages.first().copied());
                    if let Some(target) = target {
                        self.navigate(target);
                    }
                }
            }
        });
        if let Some(error) = &self.error {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("PDF update failed; previous preview retained: {error}"),
                );
                if ui.button("Retry").clicked() {
                    self.requested = None;
                }
            });
        } else if self.active_revision != self.revision {
            ui.label("Updating PDF…");
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
        let dark = preview.render_dark();
        let view = if asset {
            &mut self.pdfium_asset
        } else {
            &mut self.pdfium_preview
        };
        if let Some(link) = ui
            .push_id(
                if asset {
                    "pdfium-asset"
                } else {
                    "pdfium-preview"
                },
                |ui| view.show(ui, path, bytes, dark),
            )
            .inner
            && crate::pdfium::safe_url(&link)
        {
            let _ = self.web_link_sender.send(link);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                        view.show(ui, PathBuf::from("fixture.pdf"), bytes.clone(), false);
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
