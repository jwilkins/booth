//! What each player sees on a whole drive: drives this program writes, one
//! left in the state the field reports describe, and two rekordbox wrote.

mod common;

use booth_cli::audit::{self, Difference, Found, Problem, Side};
use booth_cli::cli::{ExportArgs, PlaylistSpec};
use booth_cli::commands;
use booth_cli::compat::Player;
use booth_cli::report::Collected;
use common::drive::*;

/// A passphrase, not the real one: what is checked is that the audit reads
/// both databases off a drive, not that a player would.
const KEY: &str = "a-key-that-is-not-the-real-one";

fn list(name: &str, tracks: &[&std::path::PathBuf]) -> PlaylistSpec {
    PlaylistSpec {
        name: name.to_string(),
        folder: String::new(),
        tracks: tracks.iter().map(|p| (*p).clone()).collect(),
    }
}

/// Rows a write returns, for the next write to carry.
type Rows = Vec<(std::path::PathBuf, booth_cli::export::pdb::Track)>;

/// Write a drive, carrying the rows an earlier write returned, as a second
/// sync does; returns this write's rows for the next.
fn write(
    drive: &std::path::Path,
    files: Vec<std::path::PathBuf>,
    lists: Vec<PlaylistSpec>,
    already: Rows,
) -> Rows {
    let mut args = ExportArgs { drive: Some(drive.to_path_buf()), ..args_for(files) };
    args.onelibrary_key = Some(KEY.to_string());
    args.playlists = lists;
    args.already = already;
    commands::export(&args, &Collected::new()).expect("the write failed")
}

fn problems(audit: &audit::Audit, player: Player) -> Vec<Problem> {
    audit.players.iter().find(|seen| seen.player == player).unwrap().problems.clone()
}

#[test]
fn a_drive_this_program_writes_is_the_same_drive_to_every_player_that_can_play_it() {
    let scratch = Scratch::new("audit-clean");
    let drive = scratch.path("USB");
    let a = write_song(&scratch, "a.flac");
    let b = write_song(&scratch, "b.flac");
    write(&drive, vec![a.clone(), b.clone()], vec![list("Saturday", &[&a, &b])], Vec::new());

    let audit = audit::audit(&drive, Some(KEY));
    assert!(audit.differences.is_empty(), "{:#?}", audit.lines());
    assert!(problems(&audit, Player::Cdj3000X).is_empty(), "{:#?}", audit.lines());
    assert!(problems(&audit, Player::Cdj3000).is_empty(), "{:#?}", audit.lines());
    assert!(problems(&audit, Player::Cdj2000Nxs2).is_empty(), "{:#?}", audit.lines());
    // FLAC is the one thing here the nexus line does not open, and the audit
    // says so on that player and no other.
    let nexus = problems(&audit, Player::Cdj2000Nexus);
    assert!(
        matches!(&nexus[..], [Problem::WontPlay { why, tracks }] if why.contains(".flac") && tracks.len() == 2),
        "{nexus:#?}"
    );
    for seen in &audit.players {
        assert_eq!((seen.tracks, seen.playlists), (2, 1), "{:?}", seen.player);
    }
}

