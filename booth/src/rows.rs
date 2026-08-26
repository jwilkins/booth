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

/// A column, which is both something to draw and something to sort by.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Column {
    Artist,
    Title,
    Bpm,
    Key,
    Energy,
    Stems,
    /// Where the file is.
    Location,
}

impl Column {
    pub const ALL: [Column; 7] = [
        Column::Artist,
        Column::Title,
        Column::Bpm,
        Column::Key,
        Column::Energy,
        Column::Stems,
        Column::Location,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Column::Artist => "Artist",
            Column::Title => "Title",
            Column::Bpm => "BPM",
            Column::Key => "Key",
            Column::Energy => "Energy",
            Column::Stems => "Stems",
            Column::Location => "Location",
        }
    }

    /// Which way round the first click sorts.
    ///
    /// Names read forwards; a measurement is nearly always wanted loudest or
    /// fastest first, because that is the end of a crate a set is built from.
    pub fn starts_descending(self) -> bool {
        matches!(self, Column::Bpm | Column::Energy)
    }

    fn width(self, widths: &Widths) -> f32 {
        match self {
            Column::Artist => widths.artist,
            Column::Title => widths.title,
            Column::Bpm => widths.bpm,
            Column::Key => widths.key,
            Column::Energy => widths.energy,
            Column::Stems => widths.stems,
            Column::Location => widths.location,
        }
    }
}

/// Which column the list is ordered by, and which way.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self { column: Column::Artist, descending: false }
    }
}

impl Sort {
    /// What clicking a header does: the same column turns round, a different
    /// one starts however that column prefers to start.
    pub fn clicked(self, column: Column) -> Self {
        match self.column == column {
            true => Self { column, descending: !self.descending },
            false => Self { column, descending: column.starts_descending() },
        }
    }

    /// The mark next to the column being sorted by.
    pub fn arrow(self) -> &'static str {
        match self.descending {
            true => "\u{25BE}",
            false => "\u{25B4}",
        }
    }
}

/// The width of each column, in the order they are drawn.
pub struct Widths {
    pub artist: f32,
    pub title: f32,
    pub bpm: f32,
    pub key: f32,
    pub energy: f32,
    pub stems: f32,
    pub location: f32,
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
    // The names and the location share what is left. The location gets the
    // smallest share of the three because it is the one that can be read from
    // its tail — a folder name — while a title cannot.
    let flexible = (total - bpm - key - energy - stems - 24.0).max(240.0);
    Widths {
        artist: flexible * 0.28,
        title: flexible * 0.44,
        location: flexible * 0.28,
        bpm,
        key,
        energy,
        stems,
    }
}

/// Draw the header. Returns the column whose name was clicked.
pub fn header_row(ui: &mut Ui, widths: &Widths, sort: Sort) -> Option<Column> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click());
    let painter = ui.painter_at(rect);
    let pointer = response.hover_pos();
    let mut clicked = None;
    let mut x = rect.left();

    for column in Column::ALL {
        let width = column.width(widths);
        // The whole column's width is the target, not just the word: a
        // ten-point label is a small thing to hit twice in a row.
        let area = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, rect.height()));
        let over = pointer.is_some_and(|at| area.contains(at));
        let on = sort.column == column;

        if over {
            painter.rect_filled(area, 0.0, theme::BOOTH_2);
        }
        if over && response.clicked() {
            clicked = Some(column);
        }

        let color = if on { theme::AMBER } else { theme::DIM };
        let text = match on {
            true => format!("{} {}", theme::label_text(column.name()), sort.arrow()),
            false => theme::label_text(column.name()),
        };
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            theme::sans(theme::LABEL),
            color,
        );
        x += width;
    }

    if pointer.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0, theme::RULE),
    );
    clicked
}

