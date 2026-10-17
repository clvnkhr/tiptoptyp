//! One translucent window tint; document colors stay opaque.
use crate::native_window::FadeDirection;
use eframe::egui::{self, Color32, Rect};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) opacity_percent: u8,
    pub(crate) opacity_end_percent: u8,
    pub(crate) blur: bool,
    pub(crate) blur_radius: u8,
    pub(crate) blur_end_radius: u8,
    pub(crate) blur_fade: FadeDirection,
    pub(crate) transparency_fade: FadeDirection,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity_percent: 80,
            opacity_end_percent: 100,
            blur: true,
            blur_radius: 8,
            blur_end_radius: 0,
            blur_fade: FadeDirection::None,
            transparency_fade: FadeDirection::None,
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
        if self.opacity_end_percent > 100 {
            return Err(
                "window_background.opacity_end_percent: choose a percentage from 0 to 100".into(),
            );
        }
        if self.blur_radius > 50 {
            return Err("window_background.blur_radius: choose a radius from 0 to 50".into());
        }
        if self.blur_end_radius > 50 {
            return Err("window_background.blur_end_radius: choose a radius from 0 to 50".into());
        }
        Ok(())
    }
    fn translucent(self, supported: bool, fullscreen: bool) -> bool {
        supported
            && !fullscreen
            && self.enabled
            && (self.opacity_percent < 100
                || self.transparency_fade != FadeDirection::None && self.opacity_end_percent < 100)
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
    blur_end_radius: u8,
    blur_fade: FadeDirection,
}

#[cfg(any(target_os = "macos", test))]
impl NativeState {
    fn new(
        identity: u64,
        enabled: bool,
        blur: bool,
        radius: u8,
        end_radius: u8,
        fade: FadeDirection,
    ) -> Self {
        let blur = enabled && blur;
        Self {
            identity,
            enabled,
            blur,
            blur_radius: if blur { radius } else { 0 },
            blur_end_radius: if blur && fade != FadeDirection::None {
                end_radius
            } else {
                0
            },
            blur_fade: if blur { fade } else { FadeDirection::None },
        }
    }
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
    let fullscreen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
    let requested = settings.translucent(cfg!(target_os = "macos"), fullscreen);
    let available = synchronize_native(
        ui.ctx(),
        requested,
        settings.blur,
        settings.blur_radius,
        settings.blur_end_radius,
        settings.blur_fade,
    );
    paint(ui, settings, requested && available);
}

#[cfg(target_os = "macos")]
fn synchronize_native(
    context: &egui::Context,
    enabled: bool,
    blur: bool,
    radius: u8,
    end_radius: u8,
    fade: FadeDirection,
) -> bool {
    let Some(identity) = eframe::window_host::identity(context, context.viewport_id()) else {
        return false;
    };
    let key = egui::Id::new((context.viewport_id(), "native-window-background"));
    let next = NativeState::new(identity, enabled, blur, radius, end_radius, fade);
    let cached = context.data(|data| data.get_temp::<NativeCache>(key));
    let Some(parent) = crate::native_window::for_viewport(context, context.viewport_id()) else {
        return false;
    };
    if !native_update_needed(
        cached.as_ref().map(|cached| cached.state),
        next,
        cached.as_ref().is_some_and(|cached| cached.blur.is_some()),
    ) {
        return cached
            .and_then(|cached| cached.blur)
            .is_none_or(|blur| blur.synchronize(&parent, None));
    }
    if !parent.set_translucent_background() {
        return false;
    }
    // Each native window owns one blur controller. Tint/theme and opacity
    // changes leave the compositor configuration alone.
    let effect = reconfigure_native_cache::<NativeCache, _>(context, key, || {
        if next.blur {
            let configuration = crate::native_window::BlurConfig {
                radius: next.blur_radius,
                end_radius: next.blur_end_radius,
                fade: next.blur_fade,
            };
            cached
                .filter(|cached| cached.state.identity == identity)
                .and_then(|cached| cached.blur)
                .filter(|blur| blur.synchronize(&parent, Some(configuration)))
                .or_else(|| parent.blur_background(configuration))
        } else {
            None
        }
    });
    let awaiting_visibility = effect
        .as_ref()
        .is_some_and(|blur| blur.awaiting_visibility());
    context.data_mut(|data| {
        data.insert_temp(
            key,
            NativeCache {
                state: next,
                blur: effect,
            },
        )
    });
    // One follow-up after the first swap/reveal. Unchanged-cache callbacks do
    // not schedule another, so hidden windows cannot create an idle loop.
    if awaiting_visibility {
        context.request_repaint();
    }
    true
}

