//! How long a drive takes: to write, to add to, and to read back.
//!
//! The number that decides whether a night goes well is not how long the
//! first write of a drive takes — that happens at home, once — but how long
//! adding one track to it takes at five to midnight. rekordbox's answer, from
//! its own forum, is hours: its Sync rewrites the whole stick, and a DJ who
//! added one track in February 2026 waited four of them. This measures Booth's
//! answer, and the measurement is published (`site/speed.html`) so the claim is
//! a number with a date on it rather than an adjective.
//!
//! Two tests. The first always runs and makes the structural claim — that the
//! second write costs a fraction of the first — as a ratio, because a ratio
//! holds on a laptop and on a loaded CI runner alike and an absolute budget
//! does not. The second is `#[ignore]`d and is the measurement: it writes
//! drives of several sizes and puts what it saw in a JSON file for the site.
//! `scripts/speed.sh` runs it; so does the `speed` workflow.
//!
//! What is measured is the drive-writing side of the work: copying audio,
//! writing and reading back the analysis files, building and verifying both
//! databases, and reading the whole thing back the way the dock does when a
//! stick is plugged in. Preparing a track — decoding it and listening to it —
//! is reported too, but it is the part that scales with how long the records
//! are, and the records here are short so that the drive is what is timed.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use booth_cli::cli::{ExportArgs, PlaylistSpec};
use booth_cli::commands;
use booth_cli::export::pdb;
use booth_cli::report::Collected;
use common::drive::*;
use serde::Serialize;

/// Where the drive is written. A directory on the medium under test — a real
/// stick, mounted — or the host's temporary directory when none is named.
const DRIVE_ENV: &str = "BOOTH_SPEED_DRIVE";
/// What to call that medium in the results.
const MEDIUM_ENV: &str = "BOOTH_SPEED_MEDIUM";
/// Where the JSON goes. Nothing is written when unset.
const OUT_ENV: &str = "BOOTH_SPEED_OUT";
/// The drive sizes to measure, in tracks, comma-separated.
const SIZES_ENV: &str = "BOOTH_SPEED_SIZES";
/// How long each synthetic record is, in seconds.
const SECONDS_ENV: &str = "BOOTH_SPEED_TRACK_SECONDS";

/// One drive, written and read back, timed phase by phase.
#[derive(Debug, Serialize)]
struct Run {
    tracks: usize,
    track_seconds: f32,
    audio_bytes: u64,
    /// The first write of the drive: every track prepared, copied, analysed,
    /// both databases built and verified. Wall-clock.
    full_s: f64,
    /// Of that, decoding and listening to the tracks (wall-clock, parallel).
    prepare_s: f64,
    /// Of that, copying the audio onto the drive.
    copy_s: f64,
    /// Of that, writing the analysis files and reading each back.
    analysis_write_s: f64,
    analysis_files: usize,
    analysis_bytes: u64,
    /// Of that, building, writing and reading back both databases.
    databases_s: f64,
    /// A second write that changes nothing but the databases: a playlist
    /// edit, a track taken off. Wall-clock.
    databases_only_s: f64,
    /// A second write that adds one track to the full drive. Wall-clock. This
    /// is the number a DJ at five to midnight is waiting on.
    add_one_s: f64,
    /// Reading the finished drive back the way the dock does when a stick is
    /// plugged in: both databases walked, every analysis file checked, every
    /// track's cues and grid read. Wall-clock.
    read_back_s: f64,
    legacy_walk_s: f64,
    onelibrary_open_s: f64,
    analysis_check_s: f64,
    cues_read_s: f64,
}

#[derive(Debug, Serialize)]
struct Results {
    schema: u32,
    /// Seconds since the Unix epoch.
    measured_at: u64,
    git: String,
    os: String,
    arch: String,
    cpus: usize,
    medium: String,
    /// Debug builds are several times slower than release, and the number
    /// published must say which it was.
    profile: &'static str,
    runs: Vec<Run>,
}

/// How long a detail line said its step took. The writer reports every step as
/// "… in 123 ms" or "… in 1.2 s", and these are summed per phase.
fn seconds_in(line: &str) -> f64 {
    let Some(at) = line.rfind(" in ") else { return 0.0 };
    let tail = line[at + 4..].trim_end_matches([':', ',', '.']);
    let tail = tail.split(':').next().unwrap_or(tail).trim();
    let mut parts = tail.split_whitespace();
    let (Some(number), Some(unit)) = (parts.next(), parts.next()) else { return 0.0 };
    let value: f64 = number.parse().unwrap_or(0.0);
    match unit {
        "ms" => value / 1000.0,
        "s" => value,
        _ => 0.0,
    }
}

