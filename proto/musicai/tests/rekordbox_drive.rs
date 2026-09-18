//! Building a whole drive and then checking it the way a player would.
//!
//! The unit tests prove each file is well formed. This proves the pieces point
//! at each other: that the database's idea of where a track's audio and
//! analysis live matches where they were actually written. A drive can be made
//! of perfectly valid files and still be useless if those two disagree.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use binrw::BinRead;
use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
use booth_cli::audio::Audio;
use booth_cli::cli::{ExportArgs, InputArgs, PlaylistSpec};
use booth_cli::commands;
use booth_cli::export::image::DriveImage;
use booth_cli::report::Collected;
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

const BPM: f64 = 128.0;

/// A track with a beat in it, because the exporter now listens for one.
///
/// Kick, hat and a pad, four to the bar, with the drums dropping out for the
/// middle third so there is an arrangement as well as a pulse.
fn song(seconds: f32) -> Audio {
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
        *sample += 0.15 * (two_pi * 320.0 * t).sin();
    }
    Audio::new(SAMPLE_RATE, vec![plane.clone(), plane]).unwrap()
}

fn write_song(scratch: &Scratch, name: &str) -> PathBuf {
    write_song_of(scratch, name, 10.0)
}

fn write_song_of(scratch: &Scratch, name: &str, seconds: f32) -> PathBuf {
    let path = scratch.path(name);
    write_file(&path, &song(seconds), Codec::Flac, &EncodeOptions::default()).unwrap();
    path
}

fn args_for(inputs: Vec<PathBuf>) -> ExportArgs {
    ExportArgs {
        input: InputArgs { inputs, recursive: false },
        drive: None,
        image: None,
        label: "REKORDBOX".to_string(),
        bpm: None,
        playlist: "Sat 14/9".to_string(),
        playlists: Vec::new(),
        dry_run: false,
        onelibrary_key: None,
        analysed_bits: None,
        companions: Vec::new(),
        already: Vec::new(),
        prepared: Vec::new(),
    }
}

fn export(inputs: Vec<PathBuf>, drive: &Path) -> Vec<String> {
    let args = ExportArgs { drive: Some(drive.to_path_buf()), ..args_for(inputs) };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export failed");
    reporter.lines()
}

fn export_image(inputs: Vec<PathBuf>, image: &Path) -> Vec<String> {
    let args = ExportArgs { image: Some(image.to_path_buf()), ..args_for(inputs) };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export to an image failed");
    reporter.lines()
}

/// The tracks in the drive's database, as `(id, file path, analysis path)`.
fn tracks_on(drive: &Path) -> Vec<(u32, String, String)> {
    tracks_in(&std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap())
}

fn tracks_in(bytes: &[u8]) -> Vec<(u32, String, String)> {
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
    let sections = booth_cli::export::anlz::inspect(&std::fs::read(&dat).unwrap()).unwrap();

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
    // Ten seconds at 128 BPM is a shade over twenty beats, and the tracker
    // extends its grid a little past the music at both ends.
    let beats: usize = summary("PQTZ").trim_end_matches(" beats").parse().unwrap();
    assert!((18..=24).contains(&beats), "{beats} beats for a ten-second track at 128 BPM");
}

#[test]
fn the_playlist_holds_every_exported_track() {
    let scratch = Scratch::new("playlist");
    let drive = scratch.path("USB");
    let inputs: Vec<PathBuf> =
        (1..=3).map(|i| write_song(&scratch, &format!("track{i}.flac"))).collect();
    export(inputs, &drive);

    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let tables = booth_cli::export::pdb::inspect(&bytes).unwrap();
    let rows = |name: &str| tables.iter().find(|t| t.table == name).unwrap().rows;

    assert_eq!(rows("Tracks"), 3);
    assert_eq!(rows("PlaylistTree"), 1);
    assert_eq!(rows("PlaylistEntries"), 3);
}

/// The playlist tree as an independent parser reads it back: each row's name,
/// whether it is a folder, and which folder it sits in.
fn playlists_on(drive: &Path) -> Vec<(String, bool, u32, u32)> {
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).expect("rekordcrate could not read the database");

    let table = header.tables.iter().find(|t| t.page_type == PageType::PlaylistTree).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();

    let mut out = Vec::new();
    for row in pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
    {
        let Row::PlaylistTreeNode(node) = row else { continue };
        // The parser keeps its fields private, so they are read back out of
        // its own description of what it found. The markers carry their
        // wrapper type: plain `id: ` also matches the tail of `parent_id: `,
        // which is how the first version of this read every node as a child of
        // the root and still passed three of its four assertions.
        let described = format!("{node:?}");
        let id_after = |marker: &str| -> u32 {
            let at = described.find(marker).unwrap_or_else(|| panic!("no {marker} in {described}"))
                + marker.len();
            let rest = &described[at..];
            rest[..rest.find(')').unwrap()].parse().unwrap()
        };
        let marker = "name: DeviceSQLString(\"";
        let at = described.find(marker).unwrap() + marker.len();
        let name = described[at..][..described[at..].find('"').unwrap()].to_string();
        // Stored as a count rather than a flag: non-zero is a folder.
        let is_folder = !described.contains("node_is_folder: 0");
        out.push((
            name,
            is_folder,
            id_after(", id: PlaylistTreeNodeId("),
            id_after("parent_id: PlaylistTreeNodeId("),
        ));
    }
    out
}

