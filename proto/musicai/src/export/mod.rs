//! Writing the files a Pioneer/AlphaTheta player reads off a USB drive.
//!
//! A prepared drive holds two things: a database of tracks and playlists, and
//! one *analysis file* per track carrying the beat grid, the cues and the
//! waveforms. This module is the analysis half. The database half (`export.pdb`)
//! comes next.
//!
//! The format is not ours and is not published by its vendor. It has been
//! reverse-engineered in public and in detail — see Deep Symmetry's [DJ Link
//! Ecosystem Analysis], and the Kaitai structures behind `crate-digger`, which
//! are what this implementation is written against. Everything here is
//! big-endian, because the players are.
//!
//! [DJ Link Ecosystem Analysis]: https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html

pub mod anlz;
pub mod image;
pub mod onelibrary;
pub mod pdb;
pub mod waveform;

/// One beat of a beat grid.
///
/// The grid is a list of beats rather than a tempo and an offset, which is why
/// variable tempo costs nothing to represent: each beat carries the tempo that
/// applies at the moment it happens.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Beat {
    /// Position within the bar, 1 to 4, where 1 is the downbeat.
    pub number: u16,
    /// Tempo at this beat, in BPM times 100.
    pub tempo_x100: u16,
    /// When the beat happens, in milliseconds from the start of the track.
    pub time_ms: u32,
}

/// The period a straight line through these beat times implies, by least
/// squares. `None` when there are too few to fit one or they do not advance.
fn fitted_period(times: &[u32]) -> Option<f64> {
    if times.len() < 2 {
        return None;
    }
    let n = times.len() as f64;
    let mean_index = (n - 1.0) / 2.0;
    let mean_time = times.iter().map(|&t| t as f64).sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut variance = 0.0;
    for (i, &time) in times.iter().enumerate() {
        let di = i as f64 - mean_index;
        covariance += di * (time as f64 - mean_time);
        variance += di * di;
    }
    if variance <= 0.0 {
        return None;
    }
    let period = covariance / variance;
    (period > 0.0).then_some(period)
}

/// Whether beat times bend, meaning no single tempo places them all.
///
/// Measured against the even grid through the two ends, so a grid that speeds
/// up and slows back down is caught by the bulge in the middle rather than
/// passed because it started and finished in the right place.
///
/// Five milliseconds, because an even grid rounded to whole milliseconds is
/// already off by up to one — the analyser here measures 0.85 ms across a
/// three-minute track — and five is far below the point where a beat sounds
/// like it is in a different place. Under three beats nothing can be said, and
/// nothing is: two points always fit a line.
pub fn bends(times: &[u32]) -> bool {
    const ROOM_MS: f64 = 5.0;
    if times.len() < 3 {
        return false;
    }
    let last = times.len() - 1;
    let period = (times[last] as f64 - times[0] as f64) / last as f64;
    times
        .iter()
        .enumerate()
        .any(|(i, at)| (*at as f64 - (times[0] as f64 + period * i as f64)).abs() > ROOM_MS)
}

/// The beats of a track, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BeatGrid {
    pub beats: Vec<Beat>,
}

impl BeatGrid {
    /// A grid at one steady tempo, starting at `first_beat_ms` and running to
    /// the end of a track `duration_ms` long.
    ///
    /// This is the grid a fixed-tempo analyser produces, and it is what the
    /// first drives will be built from. It is deliberately not the only
    /// constructor: [`BeatGrid::from_beat_times`] takes the output of an
    /// analyser that tracked a drifting tempo, which is the case this format
    /// handles well and most DJ software handles badly.
    pub fn constant(bpm: f64, first_beat_ms: u32, duration_ms: u32) -> Self {
        let mut beats = Vec::new();
        if bpm <= 0.0 {
            return Self { beats };
        }
        let period = 60_000.0 / bpm;
        let tempo_x100 = (bpm * 100.0).round().clamp(0.0, u16::MAX as f64) as u16;
        let mut n = 0u64;
        loop {
            let t = first_beat_ms as f64 + period * n as f64;
            if t > duration_ms as f64 {
                break;
            }
            beats.push(Beat { number: (n % 4) as u16 + 1, tempo_x100, time_ms: t.round() as u32 });
            n += 1;
        }
        Self { beats }
    }

