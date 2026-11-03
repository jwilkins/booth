//! Things that went wrong on other people's drives, tried here.
//!
//! Each test is a bug report against rekordbox or against another tool that
//! writes its drives — the issue is named above the test — turned into a claim
//! about what a drive this writes looks like afterwards. They are integration
//! tests because every one of them is about the drive as a whole: two databases
//! that have to agree, a playlist that has to name only what landed, a file
//! that has to be gone.
//!
//! The rule they share, and the one the owner set: **a drive always carries
//! both databases, and they always describe the same drive.** Where one of them
//! cannot be written, the other is not left describing last week.

mod common;

use booth_cli::cli::{ExportArgs, PlaylistSpec};
use booth_cli::commands::{self, PartialWrite};
use booth_cli::export::Prep;
use booth_cli::report::{Collected, Event};
use common::drive::*;

/// A passphrase, not *the* passphrase. What is being checked is that the two
/// databases on a drive agree with each other, not that a player opens them;
/// the key that makes a player open one is the build's own and is tested in
/// `rekordbox_drive.rs`.
const KEY: &str = "a-key-that-is-not-the-real-one";

/// A playlist by name, of these files.
fn list(name: &str, tracks: &[&std::path::PathBuf]) -> PlaylistSpec {
    PlaylistSpec {
        name: name.to_string(),
        folder: String::new(),
        tracks: tracks.iter().map(|p| (*p).clone()).collect(),
    }
}

/// Both databases, read back and compared field for field — the check every
/// test here ends on, because it is the one a booth with two generations of
/// deck in it is making.
fn both_databases_agree(drive: &std::path::Path) -> Vec<Shown> {
    let legacy = legacy_tracks(drive);
    let newer = onelibrary_tracks(drive, KEY);
    assert_eq!(legacy, newer, "the two databases describe two different drives");
    assert_eq!(
        legacy_playlist_entries(drive),
        onelibrary_playlist_entries(drive, KEY),
        "the two databases' playlists disagree"
    );
    // And both name analysis files that are really there, in every generation
    // — the mixed-format drives in the forum thread had rows whose waveforms
    // were missing, and a deck draws nothing for those.
    for (file, there) in analysis_files(drive) {
        assert!(there, "{file} is named by the database and not on the drive");
    }
    legacy
}

// M-Igashi/baken#208: a tool that rewrote the legacy database on a stick left
// the OneLibrary database rekordbox had put there, so a CDJ-3000X — which reads
// nothing else — browsed the stick as it was before the rewrite, with no error.
#[test]
fn a_write_that_carries_no_onelibrary_takes_last_weeks_off_the_drive() {
    let scratch = Scratch::new("stale-onelibrary");
    let drive = scratch.path("USB");
    let first = write_song(&scratch, "first.flac");
    let second = write_song(&scratch, "second.flac");

    // Week one: both databases, as every ordinary write puts on.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![first.clone()]) };
    args.onelibrary_key = Some(KEY.to_string());
    args.playlists = vec![list("Saturday", &[&first])];
    let rows = commands::export(&args, &Collected::new()).expect("the first write failed");
    assert!(drive.join(ONELIBRARY).exists());

    // Week two: a write told to leave the OneLibrary database off. The one from
    // week one describes one track; the drive now holds two.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![second.clone()]) };
    args.no_onelibrary = true;
    args.already = rows;
    args.playlists = vec![list("Saturday", &[&first, &second])];
    let reporter = Collected::new();
    let rows = commands::export(&args, &reporter).expect("the second write failed");

    assert_eq!(legacy_tracks(&drive).len(), 2);
    assert!(
        !drive.join(ONELIBRARY).exists(),
        "last week's OneLibrary database is still on the drive, describing last week's drive"
    );
    let lines = reporter.lines().join("\n");
    assert!(lines.contains("left /PIONEER/rekordbox/exportLibrary.db off the drive"), "{lines}");
    assert!(
        lines.contains("removed the /PIONEER/rekordbox/exportLibrary.db an earlier write left"),
        "taking a database off a drive is something the person is told: {lines}"
    );

    // Week three: with a key again, both are back and both say two tracks.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(Vec::new()) };
    args.onelibrary_key = Some(KEY.to_string());
    args.already = rows;
    args.playlists = vec![list("Saturday", &[&first, &second])];
    commands::export(&args, &Collected::new()).expect("the third write failed");
    assert_eq!(both_databases_agree(&drive).len(), 2);
}