/// The sum of the steps whose detail lines contain `marker`.
fn phase(details: &[String], marker: &str) -> f64 {
    details.iter().filter(|line| line.contains(marker)).map(|line| seconds_in(line)).sum()
}

/// A drive's worth of records: one song, copied under `n` names. Copied rather
/// than synthesised `n` times because encoding a FLAC is not what is being
/// timed, and because every track taking the same path through the analyser
/// is what makes two sizes comparable.
fn records(scratch: &Scratch, n: usize, seconds: f32) -> Vec<PathBuf> {
    let first = write_song_of(scratch, "record-0000.flac", seconds);
    let mut out = vec![first.clone()];
    for i in 1..n {
        let path = scratch.path(&format!("record-{i:04}.flac"));
        std::fs::copy(&first, &path).unwrap();
        out.push(path);
    }
    out
}

fn args(drive: &Path, inputs: Vec<PathBuf>, playlist: &[PathBuf]) -> ExportArgs {
    let mut args = ExportArgs { drive: Some(drive.to_path_buf()), ..args_for(inputs) };
    args.playlists = vec![PlaylistSpec {
        name: "Saturday".to_string(),
        folder: String::new(),
        tracks: playlist.to_vec(),
    }];
    args
}

/// Write a drive of `n` tracks, add one, and read it all back, timing each.
fn measure(where_: &Path, n: usize, seconds: f32) -> Run {
    let scratch = Scratch::new(&format!("speed-{n}"));
    let drive = where_.join(format!("booth-speed-{n}"));
    let _ = std::fs::remove_dir_all(&drive);
    std::fs::create_dir_all(&drive).unwrap();

    let tracks = records(&scratch, n, seconds);
    let audio_bytes: u64 =
        tracks.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();

    // The first write.
    let reporter = Collected::new();
    let began = Instant::now();
    let rows = commands::export(&args(&drive, tracks.clone(), &tracks), &reporter)
        .expect("the full write failed");
    let full_s = began.elapsed().as_secs_f64();
    let details = reporter.details();
    let copy_s = phase(&details, ": copied ");
    let analysis_write_s = phase(&details, "analysis file");
    let analysis_bytes: u64 = details
        .iter()
        .filter(|line| line.contains("analysis file"))
        .filter_map(|line| {
            let at = line.rfind(" (")? + 2;
            line[at..].split_whitespace().next()?.parse::<u64>().ok()
        })
        .sum();
    let databases_s = phase(&details, "built the database")
        + phase(&details, "wrote /PIONEER/rekordbox/export.pdb")
        + phase(&details, "back and walked it")
        + phase(&details, "built the OneLibrary database")
        + phase(&details, "wrote /PIONEER/rekordbox/exportLibrary.db")
        + phase(&details, "back and opened it");
    let prepare_s = (full_s - copy_s - analysis_write_s - databases_s).max(0.0);

    // The same drive, nothing new: only the databases are rebuilt.
    let began = Instant::now();
    let mut again = args(&drive, Vec::new(), &tracks);
    again.already = rows.clone();
    commands::export(&again, &Collected::new()).expect("the databases-only write failed");
    let databases_only_s = began.elapsed().as_secs_f64();

    // One more track on a full drive.
    let one_more = scratch.path("one-more.flac");
    std::fs::copy(&tracks[0], &one_more).unwrap();
    let mut playlist = tracks.clone();
    playlist.push(one_more.clone());
    let began = Instant::now();
    let mut adding = args(&drive, vec![one_more], &playlist);
    adding.already = rows;
    commands::export(&adding, &Collected::new()).expect("adding one track failed");
    let add_one_s = began.elapsed().as_secs_f64();

    // Reading it back, as the dock does when the stick is plugged in.
    let key = booth_cli::rekordbox::onelibrary_key(None).expect("this build carries a key");
    let began = Instant::now();
    let legacy = std::fs::read(drive.join(LEGACY)).unwrap();
    let tables = pdb::inspect(&legacy).expect("the legacy database did not walk");
    let legacy_walk_s = began.elapsed().as_secs_f64();
    assert!(tables.iter().map(|t| t.rows).sum::<usize>() > n);

    let began = Instant::now();
    let newer = std::fs::read(drive.join(ONELIBRARY)).unwrap();
    let summary = booth_cli::export::onelibrary::inspect(&newer, &key)
        .expect("the OneLibrary database did not open");
    let onelibrary_open_s = began.elapsed().as_secs_f64();
    assert_eq!(summary.tracks as usize, n + 1);

    let shown = legacy_tracks(&drive);
    let began = Instant::now();
    let mut analysis_files = 0;
    for track in &shown {
        let dat = track.analyze_path.trim_start_matches('/');
        for name in [dat.to_string(), dat.replace(".DAT", ".EXT"), dat.replace(".DAT", ".2EX")] {
            let bytes = std::fs::read(drive.join(&name)).unwrap();
            booth_cli::export::anlz::check(&bytes).unwrap();
            analysis_files += 1;
        }
    }
    let analysis_check_s = began.elapsed().as_secs_f64();

    let began = Instant::now();
    for track in &shown {
        let dat = track.analyze_path.trim_start_matches('/');
        let ext = dat.replace(".DAT", ".EXT");
        let dat = std::fs::read(drive.join(dat)).unwrap();
        let ext = std::fs::read(drive.join(ext)).unwrap();
        booth_cli::rekordbox::anlz::read_files(&dat, Some(&ext)).unwrap();
    }
    let cues_read_s = began.elapsed().as_secs_f64();
    let read_back_s = legacy_walk_s + onelibrary_open_s + analysis_check_s + cues_read_s;

    let _ = std::fs::remove_dir_all(&drive);
    Run {
        tracks: n,
        track_seconds: seconds,
        audio_bytes,
        full_s,
        prepare_s,
        copy_s,
        analysis_write_s,
        analysis_files,
        analysis_bytes,
        databases_s,
        databases_only_s,
        add_one_s,
        read_back_s,
        legacy_walk_s,
        onelibrary_open_s,
        analysis_check_s,
        cues_read_s,
    }
}

