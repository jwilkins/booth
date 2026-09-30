//! The work, and the thread it happens on.
//!
//! Nothing here draws, and nothing here holds the collection. A job is handed
//! everything it needs when it starts, and reports back as [`Update`]s that the
//! window folds into the library. That is what keeps a five-minute stem render
//! from freezing the browser, and what lets the browser stay usable while a
//! drive is being written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use booth_cli::analysis;
use booth_cli::audio::decode::decode_file;
use booth_cli::cli::{ExportArgs, InputArgs, StemsArgs};
use booth_cli::report::{Event, Reporter};
use booth_cli::stems::{Backend, Stem};

use crate::config::{self, Config};
use crate::library::{energy_from, CueMark, Phrase, StemKit, Track};
use crate::theme::CUE_COLORS;

/// What the window has asked for.
pub enum Job {
    /// Walk the paths and read what each file says about itself. Fast: no
    /// decoding beyond what the tags need.
    Import { paths: Vec<PathBuf>, recursive: bool },
    /// Listen to each track: grid, key, phrases, cues, loudness, waveform.
    Analyze(Vec<(u32, PathBuf)>),
    /// Take a copy of tracks whose files are outside the library folder.
    Adopt { tracks: Vec<Adoptable>, config: Box<Config> },
    /// Write a track's artist, title and album back into the file's own tags.
    Retag(Vec<Retag>),
    /// Re-encode files a player will not open into ones it will.
    Convert(Vec<Convertible>),
    /// Read a rekordbox library and bring it across.
    Rekordbox { path: PathBuf, key: String },
    /// Render stem kits.
    Separate {
        tracks: Vec<(u32, PathBuf)>,
        stems_in: crate::config::StemsLocation,
        backend: Backend,
        quality: booth_cli::cli::StemQuality,
    },
    /// Measure one row's waveform, and nothing else.
    ///
    /// Deliberately not [`Job::Analyze`], which is where this used to go. That
    /// re-derives the grid, the key, the cues and the phrases from the audio
    /// and writes them over whatever the collection had — which is a silent
    /// loss of every cue somebody moved by hand, for a row that only wanted a
    /// picture. And for a stem it was the wrong audio: a companion carries its
    /// parent's path, so the acapella was handed the whole mix to draw.
    Draw(Vec<Drawable>),
    /// Read the words off tracks' vocal stems.
    Transcribe { tracks: Vec<Transcribable>, whisper: crate::config::Whisper },
    /// Decode one track into memory so it can be auditioned.
    Decode {
        id: u32,
        /// Summed when there is more than one, which is what an instrumental
        /// is: the separator writes parts, never a mix of some of them.
        sources: Vec<PathBuf>,
    },
    /// Measure how loud each stem is across a track, for colouring its
    /// waveform by what is playing rather than by frequency.
    StemEnvelopes { id: u32, kit: StemKit },
    /// Read the collection's files back and say where they and the collection
    /// have come apart. `deep` reads every byte; without it, a stat and a tag
    /// read per file.
    Verify {
        /// The records themselves, because the check is a comparison and the
        /// stored side of it is what is being compared.
        tracks: Vec<Track>,
        deep: bool,
        /// Walked for files no track points at. Empty to skip that half.
        library: PathBuf,
    },
    /// Work out the two hashes for tracks that have none.
    ///
    /// Import does this as it goes, so this is for the tracks that were already
    /// in the collection before it did — without it, a library built before
    /// hashing existed can never be checked for copies at all.
    Hash(Vec<(u32, PathBuf)>),
    /// Fingerprint tracks and ask AcoustID what they are.
    Identify { tracks: Vec<(u32, PathBuf)>, key: String },
    /// Write a drive.
    Sync { args: Box<ExportArgs>, files: Vec<PathBuf> },
    /// Copy what a drive holds somewhere it will outlive the drive.
    Keep {
        /// The drive, mounted.
        root: PathBuf,
        /// What it calls itself, which becomes the folder the copies go in.
        drive: String,
        /// The state it was found in, which is what the copy records having
        /// stored. Decided by whoever asked for the copy, so that what is
        /// stored is what was checked.
        state: String,
        into: PathBuf,
        /// The library's files, for the drive's audio to be linked to rather
        /// than copied.
        known: Vec<crate::backup::Known>,
        /// What to do about music the library does not have.
        foreign: crate::config::OnForeign,
        /// Where music copied in goes, when that is what was asked for.
        library: PathBuf,
        /// The key the drive's OneLibrary database is encrypted with, for the
        /// play history to be read out of it. Without one, only the files are
        /// kept.
        key: Option<String>,
    },
}

impl Job {
    /// What the queue indicator calls it.
    pub fn name(&self) -> &'static str {
        match self {
            Job::Import { .. } => "reading",
            Job::Adopt { .. } => "copying",
            Job::Convert(_) => "converting",
            Job::Rekordbox { .. } => "reading rekordbox",
            Job::Retag(_) => "tagging",
            Job::Decode { .. } => "loading",
            Job::StemEnvelopes { .. } => "measuring stems",
            Job::Identify { .. } => "identifying",
            Job::Hash(_) => "checking for copies",
            Job::Verify { .. } => "checking the collection",
            Job::Analyze(_) => "analysing",
            Job::Separate { .. } => "stems",
            Job::Transcribe { .. } => "reading the words",
            Job::Draw { .. } => "drawing",
            Job::Sync { .. } => "writing",
            Job::Keep { .. } => "copying a drive",
        }
    }
}

/// Each companion row a kit yields, with the file it is drawn from.
///
/// Empty for a kit that is not complete: a companion row only exists when all
/// three parts do, so there is nothing to draw a picture for.
fn parts_of(kit: &StemKit) -> Vec<(crate::library::Role, Vec<PathBuf>)> {
    if !kit.is_complete() {
        return Vec::new();
    }
    crate::library::Role::PARTS
        .into_iter()
        .filter_map(|role| {
            let path = match role {
                crate::library::Role::Vocals => kit.vocals.as_ref(),
                crate::library::Role::Drums => kit.drums.as_ref(),
                crate::library::Role::Melody => kit.melody.as_ref(),
                crate::library::Role::Track => None,
            }?;
            Some((role, vec![path.clone()]))
        })
        .collect()
}

/// One row to draw, and the audio to draw it from.
pub struct Drawable {
    pub id: u32,
    /// Summed when there is more than one, which is what an instrumental is. A
    /// companion's own files, so the picture is of what will play.
    pub sources: Vec<PathBuf>,
}

/// A track to read the words off, and the stem to read them off.
///
/// The vocal stem rather than the mix. A recogniser handed a club record
/// transcribes the kick drum, and the whole reason this waits on a separation
/// is that an isolated voice is the only thing it has a chance with.
pub struct Transcribable {
    pub id: u32,
    pub vocals: PathBuf,
}

/// A track whose file should be copied into the library.
pub struct Adoptable {
    pub id: u32,
    pub path: PathBuf,
    /// The artist folder it goes under.
    pub artist: String,
}

