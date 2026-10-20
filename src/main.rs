#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod activity;
mod app;
#[allow(unsafe_code)]
mod app_icon;
mod asset;
mod auto_pairs;
mod build_info;
mod bundled_assets;
mod bundled_payload;
mod capabilities;
mod child_view;
mod compiler;
mod completion;
mod completion_edit;
mod delimiters;
#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
mod desktop_test;
mod diagnostics;
mod document;
mod document_templates;
mod editor_data;
mod editor_decoration;
mod editor_features;
mod editor_math_guides;
mod embedded_structure;
mod embedded_tex;
mod explorer;
mod file_icons;
#[allow(unsafe_code)]
mod file_versions;
mod folding;
mod font_catalog;
mod font_preview;
mod frame_monitor;
mod generic_highlight;
mod git;
mod handwriting;
mod handwriting_detexify;
mod highlight;
mod image_residency;
mod index_jobs;
mod language_support;
mod launch;
mod lsp;
mod math_conversion;
#[allow(unsafe_code)]
mod native_menu;
#[allow(unsafe_code)]
mod native_window;
#[allow(unsafe_code)]
mod open_requests;
mod package_catalog;
mod pdf;
mod pdfium;
mod performance;
#[allow(unsafe_code)]
mod popup_window;
mod presentation;
mod preview;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod preview_zoom_snapshot;
mod private_workspace;
mod process;
mod project_index;
mod rainbow;
mod resource_lock;
mod save_io;
mod screenshot;
mod search;
mod settings;
mod settings_json;
mod shortcuts;
mod snippet_presets;
mod snippets;
mod synctex;
mod syntax_theme;
mod table_sampling;
mod terminal;
#[cfg(test)]
#[allow(unsafe_code)]
mod test_allocations;
mod tex;
mod tex_completion;
mod text_encoding;
mod theme;
mod tinymist;
mod tinymist_sync;
mod tool_command;
mod toolchain;
mod unicode_fonts;
mod version_render;
mod viewport_fonts;
mod window_background;
mod window_host;
mod window_logo;
mod window_policy;
mod windowing;
mod worker;
mod workflow;
mod workspace;
mod workspace_service;
mod writing;

use launch::LaunchOptions;
use screenshot::{CaptureController, ScreenshotApp};
use shortcuts::ShortcutBindings;
use tiptoptyp::themes::{
    builtin as builtin_themes, sublime as sublime_theme, transform as theme_transform,
};
use windowing::AppShell;

fn main() -> eframe::Result {
    if std::env::args_os()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", build_info::VERSION);
        return Ok(());
    }
    if std::env::args_os().len() == 2
        && std::env::args_os()
            .nth(1)
            .is_some_and(|arg| arg == "--check-runtime")
    {
        validate_runtime(RuntimeCheck::Initialize)?;
        println!("PDF runtime initialized successfully");
        return Ok(());
    }
    if std::env::args_os().len() == 2
        && std::env::args_os()
            .nth(1)
            .is_some_and(|arg| arg == "--check-toolchain")
    {
        toolchain::validate_bundled().map_err(invalid_launch_configuration)?;
        println!("Bundled tools initialized successfully");
        return Ok(());
    }
    let profile = performance::Session::from_env()
        .map_err(|error| eframe::Error::AppCreation(std::io::Error::other(error).into()))?;
    let result = run(profile.persistence_path());
    let recorded = profile
        .finish()
        .map_err(|error| eframe::Error::AppCreation(std::io::Error::other(error).into()));
    result.and(recorded)
}

#[derive(Clone, Copy)]
enum RuntimeCheck {
    Presence,
    Initialize,
}

fn check_pdf_runtime(
    mode: RuntimeCheck,
    presence: impl FnOnce() -> Result<(), String>,
    initialize: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    match mode {
        RuntimeCheck::Presence => presence(),
        RuntimeCheck::Initialize => initialize(),
    }
}

fn validate_runtime(mode: RuntimeCheck) -> eframe::Result {
    bundled_assets::validate().map_err(invalid_launch_configuration)?;
    check_pdf_runtime(mode, pdfium::validate_presence, pdfium::validate).map_err(|error| {
        eframe::Error::AppCreation(
            std::io::Error::other(format!("Could not initialize PDF renderer: {error}")).into(),
        )
    })
}

