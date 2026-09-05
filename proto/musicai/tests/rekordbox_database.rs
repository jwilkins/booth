//! Checking `export.pdb` against somebody else's parser.
//!
//! Same reasoning as `rekordbox_export.rs`: a database our own code can read
//! back proves only that we are consistent, not that we are right. These build
//! databases and parse them with [`rekordcrate`], which reads the format from
//! its own independent implementation and refuses anything malformed.

use std::io::Cursor;

use binrw::BinRead;
use booth_core::export::pdb::{Database, Playlist, Track};
use rekordcrate::pdb::{Header, PageType, Row};

fn track(id: u32, title: &str, artist: &str) -> Track {
    Track {
        id,
        title: title.to_string(),
        artist: artist.to_string(),
        album: "Livity Sound".to_string(),
        genre: "Techno".to_string(),
        label: "Livity Sound".to_string(),
        key: "8A".to_string(),
        comment: "works out of anything in 8A".to_string(),
        file_path: format!("/Contents/{artist}/{title}.flac"),
        analyze_path: format!("/PIONEER/USBANLZ/P{:03}/{:08X}/ANLZ0000.DAT", id % 1000, id),
        tempo_x100: 12_802,
        duration_secs: 401,
        sample_rate: 44_100,
        sample_depth: 16,
        bitrate: 1_411,
        file_size: 42_000_000,
        track_number: id,
        year: 2019,
        rating: 4,
        color_id: 3,
        date_added: "2026-08-20".to_string(),
        analyze_date: "2026-08-20".to_string(),
        ..Track::default()
    }
}

/// Read every row of every table, the way a player walks the database.
fn read_all(bytes: &[u8]) -> Vec<(PageType, Vec<Row>)> {
    let mut cursor = Cursor::new(bytes);
    let header = Header::read(&mut cursor).expect("rekordcrate could not read the header");
    assert_eq!(header.page_size, 4_096);

    let mut out = Vec::new();
    for table in &header.tables {
        let pages = header
            .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
            .unwrap_or_else(|e| panic!("reading {:?} pages: {e}", table.page_type));
        let rows: Vec<Row> = pages
            .iter()
            .filter(|p| p.has_data())
            .flat_map(|p| p.row_groups.iter().flat_map(|g| g.present_rows()))
            .collect();
        out.push((table.page_type, rows));
    }
    out
}

fn rows_of(all: &[(PageType, Vec<Row>)], want: PageType) -> &[Row] {
    all.iter().find(|(t, _)| *t == want).map(|(_, r)| r.as_slice()).expect("table missing")
}

fn described(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| format!("{r:?}")).collect()
}

/// The one row that mentions `needle`.
///
/// Rows are looked up by content rather than by position throughout: the order
/// a reader hands them back in is an artefact of how the row index is walked,
/// not a property of the table, and a database is a set of rows with playlists
/// to impose order on them.
fn find(rows: &[Row], needle: &str) -> String {
    let matches: Vec<String> = described(rows).into_iter().filter(|r| r.contains(needle)).collect();
    match matches.len() {
        1 => matches.into_iter().next().unwrap(),
        0 => panic!("no row mentions {needle:?}"),
        n => panic!("{n} rows mention {needle:?}"),
    }
}

fn demo() -> Database {
    Database {
        tracks: vec![
            track(1, "Roll With The Punches", "Peverelist"),
            track(2, "Marius", "Batu"),
            track(3, "Just Getting Started", "Bruce"),
        ],
        playlists: vec![
            Playlist::folder(1, "Sat 14/9"),
            Playlist::new(2, "warm", vec![3]).in_folder(1),
            Playlist::new(3, "peak", vec![1, 2, 3]).in_folder(1),
        ],
    }
}

#[test]
fn the_header_lists_every_table_a_player_expects() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);
    let types: Vec<PageType> = all.iter().map(|(t, _)| *t).collect();

    assert_eq!(types.len(), 20);
    assert_eq!(types[0], PageType::Tracks);
    assert_eq!(types[7], PageType::PlaylistTree);
    assert_eq!(types[8], PageType::PlaylistEntries);
    assert_eq!(types[16], PageType::Columns);
    assert_eq!(types[19], PageType::History);
}

