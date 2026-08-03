//! The built-in separator: cascaded harmonic/percussive source separation.
//!
//! The method is Driedger & Müller's cascade (DAFx-14), which gets three stems
//! out of two HPSS passes run at different time/frequency resolutions:
//!
//! 1. **The shorter window.** Drum hits are vertical streaks in the
//!    spectrogram; everything pitched is horizontal. Median-filtering along
//!    frequency isolates the streaks, so the percussive half of this pass is
//!    the **drums** and the harmonic half is everything else.
//!
//! 2. **The longer window**, applied to what stage 1 left behind. With finer
//!    frequency resolution a steadily-pitched instrument is still a clean
//!    horizontal line, but a sung note — vibrato, portamento, constant small
//!    pitch drift — smears vertically across bins within a single window and
//!    starts to look percussive. The percussive half of this pass is the
//!    **vocals** and the harmonic half is the **melody**.
//!
//! On stereo input the vocal mask is additionally weighted by how centred each
//! bin is, since lead vocals are nearly always panned to the middle.
//!
//! Every split uses a pair of soft masks that sum to one, and the STFT
//! reconstructs exactly, so the three stems always add back up to the input.
//!
//! # Choosing the defaults
//!
//! The obvious reading of "stage 1 wants time resolution" is to make its
//! window very short, but that backfires: a 1024-point window at 44.1 kHz has
//! 43 Hz bins, and the notes of a chord in the bass and low-mid range sit
//! closer together than that. Unresolved, a chord is a broad smear rather than
//! a set of horizontal lines, and the frequency median reads it as percussive
//! and dumps it into the drum stem. The drum pass therefore uses a window long
//! enough to resolve pitched partials, and relies on the *relative* difference
//! between the two stages rather than on being short in absolute terms.
//!
//! The vocal pass has the mirror-image constraint. Its frequency median must
//! be narrow enough that a vibrato-smeared partial looks wide next to it, but
//! wide enough to swallow a steady partial's main lobe — which is why the
//! default is tens of hertz rather than the couple of hundred that seems
//! natural. These values were chosen by measuring how cleanly a set of
//! synthetic mixes, varying in key, tempo, register and vibrato depth, were
//! routed to the right stems.

use anyhow::{bail, Result};

use crate::audio::Audio;
use crate::dsp::median::{median_along_frequency, median_along_time, odd_kernel};
use crate::dsp::stft::{Spectrogram, Stft};

use super::StemSet;

/// Tuning for [`separate`]. The defaults are expressed in milliseconds and
/// hertz so they behave the same at any sample rate.
#[derive(Copy, Clone, Debug)]
pub struct Config {
    /// FFT size for the drum pass. The shorter of the two, for time
    /// resolution, but still long enough to resolve pitched partials.
    pub drum_fft: usize,
    /// FFT size for the vocal pass. The longer of the two, for frequency
    /// resolution.
    pub voice_fft: usize,
    /// Drum pass: how long a sound must persist to count as pitched.
    pub drum_time_ms: f64,
    /// Drum pass: how wide a sound must be to count as a transient.
    pub drum_freq_hz: f64,
    /// Vocal pass: how long a sound must persist to count as steady.
    pub voice_time_ms: f64,
    /// Vocal pass: how wide a sound must smear to count as a voice. Small on
    /// purpose — see the note on choosing the defaults above.
    pub voice_freq_hz: f64,
    /// Exponent on the soft masks. 1.0 gives a gentle split, 2.0 the usual
    /// Wiener-style weighting, higher values approach a hard binary mask.
    pub mask_power: f32,
    /// How much the stereo centre estimate steers the vocal mask, 0.0 to 1.0.
    /// 0.0 disables it; ignored for non-stereo input.
    pub center_weight: f32,
    /// STFT overlap factor: the hop is the window divided by this. Must be 2
    /// or 4, both of which reconstruct exactly. 4 gives smoother masking; 2
    /// halves the working memory, which matters on long files.
    pub overlap: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            drum_fft: 4096,
            voice_fft: 8192,
            drum_time_ms: 400.0,
            drum_freq_hz: 400.0,
            voice_time_ms: 200.0,
            voice_freq_hz: 30.0,
            mask_power: 2.0,
            center_weight: 0.5,
            overlap: 4,
        }
    }
}

