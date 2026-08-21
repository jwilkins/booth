//! Audio buffers and file I/O.

pub mod decode;
pub mod encode;
pub mod mp3;

use anyhow::{bail, Result};

/// A block of PCM audio held as de-interleaved (planar) `f32` samples in the
/// range `[-1.0, 1.0]`.
///
/// Planar layout is what almost everything downstream wants: the loudness
/// meter, the STFT, and the stem separator all work one channel at a time.
#[derive(Clone, Debug, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    /// One `Vec<f32>` per channel; every plane has the same length.
    pub planes: Vec<Vec<f32>>,
}

impl Audio {
    pub fn new(sample_rate: u32, planes: Vec<Vec<f32>>) -> Result<Self> {
        if planes.is_empty() {
            bail!("audio must have at least one channel");
        }
        let len = planes[0].len();
        if planes.iter().any(|p| p.len() != len) {
            bail!("all channel planes must be the same length");
        }
        if sample_rate == 0 {
            bail!("sample rate must be non-zero");
        }
        Ok(Self { sample_rate, planes })
    }

    /// An all-zero buffer with the same shape as `self`.
    pub fn silence_like(&self) -> Self {
        Self {
            sample_rate: self.sample_rate,
            planes: vec![vec![0.0; self.frames()]; self.channels()],
        }
    }

    pub fn channels(&self) -> usize {
        self.planes.len()
    }

    pub fn frames(&self) -> usize {
        self.planes.first().map_or(0, |p| p.len())
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }

    pub fn is_empty(&self) -> bool {
        self.frames() == 0
    }

    /// Interleave the planes into a single `[L, R, L, R, ...]` vector, the
    /// layout every encoder we use wants.
    pub fn to_interleaved(&self) -> Vec<f32> {
        let channels = self.channels();
        let frames = self.frames();
        let mut out = vec![0.0; channels * frames];
        for (c, plane) in self.planes.iter().enumerate() {
            for (i, &s) in plane.iter().enumerate() {
                out[i * channels + c] = s;
            }
        }
        out
    }

    /// Largest absolute sample value across all channels.
    pub fn sample_peak(&self) -> f32 {
        self.planes.iter().flat_map(|p| p.iter()).fold(0.0f32, |acc, &s| acc.max(s.abs()))
    }

    /// Multiply every sample by `factor`.
    pub fn scale(&mut self, factor: f32) {
        for plane in &mut self.planes {
            for s in plane.iter_mut() {
                *s *= factor;
            }
        }
    }

    /// Sum `other` into `self` sample-by-sample. Used to fold demucs' four
    /// stems down into our three.
    pub fn add_assign(&mut self, other: &Audio) -> Result<()> {
        if self.sample_rate != other.sample_rate || self.channels() != other.channels() {
            bail!("cannot mix audio with differing sample rate or channel count");
        }
        let frames = self.frames().max(other.frames());
        for (dst, src) in self.planes.iter_mut().zip(&other.planes) {
            dst.resize(frames, 0.0);
            for (d, &s) in dst.iter_mut().zip(src) {
                *d += s;
            }
        }
        Ok(())
    }

    /// Collapse to mono by averaging channels. Only used for analysis helpers.
    pub fn to_mono(&self) -> Vec<f32> {
        let channels = self.channels() as f32;
        let mut out = vec![0.0; self.frames()];
        for plane in &self.planes {
            for (o, &s) in out.iter_mut().zip(plane) {
                *o += s;
            }
        }
        for o in &mut out {
            *o /= channels;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_planes() {
        let a = Audio::new(48_000, vec![vec![1.0, 3.0], vec![2.0, 4.0]]).unwrap();
        assert_eq!(a.to_interleaved(), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn rejects_ragged_planes() {
        assert!(Audio::new(48_000, vec![vec![1.0, 2.0], vec![1.0]]).is_err());
    }

    #[test]
    fn reports_peak_and_duration() {
        let a = Audio::new(10, vec![vec![0.0, -0.75, 0.5]]).unwrap();
        assert_eq!(a.sample_peak(), 0.75);
        assert!((a.duration_secs() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn mixes_down_to_mono() {
        let a = Audio::new(48_000, vec![vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        assert_eq!(a.to_mono(), vec![0.5, 0.5]);
    }
}
