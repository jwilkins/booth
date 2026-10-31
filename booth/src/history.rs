//! What a player recorded having played, read back off a drive.
//!
//! A CDJ writes a history back to the stick it played from: every track it
//! loaded, in the order it loaded them, grouped into a session per night. It is
//! the one thing on a drive that the collection cannot produce for itself —
//! everything else there was written *from* the collection — and it is the
//! record of what was actually played rather than what was prepared.
//!
//! So it comes back as playlists, in a folder per drive, one playlist per
//! session. What the player called the session becomes the playlist's name,
//! which is a date when rekordbox and the players write one.
//!
//! # Where it is read from
//!
//! Two places, because the two generations of player write to two databases:
//!
//! - `exportLibrary.db`, the OneLibrary database, whose `history` and
//!   `history_content` tables say it plainly and which is where the CDJ-3000X
//!   and every other newer player writes. A session there is named with the
//!   day it was played. See [`read`].
//! - `export.pdb`, where a CDJ-3000 and every player before it writes its own,
//!   in two tables of the legacy database. A session there is a list the player
//!   started when the drive was mounted, named `HISTORY 001` and up, and the
//!   row has no date in it at all. See [`read_legacy`].
//!
//! # Naming a session that has no date
//!
//! Every sync writes both databases with no history in them, so a CDJ-3000
//! numbers from `HISTORY 001` again on the first night after each one. Kept
//! under the player's name alone, the second night would land on the first
//! night's playlist and replace it.
//!
//! So a numbered session is named for the day `export.pdb` was last written as
//! well as for its number: `2026-10-31 HISTORY 001`. On a drive back from a
//! night, that is the night, or the morning after it. And it is recognised
//! again by what it holds rather than by that name, because the date moves
//! whenever a player writes the file again: see [`Session::is`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use booth_cli::rekordbox::pdb::{Histories, Library};

/// One night, as the player recorded it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// What to call it: the day it was played, the player's own name for it,
    /// or both.
    pub name: String,
    /// The tracks it loaded, in the order it loaded them.
    pub played: Vec<Played>,
    /// The player's own name for it where that is a number rather than a day —
    /// `export.pdb`'s `HISTORY 001`. `None` for a OneLibrary session, whose
    /// name is already the day.
    pub numbered: Option<String>,
}

impl Session {
    /// Whether a playlist already in the drive's folder is this session, read
    /// off the drive before.
    ///
    /// By name, for a session named by the day it was played. A numbered one
    /// is a playlist carrying the same number whose tracks this session's
    /// begin with: the same list, longer if the player went on adding to it.
    /// Not by name, because the day in it is when the file was last written,
    /// and a drive played again without a sync between has its older sessions
    /// read again under a later day.
    pub fn is<T: PartialEq>(&self, name: &str, theirs: &[T], ours: &[T]) -> bool {
        let Some(number) = &self.numbered else { return name == self.name };
        let carries = name.split_once(' ').is_some_and(|(_, rest)| {
            rest == number
                || rest.strip_prefix(number.as_str()).is_some_and(|r| r.starts_with(" ("))
        });
        carries && !theirs.is_empty() && ours.starts_with(theirs)
    }
}

/// A name for a new playlist that no playlist in the folder already has.
///
/// Two nights can come out with the same name: the same number, on the same
/// day, from either side of a sync. Neither is the other, so the second is
/// numbered rather than written over the first.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} ({n})")).find(|candidate| !taken(candidate)).unwrap_or_default()
}

/// One track in a history, as the drive describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Played {
    /// Where the audio sits on the drive, as the database gives it.
    pub on_drive: String,
    pub file_name: String,
    pub bytes: u64,
}

impl Played {
    /// The file itself, if the drive is still mounted.
    pub fn file(&self, root: &Path) -> PathBuf {
        root.join(self.on_drive.trim_start_matches('/'))
    }
}