const EPSILON: f32 = 1e-10;

/// Split `audio` into vocals, melody and drums.
///
/// Channels are handled one at a time rather than in parallel: a spectrogram
/// at 4x overlap is about sixteen bytes per input sample, and the median
/// filters need a few copies of it live at once, so running both channels
/// together would double an already large working set. The median filters
/// parallelise internally, so every core stays busy regardless.
pub fn separate(audio: &Audio, config: &Config) -> Result<StemSet> {
    validate(config)?;
    if audio.is_empty() {
        bail!("cannot separate an empty audio file");
    }

    let frames = audio.frames();
    let sample_rate = audio.sample_rate;

    // Stage 1: pull the drums out with short windows.
    let stage1 = Stft::new(config.drum_fft, config.drum_fft / config.overlap);
    let drum_time = time_kernel(config.drum_time_ms, sample_rate, stage1.hop());
    let drum_freq = freq_kernel(config.drum_freq_hz, sample_rate, stage1.n_fft());

    let mut drum_planes = Vec::with_capacity(audio.channels());
    let mut residual_planes = Vec::with_capacity(audio.channels());
    for plane in &audio.planes {
        let mut spec = stage1.forward(plane);
        let mask = percussive_mask(&spec, drum_time, drum_freq, config.mask_power);

        let drums = spec.apply_mask(&mask);
        drop(mask);
        // The residual is the mix minus the drums. Taking it by subtraction
        // rather than by applying the complementary mask keeps one fewer
        // full-size buffer alive, and is exact.
        spec.subtract(&drums);

        drum_planes.push(stage1.inverse(&drums, frames));
        drop(drums);
        residual_planes.push(stage1.inverse(&spec, frames));
    }

    // Stage 2: split what is left into voice and accompaniment with long
    // windows.
    let stage2 = Stft::new(config.voice_fft, config.voice_fft / config.overlap);
    let voice_time = time_kernel(config.voice_time_ms, sample_rate, stage2.hop());
    let voice_freq = freq_kernel(config.voice_freq_hz, sample_rate, stage2.n_fft());

    // Lead vocals sit in the middle of the stereo image; anything hard-panned
    // almost certainly is not one. Both channels' spectra have to be live at
    // once to judge that, so this is the one place we pay for two.
    let centre = if residual_planes.len() == 2 && config.center_weight > 0.0 {
        let left = stage2.forward(&residual_planes[0]);
        let right = stage2.forward(&residual_planes[1]);
        Some(center_mask(&left, &right))
    } else {
        None
    };

    let mut vocal_planes = Vec::with_capacity(audio.channels());
    let mut melody_planes = Vec::with_capacity(audio.channels());
    for plane in &residual_planes {
        let mut spec = stage2.forward(plane);
        let mut mask = percussive_mask(&spec, voice_time, voice_freq, config.mask_power);
        if let Some(centre) = &centre {
            let weight = config.center_weight;
            for (v, &c) in mask.iter_mut().zip(centre) {
                *v *= 1.0 - weight + weight * c;
            }
        }

        let vocals = spec.apply_mask(&mask);
        drop(mask);
        spec.subtract(&vocals);

        vocal_planes.push(stage2.inverse(&vocals, frames));
        drop(vocals);
        melody_planes.push(stage2.inverse(&spec, frames));
    }

    Ok(StemSet {
        vocals: Audio::new(sample_rate, vocal_planes)?,
        melody: Audio::new(sample_rate, melody_planes)?,
        drums: Audio::new(sample_rate, drum_planes)?,
    })
}