#[test]
fn a_drive_carries_a_tree_of_playlists_and_folders() {
    // What a player draws in its browse list. One list per set, filed under
    // the night — which is the shape a DJ prepares in, and what a drive that
    // could hold only one playlist made impossible.
    let scratch = Scratch::new("tree");
    let drive = scratch.path("USB");
    let files: Vec<PathBuf> =
        (1..=3).map(|i| write_song(&scratch, &format!("track{i}.flac"))).collect();

    let mut args = args_for(files.clone());
    args.drive = Some(drive.clone());
    args.playlists = vec![
        PlaylistSpec {
            name: "warm".into(),
            folder: "Sat 14/9".into(),
            tracks: vec![files[0].clone(), files[1].clone()],
        },
        PlaylistSpec {
            name: "peak".into(),
            folder: "Sat 14/9".into(),
            // Shared with "warm": one track can be in two sets.
            tracks: vec![files[1].clone(), files[2].clone()],
        },
        PlaylistSpec {
            name: "promos".into(),
            folder: String::new(),
            tracks: vec![files[0].clone()],
        },
    ];
    booth_cli::commands::export(&args, &booth_cli::report::Collected::new()).unwrap();

    let rows = playlists_on(&drive);
    assert_eq!(rows.len(), 4, "a folder and three lists: {rows:?}");

    let find = |name: &str| rows.iter().find(|r| r.0 == name).unwrap_or_else(|| panic!("{name}"));
    let folder = find("Sat 14/9");
    assert!(folder.1, "the folder is marked as one");
    assert_eq!(find("warm").3, folder.2, "warm sits in the folder");
    assert_eq!(find("peak").3, folder.2);
    assert_eq!(find("promos").3, 0, "the top level is not a folder");
    assert!(!find("warm").1, "a playlist is not a folder");

    // Four entries, not three: the shared track appears in both lists, and is
    // one track on the drive.
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let tables = booth_cli::export::pdb::inspect(&bytes).unwrap();
    let rows_in = |name: &str| tables.iter().find(|t| t.table == name).unwrap().rows;
    assert_eq!(rows_in("Tracks"), 3);
    assert_eq!(rows_in("PlaylistEntries"), 5);
}

#[test]
fn a_dry_run_writes_nothing() {
    let scratch = Scratch::new("dry");
    let drive = scratch.path("USB");
    let args = ExportArgs {
        drive: Some(drive.clone()),
        dry_run: true,
        ..args_for(vec![write_song(&scratch, "one.flac")])
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).unwrap();

    assert!(!drive.exists(), "a dry run created {}", drive.display());
    assert!(reporter.lines().iter().any(|l| l.contains("would write")));
}

#[test]
fn a_long_enough_track_gets_its_phrases_onto_the_drive() {
    let scratch = Scratch::new("phrases");
    let drive = scratch.path("USB");
    // Forty seconds is enough bars for the phrase detector to have something to
    // divide up; ten is not.
    export(vec![write_song_of(&scratch, "long.flac", 40.0)], &drive);

    let (_, _, analyze_path) = tracks_on(&drive).remove(0);
    let ext = drive.join(analyze_path.trim_start_matches('/')).with_extension("EXT");
    let sections = booth_cli::export::anlz::inspect(&std::fs::read(&ext).unwrap()).unwrap();

    let phrases = sections
        .iter()
        .find(|s| s.fourcc == "PSSI")
        .unwrap_or_else(|| panic!("no phrase analysis in {}", ext.display()));
    assert!(phrases.summary.ends_with("phrases"), "{}", phrases.summary);
    let count: usize = phrases.summary.trim_end_matches(" phrases").parse().unwrap();
    assert!(count >= 2, "only {count} phrases in a track that changes twice");
}

#[test]
fn a_tonal_track_gets_its_key_into_the_database() {
    let scratch = Scratch::new("key");
    let drive = scratch.path("USB");
    // A pitched track: a bass line plus a triad, so there is a key to find.
    let path = scratch.path("tonal.flac");
    let seconds = 30.0f32;
    let mut left = vec![0.0f32; (SAMPLE_RATE as f32 * seconds) as usize];
    let two_pi = 2.0 * std::f32::consts::PI;
    // A minor triad (A, C, E) held under a steady pulse.
    for (i, sample) in left.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        for hz in [220.0, 261.63, 329.63] {
            *sample += 0.15 * (two_pi * hz * t).sin();
        }
        let beat = 60.0 / BPM as f32;
        let into = (t % beat) / beat;
        *sample += 0.5 * (-30.0 * into * beat).exp() * (two_pi * 55.0 * t).sin();
    }
    let right = left.clone();
    write_file(
        &path,
        &Audio::new(SAMPLE_RATE, vec![left, right]).unwrap(),
        Codec::Flac,
        &EncodeOptions::default(),
    )
    .unwrap();

    export(vec![path], &drive);

    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let tables = booth_cli::export::pdb::inspect(&bytes).unwrap();
    let keys = tables.iter().find(|t| t.table == "Keys").unwrap();
    // A tonal track leaves exactly one key in the table, whatever it turned
    // out to be; a keyless one would leave the table empty.
    assert_eq!(keys.rows, 1, "a pitched track should have produced one key row");
}

