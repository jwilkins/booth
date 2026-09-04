//! Keeping a copy of what was on a drive.
//!
//! # Why this exists
//!
//! A prepared drive is hours of work that lives in one place, on the cheapest
//! component in the booth. The audio is replaceable — it is in the library, or
//! it can be bought again. What is not replaceable is everything *around* the
//! audio: the cues placed on a Sunday afternoon, the grids nudged into line,
//! the playlists in the order they are meant to be played, the play history a
//! player wrote back after a gig. All of that is a few files under `PIONEER`,
//! and when a stick dies it goes with it.
//!
//! So: any drive this program writes, and any drive that looks like a player's
//! that gets plugged in while it is running, gets those files copied somewhere
//! safe.
//!
//! # Why it is a directory, for now
//!
//! A backup is an ordinary directory:
//!
//! ```text
//! <backups>/<drive>/<when>/
//!     PIONEER/…            copied, byte for byte
//!     Contents/…           links to the library's own copy of each file
//!     backup.json          what was found, what was linked, what was not
//! ```
//!
//! What cannot be done is putting the whole drive in an archive: a zip holds
//! contents and not links, so zipping a drive means copying every byte of its
//! audio, which is the thing the links exist to avoid.
//!
//! What *can* be done, and is the intended next step once there is a month of
//! these to measure, is zipping the copied half alone — the `PIONEER` tree
//! becomes one `PIONEER.zip` and the links stay beside it in the same folder.
//! It saves whatever that tree compresses by, which is worth knowing before
//! choosing: the analysis files are mostly waveform data and will not compress
//! much, `export.pdb` is fixed-size pages padded with zeroes and will compress
//! a lot, and `exportLibrary.db` is encrypted and will not compress at all.
//!
//! A directory is also the easiest thing to get *out* of again — the restore is
//! a copy, with no tool in the middle — which is the other reason to start
//! here and add the archiving once the sizes are known.
//!
//! # What the links point at
//!
//! The library's copy of the audio, never the drive's. A link into the drive
//! would dangle the moment it is unplugged, which is exactly when the backup
//! matters — and a hard link cannot cross from a stick to an internal disk
//! anyway.
//!
//! A track is the library's if a file there has the same name and length, which
//! is true of everything on a drive this program wrote. Where that fails, the
//! drive's file is hashed the way the duplicate finder hashes one — the audio
//! stream alone, tags skipped — so the same recording tagged differently or
//! renamed still counts as owned rather than missing. That costs a read of the
//! file, and is only ever reached by a file the cheap test did not settle.
//!
//! # Somebody else's drive
//!
//! What is left after that is music the library does not have, and what to do
//! with it is [`crate::config::OnForeign`]: name it and store none of it, copy
//! it into the backup so that copy is complete on its own, or copy it into the
//! library and keep it. The first is the default, because the other two are
//! measured in gigabytes and plugging a stick in is not a decision to spend
//! them.
//!
//! Whichever it is, the manifest says what was found and what was stored. A
//! backup that says "these forty tracks were on it and I do not have them" is
//! worth having; one that silently looks complete is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::OnForeign;

/// A track the library holds, as much of it as matching needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub path: PathBuf,
    pub bytes: u64,
    /// The hash of the audio alone, tags skipped — the duplicate finder's, so
    /// that "the library already has this" means the same thing in both places.
    /// Empty for a track that has never been read for one.
    pub audio_hash: String,
}

/// Where a player's own files live on a drive, and the name macOS gives the
/// same folder when a stick has been copied rather than written.
const PIONEER: [&str; 2] = ["PIONEER", ".PIONEER"];

/// Where the audio sits.
const CONTENTS: &str = "Contents";

/// Whether this looks like a drive a player would read.
///
/// Both halves have to be there. A folder of music with no `PIONEER` is
/// somebody's music folder, and a `PIONEER` with no audio is a drive that has
/// been emptied — neither is a prepared drive, and backing up either would
/// mean copying whatever happens to be plugged in.
pub fn is_a_player_drive(root: &Path) -> bool {
    pioneer(root).is_some() && root.join(CONTENTS).is_dir()
}

/// The `PIONEER` folder on a drive, under whichever of its two names it has.
pub fn pioneer(root: &Path) -> Option<PathBuf> {
    PIONEER.iter().map(|name| root.join(name)).find(|path| path.is_dir())
}