/// One track's tags, as the collection now has them.
/// A file to re-encode into something the hardware opens.
pub struct Convertible {
    pub id: u32,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Retag {
    pub id: u32,
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub date: Option<String>,
    /// What to do about a field the file already has a value for.
    pub on_existing: booth_cli::tag::OnExisting,
}

/// Something the worker found out.
pub enum Update {
    /// A rekordbox library, read and ready to be merged in.
    Rekordbox(Box<booth_cli::rekordbox::master::Collection>),
    /// A file was re-encoded, and the collection should follow it.
    Converted {
        id: u32,
        to: PathBuf,
    },
    /// A file the import walked to, and the record read out of it.
    Imported(Box<Track>),
    /// One track, listened to.
    Analyzed(Box<Analyzed>),
    /// A track was decoded and is ready to play.
    Decoded {
        id: u32,
        sound: Arc<crate::player::Sound>,
    },
    /// One track, read back and compared with what the collection says.
    Verified(Box<crate::verify::Report>),
    /// What the drive's database says after a write: each row and the file it
    /// was made from.
    Wrote(Vec<(PathBuf, booth_cli::export::pdb::Track)>),
    /// Playable files in the library folder that no track points at.
    Orphans(Vec<PathBuf>),
    /// A drive was copied.
    Kept(Box<crate::backup::Backup>),
    /// What a player recorded having played, off a drive that is still mounted.
    Played {
        drive: String,
        root: PathBuf,
        sessions: Vec<crate::history::Session>,
    },
    /// The two hashes for one track that had none.
    Hashed {
        id: u32,
        file: String,
        audio: String,
    },
    /// What a fingerprint said a track is. Empty when nothing matched, which
    /// is itself worth recording so it is not asked again.
    Identified {
        id: u32,
        best: Option<crate::identify::Match>,
    },
    /// Per-stem loudness for one track's waveform.
    Envelopes {
        id: u32,
        envelopes: crate::wave::StemEnvelopes,
    },
    /// A file was copied into the library, and the track now lives there.
    Adopted {
        id: u32,
        to: PathBuf,
    },
    /// A stem kit finished rendering.
    Separated {
        id: u32,
        kit: StemKit,
    },
    /// A vocal stem has been through the recogniser. Empty lines are a real
    /// answer: it means nothing was sung, or nothing could be made out.
    Transcribed {
        id: u32,
        lyrics: Vec<crate::library::Lyric>,
    },
    /// One row's waveform, measured from its own audio.
    Drawn {
        id: u32,
        /// What it was drawn from, carried back so the cache can record what
        /// the picture is a picture of.
        sources: Vec<PathBuf>,
        /// The three-band picture, as [`Analyzed::bands`] carries it.
        bands: Vec<u8>,
    },
    Progress {
        done: usize,
        total: usize,
    },
    /// How far into the file in hand the running job has got.
    Step {
        percent: u8,
    },
    /// A line for the log, from a command that reports its own.
    Line(String),
    Failed {
        path: PathBuf,
        message: String,
    },
    /// The job ended. `Err` carries the message to show.
    Done(Result<(), String>),
}

/// Everything one pass of the analysers learned, ready to be folded into the
/// record the library already holds.
pub struct Analyzed {
    pub id: u32,
    /// The file it was listened to, which is what its picture is a picture of.
    pub path: PathBuf,
    pub bpm: f64,
    pub grid_confidence: f32,
    pub has_grid: bool,
    pub beats: usize,
    pub key: String,
    pub key_confidence: f32,
    pub energy: u8,
    /// The measurement the energy meter is a rank of, kept so the inspector can
    /// show it and so a miscalibrated meter is diagnosable rather than just
    /// wrong.
    pub intensity: f32,
    /// Every beat, when the grid bends. Empty when a tempo says it all.
    pub beat_ms: Vec<u32>,
    pub phrases: Vec<Phrase>,
    pub cues: Vec<CueMark>,
    pub loudness_lufs: Option<f64>,
    pub peak_dbtp: Option<f64>,
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub channels: u16,
    /// The three-band picture: columns of mid, high, low, exactly as the
    /// analysis file stores them. The waveform panel paints straight from this.
    ///
    /// The scrolling resolution rather than the preview one — 150 columns a
    /// second instead of 1,200 for the whole track. The panel takes peaks
    /// across whatever a pixel covers, so at a normal window it draws the same
    /// picture either way; the difference is that zooming in has something to
    /// find. About 135 kB for a five-minute track, against 30 MB for its stems.
    pub bands: Vec<u8>,
}

/// Read what a file says about itself, without decoding it.
///
/// Everything here comes from the container: the tags, the size on disk, and —
/// for a WAV — the header field that says whether the samples are floats, which
/// is the single most common way a file that plays on a laptop refuses to load
/// in a booth.
///
/// `hashes` reads every byte, twice over. Import always wants that, since the
/// file is open anyway and the answer is what makes copies findable later; a
/// check that only wants the size and the tags should not pay for it.
pub fn read_record(id: u32, path: &Path, hashes: bool) -> Track {
    let mut track = Track::placeholder(id);
    track.path = path.to_path_buf();
    track.format = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    track.bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    track.float_samples = is_float_wav(path);
    // One 16-byte read, and only for the containers that can carry DRM. A
    // protected purchase plays in the shop that sold it and nowhere else, and
    // finding that out when it is added beats finding out in a booth.
    track.protected = matches!(track.format.as_str(), "m4a" | "m4b" | "m4p" | "mp4" | "aac")
        && booth_cli::audio::mp4::is_protected(path);

    let metadata = booth_cli::tag::read_metadata(path).unwrap_or_default();
    let tagged_title = metadata.title.filter(|t| !t.trim().is_empty());
    let tagged_artist = metadata.artist.filter(|a| !a.trim().is_empty());
    // Both, not either: a file with an artist and no title has not been tagged
    // in any way worth defending against a fingerprint.
    track.from_tags = tagged_title.is_some() && tagged_artist.is_some();

    track.artist = tagged_artist.unwrap_or_default();
    track.album = metadata.album.unwrap_or_default();
    track.title = tagged_title
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    track.year = metadata.date.as_deref().and_then(|d| d.get(..4)?.parse().ok());

    if !hashes {
        return track;
    }
    // Both hashes at import, because both are a read of the file and the file
    // is open anyway. A failure is not one: an unreadable file has bigger
    // problems, and an empty hash simply never matches another.
    match booth_cli::hash::file_sha256(path) {
        Ok(hash) => track.file_hash = hash,
        Err(e) => crate::debug!("no file hash for {}: {e:#}", path.display()),
    }
    match booth_cli::hash::audio_sha256(path) {
        Ok(hash) => track.audio_hash = hash,
        Err(e) => crate::debug!("no audio hash for {}: {e:#}", path.display()),
    }
    track
}

/// Fill in from the path what the file's own tags did not say.
///
/// Only ever a gap-filler, and deliberately not part of [`read_record`]: that
/// reports what the *file* says, and the collection check compares a fresh one
/// of those against what is stored. A name read off a folder is not something
/// the file said, and treating it as one would turn every track in a tidy
/// folder into a disagreement with itself.
///
/// A file filed as `Peverelist/Tessellations/02 - Roll With The Punches.flac`
/// has already said who made it and what it is called. Reporting "unknown
/// artist" for it because its tags are empty throws away the best evidence on
/// the disk.
pub fn name_from_path(track: &mut Track) {
    if track.from_tags {
        return;
    }
    let from_path = crate::guess::from_path(&track.path);
    crate::debug!(
        "{}: no tags worth having; the path says {}",
        track.path.display(),
        from_path.describe()
    );
    if track.artist.trim().is_empty() && !from_path.artist.is_empty() {
        track.artist = from_path.artist;
    }
    if track.album.trim().is_empty() && !from_path.album.is_empty() {
        track.album = from_path.album;
    }
    // The title always, when the path found one: what `read_record` leaves
    // there is the whole file stem, numbering and all.
    if !from_path.title.is_empty() {
        track.title = from_path.title;
    }
}

/// Whether a WAV holds floating-point samples.
///
/// Reads the `fmt ` chunk rather than decoding: format 3 is IEEE float, and
/// format 0xFFFE is extensible, whose sub-format's first two bytes say the same
/// thing. Anything that is not a RIFF/WAVE file is not a float WAV.
fn is_float_wav(path: &Path) -> bool {
    use std::io::Read;

    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let mut header = [0u8; 4096];
    let Ok(read) = file.read(&mut header) else { return false };
    let header = &header[..read];
    if header.len() < 44 || &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return false;
    }

