//! Reading rekordbox's own encrypted libraries.
//!
//! The fixtures here are built with SQLCipher rather than mocked, so what is
//! tested is the real thing: a database written encrypted, closed, and opened
//! again through the same code path a real `master.db` goes through. The
//! schema is rekordbox's, taken from the tables `pyrekordbox` documents.

use std::path::{Path, PathBuf};

use musicai::rekordbox::{self, master};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("musicai-rbdb-{name}-{}", std::process::id()));
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

const KEY: &str = "not-the-real-one-but-the-same-shape";

/// A small library in rekordbox's own schema, encrypted the way rekordbox
/// encrypts it.
fn write_library(path: &Path, key: &str) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(&format!("PRAGMA key = '{key}'")).unwrap();
    db.execute_batch(
        "
        CREATE TABLE djmdArtist (ID TEXT, Name TEXT, rb_local_deleted INTEGER);
        CREATE TABLE djmdAlbum  (ID TEXT, Name TEXT, rb_local_deleted INTEGER);
        CREATE TABLE djmdGenre  (ID TEXT, Name TEXT, rb_local_deleted INTEGER);
        CREATE TABLE djmdKey    (ID TEXT, ScaleName TEXT, Seq INTEGER);
        CREATE TABLE djmdContent (
            ID TEXT, FolderPath TEXT, FileNameL TEXT, Title TEXT,
            ArtistID TEXT, AlbumID TEXT, GenreID TEXT, KeyID TEXT,
            BPM INTEGER, Length INTEGER, Rating INTEGER, ReleaseYear TEXT,
            Commnt TEXT, DJPlayCount INTEGER, StockDate TEXT,
            rb_local_deleted INTEGER
        );
        CREATE TABLE djmdCue (
            ID TEXT, ContentID TEXT, InMsec INTEGER, Kind INTEGER,
            Comment TEXT, rb_local_deleted INTEGER
        );
        CREATE TABLE djmdMyTag (ID TEXT, Name TEXT, Seq INTEGER, rb_local_deleted INTEGER);
        CREATE TABLE djmdSongMyTag (ID TEXT, MyTagID TEXT, ContentID TEXT, rb_local_deleted INTEGER);
        CREATE TABLE djmdPlaylist (
            ID TEXT, Seq INTEGER, Name TEXT, Attribute INTEGER,
            ParentID TEXT, rb_local_deleted INTEGER
        );
        CREATE TABLE djmdSongPlaylist (
            ID TEXT, PlaylistID TEXT, ContentID TEXT, TrackNo INTEGER,
            rb_local_deleted INTEGER
        );

        INSERT INTO djmdArtist VALUES ('a1', 'Peverelist', 0);
        INSERT INTO djmdAlbum  VALUES ('b1', 'Livity Sound', 0);
        INSERT INTO djmdGenre  VALUES ('g1', 'Techno', 0);
        INSERT INTO djmdKey    VALUES ('k1', '8A', 1);

        -- A folder path that is only the folder.
        INSERT INTO djmdContent VALUES
            ('c1', '/music/livity', 'Roll With The Punches.flac', 'Roll With The Punches',
             'a1', 'b1', 'g1', 'k1', 12802, 372, 4, '2019', 'peak time', 17, '2019-05-01', 0);
        -- A folder path that is already the whole path, which also happens.
        INSERT INTO djmdContent VALUES
            ('c2', '/music/Sirens.mp3', 'Sirens.mp3', 'Sirens',
             'a1', NULL, NULL, NULL, 14000, 300, 0, NULL, NULL, 0, NULL, 0);
        -- Deleted in the app, still in the file.
        INSERT INTO djmdContent VALUES
            ('c3', '/music', 'Thrown Away.flac', 'Thrown Away',
             'a1', NULL, NULL, NULL, 12000, 100, 0, NULL, NULL, 0, NULL, 1);

        INSERT INTO djmdCue VALUES ('q1', 'c1', 0,     0, 'top',  0);
        INSERT INTO djmdCue VALUES ('q2', 'c1', 30000, 1, 'drop', 0);
        INSERT INTO djmdCue VALUES ('q3', 'c1', 60000, 9, 'huh',  0);
        INSERT INTO djmdCue VALUES ('q4', 'c1', 90000, 2, 'gone', 1);

        INSERT INTO djmdMyTag VALUES ('t1', 'peak', 1, 0);
        INSERT INTO djmdSongMyTag VALUES ('s1', 't1', 'c1', 0);

        INSERT INTO djmdPlaylist VALUES ('p0', 1, '2026',     1, NULL, 0);
        INSERT INTO djmdPlaylist VALUES ('p1', 1, 'March',    0, 'p0', 0);
        INSERT INTO djmdPlaylist VALUES ('p2', 2, 'Everything', 4, NULL, 0);
        INSERT INTO djmdSongPlaylist VALUES ('x1', 'p1', 'c2', 2, 0);
        INSERT INTO djmdSongPlaylist VALUES ('x2', 'p1', 'c1', 1, 0);
        ",
    )
    .unwrap();
    db.close().unwrap();
}

#[test]
fn an_encrypted_library_does_not_open_without_the_key() {
    let scratch = Scratch::new("locked");
    let path = scratch.0.join("master.db");
    write_library(&path, KEY);

    // Plain SQLite sees noise, which is the whole point of the exercise.
    assert!(
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .is_err(),
        "the fixture is not actually encrypted, so this test proves nothing"
    );

    let wrong = rekordbox::open(&path, "definitely not the key");
    assert!(wrong.is_err(), "the wrong key must not appear to work");
    let said = format!("{:#}", wrong.unwrap_err());
    assert!(said.contains("key is wrong"), "{said}");
}

