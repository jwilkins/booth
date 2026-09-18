//! The delta, and the checks that run before a byte is copied.
//!
//! This is the one screen that must never be wrong. Everything it reports is
//! computed from the collection and the files on disk, before anything is
//! written; the verification that runs *after* the write belongs to the export
//! command, which reads its own output back with a parser that shares no code
//! with the writer. Neither half is allowed to stand in for the other.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::library::{Drive, Library, Stamp, Track};

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
        // The name and the colour go on the drive too, so a cue renamed from
        // "drop" to the line that lands there is a change the drive has not
        // got yet. They were left out while the exporter measured its own
        // cues and neither ever reached a player.
        eat(cue.label.as_bytes());
        eat(&cue.color);
    }
    for phrase in &track.phrases {
        eat(&phrase.start_ms.to_le_bytes());
        eat(&phrase.end_ms.to_le_bytes());
        eat(phrase.kind.as_bytes());
    }
    hash
}

/// What the collection knows about a track, for the exporter to write in place
/// of what it would measure.
///
/// The same fields [`fingerprint`] hashes, and deliberately so: the fingerprint
/// is what decides a track needs writing again, and this is what gets written.
/// If one covered something the other did not, an edit would either be written
/// without being noticed or noticed without being written.
pub fn prep(track: &Track) -> booth_cli::export::Prep {
    use booth_cli::export::{Cue, Part, Prep, Rgb};

    let cues = track
        .cues
        .iter()
        .map(|cue| {
            let [r, g, b] = cue.color;
            let placed = match cue.letter {
                0 => Cue::memory(cue.time_ms),
                letter => Cue::hot(letter, cue.time_ms),
            };
            Cue {
                comment: (!cue.label.is_empty()).then(|| cue.label.clone()),
                color: Some(Rgb { r, g, b }),
                ..placed
            }
        })
        .collect();
    let parts = track
        .phrases
        .iter()
        .map(|phrase| Part {
            start_ms: phrase.start_ms,
            end_ms: phrase.end_ms,
            kind: phrase.kind.clone(),
        })
        .collect();

    Prep {
        cues,
        parts,
        // Only where there is a grid behind it. A tempo on a track nobody has
        // analysed is a guess off the file name, and handing a guess to the
        // beat tracker as a fact is how a drive ends up with a grid that is
        // confidently wrong rather than measured.
        bpm: (track.has_grid && track.bpm > 0.0).then_some(track.bpm),
        key: track.key.clone(),
    }
}

// -- what the drive has been doing on its own ------------------------------

/// The three analysis files that sit beside a track on a drive, given the
/// `.DAT` path its database row names.
///
/// The row names one file and the player finds the other two by changing the
/// extension, which is why they are derived here rather than stored.
pub fn analysis_files(analyze_path: &str) -> [String; 3] {
    let stem = analyze_path.trim_start_matches('/');
    let stem = stem.strip_suffix(".DAT").unwrap_or(stem);
    [format!("{stem}.DAT"), format!("{stem}.EXT"), format!("{stem}.2EX")]
}

/// What the drive is holding for each of the tracks written to it.
///
/// Read fresh off the stick: the files as they are now, and the edit counters
/// the OneLibrary database keeps if there is one and the key opens it. A track
/// whose files are not there at all gets no stamp rather than an empty one —
/// nothing was found, which is different from finding nothing changed.
///
/// One walk and one query for the whole drive, because this runs every time the
/// sync sheet is opened and a drive holds thousands of files.
pub fn on_the_drive(root: &Path, drive: &Drive, key: Option<&str>) -> HashMap<u32, Stamp> {
    let counters = onelibrary_counters(root, key);
    let mut found = HashMap::new();

    for written in &drive.written {
        let Some(row) = &written.row else { continue };
        let names = analysis_files(&row.analyze_path);

        let mut files = Vec::new();
        let mut newest: Option<u64> = None;
        for name in &names {
            let Ok(meta) = std::fs::metadata(root.join(name)) else { continue };
            let at = meta
                .modified()
                .ok()
                .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_secs())
                .unwrap_or(0);
            newest = Some(newest.map_or(at, |so_far: u64| so_far.max(at)));
            files.push(format!("{name}:{}:{at}", meta.len()));
        }

        // The row names the `.DAT`, with a leading slash, which is the form the
        // OneLibrary database stores too.
        let counts = counters
            .as_ref()
            .and_then(|by_path| by_path.get(row.analyze_path.as_str()).copied())
            .map(|four| four.to_vec())
            .unwrap_or_default();

        let stamp = Stamp { files, counts, at: newest };
        if !stamp.is_empty() {
            found.insert(written.id, stamp);
        }
    }
    found
}