#[cfg(any(target_os = "macos", test))]
fn reconfigure_native_cache<T: 'static, R>(
    context: &egui::Context,
    key: egui::Id,
    configure: impl FnOnce() -> R,
) -> R {
    // The local clone may reuse the old controller. If reuse fails, dropping
    // that last owner must reset the old effect before a replacement applies.
    context.data_mut(|data| data.remove::<T>(key));
    configure()
}

#[cfg(not(target_os = "macos"))]
fn synchronize_native(_: &egui::Context, _: bool, _: bool, _: u8, _: u8, _: FadeDirection) -> bool {
    false
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(crate) fn inspection(context: &egui::Context, viewport: egui::ViewportId) -> serde_json::Value {
    let key = egui::Id::new((viewport, "native-window-background"));
    let cached = context.data(|data| data.get_temp::<NativeCache>(key));
    let Some(cached) = cached else {
        return serde_json::Value::Null;
    };
    let configuration = cached.blur.and_then(|blur| {
        let parent = crate::native_window::for_viewport(context, viewport)?;
        // Native controller acceptance is not a compositor readback or proof
        // of the composed desktop pixels.
        blur.inspection(&parent)
            .map(|(applied_radius, identity, applications)| {
                serde_json::json!({
                    "method": if blur.applied_fade() == FadeDirection::None { "window-server" } else { "variable-blur" },
                    "applied_radius": applied_radius,
                    "applied_fade": blur.applied_fade(),
                    "identity": identity,
                    "applications": applications,
                })
            })
    });
    serde_json::json!({
        "owner": cached.state.identity,
        "translucent": cached.state.enabled,
        "dark": context.theme() == egui::Theme::Dark,
        "blur_radius": cached.state.blur_radius,
        "blur_end_radius": cached.state.blur_end_radius,
        "blur_fade": cached.state.blur_fade,
        "blur": configuration,
    })
}

#[cfg(any(target_os = "macos", test))]
fn native_update_needed(
    previous: Option<NativeState>,
    next: NativeState,
    blur_configured: bool,
) -> bool {
    // A failed first request is not a completed native configuration. Retry
    // once on the next ordinary UI callback; this does not request a repaint.
    previous != Some(next) || (next.blur && !blur_configured)
}

#[cfg(any(target_os = "macos", test))]
fn directional_blur_fallback(
    settings: Settings,
    fullscreen: bool,
    applied: Option<FadeDirection>,
) -> bool {
    settings.translucent(true, fullscreen)
        && settings.blur
        && settings.blur_radius != settings.blur_end_radius
        && settings.blur_radius.max(settings.blur_end_radius) > 0
        && settings.blur_fade != FadeDirection::None
        && applied == Some(FadeDirection::None)
}

#[cfg(target_os = "macos")]
fn directional_blur_unavailable(context: &egui::Context, settings: Settings) -> bool {
    let fullscreen = context.input(|input| input.viewport().fullscreen.unwrap_or(false));
    let key = egui::Id::new((context.viewport_id(), "native-window-background"));
    let applied = context
        .data(|data| data.get_temp::<NativeCache>(key))
        .filter(|cached| {
            cached.state.enabled
                && cached.state.blur
                && cached.state.blur_radius == settings.blur_radius
                && cached.state.blur_end_radius == settings.blur_end_radius
                && cached.state.blur_fade == settings.blur_fade
        })
        .and_then(|cached| cached.blur)
        .filter(|blur| !blur.awaiting_visibility())
        .map(|blur| blur.applied_fade());
    directional_blur_fallback(settings, fullscreen, applied)
}

#[cfg(not(target_os = "macos"))]
fn directional_blur_unavailable(_: &egui::Context, _: Settings) -> bool {
    false
}

fn paint(ui: &mut egui::Ui, settings: Settings, translucent: bool) {
    let background = ui.visuals().panel_fill.to_opaque();
    let tint = if translucent {
        opacity(background, settings.opacity_percent)
    } else {
        background
    };
    if translucent
        && settings.transparency_fade != FadeDirection::None
        && settings.opacity_percent != settings.opacity_end_percent
    {
        ui.painter().add(transparency_mesh(
            ui.max_rect(),
            background,
            settings.opacity_percent,
            settings.opacity_end_percent,
            settings.transparency_fade,
        ));
    } else {
        ui.painter().rect_filled(ui.max_rect(), 0.0, tint);
    }
    if translucent {
        // Panels cover overlapping regions of the root. Paint the window tint
        // once; nested panel/header frames must not multiply its opacity.
        ui.visuals_mut().panel_fill = Color32::TRANSPARENT;
    }
}

fn transparency_mesh(
    rect: Rect,
    background: Color32,
    opacity_percent: u8,
    opacity_end_percent: u8,
    direction: FadeDirection,
) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    for (position, x, y) in [
        (rect.left_top(), 0.0, 0.0),
        (rect.right_top(), 1.0, 0.0),
        (rect.right_bottom(), 1.0, 1.0),
        (rect.left_bottom(), 0.0, 1.0),
    ] {
        let percent = direction.interpolate(
            f32::from(opacity_percent.min(100)),
            f32::from(opacity_end_percent.min(100)),
            x,
            y,
        );
        mesh.vertices.push(egui::epaint::Vertex {
            pos: position,
            uv: egui::epaint::WHITE_UV,
            color: opacity(background, percent.round() as u8),
        });
    }
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    mesh
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
                    let directional = settings.blur_fade != FadeDirection::None;
                    background_slider(
                        ui,
                        &mut settings.blur_radius,
                        50,
                        false,
                        if directional { "Start" } else { "" },
                        "Start blur radius",
                        "settings.background.blur_radius",
                    );
                    if directional {
                        background_slider(
                            ui,
                            &mut settings.blur_end_radius,
                            50,
                            false,
                            "End",
                            "End blur radius",
                            "settings.background.blur_end_radius",
                        );
                    }
                });
                fade_direction_ui(ui, "Blur fades toward", "blur", &mut settings.blur_fade);
                if directional_blur_unavailable(ui.ctx(), *settings) {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Directional blur unavailable; using uniform blur.",
                    );
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Background opacity");
                let directional = settings.transparency_fade != FadeDirection::None;
                background_slider(
                    ui,
                    &mut settings.opacity_percent,
                    100,
                    true,
                    if directional { "Start" } else { "" },
                    "Start background opacity",
                    "settings.background.opacity",
                );
                if directional {
                    background_slider(
                        ui,
                        &mut settings.opacity_end_percent,
                        100,
                        true,
                        "End",
                        "End background opacity",
                        "settings.background.opacity_end_percent",
                    );
                }
            });
            fade_direction_ui(
                ui,
                "Transparency fades toward",
                "transparency",
                &mut settings.transparency_fade,
            );
        });
    });
}

