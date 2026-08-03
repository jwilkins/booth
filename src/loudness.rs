//! EBU R128 loudness measurement.

use anyhow::{Context, Result};
use ebur128::{EbuR128, Mode};

use crate::audio::Audio;
use crate::dsp::linear_to_db;

/// The measurements that drive every normalization decision.
#[derive(Copy, Clone, Debug)]
pub struct Loudness {
    /// Integrated (gated) loudness in LUFS. `-inf` for digital silence.
    pub integrated_lufs: f64,
    /// Loudness range in LU.
    pub range_lu: f64,
    /// Highest true (inter-sample) peak across channels, linear.
    pub true_peak: f64,
    /// Highest sample peak across channels, linear.
    pub sample_peak: f64,
}

impl Loudness {
    pub fn true_peak_db(&self) -> f64 {
        linear_to_db(self.true_peak)
    }

    pub fn sample_peak_db(&self) -> f64 {
        linear_to_db(self.sample_peak)
    }

    /// True when the file carries no measurable programme loudness, which
    /// means gain calculations against it are meaningless.
    pub fn is_silent(&self) -> bool {
        !self.integrated_lufs.is_finite()
    }
}

/// Measure integrated loudness, loudness range, and peaks in one pass.
pub fn measure(audio: &Audio) -> Result<Loudness> {
    let meter = meter_for(audio)?;
    summarize(&meter)
}

/// Run `audio` through a meter and hand the meter back.
///
/// Album ReplayGain needs the per-track meters kept alive so their gating
/// histories can be pooled by [`album_loudness`].
pub fn meter_for(audio: &Audio) -> Result<EbuR128> {
    let channels = u32::try_from(audio.channels()).context("too many channels to measure")?;
    let mut meter = EbuR128::new(
        channels,
        audio.sample_rate,
        Mode::I | Mode::LRA | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK,
    )
    .context("initialising the loudness meter")?;

    // The meter's own resampler for true-peak detection is stateful, so feed
    // the whole file in reasonably sized blocks rather than all at once.
    const BLOCK: usize = 65_536;
    let frames = audio.frames();
    let mut offset = 0;
    while offset < frames {
        let end = (offset + BLOCK).min(frames);
        let block: Vec<&[f32]> = audio.planes.iter().map(|p| &p[offset..end]).collect();
        meter.add_frames_planar_f32(&block).context("feeding the loudness meter")?;
        offset = end;
    }

    Ok(meter)
}

/// Read the results out of a meter that has already been fed.
pub fn summarize(meter: &EbuR128) -> Result<Loudness> {
    let integrated_lufs = meter.loudness_global().context("reading integrated loudness")?;
    // Loudness range needs several seconds of audio before it means anything;
    // report 0 rather than failing on a short clip.
    let range_lu = meter.loudness_range().unwrap_or(0.0);

    let mut true_peak = 0.0f64;
    let mut sample_peak = 0.0f64;
    for channel in 0..meter.channels() {
        true_peak = true_peak.max(meter.true_peak(channel).unwrap_or(0.0));
        sample_peak = sample_peak.max(meter.sample_peak(channel).unwrap_or(0.0));
    }

    Ok(Loudness { integrated_lufs, range_lu, true_peak, sample_peak })
}

/// Pooled integrated loudness across several tracks, as album ReplayGain
/// requires. This is not the mean of the per-track figures: the gating blocks
/// from every track are pooled and gated together, so a quiet track pulls the
/// album figure down only in proportion to how much of the album it is.
pub fn album_loudness(meters: &[EbuR128]) -> Result<f64> {
    EbuR128::loudness_global_multiple(meters.iter()).context("pooling album loudness")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f32, seconds: f64) -> Audio {
        let sample_rate = 48_000;
        let frames = (sample_rate as f64 * seconds) as usize;
        let plane: Vec<f32> = (0..frames)
            .map(|i| {
                amplitude
                    * (2.0 * std::f32::consts::PI * 1_000.0 * i as f32 / sample_rate as f32).sin()
            })
            .collect();
        Audio::new(sample_rate, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn measures_a_reference_tone() {
        // A full-scale 1 kHz sine in both channels reads about 0 LUFS: the
        // BS.1770 sum gives -0.691 (two channels, each with a mean square of
        // 0.5, plus the standard's offset) and K-weighting adds roughly
        // +0.7 dB at 1 kHz. Note this is not the -3.01 dBFS RMS of one
        // channel — loudness sums the channels.
        let loudness = measure(&sine(1.0, 5.0)).unwrap();
        assert!(loudness.integrated_lufs.abs() < 0.5, "got {} LUFS", loudness.integrated_lufs);
        assert!((loudness.sample_peak - 1.0).abs() < 0.01);
    }

    #[test]
    fn halving_amplitude_drops_six_db() {
        let loud = measure(&sine(1.0, 4.0)).unwrap();
        let quiet = measure(&sine(0.5, 4.0)).unwrap();
        assert!(
            (loud.integrated_lufs - quiet.integrated_lufs - 6.02).abs() < 0.1,
            "{} vs {}",
            loud.integrated_lufs,
            quiet.integrated_lufs
        );
    }

    #[test]
    fn reports_silence_as_non_finite() {
        let silence = Audio::new(48_000, vec![vec![0.0; 48_000], vec![0.0; 48_000]]).unwrap();
        assert!(measure(&silence).unwrap().is_silent());
    }
}