// The research log's fourth pain point on 2026-10-24: exports that run a disk
// out of room partway and leave it half written. A write that will not fit
// stops before it starts, and says so.
#[test]
fn a_write_that_will_not_fit_on_the_drive_is_refused_before_anything_is_written() {
    let scratch = Scratch::new("no-room");
    let drive = scratch.path("USB");
    // A file that says it is larger than the disk under it, without taking the
    // room: a sparse one, as large as the filesystem allows a file to be.
    let huge = scratch.path("huge.flac");
    let file = std::fs::File::create(&huge).unwrap();
    let Some(size) = [1u64 << 43, 1 << 40].into_iter().find(|&size| file.set_len(size).is_ok())
    else {
        return;
    };
    match booth_cli::volume::free_space(&drive) {
        Some(free) if free < size => {}
        // A platform that will not say, or a disk with eight terabytes free.
        _ => return,
    }

    let args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![huge]) };
    let error = commands::export(&args, &Collected::new())
        .expect_err("a write too big for the drive went ahead");
    let said = format!("{error:#}");
    assert!(said.contains("nothing was written"), "{said}");
    assert!(said.contains("--no-space-check"), "the way past it is not named: {said}");
    assert!(!drive.exists(), "the drive was written to");
}

// Nothing stale is removed from a drive that never had one: the line is only
// said when something came off.
#[test]
fn a_drive_that_never_had_a_onelibrary_is_not_told_one_was_removed() {
    let scratch = Scratch::new("no-stale-onelibrary");
    let drive = scratch.path("USB");
    let mut args =
        ExportArgs { drive: Some(drive.clone()), ..args_for(vec![write_song(&scratch, "a.flac")]) };
    args.no_onelibrary = true;
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("the write failed");
    let lines = reporter.lines().join("\n");
    assert!(!lines.contains("removed the"), "{lines}");
    assert!(!drive.join(ONELIBRARY).exists());
}

