//! Finding the beats.
//!
//! Three steps, all of them standard and none of them a model. The onset
//! envelope from [`super::features`] says where the music pushed; its
//! autocorrelation says how often, which is the tempo; and a dynamic program
//! then picks the sequence of beat times that best balances landing on those
//! onsets against staying in time. The approach is Ellis's *Beat Tracking by
//! Dynamic Programming* (2007), which is old, cheap and hard to beat without
//! reaching for a neural network.
//!
//! Most of what a DJ plays was made in a sequencer at one tempo, and for that
//! there is a fourth: the whole record is folded onto one steady beat and the
//! grid is written from it, so that a stretch where the tracker lost its way —
//! a breakdown with no kick to follow — cannot move every bar line after it.
//! See [`hold`].
//!
//! The downbeat is a separate question, answered separately: of the four
//! possible places the bar could start, the one the kick or the harmony is
//! surer of, among those the backbeat leaves. See [`downbeat`].

use crate::export::{Beat, BeatGrid};

use super::features::{Features, PITCHES};

/// The slowest and fastest tempo considered. Outside this a track is better
/// described at half or double the rate, and every DJ tool makes the same
/// assumption.
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 200.0;
/// The tempo a listener assumes in the absence of evidence, and how far from it
/// the prior stays open, in octaves. Ellis's figures.
const PRIOR_CENTRE_BPM: f64 = 120.0;
const PRIOR_WIDTH_OCTAVES: f64 = 1.4;
/// How strongly the beat tracker resists a gap that is not one period. Larger
/// values keep stricter time at the cost of following a real tempo change.
const TIGHTNESS: f32 = 100.0;
/// Beats either side of each one, over which its tempo is measured. A single
/// gap is too noisy to quote as a BPM.
/// How many beats either side of one are used to read the tempo at it.
///
/// Six either side is thirteen beats, about five seconds of music. Long enough
/// that fitting a line through them averages out the frame the tracker reports
/// in — a dozen points cut the quantisation noise by more than three — and
/// short enough to follow a record that is really speeding up.
const TEMPO_WINDOW: usize = 6;
/// How periodic the onsets have to be before a tempo is believed: the winning
/// autocorrelation peak against the average across all candidate tempos.
const MIN_CONFIDENCE: f32 = 1.5;
/// And how much onset there has to be in the first place. A track with no
/// transients still has a most-periodic lag; without this floor, the numerical
/// wobble of a sustained drone gets normalised up into a convincing grid.
const MIN_ONSET_STRENGTH: f32 = 1.0;

/// What the tracker concluded.
pub struct Beats {
    pub grid: BeatGrid,
    /// The single tempo the whole track was tracked at, in BPM.
    pub bpm: f64,
    /// How clear the tempo was: the height of the winning autocorrelation peak
    /// against the average. Around 1.0 means no rhythm was found at all;
    /// a four-to-the-floor track measures several times that.
    pub confidence: f32,
    /// What the bar lines were placed by, kept so that a downbeat in the wrong
    /// place can be traced to the beats that put it there.
    pub downbeat: Downbeat,
    /// Where the tempo was moved an octave to the one its genre is counted in,
    /// what it was heard at first and why.
    pub moved: Option<Moved>,
}

/// The octave a record's genre is counted in: from `from_bpm` up to twice it,
/// so that of any tempo exactly one octave is inside.
///
/// Which octave of a tempo is the beat is a convention as much as a
/// measurement. Hip hop at 86 is counted at 86 and drum and bass at 172 at 172,
/// though a record of either can repeat as strongly at the other or more so,
/// and the two are as far either side of the 120 the prior leans towards. The
/// genre is what the convention follows, so where the genre is known it says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Octave {
    pub from_bpm: f64,
    /// What it is the octave of, for the log: "drum and bass".
    pub because: &'static str,
}

/// A tempo moved into its genre's octave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    /// The tempo of the lag first chosen, to the frame.
    pub heard_bpm: f64,
    pub octave: Octave,
}

/// The evidence for which beat is the one, beat by beat, what it came to, and
/// which of it decided.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Downbeat {
    /// Each beat of the grid: milliseconds from the start of the track.
    pub times_ms: Vec<u32>,
    /// The low band's sharpest onset around each of those beats — the kick,
    /// where there is one.
    pub kicks: Vec<f32>,
    /// The sharpest onset from 1 to 5 kHz around each — a clap or a snare,
    /// where there is one.
    pub claps: Vec<f32>,
    /// How far the bass note moved across each beat: the two beats before it
    /// against the two from it.
    pub bass_changes: Vec<f32>,
    /// How far the chord moved across each: the bar before it against the bar
    /// from it.
    pub chord_changes: Vec<f32>,
    /// Which of the four positions won: the grid's beats `phase`, `phase + 4`,
    /// `phase + 8` and so on are the downbeats.
    pub phase: usize,
    /// What it was decided by.
    pub by: DecidedBy,
}

/// Which evidence placed the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecidedBy {
    /// Under eight beats: the first is taken as the one.
    #[default]
    TooFew,
    /// The kick: of the positions the backbeat left, it was the surer of the
    /// two, or nothing pitched was heard to set against it.
    Kick,
    /// The harmony: of the positions the backbeat left, it was the surer.
    Harmony,
}

impl Downbeat {
    /// The kick at each of the four positions in the bar, counted from the
    /// grid's first beat: the average over every beat in that position.
    pub fn kick(&self) -> [f32; 4] {
        means(&self.kicks, 0..self.kicks.len())
    }

    /// The claps, the same way.
    pub fn claps(&self) -> [f32; 4] {
        means(&self.claps, 0..self.claps.len())
    }

    /// The pair of positions the claps are on, if they are on one: `Some(0)`
    /// for the first and third, `Some(1)` for the second and fourth. The 1
    /// is on the other pair.
    pub fn backbeat(&self) -> Option<usize> {
        let claps = self.claps();
        let (even, odd) = (claps[0] + claps[2], claps[1] + claps[3]);
        if even > odd * BACKBEAT {
            Some(0)
        } else if odd > even * BACKBEAT {
            Some(1)
        } else {
            None
        }
    }

    /// How much the harmony changes at each of the four positions over some
    /// of the beats: the bass's share of its change there and the chords'
    /// share of theirs, added, so that each counts alike however large its
    /// changes run. One that hardly moves at all over these beats counts for
    /// nothing, since its shares would be shares of rounding: see
    /// [`STILL_HARMONY`].
    pub fn harmony_over(&self, beats: std::ops::Range<usize>) -> [f32; 4] {
        let moving = |changes: &[f32]| {
            let changes = self.changes_over(changes, beats.clone());
            match changes.iter().sum::<f32>() / 4.0 >= STILL_HARMONY {
                true => share(changes),
                false => [0.0; 4],
            }
        };
        let (bass, chords) = (moving(&self.bass_changes), moving(&self.chord_changes));
        std::array::from_fn(|i| bass[i] + chords[i])
    }

    /// How far the bass note moves at each of the four positions, on average.
    pub fn bass(&self) -> [f32; 4] {
        self.changes_over(&self.bass_changes, 0..self.kicks.len())
    }

    /// How far the chord moves at each, on average.
    pub fn chords(&self) -> [f32; 4] {
        self.changes_over(&self.chord_changes, 0..self.kicks.len())
    }

    /// Some changes averaged by position over some beats, leaving out the bar
    /// at either end where a chord's change is not measured, so the bass and
    /// the chords are averaged over the same beats.
    fn changes_over(&self, changes: &[f32], beats: std::ops::Range<usize>) -> [f32; 4] {
        let end = self.kicks.len().saturating_sub(CHORD_SPAN);
        means(changes, beats.start.max(CHORD_SPAN)..beats.end.min(end))
    }

    /// What one reading says at each of the four positions over some of the
    /// beats, with the positions the backbeat rules out at zero.
    pub fn reading_over(&self, by: DecidedBy, beats: std::ops::Range<usize>) -> [f32; 4] {
        let mut reading = match by {
            DecidedBy::TooFew | DecidedBy::Kick => means(&self.kicks, beats),
            DecidedBy::Harmony => self.harmony_over(beats),
        };
        if let Some(claps) = self.backbeat() {
            reading[claps] = 0.0;
            reading[claps + 2] = 0.0;
        }
        reading
    }

    /// How far ahead one reading's strongest position is of the next of those
    /// the backbeat left, over the whole track. `None` where it has nothing to
    /// say.
    pub fn lead_of(&self, by: DecidedBy) -> Option<f32> {
        lead(self.reading_over(by, 0..self.kicks.len())).map(|(_, lead)| lead)
    }

    /// What the reading that decided says, over some of the beats. Its largest
    /// over all of them is [`Downbeat::phase`].
    pub fn scores_over(&self, beats: std::ops::Range<usize>) -> [f32; 4] {
        self.reading_over(self.by, beats)
    }

    /// The same over every beat.
    pub fn scores(&self) -> [f32; 4] {
        self.scores_over(0..self.kicks.len())
    }

    /// How far ahead the winner is, by the reading that decided.
    pub fn lead(&self) -> Option<f32> {
        self.lead_of(self.by)
    }
}

/// Each position's average of some per-beat evidence, over some of the beats,
/// counted in fours from the grid's first beat.
///
/// The average rather than the sum, because the positions do not get the same
/// number of beats: on a 32-beat click track whose changes are measured from
/// its third beat to its third from last, one position has eight and the rest
/// seven, and that alone put it an eighth ahead.
fn means(of: &[f32], beats: std::ops::Range<usize>) -> [f32; 4] {
    let (mut sums, mut counts) = ([0.0f32; 4], [0usize; 4]);
    for i in beats.start..beats.end.min(of.len()) {
        sums[i % 4] += of[i];
        counts[i % 4] += 1;
    }
    std::array::from_fn(|position| match counts[position] {
        0 => 0.0,
        count => sums[position] / count as f32,
    })
}

