//! The list itself: one line per track, stem companions indented under theirs.
//!
//! Drawn by hand rather than with a table widget because every column here has
//! a fixed job — the numbers are tabular and monospaced so a crate can be
//! scanned down rather than read across, and the energy meter is five blocks
//! rather than a number because it is a rank and should not look like a
//! measurement.

use eframe::egui::{self, Color32, Rect, Sense, Ui, Vec2};

use crate::library::Track;
use crate::theme;

/// The width of each column, in the order they are drawn.
pub struct Widths {
    pub artist: f32,
    pub title: f32,
    pub bpm: f32,
    pub key: f32,
    pub energy: f32,
    pub stems: f32,
}

/// How tall one line is.
const ROW_HEIGHT: f32 = 19.0;
/// How far a stem companion is indented.
const INDENT: f32 = 14.0;

/// Split the available width between the columns.
///
/// The five narrow ones are fixed, because their contents are: a tempo is
/// always six characters, a Camelot key is always two or three. Whatever is
/// left goes to the names, which is where a wider window actually helps.
pub fn columns(total: f32) -> Widths {
    let bpm = 58.0;
    let key = 42.0;
    // Wide enough for the letter-spaced header rather than for the meter,
    // which is narrower than the word above it.
    let energy = 64.0;
    let stems = 88.0;
    let names = (total - bpm - key - energy - stems - 24.0).max(160.0);
    Widths { artist: names * 0.38, title: names * 0.62, bpm, key, energy, stems }
}

pub fn header_row(ui: &mut Ui, widths: &Widths) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let mut x = rect.left();

    for (name, width) in [
        ("Artist", widths.artist),
        ("Title", widths.title),
        ("BPM", widths.bpm),
        ("Key", widths.key),
        ("Energy", widths.energy),
        ("Stems", widths.stems),
    ] {
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            theme::label_text(name),
            theme::sans(theme::LABEL),
            theme::DIM,
        );
        x += width;
    }
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0, theme::RULE),
    );
}

/// What the pointer did to a row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Selected it.
    Clicked,
    /// Asked to hear it.
    Opened,
}

/// Draw one line. Returns what the pointer did, if anything.
pub fn row(
    ui: &mut Ui,
    track: &Track,
    indented: bool,
    selected: bool,
    widths: &Widths,
) -> Option<Hit> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_text("double-click to hear it");
    let painter = ui.painter_at(rect);

    if selected {
        painter.rect_filled(rect, 0.0, theme::AMBER.gamma_multiply(0.14));
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, theme::BOOTH_2);
    }
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0, theme::RULE.gamma_multiply(0.55)),
    );

    // A companion is dim, and the whole line is dim rather than only its name:
    // it is one thing that belongs under another, not a track with a long title.
    let ink = if indented { theme::DIM } else { theme::TEXT };
    let mut x = rect.left();

    let artist = if indented { format!("↳ {}", track.artist) } else { track.artist.clone() };
    text(
        &painter,
        rect,
        x + if indented { INDENT } else { 0.0 },
        widths.artist,
        &artist,
        ink,
        false,
    );
    x += widths.artist;

    text(&painter, rect, x, widths.title, &track.display_title(), ink, false);
    x += widths.title;

    let bpm = if track.has_grid { format!("{:.2}", track.bpm) } else { "—".into() };
    text(&painter, rect, x, widths.bpm, &bpm, ink, true);
    x += widths.bpm;

    let key = if track.key.is_empty() { "—".to_string() } else { track.key.clone() };
    text(&painter, rect, x, widths.key, &key, ink, true);
    x += widths.key;

    // A companion carries the parent's grid, so it does not repeat its energy:
    // a second meter for the same record is a second thing to read for nothing.
    if !indented {
        meter(&painter, rect, x, track.energy);
    }
    x += widths.energy;

    if !track.stems.is_empty() || indented {
        pill(
            &painter,
            rect,
            x,
            track.role.stems(),
            if indented { theme::DIM } else { theme::AMBER },
        );
    }

    // A double click is also a click, so the double is checked first: opening a
    // row selects it too, and reporting both would start playback and then
    // immediately be told to select something.
    match (response.double_clicked(), response.clicked()) {
        (true, _) => Some(Hit::Opened),
        (_, true) => Some(Hit::Clicked),
        _ => None,
    }
}

fn text(
    painter: &egui::Painter,
    rect: Rect,
    x: f32,
    width: f32,
    content: &str,
    color: Color32,
    numeric: bool,
) {
    let font = if numeric { theme::mono(theme::SMALL) } else { theme::sans(theme::BODY) };
    // Clipped rather than wrapped or shortened with an ellipsis: a row is one
    // line tall, and a truncation mark costs two characters of the name.
    let galley = painter.layout_no_wrap(content.to_string(), font, color);
    let clip =
        Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width - 8.0, rect.height()));
    painter.with_clip_rect(clip.intersect(rect)).galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// Five blocks, filled to the track's energy.
fn meter(painter: &egui::Painter, rect: Rect, x: f32, energy: u8) {
    for step in 0..5u8 {
        let block = Rect::from_min_size(
            egui::pos2(x + step as f32 * 7.0, rect.center().y - 4.5),
            Vec2::new(5.0, 9.0),
        );
        let filled = step < energy;
        painter.rect_filled(block, 0.0, if filled { theme::AMBER } else { theme::RULE });
    }
}

/// A bordered label, for what a row's stems are.
fn pill(painter: &egui::Painter, rect: Rect, x: f32, content: &str, color: Color32) {
    let galley = painter.layout_no_wrap(content.to_string(), theme::mono(10.0), color);
    let box_rect = Rect::from_min_size(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0 - 1.0),
        galley.size() + Vec2::new(8.0, 2.0),
    );
    painter.rect_stroke(
        box_rect,
        2.0,
        egui::Stroke::new(1.0, theme::RULE),
        egui::StrokeKind::Inside,
    );
    painter.galley(egui::pos2(x + 4.0, box_rect.top() + 1.0), galley, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Role;

    #[test]
    fn the_names_get_whatever_the_numbers_do_not() {
        let widths = columns(1000.0);
        let total =
            widths.artist + widths.title + widths.bpm + widths.key + widths.energy + widths.stems;
        assert!(total <= 1000.0, "the columns overflowed the window: {total}");
        assert!(widths.title > widths.artist, "titles are longer than artist names");
    }

    #[test]
    fn a_narrow_window_still_leaves_room_for_the_names() {
        // Below a certain width the columns cannot all fit; the names keep a
        // floor rather than collapsing to nothing, and the row clips instead.
        let widths = columns(120.0);
        assert!(widths.artist > 0.0 && widths.title > 0.0);
        assert_eq!(widths.bpm, 58.0, "the fixed columns stay fixed");
        assert_eq!(widths.energy, 64.0);
    }

    #[test]
    fn a_companion_row_is_labelled_as_one() {
        let mut track = Track::placeholder(1);
        track.title = "Marius".into();
        assert_eq!(track.display_title(), "Marius");
        track.role = Role::Acapella;
        assert_eq!(track.display_title(), "Marius (acapella)");
        assert_eq!(track.role.stems(), "vocals");
    }
}