/// The mount points worth looking at for a drive.
///
/// Removable media only, as far as each platform makes that distinguishable:
/// every volume on macOS bar the boot disk, and the places a desktop Linux
/// mounts a stick. Walking `/` looking for `PIONEER` folders is not on the
/// table.
pub fn volumes() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut look = |at: &Path| {
        let Ok(entries) = std::fs::read_dir(at) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                roots.push(path);
            }
        }
    };

    if cfg!(target_os = "macos") {
        look(Path::new("/Volumes"));
    } else {
        for at in ["/media", "/run/media", "/mnt"] {
            let at = Path::new(at);
            look(at);
            // Those two hold a folder per user before the volumes themselves.
            let Ok(entries) = std::fs::read_dir(at) else { continue };
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    look(&entry.path());
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// What one backup holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Backup {
    /// The drive it came from, as it called itself.
    pub drive: String,
    /// When it was taken, as seconds since the epoch.
    pub at: u64,
    /// A summary of the drive's own files, so the same drive unchanged is not
    /// stored twice.
    pub fingerprint: String,
    /// Files under `PIONEER`, copied.
    pub carried: usize,
    /// Bytes those files took.
    pub bytes: u64,
    /// Audio the library had, and which is therefore linked rather than copied.
    pub linked: usize,
    /// Audio the library did not have, and which was copied in whole because
    /// of it.
    pub copied: usize,
    /// Bytes those copies took.
    pub copied_bytes: u64,
    /// Audio the library did not have and which is not in this backup either.
    pub absent: usize,
    /// What was on the drive and is not in this backup.
    pub missing: Vec<String>,
    /// Files copied into the library, for the collection to be told about.
    #[serde(skip)]
    pub adopted: Vec<PathBuf>,
}

impl Backup {
    /// The line the log shows.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "{} of the drive's own files ({})",
            self.carried,
            crate::sync::bytes(self.bytes)
        )];
        if self.linked > 0 {
            parts.push(format!("{} tracks linked", self.linked));
        }
        if self.copied > 0 {
            parts.push(format!(
                "{} copied in ({})",
                self.copied,
                crate::sync::bytes(self.copied_bytes)
            ));
        }
        if self.absent > 0 {
            parts.push(format!("{} not in the library", self.absent));
        }
        parts.join(", ")
    }
}

/// A summary of the drive's own files: their paths, sizes and modification
/// times.
///
/// Enough to notice that a drive has been written to since it was last seen,
/// and cheap enough to compute on every insertion — it reads no file contents.
/// The audio is deliberately not part of it: a drive whose database changed is
/// a drive worth storing again, and one whose audio changed has a changed
/// database too.
pub fn fingerprint(root: &Path) -> String {
    use std::fmt::Write;

    let Some(pioneer) = pioneer(root) else { return String::new() };
    let mut lines: Vec<String> = Vec::new();
    for file in walk(&pioneer) {
        let Ok(meta) = file.metadata() else { continue };
        let at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name = file.strip_prefix(&pioneer).unwrap_or(&file).display();
        lines.push(format!("{name}:{}:{at}", meta.len()));
    }
    lines.sort();

    // A short digest of that listing rather than the listing itself, because it
    // is compared and stored rather than read.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lines.join("\n").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut out = String::new();
    let _ = write!(out, "{hash:016x}");
    out
}

