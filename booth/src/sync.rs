//! The delta, and the checks that run before a byte is copied.
//!
//! This is the one screen that must never be wrong. Everything it reports is
//! computed from the collection and the files on disk, before anything is
//! written; the verification that runs *after* the write belongs to the export
//! command, which reads its own output back with a parser that shares no code
//! with the writer. Neither half is allowed to stand in for the other.

use std::path::{Path, PathBuf};

use crate::library::{Drive, Library, Track};

/// What one sync would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Tracks not on the drive at all.
    pub add: Vec<u32>,
    /// Tracks whose prep has changed since they were written, and what changed.
    pub update: Vec<(u32, String)>,
    /// Tracks on the drive that are no longer in the playlist.
    pub remove: Vec<u32>,
    /// Stem files that would go on with them.
    pub stems: Vec<PathBuf>,
    pub add_bytes: u64,
    pub stem_bytes: u64,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.update.is_empty() && self.remove.is_empty()
    }

    /// The line the dock shows, permanently, because it is the number that
    /// decides whether tonight goes well.
    pub fn delta(&self) -> String {
        if self.is_empty() {
            return "up to date".to_string();
        }
        let mut parts = Vec::new();
        if !self.add.is_empty() {
            parts.push(format!("{} to add", self.add.len()));
        }
        if !self.update.is_empty() {
            parts.push(format!("{} changed", self.update.len()));
        }
        if !self.remove.is_empty() {
            parts.push(format!("{} to remove", self.remove.len()));
        }
        if !self.stems.is_empty() {
            parts.push(format!("{} stems", self.stems.len()));
        }
        parts.join(" · ")
    }

    /// Every track the write would touch, in the order it would touch them.
    pub fn writes(&self) -> Vec<u32> {
        self.add.iter().chain(self.update.iter().map(|(id, _)| id)).copied().collect()
    }
}

/// What a sync would do to `drive`, given what is in its playlists now.
///
/// A track in two of the drive's playlists is one track on the drive, so the
/// wanted set is a union that keeps first-seen order: the same file copied
/// twice would be two rows on the player and twice the space.
pub fn plan(library: &Library, drive: &Drive) -> Plan {
    let names = drive.playlist_names();
    let mut wanted: Vec<u32> = Vec::new();
    for name in &names {
        let Some(playlist) = library.playlists.iter().find(|p| p.name == *name) else { continue };
        for id in &playlist.tracks {
            if !wanted.contains(id) {
                wanted.push(*id);
            }
        }
    }

    let mut plan = Plan::default();
    for id in &wanted {
        let Some(track) = library.get(*id) else { continue };
        match drive.written.iter().find(|w| w.id == *id) {
            None => {
                plan.add.push(*id);
                plan.add_bytes += track.bytes;
            }
            Some(written) => {
                let now = fingerprint(track);
                if now != written.prep {
                    plan.update.push((*id, changed_text(track)));
                }
            }
        }
        if drive.with_stems {
            for (_, path) in track.stems.each() {
                let Some(path) = path else { continue };
                plan.stem_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                plan.stems.push(path.clone());
            }
        }
    }

    for written in &drive.written {
        if !wanted.contains(&written.id) {
            plan.remove.push(written.id);
        }
    }
    plan
}

/// A short account of what is different about a track's prep.
///
/// Deliberately vague about *which* cue moved: the sheet's job is to say that
/// something the player will notice has changed, and a diff of eight cue times
/// is not something anyone reads at five to midnight.
fn changed_text(track: &Track) -> String {
    let mut parts = Vec::new();
    if track.has_grid {
        parts.push(format!("{:.2} BPM", track.bpm));
    }
    if !track.key.is_empty() {
        parts.push(track.key.clone());
    }
    parts.push(format!("{} cues", track.cues.len()));
    parts.push(format!("{} phrases", track.phrases.len()));
    format!("prep changed — {}", parts.join(", "))
}

