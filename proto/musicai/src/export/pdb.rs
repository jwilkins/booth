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
    Unknown17 = 17,
    Unknown18 = 18,
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
        Table::Unknown17,
        Table::Unknown18,
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

/// The eight track colours rekordbox offers, in the order it numbers them. A
/// track's `color_id` is a row in this table, so the table has to be there even
/// when nothing is coloured.
const COLORS: [&str; 8] = ["Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"];

/// The browse menu, as rekordbox writes it: the categories a player offers when
/// you turn the encoder. The second value of each pair identifies the menu item
/// to the player and is not simply the row number — the gaps are rekordbox's.
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

/// Everything that goes on a drive.
#[derive(Clone, Debug, Default)]
pub struct Database {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
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
            COLUMNS
                .iter()
                .enumerate()
                .map(|(i, (menu, name))| RowData::plain(column_row(i as u16 + 1, *menu, name)))
                .collect(),
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

        // Every table costs its header page plus its data pages, and an empty
        // table still costs two, so the page count is known before anything is
        // written — which matters because the pages have to point past the end
        // of the file to say "no more of this table".
        let past_end: u32 =
            1 + packed.iter().map(|(_, groups)| 1 + groups.len().max(1) as u32).sum::<u32>();

        let mut pages: Vec<Vec<u8>> = Vec::with_capacity(past_end as usize);
        let mut table_refs = Vec::new();
        let mut next_index = 1u32;

        for (table, groups) in &packed {
            let first_page = next_index;
            pages.push(header_page(*table, first_page, first_page + 1));
            next_index += 1;

            if groups.is_empty() {
                // A table with nothing in it still gets a page, and that page
                // still has to be a page: written as zeroes it is not one, and
                // a drive is refused whole rather than a table at a time. The
                // header page remains the table's last, so a reader that
                // follows the chain stops before reaching this.
                pages.push(data_page(*table, next_index, past_end, 0, &[]));
                next_index += 1;
                table_refs.push((*table, first_page, first_page));
                continue;
            }

            let mut so_far = 0usize;
            for (i, group) in groups.iter().enumerate() {
                let last = i + 1 == groups.len();
                let next = if last { past_end } else { next_index + 1 };
                so_far += group.len();
                pages.push(data_page(*table, next_index, next, so_far, group));
                next_index += 1;
            }
            table_refs.push((*table, first_page, next_index - 1));
        }
        debug_assert_eq!(next_index, past_end);

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
        for (table, first, last) in &table_refs {
            put_u32(&mut header, *table as u32);
            put_u32(&mut header, past_end); // empty candidate
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
        Table::Colors => 8,
        Table::PlaylistTree => 6,
        Table::PlaylistEntries => 11,
        // Not a formula at all: the browse menu is the same 27 rows on every
        // drive, and its pages always say 3.
        Table::Columns => 3,
        _ => 1,
    }
}

