//! Writing the OneLibrary database, which is what the newer players read.
//!
//! # What this is
//!
//! `PIONEER/rekordbox/exportLibrary.db` is the library a CDJ-3000X, CDJ-1500X,
//! XDJ-AZ, XDJ-AN, OPUS-QUAD or OMNIS-DUO reads off a drive. Those players do
//! not fall back to `export.pdb`, and every older player reads only
//! `export.pdb`, so a drive that has to work in an unknown booth needs both —
//! which is what [`super::pdb`] and this module are for. Both are built from
//! the same [`Database`], on purpose: the two files disagreeing about what is
//! on the drive is the failure that firmware 3.30 turned into a room full of
//! DJs with no playlists, and the cheapest way not to have it is to have no
//! second source of truth.
//!
//! It is an ordinary SQLite database encrypted with SQLCipher 4 at its default
//! settings, under a passphrase that is fixed and identical on every drive in
//! the world, and that this build carries — see
//! [`crate::rekordbox::BUNDLED_ONELIBRARY_KEY`]. The encryption is not a
//! defence of anything and never was: it is what the file happens to be
//! wrapped in, and reading or writing one is a matter of using the same
//! wrapping rekordbox does.
//!
//! # Where the shape of it comes from
//!
//! AlphaTheta publishes no specification. `docs/onelibrary.md` sets out what is
//! known, from whom, and how sure it is; what matters here is which parts of
//! the file below are copied and which are reconstructed:
//!
//! - The **schema** is verbatim from a real rekordbox export (the fixture in
//!   rekordcrate's pull request), and agrees with two independent writers.
//! - The **browse menu** is the same twenty-seven entries, with the same kind
//!   codes and the same order, as the legacy database's own menu table — so it
//!   is generated from [`super::pdb::COLUMNS`] rather than written out twice.
//!   That the two formats agree here was checked against a third party's copy
//!   of a real export, item for item.
//! - The **category and sort rows** are the counts and the visible set a real
//!   export has, as recorded by the one project whose hand-written drives are
//!   reported to play on a CDJ-3000X. They are the least attested thing in the
//!   file: they decide what the browse screen offers, not whether it opens.
//! - The **`cue` table is left empty**, because rekordbox leaves it empty. The
//!   cues a player shows come from the analysis files, exactly as they do on a
//!   legacy drive.
//! - `analysedBits` and `contentLink` are the values a writer whose drive
//!   played on a CDJ-3000 used. Real exports carry other values whose meaning
//!   nobody has established.
//!
//! # What this does not make true
//!
//! No player has read a drive this wrote. Writing a well-formed database is
//! not the same as a player browsing it, and the two things known to be
//! missing from what is published — whether a player recomputes the analysis
//! directory rather than trusting the path in the row, and whether it accepts
//! a hand-made analysis file or quietly measures its own — are exactly the
//! things that would show up as "it mounts but re-analyses everything". The
//! sheet that offers this says so, and it should keep saying so until a player
//! has been in front of it.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};

use super::pdb::{Database, Track, COLUMNS};

/// Where the database goes on the drive.
pub const DRIVE_PATH: &str = "/PIONEER/rekordbox/exportLibrary.db";

/// Where to look for the key when none was passed in.
pub const KEY_ENV: &str = "ONELIBRARY_KEY";