/// Each of four sums as a share of all four.
fn share(sums: [f32; 4]) -> [f32; 4] {
    let total: f32 = sums.iter().sum();
    match total > 0.0 {
        true => sums.map(|sum| sum / total),
        false => [0.0; 4],
    }
}

/// The largest of four scores, and how far it is ahead of the next largest
/// above zero.
fn lead(scores: [f32; 4]) -> Option<(usize, f32)> {
    let best = (0..4).fold(0, |a, b| if scores[b] > scores[a] { b } else { a });
    let next = (0..4).filter(|&i| i != best).map(|i| scores[i]).fold(0.0f32, f32::max);
    (next > 0.0).then(|| (best, scores[best] / next))
}

/// How many beats either side of each beat a bass note is heard over, and a
/// chord: the bass moves on the beat and the chords by the bar.
const BASS_SPAN: usize = 2;
const CHORD_SPAN: usize = 4;

/// How much more 1 to 5 kHz onset one pair of alternate positions has to have
/// than the other before it is taken for a backbeat.
///
/// Measured on 25 CC records, the pairs fell into two groups with nothing
/// between: thirteen 1.23 to 2.42 apart, and twelve 1.01 to 1.14, which lean
/// either way — Signal's leans 1.10 against its own downbeat. The line is in
/// the gap, a little nearer the noise; where in it is reasoned, not measured.
const BACKBEAT: f32 = 1.18;

/// How little the bass or the chords can change a beat, on average, and still
/// be counted: a thousandth, as a cosine distance. A click track's chords
/// change by two ten-thousandths a beat, which is rounding, and as shares of
/// nothing those decided where its bar was; the stillest of 25 CC records,
/// Heart Burn, changes by fifteen ten-thousandths in the bass and sixteen in
/// the chords.
const STILL_HARMONY: f32 = 0.001;

/// Track the beats of a measured track.
///
/// Returns an empty grid rather than guessing when there is nothing periodic to
/// find — silence, an ambient intro on its own, a file that failed to decode
/// into anything.
pub fn detect(features: &Features) -> Beats {
    detect_at(features, None)
}

/// Track the beats in the octave a record's genre is counted in. A tempo the
/// caller gives wins over it: see [`detect_at`].
pub fn detect_within(features: &Features, bpm: Option<f64>, octave: Option<Octave>) -> Beats {
    track(features, bpm, octave)
}

/// Track the beats at a tempo the caller already knows.
///
/// The point of taking the tempo rather than the whole grid is that everything
/// downstream — the phrases, the cues — is measured against where the beats
/// actually are. Correcting a tempo by hand and then keeping a grid derived
/// from the wrong one would be worse than not correcting it.
pub fn detect_at(features: &Features, bpm: Option<f64>) -> Beats {
    track(features, bpm, None)
}

fn track(features: &Features, bpm: Option<f64>, octave: Option<Octave>) -> Beats {
    if loudest_onsets(&features.flux) < MIN_ONSET_STRENGTH {
        return Beats {
            grid: BeatGrid::default(),
            bpm: 0.0,
            confidence: 0.0,
            downbeat: Downbeat::default(),
            moved: None,
        };
    }
    let envelope = normalised(&features.flux, features.frame_rate);
    let (period, confidence, moved) = match bpm {
        Some(bpm) if bpm > 0.0 => {
            let period = (60.0 * features.frame_rate / bpm).round() as usize;
            (Some(period.max(1)), f32::INFINITY, None)
        }
        _ => estimate_period(&envelope, features.frame_rate, octave),
    };
    let Some(period) = period else {
        return Beats {
            grid: BeatGrid::default(),
            bpm: 0.0,
            confidence,
            downbeat: Downbeat::default(),
            moved: None,
        };
    };

    let placing = onset_envelope(features);
    let frames = track_beats(&placing, period);
    let (times, steady) = match hold(&placing, period, &frames, features) {
        Some(line) => (line.times(), Some(60_000.0 / line.period)),
        None => {
            let mut times: Vec<u32> =
                frames.iter().map(|&f| (features.seconds_at(f) * 1000.0).round() as u32).collect();
            // The autocorrelation can only report a whole number of frames,
            // which at this frame rate is a step of more than a beat per
            // minute — too coarse to hold a mix together. The tracked beat
            // times are far more precise than the lag that produced them, so
            // the tempo is read back off them.
            let steady = straighten(&mut times);
            if steady.is_none() {
                // It really does move. Follow it, but follow the tempo rather
                // than the tracker: see [`smooth`].
                smooth(&mut times);
            }
            (times, steady)
        }
    };
    let bpm = steady.unwrap_or_else(|| measured_bpm(&times));
    let mut grid = grid_from(&times, steady);
    let downbeat = downbeat(features, &times);
    renumber(&mut grid, downbeat.phase);

    Beats { grid, bpm, confidence, downbeat, moved }
}

/// How strong this track's onsets are: the level the top twentieth of frames
/// reach. A percentile rather than the peak, so one click or edit does not
/// stand in for a rhythm.
fn loudest_onsets(flux: &[f32]) -> f32 {
    if flux.is_empty() {
        return 0.0;
    }
    let mut sorted = flux.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() * 19 / 20]
}

/// What the beats are placed by: the flux over the whole spectrum and over its
/// bottom alone, each [`normalised`], added together.
///
/// The whole spectrum alone put the beats on the hats. Its flux is summed over
/// a thousand bins spaced evenly in hertz, a hat's hiss fills most of them and
/// a kick fills a dozen: with a kick on every beat and a hat on every off-beat
/// sixteen times quieter, every beat landed on the hat. On 25 CC records the
/// tracker sat on the off-beat for over a quarter of the way on twelve, and all
/// or nearly all of the way on two. The low band hears the kick and not the
/// hat, and counted alongside the rest it puts the beats back on the kick.
///
/// The tempo is still read from the whole spectrum alone (see [`detect_at`]):
/// how often the music pushes is the same either way, and the confidence that
/// decides whether there is a beat at all was measured against it.
fn onset_envelope(features: &Features) -> Vec<f32> {
    let whole = normalised(&features.flux, features.frame_rate);
    let low = normalised(&features.low_flux, features.frame_rate);
    whole.iter().zip(&low).map(|(a, b)| a + b).collect()
}

/// Flux with its slow drift removed and its scale normalised, so a quiet
/// passage contributes onsets on the same footing as a loud one.
fn normalised(flux: &[f32], frame_rate: f64) -> Vec<f32> {
    if flux.is_empty() {
        return Vec::new();
    }
    // A one-second window: long enough to be a local average rather than a
    // copy of the signal, short enough to follow an arrangement.
    let window = (frame_rate as usize).max(1);
    let mut envelope = Vec::with_capacity(flux.len());
    let mut sum = 0.0f64;
    let mut queue = std::collections::VecDeque::new();
    for &value in flux {
        queue.push_back(value);
        sum += value as f64;
        if queue.len() > window {
            sum -= queue.pop_front().unwrap() as f64;
        }
        let average = (sum / queue.len() as f64) as f32;
        envelope.push((value - average).max(0.0));
    }

    let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
    let variance = envelope.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / envelope.len() as f32;
    let deviation = variance.sqrt();
    if deviation > f32::EPSILON {
        for value in &mut envelope {
            *value /= deviation;
        }
    }
    envelope
}

/// The levels a beat sits among in a bar of four, as multiples of its period:
/// the half-beat, the beat, the half-bar and the bar.
const LEVELS: [f64; 4] = [0.5, 1.0, 2.0, 4.0];

