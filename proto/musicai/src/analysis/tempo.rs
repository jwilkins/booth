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
//! The downbeat is a separate question, answered separately: of the four
//! possible places the bar could start, the one where the kicks are — and on a
//! record whose kick lands on every beat alike, where the music changes.

use crate::export::{Beat, BeatGrid};

use super::features::{Features, BANDS};

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
}

/// How far ahead the kick's winning sum has to be to decide the downbeat on
/// its own: a tenth. Below it the other three cues are asked as well — see
/// [`Downbeat::together`].
///
/// Measured on eighteen CC records: on the ten whose kick led by 7 per cent or
/// less — by 1 or 2 per cent on eight of them, records with a kick on every
/// beat, which hits all four positions alike — most eight-bar stretches
/// favoured some other beat. On all but two of the eight that led by 12 per
/// cent or more, most stretches agreed with the whole. Nothing led by between
/// 7 and 12.
pub const CLEAR_LEAD: f32 = 1.1;

/// The bands below about 300 Hz, where the bass is: see
/// [`super::features::BANDS`], twelve log-spaced from 40 Hz.
const BASS_BANDS: std::ops::Range<usize> = 0..4;

/// How many beats either side of a beat are compared to say whether the
/// arrangement turns there: two bars. Long enough that a fill is not a turn;
/// short enough that a part coming in for eight bars is.
const TURN_BEATS: usize = 8;

/// The evidence for which beat is the one: how hard the low end hit on each
/// beat the tracker found, how the music changed there, and which of the four
/// positions that favoured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Downbeat {
    /// Each tracked beat, as the tracker heard it, before straightening moved
    /// it onto the line: milliseconds from the start of the track.
    pub times_ms: Vec<u32>,
    /// The low band's sharpest onset around each of those beats — the kick,
    /// where there is one.
    pub kicks: Vec<f32>,
    /// How different each beat sounds from the one before it, across the
    /// spectrum: a chord changes on the one, and a part comes in on it. Zero
    /// for the first beat, which has nothing before it.
    pub changes: Vec<f32>,
    /// The same in the bass alone, where a bassline's note changes with the
    /// bar.
    pub bass_changes: Vec<f32>,
    /// Where the arrangement turns: how different the two bars after a beat
    /// sound from the two before it, kept at the beats where that peaks and
    /// zero elsewhere. A drop, a break or a part coming in starts on the one.
    pub turns: Vec<f32>,
    /// Which of the four positions won: the tracked beats `phase`,
    /// `phase + 4`, `phase + 8` and so on are the downbeats.
    pub phase: usize,
    /// How many tracked beats straightening took off the front, which moves
    /// the phase in the grid the beats end up in.
    pub dropped: usize,
}

impl Downbeat {
    /// The kicks summed over each of the four positions in the bar, from the
    /// first tracked beat.
    pub fn strengths(&self) -> [f32; 4] {
        self.strengths_over(0..self.kicks.len())
    }

    /// The same over some of the tracked beats, counted in the same fours, so
    /// a stretch of the track can be seen to agree with the whole of it or not.
    pub fn strengths_over(&self, beats: std::ops::Range<usize>) -> [f32; 4] {
        by_position(&self.kicks[..beats.end.min(self.kicks.len())], beats.start)
    }

    /// Whether the kick decided on its own: whether its winning sum led the
    /// next by [`CLEAR_LEAD`] or more.
    pub fn kick_decides(&self) -> bool {
        lead(self.strengths()) >= CLEAR_LEAD
    }

    /// Each cue summed over the four positions: the kick, the beat-to-beat
    /// change, the same in the bass, and the arrangement's turns.
    pub fn cues(&self) -> [[f32; 4]; 4] {
        self.cues_over(0..self.kicks.len())
    }

    /// The same over some of the tracked beats.
    pub fn cues_over(&self, beats: std::ops::Range<usize>) -> [[f32; 4]; 4] {
        let over =
            |values: &[f32]| by_position(&values[..beats.end.min(values.len())], beats.start);
        [over(&self.kicks), over(&self.changes), over(&self.bass_changes), over(&self.turns)]
    }