// The field report: a tool rewrote export.pdb and left the exportLibrary.db an
// earlier write put there, so the CDJ-3000X browsed last week's drive with no
// error (M-Igashi/baken#208). Made here by putting last week's file back.
#[test]
fn a_onelibrary_from_last_week_is_said_as_the_tracks_the_newer_players_miss() {
    let scratch = Scratch::new("audit-stale");
    let drive = scratch.path("USB");
    let first = write_song(&scratch, "first.flac");
    let second = write_song(&scratch, "second.flac");

    let rows = write(&drive, vec![first.clone()], vec![list("Saturday", &[&first])], Vec::new());
    let last_week = std::fs::read(drive.join(ONELIBRARY)).unwrap();
    write(&drive, vec![second.clone()], vec![list("Saturday", &[&first, &second])], rows);
    std::fs::write(drive.join(ONELIBRARY), last_week).unwrap();

    let audit = audit::audit(&drive, Some(KEY));
    let short = audit
        .differences
        .iter()
        .find_map(|d| match d {
            Difference::PlaylistShort { short, playlist, tracks } => {
                Some((*short, playlist.clone(), tracks.len()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:#?}", audit.lines()));
    assert_eq!(short, (Side::OneLibrary, "Saturday".to_string(), 1));
    assert!(
        audit.differences.iter().any(|d| matches!(d, Difference::TracksOnlyOn { on: Side::Legacy, tracks } if tracks.len() == 1)),
        "{:#?}",
        audit.lines()
    );
    let said = audit.lines().join("\n");
    assert!(said.contains("“Saturday” on the CDJ-3000X"), "{said}");
}

#[test]
fn a_drive_with_neither_database_is_the_wrong_folder_rather_than_an_empty_report() {
    let scratch = Scratch::new("audit-nothing");
    let args = booth_cli::cli::AuditArgs { drive: scratch.0.clone(), onelibrary_key: None };
    let error = commands::audit(&args, &Collected::new()).expect_err("an empty folder audited");
    assert!(format!("{error:#}").contains("PIONEER"), "{error:#}");
}

#[test]
fn the_reader_and_rekordcrate_agree_on_every_track_and_entry_of_a_drive_this_writes() {
    let scratch = Scratch::new("audit-reader");
    let drive = scratch.path("USB");
    let files: Vec<_> = (0..4).map(|n| write_song(&scratch, &format!("track {n}.flac"))).collect();
    write(
        &drive,
        files.clone(),
        vec![
            list("Saturday", &[&files[2], &files[0]]),
            list("Sunday", &[&files[3], &files[1], &files[2]]),
        ],
        Vec::new(),
    );
    let Found::Read(ours) = audit::audit(&drive, Some(KEY)).legacy else { panic!("not read") };

    let theirs = legacy_tracks(&drive);
    assert_eq!(
        ours.tracks
            .iter()
            .map(|t| (t.id, t.title.clone(), t.file_path.clone(), t.analyze_path.clone()))
            .collect::<Vec<_>>(),
        theirs
            .iter()
            .map(|t| (t.id, t.title.clone(), t.file_path.clone(), t.analyze_path.clone()))
            .collect::<Vec<_>>()
    );
    let names: std::collections::HashMap<u32, String> =
        ours.nodes.iter().map(|n| (n.id, n.name.clone())).collect();
    let mut ours_entries: Vec<(String, u32, u32)> =
        ours.entries.iter().map(|e| (names[&e.playlist].clone(), e.index, e.track)).collect();
    ours_entries.sort();
    let mut theirs_entries: Vec<(String, u32)> = legacy_playlist_entries(&drive);
    theirs_entries.sort();
    let mut ours_by_name: Vec<(String, u32)> =
        ours_entries.into_iter().map(|(n, _, t)| (n, t)).collect();
    ours_by_name.sort();
    assert_eq!(ours_by_name, theirs_entries);
}

/// The two exports rekordbox wrote that `rekordcrate` keeps as fixtures, read
/// with the reader here. They live in the registry copy of a dependency, at a
/// path that is this machine's, so this is pointed at them:
///
/// ```sh
/// BOOTH_REKORDCRATE_DATA=~/.cargo/registry/src/*/rekordcrate-0.3.0/data \
///     cargo test -p booth-cli --test audit -- --ignored
/// ```
///
/// On `pdb/num_rows`, rekordcrate 0.3.0 reads 6637 playlist entries and this
/// reads 7440. This is right: every page's own eleven-bit row count matches
/// the rows its index marks present, all 94 playlists then run 1 to N with no
/// gaps, and every entry names a track that exists. The 803 rekordcrate does
/// not see are on the 26 pages that hold more than 255 rows.
///
/// That stick had been played, too, and is the one real history anywhere in
/// reach: a single list, `HISTORY 001`, of 73 tracks. Read here and by
/// rekordcrate alike, the list is named with a number and no date, its entries
/// run 1 to 73, and every one names a track on the stick.
#[test]
#[ignore = "needs rekordcrate's fixtures: BOOTH_REKORDCRATE_DATA"]
fn two_drives_rekordbox_wrote_are_read_in_full() {
    let data = std::path::PathBuf::from(std::env::var("BOOTH_REKORDCRATE_DATA").unwrap());

    let demo = audit::audit(&data.join("complete_export/demo_tracks"), Some(KEY));
    let Found::Read(library) = &demo.legacy else { panic!("{:?}", demo.legacy) };
    assert_eq!(
        library.tracks.iter().map(|t| (t.id, t.title.as_str())).collect::<Vec<_>>(),
        [(1, "Demo Track 1"), (2, "Demo Track 2")]
    );
    assert_eq!(demo.onelibrary, Found::Missing);
    assert_eq!(problems(&demo, Player::Cdj3000X), [Problem::NoLibrary]);

    let bytes = std::fs::read(data.join("pdb/num_rows/export.pdb")).unwrap();
    let library = booth_cli::rekordbox::pdb::read(&bytes).unwrap();
    assert_eq!(library.tracks.len(), 3886);
    assert_eq!(library.nodes.len(), 104);
    assert_eq!(library.entries.len(), 7440);
    let track = library.tracks.iter().find(|t| t.id == 3).unwrap();
    assert_eq!(
        track.file_path,
        "/Contents/DJ Plant Texture/1ØPILLS003 MASTER MP3s/1ØPILLS003_mastered_04.mp3"
    );
    let node = library.nodes.iter().find(|n| n.id == 48).unwrap();
    assert_eq!((node.parent, node.name.as_str()), (5, "2 - START BEATs 1"));

    let played = booth_cli::rekordbox::pdb::history(&bytes).unwrap();
    assert_eq!(
        played.lists.iter().map(|h| (h.id, h.name.as_str())).collect::<Vec<_>>(),
        [(1, "HISTORY 001")]
    );
    let positions: Vec<u32> = played.entries.iter().map(|e| e.index).collect();
    assert_eq!(positions, (1..=73).collect::<Vec<_>>());
    assert!(played.entries.iter().all(|e| e.playlist == 1));
    assert!(played.entries.iter().all(|e| library.tracks.iter().any(|t| t.id == e.track)));
    let first = library.tracks.iter().find(|t| t.id == played.entries[0].track).unwrap();
    assert_eq!(
        (first.file_name.as_str(), first.file_size),
        ("02 - Roland King - The Rowdy Swing.aiff", 55_720_044)
    );
    assert_eq!(theirs(&bytes, rekordcrate::pdb::PageType::HistoryPlaylists), 1);
    assert_eq!(theirs(&bytes, rekordcrate::pdb::PageType::HistoryEntries), 73);
}

/// How many rows rekordcrate reads in one table.
fn theirs(bytes: &[u8], table: rekordcrate::pdb::PageType) -> usize {
    use binrw::BinRead;
    let mut cursor = std::io::Cursor::new(bytes);
    let header = rekordcrate::pdb::Header::read(&mut cursor).unwrap();
    let table = header.tables.iter().find(|t| t.page_type == table).unwrap();
    header
        .read_pages(&mut cursor, binrw::Endian::Little, (&table.first_page, &table.last_page))
        .unwrap()
        .iter()
        .filter(|page| page.has_data())
        .flat_map(|page| page.row_groups.iter().flat_map(|group| group.present_rows()))
        .count()
}