#[test]
fn a_library_comes_across_with_its_names_grids_and_cues() {
    let scratch = Scratch::new("read");
    let path = scratch.0.join("master.db");
    write_library(&path, KEY);

    let db = rekordbox::open(&path, KEY).unwrap();
    let collection = master::read(&db).unwrap();

    assert_eq!(collection.tracks.len(), 2, "the deleted track should not come back");
    let track = collection.tracks.iter().find(|t| t.title == "Roll With The Punches").unwrap();

    assert_eq!(track.path, PathBuf::from("/music/livity/Roll With The Punches.flac"));
    assert_eq!(track.artist, "Peverelist");
    assert_eq!(track.album, "Livity Sound");
    assert_eq!(track.genre, "Techno");
    assert_eq!(track.key, "8A");
    // Stored as hundredths, so a drifting grid survives the trip.
    assert!((track.bpm - 128.02).abs() < 1e-9, "{}", track.bpm);
    assert_eq!(track.rating, 4);
    assert_eq!(track.year, Some(2019));
    assert_eq!(track.comment, "peak time");
    assert_eq!(track.play_count, 17);
    assert!((track.duration_secs - 372.0).abs() < 1e-9);
    assert_eq!(track.my_tags, vec!["peak".to_string()]);

    // Nought is the memory cue and one to eight are hot cues; a nine is
    // something this does not understand, and a deleted one is deleted.
    let letters: Vec<u8> = track.cues.iter().map(|c| c.letter).collect();
    assert_eq!(letters, vec![0, 1], "{:?}", track.cues);
    assert_eq!(track.cues[1].time_ms, 30_000);
    assert_eq!(track.cues[1].label, "drop");

    // The other track has almost nothing set, and that is not an error.
    let sparse = collection.tracks.iter().find(|t| t.title == "Sirens").unwrap();
    assert_eq!(sparse.path, PathBuf::from("/music/Sirens.mp3"), "the path was already whole");
    assert_eq!(sparse.album, "");
    assert_eq!(sparse.year, None);
    assert_eq!(sparse.rating, 0);
}

#[test]
fn playlists_keep_their_folders_and_their_order() {
    let scratch = Scratch::new("playlists");
    let path = scratch.0.join("master.db");
    write_library(&path, KEY);

    let db = rekordbox::open(&path, KEY).unwrap();
    let collection = master::read(&db).unwrap();

    // A folder is not a playlist — it is where one lives.
    assert_eq!(collection.playlists.len(), 2, "{:?}", collection.playlists);
    let march = collection.playlists.iter().find(|p| p.name == "March").unwrap();
    assert_eq!(march.folder, "2026");
    assert!(!march.was_smart);
    // TrackNo order, not insertion order: a playlist is an order.
    assert_eq!(march.track_ids, vec!["c1".to_string(), "c2".to_string()]);

    let smart = collection.playlists.iter().find(|p| p.name == "Everything").unwrap();
    assert!(smart.was_smart, "a smart playlist comes across as its contents, and says so");
    assert_eq!(smart.folder, "");
}

/// The schema moves between rekordbox versions. An import that fails because a
/// column was renamed is worse than one that arrives without that field.
#[test]
fn a_missing_column_costs_that_field_and_not_the_import() {
    let scratch = Scratch::new("schema-drift");
    let path = scratch.0.join("master.db");

    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(&format!("PRAGMA key = '{KEY}'")).unwrap();
    // No rating, no comment, no play count, no My Tags table at all, and no
    // soft-delete flag either.
    db.execute_batch(
        "
        CREATE TABLE djmdContent (ID TEXT, FolderPath TEXT, FileNameL TEXT, Title TEXT, BPM INTEGER);
        INSERT INTO djmdContent VALUES ('c1', '/music', 'a.flac', 'A', 12800);
        ",
    )
    .unwrap();
    db.close().unwrap();

    let db = rekordbox::open(&path, KEY).unwrap();
    let collection = master::read(&db).unwrap();
    assert_eq!(collection.tracks.len(), 1);
    assert_eq!(collection.tracks[0].title, "A");
    assert_eq!(collection.tracks[0].path, PathBuf::from("/music/a.flac"));
    assert!((collection.tracks[0].bpm - 128.0).abs() < 1e-9);
    assert_eq!(collection.tracks[0].rating, 0);
    assert!(collection.tracks[0].my_tags.is_empty());
    assert!(collection.playlists.is_empty());
}

/// The OneLibrary reader opens a drive and describes it. It does not write
/// one, and the difference is the schema rather than the encryption.
#[test]
fn a_onelibrary_drive_can_be_opened_and_described() {
    use musicai::rekordbox::onelibrary;

    let scratch = Scratch::new("onelibrary");
    let drive = scratch.0.join("USB");
    let dir = drive.join("PIONEER/rekordbox");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("exportLibrary.db");

    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(&format!("PRAGMA key = '{KEY}'")).unwrap();
    db.execute_batch(
        "CREATE TABLE content (id INTEGER, title TEXT, bpm INTEGER);
         INSERT INTO content VALUES (1, 'Sirens', 12800);
         CREATE TABLE playlist (id INTEGER, name TEXT);",
    )
    .unwrap();
    db.close().unwrap();

    assert_eq!(onelibrary::find(&drive), Some(path));

    let db = rekordbox::open(&onelibrary::find(&drive).unwrap(), KEY).unwrap();
    let shapes = onelibrary::describe(&db).unwrap();
    assert_eq!(shapes.len(), 2);

    let content = shapes.iter().find(|s| s.name == "content").unwrap();
    assert_eq!(content.rows, 1);
    assert_eq!(content.columns, vec!["id", "title", "bpm"]);

    let text = onelibrary::report(&shapes);
    assert!(text.contains("content (1 rows)"), "{text}");
}