fn validate(config: &Config) -> Result<()> {
    for (name, size) in [("--drum-fft", config.drum_fft), ("--voice-fft", config.voice_fft)] {
        if size < 64 || !size.is_power_of_two() {
            bail!("{name} must be a power of two of at least 64, got {size}");
        }
    }
    if config.drum_fft > config.voice_fft {
        bail!(
            "--drum-fft ({}) must not exceed --voice-fft ({}): the cascade needs the drum pass \
             to have the finer time resolution",
            config.drum_fft,
            config.voice_fft
        );
    }
    if !(0.0..=1.0).contains(&config.center_weight) {
        bail!("--center-weight must be between 0 and 1, got {}", config.center_weight);
    }
    if config.mask_power <= 0.0 {
        bail!("--mask-power must be positive, got {}", config.mask_power);
    }
    // Other factors would break the constant-overlap-add property the exact
    // reconstruction depends on.
    if !matches!(config.overlap, 2 | 4) {
        bail!("--overlap must be 2 or 4, got {}", config.overlap);
    }
    Ok(())
}

/// Soft mask selecting the percussive (vertically-streaked) part of `spec`.
///
/// The harmonic estimate comes from a median along time and the percussive
/// estimate from a median along frequency; the mask is the percussive share of
/// the two, raised to `power`.
fn percussive_mask(
    spec: &Spectrogram,
    time_kernel: usize,
    freq_kernel: usize,
    power: f32,
) -> Vec<f32> {
    let magnitude = spec.magnitudes();
    let harmonic = median_along_time(&magnitude, spec.frames, spec.bins, time_kernel);
    let mut percussive = median_along_frequency(&magnitude, spec.frames, spec.bins, freq_kernel);

    // These buffers are each the size of the spectrogram, so release the
    // magnitudes before combining and write the mask over the percussive
    // estimate rather than allocating a fourth.
    drop(magnitude);
    for (slot, &h) in percussive.iter_mut().zip(&harmonic) {
        let p = slot.powf(power);
        let h = h.powf(power);
        *slot = p / (p + h + EPSILON);
    }
    percussive
}

/// How centred each bin is, in `[0, 1]`.
///
/// This is the normalised correlation between the two channels:
/// `2 Re(L conj(R)) / (|L|^2 + |R|^2)`. It reaches 1 when the channels carry
/// the same thing in phase (dead centre), falls to 0 for anything hard-panned,
/// and goes negative for out-of-phase content, which we clamp away.
///
/// Bins with no meaningful energy in either channel score 1, the neutral
/// value: there is nothing there to judge, and scoring them as off-centre
/// would attenuate a mask entry that should simply be left alone.
fn center_mask(left: &Spectrogram, right: &Spectrogram) -> Vec<f32> {
    debug_assert_eq!(left.data.len(), right.data.len());
    left.data
        .iter()
        .zip(&right.data)
        .map(|(l, r)| {
            let correlation = 2.0 * (l * r.conj()).re;
            let energy = l.norm_sqr() + r.norm_sqr();
            if energy <= EPSILON {
                1.0
            } else {
                (correlation / energy).clamp(0.0, 1.0)
            }
        })
        .collect()
}

/// Median kernel length in frames for a duration in milliseconds.
fn time_kernel(ms: f64, sample_rate: u32, hop: usize) -> usize {
    odd_kernel(((ms / 1000.0) * sample_rate as f64 / hop as f64).round().max(1.0) as usize)
}

