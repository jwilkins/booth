//! Reading the tracks and playlists out of an `export.pdb`.
//!
//! What a CDJ-3000 and every player before it browses: the track rows, the
//! playlist tree and the playlist entries. The rest of the file — artists,
//! albums, keys, colours, the browse menu — is how those rows are shown, not
//! what is on the drive, and is not read.
//!
//! Also what those players write back: the history, a numbered list of the
//! tracks loaded each time the drive was mounted, in two tables of its own.
//! That is read on its own, by [`history`], so that a history this cannot make
//! sense of does not stop the drive itself being read.
//!
//! Written from Deep Symmetry's notes on the format rather than from
//! [`crate::export::pdb`], and sharing no code with it, for the same reason
//! [`crate::export::pdb::inspect`] does not: a reader built from the writer
//! agrees with the writer about everything, including what it gets wrong. It
//! is checked against `rekordcrate`, which is somebody else's reading of the
//! same notes, on two exports rekordbox itself wrote and on drives this
//! program writes.

use anyhow::{bail, Context, Result};

/// The tables read here, by the type number the header lists them under.
const TRACKS: u32 = 0;
const PLAYLIST_TREE: u32 = 7;
const PLAYLIST_ENTRIES: u32 = 8;
const HISTORY_PLAYLISTS: u32 = 11;
const HISTORY_ENTRIES: u32 = 12;

/// Every page starts with this much header before its heap; row offsets are
/// counted from the end of it.
const PAGE_HEADER_LEN: usize = 0x28;
/// The row index, at the end of the page, comes in groups of sixteen rows,
/// each group this many bytes: sixteen offsets, then the bitmask of which of
/// them are present, then a second word nobody has explained.
const ROW_GROUP_LEN: usize = 0x24;
const ROWS_PER_GROUP: usize = 16;

/// Where a track row's string offsets start, and how many there are.
const TRACK_STRINGS_AT: usize = 0x5e;
const TRACK_STRINGS: usize = 21;
/// Which of those strings are which.
const ANALYZE_PATH: usize = 14;
const TITLE: usize = 17;
const FILE_NAME: usize = 19;
const FILE_PATH: usize = 20;

/// One track as the database lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Track {
    pub id: u32,
    pub title: String,
    /// Where the audio is on the drive, from its root: `/Contents/…`.
    pub file_path: String,
    /// The last part of that, which the row keeps as a string of its own.
    pub file_name: String,
    /// Bytes, as the database was told when the drive was written.
    pub file_size: u32,
    /// Where its `.DAT` is, the same way: `/PIONEER/USBANLZ/…/ANLZ0000.DAT`.
    pub analyze_path: String,
    pub sample_rate: u32,
}

/// A playlist, or a folder of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Node {
    pub id: u32,
    /// The folder it is in; zero at the top.
    pub parent: u32,
    /// Where it sorts among the others in that folder.
    pub sort_order: u32,
    pub folder: bool,
    /// A smart playlist, whose tracks are a query rather than a list. Only a
    /// OneLibrary database has these; `export.pdb` never does.
    pub smart: bool,
    pub name: String,
}

/// One track's place in one playlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub playlist: u32,
    /// Its position in the playlist, from one.
    pub index: u32,
    pub track: u32,
}

/// One of the lists a player keeps of what it played.
///
/// A new one each time the drive is mounted, named by the player as a number —
/// `HISTORY 001`, `HISTORY 002` — and not dated: the row holds the id and the
/// name and nothing else.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct History {
    pub id: u32,
    pub name: String,
}

/// What a player can browse on a drive: read out of `export.pdb` here, or out
/// of a OneLibrary database by [`super::onelibrary::library`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    /// By id.
    pub tracks: Vec<Track>,
    /// By id.
    pub nodes: Vec<Node>,
    /// By playlist, then position.
    pub entries: Vec<Entry>,
}

/// What the players have recorded playing off a drive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Histories {
    /// By id.
    pub lists: Vec<History>,
    /// The tracks in each, by list, then position: the order they were loaded
    /// in. `playlist` is the list's id.
    pub entries: Vec<Entry>,
}

