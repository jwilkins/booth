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
///
/// One role per part the separator writes, rather than per combination a DJ
/// might want. A row here is a file on the drive, and offering an
/// `(instrumental)` that is two files summed at load time meant the browser
/// promised something no single file backed — fine in the deck, wrong on a
/// player, which has only what was written for it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    #[default]
    Track,
    /// The vocal stem on its own.
    Vocals,
    /// The drums on their own.
    Drums,
    /// Everything that is neither: bass, chords, leads, the rest of the record.
    Melody,
}

impl Role {
    /// How the row is labelled in the browser.
    pub fn suffix(self) -> &'static str {
        match self {
            Role::Track => "",
            Role::Vocals => " (vocals)",
            Role::Drums => " (drums)",
            Role::Melody => " (melody)",
        }
    }

    /// What the stem pill on the row says.
    ///
    /// The parent says `original` rather than `kit`: the pill names what the
    /// row is, and every row in a kit is part of the kit, so `kit` on the
    /// parent read as a label for the group rather than for that line.
    pub fn label(self) -> &'static str {
        match self {
            Role::Track => "original",
            Role::Vocals => "vocals",
            Role::Drums => "drums",
            Role::Melody => "melody",
        }
    }

    /// The three parts, in the order they hang under their parent.
    pub const PARTS: [Role; 3] = [Role::Vocals, Role::Drums, Role::Melody];
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

    /// The parts `other` has rendered that this kit has not.
    ///
    /// A stem kit is rendered from the audio, and two copies of a recording
    /// hold the same audio, so a part rendered from either one is the same
    /// sound — which is what makes taking one from a copy sound rather than
    /// merely convenient.
    pub fn missing_from(&self, other: &StemKit) -> Vec<&'static str> {
        [
            ("vocals", &self.vocals, &other.vocals),
            ("drums", &self.drums, &other.drums),
            ("melody", &self.melody, &other.melody),
        ]
        .into_iter()
        .filter(|(_, mine, theirs)| mine.is_none() && theirs.is_some())
        .map(|(name, _, _)| name)
        .collect()
    }

    /// Take the parts this kit lacks from `other`, leaving the rest alone.
    pub fn fill_from(&mut self, other: &StemKit) {
        for (mine, theirs) in [
            (&mut self.vocals, &other.vocals),
            (&mut self.drums, &other.drums),
            (&mut self.melody, &other.melody),
        ] {
            if mine.is_none() {
                mine.clone_from(theirs);
            }
        }
    }

    /// The parts, in the order their rows hang under the parent, so that the
    /// browser and the drive's browse list agree about what comes second.
    pub fn each(&self) -> [(&'static str, Option<&PathBuf>); 3] {
        [
            ("vocals", self.vocals.as_ref()),
            ("drums", self.drums.as_ref()),
            ("melody", self.melody.as_ref()),
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
    /// The file as it is on disk. Two agreeing means the bytes are the same,
    /// which is the only case where deleting one of them is plainly safe.
    #[serde(default)]
    pub file_hash: String,
    /// The encoded audio inside it, with the tag blocks skipped — so the same
    /// rip tagged twice still agrees with itself.
    #[serde(default)]
    pub audio_hash: String,
    pub bytes: u64,

    pub bpm: f64,
    /// How clearly the tempo stood out. Under about 2 the tracker was guessing.
    pub grid_confidence: f32,
    pub has_grid: bool,
    /// Camelot notation, e.g. `8A`, or empty when no key was found.
    pub key: String,
    pub key_confidence: f32,
    /// 1 to 5, from how much is being played, or 0 for not measured.
    /// See [`energy_from`].
    pub energy: u8,
    /// The measurement behind the meter: peak onset density, in the units
    /// `musicai::analysis::peak_intensity` reports. Kept so that the rank can
    /// be argued with — five bars hide whether a track sat just under a
    /// threshold or nowhere near one.
    #[serde(default)]
    pub intensity: f32,
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
    /// Whether the artist and title came out of the file's own tags rather than
    /// off its file name.
    ///
    /// The difference matters when a fingerprint disagrees with them: a tag is
    /// somebody's answer, and a file name is a guess.
    #[serde(default)]
    pub from_tags: bool,
    /// Whether a fingerprint lookup has been tried, so a track with no match is
    /// not looked up again on every pass.
    #[serde(default)]
    pub identified: bool,
    /// A FairPlay purchase, found by reading the container's brand at import.
    /// Nothing here can convert one, so it is worth saying early.
    #[serde(default)]
    pub protected: bool,
}

impl Track {
    /// How much is known about this track, for choosing between copies of the
    /// same recording.
    ///
    /// Kinds of thing first, quantity second: a copy carrying eight tags and
    /// nothing else does not know more about a record than one carrying an
    /// album, a year and one tag, so counting tags alongside fields would let
    /// the noisiest copy win. Only after the kinds tie does the amount decide.
    ///
    /// Everything here is something a person typed or a lookup filled in.
    /// Length, format and bitrate are not: they describe the file, and the
    /// copies of a recording hold the same audio by definition.
    pub fn how_much_is_known(&self) -> (usize, usize) {
        let kinds = [
            !self.artist.trim().is_empty(),
            !self.title.trim().is_empty(),
            !self.album.trim().is_empty(),
            self.year.is_some(),
            !self.tags.is_empty(),
            !self.cues.is_empty(),
            self.analyzed,
            !self.stems.is_empty(),
        ]
        .into_iter()
        .filter(|known| *known)
        .count();
        (kinds, self.tags.len() + self.cues.len())
    }

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
            file_hash: String::new(),
            audio_hash: String::new(),
            bytes: 0,
            bpm: 0.0,
            grid_confidence: 0.0,
            has_grid: false,
            key: String::new(),
            key_confidence: 0.0,
            energy: 0,
            intensity: 0.0,
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
            from_tags: false,
            identified: false,
            protected: false,
        }
    }

    /// Take from a rekordbox record whatever this one does not already have.
    ///
    /// The rule throughout is that nothing already here is overwritten.
    /// rekordbox's opinion of a file is not better for being older, and where
    /// this program has measured something itself, that measurement is the one
    /// the waveform was drawn from and the cues were placed against — half of
    /// each would be worse than either. What comes across is what is *missing*.
    ///
    /// Returns whether anything changed.
    pub fn fill_from(&mut self, from: &musicai::rekordbox::master::Track) -> bool {
        let track = self;
        let mut touched = false;
        let fill = |into: &mut String, value: &str| {
            if into.trim().is_empty() && !value.trim().is_empty() {
                *into = value.trim().to_string();
                return true;
            }
            false
        };
        touched |= fill(&mut track.artist, &from.artist);
        touched |= fill(&mut track.album, &from.album);
        // A title is never empty here — the scan falls back to the file name —
        // so it is only replaced when this one is still that fallback.
        let untitled = track.title.trim().is_empty()
            || track.path.file_stem().is_some_and(|stem| *stem == *track.title);
        if untitled && !from.title.trim().is_empty() {
            track.title = from.title.trim().to_string();
            touched = true;
        }
        if track.year.is_none() {
            track.year = from.year;
            touched |= from.year.is_some();
        }

        // Taken only when there is no grid here at all. Keyed on the grid
        // rather than on whether this program has analysed the file, for two
        // reasons: analysing and finding no beat leaves a track that would
        // rather have rekordbox's grid than none, and keying on `analyzed`
        // means the condition is still true after the grid has been taken —
        // so a second import would report a change it did not make.
        if !track.has_grid && from.bpm > 0.0 {
            track.bpm = from.bpm;
            track.has_grid = true;
            touched = true;
        }
        if track.key.trim().is_empty() && !from.key.trim().is_empty() {
            track.key = from.key.trim().to_string();
            touched = true;
        }
        if track.cues.is_empty() && !from.cues.is_empty() {
            track.cues = from
                .cues
                .iter()
                .map(|cue| CueMark {
                    letter: cue.letter,
                    time_ms: cue.time_ms,
                    label: cue.label.clone(),
                    color: crate::job::cue_color(cue.letter),
                })
                .collect();
            touched = true;
        }

        // These have no equivalent anywhere else, so they are taken whenever
        // rekordbox has more of them: a play count is a fact about history
        // that this program was not around for.
        if from.play_count > track.play_count {
            track.play_count = from.play_count;
            touched = true;
        }
        for tag in &from.my_tags {
            if !track.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                track.tags.push(tag.clone());
                touched = true;
            }
        }
        if from.rating > 0 {
            let star = format!("{}\u{2605}", from.rating);
            if !track.tags.contains(&star) {
                track.tags.push(star);
                touched = true;
            }
        }
        track.tags.sort();
        track.tags.dedup();
        touched
    }

    /// The file or files this row is actually made of.
    ///
    /// One file each, now that a row is one of the separator's own parts. It
    /// stays a list because the deck sums whatever it is given, and a row that
    /// could not name its own audio is a row that silently does nothing.
    pub fn sources(&self) -> Vec<PathBuf> {
        match self.role {
            Role::Track => vec![self.path.clone()],
            Role::Vocals => self.stems.vocals.iter().cloned().collect(),
            Role::Drums => self.stems.drums.iter().cloned().collect(),
            Role::Melody => self.stems.melody.iter().cloned().collect(),
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
    pub fn needs_attention(&self) -> Option<String> {
        if let Some(problem) = self.incompatibility() {
            return Some(format!("{} — {}", problem.what(), problem.fix()));
        }
        if self.analyzed && !self.has_grid {
            return Some("no beat grid could be found".into());
        }
        if self.analyzed && self.grid_confidence > 0.0 && self.grid_confidence < 2.0 {
            return Some("the tempo is a guess".into());
        }
        None
    }

    /// The first reason the hardware will not play this file, if there is one.
    ///
    /// Read from what the scan already found rather than by opening the file
    /// again, except for the one small header read that says whether an MP4 is
    /// a protected purchase — and that answer is kept on the record from the
    /// import, so this stays cheap enough to ask about every row.
    pub fn incompatibility(&self) -> Option<musicai::compat::Problem> {
        if self.protected {
            return Some(musicai::compat::Problem::Protected);
        }
        // An empty format is a record nothing has looked at yet, not a file in
        // a format nothing opens. Reporting the first as the second would put
        // every freshly added track in the attention list.
        if !self.format.is_empty() && !musicai::commands::is_playable(&self.format) {
            return Some(musicai::compat::Problem::Format(self.format.clone()));
        }
        if self.float_samples {
            return Some(musicai::compat::Problem::FloatSamples);
        }
        if self.sample_rate > 96_000 {
            return Some(musicai::compat::Problem::TooFast(self.sample_rate));
        }
        None
    }

    pub fn duration_text(&self) -> String {
        let total = self.duration_secs.round() as u64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// Where one bar of the meter ends and the next begins, in the units
/// `musicai::analysis::peak_intensity` reports: onset strength per bin per
/// frame, over the loudest fifteen seconds of the track.
///
/// This is a calibration table, not a formula. It is spaced roughly
/// logarithmically because the quantity is: the gap between a tool and a
/// groove is a doubling, not an addition.
const STEPS: [f32; 4] = [0.025, 0.055, 0.100, 0.170];

/// Turn a peak onset density into the five-bar meter.
///
/// It is a rank, not a measurement: what it has to do is sort a crate so that
/// the tools are at one end and the peak-time records at the other. Zero means
/// nothing was measured — an unanalysed track, or one with no sound in it —
/// and reads as an empty meter rather than as the quietest possible record.
pub fn energy_from(intensity: f32) -> u8 {
    // A NaN is not a quiet track; it is an answer that went wrong somewhere,
    // and it has to fall out here rather than being ranked.
    if intensity.is_nan() || intensity <= 0.0 {
        return 0;
    }
    1 + STEPS.iter().filter(|&&step| intensity >= step).count() as u8
}

/// The name a file would have if it were not a copy of one.
///
/// A file duplicated by a file manager, a browser or a sync client comes back
/// as `track_04 (1).flac` beside the `track_04.flac` it was made from, so a
/// name ending in a parenthesised number says the file was made by copying
/// something — which is a fact about where it came from, and worth a say in
/// which copy of a record is the original.
///
/// Only the parenthesised form, which is what every tool that renames on
/// collision uses. Not "final (2 of 3)", which is not a number; not a title
/// that happens to end in one, like `Untitled (1994).flac`, since a year is
/// four digits and a copy number is not.
///
/// The extension is kept as it is: `track_04 (3).m4a` came from an `.m4a`, and
/// the file it collided with may well have been a different format.
pub fn name_without_copy_number(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let inside = stem.trim_end().strip_suffix(')')?.rsplit_once('(')?.1;
    // A run of digits, and short enough to be a copy count rather than a year
    // or a catalogue number.
    if inside.is_empty() || inside.len() > 3 || !inside.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let base = stem.trim_end().strip_suffix(')')?.rsplit_once('(')?.0.trim_end();
    if base.is_empty() {
        return None;
    }
    let mut renamed = path.to_path_buf();
    renamed.set_file_name(match path.extension().and_then(|e| e.to_str()) {
        Some(extension) => format!("{base}.{extension}"),
        None => base.to_string(),
    });
    Some(renamed)
}

/// One thing about a recording that two copies of it can differ on.
///
/// Only the things a person put there or a lookup filled in. Everything else a
/// track carries — its size, its format, where it is — describes the file
/// rather than the recording, and the whole point of a group is that the
/// recording is the same.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Field {
    Artist,
    Title,
    Album,
    Year,
    Cues,
    Tags,
    Playlists,
    Plays,
    Stems,
    /// The grid, key, energy and phrases — the hours of listening.
    Analysis,
}

impl Field {
    pub fn name(self) -> &'static str {
        match self {
            Field::Artist => "artist",
            Field::Title => "title",
            Field::Album => "album",
            Field::Year => "year",
            Field::Cues => "cues",
            Field::Tags => "tags",
            Field::Playlists => "playlists",
            Field::Plays => "plays",
            Field::Stems => "stems",
            Field::Analysis => "analysis",
        }
    }
}

/// Whose answer to take where two copies disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Kept,
    Other,
}

