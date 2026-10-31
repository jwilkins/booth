//! `export.pdb` — the database a player reads to find anything at all.
//!
//! This is DeviceSQL, a store designed for embedded players with very little
//! memory: a file of fixed-size pages, each holding a heap of rows at the front
//! and an index growing backwards from the end. A table is a linked list of
//! pages. Strings are variable-length and are reached by an offset from the
//! start of the row that mentions them, which conveniently makes every row a
//! self-contained blob that can be placed anywhere in a page.
//!
//! Everything here is little-endian, unlike the analysis files, which are big.
//!
//! The format is reverse-engineered, not published. It is documented by Deep
//! Symmetry's [DJ Link Ecosystem Analysis] and by the Kaitai structures behind
//! `crate-digger`; where those left a field unexplained, the value written here
//! is the one a real rekordbox export was observed to use, and the comment says
//! so.
//!
//! [DJ Link Ecosystem Analysis]: https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use super::menu;

/// Page size. rekordbox uses 4 kB and the players expect to be told, so this
/// could vary — but there is no reason to differ.
const PAGE_LEN: usize = 4_096;
/// Every page starts with this much header before its heap.
const PAGE_HEADER_LEN: usize = 0x28;
/// A row index group covers sixteen rows in this many bytes.
const ROW_GROUP_LEN: usize = 0x24;
/// Rows per group in the index.
const ROWS_PER_GROUP: usize = 16;
/// The row count lives in a single byte for one of the two readers in the wild,
/// so a page never holds more rows than that byte can describe.
const MAX_ROWS_PER_PAGE: usize = 255;

/// The tables a rekordbox export contains, in the order it writes them. Most
/// are empty in anything we produce, but the header lists all twenty and a
/// player expects to find them.
///
/// Four never vary from one drive to the next: the colours, and the three the
/// browse menu is made of — its column names, the categories it offers and the
/// orders it sorts by. The last two were written empty here until a CDJ-3000
/// hung loading a drive; see [`super::menu`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Table {
    Tracks = 0,
    Genres = 1,
    Artists = 2,
    Albums = 3,
    Labels = 4,
    Keys = 5,
    Colors = 6,
    PlaylistTree = 7,
    PlaylistEntries = 8,
    Unknown9 = 9,
    Unknown10 = 10,
    HistoryPlaylists = 11,
    HistoryEntries = 12,
    Artwork = 13,
    Unknown14 = 14,
    Unknown15 = 15,
    Columns = 16,
    Categories = 17,
    Sorts = 18,
    History = 19,
}

impl Table {
    const ALL: [Table; 20] = [
        Table::Tracks,
        Table::Genres,
        Table::Artists,
        Table::Albums,
        Table::Labels,
        Table::Keys,
        Table::Colors,
        Table::PlaylistTree,
        Table::PlaylistEntries,
        Table::Unknown9,
        Table::Unknown10,
        Table::HistoryPlaylists,
        Table::HistoryEntries,
        Table::Artwork,
        Table::Unknown14,
        Table::Unknown15,
        Table::Columns,
        Table::Categories,
        Table::Sorts,
        Table::History,
    ];
}

/// One track, as the database describes it.
///
/// The identifiers a player uses to link a track to its artist, album and so on
/// are assigned by [`Database`]; this is the human-facing version, and names are
/// interned into the right tables on the way out.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    /// The id a player reports when this track is loaded. Must be non-zero and
    /// unique within the drive.
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub label: String,
    /// Musical key, as it should read on the player — "8A", "Fm", whatever
    /// convention the library uses.
    pub key: String,
    pub comment: String,
    /// Where the audio sits on the drive, e.g. `/Contents/Artist/track.flac`.
    pub file_path: String,
    /// Where the analysis file sits, e.g.
    /// `/PIONEER/USBANLZ/P001/00000001/ANLZ0000.DAT`. The player finds the
    /// `.EXT` and `.2EX` alongside it.
    pub analyze_path: String,
    /// Tempo in BPM times 100.
    pub tempo_x100: u32,
    pub duration_secs: u16,
    pub sample_rate: u32,
    pub sample_depth: u16,
    pub bitrate: u32,
    pub file_size: u32,
    pub track_number: u32,
    pub disc_number: u16,
    pub year: u16,
    /// Stars, 0 to 5.
    pub rating: u8,
    /// A row in the colours table, 1 to 8, or 0 for none.
    pub color_id: u8,
    pub play_count: u16,
    /// `YYYY-MM-DD`.
    pub date_added: String,
    /// `YYYY-MM-DD`.
    pub analyze_date: String,
}

/// A playlist, or a folder of playlists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub id: u32,
    /// The folder this sits in, or 0 for the root.
    pub parent_id: u32,
    /// Where it sorts among its siblings.
    pub sort_order: u32,
    pub name: String,
    pub is_folder: bool,
    /// The tracks, in play order. Empty for a folder.
    pub track_ids: Vec<u32>,
}

impl Playlist {
    pub fn new(id: u32, name: &str, track_ids: Vec<u32>) -> Self {
        Self {
            id,
            parent_id: 0,
            sort_order: id,
            name: name.to_string(),
            is_folder: false,
            track_ids,
        }
    }

    pub fn folder(id: u32, name: &str) -> Self {
        Self {
            id,
            parent_id: 0,
            sort_order: id,
            name: name.to_string(),
            is_folder: true,
            track_ids: Vec::new(),
        }
    }

    pub fn in_folder(mut self, parent_id: u32) -> Self {
        self.parent_id = parent_id;
        self
    }
}

/// One list a player kept of what it played off the drive.
///
/// A player writes these, not this program: a drive goes out with none, and a
/// CDJ adds one, numbered, each time it mounts it. They can be written here so
/// that a drive can be made the way a player leaves one, which is what reading
/// a history back is tested against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryPlaylist {
    pub id: u32,
    /// `HISTORY 001` and so on, as a player names them.
    pub name: String,
    /// The tracks, in the order they were loaded.
    pub track_ids: Vec<u32>,
}

/// The eight track colours rekordbox offers, in the order it numbers them. A
/// track's `color_id` is a row in this table, so the table has to be there even
/// when nothing is coloured.
const COLORS: [&str; 8] = ["Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"];

/// Everything that goes on a drive.
#[derive(Clone, Debug, Default)]
pub struct Database {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    /// The drive's own name, for the history row. Empty is valid — baken's
    /// fixture row has it empty — so nothing here needs it set.
    pub device_name: String,
    /// What a player recorded playing. Empty on every drive this program
    /// writes, so writing one starts its history over; see [`HistoryPlaylist`].
    pub history: Vec<HistoryPlaylist>,
}

impl Database {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialise the whole database.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.check()?;

        // Names become rows in their own tables, and tracks refer to them by
        // id. Interning is per-table and the ids start at one, because zero
        // means "not set" in every field that points at one of these.
        let mut artists = Interner::new();
        let mut albums = Interner::new();
        let mut genres = Interner::new();
        let mut labels = Interner::new();
        let mut keys = Interner::new();

        // Two of a track's fields are dates, and a real export leaves neither
        // empty: the day the track joined the collection and the day it was
        // analysed. The collection does not keep the first, and the second is
        // the day this runs — the analysis is made alongside the row — so a
        // track that arrives without either is dated the day it is written.
        let written_on = crate::clock::day(crate::clock::now());
        let mut track_rows = Vec::new();
        for track in &self.tracks {
            let artist_id = artists.intern(&track.artist);
            let album_id = albums.intern(&track.album);
            track_rows.push(RowData {
                bytes: track_row(
                    track,
                    artist_id,
                    album_id,
                    genres.intern(&track.genre),
                    labels.intern(&track.label),
                    keys.intern(&track.key),
                    &written_on,
                ),
                index_shift_at: Some(2),
            });
        }

        let mut rows: BTreeMap<u8, Vec<RowData>> = BTreeMap::new();
        rows.insert(Table::Tracks as u8, track_rows);
        rows.insert(
            Table::Artists as u8,
            artists.rows(|id, name| named_row(0x0060, id, name, 0x0a)),
        );
        rows.insert(Table::Albums as u8, albums.rows(album_row));
        rows.insert(Table::Genres as u8, genres.rows(id_and_name_row));
        rows.insert(Table::Labels as u8, labels.rows(id_and_name_row));
        rows.insert(Table::Keys as u8, keys.rows(key_row));
        rows.insert(
            Table::Colors as u8,
            COLORS
                .iter()
                .enumerate()
                .map(|(i, name)| RowData::plain(color_row(i as u16 + 1, name)))
                .collect(),
        );
        rows.insert(
            Table::Columns as u8,
            menu::COLUMNS
                .iter()
                .enumerate()
                .map(|(i, (code, name))| RowData::plain(column_row(i as u16 + 1, *code, name)))
                .collect(),
        );
        // The menu's other two tables, in rekordbox's own row order. Both real
        // exports and both working writers carry exactly these rows; every
        // drive this wrote before had the two tables empty.
        rows.insert(
            Table::Categories as u8,
            menu::categories_as_written()
                .into_iter()
                .map(|c| RowData::plain(category_row(c)))
                .collect(),
        );
        rows.insert(
            Table::Sorts as u8,
            menu::sorts_as_written().into_iter().map(|s| RowData::plain(sort_row(s))).collect(),
        );

        // One row, as a real export has. Dated at write time: the field is the
        // day the stick was made and nothing reads it back.
        rows.insert(
            Table::History as u8,
            vec![RowData::plain(history_row(
                self.tracks.len() as u32,
                &crate::clock::day(crate::clock::now()),
                &self.device_name,
            ))],
        );

        let mut tree = Vec::new();
        let mut entries = Vec::new();
        for playlist in &self.playlists {
            tree.push(RowData::plain(playlist_tree_row(playlist)));
            for (position, track_id) in playlist.track_ids.iter().enumerate() {
                entries.push(RowData::plain(playlist_entry_row(
                    position as u32 + 1,
                    *track_id,
                    playlist.id,
                )));
            }
        }
        rows.insert(Table::PlaylistTree as u8, tree);
        rows.insert(Table::PlaylistEntries as u8, entries);