/// The row-less page every table starts with.
///
/// Marked 0x64, with both row-count fields saturated and a constant nobody has
/// explained in the last field but one. Two tables — the tracks and the
/// history — carry different values again, in a body that is otherwise empty:
/// the reason is unknown, and a player that does not find them there refuses
/// the whole database rather than the table.
fn header_page(table: Table, index: u32, next: u32) -> Vec<u8> {
    let special = matches!(table, Table::Tracks | Table::History);
    let mut page = page_header(Fields {
        table,
        index,
        next,
        sequence: sequence_base(table),
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
    let marker: u32 = 0x1fff_0000 | u32::from(special);
    page[0x38..0x3c].copy_from_slice(&marker.to_le_bytes());
    let after: u32 = match table {
        Table::Tracks => 0x10,
        Table::History => 0x140,
        _ => 0,
    };
    page[0x3c..0x40].copy_from_slice(&after.to_le_bytes());
    page
}

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
    // The browse menu's pages describe themselves differently from every other
    // table's, and getting it the ordinary way round is one of the things a
    // player refuses the whole database over.
    let columns = matches!(table, Table::Columns);
    let mut page = page_header(Fields {
        table,
        index,
        next,
        sequence: match columns {
            true => sequence_base(table),
            false => sequence_base(table) + (so_far.max(1) as u32 - 1) * 5,
        },
        rows: rows.len(),
        free,
        used,
        flags: 0x24, // an ordinary data page with nothing deleted from it
        unknown5: match columns {
            true => rows.len() as u16,
            false => 1,
        },
        rows_large: match columns {
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
    // How many groups the index at the end of the page holds — one per sixteen
    // rows. The browse menu counts one more than it has.
    page.push(
        (f.rows.div_ceil(ROWS_PER_GROUP) + usize::from(matches!(f.table, Table::Columns))) as u8,
    );
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

/// The most a row may take, which is a page with one row in it.
///
/// A row cannot span pages: the page header counts its rows and its free space
/// in sixteen bits, and the index at the end of the page points at each row's
/// start. So a row that does not fit a page cannot be written at all — and
/// until this existed, one was: the page writer worked out its free space as
/// `PAGE_LEN - header - used`, which underflowed, and what went on the stick
/// was a page truncated back to 4 kB with a row cut in half inside it. Debug
/// builds panicked on the subtraction; release builds wrote the corrupt page.
pub(crate) const MAX_ROW_LEN: usize = PAGE_LEN - PAGE_HEADER_LEN - ROW_INDEX_LEN;

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
) -> Vec<u8> {
    let row = track_row_exactly(track, artist_id, album_id, genre_id, label_id, key_id);
    if row.len() <= MAX_ROW_LEN {
        return row;
    }
    // How much of the comment there is room for, in bytes of encoded string.
    // Taken off what the row came to rather than guessed at, so it is right
    // whatever else the row is carrying.
    let over = row.len() - MAX_ROW_LEN;
    let mut shorter = track.clone();
    shorter.comment = shorten(&track.comment, track.comment.len().saturating_sub(over + 8));
    track_row_exactly(&shorter, artist_id, album_id, genre_id, label_id, key_id)
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
) -> Vec<u8> {
    let mut row = Vec::with_capacity(TRACK_FIXED_LEN + 256);
    put_u16(&mut row, 0x0024);
    put_u16(&mut row, 0); // index_shift, filled in when the row is placed
    put_u32(&mut row, 0x000c_0700); // constant in every export seen
    put_u32(&mut row, track.sample_rate);
    put_u32(&mut row, 0); // composer
    put_u32(&mut row, track.file_size);
    put_u32(&mut row, 0); // an id of some kind, purpose unknown
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
        string(&track.date_added),
        string(""), // release date
        string(""), // mix name
        string(""),
        string(&track.analyze_path),
        string(&track.analyze_date),
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
    fn a_table_with_no_rows_still_gets_a_page_that_is_a_page() {
        // Written as zeroes it is not one: no page index, no table, no flags —
        // and a player refuses the whole database over it rather than skipping
        // the table.
        let bytes = Database::new().to_bytes().unwrap();
        for (n, page) in pages(&bytes).iter().enumerate() {
            let index = u32::from_le_bytes(page[0x04..0x08].try_into().unwrap());
            let flags = page[0x1b];
            assert_eq!(index as usize, n + 1, "page {n} does not know its own index");
            assert!(
                flags == 0x24 || flags == 0x64,
                "page {n} is neither a header page nor a data page: {flags:#04x}"
            );
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
        assert_eq!(page[0x1a], 1, "one group of sixteen holds three rows");
        assert_eq!(u16::from_le_bytes(page[0x20..0x22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(page[0x22..0x24].try_into().unwrap()), 2, "rows less one");
        assert_eq!(
            u32::from_le_bytes(page[0x10..0x14].try_into().unwrap()),
            10 + 2 * 5,
            "the tracks' sequence base, plus five a row after the first"
        );
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
        }
        // And every other table's header page does not.
        let page = pages(&bytes)
            .into_iter()
            .find(|page| page[0x08] == Table::Genres as u8 && page[0x1b] == 0x64)
            .unwrap();
        assert_eq!(u16::from_le_bytes(page[0x26..0x28].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(page[0x38..0x3c].try_into().unwrap()), 0x1fff_0000);
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
        let row = track_row(&a_track(1), 1, 0, 0, 0, 0);
        assert_eq!(
            u16::from_le_bytes(row[0x5e..0x60].try_into().unwrap()) as usize,
            TRACK_FIXED_LEN
        );
        // The last of the twenty-one offsets is the file path, and it should
        // land inside the row.
        let last = u16::from_le_bytes(row[0x5e + 40..0x5e + 42].try_into().unwrap()) as usize;
        assert!(last < row.len(), "the file path offset points past the end of the row");
    }

    #[test]
    fn every_table_is_present_even_when_it_has_no_rows() {
        let bytes = Database::new().to_bytes().unwrap();
        let tables = inspect(&bytes).unwrap();
        assert_eq!(tables.len(), 20);
        assert!(tables.iter().all(|t| t.rows == 0 || t.table == "Colors" || t.table == "Columns"));
        assert_eq!(table(&bytes, Table::Colors).rows, 8);
        assert_eq!(table(&bytes, Table::Columns).rows, 27);
    }

    #[test]
    fn rows_land_where_the_index_says_they_do() {
        let database = Database {
            tracks: (1..=5).map(a_track).collect(),
            playlists: vec![Playlist::new(1, "set", vec![1, 2, 3, 4, 5])],
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
        let database = Database { tracks: (1..=60).map(a_track).collect(), playlists: Vec::new() };
        let bytes = database.to_bytes().unwrap();
        let tracks = table(&bytes, Table::Tracks);
        assert!(tracks.pages > 2, "60 track rows should not fit on one page");
        assert_eq!(tracks.rows, 60);
    }

    #[test]
    fn the_file_is_a_whole_number_of_pages() {
        let bytes = Database { tracks: (1..=60).map(a_track).collect(), playlists: Vec::new() }
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
        let mut bytes = Database { tracks: (1..=5).map(a_track).collect(), playlists: Vec::new() }
            .to_bytes()
            .unwrap();
        bytes.truncate(PAGE_LEN * 2);
        let message = inspect(&bytes).unwrap_err().to_string();
        assert!(message.contains("past the end of the file"), "{message}");
    }
}
