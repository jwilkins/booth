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

    /// What the column says it is for, in the menu that adds and removes them.
    fn about(self) -> &'static str {
        match self {
            Column::Artist => "Who made it",
            Column::Title => "What it is called",
            Column::Bpm => "Tempo, from the grid",
            Column::Key => "Camelot key",
            Column::Energy => "How hard it goes, as a rank",
            Column::Stems => "Whether a kit has been rendered",
            Column::Location => "The file it plays from",
        }
    }

    /// How wide the column is before anybody drags it, and what "reset to
    /// default" puts it back to.
    ///
    /// The five narrow ones are as wide as their contents: a tempo is always
    /// six characters, a Camelot key two or three, and the energy column is
    /// wide enough for the letter-spaced word above the meter rather than for
    /// the meter. The three that hold names are wide because names are.
    pub fn default_width(self) -> f32 {
        match self {
            Column::Artist => 230.0,
            Column::Title => 360.0,
            Column::Bpm => 58.0,
            Column::Key => 42.0,
            Column::Energy => 64.0,
            Column::Stems => 88.0,
            Column::Location => 230.0,
        }
    }
}

/// The narrowest a column can be dragged.
///
/// Not zero: a column dragged shut is one whose handle has gone with it, and
/// the way to be rid of a column is to turn it off in the header's menu, where
/// it can be turned back on.
pub const MIN_WIDTH: f32 = 36.0;

/// How wide the strip is that a column boundary can be grabbed by.
const GRIP: f32 = 6.0;

/// One column as this person has it: how wide they left it, and whether they
/// want it at all.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Slot {
    pub column: Column,
    pub width: f32,
    pub shown: bool,
}

/// Which columns the list shows, in the order they are drawn, and how wide
/// each was left.
///
/// A preference about how this person reads their collection, like the sort
/// and the panel sizes — kept in the settings rather than in the collection,
/// because it says nothing about the music.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Layout {
    pub columns: Vec<Slot>,
}

impl Default for Layout {
    fn default() -> Self {
        let columns = Column::ALL
            .iter()
            .map(|&column| Slot { column, width: column.default_width(), shown: true })
            .collect();
        Self { columns }
    }
}

impl Layout {
    /// Make a layout read from the settings usable, whatever is in it.
    ///
    /// A file written by an older version knows nothing about a column added
    /// since, and one edited by hand can say anything. A missing column is
    /// added at its default rather than silently never appearing again; one
    /// listed twice is dropped; a width that is not a positive number is put
    /// back to the column's default; and a layout with nothing shown is put
    /// back to the default outright, because a list with no columns is a blank
    /// rectangle with no way out of it.
    ///
    /// A width merely narrower than a drag would allow is left alone: a window
    /// too narrow for the columns shows them scaled below that floor, and what
    /// it wrote down is what it was showing, not a mistake to be corrected on
    /// the way back in.
    pub fn repair(&mut self) {
        let mut seen = Vec::new();
        self.columns.retain(|slot| match seen.contains(&slot.column) {
            true => false,
            false => {
                seen.push(slot.column);
                true
            }
        });
        for column in Column::ALL {
            if !seen.contains(&column) {
                self.columns.push(Slot { column, width: column.default_width(), shown: true });
            }
        }
        for slot in &mut self.columns {
            if !slot.width.is_finite() || slot.width <= 0.0 {
                slot.width = slot.column.default_width();
            }
        }
        if !self.columns.iter().any(|slot| slot.shown) {
            *self = Self::default();
        }
    }

    /// How many columns are on.
    fn showing(&self) -> usize {
        self.columns.iter().filter(|slot| slot.shown).count()
    }

    /// Turn a column on or off, refusing to turn the last one off.
    pub fn toggle(&mut self, column: Column) {
        let showing = self.showing();
        for slot in &mut self.columns {
            if slot.column == column && !(slot.shown && showing == 1) {
                slot.shown = !slot.shown;
            }
        }
    }

    /// Write a width down as it is. The floor belongs to the drag, which
    /// refuses to go under it, and to [`Layout::repair`], which is reading
    /// something that may not have come from a drag at all — putting it here
    /// as well would mean a window too narrow to give every column its floor
    /// could not write down what it was actually showing.
    fn set(&mut self, column: Column, width: f32) {
        for slot in &mut self.columns {
            if slot.column == column {
                slot.width = width;
            }
        }
    }

