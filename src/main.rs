mod app;
mod asset;
mod builtin_themes;
mod compiler;
mod diagnostics;
mod document;
mod generic_highlight;
mod highlight;
mod lsp_text;
mod open_requests;
mod preview;
mod private_workspace;
mod project_index;
mod screenshot;
mod search;
mod settings;
mod sublime_theme;
mod theme;
mod theme_transform;
mod tinymist;
mod toolchain;
mod workspace;

use app::EditorApp;
use eframe::egui;
use screenshot::{CaptureController, LaunchOptions, ScreenshotApp};

fn main() -> eframe::Result {
    let launch = match LaunchOptions::from_process() {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("Invalid UI screenshot configuration: {error}");
            return Ok(());
        }
    };
    if let Some(profile) = &launch.theme_profile
        && profile.name != settings::SYSTEM_THEME_ID
        && builtin_themes::find(&profile.name).is_none()
    {
        eprintln!("Unknown built-in UI theme {:?}", profile.name);
        return Ok(());
    }
    let initial_path = launch.initial_path;
    let theme_profile = launch.theme_profile;
    let ui_snapshot_scene = launch.ui_snapshot_scene;
    let deterministic_snapshot = ui_snapshot_scene.is_some();
    let mut capture_config = launch.captures;
    if let Some(scene) = ui_snapshot_scene
        && capture_config.startup_captures.is_empty()
    {
        capture_config.startup_captures.push(scene.capture_spec());
    }
    let captures = CaptureController::new(capture_config);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
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
        // A visual-QA scene must not inherit a developer's last window size;
        // otherwise the same stable filename can contain unrelated geometry.
        persist_window: !deterministic_snapshot,
        centered: deterministic_snapshot,
        ..Default::default()
    };

    let (open_request_sender, open_requests) = open_requests::channel();

    #[cfg(target_os = "macos")]
    {
        use winit::event_loop::EventLoop;

        // Owning the event loop here gives us the one safe point after winit
        // registers its NSApplicationDelegate and before AppKit can dispatch
        // Finder's document-open event.
        let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
        open_requests::install_macos_handler(open_request_sender)
            .map_err(|error| eframe::Error::AppCreation(error.into()))?;
        let mut app = eframe::create_native(
            "tiptoptyp",
            options,
            Box::new(move |context| {
                let app = EditorApp::new(
                    context,
                    initial_path,
                    captures.clone(),
                    theme_profile,
                    ui_snapshot_scene,
                    open_requests,
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
        eframe::run_native(
            "tiptoptyp",
            options,
            Box::new(move |context| {
                let app = EditorApp::new(
                    context,
                    initial_path,
                    captures.clone(),
                    theme_profile,
                    ui_snapshot_scene,
                    open_requests,
                );
                Ok(Box::new(ScreenshotApp::new(app, captures)))
            }),
        )
    }
}
