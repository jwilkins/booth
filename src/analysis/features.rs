//! One pass over the audio, producing everything the analysers need.
//!
//! Beat tracking, phrase detection and cue placement all want the same handful
//! of things: how much the spectrum changed between frames, where the energy
//! sits, and how centred it is in the stereo image. Computing them together
//! costs one FFT pair per frame and keeps a few floats per frame afterwards,
//! rather than a spectrogram — a five-minute track measures out at about a
//! megabyte instead of a couple of hundred.

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

use crate::audio::Audio;

/// Window length. At 44.1 kHz this is 46 ms — long enough to resolve a bass
/// note, short enough that a kick drum stays an event rather than a smear.
const N_FFT: usize = 2_048;
/// Hop between frames: a quarter of the window, giving about 86 frames a
/// second, which resolves beats comfortably past 300 BPM.
const HOP: usize = 512;
/// How many log-spaced bands the spectrum is summarised into for structural
/// comparison. Twelve is enough to tell a breakdown from a drop and few enough
/// that the similarity matrix stays cheap.
pub const BANDS: usize = 12;

const BAND_LOW_HZ: f32 = 40.0;
const BAND_HIGH_HZ: f32 = 16_000.0;
/// The range a sung voice occupies, fundamentals and the formants that matter.
const VOICE_LOW_HZ: f32 = 200.0;
const VOICE_HIGH_HZ: f32 = 4_000.0;
/// Kick drums and bass lines, for finding the downbeat.
const LOW_HZ: f32 = 200.0;

/// What one pass over a track measured, frame by frame.
pub struct Features {
    /// Frames per second.
    pub frame_rate: f64,
    /// Spectral flux: how much the spectrum grew since the previous frame.
    /// Peaks at note and drum onsets, which is what beat tracking follows.
    pub flux: Vec<f32>,
    /// The same, restricted to the bottom of the spectrum. A kick lands here
    /// and a hi-hat does not, which is how the downbeat is found.
    pub low_flux: Vec<f32>,
    /// Log energy in each of [`BANDS`] bands, frame-major.
    pub bands: Vec<f32>,
    /// Total energy per frame, in dB relative to full scale.
    pub level: Vec<f32>,
    /// How much of each frame is both centred in the stereo image and in the
    /// range a voice occupies. High where someone is singing over a wide
    /// backing; also high for a centred lead synth, which is the limit of what
    /// a measurement this cheap can tell you.
    pub voice: Vec<f32>,
}

impl Features {
    pub fn frames(&self) -> usize {
        self.flux.len()
    }

    /// The band energies of one frame.
    pub fn band_frame(&self, frame: usize) -> &[f32] {
        &self.bands[frame * BANDS..(frame + 1) * BANDS]
    }

    /// Which frame a moment in the track falls in.
    pub fn frame_at(&self, seconds: f64) -> usize {
        let frame = (seconds - self.window_offset()) * self.frame_rate;
        (frame.round().max(0.0) as usize).min(self.frames().saturating_sub(1))
    }

    /// When a frame happens.
    ///
    /// The middle of its window, not the start: a frame is a picture of the
    /// whole window, and an onset shows up most strongly when it sits under the
    /// centre of the window, where the taper weights it most. Timing frames
    /// from their starts puts every beat half a window early.
    pub fn seconds_at(&self, frame: usize) -> f64 {
        frame as f64 / self.frame_rate + self.window_offset()
    }

    fn window_offset(&self) -> f64 {
        (N_FFT / 2) as f64 / (self.frame_rate * HOP as f64)
    }
}

