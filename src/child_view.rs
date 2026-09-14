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
    focus: FocusPolicy,
    bounds: ChildViewBounds,
    capture_target: &'static str,
    active: bool,
    mouse_passthrough: bool,
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
            focus: FocusPolicy::Preserve,
            bounds: ChildViewBounds::Persistent {
                inner: inner.into(),
                minimum: minimum.into(),
            },
            capture_target,
            active: true,
            mouse_passthrough: false,
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
            focus: FocusPolicy::SuspendOnBlur,
            bounds: ChildViewBounds::Fixed {
                position: owner.min,
                size: owner.size(),
            },
            capture_target,
            active: true,
            mouse_passthrough: false,
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
            focus: FocusPolicy::DismissOnBlur,
            bounds: ChildViewBounds::Fixed { position, size },
            capture_target,
            active: true,
            mouse_passthrough: false,
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
            focus: FocusPolicy::InteractiveHandoff,
            bounds: ChildViewBounds::Fixed { position, size },
            capture_target,
            active,
            mouse_passthrough: false,
        }
    }

    fn viewport(self) -> egui::ViewportBuilder {
        let viewport = match self.bounds {
            ChildViewBounds::Persistent { inner, minimum } => egui::ViewportBuilder::default()
                .with_title(self.title)
                .with_inner_size(inner)
                .with_min_inner_size(minimum)
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
            ChildViewBounds::Fixed { position, size } => theme::popup_viewport_builder(self.title)
                .with_position(position)
                .with_inner_size(size)
                .with_min_inner_size(size)
                .with_max_inner_size(size)
                .with_active(self.active)
                .with_mouse_passthrough(self.mouse_passthrough),
        };
        decorate_child_viewport(viewport)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ChildViewInput {
    pub(crate) focused: Option<bool>,
    pub(crate) close_requested: bool,
    pub(crate) escape_pressed: bool,
}

pub(crate) struct ChildViewHost;

impl ChildViewHost {
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
            (spec.role, spec.focus),
            (ChildViewRole::Persistent, FocusPolicy::Preserve)
                | (ChildViewRole::Modal, FocusPolicy::SuspendOnBlur)
                | (ChildViewRole::DismissOnBlur, FocusPolicy::DismissOnBlur)
                | (ChildViewRole::Tooltip, FocusPolicy::InteractiveHandoff)
        ));
        let id = scoped_child_viewport_id(context, spec.id_salt);
        let viewport = spec.viewport();
        let native_theme = theme::native_theme(appearance);
        crate::viewport_fonts::show_immediate(context, id, viewport, |ui, _class| {
            captures.begin_viewport(ui.ctx(), spec.capture_target);
            ui.set_style(style.clone());
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
            let input = ui.ctx().input(|input| ChildViewInput {
                focused: input.viewport().focused,
                close_requested: input.viewport().close_requested(),
                escape_pressed: input.key_pressed(egui::Key::Escape),
            });
            body(ui, input);
            captures.end_glow_viewport(ui, spec.capture_target);
        });
    }
}

pub(crate) fn viewport_scoped_id(context: &egui::Context, salt: &'static str) -> egui::Id {
    egui::Id::new((context.viewport_id(), salt))
}

pub(crate) fn scoped_child_viewport_id(
    context: &egui::Context,
    salt: &'static str,
) -> egui::ViewportId {
    egui::ViewportId::from_hash_of((context.viewport_id(), salt))
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

    #[test]
    fn modal_spec_tracks_its_owner_viewport_exactly() {
        let owner = Rect::from_min_size(Pos2::new(20.0, 40.0), Vec2::new(900.0, 600.0));
        let spec = ChildViewSpec::modal("modal", "Modal", owner, "modal");
        let ChildViewBounds::Fixed { position, size } = spec.bounds else {
            panic!("modal must have fixed owner-relative bounds");
        };
        assert_eq!(position, owner.min);
        assert_eq!(size, owner.size());
        assert_eq!(spec.focus, FocusPolicy::SuspendOnBlur);
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
        assert_eq!(spec.focus, FocusPolicy::Preserve);
    }
}
