//! The browse menu: what a player offers when the encoder is turned, and the
//! orders it can sort a list into.
//!
//! Both databases on a drive carry it, and a player builds its browse screen
//! from the copy in the one it reads. In `export.pdb` it is three tables — the
//! column names (16), the categories (17) and the sort orders (18); in
//! `exportLibrary.db` it is `menuItem`, `category` and `sort`. What is here is
//! the one definition both are written from, so the two files cannot disagree
//! about what a player should show.
//!
//! The categories and sort orders are rekordbox's defaults: what a fresh
//! install exports before anybody opens *Preferences → Advanced → Browse*.
//! Both real exports examined carry the same twenty-two and seventeen rows —
//! the same ids, columns and kinds, row for row — and differ only in which are
//! shown and in what order, which is exactly what that preference pane changes.
//! baken, whose drives play on a CDJ-3000, writes the default set too, byte for
//! byte what the first of the two exports has. So the defaults are what this
//! writes: ARTIST, ALBUM, TRACK, KEY, PLAYLIST, HISTORY, SEARCH, MATCHING,
//! FOLDER and DATE ADDED down the menu, and DEFAULT, ALPHABET, ARTIST, ALBUM,
//! BPM, RATING and KEY in the sort list.
//!
//! Tables 17 and 18 were empty on every drive this program wrote until a
//! CDJ-3000 on firmware 3.22 was found to see such a drive's database, count
//! its playlists on the Sources screen, and hang loading it. They were the
//! largest thing both working writers put in the file that this did not, and
//! vynull's reading of them is that a player has nothing to render its menu
//! from without them.

/// The column names, as rekordbox writes them: the categories a player offers
/// when you turn the encoder, and the lines it can print about a track. The
/// first value of each pair identifies the menu item to the player — it is the
/// item type of the players' own menu protocol, `0x80` and up — and is not
/// simply the row number: the gaps are rekordbox's.
pub(crate) const COLUMNS: [(u16, &str); 27] = [
    (0x0080, "GENRE"),
    (0x0081, "ARTIST"),
    (0x0082, "ALBUM"),
    (0x0083, "TRACK"),
    (0x0085, "BPM"),
    (0x0086, "RATING"),
    (0x0087, "YEAR"),
    (0x0088, "REMIXER"),
    (0x0089, "LABEL"),
    (0x008a, "ORIGINAL ARTIST"),
    (0x008b, "KEY"),
    (0x008d, "CUE"),
    (0x008e, "COLOR"),
    (0x0092, "TIME"),
    (0x0093, "BITRATE"),
    (0x0094, "FILE NAME"),
    (0x0084, "PLAYLIST"),
    (0x0098, "HOT CUE BANK"),
    (0x0095, "HISTORY"),
    (0x0091, "SEARCH"),
    (0x0096, "COMMENTS"),
    (0x008c, "DATE ADDED"),
    (0x0097, "DJ PLAY COUNT"),
    (0x0090, "FOLDER"),
    (0x00a1, "DEFAULT"),
    (0x00a2, "ALPHABET"),
    (0x00aa, "MATCHING"),
];

/// One entry of the browse menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Category {
    /// The row's own id — `category_id` in the OneLibrary database. The gaps
    /// are a real export's.
    pub id: u16,
    /// A position in [`COLUMNS`].
    pub column: usize,
    /// Which list the player opens: [`PLAIN`] for a list of names, or one of
    /// six codes for the categories that open something else.
    pub kind: u8,
    /// Where it sits in the menu, counted from one; 0 when hidden.
    pub position: u16,
    pub visible: bool,
    /// Set on MATCHING alone in both real exports, shown in one and hidden in
    /// the other, so it is not visibility. What it marks is not known; it is
    /// written as it was read.
    pub special: bool,
}

/// The kind of every category that opens a plain list of names. The six that
/// do not — TRACK, ARTIST, ALBUM, TIME, BPM and BITRATE — carry 1 to 6, which
/// is presumably which drill-down the player runs: a track list, an artist
/// tree, an album tree, and the three that offer ranges rather than names.
pub(crate) const PLAIN: u8 = 0x63;

const fn shown(id: u16, column: usize, kind: u8, position: u16) -> Category {
    Category { id, column, kind, position, visible: true, special: false }
}

const fn hidden(id: u16, column: usize, kind: u8) -> Category {
    Category { id, column, kind, position: 0, visible: false, special: false }
}

/// The browse categories, in id order.
pub(crate) const CATEGORIES: [Category; 22] = [
    hidden(1, 0, PLAIN),                                   // GENRE
    shown(2, 1, 2, 1),                                     // ARTIST
    shown(3, 2, 3, 2),                                     // ALBUM
    shown(4, 3, 1, 3),                                     // TRACK
    shown(5, 16, PLAIN, 5),                                // PLAYLIST
    hidden(6, 4, 5),                                       // BPM
    hidden(7, 5, PLAIN),                                   // RATING
    hidden(8, 6, PLAIN),                                   // YEAR
    hidden(9, 7, PLAIN),                                   // REMIXER
    hidden(10, 8, PLAIN),                                  // LABEL
    hidden(11, 9, PLAIN),                                  // ORIGINAL ARTIST
    shown(12, 10, PLAIN, 4),                               // KEY
    hidden(15, 12, PLAIN),                                 // COLOR
    shown(17, 23, PLAIN, 9),                               // FOLDER
    shown(18, 19, PLAIN, 7),                               // SEARCH
    hidden(19, 13, 4),                                     // TIME
    hidden(20, 14, 6),                                     // BITRATE
    hidden(21, 15, PLAIN),                                 // FILE NAME
    shown(22, 18, PLAIN, 6),                               // HISTORY
    hidden(23, 17, PLAIN),                                 // HOT CUE BANK
    Category { special: true, ..shown(26, 26, PLAIN, 8) }, // MATCHING
    shown(27, 21, PLAIN, 10),                              // DATE ADDED
];