/// Whether a backup of this exact drive state has already been taken.
pub fn already_kept(into: &Path, drive: &str, fingerprint: &str) -> bool {
    if fingerprint.is_empty() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(into.join(safe(drive))) else { return false };
    entries.flatten().any(|entry| {
        std::fs::read_to_string(entry.path().join("backup.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Backup>(&text).ok())
            .is_some_and(|kept| kept.fingerprint == fingerprint)
    })
}

/// Copy a drive's own files, and link its audio to the library's copy.
///
/// `library` is where music copied in goes, and is only read when `foreign` is
/// [`OnForeign::Adopt`].
pub fn keep(
    root: &Path,
    into: &Path,
    drive: &str,
    known: &[Known],
    foreign: OnForeign,
    library: &Path,
) -> Result<Backup> {
    let at = now();
    let dir = into.join(safe(drive)).join(stamp(at));
    std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;

    let mut backup = Backup {
        drive: drive.to_string(),
        at,
        fingerprint: fingerprint(root),
        ..Backup::default()
    };

    // The drive's own files, copied. This is the part that cannot be made
    // again, and it is small enough that copying it is not worth being clever
    // about.
    if let Some(pioneer) = pioneer(root) {
        let name = pioneer.file_name().unwrap_or_default();
        for file in walk(&pioneer) {
            let Ok(relative) = file.strip_prefix(&pioneer) else { continue };
            let to = dir.join(name).join(relative);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&file, &to)
                .with_context(|| format!("copying {} off the drive", file.display()))?;
            backup.carried += 1;
            backup.bytes += file.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }

    // The audio. Indexed both ways the library can be asked about a file, so
    // each lookup is a hash of the drive's file at worst rather than a walk of
    // the library.
    let (by_name, by_sound) = index(known);

    let contents = root.join(CONTENTS);
    for file in walk(&contents) {
        let Ok(relative) = file.strip_prefix(&contents) else { continue };
        let to = dir.join(CONTENTS).join(relative);
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();

        let owned = owner(&by_name, &by_sound, name, size, &file);

        if let Some(track) = owned {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if link(&track.path, &to).is_ok() {
                backup.linked += 1;
                continue;
            }
        }

        // Music the library does not have.
        match foreign {
            OnForeign::Ignore => {
                backup.absent += 1;
                backup.missing.push(relative.display().to_string());
            }
            OnForeign::Keep => {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&file, &to)
                    .with_context(|| format!("copying {} off the drive", file.display()))?;
                backup.copied += 1;
                backup.copied_bytes += size;
            }
            OnForeign::Adopt => {
                // Into the library under the drive's own artist and album
                // folders, which is the layout it was written in and the one
                // the library uses. An existing file is left alone: this is
                // somebody's music folder, not scratch space.
                let at = library.join(relative);
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if !at.exists() {
                    std::fs::copy(&file, &at)
                        .with_context(|| format!("copying {} into the library", file.display()))?;
                }
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = link(&at, &to);
                backup.copied += 1;
                backup.copied_bytes += size;
                backup.adopted.push(at);
            }
        }
    }

    std::fs::write(dir.join("backup.json"), serde_json::to_vec_pretty(&backup)?)?;
    Ok(backup)
}

/// Which library track a file on a drive is, if any.
///
/// The cheap question first: a drive this program wrote holds the library's own
/// files, so name and length settle nearly everything without opening anything.
/// When they do not, the same question the duplicate finder asks — is this the
/// same recording, whatever it has been called since — which costs one read of
/// the file and is only ever reached by a file the cheap test could not answer
/// for.
pub fn owner<'a>(
    by_name: &ByName<'a>,
    by_sound: &BySound<'a>,
    name: &str,
    size: u64,
    file: &Path,
) -> Option<&'a Known> {
    by_name.get(&(name, size)).copied().or_else(|| {
        let hash = musicai::hash::audio_sha256(file).ok()?;
        by_sound.get(hash.as_str()).copied()
    })
}

/// The library indexed by file name and length, for the cheap half of the
/// question.
pub type ByName<'a> = HashMap<(&'a str, u64), &'a Known>;
/// And by the sound of the audio, for the half that has to open a file.
pub type BySound<'a> = HashMap<&'a str, &'a Known>;

/// The two indexes [`owner`] looks in, built once for a whole drive.
pub fn index(known: &[Known]) -> (ByName<'_>, BySound<'_>) {
    let by_name = known
        .iter()
        .filter_map(|track| {
            let name = track.path.file_name()?.to_str()?;
            Some(((name, track.bytes), track))
        })
        .collect();
    let by_sound = known
        .iter()
        .filter(|track| !track.audio_hash.is_empty())
        .map(|track| (track.audio_hash.as_str(), track))
        .collect();
    (by_name, by_sound)
}

/// Point one path at another without copying it.
///
/// A hard link first, because it survives the library file being moved or
/// renamed afterwards and costs nothing but a directory entry. It only works
/// within one filesystem, so a library on another disk falls back to a symbolic
/// link, which is a weaker promise honestly kept: it will break if the file it
/// names goes away, and the manifest is what says which files a backup was
/// counting on.
fn link(from: &Path, to: &Path) -> Result<()> {
    let _ = std::fs::remove_file(to);
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(from, to)?;
    #[cfg(not(unix))]
    std::fs::copy(from, to)?;
    Ok(())
}