fn background_slider(
    ui: &mut egui::Ui,
    value: &mut u8,
    maximum: u8,
    percentage: bool,
    caption: &str,
    accessible_label: &str,
    _observer: &str,
) {
    let mut slider = egui::Slider::new(value, 0..=maximum);
    if percentage {
        slider = slider.suffix("%");
    }
    if !caption.is_empty() {
        slider = slider.text(caption);
    }
    let response = ui.add(slider);
    response.widget_info(|| {
        egui::WidgetInfo::slider(ui.is_enabled(), f64::from(*value), accessible_label)
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(_observer, &response);
}

fn fade_direction_ui(ui: &mut egui::Ui, label: &str, id: &str, direction: &mut FadeDirection) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        let response = egui::ComboBox::from_id_salt(("background-fade", id))
            .selected_text(direction.label())
            .width(82.0)
            .show_ui(ui, |ui| {
                for option in FadeDirection::ALL {
                    let response = ui.selectable_value(direction, option, option.label());
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("settings.background.{id}_fade.{option:?}"),
                        &response,
                    );
                    let _ = response;
                }
            })
            .response;
        response.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), label);
            info.current_text_value = Some(direction.label().to_owned());
            info
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(&format!("settings.background.{id}_fade"), &response);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validation_and_platform_fallback() {
        let settings = Settings::default();
        assert!(settings.translucent(true, false));
        assert!(!settings.translucent(false, false));
        assert!(
            !Settings {
                enabled: false,
                ..settings
            }
            .translucent(true, false)
        );
        assert!(
            !Settings {
                opacity_percent: 100,
                ..settings
            }
            .translucent(true, false)
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
    fn fullscreen_disables_transparency_and_blur_without_changing_preferences() {
        for enabled in [false, true] {
            for blur in [false, true] {
                for opacity_percent in [0, 80, 100] {
                    let settings = Settings {
                        enabled,
                        blur,
                        opacity_percent,
                        ..Settings::default()
                    };
                    for supported in [false, true] {
                        assert!(!settings.translucent(supported, true));
                        assert_eq!(
                            settings.translucent(supported, false),
                            supported && enabled && opacity_percent < 100
                        );
                    }
                }
            }
        }
        let context = egui::Context::default();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .fullscreen = Some(true);
        let settings = Settings {
            opacity_percent: 0,
            ..Settings::default()
        };
        let mut output = context.run_ui(input, |ui| {
            assert!(ui.input(|input| input.viewport().fullscreen.unwrap_or(false)));
            assert!(!settings.translucent(
                true,
                ui.input(|input| input.viewport().fullscreen.unwrap_or(false))
            ));
            let background = Color32::from_rgba_unmultiplied(30, 40, 50, 80);
            ui.visuals_mut().panel_fill = background;
            prepare(ui, settings);
            assert_eq!(settings.opacity_percent, 0);
            assert!(settings.enabled && settings.blur);
        });
        output.textures_delta.clear();
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Rect(rect) if rect.fill == Color32::from_rgba_unmultiplied(30, 40, 50, 80).to_opaque())
        }), "fullscreen must paint an opaque background even for a translucent theme color");
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
                    let effective = settings.translucent(true, false) && settings.blur;
                    assert_eq!(effective, enabled && blur && opacity_percent < 100);
                }
            }
        }
        let state = NativeState {
            identity: 1,
            enabled: true,
            blur: true,
            blur_radius: 8,
            blur_end_radius: 0,
            blur_fade: FadeDirection::None,
        };
        for _ in 0..1000 {
            assert!(!native_update_needed(Some(state), state, true));
        }
        for changed in [
            NativeState {
                blur_radius: 16,
                ..state
            },
            NativeState {
                blur_fade: FadeDirection::Right,
                ..state
            },
            NativeState {
                blur: false,
                ..state
            },
            NativeState {
                identity: 2,
                ..state
            },
        ] {
            assert!(native_update_needed(Some(state), changed, true));
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
    fn native_blur_direction_is_normalized_when_inactive_and_reused_when_unchanged() {
        for direction in FadeDirection::ALL {
            for enabled in [false, true] {
                for blur in [false, true] {
                    let state = NativeState::new(1, enabled, blur, 8, 0, direction);
                    assert_eq!(state.blur, enabled && blur);
                    assert_eq!(state.blur_radius, if enabled && blur { 8 } else { 0 });
                    assert_eq!(
                        state.blur_fade,
                        if enabled && blur {
                            direction
                        } else {
                            FadeDirection::None
                        }
                    );
                    for _ in 0..1000 {
                        assert!(!native_update_needed(Some(state), state, true));
                    }
                    if enabled && blur && direction != FadeDirection::None {
                        assert!(native_update_needed(
                            Some(NativeState::new(
                                1,
                                enabled,
                                blur,
                                8,
                                0,
                                FadeDirection::None
                            )),
                            state,
                            true,
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn directional_blur_warning_only_reports_an_active_uniform_fallback() {
        let settings = Settings {
            blur_fade: FadeDirection::Right,
            ..Settings::default()
        };
        assert!(directional_blur_fallback(
            settings,
            false,
            Some(FadeDirection::None)
        ));
        assert!(!directional_blur_fallback(
            settings,
            true,
            Some(FadeDirection::None)
        ));
        assert!(!directional_blur_fallback(settings, false, None));
        assert!(!directional_blur_fallback(
            settings,
            false,
            Some(FadeDirection::Right)
        ));
        for settings in [
            Settings {
                enabled: false,
                ..settings
            },
            Settings {
                blur: false,
                ..settings
            },
            Settings {
                blur_radius: 0,
                ..settings
            },
            Settings {
                opacity_percent: 100,
                ..settings
            },
            Settings {
                blur_fade: FadeDirection::None,
                ..settings
            },
        ] {
            assert!(!directional_blur_fallback(
                settings,
                false,
                Some(FadeDirection::None)
            ));
        }
    }

    #[test]
    fn failed_blur_configuration_is_retried_but_settled_controllers_are_reused() {
        let requested = NativeState {
            identity: 1,
            enabled: true,
            blur: true,
            blur_radius: 8,
            blur_end_radius: 0,
            blur_fade: FadeDirection::None,
        };
        assert!(
            native_update_needed(Some(requested), requested, false),
            "a cached request without successful native configuration must retry"
        );
        assert!(!native_update_needed(Some(requested), requested, true));
        let transparency_only = NativeState {
            blur: false,
            blur_radius: 0,
            ..requested
        };
        assert!(!native_update_needed(
            Some(transparency_only),
            transparency_only,
            false,
        ));
        let disabled = NativeState {
            enabled: false,
            ..transparency_only
        };
        assert!(!native_update_needed(Some(disabled), disabled, false));
    }
    #[test]
    fn replacement_resets_previous_blur_before_applying_new_configuration() {
        use std::sync::{Arc, Mutex};

        struct Controller {
            events: Arc<Mutex<Vec<&'static str>>>,
            reset: &'static str,
        }
        impl Drop for Controller {
            fn drop(&mut self) {
                self.events.lock().unwrap().push(self.reset);
            }
        }
        let context = egui::Context::default();
        let key = egui::Id::new("blur-replacement-order");
        let events = Arc::new(Mutex::new(Vec::new()));
        let previous = Arc::new(Controller {
            events: events.clone(),
            reset: "reset previous",
        });
        context.data_mut(|data| data.insert_temp(key, previous.clone()));
        let replacement = reconfigure_native_cache::<Arc<Controller>, _>(&context, key, || {
            drop(previous);
            events.lock().unwrap().push("apply replacement");
            Some(Arc::new(Controller {
                events: events.clone(),
                reset: "reset replacement",
            }))
        });
        assert_eq!(
            *events.lock().unwrap(),
            ["reset previous", "apply replacement"],
            "a delayed old reset must never erase a newly applied blur"
        );
        drop(replacement);
        assert_eq!(
            *events.lock().unwrap(),
            ["reset previous", "apply replacement", "reset replacement"]
        );

        events.lock().unwrap().clear();
        let previous = Arc::new(Controller {
            events: events.clone(),
            reset: "reset reused",
        });
        context.data_mut(|data| data.insert_temp(key, previous.clone()));
        let reused =
            reconfigure_native_cache::<Arc<Controller>, _>(&context, key, || Some(previous));
        assert!(
            events.lock().unwrap().is_empty(),
            "reuse must not reset blur"
        );
        drop(reused);
        assert_eq!(*events.lock().unwrap(), ["reset reused"]);
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
            .with_size(egui::vec2(620.0, 260.0))
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
    fn transparency_mesh_uses_exact_bounds_and_fades_to_opaque_at_the_chosen_edge() {
        let rect = Rect::from_min_size(egui::pos2(17.0, 29.0), egui::vec2(640.0, 420.0));
        for direction in FadeDirection::ALL {
            let mesh = transparency_mesh(rect, Color32::WHITE, 60, 100, direction);
            assert_eq!(mesh.vertices.len(), 4);
            assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
            for (vertex, position, x, y) in [
                (&mesh.vertices[0], rect.left_top(), 0.0, 0.0),
                (&mesh.vertices[1], rect.right_top(), 1.0, 0.0),
                (&mesh.vertices[2], rect.right_bottom(), 1.0, 1.0),
                (&mesh.vertices[3], rect.left_bottom(), 0.0, 1.0),
            ] {
                assert_eq!(vertex.pos, position);
                assert_eq!(vertex.uv, egui::epaint::WHITE_UV);
                assert_eq!(
                    vertex.color.a(),
                    if direction.factor_at(x, y) == 1.0 {
                        153
                    } else {
                        255
                    }
                );
            }
        }
    }

    #[test]
    fn arbitrary_transparency_endpoints_support_reverse_equal_and_uniform_values() {
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        for (start, end) in [(60, 100), (100, 25), (80, 80), (0, 100), (100, 0)] {
            for direction in FadeDirection::ALL {
                let mesh = transparency_mesh(rect, Color32::WHITE, start, end, direction);
                for (vertex, (x, y)) in
                    mesh.vertices
                        .iter()
                        .zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
                {
                    let expected = if direction.factor_at(x, y) == 1.0 {
                        start
                    } else {
                        end
                    };
                    assert_eq!(
                        vertex.color,
                        opacity(Color32::WHITE, expected),
                        "{direction:?}: {start} to {end}"
                    );
                }
            }
        }
        let settings = Settings {
            opacity_percent: 100,
            opacity_end_percent: 20,
            transparency_fade: FadeDirection::Right,
            ..Settings::default()
        };
        assert!(settings.translucent(true, false));
        assert!(!settings.translucent(true, true));
        assert!(!settings.translucent(false, false));
        assert!(
            !Settings {
                transparency_fade: FadeDirection::None,
                ..settings
            }
            .translucent(true, false)
        );
        assert!(
            !Settings {
                opacity_end_percent: 100,
                ..settings
            }
            .translucent(true, false)
        );
    }

    #[test]
    fn native_end_radius_changes_invalidate_only_active_directional_blur() {
        let uniform = NativeState::new(1, true, true, 8, 0, FadeDirection::None);
        assert_eq!(
            NativeState::new(1, true, true, 8, 30, FadeDirection::None),
            uniform
        );
        assert_eq!(
            NativeState::new(1, false, true, 8, 30, FadeDirection::Right).blur_end_radius,
            0
        );
        assert_eq!(
            NativeState::new(1, true, false, 8, 30, FadeDirection::Right).blur_end_radius,
            0
        );
        for (start, end) in [(8, 0), (0, 30), (20, 20), (35, 15)] {
            let state = NativeState::new(1, true, true, start, end, FadeDirection::Right);
            assert_eq!(state.blur_radius, start);
            assert_eq!(state.blur_end_radius, end);
            for _ in 0..1000 {
                assert!(!native_update_needed(Some(state), state, true));
            }
            assert!(native_update_needed(
                Some(state),
                NativeState::new(1, true, true, start, (end + 1) % 51, FadeDirection::Right),
                true
            ));
        }
    }

    #[test]
    fn end_values_validate_and_round_trip_without_changing_starts_or_directions() {
        let defaults = Settings::default();
        assert_eq!(defaults.blur_end_radius, 0);
        assert_eq!(defaults.opacity_end_percent, 100);
        let mut value = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        value["window_background"]["blur_fade"] = serde_json::json!("right");
        value["window_background"]["transparency_fade"] = serde_json::json!("up");
        for (field, maximum, endpoint) in [
            ("blur_end_radius", 50, 25),
            ("opacity_end_percent", 100, 40),
        ] {
            for valid in [0, endpoint, maximum] {
                value["window_background"][field] = serde_json::json!(valid);
                let settings = crate::settings_json::parse(&value.to_string())
                    .unwrap()
                    .window_background;
                assert_eq!(serde_json::to_value(settings).unwrap()[field], valid);
                assert_eq!(settings.blur_radius, defaults.blur_radius);
                assert_eq!(settings.opacity_percent, defaults.opacity_percent);
                assert_eq!(settings.blur_fade, FadeDirection::Right);
                assert_eq!(settings.transparency_fade, FadeDirection::Up);
            }
            value["window_background"][field] = serde_json::json!(maximum + 1);
            let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains(&format!("window_background.{field}"))
                    && error.contains(&format!("0 to {maximum}")),
                "{error}"
            );
            value["window_background"][field] = serde_json::json!(endpoint);
        }
    }

    #[test]
    fn fade_preferences_round_trip_independently_and_reject_impossible_directions() {
        let mut value = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        for blur in FadeDirection::ALL {
            for transparency in FadeDirection::ALL {
                value["window_background"]["blur_fade"] = serde_json::to_value(blur).unwrap();
                value["window_background"]["transparency_fade"] =
                    serde_json::to_value(transparency).unwrap();
                let settings = crate::settings_json::parse(&value.to_string())
                    .unwrap()
                    .window_background;
                assert_eq!(settings.blur_fade, blur);
                assert_eq!(settings.transparency_fade, transparency);
            }
        }
        for field in ["blur_fade", "transparency_fade"] {
            value["window_background"][field] = serde_json::json!("diagonal");
            let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains(&format!("window_background.{field}"))
                    && error.contains("choose one of"),
                "{error}"
            );
            value["window_background"][field] = serde_json::json!("none");
        }
    }

    #[test]
    fn disabled_unsupported_and_fullscreen_backgrounds_do_not_paint_a_gradient() {
        for direction in FadeDirection::ALL {
            let settings = Settings {
                blur_fade: direction,
                transparency_fade: direction,
                ..Settings::default()
            };
            assert!(!settings.translucent(false, false));
            assert!(!settings.translucent(true, true));
            assert!(
                !Settings {
                    enabled: false,
                    ..settings
                }
                .translucent(true, false)
            );
            for (enabled, fullscreen) in [(false, false), (true, true)] {
                let context = egui::Context::default();
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .fullscreen = Some(fullscreen);
                let mut output = context.run_ui(input, |ui| {
                    prepare(
                        ui,
                        Settings {
                            enabled,
                            ..settings
                        },
                    );
                });
                output.textures_delta.clear();
                assert!(
                    !output
                        .shapes
                        .iter()
                        .any(|shape| matches!(shape.shape, egui::epaint::Shape::Mesh(_)))
                );
            }
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn fade_dropdowns_edit_independent_preferences_and_remain_allocated_when_disabled() {
        use egui_kittest::kittest::{NodeT as _, Queryable, by};
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(620.0, 320.0))
            .build_ui_state(settings_ui, Settings::default());
        harness.run();
        let dropdown =
            |label: &'static str| by().label(label).role(egui::accesskit::Role::ComboBox);
        let blur_bounds = harness.get(dropdown("Blur fades toward")).rect();
        let transparency_bounds = harness.get(dropdown("Transparency fades toward")).rect();
        harness.get(dropdown("Blur fades toward")).click();
        harness.run();
        harness.get_by_label("Right").click();
        harness.run();
        assert_eq!(harness.state().blur_fade, FadeDirection::Right);
        assert_eq!(harness.state().transparency_fade, FadeDirection::None);
        harness.get(dropdown("Transparency fades toward")).click();
        harness.run();
        harness.get_by_label("Up").click();
        harness.run();
        assert_eq!(harness.state().blur_fade, FadeDirection::Right);
        assert_eq!(harness.state().transparency_fade, FadeDirection::Up);
        for label in [
            "Start blur radius",
            "End blur radius",
            "Start background opacity",
            "End background opacity",
        ] {
            assert!(
                harness
                    .get(by().label(label).role(egui::accesskit::Role::Slider))
                    .rect()
                    .width()
                    > 0.0
            );
        }
        let original_end = harness.state().opacity_end_percent;
        harness
            .get(
                by().label("End background opacity")
                    .role(egui::accesskit::Role::Slider),
            )
            .focus();
        harness.run();
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        assert!(harness.state().opacity_end_percent < original_end);
        assert_eq!(
            harness.state().opacity_percent,
            Settings::default().opacity_percent
        );
        assert_eq!(
            harness.state().blur_end_radius,
            Settings::default().blur_end_radius
        );
        harness.get_by_label("Soft blur").click();
        harness.run();
        assert_eq!(
            harness.get(dropdown("Blur fades toward")).rect(),
            blur_bounds
        );
        assert_eq!(
            harness.get(dropdown("Transparency fades toward")).rect(),
            transparency_bounds
        );
        assert!(
            harness
                .get(dropdown("Blur fades toward"))
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            !harness
                .get(dropdown("Transparency fades toward"))
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Translucent background").click();
        harness.run();
        assert!(
            harness
                .get(dropdown("Transparency fades toward"))
                .accesskit_node()
                .is_disabled()
        );
        assert_eq!(harness.state().blur_fade, FadeDirection::Right);
        assert_eq!(harness.state().transparency_fade, FadeDirection::Up);
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
            blur_end_radius: 0,
            blur_fade: FadeDirection::None,
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
            assert!(!native_update_needed(Some(first), first, true));
        }
        assert!(native_update_needed(
            Some(first),
            NativeState {
                enabled: false,
                ..first
            },
            true,
        ));
        assert!(native_update_needed(
            Some(first),
            NativeState {
                identity: 2,
                ..first
            },
            true,
        ));
    }
}
