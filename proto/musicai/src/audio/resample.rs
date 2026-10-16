//! Changing a signal's sample rate, for handing it to a model.
//!
//! Two things in this program want audio at 16 kHz mono: the speech recogniser,
//! which will read nothing else, and the genre classifier, which was trained on
//! nothing else. Both are fed from here. Nothing anybody listens to goes
//! through it — see `compat.rs` for why a library is not written through a
//! resampler of this program's — so it is built for being right enough and
//! quick, not for being transparent.
//!
//! A windowed sinc, as a polyphase table. The fractional position of an output
//! sample repeats: 44.1 kHz to 16 kHz comes round every 160 output samples,
//! 48 kHz to 16 kHz every one, so the kernel is evaluated once per distinct
//! position and the conversion itself is a dot product per output sample. The
//! per-sample version this replaced evaluated three transcendentals per tap and
//! took longer over a track than the analysis did; this one takes half a second
//! over five minutes of 44.1 kHz, measured in a release build.

/// How many input samples either side of an output sample are read, at the
/// lower of the two rates. More is a sharper filter and a slower one.
const TAPS: f64 = 24.0;

/// The most positions a table is built for.
///
/// A standard pair of rates needs far fewer, and an odd pair that would need
/// more is rounded to the nearest of this many, which puts a sample at most
/// 1/8192 of an input sample from where it belongs — far under anything a
/// recogniser or a classifier can tell.
const MAX_PHASES: usize = 4096;

/// Change a signal's sample rate, band-limited.
///
/// The output is `floor(len * to / from)` samples long. Going down, everything
/// above the new Nyquist is filtered out rather than folded back — the whole
/// reason this is not a bare pick-every-nth, and the one property the tests
/// hold it to.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if input.is_empty() || from == to || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = to as f64 / from as f64;
    // Cutoff in cycles per input sample: below the output's Nyquist when going
    // down, with a little room for the filter to roll off in.
    let cutoff = 0.45 * ratio.min(1.0);
    let half = (TAPS / ratio.min(1.0)).ceil() as usize;
    let taps = 2 * half;

    // Output sample `n` sits at `n * from / to` input samples. The fractional
    // part cycles with period `to / gcd`, so that many positions is every
    // position there is.
    let period = (to / gcd(from, to)) as usize;
    let phases = period.min(MAX_PHASES);
    let table = Table::build(phases, half, cutoff);

    let out_len = (input.len() as u64 * to as u64 / from as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let scaled = n as u64 * from as u64;
        let whole = (scaled / to as u64) as usize;
        let remainder = scaled % to as u64;
        // Nearest row; the rounding can land on `phases`, which is why the
        // table has one more row than that.
        let phase = ((remainder * phases as u64 + to as u64 / 2) / to as u64) as usize;
        let row = table.row(phase);

        let first = whole as isize - half as isize + 1;
        let lo = first.max(0) as usize;
        let hi = (first + taps as isize).min(input.len() as isize) as usize;
        let used = &row[(lo as isize - first) as usize..(hi as isize - first) as usize];
        let sum: f32 = input[lo..hi].iter().zip(used).map(|(x, w)| x * w).sum();
        // Each row sums to one, so this is only at the two ends, where some of
        // the kernel hangs off the signal: dividing by what was actually used
        // gives a sane answer there rather than a fade.
        out.push(match used.len() == taps {
            true => sum,
            false => {
                let weight: f32 = used.iter().sum();
                if weight.abs() > 1e-6 {
                    sum / weight
                } else {
                    0.0
                }
            }
        });
    }
    out
}

/// The kernel at each fractional position, one row per position.
struct Table {
    taps: usize,
    weights: Vec<f32>,
}

impl Table {
    fn build(phases: usize, half: usize, cutoff: f64) -> Self {
        let taps = 2 * half;
        let mut weights = Vec::with_capacity((phases + 1) * taps);
        for phase in 0..=phases {
            let frac = phase as f64 / phases as f64;
            let start = weights.len();
            for m in 0..taps {
                let offset = m as f64 - half as f64 + 1.0 - frac;
                // Blackman window over the taps, so the stopband is deep
                // enough that what is filtered out stays out.
                let u = offset / half as f64;
                let window = 0.42
                    + 0.5 * (std::f64::consts::PI * u).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
                weights.push((window * sinc(2.0 * cutoff * offset)) as f32);
            }
            // Unity gain at every position, so a steady tone does not ripple
            // at the rate the positions cycle.
            let row = &mut weights[start..];
            let total: f32 = row.iter().sum();
            for w in row {
                *w /= total;
            }
        }
        Self { taps, weights }
    }