/// A stable summary of everything about a track that ends up on the drive.
///
/// FNV-1a over a canonical encoding rather than the standard library's hasher,
/// because this number is written to disk and compared against on a later run,
/// possibly by a differently built binary. A hash that is only stable within one
/// process would report the whole drive as changed after an upgrade.
pub fn fingerprint(track: &Track) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };

    eat(track.artist.as_bytes());
    eat(track.title.as_bytes());
    // To two decimals, which is the precision the format stores and the player
    // displays: a grid that differs below that is not a grid that changed.
    eat(format!("{:.2}", track.bpm).as_bytes());
    eat(track.key.as_bytes());
    eat(&[track.has_grid as u8]);
    for cue in &track.cues {
        eat(&[cue.letter]);
        eat(&cue.time_ms.to_le_bytes());
    }
    for phrase in &track.phrases {
        eat(&phrase.start_ms.to_le_bytes());
        eat(&phrase.end_ms.to_le_bytes());
        eat(phrase.kind.as_bytes());
    }
    hash
}

/// How serious a preflight finding is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Checked, and fine.
    Ok,
    /// It will be written, and something about it is worse than it should be.
    Warn,
    /// The writer will refuse this file. The rest of the drive still goes.
    Bad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub text: String,
}

/// Everything checkable before the write starts.
///
/// Each of these is a state that looks fine in a file browser and fails in a
/// booth — which is the only reason any of them is worth a line on the screen.
pub fn preflight(library: &Library, plan: &Plan, destination: &Path, is_image: bool) -> Vec<Check> {
    let tracks: Vec<&Track> = plan.writes().iter().filter_map(|id| library.get(*id)).collect();
    let mut checks = Vec::new();

    // -- formats a player cannot open
    let unplayable: Vec<&&Track> =
        tracks.iter().filter(|t| !musicai::commands::is_playable(&t.format)).collect();
    checks.push(match unplayable.len() {
        0 => Check {
            level: Level::Ok,
            text: format!("{} files are formats a player opens", tracks.len()),
        },
        n => Check {
            level: Level::Bad,
            text: format!(
                "{n} files a player cannot open ({}) — they will be skipped",
                list(unplayable.iter().map(|t| t.format.as_str())),
            ),
        },
    });

    // -- 32-bit float WAVs
    let floats = tracks.iter().filter(|t| t.float_samples).count();
    if floats > 0 {
        checks.push(Check {
            level: Level::Warn,
            text: format!(
                "{floats} files are 32-bit float WAV — they copy, and a player will refuse to \
                 load them. Re-encode to 24-bit first."
            ),
        });
    }

    // -- sample rates
    let fast = tracks.iter().filter(|t| t.sample_rate > 96_000).count();
    checks.push(match fast {
        0 => Check { level: Level::Ok, text: "all sample rates are 96 kHz or below".into() },
        n => Check {
            level: Level::Bad,
            text: format!("{n} files are above the 96 kHz a player will accept"),
        },
    });

    // -- path lengths, worked out with the writer's own rule
    let long: Vec<&&Track> = tracks
        .iter()
        .filter(|track| {
            let filename = track
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            musicai::commands::on_drive_path(&track.artist, &filename).len()
                > musicai::commands::MAX_ON_DRIVE_PATH
        })
        .collect();
    checks.push(match long.len() {
        0 => Check {
            level: Level::Ok,
            text: format!(
                "all paths are under {} characters",
                musicai::commands::MAX_ON_DRIVE_PATH
            ),
        },
        n => Check {
            level: Level::Bad,
            text: format!("{n} paths are longer than a player will follow"),
        },
    });

    // -- beat grids
    let ungridded = tracks.iter().filter(|t| !t.has_grid).count();
    if ungridded > 0 {
        checks.push(Check {
            level: Level::Warn,
            text: format!(
                "{ungridded} files have no beat grid — they will load, without sync or quantize"
            ),
        });
    }

    checks.push(space(plan, destination, is_image));
    checks
}

/// Whether what is being written will fit.
fn space(plan: &Plan, destination: &Path, is_image: bool) -> Check {
    let needed = plan.add_bytes + plan.stem_bytes + analysis_allowance(plan);
    if is_image {
        // An image is created at the size the write needs, so the question is
        // whether the disk under it has room, not whether the image does.
        return Check {
            level: Level::Ok,
            text: format!("a {} image will be created", bytes(needed)),
        };
    }
    match free_space(destination) {
        None => Check {
            level: Level::Warn,
            text: format!("{} to write; free space on the drive is unknown", bytes(needed)),
        },
        Some(free) if free > needed => Check {
            level: Level::Ok,
            text: format!("{} to write, {} free after it", bytes(needed), bytes(free - needed)),
        },
        Some(free) => Check {
            level: Level::Bad,
            text: format!("{} to write and only {} free", bytes(needed), bytes(free)),
        },
    }
}

