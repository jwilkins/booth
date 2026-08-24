//! The collection: what is known about each track, and where that is kept.
//!
//! A track here is a record about a file, never the file itself. Nothing in
//! this module writes audio, moves anything, or changes a tag — the collection
//! describes what is on disk, and every destructive act belongs to a job the
//! user asked for. That separation is what makes it safe for the library to be
//! rebuilt from a scan at any point.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What a row is: a track, or one of the stem renders that hangs under it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[default]
    Track,
    /// The vocal stem on its own.
    Acapella,
    /// Everything but the vocal.
    Instrumental,
}

impl Role {
    /// How the row is labelled in the browser.
    pub fn suffix(self) -> &'static str {
        match self {
            Role::Track => "",
            Role::Acapella => " (acapella)",
            Role::Instrumental => " (instrumental)",
        }
    }

    /// Which of the separator's outputs this row is made of.
    pub fn stems(self) -> &'static str {
        match self {
            Role::Track => "kit",
            Role::Acapella => "vocals",
            Role::Instrumental => "melody+drums",
        }
    }
}

/// Which stem files exist for a track.
///
/// The three are the ones the separator actually produces. The spec page drew
/// four, borrowing a different model's names; there is no sense in a library
/// that reports a `bass` stem nothing ever writes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StemKit {
    pub vocals: Option<PathBuf>,
    pub melody: Option<PathBuf>,
    pub drums: Option<PathBuf>,
}

impl StemKit {
    pub fn is_empty(&self) -> bool {
        self.vocals.is_none() && self.melody.is_none() && self.drums.is_none()
    }

    /// Whether all three are rendered, which is what a companion row needs.
    pub fn is_complete(&self) -> bool {
        self.vocals.is_some() && self.melody.is_some() && self.drums.is_some()
    }

    pub fn each(&self) -> [(&'static str, Option<&PathBuf>); 3] {
        [
            ("vocals", self.vocals.as_ref()),
            ("melody", self.melody.as_ref()),
            ("drums", self.drums.as_ref()),
        ]
    }
}

/// One stretch of a track, as the phrase strip draws it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    pub start_ms: u32,
    pub end_ms: u32,
    /// `intro`, `build`, `break`, `drop` or `outro`.
    pub kind: String,
}

/// A cue point, as the waveform draws it and the export writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueMark {
    /// 0 for a memory cue, 1–8 for hot cues A–H.
    pub letter: u8,
    pub time_ms: u32,
    pub label: String,
    pub color: [u8; 3],
}

impl CueMark {
    /// A, B, C… for a hot cue; a bullet for a memory cue.
    pub fn name(&self) -> String {
        match self.letter {
            0 => "•".to_string(),
            n => char::from(b'A' + (n - 1).min(7)).to_string(),
        }
    }
}

/// Everything the collection knows about one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u32,
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub year: Option<u32>,
    pub duration_secs: f64,
    /// The file extension, lower-cased.
    pub format: String,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Set for the float WAVs that look fine on a laptop and fail in a booth.
    pub float_samples: bool,
    pub bytes: u64,

    pub bpm: f64,
    /// How clearly the tempo stood out. Under about 2 the tracker was guessing.
    pub grid_confidence: f32,
    pub has_grid: bool,
    /// Camelot notation, e.g. `8A`, or empty when no key was found.
    pub key: String,
    pub key_confidence: f32,
    /// 1 to 5, from how much is being played. See [`energy_from`].
    pub energy: u8,
    pub beats: usize,
    pub phrases: Vec<Phrase>,
    pub cues: Vec<CueMark>,
    pub loudness_lufs: Option<f64>,
    pub peak_dbtp: Option<f64>,

    pub tags: Vec<String>,
    /// Seconds since the Unix epoch.
    pub added: u64,
    pub last_played: Option<u64>,
    pub play_count: u32,

    pub stems: StemKit,
    pub role: Role,
    /// For a stem companion, the track it belongs to.
    pub parent: Option<u32>,
    /// Whether the analysers have run. An unanalysed track has no grid, no key
    /// and no cues, which is different from having been analysed and found to
    /// have none.
    pub analyzed: bool,
}