/// The schema, as a real rekordbox export declares it.
///
/// Copied statement for statement, including the two misspellings
/// (`isComplation`, `OutFileOffsetInBlock`), because a column named something
/// else is a column a player does not find. Twenty-two tables and four indexes.
pub const SCHEMA: &str = "\
CREATE TABLE content(content_id integer primary key, title varchar, titleForSearch varchar, \
subtitle varchar, bpmx100 integer, length integer, trackNo integer, discNo integer, \
artist_id_artist integer, artist_id_remixer integer, artist_id_originalArtist integer, \
artist_id_composer integer, artist_id_lyricist integer, album_id integer, genre_id integer, \
label_id integer, key_id integer, color_id integer, image_id integer, djComment varchar, \
rating integer, releaseYear integer, releaseDate varchar, dateCreated varchar, \
dateAdded varchar, path varchar, fileName varchar, fileSize integer, fileType integer, \
bitrate integer, bitDepth integer, samplingRate integer, isrc varchar, djPlayCount integer, \
isHotCueAutoLoadOn integer, isKuvoDeliverStatusOn integer, kuvoDeliveryComment varchar, \
masterDbId integer, masterContentId integer, analysisDataFilePath varchar, \
analysedBits integer, contentLink integer, hasModified integer, cueUpdateCount integer, \
analysisDataUpdateCount integer, informationUpdateCount integer);
CREATE TABLE genre(genre_id integer primary key, name varchar);
CREATE TABLE artist(artist_id integer primary key, name varchar, nameForSearch varchar);
CREATE TABLE album(album_id integer primary key, name varchar, artist_id integer, \
image_id integer, isComplation integer, nameForSearch varchar);
CREATE TABLE label(label_id integer primary key, name varchar);
CREATE TABLE key(key_id integer primary key, name varchar);
CREATE TABLE color(color_id integer primary key, name varchar);
CREATE TABLE image(image_id integer primary key, path varchar);
CREATE TABLE playlist(playlist_id integer primary key, sequenceNo integer, name varchar, \
image_id integer, attribute integer, playlist_id_parent integer);
CREATE TABLE playlist_content(playlist_id integer, content_id integer, sequenceNo integer);
CREATE TABLE history(history_id integer primary key, sequenceNo integer, name varchar, \
attribute integer, history_id_parent integer);
CREATE TABLE history_content(history_id integer, content_id integer, sequenceNo integer);
CREATE TABLE hotCueBankList(hotCueBankList_id integer primary key, sequenceNo integer, \
name varchar, image_id integer, attribute integer, hotCueBankList_id_parent integer);
CREATE TABLE hotCueBankList_cue(hotCueBankList_id integer, cue_id integer, sequenceNo integer);
CREATE TABLE cue(cue_id integer primary key, content_id integer, kind integer, \
colorTableIndex integer, cueComment varchar, isActiveLoop integer, beatLoopNumerator integer, \
beatLoopDenominator integer, inUsec integer, outUsec integer, in150FramePerSec integer, \
out150FramePerSec integer, inMpegFrameNumber integer, outMpegFrameNumber integer, \
inMpegAbs integer, outMpegAbs integer, inDecodingStartFramePosition integer, \
outDecodingStartFramePosition integer, inFileOffsetInBlock integer, \
OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer, \
outNumberOfSampleInBlock integer);
CREATE TABLE myTag(myTag_id integer primary key, sequenceNo integer, name varchar, \
attribute integer, myTag_id_parent integer);
CREATE TABLE myTag_content(myTag_id integer, content_id integer);
CREATE TABLE menuItem(menuItem_id integer primary key, kind integer, name varchar);
CREATE TABLE category(category_id integer primary key, menuItem_id integer, \
sequenceNo integer, isVisible integer);
CREATE TABLE sort(sort_id integer primary key, menuItem_id integer, sequenceNo integer, \
isVisible integer, isSelectedAsSubColumn integer);
CREATE TABLE property(deviceName varchar, dbVersion varchar, numberOfContents integer, \
createdDate varchar, backGroundColorType integer, myTagMasterDBID integer);
CREATE TABLE recommendedLike(content_id_1 integer, content_id_2 integer, rating integer, \
createdDate integer);
CREATE INDEX index_playlist_content_playlist_id on playlist_content(playlist_id);
CREATE INDEX index_myTag_content_myTag_id on myTag_content(myTag_id);
CREATE INDEX index_myTag_content_content_id on myTag_content(content_id);
CREATE INDEX index_hotCueBankList_cue_hotCueBankList_id on hotCueBankList_cue(hotCueBankList_id);
";

/// The version every export written since 2023 declares.
const DB_VERSION: &str = "1000";

/// The eight colours, in the order their ids run. Same list as the legacy
/// database's, because it is the same palette on the same players.
const COLORS: [&str; 8] = ["Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"];

/// The twenty-four keys, in the order their ids run: twelve major, then twelve
/// minor. A player shows this name, so the spelling is rekordbox's rather than
/// whatever the library happens to use.
const KEYS: [&str; 24] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B", "Cm", "C#m", "Dm", "Ebm",
    "Em", "Fm", "F#m", "Gm", "Abm", "Am", "Bbm", "Bm",
];

/// Camelot wheel positions to key ids, so a library that thinks in "8A" writes
/// the row a player will read as "Am".
const CAMELOT: [(&str, i64); 24] = [
    ("1A", 21),
    ("1B", 12),
    ("2A", 16),
    ("2B", 7),
    ("3A", 23),
    ("3B", 2),
    ("4A", 18),
    ("4B", 9),
    ("5A", 13),
    ("5B", 5),
    ("6A", 20),
    ("6B", 11),
    ("7A", 15),
    ("7B", 6),
    ("8A", 22),
    ("8B", 1),
    ("9A", 17),
    ("9B", 8),
    ("10A", 24),
    ("10B", 3),
    ("11A", 19),
    ("11B", 10),
    ("12A", 14),
    ("12B", 4),
];