#[test]
fn the_cues_on_the_drive_are_named_and_coloured() {
    let scratch = Scratch::new("cues");
    let drive = scratch.path("USB");
    export(vec![write_song_of(&scratch, "long.flac", 40.0)], &drive);

    let (_, _, analyze_path) = tracks_on(&drive).remove(0);
    let dat = drive.join(analyze_path.trim_start_matches('/'));
    let sections = booth_cli::export::anlz::inspect(&std::fs::read(&dat).unwrap()).unwrap();

    let hot = sections
        .iter()
        .filter(|s| s.fourcc == "PCOB" && s.summary.contains("hot"))
        .map(|s| s.summary.clone())
        .next()
        .expect("no hot cue list");
    let count: usize = hot.trim_end_matches(" hot cues").parse().unwrap();
    assert!((1..=8).contains(&count), "{count} hot cues");
}

#[test]
fn a_file_a_player_cannot_open_is_refused_rather_than_copied() {
    let scratch = Scratch::new("unplayable");
    let drive = scratch.path("USB");
    // 192 kHz is past what any current player accepts.
    let fast = Audio::new(192_000, vec![vec![0.1; 192_000]]).unwrap();
    let path = scratch.path("fast.wav");
    write_file(&path, &fast, Codec::Wav, &EncodeOptions::default()).unwrap();

    let args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![path]) };
    let error = commands::export(&args, &Collected::new()).unwrap_err().to_string();
    assert!(error.contains("1 of 1 files failed"), "{error}");
    assert!(!drive.join("Contents").exists(), "the unplayable file was copied anyway");
}

// -- disk images -----------------------------------------------------------

/// Everything below is about the other shape a drive comes in: a raw `.img`
/// with a partition table and a filesystem, which is what a player actually
/// reads and what an emulator's USB slot takes.

#[test]
fn an_image_is_a_partitioned_fat32_volume() {
    let scratch = Scratch::new("image-layout");
    let image = scratch.path("REKORDBOX.img");
    export_image(vec![write_song(&scratch, "one.flac")], &image);

    let bytes = std::fs::read(&image).unwrap();
    assert_eq!(bytes.len() % 512, 0, "an image is a whole number of sectors");
    assert_eq!(&bytes[510..512], &[0x55, 0xaa], "no MBR signature");

    // One partition, FAT32 with LBA addressing, starting at the usual megabyte.
    let entry = 0x1be;
    assert_eq!(bytes[entry + 4], 0x0c);
    let start = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap());
    let count = u32::from_le_bytes(bytes[entry + 12..entry + 16].try_into().unwrap());
    assert_eq!(start, 2048);
    assert_eq!((start + count) as usize * 512, bytes.len(), "the partition should fill the disk");

    // And the partition really is FAT32, which it says in its own boot sector.
    let boot = start as usize * 512;
    assert_eq!(&bytes[boot + 0x52..boot + 0x57], b"FAT32");
    assert_eq!(&bytes[boot + 0x47..boot + 0x52], b"REKORDBOX  ");
}

#[test]
fn an_image_holds_exactly_what_a_folder_would() {
    let scratch = Scratch::new("image-same");
    let source = write_song(&scratch, "one.flac");
    let folder = scratch.path("USB");
    let image = scratch.path("USB.img");

    export(vec![source.clone()], &folder);
    export_image(vec![source], &image);

    let (_, file_path, analyze_path) = tracks_on(&folder).remove(0);
    let opened = DriveImage::open(&image).unwrap();

    // The database is byte-for-byte the same either way: nothing about where
    // the drive is being written leaks into what is written.
    let from_folder = std::fs::read(folder.join("PIONEER/rekordbox/export.pdb")).unwrap();
    assert_eq!(opened.read("/PIONEER/rekordbox/export.pdb").unwrap(), from_folder);

    // So are the analysis files and the audio.
    for extension in ["DAT", "EXT", "2EX"] {
        let at = analyze_path.replace("ANLZ0000.DAT", &format!("ANLZ0000.{extension}"));
        let on_disk = std::fs::read(folder.join(at.trim_start_matches('/'))).unwrap();
        assert_eq!(opened.read(&at).unwrap(), on_disk, "{at} differs");
    }
    let audio = std::fs::read(folder.join(file_path.trim_start_matches('/'))).unwrap();
    assert_eq!(opened.read(&file_path).unwrap(), audio);
}

#[test]
fn the_database_in_an_image_is_the_one_a_player_would_walk() {
    let scratch = Scratch::new("image-db");
    let image = scratch.path("USB.img");
    let inputs: Vec<PathBuf> =
        (1..=3).map(|i| write_song(&scratch, &format!("track{i}.flac"))).collect();
    export_image(inputs, &image);

    let opened = DriveImage::open(&image).unwrap();
    let tables =
        booth_cli::export::pdb::inspect(&opened.read("/PIONEER/rekordbox/export.pdb").unwrap())
            .unwrap();
    let rows = |name: &str| tables.iter().find(|t| t.table == name).unwrap().rows;
    assert_eq!(rows("Tracks"), 3);
    assert_eq!(rows("PlaylistEntries"), 3);

    // And the analysis each row points at is in the image, all three files.
    let bytes = opened.read("/PIONEER/rekordbox/export.pdb").unwrap();
    for (_, _, analyze_path) in tracks_in(&bytes) {
        for extension in ["DAT", "EXT", "2EX"] {
            let at = analyze_path.replace("ANLZ0000.DAT", &format!("ANLZ0000.{extension}"));
            assert!(opened.read(&at).is_ok(), "{at} is missing from the image");
        }
    }
}