/// Something one copy says that the kept track disagrees with.
#[derive(Clone, Debug, PartialEq)]
pub struct Disagreement {
    pub field: Field,
    pub kept: String,
    pub other: String,
}

/// What folding one copy into the kept track would do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merge {
    /// What the kept track has nothing for, which this copy can fill in. Not a
    /// decision anybody needs to be asked about: a blank has no other answer.
    pub adds: Vec<(Field, String)>,
    /// What they both say something about, and say differently. The only part
    /// that needs a person.
    pub conflicts: Vec<Disagreement>,
}

impl Merge {
    /// Whether this copy can be folded in without asking anybody anything.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.adds.is_empty() && self.conflicts.is_empty()
    }

    /// What it would add, for the one line the sheet has room for.
    pub fn summary(&self) -> String {
        let names: Vec<&str> = self.adds.iter().map(|(field, _)| field.name()).collect();
        match names.is_empty() {
            true => "nothing the kept copy lacks".to_string(),
            false => format!("adds {}", names.join(", ")),
        }
    }
}

/// A set of tracks that are the same recording.
pub struct Copies {
    /// The one to keep, chosen by [`Library::duplicate_groups`].
    pub keep: u32,
    /// The others, in the order they were found.
    pub rest: Vec<Duplicate>,
}

impl Copies {
    /// Every copy in the group, the one to keep first.
    pub fn all(&self) -> Vec<u32> {
        let mut ids = vec![self.keep];
        ids.extend(self.rest.iter().map(|copy| copy.id));
        ids
    }

    /// A name for this group that does not move when the choice of which copy
    /// to keep does — so that a decision made about a group survives the user
    /// changing their mind about which of its files to keep.
    pub fn key(&self) -> u32 {
        self.all().into_iter().min().unwrap_or(self.keep)
    }
}