    // Walk the chunks: `fmt ` is normally first, but nothing requires it.
    let mut at = 12;
    while at + 8 <= header.len() {
        let id = &header[at..at + 4];
        let size = u32::from_le_bytes(header[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 2 <= header.len() {
            let format = u16::from_le_bytes(header[body..body + 2].try_into().unwrap());
            return match format {
                3 => true,
                0xFFFE if body + 26 <= header.len() => {
                    u16::from_le_bytes(header[body + 24..body + 26].try_into().unwrap()) == 3
                }
                _ => false,
            };
        }
        // Chunks are padded to an even length.
        at = body + size + (size & 1);
    }
    false
}

/// Listen to one track.
pub fn analyze_file(id: u32, path: &Path) -> anyhow::Result<Analyzed> {
    let audio = decode_file(path)?;
    let analysis = analysis::analyze(&audio);
    let waveform = booth_cli::export::waveform::analyze(&audio);
    let loudness = booth_cli::loudness::measure(&audio).ok();

    let beat_ms: Vec<u32> = analysis.grid.beats.iter().map(|b| b.time_ms).collect();
    // Kept only when a tempo and a downbeat could not put these beats back.
    // The tracker here runs at one tempo, so this is almost always empty; it
    // is here so a track whose grid was bent keeps its shape through a
    // re-analysis rather than being quietly straightened.
    let bent = match analysis.grid.bends() {
        true => analysis.grid.times_from_downbeat(),
        false => Vec::new(),
    };
    let phrases = analysis
        .structure
        .sections
        .iter()
        .map(|section| Phrase {
            start_ms: beat_time(&beat_ms, section.start_beat),
            end_ms: beat_time(&beat_ms, section.end_beat),
            kind: section.kind.label().to_string(),
        })
        .collect();

    let cues = cue_marks(&analysis.cues);

    // The energy meter reads the loudest stretch rather than the average: what
    // decides where a record sits in a crate is how hard it goes at its peak,
    // not how much of it is intro. The raw figure goes to the log because the
    // five bars are a calibration of it, and a calibration is only as good as
    // the numbers somebody looked at.
    let intensity = analysis.intensity;
    crate::debug!("#{id} peak onset density {intensity:.4} -> energy {}", energy_from(intensity));

    Ok(Analyzed {
        id,
        path: path.to_path_buf(),
        bpm: analysis.bpm,
        grid_confidence: analysis.confidence,
        has_grid: analysis.found_beats(),
        beats: analysis.grid.beats.len(),
        key: analysis.camelot(),
        key_confidence: analysis.key.as_ref().map(|k| k.confidence).unwrap_or(0.0),
        energy: energy_from(intensity),
        intensity,
        beat_ms: bent,
        phrases,
        cues,
        loudness_lufs: loudness.as_ref().map(|l| l.integrated_lufs),
        peak_dbtp: loudness.as_ref().map(|l| 20.0 * l.true_peak.max(1e-9).log10()),
        duration_secs: audio.duration_secs(),
        sample_rate: audio.sample_rate,
        channels: audio.channels() as u16,
        bands: waveform.band_detail,
    })
}

fn beat_time(beats: &[u32], beat_number: u16) -> u32 {
    let index = beat_number.saturating_sub(1) as usize;
    beats.get(index).copied().unwrap_or_else(|| beats.last().copied().unwrap_or(0))
}

/// The engine's cues, as the collection keeps them.
///
/// Shared by the analyser and by the auto-cue pass that runs off a track's
/// words, so that a cue means the same thing and is the same colour whichever
/// of the two placed it.
pub fn cue_marks(cues: &[booth_cli::export::Cue]) -> Vec<CueMark> {
    cues.iter()
        .map(|cue| CueMark {
            letter: cue.hot_cue,
            time_ms: cue.time_ms,
            label: cue.comment.clone().unwrap_or_default(),
            color: cue
                .color
                .map(|rgb| [rgb.r, rgb.g, rgb.b])
                .unwrap_or_else(|| cue_color(cue.hot_cue)),
        })
        .collect()
}

pub fn cue_color(letter: u8) -> [u8; 3] {
    let color = CUE_COLORS[(letter.saturating_sub(1) as usize) % CUE_COLORS.len()];
    [color.r(), color.g(), color.b()]
}

/// Fingerprint each track and ask what it is.
///
/// One at a time and paced, because both services ask for that: AcoustID wants
/// no more than three requests a second and MusicBrainz wants one. A library
/// tool that gets someone's IP blocked has done them real harm, so the pacing
/// is the services' own and is not configurable down.
fn identify(tracks: &[(u32, PathBuf)], key: &str, reporter: &Channel) -> anyhow::Result<()> {
    use booth_cli::tag::{acoustid, fingerprint, musicbrainz};

    let mut acoustid = acoustid::Client::new(key.to_string(), acoustid::DEFAULT_MIN_INTERVAL);
    let mut brainz = musicbrainz::Client::new(std::time::Duration::from_millis(1_100));
    let total = tracks.len();

    for (done, (id, path)) in tracks.iter().enumerate() {
        if reporter.cancelled() {
            break;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();

        let looked_up = (|| -> anyhow::Result<Option<crate::identify::Match>> {
            let audio = decode_file(path)?;
            let print = fingerprint::fingerprint(&audio)?;
            let candidates = acoustid.lookup(&print)?;
            crate::debug!("{name}: {} candidates", candidates.len());

            // The best one only. A list of near-misses is not a question
            // anybody can answer better than the top score can.
            let Some(best) = candidates
                .into_iter()
                .max_by(|a, b| a.score.total_cmp(&b.score))
                .filter(|best| best.score >= crate::identify::FLOOR)
            else {
                return Ok(None);
            };

            let mut found = crate::identify::Match {
                score: best.score,
                artist: best.artist.clone().unwrap_or_default(),
                title: best.title.clone().unwrap_or_default(),
                recording_mbid: best.recording_mbid.clone(),
                acoustid: best.acoustid.clone(),
                ..Default::default()
            };

            // MusicBrainz for the album and the year, which AcoustID does not
            // carry. A failure here is not a failure of the identification.
            match brainz.lookup_recording(&best.recording_mbid) {
                Ok(recording) => {
                    let metadata = booth_cli::tag::Metadata::from_musicbrainz(
                        &recording,
                        Some(&best.acoustid),
                    );
                    if let Some(artist) = metadata.artist {
                        found.artist = artist;
                    }
                    if let Some(title) = metadata.title {
                        found.title = title;
                    }
                    found.album = metadata.album.unwrap_or_default();
                    found.year =
                        metadata.date.as_deref().and_then(|date| date.get(..4)?.parse().ok());
                }
                Err(e) => crate::warn!("{name}: musicbrainz lookup failed: {e:#}"),
            }
            Ok(Some(found))
        })();

        match looked_up {
            Ok(best) => {
                match &best {
                    Some(found) => crate::info!(
                        "{name}: {} ({:.0}% confident)",
                        found.describe(),
                        found.score * 100.0
                    ),
                    None => crate::info!("{name}: no match"),
                }
                let _ = reporter.tx.send(Update::Identified { id: *id, best });
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: path.clone(), message: format!("{e:#}") });
            }
        }
        let _ = reporter.tx.send(Update::Progress { done: done + 1, total });
        (reporter.wake)();
    }
    Ok(())
}

/// How loud each stem is, column by column, on the same grid as the waveform.
///
/// Measured from the rendered files rather than inferred from the mix, which is
/// the whole point: a band split can say where the bass is, and only a
/// separation can say where the voice is.
pub fn stem_envelopes(kit: &StemKit) -> anyhow::Result<crate::wave::StemEnvelopes> {
    let measure = |path: Option<&PathBuf>| -> anyhow::Result<Vec<u8>> {
        let Some(path) = path else { return Ok(Vec::new()) };
        let audio = decode_file(path)?;
        let waveform = booth_cli::export::waveform::analyze(&audio);
        // The three band bytes of each column, collapsed to how much is there:
        // for colouring, what matters is which stem is loudest, not what it is
        // made of.
        Ok(waveform
            .band_preview
            .chunks(3)
            .map(|column| column.iter().copied().max().unwrap_or(0))
            .collect())
    };

    Ok(crate::wave::StemEnvelopes {
        vocals: measure(kit.vocals.as_ref())?,
        melody: measure(kit.melody.as_ref())?,
        drums: measure(kit.drums.as_ref())?,
    })
}

/// The record this file is a stem of, when its name says it is one and that
/// record is beside it.
///
/// The mirror of [`find_stems`], which builds the name; this reads it back. The
/// rule is the separator's: `<record>-<part>` beside `<record>`. Both halves
/// have to hold — a record legitimately called "Midnight-Drums" is a record, and
/// the thing that tells it from a kit's drum part is whether there is a
/// "Midnight" next to it.
///
/// Asked when music arrives from somewhere this program did not put it, because
/// a stem that becomes a track of its own is three junk rows per record in the
/// browser and three rows the player will show as separate music.
pub fn cut_from(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_str()?;
    let (record, part) = stem.rsplit_once('-')?;
    if !Stem::ALL.iter().any(|known| known.name() == part) {
        return None;
    }
    let folder = path.parent()?;
    ["wav", "flac", "mp3", "m4a", "aac", "aiff", "aif"]
        .iter()
        .map(|extension| folder.join(format!("{record}.{extension}")))
        .find(|beside| beside.is_file())
}

/// Which stems were written for a track, by looking for them.
///
/// The separator names its outputs `<stem-name>` beside the track's file stem,
/// so the kit is a fact about the disk rather than something to remember. A
/// library that believes it has stems it does not have is a library that
/// promises an acapella at the wrong moment.
pub fn find_stems(stems_in: &crate::config::StemsLocation, source: &Path) -> StemKit {
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let places = stems_in.search(source);
    let find = |name: &str| -> Option<PathBuf> {
        places
            .iter()
            .flat_map(|dir| {
                ["wav", "flac", "mp3"]
                    .iter()
                    .map(|extension| dir.join(format!("{stem}-{name}.{extension}")))
            })
            .find(|path| path.exists())
    };
    StemKit {
        vocals: find(Stem::Vocals.name()),
        melody: find(Stem::Melody.name()),
        drums: find(Stem::Drums.name()),
    }
}

// -- running ---------------------------------------------------------------

/// Decode one file, or sum several into one sound.
///
/// Summed at unity and then brought back under full scale together, rather than
/// halved on the way in: two stems of the same record are already the right
/// balance against each other, and scaling them apart is a mix decision this
/// has no business making. Only the total needs to fit.
/// Draw one row and report it, saying so rather than failing if it will not
/// decode.
///
/// A kit with one corrupt stem should still get the other two drawn, and a
/// separation that rendered fine should not be reported as having failed
/// because the picture of it could not be made.
fn draw_one(row: &Drawable, reporter: &Channel) {
    match decode_sum(&row.sources) {
        Ok(audio) => {
            let bands = booth_cli::export::waveform::analyze(&audio).band_detail;
            crate::debug!(
                "drew #{} from {}",
                row.id,
                crate::library::plural(row.sources.len(), "file")
            );
            let _ =
                reporter.tx.send(Update::Drawn { id: row.id, sources: row.sources.clone(), bands });
        }
        Err(e) => crate::warn!("could not draw #{}: {e:#}", row.id),
    }
}

/// Draw each row from its own audio.
///
/// One at a time, and a row that will not decode does not take the rest with
/// it: a kit with one corrupt stem should still get the other two drawn.
fn draw(rows: &[Drawable], reporter: &Channel) -> anyhow::Result<()> {
    let batch =
        Batch { inner: reporter, done: std::sync::atomic::AtomicUsize::new(0), total: rows.len() };
    for row in rows {
        if reporter.cancelled() {
            break;
        }
        batch.starting();
        draw_one(row, reporter);
        batch.finished_one();
        (reporter.wake)();
    }
    Ok(())
}

/// The audio a row is made of, summed when it is made of several.
///
/// The deck's own summing goes through [`crate::player::Sound`], which is
/// interleaved and rate-matched for playback; the waveform wants an [`Audio`]
/// to measure. Same rule either way: an instrumental is its melody and its
/// drums added together, and nothing else is more than one file.
fn decode_sum(sources: &[PathBuf]) -> anyhow::Result<booth_cli::audio::Audio> {
    let first = sources.first().ok_or_else(|| anyhow::anyhow!("nothing to draw"))?;
    let mut audio = decode_file(first)?;
    for path in &sources[1..] {
        audio
            .add_assign(&decode_file(path)?)
            .map_err(|e| anyhow::anyhow!("mixing {} into the picture: {e}", path.display()))?;
    }
    Ok(audio)
}

fn decode_sources(sources: &[PathBuf]) -> anyhow::Result<crate::player::Sound> {
    let first = sources.first().ok_or_else(|| anyhow::anyhow!("nothing to play"))?;
    let mut sound = crate::player::Sound::from_audio(&decode_file(first)?);
    for path in &sources[1..] {
        let next = crate::player::Sound::from_audio(&decode_file(path)?);
        anyhow::ensure!(
            next.rate == sound.rate && next.channels == sound.channels,
            "{} does not match the stem beside it",
            path.display()
        );
        if next.samples.len() > sound.samples.len() {
            sound.samples.resize(next.samples.len(), 0.0);
        }
        for (into, from) in sound.samples.iter_mut().zip(&next.samples) {
            *into += from;
        }
    }
    if sources.len() > 1 {
        let peak = sound.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        if peak > 1.0 {
            let gain = 1.0 / peak;
            for sample in &mut sound.samples {
                *sample *= gain;
            }
        }
    }
    Ok(sound)
}

/// A reporter that forwards a command's own progress to the window.
struct Channel {
    tx: Sender<Update>,
    cancel: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Reporter for Channel {
    fn event(&self, event: Event) {
        let update = match event {
            // Straight to the log rather than through the window: this is the
            // step-by-step account of a job, it belongs at the log's most
            // detailed level, and going the direct way keeps it in order with
            // the rest of what this thread has to say.
            Event::Detail(text) => {
                crate::debug!("{text}");
                return;
            }
            Event::Progress { done, total } => Update::Progress { done, total },
            Event::Started { total } => Update::Progress { done: 0, total },
            Event::Step { percent } => Update::Step { percent },
            Event::Line(text) | Event::Heading(text) | Event::Summary(text) => Update::Line(text),
            Event::Failed { path, message } => Update::Failed { path, message },
            // The window keeps its own count of what finished; a command's
            // summary of its own run would be a second, disagreeing one.
            Event::Stage { .. } | Event::Finished { .. } => return,
        };
        let _ = self.tx.send(update);
        (self.wake)();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Owns the worker thread and the channel back from it.
pub struct Runner {
    rx: Receiver<Update>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// What is running, for the queue indicator.
    pub name: &'static str,
}

impl Runner {
    pub fn start(job: Job, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let name = job.name();
        let reporter = Channel { tx: tx.clone(), cancel: Arc::clone(&cancel), wake };

        let handle = std::thread::spawn(move || {
            let result = run(job, &reporter);
            let _ = tx.send(Update::Done(result.map_err(|e| format!("{e:#}"))));
            (reporter.wake)();
        });

        Self { rx, cancel, handle: Some(handle), name }
    }

    /// Everything that has arrived since the last look.
    pub fn drain(&self) -> Vec<Update> {
        let mut updates = Vec::new();
        while let Ok(update) = self.rx.try_recv() {
            updates.push(update);
        }
        updates
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        // Closing the window mid-render should not leave a thread writing files
        // into a directory nobody is watching.
        self.cancel();
    }
}

fn run(job: Job, reporter: &Channel) -> anyhow::Result<()> {
    match job {
        Job::Import { paths, recursive } => import(&paths, recursive, reporter),
        Job::Analyze(tracks) => analyze_all(&tracks, reporter),
        Job::Adopt { tracks, config } => adopt(&tracks, &config, reporter),
        Job::Convert(tracks) => convert(&tracks, reporter),
        Job::Rekordbox { path, key } => {
            let connection = booth_cli::rekordbox::open(&path, &key)?;
            let collection = booth_cli::rekordbox::master::read(&connection)?;
            crate::info!(
                "rekordbox: {} tracks, {} playlists",
                collection.tracks.len(),
                collection.playlists.len()
            );
            let _ = reporter.tx.send(Update::Rekordbox(Box::new(collection)));
            (reporter.wake)();
            Ok(())
        }
        Job::Retag(tracks) => retag(&tracks, reporter),
        Job::Identify { tracks, key } => identify(&tracks, &key, reporter),
        Job::Hash(tracks) => hash_all(&tracks, reporter),
        Job::Verify { tracks, deep, library } => verify_all(&tracks, deep, &library, reporter),
        Job::Keep { root, drive, state, into, known, foreign, library, key } => {
            let kept = match crate::backup::keep(
                &root, &into, &drive, &state, &known, foreign, &library,
            ) {
                Ok(kept) => kept,
                Err(e) => {
                    // Said here as well as returned, because a copy is minutes
                    // of work and the log is where somebody looks to find out
                    // how far it got before it stopped.
                    crate::error!("copying {drive} failed: {e:#}");
                    return Err(e);
                }
            };
            let _ = reporter.tx.send(Update::Kept(Box::new(kept)));
            (reporter.wake)();

            // The one thing on a drive the collection cannot make for itself.
            // Read after the copy rather than before it, so that a history this
            // fails to make sense of has still been stored.
            if let Some(key) = key {
                match crate::history::read(&root, &key) {
                    Ok(sessions) if !sessions.is_empty() => {
                        let played = sessions.iter().map(|s| s.played.len()).sum::<usize>();
                        crate::info!(
                            "{} read off {drive}, {}",
                            crate::library::plural(sessions.len(), "session"),
                            crate::library::plural(played, "track")
                        );
                        let _ = reporter.tx.send(Update::Played {
                            drive: drive.clone(),
                            root: root.clone(),
                            sessions,
                        });
                        (reporter.wake)();
                    }
                    Ok(_) => {}
                    Err(e) => crate::debug!("no history off {drive}: {e:#}"),
                }
            }
            Ok(())
        }
        Job::StemEnvelopes { id, kit } => {
            let envelopes = stem_envelopes(&kit)?;
            let _ = reporter.tx.send(Update::Envelopes { id, envelopes });
            (reporter.wake)();
            Ok(())
        }
        Job::Decode { id, sources } => {
            // Decoded whole rather than streamed: a track is tens of megabytes
            // as f32, auditioning wants instant seeking anywhere in it, and the
            // analysers already decode the same way.
            let sound = Arc::new(decode_sources(&sources)?);
            let _ = reporter.tx.send(Update::Decoded { id, sound });
            (reporter.wake)();
            Ok(())
        }
        Job::Separate { tracks, stems_in, backend, quality } => {
            separate(&tracks, &stems_in, backend, quality, reporter)
        }
        Job::Transcribe { tracks, whisper } => transcribe(&tracks, &whisper, reporter),
        Job::Draw(rows) => draw(&rows, reporter),
        Job::Sync { args, files } => {
            // The file list was settled when the plan was drawn up, so the
            // command is told exactly what to write rather than walking a
            // folder again and possibly finding something else.
            let mut args = *args;
            let at = args
                .drive
                .as_ref()
                .or(args.image.as_ref())
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let bytes: u64 =
                files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
            crate::info!("writing {} files ({}) to {at}", files.len(), crate::sync::bytes(bytes));
            crate::debug!(
                "labelled {:?}, {} playlists, {} rows carried, {} companion pairs, {}",
                args.label,
                args.playlists.len(),
                args.already.len(),
                args.companions.len(),
                match args.dry_run {
                    true => "a dry run",
                    false => "for real",
                }
            );
            // The tree the player will show, list by list, because it is what
            // is being written and the easiest thing to get wrong without
            // noticing until the drive is in a booth.
            for spec in &args.playlists {
                crate::debug!(
                    "  playlist {}{} — {} entries",
                    match spec.folder.is_empty() {
                        true => String::new(),
                        false => format!("{}/", spec.folder),
                    },
                    spec.name,
                    spec.tracks.len()
                );
            }
            for file in &files {
                crate::debug!("  to prepare: {}", file.display());
            }
            for (path, _) in &args.already {
                crate::debug!("  carried: {}", path.display());
            }
            args.input = InputArgs { inputs: files, recursive: false };
            // The rows the drive ended up with, so the next write can carry
            // them rather than preparing everything again.
            let began = std::time::Instant::now();
            let outcome = booth_cli::commands::export(&args, reporter);
            match &outcome {
                Ok(rows) => crate::info!(
                    "wrote {at} in {:.1}s — {} rows on the drive",
                    began.elapsed().as_secs_f64(),
                    rows.len()
                ),
                Err(e) => crate::error!(
                    "writing {at} failed after {:.1}s: {e:#}",
                    began.elapsed().as_secs_f64()
                ),
            }
            let _ = reporter.tx.send(Update::Wrote(outcome?));
            (reporter.wake)();
            Ok(())
        }
    }
}

fn import(paths: &[PathBuf], recursive: bool, reporter: &Channel) -> anyhow::Result<()> {
    let files = booth_cli::discover::collect(paths, recursive)?;
    let total = files.len();
    crate::info!(
        "walked {} and found {}",
        crate::library::plural(paths.len(), "path"),
        crate::library::plural(total, "playable file")
    );
    for (done, path) in files.iter().enumerate() {
        if reporter.cancelled() {
            break;
        }
        // The id is filled in by the window, which owns the counter; zero here
        // means "not yet placed".
        let mut record = read_record(0, path, true);
        name_from_path(&mut record);
        crate::debug!(
            "imported {} ({}, {})",
            record.path.display(),
            record.format,
            crate::sync::bytes(record.bytes)
        );
        let _ = reporter.tx.send(Update::Imported(Box::new(record)));
        let _ = reporter.tx.send(Update::Progress { done: done + 1, total });
        (reporter.wake)();
    }
    Ok(())
}

/// Read the collection's files back and report where they disagree with it.
///
/// In parallel, like the other passes that only read: each file is a stat, a
/// tag read, and — when the check is thorough — every byte of it. The walk for
/// files nobody knows about comes last, because it is one pass over the folder
/// rather than one per track and there is no reason to make the per-file
/// answers wait for it.
fn verify_all(
    tracks: &[Track],
    deep: bool,
    library: &Path,
    reporter: &Channel,
) -> anyhow::Result<()> {
    use rayon::prelude::*;
    use std::sync::atomic::AtomicUsize;

    let total = tracks.len();
    let done = AtomicUsize::new(0);

    tracks.par_iter().for_each(|track| {
        if reporter.cancelled() {
            return;
        }
        let report = crate::verify::check(track, deep);
        // Only what has something to say. A collection that is entirely in
        // order should send nothing at all rather than one message per file
        // saying so.
        if !report.troubles.is_empty() {
            let _ = reporter.tx.send(Update::Verified(Box::new(report)));
        }
        let now = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let _ = reporter.tx.send(Update::Progress { done: now, total });
        (reporter.wake)();
    });

    if !reporter.cancelled() && !library.as_os_str().is_empty() {
        let known: Vec<PathBuf> = tracks.iter().map(|track| track.path.clone()).collect();
        let strays = crate::verify::orphans(library, &known);
        if !strays.is_empty() {
            let _ = reporter.tx.send(Update::Orphans(strays));
            (reporter.wake)();
        }
    }
    Ok(())
}

/// Hash tracks that have none, so they can be compared with the rest.
///
/// In parallel like analysis, and for the same reason: it is a read of every
/// byte of every file and nothing else, so the only limit worth having is the
/// disk's. Unlike analysis it does not decode, so a whole library is minutes
/// rather than hours.
fn hash_all(tracks: &[(u32, PathBuf)], reporter: &Channel) -> anyhow::Result<()> {
    use rayon::prelude::*;
    use std::sync::atomic::AtomicUsize;

    let total = tracks.len();
    let done = AtomicUsize::new(0);

    tracks.par_iter().for_each(|(id, path)| {
        if reporter.cancelled() {
            return;
        }
        // A file that will not open is not a failure worth stopping for — it
        // has bigger problems than being a copy, and an empty hash simply
        // never matches another. Both are sent even so, so that a track is
        // marked as looked at rather than asked about again every time.
        let file = booth_cli::hash::file_sha256(path).unwrap_or_else(|e| {
            crate::debug!("no file hash for {}: {e:#}", path.display());
            String::new()
        });
        let audio = booth_cli::hash::audio_sha256(path).unwrap_or_else(|e| {
            crate::debug!("no audio hash for {}: {e:#}", path.display());
            String::new()
        });
        let _ = reporter.tx.send(Update::Hashed { id: *id, file, audio });
        let now = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let _ = reporter.tx.send(Update::Progress { done: now, total });
        (reporter.wake)();
    });
    Ok(())
}

fn analyze_all(tracks: &[(u32, PathBuf)], reporter: &Channel) -> anyhow::Result<()> {
    use rayon::prelude::*;
    use std::sync::atomic::AtomicUsize;

    let total = tracks.len();
    let done = AtomicUsize::new(0);

    // In parallel, because analysis is the slow part of adding a crate and it
    // is pure measurement — nothing here writes anything.
    tracks.par_iter().for_each(|(id, path)| {
        if reporter.cancelled() {
            return;
        }
        let started = std::time::Instant::now();
        match analyze_file(*id, path) {
            Ok(analyzed) => {
                crate::debug!(
                    "listened to {} in {:.1}s",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    started.elapsed().as_secs_f32()
                );
                let _ = reporter.tx.send(Update::Analyzed(Box::new(analyzed)));
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: path.clone(), message: format!("{e:#}") });
            }
        }
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = reporter.tx.send(Update::Progress { done: finished, total });
        (reporter.wake)();
    });
    Ok(())
}

/// Copy files into the library folder, one at a time.
///
/// The originals are left alone. Nothing here deletes or moves anything: a
/// person who pointed at a folder of borrowed music should get their music
/// back exactly as they lent it.
fn adopt(tracks: &[Adoptable], config: &Config, reporter: &Channel) -> anyhow::Result<()> {
    let total = tracks.len();
    for (done, track) in tracks.iter().enumerate() {
        if reporter.cancelled() {
            break;
        }
        crate::debug!("copying {} in", track.path.display());
        match config::copy_in(config, &track.artist, &track.path) {
            // The window logs this one with where it landed, so there is no
            // line here: two records of one copy is one too many.
            Ok(to) => {
                let _ = reporter.tx.send(Update::Adopted { id: track.id, to });
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: track.path.clone(), message: format!("{e:#}") });
            }
        }
        let _ = reporter.tx.send(Update::Progress { done: done + 1, total });
        (reporter.wake)();
    }
    Ok(())
}

