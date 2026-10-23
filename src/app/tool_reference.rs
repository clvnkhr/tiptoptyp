//! On-demand local reference cards and explicit tool-preset actions.
use crate::{
    settings::AppSettings,
    shortcuts::{ShortcutAction, ShortcutBindings},
    tool_profiles::{self, ToolProfile},
    tool_reference::{ReferenceId, reference},
};
use eframe::egui;
use std::sync::LazyLock;

#[derive(Clone, Copy)]
struct OpenReference {
    id: ReferenceId,
    verified_pinned: bool,
}

fn state_id(context: &egui::Context, salt: &str) -> egui::Id {
    egui::Id::new(("offline-tool-reference", context.viewport_id(), salt))
}

// Build pins never change during a process. Avoid formatting version strings
// on every Settings repaint merely to decide whether a button needs a badge.
static NEEDS_REVIEW: LazyLock<Vec<ReferenceId>> = LazyLock::new(|| {
    ReferenceId::ALL
        .into_iter()
        .filter(|id| reference(*id).warning(true).is_some())
        .collect()
});

pub(super) fn button(ui: &mut egui::Ui, id: ReferenceId, verified_pinned: bool) {
    button_named(ui, id, verified_pinned, "Reference");
}

pub(super) fn button_named(ui: &mut egui::Ui, id: ReferenceId, verified_pinned: bool, title: &str) {
    let label = if verified_pinned && NEEDS_REVIEW.contains(&id) {
        format!("{title} ⚠")
    } else {
        title.to_owned()
    };
    let response = ui.small_button(label);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("{} offline reference", reference(id).title),
        )
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.clip_rect().contains_rect(response.rect) {
        crate::desktop_test::observe(&format!("settings.reference.{}", id.key()), &response);
    }
    if response.clicked() {
        let key = state_id(ui.ctx(), "open");
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                key,
                OpenReference {
                    id,
                    verified_pinned,
                },
            )
        });
        ui.ctx().request_repaint();
    }
}

pub(super) fn profile_buttons(ui: &mut egui::Ui, id: ReferenceId, settings: &AppSettings) {
    if !tool_profiles::supports_verbose(id) {
        return;
    }
    let selected = tool_profiles::selected(id, settings);
    for profile in ToolProfile::ALL {
        let available = profile != ToolProfile::Verbose || tool_profiles::supports_verbose(id);
        let response = ui.add_enabled(
            available,
            egui::Button::new(profile.label())
                .small()
                .selected(selected == Some(profile)),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                available,
                format!("{} {} preset", reference(id).title, profile.label()),
            )
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        if ui.clip_rect().contains_rect(response.rect) {
            crate::desktop_test::observe(
                &format!("settings.profile.{}.{}", id.key(), profile.label()),
                &response,
            );
        }
        if response.clicked() {
            let key = state_id(ui.ctx(), "preset");
            ui.ctx()
                .data_mut(|data| data.insert_temp(key, Some((id, profile))));
            ui.ctx().request_repaint();
        }
    }
}

pub(super) fn controls(
    ui: &mut egui::Ui,
    id: ReferenceId,
    verified_pinned: bool,
    settings: &AppSettings,
) {
    button(ui, id, verified_pinned);
    profile_buttons(ui, id, settings);
}

pub(super) fn apply_requested(context: &egui::Context, edited: &mut AppSettings) {
    let key = state_id(context, "preset");
    if let Some((id, profile)) = context
        .data_mut(|data| data.remove_temp::<Option<(ReferenceId, ToolProfile)>>(key))
        .flatten()
        && let Err(error) = tool_profiles::apply(id, profile, edited)
    {
        let key = state_id(context, "error");
        context.data_mut(|data| data.insert_temp(key, error));
    }
}

pub(super) fn is_open(context: &egui::Context) -> bool {
    let open = state_id(context, "open");
    let error = state_id(context, "error");
    context.data(|data| {
        data.get_temp::<OpenReference>(open).is_some() || data.get_temp::<String>(error).is_some()
    })
}

pub(super) fn close(context: &egui::Context) {
    let open = state_id(context, "open");
    let error = state_id(context, "error");
    context.data_mut(|data| {
        data.remove::<OpenReference>(open);
        data.remove::<String>(error);
    });
}

/// A reference belongs to Settings. Close/Find must not operate its background
/// form, while native window-management shortcuts keep their normal owner.
pub(super) fn consume_shortcuts(context: &egui::Context, shortcuts: &ShortcutBindings) -> bool {
    if !is_open(context) {
        return false;
    }
    let mut handled = false;
    let mut closing = false;
    context.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } = event
            {
                let action = shortcuts.action_for_key_event(*key, *modifiers);
                if (*key == egui::Key::Escape && modifiers.is_none())
                    || matches!(
                        action,
                        Some(ShortcutAction::CloseTab | ShortcutAction::CloseWindow)
                    )
                {
                    closing = true;
                    handled = true;
                    return false;
                }
                if matches!(
                    action,
                    Some(ShortcutAction::Find | ShortcutAction::FindReplace)
                ) {
                    handled = true;
                    return false;
                }
            }
            true
        })
    });
    if closing {
        close(context);
    }
    handled
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(super) fn opened(context: &egui::Context, viewport: egui::ViewportId) -> Option<ReferenceId> {
    let id = egui::Id::new(("offline-tool-reference", viewport, "open"));
    context
        .data(|data| data.get_temp::<OpenReference>(id))
        .map(|open| open.id)
}

