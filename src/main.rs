#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod app;
#[allow(unsafe_code)]
mod app_icon;
mod asset;
mod auto_pairs;
mod build_info;
mod capabilities;
mod child_view;
mod compiler;
mod completion;
mod completion_edit;
mod delimiters;
mod diagnostics;
mod document;
mod editor_data;
mod editor_features;
mod embedded_structure;
mod explorer;
mod folding;
mod font_catalog;
mod font_preview;
mod generic_highlight;
mod git;
mod highlight;
mod index_jobs;
mod launch;
#[allow(unsafe_code)]
mod native_menu;
#[allow(unsafe_code)]
mod native_window;
#[allow(unsafe_code)]
mod open_requests;
mod package_catalog;
mod pdf;
mod pdf_pages;
mod pdf_residency;
mod performance;
mod presentation;
mod preview;
mod private_workspace;
mod process;
mod project_index;
mod rainbow;
mod resource_lock;
mod save_io;
mod screenshot;
mod search;
mod settings;
mod shortcuts;
mod syntax_theme;
#[cfg(test)]
#[allow(unsafe_code)]
mod test_allocations;
mod tex_completion;
mod theme;
mod tinymist;
mod tinymist_sync;
mod toolchain;
mod unicode_fonts;
mod viewport_fonts;
mod window_logo;
mod windowing;
mod worker;
mod workflow;
mod workspace;
mod workspace_service;

use eframe::egui;
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
    let profile = performance::Session::from_env()
        .map_err(|error| eframe::Error::AppCreation(std::io::Error::other(error).into()))?;
    let result = run(profile.persistence_path());
    let recorded = profile
        .finish()
        .map_err(|error| eframe::Error::AppCreation(std::io::Error::other(error).into()));
    result.and(recorded)
}

fn run(profile_storage: Option<std::path::PathBuf>) -> eframe::Result {
    let launch =
        LaunchOptions::from_process(profile_storage).map_err(invalid_launch_configuration)?;
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
        viewport: egui::ViewportBuilder::default()
            .with_icon(app_icon::runtime_icon())
            .with_inner_size(theme::METRICS.chrome.main_size)
            .with_min_inner_size(theme::METRICS.chrome.main_min_size)
            // Child popup viewports use real alpha so rounded cards can sit
            // above WKWebView without opaque corner wedges.
            .with_transparent(true)
            // On macOS the normal title bar becomes part of the app toolbar. The
            // traffic-light controls remain native, while the otherwise empty
            // title strip no longer costs a row of vertical space.
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false),
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
    use super::native_options;

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