/// Re-encode a file into something the hardware opens, beside the original.
///
/// FLAC, always. The sources worth converting are a lossless format nothing
/// opens or a float WAV, and both deserve a lossless destination — re-encoding
/// somebody's master to MP3 to get it onto a stick is a decision they should
/// make deliberately rather than have made for them by a warning dialog.
///
/// The original is never touched. What comes back is a second file, and the
/// collection is repointed at it; if the conversion turns out to be wrong, the
/// thing it was made from is still there.
fn convert(tracks: &[Convertible], reporter: &Channel) -> anyhow::Result<()> {
    use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};

    let total = tracks.len();
    for (done, track) in tracks.iter().enumerate() {
        if reporter.cancelled() {
            break;
        }
        let result = (|| -> anyhow::Result<PathBuf> {
            let audio = decode_file(&track.path)?;
            let to = unused_path(&track.path.with_extension("flac"));
            crate::info!("converting {} to {}", track.path.display(), to.display());
            write_file(&to, &audio, Codec::Flac, &EncodeOptions::default())?;

            // The names come with it. A converted file that arrives untitled
            // would look like a different record sitting next to the original.
            let metadata = booth_cli::tag::read_metadata(&track.path).unwrap_or_default();
            let _ = booth_cli::tag::write_tags(
                &to,
                &metadata,
                booth_cli::tag::OnExisting::Overwrite,
                None,
            );
            Ok(to)
        })();

        match result {
            Ok(to) => {
                let _ = reporter.tx.send(Update::Converted { id: track.id, to });
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: track.path.clone(), message: format!("{e:#}") });
            }
        }
        let _ = reporter.tx.send(Update::Progress { done: done + 1, total });
        (reporter.wake)();
    }
    Ok(())
}