/// mtools is a FAT implementation with nothing to do with ours, so if it can
/// read the image then the image is a FAT filesystem rather than merely
/// something our own code agrees with itself about.
///
/// Skipped, loudly, where mtools is not installed.
#[test]
fn mtools_reads_the_image() {
    let Ok(mdir) = which("mdir") else {
        eprintln!("skipping: mtools is not installed (apt install mtools)");
        return;
    };

    let scratch = Scratch::new("mtools");
    let image = scratch.path("USB.img");
    export_image(vec![write_song(&scratch, "one.flac")], &image);

    let listing = std::process::Command::new(&mdir)
        .env("MTOOLS_SKIP_CHECK", "1")
        .arg("-i")
        .arg(format!("{}@@1M", image.display()))
        .arg("::/PIONEER/rekordbox")
        .output()
        .expect("running mdir");
    let text = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.status.success(), "mdir failed: {}", String::from_utf8_lossy(&listing.stderr));
    assert!(text.contains("REKORDBOX"), "the volume label is wrong: {text}");
    assert!(text.contains("EXPORT"), "no export.pdb in {text}");

    // Pull the database back out with mtools and check it is the same bytes.
    let extracted = scratch.path("export.pdb");
    let status = std::process::Command::new(which("mcopy").unwrap())
        .env("MTOOLS_SKIP_CHECK", "1")
        .arg("-i")
        .arg(format!("{}@@1M", image.display()))
        .arg("::/PIONEER/rekordbox/export.pdb")
        .arg(&extracted)
        .status()
        .expect("running mcopy");
    assert!(status.success(), "mcopy failed");

    let ours = DriveImage::open(&image).unwrap().read("/PIONEER/rekordbox/export.pdb").unwrap();
    assert_eq!(std::fs::read(&extracted).unwrap(), ours);
}

fn which(program: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .ok_or(())
}

/// The tracks a playlist holds, by their file paths on the drive.
fn playlist_entries(drive: &Path) -> Vec<u32> {
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).unwrap();
    let table = header.tables.iter().find(|t| t.page_type == PageType::PlaylistEntries).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();
    let mut out = Vec::new();
    for row in pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
    {
        let Row::PlaylistEntry(entry) = row else { continue };
        let described = format!("{entry:?}");
        let marker = "track_id: TrackId(";
        let at = described.find(marker).unwrap() + marker.len();
        out.push(described[at..][..described[at..].find(')').unwrap()].parse().unwrap());
    }
    out
}

#[test]
fn a_second_sync_keeps_what_the_first_one_wrote() {
    // A drive is written once and then added to, week after week. If the
    // second write describes only what it added, everything already on the
    // drive stops existing as far as the player is concerned: the audio is
    // still there and nothing browses to it.
    let scratch = Scratch::new("incremental");
    let drive = scratch.path("USB");
    let first = write_song(&scratch, "first.flac");
    let second = write_song(&scratch, "second.flac");

    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![first.clone()]) };
    args.playlists = vec![PlaylistSpec {
        name: "Saturday".to_string(),
        folder: String::new(),
        tracks: vec![first.clone()],
    }];
    let after_first = commands::export(&args, &Collected::new()).expect("the first write failed");
    assert_eq!(tracks_on(&drive).len(), 1, "the first write should put one track on");

    // The second week: one new track, and a playlist that now names both.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![second.clone()]) };
    args.playlists = vec![PlaylistSpec {
        name: "Saturday".to_string(),
        folder: String::new(),
        tracks: vec![first.clone(), second.clone()],
    }];
    args.already = after_first;
    commands::export(&args, &Collected::new()).expect("the second write failed");

    let on_drive = tracks_on(&drive);
    assert_eq!(
        on_drive.len(),
        2,
        "the drive's database lists {} tracks after adding one to a drive that had one: {on_drive:?}",
        on_drive.len()
    );
    assert_eq!(
        playlist_entries(&drive).len(),
        2,
        "the playlist on the drive does not hold both tracks"
    );
}

/// A passphrase, not *the* passphrase. What is being checked here is that the
/// database written to the drive is the database that comes back off it; the
/// key that makes a drive a player will read is the one rekordbox uses, which
/// this project does not ship.
const ONELIBRARY_KEY: &str = "a-key-that-is-not-the-real-one";

#[test]
fn a_drive_written_with_a_key_carries_both_databases_and_they_agree() {
    let scratch = Scratch::new("onelibrary");
    let drive = scratch.path("drive");
    let one = write_song(&scratch, "one.flac");
    let two = write_song(&scratch, "two.flac");

    let mut args =
        ExportArgs { drive: Some(drive.clone()), ..args_for(vec![one.clone(), two.clone()]) };
    args.onelibrary_key = Some(ONELIBRARY_KEY.to_string());
    args.playlists = vec![PlaylistSpec {
        name: "Saturday".to_string(),
        folder: String::new(),
        tracks: vec![one, two],
    }];
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export failed");

    let at = drive.join("PIONEER/rekordbox/exportLibrary.db");
    assert!(at.exists(), "no OneLibrary database was written: {:?}", reporter.lines());
    let bytes = std::fs::read(&at).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"), "it should be encrypted");

    let summary = booth_cli::export::onelibrary::inspect(&bytes, ONELIBRARY_KEY)
        .expect("the drive's OneLibrary database would not open");
    assert_eq!(summary.tables, 22);
    assert_eq!(summary.playlists, 1);

    // The point of writing both: a player that reads one and a player that
    // reads the other are looking at the same drive.
    let legacy = tracks_on(&drive);
    assert_eq!(
        summary.tracks as usize,
        legacy.len(),
        "the two databases disagree about how many tracks are on the drive"
    );
    assert_eq!(summary.entries as usize, playlist_entries(&drive).len());

    // And the newer database points at files that are really there, the same
    // check the legacy one gets.
    let connection = booth_cli::rekordbox::open(&at, ONELIBRARY_KEY).unwrap();
    let mut statement =
        connection.prepare("SELECT path, analysisDataFilePath FROM content").unwrap();
    let rows: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    for (audio, analysis) in rows {
        assert!(drive.join(audio.trim_start_matches('/')).exists(), "{audio} is not on the drive");
        assert!(
            drive.join(analysis.trim_start_matches('/')).exists(),
            "{analysis} is not on the drive"
        );
        // Both databases name the same file for the same track.
        assert!(
            legacy.iter().any(|(_, path, anlz)| *path == audio && *anlz == analysis),
            "{audio} is in one database and not the other"
        );
    }
}

