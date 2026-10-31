//! What each player will see on a drive.
//!
//! A drive carries two libraries. A CDJ-3000, and every player before it,
//! reads `export.pdb` and nothing else; a CDJ-3000X, and the rest of the
//! current line, reads `exportLibrary.db` and nothing else. Neither falls back
//! to the other. So the same stick can show a playlist in one booth and not in
//! the next, with no error on either deck — which is what DJs report after
//! converting a drive, after a tool rewrote one database and left the other,
//! and after the firmware 3.30 week (see `docs/rekordbox-replacement-spec.md`).
//!
//! This reads both off the drive and says, per generation of player, what it
//! will browse and what will not work on it: a playlist one database has and
//! the other does not, tracks a playlist lists in one and not the other, audio
//! that is not on the stick, files the player will not open, and the analysis
//! files it needs that are missing. It changes nothing. A drive this program
//! writes builds both databases from one description, so the next sync to it
//! is the way to bring the two back together; a drive written by anything
//! else is described, not repaired.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::compat::Player;
use crate::rekordbox::pdb::{Library, Node, Track};

/// Where the legacy database is on a drive.
pub const LEGACY_PATH: &str = "PIONEER/rekordbox/export.pdb";

/// How many tracks a finding names before it says how many more.
const NAMED: usize = 5;

/// Which of the two databases.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    /// `export.pdb`.
    Legacy,
    /// `exportLibrary.db`.
    OneLibrary,
}

impl Side {
    pub fn file(self) -> &'static str {
        match self {
            Side::Legacy => "export.pdb",
            Side::OneLibrary => "exportLibrary.db",
        }
    }

    /// Who reads it, for saying "shows on … but not on …".
    pub fn players(self) -> &'static str {
        match self {
            Side::Legacy => "the CDJ-3000 and older players",
            Side::OneLibrary => "the CDJ-3000X, XDJ-AZ, Opus Quad and the rest of the current line",
        }
    }

    pub fn other(self) -> Side {
        match self {
            Side::Legacy => Side::OneLibrary,
            Side::OneLibrary => Side::Legacy,
        }
    }

    fn of(player: Player) -> Side {
        match player.reads_onelibrary() {
            true => Side::OneLibrary,
            false => Side::Legacy,
        }
    }
}

/// What was found where one of the databases should be.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Missing,
    /// There, and would not open or read: why.
    Unreadable(String),
    Read(Library),
}

impl Found {
    fn library(&self) -> Option<&Library> {
        match self {
            Found::Read(library) => Some(library),
            _ => None,
        }
    }
}

/// A way the two databases disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Difference {
    /// A playlist that shows on the players reading `on`, and not on the
    /// others. Named by its folders and its own name.
    PlaylistOnlyOn { on: Side, playlist: String },
    /// A playlist both have, which on the players reading `short` lacks these
    /// tracks the other lists.
    PlaylistShort { short: Side, playlist: String, tracks: Vec<String> },
    /// A playlist both have with the same tracks in a different order.
    PlaylistOrder { playlist: String },
    /// Tracks in the collection the players reading `on` browse, and not in
    /// the other's.
    TracksOnlyOn { on: Side, tracks: Vec<String> },
}

/// Something that will not work on one generation of player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The database it reads is not on the drive: it shows no library at all.
    NoLibrary,
    /// The database it reads is there and could not be read.
    Unreadable(String),
    /// Tracks it lists whose audio is not on the drive.
    AudioMissing(Vec<String>),
    /// Tracks it lists and will not load, for one reason.
    WontPlay { why: String, tracks: Vec<String> },
    /// Tracks it lists without one of the analysis files it reads, and what it
    /// does without.
    NoAnalysis { file: &'static str, without: &'static str, tracks: Vec<String> },
}