        let mut histories = Vec::new();
        let mut played = Vec::new();
        for history in &self.history {
            histories.push(id_and_name_row(history.id, &history.name));
            for (position, track_id) in history.track_ids.iter().enumerate() {
                played.push(RowData::plain(history_entry_row(
                    *track_id,
                    history.id,
                    position as u32 + 1,
                )));
            }
        }
        rows.insert(Table::HistoryPlaylists as u8, histories);
        rows.insert(Table::HistoryEntries as u8, played);

        self.lay_out(&rows)
    }

    /// Refuse to write something a player would choke on, while there is still
    /// a name to put in the message.
    fn check(&self) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for track in &self.tracks {
            if track.id == 0 {
                bail!("track {:?} has id 0, which a player reads as \"no track\"", track.title);
            }
            if !seen.insert(track.id) {
                bail!("two tracks share id {} ({:?})", track.id, track.title);
            }
            if track.file_path.is_empty() {
                bail!("track {:?} has no path on the drive", track.title);
            }
        }
        let known: std::collections::BTreeSet<u32> = seen;
        let mut playlist_ids = std::collections::BTreeSet::new();
        for playlist in &self.playlists {
            if playlist.id == 0 {
                bail!("playlist {:?} has id 0", playlist.name);
            }
            if !playlist_ids.insert(playlist.id) {
                bail!("two playlists share id {} ({:?})", playlist.id, playlist.name);
            }
            for track_id in &playlist.track_ids {
                if !known.contains(track_id) {
                    bail!(
                        "playlist {:?} refers to track {track_id}, which is not on the drive",
                        playlist.name
                    );
                }
            }
        }
        Ok(())
    }

    /// Place every table's rows into pages and write the file.
    ///
    /// Each table gets a leading page with no rows in it — rekordbox writes one
    /// and the players expect the table's first page to be that rather than
    /// data — followed by as many data pages as the rows need. A table with no
    /// rows still gets its pair of pages; the second is left as zeroes and the
    /// table's `last_page` points back at the first, which is what a real
    /// export does.
    fn lay_out(&self, rows: &BTreeMap<u8, Vec<RowData>>) -> Result<Vec<u8>> {
        let empty = Vec::new();
        let packed: Vec<(Table, Vec<Vec<&RowData>>)> = Table::ALL
            .iter()
            .map(|table| Ok((*table, pack(rows.get(&(*table as u8)).unwrap_or(&empty))?)))
            .collect::<Result<_>>()?;

        // Every table costs its header page, its data pages, and one more: an
        // all-zero spare at the end of the file that is its "empty candidate",
        // the page the file header names as where the table would grow into.
        // So the page count is known before anything is written.
        //
        // The spare is what this got wrong for five rounds against a
        // CDJ-3000. Every table named the same candidate, and the one it named
        // was `next_unused` — a page that did not exist. In both real exports
        // and in baken (which plays on a CDJ-3000 on 3.22) every table has its
        // own, every one is below `next_unused`, a populated table's last data
        // page points `next` at it, and an empty table's header does. A player
        // reads this directory before it reads any table, so twenty tables
        // sharing a page past the end of the file is refused as no database at
        // all, while a parser that never follows the pointer sees nothing wrong.
        let table_count = Table::ALL.len() as u32;
        let data_pages: u32 = packed.iter().map(|(_, groups)| groups.len() as u32).sum();
        let first_spare = 1 + table_count + data_pages;
        let past_end = first_spare + table_count;

        let mut pages: Vec<Vec<u8>> = Vec::with_capacity(past_end as usize);
        let mut table_refs = Vec::new();
        let mut next_index = 1u32;

        for (i, (table, groups)) in packed.iter().enumerate() {
            let empty = first_spare + i as u32;
            let first_page = next_index;

            if groups.is_empty() {
                // Nothing to point at but the spare. The header stays the
                // table's last page, and its first-data word says there is
                // none, which is what a real export writes for an empty table
                // — not the index after it, which this wrote until the fixture
                // was read for exactly this case.
                pages.push(header_page(*table, first_page, empty, None));
                next_index += 1;
                table_refs.push((*table, empty, first_page, first_page));
                continue;
            }

            pages.push(header_page(*table, first_page, first_page + 1, Some(first_page + 1)));
            next_index += 1;

            let mut so_far = 0usize;
            for (k, group) in groups.iter().enumerate() {
                let last = k + 1 == groups.len();
                let next = if last { empty } else { next_index + 1 };
                so_far += group.len();
                pages.push(data_page(*table, next_index, next, so_far, group));
                next_index += 1;
            }
            table_refs.push((*table, empty, first_page, next_index - 1));
        }
        debug_assert_eq!(next_index, first_spare);

        // The spares. All zeros — which both real exports and baken write, and
        // which an earlier comment here said a player would refuse. It does
        // not; what it refused was the directory above.
        for _ in 0..table_count {
            pages.push(vec![0u8; PAGE_LEN]);
        }

        let mut header = Vec::with_capacity(PAGE_LEN);
        put_u32(&mut header, 0);
        put_u32(&mut header, PAGE_LEN as u32);
        put_u32(&mut header, Table::ALL.len() as u32);
        put_u32(&mut header, past_end); // next unused page
                                        // Five, in every export anyone has looked at, and nobody knows what of.
        put_u32(&mut header, 5);
        // Higher than any page's own sequence, which is the rule and not a
        // formality: a file whose header does not lead its pages is the first
        // way a growing library was found to stop being read at all.
        let highest = pages
            .iter()
            .map(|page| u32::from_le_bytes(page[0x10..0x14].try_into().unwrap_or_default()))
            .max()
            .unwrap_or(0);
        put_u32(&mut header, highest + 1);
        put_u32(&mut header, 0);
        for (table, empty, first, last) in &table_refs {
            put_u32(&mut header, *table as u32);
            put_u32(&mut header, *empty);
            put_u32(&mut header, *first);
            put_u32(&mut header, *last);
        }
        header.resize(PAGE_LEN, 0);

        Ok(std::iter::once(header).chain(pages).flatten().collect())
    }
}

/// A row, and where in it the packer must write the position it ends up at.
struct RowData {
    bytes: Vec<u8>,
    /// Offset of the `index_shift` field, for the row types that carry one.
    index_shift_at: Option<usize>,
}

impl RowData {
    fn plain(bytes: Vec<u8>) -> Self {
        Self { bytes, index_shift_at: None }
    }
}

/// Names to ids, in first-seen order. Empty names are not interned: a track with
/// no album refers to album 0, which every reader takes as "none".
struct Interner {
    ids: BTreeMap<String, u32>,
    order: Vec<String>,
}

impl Interner {
    fn new() -> Self {
        Self { ids: BTreeMap::new(), order: Vec::new() }
    }

    fn intern(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = self.order.len() as u32 + 1;
        self.ids.insert(name.to_string(), id);
        self.order.push(name.to_string());
        id
    }

    fn rows(&self, encode: impl Fn(u32, &str) -> RowData) -> Vec<RowData> {
        self.order.iter().enumerate().map(|(i, name)| encode(i as u32 + 1, name)).collect()
    }
}

// -- pages -----------------------------------------------------------------

/// Split rows into page-sized groups.
///
/// Fails on a row that will not fit a page at all. Nothing should reach here
/// that does — [`track_row`] shortens the one field that can grow without
/// bound, and [`Database::check`] rejects the rest — but the alternative to
/// failing is a page that claims more free space than it has and gets cut back
/// to 4 kB with a row sliced through the middle, which is a database a player
/// may refuse or crash on rather than one it reads badly.
fn pack(rows: &[RowData]) -> Result<Vec<Vec<&RowData>>> {
    let mut pages = Vec::new();
    let mut current: Vec<&RowData> = Vec::new();
    let mut used = 0usize;

    for row in rows {
        if aligned(row.bytes.len()) > MAX_ROW_LEN {
            bail!("a {}-byte row will not fit a {PAGE_LEN}-byte page", row.bytes.len());
        }
        let size = aligned(row.bytes.len());
        let next_count = current.len() + 1;
        let index_len = index_size(next_count);
        if !current.is_empty()
            && (used + size + index_len > PAGE_LEN - PAGE_HEADER_LEN
                || next_count > MAX_ROWS_PER_PAGE)
        {
            pages.push(std::mem::take(&mut current));
            used = 0;
        }
        used += size;
        current.push(row);
    }
    if !current.is_empty() {
        pages.push(current);
    }
    Ok(pages)
}

fn aligned(len: usize) -> usize {
    len.div_ceil(4) * 4
}

/// Bytes the row index takes at the end of a page: two per row, plus four for
/// the presence and transaction flags of each group of sixteen.
fn index_size(rows: usize) -> usize {
    let groups = rows.div_ceil(ROWS_PER_GROUP);
    rows * 2 + groups * 4
}

/// Where a table's page sequence numbers start.
///
/// The sequence is a transaction counter of some kind: rekordbox writes
/// `base + (rows so far - 1) * 5` on each data page, and the file's own header
/// carries a number higher than any of them. The bases below were recovered by
/// bisecting what a CDJ-3000 would accept; the tables not listed have never
/// been reported, and take 1, which is what this program wrote for everything
/// before any of this was known.
fn sequence_base(table: Table) -> u32 {
    match table {
        Table::Tracks => 10,
        Table::Artists => 7,
        Table::Albums => 9,
        Table::PlaylistTree => 6,
        Table::PlaylistEntries => 11,
        // Not a formula at all. The four fixed lists are written once, when
        // rekordbox creates the file, and their pages carry the number of the
        // transaction that did it: 2, 3, 4 and 5 in both real exports, colours
        // first. The colours said 8 here, from the bisection above, until the
        // two files were read.
        Table::Colors => 2,
        Table::Columns => 3,
        Table::Categories => 4,
        Table::Sorts => 5,
        _ => 1,
    }
}