    /// Move a boundary: `by` points from the column after `at` to the one at
    /// `at`, counting only the columns that are showing. Says whether it moved.
    ///
    /// Both ends at once, so the pair keeps its total and the columns to the
    /// right of it do not shuffle along under the pointer. Refused rather than
    /// clamped when either end would go under the floor: a drag that keeps
    /// eating into a column already at its narrowest would silently spend the
    /// next one after it.
    ///
    /// It works from the widths on screen, and writes them all back as it
    /// goes. That second half is what makes a drag land where the pointer is:
    /// a window narrower than the columns were left at shows them all scaled
    /// down, so writing a dragged width straight into the settings would put
    /// a number in that the next frame scales again — and the boundary would
    /// wander off the other way while the pointer pulled it. Taking what is on
    /// screen as what was meant makes the columns and the settings the same
    /// numbers, and from there a drag is a drag.
    pub fn drag_edge(&mut self, widths: &Widths, at: usize, by: f32) -> bool {
        let shown: Vec<(Column, f32)> = widths.iter().collect();
        let (Some(&(column, width)), Some(&(next, beside))) = (shown.get(at), shown.get(at + 1))
        else {
            return false;
        };
        let here = width + by;
        let there = beside - by;
        if by == 0.0 || here < MIN_WIDTH || there < MIN_WIDTH {
            return false;
        }
        for (which, was) in shown {
            self.set(which, was);
        }
        self.set(column, here);
        self.set(next, there);
        true
    }

    /// Lay the shown columns out across the width the list has.
    ///
    /// The list always fills the window: whatever is left over after the
    /// stored widths goes to the last column, and if they add up to more than
    /// there is room for they are all scaled down to fit. Scrolling sideways
    /// would be the other answer, and it is the wrong one here — a header that
    /// can be scrolled off is a header that stops saying what a column is.
    pub fn widths(&self, total: f32) -> Widths {
        let mut shown: Vec<(Column, f32)> =
            self.columns.iter().filter(|slot| slot.shown).map(|s| (s.column, s.width)).collect();
        if shown.is_empty() {
            return Widths(shown);
        }
        let sum: f32 = shown.iter().map(|(_, width)| *width).sum();
        if sum > total && sum > 0.0 {
            let scale = (total / sum).max(0.0);
            for (_, width) in &mut shown {
                *width *= scale;
            }
        } else if let Some((_, width)) = shown.last_mut() {
            *width += total - sum;
        }
        Widths(shown)
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

/// The shown columns and the width each gets this frame, in drawing order.
///
/// Worked out once and handed to both the header and the rows, so the two
/// cannot drift apart and leave a name over the wrong column.
pub struct Widths(Vec<(Column, f32)>);

impl Widths {
    pub fn iter(&self) -> impl Iterator<Item = (Column, f32)> + '_ {
        self.0.iter().copied()
    }

    /// How wide a column came out, or nothing if it is turned off.
    pub fn of(&self, column: Column) -> Option<f32> {
        self.0.iter().find(|(which, _)| *which == column).map(|(_, width)| *width)
    }

    pub fn total(&self) -> f32 {
        self.0.iter().map(|(_, width)| *width).sum()
    }
}

/// How tall one line is.
const ROW_HEIGHT: f32 = 19.0;
/// How far a stem companion is indented.
const INDENT: f32 = 14.0;

/// What the header was asked to do this frame.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    /// The column whose name was clicked, to sort by.
    pub sorted: Option<Column>,
    /// A boundary was dragged. Arrives as a stream of sub-point changes, so
    /// what it wants is to be written once the drag is over.
    pub resized: bool,
    /// A column was added, removed or put back to its default from the menu.
    /// One click, one answer: worth writing down now.
    pub chosen: bool,
}