/// Room for the analysis files, which are written per track and are not part of
/// any file's size on disk. Two megabytes each is generous for anything under
/// about half an hour long — the same allowance the writer sizes an image with.
fn analysis_allowance(plan: &Plan) -> u64 {
    plan.writes().len() as u64 * 2 * 1024 * 1024
}

/// Free bytes on the filesystem holding `path`, if it can be found out.
///
/// There is no portable way to ask, so on the platforms that answer the answer
/// is used, and on the ones that do not the check says it does not know rather
/// than guessing. A preflight that invents a free-space number is worse than
/// one that admits it has none.
#[cfg(unix)]
fn free_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let existing = nearest_existing(path)?;
    let c_path = CString::new(existing.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is a valid NUL-terminated string that outlives the call,
    // and `statvfs` either fills the struct and returns zero or returns non-zero
    // and leaves it alone — which is why it is only read on success.
    let filled = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if filled != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    // The casts are redundant on 64-bit and not on 32-bit, where these fields
    // are `c_ulong`.
    #[allow(clippy::unnecessary_cast)]
    (stat.f_bavail as u64).checked_mul(stat.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// The path itself if it exists, else the nearest parent that does — a drive
/// folder that has not been created yet still sits on a filesystem with a size.
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut at = path;
    loop {
        if at.exists() {
            return Some(at.to_path_buf());
        }
        at = at.parent()?;
    }
}

/// A size, in the units a person reads.
pub fn bytes(count: u64) -> String {
    const UNITS: [(&str, u64); 4] =
        [("TB", 1 << 40), ("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)];
    for (unit, size) in UNITS {
        if count >= size {
            return format!("{:.1} {unit}", count as f64 / size as f64);
        }
    }
    format!("{count} B")
}

/// A few of something, and then "+n".
fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let mut seen: Vec<&str> = items.collect();
    seen.sort_unstable();
    seen.dedup();
    match seen.len() {
        0..=3 => seen.join(", "),
        n => format!("{}, +{}", seen[..3].join(", "), n - 3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{CueMark, Phrase, Playlist, Written};

    fn library_with(tracks: usize) -> (Library, Vec<u32>) {
        let mut library = Library::new();
        let mut ids = Vec::new();
        for i in 0..tracks {
            let id = library.add(Path::new(&format!("/music/track{i}.flac")));
            let track = library.get_mut(id).unwrap();
            track.artist = format!("Artist {i}");
            track.title = format!("Track {i}");
            track.bytes = 40 * 1024 * 1024;
            track.bpm = 128.0;
            track.key = "8A".into();
            track.has_grid = true;
            track.analyzed = true;
            track.sample_rate = 44_100;
            ids.push(id);
        }
        library.playlists.push(Playlist {
            name: "peak".into(),
            folder: "Sat".into(),
            tracks: ids.clone(),
        });
        (library, ids)
    }

    fn drive_for(library: &Library, written: &[u32]) -> Drive {
        Drive {
            label: "SANDISK-64".into(),
            path: PathBuf::from("/Volumes/SANDISK-64"),
            playlist: "peak".into(),
            written: written
                .iter()
                .map(|id| Written { id: *id, prep: fingerprint(library.get(*id).unwrap()) })
                .collect(),
            ..Drive::default()
        }
    }

    #[test]
    fn an_empty_drive_is_all_additions() {
        let (library, ids) = library_with(3);
        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);

        assert_eq!(plan.add, ids);
        assert!(plan.update.is_empty() && plan.remove.is_empty());
        assert_eq!(plan.add_bytes, 3 * 40 * 1024 * 1024);
        assert_eq!(plan.delta(), "3 to add");
    }

    #[test]
    fn a_drive_that_matches_the_playlist_has_nothing_to_do() {
        let (library, ids) = library_with(3);
        let drive = drive_for(&library, &ids);
        let plan = plan(&library, &drive);

        assert!(plan.is_empty());
        assert_eq!(plan.delta(), "up to date");
    }

    #[test]
    fn moving_a_cue_makes_a_track_an_update_rather_than_an_addition() {
        let (mut library, ids) = library_with(3);
        let drive = drive_for(&library, &ids);

        library.get_mut(ids[1]).unwrap().cues.push(CueMark {
            letter: 3,
            time_ms: 64_000,
            label: "drop".into(),
            color: [0, 0, 0],
        });

        let plan = plan(&library, &drive);
        assert!(plan.add.is_empty(), "the file is already there");
        assert_eq!(plan.update.len(), 1);
        assert_eq!(plan.update[0].0, ids[1]);
        assert!(plan.update[0].1.contains("1 cues"), "{}", plan.update[0].1);
        assert_eq!(plan.delta(), "1 changed");
    }

    #[test]
    fn a_track_taken_out_of_the_playlist_is_a_removal() {
        let (mut library, ids) = library_with(3);
        let drive = drive_for(&library, &ids);
        library.playlists[0].tracks.retain(|id| *id != ids[0]);

        let plan = plan(&library, &drive);
        assert_eq!(plan.remove, vec![ids[0]]);
        assert!(plan.add.is_empty());
    }

    #[test]
    fn the_fingerprint_notices_prep_and_ignores_the_rest() {
        let (mut library, ids) = library_with(1);
        let before = fingerprint(library.get(ids[0]).unwrap());

        // Things the player will see.
        library.get_mut(ids[0]).unwrap().bpm = 130.0;
        assert_ne!(fingerprint(library.get(ids[0]).unwrap()), before);
        library.get_mut(ids[0]).unwrap().bpm = 128.0;
        assert_eq!(fingerprint(library.get(ids[0]).unwrap()), before);

        library.get_mut(ids[0]).unwrap().phrases.push(Phrase {
            start_ms: 0,
            end_ms: 1_000,
            kind: "intro".into(),
        });
        assert_ne!(fingerprint(library.get(ids[0]).unwrap()), before);
        library.get_mut(ids[0]).unwrap().phrases.clear();

        // Things it will not: play counts, tags, when it was added.
        let track = library.get_mut(ids[0]).unwrap();
        track.play_count += 7;
        track.tags.push("peak".into());
        track.last_played = Some(1);
        assert_eq!(
            fingerprint(library.get(ids[0]).unwrap()),
            before,
            "a play count should not rewrite the drive"
        );
    }

    #[test]
    fn a_tempo_below_the_stored_precision_is_not_a_change() {
        let (mut library, ids) = library_with(1);
        let before = fingerprint(library.get(ids[0]).unwrap());
        // The format stores hundredths, so anything under that is not visible
        // to the player and must not mark the track for rewriting.
        library.get_mut(ids[0]).unwrap().bpm = 128.0001;
        assert_eq!(fingerprint(library.get(ids[0]).unwrap()), before);
    }

    #[test]
    fn a_drive_plans_across_every_playlist_it_carries() {
        let (mut library, ids) = library_with(4);
        // Two lists that overlap: a track in both is one track on the drive.
        library.playlists[0].tracks = vec![ids[0], ids[1]];
        library.playlists.push(Playlist {
            name: "warm".into(),
            folder: "Sat".into(),
            tracks: vec![ids[1], ids[2]],
        });
        let drive =
            Drive { playlists: vec!["peak".into(), "warm".into()], ..drive_for(&library, &[]) };

        let plan = plan(&library, &drive);
        assert_eq!(plan.add, vec![ids[0], ids[1], ids[2]], "the union, in first-seen order");
        assert_eq!(plan.add_bytes, 3 * 40 * 1024 * 1024, "the shared track is not paid for twice");
        assert!(!plan.add.contains(&ids[3]), "a track in neither list stays off");
    }

    #[test]
    fn taking_a_playlist_off_a_drive_marks_what_only_it_wanted_for_removal() {
        let (mut library, ids) = library_with(2);
        library.playlists[0].tracks = vec![ids[0]];
        library.playlists.push(Playlist {
            name: "warm".into(),
            folder: String::new(),
            tracks: vec![ids[1]],
        });

        // Both were written; now only one list is carried.
        let drive = Drive { playlists: vec!["peak".into()], ..drive_for(&library, &ids) };
        let plan = plan(&library, &drive);
        assert_eq!(plan.remove, vec![ids[1]]);
        assert!(plan.add.is_empty());
    }

    #[test]
    fn stems_go_on_only_when_the_drive_carries_them() {
        let (mut library, ids) = library_with(1);
        let track = library.get_mut(ids[0]).unwrap();
        track.stems.vocals = Some("/stems/a-vocals.wav".into());
        track.stems.melody = Some("/stems/a-melody.wav".into());
        track.stems.drums = Some("/stems/a-drums.wav".into());

        let mut drive = drive_for(&library, &[]);
        assert!(plan(&library, &drive).stems.is_empty());

        drive.with_stems = true;
        assert_eq!(plan(&library, &drive).stems.len(), 3);
    }

    #[test]
    fn a_clean_collection_passes_every_check() {
        let (library, _) = library_with(2);
        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);

        let checks = preflight(&library, &plan, Path::new("/tmp"), false);
        let worst = checks.iter().map(|c| c.level).max().unwrap();
        assert_eq!(worst, Level::Ok, "{checks:#?}");
    }

    #[test]
    fn the_things_that_fail_silently_in_a_booth_each_get_a_line() {
        let (mut library, ids) = library_with(4);
        library.get_mut(ids[0]).unwrap().float_samples = true;
        library.get_mut(ids[1]).unwrap().sample_rate = 192_000;
        library.get_mut(ids[2]).unwrap().has_grid = false;
        // A file no player opens.
        library.get_mut(ids[3]).unwrap().format = "ogg".into();

        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);
        let checks = preflight(&library, &plan, Path::new("/tmp"), false);
        let text = checks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("\n");

        assert!(text.contains("float"), "{text}");
        assert!(text.contains("96 kHz"), "{text}");
        assert!(text.contains("beat grid"), "{text}");
        assert!(text.contains("cannot open"), "{text}");
        assert_eq!(checks.iter().map(|c| c.level).max().unwrap(), Level::Bad);
    }

    #[test]
    fn a_long_path_is_predicted_with_the_writers_own_rule() {
        let (mut library, ids) = library_with(1);
        let track = library.get_mut(ids[0]).unwrap();
        // Long enough that /Contents/<artist>/<file> runs past what a player
        // will follow, even after the writer truncates the artist folder.
        track.artist = "A".repeat(200);
        track.path = PathBuf::from(format!("/music/{}.flac", "B".repeat(200)));

        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);
        let checks = preflight(&library, &plan, Path::new("/tmp"), false);
        assert!(
            checks.iter().any(|c| c.level == Level::Bad && c.text.contains("longer")),
            "{checks:#?}"
        );
    }

    #[test]
    fn an_image_is_sized_to_the_write_rather_than_checked_against_a_disk() {
        let (library, _) = library_with(2);
        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);

        let checks = preflight(&library, &plan, Path::new("/tmp/drive.img"), true);
        let space = checks.last().unwrap();
        assert_eq!(space.level, Level::Ok);
        assert!(space.text.contains("image will be created"), "{}", space.text);
    }

    #[test]
    fn free_space_is_reported_when_the_platform_will_say() {
        // On the platforms that answer, a real directory has a real size; on
        // the ones that do not, the check says so instead of guessing.
        let answer = free_space(Path::new("."));
        if cfg!(unix) {
            assert!(answer.is_some_and(|free| free > 0), "{answer:?}");
        }
    }

    #[test]
    fn a_write_that_will_not_fit_is_a_blocker() {
        let (mut library, ids) = library_with(2);
        for id in &ids {
            // Larger than any disk this will run on.
            library.get_mut(*id).unwrap().bytes = 900 * 1024 * 1024 * 1024;
        }
        let drive = drive_for(&library, &[]);
        let plan = plan(&library, &drive);

        let checks = preflight(&library, &plan, Path::new("."), false);
        let space = checks.last().unwrap();
        if free_space(Path::new(".")).is_some() {
            assert_eq!(space.level, Level::Bad, "{}", space.text);
            assert!(space.text.contains("only"), "{}", space.text);
        }
    }

    #[test]
    fn sizes_read_the_way_a_person_says_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "3.5 GB");
    }
}