/// Read the tracks and playlists out of an `export.pdb`.
pub fn read(bytes: &[u8]) -> Result<Library> {
    let file = File::new(bytes)?;

    let mut tracks = Vec::new();
    for row in file.rows(TRACKS)? {
        tracks.push(track(row).context("a track row")?);
    }
    let mut nodes = Vec::new();
    for row in file.rows(PLAYLIST_TREE)? {
        nodes.push(Node {
            parent: u32_at(row, 0x00)?,
            sort_order: u32_at(row, 0x08)?,
            id: u32_at(row, 0x0c)?,
            folder: u32_at(row, 0x10)? != 0,
            smart: false,
            name: string_at(row, 0x14).context("a playlist's name")?,
        });
    }
    let mut entries = Vec::new();
    for row in file.rows(PLAYLIST_ENTRIES)? {
        entries.push(Entry {
            index: u32_at(row, 0x00)?,
            track: u32_at(row, 0x04)?,
            playlist: u32_at(row, 0x08)?,
        });
    }

    tracks.sort();
    nodes.sort();
    entries.sort();
    Ok(Library { tracks, nodes, entries })
}

/// Read the history the players have written into an `export.pdb`.
///
/// The entries name tracks by id, which [`read`] gives the rows of.
pub fn history(bytes: &[u8]) -> Result<Histories> {
    let file = File::new(bytes)?;
    let mut lists = Vec::new();
    for row in file.rows(HISTORY_PLAYLISTS)? {
        lists.push(History {
            id: u32_at(row, 0x00)?,
            name: string_at(row, 0x04).context("a history's name")?,
        });
    }
    // The same three numbers as a playlist entry, in a different order.
    let mut entries = Vec::new();
    for row in file.rows(HISTORY_ENTRIES)? {
        entries.push(Entry {
            track: u32_at(row, 0x00)?,
            playlist: u32_at(row, 0x04)?,
            index: u32_at(row, 0x08)?,
        });
    }
    lists.sort();
    entries.sort();
    Ok(Histories { lists, entries })
}

fn track(row: &[u8]) -> Result<Track> {
    let string = |which: usize| -> Result<String> {
        let at = u16_at(row, TRACK_STRINGS_AT + 2 * which)? as usize;
        string_at(row, at)
    };
    if row.len() < TRACK_STRINGS_AT + 2 * TRACK_STRINGS {
        bail!("{} bytes is too short for a track row", row.len());
    }
    Ok(Track {
        id: u32_at(row, 0x48)?,
        sample_rate: u32_at(row, 0x08)?,
        file_size: u32_at(row, 0x10)?,
        title: string(TITLE)?,
        file_name: string(FILE_NAME)?,
        file_path: string(FILE_PATH)?,
        analyze_path: string(ANALYZE_PATH)?,
    })
}

/// The pages of a database, and the tables the header page lists.
struct File<'a> {
    bytes: &'a [u8],
    page_len: usize,
    /// `(type, first page, last page)`.
    tables: Vec<(u32, u32, u32)>,
}

impl<'a> File<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        let page_len = u32_at(bytes, 0x04)? as usize;
        if page_len < PAGE_HEADER_LEN + ROW_GROUP_LEN || bytes.len() % page_len != 0 {
            bail!("a page length of {page_len} does not fit a file of {} bytes", bytes.len());
        }
        let count = u32_at(bytes, 0x08)? as usize;
        if 0x1c + count * 16 > page_len {
            bail!("{count} tables will not fit in the header page");
        }
        let tables = (0..count)
            .map(|i| {
                let at = 0x1c + i * 16;
                Ok((u32_at(bytes, at)?, u32_at(bytes, at + 8)?, u32_at(bytes, at + 12)?))
            })
            .collect::<Result<_>>()?;
        Ok(Self { bytes, page_len, tables })
    }

    /// Every row of one table that its pages mark as present, each from where
    /// it starts to the end of its page. A table the header does not list has
    /// no rows.
    fn rows(&self, kind: u32) -> Result<Vec<&'a [u8]>> {
        let Some(&(_, first, last)) = self.tables.iter().find(|(k, ..)| *k == kind) else {
            return Ok(Vec::new());
        };
        let pages = self.bytes.len() / self.page_len;
        let mut rows = Vec::new();
        let mut index = first;
        for walked in 0.. {
            if walked > pages || index as usize >= pages {
                bail!("table {kind}'s pages run past the end of the file or loop at {index}");
            }
            let page = &self.bytes[index as usize * self.page_len..][..self.page_len];
            // A page whose flags have 0x40 set is an index page, with no rows.
            if page[0x1b] & 0x40 == 0 {
                let offsets = (u32_at(page, 0x18)? & 0x1fff) as usize;
                for row in 0..offsets {
                    let group = page.len() - (row / ROWS_PER_GROUP) * ROW_GROUP_LEN;
                    let present = u16_at(page, group - 4)?;
                    if present & (1 << (row % ROWS_PER_GROUP)) == 0 {
                        continue;
                    }
                    let offset = u16_at(page, group - 6 - 2 * (row % ROWS_PER_GROUP))? as usize;
                    let start = PAGE_HEADER_LEN + offset;
                    if start >= page.len() {
                        bail!("a row of table {kind} on page {index} starts past the page");
                    }
                    rows.push(&page[start..]);
                }
            }
            if index == last {
                break;
            }
            index = u32_at(page, 0x0c)?;
        }
        Ok(rows)
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    match bytes.get(at..at + 2) {
        Some(b) => Ok(u16::from_le_bytes([b[0], b[1]])),
        None => bail!("truncated at byte {at}"),
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    match bytes.get(at..at + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => bail!("truncated at byte {at}"),
    }
}