/// The edit counters off the drive's OneLibrary database, by analysis path.
///
/// `None` when there is no such database, no key for it, or it would not open:
/// all three mean the question could not be asked, and a caller must not read
/// that as the answer being no.
fn onelibrary_counters(root: &Path, key: Option<&str>) -> Option<HashMap<String, [i64; 4]>> {
    let path = booth_cli::rekordbox::onelibrary::find(root)?;
    let key = booth_cli::rekordbox::onelibrary_key(key)?;
    let connection = match booth_cli::rekordbox::open(&path, &key) {
        Ok(connection) => connection,
        Err(e) => {
            crate::debug!("cannot read {} for what the player changed: {e:#}", path.display());
            return None;
        }
    };
    match booth_cli::rekordbox::onelibrary::edits(&connection) {
        Ok(rows) => {
            Some(rows.into_iter().map(|row| (row.analysis_path.clone(), row.counts())).collect())
        }
        Err(e) => {
            crate::debug!("cannot read the edit counters off {}: {e:#}", path.display());
            None
        }
    }
}

/// Read back what the player left, for a track whose drive copy is being kept.
///
/// `None` when the files are not there or will not parse — which is a real
/// answer and the caller has to have one for it: the drive's copy can still be
/// protected by leaving it alone, it just cannot be shown.
pub fn what_the_player_left(
    root: &Path,
    written: &crate::library::Written,
) -> Option<booth_cli::rekordbox::anlz::Analysis> {
    let row = written.row.as_ref()?;
    let names = analysis_files(&row.analyze_path);
    let dat = std::fs::read(root.join(&names[0])).ok()?;
    let ext = std::fs::read(root.join(&names[1])).ok();
    booth_cli::rekordbox::anlz::read_files(&dat, ext.as_deref()).ok()
}

/// Which copy of a track's prep to keep.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// The collection's.
    Mine,
    /// The drive's.
    Theirs,
}

/// A track that has been edited here and on the drive since the two last
/// agreed.
///
/// Nothing can settle this on its own. The times say which is the newer and
/// that is the one to keep — but a time is not a reason, and the one thing
/// worse than losing an edit is losing it silently, so this is put to the
/// person rather than decided for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub id: u32,
    /// When the collection's copy was last edited, if it has ever said.
    pub mine: Option<u64>,
    /// When the drive's was, from the newest of its analysis files.
    pub theirs: Option<u64>,
}

impl Conflict {
    /// Which side the clocks say is right, or `None` when they cannot say —
    /// either because one side has no time, or because they are the same
    /// second and there is nothing to choose between them.
    pub fn newer(&self) -> Option<Side> {
        let (mine, theirs) = (self.mine?, self.theirs?);
        match mine.cmp(&theirs) {
            std::cmp::Ordering::Greater => Some(Side::Mine),
            std::cmp::Ordering::Less => Some(Side::Theirs),
            std::cmp::Ordering::Equal => None,
        }
    }

    /// What to do about it when nobody has said: keep the newer, and keep the
    /// collection's when the clocks will not choose.
    ///
    /// The collection wins a tie because it is the side that can be looked at
    /// and corrected afterwards. Nothing here can read a player's edits back,
    /// so keeping the drive's is keeping something nobody can see.
    pub fn default_side(&self) -> Side {
        self.newer().unwrap_or(Side::Mine)
    }
}

