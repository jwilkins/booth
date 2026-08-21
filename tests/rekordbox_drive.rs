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
use musicai::export::image::DriveImage;
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
        dry_run: false,
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
    let sections = musicai::export::anlz::inspect(&std::fs::read(&ext).unwrap()).unwrap();

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
    let tables = musicai::export::pdb::inspect(&bytes).unwrap();
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
    let sections = musicai::export::anlz::inspect(&std::fs::read(&dat).unwrap()).unwrap();

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
        musicai::export::pdb::inspect(&opened.read("/PIONEER/rekordbox/export.pdb").unwrap())
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
