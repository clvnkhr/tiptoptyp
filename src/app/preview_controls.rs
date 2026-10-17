//! One presentation for both preview engines. Backends only exchange state/actions.
use super::*;

fn control(ui: &mut egui::Ui, icon: UiIcon, label: &str) -> egui::Response {
    let response = icon_button(ui, icon, label);
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.is_rect_visible(response.rect) {
        crate::desktop_test::observe(&format!("preview.{label}"), &response);
    }
    response
}

pub(super) const SALT: &str = "preview-controls";
#[derive(Clone, Default, serde::Deserialize)]
pub(super) struct Snapshot {
    #[serde(default)]
    pub focused: bool,
    pub page: usize,
    pub count: usize,
    pub zoom: f32,
    pub back: bool,
    pub forward: bool,
    #[serde(default)]
    pub outline: Vec<(String, usize)>,
}
#[derive(Clone, serde::Serialize)]
#[serde(tag = "action", content = "value", rename_all = "kebab-case")]
pub(super) enum Action {
    Back,
    Forward,
    Page(usize),
    ZoomIn,
    ZoomOut,
    Fit,
    Find(String),
    Outline(usize),
    Location(Location),
    PopOut,
}
#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct Location {
    pub page: usize,
    pub x: f32,
    pub y: f32,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Target {
    #[default]
    Source,
    Asset(DocumentKey),
}

#[derive(Default)]
pub(super) struct Controls {
    pub target: Target,
    pub popout_target: Target,
    pub open: bool,
    pub popout: Option<ViewMode>,
    pub outline: bool,
    pub query: String,
    pub edit_events: Vec<egui::Event>,
    pub focus_find: bool,
    pub return_to_editor: bool,
    pub position: Option<Pos2>,
    pub available: Option<Rect>,
    pub asset_available: Option<(DocumentKey, Rect)>,
    pub web: Snapshot,
    pub native_outline: Vec<(String, Location)>,
    measured_size: Option<(bool, usize, egui::Vec2)>,
    drag_anchor: Option<DragAnchor>,
}
#[derive(Clone, Copy)]
struct DragAnchor {
    pointer_start: Pos2,
    window_start: Pos2,
    pointer_last: Pos2,
}

impl DragAnchor {
    fn move_to(&mut self, pointer: Pos2) -> Option<Pos2> {
        if self.pointer_last == pointer {
            return None;
        }
        self.pointer_last = pointer;
        Some(drag_position(
            (self.pointer_start, self.window_start),
            pointer,
        ))
    }
}

fn drag_position((pointer_start, window_start): (Pos2, Pos2), pointer: Pos2) -> Pos2 {
    window_start + (pointer - pointer_start)
}

/// Query the actual parent view during a native drag. A popup's cached local
/// pointer cannot be added to its new origin: that invents movement each frame.
fn current_parent_pointer(context: &egui::Context) -> Option<Pos2> {
    #[cfg(target_os = "macos")]
    {
        crate::native_window::for_viewport(context, context.parent_viewport_id())?
            .file_drag_pointer(context.zoom_factor())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = context;
        None
    }
}

impl Controls {
    fn close(&mut self) {
        self.open = false;
        self.focus_find = false;
        self.edit_events.clear();
        self.drag_anchor = None;
    }

    fn toggle(&mut self, target: Target) {
        if self.select(target) || !self.open {
            self.open = true;
        } else {
            self.close();
        }
    }

    pub(super) fn select(&mut self, target: Target) -> bool {
        if self.target == target {
            return false;
        }
        self.target = target;
        self.edit_events.clear();
        self.focus_find = false;
        self.position = None;
        self.drag_anchor = None;
        true
    }