/// The beat period, in frames, from the autocorrelation of the envelope.
///
/// The most periodic lag was the whole of it, and on records with a rhythm in
/// threes over the four it is a lag in threes: VogelAudio's bass every three
/// half-beats put it at 86.67 BPM, two thirds of the 130 madmom and Essentia
/// both find, and Phobos's every three quarter-beats at 130.65 against their
/// 98. Every bar line after that is in the wrong place, and no later step
/// looks further than a frame from the lag chosen here.
///
/// What tells a beat from a lag in threes is what is around it. Under a beat
/// the half-beat repeats too, and above it the half-bar and the bar. Around
/// three half-beats are three quarter-beats and a bar and a half, which a bar
/// of four has little on. So each candidate is scored by its own correlation
/// and those of its [`LEVELS`] together.
///
/// That score chooses the grid and not the octave: a lag twice as long has the
/// same levels but one, and which of the two is the beat is the prior's
/// question, as it always was. So the octave of the grid the levels choose is
/// chosen as before, and where they choose the grid the most periodic lag is
/// on, nothing changes.
///
/// Unless the genre says which octave: then the lag is moved by octaves into
/// it, where the tracker's range reaches. See [`Octave`].
fn estimate_period(
    envelope: &[f32],
    frame_rate: f64,
    octave: Option<Octave>,
) -> (Option<usize>, f32, Option<Moved>) {
    let shortest = (60.0 * frame_rate / MAX_BPM).round() as usize;
    let longest = (60.0 * frame_rate / MIN_BPM).round() as usize;
    if envelope.len() < longest * 2 {
        return (None, 0.0, None);
    }

    // Every lag up to a bar of the slowest beat, and a frame past it, so that
    // each candidate's levels and neighbours are there to be read.
    let correlation: Vec<f64> = (0..=longest * 4 + 1)
        .map(|lag| match lag {
            0 => 0.0,
            lag if lag >= envelope.len() => 0.0,
            lag => {
                let mut sum = 0.0f64;
                for i in lag..envelope.len() {
                    sum += (envelope[i] * envelope[i - lag]) as f64;
                }
                sum / (envelope.len() - lag) as f64
            }
        })
        .collect();
    let centre = 60.0 / PRIOR_CENTRE_BPM * frame_rate;
    // Weight towards the tempo a listener would pick, which is what stops a
    // four-to-the-floor track being reported at half or double speed.
    let prior = |lag: usize| {
        let octaves = (lag as f64 / centre).log2();
        (-0.5 * (octaves / PRIOR_WIDTH_OCTAVES).powi(2)).exp()
    };
    let weighted = |lag: usize| correlation[lag] * prior(lag);

    let mut best = (0usize, f64::MIN);
    let mut total = 0.0f64;
    for lag in shortest..=longest {
        total += weighted(lag);
        if weighted(lag) > best.1 {
            best = (lag, weighted(lag));
        }
    }
    let average = total / (longest + 1 - shortest).max(1) as f64;
    let confidence = if average > 0.0 { (best.1 / average) as f32 } else { 0.0 };
    // Nothing periodic enough to be a beat. Better to say so than to invent a
    // grid a DJ then has to notice is wrong.
    if best.0 == 0 || confidence < MIN_CONFIDENCE {
        return (None, confidence, None);
    }

    // The lag a level falls on, give or take the frame its rounding moved it.
    let near = |at: f64| -> Option<usize> {
        let middle = at.round() as usize;
        (middle.saturating_sub(1)..=middle + 1)
            .filter(|&lag| lag >= 1 && lag < correlation.len())
            .max_by(|&a, &b| correlation[a].total_cmp(&correlation[b]))
    };
    let levels = |lag: usize| {
        LEVELS.iter().filter_map(|&m| near(lag as f64 * m)).map(|at| correlation[at]).sum::<f64>()
            * prior(lag)
    };
    let peaks = (shortest.max(1)..=longest).filter(|&lag| {
        correlation[lag] >= correlation[lag - 1] && correlation[lag] >= correlation[lag + 1]
    });
    let octaves = |of: usize| [0.5, 1.0, 2.0].map(|m| of as f64 * m);
    let period = match peaks.max_by(|&a, &b| levels(a).total_cmp(&levels(b))) {
        None => best.0,
        // Within a frame and a half of one of the most periodic lag's octaves:
        // its own rounding and a doubled one's.
        Some(grid) if octaves(best.0).iter().any(|&at| (grid as f64 - at).abs() <= 1.5) => best.0,
        Some(grid) => octaves(grid)
            .into_iter()
            .filter_map(near)
            .filter(|lag| (shortest..=longest).contains(lag))
            .max_by(|&a, &b| weighted(a).total_cmp(&weighted(b)))
            .unwrap_or(best.0),
    };

    let Some(octave) = octave else {
        return (Some(period), confidence, None);
    };
    let bpm = |lag: f64| 60.0 * frame_rate / lag;
    let mut at = period as f64;
    // Three octaves either way is further than the tracker's range goes.
    for _ in 0..3 {
        if bpm(at) < octave.from_bpm {
            at /= 2.0;
        } else if bpm(at) >= octave.from_bpm * 2.0 {
            at *= 2.0;
        }
    }
    let moved = near(at).filter(|lag| (shortest..=longest).contains(lag));
    match moved {
        Some(lag) if (at - period as f64).abs() >= 1.0 => {
            let moved = Moved { heard_bpm: bpm(period as f64), octave };
            (Some(lag), confidence, Some(moved))
        }
        _ => (Some(period), confidence, None),
    }
}

/// Choose the beat times: the sequence maximising onset strength minus the cost
/// of every gap that is not one period.
fn track_beats(envelope: &[f32], period: usize) -> Vec<usize> {
    if envelope.is_empty() || period == 0 {
        return Vec::new();
    }
    let earliest = period / 2;
    let latest = period * 2;

    let mut score = vec![f32::MIN; envelope.len()];
    let mut previous = vec![usize::MAX; envelope.len()];

    for frame in 0..envelope.len() {
        let mut best = (usize::MAX, 0.0f32);
        let from = frame.saturating_sub(latest);
        let to = frame.saturating_sub(earliest);
        for (candidate, &previous_score) in score.iter().enumerate().take(to).skip(from) {
            if previous_score == f32::MIN {
                continue;
            }
            let gap = (frame - candidate) as f32 / period as f32;
            let cost = -TIGHTNESS * gap.ln().powi(2);
            let total = previous_score + cost;
            if best.0 == usize::MAX || total > best.1 {
                best = (candidate, total);
            }
        }
        score[frame] = envelope[frame] + if best.0 == usize::MAX { 0.0 } else { best.1 };
        previous[frame] = best.0;
    }

    // Start the backtrace from the best score near the end, not the very last
    // frame, which is rarely a beat.
    let tail = envelope.len().saturating_sub(period);
    let Some(mut at) = (tail..envelope.len()).max_by(|a, b| score[*a].total_cmp(&score[*b])) else {
        return Vec::new();
    };

    let mut beats = vec![at];
    while previous[at] != usize::MAX {
        at = previous[at];
        beats.push(at);
    }
    beats.reverse();
    beats
}

/// How finely one beat is divided when the onsets are folded onto it: ten
/// milliseconds a step at 125 BPM, finer than a frame.
const FOLD_STEPS: usize = 48;

/// How far above its average step the folded beat has to peak before a record
/// is taken to sit on one steady beat.
///
/// Measured on 25 CC records: the 22 whose grid is held folded at 3.0 to 13.7,
/// and the two with no steady beat at all at 1.31 and 1.44 (a third, Heart
/// Burn, folds at 3.2 and is turned away later, by [`hold`]'s bow test). Two
/// is between the groups, where exactly is reasoned rather than measured; a
/// record that folds under it keeps the tracker's beats, as before, and is
/// straightened from those if they allow it.
const STEADY_FOLD: f32 = 2.0;

/// How far from the steady beat, as a fraction of one, an onset is still
/// heard as that beat. Wider than a tracker's scatter — the ±60 ms jitter the
/// tests put a click track through is an eighth of a beat at 128 — and narrower
/// than the sixteenth either side, where a hat or a bass note sits.
const BEAT_REACH: f64 = 0.15;

/// A straight grid: beat `k` is at `first + period * k` milliseconds, for `k`
/// in `beats`.
struct Line {
    period: f64,
    first: f64,
    beats: std::ops::RangeInclusive<i64>,
}

impl Line {
    fn times(&self) -> Vec<u32> {
        self.beats
            .clone()
            .map(|k| self.first + self.period * k as f64)
            .filter(|&ms| ms >= 0.0)
            .map(|ms| ms.round() as u32)
            .collect()
    }
}