/// The row-less page every table starts with.
///
/// Marked 0x64, with both row-count fields saturated and a constant nobody has
/// explained in the last field but one. Two tables — the tracks and the
/// history — carry a count of their own in a body that is otherwise the
/// free-slot fill: the reason is unknown, and a player that does not find them
/// there refuses the whole database rather than the table.
fn header_page(table: Table, index: u32, next: u32, first_data: Option<u32>) -> Vec<u8> {
    let special = matches!(table, Table::Tracks | Table::History);
    let mut page = page_header(Fields {
        table,
        index,
        next,
        // The fixed lists' header pages say 1 in both real exports, whatever
        // their data pages say; the rest take their table's base, as before.
        sequence: match is_fixed_list(table) {
            true => 1,
            false => sequence_base(table),
        },
        rows: 0,
        free: 0,
        used: 0,
        flags: 0x64,
        unknown5: 0x1fff,
        rows_large: 0x1fff,
        unknown6: 1004,
        unknown7: u16::from(special),
    });
    page.resize(PAGE_LEN, 0);

    // The body, which was written as zeroes and is where a player goes looking
    // for the table's rows.
    //
    // Both real exports and baken write the same five words here: this page's
    // own index, the index of the table's first data page, `0x03ffffff`, a
    // zero, and the marker. The second of those is the one that matters —
    // without it a player has a header page that parses and no way to reach
    // what it leads to, which is a drive that browses as an empty library.
    // baken's own comment on the equivalent code says exactly that: "Without
    // this content the CDJ accepts the sentinel page but can't locate the data
    // pages it leads to."
    //
    // This was one of the candidates for why a CDJ-3000 on official firmware
    // 3.22 found no database on a drive a CDJ-3000X read perfectly. It was not
    // the fault — the file header's directory was, see `lay_out` — but it
    // stays, because it is what every working file has.
    let mut word = |at: usize, v: u32| page[at..at + 4].copy_from_slice(&v.to_le_bytes());
    word(0x28, index);
    // The first data page, or NONE for a table with no rows — where `next`
    // above still points somewhere, at the table's empty spare.
    word(0x2c, first_data.unwrap_or(NONE_PAGE));
    word(0x30, NONE_PAGE);
    word(0x34, 0);
    word(0x38, 0x1fff_0000 | u32::from(special));

    // And then the free-slot fill: 1004 words of it, which is 0x3c up to the
    // twenty zero bytes every header page ends with. The two tables that carry
    // a count put it in the first slot and the fill runs on after it. This
    // left the rest of their pages as zeroes, having read the real one only as
    // far as the count.
    for at in (0x3c..PAGE_LEN - 20).step_by(4) {
        word(at, FREE_SLOT);
    }
    match table {
        Table::Tracks => word(0x3c, 0x10),
        Table::History => word(0x3c, 0x140),
        _ => {}
    }
    page
}

/// What a page index is when it points at nothing.
const NONE_PAGE: u32 = 0x03ff_ffff;

/// The word a header page's unused heap is filled with, 1004 times.
const FREE_SLOT: u32 = 0x1fff_fff8;

/// A page of rows, with its index built backwards from the end.
///
/// `so_far` is how many rows of this table have been written including these,
/// because the sequence number counts the table's rows rather than the page's.
fn data_page(table: Table, index: u32, next: u32, so_far: usize, rows: &[&RowData]) -> Vec<u8> {
    let mut heap = Vec::new();
    let mut offsets = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.iter().enumerate() {
        offsets.push(heap.len() as u16);
        let mut bytes = row.bytes.clone();
        if let Some(at) = row.index_shift_at {
            // Rows that carry one record their position in the page index,
            // counted in steps of 0x20 rather than in rows.
            bytes[at..at + 2].copy_from_slice(&((row_index as u16) * 0x20).to_le_bytes());
        }
        heap.extend_from_slice(&bytes);
        heap.resize(aligned(heap.len()), 0);
    }

    let used = heap.len();
    let free = PAGE_LEN - PAGE_HEADER_LEN - used - index_size(rows.len());
    // The fixed lists describe their own pages differently, and getting it the
    // ordinary way round is one of the things a player refuses the whole
    // database over: they put the row count in `unknown5` and leave
    // `num_rows_large` at zero, where every other table says 1 and
    // `rows - 1`. The browse menu was the known one; the colours were found
    // the same way in both real exports, and then the categories and sort
    // orders when this started writing them.
    let fixed = is_fixed_list(table);
    let mut page = page_header(Fields {
        table,
        index,
        next,
        sequence: match fixed {
            true => sequence_base(table),
            false => sequence_base(table) + (so_far.max(1) as u32 - 1) * 5,
        },
        rows: rows.len(),
        free,
        used,
        flags: 0x24, // an ordinary data page with nothing deleted from it
        unknown5: match fixed {
            true => rows.len() as u16,
            false => 1,
        },
        rows_large: match fixed {
            true => 0,
            false => rows.len().max(1) as u16 - 1,
        },
        unknown6: 0,
        unknown7: 0,
    });
    page.extend_from_slice(&heap);
    page.resize(PAGE_LEN, 0);

    for (row_index, offset) in offsets.iter().enumerate() {
        let group = row_index / ROWS_PER_GROUP;
        let base = PAGE_LEN - group * ROW_GROUP_LEN;
        let at = base - (6 + 2 * (row_index % ROWS_PER_GROUP));
        page[at..at + 2].copy_from_slice(&offset.to_le_bytes());
    }
    for group in 0..rows.len().div_ceil(ROWS_PER_GROUP) {
        let in_group = (rows.len() - group * ROWS_PER_GROUP).min(ROWS_PER_GROUP);
        let present = if in_group == 16 { u16::MAX } else { (1u16 << in_group) - 1 };
        let base = PAGE_LEN - group * ROW_GROUP_LEN;
        page[base - 4..base - 2].copy_from_slice(&present.to_le_bytes());
        page[base - 2..base].copy_from_slice(&present.to_le_bytes());
    }
    page
}

/// The tables that are the same on every drive — the colours, and the three the
/// browse menu is made of — whose pages rekordbox describes differently from
/// every other table's: the row count in `unknown5`, zero in `num_rows_large`
/// and a fixed sequence number. Measured on all four in both real exports.
fn is_fixed_list(table: Table) -> bool {
    matches!(table, Table::Colors | Table::Columns | Table::Categories | Table::Sorts)
}

/// Everything a page says about itself before its rows.
///
/// Spelled out one field at a time rather than derived, because a parser and a
/// player disagree about how much of this matters: a parser reads the row
/// count and ignores the rest, and a player refuses a drive over a byte it did
/// not expect. The values are `morizkraemer/fourfour`'s, found by bisecting
/// what a CDJ-3000 would accept.
struct Fields {
    table: Table,
    index: u32,
    next: u32,
    sequence: u32,
    rows: usize,
    free: usize,
    used: usize,
    flags: u8,
    unknown5: u16,
    rows_large: u16,
    unknown6: u16,
    unknown7: u16,
}

fn page_header(f: Fields) -> Vec<u8> {
    let mut page = Vec::with_capacity(PAGE_LEN);
    put_u32(&mut page, 0);
    put_u32(&mut page, f.index);
    put_u32(&mut page, f.table as u32);
    put_u32(&mut page, f.next);
    put_u32(&mut page, f.sequence);
    put_u32(&mut page, 0);
    page.push(f.rows.min(MAX_ROWS_PER_PAGE) as u8);
    // Whatever this is, it is the row count modulo eight in the top three
    // bits.
    page.push(((f.rows % 8) * 0x20) as u8);
    // The row count divided by eight, which is not the group count and was
    // written as one for a long time.
    //
    // Measured against the data pages of two real exports: `rows >> 3` is what
    // every page of both carries, and `rows.div_ceil(16)` disagreed with 158 of
    // the 720 pages of one of them. The browse menu is the evidence that the
    // old formula was wrong rather than merely different — it needed a
    // hand-added `+ 1` to be accepted, and `27 >> 3` is 3 where
    // `27.div_ceil(16)` is 2, so that `+ 1` was the residual of this bug on the
    // one table somebody had checked against hardware.
    //
    // Only the `0x24` pages of a real export are evidence here. A `0x34` page
    // is mid-transaction and its `0x18` and `0x19` disagree about how many rows
    // it has, so it says nothing about what a fresh write should look like.
    page.push((f.rows >> 3) as u8);
    page.push(f.flags);
    put_u16(&mut page, f.free as u16);
    put_u16(&mut page, f.used as u16);
    put_u16(&mut page, f.unknown5);
    put_u16(&mut page, f.rows_large);
    put_u16(&mut page, f.unknown6);
    put_u16(&mut page, f.unknown7);
    debug_assert_eq!(page.len(), PAGE_HEADER_LEN);
    page
}

// -- rows ------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A DeviceSQL string.
///
/// Short ASCII gets a single length byte, incremented, doubled and incremented
/// again — which is why an empty string is `0x03`. Anything longer, or anything
/// that is not ASCII, gets a four-byte header and its own encoding.
pub fn string(text: &str) -> Vec<u8> {
    if text.is_ascii() && text.len() < 127 {
        let mut out = Vec::with_capacity(text.len() + 1);
        out.push((((text.len() + 1) << 1) | 1) as u8);
        out.extend_from_slice(text.as_bytes());
        return out;
    }
    if text.is_ascii() {
        let mut out = vec![0x40];
        put_u16(&mut out, (text.len() + 4) as u16);
        out.push(0);
        out.extend_from_slice(text.as_bytes());
        return out;
    }
    let encoded: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = vec![0x90];
    put_u16(&mut out, (encoded.len() + 4) as u16);
    out.push(0);
    out.extend_from_slice(&encoded);
    out
}