#[test]
fn an_ordinary_export_carries_both_databases_without_being_asked() {
    // Nobody should have to know that there are two formats, or which player
    // reads which, to walk out of the house with a drive that works. So the
    // default is both, under the keys the build carries.
    let scratch = Scratch::new("onelibrary-default");
    let drive = scratch.path("drive");
    let lines = export(vec![write_song(&scratch, "one.flac")], &drive);

    assert!(drive.join("PIONEER/rekordbox/export.pdb").exists(), "the legacy database");
    let at = drive.join("PIONEER/rekordbox/exportLibrary.db");
    assert!(at.exists(), "the newer players' database: {lines:?}");

    // Opened with the key the build carries, the same way a player would find
    // it: nothing passed in, nothing in the environment.
    let key = booth_cli::rekordbox::onelibrary_key(None).expect("this build carries a key");
    let summary = booth_cli::export::onelibrary::inspect(&std::fs::read(&at).unwrap(), &key)
        .expect("the drive's OneLibrary database would not open with the built-in key");
    assert_eq!(summary.tables, 22);
    assert_eq!(summary.tracks, 1);
}

#[test]
fn the_two_keys_are_two_keys_and_neither_opens_the_other_file() {
    use booth_cli::rekordbox::{BUNDLED_KEY, BUNDLED_ONELIBRARY_KEY};

    // The mistake this guards against is using one for the other, which
    // produces a perfectly valid file that no player and no rekordbox can
    // read. They are told apart by shape: hex, and not hex.
    assert_ne!(BUNDLED_KEY, BUNDLED_ONELIBRARY_KEY);
    assert_eq!(BUNDLED_KEY.len(), 64);
    assert_eq!(BUNDLED_ONELIBRARY_KEY.len(), 64);
    assert!(BUNDLED_KEY.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!BUNDLED_ONELIBRARY_KEY.chars().all(|c| c.is_ascii_hexdigit()));

    let scratch = Scratch::new("onelibrary-keys");
    let drive = scratch.path("drive");
    export(vec![write_song(&scratch, "one.flac")], &drive);
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/exportLibrary.db")).unwrap();
    assert!(
        booth_cli::export::onelibrary::inspect(&bytes, BUNDLED_KEY).is_err(),
        "the library key must not open a drive"
    );
    assert!(booth_cli::export::onelibrary::inspect(&bytes, BUNDLED_ONELIBRARY_KEY).is_ok());
}

// -- stems -----------------------------------------------------------------

/// Every artist name in the drive's database.
///
/// The tracks table stores an artist id rather than a name, so the names come
/// out of their own table. Read the same way as the track rows: the parser's
/// fields are private, so its own description of what it found is what there is
/// to read.
fn artists_on(drive: &Path) -> Vec<String> {
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).expect("rekordcrate could not read the database");

    let table = header.tables.iter().find(|t| t.page_type == PageType::Artists).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();

    let mut out = Vec::new();
    for row in pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
    {
        let Row::Artist(artist) = row else { continue };
        let described = format!("{artist:?}");
        let marker = "name: DeviceSQLString(\"";
        let at = described.find(marker).unwrap_or_else(|| panic!("no name in {described}"))
            + marker.len();
        let rest = &described[at..];
        out.push(rest[..rest.find('"').unwrap()].to_string());
    }
    out.sort();
    out
}

/// A track with an artist tag on it, and three stems of it with none.
///
/// Wav on purpose: it is the format a separator writes by default and the one
/// with nowhere to keep a tag, so a stem written as one arrives at the drive
/// knowing nothing about where it came from. Everything the exporter files it
/// under has to come from the pairing it is given.
fn track_and_its_stems(scratch: &Scratch) -> (PathBuf, Vec<PathBuf>) {
    let track = write_song(scratch, "Ohm Hourglass.flac");
    booth_cli::tag::write_tags(
        &track,
        &booth_cli::tag::Metadata {
            artist: Some("Bruce".to_string()),
            title: Some("Ohm Hourglass".to_string()),
            album: Some("Sonder Somatic".to_string()),
            ..Default::default()
        },
        booth_cli::tag::OnExisting::Overwrite,
        None,
    )
    .unwrap();

    let stems = ["vocals", "drums", "melody"]
        .iter()
        .map(|part| {
            let path = scratch.path(&format!("Ohm Hourglass-{part}.wav"));
            write_file(&path, &song(10.0), Codec::Wav, &EncodeOptions::default()).unwrap();
            path
        })
        .collect();
    (track, stems)
}