/// The browse categories: which menu items the player offers, in what order,
/// and which of them start visible. The first field is the row's own id, whose
/// gaps are a real export's; the second is a position in [`COLUMNS`].
const CATEGORIES: [(i64, usize, i64, i64); 22] = [
    (1, 0, 0, 0),    // GENRE
    (2, 1, 1, 1),    // ARTIST
    (3, 2, 2, 1),    // ALBUM
    (4, 3, 3, 1),    // TRACK
    (5, 16, 5, 1),   // PLAYLIST
    (6, 4, 0, 0),    // BPM
    (7, 5, 0, 0),    // RATING
    (8, 6, 0, 0),    // YEAR
    (9, 7, 0, 0),    // REMIXER
    (10, 8, 0, 0),   // LABEL
    (11, 9, 0, 0),   // ORIGINAL ARTIST
    (12, 10, 4, 1),  // KEY
    (15, 12, 0, 0),  // COLOR
    (17, 23, 9, 1),  // FOLDER
    (18, 19, 7, 1),  // SEARCH
    (19, 13, 0, 0),  // TIME
    (20, 14, 0, 0),  // BITRATE
    (21, 15, 0, 0),  // FILE NAME
    (22, 18, 6, 1),  // HISTORY
    (23, 17, 0, 0),  // HOT CUE BANK
    (26, 26, 8, 1),  // MATCHING
    (27, 21, 10, 1), // DATE ADDED
];

/// The sort columns, in the same shape: row id, position in [`COLUMNS`],
/// order, visible, and whether it doubles as the second line of a browse row.
const SORTS: [(i64, usize, i64, i64, i64); 17] = [
    (0, 24, 1, 1, 0),  // DEFAULT
    (1, 25, 2, 1, 0),  // ALPHABET
    (2, 1, 3, 1, 0),   // ARTIST
    (3, 2, 4, 1, 0),   // ALBUM
    (4, 4, 5, 1, 0),   // BPM
    (5, 5, 6, 1, 0),   // RATING
    (6, 0, 0, 0, 0),   // GENRE
    (7, 20, 0, 0, 0),  // COMMENTS
    (8, 13, 0, 0, 0),  // TIME
    (9, 7, 0, 0, 0),   // REMIXER
    (10, 8, 0, 0, 0),  // LABEL
    (11, 9, 0, 0, 0),  // ORIGINAL ARTIST
    (12, 10, 7, 1, 0), // KEY
    (13, 14, 0, 0, 0), // BITRATE
    (15, 12, 0, 0, 0), // COLOR
    (16, 22, 0, 0, 0), // DJ PLAY COUNT
    (17, 21, 0, 0, 0), // DATE ADDED
];

/// rekordbox's own four My Tag groups, and nothing under them.
///
/// A drive this program writes has no My Tags on it — the collection has no
/// such thing yet — so the groups are here for the player's tag editor to hang
/// its own on, and the tags themselves are the user's to make.
const MY_TAG_GROUPS: [(i64, &str); 4] =
    [(1, "Genre"), (2, "Components"), (3, "Situation"), (4, "Untitled Column")];

/// A fully analysed track, as the writer whose drive played on a CDJ-3000
/// marked one. Real exports have been reported carrying other values, and
/// nobody has established what the bits mean.
const ANALYSED: i64 = 105;

/// Whether this looks like the key for the *other* encrypted rekordbox
/// database.
///
/// `master.db`'s key is sixty-four hex digits; this one is sixty-four
/// characters of ordinary text. Somebody who has been reading their own
/// library will have the first one to hand, and writing a drive with it
/// produces a file that is perfectly valid, encrypted with the wrong key, and
/// silently unreadable by every player — which is worth one comparison to
/// avoid.
pub fn is_the_other_key(key: &str) -> bool {
    key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit())
}

