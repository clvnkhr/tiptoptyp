use eframe::egui::{self, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

use crate::{screenshot::CaptureController, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildViewRole {
    Persistent,
    Modal,
    DismissOnBlur,
    Tooltip,
}

impl ChildViewRole {
    fn focus_policy(self) -> FocusPolicy {
        match self {
            Self::Persistent => FocusPolicy::Preserve,
            Self::Modal => FocusPolicy::SuspendOnBlur,
            Self::DismissOnBlur => FocusPolicy::DismissOnBlur,
            Self::Tooltip => FocusPolicy::InteractiveHandoff,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusPolicy {
    Preserve,
    SuspendOnBlur,
    DismissOnBlur,
    InteractiveHandoff,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ChildViewBounds {
    Persistent { inner: Vec2, minimum: Vec2 },
    Fixed { position: Pos2, size: Vec2 },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChildViewSpec {
    id_salt: &'static str,
    title: &'static str,
    role: ChildViewRole,
    bounds: ChildViewBounds,
    capture_target: &'static str,
    active: bool,
    mouse_passthrough: bool,
    visible: Option<bool>,
    dormant_host: bool,
}

impl ChildViewSpec {
    pub(crate) fn persistent(
        id_salt: &'static str,
        title: &'static str,
        inner: impl Into<Vec2>,
        minimum: impl Into<Vec2>,
        capture_target: &'static str,
    ) -> Self {
        Self {
            id_salt,
            title,
            role: ChildViewRole::Persistent,
            bounds: ChildViewBounds::Persistent {
                inner: inner.into(),
                minimum: minimum.into(),
            },
            capture_target,
            active: true,
            mouse_passthrough: false,
            visible: None,
            dormant_host: false,
        }
    }

    pub(crate) fn modal(
        id_salt: &'static str,
        title: &'static str,
        owner: Rect,
        capture_target: &'static str,
    ) -> Self {
        Self {
            id_salt,
            title,
            role: ChildViewRole::Modal,
            bounds: ChildViewBounds::Fixed {
                position: owner.min,
                size: owner.size(),
            },
            capture_target,
            active: true,
            mouse_passthrough: false,
            visible: None,
            dormant_host: false,
        }
    }

    pub(crate) fn dismiss_on_blur(
        id_salt: &'static str,
        title: &'static str,
        position: Pos2,
        size: Vec2,
        capture_target: &'static str,
    ) -> Self {
        Self {
            id_salt,
            title,
            role: ChildViewRole::DismissOnBlur,
            bounds: ChildViewBounds::Fixed { position, size },
            capture_target,
            active: true,
            mouse_passthrough: false,
            visible: None,
            dormant_host: false,
        }
    }

    pub(crate) fn tooltip(
        id_salt: &'static str,
        title: &'static str,
        position: Pos2,
        size: Vec2,
        active: bool,
        capture_target: &'static str,
    ) -> Self {
        Self {
            id_salt,
            title,
            role: ChildViewRole::Tooltip,
            bounds: ChildViewBounds::Fixed { position, size },
            capture_target,
            active,
            mouse_passthrough: false,
            visible: None,
            dormant_host: false,
        }
    }

    /// A fixed, modeless tool surface above native document content.
    pub(crate) fn modeless_popup(
        id_salt: &'static str,
        title: &'static str,
        bounds: Rect,
        active: bool,
        capture_target: &'static str,
    ) -> Self {
        Self {
            role: ChildViewRole::Persistent,
            ..Self::tooltip(
                id_salt,
                title,
                bounds.min,
                bounds.size(),
                active,
                capture_target,
            )
        }
    }

    pub(crate) fn with_visible(mut self, visible: bool) -> Self {
        self.visible = Some(visible);
        self
    }

    pub(crate) fn with_dormant_hosting(mut self, dormant_host: bool) -> Self {
        self.dormant_host = dormant_host;
        self
    }

    fn viewport(self) -> egui::ViewportBuilder {
        let viewport = match self.bounds {
            ChildViewBounds::Persistent { inner, minimum } => crate::window_policy::document()
                .with_title(self.title)
                .with_inner_size(inner)
                .with_min_inner_size(minimum)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
            ChildViewBounds::Fixed { position, size } => {
                let builder = theme::popup_viewport_builder(self.title)
                    .with_position(position)
                    .with_inner_size(size)
                    .with_active(self.active)
                    .with_mouse_passthrough(self.mouse_passthrough);
                // Modeless controls resize with their content. Conflicting old
                // min/max constraints must not fight an expand/collapse update.
                if self.role == ChildViewRole::Persistent {
                    builder
                } else {
                    builder.with_min_inner_size(size).with_max_inner_size(size)
                }
            }
        };
        let mut viewport = decorate_child_viewport(viewport);
        viewport.visible = self.visible;
        viewport
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ChildViewInput {
    pub(crate) focused: Option<bool>,
    pub(crate) close_requested: bool,
    pub(crate) escape_pressed: bool,
}

pub(crate) struct ChildViewHost;

pub(crate) use crate::window_host::Lifecycle as ChildViewLifecycle;
use crate::window_host::Token as LifecycleToken;

fn begin_lifecycle(
    context: &egui::Context,
    id: egui::ViewportId,
    spec: ChildViewSpec,
) -> Option<LifecycleToken> {
    crate::window_host::show(
        context,
        id,
        context.viewport_id(),
        spec.visible != Some(false),
        spec.dormant_host,
    )
}
fn callback_is_live(context: &egui::Context, token: LifecycleToken) -> bool {
    crate::window_host::live(context, token)
}

struct ChildPaint {
    token: LifecycleToken,
    spec: ChildViewSpec,
    appearance: egui::Theme,
    captures: CaptureController,
    style: std::sync::Arc<egui::Style>,
}
impl ChildPaint {
    fn paint(
        &self,
        ui: &mut egui::Ui,
        class: egui::ViewportClass,
        mut body: impl FnMut(&mut egui::Ui, ChildViewInput),
    ) {
        if !callback_is_live(ui.ctx(), self.token) {
            return;
        }
        let _span = crate::performance::span(self.spec.capture_target);
        self.captures
            .begin_viewport(ui.ctx(), self.spec.capture_target);
        ui.set_style(self.style.clone());
        if class != egui::ViewportClass::EmbeddedWindow {
            sync_native_theme(ui.ctx(), self.appearance);
        }
        let input = ui.ctx().input(|input| ChildViewInput {
            focused: input.viewport().focused,
            close_requested: input.viewport().close_requested(),
            escape_pressed: input.key_pressed(egui::Key::Escape),
        });
        body(ui, input);
        self.captures
            .end_glow_viewport(ui, self.spec.capture_target);
    }
}

impl ChildViewHost {
    /// Independent child painting avoids an editor paint and a nested GL
    /// buffer swap for every child event. The caller owns input invalidation
    /// and returns application actions separately from viewport-local state.
    pub(crate) fn show_deferred(
        context: &egui::Context,
        captures: &CaptureController,
        spec: ChildViewSpec,
        appearance: egui::Theme,
        style: &std::sync::Arc<egui::Style>,
        body: impl Fn(&mut egui::Ui, ChildViewInput) + Send + Sync + 'static,
    ) {
        let id = scoped_child_viewport_id(context, spec.id_salt);
        let Some(token) = begin_lifecycle(context, id, spec) else {
            return;
        };
        let paint = ChildPaint {
            token,
            spec,
            appearance,
            captures: captures.clone(),
            style: style.clone(),
        };
        crate::viewport_fonts::show_deferred(context, id, spec.viewport(), move |ui, class| {
            paint.paint(ui, class, |ui, input| body(ui, input));
        });
    }

    /// Applies the host-owned viewport, style, native theme and capture
    /// lifecycle, leaving the body responsible only for view-specific UI and
    /// actions.
    pub(crate) fn show(
        context: &egui::Context,
        captures: &CaptureController,
        spec: ChildViewSpec,
        appearance: egui::Theme,
        style: &std::sync::Arc<egui::Style>,
        mut body: impl FnMut(&mut egui::Ui, ChildViewInput),
    ) {
        debug_assert!(matches!(
            (spec.role, spec.role.focus_policy()),
            (ChildViewRole::Persistent, FocusPolicy::Preserve)
                | (ChildViewRole::Modal, FocusPolicy::SuspendOnBlur)
                | (ChildViewRole::DismissOnBlur, FocusPolicy::DismissOnBlur)
                | (ChildViewRole::Tooltip, FocusPolicy::InteractiveHandoff)
        ));
        let id = scoped_child_viewport_id(context, spec.id_salt);
        let Some(token) = begin_lifecycle(context, id, spec) else {
            return;
        };
        let viewport = spec.viewport();
        let paint = ChildPaint {
            token,
            spec,
            appearance,
            captures: captures.clone(),
            style: style.clone(),
        };
        crate::viewport_fonts::show_immediate(context, id, viewport, |ui, class| {
            paint.paint(ui, class, &mut body);
        });
    }

    pub(crate) fn hide(context: &egui::Context, salt: &'static str) {
        let id = scoped_child_viewport_id(context, salt);
        if crate::window_host::transition(context, id, ChildViewLifecycle::TemporarilyHidden) {
            context.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
        }
    }

    pub(crate) fn close(context: &egui::Context, salt: &'static str) {
        let id = scoped_child_viewport_id(context, salt);
        if crate::window_host::transition(context, id, ChildViewLifecycle::DurablyClosed) {
            context.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
            crate::font_preview::dispose_viewport(context, id);
        }
    }

    pub(crate) fn dormant_owner(context: &egui::Context) {
        let owner = context.viewport_id();
        let closed = crate::window_host::retire_owner(context, owner);
        for id in closed {
            context.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
            crate::font_preview::dispose_viewport(context, id);
        }
    }

    pub(crate) fn resume_owner(context: &egui::Context) {
        let owner = context.viewport_id();
        crate::window_host::resume_owner(context, owner);
    }
}

fn sync_native_theme(context: &egui::Context, appearance: egui::Theme) {
    let id = viewport_scoped_id(context, "native-child-theme");
    let changed = context.data_mut(|data| {
        let previous = data.get_temp::<egui::Theme>(id);
        data.insert_temp(id, appearance);
        previous != Some(appearance)
    });
    // Commands request another repaint even when the OS value is unchanged.
    // The frame counter restarts when egui recreates a closed viewport; its
    // context data can outlive that window, so a reopened child must sync again.
    if changed || context.cumulative_frame_nr() == 0 {
        context.send_viewport_cmd(egui::ViewportCommand::SetTheme(theme::native_theme(
            appearance,
        )));
    }
}

pub(crate) fn viewport_scoped_id(context: &egui::Context, salt: &'static str) -> egui::Id {
    egui::Id::new((context.viewport_id(), salt))
}

pub(crate) fn scoped_child_viewport_id(
    context: &egui::Context,
    salt: &'static str,
) -> egui::ViewportId {
    child_viewport_id(context.viewport_id(), salt)
}

pub(crate) fn child_viewport_id(owner: egui::ViewportId, salt: &'static str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of((owner, salt))
}

fn decorate_child_viewport(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    #[cfg(not(target_os = "macos"))]
    {
        viewport.with_decorations(false)
    }
    #[cfg(target_os = "macos")]
    {
        viewport
    }
}

pub(crate) const POPUP_BLUR_GRACE: Duration = Duration::from_millis(120);

pub(crate) fn popup_focus_should_close(
    had_focus: &mut bool,
    blur_started: &mut Option<Instant>,
    focused: Option<bool>,
    now: Instant,
) -> bool {
    match focused {
        Some(true) => {
            *had_focus = true;
            *blur_started = None;
            false
        }
        Some(false) if *had_focus => {
            let started = *blur_started.get_or_insert(now);
            now.saturating_duration_since(started) >= POPUP_BLUR_GRACE
        }
        Some(false) | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn modeless_tool_surfaces_use_popup_alpha_without_blur_dismissal() {
        let spec = ChildViewSpec::modeless_popup(
            "find",
            "Find",
            Rect::from_min_size(Pos2::ZERO, Vec2::new(620.0, 90.0)),
            false,
            "find-replace",
        );
        assert_eq!(spec.role.focus_policy(), FocusPolicy::Preserve);
        let viewport = spec.viewport();
        assert_eq!(viewport.transparent, Some(true));
        assert_eq!(viewport.decorations, Some(false));
        assert_eq!(viewport.active, Some(false));
        assert_eq!(viewport.min_inner_size, None);
        assert_eq!(viewport.max_inner_size, None);
        assert_eq!(viewport.resizable, Some(false));
    }

    #[test]
    fn deferred_tooltip_does_not_paint_during_parent_frames() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let paints = Arc::new(AtomicUsize::new(0));
        let captures = CaptureController::disabled_for_tests();
        let child = scoped_child_viewport_id(&context, "test-tooltip");
        let register = || {
            context.run_ui(egui::RawInput::default(), |ui| {
                let paints = paints.clone();
                ChildViewHost::show_deferred(
                    ui.ctx(),
                    &captures,
                    ChildViewSpec::tooltip(
                        "test-tooltip",
                        "Tooltip",
                        Pos2::ZERO,
                        Vec2::new(200.0, 100.0),
                        false,
                        "diagnostic",
                    ),
                    egui::Theme::Dark,
                    &ui.ctx().style_of(egui::Theme::Dark),
                    move |ui, _| {
                        paints.fetch_add(1, Ordering::Relaxed);
                        ui.label("Independent popup");
                    },
                );
            })
        };
        let mut output = register();
        assert_eq!(paints.load(Ordering::Relaxed), 0);
        let viewport = &output.viewport_output[&child];
        assert!(viewport.class == egui::ViewportClass::Deferred);
        let callback = viewport.viewport_ui_cb.clone().unwrap();
        output.textures_delta.clear();
        let mut input = egui::RawInput {
            viewport_id: child,
            ..Default::default()
        };
        input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        context
            .run_ui(input, |ui| callback(ui))
            .drop_without_applying_deltas();
        assert!(paints.load(Ordering::Relaxed) > 0);
        let count = paints.load(Ordering::Relaxed);
        for _ in 0..10 {
            register().drop_without_applying_deltas();
        }
        assert_eq!(
            paints.load(Ordering::Relaxed),
            count,
            "editor frames must not render the child"
        );
    }

    #[test]
    fn persistent_child_settles_and_reapplies_native_theme_after_change_or_reopen() {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let captures = CaptureController::disabled_for_tests();
        let outputs = Rc::new(RefCell::new(Vec::new()));
        let child_outputs = outputs.clone();
        egui::Context::set_immediate_viewport_renderer(move |context, mut child| {
            let mut input = egui::RawInput {
                viewport_id: child.ids.this,
                ..Default::default()
            };
            input.viewports.insert(
                child.ids.this,
                egui::ViewportInfo {
                    parent: Some(child.ids.parent),
                    ..Default::default()
                },
            );
            let mut output = context.run_ui(input, |ui| (child.viewport_ui_cb)(ui));
            let viewport = &output.viewport_output[&child.ids.this];
            child_outputs
                .borrow_mut()
                .push((viewport.commands.clone(), viewport.repaint_delay));
            output.textures_delta.clear();
        });
        let render = |appearance: Option<egui::Theme>| {
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.label("Editor");
                if let Some(appearance) = appearance {
                    ChildViewHost::show(
                        ui.ctx(),
                        &captures,
                        ChildViewSpec::persistent(
                            "settings",
                            "Settings",
                            [500.0, 560.0],
                            [360.0, 300.0],
                            "settings",
                        ),
                        appearance,
                        &ui.ctx().style_of(appearance),
                        |ui, _| {
                            ui.label("Idle Settings");
                        },
                    );
                }
            });
            // Commands from immediate children are delivered with the parent output.
            let child_id = scoped_child_viewport_id(&context, "settings");
            if let Some(child) = output.viewport_output.get(&child_id)
                && let Some((commands, _)) = outputs.borrow_mut().last_mut()
            {
                commands.extend(child.commands.clone());
            }
            output.textures_delta.clear();
        };
        for _ in 0..6 {
            render(Some(egui::Theme::Dark));
        }
        let (commands, delay) = outputs.borrow().last().unwrap().clone();
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::SetTheme(_))),
            "idle frame sent {commands:?}"
        );
        assert_eq!(delay, Duration::MAX, "idle child scheduled another repaint");
        outputs.borrow_mut().clear();
        render(Some(egui::Theme::Light));
        assert!(
            outputs
                .borrow()
                .iter()
                .any(|(commands, _)| commands.iter().any(|command| matches!(
                    command,
                    egui::ViewportCommand::SetTheme(egui::SystemTheme::Light)
                )))
        );
        for _ in 0..3 {
            render(None);
        }
        outputs.borrow_mut().clear();
        render(Some(egui::Theme::Light));
        assert!(
            outputs
                .borrow()
                .iter()
                .any(|(commands, _)| commands.iter().any(|command| matches!(
                    command,
                    egui::ViewportCommand::SetTheme(egui::SystemTheme::Light)
                ))),
            "recreated child needs its native appearance restored"
        );
    }

    #[test]
    fn modal_spec_tracks_its_owner_viewport_exactly() {
        let owner = Rect::from_min_size(Pos2::new(20.0, 40.0), Vec2::new(900.0, 600.0));
        let spec = ChildViewSpec::modal("modal", "Modal", owner, "modal");
        let ChildViewBounds::Fixed { position, size } = spec.bounds else {
            panic!("modal must have fixed owner-relative bounds");
        };
        assert_eq!(position, owner.min);
        assert_eq!(size, owner.size());
        assert_eq!(spec.role.focus_policy(), FocusPolicy::SuspendOnBlur);
    }

    #[test]
    fn persistent_spec_keeps_requested_minimum() {
        let spec = ChildViewSpec::persistent(
            "settings",
            "Settings",
            Vec2::new(800.0, 700.0),
            Vec2::new(480.0, 360.0),
            "settings",
        );
        let ChildViewBounds::Persistent { inner, minimum } = spec.bounds else {
            panic!("settings must remain a persistent child");
        };
        assert_eq!(inner, Vec2::new(800.0, 700.0));
        assert_eq!(minimum, Vec2::new(480.0, 360.0));
        assert_eq!(spec.role.focus_policy(), FocusPolicy::Preserve);
    }

    #[test]
    fn hide_reopen_close_and_late_callbacks_have_distinct_lifecycles() {
        let context = egui::Context::default();
        let id = scoped_child_viewport_id(&context, "lifecycle");
        let spec = ChildViewSpec::tooltip(
            "lifecycle",
            "Lifecycle",
            Pos2::ZERO,
            Vec2::splat(100.0),
            true,
            "lifecycle",
        );
        let first = begin_lifecycle(&context, id, spec).unwrap();
        assert!(callback_is_live(&context, first));

        ChildViewHost::hide(&context, "lifecycle");
        assert!(!callback_is_live(&context, first));
        let reopened = begin_lifecycle(&context, id, spec).unwrap();
        assert!(callback_is_live(&context, reopened));
        assert_eq!(first.generation, reopened.generation);

        ChildViewHost::close(&context, "lifecycle");
        assert!(!callback_is_live(&context, first));
        assert!(!callback_is_live(&context, reopened));
        let recreated = begin_lifecycle(&context, id, spec).unwrap();
        assert!(callback_is_live(&context, recreated));
        assert_ne!(reopened.generation, recreated.generation);
    }

    #[test]
    fn dormant_owner_keeps_only_explicit_native_hosts_alive() {
        let context = egui::Context::default();
        let transient_id = scoped_child_viewport_id(&context, "transient");
        let hosted_id = scoped_child_viewport_id(&context, "hosted");
        let transient = begin_lifecycle(
            &context,
            transient_id,
            ChildViewSpec::dismiss_on_blur(
                "transient",
                "Transient",
                Pos2::ZERO,
                Vec2::splat(20.0),
                "transient",
            ),
        )
        .unwrap();
        let hosted = begin_lifecycle(
            &context,
            hosted_id,
            ChildViewSpec::persistent(
                "hosted",
                "Hosted",
                Vec2::splat(100.0),
                Vec2::splat(50.0),
                "hosted",
            )
            .with_dormant_hosting(true),
        )
        .unwrap();
        ChildViewHost::dormant_owner(&context);
        assert!(!callback_is_live(&context, transient));
        assert!(callback_is_live(&context, hosted));
        assert!(
            begin_lifecycle(
                &context,
                transient_id,
                ChildViewSpec::dismiss_on_blur(
                    "transient",
                    "Transient",
                    Pos2::ZERO,
                    Vec2::splat(20.0),
                    "transient",
                ),
            )
            .is_none()
        );
        ChildViewHost::resume_owner(&context);
        assert!(
            begin_lifecycle(
                &context,
                transient_id,
                ChildViewSpec::dismiss_on_blur(
                    "transient",
                    "Transient",
                    Pos2::ZERO,
                    Vec2::splat(20.0),
                    "transient",
                ),
            )
            .is_some()
        );
    }
}