/// Something asked of one track from its own line.
///
/// Each of these already exists as a batch over everything showing. They are
/// the same operations on a selection of one, deliberately: a re-analysis is
/// not a different thing from a first analysis, and two code paths for it are
/// how they come to disagree.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Play,
    Analyze,
    Identify,
    Separate,
    CopyIn,
    Reveal,
    Forget,
}

impl Action {
    /// What the menu item says, given what the track already has.
    ///
    /// Re-doing something is named as re-doing it. An item that reads
    /// "Analyse" on a track that has already been analysed invites the reading
    /// that nothing will happen.
    pub fn label(self, track: &Track, menu: Menu) -> &'static str {
        match self {
            Action::Play if menu.playing => "Pause",
            Action::Play => "Play",
            Action::Analyze if track.analyzed => "Re-analyse",
            Action::Analyze => "Analyse",
            Action::Identify if track.identified => "Look up again",
            Action::Identify => "Look up tags",
            Action::Separate if !track.stems.is_empty() => "Render stems again",
            Action::Separate => "Render stems",
            Action::CopyIn => "Copy into the library",
            Action::Reveal => "Copy the file path",
            Action::Forget => "Remove from the collection",
        }
    }
}

/// What the pointer did to a row.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Selected it.
    Clicked,
    /// Asked to hear it.
    Opened,
    /// Picked something out of the right-click menu.
    Chose(Action),
}

/// What the menu needs to know that the row itself cannot see.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// Whether the file already sits under the library folder, which decides
    /// whether copying it in is offered at all.
    pub in_library: bool,
    /// Whether anything is playing this track now.
    pub playing: bool,
}

/// Draw one line. Returns what the pointer did, if anything.
pub fn row(
    ui: &mut Ui,
    track: &Track,
    indented: bool,
    selected: bool,
    widths: &Widths,
    menu: Menu,
) -> Option<Hit> {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_text(match track.path.parent() {
        Some(_) => {
            format!("{}\ndouble-click to hear it, right-click for the rest", track.path.display())
        }
        None => "double-click to hear it, right-click for the rest".to_string(),
    });
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
    x += widths.stems;

    // The whole path, trimmed from the front when it will not fit, so that the
    // file name is always the part that survives.
    let font = theme::mono(10.0);
    let location = fit_tail(&painter, &font, &location_of(track), widths.location - 8.0);
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        location,
        font,
        theme::DIM,
    );

    // A companion is a file the parent owns: there is nothing to analyse, look
    // up or separate about it separately, and doing any of those to it would
    // put a second answer beside the one it inherited.
    let chosen = if indented {
        context_menu(&response, track, menu, &[Action::Play, Action::Reveal])
    } else {
        let mut items = vec![Action::Play, Action::Analyze, Action::Identify, Action::Separate];
        if !menu.in_library {
            items.push(Action::CopyIn);
        }
        items.push(Action::Reveal);
        items.push(Action::Forget);
        context_menu(&response, track, menu, &items)
    };
    if let Some(action) = chosen {
        return Some(Hit::Chose(action));
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

/// The right-click menu, with a rule before the two items that are not undoable
/// by clicking again.
fn context_menu(
    response: &egui::Response,
    track: &Track,
    menu: Menu,
    items: &[Action],
) -> Option<Action> {
    let mut chosen = None;
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        // The track it will act on, because a right-click does not select and
        // a menu with no subject is a menu you have to guess at.
        ui.label(egui::RichText::new(track.display_title()).color(theme::DIM).size(11.0));
        ui.separator();
        for &item in items {
            if matches!(item, Action::CopyIn | Action::Forget) {
                ui.separator();
            }
            let label = egui::RichText::new(item.label(track, menu))
                .color(if item == Action::Forget { theme::ALERT } else { theme::TEXT });
            if ui.button(label).clicked() {
                chosen = Some(item);
                ui.close();
            }
        }
    });
    chosen
}