    /// The four cues together: each one's sums as shares of its own total,
    /// added, so that none counts for more because of the units it is in.
    ///
    /// Asked only where the kick is a near tie, and there the kick's shares are
    /// all about a quarter and it is the other three that speak. Measured on
    /// the six of eighteen CC records whose kick led by 1.4 times or more, the
    /// other three together chose the kick's beat on five and tied on the
    /// sixth: as often as that by chance is under one in two hundred.
    pub fn together(&self) -> [f32; 4] {
        self.together_over(0..self.kicks.len())
    }

    /// The same over some of the tracked beats.
    pub fn together_over(&self, beats: std::ops::Range<usize>) -> [f32; 4] {
        shares(&self.cues_over(beats))
    }
}

/// Sums of several cues as shares of each cue's own total, added.
fn shares(cues: &[[f32; 4]]) -> [f32; 4] {
    let mut total = [0.0f32; 4];
    for cue in cues {
        let sum: f32 = cue.iter().sum();
        for (position, value) in cue.iter().enumerate() {
            total[position] += if sum > 0.0 { value / sum } else { 0.25 };
        }
    }
    total
}

/// Values summed by their position in the bar, counting the first as
/// position `from % 4`.
fn by_position(values: &[f32], from: usize) -> [f32; 4] {
    let mut sums = [0.0f32; 4];
    for (i, value) in values.iter().enumerate().skip(from) {
        sums[i % 4] += value;
    }
    sums
}

/// How far the largest of four sums is ahead of the next: the one divided by
/// the other. One when there is nothing to choose between.
pub fn lead(sums: [f32; 4]) -> f32 {
    let mut sorted = sums;
    sorted.sort_by(|a, b| b.total_cmp(a));
    match (sorted[0] > 0.0, sorted[1] > 0.0) {
        (false, _) => 1.0,
        (true, false) => f32::INFINITY,
        (true, true) => sorted[0] / sorted[1],
    }
}

/// The position of the largest of four sums; the first of two alike.
fn largest(sums: [f32; 4]) -> usize {
    (0..4).fold(0, |best, position| if sums[position] > sums[best] { position } else { best })
}

/// Track the beats of a measured track.
///
/// Returns an empty grid rather than guessing when there is nothing periodic to
/// find — silence, an ambient intro on its own, a file that failed to decode
/// into anything.
pub fn detect(features: &Features) -> Beats {
    detect_at(features, None)
}