    fn available_for(&self, target: Target) -> Option<Rect> {
        match target {
            Target::Source => self.available,
            Target::Asset(key) => self
                .asset_available
                .filter(|(owner, _)| *owner == key)
                .map(|(_, rect)| rect),
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspected_size(&self) -> Option<egui::Vec2> {
        self.measured_size
            .filter(|(outline, _, _)| *outline == self.outline)
            .map(|(_, _, size)| size)
    }

    pub(super) fn set_outline(&mut self, value: &serde_json::Value) {
        fn visit(items: &[serde_json::Value], depth: usize, result: &mut Vec<(String, Location)>) {
            if depth > 32 {
                return;
            }
            for item in items {
                if result.len() >= 2048 {
                    return;
                }
                if let (Some(title), Some(page), Some(x), Some(y)) = (
                    item["title"].as_str(),
                    item["position"]["page_no"].as_u64(),
                    item["position"]["x"].as_f64(),
                    item["position"]["y"].as_f64(),
                ) && page > 0
                    && x.is_finite()
                    && y.is_finite()
                {
                    result.push((
                        format!("{}{}", "  ".repeat(depth), title),
                        Location {
                            page: page as usize - 1,
                            x: x as f32,
                            y: y as f32,
                        },
                    ));
                }
                if let Some(children) = item["children"].as_array() {
                    visit(children, depth + 1, result);
                }
            }
        }
        self.native_outline.clear();
        if let Some(items) = value["items"].as_array() {
            visit(items, 0, &mut self.native_outline);
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) -> Option<Action> {
        let _queued_edits =
            crate::child_view::begin_queued_edit_events(ui.ctx(), &mut self.edit_events);
        // Content determines the popup height. Measuring against last frame's
        // shorter native viewport feeds its clipping limit back into sizing.
        ui.set_max_height(10_000.0);
        let mut action = None;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if control(ui, UiIcon::ZoomOut, "Minimize controls").clicked() {
                    self.close();
                }
                let popout = ui.add_enabled(self.popout.is_none(), egui::Button::new("Pop out"));
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("preview.Pop out", &popout);
                if popout.clicked() {
                    action = Some(Action::PopOut);
                }
                let _outline = ui.toggle_value(&mut self.outline, "Outline");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("preview.Outline", &_outline);
                let handle = ui.add_sized(
                    [ui.available_width(), ui.spacing().interact_size.y],
                    egui::Label::new("Preview").sense(Sense::drag()),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("preview.drag_handle", &handle);
                let embedded = ui.ctx().embed_viewports();
                let (origin, pointer, pressed, pointer_moved) = ui.input(|input| {
                    (
                        if embedded {
                            Pos2::ZERO
                        } else {
                            input
                                .viewport()
                                .inner_rect
                                .map_or(Pos2::ZERO, |rect| rect.min)
                        },
                        input.pointer.interact_pos(),
                        input.pointer.press_origin(),
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::PointerMoved(_))),
                    )
                });
                let dragging = handle.drag_started() || handle.dragged() || handle.drag_stopped();
                let native_pointer = if !embedded && dragging {
                    current_parent_pointer(ui.ctx())
                } else {
                    None
                };
                if handle.drag_started()
                    && let (Some(pressed), Some(pointer)) = (pressed, pointer)
                {
                    // Preserve the press offset even if egui begins the drag
                    // only after crossing its movement threshold.
                    let pointer_start = native_pointer
                        .map_or(origin + pressed.to_vec2(), |current| {
                            current - (pointer - pressed)
                        });
                    self.drag_anchor = Some(DragAnchor {
                        pointer_start,
                        window_start: self.position.unwrap_or_default(),
                        pointer_last: pointer_start,
                    });
                }
                let fresh_pointer = native_pointer.or_else(|| {
                    pointer
                        .filter(|_| pointer_moved)
                        .map(|pointer| origin + pointer.to_vec2())
                });
                if dragging
                    && let (Some(anchor), Some(pointer)) = (&mut self.drag_anchor, fresh_pointer)
                    && let Some(position) = anchor.move_to(pointer)
                    && self.position != Some(position)
                {
                    self.position = Some(position);
                    ui.ctx().request_repaint_of(ui.ctx().parent_viewport_id());
                }
                if handle.drag_stopped() {
                    self.drag_anchor = None;
                }
            });
        });
        ui.horizontal(|ui| {
            for (enabled, symbol, label, navigation) in [
                (snapshot.back, "←", "Back", Action::Back),
                (snapshot.forward, "→", "Forward", Action::Forward),
            ] {
                let response = ui.add_enabled(enabled, egui::Button::new(symbol));
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
                });
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe(&format!("preview.{label}"), &response);
                if response.on_hover_text(label).clicked() {
                    action = Some(navigation);
                }
            }
            ui.separator();
            if control(ui, UiIcon::Previous, "Previous page").clicked() {
                action = Some(Action::Page(snapshot.page.saturating_sub(1)));
            }
            let mut page = snapshot.page + 1;
            if ui
                .add(egui::DragValue::new(&mut page).range(1..=snapshot.count.max(1)))
                .changed()
            {
                action = Some(Action::Page(page - 1));
            }
            ui.label(format!("/ {}", snapshot.count));
            if control(ui, UiIcon::Next, "Next page").clicked() {
                action = Some(Action::Page(
                    (snapshot.page + 1).min(snapshot.count.saturating_sub(1)),
                ));
            }
            ui.separator();
            if control(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                action = Some(Action::ZoomOut);
            }
            if control(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                action = Some(Action::ZoomIn);
            }
            if control(ui, UiIcon::FitWidth, "Fit page width").clicked() {
                action = Some(Action::Fit);
            }
            ui.label(format!("{:.0}%", snapshot.zoom * 100.0));
        });
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Find in preview")
                    .desired_width(245.0),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    response.enabled(),
                    "Find in preview",
                )
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("preview.query", &response);
            if self.focus_find {
                crate::window_host::focus(
                    ui.ctx(),
                    ui.ctx().viewport_id(),
                    crate::window_host::FocusCause::UserAction,
                );
                response.request_focus();
                self.focus_find = false;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if control(ui, UiIcon::Next, "Find next").clicked() || enter {
                action = Some(Action::Find(self.query.clone()));
            }
        });
        if self.outline {
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    if snapshot.outline.is_empty() {
                        ui.weak("No outline available");
                    }
                    for (index, (title, _)) in snapshot.outline.iter().enumerate() {
                        let response = ui
                            .add(egui::Button::new(title).wrap_mode(egui::TextWrapMode::Truncate))
                            .on_hover_text(title);
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if index < 16 && ui.is_rect_visible(response.rect) {
                            crate::desktop_test::observe(
                                &format!("preview.outline.{index}"),
                                &response,
                            );
                        }
                        if response.clicked() {
                            action = Some(Action::Outline(index));
                        }
                    }
                });
        }
        action
    }
}
impl EditorApp {
    pub(super) fn preview_source_popped_out(&self) -> bool {
        self.preview_controls.popout.is_some()
            && self.preview_controls.popout_target == Target::Source
    }

    pub(super) fn preview_asset_popped_out(&self) -> bool {
        self.preview_controls.popout.is_some()
            && self.preview_controls.popout_target == Target::Asset(self.document().key())
    }

    pub(super) fn preview_controls_in_popout(&self) -> bool {
        self.preview_controls.popout.is_some()
            && self.preview_controls.target == self.preview_controls.popout_target
    }

    fn valid_preview_target(&self, target: Target) -> bool {
        match target {
            Target::Source => self.source_preview_available(),
            Target::Asset(key) => self.pdfium_asset_requested() && key == self.document().key(),
        }
    }

    pub(super) fn main_preview_controls_target(&self) -> Option<Target> {
        if self.tabs.is_empty() || self.bottom_panel.is_maximized() {
            return None;
        }
        if self.source_preview_available()
            && !self.preview_source_popped_out()
            && preview_visible_for(
                self.document().kind(),
                self.view_mode,
                self.designated_preview_path().is_some(),
            )
        {
            Some(Target::Source)
        } else if self.pdfium_asset_requested() && !self.preview_asset_popped_out() {
            Some(Target::Asset(self.document().key()))
        } else {
            None
        }
    }

    fn refresh_preview_controls_target(&mut self) -> bool {
        if self.valid_preview_target(self.preview_controls.target) {
            return true;
        }
        // Retire both source and asset owners before choosing the next default.
        // A source closing beside a PDF must not redirect its queued control
        // action to that PDF. Fresh controls are opened by an explicit toggle.
        let replacement = self.main_preview_controls_target();
        self.preview_controls
            .select(replacement.unwrap_or_default());
        self.preview_controls.close();
        false
    }

    pub(super) fn focused_preview_target(&self, context: &egui::Context) -> Option<Target> {
        if !self.preview_visible() {
            return None;
        }
        let owner = context.viewport_id();
        let focused = focused_input_viewport(context);
        let preview_window = scoped_child_viewport_id(context, "preview-window");
        let controls = scoped_child_viewport_id(context, SALT);
        let popout_controls = crate::child_view::child_viewport_id(preview_window, SALT);
        if self.preview_controls.popout.is_some()
            && (focused == preview_window || focused == popout_controls)
        {
            return self
                .valid_preview_target(self.preview_controls.popout_target)
                .then_some(self.preview_controls.popout_target);
        }
        if focused == controls {
            return self
                .valid_preview_target(self.preview_controls.target)
                .then_some(self.preview_controls.target);
        }
        if focused != owner {
            return None;
        }
        if self.pdfium_asset_requested()
            && !self.preview_asset_popped_out()
            && self.pdfium_asset.has_focus(context)
        {
            return Some(Target::Asset(self.document().key()));
        }
        if self.main_preview_controls_target() == Some(Target::Source)
            && ((self.pdfium_preview_requested() && self.pdfium_preview.has_focus(context))
                || (self.interactive_preview_requested()
                    && self.preview_controls.web.focused
                    && context.memory(|memory| memory.focused().is_none())
                    && self.explorer.focused_section().is_none()
                    && !self.terminal_focused(context, owner)))
        {
            return Some(Target::Source);
        }
        None
    }