/// Order two tracks by a column.
///
/// A track with nothing in the column always sorts last, whichever way round
/// the sort is: an ungridded track is not slower than every other track, and
/// burying the ones still to be worked on under a reversed sort would hide
/// exactly the ones being looked for. Ties fall back to artist and title so
/// that the order is stable rather than whatever the collection happened to be
/// in.
pub fn compare(a: &Track, b: &Track, sort: Sort) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let missing = |track: &Track| match sort.column {
        Column::Artist => track.artist.trim().is_empty(),
        Column::Title => track.title.trim().is_empty(),
        Column::Bpm => !track.has_grid || track.bpm <= 0.0,
        Column::Key => track.key.is_empty(),
        Column::Energy => track.energy == 0,
        Column::Stems => track.stems.is_empty(),
        Column::Location => track.path.as_os_str().is_empty(),
    };
    match (missing(a), missing(b)) {
        (true, true) => return names(a, b),
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        (false, false) => {}
    }

    let order = match sort.column {
        Column::Artist => text_of(&a.artist).cmp(&text_of(&b.artist)),
        Column::Title => text_of(&a.title).cmp(&text_of(&b.title)),
        Column::Bpm => a.bpm.total_cmp(&b.bpm),
        // By the wheel's own order — 1A, 1B, 2A — rather than alphabetically,
        // which would put 10A between 1A and 2A and make the column useless for
        // the one thing it is read for.
        Column::Key => wheel(&a.key).cmp(&wheel(&b.key)),
        Column::Energy => a.energy.cmp(&b.energy),
        Column::Stems => a.stems.is_complete().cmp(&b.stems.is_complete()),
        Column::Location => a.path.cmp(&b.path),
    };
    let order = match sort.descending {
        true => order.reverse(),
        false => order,
    };
    order.then_with(|| names(a, b))
}

fn names(a: &Track, b: &Track) -> std::cmp::Ordering {
    text_of(&a.artist)
        .cmp(&text_of(&b.artist))
        .then_with(|| text_of(&a.title).cmp(&text_of(&b.title)))
}

/// Case-insensitively, because a crate sorted with the capitals first is not
/// sorted in any way a person means.
fn text_of(value: &str) -> String {
    value.to_lowercase()
}

/// A Camelot key as a number the wheel's order agrees with.
fn wheel(key: &str) -> u32 {
    let trimmed = key.trim();
    let Some(letter) = trimmed.chars().last() else { return u32::MAX };
    let number: u32 = trimmed[..trimmed.len() - letter.len_utf8()].parse().unwrap_or(u32::MAX / 4);
    let side = match letter.to_ascii_uppercase() {
        'A' => 0,
        'B' => 1,
        _ => 2,
    };
    number * 4 + side
}

/// Where a track's file is, folder and file name both.
///
/// Home is `~`, because most of a path under it is the same for every track and
/// says nothing. When the column is too narrow for the rest, it is the *front*
/// that goes — see [`fit_tail`] — because the end is the part that identifies
/// the file.
pub fn location_of(track: &Track) -> String {
    // For a companion, the stem it is made of rather than its parent's file:
    // a row that names somebody else's path is worse than one that names none.
    // An instrumental is two files, and the first of them is enough to say
    // which folder to look in, which is what the column is read for.
    let shown = match track.sources().first() {
        Some(path) => path.display().to_string(),
        None => track.path.display().to_string(),
    };
    match std::env::var_os("HOME").map(|home| home.to_string_lossy().into_owned()) {
        Some(home) if !home.is_empty() && shown.starts_with(&home) => {
            format!("~{}", &shown[home.len()..])
        }
        _ => shown,
    }
}