/// Tracks both sides have changed since the last sync.
///
/// `found` is what the drive says now, from [`on_the_drive`]. A track with no
/// stamp on either side is not a conflict: no evidence is not evidence of a
/// change, and refusing to write on the strength of it would make a drive
/// written by an older build unwritable.
pub fn conflicts(library: &Library, drive: &Drive, found: &HashMap<u32, Stamp>) -> Vec<Conflict> {
    let mut clashing = Vec::new();
    for written in &drive.written {
        let Some(track) = library.get(written.id) else { continue };
        if fingerprint(track) == written.prep {
            continue;
        }
        let (Some(before), Some(now)) = (written.theirs.as_ref(), found.get(&written.id)) else {
            continue;
        };
        if before == now {
            continue;
        }
        clashing.push(Conflict { id: written.id, mine: track.edited, theirs: now.at });
    }
    clashing
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

    /// Which copy of a track to keep, when both have moved.
    mod when_the_player_has_been_at_it {
        use super::*;
        use booth_cli::export::pdb;

        /// A drive holding one track, with a row naming its analysis file and
        /// a record of what that file looked like at the time.
        fn drive_holding(library: &Library, id: u32, theirs: Option<Stamp>) -> Drive {
            let mut drive = drive_for(library, &[id]);
            drive.written[0].row = Some(pdb::Track {
                analyze_path: "/PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.DAT".into(),
                ..pdb::Track::default()
            });
            drive.written[0].theirs = theirs;
            drive
        }

        fn stamp(size: u64, at: u64) -> Stamp {
            Stamp {
                files: vec![format!("PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.DAT:{size}:{at}")],
                counts: vec![0, 0, 0, 0],
                at: Some(at),
            }
        }

        /// What the drive says now, as [`on_the_drive`] would return it.
        fn now(id: u32, stamp: Stamp) -> HashMap<u32, Stamp> {
            HashMap::from([(id, stamp)])
        }

        #[test]
        fn a_track_changed_in_both_places_is_a_question() {
            let (mut library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], Some(stamp(4_000, 100)));
            // Edited here since the write.
            let track = library.get_mut(ids[0]).unwrap();
            track.bpm = 130.0;
            track.edited = Some(300);

            let clashing = conflicts(&library, &drive, &now(ids[0], stamp(4_200, 200)));
            assert_eq!(clashing.len(), 1, "{clashing:?}");
            assert_eq!(clashing[0].mine, Some(300));
            assert_eq!(clashing[0].theirs, Some(200));
            // Ours is the later of the two, so that is what it starts on.
            assert_eq!(clashing[0].newer(), Some(Side::Mine));
        }

        #[test]
        fn a_track_only_the_player_touched_is_not_a_question() {
            // Nothing here changed, so nothing here is going to be written
            // over it. There is nothing to ask.
            let (library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], Some(stamp(4_000, 100)));
            assert!(conflicts(&library, &drive, &now(ids[0], stamp(4_200, 200))).is_empty());
        }

        #[test]
        fn a_track_only_we_touched_is_not_a_question_either() {
            let (mut library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], Some(stamp(4_000, 100)));
            library.get_mut(ids[0]).unwrap().bpm = 130.0;
            // The drive is exactly as it was left.
            assert!(conflicts(&library, &drive, &now(ids[0], stamp(4_000, 100))).is_empty());
        }

        #[test]
        fn a_drive_that_was_never_stamped_is_never_refused() {
            // Written by a build from before any of this existed. No evidence
            // is not evidence of a change, and treating it as one would make
            // every older drive unwritable.
            let (mut library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], None);
            library.get_mut(ids[0]).unwrap().bpm = 130.0;
            assert!(conflicts(&library, &drive, &now(ids[0], stamp(9_999, 999))).is_empty());
        }

        #[test]
        fn a_counter_moving_is_a_change_even_when_the_files_have_not() {
            // The columns the format keeps for exactly this. What a player
            // writes into them is undocumented, so they are read as evidence
            // and never as proof of the negative — but evidence is evidence.
            let (mut library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], Some(stamp(4_000, 100)));
            library.get_mut(ids[0]).unwrap().bpm = 130.0;

            let bumped = Stamp { counts: vec![1, 2, 0, 0], ..stamp(4_000, 100) };
            assert_eq!(conflicts(&library, &drive, &now(ids[0], bumped)).len(), 1);
        }

        #[test]
        fn the_later_edit_is_the_one_a_row_starts_on() {
            let later_there = Conflict { id: 1, mine: Some(100), theirs: Some(200) };
            assert_eq!(later_there.default_side(), Side::Theirs);

            let later_here = Conflict { id: 1, mine: Some(300), theirs: Some(200) };
            assert_eq!(later_here.default_side(), Side::Mine);

            // Where the clocks will not choose, the collection keeps it: it is
            // the side somebody can look at and correct afterwards.
            let same = Conflict { id: 1, mine: Some(200), theirs: Some(200) };
            assert_eq!(same.newer(), None);
            assert_eq!(same.default_side(), Side::Mine);

            let unknown = Conflict { id: 1, mine: None, theirs: Some(200) };
            assert_eq!(unknown.newer(), None);
            assert_eq!(unknown.default_side(), Side::Mine);
        }

        #[test]
        fn the_three_files_beside_a_track_are_found_from_the_one_the_row_names() {
            assert_eq!(
                analysis_files("/PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.DAT"),
                [
                    "PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.DAT".to_string(),
                    "PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.EXT".to_string(),
                    "PIONEER/USBANLZ/P016/0000c1b2/ANLZ0000.2EX".to_string(),
                ]
            );
        }

        #[test]
        fn reading_a_real_drive_notices_a_file_that_changed_and_nothing_else() {
            let root =
                std::env::temp_dir().join(format!("booth-drive-edits-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let folder = root.join("PIONEER/USBANLZ/P016/0000c1b2");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("ANLZ0000.DAT"), b"cues as written").unwrap();
            std::fs::write(folder.join("ANLZ0000.EXT"), b"named cues").unwrap();

            let (library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], None);

            let before = on_the_drive(&root, &drive, None);
            let before = before.get(&ids[0]).expect("nothing found for a track that is there");
            assert_eq!(before.files.len(), 2, "the .2EX is not there and is not invented");
            assert!(before.at.is_some());
            // No OneLibrary database to read, which is not the same as four
            // zeroes and must not read as them.
            assert!(before.counts.is_empty());

            // What a deck doing something to the track looks like from here.
            std::fs::write(folder.join("ANLZ0000.EXT"), b"named cues, one of them moved").unwrap();
            let after = on_the_drive(&root, &drive, None);
            assert_ne!(after.get(&ids[0]), Some(before), "an edited file read as unchanged");

            // And reading it twice without touching it does not.
            assert_eq!(on_the_drive(&root, &drive, None), after);

            std::fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_track_whose_files_are_gone_is_not_stamped_as_empty() {
            // Nothing found is not the same as finding nothing, and an empty
            // stamp compared against a real one would read as a change on
            // every sync.
            let (library, ids) = library_with(1);
            let drive = drive_holding(&library, ids[0], None);
            let found = on_the_drive(Path::new("/nowhere-at-all"), &drive, None);
            assert!(found.is_empty(), "{found:?}");
        }
    }

    #[test]
    fn everything_the_fingerprint_watches_is_something_the_prep_carries() {
        // The two have to cover the same ground. Something in the prep that is
        // not in the fingerprint is an edit written without being noticed, and
        // so never written at all; something in the fingerprint that is not in
        // the prep is a track rewritten with the same contents for ever.
        let (mut library, ids) = library_with(1);
        let id = ids[0];
        library.get_mut(id).unwrap().cues.push(crate::library::CueMark {
            letter: 1,
            time_ms: 4_000,
            label: "drop".into(),
            color: [1, 2, 3],
        });
        library.get_mut(id).unwrap().phrases.push(Phrase {
            start_ms: 0,
            end_ms: 8_000,
            kind: "intro".into(),
        });

        let before = (fingerprint(library.get(id).unwrap()), prep(library.get(id).unwrap()));

        // A cue renamed. It goes on the drive, so it has to count as a change.
        library.get_mut(id).unwrap().cues[0].label = "hold me closer now".into();
        let after = (fingerprint(library.get(id).unwrap()), prep(library.get(id).unwrap()));
        assert_ne!(after.0, before.0, "renaming a cue left the drive thinking it was up to date");
        assert_ne!(after.1, before.1);

        // And recoloured.
        library.get_mut(id).unwrap().cues[0].color = [9, 9, 9];
        assert_ne!(fingerprint(library.get(id).unwrap()), after.0);

        // A play count is neither.
        let unchanged = (fingerprint(library.get(id).unwrap()), prep(library.get(id).unwrap()));
        library.get_mut(id).unwrap().play_count += 1;
        assert_eq!(
            (fingerprint(library.get(id).unwrap()), prep(library.get(id).unwrap())),
            unchanged
        );
    }

    #[test]
    fn a_track_nobody_has_analysed_hands_the_exporter_no_tempo_to_trust() {
        // A tempo with no grid behind it is a guess off a file name, and the
        // beat tracker given a guess as a fact produces a grid that is
        // confidently wrong rather than measured.
        let (mut library, ids) = library_with(1);
        library.get_mut(ids[0]).unwrap().has_grid = false;
        assert_eq!(prep(library.get(ids[0]).unwrap()).bpm, None);

        library.get_mut(ids[0]).unwrap().has_grid = true;
        assert_eq!(prep(library.get(ids[0]).unwrap()).bpm, Some(128.0));
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
            written.push(Written {
                id: *id,
                prep: fingerprint(track),
                row: Some(parent),
                stems,
                theirs: None,
            });
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