#[test]
fn tracks_come_back_with_their_titles_paths_and_numbers() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);
    let tracks = rows_of(&all, PageType::Tracks);
    assert_eq!(tracks.len(), 3);

    let peverelist = find(tracks, "Roll With The Punches");
    assert!(peverelist.contains("/Contents/Peverelist/Roll With The Punches.flac"));
    assert!(peverelist.contains("ANLZ0000.DAT"));
    assert!(peverelist.contains("tempo: 12802"), "tempo is BPM times 100: {peverelist}");
    assert!(peverelist.contains("duration: 401"), "duration is in seconds: {peverelist}");
    assert!(peverelist.contains("rating: 4"));
    assert!(peverelist.contains("year: 2019"));
    find(tracks, "Just Getting Started");
}

#[test]
fn the_filename_is_derived_from_the_path() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);
    assert!(find(rows_of(&all, PageType::Tracks), "Marius")
        .contains("filename: DeviceSQLString(\"Marius.flac\")"));
}

#[test]
fn names_are_interned_into_their_own_tables_and_shared() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);

    // Three different artists, but one album, genre, label and key between them.
    assert_eq!(rows_of(&all, PageType::Artists).len(), 3);
    assert_eq!(rows_of(&all, PageType::Albums).len(), 1);
    assert_eq!(rows_of(&all, PageType::Genres).len(), 1);
    assert_eq!(rows_of(&all, PageType::Labels).len(), 1);
    assert_eq!(rows_of(&all, PageType::Keys).len(), 1);

    find(rows_of(&all, PageType::Artists), "Peverelist");
    find(rows_of(&all, PageType::Artists), "Batu");
    find(rows_of(&all, PageType::Keys), "8A");
    find(rows_of(&all, PageType::Labels), "Livity Sound");

    // The track rows link to that one key by id.
    let peverelist = find(rows_of(&all, PageType::Tracks), "Roll With The Punches");
    assert!(peverelist.contains("key_id: KeyId(1)"), "{peverelist}");
}

#[test]
fn a_field_a_track_does_not_have_is_left_unset_rather_than_interned() {
    let mut database = demo();
    database.tracks[0].genre = String::new();
    database.tracks[1].genre = String::new();
    database.tracks[2].genre = String::new();
    let bytes = database.to_bytes().unwrap();
    assert!(rows_of(&read_all(&bytes), PageType::Genres).is_empty());
}