/// Trim a string from the front until it fits, marking where it was cut.
///
/// The opposite of the usual truncation, and deliberately: a path clipped at
/// the end is every path in the same folder looking identical, which is the one
/// thing this column exists not to do.
fn fit_tail(painter: &egui::Painter, font: &egui::FontId, text: &str, width: f32) -> String {
    let measure = |value: &str| {
        painter.layout_no_wrap(value.to_string(), font.clone(), Color32::WHITE).size().x
    };
    if measure(text) <= width {
        return text.to_string();
    }

    // Cut on path separators where possible, so what is left still reads as a
    // path rather than as a word chopped in half.
    let mut best: Option<String> = None;
    for (at, _) in text.match_indices('/') {
        let candidate = format!("…{}", &text[at..]);
        if measure(&candidate) <= width {
            best = Some(candidate);
            break;
        }
    }
    if let Some(fitted) = best {
        return fitted;
    }

    // No separator left that fits: fall back to characters, so a very narrow
    // column still shows the end of the file name rather than nothing.
    let mut start = 0;
    for (at, _) in text.char_indices() {
        start = at;
        if measure(&format!("…{}", &text[at..])) <= width {
            break;
        }
    }
    format!("…{}", &text[start..])
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
    use std::path::PathBuf;

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

    fn track(id: u32, artist: &str, title: &str, bpm: f64, key: &str) -> Track {
        let mut track = Track::placeholder(id);
        track.artist = artist.into();
        track.title = title.into();
        track.bpm = bpm;
        track.has_grid = bpm > 0.0;
        track.key = key.into();
        track.energy = 3;
        track
    }

    fn order(mut tracks: Vec<Track>, sort: Sort) -> Vec<String> {
        tracks.sort_by(|a, b| compare(a, b, sort));
        tracks.into_iter().map(|t| t.title).collect()
    }

    #[test]
    fn clicking_a_column_sorts_by_it_and_clicking_again_turns_it_round() {
        let sort = Sort::default();
        assert_eq!(sort.column, Column::Artist);

        let by_title = sort.clicked(Column::Title);
        assert_eq!(by_title.column, Column::Title);
        assert!(!by_title.descending, "names read forwards first");

        assert!(by_title.clicked(Column::Title).descending, "the same column turns round");
        assert!(!by_title.clicked(Column::Title).clicked(Column::Title).descending);
    }

    #[test]
    fn a_measurement_starts_at_the_loud_end() {
        // A tempo or an energy is nearly always wanted highest first: that is
        // the end of a crate a set gets built from.
        assert!(Sort::default().clicked(Column::Bpm).descending);
        assert!(Sort::default().clicked(Column::Energy).descending);
        assert!(!Sort::default().clicked(Column::Location).descending);
    }

    #[test]
    fn text_sorts_without_the_capitals_first() {
        let tracks =
            vec![track(1, "batu", "lower", 128.0, "8A"), track(2, "Alpha", "upper", 128.0, "8A")];
        let sort = Sort { column: Column::Artist, descending: false };
        assert_eq!(order(tracks, sort), vec!["upper", "lower"]);
    }

    #[test]
    fn a_tempo_sorts_as_a_number_rather_than_as_text() {
        let tracks = vec![
            track(1, "a", "ninety", 90.0, "8A"),
            track(2, "b", "one-seventy", 170.0, "8A"),
            track(3, "c", "one-twenty-eight", 128.0, "8A"),
        ];
        let sort = Sort { column: Column::Bpm, descending: false };
        assert_eq!(order(tracks.clone(), sort), vec!["ninety", "one-twenty-eight", "one-seventy"]);

        let sort = Sort { column: Column::Bpm, descending: true };
        assert_eq!(order(tracks, sort), vec!["one-seventy", "one-twenty-eight", "ninety"]);
    }

    #[test]
    fn keys_sort_around_the_wheel_rather_than_alphabetically() {
        // Alphabetically, 10A falls between 1A and 2A, which makes the column
        // useless for the one thing it is read for.
        let tracks = vec![
            track(1, "a", "ten", 128.0, "10A"),
            track(2, "b", "one", 128.0, "1A"),
            track(3, "c", "two", 128.0, "2A"),
            track(4, "d", "one-b", 128.0, "1B"),
        ];
        let sort = Sort { column: Column::Key, descending: false };
        assert_eq!(order(tracks, sort), vec!["one", "one-b", "two", "ten"]);
    }

    #[test]
    fn a_track_with_nothing_in_the_column_sorts_last_either_way() {
        // Reversing a sort must not bury the tracks still to be worked on
        // under everything else — they are usually what is being looked for.
        let mut ungridded = track(3, "c", "no grid", 0.0, "");
        ungridded.has_grid = false;
        let tracks =
            vec![track(1, "a", "slow", 90.0, "8A"), track(2, "b", "fast", 170.0, "8A"), ungridded];

        for descending in [false, true] {
            let sort = Sort { column: Column::Bpm, descending };
            let listed = order(tracks.clone(), sort);
            assert_eq!(listed.last().unwrap(), "no grid", "descending: {descending}");
        }
    }

    #[test]
    fn a_missing_key_sorts_last_too() {
        let tracks = vec![
            track(1, "a", "keyed", 128.0, "8A"),
            track(2, "b", "no key", 128.0, ""),
            track(3, "c", "also keyed", 128.0, "2A"),
        ];
        for descending in [false, true] {
            let sort = Sort { column: Column::Key, descending };
            assert_eq!(order(tracks.clone(), sort).last().unwrap(), "no key");
        }
    }

    #[test]
    fn a_tie_falls_back_to_the_names_so_the_order_is_stable() {
        // Every tempo the same, so only the fallback decides — and it has to
        // decide the same way every time the list is rebuilt.
        let tracks = vec![
            track(1, "Zed", "z", 128.0, "8A"),
            track(2, "Alpha", "a", 128.0, "8A"),
            track(3, "Mid", "m", 128.0, "8A"),
        ];
        let sort = Sort { column: Column::Bpm, descending: false };
        assert_eq!(order(tracks.clone(), sort), vec!["a", "m", "z"]);
        assert_eq!(order(tracks, sort), vec!["a", "m", "z"], "twice the same");
    }

    #[test]
    fn sorting_by_location_groups_a_folder_together() {
        let mut one = track(1, "a", "first", 128.0, "8A");
        let mut two = track(2, "b", "second", 128.0, "8A");
        let mut three = track(3, "c", "third", 128.0, "8A");
        one.path = "/music/Batu/x.flac".into();
        two.path = "/downloads/y.flac".into();
        three.path = "/music/Batu/a.flac".into();

        let sort = Sort { column: Column::Location, descending: false };
        assert_eq!(order(vec![one, two, three], sort), vec!["second", "third", "first"]);
    }

    #[test]
    fn a_location_is_the_whole_path_shortened_at_home() {
        let mut track = Track::placeholder(1);
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            track.path = PathBuf::from(format!("{home}/Music/Booth/Batu/Marius.flac"));
            assert_eq!(location_of(&track), "~/Music/Booth/Batu/Marius.flac");
        }

        track.path = "/mnt/usb/Batu/Marius.flac".into();
        assert_eq!(location_of(&track), "/mnt/usb/Batu/Marius.flac");
    }

    #[test]
    fn a_location_always_names_the_file() {
        let mut track = Track::placeholder(1);
        track.path = "Marius.flac".into();
        assert_eq!(location_of(&track), "Marius.flac");

        // Two files in the same folder are two different locations, which is
        // the whole reason the file name is in there.
        let mut other = Track::placeholder(2);
        track.path = "/music/Batu/a.flac".into();
        other.path = "/music/Batu/b.flac".into();
        assert_ne!(location_of(&track), location_of(&other));
    }

    #[test]
    fn every_column_fits_in_the_window() {
        let widths = columns(1200.0);
        let total: f32 = Column::ALL.iter().map(|c| c.width(&widths)).sum();
        assert!(total <= 1200.0, "the columns overflowed: {total}");
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