pub(super) fn show(ui: &mut egui::Ui, settings: &AppSettings) {
    let context = ui.ctx().clone();
    let error_id = state_id(&context, "error");
    let open_id = state_id(&context, "open");
    let error = context.data(|data| data.get_temp::<String>(error_id));
    let open = context.data(|data| data.get_temp::<OpenReference>(open_id));
    if error.is_none() && open.is_none() {
        return;
    }
    let mut closing = false;
    let result = egui::Modal::new(state_id(&context, "modal")).show(&context, |ui| {
        ui.set_width((ui.ctx().content_rect().width() - 40.0).clamp(200.0, 600.0));
        ui.horizontal_wrapped(|ui| {
            ui.strong(if error.is_some() {
                "Could not apply preset".into()
            } else {
                format!("{} · Offline reference", reference(open.unwrap().id).title)
            });
            let response = ui.small_button("×");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close reference")
            });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.reference.close", &response);
            closing |= response.clicked();
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(("offline-reference", open.map(|open| open.id)))
            .max_height((ui.ctx().content_rect().height() - 110.0).max(80.0))
            .show(ui, |ui| {
                if let Some(error) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if let Some(open) = open {
                    let entry = reference(open.id);
                    ui.label(format!("Reviewed: {}", entry.reviewed_version));
                    ui.weak(entry.status_label(open.verified_pinned));
                    if let Some(warning) = entry.warning(open.verified_pinned) {
                        ui.colored_label(ui.visuals().warn_fg_color, warning);
                    }
                    ui.horizontal_wrapped(|ui| profile_buttons(ui, open.id, settings));
                    if !tool_profiles::supports_verbose(open.id) {
                        ui.weak(
                            "This integration has no additional Problems diagnostics to enable.",
                        );
                    }
                    ui.label(entry.summary);
                    for example in entry.examples {
                        ui.strong(example.label);
                        let mut text = example.syntax;
                        super::code_editor::editor(
                            ui,
                            (open.id.key(), example.label),
                            &mut text,
                            example.language,
                            settings,
                            example.syntax.lines().count().clamp(1, 6),
                        );
                        ui.weak(example.explanation);
                    }
                    for note in entry.notes {
                        ui.label(*note);
                    }
                    if !entry.sources.is_empty() {
                        ui.separator();
                        ui.weak("Online documentation");
                        for source in entry.sources {
                            super::settings_controls::documentation_link(
                                ui,
                                source.title,
                                source.url,
                            );
                        }
                    }
                }
            });
    });
    if result.should_close() || closing {
        close(&context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    fn harness(size: egui::Vec2, id: ReferenceId) -> Harness<'static, AppSettings> {
        let mut harness = Harness::builder().with_size(size).build_ui_state(
            move |ui, settings: &mut AppSettings| {
                consume_shortcuts(ui.ctx(), &settings.effective_shortcuts());
                ui.horizontal(|ui| {
                    ui.label("Tool");
                    controls(ui, id, true, settings);
                });
                ui.label("Next control");
                apply_requested(ui.ctx(), settings);
                show(ui, settings);
            },
            AppSettings::default(),
        );
        crate::theme::configure_test_fonts(&harness.ctx);
        harness.run();
        harness
    }

    #[test]
    fn local_reference_does_not_edit_preferences_or_add_a_settings_row() {
        let id = ReferenceId::Ltex;
        let mut harness = harness(egui::vec2(730.0, 560.0), id);
        let before = harness.state().clone();
        let next = harness.get_by_label("Next control").rect();
        harness
            .get_by_label(&format!("{} offline reference", reference(id).title))
            .click();
        harness.run();
        assert!(is_open(&harness.ctx));
        assert_eq!(harness.state(), &before);
        assert_eq!(harness.get_by_label("Next control").rect(), next);
        harness.get_by_label(&format!("Reviewed: {}", reference(id).reviewed_version));
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
        harness.run();
        assert!(is_open(&harness.ctx));
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
        harness.run();
        assert!(!is_open(&harness.ctx));
        assert_eq!(harness.state(), &before);
    }

    #[test]
    fn presets_are_explicit_and_restore_normal_without_enabling_providers() {
        let id = ReferenceId::Ltex;
        let mut harness = harness(egui::vec2(730.0, 560.0), id);
        let before = harness.state().clone();
        let label = |profile| format!("{} {profile} preset", reference(id).title);
        harness.get_by_label(&label("Verbose")).click();
        harness.run();
        assert_eq!(
            tool_profiles::selected(id, harness.state()),
            Some(ToolProfile::Verbose)
        );
        assert!(!harness.state().ltex.enabled);
        assert_eq!(
            harness.state().ltex.configuration["dictionary"],
            before.ltex.configuration["dictionary"]
        );
        harness.get_by_label(&label("Normal")).click();
        harness.run();
        assert_eq!(harness.state(), &before);
    }

    #[test]
    fn reference_close_stays_inside_small_and_normal_windows() {
        for size in [egui::vec2(360.0, 320.0), egui::vec2(730.0, 560.0)] {
            let id = ReferenceId::Liquid;
            let mut harness = harness(size, id);
            harness
                .get_by_label(&format!("{} offline reference", reference(id).title))
                .click();
            harness.run();
            let rect = harness.get_by_label("Close reference").rect();
            assert!(
                rect.left() >= 0.0
                    && rect.top() >= 0.0
                    && rect.right() <= size.x
                    && rect.bottom() <= size.y,
                "{size:?}: {rect:?}"
            );
            harness.key_press(egui::Key::Escape);
            harness.run();
            assert!(!is_open(&harness.ctx));
        }
    }
}