/// The one row the history table (0x13) carries in a real export.
///
/// `0x0280`, the track count, a zero, the export date, `0x19 0x1e`, the string
/// `"1000"`, and the device name, zero-padded to forty bytes. The count is
/// what a CDJ-3000 shows as Songs.
///
/// Every byte of that is from baken's `pdb::rows::history_property`
/// (<https://github.com/M-Igashi/baken>, MIT, Copyright 2025 Masanari Higashi),
/// which read it off a real rekordbox 7 export and whose sticks play on a
/// CDJ-3000 on firmware 3.22. Reimplemented here against Booth's own string
/// encoder rather than copied, and pinned to baken's own fixture bytes in
/// `the_history_row_is_the_forty_bytes_a_real_export_carries`.
///
/// vynull (<https://github.com/vynulldev/vynull>, GPL-3.0 — read for what it
/// knows about the format, no code taken) writes the same row and agrees on
/// every field: `0x0280`, a `0x20`-per-slot index shift sharing those four
/// bytes, the count at 0x04, a zero word at 0x08, then the strings inline. It
/// stops at thirty-two bytes where baken pads to forty, and forty is right:
/// a real export's history pages report `heap_used` 40 for one row and 480 for
/// twelve. vynull also records that a count of zero makes a deck treat the
/// stick as "no real export" and suppress features, which is a reason to send
/// the real number rather than a placeholder.
///
/// Why it matters: this table was written empty, and an empty history table is
/// the last item on fourfour's list of what a CDJ-3000 checks before accepting
/// a database. A drive this program wrote was refused outright by a CDJ-3000
/// while a CDJ-3000X read the same stick, which is the shape of a fault in the
/// legacy database rather than in the analysis beside it.
fn history_row(track_count: u32, export_date: &str, device_name: &str) -> Vec<u8> {
    let mut row = Vec::new();
    put_u32(&mut row, 0x0280);
    put_u32(&mut row, track_count);
    put_u32(&mut row, 0);
    row.extend_from_slice(&string(export_date));
    row.extend_from_slice(&[0x19, 0x1e]);
    row.extend_from_slice(&string("1000"));

    // rekordbox packs an ASCII string where it falls and starts a UTF-16 one
    // on a four-byte boundary of the row. A volume label is the one field here
    // that can be either, and baken records a CDJ-2000NXS2 freezing on a stick
    // whose strings were unaligned.
    let name = string(device_name);
    if name.first() == Some(&0x90) {
        row.resize(aligned(row.len()), 0);
    }
    row.extend_from_slice(&name);

    if row.len() < HISTORY_ROW_LEN {
        row.resize(HISTORY_ROW_LEN, 0);
    }
    row
}

/// What a history row is padded out to, whatever its strings came to.
const HISTORY_ROW_LEN: usize = 40;

/// Artist rows, and anything else shaped like them: a subtype, an id, and a
/// one-byte offset to the name.
fn named_row(subtype: u16, id: u32, name: &str, fixed_len: u8) -> RowData {
    let mut row = Vec::new();
    put_u16(&mut row, subtype);
    put_u16(&mut row, 0); // index_shift, filled in when the row is placed
    put_u32(&mut row, id);
    row.push(0x03); // an empty string nothing points at
    row.push(fixed_len);
    row.extend_from_slice(&string(name));
    RowData { bytes: row, index_shift_at: Some(2) }
}

fn album_row(id: u32, name: &str) -> RowData {
    let mut row = Vec::new();
    put_u16(&mut row, 0x0080);
    put_u16(&mut row, 0); // index_shift
    put_u32(&mut row, 0);
    put_u32(&mut row, 0); // artist_id: which artist an album belongs to is not modelled yet
    put_u32(&mut row, id);
    put_u32(&mut row, 0);
    row.push(0x03);
    row.push(0x16);
    row.extend_from_slice(&string(name));
    RowData { bytes: row, index_shift_at: Some(2) }
}

fn id_and_name_row(id: u32, name: &str) -> RowData {
    let mut row = Vec::new();
    put_u32(&mut row, id);
    row.extend_from_slice(&string(name));
    RowData::plain(row)
}

fn key_row(id: u32, name: &str) -> RowData {
    let mut row = Vec::new();
    put_u32(&mut row, id);
    put_u32(&mut row, id); // the id, again, for reasons nobody has established
    row.extend_from_slice(&string(name));
    RowData::plain(row)
}

fn color_row(id: u16, name: &str) -> Vec<u8> {
    let mut row = vec![0, 0, 0, 0, id as u8];
    put_u16(&mut row, id);
    row.push(0);
    row.extend_from_slice(&string(name));
    row
}

fn column_row(id: u16, menu: u16, name: &str) -> Vec<u8> {
    let mut row = Vec::new();
    put_u16(&mut row, id);
    put_u16(&mut row, menu);
    // The name is wrapped in two private-use characters, which is how rekordbox
    // marks a label the player should translate rather than display verbatim.
    row.extend_from_slice(&string(&format!("\u{fffa}{name}\u{fffb}")));
    row
}

fn playlist_tree_row(playlist: &Playlist) -> Vec<u8> {
    let mut row = Vec::new();
    put_u32(&mut row, playlist.parent_id);
    put_u32(&mut row, 0);
    put_u32(&mut row, playlist.sort_order);
    put_u32(&mut row, playlist.id);
    put_u32(&mut row, u32::from(playlist.is_folder));
    row.extend_from_slice(&string(&playlist.name));
    row
}

fn playlist_entry_row(position: u32, track_id: u32, playlist_id: u32) -> Vec<u8> {
    let mut row = Vec::new();
    put_u32(&mut row, position);
    put_u32(&mut row, track_id);
    put_u32(&mut row, playlist_id);
    row
}

/// A playlist entry's three numbers, with the position moved to the end.
fn history_entry_row(track_id: u32, history_id: u32, position: u32) -> Vec<u8> {
    let mut row = Vec::new();
    put_u32(&mut row, track_id);
    put_u32(&mut row, history_id);
    put_u32(&mut row, position);
    row
}

/// The most a row may take, which is a page with one row in it.
///
/// A row cannot span pages: the page header counts its rows and its free space
/// in sixteen bits, and the index at the end of the page points at each row's
/// start. So a row that does not fit a page cannot be written at all — and
/// until this existed, one was: the page writer worked out its free space as
/// `PAGE_LEN - header - used`, which underflowed, and what went on the stick
/// was a page truncated back to 4 kB with a row cut in half inside it. Debug
/// builds panicked on the subtraction; release builds wrote the corrupt page.
///
/// Rounded down to the four-byte alignment rows are placed on, which is not
/// cosmetic: 4050 bytes of row occupy 4052 bytes of page, so the unrounded
/// limit admitted two lengths — 4049 and 4050 — that [`pack`] then refused,
/// and a drive failed to write on an ASCII comment of exactly 3846 bytes. The
/// two checks now ask the same question, because a limit that is a multiple of
/// four makes `len <= MAX_ROW_LEN` and `aligned(len) <= MAX_ROW_LEN` the same
/// test.
pub(crate) const MAX_ROW_LEN: usize = (PAGE_LEN - PAGE_HEADER_LEN - ROW_INDEX_LEN) / 4 * 4;

/// What the index at the end of a page costs for a single row: two bytes for
/// its offset, four for its group's flags.
const ROW_INDEX_LEN: usize = 6;

/// The number of strings a track row carries, whatever their contents.
const TRACK_STRINGS: usize = 21;
/// Fixed part of a track row: the numbers, then the table of string offsets.
const TRACK_FIXED_LEN: usize = 0x5e + TRACK_STRINGS * 2;

/// A track row, with the comment shortened if that is what it takes to fit.
///
/// The comment is the one field somebody can make arbitrarily long — it is
/// free text, and a lyric or a long note goes in it — so it is the one that
/// gives way. Refusing to write a whole drive because a note is long would be
/// a worse answer than putting a shorter note on the stick, and the
/// collection keeps the whole of it either way.
///
/// Everything else that can overflow a row is a path or a name, and those are
/// not ours to shorten: a truncated path is a track the player cannot open,
/// which is worse than being told the drive cannot be written. Those are
/// caught by [`Database::check`].
fn track_row(
    track: &Track,
    artist_id: u32,
    album_id: u32,
    genre_id: u32,
    label_id: u32,
    key_id: u32,
    written_on: &str,
) -> Vec<u8> {
    let mut row =
        track_row_exactly(track, artist_id, album_id, genre_id, label_id, key_id, written_on);
    // Cut, re-encode, and ask the row again, rather than working a budget out
    // once: the overflow is counted in encoded bytes and the cut is made in
    // UTF-8 ones, and the two are not the same length in either direction. A
    // Japanese comment is three UTF-8 bytes and two encoded bytes per
    // character, so taking the overflow off its UTF-8 length removes two
    // thirds of what it has to — a 2584-character comment left a 4485-byte
    // row, and a longer one left a longer row still, without bound.
    //
    // Each pass asks for the overflow back and a little more so that a cut
    // which buys less than it costs still converges, and the loop ends when
    // there is no comment left: a row that is still too big then is one the
    // comment was not making too big, and [`pack`] refuses it and says so.
    let mut keep = track.comment.len();
    let mut shorter = track.clone();
    while row.len() > MAX_ROW_LEN && keep > 0 {
        keep = keep.saturating_sub(row.len() - MAX_ROW_LEN + 8);
        shorter.comment = shorten(&track.comment, keep);
        keep = shorter.comment.len();
        row = track_row_exactly(
            &shorter, artist_id, album_id, genre_id, label_id, key_id, written_on,
        );
    }
    row
}