/// The players a [`Player`] stands for, as a line of the audit names them.
pub fn generation(player: Player) -> &'static str {
    match player {
        Player::Cdj3000X => "CDJ-3000X, XDJ-AZ, Opus Quad and the rest of the current line",
        Player::Cdj3000 => "CDJ-3000",
        Player::Cdj2000Nxs2 => "CDJ-2000NXS2, XDJ-1000MK2, XDJ-700 and the nexus 2 line",
        Player::Cdj2000Nexus => "CDJ-2000NXS, CDJ-2000, CDJ-900NXS and the nexus line",
    }
}

/// What one generation of player sees.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub player: Player,
    pub side: Side,
    pub playlists: usize,
    pub tracks: usize,
    pub problems: Vec<Problem>,
}

/// Everything the audit found on one drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Audit {
    pub legacy: Found,
    pub onelibrary: Found,
    /// Empty when both were read and describe the same drive.
    pub differences: Vec<Difference>,
    /// Newest first.
    pub players: Vec<Seen>,
}

/// Read a mounted drive, or a folder laid out as one, and say what each
/// player will see on it.
///
/// `key` is the OneLibrary key, as [`crate::rekordbox::onelibrary_key`] takes
/// it: `None` for the environment's or this build's.
pub fn audit(drive: &Path, key: Option<&str>) -> Audit {
    let legacy = match std::fs::read(drive.join(LEGACY_PATH)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Missing,
        Err(e) => Found::Unreadable(format!("{e}")),
        Ok(bytes) => match crate::rekordbox::pdb::read(&bytes) {
            Ok(library) => Found::Read(library),
            Err(e) => Found::Unreadable(format!("{e:#}")),
        },
    };
    let onelibrary = match crate::rekordbox::onelibrary::find(drive) {
        None => Found::Missing,
        Some(path) => match crate::rekordbox::onelibrary_key(key) {
            None => Found::Unreadable("this build carries no key to open it with".into()),
            Some(key) => match crate::rekordbox::open(&path, &key)
                .and_then(|connection| crate::rekordbox::onelibrary::library(&connection))
            {
                Ok(library) => Found::Read(library),
                Err(e) => Found::Unreadable(format!("{e:#}")),
            },
        },
    };
    of(legacy, onelibrary, &|path: &str| drive.join(path.trim_start_matches('/')).exists())
}

/// The audit of two databases already read, with `there` saying whether a
/// file named by its path on the drive exists. Apart from [`audit`] so that it
/// can be asked about drives that are not on a disk.
pub fn of(legacy: Found, onelibrary: Found, there: &dyn Fn(&str) -> bool) -> Audit {
    let differences = match (legacy.library(), onelibrary.library()) {
        (Some(a), Some(b)) => compare(a, b),
        _ => Vec::new(),
    };
    let players = Player::ALL
        .iter()
        .map(|&player| {
            let side = Side::of(player);
            let found = match side {
                Side::Legacy => &legacy,
                Side::OneLibrary => &onelibrary,
            };
            seen(player, side, found, there)
        })
        .collect();
    Audit { legacy, onelibrary, differences, players }
}

