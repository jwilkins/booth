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
//! `exportLibrary.db`, the OneLibrary database, whose `history` and
//! `history_content` tables say it plainly and which is where the CDJ-3000X and
//! every other newer player writes.
//!
//! **Not** from `export.pdb`, which is where a CDJ-3000 writes its own. That
//! file holds the same three tables and this program can write one, but reading
//! individual rows back out of DeviceSQL is a parser that does not exist here
//! yet — `inspect` walks its pages and counts its rows without ever decoding
//! one. A drive played only on older hardware therefore has a history this does
//! not see, and says so rather than reporting an empty one.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One night, as the player recorded it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// What the player called it, which is a date when it names one at all.
    pub name: String,
    /// The tracks it loaded, in the order it loaded them.
    pub played: Vec<Played>,
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
        out.push(Session { name: named(&name, id), played });
    }
    crate::debug!("{} sessions with anything in them", out.len());
    Ok(out)
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
        // CDJ-3000 has one, in export.pdb, that nothing here reads yet.
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
