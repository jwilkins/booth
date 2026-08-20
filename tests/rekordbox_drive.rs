//! Building a whole drive and then checking it the way a player would.
//!
//! The unit tests prove each file is well formed. This proves the pieces point
//! at each other: that the database's idea of where a track's audio and
//! analysis live matches where they were actually written. A drive can be made
//! of perfectly valid files and still be useless if those two disagree.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use binrw::BinRead;
use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;
use musicai::cli::{ExportArgs, InputArgs};
use musicai::commands;
use musicai::report::Collected;
use rekordcrate::pdb::{Header, PageType, Row};

const SAMPLE_RATE: u32 = 44_100;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-drive-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Four seconds with something in every frequency band.
fn song() -> Audio {
    let frames = SAMPLE_RATE as usize * 4;
    let plane: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let two_pi = 2.0 * std::f32::consts::PI;
            0.5 * (two_pi * 60.0 * t).sin()
                + 0.25 * (two_pi * 700.0 * t).sin()
                + 0.1 * (two_pi * 9_000.0 * t).sin()
        })
        .collect();
    Audio::new(SAMPLE_RATE, vec![plane.clone(), plane]).unwrap()
}

fn write_song(scratch: &Scratch, name: &str) -> PathBuf {
    let path = scratch.path(name);
    write_file(&path, &song(), Codec::Flac, &EncodeOptions::default()).unwrap();
    path
}

fn export(inputs: Vec<PathBuf>, drive: &Path) -> Vec<String> {
    let args = ExportArgs {
        input: InputArgs { inputs, recursive: false },
        drive: drive.to_path_buf(),
        bpm: 128.0,
        first_beat: 412,
        playlist: "Sat 14/9".to_string(),
        dry_run: false,
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export failed");
    reporter.lines()
}

/// The tracks in the drive's database, as `(id, file path, analysis path)`.
fn tracks_on(drive: &Path) -> Vec<(u32, String, String)> {
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).expect("rekordcrate could not read the database");

    let table = header.tables.iter().find(|t| t.page_type == PageType::Tracks).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();

    let mut out = Vec::new();
    for row in pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
    {
        let Row::Track(track) = row else { continue };
        // The row's fields are private to the parser, so read them back out of
        // its own description of what it found.
        let described = format!("{track:?}");
        let field = |name: &str| -> String {
            let marker = format!("{name}: DeviceSQLString(\"");
            let at = described.find(&marker).unwrap_or_else(|| panic!("no {name} in {described}"))
                + marker.len();
            let rest = &described[at..];
            rest[..rest.find('"').unwrap()].to_string()
        };
        let id_at = described.find("id: TrackId(").unwrap() + 12;
        let id: u32 = described[id_at..][..described[id_at..].find(')').unwrap()].parse().unwrap();
        out.push((id, field("file_path"), field("analyze_path")));
    }
    out.sort();
    out
}

#[test]
fn the_database_points_at_files_that_are_really_there() {
    let scratch = Scratch::new("complete");
    let drive = scratch.path("USB");
    let inputs = vec![write_song(&scratch, "one.flac"), write_song(&scratch, "two.flac")];

    export(inputs, &drive);

    let tracks = tracks_on(&drive);
    assert_eq!(tracks.len(), 2);

    for (id, file_path, analyze_path) in &tracks {
        // Every path in the database is absolute from the root of the drive,
        // which is how a player reads them.
        assert!(file_path.starts_with("/Contents/"), "track {id}: {file_path}");
        assert!(analyze_path.starts_with("/PIONEER/USBANLZ/"), "track {id}: {analyze_path}");

        let audio = drive.join(file_path.trim_start_matches('/'));
        assert!(audio.is_file(), "track {id}: no audio at {}", audio.display());

        // The database names the .DAT; the player looks for its two siblings
        // alongside it, so all three have to be there.
        let dat = drive.join(analyze_path.trim_start_matches('/'));
        assert!(dat.is_file(), "track {id}: no analysis at {}", dat.display());
        for extension in ["EXT", "2EX"] {
            let sibling = dat.with_extension(extension);
            assert!(sibling.is_file(), "track {id}: no {extension} beside the DAT");
        }
    }
}

#[test]
fn the_analysis_on_the_drive_agrees_with_the_database() {
    let scratch = Scratch::new("agrees");
    let drive = scratch.path("USB");
    export(vec![write_song(&scratch, "one.flac")], &drive);

    let (_, file_path, analyze_path) = tracks_on(&drive).remove(0);
    let dat = drive.join(analyze_path.trim_start_matches('/'));
    let sections = musicai::export::anlz::inspect(&std::fs::read(&dat).unwrap()).unwrap();

    // rekordcrate cannot be the reader here: an exported drive carries a memory
    // cue at the start of every track, and that hits the cue-type disagreement
    // recorded in `rekordbox_export.rs`. Section-by-section validation against
    // the independent parser lives in that file; what this one checks is that
    // the two halves of the drive agree with each other.
    let summary = |fourcc: &str| {
        sections
            .iter()
            .find(|s| s.fourcc == fourcc)
            .unwrap_or_else(|| panic!("no {fourcc} section in {}", dat.display()))
            .summary
            .clone()
    };

    // The analysis file carries its own copy of where the audio is, and the two
    // must agree or the player draws one track's waveform over another's.
    assert_eq!(summary("PPTH"), file_path);
    // Four seconds at 128 BPM from 412 ms in.
    assert_eq!(summary("PQTZ"), "8 beats");
}

#[test]
fn the_playlist_holds_every_exported_track() {
    let scratch = Scratch::new("playlist");
    let drive = scratch.path("USB");
    let inputs: Vec<PathBuf> =
        (1..=3).map(|i| write_song(&scratch, &format!("track{i}.flac"))).collect();
    export(inputs, &drive);

    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let tables = musicai::export::pdb::inspect(&bytes).unwrap();
    let rows = |name: &str| tables.iter().find(|t| t.table == name).unwrap().rows;

    assert_eq!(rows("Tracks"), 3);
    assert_eq!(rows("PlaylistTree"), 1);
    assert_eq!(rows("PlaylistEntries"), 3);
}

#[test]
fn a_dry_run_writes_nothing() {
    let scratch = Scratch::new("dry");
    let drive = scratch.path("USB");
    let args = ExportArgs {
        input: InputArgs { inputs: vec![write_song(&scratch, "one.flac")], recursive: false },
        drive: drive.clone(),
        bpm: 128.0,
        first_beat: 0,
        playlist: "test".to_string(),
        dry_run: true,
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).unwrap();

    assert!(!drive.exists(), "a dry run created {}", drive.display());
    assert!(reporter.lines().iter().any(|l| l.contains("would write")));
}

#[test]
fn a_file_a_player_cannot_open_is_refused_rather_than_copied() {
    let scratch = Scratch::new("unplayable");
    let drive = scratch.path("USB");
    // 192 kHz is past what any current player accepts.
    let fast = Audio::new(192_000, vec![vec![0.1; 192_000]]).unwrap();
    let path = scratch.path("fast.wav");
    write_file(&path, &fast, Codec::Wav, &EncodeOptions::default()).unwrap();

    let args = ExportArgs {
        input: InputArgs { inputs: vec![path], recursive: false },
        drive: drive.clone(),
        bpm: 128.0,
        first_beat: 0,
        playlist: "test".to_string(),
        dry_run: false,
    };
    let error = commands::export(&args, &Collected::new()).unwrap_err().to_string();
    assert!(error.contains("1 of 1 files failed"), "{error}");
    assert!(!drive.join("Contents").exists(), "the unplayable file was copied anyway");
}