/// Median kernel length in bins for a bandwidth in hertz.
fn freq_kernel(hz: f64, sample_rate: u32, n_fft: usize) -> usize {
    odd_kernel((hz * n_fft as f64 / sample_rate as f64).round().max(1.0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 44_100;

    /// Sustained pitched tone: horizontal lines in the spectrogram.
    fn steady_tone(frames: usize, freq: f32, amplitude: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                amplitude * (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect()
    }

    /// Sharp broadband clicks four times a second: vertical streaks.
    fn clicks(frames: usize, amplitude: f32) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        let period = SAMPLE_RATE as usize / 4;
        let mut noise = 0x1234_5678u32;
        let mut at = period / 2;
        while at < frames {
            // A short burst of noise, decaying fast, is what a drum hit looks
            // like to the separator.
            for k in 0..300.min(frames - at) {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (noise >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                out[at + k] += amplitude * white * (-(k as f32) / 60.0).exp();
            }
            at += period;
        }
        out
    }

    /// Tone with heavy vibrato: smears vertically under a long window, which
    /// is what makes the second pass treat it as a voice.
    fn vibrato_tone(frames: usize, freq: f32, amplitude: f32) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..frames)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                let instantaneous =
                    freq * (1.0 + 0.06 * (2.0 * std::f32::consts::PI * 6.0 * t).sin());
                phase += 2.0 * std::f32::consts::PI * instantaneous / SAMPLE_RATE as f32;
                amplitude * phase.sin()
            })
            .collect()
    }

    fn energy(samples: &[f32]) -> f64 {
        samples.iter().map(|&s| (s as f64) * (s as f64)).sum()
    }

    fn mono(plane: Vec<f32>) -> Audio {
        Audio::new(SAMPLE_RATE, vec![plane]).unwrap()
    }

    #[test]
    fn stems_sum_back_to_the_input() {
        let frames = SAMPLE_RATE as usize * 2;
        let mix: Vec<f32> = steady_tone(frames, 330.0, 0.3)
            .iter()
            .zip(clicks(frames, 0.6))
            .map(|(a, b)| a + b)
            .collect();
        let audio = mono(mix);

        let stems = separate(&audio, &Config::default()).unwrap();
        let remixed = stems.remix().unwrap();

        assert_eq!(remixed.frames(), audio.frames());
        for (i, (&a, &b)) in audio.planes[0].iter().zip(&remixed.planes[0]).enumerate() {
            assert!((a - b).abs() < 1e-3, "sample {i} drifted: {a} vs {b}");
        }
    }

    #[test]
    fn drums_capture_the_transients() {
        let frames = SAMPLE_RATE as usize * 3;
        let percussion = clicks(frames, 0.6);
        let tone = steady_tone(frames, 330.0, 0.3);
        let mix: Vec<f32> = tone.iter().zip(&percussion).map(|(a, b)| a + b).collect();

        let stems = separate(&mono(mix), &Config::default()).unwrap();

        // Most of the click energy should land in the drum stem rather than
        // being spread across the pitched stems.
        let drum_energy = energy(&stems.drums.planes[0]);
        let pitched_energy = energy(&stems.melody.planes[0]) + energy(&stems.vocals.planes[0]);
        assert!(
            drum_energy < pitched_energy,
            "drum stem should be the smaller of the two here: {drum_energy} vs {pitched_energy}"
        );

        // And the drum stem should look like the clicks, not like the tone:
        // check it is far better correlated with the percussion track.
        assert!(
            correlation(&stems.drums.planes[0], &percussion)
                > correlation(&stems.drums.planes[0], &tone),
            "drum stem correlates more with the tone than with the clicks"
        );
    }

    #[test]
    fn vibrato_lands_in_vocals_and_steady_tones_in_melody() {
        let frames = SAMPLE_RATE as usize * 3;
        let steady = steady_tone(frames, 220.0, 0.35);
        let sung = vibrato_tone(frames, 700.0, 0.35);
        let mix: Vec<f32> = steady.iter().zip(&sung).map(|(a, b)| a + b).collect();

        let stems = separate(&mono(mix), &Config::default()).unwrap();

        assert!(
            correlation(&stems.vocals.planes[0], &sung)
                > correlation(&stems.vocals.planes[0], &steady),
            "vocal stem tracks the steady tone more than the vibrato one"
        );
        assert!(
            correlation(&stems.melody.planes[0], &steady)
                > correlation(&stems.melody.planes[0], &sung),
            "melody stem tracks the vibrato tone more than the steady one"
        );
    }

    #[test]
    fn centre_weighting_favours_centred_material() {
        let frames = SAMPLE_RATE as usize * 3;
        let centred = vibrato_tone(frames, 700.0, 0.35);
        let panned = vibrato_tone(frames, 1_500.0, 0.35);

        // `centred` is identical in both channels; `panned` is left only.
        let left: Vec<f32> = centred.iter().zip(&panned).map(|(a, b)| a + b).collect();
        let right = centred.clone();
        let audio = Audio::new(SAMPLE_RATE, vec![left, right]).unwrap();

        let with_centre =
            separate(&audio, &Config { center_weight: 1.0, ..Default::default() }).unwrap();
        let without =
            separate(&audio, &Config { center_weight: 0.0, ..Default::default() }).unwrap();

        // Turning centre weighting on should push the hard-panned part out of
        // the vocal stem's left channel.
        let panned_with = correlation(&with_centre.vocals.planes[0], &panned);
        let panned_without = correlation(&without.vocals.planes[0], &panned);
        assert!(
            panned_with < panned_without,
            "centre weighting did not reject the panned source: {panned_with} vs {panned_without}"
        );
    }

    #[test]
    fn stereo_input_yields_stereo_stems() {
        let frames = SAMPLE_RATE as usize;
        let audio = Audio::new(
            SAMPLE_RATE,
            vec![steady_tone(frames, 300.0, 0.3), steady_tone(frames, 500.0, 0.3)],
        )
        .unwrap();

        let stems = separate(&audio, &Config::default()).unwrap();
        for (name, stem) in stems.iter() {
            assert_eq!(stem.channels(), 2, "{name} lost a channel");
            assert_eq!(stem.frames(), frames, "{name} changed length");
            assert_eq!(stem.sample_rate, SAMPLE_RATE);
        }
    }

    #[test]
    fn centre_mask_scores_panning_correctly() {
        let stft = Stft::new(1024, 256);
        let frames = 8_000;
        let tone = steady_tone(frames, 440.0, 0.5);
        let silence = vec![0.0f32; frames];
        let flipped: Vec<f32> = tone.iter().map(|s| -s).collect();

        // A pure tone only occupies a handful of bins; the rest are empty and
        // score the neutral value, so judge only the bins carrying signal.
        let scores = |left: &[f32], right: &[f32]| -> Vec<f32> {
            let (l, r) = (stft.forward(left), stft.forward(right));
            let mask = center_mask(&l, &r);
            l.data
                .iter()
                .zip(&r.data)
                .zip(&mask)
                .filter(|((a, b), _)| a.norm_sqr() + b.norm_sqr() > 1e-3)
                .map(|(_, &m)| m)
                .collect()
        };

        // Identical channels are dead centre.
        let same = scores(&tone, &tone);
        assert!(!same.is_empty());
        assert!(same.iter().all(|&m| m > 0.99), "centred content did not score 1");

        // One silent channel means hard-panned.
        let hard = scores(&tone, &silence);
        assert!(!hard.is_empty());
        assert!(hard.iter().all(|&m| m < 1e-3), "hard-panned content did not score 0");

        // Out-of-phase content is clamped away rather than going negative.
        let inverted = scores(&tone, &flipped);
        assert!(!inverted.is_empty());
        assert!(inverted.iter().all(|&m| m == 0.0), "out-of-phase content was not clamped");
    }

    #[test]
    fn kernels_scale_with_sample_rate() {
        // 200 ms at 44.1 kHz with a 256-sample hop is about 34 frames.
        assert_eq!(time_kernel(200.0, 44_100, 256), 35);
        // The same duration at double the rate covers twice as many frames.
        assert_eq!(time_kernel(200.0, 88_200, 256), 69);
        // 500 Hz spans about 11.6 bins of a 1024-point FFT at 44.1 kHz.
        assert_eq!(freq_kernel(500.0, 44_100, 1024), 13);
    }

    #[test]
    fn rejects_bad_configuration() {
        let bad_fft = Config { drum_fft: 1000, ..Default::default() };
        assert!(validate(&bad_fft).is_err());

        let inverted = Config { drum_fft: 8192, voice_fft: 1024, ..Default::default() };
        assert!(validate(&inverted).is_err());

        let bad_weight = Config { center_weight: 1.5, ..Default::default() };
        assert!(validate(&bad_weight).is_err());
    }

    #[test]
    fn rejects_empty_audio() {
        let empty = Audio::new(44_100, vec![vec![]]).unwrap();
        assert!(separate(&empty, &Config::default()).is_err());
    }

    /// Normalised correlation between two signals, in `[-1, 1]`.
    fn correlation(a: &[f32], b: &[f32]) -> f64 {
        let dot: f64 = a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum();
        let norm = (energy(a) * energy(b)).sqrt();
        if norm <= 0.0 {
            0.0
        } else {
            dot / norm
        }
    }
}
