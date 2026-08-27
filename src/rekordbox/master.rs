//! Reading `master.db` — rekordbox's library on your own machine.
//!
//! The schema moved between rekordbox 6 and 7 and will move again, so every
//! column is looked up rather than assumed: the table is asked what it has,
//! and anything missing is left empty instead of failing the import. A library
//! that comes across with no colours is worth having; one that refuses to come
//! across because a column was renamed is not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A whole rekordbox library, in terms that mean something outside rekordbox.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Collection {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
}

/// One track, as rekordbox had it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    /// rekordbox's own row id, which is what the playlists and cues point at.
    pub id: String,
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub genre: String,
    pub year: Option<u32>,
    pub bpm: f64,
    /// Camelot or whatever rekordbox stored as the key's name.
    pub key: String,
    /// Stars, 0 to 5.
    pub rating: u8,
    pub comment: String,
    pub play_count: u32,
    pub duration_secs: f64,
    pub cues: Vec<Cue>,
    /// My Tags, which are rekordbox's nearest thing to this program's tags.
    pub my_tags: Vec<String>,
}

/// A cue point, in the terms the rest of this program uses: nought is the
/// memory cue and one to eight are hot cues A to H.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cue {
    pub letter: u8,
    pub time_ms: u32,
    pub label: String,
}

/// A playlist, flattened to the folder it sits in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playlist {
    pub name: String,
    /// The folders above it, joined with `/`. Empty at the top level.
    pub folder: String,
    /// rekordbox track ids, in playlist order.
    pub track_ids: Vec<String>,
    /// Smart playlists come across as their contents at the moment of import,
    /// because their rules are a rekordbox dialect this program does not speak.
    pub was_smart: bool,
}

/// Read a whole library out of an already-opened database.
pub fn read(connection: &rusqlite::Connection) -> Result<Collection> {
    let artists = names(connection, "djmdArtist")?;
    let albums = names(connection, "djmdAlbum")?;
    let genres = names(connection, "djmdGenre")?;
    let keys = names_from(connection, "djmdKey", "ScaleName")?;

    let mut cues = cues_by_track(connection).unwrap_or_default();
    let mut my_tags = my_tags_by_track(connection).unwrap_or_default();
    let tracks = read_tracks(connection, &artists, &albums, &genres, &keys, &mut cues, &mut my_tags)
        .context("reading djmdContent")?;
    let playlists = read_playlists(connection).unwrap_or_default();

    Ok(Collection { tracks, playlists })
}

/// The `ID` to `Name` mapping of one of the little lookup tables.
fn names(connection: &rusqlite::Connection, table: &str) -> Result<HashMap<String, String>> {
    names_from(connection, table, "Name")
}