/// Build the database, and hand back the bytes to put on the drive.
///
/// SQLCipher writes files, not buffers, so this builds one in a temporary
/// place and reads it back — which also means the drive never sees a
/// half-written database, and that a FAT image gets the same bytes a folder
/// does.
pub fn to_bytes(database: &Database, key: &str, device_name: &str) -> Result<Vec<u8>> {
    if key.trim().is_empty() {
        bail!("no OneLibrary key");
    }
    if is_the_other_key(key) {
        bail!(
            "that is the master.db key, not the OneLibrary one — they are different keys, and \
             a drive written with the wrong one is unreadable by every player"
        );
    }

    let scratch = Scratch::new()?;
    {
        let connection = Connection::open(&scratch.path)
            .with_context(|| format!("creating {}", scratch.path.display()))?;
        connection
            .execute_batch(&format!("PRAGMA key = '{}'", key.replace('\'', "''")))
            .context("setting the key")?;
        // Everything in the one file. rekordbox leaves most of a fresh export
        // in a write-ahead log beside the database and checkpoints it when the
        // drive is ejected; a drive that is unplugged before that looks empty
        // to a player. Journalling in the file we are about to copy has the
        // same end state with nothing to forget.
        connection.execute_batch("PRAGMA journal_mode = DELETE").context("journal mode")?;
        connection.execute_batch(SCHEMA).context("creating the tables")?;
        fill(&connection, database, device_name)?;
    }

    let bytes = std::fs::read(&scratch.path)
        .with_context(|| format!("reading back {}", scratch.path.display()))?;
    Ok(bytes)
}

/// Everything that goes in the file, in one transaction.
fn fill(connection: &Connection, database: &Database, device_name: &str) -> Result<()> {
    let today: String = connection.query_row("SELECT date('now')", [], |row| row.get(0))?;

    connection.execute_batch("BEGIN")?;
    seed(connection)?;

    // The lookups, before the tracks that point at them. Case-insensitively
    // deduplicated, because "Peverelist" and "peverelist" are one artist to
    // everyone except a string comparison.
    let artists = Lookup::build(database.tracks.iter().map(|t| t.artist.as_str()));
    let genres = Lookup::build(database.tracks.iter().map(|t| t.genre.as_str()));
    let labels = Lookup::build(database.tracks.iter().map(|t| t.label.as_str()));
    let albums = Lookup::build(database.tracks.iter().map(|t| t.album.as_str()));

    for (name, id) in artists.rows() {
        connection.execute(
            "INSERT INTO artist (artist_id, name, nameForSearch) VALUES (?1, ?2, ?3)",
            params![id, name, name.to_lowercase()],
        )?;
    }
    for (name, id) in genres.rows() {
        connection
            .execute("INSERT INTO genre (genre_id, name) VALUES (?1, ?2)", params![id, name])?;
    }
    for (name, id) in labels.rows() {
        connection
            .execute("INSERT INTO label (label_id, name) VALUES (?1, ?2)", params![id, name])?;
    }
    // An album belongs to an artist, and which one is a question a track
    // answers: the first track that names the album says whose it is.
    let mut album_artist: HashMap<i64, i64> = HashMap::new();
    for track in &database.tracks {
        if let (Some(album), Some(artist)) = (albums.get(&track.album), artists.get(&track.artist))
        {
            album_artist.entry(album).or_insert(artist);
        }
    }
    for (name, id) in albums.rows() {
        connection.execute(
            "INSERT INTO album (album_id, name, artist_id, image_id, isComplation, nameForSearch) \
             VALUES (?1, ?2, ?3, 0, 0, ?4)",
            params![id, name, album_artist.get(&id).copied().unwrap_or(0), name.to_lowercase()],
        )?;
    }

    for track in &database.tracks {
        insert_track(
            connection,
            track,
            &today,
            artists.get(&track.artist).unwrap_or(0),
            albums.get(&track.album).unwrap_or(0),
            genres.get(&track.genre).unwrap_or(0),
            labels.get(&track.label).unwrap_or(0),
        )?;
    }

    for playlist in &database.playlists {
        connection.execute(
            "INSERT INTO playlist (playlist_id, sequenceNo, name, image_id, attribute, \
             playlist_id_parent) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                playlist.id,
                playlist.sort_order,
                playlist.name,
                i64::from(playlist.is_folder),
                playlist.parent_id
            ],
        )?;
        for (n, id) in playlist.track_ids.iter().enumerate() {
            connection.execute(
                "INSERT INTO playlist_content (playlist_id, content_id, sequenceNo) \
                 VALUES (?1, ?2, ?3)",
                params![playlist.id, id, n as i64 + 1],
            )?;
        }
    }

    connection.execute(
        "INSERT INTO property (deviceName, dbVersion, numberOfContents, createdDate, \
         backGroundColorType, myTagMasterDBID) VALUES (?1, ?2, ?3, ?4, 0, 0)",
        params![device_name, DB_VERSION, database.tracks.len() as i64, today],
    )?;
    connection.execute_batch("COMMIT")?;
    Ok(())
}