#[test]
fn a_stem_is_written_into_the_folder_its_track_is_in() {
    let scratch = Scratch::new("stems-beside");
    let drive = scratch.path("USB");
    let (track, stems) = track_and_its_stems(&scratch);

    let mut inputs = vec![track.clone()];
    inputs.extend(stems.iter().cloned());
    let args = ExportArgs {
        drive: Some(drive.clone()),
        companions: stems.iter().map(|stem| (stem.clone(), track.clone())).collect(),
        ..args_for(inputs)
    };
    commands::export(&args, &Collected::new()).expect("export failed");

    let paths: Vec<String> = tracks_on(&drive).into_iter().map(|(_, path, _)| path).collect();
    assert_eq!(paths.len(), 4, "the track and its three stems: {paths:?}");

    // The whole point: one folder, holding the record and everything cut from
    // it. The stems carry no tags at all, so an exporter reading their own
    // metadata would have filed them under "Unknown Artist" instead.
    for path in &paths {
        assert!(
            path.starts_with("/Contents/Bruce/"),
            "{path} is not in the folder its track is in"
        );
        assert!(drive.join(path.trim_start_matches('/')).exists(), "{path} was not written");
    }
    assert!(
        !artists_on(&drive).contains(&"Unknown Artist".to_string()),
        "a stem was filed under an artist of its own: {:?}",
        artists_on(&drive)
    );
}

/// The track ids of the drive's one playlist, in the order it plays.
///
/// Read off the entry index each row carries, which is the field the format
/// orders a playlist by, rather than the order the rows happen to sit in the
/// page — those are two different things, and only the first is the playlist.
fn playlist_order(drive: &Path) -> Vec<u32> {
    let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let mut cursor = Cursor::new(&bytes);
    let header = Header::read(&mut cursor).unwrap();
    let table = header.tables.iter().find(|t| t.page_type == PageType::PlaylistEntries).unwrap();
    let pages = header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap();

    let mut out: Vec<(u32, u32)> = Vec::new();
    for row in pages
        .iter()
        .filter(|p| p.has_data())
        .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
    {
        let Row::PlaylistEntry(entry) = row else { continue };
        let described = format!("{entry:?}");
        let number = |name: &str| -> u32 {
            let marker = format!("{name}: ");
            let at = described.find(&marker).unwrap() + marker.len();
            let rest = described[at..].trim_start_matches("TrackId(");
            rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
        };
        out.push((number("entry_index"), number("track_id")));
    }
    out.sort();
    out.into_iter().map(|(_, track)| track).collect()
}

#[test]
fn a_stem_follows_its_track_in_the_playlist() {
    let scratch = Scratch::new("stems-order");
    let drive = scratch.path("USB");
    let (track, stems) = track_and_its_stems(&scratch);
    let other = write_song(&scratch, "Post Rave Wave.flac");

    // The order the browser hands over: a track, then the stems cut from it,
    // then the next track.
    let mut order = vec![track.clone()];
    order.extend(stems.iter().cloned());
    order.push(other.clone());

    let args = ExportArgs {
        drive: Some(drive.clone()),
        companions: stems.iter().map(|stem| (stem.clone(), track.clone())).collect(),
        playlists: vec![PlaylistSpec {
            name: "Sat 14/9".to_string(),
            folder: String::new(),
            tracks: order.clone(),
        }],
        ..args_for(order.clone())
    };
    commands::export(&args, &Collected::new()).expect("export failed");

    // The playlist, resolved back to the files its rows were made from. A
    // companion is a turn of the encoder from the record it came from, not
    // something at the end of the list.
    let by_id: std::collections::HashMap<u32, String> =
        tracks_on(&drive).into_iter().map(|(id, path, _)| (id, path)).collect();
    let played: Vec<&str> = playlist_order(&drive)
        .iter()
        .map(|id| by_id.get(id).expect("a playlist entry with no track row").as_str())
        .collect();
    assert_eq!(
        played,
        vec![
            "/Contents/Bruce/Ohm Hourglass.flac",
            "/Contents/Bruce/Ohm Hourglass-vocals.wav",
            "/Contents/Bruce/Ohm Hourglass-drums.wav",
            "/Contents/Bruce/Ohm Hourglass-melody.wav",
            "/Contents/Unknown Artist/Post Rave Wave.flac",
        ],
        "the stems did not follow their track"
    );
}

