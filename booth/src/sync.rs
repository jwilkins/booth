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
    /// Stem files that would go on with them, each with the track it came
    /// from. The parent is kept because a stem is filed under its parent's
    /// artist — the checks below have to ask the same question the writer
    /// does, and the stem's own tags are not what it answers with.
    pub stems: Vec<(u32, PathBuf)>,
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

/// Every track that should be on `drive`, in the order it goes on.
///
/// A track in two of the drive's playlists is one track on the drive, so this
/// is a union that keeps first-seen order: the same file copied twice would be
/// two rows on the player and twice the space.
///
/// The plan is a comparison against this, and the write has to put the same
/// question to the collection — asking it twice in two places is how the two
/// come to disagree about what the drive is supposed to hold.
pub fn wanted(library: &Library, drive: &Drive) -> Vec<u32> {
    let mut wanted: Vec<u32> = Vec::new();
    for name in &drive.playlist_names() {
        let Some(playlist) = library.playlists.iter().find(|p| p.name == *name) else { continue };
        for id in &playlist.tracks {
            if !wanted.contains(id) {
                wanted.push(*id);
            }
        }
    }
    wanted
}

/// What a sync would do to `drive`, given what is in its playlists now.
///
/// A track in two of the drive's playlists is one track on the drive, so the
/// wanted set is a union that keeps first-seen order: the same file copied
/// twice would be two rows on the player and twice the space.
pub fn plan(library: &Library, drive: &Drive) -> Plan {
    let wanted = wanted(library, drive);

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
                plan.stems.push((*id, path.clone()));
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

/// What the next write hands the exporter: the rows it can carry rather than
/// make again, and the files it has to prepare.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Carry {
    /// Rows already on the drive, each with the file it was made from. Handed
    /// straight to the exporter, which builds its database from these and what
    /// this run adds — so the database describes the whole drive rather than
    /// the last thing done to it.
    pub already: Vec<(PathBuf, booth_cli::export::pdb::Track)>,
    /// The tracks whose rows are being carried, so the drive's record can keep
    /// what it already knew about them.
    pub carried: Vec<u32>,
    /// Every file the exporter has to prepare: the tracks being written, then
    /// the stems going on with them.
    pub files: Vec<PathBuf>,
}