/// One track's row. Forty-six columns, in the order a real export declares
/// them.
fn insert_track(
    connection: &Connection,
    track: &Track,
    today: &str,
    artist_id: i64,
    album_id: i64,
    genre_id: i64,
    label_id: i64,
) -> Result<()> {
    let added = match track.date_added.is_empty() {
        true => today,
        false => track.date_added.as_str(),
    };
    let file_name = track.file_path.rsplit('/').next().unwrap_or(&track.file_path);
    connection.execute(
        "INSERT INTO content (\
            content_id, title, titleForSearch, subtitle, bpmx100, length, trackNo, discNo, \
            artist_id_artist, artist_id_remixer, artist_id_originalArtist, artist_id_composer, \
            artist_id_lyricist, album_id, genre_id, label_id, key_id, color_id, image_id, \
            djComment, rating, releaseYear, releaseDate, dateCreated, dateAdded, path, \
            fileName, fileSize, fileType, bitrate, bitDepth, samplingRate, isrc, djPlayCount, \
            isHotCueAutoLoadOn, isKuvoDeliverStatusOn, kuvoDeliveryComment, masterDbId, \
            masterContentId, analysisDataFilePath, analysedBits, contentLink, hasModified, \
            cueUpdateCount, analysisDataUpdateCount, informationUpdateCount) \
         VALUES (?1, ?2, ?3, '', ?4, ?5, ?6, ?7, ?8, 0, 0, 0, 0, ?9, ?10, ?11, ?12, ?13, 0, \
            ?14, ?15, ?16, '', ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, '', ?26, 0, 0, '', \
            0, 0, ?27, ?28, 0, 0, 0, 0, 0)",
        params![
            track.id,
            track.title,
            track.title.to_lowercase(),
            track.tempo_x100,
            track.duration_secs,
            track.track_number,
            track.disc_number,
            artist_id,
            album_id,
            genre_id,
            label_id,
            key_id(&track.key).unwrap_or(0),
            i64::from(track.color_id),
            track.comment,
            track.rating,
            track.year,
            added,
            added,
            track.file_path,
            file_name,
            track.file_size,
            file_type(&track.file_path),
            track.bitrate,
            track.sample_depth,
            track.sample_rate,
            track.play_count,
            track.analyze_path,
            ANALYSED,
        ],
    )?;
    Ok(())
}

/// The rows every drive carries whatever is on it: the palette, the keys, the
/// browse menu and the tag groups.
fn seed(connection: &Connection) -> Result<()> {
    for (i, name) in COLORS.iter().enumerate() {
        connection.execute(
            "INSERT INTO color (color_id, name) VALUES (?1, ?2)",
            params![i as i64 + 1, name],
        )?;
    }
    for (i, name) in KEYS.iter().enumerate() {
        connection.execute(
            "INSERT INTO key (key_id, name) VALUES (?1, ?2)",
            params![i as i64 + 1, name],
        )?;
    }
    for (i, (kind, name)) in COLUMNS.iter().enumerate() {
        // Wrapped the way rekordbox wraps them, which is how a player knows it
        // is looking at a label to translate rather than one to print.
        connection.execute(
            "INSERT INTO menuItem (menuItem_id, kind, name) VALUES (?1, ?2, ?3)",
            params![i as i64 + 1, kind, format!("\u{fffa}{name}\u{fffb}")],
        )?;
    }
    for (id, item, order, visible) in CATEGORIES {
        connection.execute(
            "INSERT INTO category (category_id, menuItem_id, sequenceNo, isVisible) \
             VALUES (?1, ?2, ?3, ?4)",
            params![id, item as i64 + 1, order, visible],
        )?;
    }
    for (id, item, order, visible, sub) in SORTS {
        connection.execute(
            "INSERT INTO sort (sort_id, menuItem_id, sequenceNo, isVisible, \
             isSelectedAsSubColumn) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, item as i64 + 1, order, visible, sub],
        )?;
    }
    for (id, name) in MY_TAG_GROUPS {
        connection.execute(
            "INSERT INTO myTag (myTag_id, sequenceNo, name, attribute, myTag_id_parent) \
             VALUES (?1, ?2, ?3, 1, 0)",
            params![id, id, name],
        )?;
    }
    Ok(())
}