/// A DeviceSQL string at `at`.
///
/// Three forms. An odd first byte is short ASCII, the byte being twice the
/// length plus one, header included. `0x40` is ASCII too long for that, and
/// `0x90` is UTF-16, little-endian: both have the length of the whole string,
/// header included, in the two bytes after, then a pad byte, then the text.
fn string_at(bytes: &[u8], at: usize) -> Result<String> {
    let Some(&kind) = bytes.get(at) else { bail!("a string past the end of its row") };
    let body = |from: usize, len: usize| match bytes.get(from..from + len) {
        Some(body) => Ok(body),
        None => bail!("a string of {len} bytes at {at} runs past its row"),
    };
    if kind & 1 == 1 {
        let len = (kind as usize >> 1).saturating_sub(1);
        return Ok(String::from_utf8_lossy(body(at + 1, len)?).into_owned());
    }
    let len = (u16_at(bytes, at + 1)? as usize)
        .checked_sub(4)
        .with_context(|| format!("a string at {at} shorter than its own header"))?;
    match kind {
        0x40 => Ok(String::from_utf8_lossy(body(at + 4, len)?).into_owned()),
        0x90 => {
            let units: Vec<u16> = body(at + 4, len)?
                .chunks_exact(2)
                .map(|u| u16::from_le_bytes([u[0], u[1]]))
                .collect();
            Ok(String::from_utf16_lossy(&units))
        }
        other => bail!("a string of kind {other:#04x} at {at}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::pdb as written;

    fn a_track(id: u32, title: &str, file_path: &str) -> written::Track {
        written::Track {
            id,
            title: title.to_string(),
            file_path: file_path.to_string(),
            analyze_path: format!("/PIONEER/USBANLZ/P00{id}/0000000{id}/ANLZ0000.DAT"),
            sample_rate: 44_100,
            ..written::Track::default()
        }
    }

    #[test]
    fn what_this_program_writes_reads_back_as_the_same_tracks_and_playlists() {
        // Short ASCII, ASCII too long for one length byte, and UTF-16.
        let long = "x".repeat(200);
        let mut database = written::Database::new();
        database.tracks = vec![
            a_track(1, "Hold Me", "/Contents/a.flac"),
            a_track(2, &long, "/Contents/b.mp3"),
            a_track(3, "1ØPILLS — Ø", "/Contents/Ø/c.wav"),
        ];
        database.playlists = vec![
            written::Playlist::folder(1, "Saturday"),
            written::Playlist::new(2, "Warm-up", vec![3, 1]).in_folder(1),
            written::Playlist::new(3, "Peak", vec![2]),
        ];
        let read = read(&database.to_bytes().unwrap()).unwrap();

        assert_eq!(
            read.tracks
                .iter()
                .map(|t| (t.id, t.title.as_str(), t.file_path.as_str()))
                .collect::<Vec<_>>(),
            [
                (1, "Hold Me", "/Contents/a.flac"),
                (2, long.as_str(), "/Contents/b.mp3"),
                (3, "1ØPILLS — Ø", "/Contents/Ø/c.wav"),
            ]
        );
        assert_eq!(read.tracks[2].analyze_path, "/PIONEER/USBANLZ/P003/00000003/ANLZ0000.DAT");
        assert_eq!(read.tracks[0].sample_rate, 44_100);
        assert_eq!(
            read.nodes
                .iter()
                .map(|n| (n.id, n.parent, n.folder, n.name.as_str()))
                .collect::<Vec<_>>(),
            [(1, 0, true, "Saturday"), (2, 1, false, "Warm-up"), (3, 0, false, "Peak")]
        );
        assert_eq!(
            read.entries.iter().map(|e| (e.playlist, e.index, e.track)).collect::<Vec<_>>(),
            [(2, 1, 3), (2, 2, 1), (3, 1, 2)]
        );
    }

    #[test]
    fn a_drive_of_a_thousand_tracks_reads_back_every_one_of_them_across_its_pages() {
        let mut database = written::Database::new();
        database.tracks = (1..=1000)
            .map(|id| a_track(id, &format!("Track {id}"), &format!("/Contents/{id}.mp3")))
            .collect();
        database.playlists = vec![written::Playlist::new(1, "All", (1..=1000).rev().collect())];
        let read = read(&database.to_bytes().unwrap()).unwrap();
        assert_eq!(read.tracks.len(), 1000);
        assert_eq!(read.tracks[999].title, "Track 1000");
        assert_eq!(read.entries.len(), 1000);
        assert_eq!((read.entries[0].index, read.entries[0].track), (1, 1000));
    }

    #[test]
    fn a_players_history_reads_back_as_its_numbered_lists_in_the_order_it_loaded_them() {
        let mut database = written::Database::new();
        database.tracks = vec![
            a_track(1, "Hold Me", "/Contents/Peverelist/a.flac"),
            a_track(2, "Roll With The Punches", "/Contents/Peverelist/b.flac"),
        ];
        database.tracks[1].file_size = 55_720_044;
        database.history = vec![
            written::HistoryPlaylist {
                id: 1,
                name: "HISTORY 001".into(),
                track_ids: vec![2, 1, 2],
            },
            written::HistoryPlaylist { id: 2, name: "HISTORY 002".into(), track_ids: vec![1] },
        ];
        let bytes = database.to_bytes().unwrap();
        let played = history(&bytes).unwrap();

        assert_eq!(
            played.lists.iter().map(|h| (h.id, h.name.as_str())).collect::<Vec<_>>(),
            [(1, "HISTORY 001"), (2, "HISTORY 002")]
        );
        assert_eq!(
            played.entries.iter().map(|e| (e.playlist, e.index, e.track)).collect::<Vec<_>>(),
            [(1, 1, 2), (1, 2, 1), (1, 3, 2), (2, 1, 1)],
            "a track loaded twice is in the list twice"
        );
        let read = read(&bytes).unwrap();
        assert!(read.nodes.is_empty() && read.entries.is_empty(), "a history is not a playlist");
        assert_eq!(
            (read.tracks[1].file_name.as_str(), read.tracks[1].file_size),
            ("b.flac", 55_720_044)
        );
    }

    #[test]
    fn a_drive_no_player_has_written_to_has_no_history() {
        let mut database = written::Database::new();
        database.tracks = vec![a_track(1, "Hold Me", "/Contents/a.flac")];
        assert_eq!(history(&database.to_bytes().unwrap()).unwrap(), Histories::default());
    }

    #[test]
    fn a_file_that_is_not_a_database_is_an_error_rather_than_an_empty_drive() {
        assert!(read(&[0u8; 100]).is_err());
        assert!(read(b"not a database at all, and not page-sized either").is_err());
    }

    #[test]
    fn each_string_form_reads_as_what_it_says() {
        assert_eq!(string_at(&[0x03], 0).unwrap(), "");
        assert_eq!(string_at(&[0x09, b'a', b'b', b'c'], 0).unwrap(), "abc");
        assert_eq!(string_at(&[0x40, 7, 0, 0, b'x', b'y', b'z'], 0).unwrap(), "xyz");
        assert_eq!(string_at(&[0x90, 8, 0, 0, 0xd8, 0x00, 0x41, 0x00], 0).unwrap(), "ØA");
        assert!(string_at(&[0x09, b'a'], 0).is_err(), "runs past its row");
        assert!(string_at(&[0x12, 0, 0, 0], 0).is_err(), "no such kind");
    }
}
