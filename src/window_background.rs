//! One translucent window tint; document colors stay opaque.
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) opacity_percent: u8,
    pub(crate) blur: bool,
    pub(crate) blur_radius: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity_percent: 80,
            blur: true,
            blur_radius: 8,
        }
    }
}
impl Settings {
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.opacity_percent > 100 {
            return Err(
                "window_background.opacity_percent: choose a percentage from 0 to 100".into(),
            );
        }
        if self.blur_radius > 50 {
            return Err("window_background.blur_radius: choose a radius from 0 to 50".into());
        }
        Ok(())
    }
    fn translucent(self, supported: bool) -> bool {
        supported && self.enabled && self.opacity_percent < 100
    }
}

const CONFIG: &str = "window-background-config";
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct NativeState {
    identity: u64,
    enabled: bool,
    blur: bool,
    blur_radius: u8,
    dark: bool,
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct NativeCache {
    state: NativeState,
    #[cfg(target_os = "macos")]
    blur: Option<crate::native_window::NativeBlur>,
}

/// Root windows publish the shared appearance preference for persistent tools.
pub(crate) fn publish(context: &egui::Context, settings: Settings) {
    context.data_mut(|data| data.insert_temp(egui::Id::new(CONFIG), settings));
}
pub(crate) fn prepare_tool(ui: &mut egui::Ui) {
    let settings = ui
        .ctx()
        .data(|data| data.get_temp::<Settings>(egui::Id::new(CONFIG)))
        .unwrap_or_default();
    prepare(ui, settings);
}
pub(crate) fn prepare(ui: &mut egui::Ui, settings: Settings) {
    let requested = settings.translucent(cfg!(target_os = "macos"));
    let available = synchronize_native(ui.ctx(), requested, settings.blur, settings.blur_radius);
    paint(ui, settings, requested && available);
}

#[cfg(target_os = "macos")]
fn synchronize_native(context: &egui::Context, enabled: bool, blur: bool, radius: u8) -> bool {
    let Some(identity) = eframe::window_host::identity(context, context.viewport_id()) else {
        return false;
    };
    let key = egui::Id::new((context.viewport_id(), "native-window-background"));
    let next = NativeState {
        identity,
        enabled,
        blur: enabled && blur,
        blur_radius: if enabled && blur { radius } else { 0 },
        dark: context.theme() == egui::Theme::Dark,
    };
    let cached = context.data(|data| data.get_temp::<NativeCache>(key));
    let Some(parent) = crate::native_window::for_viewport(context, context.viewport_id()) else {
        return false;
    };
    if !native_update_needed(cached.as_ref().map(|cached| cached.state), next) {
        return cached
            .and_then(|cached| cached.blur)
            .is_none_or(|blur| blur.synchronize(&parent, None));
    }
    if !parent.set_translucent_background() {
        return false;
    }
    // Remove the old underlay before changing owners. A same-window palette
    // update reuses its material view; opacity changes don't touch AppKit.
    let effect = if next.blur {
        cached
            .filter(|cached| cached.state.identity == identity)
            .and_then(|cached| cached.blur)
            .filter(|blur| blur.synchronize(&parent, Some((next.dark, next.blur_radius))))
            .or_else(|| parent.blur_background(next.dark, next.blur_radius))
    } else {
        None
    };
    context.data_mut(|data| {
        data.insert_temp(
            key,
            NativeCache {
                state: next,
                blur: effect,
            },
        )
    });
    true
}

#[cfg(not(target_os = "macos"))]
fn synchronize_native(_: &egui::Context, _: bool, _: bool, _: u8) -> bool {
    false
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(crate) fn inspection(context: &egui::Context, viewport: egui::ViewportId) -> serde_json::Value {
    let key = egui::Id::new((viewport, "native-window-background"));
    let cached = context.data(|data| data.get_temp::<NativeCache>(key));
    let Some(cached) = cached else {
        return serde_json::Value::Null;
    };
    let geometry = cached.blur.and_then(|blur| {
        let parent = crate::native_window::for_viewport(context, viewport)?;
        blur.inspection(&parent).map(|(frame, below, identity)| serde_json::json!({"frame":frame,"below_renderer":below,"identity":identity}))
    });
    serde_json::json!({"owner":cached.state.identity,"translucent":cached.state.enabled,"dark":cached.state.dark,"blur_radius":cached.state.blur_radius,"blur":geometry})
}

#[cfg(any(target_os = "macos", test))]
fn native_update_needed(previous: Option<NativeState>, next: NativeState) -> bool {
    previous != Some(next)
}

fn paint(ui: &mut egui::Ui, settings: Settings, translucent: bool) {
    let background = ui.visuals().panel_fill;
    let tint = if translucent {
        opacity(background, settings.opacity_percent)
    } else {
        background
    };
    ui.painter().rect_filled(ui.max_rect(), 0.0, tint);
    if translucent {
        // Panels cover overlapping regions of the root. Paint the window tint
        // once; nested panel/header frames must not multiply its opacity.
        ui.visuals_mut().panel_fill = Color32::TRANSPARENT;
    }
}

pub(crate) fn surface(ui: &egui::Ui, color: Color32) -> Color32 {
    surface_for_style(ui.style(), color)
}
pub(crate) fn surface_for_style(style: &egui::Style, color: Color32) -> Color32 {
    if style.visuals.panel_fill == Color32::TRANSPARENT {
        // A slight local shade separates the editor and Explorer sections while
        // retaining the underlying window translucency, even when nested.
        opacity(color, 10)
    } else {
        color
    }
}
fn opacity(color: Color32, percent: u8) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(
        r,
        g,
        b,
        ((u16::from(a) * u16::from(percent.min(100)) + 50) / 100) as u8,
    )
}

pub(crate) fn settings_ui(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.add_enabled_ui(cfg!(target_os = "macos"), |ui| {
        let response = ui.checkbox(&mut settings.enabled, "Translucent background");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("settings.background.enabled", &response);
        let _ = response;
        ui.add_enabled_ui(settings.enabled, |ui| {
            let response = ui.checkbox(&mut settings.blur, "Soft blur");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.background.blur", &response);
            let _ = response;
            ui.add_enabled_ui(settings.blur, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Blur radius");
                    let response = ui.add(egui::Slider::new(&mut settings.blur_radius, 0..=50));
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("settings.background.blur_radius", &response);
                    let _ = response;
                });
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Background opacity");
                let response =
                    ui.add(egui::Slider::new(&mut settings.opacity_percent, 0..=100).suffix("%"));
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.background.opacity", &response);
                let _ = response;
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validation_and_platform_fallback() {
        let settings = Settings::default();
        assert!(settings.translucent(true));
        assert!(!settings.translucent(false));
        assert!(
            !Settings {
                enabled: false,
                ..settings
            }
            .translucent(true)
        );
        assert!(
            !Settings {
                opacity_percent: 100,
                ..settings
            }
            .translucent(true)
        );
        assert!(
            Settings {
                opacity_percent: 101,
                ..settings
            }
            .validate()
            .unwrap_err()
            .contains("0 to 100")
        );
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), settings);
    }
    #[test]
    fn blur_defaults_on_and_native_configuration_is_stable_until_preferences_change() {
        assert!(Settings::default().blur);
        assert_eq!(Settings::default().blur_radius, 8);
        for enabled in [false, true] {
            for blur in [false, true] {
                for opacity_percent in [0, 80, 100] {
                    let settings = Settings {
                        enabled,
                        blur,
                        opacity_percent,
                        ..Settings::default()
                    };
                    let effective = settings.translucent(true) && settings.blur;
                    assert_eq!(effective, enabled && blur && opacity_percent < 100);
                }
            }
        }
        let state = NativeState {
            identity: 1,
            enabled: true,
            blur: true,
            blur_radius: 8,
            dark: false,
        };
        for _ in 0..1000 {
            assert!(!native_update_needed(Some(state), state));
        }
        for changed in [
            NativeState {
                blur_radius: 16,
                ..state
            },
            NativeState {
                blur: false,
                ..state
            },
            NativeState {
                dark: true,
                ..state
            },
            NativeState {
                identity: 2,
                ..state
            },
        ] {
            assert!(native_update_needed(Some(state), changed));
        }
        let mut value = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        value["window_background"]["blur"] = serde_json::json!("on");
        assert!(
            crate::settings_json::parse(&value.to_string())
                .unwrap_err()
                .contains("blur")
        );
        value["window_background"]["blur"] = serde_json::json!(false);
        assert!(
            !crate::settings_json::parse(&value.to_string())
                .unwrap()
                .window_background
                .blur
        );
    }
    #[test]
    fn json_rejects_impossible_opacity_and_round_trips_preferences() {
        let mut value = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        value["window_background"]["opacity_percent"] = serde_json::json!(101);
        let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
        assert!(
            error.contains("window_background.opacity_percent") && error.contains("0 to 100"),
            "{error}"
        );
        value["window_background"]["opacity_percent"] = serde_json::json!(60);
        assert_eq!(
            crate::settings_json::parse(&value.to_string())
                .unwrap()
                .window_background
                .opacity_percent,
            60
        );
    }
    #[test]
    fn blur_radius_json_limits_and_round_trip() {
        let mut value = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        for radius in [0, 8, 30, 50] {
            value["window_background"]["blur_radius"] = serde_json::json!(radius);
            assert_eq!(
                crate::settings_json::parse(&value.to_string())
                    .unwrap()
                    .window_background
                    .blur_radius,
                radius
            );
        }
        value["window_background"]["blur_radius"] = serde_json::json!(51);
        let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
        assert!(
            error.contains("window_background.blur_radius") && error.contains("0 to 50"),
            "{error}"
        );
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn appearance_checkbox_keeps_opacity_control_visible_and_stable() {
        use egui_kittest::kittest::Queryable;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(620.0, 180.0))
            .build_ui_state(settings_ui, Settings::default());
        harness.run();
        let bounds = harness.get_by_label("Background opacity").rect();
        let radius_bounds = harness.get_by_label("Blur radius").rect();
        harness.get_by_label("Soft blur").click();
        harness.run();
        assert!(!harness.state().blur);
        assert_eq!(harness.get_by_label("Blur radius").rect(), radius_bounds);
        harness.get_by_label("Translucent background").click();
        harness.run();
        assert!(!harness.state().enabled);
        assert_eq!(harness.get_by_label("Background opacity").rect(), bounds);
    }
    #[test]
    fn local_window_tint_cannot_mutate_theme_or_document_palette() {
        let context = egui::Context::default();
        let original = context.style_of(context.theme());
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            paint(ui, Settings::default(), true);
            assert_eq!(ui.visuals().panel_fill, Color32::TRANSPARENT);
            assert_eq!(ui.visuals().text_color(), original.visuals.text_color());
            assert_eq!(
                ui.visuals().window_fill,
                original.visuals.window_fill,
                "popups stay opaque"
            );
            assert_eq!(
                ui.visuals().extreme_bg_color,
                original.visuals.extreme_bg_color,
                "terminal and PDF palette stays unchanged"
            );
            assert!(surface(ui, Color32::WHITE).a() < 30);
        });
        output.textures_delta.clear();
        assert_eq!(
            context.style_of(context.theme()).visuals.panel_fill,
            original.visuals.panel_fill
        );
    }
    #[test]
    fn disabling_restores_opaque_surface_without_changing_native_identity() {
        let mut harness = egui_kittest::Harness::builder().build_ui_state(
            |ui, enabled| {
                let settings = Settings {
                    enabled: *enabled,
                    ..Settings::default()
                };
                paint(ui, settings, settings.enabled);
                if ui.button("Toggle").clicked() {
                    *enabled = !*enabled;
                }
                ui.label(format!("alpha={}", surface(ui, Color32::WHITE).a()));
            },
            true,
        );
        use egui_kittest::kittest::Queryable;
        harness.run();
        assert!(harness.query_by_label("alpha=26").is_some());
        harness.get_by_label("Toggle").click();
        harness.run();
        assert!(harness.query_by_label("alpha=255").is_some());
        let first = NativeState {
            identity: 1,
            enabled: true,
            blur: true,
            blur_radius: 8,
            dark: false,
        };
        assert_ne!(
            first,
            NativeState {
                identity: 2,
                ..first
            },
            "native replacement forces attachment"
        );
        for _ in 0..1_000 {
            assert!(!native_update_needed(Some(first), first));
        }
        assert!(native_update_needed(
            Some(first),
            NativeState {
                enabled: false,
                ..first
            }
        ));
        assert!(native_update_needed(
            Some(first),
            NativeState {
                identity: 2,
                ..first
            }
        ));
    }
}