/// The steady beat of a record made to one tempo, read off its onsets rather
/// than off the beats the tracker chose.
///
/// The tracker follows whatever is loudest from one beat to the next, so in a
/// breakdown it can follow a bassline half a beat over or a riff in threes,
/// and come out of it a beat or half a beat from where it went in. Measured on
/// the 22 of 25 CC records that have one steady beat, its beats were within a
/// tenth of a beat of it for a median 79 per cent of the way, and for under
/// half on three. Every bar line after such a stretch is in the wrong place,
/// and the grid is not straightened at all, because its beats no longer lie on
/// a line: three of the 25 were.
///
/// So the whole record is folded onto one beat: every frame's onset is added
/// into the step of the beat it falls in, over a fine range of tempos round the
/// one the autocorrelation found, and the tempo whose fold peaks sharpest is the
/// record's. That is a question about the whole record at once, which one
/// stretch heard wrongly cannot outvote; on twenty of the 22 it came out at a
/// whole number of BPM, as a sequencer would have it, to within a hundredth.
///
/// Then each beat of that steady grid is looked for in the onsets — the
/// strongest within [`BEAT_REACH`] of where it should be — and the beats found
/// are held to the same standard as a tracker's: they must not [`bow`] away
/// from one line. A record whose tempo really moves fails here and is left to
/// the tracker.
///
/// [`bow`]: crate::export::bow
fn hold(envelope: &[f32], period: usize, beats: &[usize], features: &Features) -> Option<Line> {
    let (&from, &to) = (beats.first()?, beats.last()?);
    if beats.len() < 16 || period == 0 {
        return None;
    }
    // Only the frames with an onset in them: a frame of nothing adds nothing
    // to the fold, and about half of the envelope is nothing.
    let sparse = |onsets: &[f32]| -> Vec<(f64, f32)> {
        (from..=to).filter(|&f| onsets[f] > 0.0).map(|f| (f as f64, onsets[f])).collect()
    };
    let fold = |onsets: &[(f64, f32)], period: f64| {
        let mut steps = [0.0f32; FOLD_STEPS];
        let per_frame = 1.0 / period;
        for &(frame, onset) in onsets {
            let at = frame * per_frame;
            steps[((at - at.floor()) * FOLD_STEPS as f64) as usize % FOLD_STEPS] += onset;
        }
        steps
    };
    let sharpness = |steps: &[f32; FOLD_STEPS]| {
        let mean = steps.iter().sum::<f32>() / FOLD_STEPS as f32;
        let peak = steps.iter().fold(0.0f32, |a, &b| a.max(b));
        if mean > 0.0 {
            peak / mean
        } else {
            0.0
        }
    };
    let onsets = sparse(envelope);
    // The autocorrelation's period is a whole number of frames, so the real
    // one is within half a frame of it. The search ends on steps fine enough
    // that the last beat of the record moves by under a thirty-second of a
    // beat between them; it starts on steps four times that, which still land
    // within half the width of the sharpest peak, and refines the best three.
    let count = (to - from) as f64 / period as f64;
    let fine = period as f64 / (32.0 * count.max(1.0));
    let coarse = fine * 4.0;
    let mut tried: Vec<(f32, f64)> = Vec::new();
    let mut candidate = period as f64 - 0.6;
    while candidate <= period as f64 + 0.6 {
        tried.push((sharpness(&fold(&onsets, candidate)), candidate));
        candidate += coarse;
    }
    tried.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut best = (0.0f32, period as f64, [0.0f32; FOLD_STEPS]);
    for &(_, around) in tried.iter().take(3) {
        for nudge in -4..=4 {
            let candidate = around + nudge as f64 * fine;
            let steps = fold(&onsets, candidate);
            let sharp = sharpness(&steps);
            if sharp > best.0 {
                best = (sharp, candidate, steps);
            }
        }
    }
    let (sharpness, period, steps) = best;
    if sharpness < STEADY_FOLD {
        return None;
    }
    // Where in the beat the beat is. The fold's own peak is not the answer: a
    // hat's onset is sharp and lands in one step where a kick's rises over the
    // three or four frames the window takes to fill, so the fold can peak as
    // high on an off-beat hat as on the kick, and on Orquidea it peaked higher.
    // The tracker's beats are, beat to beat, mostly where the kick is — what
    // they get wrong is the stretch between, where there is no kick to follow
    // — so they are folded onto the same beat, each counting for as hard as
    // the low band hits at it, and wherever the most of that lands is the
    // beat. The weight is for the records where the tracker followed the hats
    // for most of the way (Heart Burn): its beats on the kick still outweigh
    // the ones that are not. The fold's peak nearest is exactly where.
    let mut votes = [0.0f32; FOLD_STEPS];
    for &frame in beats {
        let at = frame as f64 / period;
        let low =
            &features.low_flux[frame.saturating_sub(2)..(frame + 3).min(features.low_flux.len())];
        votes[((at - at.floor()) * FOLD_STEPS as f64) as usize % FOLD_STEPS] +=
            low.iter().fold(0.0f32, |a, &b| a.max(b));
    }
    let around =
        |at: usize| -> f32 { (0..3).map(|d| votes[(at + FOLD_STEPS + d - 1) % FOLD_STEPS]).sum() };
    let kick = (0..FOLD_STEPS).fold(0, |a, b| if around(b) > around(a) { b } else { a });
    let top = (0..7).map(|d| (kick + FOLD_STEPS + d - 3) % FOLD_STEPS).fold(kick, |a, b| {
        if steps[b] > steps[a] {
            b
        } else {
            a
        }
    });
    let (left, right) = (steps[(top + FOLD_STEPS - 1) % FOLD_STEPS], steps[(top + 1) % FOLD_STEPS]);
    let curve = left - 2.0 * steps[top] + right;
    let nudge = if curve < 0.0 { 0.5 * (left - right) / curve } else { 0.0 };
    let phase = (top as f64 + 0.5 + nudge as f64) / FOLD_STEPS as f64 * period;

    // Each beat of the steady grid, as the onsets have it.
    let reach = (BEAT_REACH * period).round() as usize;
    let first_k = ((from as f64 - phase) / period).ceil() as i64;
    let last_k = ((to as f64 - phase) / period).floor() as i64;
    let mut heard: Vec<(i64, f64)> = Vec::new();
    for k in first_k..=last_k {
        let centre = (phase + period * k as f64).round() as usize;
        let window = centre.saturating_sub(reach)..(centre + reach + 1).min(envelope.len());
        let Some(at) = window.max_by(|a, b| envelope[*a].total_cmp(&envelope[*b])) else {
            continue;
        };
        // Nothing much at this beat — a breakdown, a gap — says nothing
        // about where it is.
        if envelope[at] >= 1.0 {
            heard.push((k, features.seconds_at(at) * 1000.0));
        }
    }
    if heard.len() < 16 {
        return None;
    }
    let line = |using: &[(i64, f64)]| {
        let n = using.len() as f64;
        let mean_k = using.iter().map(|b| b.0 as f64).sum::<f64>() / n;
        let mean_t = using.iter().map(|b| b.1).sum::<f64>() / n;
        let (mut covariance, mut variance) = (0.0, 0.0);
        for &(k, t) in using {
            covariance += (k as f64 - mean_k) * (t - mean_t);
            variance += (k as f64 - mean_k).powi(2);
        }
        let period = covariance / variance;
        (period, mean_t - period * mean_k)
    };
    // Some of what is heard is something else: in a breakdown the loudest
    // onset near the beat can be a clap a little late or a bass note a little
    // early, all through it. On 25 CC records the beats heard sat a median 3 ms
    // off the line, and a few stretches of something else bowed four steady
    // records past the limit — Groovielaugh, at a whole 125.000 BPM, by 9.5 ms
    // against 7.5. A record whose tempo moves keeps nearly every beat inside
    // the cut and bows as far as before: 11.5 to 14 ms on the tests' ramps,
    // against a jittered click track's 5. So the beats over three times the
    // typical distance off the first line are left out of the test and of the
    // line that is kept.
    let (period, first) = line(&heard);
    let residual: Vec<f64> = heard.iter().map(|&(k, t)| t - (first + period * k as f64)).collect();
    let mut distance: Vec<f64> = residual.iter().map(|r| r.abs()).collect();
    distance.sort_by(f64::total_cmp);
    let cut = (3.0 * distance[distance.len() / 2]).max(1.0);
    let kept: Vec<(i64, f64)> =
        heard.iter().zip(&residual).filter(|(_, r)| r.abs() <= cut).map(|(b, _)| *b).collect();
    if kept.len() < 16 {
        return None;
    }
    let (period, first) = line(&kept);
    let residual: Vec<f64> = kept.iter().map(|&(k, t)| t - (first + period * k as f64)).collect();
    if crate::export::bow_of(&residual) > period * crate::export::STRAIGHT_ENOUGH {
        return None;
    }
    (period > 0.0).then_some(Line { period, first, beats: first_k..=last_k })
}

/// Which of the four positions in the bar the first beat holds, and what it
/// was decided on.
///
/// Kicks were the whole of it. But a four-on-the-floor kick hits every beat
/// alike, and where it leads by a few per cent the lead is the mix's dynamics
/// rather than the bar: on CC records gridded on the kick, leads of 1.10 and
/// 1.11 were on the wrong beat, and right ones led by as little as 1.01.
///
/// What differs between the beats of a bar on such a record is the rest of
/// it. The clap or snare is on the 2 and the 4, so where one pair of positions
/// has clearly more 1 to 5 kHz onset than the other, the 1 is on the other
/// pair. And the harmony changes on the 1: the bass note, and the chord. Of
/// the positions left, the kick and the harmony each say which they favour and
/// by how much, and the surer of the two decides — so a kick accented on the
/// 1, or played only on it, still places the bar as it always did.
///
/// Measured against the eleven CC records whose downbeat the arrangement's
/// changes and the backbeat agree on — how that reference was made is in the
/// pull request that added this — the kick alone was right on six, and this on
/// all eleven. Of five more where the reference is less sure, it agreed on two
/// and disagreed with three, one of those by under one per cent, which the
/// log calls a guess.
fn downbeat(features: &Features, times: &[u32]) -> Downbeat {
    let frames: Vec<usize> =
        times.iter().map(|&ms| features.frame_at(ms as f64 / 1000.0)).collect();
    // A beat marker can sit a couple of frames either side of the transient
    // it belongs to, and the transient itself takes a window to arrive.
    let sharpest = |of: &[f32]| -> Vec<f32> {
        frames
            .iter()
            .map(|&frame| {
                let window = frame.saturating_sub(2)..(frame + 3).min(of.len());
                of[window].iter().fold(0.0f32, |a, &b| a.max(b))
            })
            .collect()
    };
    let kicks = sharpest(&features.low_flux);
    let claps = sharpest(&features.mid_flux);
    // Each beat's chord and bass, added up over the frames up to the next.
    let held = |profile: fn(&Features, usize) -> &[f32]| -> Vec<[f32; PITCHES]> {
        (0..frames.len())
            .map(|i| {
                let from = frames[i];
                // The last beat runs as long as the one before it.
                let next = match frames.get(i + 1) {
                    Some(&next) => next,
                    None => from + (from - frames[i.saturating_sub(1)]),
                };
                let to = next.max(from + 1).min(features.frames());
                let mut sum = [0.0f32; PITCHES];
                for frame in from..to {
                    for (total, value) in sum.iter_mut().zip(profile(features, frame)) {
                        *total += value;
                    }
                }
                sum
            })
            .collect()
    };
    let chords = held(Features::chroma_frame);
    let bass = held(Features::bass_frame);
    let bass_changes = changes(&bass, BASS_SPAN);
    let chord_changes = changes(&chords, CHORD_SPAN);

    let mut found = Downbeat {
        times_ms: times.to_vec(),
        kicks,
        claps,
        bass_changes,
        chord_changes,
        phase: 0,
        by: DecidedBy::TooFew,
    };
    if times.len() < 8 {
        return found;
    }
    // Each reading's lead, among the positions the backbeat left; the surer
    // decides. A kick accented on the 1 is surer than any harmony, and a kick
    // on every beat is surer of nothing.
    let kick = found.lead_of(DecidedBy::Kick).unwrap_or(1.0);
    found.by = match found.lead_of(DecidedBy::Harmony) {
        Some(harmony) if harmony > kick => DecidedBy::Harmony,
        _ => DecidedBy::Kick,
    };
    let scores = found.scores();
    found.phase = (0..4).fold(0, |a, b| if scores[b] > scores[a] { b } else { a });
    found
}