/// The id of a key named either way round, or `None` for anything else.
///
/// A library that stores "8A" and one that stores "Am" mean the same key, and
/// a player only knows the second. Anything unrecognised is no key rather than
/// a wrong one.
pub fn key_id(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let upper = text.to_uppercase();
    if let Some((_, id)) = CAMELOT.iter().find(|(name, _)| *name == upper) {
        return Some(*id);
    }
    KEYS.iter().position(|name| name.eq_ignore_ascii_case(text)).map(|i| i as i64 + 1)
}

/// The code a player expects for a container, from its extension.
///
/// The one field in the row where the published sources disagree: these are
/// the codes three writers and `pyrekordbox`'s own enumeration use, and one
/// other library's enumeration says something different. Nobody has published
/// the codes read back out of a real export beside the files they describe.
pub fn file_type(path: &str) -> i64 {
    let lower = path.to_lowercase();
    match () {
        _ if lower.ends_with(".mp3") => 1,
        _ if lower.ends_with(".m4a") || lower.ends_with(".aac") => 4,
        _ if lower.ends_with(".flac") => 5,
        _ if lower.ends_with(".wav") => 11,
        _ if lower.ends_with(".aif") || lower.ends_with(".aiff") => 12,
        _ => 0,
    }
}

/// Names to ids, one id per distinct name, blanks skipped.
struct Lookup {
    ids: HashMap<String, i64>,
    order: Vec<(String, i64)>,
}

impl Lookup {
    fn build<'a>(names: impl Iterator<Item = &'a str>) -> Self {
        let mut lookup = Lookup { ids: HashMap::new(), order: Vec::new() };
        for name in names {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let folded = name.to_lowercase();
            if lookup.ids.contains_key(&folded) {
                continue;
            }
            let id = lookup.order.len() as i64 + 1;
            lookup.ids.insert(folded, id);
            lookup.order.push((name.to_string(), id));
        }
        lookup
    }

    fn get(&self, name: &str) -> Option<i64> {
        self.ids.get(&name.trim().to_lowercase()).copied()
    }

    fn rows(&self) -> impl Iterator<Item = (&str, i64)> {
        self.order.iter().map(|(name, id)| (name.as_str(), *id))
    }
}

/// A file to build in and then take away, including the journal SQLite may
/// leave beside it.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(
            "musicai-onelibrary-{}-{}-{}.db",
            std::process::id(),
            stamp,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for side in ["-wal", "-shm", "-journal"] {
            let mut beside = self.path.clone().into_os_string();
            beside.push(side);
            let _ = std::fs::remove_file(PathBuf::from(beside));
        }
    }
}

/// What a written database says, read back the way a reader would read it.
///
/// The export is not finished until this works: the bytes go on the drive, and
/// then they come off it again and get opened, keyed and counted. A database
/// that only this program's writer can read is not evidence of anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub tables: usize,
    pub tracks: i64,
    pub playlists: i64,
    pub entries: i64,
}

