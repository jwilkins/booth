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
//! # Why it is a directory and not a zip
//!
//! The obvious way to save space is to not copy the audio twice, and the
//! obvious way to do that is a link. But a zip file has no links in it — it is
//! a container of *contents*, so zipping a drive means copying every byte of
//! audio into the archive, which is the thing being avoided. The two ideas are
//! mutually exclusive and the links are worth more, so a backup here is an
//! ordinary directory:
//!
//! ```text
//! <backups>/<drive>/<when>/
//!     PIONEER/…            copied, byte for byte
//!     Contents/…           links to the library's own copy of each file
//!     backup.json          what was found, what was linked, what was not
//! ```
//!
//! A directory is also the thing that is easiest to get *out* of again: the
//! restore is a copy, with no tool in the middle. Archiving one into a `.zip`
//! or a `.dmg` afterwards is a reasonable thing to want, and is a step that can
//! be added on top of this without changing what is stored.
//!
//! # What the links point at
//!
//! The library's copy of the audio, never the drive's. A link into the drive
//! would dangle the moment it is unplugged, which is exactly when the backup
//! matters — and a hard link cannot cross from a stick to an internal disk
//! anyway. Where the library has no copy, which is the case for somebody
//! else's drive, the manifest records what was there and the audio is left out.
//! A backup that says "these forty tracks were on it and I do not have them" is
//! worth having; one that silently looks complete is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
    /// Audio the library did not have. Named in `missing`, not stored.
    pub absent: usize,
    /// What was on the drive and is not in this backup.
    pub missing: Vec<String>,
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
/// `known` is what the library holds, as (path, size) pairs. A track is matched
/// by file name and length, which is what a drive keeps of it: the drive's copy
/// was made from the library's, so the two agree on both, and neither the
/// drive's folder layout nor its database ids survive to be compared.
pub fn keep(root: &Path, into: &Path, drive: &str, known: &[(PathBuf, u64)]) -> Result<Backup> {
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

    // The audio, linked. Indexed by name and size so the lookup is one pass
    // over the library rather than one per file on the drive.
    let library: HashMap<(String, u64), &PathBuf> = known
        .iter()
        .filter_map(|(path, size)| {
            let name = path.file_name()?.to_str()?.to_string();
            Some(((name, *size), path))
        })
        .collect();

    let contents = root.join(CONTENTS);
    for file in walk(&contents) {
        let Ok(relative) = file.strip_prefix(&contents) else { continue };
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();

        let Some(source) = library.get(&(name, size)) else {
            backup.absent += 1;
            backup.missing.push(relative.display().to_string());
            continue;
        };
        let to = dir.join(CONTENTS).join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match link(source, &to) {
            Ok(()) => backup.linked += 1,
            Err(_) => {
                backup.absent += 1;
                backup.missing.push(relative.display().to_string());
            }
        }
    }

    std::fs::write(dir.join("backup.json"), serde_json::to_vec_pretty(&backup)?)?;
    Ok(backup)
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
    fn a_drive(scratch: &Scratch) -> (PathBuf, Vec<(PathBuf, u64)>) {
        let audio = b"the audio itself, which is the big part".to_vec();
        scratch.file("drive/PIONEER/rekordbox/export.pdb", b"a database");
        scratch.file("drive/PIONEER/rekordbox/exportLibrary.db", b"another one");
        scratch.file("drive/PIONEER/USBANLZ/P001/00000001/ANLZ0000.DAT", b"a grid");
        scratch.file("drive/Contents/Peverelist/Roll With The Punches.flac", &audio);
        let in_library = scratch.file("library/Peverelist/Roll With The Punches.flac", &audio);
        (scratch.path("drive"), vec![(in_library, audio.len() as u64)])
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
        let into = scratch.path("backups");

        let kept = keep(&drive, &into, "MY STICK", &known).unwrap();
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
        let into = scratch.path("backups");
        keep(&drive, &into, "MY STICK", &known).unwrap();

        let at = std::fs::read_dir(into.join("MY STICK")).unwrap().next().unwrap().unwrap().path();
        let track = at.join("Contents/Peverelist/Roll With The Punches.flac");
        let kind = std::fs::symlink_metadata(&track).unwrap();
        let same_file = match kind.file_type().is_symlink() {
            true => std::fs::read_link(&track).unwrap() == known[0].0,
            false => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    std::fs::metadata(&track).unwrap().ino()
                        == std::fs::metadata(&known[0].0).unwrap().ino()
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
        let kept = keep(&drive, &scratch.path("backups"), "THEIRS", &[]).unwrap();

        assert_eq!(kept.linked, 0);
        assert_eq!(kept.absent, 1);
        assert_eq!(kept.missing, vec!["Peverelist/Roll With The Punches.flac".to_string()]);
        assert_eq!(kept.carried, 3, "the drive's own files are kept either way");
    }

    #[test]
    fn the_same_drive_unchanged_is_not_stored_twice() {
        let scratch = Scratch::new("dedupe");
        let (drive, known) = a_drive(&scratch);
        let into = scratch.path("backups");

        let first = keep(&drive, &into, "MY STICK", &known).unwrap();
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
