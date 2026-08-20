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

    /// A grid from measured beat times, in milliseconds, assuming the first is
    /// a downbeat.
    ///
    /// The tempo written against each beat is the one implied by the gap to the
    /// next beat, so a track that drifts is described as drifting rather than
    /// averaged into a lie. The last beat inherits the tempo of the one before
    /// it, there being no following gap to measure.
    pub fn from_beat_times(times: &[u32]) -> Self {
        let mut beats = Vec::with_capacity(times.len());
        for (i, &t) in times.iter().enumerate() {
            let gap = if i + 1 < times.len() {
                times[i + 1].saturating_sub(t)
            } else if i > 0 {
                t.saturating_sub(times[i - 1])
            } else {
                0
            };
            let bpm = if gap > 0 { 60_000.0 / gap as f64 } else { 0.0 };
            beats.push(Beat {
                number: (i % 4) as u16 + 1,
                tempo_x100: (bpm * 100.0).round().clamp(0.0, u16::MAX as f64) as u16,
                time_ms: t,
            });
        }
        Self { beats }
    }

    pub fn is_empty(&self) -> bool {
        self.beats.is_empty()
    }
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
    fn measured_grid_records_the_tempo_of_each_gap() {
        // A track that slows down: 500 ms, then 500, then 600.
        let grid = BeatGrid::from_beat_times(&[0, 500, 1000, 1600]);
        assert_eq!(grid.beats[0].tempo_x100, 12_000);
        assert_eq!(grid.beats[2].tempo_x100, 10_000);
        // The last beat has no following gap, so it keeps the previous tempo.
        assert_eq!(grid.beats[3].tempo_x100, 10_000);
    }

    #[test]
    fn zero_tempo_yields_no_beats_rather_than_looping_forever() {
        assert!(BeatGrid::constant(0.0, 0, 10_000).is_empty());
    }
}