/// One copy that is not the one being kept.
pub struct Duplicate {
    pub id: u32,
    /// Whether this file is byte-for-byte the kept one.
    ///
    /// Per copy rather than per group, because a group can be mixed: two
    /// identical files and a third that is the same recording carrying
    /// different tags is one recording in three places, and saying so twice in
    /// overlapping halves would be a worse description of it.
    ///
    /// It decides how safe deleting is. An identical file loses nothing at
    /// all; one that only matches by audio loses whatever its tags say that
    /// the kept copy's do not.
    pub identical: bool,
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
    /// The playlist that was written to it, from before a drive could carry
    /// more than one. Read when loading an older collection and then left
    /// alone; `playlists` is what everything asks.
    #[serde(default)]
    pub playlist: String,
    /// The playlists written to it, by name, in the order they go on.
    ///
    /// A player shows a tree, so a drive that could hold only one list was
    /// making the DJ choose between taking the night's sets and taking one of
    /// them. The folder each sits in comes from the playlist itself.
    #[serde(default)]
    pub playlists: Vec<String>,
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

    /// The playlists this drive carries.
    ///
    /// Collections written before a drive could hold more than one have the
    /// single `playlist` field and an empty list; reading it here rather than
    /// rewriting the file on load means an older collection opens in an older
    /// build afterwards, which matters while both exist.
    pub fn playlist_names(&self) -> Vec<String> {
        match self.playlists.is_empty() {
            true => self
                .playlist
                .is_empty()
                .then(Vec::new)
                .unwrap_or_else(|| vec![self.playlist.clone()]),
            false => self.playlists.clone(),
        }
    }
}