impl Track {
    /// A record with nothing known about it yet, which is what a freshly
    /// scanned file is.
    pub fn placeholder(id: u32) -> Self {
        Self {
            id,
            path: PathBuf::new(),
            artist: String::new(),
            title: String::new(),
            album: String::new(),
            year: None,
            duration_secs: 0.0,
            format: String::new(),
            bitrate_kbps: 0,
            sample_rate: 0,
            channels: 0,
            float_samples: false,
            bytes: 0,
            bpm: 0.0,
            grid_confidence: 0.0,
            has_grid: false,
            key: String::new(),
            key_confidence: 0.0,
            energy: 0,
            beats: 0,
            phrases: Vec::new(),
            cues: Vec::new(),
            loudness_lufs: None,
            peak_dbtp: None,
            tags: Vec::new(),
            added: now(),
            last_played: None,
            play_count: 0,
            stems: StemKit::default(),
            role: Role::Track,
            parent: None,
            analyzed: false,
        }
    }

    /// The title as the browser shows it, with the stem suffix for a companion.
    pub fn display_title(&self) -> String {
        format!("{}{}", self.title, self.role.suffix())
    }

    /// Whether this track still needs work before it can go on a drive.
    pub fn unprepared(&self) -> bool {
        !self.analyzed || !self.has_grid
    }

    /// Whether something about it is wrong rather than merely unfinished.
    ///
    /// These are the states that look fine in a file browser and fail in a
    /// booth, which is why they get their own count in the sidebar.
    pub fn needs_attention(&self) -> Option<&'static str> {
        if self.float_samples {
            return Some("32-bit float, which a player will not open");
        }
        if self.sample_rate > 96_000 {
            return Some("above the 96 kHz a player will accept");
        }
        if self.analyzed && !self.has_grid {
            return Some("no beat grid could be found");
        }
        if self.analyzed && self.grid_confidence > 0.0 && self.grid_confidence < 2.0 {
            return Some("the tempo is a guess");
        }
        None
    }

    pub fn duration_text(&self) -> String {
        let total = self.duration_secs.round() as u64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// Turn a mean onset strength into the five-bar meter.
///
/// It is a rank, not a measurement: what it has to do is sort a crate so that
/// the tools are at one end and the peak-time records at the other. The
/// thresholds come from the same onset envelope the phrase detector uses, and
/// are deliberately coarse — a five-bar meter that claimed more precision than
/// this would be inventing it.
pub fn energy_from(intensity: f32) -> u8 {
    match intensity {
        i if i < 0.6 => 1,
        i if i < 1.2 => 2,
        i if i < 2.0 => 3,
        i if i < 3.2 => 4,
        _ => 5,
    }
}

/// A playlist, and the folder it sits in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    /// The folder shown above it in the sidebar, e.g. a date. Empty for a
    /// playlist that sits at the top level.
    pub folder: String,
    pub tracks: Vec<u32>,
}

/// A query the user kept. There is no separate smart-playlist concept: this is
/// it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub text: String,
}

/// One track as it was written to a drive.
///
/// The fingerprint is what makes an update distinguishable from an addition
/// without re-reading the drive: it summarises the prep the player will see, so
/// a moved cue marks the track for rewriting and a play count does not.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub id: u32,
    pub prep: u64,
}

/// A drive the collection has written to, and what was on it when it did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Drive {
    pub label: String,
    pub path: PathBuf,
    /// Whether `path` names an image file rather than a mounted volume.
    pub is_image: bool,
    /// The playlist that was written to it.
    pub playlist: String,
    /// What was on it after the last sync.
    pub written: Vec<Written>,
    /// Whether the stem companions went on too.
    pub with_stems: bool,
    pub bytes: u64,
    pub last_sync: Option<u64>,
}

impl Drive {
    pub fn track_ids(&self) -> Vec<u32> {
        self.written.iter().map(|w| w.id).collect()
    }
}

/// The whole collection.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub saved: Vec<SavedQuery>,
    pub drives: Vec<Drive>,
    next_id: u32,
}