/// Measure a decoded track.
pub fn extract(audio: &Audio) -> Features {
    let sample_rate = audio.sample_rate as f32;
    let mut planner = RealFftPlanner::<f32>::new();
    let forward: Arc<dyn RealToComplex<f32>> = planner.plan_fft_forward(N_FFT);

    // A periodic Hann window. Nothing is reconstructed from these frames, so
    // the square-root window the separator needs would only cost resolution.
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
        .collect();

    let bins = N_FFT / 2 + 1;
    let bin_hz = sample_rate / N_FFT as f32;
    let band_of = band_map(bin_hz, bins);
    let voice_bins = bin_range(bin_hz, bins, VOICE_LOW_HZ, VOICE_HIGH_HZ);
    let low_bins = bin_range(bin_hz, bins, 0.0, LOW_HZ);

    let frames = if audio.frames() < N_FFT { 1 } else { (audio.frames() - N_FFT) / HOP + 1 };
    let mut features = Features {
        frame_rate: audio.sample_rate as f64 / HOP as f64,
        flux: Vec::with_capacity(frames),
        low_flux: Vec::with_capacity(frames),
        bands: Vec::with_capacity(frames * BANDS),
        level: Vec::with_capacity(frames),
        voice: Vec::with_capacity(frames),
    };

    let mut mid_time = vec![0.0f32; N_FFT];
    let mut side_time = vec![0.0f32; N_FFT];
    let mut mid_spec = vec![Complex32::default(); bins];
    let mut side_spec = vec![Complex32::default(); bins];
    let mut scratch = forward.make_scratch_vec();
    let mut previous = vec![0.0f32; bins];
    let mut current = vec![0.0f32; bins];
    // The transform is unnormalised, so scale power back to something where
    // full scale is one. Without this the noise floor of a pure tone sits high
    // enough for the log compression below to turn it into onsets.
    let scale = 1.0 / (N_FFT as f32 / 2.0).powi(2);

    for frame in 0..frames {
        let start = frame * HOP;
        fill_mid_side(audio, start, &window, &mut mid_time, &mut side_time);
        let _ = forward.process_with_scratch(&mut mid_time, &mut mid_spec, &mut scratch);
        let _ = forward.process_with_scratch(&mut side_time, &mut side_spec, &mut scratch);

        let mut total = 0.0f64;
        let mut voiced = 0.0f64;
        let mut band_energy = [0.0f64; BANDS];

        for bin in 0..bins {
            let mid = mid_spec[bin].norm_sqr() * scale;
            let side = side_spec[bin].norm_sqr() * scale;
            let power = mid + side;
            total += power as f64;
            band_energy[band_of[bin]] += power as f64;
            // Compression before differencing, so a change in a quiet band
            // counts for something next to a change in a loud one.
            current[bin] = (1.0 + 1_000.0 * power.sqrt()).ln();

            if bin >= voice_bins.0 && bin < voice_bins.1 {
                let centred = mid / (mid + side + f32::EPSILON);
                voiced += (mid * centred) as f64;
            }
        }

        let mut flux = 0.0f32;
        let mut low_flux = 0.0f32;
        for bin in 0..bins {
            let rise = (current[bin] - previous[bin]).max(0.0);
            flux += rise;
            if bin < low_bins.1 {
                low_flux += rise;
            }
        }
        std::mem::swap(&mut previous, &mut current);

        for energy in band_energy {
            features.bands.push((energy + 1e-12).ln() as f32);
        }
        features.flux.push(flux);
        features.low_flux.push(low_flux);
        features.level.push(10.0 * ((total / bins as f64) + 1e-12).log10() as f32);
        features.voice.push((voiced / (total + 1e-12)) as f32);
    }

    // The first frame has nothing to be different from, so its flux is an
    // artefact of starting rather than an onset.
    if let Some(first) = features.flux.first_mut() {
        *first = 0.0;
    }
    if let Some(first) = features.low_flux.first_mut() {
        *first = 0.0;
    }
    features
}

fn fill_mid_side(audio: &Audio, start: usize, window: &[f32], mid: &mut [f32], side: &mut [f32]) {
    let left = &audio.planes[0];
    let right = audio.planes.get(1).unwrap_or(left);
    for i in 0..window.len() {
        let at = start + i;
        let (l, r) = if at < left.len() { (left[at], right[at]) } else { (0.0, 0.0) };
        mid[i] = (l + r) * 0.5 * window[i];
        side[i] = (l - r) * 0.5 * window[i];
    }
}

/// Which band each bin belongs to, log-spaced across the audible range.
fn band_map(bin_hz: f32, bins: usize) -> Vec<usize> {
    let low = BAND_LOW_HZ.ln();
    let high = BAND_HIGH_HZ.ln();
    (0..bins)
        .map(|bin| {
            let hz = (bin as f32 * bin_hz).max(BAND_LOW_HZ);
            let position = (hz.ln() - low) / (high - low);
            ((position * BANDS as f32) as usize).min(BANDS - 1)
        })
        .collect()
}

