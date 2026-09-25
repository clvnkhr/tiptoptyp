//! One presentation for both preview engines. Backends only exchange state/actions.
use super::*;

pub(super) const SALT: &str = "preview-controls";
#[derive(Clone, Default, serde::Deserialize)]
pub(super) struct Snapshot {
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
#[derive(Default)]
pub(super) struct Controls {
    pub open: bool,
    pub popout: Option<ViewMode>,
    pub outline: bool,
    pub query: String,
    pub edit_events: Vec<egui::Event>,
    pub focus_find: bool,
    pub position: Option<Pos2>,
    pub available: Option<Rect>,
    pub web: Snapshot,
    pub native_outline: Vec<(String, Location)>,
    measured_size: Option<(bool, usize, egui::Vec2)>,
}
impl Controls {
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
        ui.ctx()
            .input_mut(|input| input.events.append(&mut self.edit_events));
        let mut action = None;
        ui.horizontal(|ui| {
            let handle = ui.add(egui::Label::new("Preview").sense(Sense::drag()));
            if handle.dragged() {
                *self.position.get_or_insert(Pos2::ZERO) += handle.drag_delta();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, UiIcon::Down, "Minimize controls").clicked() {
                    self.open = false;
                }
                if ui
                    .add_enabled(self.popout.is_none(), egui::Button::new("Pop out"))
                    .clicked()
                {
                    action = Some(Action::PopOut);
                }
                ui.toggle_value(&mut self.outline, "Outline");
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
                if response.on_hover_text(label).clicked() {
                    action = Some(navigation);
                }
            }
            ui.separator();
            if icon_button(ui, UiIcon::Previous, "Previous page").clicked() {
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
            if icon_button(ui, UiIcon::Next, "Next page").clicked() {
                action = Some(Action::Page(
                    (snapshot.page + 1).min(snapshot.count.saturating_sub(1)),
                ));
            }
            ui.separator();
            if icon_button(ui, UiIcon::ZoomOut, "Zoom out").clicked() {
                action = Some(Action::ZoomOut);
            }
            if icon_button(ui, UiIcon::ZoomIn, "Zoom in").clicked() {
                action = Some(Action::ZoomIn);
            }
            if icon_button(ui, UiIcon::FitWidth, "Fit page width").clicked() {
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
            if self.focus_find {
                response.request_focus();
                self.focus_find = false;
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if icon_button(ui, UiIcon::Next, "Find next").clicked() || enter {
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
                        if ui
                            .add(egui::Button::new(title).wrap_mode(egui::TextWrapMode::Truncate))
                            .on_hover_text(title)
                            .clicked()
                        {
                            action = Some(Action::Outline(index));
                        }
                    }
                });
        }
        action
    }
}
impl EditorApp {
    pub(super) fn preview_controls_enabled(&self) -> bool {
        !self.tabs.is_empty()
            && self.view_mode != ViewMode::Code
            && (self.source_preview_available() || self.document().kind() == DocumentKind::Pdf)
    }
    pub(super) fn show_preview_controls(&mut self, context: &egui::Context) {
        // While detached, this method is called only inside the preview window.
        let in_popout = self.preview_controls.popout.is_some();
        if !self.preview_controls.open || (!self.preview_controls_enabled() && !in_popout) {
            ChildViewHost::close(context, SALT);
            return;
        }
        let native = self.interactive_preview_active();
        let snapshot = if native {
            let mut snapshot = self.preview_controls.web.clone();
            snapshot.outline = self
                .preview_controls
                .native_outline
                .iter()
                .map(|(title, location)| (title.clone(), location.page))
                .collect();
            snapshot
        } else if self.pdfium_asset_requested() {
            self.pdfium_asset.controls_snapshot()
        } else {
            self.pdfium_preview.controls_snapshot()
        };
        let available = self
            .preview_controls
            .available
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
                        self.preview_controls.open = false;
                    }
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
        if let Some(action) = action {
            self.preview_control_action(action);
            if !self.preview_controls.open {
                ChildViewHost::close(context, SALT);
            }
        }
    }
    pub(super) fn restore_preview_window(&mut self, context: &egui::Context) {
        if let Some(mode) = self.preview_controls.popout.take() {
            self.view_mode = mode;
            self.preview_controls.position = None;
            self.preview_controls.open = false;
            self.discard_webview();
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
        if self.tabs.is_empty() {
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
                            self.preview_controls.open = !self.preview_controls.open;
                        }
                    });
                });
                egui::CentralPanel::default()
                    .frame(theme::content_panel_frame(ui.style()).inner_margin(0.0))
                    .show(ui, |ui| self.show_preview(ui, None));
                self.show_preview_controls(ui.ctx());
            },
        );
        if close {
            self.restore_preview_window(context);
        }
    }
    pub(super) fn preview_control_action(&mut self, action: Action) {
        if matches!(action, Action::PopOut) {
            self.preview_controls.popout = Some(self.view_mode);
            self.view_mode = ViewMode::Code;
            self.preview_controls.open = false;
            self.preview_controls.position = None;
            self.discard_webview();
            return;
        }
        if self.interactive_preview_active() {
            match &action {
                Action::Find(query) => self.handle_web_action(
                    &serde_json::json!({"type":"find","query":query}).to_string(),
                ),
                Action::Outline(index) => {
                    if let Some((_, location)) = self.preview_controls.native_outline.get(*index) {
                        self.preview_control_action(Action::Location(location.clone()));
                    }
                    return;
                }
                _ => {}
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some(webview) = &self.webview {
                let _ = webview.evaluate_script(&format!(
                    "window.tiptoptypPreviewAction?.({})",
                    serde_json::to_string(&action).unwrap()
                ));
            }
        } else if self.pdfium_asset_requested() {
            self.pdfium_asset.controls_action(action);
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
    fn controls_have_one_layout_and_emit_backend_actions() {
        let snapshot = Snapshot {
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
            let button = harness.get_by_label("Preview controls");
            let next = harness.get_by_label(if kind == DocumentKind::Typst {
                "miTeX"
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