impl Library {
    /// The queries a new collection starts with. They are the ones from the
    /// spec, and they double as documentation of what the bar understands.
    pub fn starter_queries() -> Vec<SavedQuery> {
        [
            ("Unprepared", "missing:grid"),
            ("No stems yet", "missing:stems"),
            ("Never played", "played:never"),
            ("New instrumentals", "added:<14d -tag:vocal"),
            ("Won't survive the booth", "format:mp3 bitrate:<256"),
        ]
        .into_iter()
        .map(|(name, text)| SavedQuery { name: name.into(), text: text.into() })
        .collect()
    }

    pub fn new() -> Self {
        Self { saved: Self::starter_queries(), ..Self::default() }
    }

    /// Add a file, or return the existing record if it is already known.
    ///
    /// Paths are the identity: re-scanning a folder must not double every track
    /// in it, and a rescan is the ordinary way to pick up new files.
    pub fn add(&mut self, path: &Path) -> u32 {
        if let Some(existing) = self.tracks.iter().find(|t| t.path == path) {
            return existing.id;
        }
        self.next_id += 1;
        let id = self.next_id;
        let mut track = Track::placeholder(id);
        track.path = path.to_path_buf();
        track.title =
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        track.format =
            path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.tracks.push(track);
        id
    }

    pub fn get(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Remove a track and everything that refers to it.
    pub fn remove(&mut self, id: u32) {
        self.tracks.retain(|t| t.id != id && t.parent != Some(id));
        for playlist in &mut self.playlists {
            playlist.tracks.retain(|t| *t != id);
        }
        for drive in &mut self.drives {
            drive.written.retain(|w| w.id != id);
        }
    }

    /// The stem companion rows, which are derived rather than stored.
    ///
    /// A companion exists exactly when the files it is made of do, so there is
    /// no way for the browser to promise an acapella that is not on disk.
    pub fn companions(&self, track: &Track) -> Vec<Track> {
        if track.role != Role::Track || !track.stems.is_complete() {
            return Vec::new();
        }
        [Role::Acapella, Role::Instrumental]
            .into_iter()
            .map(|role| Track {
                id: companion_id(track.id, role),
                role,
                parent: Some(track.id),
                stems: StemKit::default(),
                ..track.clone()
            })
            .collect()
    }

    /// Track ids by playlist name, for the query context.
    pub fn playlist_index(&self) -> HashMap<String, Vec<u32>> {
        self.playlists.iter().map(|p| (p.name.clone(), p.tracks.clone())).collect()
    }

    /// Track ids by drive label, for the query context.
    pub fn drive_index(&self) -> HashMap<String, Vec<u32>> {
        self.drives.iter().map(|d| (d.label.clone(), d.track_ids())).collect()
    }

    /// The folders of the playlist tree, in the order they were first seen,
    /// each with its playlists.
    pub fn playlist_tree(&self) -> Vec<(String, Vec<&Playlist>)> {
        let mut tree: Vec<(String, Vec<&Playlist>)> = Vec::new();
        for playlist in &self.playlists {
            match tree.iter_mut().find(|(folder, _)| *folder == playlist.folder) {
                Some((_, list)) => list.push(playlist),
                None => tree.push((playlist.folder.clone(), vec![playlist])),
            }
        }
        tree
    }

    pub fn unprepared_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.unprepared()).count()
    }

    pub fn attention_count(&self) -> usize {
        self.tracks.iter().filter(|t| t.needs_attention().is_some()).count()
    }

    // -- persistence -------------------------------------------------------

    /// Where the collection is kept.
    pub fn default_path() -> PathBuf {
        data_dir().join("library.json")
    }

    /// Read the collection, or start a new one if there is not one yet.
    ///
    /// A file that exists but will not parse is an error rather than a fresh
    /// start: silently replacing a library with an empty one is the worst thing
    /// this function could do.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let text = std::fs::read_to_string(path)?;
        let library: Library = serde_json::from_str(&text)?;
        Ok(library)
    }

    /// Write the collection, via a temporary file so that an interrupted save
    /// leaves the previous one intact.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.new");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

/// A stable id for a derived companion row.
///
/// The high bits are free because real ids come from a counter that starts at
/// one, so a companion can have an id of its own — which the browser needs for
/// selection — without ever colliding with a track.
pub fn companion_id(parent: u32, role: Role) -> u32 {
    let tag = match role {
        Role::Track => 0,
        Role::Acapella => 1,
        Role::Instrumental => 2,
    };
    0x8000_0000 | (parent << 2) | tag
}