/// Track the beats at a tempo the caller already knows.
///
/// The point of taking the tempo rather than the whole grid is that everything
/// downstream — the phrases, the cues — is measured against where the beats
/// actually are. Correcting a tempo by hand and then keeping a grid derived
/// from the wrong one would be worse than not correcting it.
pub fn detect_at(features: &Features, bpm: Option<f64>) -> Beats {
    if loudest_onsets(&features.flux) < MIN_ONSET_STRENGTH {
        return Beats {
            grid: BeatGrid::default(),
            bpm: 0.0,
            confidence: 0.0,
            downbeat: Downbeat::default(),
        };
    }
    let envelope = onset_envelope(features);
    let (period, confidence) = match bpm {
        Some(bpm) if bpm > 0.0 => {
            let period = (60.0 * features.frame_rate / bpm).round() as usize;
            (Some(period.max(1)), f32::INFINITY)
        }
        _ => estimate_period(&envelope, features.frame_rate),
    };
    let Some(period) = period else {
        return Beats {
            grid: BeatGrid::default(),
            bpm: 0.0,
            confidence,
            downbeat: Downbeat::default(),
        };
    };

    let frames = track_beats(&envelope, period);
    let mut times: Vec<u32> =
        frames.iter().map(|&f| (features.seconds_at(f) * 1000.0).round() as u32).collect();

    // The autocorrelation can only report a whole number of frames, which at
    // this frame rate is a step of more than a beat per minute — too coarse to
    // hold a mix together. The tracked beat times are far more precise than the
    // lag that produced them, so the tempo is read back off them.
    let straightened = straighten(&mut times);
    if straightened.is_none() {
        // It really does move. Follow it, but follow the tempo rather than
        // the tracker: see [`smooth`].
        smooth(&mut times);
    }
    let steady = straightened.as_ref().map(|s| s.bpm);
    let bpm = steady.unwrap_or_else(|| measured_bpm(&times));
    let mut grid = grid_from(&times, steady);
    // The phase is an index into the beats that were tracked, so it moves with
    // them when straightening takes some off the front.
    let dropped = straightened.as_ref().map(|s| s.dropped).unwrap_or(0);
    let downbeat = Downbeat { dropped, ..downbeat(features, &frames) };
    renumber(&mut grid, downbeat.phase + 4 - dropped % 4);

    Beats { grid, bpm, confidence, downbeat }
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

/// The onset envelope the rest of this works from: flux, with its slow drift
/// removed and its scale normalised, so a quiet passage contributes onsets on
/// the same footing as a loud one.
fn onset_envelope(features: &Features) -> Vec<f32> {
    let flux = &features.flux;
    if flux.is_empty() {
        return Vec::new();
    }
    // A one-second window: long enough to be a local average rather than a
    // copy of the signal, short enough to follow an arrangement.
    let window = (features.frame_rate as usize).max(1);
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

/// The beat period, in frames, from the autocorrelation of the envelope.
fn estimate_period(envelope: &[f32], frame_rate: f64) -> (Option<usize>, f32) {
    let shortest = (60.0 * frame_rate / MAX_BPM).round() as usize;
    let longest = (60.0 * frame_rate / MIN_BPM).round() as usize;
    if envelope.len() < longest * 2 {
        return (None, 0.0);
    }

    let centre = 60.0 / PRIOR_CENTRE_BPM * frame_rate;
    let mut best = (0usize, f64::MIN);
    let mut total = 0.0f64;
    let mut counted = 0usize;

    for lag in shortest..=longest {
        let mut correlation = 0.0f64;
        for i in lag..envelope.len() {
            correlation += (envelope[i] * envelope[i - lag]) as f64;
        }
        correlation /= (envelope.len() - lag) as f64;
        // Weight towards the tempo a listener would pick, which is what stops
        // a four-to-the-floor track being reported at half or double speed.
        let octaves = (lag as f64 / centre).log2();
        let weighted = correlation * (-0.5 * (octaves / PRIOR_WIDTH_OCTAVES).powi(2)).exp();
        total += weighted;
        counted += 1;
        if weighted > best.1 {
            best = (lag, weighted);
        }
    }

    let average = total / counted.max(1) as f64;
    let confidence = if average > 0.0 { (best.1 / average) as f32 } else { 0.0 };
    // Nothing periodic enough to be a beat. Better to say so than to invent a
    // grid a DJ then has to notice is wrong.
    if best.0 == 0 || confidence < MIN_CONFIDENCE {
        return (None, confidence);
    }
    (Some(best.0), confidence)
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

/// Which of the four positions in the bar the first beat holds, and what it
/// was decided on.
///
/// Kicks land on the downbeat far more often than not, so where one position
/// carries clearly the most low-frequency onset, it wins. A record with a kick
/// on every beat gives all four alike, and then the music around it decides:
/// a chord, a bass note, a part coming in or the drop itself lands on the one.
fn downbeat(features: &Features, beats: &[usize]) -> Downbeat {
    let kicks: Vec<f32> = beats
        .iter()
        .map(|&frame| {
            // A beat marker can sit a couple of frames either side of the
            // transient it belongs to, and the transient itself takes a
            // window to arrive.
            let from = frame.saturating_sub(2);
            let to = (frame + 3).min(features.low_flux.len());
            features.low_flux[from..to].iter().fold(0.0f32, |a, &b| a.max(b))
        })
        .collect();
    let times_ms =
        beats.iter().map(|&frame| (features.seconds_at(frame) * 1000.0).round() as u32).collect();
    let sounds = beat_sounds(features, beats);
    let mut found = Downbeat {
        times_ms,
        kicks,
        changes: changes(&sounds, 0..BANDS),
        bass_changes: changes(&sounds, BASS_BANDS),
        turns: turns(&sounds),
        phase: 0,
        dropped: 0,
    };
    if beats.len() < 8 {
        return found;
    }
    found.phase = match found.kick_decides() {
        true => largest(found.strengths()),
        false => largest(found.together()),
    };
    found
}

/// What each beat sounds like: the energy in each band, averaged from the beat
/// to the next one.
fn beat_sounds(features: &Features, beats: &[usize]) -> Vec<[f32; BANDS]> {
    let frames = features.frames();
    (0..beats.len())
        .map(|i| {
            let from = beats[i].min(frames.saturating_sub(1));
            // The last beat has no next one, so it is given the one before's
            // length.
            let length = match (beats.get(i + 1), i.checked_sub(1).map(|j| beats[j])) {
                (Some(&next), _) => next.saturating_sub(beats[i]),
                (None, Some(previous)) => beats[i].saturating_sub(previous),
                (None, None) => 1,
            };
            let to = (from + length.max(1)).min(frames).max(from + 1);
            let mut sound = [0.0f32; BANDS];
            for frame in from..to {
                for (band, energy) in features.band_frame(frame).iter().enumerate() {
                    sound[band] += energy;
                }
            }
            sound.map(|energy| energy / (to - from) as f32)
        })
        .collect()
}

/// How far each beat's sound is from the one before's, in these bands.
fn changes(sounds: &[[f32; BANDS]], bands: std::ops::Range<usize>) -> Vec<f32> {
    (0..sounds.len())
        .map(|i| match i.checked_sub(1) {
            Some(previous) => distance(&sounds[previous], &sounds[i], bands.clone()),
            None => 0.0,
        })
        .collect()
}

/// Where the arrangement turns: [`TURN_BEATS`] after each beat against as many
/// before it, kept only at the beats where that difference peaks. A change
/// that the window slides across reads high for several beats running, and it
/// is the beat it peaks on that the change happened on.
fn turns(sounds: &[[f32; BANDS]]) -> Vec<f32> {
    let n = sounds.len();
    let mean = |range: std::ops::Range<usize>| {
        let mut sound = [0.0f32; BANDS];
        for i in range.clone() {
            for band in 0..BANDS {
                sound[band] += sounds[i][band];
            }
        }
        sound.map(|energy| energy / range.len() as f32)
    };
    let difference: Vec<f32> = (0..n)
        .map(|i| match i >= TURN_BEATS && i + TURN_BEATS <= n {
            true => distance(&mean(i - TURN_BEATS..i), &mean(i..i + TURN_BEATS), 0..BANDS),
            false => 0.0,
        })
        .collect();
    (0..n)
        .map(|i| {
            let peak = i > 0
                && i + 1 < n
                && difference[i] > difference[i - 1]
                && difference[i] >= difference[i + 1];
            if peak {
                difference[i]
            } else {
                0.0
            }
        })
        .collect()
}

fn distance(a: &[f32; BANDS], b: &[f32; BANDS], bands: std::ops::Range<usize>) -> f32 {
    bands.map(|band| (a[band] - b[band]).powi(2)).sum::<f32>().sqrt()
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
fn straighten(times: &mut Vec<u32>) -> Option<Straightened> {
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
    // rather than squashed onto it, and the caller is told how many so the
    // downbeat keeps its phase.
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
    Some(Straightened { bpm: 60_000.0 / period, dropped })
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

/// What straightening a grid came to: the one tempo it is at, and how many
/// beats came off the front because the line began before the track did.
struct Straightened {
    bpm: f64,
    dropped: usize,
}

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
    /// louder one, which is what the downbeat detector is meant to find.
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
        let envelope = onset_envelope(f);
        let (period, _) = estimate_period(&envelope, f.frame_rate);
        let frames = track_beats(&envelope, period.unwrap());
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
            assert_eq!(
                loop_spread(&found.grid, 8),
                1,
                "±{jitter} ms of scatter left eight-beat loops disagreeing",
            );
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

    #[test]
    fn the_downbeat_is_where_the_accent_is() {
        let found = detect(&features::extract(&clicks(120.0, 8, true)));
        let period_ms = 500.0;
        let downbeats: Vec<f64> = found
            .grid
            .beats
            .iter()
            .filter(|b| b.number == 1)
            .map(|b| (b.time_ms as f64 - LEAD_IN_MS) / period_ms)
            .collect();

        assert!(downbeats.len() >= 4, "found {} downbeats", downbeats.len());
        // Accented clicks are on beats 0, 4, 8 … so every downbeat should land
        // on a multiple of four beats.
        for position in &downbeats {
            let off = (position / 4.0 - (position / 4.0).round()).abs() * 4.0;
            assert!(off < 0.2, "a downbeat landed {off:.2} beats from the accent");
        }
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

    /// Four to the floor: the same kick on every beat, so the kick cannot say
    /// which is the one, with a two-note pad over it that changes every bar —
    /// low for one bar, an octave up for the next. The first beat played is
    /// the third of its bar, so a downbeat put on the first beat is wrong.
    ///
    /// `accent` puts a louder, lower kick on the beat that many beats after
    /// each bar line instead of an even one, to see whether the kick still
    /// decides where it can.
    ///
    /// Returns the audio, and the times of the beats that many beats after
    /// each bar line, through to a bar past the end for a beat the tracker
    /// finds in the tail.
    fn floor_with_a_pad(bars: usize, accent: Option<usize>) -> (Audio, impl Fn(usize) -> Vec<f64>) {
        let bpm = 124.0;
        let period = 60.0 / bpm;
        let lead_in = RATE as usize / 2;
        // Two beats of the first bar are not played.
        let skipped = 2;
        let beats = bars * 4 - skipped;
        let frames = lead_in + (RATE as f64 * period * beats as f64) as usize + RATE as usize;
        let mut plane = vec![0.0f32; frames];
        let at = |beat: usize| lead_in + (RATE as f64 * period * beat as f64) as usize;
        let two_pi = 2.0 * std::f32::consts::PI;
        for beat in 0..beats {
            let accented = accent.is_some_and(|on| (beat + skipped) % 4 == on);
            let (hz, gain) = if accented { (50.0, 1.0) } else { (60.0, 0.6) };
            for i in 0..(RATE as usize / 8) {
                let t = i as f32 / RATE as f32;
                if let Some(sample) = plane.get_mut(at(beat) + i) {
                    *sample += gain * (-30.0 * t).exp() * (two_pi * hz * t).sin();
                }
            }
        }
        for bar in 0..bars {
            let from = (bar * 4).saturating_sub(skipped);
            let to = ((bar + 1) * 4 - skipped).min(beats);
            let notes: [f32; 2] = if bar % 2 == 0 { [330.0, 415.0] } else { [660.0, 830.0] };
            let (start, end) = (at(from), at(to));
            // Faded in and out over ten milliseconds, so the change of chord
            // is not itself a click in the low band.
            let fade = RATE as usize / 100;
            for (i, sample) in plane.iter_mut().enumerate().take(end).skip(start) {
                let t = i as f32 / RATE as f32;
                let edge = ((i - start).min(end - i) as f32 / fade as f32).min(1.0);
                *sample +=
                    0.12 * edge * notes.iter().map(|hz| (two_pi * hz * t).sin()).sum::<f32>();
            }
        }
        let on = move |position: usize| {
            (0..beats + 4)
                .filter(|beat| (beat + skipped) % 4 == position)
                .map(|beat| LEAD_IN_MS + period * 1000.0 * beat as f64)
                .collect()
        };
        (Audio::new(RATE, vec![plane.clone(), plane]).unwrap(), on)
    }

    /// The downbeats a grid has, in milliseconds.
    fn downbeats_of(found: &Beats) -> Vec<f64> {
        found.grid.beats.iter().filter(|b| b.number == 1).map(|b| b.time_ms as f64).collect()
    }

    #[test]
    fn on_a_kick_on_every_beat_the_change_of_chord_says_where_the_bar_starts() {
        let (audio, on) = floor_with_a_pad(16, None);
        let bar_lines = on(0);
        let found = detect(&features::extract(&audio));
        assert!(
            !found.downbeat.kick_decides(),
            "the kick was not a tie: {:?}",
            found.downbeat.strengths()
        );
        let downbeats = downbeats_of(&found);
        assert!(downbeats.len() >= 12, "{} downbeats", downbeats.len());
        // Every downbeat within a sixth of a beat of a bar line.
        let period_ms = 60_000.0 / 124.0;
        for at in &downbeats {
            let off = bar_lines.iter().map(|line| (line - at).abs()).fold(f64::MAX, f64::min);
            assert!(
                off < period_ms / 6.0,
                "a downbeat at {at:.0} ms is {off:.0} ms from a bar line"
            );
        }
    }

    #[test]
    fn a_clear_kick_is_not_overruled_by_the_music_around_it() {
        // The accent two beats after each bar line: where the chord changes
        // says one thing and the kick, clearly, another. The kick wins.
        let (audio, on) = floor_with_a_pad(16, Some(2));
        let accents = on(2);
        let found = detect(&features::extract(&audio));
        assert!(found.downbeat.kick_decides(), "{:?}", found.downbeat.strengths());
        let period_ms = 60_000.0 / 124.0;
        for at in downbeats_of(&found) {
            let off = accents.iter().map(|accent| (accent - at).abs()).fold(f64::MAX, f64::min);
            assert!(off < period_ms / 6.0, "a downbeat at {at:.0} ms is not on the accent");
        }
    }

    #[test]
    fn the_four_cues_together_are_shares_so_no_cue_counts_for_its_units() {
        let found = Downbeat {
            kicks: vec![1.0; 8],
            // A thousand times the units, and the same shape as the next.
            changes: vec![0.0, 1000.0, 0.0, 0.0, 0.0, 1000.0, 0.0, 0.0],
            bass_changes: vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            turns: vec![0.0; 8],
            ..Downbeat::default()
        };
        let together = found.together();
        assert!((together[1] - together[2]).abs() < 1e-6, "{together:?}");
        assert_eq!(lead([1.0, 1.0, 1.0, 1.0]), 1.0);
        assert_eq!(lead([0.0; 4]), 1.0, "nothing measured is no lead");
    }

    /// What each cue says about the downbeat on the records in
    /// `BOOTH_DRUM_RECORDS`, and where the downbeat was and now is. Prints;
    /// a record's answer here is only as good as the cue, there being no
    /// reference to hold it to.
    #[test]
    #[ignore = "needs records on disk"]
    fn real_records_downbeat_by_each_cue() {
        use crate::audio::decode::decode_file;
        let Some(records) = std::env::var_os("BOOTH_DRUM_RECORDS") else { return };
        let mut names: Vec<_> = std::fs::read_dir(records)
            .unwrap()
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "mp3" || ext == "flac"))
            .collect();
        names.sort();
        let said = |sums: [f32; 4]| format!("{} {:>5.2}", largest(sums) + 1, lead(sums));
        eprintln!(
            "{:<24} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}  halves  downbeat",
            "record", "kick", "change", "bass", "turns", "others", "together"
        );
        let mut moved = 0;
        for path in &names {
            let name: String =
                path.file_stem().unwrap().to_string_lossy().chars().take(24).collect();
            let found = detect(&features::extract(&decode_file(path).unwrap())).downbeat;
            let [kick, change, bass, turn] = found.cues();
            let before = largest(kick);
            moved += usize::from(before != found.phase);
            // Whether each half of the record, read alone, says what the other
            // does: the kick's, then the other three's together.
            let half = found.kicks.len() / 2 / 4 * 4;
            let halves = |pick: &dyn Fn(std::ops::Range<usize>) -> [f32; 4]| {
                largest(pick(0..half)) == largest(pick(half..found.kicks.len()))
            };
            let kick_halves = halves(&|range| found.strengths_over(range));
            let other_halves = halves(&|range| shares(&found.cues_over(range)[1..]));
            eprintln!(
                "{name:<24} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}  {} {}  {} -> {}{}",
                said(kick),
                said(change),
                said(bass),
                said(turn),
                said(shares(&[change, bass, turn])),
                said(found.together()),
                if kick_halves { "k" } else { "." },
                if other_halves { "o" } else { "." },
                before + 1,
                found.phase + 1,
                if found.kick_decides() { "" } else { "  (kick a near tie)" }
            );
        }
        eprintln!("{moved} of {} moved", names.len());
    }
}