/// A path nothing is using yet, so a conversion never lands on top of a file
/// that is already there.
fn unused_path(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_path_buf();
    }
    let stem = wanted.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension =
        wanted.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = wanted.parent().unwrap_or(Path::new("."));
    for n in 2..1_000 {
        let candidate = parent.join(format!("{stem} ({n}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    wanted.to_path_buf()
}

/// Write the collection's names back into the files' own tags.
///
/// Whether an existing value is replaced or left alone is decided by the caller
/// and carried on each job: pressing the button in the inspector is an explicit
/// instruction and overwrites, while the automatic write-back behind a
/// fingerprint lookup only fills in blanks.
fn retag(tracks: &[Retag], reporter: &Channel) -> anyhow::Result<()> {
    let total = tracks.len();
    for (done, track) in tracks.iter().enumerate() {
        if reporter.cancelled() {
            break;
        }
        let metadata = booth_cli::tag::Metadata {
            title: non_empty(&track.title),
            artist: non_empty(&track.artist),
            album: non_empty(&track.album),
            date: track.date.clone(),
            ..Default::default()
        };
        match booth_cli::tag::write_tags(&track.path, &metadata, track.on_existing, None) {
            Ok(outcome) => {
                let fields: Vec<&str> = outcome.written.iter().map(|field| field.label()).collect();
                let _ = reporter.tx.send(Update::Line(format!(
                    "{}: {}",
                    track.path.file_name().unwrap_or_default().to_string_lossy(),
                    match fields.is_empty() {
                        true => "already tagged that way".to_string(),
                        false => format!("wrote {}", fields.join(", ")),
                    }
                )));
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: track.path.clone(), message: format!("{e:#}") });
            }
        }
        let _ = reporter.tx.send(Update::Progress { done: done + 1, total });
        (reporter.wake)();
    }
    Ok(())
}

fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A reporter that counts the batch itself, over a command that only knows
/// about the one file it was given.
///
/// Stem separation runs a track at a time so that a finished kit is usable
/// immediately, which means the command underneath is started once per track
/// and reports `1/1` every time. That is worse than no count at all: it looks
/// like the work restarts. So its own batch events are dropped and the count
/// comes from out here, where the length of the run is known.
struct Batch<'a> {
    inner: &'a Channel,
    done: std::sync::atomic::AtomicUsize,
    total: usize,
}

impl Batch<'_> {
    /// Show the count before the track begins, rather than after it ends: on a
    /// job of minutes a track, a counter that only moves on completion spends
    /// most of its life a track behind.
    fn starting(&self) {
        let done = self.done.load(Ordering::Relaxed);
        let _ = self.inner.tx.send(Update::Progress { done, total: self.total });
        (self.inner.wake)();
    }

    fn finished_one(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}

impl Reporter for Batch<'_> {
    fn event(&self, event: Event) {
        match event {
            // The command's own idea of the batch, which is one file long.
            Event::Started { .. } | Event::Progress { .. } | Event::Finished { .. } => {}
            other => self.inner.event(other),
        }
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
}

fn separate(
    tracks: &[(u32, PathBuf)],
    stems_in: &crate::config::StemsLocation,
    backend: Backend,
    quality: booth_cli::cli::StemQuality,
    reporter: &Channel,
) -> anyhow::Result<()> {
    let mut args = StemsArgs::defaults();
    args.backend = backend;
    args.quality = quality;
    crate::info!(
        "separating with {} shifts={}, writing {}{}",
        args.model(),
        args.shifts(),
        args.format.name(),
        match args.vbr() {
            Some(quality) => format!(" V{quality}"),
            None => format!(" {} kbps", args.bitrate),
        }
    );

    // One file at a time, so that a kit becomes available as soon as it is
    // rendered rather than at the end of the batch: a DJ waiting on stems for
    // one record should get that record back, not a progress bar.
    //
    // The count is kept here rather than left to the command, which is handed
    // one file at a time and would report 1/1 over and over. `Batch` hides the
    // command's own counting for the same reason, and lets its per-file
    // percentage through, which is the part worth seeing on a job this long.
    let batch = Batch {
        inner: reporter,
        done: std::sync::atomic::AtomicUsize::new(0),
        total: tracks.len(),
    };
    for (id, path) in tracks {
        if reporter.cancelled() {
            break;
        }
        batch.starting();
        args.input = InputArgs { inputs: vec![path.clone()], recursive: false };
        args.out_dir = stems_in.for_source(path);
        crate::info!(
            "separating {} into {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            args.out_dir.display()
        );
        if let Err(e) = std::fs::create_dir_all(&args.out_dir) {
            let _ = reporter.tx.send(Update::Failed {
                path: path.clone(),
                message: format!("cannot write to {}: {e}", args.out_dir.display()),
            });
            continue;
        }
        match booth_cli::commands::stems_files(&args, std::slice::from_ref(path), &batch) {
            Ok(()) => {
                let kit = find_stems(stems_in, path);
                crate::debug!(
                    "stems for #{id}: vocals {}, melody {}, drums {}",
                    kit.vocals.is_some(),
                    kit.melody.is_some(),
                    kit.drums.is_some()
                );
                // The pictures, here, while this track's stems are the most
                // recent thing that happened — not as a job queued behind the
                // rest of the batch.
                //
                // A kit rendered first in a queue of ten used to wait for the
                // other nine to separate before anything could draw it, which
                // is tens of minutes of a stem row that plays and shows
                // nothing. Drawn here it costs a decode of three files that
                // were written a second ago, against the minutes of separation
                // that just produced them.
                for (role, sources) in parts_of(&kit) {
                    draw_one(
                        &Drawable { id: crate::library::companion_id(*id, role), sources },
                        reporter,
                    );
                }
                let _ = reporter.tx.send(Update::Separated { id: *id, kit });
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: path.clone(), message: format!("{e:#}") });
            }
        }
        batch.finished_one();
        (reporter.wake)();
    }
    Ok(())
}