/// A string cut to at most `bytes`, on a character boundary.
///
/// Characters rather than bytes because the string encoding is UTF-16 for
/// anything that is not ASCII, and half a character is not a string.
fn shorten(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let mut end = bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn track_row_exactly(
    track: &Track,
    artist_id: u32,
    album_id: u32,
    genre_id: u32,
    label_id: u32,
    key_id: u32,
    written_on: &str,
) -> Vec<u8> {
    let mut row = Vec::with_capacity(TRACK_FIXED_LEN + 256);
    put_u16(&mut row, 0x0024);
    put_u16(&mut row, 0); // index_shift, filled in when the row is placed
    put_u32(&mut row, 0x000c_0700); // constant in every export seen
    put_u32(&mut row, track.sample_rate);
    put_u32(&mut row, 0); // composer
    put_u32(&mut row, track.file_size);
    // Unique to the row in every real export — a large number that looks like
    // rekordbox's own id for the track. vynull takes it for a lookup key and
    // writes the track id; baken writes the id plus twenty. Zero, which this
    // wrote, is the one value no working file has.
    put_u32(&mut row, track.id);
    put_u16(&mut row, 0xfa80); // two more constants nobody has explained
    put_u16(&mut row, 0x05e7);
    put_u32(&mut row, 0); // artwork
    put_u32(&mut row, key_id);
    put_u32(&mut row, 0); // original artist
    put_u32(&mut row, label_id);
    put_u32(&mut row, 0); // remixer
    put_u32(&mut row, track.bitrate);
    put_u32(&mut row, track.track_number);
    put_u32(&mut row, track.tempo_x100);
    put_u32(&mut row, genre_id);
    put_u32(&mut row, album_id);
    put_u32(&mut row, artist_id);
    put_u32(&mut row, track.id);
    put_u16(&mut row, track.disc_number);
    put_u16(&mut row, track.play_count);
    put_u16(&mut row, track.year);
    put_u16(&mut row, track.sample_depth);
    put_u16(&mut row, track.duration_secs);
    put_u16(&mut row, 41); // always 41
    row.push(track.color_id);
    row.push(track.rating.min(5));
    put_u16(&mut row, 1); // always 1
    put_u16(&mut row, 3); // 2 or 3, alternating, for reasons unknown
    debug_assert_eq!(row.len(), 0x5e);

    let filename = track.file_path.rsplit('/').next().unwrap_or_default();
    let date_added = match track.date_added.is_empty() {
        true => written_on,
        false => track.date_added.as_str(),
    };
    let analyze_date = match track.analyze_date.is_empty() {
        true => written_on,
        false => track.analyze_date.as_str(),
    };
    // The order is the format's, not ours. Six of these have no established
    // meaning; the values are what a real export was seen to hold.
    let strings: [Vec<u8>; TRACK_STRINGS] = [
        string(""), // ISRC
        string(""), // "texter"
        string("3"),
        string("3"),
        string(""),
        string(""),   // "message"
        string(""),   // shown on Kuvo when "ON"
        string("ON"), // auto-load hot cues
        string(""),
        string(""),
        string(date_added),
        string(""), // release date
        string(""), // mix name
        string(""),
        string(&track.analyze_path),
        string(analyze_date),
        string(&track.comment),
        string(&track.title),
        string(""),
        string(filename),
        string(&track.file_path),
    ];

    let mut offset = TRACK_FIXED_LEN;
    for text in &strings {
        put_u16(&mut row, offset as u16);
        offset += text.len();
    }
    debug_assert_eq!(row.len(), TRACK_FIXED_LEN);
    for text in &strings {
        row.extend_from_slice(text);
    }
    row
}

/// A row of the categories table: eight bytes, `(column, id, kind, flags,
/// position)`, where the flags byte is 1 for hidden and 2 for MATCHING's mark.
fn category_row(category: &menu::Category) -> Vec<u8> {
    let mut row = Vec::with_capacity(8);
    put_u16(&mut row, category.column as u16 + 1);
    put_u16(&mut row, category.id);
    row.push(category.kind);
    row.push(u8::from(!category.visible) | (u8::from(category.special) << 1));
    put_u16(&mut row, category.position);
    row
}

/// A row of the sort orders table: eight bytes, `(column, id, hidden, position,
/// sub-column)` — the fields the OneLibrary `sort` table has, a byte each for
/// the two flags and the position.
fn sort_row(sort: &menu::Sort) -> Vec<u8> {
    let mut row = Vec::with_capacity(8);
    put_u16(&mut row, sort.column as u16 + 1);
    put_u16(&mut row, sort.id);
    row.push(u8::from(!sort.visible));
    row.push(sort.position);
    put_u16(&mut row, u16::from(sort.sub_column));
    row
}

/// The bitrate a player prints for a lossless file, in kbps: the rate of its
/// samples. A real export's 16-bit 44.1 kHz stereo WAV says 1411 and its
/// 24-bit one 2116, both `rate × depth × channels ÷ 1000` rounded down; this
/// wrote 0 for every such file.
pub fn pcm_kbps(sample_rate: u32, sample_depth: u16, channels: u32) -> u32 {
    sample_rate * u32::from(sample_depth) * channels / 1000
}

/// The bitrate a player prints for a lossy file this did not make, in kbps:
/// its size over its length, which is the figure on the tin for a
/// constant-rate file and the average for a variable-rate one. A real export's
/// 320 kbps MP3 says 320, and its 6 899 624 bytes over 172 s come to 320.
pub fn average_kbps(file_size: u64, duration_secs: f64) -> u32 {
    if duration_secs <= 0.0 {
        return 0;
    }
    (file_size as f64 * 8.0 / duration_secs / 1000.0) as u32
}

/// What a reader found in one table of a database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    pub table: String,
    pub pages: usize,
    /// Rows the row index marks as present.
    pub rows: usize,
}