// The AlphaTheta community thread on Device Library Plus (six users, 2023):
// drives carrying both formats where tracks showed in one and not the other,
// or showed without their waveforms. And the owner's rule for this program:
// both databases, always, and always up to date. A week of ordinary changes —
// one added, one changed, one taken off — must leave the two saying the same
// thing about every track and every playlist.
#[test]
fn both_databases_tell_the_same_story_after_a_week_of_changes() {
    let scratch = Scratch::new("parity");
    let drive = scratch.path("USB");
    let a = write_song(&scratch, "a.flac");
    let b = write_song(&scratch, "b.flac");
    let c = write_song(&scratch, "c.flac");
    let d = write_song(&scratch, "d.flac");

    // Saturday: three tracks, one with words.
    let mut args = ExportArgs {
        drive: Some(drive.clone()),
        ..args_for(vec![a.clone(), b.clone(), c.clone()])
    };
    args.onelibrary_key = Some(KEY.to_string());
    args.playlists = vec![list("Saturday", &[&a, &b, &c])];
    args.prepared = vec![(
        b.clone(),
        Prep { comment: "the one with the whistle".into(), ..Default::default() },
    )];
    let rows = commands::export(&args, &Collected::new()).expect("the first write failed");
    let before = both_databases_agree(&drive);
    assert_eq!(before.len(), 3);

    // The week after: `a` is carried as it was, `b`'s words changed so it is
    // prepared again, `c` is off the playlist and so off the drive, and `d` is
    // new — in two playlists, which is one track on the drive.
    let mut args =
        ExportArgs { drive: Some(drive.clone()), ..args_for(vec![b.clone(), d.clone()]) };
    args.onelibrary_key = Some(KEY.to_string());
    args.already = rows.into_iter().filter(|(from, _)| *from == a).collect();
    args.playlists = vec![list("Saturday", &[&a, &b, &d]), list("Sunday", &[&d])];
    args.prepared = vec![(
        b.clone(),
        Prep { comment: "the whistle one, second drop".into(), ..Default::default() },
    )];
    commands::export(&args, &Collected::new()).expect("the second write failed");

    let after = both_databases_agree(&drive);
    assert_eq!(after.len(), 3, "{after:#?}");
    assert!(
        after.iter().all(|t| !t.file_path.ends_with("/c.flac")),
        "the track taken off is still in a database: {after:#?}"
    );
    let b_row = after.iter().find(|t| t.file_path.ends_with("/b.flac")).expect("b is gone");
    assert_eq!(b_row.comment, "the whistle one, second drop", "the change did not reach the drive");

    let entries = legacy_playlist_entries(&drive);
    let saturday: Vec<u32> =
        entries.iter().filter(|(l, _)| l == "Saturday").map(|(_, t)| *t).collect();
    let sunday: Vec<u32> = entries.iter().filter(|(l, _)| l == "Sunday").map(|(_, t)| *t).collect();
    assert_eq!(saturday.len(), 3, "{entries:?}");
    assert_eq!(sunday.len(), 1, "{entries:?}");
    assert_eq!(sunday[0], *saturday.last().unwrap(), "d in two playlists is one id");
}

// rekordbox: "one or more files were not exported", a playlist on the stick
// missing tracks that were fine in the app, and nothing to say which or why.
// And a fault of this program's own, found writing the test: a write with one
// bad file in it threw away the rows of every good one, so the next sync
// prepared the whole drive again.
#[test]
fn one_bad_file_costs_neither_the_rest_of_the_drive_nor_its_rows() {
    let scratch = Scratch::new("partial");
    let drive = scratch.path("USB");
    let good = write_song(&scratch, "good.flac");
    let also = write_song(&scratch, "also.flac");
    // A format nothing here decodes, so the writer refuses it rather than
    // putting something unplayable on. It used to be a 192 kHz WAV, which the
    // writer now carries as an mp3.
    let bad = scratch.path("bad.ape");
    std::fs::write(&bad, b"MAC \x96\x0f not a file anything here decodes").unwrap();

    let mut args = ExportArgs {
        drive: Some(drive.clone()),
        ..args_for(vec![good.clone(), bad.clone(), also.clone()])
    };
    args.onelibrary_key = Some(KEY.to_string());
    args.playlists = vec![list("Saturday", &[&good, &bad, &also])];
    let reporter = Collected::new();
    let error = commands::export(&args, &reporter).expect_err("a failed file is still a failure");

    // The failure names the file and says why, as it happens.
    let failed: Vec<(String, String)> = reporter
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Failed { path, message } => Some((path.display().to_string(), message)),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].0.ends_with("bad.ape"), "{failed:?}");
    assert!(failed[0].1.contains("nothing here decodes"), "the reason is the reason: {failed:?}");

    // The drive holds the two that landed, in both databases, and the playlist
    // names those two and nothing else — not a row pointing at a file that is
    // not there.
    let on_drive = both_databases_agree(&drive);
    assert_eq!(on_drive.len(), 2, "{on_drive:#?}");
    let entries = legacy_playlist_entries(&drive);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(!drive.join("Contents").join("bad.ape").exists());

    // And the rows come back with the error, so the next write carries them.
    let partial = error
        .downcast_ref::<PartialWrite>()
        .unwrap_or_else(|| panic!("the error carried no rows: {error:#}"));
    assert_eq!((partial.failed, partial.processed), (1, 3));
    assert_eq!(partial.rows.len(), 2, "{:?}", partial.rows);
    assert!(partial.rows.iter().any(|(from, _)| *from == good));
    assert!(partial.rows.iter().any(|(from, _)| *from == also));
    assert_eq!(error.to_string(), "1 of 3 files failed");

    // Carried, the next write prepares nothing it already has.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(Vec::new()) };
    args.onelibrary_key = Some(KEY.to_string());
    args.already = partial.rows.clone();
    args.playlists = vec![list("Saturday", &[&good, &also])];
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("carrying the rows failed");
    let details = reporter.details().join("\n");
    assert!(details.contains("0 files to prepare, 2 rows carried"), "{details}");
    assert_eq!(both_databases_agree(&drive).len(), 2);
}

