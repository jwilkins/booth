//! The program: read any paths named on the command line, then show the window.

#![windows_subsystem = "windows"]

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // Anything named on the command line is imported at startup, which makes
    // the window usable from a shell and from a file manager's "open with".
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("Booth"),
        ..Default::default()
    };

    eframe::run_native(
        "Booth",
        options,
        Box::new(|cc| Ok(Box::new(booth::app::App::new(cc, files)))),
    )
}