/// Draw the header, and let it be worked: click a name to sort, drag a
/// boundary to resize, right-click for which columns there are at all.
///
/// The layout is taken by reference and changed in place, because a drag and a
/// menu both act on it and threading either back out as a value would mean
/// naming every way a header can be touched twice.
pub fn header_row(ui: &mut Ui, layout: &mut Layout, widths: &Widths, sort: Sort) -> Head {
    let mut head = Head::default();
    // Exactly as wide as the columns it is drawing, which is what the rows
    // under it get: a header that ran on across the scroll bar's gutter would
    // put its rule and its hit areas a few points past where the row below
    // ends, and the last column's name would answer for a strip of nothing.
    let (rect, response) = ui.allocate_exact_size(Vec2::new(widths.total(), 18.0), Sense::click());

    // The boundaries first, so that the pointer belongs to whichever handle it
    // is over rather than to the column name behind it: a drag that sorted the
    // list halfway through would be a nasty surprise.
    let shown: Vec<(Column, f32)> = widths.iter().collect();
    let mut grips = Vec::new();
    let mut x = rect.left();
    for (at, (_, width)) in shown.iter().enumerate() {
        x += width;
        // Nothing after the last column: what is on its right is the edge of
        // the window, and there is no neighbour to take the width from.
        if at + 1 == shown.len() {
            break;
        }
        let grip = Rect::from_min_max(
            egui::pos2(x - GRIP / 2.0, rect.top()),
            egui::pos2(x + GRIP / 2.0, rect.bottom()),
        );
        let handle = ui.interact(grip, ui.id().with(("column-edge", at)), Sense::drag());
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if layout.drag_edge(widths, at, handle.drag_delta().x) {
            head.resized = true;
        }
        grips.push((grip, handle.hovered() || handle.dragged()));
    }
    let on_a_grip = grips.iter().any(|(_, lit)| *lit);

    let painter = ui.painter_at(rect);
    let pointer = response.hover_pos();
    let mut x = rect.left();

    for (column, width) in shown.iter().copied() {
        // The whole column's width is the target, not just the word: a
        // ten-point label is a small thing to hit twice in a row.
        let area = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, rect.height()));
        let over = !on_a_grip && pointer.is_some_and(|at| area.contains(at));
        let on = sort.column == column;

        if over {
            painter.rect_filled(area, 0.0, theme::booth_2());
        }
        if over && response.clicked() {
            head.sorted = Some(column);
        }

        let color = if on { theme::amber() } else { theme::dim() };
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

    // Painted rather than built out of widgets, like the rows below it, so
    // this is the only thing that says the strip is there and that clicking it
    // does something — and it is what lets a test ask where the header is, to
    // check that scrolling the list has not carried it off.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "columns"));

    if pointer.is_some() && !on_a_grip {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // A boundary is invisible until the pointer finds it, and then it is a
    // line: enough to say the pointer is on something and that the something
    // runs the height of the strip.
    for (grip, lit) in grips {
        if lit {
            painter.line_segment(
                [grip.center_top(), grip.center_bottom()],
                egui::Stroke::new(1.0_f32, theme::amber()),
            );
        }
    }
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0_f32, theme::rule()),
    );

    columns_menu(&response, layout, &mut head);
    head
}

/// The header's right-click menu: which columns there are, and a way back.
///
/// Every column is listed whether it is on or off, because a menu that only
/// lists what is showing is one you cannot use to get a column back.
fn columns_menu(response: &egui::Response, layout: &mut Layout, head: &mut Head) {
    response.context_menu(|ui| {
        ui.set_min_width(200.0);
        ui.label(egui::RichText::new("Columns").color(theme::dim()).size(theme::SMALL));
        ui.separator();
        let showing = layout.showing();
        let listed: Vec<Slot> = layout.columns.clone();
        for slot in listed {
            // The last one left cannot be turned off: a list with no columns
            // is a blank rectangle, and the menu to undo it is on a header
            // that no longer has anything in it.
            let last = slot.shown && showing == 1;
            let mut on = slot.shown;
            let item = ui.add_enabled(
                !last,
                egui::Checkbox::new(&mut on, egui::RichText::new(slot.column.name())),
            );
            let item = match last {
                true => item.on_disabled_hover_text("The list needs one column"),
                false => item.on_hover_text(slot.column.about()),
            };
            if item.changed() {
                layout.toggle(slot.column);
                head.chosen = true;
            }
        }
        ui.separator();
        if ui.button("Reset to default").clicked() {
            *layout = Layout::default();
            head.chosen = true;
            ui.close();
        }
    });
}

