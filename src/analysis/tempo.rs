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
const TEMPO_WINDOW: usize = 4;
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
    let steady = straighten(&mut times);
    let bpm = steady.unwrap_or_else(|| measured_bpm(&times));
    let mut grid = grid_from(&times, steady);
    let phase = downbeat_phase(features, &frames);
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

/// Fit a constant tempo to the tracked beats, and if it fits, use it.
///
/// A production that was made to a click really is at one tempo, and a grid
/// that wobbles by ten milliseconds a beat is worse than one that does not —
/// it makes the player's tempo readout flicker and it puts every cue slightly
/// off. So: fit a line through the beat times, and if no beat is far from it,
/// replace the tracked times with the line. Returns the tempo when it did.
fn straighten(times: &mut [u32]) -> Option<f64> {
    if times.len() < 8 {
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
    if period <= 0.0 {
        return None;
    }
    let first = mean_time - period * mean_index;

    let worst = times
        .iter()
        .enumerate()
        .map(|(i, &time)| (time as f64 - (first + period * i as f64)).abs())
        .fold(0.0f64, f64::max);
    // A twentieth of a beat. Past that the track is not at one tempo and the
    // tracked times, wobble and all, are the truthful answer.
    if worst > period / 20.0 {
        return None;
    }

    for (i, time) in times.iter_mut().enumerate() {
        *time = (first + period * i as f64).round().max(0.0) as u32;
    }
    Some(60_000.0 / period)
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

/// Build a grid from beat times, quoting each beat's tempo from the gaps around
/// it rather than from the one gap that follows it, which is too noisy to read.
fn grid_from(times: &[u32], steady: Option<f64>) -> BeatGrid {
    let gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    let beats = times
        .iter()
        .enumerate()
        .map(|(i, &time)| {
            let bpm = steady.unwrap_or_else(|| {
                let from = i.saturating_sub(TEMPO_WINDOW);
                let to = (i + TEMPO_WINDOW).min(gaps.len());
                let mut local: Vec<f64> = gaps.get(from..to).unwrap_or(&[]).to_vec();
                local.sort_by(f64::total_cmp);
                let gap = local.get(local.len() / 2).copied().unwrap_or(0.0);
                if gap > 0.0 {
                    60_000.0 / gap
                } else {
                    0.0
                }
            });
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