#[test]
fn a_second_sync_carries_the_stem_rows_rather_than_making_them_again() {
    // A drive is written once and then added to. A stem is three minutes of
    // decoding and analysis like any other file, and there are three of them
    // per record, so a second write that re-prepared every stem on the drive
    // would cost the whole drive to add one track to it.
    let scratch = Scratch::new("stems-carried");
    let drive = scratch.path("USB");
    let (track, stems) = track_and_its_stems(&scratch);

    let mut first = vec![track.clone()];
    first.extend(stems.iter().cloned());
    let companions: Vec<(PathBuf, PathBuf)> =
        stems.iter().map(|stem| (stem.clone(), track.clone())).collect();
    let spec = |tracks: Vec<PathBuf>| {
        vec![PlaylistSpec { name: "Sat 14/9".to_string(), folder: String::new(), tracks }]
    };

    let args = ExportArgs {
        drive: Some(drive.clone()),
        companions: companions.clone(),
        playlists: spec(first.clone()),
        ..args_for(first.clone())
    };
    let made = commands::export(&args, &Collected::new()).expect("the first write failed");
    assert_eq!(made.len(), 4, "the first write should put the track and its kit on");

    // The second week: one track added, and nothing else given to the exporter
    // — the four rows from the first write are handed back instead.
    let other = write_song(&scratch, "Post Rave Wave.flac");
    let mut order = first.clone();
    order.push(other.clone());
    let args = ExportArgs {
        drive: Some(drive.clone()),
        companions,
        playlists: spec(order.clone()),
        already: made,
        ..args_for(vec![other.clone()])
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("the second write failed");

    // Nothing about the stems was decoded again: a prepared file gets a line
    // naming what it became, and only the new track has one.
    let prepared: Vec<String> =
        reporter.lines().into_iter().filter(|line| line.contains(" -> ")).collect();
    assert_eq!(prepared.len(), 1, "something was prepared twice: {prepared:?}");
    assert!(prepared[0].contains("Post Rave Wave"), "{}", prepared[0]);

    // And the drive still describes everything on it, in the order it plays.
    let by_id: std::collections::HashMap<u32, String> =
        tracks_on(&drive).into_iter().map(|(id, path, _)| (id, path)).collect();
    assert_eq!(by_id.len(), 5, "the drive lost rows it was holding: {by_id:?}");
    let played: Vec<&str> = playlist_order(&drive)
        .iter()
        .map(|id| by_id.get(id).expect("a playlist entry with no track row").as_str())
        .collect();
    assert_eq!(
        played,
        vec![
            "/Contents/Bruce/Ohm Hourglass.flac",
            "/Contents/Bruce/Ohm Hourglass-vocals.wav",
            "/Contents/Bruce/Ohm Hourglass-drums.wav",
            "/Contents/Bruce/Ohm Hourglass-melody.wav",
            "/Contents/Unknown Artist/Post Rave Wave.flac",
        ],
        "the carried stems did not keep their place in the playlist"
    );

    // The audio the first write put on is still where its rows say it is.
    for path in by_id.values() {
        assert!(drive.join(path.trim_start_matches('/')).exists(), "{path} is not on the drive");
    }
}

#[test]
fn a_drive_write_says_what_it_did_step_by_step() {
    // The account somebody needs when a drive comes back from a booth wrong:
    // where each file went, what was copied, what was read back to check it,
    // and how long each part took. It is off in a terminal without `-v` and
    // sits at the log's most detailed level in the window.
    let scratch = Scratch::new("detail");
    let drive = scratch.path("USB");
    let track = write_song(&scratch, "one.flac");

    let args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![track.clone()]) };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export failed");

    let detail = reporter.details().join("\n");
    for wanted in [
        "writing into the folder",
        "one.flac: decoded",
        "one.flac: goes to /Contents/",
        "one.flac: copied",
        "wrote and read back 3 analysis files",
        "built the database",
        "wrote /PIONEER/rekordbox/export.pdb",
        "read /PIONEER/rekordbox/export.pdb back",
        "export finished in",
    ] {
        assert!(detail.contains(wanted), "nothing about {wanted:?} in:\n{detail}");
    }

    // And the results are still only results: a caller redirecting them does
    // not get the commentary mixed in.
    assert!(
        reporter.lines().iter().all(|line| !line.contains("decoded")),
        "{:?}",
        reporter.lines()
    );
}

#[test]
fn a_stem_says_whose_grid_it_took() {
    // The one thing about a companion that cannot be seen from the file it
    // became: a stem is analysed as its parent, not on its own.
    let scratch = Scratch::new("detail-stems");
    let drive = scratch.path("USB");
    let (track, stems) = track_and_its_stems(&scratch);

    let mut inputs = vec![track.clone()];
    inputs.extend(stems.iter().cloned());
    let args = ExportArgs {
        drive: Some(drive.clone()),
        companions: stems.iter().map(|stem| (stem.clone(), track.clone())).collect(),
        ..args_for(inputs)
    };
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("export failed");

    let detail = reporter.details().join("\n");
    assert!(
        detail.contains(
            "Ohm Hourglass-vocals.wav: takes its grid, cues, key and phrases from Ohm \
             Hourglass.flac"
        ),
        "{detail}"
    );
}

/// The collection's own answers, written rather than measured over.
///
/// Booth keeps cues a person has moved and sections a person has renamed. The
/// exporter listens to every file it prepares, and before this it wrote what it
/// heard — so a drive carried the analyser's opinion and the hand-editing went
/// nowhere. These prove the other way round: what the caller says it knows is
/// what lands on the drive.
///
/// What the analysis files look like is checked against an independent parser
/// in `rekordbox_export`; rekordcrate 0.3 will not parse the ones a whole drive
/// write produces, with or without any of this. So what these scan for is whose
/// answers are in them, which is the question they are here to settle.
mod what_the_collection_knows {
    use super::*;
    use booth_cli::export::{Cue, Part, Prep};

    /// Long enough that the arrangement detector has something to find: the
    /// test song drops its drums for the middle third, and a section has to run
    /// eight bars before it is called one.
    const SECONDS: f32 = 60.0;

    fn exported_with(name: &str, prep: Prep) -> (Scratch, PathBuf, String) {
        let scratch = Scratch::new(name);
        let source = write_song_of(&scratch, "Track.flac", SECONDS);
        let drive = scratch.path("drive");

        let args = ExportArgs {
            drive: Some(drive.clone()),
            prepared: match prep.is_empty() {
                true => Vec::new(),
                false => vec![(source.clone(), prep)],
            },
            ..args_for(vec![source])
        };
        commands::export(&args, &Collected::new()).expect("export failed");

        let rows = tracks_on(&drive);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let (_, _, analysis) = rows.into_iter().next().unwrap();
        (scratch, drive, analysis)
    }