fn where_to_write() -> PathBuf {
    std::env::var_os(DRIVE_ENV).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

#[test]
fn adding_one_track_to_a_drive_costs_a_fraction_of_writing_it() {
    // Twelve short records, which is enough for the two databases and
    // thirty-six analysis files to be real work, and few enough that this runs
    // with the rest of the suite.
    let run = measure(&where_to_write(), 12, 8.0);
    eprintln!("{run:#?}");

    // The claim. rekordbox's Sync rewrites the whole stick to add a track;
    // this carries every row it already has and prepares one file. Half is a
    // deliberately loose line — the honest figure is nearer a twelfth — so
    // that a loaded machine does not fail it, and a regression that made the
    // second write rewrite everything would still be caught.
    assert!(
        run.add_one_s < run.full_s / 2.0,
        "adding one track took {:.2}s against {:.2}s for the whole drive",
        run.add_one_s,
        run.full_s
    );
    assert!(
        run.databases_only_s < run.full_s / 2.0,
        "rewriting the databases alone took {:.2}s against {:.2}s for the whole drive",
        run.databases_only_s,
        run.full_s
    );
    // And reading the whole drive back is cheaper than writing it, which is
    // the condition for checking a stick on the way out of the door.
    assert!(run.read_back_s < run.full_s, "{run:#?}");
}

/// The measurement. Run it with `scripts/speed.sh`, or:
///
/// ```sh
/// BOOTH_SPEED_OUT=site/speed.json cargo test -p booth-cli --release --test speed \
///     -- --ignored measure --nocapture
/// ```
///
/// `BOOTH_SPEED_DRIVE` points it at a mounted stick; without it the host's
/// temporary directory stands in, and the results say so.
#[test]
#[ignore = "a measurement, minutes long; scripts/speed.sh runs it"]
fn measure_how_long_a_drive_takes_to_write_add_to_and_read_back() {
    let sizes: Vec<usize> = std::env::var(SIZES_ENV)
        .unwrap_or_else(|_| "25,100,400".to_string())
        .split(',')
        .filter_map(|n| n.trim().parse().ok())
        .collect();
    let seconds: f32 = std::env::var(SECONDS_ENV).ok().and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let where_ = where_to_write();
    let medium = std::env::var(MEDIUM_ENV).unwrap_or_else(|_| match std::env::var_os(DRIVE_ENV) {
        Some(at) => format!("the drive at {}", Path::new(&at).display()),
        None => "the host's temporary directory".to_string(),
    });

    let mut runs = Vec::new();
    for n in sizes {
        let run = measure(&where_, n, seconds);
        eprintln!(
            "{n} tracks: wrote in {:.1}s, databases alone {:.2}s, +1 track {:.2}s, read back {:.2}s",
            run.full_s, run.databases_only_s, run.add_one_s, run.read_back_s
        );
        runs.push(run);
    }

    let results = Results {
        schema: 1,
        measured_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        git: std::env::var("GITHUB_SHA").ok().unwrap_or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_default()
        }),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        medium,
        profile: if cfg!(debug_assertions) { "debug" } else { "release" },
        runs,
    };
    if let Some(out) = std::env::var_os(OUT_ENV) {
        let json = serde_json::to_string_pretty(&results).unwrap();
        std::fs::write(&out, json).expect("could not write the results");
        eprintln!("wrote {}", Path::new(&out).display());
    } else {
        eprintln!("{}", serde_json::to_string_pretty(&results).unwrap());
    }
}
