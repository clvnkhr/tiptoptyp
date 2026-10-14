//! Transient Settings navigation; categories never alter saved preferences.
use super::{SettingsSection, SettingsTarget};
use eframe::egui;

pub(super) const RAIL_WIDTH: f32 = 116.0;
pub(super) fn uses_rail(width: f32) -> bool {
    width >= 680.0
}

pub(super) fn rail(ui: &mut egui::Ui, selected: &mut SettingsSection) {
    ui.set_width(RAIL_WIDTH);
    for section in SettingsSection::ALL {
        let response = ui.add_sized(
            [RAIL_WIDTH, ui.spacing().interact_size.y],
            egui::Button::new(section.title())
                .selected(*selected == section)
                .frame(false),
        );
        observe(section, &response);
        if response.clicked() {
            *selected = section;
        }
    }
}

pub(super) fn selector(ui: &mut egui::Ui, selected: &mut SettingsSection) {
    let response = egui::ComboBox::from_id_salt("settings-category")
        .width(105.0)
        .selected_text(selected.title())
        .show_ui(ui, |ui| {
            for section in SettingsSection::ALL {
                let response = ui.selectable_value(selected, section, section.title());
                observe(section, &response);
            }
        })
        .response;
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.category.selector", &response);
    let _ = response;
}
fn observe(section: SettingsSection, response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("settings.category.{}", section.title()), response);
    let _ = (section, response);
}

pub(super) fn route_target(
    selected: &mut SettingsSection,
    colors_open: &mut bool,
    target: SettingsTarget,
) {
    *selected = target.section();
    if matches!(
        target,
        SettingsTarget::InvertColors | SettingsTarget::HueShift | SettingsTarget::ThemeColors
    ) {
        *colors_open = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn every_search_target_has_a_visible_category_and_colors_reveal_only_when_needed() {
        for target in SettingsTarget::ALL {
            let mut selected = SettingsSection::Templates;
            let mut colors = false;
            route_target(&mut selected, &mut colors, target);
            assert!(SettingsSection::ALL.contains(&selected));
            assert_eq!(selected, target.section());
            assert_eq!(
                colors,
                matches!(
                    target,
                    SettingsTarget::InvertColors
                        | SettingsTarget::HueShift
                        | SettingsTarget::ThemeColors
                )
            );
        }
    }
    #[test]
    fn categories_select_without_changing_preferences() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(140.0, 360.0))
            .build_ui_state(rail, SettingsSection::Appearance);
        harness.run();
        for section in SettingsSection::ALL {
            harness.get_by_label(section.title()).click();
            harness.run();
            assert_eq!(*harness.state(), section);
        }
    }
    #[test]
    fn narrow_window_uses_a_selector_instead_of_squeezing_the_form() {
        assert!(!uses_rail(400.0));
        assert!(!uses_rail(679.0));
        assert!(uses_rail(680.0));
    }
}