    /// Find belongs to the focused viewer or its controls, even when that
    /// viewer lives in a separate window and the source remains visible.
    pub(super) fn open_preview_find(&mut self, context: &egui::Context) -> bool {
        let Some(target) = self.focused_preview_target(context) else {
            return false;
        };
        self.preview_controls.select(target);
        self.preview_controls.open = true;
        self.preview_controls.focus_find = true;
        context.request_repaint_of(if self.preview_controls_in_popout() {
            scoped_child_viewport_id(context, "preview-window")
        } else {
            context.viewport_id()
        });
        true
    }

    pub(super) fn route_preview_zoom(
        &mut self,
        context: &egui::Context,
        action: PreviewZoomAction,
    ) -> bool {
        let focused = self.focused_preview_target(context);
        if focused.is_none() {
            let viewport = focused_input_viewport(context);
            let window = scoped_child_viewport_id(context, "preview-window");
            let controls = scoped_child_viewport_id(context, SALT);
            let detached_controls = crate::child_view::child_viewport_id(window, SALT);
            let obsolete_controls =
                viewport == controls && !self.valid_preview_target(self.preview_controls.target);
            let obsolete_window = (viewport == window || viewport == detached_controls)
                && (self.preview_controls.popout.is_none()
                    || !self.valid_preview_target(self.preview_controls.popout_target));
            if obsolete_controls || obsolete_window {
                // The native child may still report focus during replacement.
                // Its event belongs to the retired viewer, never a fallback.
                return true;
            }
        }
        // Images have their own controls and no page focus widget. Preserve
        // their root-window fallback unless a source viewer owns this input.
        if focused.is_none()
            && self.document().kind() == DocumentKind::Image
            && focused_input_viewport(context) == context.viewport_id()
        {
            return false;
        }
        let Some(target) = focused
            .or_else(|| self.main_preview_controls_target())
            .or_else(|| {
                self.preview_controls
                    .popout
                    .filter(|_| self.valid_preview_target(self.preview_controls.popout_target))
                    .map(|_| self.preview_controls.popout_target)
            })
        else {
            return false;
        };
        self.preview_control_action_for(
            target,
            match action {
                PreviewZoomAction::In => Action::ZoomIn,
                PreviewZoomAction::Out => Action::ZoomOut,
                PreviewZoomAction::Reset => Action::Fit,
            },
        );
        true
    }

    pub(super) fn preview_controls_enabled(&self) -> bool {
        self.main_preview_controls_target().is_some()
    }

    pub(super) fn main_preview_controls_open(&self) -> bool {
        self.preview_controls.open
            && self.main_preview_controls_target() == Some(self.preview_controls.target)
    }

    pub(super) fn toggle_main_preview_controls(&mut self) {
        if let Some(target) = self.main_preview_controls_target() {
            self.preview_controls.toggle(target);
        }
    }

    pub(super) fn preview_controls_snapshot(&self) -> Snapshot {
        match self.preview_controls.target {
            Target::Source if self.interactive_preview_active() => {
                let mut snapshot = self.preview_controls.web.clone();
                snapshot.outline = self
                    .preview_controls
                    .native_outline
                    .iter()
                    .map(|(title, location)| (title.clone(), location.page))
                    .collect();
                snapshot
            }
            Target::Source => self.pdfium_preview.controls_snapshot(),
            Target::Asset(_) => self.pdfium_asset.controls_snapshot(),
        }
    }
    pub(super) fn show_preview_controls(
        &mut self,
        context: &egui::Context,
        in_preview_window: bool,
    ) {
        let target_valid = self.refresh_preview_controls_target();
        let in_popout = self.preview_controls_in_popout();
        // Each viewer keeps its own root/persistent-window owner. A detached
        // asset can coexist with the pinned source in the main window.
        if self.latexdiff.is_some()
            || !target_valid
            || !self.preview_controls.open
            || in_popout != in_preview_window
            || (!in_popout && !self.preview_controls_enabled())
        {
            ChildViewHost::close(context, SALT);
            return;
        }
        let snapshot = self.preview_controls_snapshot();
        let available = self
            .preview_controls
            .available_for(self.preview_controls.target)
            .unwrap_or(context.content_rect());
        let shape = (self.preview_controls.outline, snapshot.outline.len());
        let size = self
            .preview_controls
            .measured_size
            .filter(|(outline, count, _)| (*outline, *count) == shape)
            .map_or(
                egui::vec2(360.0, if shape.0 { 330.0 } else { 108.0 }),
                |(_, _, size)| size,
            );
        let position = self
            .preview_controls
            .position
            .get_or_insert(available.min + egui::vec2(8.0, 8.0));
        *position = egui::pos2(
            position
                .x
                .clamp(0.0, (context.content_rect().right() - size.x).max(0.0)),
            position
                .y
                .clamp(0.0, (context.content_rect().bottom() - size.y).max(0.0)),
        );
        let bounds = Rect::from_min_size(*position, size);
        let mut action = None;
        let viewport = scoped_child_viewport_id(context, SALT);
        let shortcuts = self.settings.effective_shortcuts();
        let allow_requested_paste = self
            .pending_widget_paste
            .is_some_and(|request| request.admits(viewport, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut requested_paste_admitted = false;
        if context.embed_viewports() {
            egui::Area::new(viewport_scoped_id(context, SALT))
                .order(egui::Order::Foreground)
                .fixed_pos(bounds.min)
                .show(context, |ui| {
                    let card = theme::popup_card_frame(ui.style()).show(ui, |ui| {
                        ui.set_width(344.0);
                        action = self.preview_controls.show(ui, &snapshot);
                    });
                    let measured = card.response.rect.size();
                    if self.preview_controls.measured_size != Some((shape.0, shape.1, measured)) {
                        self.preview_controls.measured_size = Some((shape.0, shape.1, measured));
                        context.request_repaint();
                    }
                });
        } else if let Some(window) = context.input(|i| i.viewport().inner_rect) {
            let appearance = context.theme();
            let style = context.style_of(appearance);
            let spec = ChildViewSpec::modeless_popup(
                SALT,
                "Preview controls",
                bounds.translate(window.min.to_vec2()),
                false,
                "preview-controls",
            );
            ChildViewHost::show(
                context,
                &self.captures,
                spec,
                appearance,
                &style,
                |ui, input| {
                    if input.close_requested || input.escape_pressed {
                        self.preview_controls.close();
                        return;
                    }
                    let edits = prepare_child_widget_edit_events(
                        ui.ctx(),
                        &shortcuts,
                        allow_requested_paste,
                    );
                    paste_requested |= edits.paste_requested;
                    requested_paste_admitted |= edits.requested_paste_admitted;
                    let card = theme::popup_card_frame(ui.style()).show(ui, |ui| {
                        ui.set_width(344.0);
                        action = self.preview_controls.show(ui, &snapshot);
                    });
                    let measured = card.response.rect.size();
                    if self.preview_controls.measured_size != Some((shape.0, shape.1, measured)) {
                        self.preview_controls.measured_size = Some((shape.0, shape.1, measured));
                        context.request_repaint();
                    }
                },
            );
        }
        if requested_paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, viewport);
        }
        if let Some(action) = action {
            self.preview_control_action(action);
            if !self.preview_controls.open {
                ChildViewHost::close(context, SALT);
            }
        }
    }
    pub(super) fn restore_preview_window(&mut self, context: &egui::Context) {
        let controls_detached = self.preview_controls_in_popout();
        if let Some(mode) = self.preview_controls.popout.take() {
            self.view_mode = mode;
            if controls_detached {
                self.preview_controls.position = None;
                self.preview_controls.close();
            }
            if self.preview_controls.popout_target == Target::Source {
                self.discard_webview();
            }
            let owner = scoped_child_viewport_id(context, "preview-window");
            ChildViewHost::close_viewport(
                context,
                crate::child_view::child_viewport_id(owner, SALT),
            );
            ChildViewHost::close(context, "preview-window");
            context.request_repaint();
        }
    }
    pub(super) fn show_preview_window(&mut self, context: &egui::Context) {
        if self.preview_controls.popout.is_none() {
            return;
        }
        if self.tabs.is_empty() || !self.valid_preview_target(self.preview_controls.popout_target) {
            self.restore_preview_window(context);
            return;
        }
        let appearance = context.theme();
        let style = context.style_of(appearance);
        let captures = self.captures.clone();
        let mut close = false;
        ChildViewHost::show(
            context,
            &captures,
            ChildViewSpec::persistent(
                "preview-window",
                "tiptoptyp Preview",
                [800.0, 700.0],
                [360.0, 280.0],
                "preview-window",
            ),
            appearance,
            &style,
            |ui, input| {
                close |= input.close_requested;
                egui::Panel::top("preview-title").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        #[cfg(target_os = "macos")]
                        theme::reserve_window_controls(ui);
                        close |= ui.button("Return to document").clicked();
                        if icon_button(ui, UiIcon::Menu, "Preview controls").clicked() {
                            self.preview_controls
                                .toggle(self.preview_controls.popout_target);
                        }
                    });
                });
                egui::CentralPanel::default()
                    .frame(theme::content_panel_frame(ui.style()).inner_margin(0.0))
                    .show(ui, |ui| match self.preview_controls.popout_target {
                        Target::Source => self.show_preview(ui, None, false),
                        Target::Asset(_) => self.show_asset_view(ui),
                    });
                self.show_preview_controls(ui.ctx(), true);
                if matches!(self.app_popup, Some(AppPopup::PreviewSource { window, .. }) if window == ui.ctx().viewport_id())
                {
                    self.show_app_popup_window(ui.ctx());
                }
            },
        );
        if close {
            self.restore_preview_window(context);
        }
    }
    pub(super) fn preview_control_action(&mut self, action: Action) {
        if !self.refresh_preview_controls_target() {
            return;
        }
        if matches!(action, Action::PopOut) {
            if self.preview_controls.popout.is_some() {
                return;
            }
            self.preview_controls.popout_target = self.preview_controls.target;
            self.preview_controls.popout = Some(self.view_mode);
            self.view_mode = ViewMode::Code;
            self.preview_controls.close();
            self.preview_controls.position = None;
            if self.preview_controls.popout_target == Target::Source {
                self.discard_webview();
            }
            return;
        }
        self.preview_control_action_for(self.preview_controls.target, action);
    }

    pub(super) fn preview_control_action_for(&mut self, target: Target, action: Action) {
        if !self.valid_preview_target(target) {
            return;
        }
        if matches!(target, Target::Asset(_)) {
            self.pdfium_asset.controls_action(action);
        } else if self.interactive_preview_active() {
            match &action {
                Action::Find(query) => self.handle_web_action(
                    &serde_json::json!({"type":"find","query":query}).to_string(),
                ),
                Action::Outline(index) => {
                    if let Some((_, location)) = self.preview_controls.native_outline.get(*index) {
                        self.preview_control_action_for(target, Action::Location(location.clone()));
                    }
                    return;
                }
                _ => {}
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(webview) = &self.webview {
                #[cfg(target_os = "macos")]
                if self
                    .webview_snapshot
                    .action(&serde_json::to_value(&action).unwrap())
                {
                    return;
                }
                let _ = webview.evaluate_script(&format!(
                    "window.tiptoptypPreviewAction?.({})",
                    serde_json::to_string(&action).unwrap()
                ));
            }
        } else {
            self.pdfium_preview.controls_action(action);
        }
    }
}