/// Read the words off each track's vocal stem.
///
/// One at a time, with a scratch directory each that is cleaned up either way:
/// the recogniser writes a converted copy of the audio next to its transcript,
/// and a job that leaves a wav per track behind in the temp directory is a job
/// that fills a disk over a library.
fn transcribe(
    tracks: &[Transcribable],
    whisper: &crate::config::Whisper,
    reporter: &Channel,
) -> anyhow::Result<()> {
    let batch = Batch {
        inner: reporter,
        done: std::sync::atomic::AtomicUsize::new(0),
        total: tracks.len(),
    };
    for track in tracks {
        if reporter.cancelled() {
            break;
        }
        batch.starting();

        let work = std::env::temp_dir().join(format!("booth-words-{}", track.id));
        let _ = std::fs::remove_dir_all(&work);
        let config = whisper.to_cli(work.clone());
        crate::info!(
            "reading the words off {}",
            track.vocals.file_name().unwrap_or_default().to_string_lossy()
        );

        let outcome =
            booth_cli::transcribe::whisper::transcribe(&track.vocals, &config, &|percent| {
                let _ = reporter.tx.send(Update::Step { percent });
                (reporter.wake)();
            });
        let _ = std::fs::remove_dir_all(&work);

        match outcome {
            Ok(transcript) => {
                crate::debug!(
                    "#{}: {}, hook {:?}",
                    track.id,
                    crate::library::plural(transcript.lines.len(), "line"),
                    transcript.hook().map(|refrain| refrain.text)
                );
                let lyrics = crate::library::lyrics_from(&transcript);
                let _ = reporter.tx.send(Update::Transcribed { id: track.id, lyrics });
            }
            Err(e) => {
                let _ = reporter
                    .tx
                    .send(Update::Failed { path: track.vocals.clone(), message: format!("{e:#}") });
            }
        }
        batch.finished_one();
        (reporter.wake)();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use booth_cli::audio::encode::{write_file, Codec, EncodeOptions};
    use booth_cli::audio::Audio;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("booth-job-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file with kicks on the beat, which is the least the analysers can work
    /// with: a bare tone has no onsets, and the tempo detector is right to
    /// refuse it rather than invent a grid.
    fn write_beats(path: &Path, bpm: f64, bars: usize) {
        let rate = 44_100usize;
        let period = 60.0 / bpm;
        let beats = bars * 4;
        let lead_in = rate / 2;
        let frames = lead_in + (rate as f64 * period * beats as f64) as usize + rate;
        let mut plane = vec![0.0f32; frames];
        for beat in 0..beats {
            let start = lead_in + (rate as f64 * period * beat as f64) as usize;
            let (hz, gain) = if beat % 4 == 0 { (55.0, 1.0) } else { (150.0, 0.5) };
            for i in 0..rate / 8 {
                let Some(sample) = plane.get_mut(start + i) else { break };
                let t = i as f32 / rate as f32;
                *sample += gain * (-30.0 * t).exp() * (std::f32::consts::TAU * hz * t).sin();
            }
        }
        let audio = Audio::new(rate as u32, vec![plane.clone(), plane]).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_file(path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();
    }

    /// A tone, for when what matters is which file the audio came from rather
    /// than what is in it.
    fn write_tone(path: &Path, hz: f32, gain: f32) {
        let rate = 44_100usize;
        let plane: Vec<f32> = (0..rate)
            .map(|i| gain * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect();
        let audio = Audio::new(rate as u32, vec![plane.clone(), plane]).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_file(path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();
    }

    #[test]
    fn a_picture_is_of_the_files_it_was_given_and_not_of_a_track() {
        // What a stem companion needs: it carries its parent's path, so
        // anything that drew from that drew the whole mix under the acapella's
        // name.
        let dir = scratch("drawing");
        let bass = dir.join("bass.wav");
        let top = dir.join("top.wav");
        write_tone(&bass, 60.0, 0.8);
        write_tone(&top, 8_000.0, 0.8);

        let low = decode_sum(std::slice::from_ref(&bass)).unwrap();
        let high = decode_sum(std::slice::from_ref(&top)).unwrap();
        let both = decode_sum(&[bass.clone(), top.clone()]).unwrap();

        let bands = |audio: &Audio| {
            let drawn = booth_cli::export::waveform::analyze(audio).band_detail;
            // Three bytes a column — mid, high, low, as the format stores them.
            let at = drawn.len() / 2 / 3 * 3;
            (drawn[at], drawn[at + 1], drawn[at + 2])
        };

        let (_, _, only_bass) = bands(&low);
        let (_, top_high, _) = bands(&high);
        assert!(only_bass > 0, "a bass tone should have a low band");
        assert!(top_high > 0, "a treble tone should have a high band");

        // Summed, the picture has both in it — which is what an instrumental
        // is, and is not what either file alone looks like.
        let (_, mixed_high, mixed_low) = bands(&both);
        assert!(mixed_low > 0 && mixed_high > 0, "the sum lost a band: {:?}", bands(&both));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_finished_kit_names_a_picture_for_each_of_its_parts() {
        // Drawn as the kit lands, not queued behind the rest of the batch: a
        // kit rendered first in a queue of ten used to wait for the other nine
        // to separate before anything could draw it, which is tens of minutes
        // of a stem row that plays and shows nothing.
        let dir = scratch("parts");
        let kit = StemKit {
            vocals: Some(dir.join("v.mp3")),
            drums: Some(dir.join("d.mp3")),
            melody: Some(dir.join("m.mp3")),
        };

        let parts = parts_of(&kit);
        let roles: Vec<crate::library::Role> = parts.iter().map(|(role, _)| *role).collect();
        assert_eq!(roles, crate::library::Role::PARTS.to_vec());
        // Each from its own single file — a companion is one stem, never a sum.
        assert!(parts.iter().all(|(_, sources)| sources.len() == 1));
        assert_eq!(parts[0].1, vec![dir.join("v.mp3")]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_half_rendered_kit_has_no_rows_to_draw() {
        // A companion row only exists once all three parts do, so there is
        // nothing for a picture to belong to.
        let kit = StemKit { vocals: Some(PathBuf::from("v.mp3")), ..StemKit::default() };
        assert!(parts_of(&kit).is_empty());
        assert!(parts_of(&StemKit::default()).is_empty());
    }

    #[test]
    fn drawing_nothing_is_an_error_rather_than_an_empty_picture() {
        // A companion whose kit has no such part has no sources, and a blank
        // picture would look exactly like a quiet one.
        assert!(decode_sum(&[]).is_err());
    }

    #[test]
    fn a_record_is_read_without_decoding_the_audio() {
        let dir = scratch("record");
        let path = dir.join("track.wav");
        write_beats(&path, 128.0, 4);

        let track = read_record(7, &path, true);
        assert_eq!(track.id, 7);
        assert_eq!(track.format, "wav");
        assert_eq!(track.title, "track", "the file name, until a tag says otherwise");
        assert!(track.bytes > 0);
        assert!(!track.analyzed, "reading a record is not analysing it");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_float_wav_is_spotted_from_its_header() {
        let dir = scratch("float");
        let integer = dir.join("integer.wav");
        write_beats(&integer, 128.0, 1);
        assert!(!is_float_wav(&integer));

        // Hand-built, because the encoder here does not write float files —
        // they arrive from other people's exports, which is the whole problem.
        let float = dir.join("float.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&352_800u32.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        std::fs::write(&float, &bytes).unwrap();

        assert!(is_float_wav(&float));
        assert!(read_record(1, &float, false).float_samples);
        // And it is exactly the kind of thing the sidebar counts.
        assert!(read_record(1, &float, false)
            .needs_attention(booth_cli::compat::Player::default())
            .is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_flac_is_never_a_float_wav() {
        let dir = scratch("not-wav");
        let path = dir.join("track.flac");
        write_beats(&dir.join("source.wav"), 128.0, 1);
        std::fs::copy(dir.join("source.wav"), &path).unwrap();
        // A .flac extension short-circuits before the header is even read.
        assert!(!is_float_wav(&path));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn analysis_produces_everything_a_row_needs() {
        let dir = scratch("analyze");
        let path = dir.join("track.wav");
        write_beats(&path, 128.0, 8);

        let analyzed = analyze_file(3, &path).unwrap();
        assert_eq!(analyzed.id, 3);
        assert!(analyzed.has_grid);
        assert!((analyzed.bpm - 128.0).abs() < 1.0, "{} BPM", analyzed.bpm);
        assert!(analyzed.beats > 0);
        assert!(analyzed.energy >= 1 && analyzed.energy <= 5);
        assert!(analyzed.loudness_lufs.is_some());
        assert_eq!(analyzed.sample_rate, 44_100);
        assert_eq!(analyzed.channels, 2);
        // The waveform is kept at the scrolling resolution rather than the
        // preview's fixed 1,200 columns, so that zooming in has something to
        // find: 150 columns a second, three bytes each.
        assert_eq!(analyzed.bands.len() % 3, 0);
        let columns = analyzed.bands.len() / 3;
        let expected = (analyzed.duration_secs * 150.0).round() as usize;
        assert!(
            columns.abs_diff(expected) <= 2,
            "{columns} columns for {:.2}s, expected about {expected}",
            analyzed.duration_secs
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn phrases_are_placed_in_milliseconds_rather_than_beats() {
        let dir = scratch("phrases");
        let path = dir.join("track.wav");
        write_beats(&path, 128.0, 16);

        let analyzed = analyze_file(1, &path).unwrap();
        for phrase in &analyzed.phrases {
            assert!(phrase.end_ms >= phrase.start_ms, "{phrase:?}");
            assert!(
                (phrase.end_ms as f64) <= analyzed.duration_secs * 1000.0 + 1_000.0,
                "a phrase ran past the end of the track: {phrase:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_track_with_no_beat_reports_that_rather_than_guessing() {
        let dir = scratch("drone");
        let path = dir.join("drone.wav");
        let rate = 44_100;
        let samples: Vec<f32> = (0..rate * 8)
            .map(|i| 0.2 * (std::f32::consts::TAU * 220.0 * i as f32 / rate as f32).sin())
            .collect();
        let audio = Audio::new(rate as u32, vec![samples.clone(), samples]).unwrap();
        write_file(&path, &audio, Codec::Wav, &EncodeOptions::default()).unwrap();

        let analyzed = analyze_file(1, &path).unwrap();
        assert!(!analyzed.has_grid);
        assert_eq!(analyzed.beats, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stem_kit_is_found_by_looking_rather_than_by_remembering() {
        use crate::config::{StemsIn, StemsLocation};
        let dir = scratch("stems");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let source = dir.join("Roll With The Punches.flac");
        let folder = StemsLocation { in_: StemsIn::Folder, folder: out.clone() };

        assert!(find_stems(&folder, &source).is_empty());

        for stem in ["vocals", "melody"] {
            std::fs::write(out.join(format!("Roll With The Punches-{stem}.wav")), b"").unwrap();
        }
        let kit = find_stems(&folder, &source);
        assert!(kit.vocals.is_some() && kit.melody.is_some());
        assert!(!kit.is_complete(), "two of three is not a kit");

        std::fs::write(out.join("Roll With The Punches-drums.wav"), b"").unwrap();
        assert!(find_stems(&folder, &source).is_complete());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn changing_where_stems_go_does_not_lose_the_ones_already_rendered() {
        use crate::config::{StemsIn, StemsLocation};
        let dir = scratch("stems-moved");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let source = dir.join("Sirens.flac");
        for stem in ["vocals", "melody", "drums"] {
            std::fs::write(out.join(format!("Sirens-{stem}.mp3")), b"").unwrap();
        }

        // Rendered into the folder, then the setting changed to beside. A kit
        // is minutes of work; switching a preference must not appear to delete
        // one.
        let beside = StemsLocation { in_: StemsIn::Beside, folder: out.clone() };
        assert!(find_stems(&beside, &source).is_complete());

        // And the other way round: rendered beside a track, then the setting
        // changed to a folder that has never had anything put in it.
        let beside_source = dir.join("Vessel.flac");
        for stem in ["vocals", "melody", "drums"] {
            std::fs::write(dir.join(format!("Vessel-{stem}.mp3")), b"").unwrap();
        }
        let elsewhere = dir.join("far-away");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let folder = StemsLocation { in_: StemsIn::Folder, folder: elsewhere };
        assert!(find_stems(&folder, &beside_source).is_complete());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_import_reports_every_file_it_walked_to() {
        let dir = scratch("import");
        write_beats(&dir.join("a.wav"), 128.0, 1);
        write_beats(&dir.join("nested").join("b.wav"), 128.0, 1);

        let mut runner = Runner::start(
            Job::Import { paths: vec![dir.clone()], recursive: true },
            Arc::new(|| {}),
        );
        runner.join();

        let updates = runner.drain();
        let names: Vec<String> = updates
            .iter()
            .filter_map(|u| match u {
                Update::Imported(track) => {
                    Some(track.path.file_name()?.to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_analysis_job_reports_one_result_per_track_and_finishes() {
        let dir = scratch("analyze-job");
        let a = dir.join("a.wav");
        let b = dir.join("b.wav");
        write_beats(&a, 128.0, 4);
        write_beats(&b, 140.0, 4);

        let mut runner = Runner::start(Job::Analyze(vec![(1, a), (2, b)]), Arc::new(|| {}));
        runner.join();

        let updates = runner.drain();
        let mut ids: Vec<u32> = updates
            .iter()
            .filter_map(|u| match u {
                Update::Analyzed(analyzed) => Some(analyzed.id),
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_will_not_decode_fails_alone() {
        let dir = scratch("bad");
        let good = dir.join("good.wav");
        let bad = dir.join("bad.wav");
        write_beats(&good, 128.0, 4);
        std::fs::write(&bad, b"not a wav").unwrap();

        let mut runner = Runner::start(Job::Analyze(vec![(1, good), (2, bad)]), Arc::new(|| {}));
        runner.join();

        let updates = runner.drain();
        assert!(updates.iter().any(|u| matches!(u, Update::Analyzed(a) if a.id == 1)));
        assert!(updates.iter().any(|u| matches!(u, Update::Failed { .. })));
        // The job as a whole still ends tidily: one bad file is not a crash.
        assert!(matches!(updates.last(), Some(Update::Done(Ok(())))));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_stops_the_run() {
        let dir = scratch("cancel");
        for i in 0..4 {
            write_beats(&dir.join(format!("{i}.wav")), 128.0, 4);
        }

        let mut runner = Runner::start(
            Job::Import { paths: vec![dir.clone()], recursive: false },
            Arc::new(|| {}),
        );
        runner.cancel();
        runner.join();

        // However far it got, the job ends tidily rather than half-way.
        assert!(matches!(runner.drain().last(), Some(Update::Done(Ok(())))));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