/// Read every session a drive holds.
///
/// An empty list means the drive has no OneLibrary history, which is not the
/// same as it having no history — see the module note.
pub fn read(root: &Path, key: &str) -> Result<Vec<Session>> {
    let Some(at) = booth_cli::rekordbox::onelibrary::find(root) else {
        crate::debug!("{} has no OneLibrary database, so no history this can read", root.display());
        return Ok(Vec::new());
    };
    crate::debug!("reading the play history out of {}", at.display());
    let connection = booth_cli::rekordbox::open(&at, key)
        .with_context(|| format!("opening {} to read its history", at.display()))?;

    // The sessions themselves. `attribute` marks a folder rather than a list,
    // the same as it does for playlists, and a folder holds no tracks of its
    // own.
    let mut statement = connection
        .prepare("SELECT history_id, name, attribute FROM history ORDER BY sequenceNo, history_id")
        .context("this database has no history table")?;
    let sessions: Vec<(i64, String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get::<_, String>(1)?, row.get(2)?)))?
        .filter_map(|row| row.ok())
        .collect();

    let mut out = Vec::new();
    for (id, name, attribute) in sessions {
        if attribute != 0 {
            continue;
        }
        let mut statement = connection.prepare(
            "SELECT c.path, c.fileName, c.fileSize FROM history_content h \
             JOIN content c ON c.content_id = h.content_id \
             WHERE h.history_id = ?1 ORDER BY h.sequenceNo",
        )?;
        let played: Vec<Played> = statement
            .query_map([id], |row| {
                Ok(Played {
                    on_drive: row.get(0)?,
                    file_name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    bytes: row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u64,
                })
            })?
            .filter_map(|row| row.ok())
            .collect();

        // A session with nothing in it is a session that was never played.
        if played.is_empty() {
            continue;
        }
        crate::debug!("session {id} \"{name}\": {} tracks", played.len());
        out.push(Session { name: named(&name, id), played, numbered: None });
    }
    crate::debug!("{} sessions with anything in them", out.len());
    Ok(out)
}

/// Read every session a CDJ-3000, or any player before it, recorded in the
/// drive's `export.pdb`.
///
/// No key: the legacy database is not encrypted. An empty list means the drive
/// has no `export.pdb`, or one no player has written a history into.
pub fn read_legacy(root: &Path) -> Result<Vec<Session>> {
    let at = root.join(booth_cli::audit::LEGACY_PATH);
    let bytes = match std::fs::read(&at) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::debug!("{} has no export.pdb, so no history in one", root.display());
            return Ok(Vec::new());
        }
        read => read.with_context(|| format!("reading {}", at.display()))?,
    };
    let library = booth_cli::rekordbox::pdb::read(&bytes)
        .with_context(|| format!("reading the tracks out of {}", at.display()))?;
    let histories = booth_cli::rekordbox::pdb::history(&bytes)
        .with_context(|| format!("reading the history out of {}", at.display()))?;

    // The day the player last wrote the file, by the clock it wrote it with. A
    // filesystem that will not say is dated today, which is when it was read.
    let written = std::fs::metadata(&at)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(booth_cli::clock::now, |since| since.as_secs());
    let sessions = numbered(&library, &histories, &booth_cli::clock::day(written));
    crate::debug!("{} sessions in {}", sessions.len(), at.display());
    Ok(sessions)
}

/// The sessions in a legacy database, each named for `day` and its number.
fn numbered(library: &Library, histories: &Histories, day: &str) -> Vec<Session> {
    let tracks: HashMap<u32, _> = library.tracks.iter().map(|t| (t.id, t)).collect();
    let mut out = Vec::new();
    for history in &histories.lists {
        // In the order they were loaded: the entries come sorted by list, then
        // by position.
        let entries = histories.entries.iter().filter(|e| e.playlist == history.id);
        let mut missing = 0usize;
        let played: Vec<Played> = entries
            .filter_map(|entry| {
                let found = tracks.get(&entry.track);
                missing += usize::from(found.is_none());
                found
            })
            .map(|track| Played {
                on_drive: track.file_path.clone(),
                file_name: track.file_name.clone(),
                bytes: u64::from(track.file_size),
            })
            .collect();
        if missing > 0 {
            crate::debug!("{missing} entries in \"{}\" name no track on the drive", history.name);
        }
        if played.is_empty() {
            continue;
        }
        let number = named(&history.name, i64::from(history.id));
        crate::debug!("\"{number}\": {} tracks", played.len());
        out.push(Session { name: format!("{day} {number}"), played, numbered: Some(number) });
    }
    out
}