#[test]
fn the_playlist_tree_keeps_its_shape() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);
    let tree: Vec<_> = rows_of(&all, PageType::PlaylistTree)
        .iter()
        .filter_map(|r| match r {
            Row::PlaylistTreeNode(node) => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(tree.len(), 3);

    let by_name = |want: &str| {
        tree.iter()
            .find(|n| n.name.clone().into_string().unwrap() == want)
            .unwrap_or_else(|| panic!("no playlist called {want:?}"))
    };
    let folder = by_name("Sat 14/9");
    assert!(folder.is_folder());
    let peak = by_name("peak");
    assert!(!peak.is_folder());
    assert_eq!(peak.parent_id, folder.id, "peak should sit inside the folder");
    assert_eq!(by_name("warm").parent_id, folder.id);
}

#[test]
fn a_playlist_holds_its_tracks_in_order() {
    let bytes = demo().to_bytes().unwrap();
    let all = read_all(&bytes);
    let entries = described(rows_of(&all, PageType::PlaylistEntries));

    // One entry for "warm", three for "peak".
    assert_eq!(entries.len(), 4);
    let in_playlist = |id: u32| -> Vec<&String> {
        entries.iter().filter(|e| e.contains(&format!("PlaylistTreeNodeId({id})"))).collect()
    };
    assert_eq!(in_playlist(2).len(), 1);
    assert_eq!(in_playlist(3).len(), 3);

    // "peak" holds tracks 1, 2 and 3 at positions 1, 2 and 3.
    for (position, track) in [(1, 1), (2, 2), (3, 3)] {
        assert!(
            in_playlist(3).iter().any(|e| e.contains(&format!("entry_index: {position}"))
                && e.contains(&format!("TrackId({track})"))),
            "peak is missing track {track} at position {position}: {:?}",
            in_playlist(3)
        );
    }
}

#[test]
fn the_colour_and_menu_tables_are_written_whether_or_not_they_are_used() {
    let bytes = Database::new().to_bytes().unwrap();
    let all = read_all(&bytes);

    let colors = rows_of(&all, PageType::Colors);
    assert_eq!(colors.len(), 8);
    find(colors, "Pink");
    find(colors, "Purple");

    let columns = rows_of(&all, PageType::Columns);
    assert_eq!(columns.len(), 27);
    find(columns, "GENRE");
    find(columns, "HOT CUE BANK");
}

#[test]
fn an_empty_database_is_still_a_valid_one() {
    let bytes = Database::new().to_bytes().unwrap();
    let all = read_all(&bytes);
    assert!(rows_of(&all, PageType::Tracks).is_empty());
    assert!(rows_of(&all, PageType::PlaylistTree).is_empty());
    // Two pages per table, plus the header page.
    assert_eq!(bytes.len(), 4_096 * (1 + 20 * 2));
}

#[test]
fn a_library_too_big_for_one_page_spills_onto_the_next() {
    let tracks: Vec<Track> =
        (1..=250).map(|i| track(i, &format!("Track {i}"), &format!("Artist {i}"))).collect();
    let ids: Vec<u32> = tracks.iter().map(|t| t.id).collect();
    let database = Database { tracks, playlists: vec![Playlist::new(1, "everything", ids)] };

    let bytes = database.to_bytes().unwrap();
    let all = read_all(&bytes);

    // A track row is a few hundred bytes, so 250 of them cannot fit in one
    // 4 kB page; the point of the test is that following the page chain finds
    // all of them anyway.
    assert!(bytes.len() > 4_096 * 41, "the database should have needed extra pages");
    assert_eq!(rows_of(&all, PageType::Tracks).len(), 250);
    assert_eq!(rows_of(&all, PageType::Artists).len(), 250);
    assert_eq!(rows_of(&all, PageType::PlaylistEntries).len(), 250);

    let tracks = rows_of(&all, PageType::Tracks);
    for i in [1, 137, 250] {
        find(tracks, &format!("title: DeviceSQLString(\"Track {i}\")"));
    }
}

#[test]
fn a_title_that_is_not_ascii_survives() {
    let mut database = demo();
    database.tracks[0].title = "Jóga (Björk cover)".to_string();
    database.tracks[1].title = "\u{6771}\u{4eac}".to_string();
    let bytes = database.to_bytes().unwrap();
    let all = read_all(&bytes);
    find(rows_of(&all, PageType::Tracks), "Jóga (Björk cover)");
    find(rows_of(&all, PageType::Tracks), "\u{6771}\u{4eac}");
}

#[test]
fn a_long_ascii_title_survives_too() {
    // Past 126 characters the short form runs out of length byte, and the
    // string switches to a four-byte header.
    let long = "A".repeat(200);
    let mut database = demo();
    database.tracks[0].title = long.clone();
    let bytes = database.to_bytes().unwrap();
    let all = read_all(&bytes);
    find(rows_of(&all, PageType::Tracks), &long);
}

#[test]
fn a_database_that_would_confuse_a_player_is_refused() {
    let mut duplicate = demo();
    duplicate.tracks[1].id = 1;
    assert!(duplicate.to_bytes().unwrap_err().to_string().contains("share id 1"));

    let mut zero = demo();
    zero.tracks[0].id = 0;
    assert!(zero.to_bytes().unwrap_err().to_string().contains("no track"));

    let mut dangling = demo();
    dangling.playlists[2].track_ids.push(99);
    let message = dangling.to_bytes().unwrap_err().to_string();
    assert!(message.contains("track 99"), "{message}");

    let mut pathless = demo();
    pathless.tracks[0].file_path = String::new();
    assert!(pathless.to_bytes().unwrap_err().to_string().contains("no path"));
}