// chrisle/rbxport#347: a tool wrote ratings into `exportLibrary.db` on the
// legacy format's 0–255 scale, and a CDJ-3000X — which reads them as stars,
// 0 to 5 — showed none. The two databases keep different scales for the same
// number, which is exactly the kind of thing that drifts when they are written
// by two code paths. Here they are written from one row, and read back off the
// drive as the same number of stars from each.
#[test]
fn a_rating_reads_as_the_same_stars_from_both_databases() {
    let scratch = Scratch::new("stars");
    let drive = scratch.path("USB");
    let song = write_song(&scratch, "four-stars.flac");
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(vec![song.clone()]) };
    args.onelibrary_key = Some(KEY.to_string());
    let rows = commands::export(&args, &Collected::new()).expect("the write failed");

    // The exporter has no rating of its own to write, so give the row one the
    // way a collection would and write the drive again from it.
    let mut rows = rows;
    rows[0].1.rating = 4;
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(Vec::new()) };
    args.onelibrary_key = Some(KEY.to_string());
    args.already = rows;
    commands::export(&args, &Collected::new()).expect("the second write failed");

    let shown = both_databases_agree(&drive);
    assert_eq!(shown[0].rating, 4, "four stars, in both, on the 0–5 scale: {shown:#?}");
}

// Found writing the tests above, and the cousin of the forum's four-hour sync
// that adding one track did not shorten: a sync whose only change was taking a
// track off a drive was refused outright — "no audio files found" — because
// the writer asked for files to prepare and there were none. Nothing to
// prepare is not nothing to do: both databases change, and no audio file does.
#[test]
fn a_sync_that_only_takes_a_track_off_prepares_nothing_and_rewrites_both_databases() {
    let scratch = Scratch::new("removal-only");
    let drive = scratch.path("USB");
    let keep = write_song(&scratch, "keep.flac");
    let drop = write_song(&scratch, "drop.flac");

    let mut args =
        ExportArgs { drive: Some(drive.clone()), ..args_for(vec![keep.clone(), drop.clone()]) };
    args.onelibrary_key = Some(KEY.to_string());
    args.playlists = vec![list("Saturday", &[&keep, &drop])];
    let rows = commands::export(&args, &Collected::new()).expect("the first write failed");
    assert_eq!(both_databases_agree(&drive).len(), 2);

    // The same drive, one track fewer, nothing new: only rows, no files.
    let mut args = ExportArgs { drive: Some(drive.clone()), ..args_for(Vec::new()) };
    args.onelibrary_key = Some(KEY.to_string());
    args.already = rows.into_iter().filter(|(from, _)| *from == keep).collect();
    args.playlists = vec![list("Saturday", &[&keep])];
    let reporter = Collected::new();
    commands::export(&args, &reporter).expect("a removal-only sync was refused");

    let details = reporter.details().join("\n");
    assert!(details.contains("0 files to prepare, 1 row carried"), "{details}");
    assert!(!details.contains("decoded"), "nothing should have been listened to: {details}");
    let shown = both_databases_agree(&drive);
    assert_eq!(shown.len(), 1, "{shown:#?}");
    assert!(shown[0].file_path.ends_with("/keep.flac"));
    assert_eq!(legacy_playlist_entries(&drive).len(), 1);
}
