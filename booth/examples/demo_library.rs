//! Builds the collection the screenshots are taken of.
//!
//! The pictures in the READMEs have to be re-taken whenever the window changes,
//! and a screenshot of an empty window says nothing. So the collection in them
//! is made here rather than by hand: a fixed set of records, fixed tempos, keys
//! and cues, and a drive with something to do. Running it twice gives the same
//! window twice, which is what makes two screenshots comparable and what makes
//! "re-take the screenshots" a command rather than an afternoon.
//!
//! Nothing here is anybody's music. The audio files are a few seconds of tone
//! written on the spot, because the window needs a file to exist at the path a
//! row names — it checks, and a row pointing nowhere draws as a missing track.
//!
//!     cargo run -p booth --example demo_library -- <dir>
//!
//! See `scripts/screenshots.sh`, which is what actually calls it.

use std::path::{Path, PathBuf};

use booth::config::Config;
use booth::library::{CueMark, Drive, Library, Lyric, Phrase, Track};

/// The records, in the order they go in the list. Artist, title, album, tempo,
/// key, energy, seconds.
const RECORDS: [(&str, &str, &str, f64, &str, u8, f64); 9] = [
    ("Peverelist", "Roll With The Punches", "Jarvik Mindstate", 130.0, "8A", 3, 372.0),
    ("Batu", "Marrow", "False Reality", 133.0, "11A", 4, 318.0),
    ("Bruce", "Not Stochastic", "Sonder Somatic", 128.0, "5A", 3, 401.0),
    ("Objekt", "Ganzfeld", "Flatland", 124.0, "2B", 5, 344.0),
    ("Shanti Celeste", "Make Time", "Tangerine", 126.0, "9B", 4, 389.0),
    ("Lurka", "Beacon", "Beacon", 136.0, "12A", 5, 297.0),
    ("Bakongo", "Kabuki", "Kabuki", 138.0, "6A", 5, 265.0),
    ("Hodge", "Ghetto Gremlin", "Hot Sauce", 132.0, "4A", 4, 356.0),
    ("Lu.Re", "Hourglass", "Ohm Resistance", 129.0, "1A", 2, 412.0),
];

/// The sections every record is given, as fractions of its length. Not measured
/// — made up, and made up the same way every time, so the phrase strip in a
/// screenshot is the same strip next time.
const ARRANGEMENT: [(f64, f64, &str); 7] = [
    (0.00, 0.10, "intro"),
    (0.10, 0.22, "build"),
    (0.22, 0.42, "drop"),
    (0.42, 0.54, "break"),
    (0.54, 0.64, "build"),
    (0.64, 0.88, "drop"),
    (0.88, 1.00, "outro"),
];

/// What the records sing, as fractions of their length. The hook lands on
/// each of the two drops, which is what makes it the hook — the panel that
/// shows what a track keeps saying has nothing to show without words, and a
/// screenshot of an empty section says the feature is broken.
const SUNG: [(f64, &str); 6] = [
    (0.14, "walking through the city at night"),
    (0.24, "hold me closer now"),
    (0.46, "nothing here but the lights and us"),
    (0.66, "hold me closer now"),
    (0.80, "hold me closer now"),
    (0.92, "nothing here but the lights and us"),
];

