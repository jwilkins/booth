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
    fn remix_adds_all_three() {
        let one = Audio::new(48_000, vec![vec![1.0, 1.0]]).unwrap();
        let set = StemSet { vocals: one.clone(), melody: one.clone(), drums: one };
        assert_eq!(set.remix().unwrap().planes[0], vec![3.0, 3.0]);
    }
}