fn seen(player: Player, side: Side, found: &Found, there: &dyn Fn(&str) -> bool) -> Seen {
    let library = match found {
        Found::Missing => return nothing(player, side, Problem::NoLibrary),
        Found::Unreadable(why) => return nothing(player, side, Problem::Unreadable(why.clone())),
        Found::Read(library) => library,
    };
    let mut problems = Vec::new();

    let missing: Vec<&Track> = library.tracks.iter().filter(|t| !there(&t.file_path)).collect();
    if !missing.is_empty() {
        problems.push(Problem::AudioMissing(names(&missing)));
    }
    // By reason, in the order the reasons are first met, so a crate of FLAC
    // on a nexus is one line and not three hundred.
    let mut wont: Vec<(String, Vec<String>)> = Vec::new();
    for track in &library.tracks {
        let extension = track.file_path.rsplit_once('.').map_or("", |(_, ext)| ext);
        let why = if !player.opens(extension) {
            Some(format!("it does not open .{} files", extension.to_ascii_lowercase()))
        } else if track.sample_rate > player.max_sample_rate() {
            Some(format!(
                "it is {} kHz and the {} stops at {} kHz",
                track.sample_rate / 1000,
                player.name(),
                player.max_sample_rate() / 1000
            ))
        } else {
            None
        };
        if let Some(why) = why {
            match wont.iter_mut().find(|(known, _)| *known == why) {
                Some((_, tracks)) => tracks.push(name(track)),
                None => wont.push((why, vec![name(track)])),
            }
        }
    }
    problems.extend(wont.into_iter().map(|(why, tracks)| Problem::WontPlay { why, tracks }));

    // Only the files this player reads: a 2009 deck has no use for the
    // three-band waveform, and saying it is missing there is noise. The `.EXT`
    // arrived with the nexus 2 line, which is everything from it up.
    let mut files = vec![("DAT", "no beat grid, cues or waveform")];
    if player <= Player::Cdj2000Nxs2 {
        files.push((
            "EXT",
            match player.shows_phrases() {
                true => "no colour waveform, no phrases, and no cue colours or names",
                false => "no colour waveform, and no cue colours or names",
            },
        ));
    }
    if player.shows_three_band() {
        files.push(("2EX", "no three-band waveform"));
    }
    for (file, without) in files {
        let lacking: Vec<&Track> = library
            .tracks
            .iter()
            .filter(|t| t.analyze_path.is_empty() || !there(&analysis(&t.analyze_path, file)))
            .collect();
        if !lacking.is_empty() {
            problems.push(Problem::NoAnalysis { file, without, tracks: names(&lacking) });
        }
    }

    Seen {
        player,
        side,
        playlists: library.nodes.iter().filter(|n| !n.folder).count(),
        tracks: library.tracks.len(),
        problems,
    }
}

fn nothing(player: Player, side: Side, problem: Problem) -> Seen {
    Seen { player, side, playlists: 0, tracks: 0, problems: vec![problem] }
}

/// A `.DAT`'s path with its extension swapped for `file`'s.
fn analysis(dat: &str, file: &str) -> String {
    match dat.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.{file}"),
        None => format!("{dat}.{file}"),
    }
}

/// How a track is named in a finding: its title, or its file where it has none.
fn name(track: &Track) -> String {
    match track.title.trim() {
        "" => track.file_path.rsplit('/').next().unwrap_or(&track.file_path).to_string(),
        title => title.to_string(),
    }
}

fn names(tracks: &[&Track]) -> Vec<String> {
    tracks.iter().map(|t| name(t)).collect()
}

/// A track's identity across the two databases: where its audio is. The ids
/// are each database's own and need not agree between them.
fn key(track: &Track) -> String {
    track.file_path.trim_start_matches('/').to_string()
}

/// Every playlist by where it is in the tree — `Saturday / Warm-up` — with its
/// tracks in order, by [`key`]. A smart playlist's tracks are a query the
/// player runs, so it is listed with none.
fn playlists(library: &Library) -> BTreeMap<String, (bool, Vec<String>)> {
    let by_id: BTreeMap<u32, &Node> = library.nodes.iter().map(|n| (n.id, n)).collect();
    let tracks: BTreeMap<u32, String> = library.tracks.iter().map(|t| (t.id, key(t))).collect();
    let path = |node: &Node| {
        let mut names = vec![node.name.clone()];
        let mut parent = node.parent;
        // Bounded by the number of nodes, so a loop in the tree ends.
        for _ in 0..by_id.len() {
            let Some(up) = by_id.get(&parent) else { break };
            names.push(up.name.clone());
            parent = up.parent;
        }
        names.reverse();
        names.join(" / ")
    };
    let mut out = BTreeMap::new();
    for node in library.nodes.iter().filter(|n| !n.folder) {
        let mut entries: Vec<_> =
            library.entries.iter().filter(|e| e.playlist == node.id).collect();
        entries.sort_by_key(|e| e.index);
        let listed = match node.smart {
            true => Vec::new(),
            false => entries.iter().filter_map(|e| tracks.get(&e.track).cloned()).collect(),
        };
        out.entry(path(node)).or_insert((node.smart, listed));
    }
    out
}