/// How far a profile moves across each beat: the `span` beats before it
/// against the `span` from it, as the cosine distance between their sums. Zero
/// where there are not `span` beats either side.
fn changes(profiles: &[[f32; PITCHES]], span: usize) -> Vec<f32> {
    let total = |beats: &[[f32; PITCHES]]| -> [f32; PITCHES] {
        std::array::from_fn(|pitch| beats.iter().map(|beat| beat[pitch]).sum())
    };
    (0..profiles.len())
        .map(|i| {
            if i < span || i + span > profiles.len() {
                return 0.0;
            }
            let (before, after) = (total(&profiles[i - span..i]), total(&profiles[i..i + span]));
            let dot: f32 = before.iter().zip(&after).map(|(a, b)| a * b).sum();
            let norm = |v: &[f32; PITCHES]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
            match norm(&before) * norm(&after) {
                product if product > 0.0 => 1.0 - dot / product,
                _ => 0.0,
            }
        })
        .collect()
}

/// A straight line through beat times: the period between them, and where the
/// first one falls.
///
/// Least squares over whichever beats are named, so a caller can fit through
/// the ones that agree and leave the ones that do not out of it.
fn line_through(times: &[u32], using: impl Iterator<Item = usize> + Clone) -> Option<(f64, f64)> {
    let count = using.clone().count();
    if count < 4 {
        return None;
    }
    let n = count as f64;
    let mean_index = using.clone().map(|i| i as f64).sum::<f64>() / n;
    let mean_time = using.clone().map(|i| times[i] as f64).sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut variance = 0.0;
    for i in using {
        let di = i as f64 - mean_index;
        covariance += di * (times[i] as f64 - mean_time);
        variance += di * di;
    }
    if variance <= 0.0 {
        return None;
    }
    let period = covariance / variance;
    (period > 0.0).then_some((period, mean_time - period * mean_index))
}

/// Fit a constant tempo to the tracked beats, and if it fits, use it.
///
/// A production made to a click really is at one tempo, and a grid that wobbles
/// is worse than one that does not: the player's tempo readout flickers, every
/// cue sits slightly off, and two decks told to sync are each chasing a number
/// that will not hold still. So: fit a line through the beat times, and where
/// the track is plainly at one tempo, replace the tracked times with the line.
///
/// # Why the test is about bow and not about scatter
///
/// It used to be "is no beat far from the line", which almost never fired: one
/// mistracked beat in 640 threw away a perfect fit for the other 639. Then it
/// was "are most beats near the line", measured at the ninetieth percentile
/// against one analysis frame — and that fired on a metronome and gave up on
/// anything else. Measured on a 128 BPM click track with the clicks nudged by
/// ±24 ms, which is less than a drummer: the grid came back with 48 different
/// tempos in it, a BPM that was a whole beat per minute wrong, and eight-beat
/// stretches whose lengths differed by 46 ms depending on where in the track
/// they were taken. Which is a loop walking away from the other deck.
///
/// The trouble is that scatter and drift look the same beat by beat. Only
/// their shape differs: scatter is as often early as late and drift is not. So
/// the question asked is [`bend`] — how far the beats depart from the line
/// once sixteen bars of them are averaged — and a track is written at one
/// tempo unless it really moves.
fn straighten(times: &mut Vec<u32>) -> Option<f64> {
    if times.len() < 8 {
        return None;
    }
    let (period, first) = line_through(times, 0..times.len())?;
    if crate::export::bow(times, period, first) > period * crate::export::STRAIGHT_ENOUGH {
        return None;
    }

    let off =
        |period: f64, first: f64, i: usize| (times[i] as f64 - (first + period * i as f64)).abs();
    // Fitted again without the beats that disagree, so one mistracked beat
    // moves nothing. The line for "disagrees" is drawn from the track's own
    // scatter rather than from a fixed number of milliseconds: how far a
    // tracker's beats sit from the truth depends on what it was tracking, and
    // a threshold that suits a click track throws away most of a live take.
    let mut spread: Vec<f64> = (0..times.len()).map(|i| off(period, first, i)).collect();
    spread.sort_by(f64::total_cmp);
    let tolerance = (spread[spread.len() * 9 / 10] * 2.0).max(1.0);
    let keep = || (0..times.len()).filter(|i| off(period, first, *i) <= tolerance);
    let (period, first) = line_through(times, keep()).unwrap_or((period, first));

    // A line fitted through the whole track can start a little before the
    // track does, and a beat clamped to zero is a beat off the grid at the one
    // place every player parks. So the beats before the start are dropped
    // rather than squashed onto it.
    let dropped = match first < 0.0 {
        true => (-first / period).ceil() as usize,
        false => 0,
    };
    if dropped >= times.len() {
        return None;
    }
    times.drain(..dropped);
    for (i, time) in times.iter_mut().enumerate() {
        *time = (first + period * (i + dropped) as f64).round().max(0.0) as u32;
    }
    Some(60_000.0 / period)
}

/// Put each beat where a line through the beats around it says it should be.
///
/// For the records that will not take one tempo. The alternative is the times
/// the tracker measured, and those carry every missed kick and every soft
/// attack straight into the grid — so two eight-beat loops taken from
/// different parts of such a track come out different lengths, which is the
/// complaint whether or not the tempo is constant. A line through sixteen bars
/// follows a tempo that really moves and averages away the rest.
///
/// Monotonic afterwards, because a grid is a sequence of times and a player
/// reading one that goes backwards has no good options.
fn smooth(times: &mut [u32]) {
    if times.len() < SMOOTH_AROUND * 2 {
        return;
    }
    let fitted: Vec<u32> = (0..times.len())
        .map(|i| {
            let from = i.saturating_sub(SMOOTH_AROUND);
            let to = (i + SMOOTH_AROUND + 1).min(times.len());
            line_through(times, from..to)
                .map(|(period, first)| (first + period * i as f64).round().max(0.0) as u32)
                .unwrap_or(times[i])
        })
        .collect();
    let mut last = 0;
    for (time, fitted) in times.iter_mut().zip(fitted) {
        last = fitted.max(last + u32::from(last > 0));
        *time = last;
    }
}

/// How many beats either side the smoothing fits through: eight bars.
const SMOOTH_AROUND: usize = 32;

/// The tempo implied by the middle of the distribution of gaps, for a track
/// whose tempo moves.
fn measured_bpm(times: &[u32]) -> f64 {
    let mut gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    if gaps.is_empty() {
        return 0.0;
    }
    gaps.sort_by(f64::total_cmp);
    let median = gaps[gaps.len() / 2];
    if median > 0.0 {
        60_000.0 / median
    } else {
        0.0
    }
}

/// Build a grid from beat times, quoting each beat's tempo from a line fitted
/// through the beats around it.
///
/// Not from the gap that follows it, and not from the median of the gaps
/// nearby: both read the tempo off numbers that arrive quantised to an analysis
/// frame, so on a 128 BPM track the only answers available were 126.05 and
/// 129.03 — the two tempos a 41-frame and a 40-frame gap imply. A player shown
/// that has a tempo readout that flickers between two wrong numbers, and two
/// players told to sync are each chasing it.
///
/// A line through a dozen beats averages the quantisation away and lands within
/// a hundredth of a BPM, which is the resolution the format stores anyway.
fn grid_from(times: &[u32], steady: Option<f64>) -> BeatGrid {
    let beats = times
        .iter()
        .enumerate()
        .map(|(i, &time)| {
            let bpm = steady
                .or_else(|| {
                    let from = i.saturating_sub(TEMPO_WINDOW);
                    let to = (i + TEMPO_WINDOW + 1).min(times.len());
                    line_through(times, from..to).map(|(period, _)| 60_000.0 / period)
                })
                .unwrap_or(0.0);
            Beat {
                number: (i % 4) as u16 + 1,
                tempo_x100: (bpm * 100.0).round().clamp(0.0, u16::MAX as f64) as u16,
                time_ms: time,
            }
        })
        .collect();
    BeatGrid { beats }
}