    /// Whether this grid bends: whether it says anything a tempo and a
    /// downbeat could not say on their own.
    ///
    /// What it is for is deciding whether a grid is worth keeping beat by
    /// beat. A drive always gets every beat written out — that is what a
    /// player reads — but a collection holding thousands of numbers to repeat
    /// what two of them already say is a collection doing the analysis file's
    /// job.
    pub fn bends(&self) -> bool {
        let times: Vec<u32> = self.beats.iter().map(|beat| beat.time_ms).collect();
        bends(&times)
    }

    /// The beat times from the first downbeat on.
    ///
    /// From the downbeat because everything that counts in fours counts from
    /// the first beat of the list — the bar marks, the bar number in the
    /// transport, the length of a phrase — so the list has to start on one.
    /// The beats before it are the part bar at the head of the track, which
    /// a player does not number either.
    pub fn times_from_downbeat(&self) -> Vec<u32> {
        let first = self.beats.iter().position(|beat| beat.number == 1).unwrap_or(0);
        self.beats[first..].iter().map(|beat| beat.time_ms).collect()
    }

    /// A grid from measured beat times, in milliseconds, assuming the first is
    /// a downbeat.
    ///
    /// The tempo written against each beat comes from a line fitted through the
    /// beats around it, so a track that drifts is described as drifting rather
    /// than averaged into a lie — and a track that does not is described at one
    /// tempo rather than at whatever the rounding of two beat times implies.
    ///
    /// Taking it from the single gap to the next beat, which is what this did,
    /// reads the tempo off numbers stored to the millisecond: at 128 BPM the
    /// beats fall 468.75 ms apart, the stored gaps alternate 468 and 469, and
    /// the tempo written alternates 128.21 and 127.93. A quarter of a BPM of
    /// jitter, beat by beat, on a track that never changed tempo.
    pub fn from_beat_times(times: &[u32]) -> Self {
        /// Beats either side of one that are used to read the tempo at it.
        const AROUND: usize = 6;

        let beats = times
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let from = i.saturating_sub(AROUND);
                let to = (i + AROUND + 1).min(times.len());
                let bpm = fitted_period(&times[from..to]).map(|p| 60_000.0 / p).unwrap_or(0.0);
                Beat {
                    number: (i % 4) as u16 + 1,
                    tempo_x100: (bpm * 100.0).round().clamp(0.0, u16::MAX as f64) as u16,
                    time_ms: t,
                }
            })
            .collect();
        Self { beats }
    }

    pub fn is_empty(&self) -> bool {
        self.beats.is_empty()
    }
}

/// What a caller already knows about a track, for the exporter to write rather
/// than measure its own.
///
/// The exporter listens to every file it prepares, because it has to: the
/// waveform is of the audio and nothing else can supply it. But a collection
/// that has been kept by a person holds answers the audio does not — a cue
/// moved by hand, a section renamed, a key corrected — and an exporter that
/// measures its own and writes those instead is an exporter that quietly
/// discards the work. So the measured answers are the fallback and these are
/// what is written where they exist.
///
/// Every field is optional in the sense that an empty one means "you decide".
/// A caller with nothing to say passes `Prep::default()`, or does not pass one
/// at all, and gets exactly what it got before this existed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prep {
    /// The cues as the collection has them, memory and hot together, in
    /// milliseconds. Empty means the exporter suggests its own.
    pub cues: Vec<Cue>,
    /// The sections as the collection has them. Empty means the exporter finds
    /// its own.
    pub parts: Vec<Part>,
    /// The tempo the collection has. The beats are still tracked rather than
    /// assumed from it — a tempo says how far apart they are, not where they
    /// fall. `None` lets the tracker decide the tempo too.
    pub bpm: Option<f64>,
    /// The key as the collection has it, e.g. `8A`. Empty means the exporter
    /// detects its own.
    pub key: String,
    /// Every beat, for a track whose grid bends and could not be rebuilt from
    /// `bpm`. Empty means the tracker's own grid is used, which is the case for
    /// almost every record.
    ///
    /// What reaches the drive is a full beat list either way — a player reads
    /// beats, not tempos. This only decides whose beats they are, and it exists
    /// because a grid somebody bent by hand on a player is not something to
    /// measure over.
    pub beat_ms: Vec<u32>,
}

impl Prep {
    /// Whether there is anything here worth preferring to a measurement.
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
            && self.parts.is_empty()
            && self.bpm.is_none()
            && self.key.is_empty()
            && self.beat_ms.is_empty()
    }
}