/// The folder a drive's sessions go in.
///
/// One level, because that is what the collection's playlists have: a folder
/// and a name. The prefix is what groups every drive's history together in the
/// sidebar, and is the shape a nested tree would take if there were one.
pub fn folder(drive: &str) -> String {
    format!("History/{}", drive.trim())
}

/// What to call a session.
///
/// The player's own name for it, which is a date, because a date is what makes
/// one night findable among a year of them. A session the player left unnamed
/// falls back to its number on the drive, which at least distinguishes it from
/// the others.
fn named(name: &str, id: i64) -> String {
    match name.trim().is_empty() {
        true => format!("session {id}"),
        false => name.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drive with a OneLibrary database holding two sessions.
    ///
    /// Written here rather than fetched, because what is being tested is that
    /// the right rows come back out of the shape a player leaves behind.
    fn a_drive_with_history(dir: &Path, key: &str) -> PathBuf {
        use booth_cli::export::onelibrary;
        use booth_cli::export::pdb::{Database, Track};

        let tracks: Vec<Track> = (1..=3)
            .map(|id| Track {
                id,
                title: format!("Track {id}"),
                artist: "Peverelist".into(),
                file_path: format!("/Contents/Peverelist/track{id}.flac"),
                file_size: 100 + id,
                ..Track::default()
            })
            .collect();
        let bytes = onelibrary::to_bytes(
            &Database { tracks, playlists: Vec::new(), ..Default::default() },
            key,
            "TESTSTICK",
            onelibrary::analysed_bits(None),
        )
        .unwrap();

        let at = dir.join("PIONEER/rekordbox/exportLibrary.db");
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, &bytes).unwrap();

        // The player's part: a history it wrote after a night, and a folder
        // above it of the kind the format allows. Written through a connection
        // of its own, because the one this module uses opens read-only and a
        // player is not a reader.
        let connection = rusqlite::Connection::open(&at).unwrap();
        connection.execute_batch(&format!("PRAGMA key = '{key}'")).unwrap();
        connection
            .execute_batch(
                "INSERT INTO history (history_id, sequenceNo, name, attribute, history_id_parent) \
                     VALUES (1, 1, '2026-09-04', 0, 0), \
                            (2, 2, '2026-09-11', 0, 0), \
                            (3, 3, 'a folder', 1, 0), \
                            (4, 4, '', 0, 0);
                 INSERT INTO history_content (history_id, content_id, sequenceNo) \
                     VALUES (1, 3, 1), (1, 1, 2), (2, 2, 1);",
            )
            .unwrap();
        drop(connection);
        at
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("booth-history-{name}-{}", std::process::id()));
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

    const KEY: &str = "a-key-that-is-not-the-real-one";

    #[test]
    fn every_session_comes_back_with_its_tracks_in_the_order_they_were_played() {
        let scratch = Scratch::new("read");
        a_drive_with_history(&scratch.0, KEY);

        let sessions = read(&scratch.0, KEY).unwrap();
        assert_eq!(sessions.len(), 2, "two nights, a folder and an empty one aside: {sessions:?}");

        assert_eq!(sessions[0].name, "2026-09-04");
        let played: Vec<&str> = sessions[0].played.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(
            played,
            ["track3.flac", "track1.flac"],
            "the order is the order they were loaded, not the order they are stored"
        );
        assert_eq!(sessions[0].played[0].on_drive, "/Contents/Peverelist/track3.flac");
        assert_eq!(sessions[0].played[0].bytes, 103);

        assert_eq!(sessions[1].name, "2026-09-11");
        assert_eq!(sessions[1].played.len(), 1);
    }

    #[test]
    fn a_folder_of_sessions_is_not_itself_a_session() {
        // The history is a tree like the playlists, and a folder holds nothing.
        let scratch = Scratch::new("folders");
        a_drive_with_history(&scratch.0, KEY);
        let sessions = read(&scratch.0, KEY).unwrap();
        assert!(!sessions.iter().any(|s| s.name == "a folder"));
        assert!(!sessions.iter().any(|s| s.played.is_empty()), "nor is an empty one");
    }

    #[test]
    fn a_drive_with_no_onelibrary_database_has_no_history_to_read() {
        // Which is not the same as having no history: a drive played only on a
        // CDJ-3000 has one, in export.pdb, which `read_legacy` reads.
        let scratch = Scratch::new("none");
        std::fs::create_dir_all(scratch.0.join("PIONEER/rekordbox")).unwrap();
        assert_eq!(read(&scratch.0, KEY).unwrap(), Vec::new());
    }

    #[test]
    fn the_wrong_key_is_an_error_rather_than_an_empty_history() {
        // Silently reporting no history for a drive that has one would be the
        // worst of the three answers.
        let scratch = Scratch::new("wrongkey");
        a_drive_with_history(&scratch.0, KEY);
        assert!(read(&scratch.0, "some-other-key").is_err());
    }

    /// A drive a CDJ-3000 played: an `export.pdb` with three tracks, and the
    /// lists the player added to it, last written at `written` seconds.
    ///
    /// Through the writer, which can put a history in for exactly this, and
    /// whose history rows rekordcrate reads as a player's
    /// (`rekordbox_database.rs`).
    fn a_drive_a_cdj_3000_played(dir: &Path, lists: &[(&str, &[u32])], written: u64) -> PathBuf {
        use booth_cli::export::pdb::{Database, HistoryPlaylist, Track};

        let tracks: Vec<Track> = (1..=3)
            .map(|id| Track {
                id,
                title: format!("Track {id}"),
                artist: "Peverelist".into(),
                file_path: format!("/Contents/Peverelist/track{id}.flac"),
                file_size: 100 + id,
                ..Track::default()
            })
            .collect();
        let history = lists
            .iter()
            .zip(1..)
            .map(|((name, tracks), id)| HistoryPlaylist {
                id,
                name: name.to_string(),
                track_ids: tracks.to_vec(),
            })
            .collect();
        let bytes = Database { tracks, history, ..Default::default() }.to_bytes().unwrap();

        let at = dir.join(booth_cli::audit::LEGACY_PATH);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, bytes).unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(written);
        std::fs::File::options().write(true).open(&at).unwrap().set_modified(when).unwrap();
        at
    }

    /// Half past one in the morning on 31 October 2026, UTC.
    const AFTER_A_NIGHT: u64 = 1_793_410_200;

    #[test]
    fn a_cdj_3000s_history_comes_back_named_for_the_day_and_its_number() {
        let scratch = Scratch::new("legacy");
        a_drive_a_cdj_3000_played(
            &scratch.0,
            &[("HISTORY 001", &[3, 1]), ("HISTORY 002", &[2])],
            AFTER_A_NIGHT,
        );

        let sessions = read_legacy(&scratch.0).unwrap();
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["2026-10-31 HISTORY 001", "2026-10-31 HISTORY 002"]);
        assert_eq!(sessions[0].numbered.as_deref(), Some("HISTORY 001"));

        let played: Vec<&str> = sessions[0].played.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(played, ["track3.flac", "track1.flac"], "in the order they were loaded");
        assert_eq!(sessions[0].played[0].on_drive, "/Contents/Peverelist/track3.flac");
        assert_eq!(sessions[0].played[0].bytes, 103);
    }

    #[test]
    fn a_list_the_player_started_and_loaded_nothing_into_is_not_a_session() {
        let scratch = Scratch::new("legacy-empty");
        a_drive_a_cdj_3000_played(
            &scratch.0,
            &[("HISTORY 001", &[]), ("HISTORY 002", &[2])],
            AFTER_A_NIGHT,
        );
        let sessions = read_legacy(&scratch.0).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].numbered.as_deref(), Some("HISTORY 002"));
    }

    #[test]
    fn a_drive_with_no_export_pdb_has_no_legacy_history_and_is_not_an_error() {
        let scratch = Scratch::new("legacy-none");
        assert_eq!(read_legacy(&scratch.0).unwrap(), Vec::new());
    }

    #[test]
    fn an_export_pdb_that_will_not_read_is_an_error_rather_than_no_history() {
        let scratch = Scratch::new("legacy-broken");
        let at = scratch.0.join(booth_cli::audit::LEGACY_PATH);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, [0u8; 100]).unwrap();
        assert!(read_legacy(&scratch.0).is_err());
    }

    /// A numbered session, as `read_legacy` makes one.
    fn numbered_session(day: &str, number: &str) -> Session {
        Session {
            name: format!("{day} {number}"),
            played: Vec::new(),
            numbered: Some(number.to_string()),
        }
    }

    #[test]
    fn a_numbered_session_read_again_under_a_later_day_is_still_the_same_session() {
        // Played again without a sync between, so the file was written again
        // and its day moved; HISTORY 001 is the list it was the first time.
        let again = numbered_session("2026-11-07", "HISTORY 001");
        assert!(again.is("2026-10-31 HISTORY 001", &[3, 1], &[3, 1]));
        assert!(
            again.is("2026-10-31 HISTORY 001", &[3, 1], &[3, 1, 2]),
            "or the same list, longer"
        );
    }

    #[test]
    fn the_next_night_numbered_one_again_after_a_sync_is_a_night_of_its_own() {
        let next = numbered_session("2026-11-07", "HISTORY 001");
        assert!(!next.is("2026-10-31 HISTORY 001", &[3, 1], &[2, 3]));
        // Nor a different number holding the same tracks.
        assert!(!next.is("2026-10-31 HISTORY 002", &[2, 3], &[2, 3]));
    }

    #[test]
    fn a_numbered_session_kept_under_a_second_name_is_still_found_under_it() {
        let again = numbered_session("2026-10-31", "HISTORY 001");
        assert!(again.is("2026-10-31 HISTORY 001 (2)", &[2], &[2]));
        assert!(!again.is("2026-10-31 HISTORY 0010", &[2], &[2]), "a longer number is another");
    }

    #[test]
    fn a_session_named_by_its_day_is_found_by_that_name_alone() {
        let dated = Session { name: "2026-09-04".into(), ..Session::default() };
        assert!(dated.is("2026-09-04", &[1], &[2]));
        assert!(!dated.is("2026-09-11", &[1], &[1]));
    }

    #[test]
    fn a_new_night_whose_name_is_taken_is_numbered_rather_than_written_over_the_first() {
        let taken = ["2026-10-31 HISTORY 001", "2026-10-31 HISTORY 001 (2)"];
        assert_eq!(
            free_name("2026-10-31 HISTORY 002", |n| taken.contains(&n)),
            "2026-10-31 HISTORY 002"
        );
        assert_eq!(
            free_name("2026-10-31 HISTORY 001", |n| taken.contains(&n)),
            "2026-10-31 HISTORY 001 (3)"
        );
    }

    #[test]
    fn a_session_the_player_left_unnamed_is_still_told_apart_from_the_others() {
        assert_eq!(named("2026-09-04", 1), "2026-09-04");
        assert_eq!(named("  2026-09-04 ", 1), "2026-09-04");
        assert_eq!(named("", 7), "session 7");
    }

    #[test]
    fn every_drives_history_sits_under_one_folder() {
        assert_eq!(folder("MY STICK"), "History/MY STICK");
        assert_eq!(folder(" SANDISK "), "History/SANDISK");
    }

    #[test]
    fn a_played_track_says_where_it_is_on_the_drive() {
        let played = Played {
            on_drive: "/Contents/Peverelist/track1.flac".into(),
            file_name: "track1.flac".into(),
            bytes: 10,
        };
        assert_eq!(
            played.file(Path::new("/Volumes/MY STICK")),
            Path::new("/Volumes/MY STICK/Contents/Peverelist/track1.flac")
        );
    }
}
