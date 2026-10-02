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
//! possible places the bar could start, the one where the kicks are.

use crate::export::{Beat, BeatGrid};

use super::features::Features;

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
        return Beats { grid: BeatGrid::default(), bpm: 0.0, confidence: 0.0 };
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
        return Beats { grid: BeatGrid::default(), bpm: 0.0, confidence };
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
    let phase = downbeat_phase(features, &frames) + 4 - dropped % 4;
    renumber(&mut grid, phase);

    Beats { grid, bpm, confidence }
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

/// Which of the four positions in the bar the first beat holds.
///
/// Kicks land on the downbeat far more often than not, so the phase whose beats
/// carry the most low-frequency onset wins.
fn downbeat_phase(features: &Features, beats: &[usize]) -> usize {
    if beats.len() < 8 {
        return 0;
    }
    let mut best = (0usize, f32::MIN);
    for phase in 0..4 {
        let strength: f32 = beats
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 4 == phase)
            .map(|(_, &frame)| {
                // A beat marker can sit a couple of frames either side of the
                // transient it belongs to, and the transient itself takes a
                // window to arrive.
                let from = frame.saturating_sub(2);
                let to = (frame + 3).min(features.low_flux.len());
                features.low_flux[from..to].iter().fold(0.0f32, |a, &b| a.max(b))
            })
            .sum();
        if strength > best.1 {
            best = (phase, strength);
        }
    }
    best.0
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
}