fn names_from(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<HashMap<String, String>> {
    if !super::columns(connection, table).map(|c| c.iter().any(|n| n == column)).unwrap_or(false) {
        return Ok(HashMap::new());
    }
    let mut statement = connection.prepare(&format!("SELECT ID, \"{column}\" FROM \"{table}\""))?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut found = HashMap::new();
    for row in rows.flatten() {
        if let (Some(id), Some(name)) = row {
            found.insert(id, name);
        }
    }
    Ok(found)
}

/// Which of the columns this build knows about the table in front of it has.
///
/// Everything downstream reads by name through this, so a rekordbox version
/// that dropped or renamed something loses that field rather than the import.
struct Present(Vec<String>);

impl Present {
    fn of(connection: &rusqlite::Connection, table: &str) -> Self {
        Self(super::columns(connection, table).unwrap_or_default())
    }

    fn has(&self, column: &str) -> bool {
        self.0.iter().any(|name| name == column)
    }

    /// The wanted columns that exist, as a `SELECT` list, with the missing
    /// ones standing in as nulls so the column positions still line up.
    fn select(&self, wanted: &[&str]) -> String {
        wanted
            .iter()
            .map(|column| match self.has(column) {
                true => format!("\"{column}\""),
                false => format!("NULL AS \"{column}\""),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// rekordbox soft-deletes: a row that is gone from the app is still in the
    /// file with a flag set. Importing those would bring back everything the
    /// user has ever thrown away.
    fn alive(&self) -> &'static str {
        match self.has("rb_local_deleted") {
            true => " WHERE COALESCE(rb_local_deleted, 0) = 0",
            false => "",
        }
    }
}

const TRACK_COLUMNS: [&str; 15] = [
    "ID",
    "FolderPath",
    "FileNameL",
    "Title",
    "ArtistID",
    "AlbumID",
    "GenreID",
    "KeyID",
    "BPM",
    "Length",
    "Rating",
    "ReleaseYear",
    "Commnt",
    "DJPlayCount",
    "StockDate",
];

fn read_tracks(
    connection: &rusqlite::Connection,
    artists: &HashMap<String, String>,
    albums: &HashMap<String, String>,
    genres: &HashMap<String, String>,
    keys: &HashMap<String, String>,
    cues: &mut HashMap<String, Vec<Cue>>,
    my_tags: &mut HashMap<String, Vec<String>>,
) -> Result<Vec<Track>> {
    let present = Present::of(connection, "djmdContent");
    let query = format!(
        "SELECT {} FROM djmdContent{}",
        present.select(&TRACK_COLUMNS),
        present.alive()
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], |row| {
        let text = |at: usize| row.get::<_, Option<String>>(at).unwrap_or_default();
        let number = |at: usize| row.get::<_, Option<f64>>(at).unwrap_or_default();
        Ok((
            text(0),
            text(1),
            text(2),
            text(3),
            text(4),
            text(5),
            text(6),
            text(7),
            number(8),
            number(9),
            number(10),
            text(11),
            text(12),
            number(13),
            text(14),
        ))
    })?;

    let mut tracks = Vec::new();
    for row in rows.flatten() {
        let (id, folder, filename, title, artist, album, genre, key, bpm, length, rating, year, comment, plays, _stock) =
            row;
        let Some(id) = id else { continue };
        let path = join_path(folder.as_deref(), filename.as_deref());
        if path.as_os_str().is_empty() {
            continue;
        }
        let lookup = |table: &HashMap<String, String>, id: Option<String>| {
            id.and_then(|id| table.get(&id).cloned()).unwrap_or_default()
        };
        tracks.push(Track {
            path,
            artist: lookup(artists, artist),
            title: title.unwrap_or_default(),
            album: lookup(albums, album),
            genre: lookup(genres, genre),
            key: lookup(keys, key),
            // Stored as hundredths, so that a 128.02 grid stays 128.02.
            bpm: bpm.unwrap_or_default() / 100.0,
            duration_secs: length.unwrap_or_default(),
            rating: rating.unwrap_or_default().clamp(0.0, 5.0) as u8,
            year: year.and_then(|y| y.trim().get(..4)?.parse().ok()),
            comment: comment.unwrap_or_default(),
            play_count: plays.unwrap_or_default().max(0.0) as u32,
            cues: cues.remove(&id).unwrap_or_default(),
            my_tags: my_tags.remove(&id).unwrap_or_default(),
            id,
        });
    }
    Ok(tracks)
}

/// Put a track's folder and file name back together.
///
/// `FolderPath` is sometimes the folder and sometimes the whole path, which is
/// not a thing that can be told from the schema — only by looking. Both
/// readings are tried rather than one being assumed, because assuming produces
/// `/music/track.flac/track.flac` and a library that appears to have no files.
fn join_path(folder: Option<&str>, filename: Option<&str>) -> PathBuf {
    let folder = folder.unwrap_or_default().trim();
    let filename = filename.unwrap_or_default().trim();
    if folder.is_empty() {
        return PathBuf::from(filename);
    }
    if filename.is_empty() {
        return PathBuf::from(folder);
    }
    let already_whole = Path::new(folder)
        .file_name()
        .is_some_and(|name| name.to_string_lossy() == filename);
    match already_whole {
        true => PathBuf::from(folder),
        false => Path::new(folder).join(filename),
    }
}

const CUE_COLUMNS: [&str; 4] = ["ContentID", "InMsec", "Kind", "Comment"];

fn cues_by_track(connection: &rusqlite::Connection) -> Result<HashMap<String, Vec<Cue>>> {
    let present = Present::of(connection, "djmdCue");
    if !present.has("ContentID") {
        return Ok(HashMap::new());
    }
    let query =
        format!("SELECT {} FROM djmdCue{}", present.select(&CUE_COLUMNS), present.alive());
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0).unwrap_or_default(),
            row.get::<_, Option<f64>>(1).unwrap_or_default(),
            row.get::<_, Option<f64>>(2).unwrap_or_default(),
            row.get::<_, Option<String>>(3).unwrap_or_default(),
        ))
    })?;

    let mut found: HashMap<String, Vec<Cue>> = HashMap::new();
    for (content, at, kind, comment) in rows.flatten() {
        let Some(content) = content else { continue };
        let Some(at) = at else { continue };
        if at < 0.0 {
            continue;
        }
        // The convention the rest of this program uses, and the one the
        // analysis files use: nought is the memory cue, one to eight are hot
        // cues A to H. Anything outside that is something this does not
        // understand, and is dropped rather than turned into a cue nobody put
        // there.
        let letter = kind.unwrap_or_default();
        if !(0.0..=8.0).contains(&letter) {
            continue;
        }
        found.entry(content).or_default().push(Cue {
            letter: letter as u8,
            time_ms: at as u32,
            label: comment.unwrap_or_default(),
        });
    }
    for cues in found.values_mut() {
        cues.sort_by_key(|cue| (cue.letter, cue.time_ms));
        cues.dedup_by_key(|cue| cue.letter);
    }
    Ok(found)
}

