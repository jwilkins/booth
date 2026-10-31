//! A drive to write and two databases to read back, shared between the tests
//! that build whole drives: the field reports and the speed measurements.
//!
//! The legacy database is read with `rekordcrate`, a parser that shares no code
//! with the writer, which is the point: a test that read `export.pdb` with the
//! code that wrote it would agree with itself about everything. The OneLibrary
//! database is read with SQLite, as a player does.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use binrw::BinRead;
use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
use booth_cli::audio::Audio;
use booth_cli::cli::{ExportArgs, InputArgs};
use rekordcrate::pdb::{Header, PageType};

pub const SAMPLE_RATE: u32 = 44_100;
pub const BPM: f64 = 128.0;

/// A directory that is gone when the test is.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-drive-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A track with a beat in it, because the exporter listens for one.
///
/// Kick, hat and a pad, four to the bar, with the drums dropping out for the
/// middle third so there is an arrangement as well as a pulse. `pitch` moves
/// the pad so that two songs are two different files rather than one copied.
pub fn song(seconds: f32, pitch: f32) -> Audio {
    let beat = 60.0 / BPM as f32;
    let beats = (seconds / beat) as usize;
    let len = SAMPLE_RATE as usize / 2 + (SAMPLE_RATE as f32 * seconds) as usize;
    let mut plane = vec![0.0f32; len];
    let two_pi = 2.0 * std::f32::consts::PI;

    for index in 0..beats {
        let quiet = index > beats / 3 && index < beats * 2 / 3;
        let (kick, hat) = if quiet { (0.3f32, 0.0f32) } else { (1.0, 0.25) };
        let start = SAMPLE_RATE as usize / 2 + (SAMPLE_RATE as f32 * beat * index as f32) as usize;
        for i in 0..(SAMPLE_RATE as usize / 4) {
            let at = start + i;
            if at >= len {
                break;
            }
            let t = i as f32 / SAMPLE_RATE as f32;
            plane[at] += 0.5
                * (kick * (-30.0 * t).exp() * (two_pi * 55.0 * t).sin()
                    + hat * (-60.0 * t).exp() * (two_pi * 9_000.0 * t).sin());
        }
    }
    for (i, sample) in plane.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample += 0.15 * (two_pi * pitch * t).sin();
    }
    Audio::new(SAMPLE_RATE, vec![plane.clone(), plane]).unwrap()
}

pub fn write_song(scratch: &Scratch, name: &str) -> PathBuf {
    write_song_of(scratch, name, 10.0)
}

pub fn write_song_of(scratch: &Scratch, name: &str, seconds: f32) -> PathBuf {
    let path = scratch.path(name);
    write_file(&path, &song(seconds, 320.0), Codec::Flac, &EncodeOptions::default()).unwrap();
    path
}

/// The arguments every drive test starts from.
pub fn args_for(inputs: Vec<PathBuf>) -> ExportArgs {
    ExportArgs {
        input: InputArgs { inputs, recursive: false },
        drive: None,
        image: None,
        label: "REKORDBOX".to_string(),
        // The literal is deliberately exhaustive, so a new field is a
        // compile error and somebody has to decide what the tests want.
        player: booth_cli::compat::Player::default(),
        bpm: None,
        playlist: "Sat 14/9".to_string(),
        playlists: Vec::new(),
        dry_run: false,
        no_space_check: false,
        onelibrary_key: None,
        analysed_bits: None,
        no_onelibrary: false,
        companions: Vec::new(),
        already: Vec::new(),
        prepared: Vec::new(),
        settings: Vec::new(),
    }
}

pub const LEGACY: &str = "PIONEER/rekordbox/export.pdb";
pub const ONELIBRARY: &str = "PIONEER/rekordbox/exportLibrary.db";

/// What a player shows about a track, read off a drive's database.
///
/// The same shape from either database, so the two can be compared field by
/// field — which is the check that matters on a drive carrying both.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shown {
    pub id: u32,
    pub title: String,
    pub tempo_x100: u32,
    pub rating: u8,
    pub comment: String,
    pub file_path: String,
    pub analyze_path: String,
}

/// Every row of a table in the legacy database, as `rekordcrate` describes it.
fn legacy_rows(drive: &Path, kind: PageType) -> Vec<String> {
    let bytes = std::fs::read(drive.join(LEGACY)).expect("no legacy database on the drive");
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).expect("rekordcrate could not read the database");
    let table = header.tables.iter().find(|t| t.page_type == kind).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();
    pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
        .map(|row| format!("{row:?}"))
        .collect()
}