/// One section of a track, in milliseconds, as a collection keeps it.
///
/// Milliseconds rather than beat numbers because that is what a collection can
/// keep without also keeping the whole grid, and because a section's position
/// in a record does not change when the grid is re-measured. Turning it back
/// into the beat number the analysis file wants is the exporter's job, against
/// the grid it is actually writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub start_ms: u32,
    pub end_ms: u32,
    /// `intro`, `build`, `break`, `drop` or `outro`, as
    /// [`crate::analysis::structure::Kind::label`] writes them.
    pub kind: String,
}

/// Whether a cue is a point to jump to or a loop to fall into.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CueKind {
    Point,
    /// A loop, ending at the given millisecond position.
    Loop {
        end_ms: u32,
    },
}

/// A colour, as the extended cue format stores it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A cue point, memory cue or hot cue.
///
/// `hot_cue` is 0 for a memory cue, or 1 to 8 for hot cues A through H. The two
/// kinds live in separate sections of the analysis file, and [`anlz`] splits a
/// single list into the right two on the way out, so callers never have to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub hot_cue: u8,
    pub kind: CueKind,
    pub time_ms: u32,
    pub color: Option<Rgb>,
    pub comment: Option<String>,
    /// Loop length in beats as a fraction, for a quantised loop.
    pub loop_beats: Option<(u16, u16)>,
}

impl Cue {
    pub fn memory(time_ms: u32) -> Self {
        Self {
            hot_cue: 0,
            kind: CueKind::Point,
            time_ms,
            color: None,
            comment: None,
            loop_beats: None,
        }
    }

    /// A hot cue. `letter` is 1 for A through 8 for H.
    pub fn hot(letter: u8, time_ms: u32) -> Self {
        Self { hot_cue: letter, ..Self::memory(time_ms) }
    }

    pub fn with_comment(mut self, comment: &str) -> Self {
        self.comment = Some(comment.to_string());
        self
    }

    pub fn with_color(mut self, r: u8, g: u8, b: u8) -> Self {
        self.color = Some(Rgb { r, g, b });
        self
    }

    pub fn looping(mut self, end_ms: u32) -> Self {
        self.kind = CueKind::Loop { end_ms };
        self
    }

    pub fn is_hot(&self) -> bool {
        self.hot_cue != 0
    }
}

/// How rekordbox labels the phrases of a track. The mood decides which set of
/// phrase names applies, so it has to be chosen before the phrases are.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mood {
    High = 1,
    Mid = 2,
    Low = 3,
}

/// One phrase of a track's structure, starting at a beat number.
///
/// `kind` is the phrase label within the track's mood: in [`Mood::Mid`], 1 is
/// Intro, 2 to 7 are Verse 1 to Verse 6, 8 is Bridge, 9 is Chorus and 10 is
/// Outro.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Phrase {
    pub beat: u16,
    pub kind: u16,
}