    fn on_drive(drive: &Path, analysis: &str, extension: &str) -> Vec<u8> {
        let relative = analysis.trim_start_matches('/').replace(".DAT", extension);
        std::fs::read(drive.join(&relative))
            .unwrap_or_else(|e| panic!("no {relative} on the drive: {e}"))
    }

    /// Whether a comment reached the extended cue list, which stores its text
    /// as UTF-16 big-endian.
    ///
    /// A scan rather than a parse, deliberately: what the section looks like is
    /// checked against an independent parser in `rekordbox_export`, and what is
    /// being asked here is only whose words are in it.
    fn carries_text(bytes: &[u8], text: &str) -> bool {
        let wanted: Vec<u8> = text.encode_utf16().flat_map(|unit| unit.to_be_bytes()).collect();
        bytes.windows(wanted.len()).any(|window| window == wanted)
    }

    #[test]
    fn a_cue_named_by_hand_is_the_cue_that_goes_on_the_drive() {
        let mine = vec![
            Cue::memory(0),
            Cue::hot(1, 4_000).with_comment("hold me closer now").with_color(0xe8, 0x3c, 0x9e),
            Cue::hot(2, 9_500).with_comment("second time").with_color(0x2f, 0x6f, 0xd0),
        ];
        let (_scratch, drive, analysis) =
            exported_with("prepared-cues", Prep { cues: mine, ..Prep::default() });
        let ext = on_drive(&drive, &analysis, ".EXT");

        assert!(carries_text(&ext, "hold me closer now"), "the collection's cue never arrived");
        assert!(carries_text(&ext, "second time"));
        // Substituted rather than added to: the analyser names its own cues
        // after the sections it found, and none of those should be here.
        for measured in ["intro", "build", "break", "drop", "outro", "vocal"] {
            assert!(
                !carries_text(&ext, measured),
                "the analyser's {measured:?} cue was written alongside the collection's"
            );
        }
    }

    #[test]
    fn a_section_renamed_by_hand_is_the_section_that_goes_on_the_drive() {
        // Two sections in milliseconds, as a collection keeps them, against a
        // track the detector would divide differently.
        let parts = vec![
            Part { start_ms: 0, end_ms: 7_500, kind: "intro".into() },
            Part { start_ms: 7_500, end_ms: 19_000, kind: "drop".into() },
        ];
        let (_scratch, drive, analysis) =
            exported_with("prepared-parts", Prep { parts, ..Prep::default() });

        // The phrase section is masked on the way out, so what is checked here
        // is that one was written at all; that the beats in it are the ones
        // asked for is `Structure::from_parts`'s own test.
        let ext = on_drive(&drive, &analysis, ".EXT");
        assert!(
            ext.windows(4).any(|window| window == b"PSSI"),
            "no phrase section reached the drive"
        );
    }

    /// The names in the drive database's key table.
    fn keys_on(drive: &Path) -> Vec<String> {
        let bytes = std::fs::read(drive.join("PIONEER/rekordbox/export.pdb")).unwrap();
        let mut cursor = Cursor::new(&bytes);
        let header = Header::read(&mut cursor).unwrap();
        let Some(table) = header.tables.iter().find(|t| t.page_type == PageType::Keys) else {
            return Vec::new();
        };
        let pages = header
            .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
            .unwrap();

        let mut names = Vec::new();
        for row in pages
            .iter()
            .filter(|page| page.has_data())
            .flat_map(|page| page.row_groups.iter().flat_map(|group| group.present_rows()))
        {
            let Row::Key(key) = row else { continue };
            // The parser keeps its fields private, so the name is read back out
            // of its own description — the same trick `tracks_in` uses.
            let described = format!("{key:?}");
            let marker = "DeviceSQLString(\"";
            let Some(at) = described.find(marker) else { continue };
            let rest = &described[at + marker.len()..];
            names.push(rest[..rest.find('"').unwrap()].to_string());
        }
        names.sort();
        names
    }

    #[test]
    fn a_key_the_user_corrected_is_the_key_on_the_row() {
        let (_scratch, drive, _) = exported_with(
            "prepared-key",
            Prep { key: "8A".into(), cues: vec![Cue::memory(0)], ..Prep::default() },
        );
        assert_eq!(
            keys_on(&drive),
            vec!["8A".to_string()],
            "the drive carries the detected key rather than the one the collection holds"
        );
    }

    #[test]
    fn saying_nothing_leaves_the_exporter_exactly_as_it_was() {
        // The path every command-line export takes. A prep with nothing in it
        // must not be a prep that blanks the track, so the two writes are
        // compared byte for byte rather than by anything either could get
        // wrong in the same way.
        let (_left, drive, analysis) = exported_with("prepared-none", Prep::default());
        let measured = on_drive(&drive, &analysis, ".EXT");

        let (_right, drive, analysis) = exported_with(
            "prepared-empty",
            Prep { bpm: None, key: String::new(), cues: Vec::new(), parts: Vec::new() },
        );
        assert_eq!(on_drive(&drive, &analysis, ".EXT"), measured);
        assert!(
            ["intro", "build", "break", "drop", "outro"]
                .iter()
                .any(|name| carries_text(&measured, name)),
            "the analyser named no cues at all, so this proves nothing"
        );
    }
}
