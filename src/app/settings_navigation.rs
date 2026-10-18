//! Transient Settings navigation; categories never alter saved preferences.
use super::{SettingsSection, SettingsTarget};
use eframe::egui;

/// Measure what the real buttons draw, including their actual frame margins.
/// This keeps category labels intact when the interface font or scale changes.
pub(super) fn rail_width(ui: &egui::Ui) -> f32 {
    let style = ui.style().button_style(
        &egui::widget_style::Classes::default(),
        egui::widget_style::WidgetState::Inactive,
    );
    let font = style.text_style.font_id;
    let label_width = SettingsSection::ALL
        .into_iter()
        .map(|section| {
            ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(section.title().into(), font.clone(), egui::Color32::WHITE)
            })
            .size()
            .x
        })
        .fold(0.0_f32, f32::max);
    (label_width + style.frame.inner_margin.sum().x).ceil()
}
pub(super) fn uses_rail(width: f32) -> bool {
    width >= 680.0
}

pub(super) fn rail(ui: &mut egui::Ui, selected: &mut SettingsSection) {
    let width = rail_width(ui);
    ui.set_width(width);
    for section in SettingsSection::ALL {
        let response = ui.add_sized(
            [width, ui.spacing().interact_size.y],
            egui::Button::new((section.title(), egui::Atom::grow()))
                .selected(*selected == section)
                .frame_when_inactive(*selected == section)
                .gap(0.0)
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        observe(section, &response);
        if response.clicked() {
            *selected = section;
        }
    }
}

pub(super) fn scrolling_rail(ui: &mut egui::Ui, selected: &mut SettingsSection) {
    ui.scope(|ui| {
        // Let short windows scroll the category list without reserving a new
        // horizontal column or covering the measured labels with a wide bar.
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = true;
        scroll.floating_allocated_width = 0.0;
        scroll.bar_width = scroll.floating_width;
        scroll.content_margin = egui::Margin::ZERO;
        egui::ScrollArea::vertical()
            .id_salt("settings-category-rail")
            .auto_shrink([false, false])
            .show(ui, |ui| rail(ui, selected));
    });
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
    fn rail_is_left_aligned_and_fits_exactly_the_widest_label_at_each_font_scale() {
        for font_size in [12.0, 19.0, 28.0] {
            for scale in [1.0, 2.0] {
                let mut harness = Harness::builder()
                    .with_size(Vec2::new(350.0, 500.0))
                    .build_ui_state(rail, SettingsSection::Appearance);
                harness.ctx.set_pixels_per_point(scale);
                harness.ctx.style_mut_of(harness.ctx.theme(), |style| {
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::proportional(font_size),
                    );
                });
                harness.run();
                let rows = SettingsSection::ALL.map(|section| {
                    let button = harness.get_by_label(section.title()).rect();
                    let text = harness
                        .output()
                        .shapes
                        .iter()
                        .find_map(|shape| {
                            if let egui::Shape::Text(text) = &shape.shape
                                && text.galley.job.text == section.title()
                            {
                                Some(text)
                            } else {
                                None
                            }
                        })
                        .expect("every category paints its complete label");
                    assert!(
                        button
                            .contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())),
                        "{font_size}px at {scale}x clips {}: {button:?}",
                        section.title()
                    );
                    (button, text.pos.x, text.galley.size().x)
                });
                for (button, text_left, _) in rows {
                    assert!((button.width() - rows[0].0.width()).abs() < 0.01);
                    assert!(
                        (text_left - rows[0].1).abs() < 0.01,
                        "category text must align left"
                    );
                }
                let widest = rows.into_iter().map(|row| row.2).fold(0.0_f32, f32::max);
                let margin = harness
                    .ctx
                    .style_of(harness.ctx.theme())
                    .button_style(
                        &egui::widget_style::Classes::default(),
                        egui::widget_style::WidgetState::Inactive,
                    )
                    .frame
                    .inner_margin
                    .sum()
                    .x;
                assert!(
                    (rows[0].0.width() - widest - margin).abs() < 1.01,
                    "rail must not reserve excess blank width"
                );
            }
        }
    }

    #[test]
    fn narrow_window_uses_a_selector_instead_of_squeezing_the_form() {
        assert!(!uses_rail(400.0));
        assert!(!uses_rail(679.0));
        assert!(uses_rail(680.0));
    }
}
