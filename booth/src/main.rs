//! The program: read any paths named on the command line, then show the window.

#![windows_subsystem = "windows"]

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    // Before anything else, so that a failure to open a window is itself in the
    // log. `BOOTH_LOG=warn` turns it down; `off` turns it off.
    booth::log::start(Some(&booth::log::default_path()));
    booth::info!(
        "booth {} starting — log at {}",
        env!("CARGO_PKG_VERSION"),
        booth::log::default_path().display()
    );
    // The things a bug report needs and nobody thinks to ask for. Written
    // every run, because the run that goes wrong is never the one somebody
    // turned the logging up for first.
    booth::info!(
        "{} on {}, {} — everything below is UTC",
        std::env::consts::ARCH,
        std::env::consts::OS,
        booth::clock::stamp(booth::clock::now())
    );
    booth::info!("data in {}", booth::library::data_dir().display());
    booth::debug!("logging at {} (BOOTH_LOG to change it)", booth::log::threshold());
    if let Ok(at) = std::env::current_dir() {
        booth::debug!("started in {}", at.display());
    }

    // Anything named on the command line is imported at startup, which makes
    // the window usable from a shell and from a file manager's "open with".
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if !files.is_empty() {
        booth::info!("{} named on the command line", files.len());
        for file in &files {
            booth::debug!("  {}", file.display());
        }
    }

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