/// Walk a database the way a player does and report what is in it.
///
/// Written from the format documentation rather than from the writer above, and
/// sharing no code with it: it follows the page chain of each table from the
/// header, reads each page's row index, and counts what is actually marked
/// present. This is the database half of the drive verifier.
pub fn inspect(bytes: &[u8]) -> Result<Vec<TableInfo>> {
    let u32_at = |at: usize| -> Result<u32> {
        let end = at + 4;
        if end > bytes.len() {
            bail!("database is truncated at byte {at}");
        }
        Ok(u32::from_le_bytes(bytes[at..end].try_into()?))
    };
    let u16_at = |at: usize| -> Result<u16> {
        let end = at + 2;
        if end > bytes.len() {
            bail!("database is truncated at byte {at}");
        }
        Ok(u16::from_le_bytes(bytes[at..end].try_into()?))
    };

    let page_len = u32_at(4)? as usize;
    if page_len == 0 || bytes.len() % page_len != 0 {
        bail!("a page length of {page_len} does not divide a file of {} bytes", bytes.len());
    }
    let total_pages = bytes.len() / page_len;
    let num_tables = u32_at(8)? as usize;
    if num_tables == 0 || 28 + num_tables * 16 > page_len {
        bail!("{num_tables} tables will not fit in the header page");
    }

    let mut found = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let at = 28 + i * 16;
        let kind = u32_at(at)?;
        let first = u32_at(at + 8)?;
        let last = u32_at(at + 12)?;

        let mut rows = 0usize;
        let mut pages = 0usize;
        let mut index = first;
        loop {
            if index as usize >= total_pages {
                bail!("table {kind} points at page {index}, past the end of the file");
            }
            let page = index as usize * page_len;
            if u32_at(page + 4)? != index {
                bail!("page {index} says it is page {}", u32_at(page + 4)?);
            }
            pages += 1;
            let flags = bytes[page + 27];
            if flags & 0x40 == 0 {
                // A data page: walk its index and count the rows it marks
                // present, sixteen at a time from the end of the page backwards.
                let packed = u32_at(page + 24)? & 0x00ff_ffff;
                let offsets = (packed & 0x1fff) as usize;
                for group in 0..offsets.div_ceil(ROWS_PER_GROUP) {
                    let base = page + page_len - group * ROW_GROUP_LEN;
                    let present = u16_at(base - 4)?;
                    let in_group = (offsets - group * ROWS_PER_GROUP).min(ROWS_PER_GROUP);
                    rows += (0..in_group).filter(|r| present & (1 << r) != 0).count();
                }
            }
            if index == last {
                break;
            }
            let next = u32_at(page + 12)?;
            if next == index || pages > total_pages {
                bail!("the page chain of table {kind} loops at page {index}");
            }
            index = next;
        }

        let name = Table::ALL
            .iter()
            .find(|t| **t as u32 == kind)
            .map_or_else(|| format!("table {kind}"), |t| format!("{t:?}"));
        found.push(TableInfo { table: name, pages, rows });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_track(id: u32) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist: "Peverelist".to_string(),
            file_path: format!("/Contents/track{id}.flac"),
            tempo_x100: 12_800,
            ..Track::default()
        }
    }

    fn table(bytes: &[u8], want: Table) -> TableInfo {
        let name = format!("{want:?}");
        inspect(bytes).unwrap().into_iter().find(|t| t.table == name).unwrap()
    }

    /// Every page after the file header, as its own slice.
    fn pages(bytes: &[u8]) -> Vec<&[u8]> {
        bytes.chunks_exact(PAGE_LEN).skip(1).collect()
    }

    /// Every page is exactly one page long and says how much of itself is used.
    fn pages_are_sane(bytes: &[u8]) {
        assert_eq!(bytes.len() % PAGE_LEN, 0, "the file is not a whole number of pages");
        for (n, page) in pages(bytes).iter().enumerate() {
            let free = u16::from_le_bytes(page[0x1c..0x1e].try_into().unwrap()) as usize;
            let used = u16::from_le_bytes(page[0x1e..0x20].try_into().unwrap()) as usize;
            assert!(
                used + free <= PAGE_LEN,
                "page {n} claims {used} used and {free} free, which is more than a page"
            );
        }
    }

    #[test]
    fn a_comment_too_long_for_a_page_is_shortened_rather_than_cutting_the_row_in_half() {
        // The fault, and it reached a drive: a row is laid out inside one
        // 4 kB page, and the page writer worked out its free space as
        // `PAGE_LEN - header - used`. A comment of a few thousand characters
        // made `used` the larger, the subtraction went backwards, and what
        // was written was a page truncated back to 4 kB with a row cut
        // through the middle of it. Debug builds panicked; release builds
        // put it on the stick.
        for len in [100usize, 3_000, 3_900, 20_000, 200_000] {
            let mut track = a_track(1);
            track.comment = "x".repeat(len);
            let mut db = Database::new();
            db.tracks.push(track);

            let bytes = db.to_bytes().unwrap_or_else(|e| panic!("{len}-char comment: {e:#}"));
            pages_are_sane(&bytes);
            assert_eq!(table(&bytes, Table::Tracks).rows, 1, "the track went missing at {len}");
        }
    }

    #[test]
    fn a_comment_of_any_length_leaves_a_row_a_page_will_take() {
        // The fault a drive failed on: rows are placed on four-byte
        // boundaries, so a 4049-byte row costs a page 4052 bytes, but the
        // shortening compared the row against an unrounded 4050 and left it
        // alone — three bytes inside the limit it asked about and one past the
        // real one. An ASCII comment of 3846 bytes was all it took, and the
        // write failed on a stick with a page and a half of lyrics in it.
        for len in 3_700..4_000 {
            let mut track = a_track(1);
            track.comment = "x".repeat(len);
            let row = track_row(&track, 0, 0, 0, 0, 0, "2026-10-11");
            assert!(
                aligned(row.len()) <= MAX_ROW_LEN,
                "a {len}-byte comment left a {}-byte row, which costs {} bytes of page",
                row.len(),
                aligned(row.len())
            );
        }

        let mut track = a_track(1);
        track.comment = "x".repeat(3_846);
        let mut db = Database::new();
        db.tracks.push(track);
        let bytes = db.to_bytes().expect("the length the drive failed on");
        pages_are_sane(&bytes);
    }

    #[test]
    fn a_comment_that_costs_less_encoded_than_it_does_in_utf8_is_cut_until_it_fits() {
        // The second half of the same fault. Japanese is three UTF-8 bytes
        // and two encoded bytes per character, so taking the overflow off the
        // comment's UTF-8 length removes two thirds of what it has to: one
        // pass left a 4485-byte row at 2584 characters, and a longer comment
        // left a longer row, without bound.
        for chars in [1_923usize, 2_584, 4_000, 20_000] {
            let mut track = a_track(1);
            track.comment = "\u{3042}".repeat(chars);
            let row = track_row(&track, 0, 0, 0, 0, 0, "2026-10-11");
            assert!(
                aligned(row.len()) <= MAX_ROW_LEN,
                "{chars} characters left a {}-byte row",
                row.len()
            );

            let mut db = Database::new();
            db.tracks.push(track);
            let bytes = db.to_bytes().unwrap_or_else(|e| panic!("{chars} characters: {e:#}"));
            pages_are_sane(&bytes);
        }
    }

    #[test]
    fn a_comment_that_fits_is_left_alone() {
        // The shortening is a last resort, not a policy: a note somebody
        // typed should reach the player as they typed it.
        let mut track = a_track(1);
        track.comment = "peak time \u{2014} big room".to_string();
        let mut db = Database::new();
        db.tracks.push(track.clone());

        let bytes = db.to_bytes().unwrap();
        let text = String::from_utf16_lossy(
            &bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect::<Vec<_>>(),
        );
        assert!(
            text.contains(&track.comment)
                || String::from_utf8_lossy(&bytes).contains(&track.comment),
            "the comment did not survive"
        );
    }

    #[test]
    fn a_row_that_cannot_fit_a_page_is_refused_rather_than_written_broken() {
        // Everything but the comment is a path or a name, and those are not
        // ours to shorten — a truncated path is a track the player cannot
        // open. So the write fails and says why.
        let mut track = a_track(1);
        track.file_path = format!("/Contents/{}.flac", "d".repeat(5_000));
        let mut db = Database::new();
        db.tracks.push(track);

        let refused = db.to_bytes().expect_err("a 5 kB path should not be writable");
        assert!(
            refused.to_string().contains("will not fit"),
            "the error should say what is wrong: {refused}"
        );
    }

    #[test]
    fn the_file_header_leads_every_page_it_describes() {
        // A player stops reading a database whose header sequence does not
        // exceed its pages', and the way that fails is by growing: a small
        // library works, and one big enough to push a page's sequence past the
        // header's stops being seen at all.
        let mut database = Database::new();
        for id in 1..40u32 {
            database.tracks.push(Track {
                id,
                title: format!("Track {id}"),
                file_path: format!("/Contents/A/{id}.flac"),
                ..Track::default()
            });
        }
        database.playlists.push(Playlist::new(1, "All", (1..40).collect()));
        let bytes = database.to_bytes().unwrap();

        let header = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap());
        let highest = pages(&bytes)
            .iter()
            .map(|page| u32::from_le_bytes(page[0x10..0x14].try_into().unwrap()))
            .max()
            .unwrap();
        assert!(header > highest, "header sequence {header} does not lead {highest}");
        assert_eq!(u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap()), 5);
    }

    #[test]
    fn every_table_ends_in_an_all_zero_spare_and_every_other_page_is_a_page() {
        // This test used to claim the opposite — that a page written as zeroes
        // "is not one" and a player refuses the database over it. Both real
        // exports and baken write exactly that for every table's empty
        // candidate. What the player refused was the directory naming a page
        // past the end of the file for all twenty tables at once.
        let bytes = Database::new().to_bytes().unwrap();
        let all = pages(&bytes);
        let (real, spares) = all.split_at(all.len() - Table::ALL.len());
        for (n, page) in real.iter().enumerate() {
            let index = u32::from_le_bytes(page[0x04..0x08].try_into().unwrap());
            let flags = page[0x1b];
            assert_eq!(index as usize, n + 1, "page {n} does not know its own index");
            assert!(
                flags == 0x24 || flags == 0x64,
                "page {n} is neither a header page nor a data page: {flags:#04x}"
            );
        }
        assert_eq!(spares.len(), Table::ALL.len(), "one spare per table");
        for (n, page) in spares.iter().enumerate() {
            assert!(page.iter().all(|&b| b == 0), "spare {n} is not all zeros");
        }
    }

    #[test]
    fn the_browse_menu_describes_its_pages_the_way_only_it_does() {
        // The one table whose pages count themselves differently. Getting it
        // the ordinary way round is a database a player will not open.
        let bytes = Database::new().to_bytes().unwrap();
        let page = pages(&bytes)
            .into_iter()
            .find(|page| page[0x08] == Table::Columns as u8 && page[0x1b] == 0x24 && page[0x18] > 0)
            .expect("the browse menu has a data page");
        assert_eq!(page[0x18], 27, "twenty-seven menu items");
        assert_eq!(u16::from_le_bytes(page[0x20..0x22].try_into().unwrap()), 27);
        assert_eq!(u16::from_le_bytes(page[0x22..0x24].try_into().unwrap()), 0);
        assert_eq!(page[0x1a], 3, "two groups of sixteen, and one more besides");
        assert_eq!(u32::from_le_bytes(page[0x10..0x14].try_into().unwrap()), 3);
    }

    #[test]
    fn a_data_page_counts_its_rows_the_way_a_player_reads_them() {
        let mut database = Database::new();
        for id in 1..4u32 {
            database.tracks.push(Track {
                id,
                title: format!("Track {id}"),
                file_path: format!("/Contents/A/{id}.flac"),
                ..Track::default()
            });
        }
        let bytes = database.to_bytes().unwrap();
        let page = pages(&bytes)
            .into_iter()
            .find(|page| page[0x08] == Table::Tracks as u8 && page[0x1b] == 0x24)
            .expect("the tracks have a data page");
        assert_eq!(page[0x18], 3);
        assert_eq!(page[0x19], 3 * 0x20, "the row count modulo eight, in the top bits");
        assert_eq!(page[0x1a], 0, "three rows shifted down by three");
        assert_eq!(u16::from_le_bytes(page[0x20..0x22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(page[0x22..0x24].try_into().unwrap()), 2, "rows less one");
        assert_eq!(
            u32::from_le_bytes(page[0x10..0x14].try_into().unwrap()),
            10 + 2 * 5,
            "the tracks' sequence base, plus five a row after the first"
        );
    }

    /// A header page's body, against the bytes a real export carries.
    ///
    /// Transcribed from the `complete_export` fixture, whose table layout this
    /// happens to reproduce exactly, so the page indices are comparable too:
    ///
    /// ```text
    /// tracks   00000001 00000002 03ffffff 00000000 1fff0001 00000010
    /// colours  0000000d 0000000e 03ffffff 00000000 1fff0000 1ffffff8 x1004
    /// history  00000027 00000028 03ffffff 00000000 1fff0001 00000140
    /// ```
    ///
    /// The second word is the one this was missing, on every table. Everything
    /// from `0x28` to the end was zeroes, so a player had a header page that
    /// parsed and nothing saying where the rows were.
    #[test]
    fn a_header_page_points_at_the_first_data_page_of_its_table() {
        let bytes = Database::new().to_bytes().unwrap();
        let word =
            |page: &[u8], at: usize| u32::from_le_bytes(page[at..at + 4].try_into().unwrap());

        for table in Table::ALL {
            let page = header_page_of(&bytes, table);
            let index = word(page, 0x04);
            assert_eq!(word(page, 0x28), index, "{table:?}: its own index");
            // The first data page — or NONE for a table with no rows, which is
            // what a real export writes and what this got wrong for a round.
            let (_, _, first, last) = directory_entry(&bytes, table);
            let want_first = if first != last { index + 1 } else { NONE_PAGE };
            assert_eq!(word(page, 0x2c), want_first, "{table:?}: the first data page");
            assert_eq!(word(page, 0x30), NONE_PAGE, "{table:?}");
            assert_eq!(word(page, 0x34), 0, "{table:?}");
            assert!(page[PAGE_LEN - 20..].iter().all(|&b| b == 0), "{table:?}: the last twenty");

            // Two tables put a number where the rest put the free-slot fill.
            match table {
                Table::Tracks => assert_eq!(word(page, 0x3c), 0x10),
                Table::History => assert_eq!(word(page, 0x3c), 0x140),
                _ => {
                    let fill = (0x3c..PAGE_LEN - 20)
                        .step_by(4)
                        .filter(|&at| word(page, at) == FREE_SLOT)
                        .count();
                    assert_eq!(fill, 1004, "{table:?}: free-slot words");
                }
            }
        }
    }

    /// The file header's table directory, which a player reads before it reads
    /// any table — and which five rounds of comparing tables could not have
    /// faulted.
    ///
    /// Every property here is one both real exports and baken share and this
    /// did not: a distinct empty candidate per table, every one below
    /// `next_unused`, every one an all-zero page that exists, a populated
    /// table's last data page pointing at it, and an empty table's header
    /// pointing at it with no first-data page.
    #[test]
    fn every_table_has_its_own_empty_page_and_the_chains_end_there() {
        let database = Database {
            tracks: vec![a_track(1), a_track(2)],
            playlists: vec![Playlist::new(1, "set", vec![1, 2])],
            ..Default::default()
        };
        let bytes = database.to_bytes().unwrap();
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let page = |i: u32| &bytes[i as usize * PAGE_LEN..(i as usize + 1) * PAGE_LEN];
        let next_unused = word(12);
        assert_eq!(next_unused as usize, bytes.len() / PAGE_LEN, "next_unused is the page count");

        let mut candidates = std::collections::BTreeSet::new();
        for table in Table::ALL {
            let (_, empty, first, last) = directory_entry(&bytes, table);
            assert!(empty < next_unused, "{table:?}: candidate {empty} not below {next_unused}");
            assert!(candidates.insert(empty), "{table:?}: candidate {empty} is shared");
            assert!(page(empty).iter().all(|&b| b == 0), "{table:?}: candidate is not zeros");
            if first == last {
                assert_eq!(word(first as usize * PAGE_LEN + 0x0c), empty, "{table:?}: header.next");
                assert_eq!(word(first as usize * PAGE_LEN + 0x2c), NONE_PAGE, "{table:?}: no data");
            } else {
                assert_eq!(word(last as usize * PAGE_LEN + 0x0c), empty, "{table:?}: last.next");
            }
        }
        assert_eq!(candidates.len(), Table::ALL.len());
    }

    /// `(kind, empty candidate, first page, last page)` for one table, out of
    /// the file header's directory.
    fn directory_entry(bytes: &[u8], want: Table) -> (u32, u32, u32, u32) {
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        for i in 0..word(8) as usize {
            let at = 0x1c + i * 16;
            if word(at) == want as u32 {
                return (word(at), word(at + 4), word(at + 8), word(at + 12));
            }
        }
        panic!("no directory entry for {want:?}");
    }

    fn header_page_of(bytes: &[u8], want: Table) -> &[u8] {
        bytes
            .chunks_exact(PAGE_LEN)
            .skip(1)
            .find(|page| {
                u32::from_le_bytes(page[8..12].try_into().unwrap()) == want as u32
                    && page[0x1b] == 0x64
            })
            .unwrap_or_else(|| panic!("no header page for {want:?}"))
    }

    #[test]
    fn the_tracks_and_the_history_say_something_of_their_own_on_their_header_pages() {
        let bytes = Database::new().to_bytes().unwrap();
        for (table, after) in [(Table::Tracks, 0x10u32), (Table::History, 0x140)] {
            let page = pages(&bytes)
                .into_iter()
                .find(|page| page[0x08] == table as u8 && page[0x1b] == 0x64)
                .unwrap_or_else(|| panic!("{table:?} has a header page"));
            assert_eq!(u16::from_le_bytes(page[0x26..0x28].try_into().unwrap()), 1, "{table:?}");
            assert_eq!(
                u32::from_le_bytes(page[0x38..0x3c].try_into().unwrap()),
                0x1fff_0001,
                "{table:?}"
            );
            assert_eq!(
                u32::from_le_bytes(page[0x3c..0x40].try_into().unwrap()),
                after,
                "{table:?}"
            );
            // The count takes the first slot of the fill, not the place of it:
            // the real page runs on with free slots from 0x40 to the end.
            assert_eq!(
                u32::from_le_bytes(page[0x40..0x44].try_into().unwrap()),
                FREE_SLOT,
                "{table:?}"
            );
            assert_eq!(
                u32::from_le_bytes(page[PAGE_LEN - 24..PAGE_LEN - 20].try_into().unwrap()),
                FREE_SLOT,
                "{table:?}"
            );
        }
        // And every other table's header page does not.
        let page = pages(&bytes)
            .into_iter()
            .find(|page| page[0x08] == Table::Genres as u8 && page[0x1b] == 0x64)
            .unwrap();
        assert_eq!(u16::from_le_bytes(page[0x26..0x28].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(page[0x38..0x3c].try_into().unwrap()), 0x1fff_0000);
    }

    /// The forty bytes a real export's history table carries.
    ///
    /// The two vectors are baken's own, from its `rows.rs` tests, which read
    /// them off a real rekordbox 7 export: one empty drive and the 587-track
    /// reference. Transcribed rather than derived, because the point of them is
    /// that they came off a file and not out of a formula.
    ///
    /// The forty is measured here rather than taken on trust, because the two
    /// implementations that write this row disagree about it: the history pages
    /// of rekordcrate's `complete_export` fixture report `heap_used` 40 for a
    /// page of one row and 480 for a page of twelve, so every row in a real
    /// export is forty bytes.
    #[test]
    fn the_history_row_is_the_forty_bytes_a_real_export_carries() {
        let row = history_row(0, "2024-11-16", "");
        assert_eq!(row.len(), 40, "a history row is padded to forty bytes");
        let want = [
            0x80, 0x02, 0x00, 0x00, // 0x0280
            0x00, 0x00, 0x00, 0x00, // no tracks
            0x00, 0x00, 0x00, 0x00, // and a zero nobody has explained
            0x17, b'2', b'0', b'2', b'4', b'-', b'1', b'1', b'-', b'1', b'6', // the date
            0x19, 0x1e, // two more unexplained bytes
            0x0b, b'1', b'0', b'0', b'0', // the string "1000"
            0x03, // an empty device name
        ];
        assert_eq!(&row[..want.len()], &want);
        assert!(row[want.len()..].iter().all(|&b| b == 0), "the padding is not zeroes");

        // The count is what a CDJ-3000 shows as Songs, so it is the one field
        // here whose value a person could notice being wrong.
        let reference = history_row(587, "2025-04-11", "");
        assert_eq!(&reference[..8], &[0x80, 0x02, 0x00, 0x00, 0x4b, 0x02, 0x00, 0x00]);
    }

    /// A UTF-16 device name starts on a four-byte boundary of the row.
    ///
    /// baken records a CDJ-2000NXS2 freezing on a stick whose strings were
    /// packed without this, which is a harder failure than the one this change
    /// is chasing and worth not reintroducing.
    #[test]
    fn a_device_name_that_is_not_ascii_is_aligned_within_the_row() {
        let row = history_row(1, "2026-10-11", "Ünlaut");
        let at = row.iter().position(|&b| b == 0x90).expect("no utf-16 string in the row");
        assert_eq!(at % 4, 0, "the name starts at {at}, which is not a four-byte boundary");
        assert!(row.len() >= 40);
    }

    /// The table a CDJ-3000 refuses a whole database over is no longer empty.
    #[test]
    fn the_history_table_carries_its_one_row() {
        let database = Database {
            tracks: vec![a_track(1), a_track(2)],
            playlists: Vec::new(),
            device_name: "OVERCUE".into(),
            ..Default::default()
        };
        let bytes = database.to_bytes().unwrap();
        let history = table(&bytes, Table::History);
        assert_eq!(history.rows, 1, "the history table is empty again");

        // And the count it reports is the collection's, which is the number a
        // player puts on the screen.
        let found = bytes.windows(8).any(|w| w == [0x80, 0x02, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00]);
        assert!(found, "no history row naming two tracks is in the file");
    }

    #[test]
    fn an_empty_string_is_the_three_the_format_docs_mention() {
        assert_eq!(string(""), vec![0x03]);
    }

    #[test]
    fn a_short_string_carries_its_length_incremented_doubled_and_incremented() {
        // "Dm" is two characters: (2 + 1) * 2 + 1 = 7.
        assert_eq!(string("Dm"), vec![0x07, b'D', b'm']);
        assert_eq!(string("Pink")[0], 0x0b);
    }

    #[test]
    fn a_long_string_switches_to_a_four_byte_header() {
        let long = "A".repeat(200);
        let encoded = string(&long);
        assert_eq!(encoded[0], 0x40);
        assert_eq!(u16::from_le_bytes(encoded[1..3].try_into().unwrap()) as usize, 204);
        assert_eq!(encoded.len(), 204);
    }

    #[test]
    fn a_string_that_is_not_ascii_becomes_utf16() {
        let encoded = string("Jóga");
        assert_eq!(encoded[0], 0x90);
        // Four characters, two bytes each, plus the four-byte header.
        assert_eq!(u16::from_le_bytes(encoded[1..3].try_into().unwrap()), 12);
        assert_eq!(encoded.len(), 12);
    }

    #[test]
    fn a_track_row_puts_its_strings_after_a_fixed_part_of_a_known_size() {
        let row = track_row(&a_track(1), 1, 0, 0, 0, 0, "2026-10-11");
        assert_eq!(
            u16::from_le_bytes(row[0x5e..0x60].try_into().unwrap()) as usize,
            TRACK_FIXED_LEN
        );
        // The last of the twenty-one offsets is the file path, and it should
        // land inside the row.
        let last = u16::from_le_bytes(row[0x5e + 40..0x5e + 42].try_into().unwrap()) as usize;
        assert!(last < row.len(), "the file path offset points past the end of the row");
    }

    /// Each written table's data page, against the one real export whose pages
    /// are all `0x24`.
    ///
    /// `(table, rows, 0x19, 0x1a, unknown5, num_rows_large)` read out of
    /// rekordcrate's `complete_export` fixture. Its `0x34` pages are left out
    /// on purpose: a page carrying that flag is mid-transaction, its `0x18` and
    /// `0x19` disagree about how many rows it holds, and it is therefore no
    /// evidence about what writing a page fresh should produce.
    ///
    /// Tables 17 and 18 — the categories and the sort orders — are in the
    /// fixed-list family too, and the fixture's pages say so: 22 rows with
    /// `unknown5` 22, 17 rows with `unknown5` 17, `num_rows_large` 0 on both.
    #[test]
    fn a_data_page_describes_itself_the_way_a_real_export_does() {
        // table 4 (labels) and 5 (keys) are ordinary; 6 (colours), 16 (the
        // browse menu), 17 and 18 are the fixed-list family.
        let measured: [(Table, usize, u8, u8, u16, u16); 6] = [
            (Table::Labels, 1, 0x20, 0, 1, 0),
            (Table::Keys, 5, 0xa0, 0, 1, 4),
            (Table::Colors, 8, 0x00, 1, 8, 0),
            (Table::Columns, 27, 0x60, 3, 27, 0),
            (Table::Categories, 22, 0xc0, 2, 22, 0),
            (Table::Sorts, 17, 0x20, 2, 17, 0),
        ];

        for (table, rows, want_19, want_1a, want_u5, want_nrl) in measured {
            let page = page_header(Fields {
                table,
                index: 1,
                next: 2,
                sequence: sequence_base(table),
                rows,
                free: 0,
                used: 0,
                flags: 0x24,
                unknown5: match is_fixed_list(table) {
                    true => rows as u16,
                    false => 1,
                },
                rows_large: match is_fixed_list(table) {
                    true => 0,
                    false => rows.max(1) as u16 - 1,
                },
                unknown6: 0,
                unknown7: 0,
            });
            let got = (
                page[0x18],
                page[0x19],
                page[0x1a],
                u16::from_le_bytes(page[0x20..0x22].try_into().unwrap()),
                u16::from_le_bytes(page[0x22..0x24].try_into().unwrap()),
            );
            assert_eq!(
                got,
                (rows as u8, want_19, want_1a, want_u5, want_nrl),
                "{table:?} with {rows} rows"
            );
        }
    }

    /// The whole file, so the fields the writer fills in for itself are checked
    /// too rather than only the header function.
    #[test]
    fn the_colours_page_says_eight_rows_where_it_used_to_say_one() {
        let bytes = Database::new().to_bytes().unwrap();
        let page = colors_data_page(&bytes);
        assert_eq!(u16::from_le_bytes(page[0x20..0x22].try_into().unwrap()), 8, "unknown5");
        assert_eq!(u16::from_le_bytes(page[0x22..0x24].try_into().unwrap()), 0, "num_rows_large");
        assert_eq!(page[0x1a], 1, "8 >> 3");
    }

    /// The browse menu's page, which needed a hand-added `+ 1` to be accepted
    /// by a player and now gets its 3 from the formula instead.
    #[test]
    fn the_browse_menu_still_says_three_without_being_told_to() {
        let bytes = Database::new().to_bytes().unwrap();
        let page = table_data_page(&bytes, Table::Columns);
        assert_eq!(page[0x1a], 3, "27 >> 3");
        assert_eq!(page[0x18], 27);
    }

    fn colors_data_page(bytes: &[u8]) -> &[u8] {
        table_data_page(bytes, Table::Colors)
    }

    /// The first data page of a table — the one after its header page.
    fn table_data_page(bytes: &[u8], want: Table) -> &[u8] {
        for page in bytes.chunks_exact(PAGE_LEN).skip(1) {
            let table = u32::from_le_bytes(page[8..12].try_into().unwrap());
            if table == want as u32 && page[0x1b] == 0x24 && page[0x18] > 0 {
                return page;
            }
        }
        panic!("no populated data page for {want:?}");
    }

    #[test]
    fn every_table_is_present_even_when_it_has_no_rows() {
        let bytes = Database::new().to_bytes().unwrap();
        let tables = inspect(&bytes).unwrap();
        assert_eq!(tables.len(), 20);

        // Five tables have rows on a drive with no music at all: the eight
        // colours and the three tables of the browse menu, which are the same
        // on every export, and the one history row, which a CDJ-3000 wants
        // there whether or not anything has been played.
        let furnished = ["Colors", "Columns", "Categories", "Sorts", "History"];
        assert!(tables.iter().all(|t| t.rows == 0 || furnished.contains(&t.table.as_str())));
        assert_eq!(table(&bytes, Table::Colors).rows, 8);
        assert_eq!(table(&bytes, Table::Columns).rows, 27);
        assert_eq!(table(&bytes, Table::Categories).rows, 22);
        assert_eq!(table(&bytes, Table::Sorts).rows, 17);
        assert_eq!(table(&bytes, Table::History).rows, 1);
    }

    /// The two tables, against the bytes a real export carries.
    ///
    /// Transcribed from rekordcrate's `complete_export` fixture, a rekordbox 6
    /// export with the browse preferences at their defaults; baken writes the
    /// same bytes. Row order included: the hidden rows first, by id, then the
    /// shown ones down the menu.
    #[test]
    fn the_categories_and_sort_orders_are_the_rows_a_real_export_carries() {
        let bytes = Database::new().to_bytes().unwrap();
        let heap = |table: Table, rows: usize| {
            let page = table_data_page(&bytes, table);
            assert_eq!(page[0x18] as usize, rows, "{table:?}");
            page[PAGE_HEADER_LEN..PAGE_HEADER_LEN + rows * 8].to_vec()
        };
        let categories = "\
            0100010063010000 0500060005010000 0600070063010000 0700080063010000 \
            0800090063010000 09000a0063010000 0a000b0063010000 0d000f0063010000 \
            0e00130004010000 0f00140006010000 1000150063010000 1200170063010000 \
            0200020002000100 0300030003000200 0400040001000300 0b000c0063000400 \
            1100050063000500 1300160063000600 1400120063000700 1b001a0063020800 \
            1800110063000900 16001b0063000a00";
        let sorts = "\
            0100060001000000 1500070001000000 0e00080001000000 0800090001000000 \
            09000a0001000000 0a000b0001000000 0f000d0001000000 0d000f0001000000 \
            1700100001000000 1600110001000000 1900000000010000 1a00010000020000 \
            0200020000030000 0300030000040000 0500040000050000 0600050000060000 \
            0b000c0000070000";
        let packed = |text: &str| text.split_whitespace().collect::<String>();
        assert_eq!(hex_of(&heap(Table::Categories, 22)), packed(categories));
        assert_eq!(hex_of(&heap(Table::Sorts, 17)), packed(sorts));
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `2, 3, 4, 5`, in both real exports, on the pages of the four tables that
    /// never change; their header pages say 1 like everyone else's.
    #[test]
    fn the_fixed_lists_carry_the_sequence_numbers_rekordbox_gave_them() {
        let bytes = Database::new().to_bytes().unwrap();
        let sequence = |page: &[u8]| u32::from_le_bytes(page[0x10..0x14].try_into().unwrap());
        for (table, want) in
            [(Table::Colors, 2), (Table::Columns, 3), (Table::Categories, 4), (Table::Sorts, 5)]
        {
            assert_eq!(sequence(table_data_page(&bytes, table)), want, "{table:?}");
            assert_eq!(sequence(header_page_of(&bytes, table)), 1, "{table:?} header");
        }
    }

    /// The eleventh and sixteenth strings of a track row — the date it was
    /// added and the date it was analysed — which a real export never leaves
    /// empty.
    fn dates_of(row: &[u8]) -> (String, String) {
        let string_at = |n: usize| {
            let at = u16::from_le_bytes(row[0x5e + 2 * n..0x5e + 2 * n + 2].try_into().unwrap())
                as usize;
            // Short ASCII: the length byte is `(len + 1) << 1 | 1`.
            let len = (row[at] >> 1) as usize - 1;
            String::from_utf8(row[at + 1..at + 1 + len].to_vec()).unwrap()
        };
        (string_at(10), string_at(15))
    }

    #[test]
    fn a_track_without_dates_is_dated_the_day_it_is_written() {
        let row = track_row(&a_track(1), 1, 0, 0, 0, 0, "2026-10-11");
        assert_eq!(dates_of(&row), ("2026-10-11".to_string(), "2026-10-11".to_string()));

        let dated = Track {
            date_added: "2018-05-25".to_string(),
            analyze_date: "2022-02-02".to_string(),
            ..a_track(1)
        };
        let row = track_row(&dated, 1, 0, 0, 0, 0, "2026-10-11");
        assert_eq!(dates_of(&row), ("2018-05-25".to_string(), "2022-02-02".to_string()));

        // And the whole file dates them today, rather than leaving the choice
        // to a caller that has no date to give.
        let bytes = Database { tracks: vec![a_track(7)], ..Default::default() }.to_bytes().unwrap();
        let page = table_data_page(&bytes, Table::Tracks);
        let at = u16::from_le_bytes(page[PAGE_LEN - 6..PAGE_LEN - 4].try_into().unwrap()) as usize;
        let today = crate::clock::day(crate::clock::now());
        assert_eq!(dates_of(&page[PAGE_HEADER_LEN + at..]), (today.clone(), today));
    }

    #[test]
    fn a_track_row_carries_its_own_id_where_every_working_writer_puts_something_unique() {
        let row = track_row(&a_track(42), 1, 0, 0, 0, 0, "2026-10-11");
        assert_eq!(u32::from_le_bytes(row[0x14..0x18].try_into().unwrap()), 42);
    }

    /// Three rows of two real exports: a 320 kbps MP3, a 16-bit WAV and a
    /// 24-bit one.
    #[test]
    fn a_bitrate_is_the_one_a_real_export_prints() {
        assert_eq!(average_kbps(6_899_624, 172.0), 320);
        assert_eq!(pcm_kbps(44_100, 16, 2), 1411);
        assert_eq!(pcm_kbps(44_100, 24, 2), 2116);
        assert_eq!(average_kbps(1, 0.0), 0, "a file with no length has no rate");
    }

    #[test]
    fn rows_land_where_the_index_says_they_do() {
        let database = Database {
            tracks: (1..=5).map(a_track).collect(),
            playlists: vec![Playlist::new(1, "set", vec![1, 2, 3, 4, 5])],
            ..Default::default()
        };
        let bytes = database.to_bytes().unwrap();
        assert_eq!(table(&bytes, Table::Tracks).rows, 5);
        assert_eq!(table(&bytes, Table::PlaylistEntries).rows, 5);
        assert_eq!(table(&bytes, Table::PlaylistTree).rows, 1);
        // One artist, shared by all five.
        assert_eq!(table(&bytes, Table::Artists).rows, 1);
    }

    #[test]
    fn a_table_that_outgrows_a_page_gets_another_one() {
        let database = Database {
            tracks: (1..=60).map(a_track).collect(),
            playlists: Vec::new(),
            ..Default::default()
        };
        let bytes = database.to_bytes().unwrap();
        let tracks = table(&bytes, Table::Tracks);
        assert!(tracks.pages > 2, "60 track rows should not fit on one page");
        assert_eq!(tracks.rows, 60);
    }

    #[test]
    fn the_file_is_a_whole_number_of_pages() {
        let bytes = Database {
            tracks: (1..=60).map(a_track).collect(),
            playlists: Vec::new(),
            ..Default::default()
        }
        .to_bytes()
        .unwrap();
        assert_eq!(bytes.len() % PAGE_LEN, 0);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, PAGE_LEN);
        // The header says where the unused pages start, which is the end.
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize,
            bytes.len() / PAGE_LEN
        );
    }

    #[test]
    fn inspect_rejects_a_file_that_is_not_a_database() {
        assert!(inspect(b"not a database").is_err());
        assert!(inspect(&[0u8; 64]).is_err(), "a page length of zero is not a database");
    }

    #[test]
    fn inspect_notices_a_chain_that_leaves_the_file() {
        // Truncation is only detectable where something points at what is
        // missing: the trailing pages of an empty table are referenced by
        // nothing, so losing those is invisible here and is caught by checking
        // the file's length instead.
        let mut bytes = Database {
            tracks: (1..=5).map(a_track).collect(),
            playlists: Vec::new(),
            ..Default::default()
        }
        .to_bytes()
        .unwrap();
        bytes.truncate(PAGE_LEN * 2);
        let message = inspect(&bytes).unwrap_err().to_string();
        assert!(message.contains("past the end of the file"), "{message}");
    }
}
