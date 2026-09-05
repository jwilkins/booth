//! Chromaprint acoustic fingerprints.
//!
//! A fingerprint is a compact summary of what a recording actually sounds
//! like, so the same song identifies correctly regardless of how it was
//! encoded, tagged or named. This is the only part of tagging that happens
//! locally; turning a fingerprint into metadata needs the AcoustID service.
//!
//! The output is byte-compatible with the reference `fpcalc` tool: the same
//! algorithm (Chromaprint's TEST2, which is its default), the same 120-second
//! limit, and the same compressed URL-safe base64 encoding AcoustID expects.

use anyhow::{Context, Result};
use base64::Engine;
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};

use crate::audio::Audio;

/// How much of a track is fingerprinted, in seconds.
///
/// `fpcalc` uses the first two minutes and AcoustID's index is built from
/// fingerprints of that length, so matching the limit matters: fingerprinting
/// a whole eight-minute track would produce something the index has never
/// seen.
pub const FINGERPRINT_SECONDS: usize = 120;

/// A fingerprint plus the duration AcoustID needs alongside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// Compressed, URL-safe base64, exactly as `fpcalc` prints it.
    pub compressed: String,
    /// Duration of the *whole* track in seconds, not of the fingerprinted
    /// portion. AcoustID uses it to narrow candidates, so it has to describe
    /// the recording rather than the sample we took.
    pub duration_secs: u32,
}

/// Compute the fingerprint of `audio`.
pub fn fingerprint(audio: &Audio) -> Result<Fingerprint> {
    if audio.is_empty() {
        anyhow::bail!("cannot fingerprint an empty audio file");
    }

    let config = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&config);

    let channels = u32::try_from(audio.channels()).context("too many channels to fingerprint")?;
    printer.start(audio.sample_rate, channels).map_err(|e| {
        anyhow::anyhow!("chromaprint rejected {} Hz / {channels}ch: {e:?}", audio.sample_rate)
    })?;

    printer.consume(&interleaved_i16(audio, FINGERPRINT_SECONDS));
    printer.finish();

    let raw = printer.fingerprint();
    if raw.is_empty() {
        anyhow::bail!("audio was too short to fingerprint ({:.1}s)", audio.duration_secs());
    }

    let compressed = FingerprintCompressor::from(&config).compress(raw);

    Ok(Fingerprint {
        // AcoustID wants URL-safe base64 with no padding.
        compressed: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed),
        duration_secs: audio.duration_secs().round() as u32,
    })
}

/// Interleave the first `seconds` of `audio` as 16-bit samples, which is what
/// the fingerprinter consumes.
fn interleaved_i16(audio: &Audio, seconds: usize) -> Vec<i16> {
    let limit = (audio.sample_rate as usize).saturating_mul(seconds).min(audio.frames());
    let channels = audio.channels();

    let mut out = vec![0i16; limit * channels];
    for (c, plane) in audio.planes.iter().enumerate() {
        for (i, &sample) in plane[..limit].iter().enumerate() {
            // Clamp rather than wrap: a sample over full scale must not come
            // back as a large negative one and corrupt the fingerprint.
            let scaled = (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0);
            out[i * channels + c] = scaled as i16;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something with enough spectral variation to fingerprint: a chord
    /// sequence rather than a single tone.
    fn music(seconds: usize, transpose: f32) -> Audio {
        let sample_rate = 44_100;
        let frames = sample_rate * seconds;
        let mut plane = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            // Change chord every second.
            let step = (t as usize % 4) as f32;
            let root = 220.0 * transpose * (1.0 + 0.06 * step);
            let mut v = 0.0;
            for h in [1.0, 1.26, 1.5, 2.0] {
                v += 0.2 * (2.0 * std::f32::consts::PI * root * h * t).sin();
            }
            plane.push(v * 0.5);
        }
        Audio::new(sample_rate as u32, vec![plane.clone(), plane]).unwrap()
    }

    #[test]
    fn produces_a_url_safe_base64_fingerprint() {
        let fp = fingerprint(&music(15, 1.0)).unwrap();
        assert!(!fp.compressed.is_empty());
        assert_eq!(fp.duration_secs, 15);
        // URL-safe alphabet, no padding — anything else and AcoustID rejects it.
        assert!(
            fp.compressed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "unexpected characters in {}",
            fp.compressed
        );
    }

    #[test]
    fn is_deterministic() {
        let a = fingerprint(&music(12, 1.0)).unwrap();
        let b = fingerprint(&music(12, 1.0)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn differs_for_different_audio() {
        let a = fingerprint(&music(12, 1.0)).unwrap();
        let b = fingerprint(&music(12, 1.5)).unwrap();
        assert_ne!(a.compressed, b.compressed);
    }

    #[test]
    fn reports_the_whole_duration_but_only_fingerprints_the_limit() {
        // A track longer than the limit still reports its true duration, and
        // its fingerprint matches that of its own first two minutes.
        let long = music(FINGERPRINT_SECONDS + 30, 1.0);
        let clipped = music(FINGERPRINT_SECONDS, 1.0);

        let a = fingerprint(&long).unwrap();
        let b = fingerprint(&clipped).unwrap();

        assert_eq!(a.duration_secs as usize, FINGERPRINT_SECONDS + 30);
        assert_eq!(b.duration_secs as usize, FINGERPRINT_SECONDS);
        assert_eq!(a.compressed, b.compressed, "the 120s limit was not applied");
    }

    #[test]
    fn conversion_clamps_instead_of_wrapping() {
        let audio = Audio::new(44_100, vec![vec![2.0, -2.0, 0.0]]).unwrap();
        assert_eq!(interleaved_i16(&audio, 1), vec![32_767, -32_768, 0]);
    }

    #[test]
    fn conversion_interleaves_channels() {
        let audio = Audio::new(44_100, vec![vec![1.0, 0.0], vec![0.0, -1.0]]).unwrap();
        assert_eq!(interleaved_i16(&audio, 1), vec![32_767, 0, 0, -32_768]);
    }

    #[test]
    fn rejects_audio_too_short_to_fingerprint() {
        let tiny = Audio::new(44_100, vec![vec![0.0; 100]]).unwrap();
        let err = fingerprint(&tiny).unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn rejects_empty_audio() {
        let empty = Audio::new(44_100, vec![vec![]]).unwrap();
        assert!(fingerprint(&empty).is_err());
    }
}