/// The whole collection.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    /// The folders of the playlist tree, in the order they were made.
    ///
    /// Kept rather than derived from the playlists inside them, so that a
    /// folder can exist before it has anything in it. Making the folder and
    /// then filling it is the order people work in, and a folder that vanished
    /// the moment its last playlist moved out would be a folder you could not
    /// rearrange.
    #[serde(default)]
    pub folders: Vec<String>,
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
        Role::PARTS
            .into_iter()
            .map(|role| Track {
                id: companion_id(track.id, role),
                role,
                parent: Some(track.id),
                // The kit stays on the companion rather than being blanked:
                // it is what says which files the row is actually made of, and
                // a row that cannot name its own audio cannot be played.
                ..track.clone()
            })
            .collect()
    }

    /// A track, or one of the companion rows derived from one.
    ///
    /// The browser gives companions ids so they can be selected, but they are
    /// not in the collection — they are made when the list is built. Anything
    /// that takes an id off a row has to be able to get back to a track, or the
    /// row silently does nothing, which is what a companion that would not play
    /// was.
    pub fn row(&self, id: u32) -> Option<Track> {
        if let Some(track) = self.get(id) {
            return Some(track.clone());
        }
        let (parent, role) = companion_of(id)?;
        let track = self.get(parent)?;
        self.companions(track).into_iter().find(|companion| companion.role == role)
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
        // The root first when anything is in it, then the folders in the order
        // they were made, then any folder a playlist names that is not on the
        // list — which is what an imported library arrives as.
        let mut tree: Vec<(String, Vec<&Playlist>)> = vec![(String::new(), Vec::new())];
        for folder in &self.folders {
            tree.push((folder.clone(), Vec::new()));
        }
        for playlist in &self.playlists {
            match tree.iter_mut().find(|(folder, _)| *folder == playlist.folder) {
                Some((_, list)) => list.push(playlist),
                None => tree.push((playlist.folder.clone(), vec![playlist])),
            }
        }
        tree.retain(|(folder, lists)| !folder.is_empty() || !lists.is_empty());
        tree
    }

    /// Make a folder, or say why not. The name is what identifies it, so two
    /// of the same name would be one folder drawn twice.
    pub fn add_folder(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a folder needs a name".into());
        }
        if self.folders.iter().any(|f| f == name) {
            return Err(format!("there is already a folder called \u{201c}{name}\u{201d}"));
        }
        self.folders.push(name.to_string());
        Ok(())
    }

    /// Make a playlist, or say why not.
    pub fn add_playlist(&mut self, name: &str, folder: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a playlist needs a name".into());
        }
        if self.playlists.iter().any(|p| p.name == name) {
            return Err(format!("there is already a playlist called \u{201c}{name}\u{201d}"));
        }
        self.playlists.push(Playlist {
            name: name.to_string(),
            folder: folder.to_string(),
            tracks: Vec::new(),
        });
        Ok(())
    }

    /// Rename a playlist, bringing the drives that carry it along.
    ///
    /// A drive names its playlists by name, so a rename that did not follow
    /// through would leave the drive pointing at nothing and the next sync
    /// proposing to delete everything on it.
    pub fn rename_playlist(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("a playlist needs a name".into());
        }
        if to == from {
            return Ok(());
        }
        if self.playlists.iter().any(|p| p.name == to) {
            return Err(format!("there is already a playlist called \u{201c}{to}\u{201d}"));
        }
        let Some(playlist) = self.playlists.iter_mut().find(|p| p.name == from) else {
            return Err(format!("no playlist called \u{201c}{from}\u{201d}"));
        };
        playlist.name = to.to_string();
        for drive in &mut self.drives {
            for name in drive.playlists.iter_mut().filter(|n| *n == from) {
                *name = to.to_string();
            }
            if drive.playlist == from {
                drive.playlist = to.to_string();
            }
        }
        Ok(())
    }

    /// Rename a folder, moving what is in it with it.
    pub fn rename_folder(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("a folder needs a name".into());
        }
        if to == from {
            return Ok(());
        }
        if self.folders.iter().any(|f| f == to) {
            return Err(format!("there is already a folder called \u{201c}{to}\u{201d}"));
        }
        for folder in self.folders.iter_mut().filter(|f| *f == from) {
            *folder = to.to_string();
        }
        for playlist in self.playlists.iter_mut().filter(|p| p.folder == from) {
            playlist.folder = to.to_string();
        }
        Ok(())
    }

    /// Delete a playlist, and stop any drive from asking for it.
    ///
    /// The tracks are untouched: a playlist is a list of what to write, not a
    /// place the music is kept, and deleting one has never meant losing a file.
    pub fn remove_playlist(&mut self, name: &str) {
        self.playlists.retain(|p| p.name != name);
        for drive in &mut self.drives {
            drive.playlists.retain(|n| n != name);
            if drive.playlist == name {
                drive.playlist.clear();
            }
        }
    }

    /// Delete a folder. What was inside comes back to the top level rather
    /// than going with it — the playlists are the work, the folder is where
    /// they were filed.
    pub fn remove_folder(&mut self, name: &str) {
        self.folders.retain(|f| f != name);
        for playlist in self.playlists.iter_mut().filter(|p| p.folder == name) {
            playlist.folder.clear();
        }
    }

    /// The tracks whose audio has never been hashed, and where their files are.
    ///
    /// Import hashes as it goes, so these are the ones that were already in the
    /// collection before it did. They are invisible to `duplicate_groups`,
    /// which is why they have to be findable: a library that predates hashing
    /// would otherwise report no copies for ever and never say why.
    pub fn unhashed(&self) -> Vec<(u32, PathBuf)> {
        self.tracks
            .iter()
            .filter(|track| track.role == Role::Track && track.audio_hash.is_empty())
            .map(|track| (track.id, track.path.clone()))
            .collect()
    }

    /// What folding one copy into the one being kept would do.
    ///
    /// The two hold the same recording, so the file's own numbers — length,
    /// format, bitrate — cannot differ in any way worth reporting. What can
    /// differ is what somebody wrote down about it, and there the rule is that
    /// a blank is not an opinion: where the kept track says nothing and the
    /// copy says something, the copy is simply right, and nobody needs asking.
    /// Only where both say something, and say it differently, is there a
    /// question — and that is the only thing this reports as a conflict.
    pub fn plan_merge(&self, keep: u32, other: u32) -> Merge {
        let mut merge = Merge::default();
        let (Some(kept), Some(copy)) = (self.get(keep), self.get(other)) else { return merge };

        /// A blank on the kept side is filled; two different answers are asked
        /// about; the same answer twice is nothing at all.
        fn words(merge: &mut Merge, field: Field, kept: &str, other: &str) {
            let (kept, other) = (kept.trim(), other.trim());
            if other.is_empty() || kept == other {
                return;
            }
            match kept.is_empty() {
                true => merge.adds.push((field, other.to_string())),
                false => merge.conflicts.push(Disagreement {
                    field,
                    kept: kept.to_string(),
                    other: other.to_string(),
                }),
            }
        }

        words(&mut merge, Field::Artist, &kept.artist, &copy.artist);
        words(&mut merge, Field::Title, &kept.title, &copy.title);
        words(&mut merge, Field::Album, &kept.album, &copy.album);
        words(
            &mut merge,
            Field::Year,
            &kept.year.map(|y| y.to_string()).unwrap_or_default(),
            &copy.year.map(|y| y.to_string()).unwrap_or_default(),
        );

        // Tags are a set, so two different sets are not a disagreement: having
        // been called both "peak" and "warmup" by two different imports is
        // something a person did twice, not something to choose between.
        let new_tags: Vec<String> =
            copy.tags.iter().filter(|tag| !kept.tags.contains(tag)).cloned().collect();
        if !new_tags.is_empty() {
            merge.adds.push((Field::Tags, new_tags.join(", ")));
        }

        // A playlist holding the copy should hold the kept one instead. Also
        // not a disagreement: the answer is both.
        let joins: Vec<&str> = self
            .playlists
            .iter()
            .filter(|list| list.tracks.contains(&other) && !list.tracks.contains(&keep))
            .map(|list| list.name.as_str())
            .collect();
        if !joins.is_empty() {
            merge.adds.push((Field::Playlists, joins.join(", ")));
        }

        if copy.play_count > 0 {
            merge.adds.push((Field::Plays, plural(copy.play_count as usize, "play")));
        }

        let stems = kept.stems.missing_from(&copy.stems).len();
        if stems > 0 {
            merge.adds.push((Field::Stems, plural(stems, "part")));
        }

        // Cues are placed by hand against the audio, and the audio is the same
        // in both, so the copy's marks are as good as the kept one's. Two
        // different sets is a real question — one of them is somebody's work.
        if kept.cues != copy.cues && !copy.cues.is_empty() {
            let describe = |cues: &[CueMark]| plural(cues.len(), "cue");
            match kept.cues.is_empty() {
                true => merge.adds.push((Field::Cues, describe(&copy.cues))),
                false => merge.conflicts.push(Disagreement {
                    field: Field::Cues,
                    kept: describe(&kept.cues),
                    other: describe(&copy.cues),
                }),
            }
        }

        // The listening is hours of work and is the same measurement of the
        // same audio, so an unanalysed keeper takes it. Two analyses are never
        // asked about: they measured identical bytes, so any difference between
        // them is noise, and there is nothing to choose.
        if !kept.analyzed && copy.analyzed {
            let what = match copy.key.is_empty() {
                true => format!("{:.1} bpm", copy.bpm),
                false => format!("{:.1} bpm, {}", copy.bpm, copy.key),
            };
            merge.adds.push((Field::Analysis, what));
        }

        merge.adds.sort_by_key(|(field, _)| *field);
        merge.conflicts.sort_by_key(|conflict| conflict.field);
        merge
    }

    /// Fold one copy into the one being kept, taking `picks` where they differ.
    ///
    /// Everything the kept track had nothing for is filled in; anything they
    /// disagree about stays as the kept track had it unless `picks` says
    /// otherwise. Call this before removing the copy — playlists still holding
    /// it are moved over here, and once it is gone there is nothing to move.
    pub fn merge_copy(&mut self, keep: u32, other: u32, picks: &HashMap<Field, Side>) {
        let plan = self.plan_merge(keep, other);
        let Some(copy) = self.get(other).cloned() else { return };

        // Whether the kept track ends up with the copy's answer for a field:
        // either it had none, or it had one and the copy's was chosen.
        let take = |field: Field| {
            plan.adds.iter().any(|(seen, _)| *seen == field)
                || (plan.conflicts.iter().any(|c| c.field == field)
                    && picks.get(&field) == Some(&Side::Other))
        };

        // Playlists first, while there are still two tracks for them to point
        // at. In place, so the order somebody built the list in survives.
        if take(Field::Playlists) {
            for list in &mut self.playlists {
                if !list.tracks.contains(&keep) {
                    for slot in list.tracks.iter_mut().filter(|t| **t == other) {
                        *slot = keep;
                    }
                }
            }
        }

        let analysis = take(Field::Analysis);
        let Some(kept) = self.get_mut(keep) else { return };

        if take(Field::Artist) {
            kept.artist = copy.artist.clone();
        }
        if take(Field::Title) {
            kept.title = copy.title.clone();
        }
        if take(Field::Album) {
            kept.album = copy.album.clone();
        }
        if take(Field::Year) {
            kept.year = copy.year;
        }
        if take(Field::Tags) {
            for tag in &copy.tags {
                if !kept.tags.contains(tag) {
                    kept.tags.push(tag.clone());
                }
            }
        }
        if take(Field::Cues) {
            kept.cues = copy.cues.clone();
        }
        if take(Field::Plays) {
            kept.play_count += copy.play_count;
        }
        // Outside its field: the later of two dates is the answer whichever
        // copy it came from, and it is not something to be asked about.
        kept.last_played = kept.last_played.max(copy.last_played);
        if take(Field::Stems) {
            kept.stems.fill_from(&copy.stems);
        }
        if analysis {
            kept.bpm = copy.bpm;
            kept.grid_confidence = copy.grid_confidence;
            kept.has_grid = copy.has_grid;
            kept.key = copy.key.clone();
            kept.key_confidence = copy.key_confidence;
            kept.energy = copy.energy;
            kept.intensity = copy.intensity;
            kept.beats = copy.beats;
            kept.phrases = copy.phrases.clone();
            kept.loudness_lufs = copy.loudness_lufs;
            kept.peak_dbtp = copy.peak_dbtp;
            kept.analyzed = true;
            if kept.cues.is_empty() {
                kept.cues = copy.cues.clone();
            }
        }
        // The picture and the stem envelopes are cached under the track's id,
        // so taking the listening without them would leave a track that says
        // it is analysed and draws nothing until it is analysed again.
        if analysis {
            if let Some(bands) = cached_waveform(other) {
                let _ = cache_waveform(keep, &bands);
            }
            if let Some(envelopes) = cached_envelopes(other) {
                let _ = cache_envelopes(keep, &envelopes);
            }
        }
    }

    /// Tracks that are the same recording, grouped, worst offenders first.
    ///
    /// Grouped by the audio's hash alone, because that is the broader of the
    /// two relations and contains the other: identical bytes mean identical
    /// audio, so anything the file hash would pair is already together here.
    /// Grouping by both in turn instead made a byte-identical pair use up its
    /// members, and a third copy of the same recording carrying different tags
    /// was then left on its own and reported as nothing at all.
    ///
    /// The one to keep is the copy that knows the most about the record — the
    /// one with the album, the year, the tags, the cues, the listening — since
    /// that is the work that would be lost, and since a disagreement between
    /// two copies is settled in the keeper's favour unless somebody says
    /// otherwise. Then the file whose name no copier wrote — a
    /// `track_04 (1).flac` was made from something, and that something is the
    /// likelier original. Being inside the library folder only breaks what is
    /// left. Never the shortest path or the newest file: both are accidents.
    pub fn duplicate_groups(&self, library_path: &Path) -> Vec<Copies> {
        let mut by_audio: Vec<(&str, Vec<u32>)> = Vec::new();
        for track in self.tracks.iter().filter(|t| t.role == Role::Track) {
            // An unreadable file has bigger problems than being a copy, and
            // one empty hash matching another would put every one of them in a
            // group together and offer to delete them.
            if track.audio_hash.is_empty() {
                continue;
            }
            match by_audio.iter_mut().find(|(seen, _)| *seen == track.audio_hash) {
                Some((_, ids)) => ids.push(track.id),
                None => by_audio.push((&track.audio_hash, vec![track.id])),
            }
        }

        let mut groups: Vec<Copies> = by_audio
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(_, ids)| {
                let keep = self.pick_keeper(&ids, library_path);
                let kept_file = self.get(keep).map(|t| t.file_hash.clone()).unwrap_or_default();
                let rest = ids
                    .into_iter()
                    .filter(|id| *id != keep)
                    .map(|id| Duplicate {
                        id,
                        identical: self
                            .get(id)
                            .is_some_and(|t| !t.file_hash.is_empty() && t.file_hash == kept_file),
                    })
                    .collect();
                Copies { keep, rest }
            })
            .collect();
        groups.sort_by_key(|group| std::cmp::Reverse(group.rest.len()));
        groups
    }

    /// Which of a set of copies to keep. See [`Library::duplicate_groups`].
    fn pick_keeper(&self, ids: &[u32], library_path: &Path) -> u32 {
        // Most known first, because the copy that knows the most is the one
        // whose answer should stand where two of them disagree — and a
        // disagreement is decided in the keeper's favour unless somebody says
        // otherwise. Being inside the library folder only settles what the
        // name has not: it says which copy this program is responsible for,
        // not which one is right about the record.
        //
        // The last term settles the rest. Without it two copies that score the
        // same left the answer to whichever `max_by_key` happened to reach
        // last, which is neither a decision nor the same one twice; the
        // earliest known copy is the one playlists and drives already point at.
        let score = |id: &u32| -> ((usize, usize), bool, bool, std::cmp::Reverse<u32>) {
            let Some(track) = self.get(*id) else {
                return ((0, 0), false, false, std::cmp::Reverse(*id));
            };
            (
                track.how_much_is_known(),
                // A name a copier wrote — `track_04 (1).flac` — says this file
                // was made from another one, which is a fact about where it
                // came from rather than about the record. So it ranks below
                // what the file knows and above where it happens to sit.
                name_without_copy_number(&track.path).is_none(),
                track.path.starts_with(library_path),
                std::cmp::Reverse(*id),
            )
        };
        ids.iter().max_by_key(|id| score(id)).copied().unwrap_or(ids[0])
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
    0x8000_0000 | (parent << 2) | role_tag(role)
}

