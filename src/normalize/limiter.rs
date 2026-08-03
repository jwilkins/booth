//! A look-ahead peak limiter.
//!
//! The gain envelope is built in two steps: a centred sliding *minimum* of the
//! per-sample gain each peak demands, followed by two centred moving averages
//! that round off the corners. Because the minimum window is at least as wide
//! as the smoothing windows combined, every value that gets averaged into
//! sample `n` is already no greater than the gain `n` itself needs — so the
//! smoothing can only ever lower the envelope further, never raise it above
//! what the ceiling allows. That gives a limiter with no overshoot and no
//! zipper artefacts, without needing to tune attack and release separately.

use std::collections::VecDeque;

use crate::audio::Audio;
use crate::dsp::db_to_linear;

/// Default look-ahead. Long enough to ramp gently, short enough not to audibly
/// duck the material before a transient.
pub const DEFAULT_LOOKAHEAD_MS: f64 = 5.0;

/// Attenuate `audio` so that no sample exceeds `ceiling_db` dBFS.
///
/// Returns the deepest gain reduction applied, in dB (0.0 if the signal was
/// already under the ceiling).
pub fn limit(audio: &mut Audio, ceiling_db: f64, lookahead_ms: f64) -> f64 {
    let frames = audio.frames();
    if frames == 0 {
        return 0.0;
    }

    let ceiling = db_to_linear(ceiling_db) as f32;
    let lookahead = ((lookahead_ms / 1000.0) * audio.sample_rate as f64).round() as usize;
    let radius = lookahead.max(1).min(frames);

    // Gain each sample needs on its own.
    let mut desired = vec![1.0f32; frames];
    for plane in &audio.planes {
        for (d, &s) in desired.iter_mut().zip(plane) {
            let magnitude = s.abs();
            if magnitude > ceiling {
                *d = d.min(ceiling / magnitude);
            }
        }
    }

    if desired.iter().all(|&g| g >= 1.0) {
        return 0.0;
    }

    let envelope = sliding_min(&desired, radius);
    let envelope = moving_average(&envelope, radius / 2);
    let envelope = moving_average(&envelope, radius / 2);

    for plane in &mut audio.planes {
        for (s, &g) in plane.iter_mut().zip(&envelope) {
            *s *= g;
        }
    }

    // Report as a negative dB figure, e.g. -2.4 dB of reduction.
    let min_gain = envelope.iter().copied().fold(1.0f32, f32::min);
    crate::dsp::linear_to_db(min_gain as f64)
}

/// Minimum of `x` over a centred window of `+/-radius`, in linear time via a
/// monotonic deque.
fn sliding_min(x: &[f32], radius: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = Vec::with_capacity(n);
    let mut window: VecDeque<usize> = VecDeque::new();
    let mut next = 0usize;

    for i in 0..n {
        let hi = (i + radius).min(n - 1);
        while next <= hi {
            while window.back().is_some_and(|&j| x[j] >= x[next]) {
                window.pop_back();
            }
            window.push_back(next);
            next += 1;
        }
        let lo = i.saturating_sub(radius);
        while window.front().is_some_and(|&j| j < lo) {
            window.pop_front();
        }
        out.push(x[*window.front().expect("window is never empty")]);
    }
    out
}

/// Mean of `x` over a centred window of `+/-radius`, with edges clamped to the
/// available samples.
fn moving_average(x: &[f32], radius: usize) -> Vec<f32> {
    if radius == 0 || x.is_empty() {
        return x.to_vec();
    }
    let n = x.len();
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0.0f64);
    for &v in x {
        prefix.push(prefix[prefix.len() - 1] + v as f64);
    }

    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(n);
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::linear_to_db;

    fn spiky_audio() -> Audio {
        let mut plane = vec![0.0f32; 48_000];
        for (i, s) in plane.iter_mut().enumerate() {
            *s = 0.3 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 48_000.0).sin();
        }
        // Three transients well past full scale.
        for &i in &[5_000usize, 20_000, 44_000] {
            plane[i] = 2.5;
            plane[i + 1] = -2.2;
        }
        Audio::new(48_000, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn never_overshoots_the_ceiling() {
        let mut audio = spiky_audio();
        limit(&mut audio, -1.0, DEFAULT_LOOKAHEAD_MS);
        let peak_db = linear_to_db(audio.sample_peak() as f64);
        assert!(peak_db <= -1.0 + 1e-4, "peak came out at {peak_db} dBFS");
    }

    #[test]
    fn leaves_quiet_material_untouched() {
        let original = spiky_audio();
        let mut quiet = original.clone();
        quiet.scale(0.1);
        let before = quiet.clone();

        let reduction = limit(&mut quiet, -1.0, DEFAULT_LOOKAHEAD_MS);
        assert_eq!(reduction, 0.0);
        assert_eq!(quiet, before);
    }

    #[test]
    fn keeps_the_body_of_the_signal_close_to_unity() {
        // Gain reduction should be local to the transients, not a global duck.
        let mut audio = spiky_audio();
        let before = audio.clone();
        limit(&mut audio, -1.0, DEFAULT_LOOKAHEAD_MS);

        let untouched = 30_000; // far from every transient
        let ratio = audio.planes[0][untouched] / before.planes[0][untouched];
        assert!((ratio - 1.0).abs() < 1e-3, "steady-state gain drifted to {ratio}");
    }

    #[test]
    fn envelope_is_smooth() {
        // Adjacent gain steps should be small, or the limiter would buzz.
        let mut audio = spiky_audio();
        let before = audio.clone();
        limit(&mut audio, -1.0, DEFAULT_LOOKAHEAD_MS);

        let mut worst: f32 = 0.0;
        for i in 1..audio.frames() {
            let a = before.planes[0][i];
            let b = before.planes[0][i - 1];
            if a.abs() > 1e-6 && b.abs() > 1e-6 {
                let g0 = audio.planes[0][i] / a;
                let g1 = audio.planes[0][i - 1] / b;
                worst = worst.max((g0 - g1).abs());
            }
        }
        assert!(worst < 0.05, "largest single-sample gain step was {worst}");
    }

    #[test]
    fn sliding_min_matches_a_naive_scan() {
        let x: Vec<f32> = (0..200).map(|i| ((i * 37) % 61) as f32).collect();
        let radius = 7;
        for (i, &fast) in sliding_min(&x, radius).iter().enumerate() {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(x.len());
            let slow = x[lo..hi].iter().copied().fold(f32::INFINITY, f32::min);
            assert_eq!(fast, slow, "at {i}");
        }
    }

    #[test]
    fn handles_a_single_sample() {
        let mut audio = Audio::new(48_000, vec![vec![1.0]]).unwrap();
        limit(&mut audio, -6.0, DEFAULT_LOOKAHEAD_MS);
        assert!(audio.planes[0][0] <= db_to_linear(-6.0) as f32 + 1e-6);
    }
}