/// Every file under a directory, deepest last, symlinks not followed.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => found.push(path),
                _ => {}
            }
        }
    }
    found.sort();
    found
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A folder name for a moment: sortable, and readable without a tool.
fn stamp(seconds: u64) -> String {
    // Days since the epoch to a date, by the civil-calendar algorithm, so that
    // a backup folder says when it was taken without pulling in a calendar
    // library for one line of output.
    let days = (seconds / 86_400) as i64;
    let seconds_today = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}{:02}{:02}",
        seconds_today / 3_600,
        (seconds_today / 60) % 60,
        seconds_today % 60
    )
}

/// A name that is safe to make a folder out of.
fn safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c.is_alphanumeric() || " -_.".contains(c) {
            true => c,
            false => '_',
        })
        .collect();
    match cleaned.trim().is_empty() {
        true => "drive".to_string(),
        false => cleaned.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("booth-backup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self, at: &str) -> PathBuf {
            self.0.join(at)
        }

        /// A file with contents, and every directory above it.
        fn file(&self, at: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path(at);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A drive with one track on it, and a library holding that track.
    fn a_drive(scratch: &Scratch) -> (PathBuf, Vec<Known>) {
        let audio = b"the audio itself, which is the big part".to_vec();
        scratch.file("drive/PIONEER/rekordbox/export.pdb", b"a database");
        scratch.file("drive/PIONEER/rekordbox/exportLibrary.db", b"another one");
        scratch.file("drive/PIONEER/USBANLZ/P001/00000001/ANLZ0000.DAT", b"a grid");
        scratch.file("drive/Contents/Peverelist/Roll With The Punches.flac", &audio);
        let in_library = scratch.file("library/Peverelist/Roll With The Punches.flac", &audio);
        let known =
            Known { path: in_library, bytes: audio.len() as u64, audio_hash: String::new() };
        (scratch.path("drive"), vec![known])
    }

    /// The arguments that do not vary, for the tests that are not about them.
    fn into(scratch: &Scratch) -> (PathBuf, PathBuf) {
        (scratch.path("backups"), scratch.path("library"))
    }

    #[test]
    fn a_prepared_drive_is_recognised_and_a_folder_of_music_is_not() {
        let scratch = Scratch::new("recognise");
        let (drive, _) = a_drive(&scratch);
        assert!(is_a_player_drive(&drive));

        scratch.file("just-music/Contents/a.flac", b"x");
        assert!(!is_a_player_drive(&scratch.path("just-music")), "no PIONEER");

        scratch.file("emptied/PIONEER/rekordbox/export.pdb", b"x");
        assert!(!is_a_player_drive(&scratch.path("emptied")), "no audio");
    }

    #[test]
    fn the_dotted_folder_a_mac_leaves_behind_counts_too() {
        let scratch = Scratch::new("dotted");
        scratch.file("drive/.PIONEER/rekordbox/export.pdb", b"a database");
        scratch.file("drive/Contents/a.flac", b"x");
        assert!(is_a_player_drive(&scratch.path("drive")));
    }

    #[test]
    fn the_drives_own_files_are_copied_and_its_audio_is_linked() {
        let scratch = Scratch::new("keep");
        let (drive, known) = a_drive(&scratch);
        let (into, library) = into(&scratch);

        let kept = keep(&drive, &into, "MY STICK", &known, OnForeign::Ignore, &library).unwrap();
        assert_eq!(kept.carried, 3, "the database, the other one, and the analysis");
        assert_eq!(kept.linked, 1);
        assert_eq!(kept.absent, 0);

        let at = std::fs::read_dir(into.join("MY STICK")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(
            std::fs::read(at.join("PIONEER/rekordbox/export.pdb")).unwrap(),
            b"a database",
            "the database is really there, not a link to a drive that will be unplugged"
        );
        let track = at.join("Contents/Peverelist/Roll With The Punches.flac");
        assert!(track.exists(), "the audio is reachable through the backup");
        assert_eq!(std::fs::read(&track).unwrap(), b"the audio itself, which is the big part");
    }

    #[test]
    fn linked_audio_costs_no_second_copy_of_itself() {
        // The whole point: a four-gigabyte stick should not become another four
        // gigabytes. What is stored is the drive's own files and a directory
        // entry per track.
        let scratch = Scratch::new("space");
        let (drive, known) = a_drive(&scratch);
        let (into, library) = into(&scratch);
        keep(&drive, &into, "MY STICK", &known, OnForeign::Ignore, &library).unwrap();

        let at = std::fs::read_dir(into.join("MY STICK")).unwrap().next().unwrap().unwrap().path();
        let track = at.join("Contents/Peverelist/Roll With The Punches.flac");
        let kind = std::fs::symlink_metadata(&track).unwrap();
        let same_file = match kind.file_type().is_symlink() {
            true => std::fs::read_link(&track).unwrap() == known[0].path,
            false => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    std::fs::metadata(&track).unwrap().ino()
                        == std::fs::metadata(&known[0].path).unwrap().ino()
                }
                #[cfg(not(unix))]
                false
            }
        };
        assert!(same_file, "the backup holds a second copy of the audio rather than a link");
    }

    #[test]
    fn a_track_the_library_does_not_have_is_named_rather_than_silently_left_out() {
        // Somebody else's drive. There is nothing to link to, and a backup that
        // looks complete would be worse than one that says what it is missing.
        let scratch = Scratch::new("foreign");
        let (drive, _) = a_drive(&scratch);
        let (backups, library) = into(&scratch);
        let kept = keep(&drive, &backups, "THEIRS", &[], OnForeign::Ignore, &library).unwrap();

        assert_eq!(kept.linked, 0);
        assert_eq!(kept.absent, 1);
        assert_eq!(kept.missing, vec!["Peverelist/Roll With The Punches.flac".to_string()]);
        assert_eq!(kept.carried, 3, "the drive's own files are kept either way");
    }

    /// A real FLAC, so that the audio hash the duplicate finder uses can be
    /// taken of it. A made-up byte string has no audio stream to hash.
    fn a_flac(at: &Path, seed: f32) -> u64 {
        use musicai::audio::encode::{write_file, Codec, EncodeOptions};
        use musicai::audio::Audio;

        let plane: Vec<f32> = (0..8_000).map(|i| (i as f32 * seed / 800.0).sin() * 0.4).collect();
        let audio = Audio::new(8_000, vec![plane.clone(), plane]).unwrap();
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        write_file(at, &audio, Codec::Flac, &EncodeOptions::default()).unwrap();
        at.metadata().unwrap().len()
    }

    #[test]
    fn the_same_recording_under_another_name_is_still_the_librarys() {
        // The whole point of asking the duplicate finder's question: a drive
        // written by somebody else names its files their way, and copying
        // music we already have because of that would be the expensive kind of
        // wrong.
        let scratch = Scratch::new("bysound");
        let (into, library_at) = into(&scratch);
        scratch.file("drive/PIONEER/rekordbox/export.pdb", b"a database");
        a_flac(&scratch.path("drive/Contents/Their Folder/track_04.flac"), 3.0);
        let mine = scratch.path("library/Peverelist/Roll With The Punches.flac");
        a_flac(&mine, 3.0);

        let known = vec![Known {
            path: mine.clone(),
            // Deliberately wrong, so only the sound can match them.
            bytes: 1,
            audio_hash: musicai::hash::audio_sha256(&mine).unwrap(),
        }];
        let kept =
            keep(&scratch.path("drive"), &into, "THEIRS", &known, OnForeign::Ignore, &library_at)
                .unwrap();

        assert_eq!(kept.linked, 1, "the same recording should be recognised: {kept:?}");
        assert_eq!(kept.absent, 0);
        assert_eq!(kept.copied, 0);
    }

    #[test]
    fn a_different_recording_is_not_mistaken_for_one_we_have() {
        let scratch = Scratch::new("notmine");
        let (into, library_at) = into(&scratch);
        scratch.file("drive/PIONEER/rekordbox/export.pdb", b"a database");
        a_flac(&scratch.path("drive/Contents/Their Folder/theirs.flac"), 7.0);
        let mine = scratch.path("library/Mine/mine.flac");
        a_flac(&mine, 3.0);

        let known = vec![Known {
            path: mine.clone(),
            bytes: 1,
            audio_hash: musicai::hash::audio_sha256(&mine).unwrap(),
        }];
        let kept =
            keep(&scratch.path("drive"), &into, "THEIRS", &known, OnForeign::Ignore, &library_at)
                .unwrap();
        assert_eq!(kept.linked, 0);
        assert_eq!(kept.absent, 1);
    }

    #[test]
    fn what_the_library_lacks_is_copied_into_the_backup_when_that_is_asked_for() {
        let scratch = Scratch::new("keepforeign");
        let (into, library_at) = into(&scratch);
        let (drive, _) = a_drive(&scratch);

        let kept = keep(&drive, &into, "THEIRS", &[], OnForeign::Keep, &library_at).unwrap();
        assert_eq!(kept.copied, 1);
        assert_eq!(kept.absent, 0, "nothing is missing from a backup that copied it");
        assert!(kept.adopted.is_empty(), "the library is not touched");

        let at = std::fs::read_dir(into.join("THEIRS")).unwrap().next().unwrap().unwrap().path();
        let track = at.join("Contents/Peverelist/Roll With The Punches.flac");
        assert!(!std::fs::symlink_metadata(&track).unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::read(&track).unwrap(),
            b"the audio itself, which is the big part",
            "the backup holds the music itself, so it can be put back"
        );
    }

    #[test]
    fn what_the_library_lacks_is_copied_into_it_when_that_is_asked_for() {
        let scratch = Scratch::new("adopt");
        let (into, library_at) = into(&scratch);
        let (drive, _) = a_drive(&scratch);
        // The library's own copy of this track is not in `known`, so the drive
        // is foreign as far as this call is concerned.
        std::fs::remove_dir_all(&library_at).unwrap();

        let kept = keep(&drive, &into, "THEIRS", &[], OnForeign::Adopt, &library_at).unwrap();
        assert_eq!(kept.copied, 1);
        assert_eq!(kept.absent, 0);

        let landed = library_at.join("Peverelist/Roll With The Punches.flac");
        assert_eq!(kept.adopted, vec![landed.clone()], "the collection is told what arrived");
        assert!(landed.exists(), "under the drive's own artist folder, which the library uses too");
        assert_eq!(std::fs::read(&landed).unwrap(), b"the audio itself, which is the big part");
    }

    #[test]
    fn adopting_does_not_write_over_something_already_in_the_library() {
        // A file already there is somebody's music, not scratch space.
        let scratch = Scratch::new("adopt-existing");
        let (into, library_at) = into(&scratch);
        let (drive, _) = a_drive(&scratch);
        let already = scratch.file("library/Peverelist/Roll With The Punches.flac", b"mine");

        keep(&drive, &into, "THEIRS", &[], OnForeign::Adopt, &library_at).unwrap();
        assert_eq!(std::fs::read(&already).unwrap(), b"mine");
    }

    #[test]
    fn the_same_drive_unchanged_is_not_stored_twice() {
        let scratch = Scratch::new("dedupe");
        let (drive, known) = a_drive(&scratch);
        let (into, library) = into(&scratch);

        let first = keep(&drive, &into, "MY STICK", &known, OnForeign::Ignore, &library).unwrap();
        assert!(!first.fingerprint.is_empty());
        assert!(already_kept(&into, "MY STICK", &first.fingerprint));

        // A drive that has been written to since is a different drive.
        std::fs::write(drive.join("PIONEER/rekordbox/export.pdb"), b"a longer database").unwrap();
        assert_ne!(fingerprint(&drive), first.fingerprint);
        assert!(!already_kept(&into, "MY STICK", &fingerprint(&drive)));
    }

    #[test]
    fn a_drive_with_nothing_of_its_own_has_no_fingerprint_to_compare() {
        let scratch = Scratch::new("bare");
        std::fs::create_dir_all(scratch.path("bare")).unwrap();
        assert_eq!(fingerprint(&scratch.path("bare")), "");
        assert!(!already_kept(&scratch.path("backups"), "BARE", ""));
    }

    #[test]
    fn a_backup_says_what_it_holds() {
        let backup =
            Backup { carried: 12, bytes: 3_000_000, linked: 40, absent: 2, ..Backup::default() };
        let said = backup.summary();
        assert!(said.contains("12 of the drive's own files"), "{said}");
        assert!(said.contains("40 tracks linked"), "{said}");
        assert!(said.contains("2 not in the library"), "{said}");
    }

    #[test]
    fn a_drive_name_becomes_a_folder_name_without_becoming_a_path() {
        assert_eq!(safe("MY STICK"), "MY STICK");
        assert_eq!(safe("../../etc"), ".._.._etc");
        assert_eq!(safe("Saturday/Night"), "Saturday_Night");
        assert_eq!(safe("   "), "drive");
    }

    #[test]
    fn a_backup_folder_says_when_it_was_taken() {
        assert_eq!(stamp(0), "1970-01-01 000000");
        assert_eq!(stamp(1_756_944_000), "2025-09-04 000000");
        // Sortable, which is what makes the newest one findable.
        assert!(stamp(1_756_944_000) < stamp(1_756_944_001));
    }
}