/// Slide the bar lines so that beat `phase` is the downbeat.
fn renumber(grid: &mut BeatGrid, phase: usize) {
    for (i, beat) in grid.beats.iter_mut().enumerate() {
        beat.number = ((i + 4 - phase % 4) % 4) as u16 + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::super::features;
    use super::*;
    use crate::audio::Audio;

    const RATE: u32 = 44_100;
    /// The silence `clicks` puts before the first beat.
    const LEAD_IN_MS: f64 = 500.0;

    /// A click track: a kick on every beat, and on every fourth beat a lower,
    /// louder one.
    fn clicks(bpm: f64, bars: usize, accent_downbeats: bool) -> Audio {
        let period = 60.0 / bpm;
        let beats = bars * 4;
        // Half a second of silence first: a real track does not begin with a
        // kick on its first sample, and an onset needs something to be an
        // onset against.
        let lead_in = RATE as usize / 2;
        let frames = lead_in + (RATE as f64 * period * beats as f64) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        for beat in 0..beats {
            let start = lead_in + (RATE as f64 * period * beat as f64) as usize;
            let downbeat = accent_downbeats && beat % 4 == 0;
            let (hz, gain) = if downbeat { (55.0, 1.0) } else { (150.0, 0.5) };
            for i in 0..(RATE as usize / 8) {
                let at = start + i;
                if at >= frames {
                    break;
                }
                let t = i as f32 / RATE as f32;
                let decay = (-30.0 * t).exp();
                plane[at] += gain * decay * (2.0 * std::f32::consts::PI * hz * t).sin();
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    /// A click track built to order: its tempo at each point, and how far off
    /// the beat each click is allowed to land.
    ///
    /// The jitter is what a tracker is up against on a real record — a soft
    /// kick attack, a swung hat, a drummer — and it never changes the tempo.
    /// Which is the whole point: a grid should come back straight from this.
    fn made(bars: usize, jitter_ms: f64, seed: u64, tempo: impl Fn(f64) -> f64) -> Audio {
        let beats = bars * 4;
        let lead_in = RATE as usize / 2;
        let mut state = seed | 1;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0
        };

        let mut at_secs = 0.0f64;
        let mut starts = Vec::with_capacity(beats);
        for beat in 0..beats {
            starts.push(at_secs + next() * jitter_ms / 1000.0);
            at_secs += 60.0 / tempo(beat as f64 / beats as f64);
        }

        let frames = lead_in + (RATE as f64 * at_secs) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        for (beat, at) in starts.iter().enumerate() {
            let start = lead_in + (RATE as f64 * at.max(0.0)) as usize;
            let (hz, gain) = if beat % 4 == 0 { (55.0, 1.0) } else { (150.0, 0.5) };
            for i in 0..(RATE as usize / 8) {
                let frame = start + i;
                if frame >= frames {
                    break;
                }
                let t = i as f32 / RATE as f32;
                let decay = (-30.0 * t).exp();
                plane[frame] += gain * decay * (2.0 * std::f32::consts::PI * hz * t).sin();
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    fn jittered(bpm: f64, bars: usize, jitter_ms: f64, seed: u64) -> Audio {
        made(bars, jitter_ms, seed, |_| bpm)
    }

    fn ramped(from_bpm: f64, to_bpm: f64, bars: usize, jitter_ms: f64) -> Audio {
        made(bars, jitter_ms, 11, |at| from_bpm + (to_bpm - from_bpm) * at)
    }

    /// Every stretch of `beats` beats in a grid, by length in milliseconds.
    fn loop_lengths(grid: &BeatGrid, beats: usize) -> Vec<u32> {
        let times: Vec<u32> = grid.beats.iter().map(|b| b.time_ms).collect();
        match times.len() > beats {
            true => times.windows(beats + 1).map(|w| w[beats] - w[0]).collect(),
            false => Vec::new(),
        }
    }

    /// The worst disagreement between any two such stretches, which is what a
    /// DJ feels as a loop walking away from the other deck.
    fn loop_spread(grid: &BeatGrid, beats: usize) -> u32 {
        let lengths = loop_lengths(grid, beats);
        match lengths.is_empty() {
            true => 0,
            false => lengths.iter().max().unwrap() - lengths.iter().min().unwrap(),
        }
    }

    /// The beat times the tracker produces, before anything straightens them.
    fn tracked(f: &Features) -> Vec<u32> {
        let (period, ..) = estimate_period(&normalised(&f.flux, f.frame_rate), f.frame_rate, None);
        let frames = track_beats(&onset_envelope(f), period.unwrap());
        frames.iter().map(|&n| (f.seconds_at(n) * 1000.0).round() as u32).collect()
    }

    #[test]
    fn eight_beats_anywhere_are_eight_beats_anywhere_else() {
        // What a DJ actually needs from a grid, and the thing two players
        // reading the same track have to agree about. A loop taken from the
        // intro and a loop taken from the last drop have to be the same
        // length, or one deck walks away from the other while they play.
        //
        // The fault: a tracker's beats scatter about the truth on any real
        // record, and a grid accepted as steady only when they barely
        // scattered at all gave up on most of them. At ±24 ms of scatter —
        // less than a drummer — the grid came back with 48 different tempos
        // in it and eight-beat stretches whose lengths differed by 46 ms.
        for jitter in [0.0, 24.0, 40.0, 60.0] {
            let found = detect(&features::extract(&jittered(128.0, 24, jitter, 7)));
            assert!(
                (found.bpm - 128.0).abs() < 0.05,
                "±{jitter} ms of scatter moved the tempo to {:.3}",
                found.bpm
            );
            // Give or take the millisecond the times are stored in.
            let spread = loop_spread(&found.grid, 8);
            assert!(spread <= 1, "±{jitter} ms of scatter left eight-beat loops {spread} ms apart");
            let tempos: std::collections::BTreeSet<u16> =
                found.grid.beats.iter().map(|b| b.tempo_x100).collect();
            assert_eq!(tempos.len(), 1, "±{jitter} ms of scatter gave {tempos:?}");
        }
    }

    #[test]
    fn scatter_is_told_from_drift_by_its_shape_and_not_by_its_size() {
        // Why the test is [`bend`] and not how far the worst beat sits. These
        // two are the same size beat by beat and nothing alike in shape: the
        // scatter is as often early as late, and the drift is not.
        let loose = features::extract(&jittered(128.0, 24, 60.0, 7));
        let moving = features::extract(&ramped(128.0, 127.0, 24, 0.0));
        let (loose, moving) = (tracked(&loose), tracked(&moving));

        let worst = |times: &[u32]| {
            let (period, first) = line_through(times, 0..times.len()).unwrap();
            let off = (0..times.len())
                .map(|i| (times[i] as f64 - (first + period * i as f64)).abs())
                .fold(0.0, f64::max);
            (off, crate::export::bow(times, period, first), period)
        };
        let (scatter_off, scatter_bend, period) = worst(&loose);
        let (drift_off, drift_bend, _) = worst(&moving);

        assert!(
            scatter_off > drift_off * 0.7,
            "the two are meant to scatter alike: {scatter_off:.1} against {drift_off:.1}"
        );
        assert!(
            scatter_bend < period * crate::export::STRAIGHT_ENOUGH,
            "scatter read as drift: {scatter_bend:.1} ms"
        );
        assert!(
            drift_bend > period * crate::export::STRAIGHT_ENOUGH,
            "a whole beat per minute of drift read as scatter: {drift_bend:.1} ms"
        );
    }

    #[test]
    fn a_track_whose_tempo_really_moves_is_still_written_moving() {
        // The other half of the bargain, and the one that straightening too
        // eagerly would break. A record that speeds up has to come back
        // speeding up, or every cue in the second half is in the wrong place.
        for (from, to) in [(128.0, 127.0), (124.0, 127.0), (128.0, 120.0)] {
            let found = detect(&features::extract(&ramped(from, to, 24, 0.0)));
            let tempos: std::collections::BTreeSet<u16> =
                found.grid.beats.iter().map(|b| b.tempo_x100).collect();
            assert!(tempos.len() > 20, "{from}→{to} was flattened to {} tempos", tempos.len());

            // And in the right direction, by about the right amount.
            let quoted: Vec<f64> =
                found.grid.beats.iter().map(|b| b.tempo_x100 as f64 / 100.0).collect();
            let moved = quoted.last().unwrap() - quoted.first().unwrap();
            let real = to - from;
            assert!(
                moved.signum() == real.signum() && moved.abs() > real.abs() * 0.5,
                "{from}→{to} came out moving by {moved:.2}"
            );
        }
    }

    #[test]
    fn a_grid_that_cannot_be_straightened_still_does_not_carry_the_scatter() {
        // A track that really moves cannot have one tempo, but it can still
        // have a tempo that only moves the way the record does. Left as the
        // tracker measured them, the beats of a drifting record carry every
        // missed kick into the grid as well — so a loop from one bar and a
        // loop from the next are different lengths for no musical reason.
        let found = detect(&features::extract(&ramped(128.0, 124.0, 24, 40.0)));
        let lengths = loop_lengths(&found.grid, 8);
        assert!(lengths.len() > 40, "{} eight-beat stretches", lengths.len());

        // A track slowing down evenly: each stretch is longer than the one
        // before it, give or take the millisecond the times are stored in.
        // Scatter would show up here as stretches that jump about.
        let backwards = lengths.windows(2).filter(|pair| pair[1] + 1 < pair[0]).count();
        assert!(
            backwards * 20 < lengths.len(),
            "{backwards} of {} stretches got shorter on a track that only slows",
            lengths.len()
        );
    }

    fn detect_bpm(bpm: f64) -> Beats {
        detect(&features::extract(&clicks(bpm, 8, true)))
    }

    #[test]
    fn a_click_track_gives_up_its_tempo() {
        for wanted in [90.0, 120.0, 128.0, 140.0] {
            let found = detect_bpm(wanted);
            assert!(
                (found.bpm - wanted).abs() < 1.5,
                "wanted {wanted} BPM, found {:.2}",
                found.bpm
            );
        }
    }

    /// Pulses laid over each other: each `(every, hz, gain)` a decaying tone
    /// every `every` beats at `bpm`, for `bars` bars.
    fn layered(bpm: f64, bars: usize, layers: &[(f64, f32, f32)]) -> Audio {
        let period = 60.0 / bpm;
        let lead_in = RATE as usize / 2;
        let length = period * (bars * 4) as f64;
        let frames = lead_in + (RATE as f64 * length) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        for &(every, hz, gain) in layers {
            let mut at = 0.0;
            while at < length {
                let start = lead_in + (RATE as f64 * at) as usize;
                for i in 0..(RATE as usize / 8).min(frames - start) {
                    let t = i as f32 / RATE as f32;
                    plane[start + i] +=
                        gain * (-30.0 * t).exp() * (2.0 * std::f32::consts::PI * hz * t).sin();
                }
                at += period * every;
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn a_stab_every_three_half_beats_does_not_take_the_tempo_with_it() {
        // A kick on every beat, a quiet hat on every half-beat, and over them a
        // bass stab twice as loud as the kick every beat and a half, as
        // VogelAudio's is. The most periodic lag is the stab's, 86.67 BPM.
        let audio = layered(130.0, 32, &[(1.0, 60.0, 0.5), (0.5, 6000.0, 0.03), (1.5, 110.0, 1.0)]);
        let found = detect(&features::extract(&audio));
        assert!((found.bpm - 130.0).abs() < 1.0, "found {:.2} BPM for a 130 BPM track", found.bpm);
    }

    #[test]
    fn a_genre_moves_the_tempo_into_its_octave_and_leaves_one_already_in_it() {
        let hip_hop = Octave { from_bpm: 60.0, because: "hip hop" };
        let drum_and_bass = Octave { from_bpm: 120.0, because: "drum and bass" };
        let f = features::extract(&clicks(174.0, 16, true));
        let moved = detect_within(&f, None, Some(hip_hop));
        assert!((moved.bpm - 87.0).abs() < 1.0, "found {:.2}", moved.bpm);
        let heard = moved.moved.expect("moved, and said so").heard_bpm;
        assert!((heard - 174.0).abs() < 4.0, "heard at {heard:.2}");
        let kept = detect_within(&f, None, Some(drum_and_bass));
        assert!((kept.bpm - 174.0).abs() < 1.0, "found {:.2}", kept.bpm);
        assert_eq!(kept.moved, None);
    }

    #[test]
    fn a_tempo_given_by_hand_wins_over_the_genres_octave() {
        let hip_hop = Octave { from_bpm: 60.0, because: "hip hop" };
        let f = features::extract(&clicks(174.0, 16, true));
        let found = detect_within(&f, Some(174.0), Some(hip_hop));
        assert!((found.bpm - 174.0).abs() < 1.0, "found {:.2}", found.bpm);
        assert_eq!(found.moved, None);
    }

    #[test]
    fn a_fast_track_is_not_reported_at_half_speed() {
        let found = detect_bpm(174.0);
        assert!((found.bpm - 174.0).abs() < 2.0, "found {:.2} for a 174 BPM track", found.bpm);
    }

    #[test]
    fn the_beats_land_on_the_clicks() {
        let bpm = 128.0;
        let found = detect_bpm(bpm);
        let period_ms = 60_000.0 / bpm;

        assert!(found.grid.beats.len() > 24, "only {} beats", found.grid.beats.len());
        for beat in &found.grid.beats {
            let from_first_click = beat.time_ms as f64 - LEAD_IN_MS;
            let nearest = (from_first_click / period_ms).round() * period_ms;
            let error = (from_first_click - nearest).abs();
            assert!(error < 30.0, "a beat at {} ms is {error:.0} ms off", beat.time_ms);
        }
    }

    #[test]
    fn every_beat_is_numbered_within_the_bar() {
        let found = detect_bpm(128.0);
        assert!(found.grid.beats.iter().all(|b| (1..=4).contains(&b.number)));
        // The numbers advance one at a time and wrap at four.
        for pair in found.grid.beats.windows(2) {
            let expected = pair[0].number % 4 + 1;
            assert_eq!(pair[1].number, expected);
        }
    }

    /// Where each of a grid's downbeats falls in the bars of a record whose
    /// first beat is at [`LEAD_IN_MS`] and whose first bar line is `early`
    /// beats after it: 0 for the 1, 1 for the 2, and so on.
    fn bar_positions(grid: &BeatGrid, bpm: f64, early: usize) -> Vec<usize> {
        let period = 60_000.0 / bpm;
        grid.beats
            .iter()
            .filter(|beat| beat.number == 1)
            .map(|beat| ((beat.time_ms as f64 - LEAD_IN_MS) / period).round() as usize)
            .map(|beat| (beat + 4 - early % 4) % 4)
            .collect()
    }

    #[test]
    fn a_kick_only_on_the_one_puts_the_bar_there() {
        // The test this replaces asked the same of a click track whose 1 was
        // a click twice as loud as the rest, and passed — but only because
        // the 1 had nine beats to the others' eight. Averaged, the accent read
        // 24.7 in the low band against 25.2: a log-compressed onset does not
        // hear a kick twice as loud as another, so what the kick can say is
        // where it is, not how hard. Here it is only on the 1.
        let audio =
            club(125.0, 16, Over { kick_only_on_the_one: true, early: 2, ..Over::default() });
        let found = detect(&features::extract(&audio));
        let positions = bar_positions(&found.grid, 125.0, 2);
        assert!(positions.len() >= 12, "{} downbeats", positions.len());
        assert!(positions.iter().all(|&p| p == 0), "downbeats on {positions:?}");
        assert_eq!(found.downbeat.by, DecidedBy::Kick);
    }

    #[test]
    fn on_a_kick_on_every_beat_the_bar_starts_where_the_chord_changes() {
        // The kick hits all four beats alike and cannot say; the chord changes
        // on every 1, all of it above the band the kick is read in, and says.
        let audio = club(125.0, 16, Over { chords: true, early: 2, ..Over::default() });
        let found = detect(&features::extract(&audio));
        let positions = bar_positions(&found.grid, 125.0, 2);
        assert!(positions.len() >= 12, "{} downbeats", positions.len());
        assert!(positions.iter().all(|&p| p == 0), "downbeats on {positions:?}");
        assert_eq!(found.downbeat.by, DecidedBy::Harmony);
    }

    #[test]
    fn a_bass_line_moving_on_every_one_is_heard_as_the_harmony_and_not_as_the_kick() {
        // A new bass note is an onset in the low band too, so the kick reads
        // a bass line moving on the 1 as a kick a little harder there: 1.11
        // times the next, on this record. The harmony reads it as what it is,
        // 3.4 times the next, and is the surer.
        let audio = club(125.0, 16, Over { bass: true, early: 2, ..Over::default() });
        let found = detect(&features::extract(&audio));
        let positions = bar_positions(&found.grid, 125.0, 2);
        assert!(positions.len() >= 12, "{} downbeats", positions.len());
        assert!(positions.iter().all(|&p| p == 0), "downbeats on {positions:?}");
        assert_eq!(found.downbeat.by, DecidedBy::Harmony);
    }

    #[test]
    fn a_clap_on_the_two_and_four_keeps_the_one_off_them() {
        // With nothing pitched, a backbeat cannot say which of the 1 and the
        // 3 is the 1. It can say it is neither the 2 nor the 4.
        let audio = club(125.0, 16, Over { claps: true, early: 2, ..Over::default() });
        let found = detect(&features::extract(&audio));
        let positions = bar_positions(&found.grid, 125.0, 2);
        assert!(positions.len() >= 12, "{} downbeats", positions.len());
        assert!(positions.iter().all(|&p| p % 2 == 0), "downbeats on {positions:?}");
        assert_eq!(found.downbeat.backbeat().map(|claps| claps % 2), Some((2 + 1) % 2));
    }

    #[test]
    fn a_backbeat_and_a_change_of_chord_put_the_bar_on_the_one() {
        let over = Over { claps: true, chords: true, early: 3, ..Over::default() };
        let found = detect(&features::extract(&club(128.0, 16, over)));
        let positions = bar_positions(&found.grid, 128.0, 3);
        assert!(positions.len() >= 12, "{} downbeats", positions.len());
        assert!(positions.iter().all(|&p| p == 0), "downbeats on {positions:?}");
    }

    #[test]
    fn a_metronomic_track_comes_out_at_one_exact_tempo() {
        // The bug this exists for. A grid is accepted as steady on how most of
        // its beats sit against the fitted line, not on whether every single
        // one does — and before that, one mistracked beat in six hundred threw
        // the fit away for the whole track.
        //
        // What went on the drive instead was the tracked times, quantised to
        // an analysis frame: on a metronomic 128 BPM click track the gaps ran
        // from 441 to 477 ms and the tempo alternated between 126.05 and
        // 129.03 BPM — neither of them the tempo, and a number that will not
        // hold still is a number two players cannot sync to.
        for wanted in [128.0, 174.0] {
            let found = detect(&features::extract(&clicks(wanted, 16, true)));
            assert!((found.bpm - wanted).abs() < 0.05, "{wanted} BPM came out as {:.3}", found.bpm);

            let tempos: std::collections::BTreeSet<u16> =
                found.grid.beats.iter().map(|beat| beat.tempo_x100).collect();
            assert_eq!(tempos.len(), 1, "{wanted} BPM was written as {tempos:?}");

            // And the beats are evenly spaced, to the millisecond they are
            // stored in: two gap lengths at most, one apart.
            let times: Vec<u32> = found.grid.beats.iter().map(|beat| beat.time_ms).collect();
            let gaps: std::collections::BTreeSet<u32> =
                times.windows(2).map(|pair| pair[1] - pair[0]).collect();
            let (tight, loose) = (*gaps.iter().next().unwrap(), *gaps.iter().next_back().unwrap());
            assert!(loose - tight <= 1, "{wanted} BPM gave gaps {gaps:?}");
        }
    }

    #[test]
    fn a_grid_fitted_from_before_the_track_does_not_squash_a_beat_onto_zero() {
        // The line through a whole track can start a little before the track
        // does. Clamping that beat to zero put one beat off the grid at the
        // one place every player parks — the first gap came out 461 ms where
        // every other was 468.
        let found = detect(&features::extract(&clicks(128.0, 16, true)));
        let times: Vec<u32> = found.grid.beats.iter().map(|beat| beat.time_ms).collect();
        assert!(times[0] > 0, "the grid still begins on a clamped beat");
        let first = times[1] - times[0];
        let second = times[2] - times[1];
        assert!(first.abs_diff(second) <= 1, "the first gap is {first} and the next {second}");
    }

    #[test]
    fn a_track_with_no_pulse_gets_no_grid_rather_than_a_wrong_one() {
        let hum: Vec<f32> = (0..RATE as usize * 8)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 110.0 * i as f32 / RATE as f32).sin())
            .collect();
        let audio = Audio::new(RATE, vec![hum.clone(), hum]).unwrap();
        let found = detect(&features::extract(&audio));
        assert!(
            found.grid.is_empty(),
            "a steady drone should not produce {} beats",
            found.grid.beats.len()
        );
    }

    #[test]
    fn silence_gets_no_grid() {
        let silence = Audio::new(RATE, vec![vec![0.0; RATE as usize * 8]]).unwrap();
        let found = detect(&features::extract(&silence));
        assert!(found.grid.is_empty());
        assert_eq!(found.bpm, 0.0);
    }

    #[test]
    fn a_clear_pulse_is_more_confident_than_a_vague_one() {
        let clear = detect_bpm(128.0).confidence;
        let hum: Vec<f32> = (0..RATE as usize * 8)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 110.0 * i as f32 / RATE as f32).sin())
            .collect();
        let vague = detect(&features::extract(&Audio::new(RATE, vec![hum.clone(), hum]).unwrap()))
            .confidence;
        assert!(clear > vague, "clear {clear} should beat vague {vague}");
    }

    #[test]
    fn each_beat_carries_a_tempo_close_to_the_tracks() {
        let found = detect_bpm(128.0);
        for beat in &found.grid.beats {
            let bpm = beat.tempo_x100 as f64 / 100.0;
            assert!((bpm - 128.0).abs() < 4.0, "a beat claims {bpm:.2} BPM");
        }
    }

    /// Where a [`house`] record stops its kick and hat and plays a riff.
    struct Breakdown {
        bars: std::ops::Range<usize>,
        /// A soft 220 Hz note every this many beats…
        every: f64,
        /// …the first this far into the breakdown's first beat.
        from: f64,
    }

    /// A club record reduced to its drums: a kick on every beat, with no click
    /// to it, and an open hat on every off-beat at `hat` against the kick's
    /// 0.8. Through `breakdown`, a riff instead.
    fn house(bpm: f64, bars: usize, hat: f32, breakdown: Option<Breakdown>) -> Audio {
        let period = 60.0 / bpm;
        let lead_in = RATE as usize / 2;
        let frames = lead_in + (RATE as f64 * period * (bars * 4) as f64) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        let mut noise = 12_345u32;
        let mut add = |at_secs: f64, sound: &mut dyn FnMut(f32) -> f32| {
            let start = lead_in + (RATE as f64 * at_secs) as usize;
            for i in 0..(RATE as usize / 5) {
                if let Some(sample) = plane.get_mut(start + i) {
                    *sample += sound(i as f32 / RATE as f32);
                }
            }
        };
        let tau = 2.0 * std::f32::consts::PI;
        let quiet = |beat: usize| breakdown.as_ref().is_some_and(|b| b.bars.contains(&(beat / 4)));
        for beat in 0..bars * 4 {
            if quiet(beat) {
                continue;
            }
            let at = beat as f64 * period;
            // The pitch falls from 150 to 50 Hz, as a kick's does.
            add(at, &mut |t| {
                0.8 * (-12.0 * t).exp() * (tau * (50.0 + 100.0 * (-40.0 * t).exp()) * t).sin()
            });
            // White noise, differenced: a hiss with its weight at the top.
            let mut last = 0.0f32;
            add(at + period / 2.0, &mut |t| {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (noise >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                let hiss = white - last;
                last = white;
                hat * (-25.0 * t).exp() * hiss
            });
        }
        if let Some(b) = &breakdown {
            let mut beat = (b.bars.start * 4) as f64 + b.from;
            while beat < (b.bars.end * 4) as f64 {
                add(beat * period, &mut |t| {
                    0.5 * (t / 0.005).min(1.0) * (-8.0 * t).exp() * (tau * 220.0 * t).sin()
                });
                beat += b.every;
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    /// What a [`club`] record has besides a hat on every off-beat.
    #[derive(Clone, Copy, Default)]
    struct Over {
        /// A kick on the 1 and a higher drum on the rest, rather than a kick
        /// on every beat.
        kick_only_on_the_one: bool,
        /// A clap on the 2 and the 4.
        claps: bool,
        /// A chord, changing on every 1, all of it above the low band.
        chords: bool,
        /// A bass note, changing on every 1.
        bass: bool,
        /// How many beats the record runs before its first bar line.
        early: usize,
    }

    /// A four-on-the-floor record: a kick on every beat, all alike, and a hat
    /// on every off-beat; then whatever `over` adds. Its first beat is at
    /// [`LEAD_IN_MS`] and its first bar line `over.early` beats after.
    fn club(bpm: f64, bars: usize, over: Over) -> Audio {
        let period = 60.0 / bpm;
        let beats = bars * 4 + over.early;
        let lead_in = RATE as usize / 2;
        let frames = lead_in + (RATE as f64 * period * beats as f64) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        let tau = 2.0 * std::f32::consts::PI;
        let mut noise = 12_345u32;
        let mut white = move || {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (noise >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let at = |beat: f64| lead_in + (RATE as f64 * period * beat) as usize;
        let add = |plane: &mut Vec<f32>,
                   start: usize,
                   length: usize,
                   sound: &mut dyn FnMut(f32) -> f32| {
            for i in 0..length {
                if let Some(sample) = plane.get_mut(start + i) {
                    *sample += sound(i as f32 / RATE as f32);
                }
            }
        };
        for beat in 0..beats {
            // Where in the bar: 0 for the 1.
            let position = (beat + 4 - over.early % 4) % 4;
            let start = at(beat as f64);
            match over.kick_only_on_the_one && position != 0 {
                // A tom an octave and a half up, outside the low band.
                true => add(&mut plane, start, RATE as usize / 8, &mut |t| {
                    0.5 * (-30.0 * t).exp() * (tau * 300.0 * t).sin()
                }),
                false => add(&mut plane, start, RATE as usize / 5, &mut |t| {
                    0.8 * (-12.0 * t).exp() * (tau * (50.0 + 100.0 * (-40.0 * t).exp()) * t).sin()
                }),
            }
            let mut last = 0.0f32;
            add(&mut plane, at(beat as f64 + 0.5), RATE as usize / 8, &mut |t| {
                let w = white();
                let hiss = w - last;
                last = w;
                0.3 * (-25.0 * t).exp() * hiss
            });
            if over.claps && position % 2 == 1 {
                add(&mut plane, start, RATE as usize / 6, &mut |t| {
                    0.3 * (-20.0 * t).exp() * white()
                });
            }
        }
        if over.chords || over.bass {
            // A bass note and a triad a bar each, round a cycle of four whose
            // bass notes are far enough apart for the window to hear them
            // move, and whose chords are all above the 200 Hz the kick is read
            // under. The beats before the first bar line hold the last chord.
            let bass = [55.0f32, 98.0, 65.4, 110.0];
            let chords = [
                [261.6f32, 329.6, 392.0],
                [246.9, 293.7, 392.0],
                [220.0, 261.6, 329.6],
                [349.2, 440.0, 523.3],
            ];
            let (bass_level, chord_level) =
                (if over.bass { 0.3 } else { 0.0 }, if over.chords { 0.08 } else { 0.0 });
            let length = at(4.0) - at(0.0);
            // Slow fades, so that a new note is a change and not an onset: the
            // kick reads onsets, and this is about what it cannot read.
            let fade = RATE as f32 * 0.15;
            for bar in -1..bars as i64 {
                let first = over.early as f64 + 4.0 * bar as f64;
                if first + 4.0 <= 0.0 {
                    continue;
                }
                let start = at(first.max(0.0));
                let which = bar.rem_euclid(4) as usize;
                let shape = move |i: usize| {
                    (i as f32 / fade).min(1.0) * ((length - i) as f32 / fade).min(1.0)
                };
                let (bass, chord) = (bass[which], chords[which]);
                let mut i = 0usize;
                add(&mut plane, start, length, &mut |t| {
                    let level = shape(i);
                    i += 1;
                    let triad: f32 = chord.iter().map(|hz| (tau * hz * t).sin()).sum();
                    level * (bass_level * (tau * bass * t).sin() + chord_level * triad)
                });
            }
        }
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    /// How many of a grid's beats are within a tenth of a beat of a kick that
    /// [`house`] put on every beat, out of how many beats.
    fn on_the_kick(grid: &BeatGrid, bpm: f64) -> (usize, usize) {
        let period = 60_000.0 / bpm;
        let on = grid
            .beats
            .iter()
            .map(|beat| ((beat.time_ms as f64 - LEAD_IN_MS) / period).rem_euclid(1.0))
            .filter(|off| *off < 0.1 || *off > 0.9)
            .count();
        (on, grid.beats.len())
    }

    #[test]
    fn a_hat_on_every_off_beat_does_not_pull_the_grid_off_the_kick() {
        // The tracker followed every onset across the whole spectrum, and a
        // hat's hiss is a thousand bins wide where a kick is a dozen. A hat at
        // a sixteenth of the kick's level put every beat on the hat. A hat as
        // loud as the kick is no reason to either.
        for hat in [0.05, 0.3, 0.8] {
            let found = detect(&features::extract(&house(125.0, 16, hat, None)));
            let (on, of) = on_the_kick(&found.grid, 125.0);
            assert!(of > 60, "{of} beats");
            assert_eq!(on, of, "a hat at {hat} put {} of {of} beats off the kick", of - on);
        }
    }

    #[test]
    fn a_breakdown_the_tracker_loses_its_way_in_moves_no_beat_after_it() {
        // Eight bars without a kick, and a riff on every off-beat: the tracker
        // follows the riff and comes out of the breakdown off the kick, and
        // every bar line after it was in the wrong place. The record is at one
        // tempo throughout, so the grid should be one straight line on the kick.
        let breakdown = Breakdown { bars: 16..24, every: 1.0, from: 0.5 };
        let found = detect(&features::extract(&house(125.0, 40, 0.3, Some(breakdown))));
        let (on, of) = on_the_kick(&found.grid, 125.0);
        assert_eq!(on, of, "{} of {of} beats are off the kick", of - on);
        assert!((found.bpm - 125.0).abs() < 0.05, "the tempo came out {:.3}", found.bpm);
        let gaps: std::collections::BTreeSet<u32> =
            found.grid.beats.windows(2).map(|pair| pair[1].time_ms - pair[0].time_ms).collect();
        assert!(gaps.len() <= 2, "the grid is not straight: gaps of {gaps:?} ms");
    }
}