fn compare(legacy: &Library, onelibrary: &Library) -> Vec<Difference> {
    let mut differences = Vec::new();
    let names: BTreeMap<String, String> =
        legacy.tracks.iter().chain(&onelibrary.tracks).map(|t| (key(t), name(t))).collect();
    let titled = |keys: &mut dyn Iterator<Item = &String>| -> Vec<String> {
        keys.map(|k| names.get(k).cloned().unwrap_or_else(|| k.clone())).collect()
    };

    let a = playlists(legacy);
    let b = playlists(onelibrary);
    for (side, mine, theirs) in [(Side::Legacy, &a, &b), (Side::OneLibrary, &b, &a)] {
        for playlist in mine.keys().filter(|name| !theirs.contains_key(*name)) {
            differences.push(Difference::PlaylistOnlyOn { on: side, playlist: playlist.clone() });
        }
    }
    for (playlist, (smart, listed)) in &a {
        let Some((other_smart, other)) = b.get(playlist) else { continue };
        if *smart || *other_smart {
            continue;
        }
        let here: BTreeSet<&String> = listed.iter().collect();
        let there: BTreeSet<&String> = other.iter().collect();
        for (short, has, lacks) in
            [(Side::Legacy, &there, &here), (Side::OneLibrary, &here, &there)]
        {
            let missing = titled(&mut has.difference(lacks).copied());
            if !missing.is_empty() {
                differences.push(Difference::PlaylistShort {
                    short,
                    playlist: playlist.clone(),
                    tracks: missing,
                });
            }
        }
        if here == there && listed != other {
            differences.push(Difference::PlaylistOrder { playlist: playlist.clone() });
        }
    }

    let in_legacy: BTreeSet<String> = legacy.tracks.iter().map(key).collect();
    let in_onelibrary: BTreeSet<String> = onelibrary.tracks.iter().map(key).collect();
    for (side, mine, theirs) in
        [(Side::Legacy, &in_legacy, &in_onelibrary), (Side::OneLibrary, &in_onelibrary, &in_legacy)]
    {
        let only = titled(&mut mine.difference(theirs));
        if !only.is_empty() {
            differences.push(Difference::TracksOnlyOn { on: side, tracks: only });
        }
    }
    differences
}

/// A list of tracks for a line, the first few by name.
fn listed(tracks: &[String]) -> String {
    let named: Vec<String> =
        tracks.iter().take(NAMED).map(|t| format!("\u{201c}{t}\u{201d}")).collect();
    match tracks.len().saturating_sub(NAMED) {
        0 => named.join(", "),
        more => format!("{} and {more} more", named.join(", ")),
    }
}

fn count(n: usize, one: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    }
}

impl Difference {
    pub fn says(&self) -> String {
        match self {
            Difference::PlaylistOnlyOn { on, playlist } => format!(
                "\u{201c}{playlist}\u{201d} shows on {} but not on {}: it is in {} and not in {}",
                on.players(),
                on.other().players(),
                on.file(),
                on.other().file()
            ),
            Difference::PlaylistShort { short, playlist, tracks } => format!(
                "\u{201c}{playlist}\u{201d} on {} is missing {} that {} show: {}",
                short.players(),
                count(tracks.len(), "track"),
                short.other().players(),
                listed(tracks)
            ),
            Difference::PlaylistOrder { playlist } => format!(
                "\u{201c}{playlist}\u{201d} has the same tracks in a different order in the two databases"
            ),
            Difference::TracksOnlyOn { on, tracks } => format!(
                "{} {} only in {}, so only {} can find them: {}",
                count(tracks.len(), "track"),
                if tracks.len() == 1 { "is" } else { "are" },
                on.file(),
                on.players(),
                listed(tracks)
            ),
        }
    }
}

