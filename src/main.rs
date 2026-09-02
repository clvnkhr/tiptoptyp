mod app;
mod compiler;
mod diagnostics;
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
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "mytypst",
        options,
        Box::new(move |context| Ok(Box::new(EditorApp::new(context, initial_path)))),
    )
}