fn main() -> anyhow::Result<()> {
    let into = match std::env::args().nth(1) {
        Some(dir) => PathBuf::from(dir),
        None => {
            eprintln!("usage: demo_library <dir>");
            std::process::exit(2);
        }
    };
    // Everything below asks `data_dir()` where things go, so this is set before
    // anything else runs rather than threaded through.
    //
    // SAFETY: single-threaded, before any thread that might read the
    // environment has been started.
    unsafe { std::env::set_var("BOOTH_DATA_DIR", &into) };
    let audio_dir = into.join("music");
    std::fs::create_dir_all(&audio_dir)?;

    let mut library = Library::new();
    for (artist, title, album, bpm, key, energy, seconds) in RECORDS {
        let path = audio_dir.join(format!("{artist} - {title}.wav"));
        write_tone(&path, seconds)?;
        let id = library.add(&path);
        let track = library.get_mut(id).expect("just added");
        fill_in(track, artist, title, album, bpm, key, energy, seconds);
        let bands = picture(seconds, bpm);
        booth::library::cache_waveform(id, std::slice::from_ref(&path), &bands)?;
    }

    // Two lists and a folder, so the sidebar shows a tree rather than a column.
    library.add_playlist("Saturday", "").map_err(anyhow::Error::msg)?;
    library.add_playlist("Warm up", "Nights").map_err(anyhow::Error::msg)?;
    let all: Vec<u32> = library.tracks.iter().map(|t| t.id).collect();
    library.playlists[0].tracks = all.clone();
    library.playlists[1].tracks = all.iter().copied().take(4).collect();

    // A drive with something to do, because "8 to add" is the number the dock
    // exists for and a drive that is up to date shows none of it.
    let drive_path = into.join("TRANSCEND");
    std::fs::create_dir_all(drive_path.join("Contents"))?;
    library.drives.push(Drive {
        label: "TRANSCEND".into(),
        path: drive_path,
        playlists: vec!["Saturday".into()],
        ..Drive::default()
    });

    library.save(&Library::default_path())?;
    // The library folder is where the demo's own files are, so the inspector
    // reads as a collection somebody keeps rather than one pointed at a
    // stranger's disk — "no local copy" on every row is a true thing to say
    // about a fixture and a misleading thing to photograph.
    let config = Config { library_path: audio_dir, ..Config::default() };
    config.save(&Config::path())?;
    println!("{}", into.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fill_in(
    track: &mut Track,
    artist: &str,
    title: &str,
    album: &str,
    bpm: f64,
    key: &str,
    energy: u8,
    seconds: f64,
) {
    track.artist = artist.into();
    track.title = title.into();
    track.album = album.into();
    track.year = Some(2019);
    track.duration_secs = seconds;
    track.format = "wav".into();
    track.sample_rate = 44_100;
    track.channels = 2;
    track.bitrate_kbps = 1411;
    track.bytes = (seconds * 44_100.0 * 4.0) as u64;
    track.bpm = bpm;
    track.grid_confidence = 6.0;
    track.has_grid = true;
    track.beats = (seconds * bpm / 60.0) as usize;
    track.key = key.into();
    track.key_confidence = 0.9;
    track.energy = energy;
    track.analyzed = true;
    track.loudness_lufs = Some(-8.4);
    track.peak_dbtp = Some(-0.9);
    track.added = 1_726_000_000;
    track.play_count = 3;

    let total = seconds * 1000.0;
    track.phrases = ARRANGEMENT
        .iter()
        .map(|(from, to, kind)| Phrase {
            start_ms: (total * from) as u32,
            end_ms: (total * to) as u32,
            kind: (*kind).to_string(),
        })
        .collect();

    // The shape a prepared record actually has: a named memory cue on every
    // section, and a hot cue on each drop. The names are the ones the analyser
    // writes — numbered within their own kind — so the picture shows what a
    // track off this program looks like rather than something arranged for it.
    track.cues.clear();
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for (from, _, kind) in ARRANGEMENT {
        let word = match kind {
            "intro" => "Intro",
            "build" => "Build",
            "break" => "Break",
            "drop" => "Drop",
            _ => "Outro",
        };
        let count = match seen.iter_mut().find(|(name, _)| *name == word) {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                seen.push((word, 1));
                1
            }
        };
        track.cues.push(CueMark {
            letter: 0,
            time_ms: (total * from) as u32,
            label: match from {
                0.0 => "Start".to_string(),
                _ => format!("{word} {count}"),
            },
            color: booth::job::cue_color(0),
        });
    }
    track.cues.push(CueMark {
        letter: 0,
        time_ms: total as u32,
        label: "End".into(),
        color: booth::job::cue_color(0),
    });

    for (from, _, _) in ARRANGEMENT.iter().filter(|(_, _, kind)| *kind == "drop") {
        let letter = track.cues.iter().filter(|cue| cue.letter != 0).count() as u8 + 1;
        let shade =
            booth::theme::CUE_COLORS[(letter - 1) as usize % booth::theme::CUE_COLORS.len()];
        track.cues.push(CueMark {
            letter,
            time_ms: (total * from) as u32,
            label: format!("Drop {letter}"),
            color: [shade.r(), shade.g(), shade.b()],
        });
    }
    track.cues.sort_by_key(|cue| (cue.letter, cue.time_ms));

    // And what a recogniser made of the vocal stem, with the analysis over it
    // worked out the way the window works it out rather than written in here.
    track.lyrics = SUNG
        .iter()
        .map(|(at, text)| Lyric {
            start_ms: (total * at) as u32,
            end_ms: (total * at) as u32 + 2_400,
            text: (*text).to_string(),
        })
        .collect();
    track.lyrics_aligned = true;
    track.refrains = booth::library::refrains_from(&track.lyrics);
}

/// A three-band picture with the shape of the arrangement above.
///
/// Made rather than measured: the point is a window that looks like a window,
/// and a real analysis of a sine tone draws a flat bar.
fn picture(seconds: f64, bpm: f64) -> Vec<u8> {
    const COLUMNS: usize = 1200;
    let mut bands = Vec::with_capacity(COLUMNS * 3);
    for column in 0..COLUMNS {
        let at = column as f64 / COLUMNS as f64;
        let section = ARRANGEMENT
            .iter()
            .find(|(from, to, _)| at >= *from && at < *to)
            .map(|(_, _, kind)| *kind)
            .unwrap_or("outro");
        let (low, mid, high) = match section {
            "intro" => (0.35, 0.20, 0.15),
            "build" => (0.55, 0.45, 0.55),
            "drop" => (0.95, 0.70, 0.60),
            "break" => (0.25, 0.55, 0.35),
            _ => (0.40, 0.25, 0.20),
        };
        // A kick every beat, so the picture has the texture of a record rather
        // than of a block.
        let beat = (at * seconds * bpm / 60.0).fract();
        let thump = 1.0 - (beat * 2.0).min(1.0) * 0.45;
        let wobble = 1.0 + (column as f64 * 0.37).sin() * 0.08;
        for level in [low * thump, mid * wobble, high * wobble] {
            bands.push((level.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    bands
}

/// A few seconds of quiet tone, so the row's file exists and can be opened.
fn write_tone(path: &Path, seconds: f64) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    // Short whatever the record's length says: nothing plays these, and a
    // seven-minute wav per row is 70 MB of scratch for no picture at all.
    let _ = seconds;
    let rate = 44_100u32;
    let frames = rate as usize * 2;
    let mut out = Vec::with_capacity(44 + frames * 4);
    let data = (frames * 4) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for frame in 0..frames {
        let t = frame as f64 / rate as f64;
        let sample = ((t * 220.0 * std::f64::consts::TAU).sin() * 4_000.0) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
        out.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, out)?;
    Ok(())
}
