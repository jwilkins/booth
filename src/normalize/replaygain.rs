//! ReplayGain 2.0 tagging.
//!
//! Nothing here touches the audio: we measure loudness, work out the gain a
//! player should apply, and write it into the file's tags. The audio data is
//! left bit-for-bit identical, which is the whole point of this mode for
//! lossy sources.
//!
//! Tag names and value formats follow the ReplayGain 2.0 specification as
//! implemented by `loudgain`, so the results are readable by the usual players.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::audio::encode::Codec;
use crate::loudness::Loudness;

/// The gain and peak figures for one file, plus the album figures if album
/// mode was requested.
#[derive(Copy, Clone, Debug)]
pub struct ReplayGain {
    pub reference_lufs: f64,
    pub track_gain_db: f64,
    pub track_peak: f64,
    pub track_range_lu: f64,
    pub album_gain_db: Option<f64>,
    pub album_peak: Option<f64>,
}

impl ReplayGain {
    /// Compute track figures from a measurement.
    ///
    /// Peak is reported as a linear sample-peak ratio, per the specification;
    /// values above 1.0 are legal and tell a player how much it must attenuate
    /// to avoid clipping.
    pub fn for_track(loudness: &Loudness, reference_lufs: f64) -> Self {
        Self {
            reference_lufs,
            track_gain_db: gain_for(loudness.integrated_lufs, reference_lufs),
            track_peak: loudness.sample_peak,
            track_range_lu: loudness.range_lu,
            album_gain_db: None,
            album_peak: None,
        }
    }

    /// Attach album figures computed across every track in the album.
    pub fn with_album(mut self, album_lufs: f64, album_peak: f64) -> Self {
        self.album_gain_db = Some(gain_for(album_lufs, self.reference_lufs));
        self.album_peak = Some(album_peak);
        self
    }
}

/// Gain a player must apply to bring `measured` up (or down) to `reference`.
/// Silence gets 0 dB rather than an infinite boost.
fn gain_for(measured_lufs: f64, reference_lufs: f64) -> f64 {
    if measured_lufs.is_finite() {
        reference_lufs - measured_lufs
    } else {
        0.0
    }
}

const KEY_TRACK_GAIN: &str = "REPLAYGAIN_TRACK_GAIN";
const KEY_TRACK_PEAK: &str = "REPLAYGAIN_TRACK_PEAK";
const KEY_TRACK_RANGE: &str = "REPLAYGAIN_TRACK_RANGE";
const KEY_ALBUM_GAIN: &str = "REPLAYGAIN_ALBUM_GAIN";
const KEY_ALBUM_PEAK: &str = "REPLAYGAIN_ALBUM_PEAK";
const KEY_REFERENCE: &str = "REPLAYGAIN_REFERENCE_LOUDNESS";

/// The tag key/value pairs this measurement produces, in write order.
fn tag_pairs(rg: &ReplayGain) -> Vec<(&'static str, String)> {
    let mut pairs = vec![
        (KEY_TRACK_GAIN, format_gain(rg.track_gain_db)),
        (KEY_TRACK_PEAK, format_peak(rg.track_peak)),
        (KEY_TRACK_RANGE, format_gain(rg.track_range_lu)),
        (KEY_REFERENCE, format!("{:.2} LUFS", rg.reference_lufs)),
    ];
    if let (Some(gain), Some(peak)) = (rg.album_gain_db, rg.album_peak) {
        pairs.push((KEY_ALBUM_GAIN, format_gain(gain)));
        pairs.push((KEY_ALBUM_PEAK, format_peak(peak)));
    }
    pairs
}

fn format_gain(db: f64) -> String {
    format!("{db:.2} dB")
}

fn format_peak(linear: f64) -> String {
    format!("{linear:.6}")
}

/// Write ReplayGain tags into an existing file, leaving its audio untouched.
pub fn write_tags(path: &Path, rg: &ReplayGain) -> Result<()> {
    match Codec::from_path(path) {
        Some(Codec::Mp3) => write_id3(path, rg),
        Some(Codec::Flac) => write_vorbis_comment(path, rg),
        Some(Codec::Wav) => {
            bail!("wav has no standard ReplayGain tag; use --mode reencode for {}", path.display())
        }
        None => bail!("cannot tell what kind of file {} is", path.display()),
    }
    .with_context(|| format!("writing ReplayGain tags to {}", path.display()))
}

fn write_id3(path: &Path, rg: &ReplayGain) -> Result<()> {
    use id3::frame::ExtendedText;
    use id3::{Tag, TagLike, Version};

    // A file with no ID3 tag yet is normal, not an error.
    let mut tag = Tag::read_from_path(path).unwrap_or_default();

    for (key, value) in tag_pairs(rg) {
        tag.remove_extended_text(Some(key), None);
        tag.add_frame(ExtendedText { description: key.to_string(), value });
    }

    tag.write_to_path(path, Version::Id3v24)?;
    Ok(())
}

fn write_vorbis_comment(path: &Path, rg: &ReplayGain) -> Result<()> {
    let mut tag = metaflac::Tag::read_from_path(path)?;
    {
        let comments = tag.vorbis_comments_mut();
        for (key, value) in tag_pairs(rg) {
            comments.set(key, vec![value]);
        }
    }
    tag.save()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loudness(lufs: f64, peak: f64) -> Loudness {
        Loudness { integrated_lufs: lufs, range_lu: 7.5, true_peak: peak, sample_peak: peak }
    }

    #[test]
    fn quiet_tracks_get_positive_gain() {
        let rg = ReplayGain::for_track(&loudness(-23.0, 0.5), -18.0);
        assert!((rg.track_gain_db - 5.0).abs() < 1e-9);
    }

    #[test]
    fn loud_tracks_get_negative_gain() {
        let rg = ReplayGain::for_track(&loudness(-8.0, 1.0), -18.0);
        assert!((rg.track_gain_db - -10.0).abs() < 1e-9);
    }

    #[test]
    fn silence_gets_no_gain_rather_than_infinity() {
        let rg = ReplayGain::for_track(&loudness(f64::NEG_INFINITY, 0.0), -18.0);
        assert_eq!(rg.track_gain_db, 0.0);
    }

    #[test]
    fn formats_values_the_way_the_spec_does() {
        let rg = ReplayGain::for_track(&loudness(-23.0, 0.987654321), -18.0);
        let pairs = tag_pairs(&rg);
        assert_eq!(pairs[0], (KEY_TRACK_GAIN, "5.00 dB".to_string()));
        assert_eq!(pairs[1], (KEY_TRACK_PEAK, "0.987654".to_string()));
        assert_eq!(pairs[3], (KEY_REFERENCE, "-18.00 LUFS".to_string()));
    }

    #[test]
    fn album_tags_appear_only_in_album_mode() {
        let track = ReplayGain::for_track(&loudness(-23.0, 0.5), -18.0);
        assert_eq!(tag_pairs(&track).len(), 4);

        let album = track.with_album(-20.0, 0.9);
        let pairs = tag_pairs(&album);
        assert_eq!(pairs.len(), 6);
        assert_eq!(pairs[4], (KEY_ALBUM_GAIN, "2.00 dB".to_string()));
    }

    #[test]
    fn refuses_wav() {
        let rg = ReplayGain::for_track(&loudness(-23.0, 0.5), -18.0);
        let err = write_tags(Path::new("x.wav"), &rg).unwrap_err();
        assert!(err.to_string().contains("no standard ReplayGain tag"));
    }
}