    fn row(&self, phase: usize) -> &[f32] {
        &self.weights[phase * self.taps..(phase + 1) * self.taps]
    }
}

fn sinc(x: f64) -> f64 {
    match x.abs() < 1e-9 {
        true => 1.0,
        false => {
            let pi_x = std::f64::consts::PI * x;
            pi_x.sin() / pi_x
        }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    match b {
        0 => a,
        _ => gcd(b, a % b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        let frames = (rate as f32 * secs) as usize;
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (2.0 * std::f32::consts::PI * hz * t).sin()
            })
            .collect()
    }

    fn energy(signal: &[f32]) -> f32 {
        signal.iter().map(|s| s * s).sum::<f32>() / signal.len().max(1) as f32
    }

    #[test]
    fn a_tone_inside_the_band_keeps_its_level_and_its_pitch() {
        let out = resample(&tone(1_000.0, 44_100, 1.0), 44_100, 16_000);
        assert!(out.len().abs_diff(16_000) < 4, "{} samples", out.len());

        let ratio = energy(&out[200..out.len() - 200]) / 0.5;
        assert!((ratio - 1.0).abs() < 0.02, "level changed by a factor of {ratio:.3}");

        // Against the tone as it should be at the new rate, sample for
        // sample: a resampler that was out by a fraction of a sample, or
        // that rippled with its own table, would not match this closely.
        let wanted = tone(1_000.0, 16_000, 1.0);
        let worst = out[200..out.len() - 200]
            .iter()
            .zip(&wanted[200..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "worst sample error {worst:.4}");
    }

    #[test]
    fn what_has_nowhere_to_go_is_filtered_out_rather_than_folded_back() {
        // 15 kHz has nowhere to go at 16 kHz: taking every nth sample would
        // fold it down to 1 kHz.
        let out = resample(&tone(15_000.0, 44_100, 1.0), 44_100, 16_000);
        let left = energy(&out[200..out.len() - 200]);
        assert!(left < 0.001, "{left:.5} of the 15 kHz tone survived as an alias");
    }

    #[test]
    fn an_integer_ratio_is_the_same_filter_with_one_position() {
        // 48 kHz to 16 kHz repeats every output sample, so the table is one
        // row and the result is still a clean tone at the right level.
        let out = resample(&tone(440.0, 48_000, 1.0), 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        let ratio = energy(&out[200..out.len() - 200]) / 0.5;
        assert!((ratio - 1.0).abs() < 0.02, "level changed by a factor of {ratio:.3}");
    }

    #[test]
    fn going_up_keeps_the_signal_too() {
        let out = resample(&tone(440.0, 8_000, 1.0), 8_000, 16_000);
        assert_eq!(out.len(), 16_000);
        let wanted = tone(440.0, 16_000, 1.0);
        let worst = out[200..out.len() - 200]
            .iter()
            .zip(&wanted[200..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "worst sample error {worst:.4}");
    }

    #[test]
    fn a_signal_already_at_the_rate_is_left_alone() {
        let input = tone(440.0, 16_000, 0.1);
        assert_eq!(resample(&input, 16_000, 16_000), input);
        assert!(resample(&[], 44_100, 16_000).is_empty());
    }

    #[test]
    fn the_ends_are_not_faded() {
        // A constant in gives a constant out, including in the first and last
        // samples where part of the kernel hangs off the signal.
        let out = resample(&vec![0.5f32; 4_410], 44_100, 16_000);
        assert!((out[0] - 0.5).abs() < 0.01, "first sample {}", out[0]);
        assert!((out[out.len() - 1] - 0.5).abs() < 0.01, "last sample {}", out[out.len() - 1]);
    }

    /// How long a track takes, for the doc comment above and nothing else.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn how_long_a_track_takes() {
        // Five minutes of a stereo-shaped signal at the studio rate, to mono.
        let input = tone(440.0, 44_100, 300.0);
        let started = std::time::Instant::now();
        let out = resample(&input, 44_100, 16_000);
        eprintln!("{} samples in {:.3}s", out.len(), started.elapsed().as_secs_f32());
        let started = std::time::Instant::now();
        let out = resample(&tone(440.0, 48_000, 300.0), 48_000, 16_000);
        eprintln!("48 kHz: {} samples in {:.3}s", out.len(), started.elapsed().as_secs_f32());
    }

    #[test]
    fn the_positions_of_the_standard_rates_are_counted_right() {
        assert_eq!(44_100 / gcd(44_100, 16_000), 441);
        assert_eq!(16_000 / gcd(44_100, 16_000), 160);
        assert_eq!(16_000 / gcd(48_000, 16_000), 1);
        assert_eq!(16_000 / gcd(22_050, 16_000), 320);
    }
}