/// Where a track's three-band picture is cached.
///
/// Not in the collection file: it is 3,600 bytes a track, and a library of a
/// few thousand would turn every save into a several-megabyte rewrite. Not
/// recomputed on demand either — the spec's promise is that arrow-keying down a
/// crate moves the waveform, and re-analysing a track takes a second or two,
/// which is a second or two per row.
pub fn waveform_path(id: u32) -> PathBuf {
    data_dir().join("waveforms").join(format!("{id:08}.bands"))
}

/// Keep a track's picture for next time.
pub fn cache_waveform(id: u32, bands: &[u8]) -> std::io::Result<()> {
    let path = waveform_path(id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bands)
}

/// Read a cached picture, if there is one.
pub fn cached_waveform(id: u32) -> Option<Vec<u8>> {
    std::fs::read(waveform_path(id)).ok().filter(|bands| !bands.is_empty())
}

/// Where this program keeps its data.
pub fn data_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("BOOTH_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        return home.join("Library").join("Application Support").join("Booth");
    }
    match std::env::var_os("XDG_DATA_HOME") {
        Some(xdg) => PathBuf::from(xdg).join("booth"),
        None => home.join(".local").join("share").join("booth"),
    }
}

/// Now, in seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("booth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn adding_the_same_file_twice_adds_it_once() {
        let mut library = Library::new();
        let first = library.add(Path::new("/music/track.flac"));
        let second = library.add(Path::new("/music/track.flac"));
        assert_eq!(first, second, "a rescan must not double the collection");
        assert_eq!(library.tracks.len(), 1);
    }

    #[test]
    fn a_new_track_is_named_from_its_file_until_it_is_read() {
        let mut library = Library::new();
        let id = library.add(Path::new("/music/Peverelist - Roll.flac"));
        let track = library.get(id).unwrap();
        assert_eq!(track.title, "Peverelist - Roll");
        assert_eq!(track.format, "flac");
        assert!(!track.analyzed);
        assert!(track.unprepared());
    }

    #[test]
    fn removing_a_track_removes_every_reference_to_it() {
        let mut library = Library::new();
        let id = library.add(Path::new("/music/a.flac"));
        let other = library.add(Path::new("/music/b.flac"));
        library.playlists.push(Playlist {
            name: "peak".into(),
            folder: "Sat".into(),
            tracks: vec![id, other],
        });
        library.drives.push(Drive {
            label: "USB".into(),
            written: vec![Written { id, prep: 0 }, Written { id: other, prep: 0 }],
            ..Drive::default()
        });

        library.remove(id);
        assert_eq!(library.playlists[0].tracks, vec![other]);
        assert_eq!(library.drives[0].track_ids(), vec![other]);
    }

    #[test]
    fn companions_exist_only_when_all_three_stems_do() {
        let mut library = Library::new();
        let id = library.add(Path::new("/music/a.flac"));

        let track = library.get(id).unwrap().clone();
        assert!(library.companions(&track).is_empty(), "nothing is rendered yet");

        let mut track = track;
        track.stems.vocals = Some("/stems/a-vocals.wav".into());
        assert!(library.companions(&track).is_empty(), "a partial kit is not an acapella");

        track.stems.melody = Some("/stems/a-melody.wav".into());
        track.stems.drums = Some("/stems/a-drums.wav".into());
        let companions = library.companions(&track);
        assert_eq!(companions.len(), 2);
        assert_eq!(companions[0].role, Role::Acapella);
        assert_eq!(companions[1].role, Role::Instrumental);
        assert!(companions[0].display_title().ends_with("(acapella)"));
        // A companion carries the parent's grid, because it is the same audio.
        assert_eq!(companions[0].bpm, track.bpm);
        assert_eq!(companions[0].parent, Some(id));
    }

    #[test]
    fn a_companion_id_never_collides_with_a_track() {
        let mut library = Library::new();
        let ids: Vec<u32> =
            (0..200).map(|i| library.add(Path::new(&format!("/m/{i}.flac")))).collect();
        let companions: Vec<u32> = ids
            .iter()
            .flat_map(|id| {
                [companion_id(*id, Role::Acapella), companion_id(*id, Role::Instrumental)]
            })
            .collect();

        for companion in &companions {
            assert!(!ids.contains(companion), "{companion} is also a track id");
        }
        let mut sorted = companions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), companions.len(), "two companions share an id");
    }

    #[test]
    fn the_playlist_tree_keeps_the_order_folders_were_made_in() {
        let mut library = Library::new();
        for (folder, name) in [("Sat 14/9", "warm"), ("Digging", "promos"), ("Sat 14/9", "peak")] {
            library.playlists.push(Playlist {
                name: name.into(),
                folder: folder.into(),
                tracks: Vec::new(),
            });
        }

        let tree = library.playlist_tree();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].0, "Sat 14/9");
        assert_eq!(tree[0].1.len(), 2, "both of the night's playlists, together");
        assert_eq!(tree[1].0, "Digging");
    }

    #[test]
    fn the_things_that_fail_in_a_booth_are_the_things_that_need_attention() {
        let mut track = Track::placeholder(1);
        track.analyzed = true;
        track.has_grid = true;
        track.grid_confidence = 4.0;
        assert_eq!(track.needs_attention(), None);

        track.float_samples = true;
        assert!(track.needs_attention().unwrap().contains("float"));
        track.float_samples = false;

        track.sample_rate = 192_000;
        assert!(track.needs_attention().unwrap().contains("96 kHz"));
        track.sample_rate = 44_100;

        track.has_grid = false;
        assert!(track.needs_attention().unwrap().contains("beat grid"));
    }

    #[test]
    fn an_unanalysed_track_is_unprepared_rather_than_broken() {
        // Nothing has looked at it yet, so it has no grid — which is not the
        // same as having been looked at and found to have none.
        let track = Track::placeholder(1);
        assert!(track.unprepared());
        assert_eq!(track.needs_attention(), None);
    }

    #[test]
    fn energy_sorts_a_crate_from_tool_to_peak() {
        assert_eq!(energy_from(0.2), 1);
        assert_eq!(energy_from(1.5), 3);
        assert_eq!(energy_from(6.0), 5);
        // Monotonic, which is the only property the meter really promises.
        let mut previous = 0;
        for step in 0..60 {
            let level = energy_from(step as f32 / 10.0);
            assert!(level >= previous, "energy went down at {step}");
            previous = level;
        }
    }

    #[test]
    fn a_collection_survives_a_round_trip_through_disk() {
        let dir = scratch("library");
        let path = dir.join("library.json");

        let mut library = Library::new();
        let id = library.add(Path::new("/music/a.flac"));
        library.get_mut(id).unwrap().bpm = 128.02;
        library.get_mut(id).unwrap().tags = vec!["peak".into()];
        library.playlists.push(Playlist { name: "peak".into(), ..Playlist::default() });
        library.save(&path).unwrap();

        let read = Library::load(&path).unwrap();
        assert_eq!(read.tracks, library.tracks);
        assert_eq!(read.playlists, library.playlists);
        assert_eq!(read.saved, library.saved);

        // Ids keep counting from where they were, rather than being handed out
        // again to different files.
        let mut read = read;
        let next = read.add(Path::new("/music/b.flac"));
        assert_ne!(next, id);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_library_is_a_new_one_but_an_unreadable_one_is_an_error() {
        let dir = scratch("library-bad");
        assert_eq!(Library::load(&dir.join("nothing.json")).unwrap().tracks.len(), 0);

        let broken = dir.join("broken.json");
        std::fs::write(&broken, b"{not json").unwrap();
        // Replacing a real collection with an empty one would be the worst
        // possible response to a bad read.
        assert!(Library::load(&broken).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_does_not_destroy_the_previous_file_until_the_new_one_is_written() {
        let dir = scratch("library-atomic");
        let path = dir.join("library.json");
        let library = Library::new();
        library.save(&path).unwrap();
        library.save(&path).unwrap();

        // The temporary is not left behind to be mistaken for a collection.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["library.json".to_string()], "{leftovers:?}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
