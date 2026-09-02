mod app;
mod asset;
mod compiler;
mod diagnostics;
mod document;
mod generic_highlight;
mod highlight;
mod preview;
mod search;
mod settings;
mod tinymist;
mod toolchain;
mod workspace;

use std::path::PathBuf;

use app::EditorApp;
use eframe::egui;

fn main() -> eframe::Result {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([360.0, 260.0])
            // On macOS the normal title bar becomes part of the app toolbar. The
            // traffic-light controls remain native, while the otherwise empty
            // title strip no longer costs a row of vertical space.
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false),
        ..Default::default()
    };

    eframe::run_native(
        "tiptoptyp",
        options,
        Box::new(move |context| Ok(Box::new(EditorApp::new(context, initial_path)))),
    )
}
