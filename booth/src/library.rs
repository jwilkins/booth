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

    /// Drop a part the kit lists but the disk does not have.
    ///
    /// A kit naming a file that is not there is worse than an empty one: the
    /// browser offers an acapella row that cannot be played, and a drive write
    /// fails on it in the middle of the write.
    pub fn forget(&mut self, part: &str) {
        match part {
            "vocals" => self.vocals = None,
            "drums" => self.drums = None,
            "melody" => self.melody = None,
            _ => {}
        }
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
    ///
    /// Says whether it took any, so a caller that is about to save the
    /// collection can tell a kit that arrived from one that was already there.
    pub fn fill_from(&mut self, other: &StemKit) -> bool {
        let mut took = false;
        for (mine, theirs) in [
            (&mut self.vocals, &other.vocals),
            (&mut self.drums, &other.drums),
            (&mut self.melody, &other.melody),
        ] {
            if mine.is_none() && theirs.is_some() {
                mine.clone_from(theirs);
                took = true;
            }
        }
        took
    }

    /// Whether `path` is one of the parts this kit names.
    ///
    /// Asked when a drive's record is matched back against the collection: a
    /// re-rendered kit is a different set of files, and a row kept for a stem
    /// the kit no longer names would describe a drive that has moved on.
    pub fn has(&self, path: &Path) -> bool {
        self.each().into_iter().any(|(_, part)| part.is_some_and(|part| part == path))
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

impl Phrase {
    /// The names a section can have, in the order a track tends to use them.
    ///
    /// The same five the detector produces, because the strip has to mean one
    /// thing whether a section was measured or moved by hand.
    pub const KINDS: [&'static str; 5] = ["intro", "build", "break", "drop", "outro"];

    pub fn len_ms(&self) -> u32 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Four, and the picture, the transport and the phrase strip all count in it.
///
/// Named rather than written out at each of the three, because the bar is the
/// one number they have to agree on: a red mark every four beats, a bar number
/// that advances every four, and a section length in fours are the same claim
/// made three times, and they disagreed once already.
pub const BEATS_PER_BAR: usize = 4;

/// The shortest a section may be left.
///
/// A boundary dragged past its neighbour would invert the section, and one
/// dragged to within a pixel of it leaves a sliver nobody can grab again — so
/// the drag stops here instead. A second is well under a bar at any tempo
/// anybody plays, so it only ever bites at the very end of a drag.
pub const MIN_PHRASE_MS: u32 = 1_000;

/// What the phrase strip was asked to do to a track's sections.
///
/// The positions are already snapped by the time they arrive: where a boundary
/// may land is a question about the grid, and the grid is the window's to know.
#[derive(Clone, Debug, PartialEq)]
pub enum PhraseEdit {
    /// Move the boundary that starts phrase `at`.
    Move { at: usize, time_ms: u32 },
    /// Cut phrase `at` in two here. Both halves keep the name until one is
    /// given another — a split is somebody saying the detector missed a
    /// boundary, not that it got the name wrong.
    Split { at: usize, time_ms: u32 },
    /// Fold phrase `at` into the one before it, which is how a boundary is
    /// taken away.
    Merge { at: usize },
    /// Rename phrase `at`.
    Name { at: usize, kind: String },
}

/// Apply an edit, and say whether anything actually moved.
///
/// The strip stays what it was: sections in order, none of them inside
/// another, together covering exactly what they covered before. Every edit is
/// local — it moves one boundary, adds one, or takes one away — so a
/// hand-corrected strip never has to be re-derived from anything.
///
/// A position that cannot be honoured is clamped; an edit that cannot be
/// honoured at all is refused. A boundary dragged past its neighbour stops at
/// the floor, but a split of a section too short to have two halves is not a
/// smaller split, it is nothing.
pub fn edit_phrases(phrases: &mut Vec<Phrase>, edit: &PhraseEdit) -> bool {
    match edit {
        PhraseEdit::Name { at, kind } => match phrases.get_mut(*at) {
            Some(phrase) if phrase.kind != *kind => {
                phrase.kind = kind.clone();
                true
            }
            _ => false,
        },

        PhraseEdit::Move { at, time_ms } => {
            let Some(before) = at.checked_sub(1) else { return false };
            let (Some(previous), Some(after)) = (phrases.get(before), phrases.get(*at)) else {
                return false;
            };
            let Some(landed) = room(previous.start_ms, after.end_ms, *time_ms) else {
                return false;
            };
            if landed == previous.end_ms && landed == after.start_ms {
                return false;
            }
            phrases[before].end_ms = landed;
            phrases[*at].start_ms = landed;
            true
        }

        PhraseEdit::Split { at, time_ms } => {
            let Some(phrase) = phrases.get(*at) else { return false };
            let Some(landed) = room(phrase.start_ms, phrase.end_ms, *time_ms) else {
                return false;
            };
            let first =
                Phrase { start_ms: phrase.start_ms, end_ms: landed, kind: phrase.kind.clone() };
            phrases[*at].start_ms = landed;
            phrases.insert(*at, first);
            true
        }

        PhraseEdit::Merge { at } => {
            let Some(before) = at.checked_sub(1) else { return false };
            let Some(end) = phrases.get(*at).map(|phrase| phrase.end_ms) else { return false };
            let Some(previous) = phrases.get_mut(before) else { return false };
            previous.end_ms = end;
            phrases.remove(*at);
            true
        }
    }
}

/// Where a boundary may land between `from` and `to`, leaving a section either
/// side of it. `None` when there is not room for both.
fn room(from: u32, to: u32, wanted: u32) -> Option<u32> {
    let floor = from + MIN_PHRASE_MS;
    let ceiling = to.checked_sub(MIN_PHRASE_MS)?;
    (floor <= ceiling).then(|| wanted.clamp(floor, ceiling))
}

/// One line of a track's words, with when it was sung.
///
/// Kept in the collection rather than re-read, because getting it costs a stem
/// render and a pass through a speech recogniser — minutes a track — and
/// because a DJ who has seen what a cue says should be able to see it again
/// without paying for it twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyric {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

/// Where a track's words came from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordsFrom {
    /// Nothing has read them, or they were read before this was recorded.
    #[default]
    Unsaid,
    /// Heard off the vocal stem by a speech recogniser.
    Recogniser,
    /// Looked up, under a name a fingerprint or the file's tags gave.
    Server,
    /// Typed or corrected by hand, which outranks both.
    ByHand,
}

impl WordsFrom {
    /// What to call it on screen.
    pub fn label(self) -> &'static str {
        match self {
            WordsFrom::Unsaid => "",
            WordsFrom::Recogniser => "heard off the stem",
            WordsFrom::Server => "looked up",
            WordsFrom::ByHand => "corrected by hand",
        }
    }
}

/// A line the track keeps coming back to, and every time it comes round.
///
/// The output of the frequency analysis over a track's words: the lines that
/// are sung more than once, most repeated first. Which is what tells one
/// record from another months later — a DJ remembers the line the room sings,
/// not the file name.
///
/// Kept in the collection rather than worked out where it is wanted. Grouping
/// compares every line against every other, and the two places that show this
/// redraw sixty times a second.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refrain {
    /// The wording it was heard with most often.
    pub text: String,
    /// Where it comes round, in milliseconds, in time order. Places, not
    /// hearings: a line sung three times over in one breath lands once.
    pub at: Vec<u32>,
    /// How many times it is sung in all, which is not how many places it
    /// lands. See `booth_cli::transcribe::Refrain::times`.
    ///
    /// Zero on a collection stored before the two were told apart; `times`
    /// falls back to the places, which is what it used to mean.
    #[serde(default)]
    pub heard: usize,
}

impl Refrain {
    /// How many times it is sung.
    pub fn times(&self) -> usize {
        self.heard.max(self.at.len())
    }
}

/// The language this collection is mostly in, if it is mostly in one.
///
/// A recogniser left to itself guesses the language from the first few seconds
/// of what it is handed, and the first few seconds of an isolated vocal are
/// usually a breath. The guess is wrong often enough to matter and wrong in an
/// expensive way: it transcribes an English record as though it were Welsh and
/// the words are nonsense, which cues nonsense.
///
/// A library knows better than that, because it has been round this already.
/// What the recogniser decided on everything read so far is the best available
/// hint for the next one — so this is the collection answering its own
/// question, and it only answers where there is a clear majority: a shelf
/// that is genuinely half German is one where guessing per track is right.
pub fn common_language(tracks: &[Track]) -> Option<String> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for track in tracks {
        let said = track.language.trim();
        if !said.is_empty() {
            *counts.entry(said).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    if total < ENOUGH_TO_SAY {
        return None;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count * 2 > total)
        .map(|(language, _)| language.to_string())
}

/// How many tracks have to have been read before the collection's own answer
/// is worth preferring to the recogniser's guess.
const ENOUGH_TO_SAY: usize = 8;

/// What a track's words keep coming back to.
///
/// The one place this is worked out, so that the stored answer and the words
/// it came from cannot mean two different things.
pub fn refrains_from(lyrics: &[Lyric]) -> Vec<Refrain> {
    transcript(lyrics)
        .repeated()
        .into_iter()
        .map(|refrain| Refrain { heard: refrain.times(), text: refrain.text, at: refrain.at })
        .collect()
}

/// What a track's words say, as the comment a player shows.
///
/// Both drive databases carry one free-text field per track — `djComment` in
/// the OneLibrary one, the twenty-first string in a pdb row — and a CDJ shows
/// it in the browser and on the track info screen. It has been empty on every
/// drive this program has ever written, which is a waste of the one place the
/// words could be read in a booth.
///
/// The lines the track keeps coming back to go first, with how often each is
/// sung, and the whole lyric follows after a blank line. That order is not
/// decoration: the comment shares a 4 kB row with the path and the title, and
/// anything past what fits is cut off the end — so what a long lyric loses is
/// its last verse, and never the line that identifies the record.
pub fn words_as_comment(lyrics: &[Lyric], refrains: &[Refrain]) -> String {
    if lyrics.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for refrain in refrains {
        out.push_str(&format!("{}\u{d7} {}\n", refrain.times(), refrain.text.trim()));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    for line in lyrics {
        let said = line.text.trim();
        if !said.is_empty() {
            out.push_str(said);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// The words as the engine wants them, for finding what repeats in them.
pub fn transcript(lyrics: &[Lyric]) -> booth_cli::transcribe::Transcript {
    booth_cli::transcribe::Transcript {
        lines: lyrics
            .iter()
            .map(|line| booth_cli::transcribe::Line {
                start_ms: line.start_ms,
                end_ms: line.end_ms,
                text: line.text.clone(),
            })
            .collect(),
        ..Default::default()
    }
}

/// And back, for storing what one came home with.
pub fn lyrics_from(transcript: &booth_cli::transcribe::Transcript) -> Vec<Lyric> {
    transcript
        .lines
        .iter()
        .map(|line| Lyric { start_ms: line.start_ms, end_ms: line.end_ms, text: line.text.clone() })
        .collect()
}

/// Make what a player left on a drive the collection's copy.
///
/// The other direction from the one everything else here goes in: a CDJ-3000X
/// moved a cue or re-gridded a track on the deck, somebody has said that is the
/// copy to keep, and the collection has to be able to show it or the decision
/// means nothing.
///
/// The grid itself is not taken, only the tempo it was written at. A collection
/// keeps a tempo and a downbeat rather than thousands of beat times — see
/// [`crate::app`]'s `beat_times` — so a variable grid cannot be held here
/// without being flattened, and flattening it and writing it back would lose
/// exactly what was being protected. This is why a track whose drive copy is
/// kept is not written again: the files on the stick stay as the player left
/// them, and what comes back here is what can be shown beside them.
///
/// Returns whether anything changed.
pub fn take_prep(track: &mut Track, found: &booth_cli::rekordbox::anlz::Analysis) -> bool {
    let before = (
        track.bpm,
        track.cues.clone(),
        track.phrases.clone(),
        track.beats,
        track.beat_ms.clone(),
        track.downbeat_ms,
    );

    if let Some(bpm) = found.bpm() {
        track.bpm = bpm;
    }
    if !found.grid.beats.is_empty() {
        track.beats = found.grid.beats.len();
        track.has_grid = true;
        // A grid a tempo cannot describe is kept beat for beat. A player bends
        // grids by hand and this is the only copy of that work: rebuilding it
        // from the tempo on the way back out would hand the drive a flattened
        // version of what it just gave us.
        track.beat_ms = match found.grid.bends() {
            true => found.grid.times_from_downbeat(),
            false => Vec::new(),
        };
        // Where the drive says the one is. Phasing an even grid off the memory
        // cue instead is a guess that happens to be right when the cue is on a
        // downbeat, and silently wrong when it is not.
        track.downbeat_ms = found.grid.times_from_downbeat().first().copied();
    }

    track.cues = found
        .cues
        .iter()
        .map(|cue| CueMark {
            letter: cue.hot_cue,
            time_ms: cue.time_ms,
            label: cue.comment.clone().unwrap_or_default(),
            color: cue
                .color
                .map(|rgb| [rgb.r, rgb.g, rgb.b])
                .unwrap_or_else(|| crate::job::cue_color(cue.hot_cue)),
        })
        .collect();
    track.cues.sort_by_key(|cue| (cue.letter, cue.time_ms));

    if let Some(structure) = &found.structure {
        let ends = track.duration_secs.max(0.0) * 1000.0;
        let mut phrases = Vec::with_capacity(structure.phrases.len());
        for (index, phrase) in structure.phrases.iter().enumerate() {
            let Some(kind) = booth_cli::analysis::structure::Kind::from_id(phrase.kind) else {
                continue;
            };
            let Some(start_ms) = found.time_of(phrase.beat) else { continue };
            // A section runs to the next one, and the last runs to wherever
            // the phrases say the track ends — or to the end of the audio when
            // the grid is shorter than the record, which it is for a track
            // that fades out past its last beat.
            let end_ms = structure
                .phrases
                .get(index + 1)
                .and_then(|next| found.time_of(next.beat))
                .or_else(|| found.time_of(structure.end_beat))
                .unwrap_or(ends as u32)
                .max(start_ms);
            phrases.push(Phrase { start_ms, end_ms, kind: kind.label().to_string() });
        }
        track.phrases = phrases;
    }

    let changed = before
        != (
            track.bpm,
            track.cues.clone(),
            track.phrases.clone(),
            track.beats,
            track.beat_ms.clone(),
            track.downbeat_ms,
        );
    if changed {
        track.edited = Some(now());
    }
    changed
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
    /// `booth_cli::analysis::peak_intensity` reports. Kept so that the rank can
    /// be argued with — five bars hide whether a track sat just under a
    /// threshold or nowhere near one.
    #[serde(default)]
    pub intensity: f32,
    pub beats: usize,
    /// Every beat, for a track whose grid bends.
    ///
    /// Empty for almost every record, and that is not a track without a grid:
    /// a grid at one steady tempo is a tempo and a downbeat, and the picture
    /// rebuilds it from those two. It fills in only where those two cannot say
    /// it — a live take, a disco record, one somebody bent by hand on a player
    /// — because that grid cannot be rebuilt, and dropping it would flatten
    /// their work the next time a drive is written.
    ///
    /// From the first downbeat on, like the grid the picture rebuilds, so that
    /// everything counting in fours counts from the same beat either way.
    ///
    /// What goes on the drive is every beat regardless: a player reads the
    /// beats, not the tempo.
    #[serde(default)]
    pub beat_ms: Vec<u32>,
    /// Where the first downbeat of an even grid falls.
    ///
    /// `None` falls back to the memory cue, which is where the phase came from
    /// before there was a field for it — so a collection written by an older
    /// build keeps the grid it had.
    ///
    /// It has a field of its own because saying "the one is here" and saying
    /// "start the track here" are two different things a DJ does, and folding
    /// them together means setting the grid quietly moves a cue. Ignored where
    /// [`Track::beat_ms`] has the beats, since then there is nothing to phase.
    #[serde(default)]
    pub downbeat_ms: Option<u32>,
    pub phrases: Vec<Phrase>,
    pub cues: Vec<CueMark>,
    /// What is sung, and when, once the vocal stem has been through a speech
    /// recogniser. Empty on a track nobody has asked about, on an instrumental,
    /// and on a track whose words nothing could make out — which are three
    /// different things the inspector is careful to tell apart.
    #[serde(default)]
    pub lyrics: Vec<Lyric>,
    /// The lines those words keep coming back to, most repeated first.
    ///
    /// Worked out from `lyrics` and kept beside them, by [`refrains_from`].
    /// Rebuilt whenever the words change and filled in on load for a
    /// collection transcribed before this existed, so the two cannot drift.
    #[serde(default)]
    pub refrains: Vec<Refrain>,
    /// Whether those words have been put where the singing actually is.
    ///
    /// Whisper's timing on a stem is wrong in a measurable way — see
    /// `booth_cli::transcribe::align` — so the lines are moved to the onsets
    /// measured off the stem itself. `false` on a collection transcribed before
    /// that existed, which is the whole point of the field: the words are fine
    /// and only the times are wrong, so those tracks want a second of decoding
    /// rather than minutes of listening all over again.
    #[serde(default)]
    pub lyrics_aligned: bool,
    /// Whether this track has been found to have no singing on it.
    ///
    /// Different from having no words *yet*, which is what empty `lyrics`
    /// means on its own, and the difference is minutes: a track nobody has
    /// asked about is worth a separation and a pass through the recogniser,
    /// and one already known to be an instrumental is worth neither. Without
    /// it, every press of Words paid for the same answer again.
    #[serde(default)]
    pub instrumental: bool,
    /// How sure the recogniser was of the words, from 0 to 1.
    ///
    /// `None` on words nothing has said anything about — read before this
    /// existed, or by a recogniser that does not report it. See
    /// `booth_cli::transcribe::Transcript::confidence` for what the number is
    /// and, more to the point, what it is not: a threshold anything is
    /// dropped on.
    #[serde(default)]
    pub heard_surely: Option<f32>,
    /// Where this track's words came from.
    ///
    /// Worth recording because the two are not alike. A recogniser's words are
    /// a machine's best guess at a vocal and are wrong in ways that read like
    /// lyrics; a lyrics server's are what somebody wrote down. Somebody
    /// looking at a doubtful cue should be able to tell which they are
    /// looking at.
    #[serde(default)]
    pub words_from: WordsFrom,
    /// The language the recogniser decided it was listening to, as a code
    /// like `en`. Empty where nothing has said.
    #[serde(default)]
    pub language: String,
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
    /// When the prep last changed here, in seconds since the epoch.
    ///
    /// Not when the record changed — a tag, a rating or a play count is not
    /// something a player holds an opinion about. This is the grid, the cues,
    /// the phrases and the key: the things a drive carries and a CDJ-3000X can
    /// also edit. When both have moved since the last sync, this is one half of
    /// saying which is the newer, and the other half is on the drive.
    ///
    /// `None` on a track nothing has edited, which is not the same as one
    /// edited long ago.
    #[serde(default)]
    pub edited: Option<u64>,
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
    /// How sure the fingerprint was, from 0 to 1, where one named this track.
    ///
    /// Kept rather than thrown away with the match, because it answers a
    /// question that comes up later: whether the artist and title are an
    /// answer or a guess off a file name. A lyrics server is asked under
    /// those names, so what comes back is only this record's words if they
    /// were this record's names.
    #[serde(default)]
    pub identified_surely: Option<f64>,
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
            beat_ms: Vec::new(),
            downbeat_ms: None,
            phrases: Vec::new(),
            cues: Vec::new(),
            lyrics: Vec::new(),
            refrains: Vec::new(),
            lyrics_aligned: true,
            instrumental: false,
            heard_surely: None,
            words_from: WordsFrom::default(),
            language: String::new(),
            edited: None,
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
            identified_surely: None,
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
    pub fn fill_from(&mut self, from: &booth_cli::rekordbox::master::Track) -> bool {
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
    pub fn incompatibility(&self) -> Option<booth_cli::compat::Problem> {
        use booth_cli::compat::{Player, Problem};
        // The writer's limits, not the drive's target. A format an older deck
        // will not open is re-encoded on the way onto the stick, so it is
        // something the sync sheet mentions and not something wrong with the
        // track — and a sidebar that counted it would be telling somebody to
        // go and fix a library that is already fine.
        //
        // What is left is what no amount of re-encoding reaches: a container
        // nothing decodes, a purchase nothing can lawfully open, float samples,
        // and a rate past what the writer itself accepts.
        let player = Player::ALL[0];
        if self.protected {
            return Some(Problem::Protected);
        }
        // An empty format is a record nothing has looked at yet, not a file in
        // a format nothing opens. Reporting the first as the second would put
        // every freshly added track in the attention list.
        if !self.format.is_empty() && !player.opens(&self.format) {
            return Some(Problem::Format { extension: self.format.clone(), player });
        }
        if self.float_samples {
            return Some(Problem::FloatSamples);
        }
        if self.sample_rate > player.max_sample_rate() {
            return Some(Problem::TooFast { rate: self.sample_rate, player });
        }
        None
    }

    pub fn duration_text(&self) -> String {
        let total = self.duration_secs.round() as u64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// Where one bar of the meter ends and the next begins, in the units
/// `booth_cli::analysis::peak_intensity` reports: onset strength per bin per
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

/// What the drive was holding for one track, the last time anything looked.
///
/// A CDJ-3000X can move a cue, re-grid a track or rename a phrase on the deck,
/// and it writes the result back to the stick. Nothing here can tell what it
/// changed — reading a player's edits back is not something this build does —
/// but it can tell *that* something did, which is the difference between
/// overwriting somebody's work in silence and asking first.
///
/// Two independent pieces of evidence, because neither is enough alone. The
/// counters are the field the format keeps for exactly this question and are
/// the right thing to read; what a player actually writes into them is not
/// documented and nobody has published a reading of one, so a drive that shows
/// no change there has not said it was not edited. The analysis files cannot
/// argue: a player that rewrote a track's cues rewrote the file that holds
/// them, whatever the database says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The track's analysis files, as `name:size:modified`, in the order
    /// [`crate::sync::analysis_files`] looks for them.
    pub files: Vec<String>,
    /// `hasModified`, `cueUpdateCount`, `analysisDataUpdateCount` and
    /// `informationUpdateCount` from the OneLibrary row, when the drive has one
    /// and it could be opened. Empty when it could not, which is not the same
    /// as four zeroes.
    #[serde(default)]
    pub counts: Vec<i64>,
    /// The newest modification time among those files, in seconds since the
    /// epoch. What the drive's side of "which was edited more recently" is.
    #[serde(default)]
    pub at: Option<u64>,
}

impl Stamp {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.counts.is_empty()
    }
}

/// One track as it was written to a drive.
///
/// The fingerprint is what makes an update distinguishable from an addition
/// without re-reading the drive: it summarises the prep the player will see, so
/// a moved cue marks the track for rewriting and a play count does not.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub id: u32,
    pub prep: u64,
    /// What the drive was holding for this track when the two last agreed.
    ///
    /// Absent for a drive written before this was recorded, and for a track
    /// whose files could not be found. Absent means no evidence rather than no
    /// change: a drive that cannot be asked is never treated as having
    /// answered, so nothing is refused on the strength of it.
    #[serde(default)]
    pub theirs: Option<Stamp>,
    /// The row this track has in the drive's own database.
    ///
    /// Kept because a second write is only given what changed — preparing a
    /// track means decoding it — and a database built from only that describes
    /// a drive that no longer exists. Handing these back is what lets the
    /// database always describe the whole drive.
    ///
    /// Absent for a drive written before this was recorded, and for a track
    /// whose write failed. Those are written again rather than carried, which
    /// costs a decode and is always correct.
    #[serde(default)]
    pub row: Option<booth_cli::export::pdb::Track>,
    /// The rows this track's stems have in that database, each with the file it
    /// was made from.
    ///
    /// Kept for the same reason as `row`, and only alongside one: a stem takes
    /// its parent's grid, cues, key and phrases, so a parent being written
    /// again is a stem being written again, and a stem row is only still true
    /// while the parent's is.
    ///
    /// Empty for a drive written before this was recorded, for one that does
    /// not carry stems, and for a stem whose write failed — each of which
    /// means preparing them again, which costs three decodes and is always
    /// correct.
    #[serde(default)]
    pub stems: Vec<(PathBuf, booth_cli::export::pdb::Track)>,
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
    /// Whether the stem companions are held back.
    ///
    /// A rendered kit goes on the drive with its track unless this says not to,
    /// because a kit exists only because somebody asked for one and a stick
    /// that quietly leaves it behind is a stick with no acapella in the booth.
    /// The name is the negative one so that a collection written before this —
    /// which stored the question the other way round, and stored it as "no" for
    /// every drive that was never told otherwise — starts carrying them.
    #[serde(default)]
    pub skip_stems: bool,
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
            let _ = kept.stems.fill_from(&copy.stems);
        }
        let kept_path = kept.path.clone();
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
            // Copied across, and noted as being of the file that is staying.
            // These two are the same recording by hash, so the picture is of
            // the right audio; it is not of the right *file*, and a note
            // carried over unchanged would name one that is about to be
            // thrown away.
            let of = [kept_path.clone()];
            if let Some(bands) = cached_waveform(other, &of) {
                let _ = cache_waveform(keep, &of, &bands);
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
        let mut library: Library = serde_json::from_str(&text)?;
        // A collection transcribed before the frequency analysis was kept has
        // the words and not what repeats in them. Worked out here rather than
        // where it is shown, so nothing downstream has to wonder whether the
        // track it was handed has been through this.
        for track in &mut library.tracks {
            if track.refrains.is_empty() && !track.lyrics.is_empty() {
                track.refrains = refrains_from(&track.lyrics);
            }
        }
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

/// The recording a row belongs to: a companion's parent, or the track itself.
///
/// A track and its stems are one recording cut three ways, and several things
/// want to ask whether two rows are parts of the same one — the deck most of
/// all, since switching between them is a comparison rather than a change of
/// record.
pub fn family(id: u32) -> u32 {
    match companion_of(id) {
        Some((parent, _)) => parent,
        None => id,
    }
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

/// Where the note of what a picture was drawn from sits, beside the picture.
fn drawn_from_path(id: u32) -> PathBuf {
    data_dir().join("waveforms").join(format!("{id:08}.from"))
}

/// What a picture was drawn from: the files, and what they looked like.
///
/// `None` when a file cannot be reached, which is not the same as a file that
/// has changed — see [`cached_waveform`].
fn drawn_from(sources: &[PathBuf]) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(sources.len());
    for path in sources {
        let meta = std::fs::metadata(path).ok()?;
        let at = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs())
            .unwrap_or(0);
        lines.push(format!("{}:{}:{at}", path.display(), meta.len()));
    }
    Some(lines.join("\n"))
}

/// Keep a row's picture for next time, with a note of what it is a picture of.
///
/// The note is the whole point. A picture is of a file, but the cache is keyed
/// by row id — so without it a row whose audio changed underneath it goes on
/// showing the old picture for ever, and nothing about the row says so. Two
/// ways that happens: a stem kit rendered again, and a bug that is the reason
/// this exists, where a stem companion was drawn from its parent's mix and the
/// mix's picture was cached under the stem's name.
pub fn cache_waveform(id: u32, sources: &[PathBuf], bands: &[u8]) -> std::io::Result<()> {
    let path = waveform_path(id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bands)?;
    match drawn_from(sources) {
        Some(note) => std::fs::write(drawn_from_path(id), note),
        // Nothing to say what it is of, so nothing is claimed: the next read
        // will find no note and draw it again.
        None => {
            let _ = std::fs::remove_file(drawn_from_path(id));
            Ok(())
        }
    }
}

/// Read a cached picture, if there is one and it is still of these files.
///
/// A cache with no note beside it is one written before there were notes, or
/// one whose note was lost. Either way it cannot be vouched for, so it is
/// redrawn — which is what heals a library full of acapellas showing the mix.
///
/// A file that cannot be reached is a different matter. Nothing can be said
/// about whether the picture is still right, and a row whose audio has been
/// unplugged is better showing the last picture of it than a blank strip.
pub fn cached_waveform(id: u32, sources: &[PathBuf]) -> Option<Vec<u8>> {
    let bands = std::fs::read(waveform_path(id)).ok().filter(|bands| !bands.is_empty())?;
    match drawn_from(sources) {
        Some(now) => {
            let noted = std::fs::read_to_string(drawn_from_path(id)).ok()?;
            (noted == now).then_some(bands)
        }
        None => Some(bands),
    }
}

/// Throw away a row's cached picture.
///
/// For when the audio it was measured from has changed under it — a stem kit
/// rendered again, at a different quality or by a different separator. The
/// picture is of a file, so a new file means a new picture; without this the
/// acapella goes on showing the one it had before, and nothing about the row
/// says it is stale.
pub fn forget_waveform(id: u32) {
    let _ = std::fs::remove_file(waveform_path(id));
    let _ = std::fs::remove_file(drawn_from_path(id));
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
/// Re-exported rather than written again here: the engine's own output counts
/// the same things, and two copies of this would be two places for "1 tracks"
/// to come from.
pub use booth_cli::report::plural;

/// Now, in seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

#[cfg(test)]
mod tests {

    /// A strip of sections back to back, as the detector leaves one.
    fn strip(runs: &[(u32, u32, &str)]) -> Vec<Phrase> {
        runs.iter()
            .map(|(start, end, kind)| Phrase {
                start_ms: *start,
                end_ms: *end,
                kind: (*kind).to_string(),
            })
            .collect()
    }

    fn edges(phrases: &[Phrase]) -> Vec<(u32, u32, String)> {
        phrases.iter().map(|p| (p.start_ms, p.end_ms, p.kind.clone())).collect()
    }

    #[test]
    fn moving_a_boundary_moves_both_sections_that_meet_at_it() {
        // The point of the whole thing: a boundary is one edge shared by two
        // sections, not two edges that happen to line up. Moving it must not
        // leave a gap between them or make them overlap.
        let mut phrases = strip(&[(0, 30_000, "intro"), (30_000, 90_000, "build")]);
        assert!(edit_phrases(&mut phrases, &PhraseEdit::Move { at: 1, time_ms: 40_000 }));
        assert_eq!(
            edges(&phrases),
            vec![(0, 40_000, "intro".to_string()), (40_000, 90_000, "build".to_string())]
        );

        // Nowhere to move to is not a move.
        assert!(!edit_phrases(&mut phrases, &PhraseEdit::Move { at: 1, time_ms: 40_000 }));
        // Neither is the boundary before the first section, which is the start
        // of the track and not a boundary at all.
        assert!(!edit_phrases(&mut phrases, &PhraseEdit::Move { at: 0, time_ms: 5_000 }));
    }

    #[test]
    fn a_boundary_cannot_be_dragged_over_its_neighbour() {
        // Dragged hard to one end it stops with a section still either side.
        // Past it, the section would invert; just short of it, what is left is
        // a sliver too narrow to ever grab again.
        let mut phrases = strip(&[(0, 30_000, "intro"), (30_000, 90_000, "build")]);

        edit_phrases(&mut phrases, &PhraseEdit::Move { at: 1, time_ms: 0 });
        assert_eq!(phrases[0].start_ms, 0);
        assert_eq!(phrases[0].len_ms(), MIN_PHRASE_MS, "the first section was crushed");

        edit_phrases(&mut phrases, &PhraseEdit::Move { at: 1, time_ms: u32::MAX });
        assert_eq!(phrases[1].end_ms, 90_000);
        assert_eq!(phrases[1].len_ms(), MIN_PHRASE_MS, "the second section was crushed");
    }

    #[test]
    fn splitting_puts_a_boundary_where_the_detector_missed_one() {
        // The answer to a strip that is too coarse. Both halves keep the name:
        // a split says the boundary was missed, not that the name was wrong.
        let mut phrases = strip(&[(0, 30_000, "intro"), (30_000, 120_000, "drop")]);
        assert!(edit_phrases(&mut phrases, &PhraseEdit::Split { at: 1, time_ms: 60_000 }));
        assert_eq!(
            edges(&phrases),
            vec![
                (0, 30_000, "intro".to_string()),
                (30_000, 60_000, "drop".to_string()),
                (60_000, 120_000, "drop".to_string()),
            ]
        );

        // And then one of them is given its own name.
        assert!(edit_phrases(&mut phrases, &PhraseEdit::Name { at: 1, kind: "build".to_string() }));
        assert_eq!(phrases[1].kind, "build");
        assert!(!edit_phrases(
            &mut phrases,
            &PhraseEdit::Name { at: 1, kind: "build".to_string() }
        ));
    }

    #[test]
    fn a_section_with_no_room_for_two_halves_is_not_split() {
        // Refused rather than made smaller. A split that leaves a sliver is
        // not a smaller version of what was asked for.
        let mut phrases = strip(&[(0, MIN_PHRASE_MS + 500, "intro")]);
        assert!(!edit_phrases(&mut phrases, &PhraseEdit::Split { at: 0, time_ms: 500 }));
        assert_eq!(phrases.len(), 1, "a sliver was left behind");
    }

    #[test]
    fn merging_takes_a_boundary_away_without_leaving_a_hole() {
        // How a boundary is deleted: the section before it swallows it, so the
        // strip still covers exactly what it covered.
        let mut phrases =
            strip(&[(0, 30_000, "intro"), (30_000, 60_000, "build"), (60_000, 90_000, "drop")]);
        assert!(edit_phrases(&mut phrases, &PhraseEdit::Merge { at: 1 }));
        assert_eq!(
            edges(&phrases),
            vec![(0, 60_000, "intro".to_string()), (60_000, 90_000, "drop".to_string())]
        );

        // There is nothing before the first section to merge it into.
        assert!(!edit_phrases(&mut phrases, &PhraseEdit::Merge { at: 0 }));
    }

    #[test]
    fn an_edited_strip_still_covers_the_track() {
        // The invariant behind all of it, checked after a run of edits rather
        // than one at a time: in order, meeting exactly, same span as before.
        let mut phrases =
            strip(&[(0, 30_000, "intro"), (30_000, 60_000, "build"), (60_000, 120_000, "drop")]);
        let span = (phrases[0].start_ms, phrases[phrases.len() - 1].end_ms);

        for edit in [
            PhraseEdit::Move { at: 1, time_ms: 20_000 },
            PhraseEdit::Split { at: 2, time_ms: 90_000 },
            PhraseEdit::Name { at: 2, kind: "break".to_string() },
            PhraseEdit::Merge { at: 1 },
            PhraseEdit::Move { at: 1, time_ms: 75_000 },
        ] {
            edit_phrases(&mut phrases, &edit);
        }

        assert_eq!((phrases[0].start_ms, phrases[phrases.len() - 1].end_ms), span);
        for pair in phrases.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms, "a gap or an overlap: {pair:?}");
        }
        for phrase in &phrases {
            assert!(phrase.start_ms < phrase.end_ms, "an inverted section: {phrase:?}");
        }
    }
    use super::*;

    #[test]
    fn a_drive_written_before_the_kits_were_default_starts_carrying_them() {
        // The reason nobody's stems reached a stick. The old field said whether
        // to carry them and every drive was made with it off, so a kit that had
        // been rendered stayed on the laptop unless somebody found the tick box.
        // Reading a collection from that build has to come back carrying them,
        // or the fix only helps drives added from now on.
        let older = r#"{
            "tracks": [],
            "playlists": [],
            "saved": [],
            "next_id": 1,
            "drives": [{
                "label": "TRANSCEND",
                "path": "/Volumes/TRANSCEND",
                "is_image": false,
                "playlists": ["Saturday"],
                "written": [],
                "with_stems": false,
                "bytes": 0,
                "last_sync": null
            }]
        }"#;
        let read: Library = serde_json::from_str(older).expect("an older collection should open");
        assert!(!read.drives[0].skip_stems, "the drive still will not carry its kits");
    }

    #[test]
    fn a_drive_remembers_that_its_kits_are_held_back() {
        // The other half: somebody who wants a small stick unticks the box, and
        // that has to survive a restart too, or the drive quietly triples.
        let mut library = Library::default();
        library.drives.push(Drive {
            label: "USB".into(),
            path: PathBuf::from("/Volumes/USB"),
            skip_stems: true,
            playlists: vec!["Saturday".into()],
            ..Drive::default()
        });
        let mut track = Track::placeholder(1);
        track.stems.vocals = Some("/stems/a-vocals.wav".into());
        track.stems.drums = Some("/stems/a-drums.wav".into());
        track.stems.melody = Some("/stems/a-melody.wav".into());
        library.tracks.push(track);

        let written = serde_json::to_string(&library).expect("a collection should serialise");
        let read: Library = serde_json::from_str(&written).expect("and read back");

        assert!(read.drives[0].skip_stems, "the drive forgot that its kits are held back");
        let kit = &read.tracks[0].stems;
        assert!(kit.vocals.is_some() && kit.drums.is_some() && kit.melody.is_some(), "{kit:?}");
    }

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
            written: vec![
                Written { id, prep: 0, ..Written::default() },
                Written { id: other, prep: 0, ..Written::default() },
            ],
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

    fn rekordbox_track() -> booth_cli::rekordbox::master::Track {
        booth_cli::rekordbox::master::Track {
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
            cues: vec![booth_cli::rekordbox::master::Cue {
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
    fn words_read_before_they_were_placed_say_so_when_the_collection_is_read_back() {
        // The field exists for collections that already have words in them,
        // so the one case that matters is the one serde fills in: an older
        // collection has no such key and must come back as not placed, or the
        // tracks that need the fix are exactly the ones that never get it.
        let older = r#"{
            "tracks": [],
            "playlists": [],
            "saved": [],
            "next_id": 1,
            "drives": []
        }"#;
        let read: Library = serde_json::from_str(older).expect("an older collection should open");
        assert!(read.tracks.is_empty());

        let mut track = Track::placeholder(1);
        track.lyrics = vec![Lyric { start_ms: 0, end_ms: 900, text: "get down".into() }];
        track.lyrics_aligned = false;
        let written = serde_json::to_string(&track).expect("a track should serialise");
        let back: Track = serde_json::from_str(&written).expect("and read back");
        assert!(!back.lyrics_aligned, "a track that needs placing forgot that it does");

        // And one with no words at all is not waiting on anything.
        assert!(Track::placeholder(2).lyrics_aligned);
    }

    fn spoken(language: &str) -> Track {
        let mut track = Track::placeholder(1);
        track.language = language.to_string();
        track
    }

    #[test]
    fn a_collection_mostly_in_one_language_says_so() {
        let mut tracks: Vec<Track> = (0..9).map(|_| spoken("en")).collect();
        tracks.push(spoken("de"));
        assert_eq!(common_language(&tracks).as_deref(), Some("en"));
    }

    #[test]
    fn a_collection_split_between_languages_lets_the_recogniser_decide() {
        // Half a shelf in German is a shelf where guessing per track is the
        // right answer, and a hint would be wrong half the time.
        let mut tracks: Vec<Track> = (0..6).map(|_| spoken("en")).collect();
        tracks.extend((0..6).map(|_| spoken("de")));
        assert_eq!(common_language(&tracks), None);
    }

    #[test]
    fn a_collection_with_too_little_read_yet_does_not_presume() {
        let tracks: Vec<Track> = (0..3).map(|_| spoken("en")).collect();
        assert_eq!(common_language(&tracks), None);
        assert_eq!(common_language(&[]), None);
    }

    #[test]
    fn tracks_nothing_has_been_read_off_do_not_count_towards_it() {
        let mut tracks: Vec<Track> = (0..9).map(|_| spoken("")).collect();
        tracks.extend((0..9).map(|_| spoken("fr")));
        assert_eq!(common_language(&tracks).as_deref(), Some("fr"));
    }

    #[test]
    fn the_comment_leads_with_the_lines_the_track_keeps_coming_back_to() {
        // Both drive databases carry one free-text field per track and a CDJ
        // shows it. It had been empty on every drive this ever wrote.
        let lyrics: Vec<Lyric> = [
            (0, "walking through the city at night"),
            (40_000, "hold me closer now"),
            (100_000, "hold me closer now"),
            (160_000, "and then home"),
        ]
        .iter()
        .map(|&(start_ms, text)| Lyric { start_ms, end_ms: start_ms + 2_000, text: text.into() })
        .collect();
        let refrains = refrains_from(&lyrics);

        let comment = words_as_comment(&lyrics, &refrains);
        let lines: Vec<&str> = comment.lines().collect();

        assert_eq!(lines[0], "2\u{d7} hold me closer now", "{comment:?}");
        assert_eq!(lines[1], "", "a blank line separates the hooks from the lyric");
        assert_eq!(lines[2], "walking through the city at night");
        assert_eq!(lines.last(), Some(&"and then home"));
    }

    #[test]
    fn the_hooks_lead_because_the_end_is_what_gets_cut() {
        // The comment shares a 4 kB row with the path and the title, and
        // anything past what fits comes off the end. So a long lyric loses its
        // last verse and never the line that identifies the record.
        // Filler with nothing in common, so the grouping keeps it apart and
        // none of it out-counts the hook. Lines that differ by one word would
        // group into a single refrain, which is the whole point of the
        // grouping and makes a poor fixture.
        const FILLER: [&str; 8] = [
            "walking through the city at night",
            "nobody told me it would end",
            "a coat on the back of a chair",
            "every window on the eighteenth floor",
            "she said wait for the rain",
            "counting the stops to the river",
            "somewhere a door closes twice",
            "the last train out of the station",
        ];
        let lyrics: Vec<Lyric> = (0..400)
            .map(|i| Lyric {
                start_ms: i * 5_000,
                end_ms: i * 5_000 + 500,
                text: match i % 3 {
                    0 => "hold me closer now".to_string(),
                    _ => FILLER[i as usize % FILLER.len()].to_string(),
                },
            })
            .collect();
        let comment = words_as_comment(&lyrics, &refrains_from(&lyrics));
        assert!(comment.len() > 4_000, "the fixture is not long enough to prove anything");
        assert!(comment.starts_with("134\u{d7} hold me closer now"), "{:?}", &comment[..40]);
    }

    #[test]
    fn a_track_nobody_has_read_the_words_of_gets_no_comment() {
        assert_eq!(words_as_comment(&[], &[]), "");
    }

    #[test]
    fn what_a_track_keeps_saying_is_the_lines_it_comes_back_to() {
        let words = [
            (0, "walking through the city at night"),
            (40_000, "hold me closer now"),
            (100_000, "hold me closer now"),
            (160_000, "and then home"),
            (200_000, "hold me closer now"),
            (260_000, "and then home"),
        ];
        let lyrics: Vec<Lyric> = words
            .iter()
            .map(|&(start_ms, text)| Lyric {
                start_ms,
                end_ms: start_ms + 2_000,
                text: text.into(),
            })
            .collect();

        let refrains = refrains_from(&lyrics);
        assert_eq!(refrains.len(), 2, "only what repeats: {refrains:?}");
        assert_eq!(refrains[0].text, "hold me closer now", "most repeated first: {refrains:?}");
        assert_eq!(refrains[0].times(), 3);
        assert_eq!(refrains[0].at, vec![40_000, 100_000, 200_000], "and every time it lands");
        assert_eq!(refrains[1].times(), 2);
        assert!(
            !refrains.iter().any(|r| r.text.contains("city")),
            "a line said once is not something the track keeps saying: {refrains:?}"
        );
    }

    #[test]
    fn a_track_whose_words_were_read_before_this_existed_gets_them_on_load() {
        // The field is new. A collection transcribed last week has the words
        // and not what repeats in them, and the panel that shows it should not
        // have to wonder which it has been handed.
        let dir = scratch("old-library");
        let path = dir.join("library.json");
        let mut library = Library::new();
        let id = library.add(Path::new("/music/one.flac"));
        library.get_mut(id).unwrap().lyrics = [40_000, 100_000, 160_000]
            .into_iter()
            .map(|start_ms| Lyric { start_ms, end_ms: start_ms + 2_000, text: "get down".into() })
            .collect();
        library.save(&path).unwrap();

        // Exactly what an older Booth would have written: the words, and no
        // mention of the analysis over them.
        let text = std::fs::read_to_string(&path).unwrap();
        let mut stored: serde_json::Value = serde_json::from_str(&text).unwrap();
        stored["tracks"][0].as_object_mut().unwrap().remove("refrains");
        std::fs::write(&path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();

        let back = Library::load(&path).unwrap();
        let track = back.get(id).unwrap();
        assert_eq!(track.refrains.len(), 1, "{:?}", track.refrains);
        assert_eq!(track.refrains[0].times(), 3);
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
        assert!(track.needs_attention().unwrap().contains("does not open a .ogg"));
        track.format = "m4a".into();
        assert_eq!(track.needs_attention(), None, "a CDJ-3000 plays AAC and ALAC");

        track.protected = true;
        let said = track.needs_attention().unwrap();
        assert!(said.contains("protected"), "{said}");
        assert!(said.contains("cannot convert it"), "and it should not offer to: {said}");
    }

    #[test]
    fn a_track_an_older_deck_cannot_open_is_not_a_track_that_needs_attention() {
        // What the sidebar counts is what nobody can fix by writing a drive.
        // A 96 kHz FLAC bound for a nexus 2 booth goes on as an mp3, so the
        // library is already fine and a count that flagged it would be sending
        // somebody to repair something that is not broken. The sync sheet is
        // where the re-encode is mentioned, because that is where it happens.
        let mut track = Track::placeholder(1);
        track.analyzed = true;
        track.has_grid = true;
        track.grid_confidence = 4.0;
        track.format = "flac".into();
        track.sample_rate = 96_000;
        assert_eq!(track.needs_attention(), None);

        // Past what the writer itself takes, there is nothing to be done.
        track.sample_rate = 192_000;
        assert!(track.needs_attention().unwrap().contains("96 kHz"));
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

/// A cached picture has to say what it is a picture of.
#[cfg(test)]
mod a_picture_of_what {
    use super::*;

    /// Ids well clear of anything another test might use, since the cache is
    /// one directory shared by the whole suite.
    fn scratch(name: &str) -> (u32, PathBuf) {
        let id = 0xA000_0000 + name.bytes().map(u32::from).sum::<u32>();
        forget_waveform(id);
        let dir = std::env::temp_dir().join(format!("booth-picture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (id, dir)
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_picture_of_these_files_is_the_one_that_comes_back() {
        let (id, dir) = scratch("kept");
        let audio = dir.join("stem.wav");
        write(&audio, b"some audio");
        let of = [audio.clone()];

        cache_waveform(id, &of, &[1, 2, 3]).unwrap();
        assert_eq!(cached_waveform(id, &of), Some(vec![1, 2, 3]));

        forget_waveform(id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_with_no_note_beside_it_is_not_trusted() {
        // The bug this is here for. A stem companion carries its parent's
        // path, and an older build drew from it and cached the *mix* under the
        // stem's name — so every acapella in the library had a picture of the
        // record, and re-analysing never touched it because the cache was
        // keyed by id alone and looked perfectly present.
        let (id, dir) = scratch("unnoted");
        let audio = dir.join("stem.wav");
        write(&audio, b"some audio");

        // Written the way the old code wrote it: bands, and nothing saying
        // what they are of.
        std::fs::create_dir_all(waveform_path(id).parent().unwrap()).unwrap();
        write(&waveform_path(id), &[9, 9, 9]);
        assert_eq!(
            cached_waveform(id, std::slice::from_ref(&audio)),
            None,
            "a picture that cannot vouch for itself was used anyway"
        );

        forget_waveform(id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_of_a_file_that_has_changed_since_is_not_trusted() {
        // A kit rendered again, at another quality or by another separator.
        let (id, dir) = scratch("changed");
        let audio = dir.join("stem.wav");
        write(&audio, b"the first render");
        let of = [audio.clone()];
        cache_waveform(id, &of, &[1, 2, 3]).unwrap();

        // Long enough after that the modification time really moves.
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        write(&audio, b"a second render, of a different length");
        assert_eq!(cached_waveform(id, &of), None);

        forget_waveform(id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_of_a_file_that_is_not_there_is_better_than_no_picture() {
        // Nothing can be said about whether it is still right, and a row whose
        // drive has been unplugged is better showing the last picture of it
        // than an empty strip that reads as a track with no sound in it.
        let (id, dir) = scratch("unplugged");
        let audio = dir.join("stem.wav");
        write(&audio, b"some audio");
        cache_waveform(id, std::slice::from_ref(&audio), &[4, 5, 6]).unwrap();

        std::fs::remove_file(&audio).unwrap();
        assert_eq!(cached_waveform(id, &[audio]), Some(vec![4, 5, 6]));

        forget_waveform(id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_instrumental_is_a_picture_of_both_its_files() {
        // Two stems summed. Either one changing makes the picture wrong.
        let (id, dir) = scratch("instrumental");
        let (melody, drums) = (dir.join("melody.mp3"), dir.join("drums.mp3"));
        write(&melody, b"melody");
        write(&drums, b"drums");
        let of = [melody.clone(), drums.clone()];
        cache_waveform(id, &of, &[7]).unwrap();
        assert_eq!(cached_waveform(id, &of), Some(vec![7]));

        std::thread::sleep(std::time::Duration::from_millis(1_100));
        write(&drums, b"different drums");
        assert_eq!(cached_waveform(id, &of), None, "one of the two changed");

        forget_waveform(id);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// Taking what a player left, through the files it would actually have left.
///
/// Written with the exporter and read with the reader, so the test exercises
/// the whole path rather than a hand-built struct: a change to either end that
/// stopped them meeting would show up here.
#[cfg(test)]
mod what_the_deck_did {
    use super::*;
    use booth_cli::export::{
        anlz, waveform::WaveformData, Beat, BeatGrid, Cue, Mood, SongStructure,
    };

    /// 128 BPM, half a bar a second, sixteen beats.
    fn grid() -> BeatGrid {
        BeatGrid {
            beats: (0..16)
                .map(|i| Beat {
                    number: (i % 4) as u16 + 1,
                    tempo_x100: 12_800,
                    time_ms: i as u32 * 469,
                })
                .collect(),
        }
    }

    /// What the stick holds after somebody has been at it on a deck.
    fn off_the_drive(
        cues: &[Cue],
        structure: Option<&SongStructure>,
    ) -> booth_cli::rekordbox::anlz::Analysis {
        let grid = grid();
        let waveforms = WaveformData::silent(8.0);
        let files = anlz::Analysis {
            on_drive_path: "/Contents/Artist/Track.flac",
            grid: &grid,
            cues,
            waveforms: &waveforms,
            structure,
            vbr: None,
        };
        booth_cli::rekordbox::anlz::read_files(&files.dat(), Some(&files.ext())).unwrap()
    }

    fn a_track() -> Track {
        let mut track = Track::placeholder(1);
        track.duration_secs = 8.0;
        track.bpm = 174.0;
        track.analyzed = true;
        track.cues =
            vec![CueMark { letter: 1, time_ms: 1_000, label: "drop".into(), color: [1, 2, 3] }];
        track
    }

    #[test]
    fn the_cues_the_deck_has_replace_the_ones_we_had() {
        let cues = vec![
            Cue::memory(0),
            Cue::hot(1, 4_000).with_comment("hold me closer now").with_color(0xe8, 0x3c, 0x9e),
        ];
        let mut track = a_track();
        assert!(take_prep(&mut track, &off_the_drive(&cues, None)));

        assert_eq!(track.cues.len(), 2);
        assert_eq!(track.cues[0].letter, 0, "the memory cue sorts first");
        let hot = &track.cues[1];
        assert_eq!(hot.time_ms, 4_000);
        assert_eq!(hot.label, "hold me closer now");
        assert_eq!(hot.color, [0xe8, 0x3c, 0x9e]);
    }

    #[test]
    fn the_tempo_comes_back_off_the_grid() {
        let mut track = a_track();
        take_prep(&mut track, &off_the_drive(&[Cue::memory(0)], None));
        assert_eq!(track.bpm, 128.0, "the deck's grid says 128, not the 174 we had");
        assert_eq!(track.beats, 16);
        assert!(track.has_grid);
    }

    #[test]
    fn phrases_come_back_as_the_positions_the_strip_draws() {
        // The format keeps phrases as beat numbers; the strip wants
        // milliseconds, and the grid on the drive is what turns one into the
        // other.
        let structure = SongStructure {
            mood: Mood::High,
            end_beat: 16,
            bank: 0,
            phrases: vec![
                booth_cli::export::Phrase { beat: 1, kind: 1 },
                booth_cli::export::Phrase { beat: 9, kind: 5 },
            ],
        };
        let mut track = a_track();
        take_prep(&mut track, &off_the_drive(&[Cue::memory(0)], Some(&structure)));

        let strip: Vec<(u32, u32, &str)> = track
            .phrases
            .iter()
            .map(|phrase| (phrase.start_ms, phrase.end_ms, phrase.kind.as_str()))
            .collect();
        assert_eq!(strip, vec![(0, 8 * 469, "intro"), (8 * 469, 15 * 469, "drop")]);
    }

    #[test]
    fn taking_it_says_when_it_was_taken() {
        // Otherwise the next sync has no time for this side and cannot say
        // which of the two is the later.
        let mut track = a_track();
        assert_eq!(track.edited, None);
        take_prep(&mut track, &off_the_drive(&[Cue::memory(0)], None));
        assert!(track.edited.is_some());
    }

    #[test]
    fn taking_what_we_already_have_changes_nothing_and_says_so() {
        let cues = vec![Cue::memory(0), Cue::hot(1, 4_000).with_color(1, 2, 3)];
        let mut track = a_track();
        take_prep(&mut track, &off_the_drive(&cues, None));
        let settled = track.clone();

        assert!(!take_prep(&mut track, &off_the_drive(&cues, None)), "nothing moved");
        assert_eq!(track.cues, settled.cues);
        assert_eq!(track.edited, settled.edited, "an unchanged track is not re-stamped");
    }

    #[test]
    fn a_phrase_name_the_format_has_no_word_for_is_left_out() {
        // Low- and mid-mood tracks number their phrases differently. Guessing
        // at one would put the wrong word on the strip.
        let structure = SongStructure {
            mood: Mood::High,
            end_beat: 16,
            bank: 0,
            phrases: vec![
                booth_cli::export::Phrase { beat: 1, kind: 1 },
                booth_cli::export::Phrase { beat: 5, kind: 9 },
                booth_cli::export::Phrase { beat: 9, kind: 6 },
            ],
        };
        let mut track = a_track();
        take_prep(&mut track, &off_the_drive(&[Cue::memory(0)], Some(&structure)));

        let kinds: Vec<&str> = track.phrases.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds, vec!["intro", "outro"]);
    }
}
