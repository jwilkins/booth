//! The whole spine, with real audio: import, analyse, plan, preflight, write.
//!
//! Every part of this is unit-tested on its own; this is the test that the
//! parts fit together, and that a drive written from a collection is one a
//! parser that shares no code with the writer can read back. Nothing here is
//! mocked — the files are real, the analysis is real, and the drive is a real
//! directory laid out the way a player expects.

use std::path::{Path, PathBuf};

use musicai::audio::encode::{write_file, Codec, EncodeOptions};
use musicai::audio::Audio;

use booth::library::{self, Library, Playlist, Track, Written};
use booth::{query, sync};

/// A file with kicks on the beat, which is the least the analysers can work
/// with: a bare tone has no onsets, and the tempo detector is right to refuse
/// one rather than invent a grid.
fn write_beats(path: &Path, bpm: f64, bars: usize) {
    let rate = 44_100usize;
    let period = 60.0 / bpm;
    let beats = bars * 4;
    let lead_in = rate / 2;
    let frames = lead_in + (rate as f64 * period * beats as f64) as usize + rate;
    let mut plane = vec![0.0f32; frames];
    for beat in 0..beats {
        let start = lead_in + (rate as f64 * period * beat as f64) as usize;
        let (hz, gain) = if beat % 4 == 0 { (55.0, 1.0) } else { (150.0, 0.5) };
        for i in 0..rate / 8 {
            let Some(sample) = plane.get_mut(start + i) else { break };
            let t = i as f32 / rate as f32;
            *sample += gain * (-30.0 * t).exp() * (std::f32::consts::TAU * hz * t).sin();
        }
    }
    let audio = Audio::new(rate as u32, vec![plane.clone(), plane]).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_file(path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("booth-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Everything a collection does between a folder of music and a written drive.
#[test]
fn a_folder_of_music_becomes_a_drive_a_player_can_read() {
    let scratch = Scratch::new("drive");
    let music = scratch.0.join("music");
    for (name, bpm) in [("a.wav", 128.0), ("b.wav", 174.0)] {
        write_beats(&music.join(name), bpm, 8);
    }

    // -- import: the collection learns the files exist
    let mut collection = Library::new();
    let files = musicai::discover::collect(std::slice::from_ref(&music), true).unwrap();
    assert_eq!(files.len(), 2);
    for path in &files {
        let id = collection.add(path);
        let record = booth_read_record(id, path);
        *collection.get_mut(id).unwrap() = record;
    }
    assert_eq!(collection.tracks.len(), 2);
    assert_eq!(collection.unprepared_count(), 2, "nothing has been listened to yet");

    // -- analyse: every track gets a grid, phrases and cues
    for id in collection.tracks.iter().map(|t| t.id).collect::<Vec<_>>() {
        let path = collection.get(id).unwrap().path.clone();
        let analyzed = booth_analyze(id, &path);
        let track = collection.get_mut(id).unwrap();
        track.bpm = analyzed.0;
        track.has_grid = analyzed.1;
        track.beats = analyzed.2;
        track.duration_secs = analyzed.3;
        track.sample_rate = analyzed.4;
        track.cues = analyzed.5;
        track.phrases = analyzed.6;
        track.analyzed = true;
    }
    assert_eq!(collection.unprepared_count(), 0, "everything should have a grid now");
    for track in &collection.tracks {
        assert!(track.bpm > 0.0, "{} has no tempo", track.title);
        assert!(!track.cues.is_empty(), "{} has no cues", track.title);
    }

    // -- the query bar finds them by what was measured
    let found = matching(&collection, "bpm:120-180");
    assert_eq!(found, 2, "both tempos are in range");
    assert_eq!(matching(&collection, "bpm:128"), 1);
    assert_eq!(matching(&collection, "missing:grid"), 0);
    assert_eq!(matching(&collection, "missing:stems"), 2, "nothing is separated yet");

    // -- a playlist, and a drive to put it on
    let ids: Vec<u32> = collection.tracks.iter().map(|t| t.id).collect();
    collection.playlists.push(Playlist {
        name: "tonight".into(),
        folder: "Sat".into(),
        tracks: ids.clone(),
    });
    let drive_path = scratch.0.join("USB");
    collection.drives.push(library::Drive {
        label: "TEST-USB".into(),
        path: drive_path.clone(),
        playlist: "tonight".into(),
        ..library::Drive::default()
    });

    // -- the plan says what would happen
    let plan = sync::plan(&collection, &collection.drives[0]);
    assert_eq!(plan.add.len(), 2);
    assert!(plan.update.is_empty() && plan.remove.is_empty());
    assert_eq!(plan.delta(), "2 to add");

    // -- and the preflight says whether it can
    let checks = sync::preflight(&collection, &plan, &drive_path, false);
    let worst = checks.iter().map(|c| c.level).max().unwrap();
    assert_eq!(worst, sync::Level::Ok, "a clean collection should pass: {checks:#?}");

    // -- write it, through the same command the CLI uses
    let mut args = musicai::cli::ExportArgs::defaults();
    args.drive = Some(drive_path.clone());
    args.playlist = "tonight".into();
    args.input = musicai::cli::InputArgs { inputs: files.clone(), recursive: false };
    // Collected rather than Stdio: a passing test should be quiet, and a
    // failing one has the command's own account of what it did.
    let reporter = musicai::report::Collected::new();
    musicai::commands::export(&args, &reporter).expect("the drive should write");

    // -- and read it back the way a player would, with a parser that shares no
    //    code with the writer
    let database = drive_path.join("PIONEER/rekordbox/export.pdb");
    assert!(database.exists(), "no database was written");
    let bytes = std::fs::read(&database).unwrap();
    let tables = musicai::export::pdb::inspect(&bytes).expect("the database should parse");
    let rows: usize = tables.iter().map(|t| t.rows).sum();
    assert!(rows > 0, "the database is empty");
    assert!(drive_path.join("Contents").exists(), "the audio did not land");

    // -- the next sync has nothing to do
    let written: Vec<Written> = collection
        .tracks
        .iter()
        .map(|track| Written { id: track.id, prep: sync::fingerprint(track) })
        .collect();
    collection.drives[0].written = written;
    assert!(sync::plan(&collection, &collection.drives[0]).is_empty());

    // -- until the prep changes, and then it is an update rather than a re-add
    collection.get_mut(ids[0]).unwrap().cues.push(library::CueMark {
        letter: 4,
        time_ms: 30_000,
        label: "drop".into(),
        color: [226, 160, 63],
    });
    let plan = sync::plan(&collection, &collection.drives[0]);
    assert!(plan.add.is_empty(), "the file is already there");
    assert_eq!(plan.update.len(), 1);
    assert_eq!(plan.delta(), "1 changed");
}

/// A collection survives being closed and reopened with everything intact.
#[test]
fn a_collection_reopens_the_way_it_was_left() {
    let scratch = Scratch::new("reopen");
    let music = scratch.0.join("music");
    write_beats(&music.join("a.wav"), 128.0, 4);
    let path = scratch.0.join("library.json");

    let mut collection = Library::new();
    let id = collection.add(&music.join("a.wav"));
    {
        let track = collection.get_mut(id).unwrap();
        track.artist = "Peverelist".into();
        track.title = "Roll With The Punches".into();
        track.bpm = 128.02;
        track.key = "8A".into();
        track.has_grid = true;
        track.analyzed = true;
        track.tags = vec!["peak".into()];
    }
    collection.playlists.push(Playlist {
        name: "tonight".into(),
        folder: "Sat".into(),
        tracks: vec![id],
    });
    collection.save(&path).unwrap();

    let reopened = Library::load(&path).unwrap();
    assert_eq!(reopened.tracks, collection.tracks);
    assert_eq!(reopened.playlists, collection.playlists);
    // And the queries still find the same things.
    assert_eq!(matching(&reopened, "tag:peak key:~8A"), 1);
    assert_eq!(matching(&reopened, "-tag:peak"), 0);
}

/// How many tracks a query matches, with the collection as its context.
fn matching(collection: &Library, text: &str) -> usize {
    let drives = collection.drive_index();
    let playlists = collection.playlist_index();
    let context = query::Context {
        now: library::now(),
        drives: &drives,
        playlists: &playlists,
        duplicates: &[],
    };
    let parsed = query::Query::parse(text);
    assert!(!parsed.has_errors(), "{text} did not parse");
    collection.tracks.iter().filter(|track| parsed.matches(track, &context)).count()
}

/// The same work `job::read_record` does, without pulling the job module (and
/// the whole window with it) into this test binary.
fn booth_read_record(id: u32, path: &Path) -> Track {
    let mut track = Track::placeholder(id);
    track.path = path.to_path_buf();
    track.format = path.extension().unwrap().to_string_lossy().to_lowercase();
    track.bytes = std::fs::metadata(path).unwrap().len();
    track.title = path.file_stem().unwrap().to_string_lossy().into_owned();
    let metadata = musicai::tag::read_metadata(path).unwrap_or_default();
    track.artist = metadata.artist.unwrap_or_else(|| "Test Artist".into());
    track
}

/// Likewise for the analysis, reduced to what this test then asserts on.
#[allow(clippy::type_complexity)]
fn booth_analyze(
    _id: u32,
    path: &Path,
) -> (f64, bool, usize, f64, u32, Vec<library::CueMark>, Vec<library::Phrase>) {
    let audio = musicai::audio::decode::decode_file(path).unwrap();
    let analysis = musicai::analysis::analyze(&audio);
    let beat_ms: Vec<u32> = analysis.grid.beats.iter().map(|b| b.time_ms).collect();

    let cues = analysis
        .cues
        .iter()
        .map(|cue| library::CueMark {
            letter: cue.hot_cue,
            time_ms: cue.time_ms,
            label: cue.comment.clone().unwrap_or_default(),
            color: [226, 160, 63],
        })
        .collect();
    let phrases = analysis
        .structure
        .sections
        .iter()
        .map(|section| library::Phrase {
            start_ms: beat_ms
                .get(section.start_beat.saturating_sub(1) as usize)
                .copied()
                .unwrap_or(0),
            end_ms: beat_ms.get(section.end_beat.saturating_sub(1) as usize).copied().unwrap_or(0),
            kind: section.kind.label().to_string(),
        })
        .collect();

    (
        analysis.bpm,
        analysis.found_beats(),
        analysis.grid.beats.len(),
        audio.duration_secs(),
        audio.sample_rate,
        cues,
        phrases,
    )
}