impl Problem {
    pub fn says(&self, side: Side) -> String {
        match self {
            Problem::NoLibrary => {
                format!("there is no {} on the drive, so it shows no library at all", side.file())
            }
            Problem::Unreadable(why) => format!("its {} would not read: {why}", side.file()),
            Problem::AudioMissing(tracks) => format!(
                "{} {} listed and not on the drive: {}",
                count(tracks.len(), "track"),
                if tracks.len() == 1 { "is" } else { "are" },
                listed(tracks)
            ),
            Problem::WontPlay { why, tracks } => format!(
                "{} {} and will not load, because {why}: {}",
                count(tracks.len(), "track"),
                if tracks.len() == 1 { "browses" } else { "browse" },
                listed(tracks)
            ),
            Problem::NoAnalysis { file, without, tracks } => format!(
                "{} {} no .{file}, so {without}: {}",
                count(tracks.len(), "track"),
                if tracks.len() == 1 { "has" } else { "have" },
                listed(tracks)
            ),
        }
    }
}

impl Audit {
    /// Whether every player sees the same drive and nothing on it fails.
    pub fn is_clean(&self) -> bool {
        self.differences.is_empty() && self.players.iter().all(|seen| seen.problems.is_empty())
    }

    /// The audit as lines of text, for the terminal and for the window: the
    /// two databases, where they disagree, and then each player.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (side, found) in [(Side::Legacy, &self.legacy), (Side::OneLibrary, &self.onelibrary)] {
            out.push(match found {
                Found::Missing => format!("{}: not on the drive", side.file()),
                Found::Unreadable(why) => format!("{}: would not read: {why}", side.file()),
                Found::Read(library) => format!(
                    "{}: {}, {}",
                    side.file(),
                    count(library.tracks.len(), "track"),
                    count(library.nodes.iter().filter(|n| !n.folder).count(), "playlist")
                ),
            });
        }
        match (self.legacy.library(), self.onelibrary.library(), self.differences.is_empty()) {
            (Some(_), Some(_), true) => {
                out.push("Both databases describe the same tracks and playlists.".into())
            }
            (Some(_), Some(_), false) => {
                out.push("The two databases disagree:".into());
                out.extend(self.differences.iter().map(|d| format!("  {}", d.says())));
            }
            _ => {}
        }
        for seen in &self.players {
            let reads = match seen.problems.first() {
                Some(Problem::NoLibrary | Problem::Unreadable(_)) => {
                    format!("{} (reads {})", generation(seen.player), seen.side.file())
                }
                _ => format!(
                    "{} (reads {}): {}, {}",
                    generation(seen.player),
                    seen.side.file(),
                    count(seen.tracks, "track"),
                    count(seen.playlists, "playlist")
                ),
            };
            out.push(reads);
            match seen.problems.is_empty() {
                true => out.push("  everything on it loads".into()),
                false => {
                    out.extend(seen.problems.iter().map(|p| format!("  {}", p.says(seen.side))))
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rekordbox::pdb::Entry;

    fn track(id: u32, title: &str, file: &str) -> Track {
        Track {
            id,
            title: title.into(),
            file_path: format!("/Contents/{file}"),
            analyze_path: format!("/PIONEER/USBANLZ/P00{id}/ANLZ0000.DAT"),
            sample_rate: 44_100,
            ..Track::default()
        }
    }

    fn list(id: u32, name: &str, parent: u32, tracks: &[u32]) -> (Node, Vec<Entry>) {
        let node = Node { id, parent, name: name.into(), ..Node::default() };
        let entries = tracks
            .iter()
            .enumerate()
            .map(|(at, &track)| Entry { playlist: id, index: at as u32 + 1, track })
            .collect();
        (node, entries)
    }

    /// Three tracks, a folder "Saturday" holding "Warm-up" of all three.
    fn library() -> Library {
        let (warm_up, entries) = list(2, "Warm-up", 1, &[1, 2, 3]);
        Library {
            tracks: vec![
                track(1, "One", "a.mp3"),
                track(2, "Two", "b.aiff"),
                track(3, "Three", "c.wav"),
            ],
            nodes: vec![
                Node { id: 1, folder: true, name: "Saturday".into(), ..Node::default() },
                warm_up,
            ],
            entries,
        }
    }

    fn everything_there(_: &str) -> bool {
        true
    }

    fn of_these(legacy: Library, onelibrary: Library) -> Audit {
        of(Found::Read(legacy), Found::Read(onelibrary), &everything_there)
    }

    #[test]
    fn a_drive_whose_two_databases_agree_has_nothing_to_say_and_every_player_sees_it_all() {
        let audit = of_these(library(), library());
        assert!(audit.is_clean(), "{:#?}", audit.lines());
        assert_eq!(audit.players.len(), Player::ALL.len());
        for seen in &audit.players {
            assert_eq!((seen.tracks, seen.playlists), (3, 1), "{:?}", seen.player);
        }
        assert!(audit
            .lines()
            .iter()
            .any(|l| l == "Both databases describe the same tracks and playlists."));
    }

    #[test]
    fn a_playlist_in_only_one_database_shows_on_those_players_and_not_the_others() {
        // The report: a playlist on the CDJ-3000 that the CDJ-3000X never shows.
        let mut legacy = library();
        let (peak, entries) = list(3, "Peak", 0, &[2]);
        legacy.nodes.push(peak);
        legacy.entries.extend(entries);
        let audit = of_these(legacy, library());
        assert_eq!(
            audit.differences,
            [Difference::PlaylistOnlyOn { on: Side::Legacy, playlist: "Peak".into() }]
        );
        let said = audit.differences[0].says();
        assert!(
            said.contains("shows on the CDJ-3000 and older players but not on the CDJ-3000X"),
            "{said}"
        );
    }

    #[test]
    fn a_playlist_short_of_tracks_on_one_side_names_them_and_says_which_players() {
        let mut onelibrary = library();
        onelibrary.entries.retain(|e| e.track != 3);
        let audit = of_these(library(), onelibrary);
        assert_eq!(
            audit.differences,
            [Difference::PlaylistShort {
                short: Side::OneLibrary,
                playlist: "Saturday / Warm-up".into(),
                tracks: vec!["Three".into()],
            }]
        );
    }

    #[test]
    fn the_same_tracks_in_another_order_is_said_as_an_order() {
        let mut onelibrary = library();
        for entry in &mut onelibrary.entries {
            entry.index = 4 - entry.index;
        }
        let audit = of_these(library(), onelibrary);
        assert_eq!(
            audit.differences,
            [Difference::PlaylistOrder { playlist: "Saturday / Warm-up".into() }]
        );
    }

    #[test]
    fn tracks_are_the_same_track_by_where_their_audio_is_not_by_their_id() {
        // Two writers numbering the same drive differently is not a difference.
        let mut onelibrary = library();
        for track in &mut onelibrary.tracks {
            track.id += 100;
        }
        for entry in &mut onelibrary.entries {
            entry.track += 100;
        }
        assert!(of_these(library(), onelibrary).differences.is_empty());
    }

    #[test]
    fn a_missing_database_means_those_players_show_no_library_and_nothing_is_compared() {
        let audit = of(Found::Read(library()), Found::Missing, &everything_there);
        assert!(audit.differences.is_empty());
        let x = audit.players.iter().find(|s| s.player == Player::Cdj3000X).unwrap();
        assert_eq!(x.problems, [Problem::NoLibrary]);
        let said = x.problems[0].says(x.side);
        assert_eq!(
            said,
            "there is no exportLibrary.db on the drive, so it shows no library at all"
        );
        let older = audit.players.iter().find(|s| s.player == Player::Cdj3000).unwrap();
        assert!(older.problems.is_empty(), "{:?}", older.problems);
    }

    #[test]
    fn a_track_a_player_will_not_open_is_said_on_that_player_only() {
        // FLAC on a nexus; 96 kHz on a nexus 2.
        let mut both = library();
        both.tracks[1].file_path = "/Contents/b.flac".into();
        both.tracks[1].sample_rate = 96_000;
        let audit = of_these(both.clone(), both);
        let problems = |player: Player| {
            audit.players.iter().find(|s| s.player == player).unwrap().problems.clone()
        };
        assert!(problems(Player::Cdj3000).is_empty());
        assert_eq!(
            problems(Player::Cdj2000Nxs2),
            [Problem::WontPlay {
                why: "it is 96 kHz and the CDJ-2000NXS2 stops at 48 kHz".into(),
                tracks: vec!["Two".into()],
            }]
        );
        assert_eq!(
            problems(Player::Cdj2000Nexus),
            [Problem::WontPlay {
                why: "it does not open .flac files".into(),
                tracks: vec!["Two".into()],
            }]
        );
    }

    #[test]
    fn tracks_a_player_will_not_open_for_the_same_reason_are_one_finding() {
        let mut both = library();
        for track in &mut both.tracks {
            let stem = track.file_path.rsplit_once('.').unwrap().0.to_string();
            track.file_path = format!("{stem}.flac");
        }
        let audit = of_these(both.clone(), both);
        let nexus = &audit.players.iter().find(|s| s.player == Player::Cdj2000Nexus).unwrap();
        assert_eq!(
            nexus.problems,
            [Problem::WontPlay {
                why: "it does not open .flac files".into(),
                tracks: vec!["One".into(), "Two".into(), "Three".into()],
            }]
        );
        assert_eq!(
            nexus.problems[0].says(Side::Legacy),
            "3 tracks browse and will not load, because it does not open .flac files: \
             \u{201c}One\u{201d}, \u{201c}Two\u{201d}, \u{201c}Three\u{201d}"
        );
    }

    #[test]
    fn a_missing_analysis_file_is_said_on_the_players_that_read_it() {
        // No .2EX anywhere: the CDJ-3000s lose the three-band waveform, and the
        // nexus 2 and older never had it to lose.
        let there = |path: &str| !path.ends_with(".2EX");
        let audit = of(Found::Read(library()), Found::Read(library()), &there);
        for seen in &audit.players {
            let lacking =
                seen.problems.iter().any(|p| matches!(p, Problem::NoAnalysis { file: "2EX", .. }));
            assert_eq!(lacking, seen.player.shows_three_band(), "{:?}", seen.player);
        }
    }

    #[test]
    fn audio_that_is_not_on_the_drive_is_said_by_title() {
        let there = |path: &str| !path.ends_with("b.aiff");
        let audit = of(Found::Read(library()), Found::Read(library()), &there);
        let seen = &audit.players[0];
        assert_eq!(seen.problems, [Problem::AudioMissing(vec!["Two".into()])]);
    }

    #[test]
    fn a_long_list_of_tracks_names_five_and_counts_the_rest() {
        let tracks: Vec<String> = (1..=8).map(|n| format!("T{n}")).collect();
        assert_eq!(
            listed(&tracks),
            "\u{201c}T1\u{201d}, \u{201c}T2\u{201d}, \u{201c}T3\u{201d}, \u{201c}T4\u{201d}, \u{201c}T5\u{201d} and 3 more"
        );
    }

    #[test]
    fn a_smart_playlists_tracks_are_not_compared() {
        let mut onelibrary = library();
        onelibrary.nodes[1].smart = true;
        onelibrary.entries.clear();
        assert!(of_these(library(), onelibrary).differences.is_empty());
    }
}