fn my_tags_by_track(connection: &rusqlite::Connection) -> Result<HashMap<String, Vec<String>>> {
    let tags = names(connection, "djmdMyTag")?;
    if tags.is_empty() {
        return Ok(HashMap::new());
    }
    let present = Present::of(connection, "djmdSongMyTag");
    if !present.has("ContentID") || !present.has("MyTagID") {
        return Ok(HashMap::new());
    }
    let mut statement = connection.prepare(&format!(
        "SELECT ContentID, MyTagID FROM djmdSongMyTag{}",
        present.alive()
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0).unwrap_or_default(),
            row.get::<_, Option<String>>(1).unwrap_or_default(),
        ))
    })?;

    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    for (content, tag) in rows.flatten() {
        let (Some(content), Some(tag)) = (content, tag) else { continue };
        if let Some(name) = tags.get(&tag) {
            found.entry(content).or_default().push(name.clone());
        }
    }
    for names in found.values_mut() {
        names.sort();
        names.dedup();
    }
    Ok(found)
}

fn read_playlists(connection: &rusqlite::Connection) -> Result<Vec<Playlist>> {
    let present = Present::of(connection, "djmdPlaylist");
    if !present.has("Name") {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(&format!(
        "SELECT {} FROM djmdPlaylist{}",
        present.select(&["ID", "Name", "ParentID", "Attribute", "Seq"]),
        present.alive()
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0).unwrap_or_default(),
            row.get::<_, Option<String>>(1).unwrap_or_default(),
            row.get::<_, Option<String>>(2).unwrap_or_default(),
            row.get::<_, Option<f64>>(3).unwrap_or_default(),
            row.get::<_, Option<f64>>(4).unwrap_or_default(),
        ))
    })?;

    struct Node {
        name: String,
        parent: Option<String>,
        attribute: i64,
        seq: i64,
    }
    let mut nodes: HashMap<String, Node> = HashMap::new();
    for (id, name, parent, attribute, seq) in rows.flatten() {
        let Some(id) = id else { continue };
        nodes.insert(
            id,
            Node {
                name: name.unwrap_or_default(),
                // rekordbox writes a literal "root" as well as null; both mean
                // the same thing and neither is a folder anybody named.
                parent: parent.filter(|p| !p.is_empty() && p != "root" && p != "0"),
                attribute: attribute.unwrap_or_default() as i64,
                seq: seq.unwrap_or_default() as i64,
            },
        );
    }

    let tree: Tree =
        nodes.iter().map(|(id, n)| (id.clone(), (n.name.clone(), n.parent.clone()))).collect();
    let contents = playlist_contents(connection).unwrap_or_default();
    let mut playlists: Vec<(i64, Playlist)> = Vec::new();
    for (id, node) in &nodes {
        // Attribute 1 is a folder, and a folder is not a playlist — it is the
        // path to one, and it comes across as part of the name of what is
        // inside it.
        if node.attribute == 1 {
            continue;
        }
        playlists.push((
            node.seq,
            Playlist {
                name: node.name.clone(),
                folder: folder_of(&tree, node.parent.as_deref()),
                track_ids: contents.get(id).cloned().unwrap_or_default(),
                was_smart: node.attribute == 4,
            },
        ));
    }
    playlists.sort_by(|a, b| {
        a.1.folder.cmp(&b.1.folder).then(a.0.cmp(&b.0)).then(a.1.name.cmp(&b.1.name))
    });
    Ok(playlists.into_iter().map(|(_, playlist)| playlist).collect())
}