/// One order a list can be sorted into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sort {
    /// The row's own id — `sort_id` in the OneLibrary database.
    pub id: u16,
    /// A position in [`COLUMNS`].
    pub column: usize,
    /// Where it sits in the sort list, counted from one; 0 when hidden.
    pub position: u8,
    pub visible: bool,
    /// Whether the player also prints it as the second line of each browse
    /// row — `isSelectedAsSubColumn`. None of the defaults are.
    pub sub_column: bool,
}

const fn sort(id: u16, column: usize, position: u8) -> Sort {
    Sort { id, column, position, visible: position != 0, sub_column: false }
}

/// The sort orders, in id order.
pub(crate) const SORTS: [Sort; 17] = [
    sort(0, 24, 1),  // DEFAULT
    sort(1, 25, 2),  // ALPHABET
    sort(2, 1, 3),   // ARTIST
    sort(3, 2, 4),   // ALBUM
    sort(4, 4, 5),   // BPM
    sort(5, 5, 6),   // RATING
    sort(6, 0, 0),   // GENRE
    sort(7, 20, 0),  // COMMENTS
    sort(8, 13, 0),  // TIME
    sort(9, 7, 0),   // REMIXER
    sort(10, 8, 0),  // LABEL
    sort(11, 9, 0),  // ORIGINAL ARTIST
    sort(12, 10, 7), // KEY
    sort(13, 14, 0), // BITRATE
    sort(15, 12, 0), // COLOR
    sort(16, 22, 0), // DJ PLAY COUNT
    sort(17, 21, 0), // DATE ADDED
];

/// The categories in the order rekordbox writes them into `export.pdb`: the
/// hidden ones first, by id, then the shown ones down the menu. Both real
/// exports are in this order and so is baken.
pub(crate) fn categories_as_written() -> Vec<&'static Category> {
    let mut rows: Vec<&Category> = CATEGORIES.iter().collect();
    rows.sort_by_key(|c| (c.visible, if c.visible { c.position } else { c.id }));
    rows
}

/// The sort orders as rekordbox writes them into `export.pdb`, by the same
/// rule as the categories.
pub(crate) fn sorts_as_written() -> Vec<&'static Sort> {
    let mut rows: Vec<&Sort> = SORTS.iter().collect();
    rows.sort_by_key(|s| (s.visible, if s.visible { u16::from(s.position) } else { s.id }));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_and_sort_order_names_a_column_that_exists() {
        for category in CATEGORIES {
            assert!(category.column < COLUMNS.len(), "{category:?}");
        }
        for sort in SORTS {
            assert!(sort.column < COLUMNS.len(), "{sort:?}");
        }
    }

    #[test]
    fn the_shown_categories_are_the_default_menu_in_its_order() {
        let menu: Vec<&str> = categories_as_written()
            .into_iter()
            .filter(|c| c.visible)
            .map(|c| COLUMNS[c.column].1)
            .collect();
        assert_eq!(
            menu,
            [
                "ARTIST",
                "ALBUM",
                "TRACK",
                "KEY",
                "PLAYLIST",
                "HISTORY",
                "SEARCH",
                "MATCHING",
                "FOLDER",
                "DATE ADDED"
            ]
        );
    }

    #[test]
    fn the_shown_sort_orders_are_the_default_list_in_its_order() {
        let list: Vec<&str> = sorts_as_written()
            .into_iter()
            .filter(|s| s.visible)
            .map(|s| COLUMNS[s.column].1)
            .collect();
        assert_eq!(list, ["DEFAULT", "ALPHABET", "ARTIST", "ALBUM", "BPM", "RATING", "KEY"]);
    }

    #[test]
    fn ids_are_unique_and_positions_run_from_one_without_gaps() {
        let mut ids: Vec<u16> = CATEGORIES.iter().map(|c| c.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), CATEGORIES.len());
        let mut positions: Vec<u16> =
            CATEGORIES.iter().filter(|c| c.visible).map(|c| c.position).collect();
        positions.sort_unstable();
        assert_eq!(positions, (1..=10).collect::<Vec<_>>());

        let mut ids: Vec<u16> = SORTS.iter().map(|s| s.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), SORTS.len());
        let mut positions: Vec<u8> =
            SORTS.iter().filter(|s| s.visible).map(|s| s.position).collect();
        positions.sort_unstable();
        assert_eq!(positions, (1..=7).collect::<Vec<_>>());
    }
}