/// The phrase analysis of a track, as the CDJ-3000 draws it under the waveform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongStructure {
    pub mood: Mood,
    /// The beat at which the last phrase ends. What follows is usually silence.
    pub end_beat: u16,
    /// The lighting style bank, 0 for the default.
    pub bank: u8,
    pub phrases: Vec<Phrase>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_at_one_tempo_does_not_count_as_bending() {
        // Including the rounding to whole milliseconds, which is the only
        // unevenness an even grid has: the analyser here measures 0.85 ms of
        // it across three minutes, and that must not read as a bent grid or
        // every track would be kept beat by beat.
        for bpm in [120.0, 126.0, 128.0, 174.0, 89.7] {
            let grid = BeatGrid::constant(bpm, 17, 300_000);
            assert!(!grid.bends(), "{bpm} BPM read as bent");
        }
    }

    #[test]
    fn a_grid_that_drifts_bends() {
        // A take that creeps: each beat a millisecond and a half shorter than
        // the last, which is a band speeding up and nothing a single tempo
        // describes.
        let mut times = Vec::new();
        let (mut at, mut gap) = (0.0f64, 500.0f64);
        for _ in 0..64 {
            times.push(at.round() as u32);
            at += gap;
            gap -= 1.5;
        }
        assert!(bends(&times));
    }

    #[test]
    fn a_grid_that_speeds_up_and_comes_back_bends() {
        // The two ends land exactly where a steady tempo would, so anything
        // measuring only the ends passes it. What gives it away is the middle.
        let mut times = Vec::new();
        for i in 0..65u32 {
            let even = i as f64 * 500.0;
            let bulge = (i as f64 / 64.0 * std::f64::consts::PI).sin() * 90.0;
            times.push((even + bulge).round() as u32);
        }
        assert_eq!(times[0], 0);
        assert_eq!(*times.last().unwrap(), 32_000, "the ends fit a steady tempo");
        assert!(bends(&times), "the bulge in the middle was missed");
    }

    #[test]
    fn too_few_beats_say_nothing_about_bending() {
        // Two points fit a line, so there is nothing to disagree with yet.
        assert!(!bends(&[]));
        assert!(!bends(&[1_000]));
        assert!(!bends(&[1_000, 1_500]));
    }

    #[test]
    fn the_times_kept_start_at_the_first_downbeat() {
        // Everything downstream counts in fours from the first beat of the
        // list, so a list starting on beat three would put every bar mark,
        // bar number and phrase length two beats out.
        let mut grid = BeatGrid::constant(120.0, 0, 10_000);
        grid.beats.drain(..2);
        assert_eq!(grid.beats[0].number, 3, "the fixture did not start off the bar");

        // Beats now run 3, 4, 1, 2, ... so the first downbeat is two beats
        // along, at two seconds, and the two before it are dropped.
        let times = grid.times_from_downbeat();
        assert_eq!(times[0], 2_000, "it did not skip to the downbeat");
        assert_eq!(times[1], 2_500);
    }

    #[test]
    fn constant_grid_counts_beats_and_marks_bars() {
        // 120 BPM is a beat every 500 ms, so ten seconds is 21 beats counting
        // the one at zero.
        let grid = BeatGrid::constant(120.0, 0, 10_000);
        assert_eq!(grid.beats.len(), 21);
        assert_eq!(grid.beats[0].number, 1);
        assert_eq!(grid.beats[4].number, 1);
        assert_eq!(grid.beats[1].time_ms, 500);
        assert_eq!(grid.beats[0].tempo_x100, 12_000);
    }

    #[test]
    fn constant_grid_starts_where_told() {
        let grid = BeatGrid::constant(128.0, 412, 5_000);
        assert_eq!(grid.beats[0].time_ms, 412);
        assert!(grid.beats.last().unwrap().time_ms <= 5_000);
    }

    #[test]
    fn a_steady_run_of_beats_is_written_at_one_tempo() {
        // Taking the tempo from the single gap that follows each beat read it
        // off numbers stored to the millisecond: 128 BPM is 468.75 ms, the
        // stored gaps alternate 468 and 469, and the tempo came out
        // alternating 128.21 and 127.93. A quarter of a BPM of jitter on a
        // track that never changed tempo, and a player's readout that will not
        // hold still.
        let period = 60_000.0 / 128.0;
        let times: Vec<u32> = (0..64).map(|i| (period * i as f64).round() as u32).collect();
        let grid = BeatGrid::from_beat_times(&times);

        let tempos: std::collections::BTreeSet<u16> =
            grid.beats.iter().map(|beat| beat.tempo_x100).collect();
        // Every beat within a hundredth of a BPM of 128, which is the unit the
        // format stores: there is nowhere further to go. Read off the single
        // following gap, these spanned 12_793 to 12_821.
        for tempo in &tempos {
            assert!(tempo.abs_diff(12_800) <= 1, "{tempos:?} is not 128 BPM");
        }
    }

    #[test]
    fn a_track_that_slows_down_is_written_slowing_down() {
        // The other half: smoothing must not flatten a record that really does
        // change. A ramp from 128 to 120 over a couple of minutes should come
        // out as a tempo that falls across the track.
        let mut times = vec![0u32];
        let mut at = 0.0f64;
        for i in 0..256 {
            let bpm = 128.0 - 8.0 * (i as f64 / 256.0);
            at += 60_000.0 / bpm;
            times.push(at.round() as u32);
        }
        let grid = BeatGrid::from_beat_times(&times);

        let first = grid.beats[8].tempo_x100;
        let last = grid.beats[grid.beats.len() - 9].tempo_x100;
        assert!(first > 12_700, "it should start near 128: {first}");
        assert!(last < 12_100, "and end near 120: {last}");
        // And fall the whole way rather than in two or three steps, which is
        // what reading the median of a handful of quantised gaps gave.
        let steps = grid
            .beats
            .iter()
            .map(|beat| beat.tempo_x100)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(steps.len() > 20, "only {} distinct tempos across the ramp", steps.len());
    }

    #[test]
    fn zero_tempo_yields_no_beats_rather_than_looping_forever() {
        assert!(BeatGrid::constant(0.0, 0, 10_000).is_empty());
    }
}