/// A folder tree, as id to (name, parent).
type Tree = HashMap<String, (String, Option<String>)>;

/// The path down to a folder, joining names with a slash.
///
/// Bounded rather than recursive: a parent link that points at itself is a
/// corrupt row, not a reason to run out of stack.
const MAX_DEPTH: usize = 32;

fn folder_of(tree: &Tree, from: Option<&str>) -> String {
    let mut parts = Vec::new();
    let mut at = from.map(str::to_string);
    for _ in 0..MAX_DEPTH {
        let Some(id) = at else { break };
        let Some((name, parent)) = tree.get(&id) else { break };
        parts.push(name.clone());
        at = parent.clone();
    }
    parts.reverse();
    parts.join("/")
}

fn playlist_contents(connection: &rusqlite::Connection) -> Result<HashMap<String, Vec<String>>> {
    let present = Present::of(connection, "djmdSongPlaylist");
    if !present.has("PlaylistID") || !present.has("ContentID") {
        return Ok(HashMap::new());
    }
    let mut statement = connection.prepare(&format!(
        "SELECT {} FROM djmdSongPlaylist{}",
        present.select(&["PlaylistID", "ContentID", "TrackNo"]),
        present.alive()
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0).unwrap_or_default(),
            row.get::<_, Option<String>>(1).unwrap_or_default(),
            row.get::<_, Option<f64>>(2).unwrap_or_default(),
        ))
    })?;

    let mut found: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (playlist, content, at) in rows.flatten() {
        let (Some(playlist), Some(content)) = (playlist, content) else { continue };
        found.entry(playlist).or_default().push((at.unwrap_or_default() as i64, content));
    }
    Ok(found
        .into_iter()
        .map(|(id, mut entries)| {
            entries.sort_by_key(|(at, _)| *at);
            (id, entries.into_iter().map(|(_, content)| content).collect())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_path_that_is_already_the_whole_path_is_not_doubled() {
        // Both readings are in the wild, and getting this wrong produces
        // /music/a.flac/a.flac — a library that appears to have no files.
        assert_eq!(
            join_path(Some("/music/a.flac"), Some("a.flac")),
            PathBuf::from("/music/a.flac")
        );
        assert_eq!(join_path(Some("/music"), Some("a.flac")), PathBuf::from("/music/a.flac"));
        assert_eq!(join_path(Some("/music/"), Some("a.flac")), PathBuf::from("/music/a.flac"));
    }

    #[test]
    fn half_a_path_is_better_than_none() {
        assert_eq!(join_path(None, Some("a.flac")), PathBuf::from("a.flac"));
        assert_eq!(join_path(Some("/music"), None), PathBuf::from("/music"));
        assert_eq!(join_path(None, None), PathBuf::new());
        assert_eq!(join_path(Some("  "), Some(" ")), PathBuf::new());
    }

    #[test]
    fn a_folder_chain_becomes_a_path_and_a_loop_does_not_hang() {
        let mut tree = Tree::new();
        tree.insert("1".into(), ("2026".into(), None));
        tree.insert("2".into(), ("March".into(), Some("1".into())));
        assert_eq!(folder_of(&tree, Some("2")), "2026/March");
        assert_eq!(folder_of(&tree, None), "", "the top level is not a folder called anything");
        assert_eq!(folder_of(&tree, Some("nope")), "", "a parent that is not there");

        // A row that points at itself is corrupt, not a reason to hang.
        tree.insert("3".into(), ("loop".into(), Some("3".into())));
        assert_eq!(folder_of(&tree, Some("3")).matches('/').count(), MAX_DEPTH - 1);
    }
}