fn bin_range(bin_hz: f32, bins: usize, low_hz: f32, high_hz: f32) -> (usize, usize) {
    let low = ((low_hz / bin_hz).ceil() as usize).min(bins);
    let high = ((high_hz / bin_hz).ceil() as usize).min(bins);
    (low, high.max(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A tone that starts part way in, so there is an onset to find. A tone
    /// present from the first sample has nothing to be an onset against.
    fn tone_from(hz: f32, start_secs: f32, total_secs: f32) -> Audio {
        let frames = (RATE as f32 * total_secs) as usize;
        let start = (RATE as f32 * start_secs) as usize;
        let plane: Vec<f32> = (0..frames)
            .map(|i| {
                if i < start {
                    return 0.0;
                }
                let t = i as f32 / RATE as f32;
                0.5 * (2.0 * std::f32::consts::PI * hz * t).sin()
            })
            .collect();
        Audio::new(RATE, vec![plane.clone(), plane]).unwrap()
    }

    fn peak(values: &[f32]) -> f32 {
        values.iter().fold(0.0f32, |a, &b| a.max(b))
    }

    #[test]
    fn frames_cover_the_track_at_the_expected_rate() {
        let features = extract(&tone_from(440.0, 0.0, 2.0));
        assert!((features.frame_rate - 86.13).abs() < 0.1);
        // Two seconds at 86 frames a second, less the first window.
        assert!((features.frames() as i64 - 168).abs() < 4, "{} frames", features.frames());
    }

    #[test]
    fn an_onset_towers_over_the_steady_tone_that_follows_it() {
        let features = extract(&tone_from(440.0, 0.5, 2.0));
        let onset = features.frame_at(0.5);

        // The window is four hops long, so the tone arrives over the four
        // frames before it fills the window completely.
        let arrival = peak(&features.flux[onset - 6..onset + 2]);
        let sustained: f32 =
            features.flux[onset + 8..].iter().sum::<f32>() / (features.frames() - onset - 8) as f32;

        assert!(arrival > 10.0, "an onset should be unmistakable: {arrival}");
        assert!(
            sustained < arrival / 100.0,
            "a steady tone should not keep producing flux: {sustained} against {arrival}"
        );
    }

    #[test]
    fn silence_produces_no_onsets_at_all() {
        let silent = Audio::new(RATE, vec![vec![0.0; RATE as usize]]).unwrap();
        let features = extract(&silent);
        assert_eq!(peak(&features.flux), 0.0);
        assert_eq!(peak(&features.voice), 0.0);
    }

    #[test]
    fn bass_lands_in_the_bottom_band_and_treble_in_the_top() {
        let loudest = |hz: f32| {
            let f = extract(&tone_from(hz, 0.0, 1.0));
            let frame = f.band_frame(f.frames() / 2);
            (0..BANDS).max_by(|a, b| frame[*a].total_cmp(&frame[*b])).unwrap()
        };
        assert_eq!(loudest(50.0), 0);
        assert_eq!(loudest(12_000.0), BANDS - 1);
    }

    #[test]
    fn a_kick_moves_the_low_band_and_a_hat_does_not() {
        let low_flux = |hz: f32| peak(&extract(&tone_from(hz, 0.3, 1.0)).low_flux);
        let kick = low_flux(60.0);
        let hat = low_flux(9_000.0);
        assert!(kick > hat * 5.0, "kick {kick} should dwarf hat {hat} in the low band");
    }

    #[test]
    fn a_centred_voice_reads_higher_than_a_wide_pad() {
        // Same frequency, same level; one in the middle of the image, one hard
        // out of phase and therefore nowhere near it.
        let signal: Vec<f32> = (0..RATE as usize)
            .map(|i| 0.4 * (2.0 * std::f32::consts::PI * 800.0 * i as f32 / RATE as f32).sin())
            .collect();
        let middle = |audio: Audio| {
            let f = extract(&audio);
            f.voice[f.frames() / 2]
        };
        let centred = middle(Audio::new(RATE, vec![signal.clone(), signal.clone()]).unwrap());
        let wide = middle(
            Audio::new(RATE, vec![signal.clone(), signal.iter().map(|s| -s).collect()]).unwrap(),
        );
        assert!(centred > wide * 10.0, "centred {centred} against wide {wide}");
    }

    #[test]
    fn a_bass_note_does_not_read_as_a_voice() {
        let voiced = |hz: f32| {
            let f = extract(&tone_from(hz, 0.0, 1.0));
            f.voice[f.frames() / 2]
        };
        assert!(voiced(800.0) > voiced(60.0) * 10.0);
    }

    #[test]
    fn level_follows_the_signal() {
        let f = extract(&tone_from(440.0, 0.5, 1.5));
        assert!(f.level[2] < -80.0, "silence should be very quiet: {}", f.level[2]);
        assert!(f.level[f.frames() - 2] > -60.0);
    }

    #[test]
    fn a_track_shorter_than_one_window_still_measures() {
        let short = Audio::new(RATE, vec![vec![0.1; 100]]).unwrap();
        let features = extract(&short);
        assert_eq!(features.frames(), 1);
    }
}