/// A field out of a row's description. The parser keeps its fields private,
/// so what it found is read back out of how it prints what it found.
fn string_field(described: &str, name: &str) -> String {
    let marker = format!("{name}: DeviceSQLString(\"");
    let at = described.find(&marker).unwrap_or_else(|| panic!("no {name} in {described}"))
        + marker.len();
    let rest = &described[at..];
    rest[..rest.find('"').unwrap()].to_string()
}

fn number_field(described: &str, marker: &str) -> u64 {
    let at = described.find(marker).unwrap_or_else(|| panic!("no {marker} in {described}"))
        + marker.len();
    let rest = &described[at..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().unwrap()
}

/// The tracks in the legacy database, sorted by id.
pub fn legacy_tracks(drive: &Path) -> Vec<Shown> {
    let mut out: Vec<Shown> = legacy_rows(drive, PageType::Tracks)
        .iter()
        .filter(|row| row.starts_with("Track("))
        .map(|row| Shown {
            id: number_field(row, "id: TrackId(") as u32,
            title: string_field(row, "title"),
            tempo_x100: number_field(row, "tempo: ") as u32,
            rating: number_field(row, "rating: ") as u8,
            comment: string_field(row, "comment"),
            file_path: string_field(row, "file_path"),
            analyze_path: string_field(row, "analyze_path"),
        })
        .collect();
    out.sort();
    out
}

/// The playlist entries in the legacy database: `(playlist name, track id)` in
/// playlist order.
pub fn legacy_playlist_entries(drive: &Path) -> Vec<(String, u32)> {
    let names: std::collections::HashMap<u64, String> = legacy_rows(drive, PageType::PlaylistTree)
        .iter()
        .map(|row| (number_field(row, ", id: PlaylistTreeNodeId("), string_field(row, "name")))
        .collect();
    let mut entries: Vec<(u64, u64, u32)> = legacy_rows(drive, PageType::PlaylistEntries)
        .iter()
        .map(|row| {
            (
                number_field(row, "playlist_id: PlaylistTreeNodeId("),
                number_field(row, "entry_index: "),
                number_field(row, "track_id: TrackId(") as u32,
            )
        })
        .collect();
    entries.sort();
    entries.into_iter().map(|(list, _, track)| (names[&list].clone(), track)).collect()
}

/// The tracks in the OneLibrary database, sorted by id.
pub fn onelibrary_tracks(drive: &Path, key: &str) -> Vec<Shown> {
    let connection = booth_cli::rekordbox::open(&drive.join(ONELIBRARY), key)
        .expect("the OneLibrary database would not open");
    let mut statement = connection
        .prepare(
            "SELECT content_id, title, bpmx100, rating, djComment, path, analysisDataFilePath \
             FROM content",
        )
        .unwrap();
    let mut out: Vec<Shown> = statement
        .query_map([], |row| {
            Ok(Shown {
                id: row.get::<_, i64>(0)? as u32,
                title: row.get(1)?,
                tempo_x100: row.get::<_, i64>(2)? as u32,
                rating: row.get::<_, i64>(3)? as u8,
                comment: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                file_path: row.get(5)?,
                analyze_path: row.get(6)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    out.sort();
    out
}

/// The playlist entries in the OneLibrary database, in playlist order.
pub fn onelibrary_playlist_entries(drive: &Path, key: &str) -> Vec<(String, u32)> {
    let connection = booth_cli::rekordbox::open(&drive.join(ONELIBRARY), key)
        .expect("the OneLibrary database would not open");
    let mut statement = connection
        .prepare(
            "SELECT playlist.name, entry.content_id FROM playlist_content AS entry \
             JOIN playlist ON playlist.playlist_id = entry.playlist_id \
             ORDER BY entry.playlist_id, entry.sequenceNo",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u32)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Every analysis file the database names, in both generations, and whether
/// each is on the drive.
pub fn analysis_files(drive: &Path) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for track in legacy_tracks(drive) {
        let dat = track.analyze_path.trim_start_matches('/').to_string();
        for name in [dat.clone(), dat.replace(".DAT", ".EXT"), dat.replace(".DAT", ".2EX")] {
            let there = drive.join(&name).exists();
            out.push((name, there));
        }
    }
    out
}