/// The rows being dragged, by id.
///
/// Ids rather than tracks, because what is dropped has to be looked up in the
/// collection as it is when it lands, not as it was when the drag started.
///
/// More than one when the row picked up was part of a selection: grabbing one
/// of thirty chosen rows and having it arrive alone would be a quiet way of
/// losing twenty-nine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dragged(pub Vec<u32>);

/// Something asked of one track from its own line.
///
/// Each of these already exists as a batch over everything showing. They are
/// the same operations on a selection of one, deliberately: a re-analysis is
/// not a different thing from a first analysis, and two code paths for it are
/// how they come to disagree.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Play,
    /// Put it in the playlist at this index of [`Menu::playlists`].
    AddTo(usize),
    /// Put it in a playlist that does not exist yet.
    AddToNew,
    Analyze,
    Identify,
    Separate,
    /// Read the vocal stem and cue what it finds.
    AutoCue,
    CopyIn,
    Reveal,
    /// Take it out of the playlist being shown, leaving it in the collection.
    RemoveFromPlaylist,
    Forget,
}

impl Action {
    /// What the menu item says, given what the track already has.
    ///
    /// Re-doing something is named as re-doing it. An item that reads
    /// "Analyse" on a track that has already been analysed invites the reading
    /// that nothing will happen.
    pub fn label(self, track: &Track, menu: Menu<'_>) -> &'static str {
        match self {
            Action::Play if menu.playing => "Pause",
            Action::Play => "Play",
            Action::Analyze if track.analyzed => "Re-analyse",
            Action::Analyze => "Analyse",
            Action::Identify if track.identified => "Look up again",
            Action::Identify => "Look up tags",
            Action::Separate if !track.stems.is_empty() => "Render stems again",
            Action::Separate => "Render stems",
            Action::AutoCue if !track.lyrics.is_empty() => "Cue from the words again",
            Action::AutoCue => "Cue from the words",
            Action::AddTo(_) | Action::AddToNew => "Add to playlist",
            Action::CopyIn => "Copy into the library",
            Action::Reveal => "Copy the file path",
            Action::RemoveFromPlaylist => "Remove from this playlist",
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
pub struct Menu<'a> {
    /// Whether the file already sits under the library folder, which decides
    /// whether copying it in is offered at all.
    pub in_library: bool,
    /// Whether anything is playing this track now.
    pub playing: bool,
    /// Whether a playlist is what is being shown, which is the only place
    /// taking a track out of one means anything.
    pub in_playlist: bool,
    /// The playlists this track could be added to, in sidebar order.
    pub playlists: &'a [String],
    /// Everything a drag from this row should carry — this track alone, or the
    /// selection it is part of.
    pub dragging: &'a [u32],
}

/// Draw one line. Returns what the pointer did, if anything.
pub fn row(
    ui: &mut Ui,
    track: &Track,
    indented: bool,
    selected: bool,
    widths: &Widths,
    menu: Menu<'_>,
) -> Option<Hit> {
    let (rect, response) = ui
        .allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click_and_drag());
    let response = response.on_hover_text(match track.path.parent() {
        Some(_) => format!(
            "{}\ndouble-click to hear it, drag it onto a playlist, right-click for the rest",
            track.path.display()
        ),
        None => {
            "double-click to hear it, drag it onto a playlist, right-click for the rest".to_string()
        }
    });

    // The row is painted rather than built out of widgets, so this is the only
    // thing that says what it is. Without it the browser — the main thing in
    // the window — announces nothing at all, and nothing reading the window
    // through that tree can find a track by name.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("{} — {}", track.artist, track.display_title()),
        )
    });

    // Picking a row up.
    response.dnd_set_drag_payload(Dragged(menu.dragging.to_vec()));
    if response.dragged() {
        // What is being carried, at the pointer. Without it the drag is
        // invisible: the row stays where it is, and the only way to find out
        // whether anything was picked up is to let go somewhere and see.
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            egui::Id::new("dragging-a-row"),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            let carrying = match menu.dragging.len() {
                0 | 1 => format!("↳ {}", track.display_title()),
                n => format!("↳ {n} tracks"),
            };
            ui.label(egui::RichText::new(carrying).color(theme::amber()).size(11.0));
        });
    }
    let painter = ui.painter_at(rect);

    if selected {
        painter.rect_filled(rect, 0.0, theme::amber().gamma_multiply(0.14));
    } else if response.hovered() {
        painter.rect_filled(rect, 0.0, theme::booth_2());
    }
    painter.line_segment(
        [egui::pos2(rect.left(), rect.bottom()), egui::pos2(rect.right(), rect.bottom())],
        egui::Stroke::new(1.0_f32, theme::rule().gamma_multiply(0.55)),
    );

    // A companion is dim, and the whole line is dim rather than only its name:
    // it is one thing that belongs under another, not a track with a long title.
    let ink = if indented { theme::dim() } else { theme::text() };
    let mut x = rect.left();

    // Whichever columns are on, in whatever width they were left at. The row
    // walks the same list the header drew, so a column turned off here is a
    // column with nothing under its name rather than a gap in the middle.
    for (column, width) in widths.iter() {
        match column {
            Column::Artist => {
                let artist = match indented {
                    true => format!("↳ {}", track.artist),
                    false => track.artist.clone(),
                };
                let indent = if indented { INDENT } else { 0.0 };
                text(&painter, rect, x + indent, width - indent, &artist, ink, false);
            }
            Column::Title => text(&painter, rect, x, width, &track.display_title(), ink, false),
            Column::Bpm => {
                let bpm = if track.has_grid { format!("{:.2}", track.bpm) } else { "—".into() };
                text(&painter, rect, x, width, &bpm, ink, true);
            }
            Column::Key => {
                let key = if track.key.is_empty() { "—".to_string() } else { track.key.clone() };
                text(&painter, rect, x, width, &key, ink, true);
            }
            // A companion carries the parent's grid, so it does not repeat its
            // energy: a second meter for the same record is a second thing to
            // read for nothing.
            Column::Energy => {
                if !indented {
                    meter(&painter, rect, x, track.energy);
                }
            }
            Column::Stems => {
                if !track.stems.is_empty() || indented {
                    pill(
                        &painter,
                        rect,
                        x,
                        track.role.label(),
                        if indented { theme::dim() } else { theme::amber() },
                    );
                }
            }
            // The whole path, trimmed from the front when it will not fit, so
            // that the file name is always the part that survives.
            Column::Location => {
                let font = theme::mono(10.0);
                let location = fit_tail(&painter, &font, &location_of(track), width - 8.0);
                painter.text(
                    egui::pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    location,
                    font,
                    theme::dim(),
                );
            }
        }
        x += width;
    }

    // A companion is a file the parent owns: there is nothing to analyse, look
    // up or separate about it separately, and doing any of those to it would
    // put a second answer beside the one it inherited.
    let chosen = if indented {
        context_menu(&response, track, menu, &[Action::Play, Action::Reveal])
    } else {
        let mut items = vec![
            Action::Play,
            Action::AddToNew,
            Action::Analyze,
            Action::Identify,
            Action::Separate,
            Action::AutoCue,
        ];
        if !menu.in_library {
            items.push(Action::CopyIn);
        }
        items.push(Action::Reveal);
        if menu.in_playlist {
            items.push(Action::RemoveFromPlaylist);
        }
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
    menu: Menu<'_>,
    items: &[Action],
) -> Option<Action> {
    let mut chosen = None;
    response.context_menu(|ui| {
        ui.set_min_width(190.0);
        // The track it will act on, because a right-click does not select and
        // a menu with no subject is a menu you have to guess at.
        ui.label(egui::RichText::new(track.display_title()).color(theme::dim()).size(11.0));
        ui.separator();
        for &item in items {
            if matches!(item, Action::CopyIn | Action::RemoveFromPlaylist) {
                ui.separator();
            }
            // The one item that opens onto a list rather than doing something.
            if item == Action::AddToNew {
                ui.menu_button(egui::RichText::new("Add to playlist").color(theme::text()), |ui| {
                    for (at, name) in menu.playlists.iter().enumerate() {
                        if ui.button(name).clicked() {
                            chosen = Some(Action::AddTo(at));
                            ui.close();
                        }
                    }
                    if !menu.playlists.is_empty() {
                        ui.separator();
                    }
                    if ui.button("New playlist\u{2026}").clicked() {
                        chosen = Some(Action::AddToNew);
                        ui.close();
                    }
                });
                continue;
            }
            // Only the one that reaches past the playlist is coloured as a
            // warning: taking a track out of a list is a click away from being
            // undone, and losing it from the collection is not.
            let label = egui::RichText::new(item.label(track, menu))
                .color(if item == Action::Forget { theme::alert() } else { theme::text() });
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
        painter.rect_filled(block, 0.0, if filled { theme::amber() } else { theme::rule() });
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
        egui::Stroke::new(1.0_f32, theme::rule()),
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
    fn the_columns_fill_the_list_and_no_more() {
        // Whatever the window is, the header and the rows cover it exactly:
        // short of it leaves a strip of nothing on the right that the header
        // rule runs across anyway, and over it puts a column half off the
        // edge with no way to reach the rest of it.
        let layout = Layout::default();
        for total in [1400.0f32, 1072.0, 700.0, 320.0] {
            let widths = layout.widths(total);
            assert!(
                (widths.total() - total).abs() < 0.01,
                "{total} wide came out {}",
                widths.total()
            );
            assert_eq!(widths.iter().count(), Column::ALL.len(), "a column went missing");
        }
    }

    #[test]
    fn a_window_too_narrow_for_the_columns_scales_them_rather_than_dropping_one() {
        // The alternative is a column that is simply not there, which looks
        // exactly like one that was turned off — and the way to get it back
        // would be to widen a window nobody knew had to be widened.
        let widths = Layout::default().widths(300.0);
        assert_eq!(widths.iter().count(), Column::ALL.len());
        for (column, width) in widths.iter() {
            assert!(width > 0.0, "{} came out {width}", column.name());
        }
    }

    #[test]
    fn what_is_left_over_goes_to_the_last_column() {
        // So that a window wider than the columns were left at grows the one
        // thing that can use it — the file path, which is the longest thing in
        // the row — rather than opening a gap at the right.
        let layout = Layout::default();
        let stored: f32 = layout.columns.iter().map(|slot| slot.width).sum();
        let widths = layout.widths(stored + 200.0);
        assert_eq!(widths.of(Column::Bpm), Some(Column::Bpm.default_width()));
        assert_eq!(
            widths.of(Column::Location),
            Some(Column::Location.default_width() + 200.0),
            "the slack went somewhere else"
        );
    }

    #[test]
    fn a_column_turned_off_takes_its_width_with_it() {
        let mut layout = Layout::default();
        layout.toggle(Column::Location);
        let widths = layout.widths(1072.0);
        assert_eq!(widths.of(Column::Location), None, "a hidden column was still laid out");
        assert_eq!(widths.iter().count(), Column::ALL.len() - 1);
        assert!((widths.total() - 1072.0).abs() < 0.01, "the row stopped filling the window");

        // And comes back where it was, at the width it had.
        layout.toggle(Column::Location);
        assert_eq!(layout.widths(1072.0).iter().count(), Column::ALL.len());
    }

    #[test]
    fn dragging_a_boundary_moves_width_from_one_column_to_its_neighbour() {
        // The pair keeps its total, so the columns to the right of the one
        // being dragged stay where they are — otherwise widening the artist
        // column would slide the whole row along under the pointer.
        let mut layout = Layout::default();
        let before = layout.widths(1072.0);
        let artist = before.of(Column::Artist).unwrap();
        let title = before.of(Column::Title).unwrap();

        assert!(layout.drag_edge(&before, 0, 40.0), "the drag did nothing");
        let after = layout.widths(1072.0);
        assert_eq!(after.of(Column::Artist), Some(artist + 40.0));
        assert_eq!(after.of(Column::Title), Some(title - 40.0));
        assert_eq!(after.of(Column::Bpm), before.of(Column::Bpm), "a column beyond it moved");
        assert!((after.total() - 1072.0).abs() < 0.01, "the row stopped filling the window");
    }

    #[test]
    fn a_drag_in_a_window_too_narrow_for_the_columns_still_lands_where_it_is_pulled() {
        // The bug this is here for: with the columns scaled down to fit, a
        // width written straight into the settings gets scaled again on the
        // next frame, and the boundary walks the wrong way while the pointer
        // pulls it the right way. Pulled sixty points, it went eleven back.
        let mut layout = Layout::default();
        let narrow = 780.0;
        let before = layout.widths(narrow);
        let artist = before.of(Column::Artist).unwrap();
        assert!(before.total() < layout.columns.iter().map(|s| s.width).sum::<f32>());

        assert!(layout.drag_edge(&before, 0, 30.0));
        let after = layout.widths(narrow);
        assert_eq!(after.of(Column::Artist), Some(artist + 30.0), "the drag did not land");
        assert!((after.total() - narrow).abs() < 0.01, "the row stopped filling the window");

        // And the one after it, from where the first left off, because a drag
        // is a stream of these rather than one.
        assert!(layout.drag_edge(&after, 0, 30.0));
        assert_eq!(layout.widths(narrow).of(Column::Artist), Some(artist + 60.0));
    }

    #[test]
    fn a_boundary_stops_rather_than_eating_the_column_past_the_next() {
        // Dragging left with the next column already at its narrowest has to
        // stop somewhere. Refusing is the honest answer: clamping would leave
        // the pointer travelling while nothing moved, and taking the rest out
        // of the column after that would move a boundary nobody grabbed.
        let mut layout = Layout::default();
        let widths = layout.widths(1072.0);
        let title = widths.of(Column::Title).unwrap();
        assert!(!layout.drag_edge(&widths, 0, title - MIN_WIDTH + 1.0), "it ate past the floor");
        assert_eq!(layout.widths(1072.0).of(Column::Title), Some(title), "it moved anyway");
    }

    #[test]
    fn there_is_no_boundary_after_the_last_column() {
        // What is on its right is the edge of the window, and there is no
        // neighbour to take the width from.
        let mut layout = Layout::default();
        let widths = layout.widths(1072.0);
        assert!(!layout.drag_edge(&widths, Column::ALL.len() - 1, 20.0));
        assert!(!layout.drag_edge(&widths, 40, 20.0));
    }

    #[test]
    fn the_last_column_cannot_be_turned_off() {
        // The menu that turns them back on is on the header, and a header with
        // no columns in it has nothing to right-click.
        let mut layout = Layout::default();
        for column in Column::ALL {
            layout.toggle(column);
        }
        let left: Vec<Column> =
            layout.columns.iter().filter(|s| s.shown).map(|s| s.column).collect();
        assert_eq!(left.len(), 1, "the list was left with {} columns", left.len());
    }

    #[test]
    fn a_layout_from_an_older_settings_file_gains_what_it_never_had() {
        // The case this is really for: a column added to the program after
        // somebody last saved. Without repair it would never appear for them
        // again, and the menu would not list it either.
        let mut layout = Layout::default();
        layout.columns.retain(|slot| slot.column != Column::Stems);
        layout.columns.push(Slot { column: Column::Bpm, width: 0.0, shown: true });
        layout.repair();

        assert_eq!(layout.columns.len(), Column::ALL.len(), "a column was left out or doubled");
        assert!(layout.columns.iter().any(|slot| slot.column == Column::Stems));
        for slot in &layout.columns {
            assert!(slot.width > 0.0, "{} came back at {}", slot.column.name(), slot.width);
        }

        // And a layout with nothing shown goes back to the default rather than
        // drawing a blank rectangle with no menu on it.
        let mut empty = Layout::default();
        for slot in &mut empty.columns {
            slot.shown = false;
        }
        empty.repair();
        assert_eq!(empty, Layout::default());
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
    fn a_companion_row_is_labelled_as_one() {
        let mut track = Track::placeholder(1);
        track.title = "Marius".into();
        assert_eq!(track.display_title(), "Marius");
        assert_eq!(track.role.label(), "original");
        track.role = Role::Vocals;
        assert_eq!(track.display_title(), "Marius (vocals)");
        assert_eq!(track.role.label(), "vocals");
    }
}