fn role_tag(role: Role) -> u32 {
    match role {
        Role::Track => 0,
        Role::Vocals => 1,
        Role::Drums => 2,
        Role::Melody => 3,
    }
}

/// The track and role a companion id was made from, or `None` for a real id.
pub fn companion_of(id: u32) -> Option<(u32, Role)> {
    if id & 0x8000_0000 == 0 {
        return None;
    }
    let role = match id & 0b11 {
        1 => Role::Vocals,
        2 => Role::Drums,
        3 => Role::Melody,
        _ => return None,
    };
    Some(((id & 0x7FFF_FFFF) >> 2, role))
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

/// Where a track's per-stem loudness is cached.
pub fn envelopes_path(id: u32) -> PathBuf {
    data_dir().join("waveforms").join(format!("{id:08}.stems"))
}

/// Keep a track's stem envelopes. Written as the three planes end to end, with
/// a length so they can be split again.
pub fn cache_envelopes(id: u32, envelopes: &crate::wave::StemEnvelopes) -> std::io::Result<()> {
    let path = envelopes_path(id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let columns = envelopes.columns();
    let mut bytes = (columns as u32).to_le_bytes().to_vec();
    for plane in [&envelopes.vocals, &envelopes.melody, &envelopes.drums] {
        bytes.extend_from_slice(&plane[..columns]);
    }
    std::fs::write(path, bytes)
}

/// Read them back, if they are there and whole.
pub fn cached_envelopes(id: u32) -> Option<crate::wave::StemEnvelopes> {
    let bytes = std::fs::read(envelopes_path(id)).ok()?;
    let columns = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    // A file that is the wrong length is a file from a different version or a
    // half-finished write; there is nothing to salvage from part of it.
    if columns == 0 || bytes.len() != 4 + columns * 3 {
        return None;
    }
    let plane = |n: usize| bytes[4 + n * columns..4 + (n + 1) * columns].to_vec();
    Some(crate::wave::StemEnvelopes { vocals: plane(0), melody: plane(1), drums: plane(2) })
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

/// "1 track", "2 tracks", "3 copies".
///
/// Here rather than in the window because the log needs it too, and a run whose
/// log says "1 tracks" reads like nobody checked.
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        return format!("1 {noun}");
    }
    // A noun ending in a consonant and a y takes -ies, which is the difference
    // between "3 copies" and "3 copys". Everything else here takes -s.
    let vowel = |c: char| "aeiou".contains(c);
    match noun.strip_suffix('y').filter(|stem| stem.chars().next_back().is_some_and(|c| !vowel(c)))
    {
        Some(stem) => format!("{count} {stem}ies"),
        None => format!("{count} {noun}s"),
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
        assert!(library.companions(&track).is_empty(), "a partial kit is not a kit");

        track.stems.melody = Some("/stems/a-melody.wav".into());
        track.stems.drums = Some("/stems/a-drums.wav".into());
        let companions = library.companions(&track);
        assert_eq!(companions.len(), 3, "one row per part the separator writes");
        assert_eq!(companions[0].role, Role::Vocals);
        assert_eq!(companions[1].role, Role::Drums);
        assert_eq!(companions[2].role, Role::Melody);
        assert!(companions[0].display_title().ends_with("(vocals)"));
        // A companion carries the parent's grid, because it is the same audio.
        assert_eq!(companions[0].bpm, track.bpm);
        assert_eq!(companions[0].parent, Some(id));
    }

    fn rekordbox_track() -> musicai::rekordbox::master::Track {
        musicai::rekordbox::master::Track {
            id: "c1".into(),
            path: "/music/a.flac".into(),
            artist: "Peverelist".into(),
            title: "Roll With The Punches".into(),
            album: "Livity Sound".into(),
            genre: "Techno".into(),
            year: Some(2019),
            bpm: 128.02,
            key: "8A".into(),
            rating: 4,
            comment: "peak".into(),
            play_count: 17,
            duration_secs: 372.0,
            cues: vec![musicai::rekordbox::master::Cue {
                letter: 1,
                time_ms: 30_000,
                label: "drop".into(),
            }],
            my_tags: vec!["peak time".into()],
        }
    }

    #[test]
    fn an_import_fills_in_what_is_missing_and_nothing_else() {
        let mut track = Track::placeholder(1);
        track.path = "/music/a.flac".into();
        assert!(track.fill_from(&rekordbox_track()));

        assert_eq!(track.artist, "Peverelist");
        assert_eq!(track.title, "Roll With The Punches");
        assert_eq!(track.album, "Livity Sound");
        assert_eq!(track.year, Some(2019));
        assert_eq!(track.key, "8A");
        assert!((track.bpm - 128.02).abs() < 1e-9);
        assert!(track.has_grid);
        assert_eq!(track.play_count, 17);
        assert_eq!(track.cues.len(), 1);
        assert_eq!(track.cues[0].letter, 1);
        assert_eq!(track.cues[0].time_ms, 30_000);
        // My Tags become tags, and a rating becomes one too — this program has
        // no stars, and losing them entirely would be worse than a tag.
        assert!(track.tags.contains(&"peak time".to_string()));
        assert!(track.tags.iter().any(|t| t.starts_with('4')), "{:?}", track.tags);
    }

    #[test]
    fn an_import_never_overwrites_work_already_done_here() {
        let mut track = Track::placeholder(1);
        track.path = "/music/a.flac".into();
        track.artist = "Someone Else".into();
        track.title = "A Better Title".into();
        track.key = "3A".into();
        track.year = Some(2001);
        // Measured here, which is the case that matters: the grid on screen and
        // the cues placed against it have to stay one thing.
        track.analyzed = true;
        track.has_grid = true;
        track.bpm = 174.0;
        track.cues =
            vec![CueMark { letter: 1, time_ms: 1_234, label: "mine".into(), color: [1, 2, 3] }];

        track.fill_from(&rekordbox_track());

        assert_eq!(track.artist, "Someone Else");
        assert_eq!(track.title, "A Better Title");
        assert_eq!(track.key, "3A");
        assert_eq!(track.year, Some(2001));
        assert_eq!(track.bpm, 174.0, "an analysed grid is not replaced");
        assert_eq!(track.cues.len(), 1);
        assert_eq!(track.cues[0].time_ms, 1_234, "cues already placed are kept");
        // The things it has no other way to know still come across.
        assert_eq!(track.play_count, 17);
        assert!(track.tags.contains(&"peak time".to_string()));
    }

    #[test]
    fn a_grid_is_taken_only_when_nothing_here_has_measured_one() {
        // Not analysed: rekordbox's grid is better than no grid.
        let mut track = Track::placeholder(1);
        assert!(track.fill_from(&rekordbox_track()));
        assert!((track.bpm - 128.02).abs() < 1e-9);
        assert!(track.has_grid);
    }

    #[test]
    fn a_title_that_is_only_the_file_name_is_not_an_answer() {
        // The scan titles an untagged file after itself. That is a placeholder,
        // and rekordbox knowing better is the whole point of importing.
        let mut track = Track::placeholder(1);
        track.path = "/music/01 - track.flac".into();
        track.title = "01 - track".into();
        track.fill_from(&rekordbox_track());
        assert_eq!(track.title, "Roll With The Punches");
    }

    #[test]
    fn importing_the_same_library_twice_changes_nothing_the_second_time() {
        let mut track = Track::placeholder(1);
        track.path = "/music/a.flac".into();
        assert!(track.fill_from(&rekordbox_track()));
        let once = track.clone();
        assert!(!track.fill_from(&rekordbox_track()), "the second pass has nothing to do");
        assert_eq!(track, once, "and it changed nothing");
    }

    #[test]
    fn a_companion_row_names_the_stems_it_is_made_of() {
        let mut library = Library::new();
        let id = library.add(Path::new("/music/Sirens.flac"));
        {
            let track = library.get_mut(id).unwrap();
            track.stems.vocals = Some("/music/Sirens-vocals.mp3".into());
            track.stems.melody = Some("/music/Sirens-melody.mp3".into());
            track.stems.drums = Some("/music/Sirens-drums.mp3".into());
        }

        // Not the parent's file: playing that would be playing the mix under
        // a row that names one part of it.
        for (role, file) in [
            (Role::Vocals, "/music/Sirens-vocals.mp3"),
            (Role::Drums, "/music/Sirens-drums.mp3"),
            (Role::Melody, "/music/Sirens-melody.mp3"),
        ] {
            let companion = library.row(companion_id(id, role)).unwrap();
            assert_eq!(companion.sources(), vec![PathBuf::from(file)], "{role:?}");
        }

        // A real id still comes back as itself.
        assert_eq!(library.row(id).unwrap().sources(), vec![PathBuf::from("/music/Sirens.flac")]);
        assert!(library.row(companion_id(9_999, Role::Vocals)).is_none());
    }

    #[test]
    fn a_companion_id_can_be_taken_apart_again() {
        // What lets a row that is not in the collection be acted on at all.
        for role in Role::PARTS {
            for parent in [1u32, 2, 7, 1_000, 100_000] {
                let id = companion_id(parent, role);
                assert_eq!(companion_of(id), Some((parent, role)), "{id:#x}");
            }
        }
        assert_eq!(companion_of(1), None, "a real id is not a companion");
        assert_eq!(companion_of(companion_id(5, Role::Track)), None, "no such companion");
    }

    #[test]
    fn a_companion_id_never_collides_with_a_track() {
        let mut library = Library::new();
        let ids: Vec<u32> =
            (0..200).map(|i| library.add(Path::new(&format!("/m/{i}.flac")))).collect();
        let companions: Vec<u32> =
            ids.iter().flat_map(|id| Role::PARTS.map(|role| companion_id(*id, role))).collect();

        for companion in &companions {
            assert!(!ids.contains(companion), "{companion} is also a track id");
        }
        let mut sorted = companions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), companions.len(), "two companions share an id");
    }

    /// A track with the hashes set, so grouping has something to group on.
    fn copy_of(library: &mut Library, path: &str, file: &str, audio: &str) -> u32 {
        let id = library.add(Path::new(path));
        let track = library.get_mut(id).unwrap();
        track.file_hash = file.into();
        track.audio_hash = audio.into();
        id
    }

    #[test]
    fn counting_things_reads_like_somebody_checked() {
        assert_eq!(plural(1, "track"), "1 track");
        assert_eq!(plural(2, "track"), "2 tracks");
        assert_eq!(plural(3, "copy"), "3 copies", "not \"copys\"");
        assert_eq!(plural(1, "copy"), "1 copy");
        // A vowel before the y keeps the plain -s: days, not daies.
        assert_eq!(plural(2, "day"), "2 days");
    }

    #[test]
    fn a_blank_is_filled_in_without_being_asked_about() {
        let mut library = Library::new();
        let keep = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let other = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        {
            let track = library.get_mut(keep).unwrap();
            track.artist = "Peverelist".into();
        }
        {
            let track = library.get_mut(other).unwrap();
            track.artist = "Peverelist".into();
            track.album = "Livity Sound".into();
            track.year = Some(2019);
            track.tags = vec!["peak".into()];
            track.play_count = 3;
        }

        let plan = library.plan_merge(keep, other);
        assert!(plan.is_clean(), "nothing here disagrees: {:?}", plan.conflicts);
        let added: Vec<&str> = plan.adds.iter().map(|(field, _)| field.name()).collect();
        assert_eq!(added, vec!["album", "year", "tags", "plays"]);

        library.merge_copy(keep, other, &HashMap::new());
        let kept = library.get(keep).unwrap();
        assert_eq!(kept.album, "Livity Sound");
        assert_eq!(kept.year, Some(2019));
        assert_eq!(kept.tags, vec!["peak".to_string()]);
        assert_eq!(kept.play_count, 3, "the plays of both copies are the plays of the record");
        assert_eq!(kept.artist, "Peverelist", "the answer they agreed on did not change");
    }

    #[test]
    fn two_different_answers_are_a_question_rather_than_a_choice_made_quietly() {
        let mut library = Library::new();
        let keep = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let other = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        library.get_mut(keep).unwrap().title = "Sirens".into();
        {
            let track = library.get_mut(other).unwrap();
            track.title = "Sirens (Original Mix)".into();
            track.album = "Livity Sound".into();
        }

        let plan = library.plan_merge(keep, other);
        assert!(!plan.is_clean(), "a title said two ways is a conflict");
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].field, Field::Title);
        assert_eq!(plan.conflicts[0].kept, "Sirens");
        assert_eq!(plan.conflicts[0].other, "Sirens (Original Mix)");
        assert_eq!(
            plan.adds.iter().map(|(f, _)| f.name()).collect::<Vec<_>>(),
            vec!["album"],
            "the blank is still filled in; only the disagreement waits"
        );

        // Unanswered, the kept track's own answer stands.
        let mut quiet = library.clone();
        quiet.merge_copy(keep, other, &HashMap::new());
        assert_eq!(quiet.get(keep).unwrap().title, "Sirens");
        assert_eq!(quiet.get(keep).unwrap().album, "Livity Sound");

        // Answered the other way, the copy's does.
        library.merge_copy(keep, other, &HashMap::from([(Field::Title, Side::Other)]));
        assert_eq!(library.get(keep).unwrap().title, "Sirens (Original Mix)");
    }

    #[test]
    fn the_listening_and_the_playlists_come_across() {
        let mut library = Library::new();
        let keep = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let other = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        {
            let track = library.get_mut(other).unwrap();
            track.analyzed = true;
            track.bpm = 128.5;
            track.key = "8A".into();
            track.has_grid = true;
            track.cues =
                vec![CueMark { letter: 1, time_ms: 1000, label: String::new(), color: [1, 2, 3] }];
        }
        library.add_playlist("Saturday peak", "").unwrap();
        library.playlists[0].tracks.push(other);

        let plan = library.plan_merge(keep, other);
        assert!(plan.is_clean(), "an unanalysed track has no opinion to contradict");
        library.merge_copy(keep, other, &HashMap::new());

        let kept = library.get(keep).unwrap();
        assert!(kept.analyzed, "the hours of listening were thrown away");
        assert_eq!(kept.bpm, 128.5);
        assert_eq!(kept.key, "8A");
        assert_eq!(kept.cues.len(), 1, "the cues came with it");
        assert_eq!(
            library.playlists[0].tracks,
            vec![keep],
            "the playlist was left pointing at the copy that is about to go"
        );
    }

    #[test]
    fn two_analyses_of_the_same_audio_are_not_a_question() {
        // They measured identical bytes. Any difference between them is noise,
        // and asking somebody to choose between two noises is not a question.
        let mut library = Library::new();
        let keep = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let other = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        for (id, bpm) in [(keep, 128.02), (other, 128.03)] {
            let track = library.get_mut(id).unwrap();
            track.analyzed = true;
            track.bpm = bpm;
        }

        let plan = library.plan_merge(keep, other);
        assert!(plan.is_clean(), "{:?}", plan.conflicts);
        assert!(
            !plan.adds.iter().any(|(field, _)| *field == Field::Analysis),
            "the kept track's own listening should stand"
        );
        library.merge_copy(keep, other, &HashMap::new());
        assert_eq!(library.get(keep).unwrap().bpm, 128.02);
    }

    #[test]
    fn a_name_ending_in_a_number_in_brackets_is_a_copy_of_something() {
        let of = |name: &str| {
            name_without_copy_number(Path::new(name)).map(|path| path.display().to_string())
        };
        assert_eq!(of("/music/track_04 (1).flac"), Some("/music/track_04.flac".into()));
        assert_eq!(of("/music/track_04 (12).mp3"), Some("/music/track_04.mp3".into()));
        // The extension is the copy's own: it collided with a different format.
        assert_eq!(of("/music/track_04 (3).m4a"), Some("/music/track_04.m4a".into()));
        assert_eq!(of("/music/Sirens(2).flac"), Some("/music/Sirens.flac".into()));

        // Not everything in brackets is a copy number.
        assert_eq!(of("/music/Untitled (1994).flac"), None, "a year is not a copy count");
        assert_eq!(of("/music/Sirens (Original Mix).flac"), None);
        assert_eq!(of("/music/Sirens.flac"), None);
        assert_eq!(of("/music/(2).flac"), None, "a number alone leaves nothing to rename to");
    }

    #[test]
    fn the_copy_whose_name_says_it_is_a_copy_is_not_the_one_kept() {
        let mut library = Library::new();
        let numbered = copy_of(&mut library, "/music/track_04 (1).flac", "F1", "A1");
        let original = copy_of(&mut library, "/music/track_04.flac", "F2", "A1");
        for id in [numbered, original] {
            let track = library.get_mut(id).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
        }

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(
            groups[0].keep, original,
            "the file whose name says a copier made it was kept over the one it was made from"
        );
    }

    #[test]
    fn knowing_more_still_beats_having_a_tidier_name() {
        let mut library = Library::new();
        let numbered = copy_of(&mut library, "/music/track_04 (1).flac", "F1", "A1");
        let bare = copy_of(&mut library, "/music/track_04.flac", "F2", "A1");
        {
            let track = library.get_mut(numbered).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            track.album = "Livity Sound".into();
            track.year = Some(2019);
        }
        library.get_mut(bare).unwrap().artist = "Peverelist".into();

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(
            groups[0].keep, numbered,
            "the name is a hint about where a file came from, not about what it knows"
        );
    }

    #[test]
    fn the_copy_that_knows_the_most_is_kept_wherever_it_sits() {
        // Where two copies disagree the kept one's answer stands, so the kept
        // one had better be the copy that knows the record — not merely the
        // copy that happens to be in the right folder.
        let mut library = Library::new();
        let bare = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let full = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        {
            let track = library.get_mut(bare).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
        }
        {
            let track = library.get_mut(full).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            track.album = "Livity Sound".into();
            track.year = Some(2019);
            track.tags = vec!["peak".into()];
        }

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(
            groups[0].keep, full,
            "the copy in the library folder was kept though it knows less about the record"
        );
    }

    #[test]
    fn the_library_folder_only_settles_a_tie() {
        let mut library = Library::new();
        let inside = copy_of(&mut library, "/music/a.flac", "F1", "A1");
        let outside = copy_of(&mut library, "/downloads/a.flac", "F2", "A1");
        for id in [inside, outside] {
            let track = library.get_mut(id).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
        }

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(
            groups[0].keep, inside,
            "with nothing to choose between them, keep the one this program looks after"
        );
    }

    #[test]
    fn a_pile_of_tags_is_not_more_than_knowing_what_the_record_is() {
        let mut library = Library::new();
        let noisy = copy_of(&mut library, "/downloads/a.flac", "F1", "A1");
        let known = copy_of(&mut library, "/downloads/b.flac", "F2", "A1");
        library.get_mut(noisy).unwrap().tags = (0..8).map(|n| format!("tag{n}")).collect();
        {
            let track = library.get_mut(known).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            track.album = "Livity Sound".into();
            track.year = Some(2019);
        }

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(groups[0].keep, known, "eight tags outweighed knowing the record");

        // But between two copies that know the same kinds of thing, more of it
        // is more.
        let mut tie = Library::new();
        let one = copy_of(&mut tie, "/downloads/a.flac", "F1", "A1");
        let two = copy_of(&mut tie, "/downloads/b.flac", "F2", "A1");
        tie.get_mut(one).unwrap().tags = vec!["peak".into()];
        tie.get_mut(two).unwrap().tags = vec!["peak".into(), "warmup".into()];
        assert_eq!(tie.duplicate_groups(Path::new("/music"))[0].keep, two);
    }

    #[test]
    fn identical_files_and_matching_audio_are_told_apart() {
        let mut library = Library::new();
        let a = copy_of(&mut library, "/music/a.flac", "FILE1", "AUDIO1");
        let b = copy_of(&mut library, "/downloads/a.flac", "FILE1", "AUDIO1");
        let c = copy_of(&mut library, "/music/a-retagged.flac", "FILE2", "AUDIO1");
        let alone = copy_of(&mut library, "/music/other.flac", "FILE3", "AUDIO2");

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(groups.len(), 1, "the three copies are one group, not two overlapping ones");
        let group = &groups[0];
        assert_eq!(group.keep, a, "the copy inside the library folder is the one kept");
        assert_eq!(group.rest.len(), 2);

        let of = |wanted: u32| group.rest.iter().find(|copy| copy.id == wanted).expect("missing");
        assert!(of(b).identical, "b is byte-for-byte a, and deleting it loses nothing");
        assert!(!of(c).identical, "c is the same recording with different tags");
        assert!(!group.rest.iter().any(|copy| copy.id == alone), "a track with no twin");
    }

    #[test]
    fn audio_that_matches_without_the_files_matching_is_its_own_group() {
        // The case the file hash cannot see: one rip, tagged twice.
        let mut library = Library::new();
        let a = copy_of(&mut library, "/music/a.flac", "FILE1", "AUDIO1");
        let b = copy_of(&mut library, "/music/a-copy.flac", "FILE2", "AUDIO1");

        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].keep, a);
        assert_eq!(groups[0].rest.len(), 1);
        assert_eq!(groups[0].rest[0].id, b);
        assert!(!groups[0].rest[0].identical, "the files differ, so deleting loses its tags");
    }

    #[test]
    fn the_copy_worth_keeping_is_the_one_with_the_work_in_it() {
        let mut library = Library::new();
        let bare = copy_of(&mut library, "/elsewhere/a.flac", "F", "A");
        let named = copy_of(&mut library, "/elsewhere/b.flac", "F", "A");
        {
            let track = library.get_mut(named).unwrap();
            track.artist = "Peverelist".into();
            track.title = "Sirens".into();
            track.tags = vec!["peak".into()];
        }

        // Neither is in the library folder, so what decides is which one would
        // cost something to lose.
        let groups = library.duplicate_groups(Path::new("/music"));
        assert_eq!(groups[0].keep, named);
        assert_eq!(groups[0].rest.len(), 1);
        assert_eq!(groups[0].rest[0].id, bare);
    }

    #[test]
    fn a_track_with_no_hash_is_nobodys_duplicate() {
        // An unreadable file has bigger problems than being a copy, and an
        // empty hash matching another empty one would group every one of them
        // together and offer to delete them.
        let mut library = Library::new();
        copy_of(&mut library, "/music/a.flac", "", "");
        copy_of(&mut library, "/music/b.flac", "", "");
        assert!(library.duplicate_groups(Path::new("/music")).is_empty());
    }

    #[test]
    fn a_folder_can_exist_before_anything_is_in_it() {
        // Making the folder and then filling it is the order people work in.
        let mut library = Library::new();
        library.add_folder("Sat 14/9").unwrap();
        let tree = library.playlist_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].0, "Sat 14/9");
        assert!(tree[0].1.is_empty());

        assert!(library.add_folder("Sat 14/9").is_err(), "one folder, not two of a name");
        assert!(library.add_folder("   ").is_err(), "a folder needs a name");
    }

    #[test]
    fn renaming_a_playlist_takes_the_drives_that_carry_it_along() {
        // A drive names its playlists by name, so a rename that did not follow
        // through would leave it pointing at nothing — and the next sync would
        // read that as "everything on this stick should go".
        let mut library = Library::new();
        library.add_playlist("peak", "").unwrap();
        library.drives.push(Drive {
            label: "SANDISK".into(),
            playlists: vec!["peak".into()],
            ..Drive::default()
        });

        library.rename_playlist("peak", "peak time").unwrap();
        assert_eq!(library.playlists[0].name, "peak time");
        assert_eq!(library.drives[0].playlists, vec!["peak time".to_string()]);

        library.add_playlist("warm", "").unwrap();
        assert!(library.rename_playlist("warm", "peak time").is_err(), "two of a name");
    }

    #[test]
    fn deleting_a_folder_keeps_what_was_filed_in_it() {
        // The playlists are the work; the folder is only where they were put.
        let mut library = Library::new();
        library.add_folder("Sat 14/9").unwrap();
        library.add_playlist("warm", "Sat 14/9").unwrap();

        library.remove_folder("Sat 14/9");
        assert_eq!(library.playlists.len(), 1, "the playlist survives its folder");
        assert_eq!(library.playlists[0].folder, "", "and comes back to the top level");
        assert!(library.folders.is_empty());
    }

    #[test]
    fn deleting_a_playlist_takes_it_off_the_drives_but_not_out_of_the_collection() {
        let mut library = Library::new();
        let id = library.add(Path::new("/music/a.flac"));
        library.add_playlist("peak", "").unwrap();
        library.playlists[0].tracks.push(id);
        library.drives.push(Drive { playlists: vec!["peak".into()], ..Drive::default() });

        library.remove_playlist("peak");
        assert!(library.playlists.is_empty());
        assert!(library.drives[0].playlists.is_empty(), "the drive stops asking for it");
        assert!(library.get(id).is_some(), "a playlist is not where the music is kept");
    }

    #[test]
    fn a_drive_written_before_playlists_were_plural_still_names_its_own() {
        // Older collections carry the single `playlist` field. Reading it here
        // rather than rewriting the file on load means such a collection still
        // opens in the build that wrote it.
        let old = Drive { playlist: "Sat 14/9".into(), ..Drive::default() };
        assert_eq!(old.playlist_names(), vec!["Sat 14/9".to_string()]);

        let new = Drive { playlists: vec!["warm".into(), "peak".into()], ..Drive::default() };
        assert_eq!(new.playlist_names(), vec!["warm".to_string(), "peak".to_string()]);

        assert!(Drive::default().playlist_names().is_empty());
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
        track.has_grid = true;

        // A format nothing opens, and a purchase nothing here can convert.
        track.format = "ogg".into();
        assert!(track.needs_attention().unwrap().contains("not a format a player opens"));
        track.format = "m4a".into();
        assert_eq!(track.needs_attention(), None, "a CDJ-3000 plays AAC and ALAC");

        track.protected = true;
        let said = track.needs_attention().unwrap();
        assert!(said.contains("protected"), "{said}");
        assert!(said.contains("cannot convert it"), "and it should not offer to: {said}");
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
        assert_eq!(energy_from(0.015), 1);
        assert_eq!(energy_from(0.07), 3);
        assert_eq!(energy_from(0.30), 5);
        // Monotonic, which is the only property the meter really promises.
        let mut previous = 0;
        for step in 1..400 {
            let level = energy_from(step as f32 / 1000.0);
            assert!(level >= previous, "energy went down at {step}");
            assert!((1..=5).contains(&level), "off the meter at {step}: {level}");
            previous = level;
        }
    }

    #[test]
    fn nothing_measured_is_not_the_quietest_possible_record() {
        // Zero is what an unanalysed track carries. Reading it as a rank of
        // one would put every track still to be listened to at the tool end of
        // the crate, where it looks like an answer.
        assert_eq!(energy_from(0.0), 0);
        assert_eq!(energy_from(f32::NAN), 0);
    }

    #[test]
    fn stem_envelopes_survive_the_cache() {
        let dir = scratch("envelopes");
        // SAFETY: single-threaded test; the variable is only read to place the
        // cache somewhere disposable.
        unsafe { std::env::set_var("BOOTH_DATA_DIR", &dir) };

        let envelopes = crate::wave::StemEnvelopes {
            vocals: vec![1, 2, 3, 4],
            melody: vec![5, 6, 7, 8],
            drums: vec![9, 10, 11, 12],
        };
        cache_envelopes(7, &envelopes).unwrap();
        assert_eq!(cached_envelopes(7), Some(envelopes));
        assert_eq!(cached_envelopes(8), None, "nothing was written for that one");

        // A file of the wrong length is from another version or a half-done
        // write; there is nothing to salvage from part of it.
        std::fs::write(envelopes_path(9), b"short").unwrap();
        assert_eq!(cached_envelopes(9), None);

        unsafe { std::env::remove_var("BOOTH_DATA_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ragged_envelopes_are_cached_as_the_part_that_lines_up() {
        let dir = scratch("envelopes-ragged");
        unsafe { std::env::set_var("BOOTH_DATA_DIR", &dir) };

        // One stem measured shorter than the others: what is kept is the part
        // all three agree on, because a colour needs all three.
        let envelopes = crate::wave::StemEnvelopes {
            vocals: vec![1, 2, 3],
            melody: vec![4, 5],
            drums: vec![6, 7, 8, 9],
        };
        cache_envelopes(1, &envelopes).unwrap();
        let read = cached_envelopes(1).unwrap();
        assert_eq!(read.columns(), 2);
        assert_eq!(read.vocals, vec![1, 2]);
        assert_eq!(read.drums, vec![6, 7]);

        unsafe { std::env::remove_var("BOOTH_DATA_DIR") };
        std::fs::remove_dir_all(&dir).unwrap();
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