/// Work out what the next write to `drive` carries and what it prepares.
///
/// A drive is written once and then added to. Preparing a track means decoding
/// it, so the second write is given only what changed and the rest of the
/// database is carried through from what the last write recorded.
///
/// A stem carries on the same terms as its parent and never on its own. It
/// takes the parent's grid, cues, key and phrases rather than being listened to
/// alone, so a parent whose prep changed is a stem whose analysis is now wrong
/// — and carrying the stem's row while rewriting the parent's would put a row
/// on the drive describing a grid that is no longer there.
///
/// An image is made from nothing every time: there is no previous volume to add
/// to, so it carries nothing and prepares everything.
pub fn carry(library: &Library, drive: &Drive, plan: &Plan) -> Carry {
    let wanted = wanted(library, drive);
    let rewriting = plan.writes();

    let mut out = Carry::default();
    if !drive.is_image {
        for written in &drive.written {
            if !wanted.contains(&written.id) || rewriting.contains(&written.id) {
                continue;
            }
            // A row that was never recorded — an older collection, or a write
            // that failed — cannot be carried, so its file is prepared again.
            let (Some(track), Some(row)) = (library.get(written.id), written.row.clone()) else {
                continue;
            };
            out.already.push((track.path.clone(), row));
            out.carried.push(track.id);
            if !drive.with_stems {
                continue;
            }
            for (path, row) in &written.stems {
                if track.stems.has(path) {
                    out.already.push((path.clone(), row.clone()));
                }
            }
        }
    }

    out.files = wanted
        .iter()
        .filter(|id| !out.carried.contains(id))
        .filter_map(|id| library.get(*id))
        .map(|track| track.path.clone())
        .chain(
            plan.stems
                .iter()
                .filter(|(_, path)| !out.already.iter().any(|(carried, _)| carried == path))
                .map(|(_, path)| path.clone()),
        )
        .collect();
    out
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
        tracks.iter().filter(|t| !booth_cli::commands::is_playable(&t.format)).collect();
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
    //
    // Stems are measured too, and against their parent's artist, because that
    // is the folder the writer puts them in. They are the ones this catches:
    // a stem's name is its parent's plus "-vocals", so a track whose path fits
    // can have three companions whose paths do not.
    let long = tracks.iter().filter(|track| too_long(&track.artist, &track.path)).count()
        + plan
            .stems
            .iter()
            .filter(|(parent, path)| {
                library.get(*parent).is_some_and(|track| too_long(&track.artist, path))
            })
            .count();
    checks.push(match long {
        0 => Check {
            level: Level::Ok,
            text: format!(
                "all paths are under {} characters",
                booth_cli::commands::MAX_ON_DRIVE_PATH
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

/// Whether a file would land on a path a player will not follow, asked with the
/// writer's own rule so that the two cannot come to disagree.
fn too_long(artist: &str, path: &Path) -> bool {
    let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    booth_cli::commands::on_drive_path(artist, &filename).len()
        > booth_cli::commands::MAX_ON_DRIVE_PATH
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

/// Room for the analysis files, which are written per file and are not part of
/// any file's size on disk. Two megabytes each is generous for anything under
/// about half an hour long — the same allowance the writer sizes an image with.
///
/// Stems count. Each one is a row on the player with its own grid, waveform and
/// cues, so a drive carrying them needs four times this, not one.
fn analysis_allowance(plan: &Plan) -> u64 {
    (plan.writes().len() + plan.stems.len()) as u64 * 2 * 1024 * 1024
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
                .map(|id| Written {
                    id: *id,
                    prep: fingerprint(library.get(*id).unwrap()),
                    ..Written::default()
                })
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
    fn a_stem_whose_path_is_too_long_is_caught_before_the_write() {
        let (mut library, ids) = library_with(1);
        // The track's own path is short. Its stem's is not: a stem is named
        // after its parent plus the part it is, so a track that fits can carry
        // three companions that do not, and the folder the length is measured
        // against is the parent's.
        library.get_mut(ids[0]).unwrap().stems.vocals =
            Some(PathBuf::from(format!("/stems/{}-vocals.wav", "B".repeat(240))));

        let drive = Drive { with_stems: true, ..drive_for(&library, &[]) };
        let plan = plan(&library, &drive);
        let checks = preflight(&library, &plan, Path::new("/tmp"), false);
        assert!(
            checks.iter().any(|c| c.level == Level::Bad && c.text.contains("longer")),
            "a stem the writer will refuse passed the preflight: {checks:#?}"
        );
    }

    #[test]
    fn the_room_left_for_analysis_counts_the_stems_as_well() {
        let (mut library, ids) = library_with(1);
        let track = library.get_mut(ids[0]).unwrap();
        track.stems.vocals = Some("/stems/a-vocals.wav".into());
        track.stems.drums = Some("/stems/a-drums.wav".into());
        track.stems.melody = Some("/stems/a-melody.wav".into());

        let bare = analysis_allowance(&plan(&library, &drive_for(&library, &[])));
        let carrying = analysis_allowance(&plan(
            &library,
            &Drive { with_stems: true, ..drive_for(&library, &[]) },
        ));
        // Each stem is a row on the player, with a grid, a waveform and cues of
        // its own on the drive. A drive carrying them needs four times the room
        // for analysis, not the same amount.
        assert_eq!(carrying, bare * 4, "a stem was not given room to be analysed");
    }

    /// Give a track a whole kit, named the way the renderer names one.
    fn give_stems(library: &mut Library, id: u32) {
        let track = library.get_mut(id).unwrap();
        let name = track.path.file_stem().unwrap().to_string_lossy().into_owned();
        track.stems.vocals = Some(PathBuf::from(format!("/stems/{name}-vocals.wav")));
        track.stems.drums = Some(PathBuf::from(format!("/stems/{name}-drums.wav")));
        track.stems.melody = Some(PathBuf::from(format!("/stems/{name}-melody.wav")));
    }

    /// A drive that has been written, with a row recorded for each track on it
    /// and — when it carries them — one for each stem that went on with it.
    fn written_drive(library: &Library, ids: &[u32], with_stems: bool) -> Drive {
        let mut next = 0u32;
        let mut row = || {
            next += 1;
            booth_cli::export::pdb::Track { id: next, ..Default::default() }
        };
        let mut written = Vec::new();
        for id in ids {
            let track = library.get(*id).unwrap();
            let parent = row();
            let mut stems = Vec::new();
            if with_stems {
                for (_, path) in track.stems.each() {
                    let Some(path) = path else { continue };
                    stems.push((path.clone(), row()));
                }
            }
            written.push(Written { id: *id, prep: fingerprint(track), row: Some(parent), stems });
        }
        Drive { with_stems, written, ..drive_for(library, &[]) }
    }

    #[test]
    fn a_carried_track_brings_its_stem_rows_with_it() {
        let (mut library, ids) = library_with(2);
        for id in &ids {
            give_stems(&mut library, *id);
        }
        // The first track is on the drive with its kit; the second is new.
        let drive = written_drive(&library, &ids[..1], true);
        let carry = carry(&library, &drive, &plan(&library, &drive));

        assert_eq!(carry.carried, vec![ids[0]]);
        assert_eq!(carry.already.len(), 4, "the track and its three stems: {:?}", carry.already);

        // Not one of the four is decoded a second time.
        let first = library.get(ids[0]).unwrap();
        assert!(!carry.files.contains(&first.path), "the track was prepared again");
        for (part, path) in first.stems.each() {
            assert!(!carry.files.contains(path.unwrap()), "{part} was prepared again");
        }
        assert_eq!(carry.files.len(), 4, "only the new track and its kit: {:?}", carry.files);
    }

    #[test]
    fn a_track_written_again_takes_its_stems_with_it() {
        let (mut library, ids) = library_with(1);
        give_stems(&mut library, ids[0]);
        let drive = written_drive(&library, &ids, true);

        // A cue moves. A stem takes its cues from its parent, so a parent whose
        // prep changed is three stems whose analysis is now wrong — carrying
        // their rows would leave the drive describing a grid that is not there.
        library.get_mut(ids[0]).unwrap().cues.push(CueMark {
            letter: 1,
            time_ms: 32_000,
            label: "drop".into(),
            color: [0, 0, 0],
        });

        let plan = plan(&library, &drive);
        assert_eq!(plan.update.len(), 1, "the track should read as changed");
        let carry = carry(&library, &drive, &plan);
        assert!(carry.already.is_empty(), "nothing can be carried: {:?}", carry.already);
        assert_eq!(carry.files.len(), 4, "the track and all three stems go again");
    }

    #[test]
    fn a_stem_the_kit_no_longer_names_is_not_carried() {
        let (mut library, ids) = library_with(1);
        give_stems(&mut library, ids[0]);
        let drive = written_drive(&library, &ids, true);

        // One part re-rendered to another format: the same sound, a different
        // file, and a row that now describes something not on the drive.
        let again = PathBuf::from("/stems/track0-vocals.mp3");
        library.get_mut(ids[0]).unwrap().stems.vocals = Some(again.clone());

        let carry = carry(&library, &drive, &plan(&library, &drive));
        assert_eq!(
            carry.already.len(),
            3,
            "the track and the two parts still named: {:?}",
            carry.already
        );
        assert_eq!(carry.files, vec![again], "only the re-rendered part is made again");
    }

    #[test]
    fn a_drive_that_no_longer_carries_stems_carries_none_of_their_rows() {
        let (mut library, ids) = library_with(1);
        give_stems(&mut library, ids[0]);
        let drive = Drive { with_stems: false, ..written_drive(&library, &ids, true) };

        let carry = carry(&library, &drive, &plan(&library, &drive));
        assert_eq!(carry.already.len(), 1, "only the track's own row: {:?}", carry.already);
        assert!(carry.files.is_empty(), "{:?}", carry.files);
    }

    #[test]
    fn an_image_carries_nothing_and_prepares_everything() {
        let (mut library, ids) = library_with(1);
        give_stems(&mut library, ids[0]);
        let drive = Drive { is_image: true, ..written_drive(&library, &ids, true) };

        // An image is made from nothing every time: there is no previous volume
        // to add to, so a row recorded against one cannot be carried into it.
        let carry = carry(&library, &drive, &plan(&library, &drive));
        assert!(carry.already.is_empty() && carry.carried.is_empty());
        assert_eq!(carry.files.len(), 4);
    }

    #[test]
    fn a_collection_written_before_stem_rows_were_kept_makes_them_again() {
        let (mut library, ids) = library_with(1);
        give_stems(&mut library, ids[0]);
        let mut drive = written_drive(&library, &ids, true);
        // What an older build recorded: the track's row and nothing about its
        // stems. The track still carries; the stems are prepared afresh.
        drive.written[0].stems.clear();

        let kept = carry(&library, &drive, &plan(&library, &drive));
        assert_eq!(kept.already.len(), 1, "{:?}", kept.already);
        assert_eq!(kept.files.len(), 3, "its stems are made again: {:?}", kept.files);

        // And a stem row cannot be carried without its parent's, because the
        // parent is then what is being written again.
        drive.written[0].row = None;
        let rowless = carry(&library, &drive, &plan(&library, &drive));
        assert!(rowless.already.is_empty());
        assert_eq!(rowless.files.len(), 4);
    }

    #[test]
    fn a_record_from_an_older_build_reads_without_its_stems() {
        // The collection on disk is JSON, and one written before stems were
        // recorded has no such field. It has to load as a track with none
        // rather than fail the whole collection.
        let older = r#"{"id": 7, "prep": 12345, "row": null}"#;
        let written: Written = serde_json::from_str(older).expect("an older record should load");
        assert_eq!(written.id, 7);
        assert!(written.stems.is_empty());
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
