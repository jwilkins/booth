//! Loudness normalization: the re-encode path (change the samples) and the
//! ReplayGain path (change only the tags).

pub mod limiter;
pub mod replaygain;

use anyhow::Result;

use crate::audio::Audio;
use crate::dsp::db_to_linear;
use crate::loudness::{self, Loudness};

/// Default target for the re-encode path: the level most streaming services
/// normalize to.
pub const STREAMING_TARGET_LUFS: f64 = -14.0;

/// Reference level defined by the ReplayGain 2.0 specification.
pub const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;

/// How to reconcile the loudness target with the peak ceiling when the two
/// disagree.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum PeakPolicy {
    /// Apply less gain than the target asks for, so the ceiling is respected
    /// and the waveform is otherwise untouched. The track ends up quieter than
    /// the target.
    Attenuate,
    /// Apply the full gain and pull the transients back down with a look-ahead
    /// limiter. Hits the target exactly, at the cost of altering peaks.
    Limit,
}

#[derive(Copy, Clone, Debug)]
pub struct Settings {
    pub target_lufs: f64,
    /// Maximum permitted true peak, in dBTP.
    pub ceiling_dbtp: f64,
    pub peak_policy: PeakPolicy,
    pub lookahead_ms: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_lufs: STREAMING_TARGET_LUFS,
            ceiling_dbtp: -1.0,
            peak_policy: PeakPolicy::Attenuate,
            lookahead_ms: limiter::DEFAULT_LOOKAHEAD_MS,
        }
    }
}

/// What normalization actually did, for reporting back to the user.
#[derive(Copy, Clone, Debug)]
pub struct Report {
    pub before: Loudness,
    pub after: Loudness,
    /// Flat gain applied before any limiting.
    pub gain_db: f64,
    /// Deepest gain reduction the limiter applied, in dB (0.0 if it did not
    /// engage or was not used).
    pub limiter_reduction_db: f64,
    /// Gain we wanted but could not apply without breaching the ceiling. Zero
    /// under [`PeakPolicy::Limit`].
    pub withheld_db: f64,
    /// The input had no measurable loudness, so nothing was changed.
    pub silent: bool,
}

impl Report {
    /// How far the result landed from the requested target, in LU.
    pub fn target_error_lu(&self, target_lufs: f64) -> f64 {
        if self.after.is_silent() {
            0.0
        } else {
            self.after.integrated_lufs - target_lufs
        }
    }
}