#[cfg(test)]
mod e2e;

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};
    #[test]
    fn find_command_opens_controls_and_focuses_tinymist_query() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.view_mode = ViewMode::Split;
        app.preview = PreviewController::new(false, PreviewPreference::Interactive);
        app.preview_controls.web.focused = true;
        let source = app.document().source().to_owned();
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| {
                app.process_native_menu_commands(ui.ctx(), None);
                if app.preview_controls.open {
                    app.preview_controls.show(ui, &Snapshot::default());
                }
            },
            app,
        );
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::Find);
        harness.run_steps(3);
        assert!(harness.state().preview_controls.open);
        assert!(!harness.state().find_bar.visible);
        harness.get_by_label("Find in preview").type_text("needle");
        harness.run_steps(3);
        assert_eq!(harness.state().preview_controls.query, "needle");
        assert_eq!(harness.state().document().source(), &source);
    }

    #[test]
    fn stale_native_focus_does_not_steal_find_from_the_code_editor() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.preview = PreviewController::new(false, PreviewPreference::Interactive);
        app.preview_controls.web.focused = true;
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| app.process_native_menu_commands(ui.ctx(), None),
            app,
        );
        let editor = source_editor_id(&harness.ctx);
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(editor));
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::Find);
        harness.run_steps(2);
        assert!(!harness.state().preview_controls.open);
        assert!(harness.state().find_bar.visible);
    }

    #[test]
    fn preview_window_and_its_controls_own_find_without_source_focus() {
        for controls_focused in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.preview_controls.popout = Some(ViewMode::Split);
            app.view_mode = ViewMode::Code;
            let preview = scoped_child_viewport_id(&context, "preview-window");
            let focused = if controls_focused {
                crate::child_view::child_viewport_id(preview, SALT)
            } else {
                preview
            };
            let mut harness = Harness::builder().build_ui_state(
                |ui, app: &mut EditorApp| app.process_native_menu_commands(ui.ctx(), None),
                app,
            );
            for (viewport, is_focused) in [(egui::ViewportId::ROOT, false), (focused, true)] {
                harness
                    .input_mut()
                    .viewports
                    .entry(viewport)
                    .or_default()
                    .focused = Some(is_focused);
            }
            harness
                .state_mut()
                .enqueue_native_menu_command(AppCommand::Find);
            harness.run_steps(2);
            assert!(
                harness.state().preview_controls.open,
                "controls focused: {controls_focused}"
            );
            assert!(harness.state().preview_controls.focus_find);
            assert!(!harness.state().find_bar.visible);
        }
    }

    fn pinned_source_with_asset(
        context: &egui::Context,
        root: &Path,
        preference: PreviewPreference,
    ) -> EditorApp {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "= Pinned source\n".into(),
            root.join("source.typ"),
            DocumentKind::Typst,
            None,
        );
        app.prepare_asset_tab_fixture(context, root.join("asset.pdf"), DocumentKind::Pdf);
        app.preview = PreviewController::new(false, preference);
        if preference.is_interactive() {
            let generation = crate::tinymist::Generation(1);
            app.preview.connection.start(generation);
            app.preview.connection.initialized(generation);
            app.preview.connection.connect(
                generation,
                url::Url::parse("http://127.0.0.1:23625").unwrap(),
            );
            app.preview.webview_state = ServiceState::Ready("loaded".into());
            app.preview_controls.web.zoom = 2.0;
        }
        app
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn focused_pdf_asset_find_targets_the_asset_beside_a_pinned_tinymist_source() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Interactive);
        assert!(app.source_preview_available());
        assert!(app.interactive_preview_active());
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| app.process_native_menu_commands(ui.ctx(), None),
            app,
        );
        let ctx = harness.ctx.clone();
        harness
            .state_mut()
            .pdfium_asset
            .focus_for_tests(&ctx, "asset");
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::Find);
        harness.run_steps(2);
        assert!(harness.state().preview_controls.open);
        assert!(!harness.state().find_bar.visible);
        let asset_before = harness.state().pdfium_asset.controls_snapshot().zoom;
        let source_before = harness.state().pdfium_preview.controls_snapshot().zoom;
        harness.state_mut().preview_control_action(Action::ZoomIn);
        assert!(harness.state().pdfium_asset.controls_snapshot().zoom > asset_before);
        assert_eq!(
            harness.state().pdfium_preview.controls_snapshot().zoom,
            source_before
        );
        assert_eq!(harness.state().preview_controls.web.zoom, 2.0);
    }

    #[test]
    fn preview_zoom_shortcut_targets_the_focused_source_beside_a_pdf_asset() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let app = pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        assert!(app.source_preview_available());
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| app.handle_shortcuts(ui.ctx(), None),
            app,
        );
        let ctx = harness.ctx.clone();
        harness
            .state_mut()
            .pdfium_preview
            .focus_for_tests(&ctx, "source");
        let asset_before = harness.state().pdfium_asset.controls_snapshot().zoom;
        let source_before = harness.state().pdfium_preview.controls_snapshot().zoom;
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, egui::Key::Plus);
        harness.run_steps(2);
        assert!(harness.state().pdfium_preview.controls_snapshot().zoom > source_before);
        assert_eq!(
            harness.state().pdfium_asset.controls_snapshot().zoom,
            asset_before
        );
    }

    #[test]
    fn preview_zoom_with_code_focus_reaches_the_only_detached_viewer() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.preview = PreviewController::new(false, PreviewPreference::Pdfium);
        app.view_mode = ViewMode::Split;
        app.preview_control_action(Action::PopOut);
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| app.handle_shortcuts(ui.ctx(), None),
            app,
        );
        let editor = source_editor_id(&harness.ctx);
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(editor));
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, egui::Key::Plus);
        harness.run_steps(2);
        assert_eq!(harness.state().pdfium_preview.controls_snapshot().zoom, 1.2);
        assert!(harness.state().preview_source_popped_out());
    }

    #[test]
    fn focused_source_and_asset_controls_keep_their_own_snapshots_and_bounds() {
        for preference in PreviewPreference::ALL {
            if preference.is_interactive() && !cfg!(any(target_os = "macos", target_os = "windows"))
            {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = pinned_source_with_asset(&context, directory.path(), preference);
            app.pdfium_preview.controls_action(Action::ZoomIn);
            let key = app.document().key();
            let source_rect =
                Rect::from_min_size(egui::pos2(600.0, 100.0), egui::vec2(600.0, 700.0));
            let asset_rect = Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(600.0, 700.0));
            app.preview_controls.available = Some(source_rect);
            app.preview_controls.asset_available = Some((key, asset_rect));
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1400.0, 900.0))
                .build_ui_state(
                    |ui, app: &mut EditorApp| {
                        app.process_native_menu_commands(ui.ctx(), None);
                        app.show_preview_controls(ui.ctx(), false);
                    },
                    app,
                );
            let ctx = harness.ctx.clone();
            harness
                .state_mut()
                .pdfium_asset
                .focus_for_tests(&ctx, "asset");
            harness
                .state_mut()
                .enqueue_native_menu_command(AppCommand::Find);
            harness.run_steps(3);
            assert_eq!(harness.state().preview_controls.target, Target::Asset(key));
            assert_eq!(harness.state().preview_controls_snapshot().zoom, 1.0);
            assert_eq!(
                harness.state().preview_controls.position,
                Some(asset_rect.min + egui::vec2(8.0, 8.0))
            );
            if preference == PreviewPreference::Pdfium {
                harness
                    .state_mut()
                    .pdfium_preview
                    .focus_for_tests(&ctx, "source");
            } else {
                let focused = ctx.memory(|memory| memory.focused());
                if let Some(id) = focused {
                    ctx.memory_mut(|memory| memory.surrender_focus(id));
                }
                harness.state_mut().preview_controls.web.focused = true;
            }
            harness
                .state_mut()
                .enqueue_native_menu_command(AppCommand::Find);
            harness.run_steps(3);
            assert_eq!(harness.state().preview_controls.target, Target::Source);
            assert_eq!(
                harness.state().preview_controls_snapshot().zoom,
                if preference.is_interactive() {
                    2.0
                } else {
                    1.2
                }
            );
            assert_eq!(
                harness.state().preview_controls.position,
                Some(source_rect.min + egui::vec2(8.0, 8.0))
            );
            assert_eq!(
                harness
                    .state()
                    .preview_controls
                    .available_for(Target::Asset(key)),
                Some(asset_rect)
            );
        }
    }

    #[test]
    fn detached_asset_preserves_the_pinned_source_and_their_separate_controls_owners() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        app.view_mode = ViewMode::Split;
        let key = app.document().key();
        app.preview_controls.select(Target::Asset(key));
        app.preview_control_action(Action::PopOut);
        assert!(app.preview_asset_popped_out());
        assert!(!app.preview_source_popped_out());
        assert_eq!(
            workspace_view::content_view(
                false,
                app.document().kind(),
                app.source_preview_available(),
                app.view_mode,
                app.preview_source_popped_out()
            ),
            workspace_view::ContentView::SplitAsset
        );
        assert!(app.preview_controls_enabled());
        app.toggle_main_preview_controls();
        assert_eq!(app.preview_controls.target, Target::Source);
        assert!(app.main_preview_controls_open());
        assert!(!app.preview_controls_in_popout());
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| app.process_native_menu_commands(ui.ctx(), None),
            app,
        );
        let preview = scoped_child_viewport_id(&harness.ctx, "preview-window");
        for (viewport, focused) in [(egui::ViewportId::ROOT, false), (preview, true)] {
            harness
                .input_mut()
                .viewports
                .entry(viewport)
                .or_default()
                .focused = Some(focused);
        }
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::Find);
        harness.run_steps(2);
        assert_eq!(harness.state().preview_controls.target, Target::Asset(key));
        assert!(harness.state().preview_controls_in_popout());
        assert!(!harness.state().main_preview_controls_open());
        harness.state_mut().preview_control_action(Action::ZoomIn);
        assert_eq!(harness.state().pdfium_asset.controls_snapshot().zoom, 1.2);
        assert_eq!(harness.state().pdfium_preview.controls_snapshot().zoom, 1.0);
    }

    #[test]
    fn restoring_a_detached_viewer_preserves_the_other_viewers_controls_and_lifecycle() {
        for detached_source in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app =
                pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
            app.view_mode = ViewMode::Split;
            let key = app.document().key();
            app.preview_controls.select(if detached_source {
                Target::Source
            } else {
                Target::Asset(key)
            });
            app.preview_control_action(Action::PopOut);
            app.toggle_main_preview_controls();
            let root_target = app.preview_controls.target;
            app.preview_controls.query = "root-query".into();
            app.preview_controls.position = Some(egui::pos2(700.0, 140.0));
            let root = context.viewport_id();
            let main_controls = scoped_child_viewport_id(&context, SALT);
            let window = scoped_child_viewport_id(&context, "preview-window");
            let detached_controls = crate::child_view::child_viewport_id(window, SALT);
            let main_token =
                crate::window_host::show(&context, main_controls, root, true, false).unwrap();
            let window_token =
                crate::window_host::show(&context, window, root, true, false).unwrap();
            let detached_token =
                crate::window_host::show(&context, detached_controls, window, true, false).unwrap();
            app.restore_preview_window(&context);
            assert_eq!(app.preview_controls.target, root_target);
            assert!(
                app.preview_controls.open,
                "root controls for {root_target:?} must stay open"
            );
            assert_eq!(app.preview_controls.query, "root-query");
            assert_eq!(
                app.preview_controls.position,
                Some(egui::pos2(700.0, 140.0))
            );
            assert_eq!(app.document().key(), key);
            assert!(!crate::window_host::live(&context, window_token));
            assert!(!crate::window_host::live(&context, detached_token));
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(1400.0, 900.0),
                        )),
                        ..Default::default()
                    },
                    |ui| app.show_preview_controls(ui.ctx(), false),
                )
                .drop_without_applying_deltas();
            assert!(crate::window_host::live(&context, main_token));
            assert!(app.preview_controls.open);
            assert!(!app.preview_controls_in_popout());
        }
    }

    #[test]
    fn preview_zoom_without_source_focus_preserves_image_zoom_beside_a_pinned_source() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        app.document_mut().replace_loaded_unprojected(
            String::new(),
            directory.path().join("image.png"),
            DocumentKind::Image,
            None,
        );
        assert!(app.source_preview_available());
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| {
                // Keep the page owner registered through the modifier/key
                // frames, as a rendered PDF page does in the real viewer.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(180.0, 24.0), Sense::hover());
                let response = ui.interact(
                    rect,
                    pdfium_view::PdfiumView::page_focus_id_for_tests("source"),
                    Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Source preview page")
                });
                if response.clicked() {
                    response.request_focus();
                }
                app.handle_shortcuts(ui.ctx(), None);
            },
            app,
        );
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, egui::Key::Plus);
        harness.run_steps(2);
        assert_eq!(
            harness.state().asset_preview.requested_zoom,
            Some(METRICS.preview.zoom_step)
        );
        assert_eq!(harness.state().pdfium_preview.controls_snapshot().zoom, 1.0);
        let ctx = harness.ctx.clone();
        harness
            .state_mut()
            .pdfium_preview
            .focus_for_tests(&ctx, "source");
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, egui::Key::Plus);
        harness.run_steps(2);
        assert_eq!(harness.state().pdfium_preview.controls_snapshot().zoom, 1.2);
        assert_eq!(
            harness.state().asset_preview.requested_zoom,
            Some(METRICS.preview.zoom_step)
        );
    }

    #[test]
    fn obsolete_controls_and_popout_zoom_events_are_consumed_without_retargeting() {
        for replacement in [DocumentKind::Pdf, DocumentKind::Image] {
            for child in ["controls", "preview-window", "preview-controls"] {
                let directory = tempfile::tempdir().unwrap();
                let context = egui::Context::default();
                let mut app =
                    pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
                app.preview_controls
                    .select(Target::Asset(app.document().key()));
                if child != "controls" {
                    app.preview_control_action(Action::PopOut);
                }
                app.preview_controls.open = true;
                app.document_mut().replace_loaded_unprojected(
                    String::new(),
                    directory.path().join("replacement"),
                    replacement,
                    None,
                );
                assert!(app.source_preview_available());
                let mut harness = Harness::builder().build_ui_state(
                    |ui, state: &mut (EditorApp, bool, Option<bool>)| {
                        if state.1 {
                            state.1 = false;
                            let handled =
                                state.0.route_preview_zoom(ui.ctx(), PreviewZoomAction::In);
                            if !handled {
                                image_view::request_zoom(
                                    &mut state.0.asset_preview,
                                    PreviewZoomAction::In,
                                );
                            }
                            state.2 = Some(handled);
                        }
                    },
                    (app, false, None),
                );
                let preview = scoped_child_viewport_id(&harness.ctx, "preview-window");
                let focused = match child {
                    "controls" => scoped_child_viewport_id(&harness.ctx, SALT),
                    "preview-window" => preview,
                    _ => crate::child_view::child_viewport_id(preview, SALT),
                };
                for (viewport, focused) in [(egui::ViewportId::ROOT, false), (focused, true)] {
                    harness
                        .input_mut()
                        .viewports
                        .entry(viewport)
                        .or_default()
                        .focused = Some(focused);
                }
                harness.state_mut().1 = true;
                harness.run_steps(2);
                assert_eq!(harness.state().2, Some(true));
                assert_eq!(
                    harness.state().0.pdfium_preview.controls_snapshot().zoom,
                    1.0,
                    "{child} / {replacement:?}"
                );
                assert_eq!(harness.state().0.pdfium_asset.controls_snapshot().zoom, 1.0);
                assert_eq!(harness.state().0.asset_preview.requested_zoom, None);
            }
        }
    }

    #[test]
    fn standalone_pdf_first_zoom_shortcut_and_toolbar_controls_work_before_target_normalization() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            String::new(),
            directory.path().join("standalone.pdf"),
            DocumentKind::Pdf,
            None,
        );
        app.view_mode = ViewMode::Code;
        let mut show_controls = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1400.0, 800.0))
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    app.handle_shortcuts(ui.ctx(), None);
                    egui::Panel::top("first-pdf-toolbar").show(ui, |ui| app.show_toolbar(ui, None));
                    // Exercise the shortcut before the normal per-frame
                    // controls callback has adopted the standalone asset.
                    if show_controls || app.preview_controls.open {
                        show_controls = true;
                        app.show_preview_controls(ui.ctx(), false);
                    }
                },
                app,
            );
        assert_eq!(harness.state().preview_controls.target, Target::Source);
        assert!(!harness.state().source_preview_available());
        harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, egui::Key::Plus);
        harness.run_steps(2);
        assert_eq!(harness.state().pdfium_asset.controls_snapshot().zoom, 1.2);
        assert_eq!(harness.state().preview_controls.target, Target::Source);
        harness.get_by_label("Preview controls").click();
        harness.run_steps(3);
        assert!(harness.state().preview_controls.open);
        assert_eq!(
            harness.state().preview_controls.target,
            Target::Asset(harness.state().document().key())
        );
        harness.get_by_label("Zoom in").click();
        harness.run_steps(2);
        assert_eq!(harness.state().pdfium_asset.controls_snapshot().zoom, 1.44);
    }

    #[test]
    fn closing_an_open_source_control_owner_does_not_retarget_its_action_to_a_pdf_asset() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        let asset_path = app.document().path().clone();
        let source_tab = app.tabs.preview_id().unwrap();
        app.preview_controls.open = true;
        app.preview_controls.target = Target::Source;
        app.preview_controls.edit_events.push(egui::Event::Copy);
        app.activate_tab(source_tab, &context);
        app.finish_close_tab(&context);
        assert!(!app.source_preview_available());
        assert_eq!(app.document().kind(), DocumentKind::Pdf);
        assert_eq!(app.document().path(), &asset_path);
        app.preview_control_action(Action::ZoomIn);
        assert_eq!(
            app.pdfium_asset.controls_snapshot().zoom,
            1.0,
            "An action from retired source controls reached the unrelated PDF asset"
        );
        assert!(!app.preview_controls.open);
        assert!(
            app.preview_controls.edit_events.is_empty(),
            "retired controls retained a field command"
        );
    }

    #[test]
    fn toolbar_closing_controls_discards_pending_field_input() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        app.preview_controls
            .select(app.main_preview_controls_target().unwrap());
        app.preview_controls.open = true;
        app.preview_controls.focus_find = true;
        app.preview_controls.edit_events.push(egui::Event::Copy);
        app.toggle_main_preview_controls();
        assert!(!app.preview_controls.open);
        assert!(!app.preview_controls.focus_find);
        assert!(app.preview_controls.edit_events.is_empty());
        app.toggle_main_preview_controls();
        assert!(app.preview_controls.open);
        assert!(app.preview_controls.edit_events.is_empty());
    }

    #[test]
    fn obsolete_asset_control_actions_and_popouts_retire_on_document_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app =
            pinned_source_with_asset(&context, directory.path(), PreviewPreference::Pdfium);
        let key = app.document().key();
        app.preview_controls.asset_available = Some((
            key,
            Rect::from_min_size(egui::pos2(40.0, 80.0), egui::vec2(500.0, 600.0)),
        ));
        app.preview_controls.select(Target::Asset(key));
        app.preview_control_action(Action::PopOut);
        app.preview_controls.open = true;
        app.document_mut().replace_loaded_unprojected(
            String::new(),
            directory.path().join("other.pdf"),
            DocumentKind::Pdf,
            None,
        );
        let replaced = app.document().key();
        assert_ne!(key, replaced);
        app.preview_control_action(Action::ZoomIn);
        assert!(!app.preview_controls.open);
        assert_eq!(app.pdfium_asset.controls_snapshot().zoom, 1.0);
        assert_eq!(app.pdfium_preview.controls_snapshot().zoom, 1.0);
        app.show_preview_window(&context);
        assert!(app.preview_controls.popout.is_none());
        assert_eq!(app.document().key(), replaced);
        assert_eq!(
            app.preview_controls.available_for(Target::Asset(replaced)),
            None
        );
    }

    #[test]
    fn outline_uses_compiled_page_positions_and_bounds_untrusted_trees() {
        let mut controls = Controls::default();
        controls.set_outline(&serde_json::json!({"items":[
            {"title":"Chapter", "position":{"page_no":3,"x":12,"y":90}, "children":[
                {"title":"Section", "position":{"page_no":4,"x":20,"y":40}}
            ]},
            {"title":"Invalid", "position":{"page_no":0,"x":0,"y":0}}
        ]}));
        assert_eq!(controls.native_outline.len(), 2);
        assert_eq!(controls.native_outline[0].1.page, 2);
        assert_eq!(controls.native_outline[0].1.y, 90.0);
        assert_eq!(controls.native_outline[1].0, "  Section");
        let items = vec![
            serde_json::json!({"title":"Heading", "position":{"page_no":1,"x":0,"y":0}});
            3000
        ];
        controls.set_outline(&serde_json::json!({"items":items}));
        assert_eq!(controls.native_outline.len(), 2048);
        controls.set_outline(&serde_json::Value::Null);
        assert!(controls.native_outline.is_empty());
    }

    #[test]
    fn native_close_command_restores_preview_without_closing_its_document() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.view_mode = ViewMode::Split;
        let key = app.document().key();
        app.preview_control_action(Action::PopOut);
        let child = crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "preview-window");
        let mut harness = Harness::builder().build_ui_state(
            |ui, app: &mut EditorApp| {
                app.process_native_menu_commands(ui.ctx(), None);
            },
            app,
        );
        for id in [egui::ViewportId::ROOT, child] {
            harness.input_mut().viewports.entry(id).or_default().focused = Some(true);
        }
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::CloseTab);
        harness.run_steps(3);
        assert!(harness.state().preview_controls.popout.is_none());
        assert_eq!(harness.state().view_mode, ViewMode::Split);
        assert_eq!(harness.state().document().key(), key);
        assert!(!harness.state().tabs.is_empty());
    }

    #[test]
    fn outline_height_does_not_feed_back_from_the_previous_popup_size() {
        for count in [0, 3, 100] {
            let snapshot = Snapshot {
                focused: false,
                count: 3,
                zoom: 1.0,
                outline: (0..count).map(|i| (format!("Heading {i}"), 0)).collect(),
                ..Default::default()
            };
            let mut harness = Harness::builder()
                .with_size(egui::vec2(380.0, 108.0))
                .build_ui_state(
                    move |ui, state: &mut (Controls, f32)| {
                        let card = theme::popup_card_frame(ui.style()).show(ui, |ui| {
                            ui.set_width(344.0);
                            state.0.show(ui, &snapshot);
                        });
                        state.1 = card.response.rect.height();
                    },
                    (
                        Controls {
                            open: true,
                            outline: true,
                            ..Default::default()
                        },
                        0.0,
                    ),
                );
            harness.run_steps(3);
            let expected = harness.state().1;
            for _ in 0..10 {
                harness.set_size(egui::vec2(380.0, expected));
                harness.run_steps(2);
                assert_eq!(harness.state().1, expected, "outline count {count}");
            }
        }
    }

    #[test]
    fn queued_select_all_precedes_first_native_text_chunk() {
        let snapshot = Snapshot {
            count: 3,
            zoom: 1.0,
            ..Default::default()
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(380.0, 400.0))
            .build_ui_state(
                move |ui, controls: &mut Controls| {
                    controls.show(ui, &snapshot);
                },
                Controls {
                    open: true,
                    query: "old query".into(),
                    focus_find: true,
                    ..Default::default()
                },
            );
        harness.run_steps(3);
        for query in [
            "preview typing t Pdfium",
            "second preview query Interactive",
        ] {
            let shortcut = standard_text_edit_shortcut(AppCommand::SelectAll).unwrap();
            harness.state_mut().edit_events.push(egui::Event::Key {
                key: shortcut.logical_key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: shortcut.modifiers,
            });
            for chunk in query.as_bytes().chunks(16) {
                harness.input_mut().events.push(egui::Event::Text(
                    std::str::from_utf8(chunk).unwrap().into(),
                ));
                harness.run_steps(2);
            }
            assert_eq!(harness.state().query, query);
            assert!(harness.state().edit_events.is_empty());
        }
    }

    #[test]
    fn controls_have_one_layout_and_emit_backend_actions() {
        let snapshot = Snapshot {
            focused: false,
            page: 1,
            count: 4,
            zoom: 1.0,
            back: true,
            forward: true,
            outline: vec![("Introduction".into(), 0)],
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(380.0, 400.0))
            .build_ui_state(
                |ui, state: &mut (Controls, Option<Action>)| {
                    state.1 = state.0.show(ui, &snapshot);
                },
                (
                    Controls {
                        open: true,
                        ..Default::default()
                    },
                    None,
                ),
            );
        harness.run_steps(2);
        harness.get_by_label("Outline").click();
        harness.run_steps(2);
        harness.get_by_label("Introduction").click();
        harness.run_steps(1);
        assert!(matches!(harness.state().1, Some(Action::Outline(0))));
        harness.get_by_label("Minimize controls").click();
        harness.run_steps(2);
        assert!(!harness.state().0.open);
    }
    #[test]
    fn visible_pdf_controls_are_available_with_a_retained_code_mode() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_untitled_kind(DocumentKind::Pdf);
        app.view_mode = ViewMode::Code;
        assert!(app.preview_visible());
        assert!(app.preview_controls_enabled());
        app.preview_controls.target = Target::Asset(app.document().key());
        app.preview_controls.popout_target = app.preview_controls.target;
        app.preview_controls.popout = Some(ViewMode::Code);
        assert!(!app.preview_controls_enabled());
    }

    #[test]
    fn toolbar_toggle_is_next_to_mitex_and_disabled_in_code_mode() {
        for kind in [DocumentKind::Typst, DocumentKind::Tex] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.document_mut().replace_untitled_kind(kind);
            app.view_mode = ViewMode::Split;
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1400.0, 80.0))
                .build_ui_state(|ui, app: &mut EditorApp| app.show_toolbar(ui, None), app);
            harness.run_steps(3);
            // The untitled Typst entry compiles through the Typst CLI only
            // where no interactive Tinymist preview exists.
            if kind == DocumentKind::Tex || !cfg!(any(target_os = "macos", target_os = "windows")) {
                harness.get_by_label("Pause");
            } else {
                assert!(harness.query_by_label("Pause").is_none());
            }
            let button = harness.get_by_label("Preview controls");
            let next = harness.get_by_label(if kind == DocumentKind::Typst {
                "auto-miTeX"
            } else {
                "Find"
            });
            assert!(button.rect().right() <= next.rect().left());
            button.click();
            harness.run_steps(2);
            assert!(harness.state().preview_controls.open);
            harness.state_mut().view_mode = ViewMode::Code;
            harness.run_steps(2);
            assert!(!harness.state().preview_controls_enabled());
        }
    }
    #[test]
    fn popout_and_restore_do_not_create_another_document_or_build_owner() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        for mode in [ViewMode::Preview, ViewMode::Split] {
            app.view_mode = mode;
            let key = app.document().key();
            app.preview_control_action(Action::PopOut);
            assert_eq!(app.preview_controls.popout, Some(mode));
            assert_eq!(app.view_mode, ViewMode::Code);
            assert!(!app.preview_controls_enabled());
            assert!(app.preview_visible());
            app.restore_preview_window(&context);
            assert_eq!(app.view_mode, mode);
            assert_eq!(app.document().key(), key);
            app.preview_control_action(Action::PopOut);
            app.execute_app_command(AppCommand::Split, &context, None);
            assert!(app.preview_controls.popout.is_none());
        }
    }
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    #[test]
    fn stationary_pointer_does_not_push_a_moving_popup() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness = Harness::builder()
            .with_size(egui::vec2(380.0, 300.0))
            .build_ui_state(
                |ui, controls: &mut Controls| {
                    ui.ctx().set_embed_viewports(false);
                    controls.show(ui, &Snapshot::default());
                },
                Controls {
                    position: Some(egui::pos2(100.0, 200.0)),
                    ..Default::default()
                },
            );
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .inner_rect = Some(Rect::from_min_size(
            egui::pos2(100.0, 200.0),
            egui::vec2(380.0, 300.0),
        ));
        harness.run_steps(3);
        let start = harness.get_by_label("Preview").rect().center();
        harness.hover_at(start);
        harness.run_steps(1);
        harness.drag_at(start);
        harness.run_steps(1);
        harness.hover_at(start + egui::vec2(30.0, 40.0));
        harness.run_steps(2);
        let moved = harness.state().position.unwrap();
        assert_eq!(moved, egui::pos2(130.0, 240.0));
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .inner_rect = Some(Rect::from_min_size(moved, egui::vec2(380.0, 300.0)));
        harness.run_steps(6);
        assert_eq!(harness.state().position, Some(moved));
    }

    #[test]
    fn clamped_popup_does_not_schedule_movement_for_an_unchanged_pointer() {
        let mut anchor = DragAnchor {
            pointer_start: egui::pos2(20.0, 30.0),
            window_start: egui::pos2(10.0, 10.0),
            pointer_last: egui::pos2(20.0, 30.0),
        };
        let outside = egui::pos2(500.0, 600.0);
        assert_eq!(anchor.move_to(outside), Some(egui::pos2(490.0, 580.0)));
        // Parent clamping can alter the actual window position. It does not
        // turn a repeated pointer sample into another movement request.
        for _ in 0..8 {
            assert_eq!(anchor.move_to(outside), None);
        }
        assert_eq!(
            anchor.move_to(egui::pos2(25.0, 35.0)),
            Some(egui::pos2(15.0, 15.0))
        );
    }

    #[test]
    fn moving_viewport_does_not_reduce_pointer_travel() {
        let anchor = (egui::pos2(120.0, 220.0), egui::pos2(100.0, 200.0));
        let first = drag_position(anchor, egui::pos2(144.0, 232.0));
        assert_eq!(first, egui::pos2(124.0, 212.0));
        // The pointer's local position changes as the viewport follows it.
        assert_eq!(
            drag_position(anchor, first + egui::vec2(44.0, 32.0)),
            egui::pos2(148.0, 224.0)
        );
        assert_eq!(drag_position(anchor, egui::pos2(120.0, 220.0)), anchor.1);
    }
}
