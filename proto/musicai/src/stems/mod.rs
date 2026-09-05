//! Splitting a mix into vocals, melody and drums.

pub mod demucs;
pub mod dsp;
pub mod install;

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, Result};

use crate::audio::Audio;

/// The three parts a mix gets split into.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, clap::ValueEnum)]
pub enum Stem {
    Vocals,
    Melody,
    Drums,
}

impl Stem {
    pub const ALL: [Stem; 3] = [Stem::Vocals, Stem::Melody, Stem::Drums];

    pub fn name(self) -> &'static str {
        match self {
            Stem::Vocals => "vocals",
            Stem::Melody => "melody",
            Stem::Drums => "drums",
        }
    }
}

impl fmt::Display for Stem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Stem {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vocals" | "vocal" | "voice" => Ok(Stem::Vocals),
            "melody" | "other" | "accompaniment" | "instrumental" => Ok(Stem::Melody),
            "drums" | "drum" | "percussion" => Ok(Stem::Drums),
            other => Err(anyhow!("unknown stem {other:?} (expected vocals, melody or drums)")),
        }
    }
}

/// A separated mix. The three stems share the sample rate, channel count and
/// length of the input.
#[derive(Clone, Debug)]
pub struct StemSet {
    pub vocals: Audio,
    pub melody: Audio,
    pub drums: Audio,
}

impl StemSet {
    pub fn get(&self, stem: Stem) -> &Audio {
        match stem {
            Stem::Vocals => &self.vocals,
            Stem::Melody => &self.melody,
            Stem::Drums => &self.drums,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (Stem, &Audio)> {
        Stem::ALL.into_iter().map(|s| (s, self.get(s)))
    }

    /// Bring the stems under a peak ceiling without clipping, by one shared
    /// gain rather than one per stem.
    ///
    /// A separated stem can peak above full scale even when the mix it came
    /// from did not — the split redistributes energy — so writing the stems
    /// straight to an integer format clips them. Attenuating each stem on its
    /// own would fix that but pull them out of balance with each other, and the
    /// point of a set of stems is that they still add back up to the track. One
    /// shared gain keeps that relationship: after it, the loudest sample across
    /// all the stems sits at `ceiling`, and nothing clips.
    ///
    /// Returns the gain applied, in dB, or `None` when nothing needed doing.
    /// `ceiling` is a linear amplitude, e.g. 0.99 for a hair under full scale.
    pub fn fit_under(&mut self, ceiling: f32) -> Option<f32> {
        let peak = self.iter().map(|(_, audio)| audio.sample_peak()).fold(0.0f32, f32::max);
        if peak <= ceiling || peak <= 0.0 {
            return None;
        }
        let gain = ceiling / peak;
        self.vocals.scale(gain);
        self.melody.scale(gain);
        self.drums.scale(gain);
        Some(20.0 * gain.log10())
    }

    /// Sum the three stems back together. Used by the tests to confirm the
    /// separation is conservative — that it redistributes the mix rather than
    /// inventing or losing energy.
    pub fn remix(&self) -> Result<Audio> {
        let mut out = self.vocals.clone();
        out.add_assign(&self.melody)?;
        out.add_assign(&self.drums)?;
        Ok(out)
    }
}

/// Which separator to run.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Backend {
    /// Shell out to a locally installed `demucs`. The default: it is the only
    /// one of the two that genuinely isolates a voice.
    Demucs,
    /// The built-in signal-processing separator. Needs nothing installed and
    /// runs offline, but leaks vocals into every stem — see the module docs
    /// for what it can and cannot do.
    Dsp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stem_aliases() {
        assert_eq!("VOICE".parse::<Stem>().unwrap(), Stem::Vocals);
        assert_eq!("other".parse::<Stem>().unwrap(), Stem::Melody);
        assert_eq!("percussion".parse::<Stem>().unwrap(), Stem::Drums);
        assert!("bass".parse::<Stem>().is_err());
    }

    #[test]
    fn fit_under_pulls_a_clipping_stem_below_the_ceiling() {
        // One stem peaks well above full scale, as a separated stem can.
        let loud = Audio::new(48_000, vec![vec![1.6, -1.4]]).unwrap();
        let quiet = Audio::new(48_000, vec![vec![0.2, -0.1]]).unwrap();
        let mut set = StemSet { vocals: loud, melody: quiet.clone(), drums: quiet };

        let gain_db = set.fit_under(0.98).expect("a clipping set should be attenuated");
        assert!(gain_db < 0.0, "attenuation should be negative dB, got {gain_db}");
        // Nothing now exceeds the ceiling.
        for (_, audio) in set.iter() {
            assert!(audio.sample_peak() <= 0.981, "{} still clips", audio.sample_peak());
        }
    }

    #[test]
    fn fit_under_keeps_the_stems_in_balance() {
        // The ratio between two stems must survive the shared gain, so a set
        // still adds back up to the track.
        let a = Audio::new(48_000, vec![vec![1.5]]).unwrap();
        let b = Audio::new(48_000, vec![vec![0.3]]).unwrap();
        let mut set =
            StemSet { vocals: a, melody: b, drums: Audio::new(48_000, vec![vec![0.0]]).unwrap() };
        set.fit_under(0.98);
        let ratio = set.vocals.planes[0][0] / set.melody.planes[0][0];
        assert!((ratio - 5.0).abs() < 1e-4, "the 5:1 ratio changed to {ratio}");
    }

    #[test]
    fn fit_under_leaves_a_quiet_set_alone() {
        let quiet = Audio::new(48_000, vec![vec![0.5, -0.4]]).unwrap();
        let mut set = StemSet { vocals: quiet.clone(), melody: quiet.clone(), drums: quiet };
        assert!(set.fit_under(0.98).is_none(), "a set under the ceiling should not be touched");
    }

    #[test]
    fn remix_adds_all_three() {
        let one = Audio::new(48_000, vec![vec![1.0, 1.0]]).unwrap();
        let set = StemSet { vocals: one.clone(), melody: one.clone(), drums: one };
        assert_eq!(set.remix().unwrap().planes[0], vec![3.0, 3.0]);
    }
}