/// Normalize `audio` in place.
pub fn apply(audio: &mut Audio, settings: &Settings) -> Result<Report> {
    let before = loudness::measure(audio)?;

    if before.is_silent() {
        return Ok(Report {
            before,
            after: before,
            gain_db: 0.0,
            limiter_reduction_db: 0.0,
            withheld_db: 0.0,
            silent: true,
        });
    }

    let wanted_db = settings.target_lufs - before.integrated_lufs;

    let (gain_db, withheld_db) = match settings.peak_policy {
        PeakPolicy::Limit => (wanted_db, 0.0),
        PeakPolicy::Attenuate => {
            // The most gain we can add before the loudest true peak reaches the
            // ceiling. Negative if the input already breaches it, in which case
            // we turn the track down even if it is quieter than the target.
            let headroom_db = settings.ceiling_dbtp - before.true_peak_db();
            let applied = wanted_db.min(headroom_db);
            (applied, wanted_db - applied)
        }
    };

    audio.scale(db_to_linear(gain_db) as f32);

    let mut limiter_reduction_db = 0.0;
    if settings.peak_policy == PeakPolicy::Limit {
        // The limiter works on sample peaks, but the ceiling is specified in
        // true peak. Inter-sample peaks overshoot sample peaks by an amount
        // that is a property of the waveform, not its level, so measure that
        // overshoot once and aim the limiter that much lower.
        let overshoot_db = (before.true_peak_db() - before.sample_peak_db()).max(0.0);
        limiter_reduction_db =
            limiter::limit(audio, settings.ceiling_dbtp - overshoot_db, settings.lookahead_ms);

        // Limiting reshapes the waveform, so the overshoot estimate can end up
        // slightly stale. A flat trim closes any remaining gap exactly, since
        // true peak scales linearly with gain.
        let measured = loudness::measure(audio)?;
        let excess_db = measured.true_peak_db() - settings.ceiling_dbtp;
        if excess_db > 0.0 {
            audio.scale(db_to_linear(-excess_db) as f32);
        }
    }

    let after = loudness::measure(audio)?;
    Ok(Report { before, after, gain_db, limiter_reduction_db, withheld_db, silent: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady tone with occasional transients on top, so peak level and
    /// loudness can be set independently — which is exactly the situation the
    /// two peak policies exist to resolve.
    fn spiky_music(tone_amplitude: f32, peak_amplitude: f32) -> Audio {
        let sample_rate = 48_000;
        let frames = sample_rate * 6;
        let mut plane: Vec<f32> = (0..frames)
            .map(|i| {
                tone_amplitude
                    * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin()
            })
            .collect();
        for beat in 0..12 {
            let at = beat * (frames / 12) + 100;
            plane[at] = peak_amplitude;
            plane[at + 1] = -peak_amplitude * 0.9;
        }
        Audio::new(sample_rate as u32, vec![plane.clone(), plane]).unwrap()
    }

    /// Quiet programme material carrying near-full-scale transients: it wants
    /// a large boost, but has no headroom at all to take one.
    fn quiet_but_peaky() -> Audio {
        spiky_music(0.05, 0.95)
    }

    /// Quiet material with transients only moderately above the programme
    /// level, the way most real music sits. Normalizing this needs a few dB of
    /// limiting rather than a dozen.
    fn quiet_with_moderate_transients() -> Audio {
        spiky_music(0.05, 0.30)
    }

    #[test]
    fn attenuate_policy_respects_the_ceiling() {
        let mut audio = quiet_but_peaky();
        let settings = Settings {
            target_lufs: -14.0,
            ceiling_dbtp: -1.0,
            peak_policy: PeakPolicy::Attenuate,
            ..Default::default()
        };

        let report = apply(&mut audio, &settings).unwrap();
        assert!(
            report.after.true_peak_db() <= -1.0 + 0.01,
            "true peak {} dBTP breached the ceiling",
            report.after.true_peak_db()
        );
        // This input cannot reach -14 LUFS without clipping, so gain is held
        // back and the shortfall is reported rather than hidden.
        assert!(report.withheld_db > 1.0, "withheld only {} dB", report.withheld_db);
    }

    #[test]
    fn attenuate_policy_hits_the_target_when_there_is_headroom() {
        let mut audio = spiky_music(0.02, 0.06);
        let settings = Settings { target_lufs: -20.0, ..Default::default() };

        let report = apply(&mut audio, &settings).unwrap();
        assert!(report.withheld_db.abs() < 1e-9);
        assert!(
            report.target_error_lu(-20.0).abs() < 0.1,
            "landed at {} LUFS",
            report.after.integrated_lufs
        );
    }

    #[test]
    fn limit_policy_hits_the_target() {
        let mut audio = quiet_with_moderate_transients();
        let settings = Settings {
            target_lufs: -14.0,
            ceiling_dbtp: -1.0,
            peak_policy: PeakPolicy::Limit,
            ..Default::default()
        };

        let report = apply(&mut audio, &settings).unwrap();
        assert!(report.limiter_reduction_db < 0.0, "the limiter never engaged");
        // Limiting only ducks the transients, so with a normal amount of it
        // the integrated loudness still lands on target. That is the whole
        // reason to prefer this policy over simply turning the track down.
        assert!(
            report.target_error_lu(-14.0).abs() < 0.3,
            "landed at {} LUFS",
            report.after.integrated_lufs
        );
    }

    #[test]
    fn limit_policy_holds_the_ceiling_even_under_heavy_limiting() {
        let mut audio = quiet_but_peaky();
        let settings = Settings {
            target_lufs: -14.0,
            ceiling_dbtp: -1.0,
            peak_policy: PeakPolicy::Limit,
            ..Default::default()
        };

        let report = apply(&mut audio, &settings).unwrap();
        assert!(
            report.after.true_peak_db() <= -1.0 + 0.01,
            "true peak {} dBTP breached the ceiling",
            report.after.true_peak_db()
        );
        // This input needs about 13 dB of gain reduction on every transient.
        // Ducking that hard costs real loudness — roughly 0.6 LU here — so the
        // result sits slightly under target. That is the honest outcome, not a
        // failure: the alternative is breaching the ceiling.
        let error = report.target_error_lu(-14.0);
        assert!(error < 0.0 && error > -1.5, "landed at {} LUFS", report.after.integrated_lufs);
    }

    #[test]
    fn the_two_policies_differ_where_it_matters() {
        // Same input, same target: limiting should land materially closer to
        // the target than attenuating, since attenuating gives up loudness to
        // protect the peaks.
        let settings = Settings {
            target_lufs: -14.0,
            ceiling_dbtp: -1.0,
            peak_policy: PeakPolicy::Attenuate,
            ..Default::default()
        };

        let mut attenuated = quiet_but_peaky();
        let quiet = apply(&mut attenuated, &settings).unwrap();

        let mut limited = quiet_but_peaky();
        let loud =
            apply(&mut limited, &Settings { peak_policy: PeakPolicy::Limit, ..settings }).unwrap();

        assert!(
            loud.after.integrated_lufs > quiet.after.integrated_lufs + 5.0,
            "limiting gained only {} LU over attenuating",
            loud.after.integrated_lufs - quiet.after.integrated_lufs
        );
    }

    #[test]
    fn turns_down_material_that_is_too_loud() {
        let mut audio = spiky_music(0.9, 0.99);
        let report = apply(&mut audio, &Settings::default()).unwrap();
        assert!(report.gain_db < 0.0, "gain was {} dB", report.gain_db);
    }

    #[test]
    fn leaves_silence_alone() {
        let mut audio = Audio::new(48_000, vec![vec![0.0; 96_000], vec![0.0; 96_000]]).unwrap();
        let before = audio.clone();
        let report = apply(&mut audio, &Settings::default()).unwrap();
        assert!(report.silent);
        assert_eq!(report.gain_db, 0.0);
        assert_eq!(audio, before);
    }
}
