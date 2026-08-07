//! The musicai window.
//!
//! A front end for the same library the command-line tool uses. It is a
//! separate binary rather than a `musicai gui` subcommand so that the terminal
//! tool stays free of a window toolkit and its dependencies.

// A release build should not open a console window behind the app on Windows.
// Harmless everywhere else, and this crate is meant to be portable even though
// macOS is what it is packaged for.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod job;

fn main() -> eframe::Result {
    // Files named on the command line start out selected. That makes the app
    // usable from a shell, and on macOS it is how "Open With" hands over a
    // selection from Finder.
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([720.0, 520.0])
            .with_title("musicai")
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "musicai",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, files)))),
    )
}