/// Open a OneLibrary database held in memory and say what is in it.
pub fn inspect(bytes: &[u8], key: &str) -> Result<Summary> {
    let scratch = Scratch::new()?;
    std::fs::write(&scratch.path, bytes)
        .with_context(|| format!("writing {}", scratch.path.display()))?;
    let connection = crate::rekordbox::open(&scratch.path, key)?;
    let count = |sql: &str| -> Result<i64> {
        Ok(connection.query_row(sql, [], |row| row.get::<_, i64>(0))?)
    };
    Ok(Summary {
        tables: crate::rekordbox::tables(&connection)?.len(),
        tracks: count("SELECT count(*) FROM content")?,
        playlists: count("SELECT count(*) FROM playlist")?,
        entries: count("SELECT count(*) FROM playlist_content")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::pdb::{Database, Playlist, Track};

    /// Not the real one, on purpose: SQLCipher takes any passphrase, and what
    /// these tests check is that what was written reads back. Using the real
    /// key here would test the same thing while making every fixture a file
    /// that opens in rekordbox, which is not what a test is for.
    const KEY: &str = "a-key-that-is-not-the-real-one";

    fn a_track(id: u32, title: &str, artist: &str) -> Track {
        Track {
            id,
            title: title.into(),
            artist: artist.into(),
            album: "Livity Sound".into(),
            genre: "Techno".into(),
            label: "Livity".into(),
            key: "8A".into(),
            comment: "a comment".into(),
            file_path: format!("/Contents/{artist}/{title}.flac"),
            analyze_path: format!("/PIONEER/USBANLZ/P{:03}/{id:08X}/ANLZ0000.DAT", id % 1000),
            tempo_x100: 12400,
            duration_secs: 321,
            sample_rate: 44100,
            sample_depth: 16,
            bitrate: 1411,
            file_size: 40_000_000,
            track_number: 2,
            disc_number: 1,
            year: 2019,
            rating: 4,
            color_id: 3,
            play_count: 7,
            date_added: "2026-09-03".into(),
            analyze_date: "2026-09-03".into(),
        }
    }

    fn a_database() -> Database {
        Database {
            tracks: vec![
                a_track(1, "Roll With The Punches", "Peverelist"),
                a_track(2, "Undulate", "Peverelist"),
            ],
            playlists: vec![Playlist::new(1, "A night", vec![1, 2])],
        }
    }

    #[test]
    fn what_was_written_reads_back_through_a_reader_that_shares_no_code_with_it() {
        let bytes = to_bytes(&a_database(), KEY, "TESTSTICK").unwrap();
        let summary = inspect(&bytes, KEY).unwrap();
        assert_eq!(summary.tables, 22, "a real export has twenty-two tables");
        assert_eq!(summary.tracks, 2);
        assert_eq!(summary.playlists, 1);
        assert_eq!(summary.entries, 2);
    }

    #[test]
    fn it_is_encrypted_rather_than_a_sqlite_file_with_a_password_on_it() {
        let bytes = to_bytes(&a_database(), KEY, "TESTSTICK").unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3"), "the header is not in the clear");
        assert!(inspect(&bytes, "some-other-key").is_err(), "another key must not open it");
        assert!(
            !bytes.windows(9).any(|w| w == b"Peverelist"[..9].as_ref()),
            "no track name is readable in the file"
        );
    }

    #[test]
    fn a_track_row_says_what_the_track_says() {
        let bytes = to_bytes(&a_database(), KEY, "TESTSTICK").unwrap();
        let scratch = Scratch::new().unwrap();
        std::fs::write(&scratch.path, &bytes).unwrap();
        let connection = crate::rekordbox::open(&scratch.path, KEY).unwrap();

        let (title, bpm, path, analysis, file_type, rating, plays): (
            String,
            i64,
            String,
            String,
            i64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT title, bpmx100, path, analysisDataFilePath, fileType, rating, djPlayCount \
                 FROM content WHERE content_id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(title, "Roll With The Punches");
        assert_eq!(bpm, 12400, "centi-BPM, not BPM");
        assert_eq!(path, "/Contents/Peverelist/Roll With The Punches.flac");
        assert_eq!(analysis, "/PIONEER/USBANLZ/P001/00000001/ANLZ0000.DAT");
        assert_eq!(file_type, 5, "FLAC");
        assert_eq!(rating, 4, "stars, not the legacy format's 0/51/…/255");
        assert_eq!(plays, 7);

        // The key came in as Camelot and has to leave as something a player
        // can print.
        let key: String = connection
            .query_row(
                "SELECT key.name FROM content JOIN key ON key.key_id = content.key_id \
                 WHERE content_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(key, "Am", "8A is A minor");
    }

    #[test]
    fn the_browse_menu_is_the_one_the_other_database_writes() {
        // Both databases describe the same menu to the same players. If these
        // two ever disagree it is because somebody changed one of them, which
        // is exactly the thing worth failing a build over.
        let bytes = to_bytes(&a_database(), KEY, "TESTSTICK").unwrap();
        let scratch = Scratch::new().unwrap();
        std::fs::write(&scratch.path, &bytes).unwrap();
        let connection = crate::rekordbox::open(&scratch.path, KEY).unwrap();

        let mut statement =
            connection.prepare("SELECT kind, name FROM menuItem ORDER BY menuItem_id").unwrap();
        let rows: Vec<(i64, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 27);
        assert_eq!(rows[0], (128, "\u{fffa}GENRE\u{fffb}".to_string()));
        assert_eq!(rows[22], (151, "\u{fffa}DJ PLAY COUNT\u{fffb}".to_string()));
        for (n, (kind, name)) in rows.iter().enumerate() {
            assert_eq!(*kind, i64::from(COLUMNS[n].0));
            assert_eq!(name, &format!("\u{fffa}{}\u{fffb}", COLUMNS[n].1));
        }

        let counts = |table: &str| -> i64 {
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(counts("category"), 22);
        assert_eq!(counts("sort"), 17);
        assert_eq!(counts("color"), 8);
        assert_eq!(counts("key"), 24);
        assert_eq!(counts("cue"), 0, "rekordbox leaves this empty; the cues are in the ANLZ");
    }

    #[test]
    fn the_wrong_one_of_the_two_rekordbox_keys_is_refused_rather_than_written() {
        // A file encrypted with the master.db key is a valid file no player can
        // read, and the mistake is easy: this build carries both keys, so the
        // wrong one is always to hand.
        let hex = crate::rekordbox::BUNDLED_KEY;
        assert!(is_the_other_key(hex));
        assert!(!is_the_other_key(crate::rekordbox::BUNDLED_ONELIBRARY_KEY));
        assert!(!is_the_other_key(KEY));
        let refused = to_bytes(&a_database(), hex, "TESTSTICK").unwrap_err().to_string();
        assert!(refused.contains("master.db key"), "{refused}");
    }

    #[test]
    fn a_key_reads_either_way_round_and_an_unknown_one_is_no_key() {
        assert_eq!(key_id("8A"), Some(22));
        assert_eq!(key_id("am"), Some(22));
        assert_eq!(key_id("Am"), Some(22));
        assert_eq!(key_id("12B"), Some(4));
        assert_eq!(key_id("Eb"), Some(4));
        assert_eq!(key_id(""), None);
        assert_eq!(key_id("H sharp"), None);
    }

    #[test]
    fn the_lookups_hold_one_row_per_name_however_it_was_capitalised() {
        let mut database = a_database();
        database.tracks[1].artist = "PEVERELIST".into();
        database.tracks[1].genre = String::new();
        let bytes = to_bytes(&database, KEY, "TESTSTICK").unwrap();
        let scratch = Scratch::new().unwrap();
        std::fs::write(&scratch.path, &bytes).unwrap();
        let connection = crate::rekordbox::open(&scratch.path, KEY).unwrap();

        let artists: i64 =
            connection.query_row("SELECT count(*) FROM artist", [], |row| row.get(0)).unwrap();
        assert_eq!(artists, 1, "one artist, spelled two ways");
        let genres: i64 =
            connection.query_row("SELECT count(*) FROM genre", [], |row| row.get(0)).unwrap();
        assert_eq!(genres, 1, "a track with no genre adds no row");
        let orphan: i64 = connection
            .query_row("SELECT genre_id FROM content WHERE content_id = 2", [], |row| row.get(0))
            .unwrap();
        assert_eq!(orphan, 0, "and points at nothing rather than at somebody else's genre");
    }

    #[test]
    fn the_drive_says_how_many_tracks_are_on_it() {
        let bytes = to_bytes(&a_database(), KEY, "MYSTICK").unwrap();
        let scratch = Scratch::new().unwrap();
        std::fs::write(&scratch.path, &bytes).unwrap();
        let connection = crate::rekordbox::open(&scratch.path, KEY).unwrap();
        let (name, version, contents): (String, String, i64) = connection
            .query_row("SELECT deviceName, dbVersion, numberOfContents FROM property", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(name, "MYSTICK");
        assert_eq!(version, "1000");
        assert_eq!(contents, 2);
    }

    #[test]
    fn a_folder_of_playlists_keeps_its_shape() {
        let database = Database {
            tracks: vec![a_track(1, "One", "Peverelist")],
            playlists: vec![
                Playlist::folder(1, "Nights"),
                Playlist {
                    id: 2,
                    parent_id: 1,
                    sort_order: 1,
                    name: "Late".into(),
                    is_folder: false,
                    track_ids: vec![1],
                },
            ],
        };
        let bytes = to_bytes(&database, KEY, "TESTSTICK").unwrap();
        let scratch = Scratch::new().unwrap();
        std::fs::write(&scratch.path, &bytes).unwrap();
        let connection = crate::rekordbox::open(&scratch.path, KEY).unwrap();
        let (attribute, parent): (i64, i64) = connection
            .query_row(
                "SELECT attribute, playlist_id_parent FROM playlist WHERE playlist_id = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(attribute, 0, "a list, not a folder");
        assert_eq!(parent, 1, "inside the folder it was written in");
        let folder: i64 = connection
            .query_row("SELECT attribute FROM playlist WHERE playlist_id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(folder, 1);
    }

    #[test]
    fn writing_without_a_key_writes_nothing() {
        assert!(to_bytes(&a_database(), "", "TESTSTICK").is_err());
        assert!(to_bytes(&a_database(), "   ", "TESTSTICK").is_err());
    }

    #[test]
    fn the_scratch_file_does_not_outlive_the_write() {
        let path = {
            let scratch = Scratch::new().unwrap();
            std::fs::write(&scratch.path, b"something").unwrap();
            scratch.path.clone()
        };
        assert!(!path.exists(), "a database left in the temp directory is somebody's library");
    }
}