fn run(profile_storage: Option<std::path::PathBuf>) -> eframe::Result {
    let launch =
        LaunchOptions::from_process(profile_storage).map_err(invalid_launch_configuration)?;
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    let launch = {
        let mut launch = launch;
        if let Some(storage) = desktop_test::start().map_err(invalid_launch_configuration)? {
            if launch.mode != launch::LaunchMode::Interactive || launch.persistence_path.is_some() {
                return Err(invalid_launch_configuration(
                    "desktop journeys require normal interactive mode, without profiling or QA scenes",
                ));
            }
            launch.persistence_path = Some(storage);
        }
        launch
    };
    if let Some(profile) = &launch.theme_profile
        && profile.name != settings::SYSTEM_THEME_ID
        && builtin_themes::find(&profile.name).is_none()
    {
        return Err(invalid_launch_configuration(format!(
            "Unknown built-in UI theme {:?}",
            profile.name
        )));
    }
    if let Some(step) = launch.ui_capture_steps.iter().find(|step| {
        step.theme.name != settings::SYSTEM_THEME_ID
            && builtin_themes::find(&step.theme.name).is_none()
    }) {
        return Err(invalid_launch_configuration(format!(
            "Unknown built-in UI theme {:?}",
            step.theme.name
        )));
    }
    validate_runtime(RuntimeCheck::Presence)?;
    let launch_mode = launch.mode;
    let initial_path = launch.initial_path;
    let theme_profile = launch.theme_profile;
    let ui_snapshot_scene = launch.ui_snapshot_scene;
    let ui_capture_steps = launch.ui_capture_steps;
    let deterministic_snapshot = !launch_mode.persists_settings();
    let mut capture_config = launch.captures;
    if let Some(scene) = ui_snapshot_scene
        && ui_capture_steps.is_empty()
        && capture_config.startup_captures.is_empty()
    {
        capture_config.startup_captures.push(scene.capture_spec());
    }
    let captures = CaptureController::new(capture_config);
    let options = eframe::NativeOptions {
        persistence_path: launch.persistence_path,
        ..native_options(deterministic_snapshot)
    };

    let (open_request_sender, open_requests) = open_requests::channel();
    let (native_menu_sender, native_menu_commands) = native_menu::channel();

    #[cfg(target_os = "macos")]
    {
        use winit::event_loop::EventLoop;

        // Owning the event loop here gives us the one safe point after winit
        // registers its NSApplicationDelegate and before AppKit can dispatch
        // Finder's document-open event.
        let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
        app_icon::install_macos_application_icon()
            .map_err(|error| eframe::Error::AppCreation(error.into()))?;
        open_requests::install_macos_handler(open_request_sender)
            .map_err(|error| eframe::Error::AppCreation(error.into()))?;
        native_menu::install_macos_handler(native_menu_sender)
            .map_err(|error| eframe::Error::AppCreation(error.into()))?;
        let mut app = eframe::create_native(
            build_info::APP_NAME,
            options,
            Box::new(move |context| {
                native_menu::install_macos_menu(
                    context.egui_ctx.clone(),
                    &ShortcutBindings::current_defaults(),
                )?;
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                desktop_test::attach(&context.egui_ctx);
                let app = AppShell::new(
                    context,
                    initial_path,
                    captures.clone(),
                    theme_profile,
                    ui_snapshot_scene,
                    ui_capture_steps,
                    launch_mode,
                    open_requests,
                    native_menu_commands,
                );
                Ok(Box::new(ScreenshotApp::new(app, captures)))
            }),
            &event_loop,
        );
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        drop(open_request_sender);
        drop(native_menu_sender);
        eframe::run_native(
            build_info::APP_NAME,
            options,
            Box::new(move |context| {
                let app = AppShell::new(
                    context,
                    initial_path,
                    captures.clone(),
                    theme_profile,
                    ui_snapshot_scene,
                    ui_capture_steps,
                    launch_mode,
                    open_requests,
                    native_menu_commands,
                );
                Ok(Box::new(ScreenshotApp::new(app, captures)))
            }),
        )
    }
}

fn invalid_launch_configuration(error: impl std::fmt::Display) -> eframe::Error {
    eframe::Error::AppCreation(
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid UI screenshot configuration: {error}"),
        )
        .into(),
    )
}

fn native_options(deterministic_snapshot: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: window_policy::document()
            .with_icon(app_icon::runtime_icon())
            .with_inner_size(theme::METRICS.chrome.main_size)
            .with_min_inner_size(theme::METRICS.chrome.main_min_size),
        // Visual-QA scenes must not inherit a developer's last window size.
        // Normal launches may restore the size, but never the position: eframe
        // does not clamp persisted positions on macOS, and a monitor-layout
        // change can otherwise strand this borderless window offscreen.
        persist_window: !deterministic_snapshot,
        centered: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeCheck, check_pdf_runtime, native_options};

    #[test]
    fn startup_checks_presence_without_initializing_the_pdf_engine() {
        let presence_checks = std::cell::Cell::new(0);
        let initializations = std::cell::Cell::new(0);
        check_pdf_runtime(
            RuntimeCheck::Presence,
            || {
                presence_checks.set(presence_checks.get() + 1);
                Ok(())
            },
            || {
                initializations.set(initializations.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(presence_checks.get(), 1);
        assert_eq!(initializations.get(), 0);
    }

    #[test]
    fn explicit_runtime_probe_initializes_the_engine_and_reports_failure() {
        let error = check_pdf_runtime(
            RuntimeCheck::Initialize,
            || panic!("the runtime probe must fully load the engine"),
            || Err("renderer could not bind".to_owned()),
        )
        .unwrap_err();
        assert_eq!(error, "renderer could not bind");
    }

    #[test]
    fn main_window_is_centered_even_when_size_persistence_is_enabled() {
        let normal = native_options(false);
        assert!(normal.persist_window);

        assert!(normal.centered);
        let icon = normal.viewport.icon.as_ref().expect("runtime icon");
        assert_eq!((icon.width, icon.height), (256, 256));

        let snapshot = native_options(true);
        assert!(!snapshot.persist_window);
        assert!(snapshot.centered);
        assert_eq!(snapshot.viewport.icon, normal.viewport.icon);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod test_framebuffer;
