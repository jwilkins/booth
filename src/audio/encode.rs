//! Writing [`Audio`] back out as wav, flac or mp3.

use std::path::Path;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use flacenc::component::BitRepr;
use flacenc::error::Verify;

use super::Audio;

/// Container/codec to write.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Codec {
    Wav,
    Flac,
    Mp3,
}

impl Codec {
    pub fn extension(self) -> &'static str {
        match self {
            Codec::Wav => "wav",
            Codec::Flac => "flac",
            Codec::Mp3 => "mp3",
        }
    }

    /// Identify a codec from a file extension, for `--format same` and for
    /// deciding which tag writer ReplayGain should use.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::from_str(&ext).ok()
    }

    pub fn is_lossless(self) -> bool {
        !matches!(self, Codec::Mp3)
    }
}

impl FromStr for Codec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "wav" | "wave" => Ok(Codec::Wav),
            "flac" => Ok(Codec::Flac),
            "mp3" => Ok(Codec::Mp3),
            other => Err(anyhow!("unsupported audio format: {other}")),
        }
    }
}

/// Encoder settings shared by every output path.
#[derive(Copy, Clone, Debug)]
pub struct EncodeOptions {
    /// Bit depth for wav/flac output. Ignored for mp3.
    pub bit_depth: u16,
    /// Constant bitrate in kbps for mp3 output. Ignored otherwise.
    pub mp3_bitrate: u32,
    /// Apply TPDF dither when truncating to a 16-bit integer output.
    pub dither: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { bit_depth: 16, mp3_bitrate: 192, dither: true }
    }
}

/// Encode `audio` to `path`, choosing the encoder from `codec`.
pub fn write_file(path: &Path, audio: &Audio, codec: Codec, opts: &EncodeOptions) -> Result<()> {
    if audio.is_empty() {
        bail!("refusing to write an empty audio file to {}", path.display());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
    }

    match codec {
        Codec::Wav => write_wav(path, audio, opts),
        Codec::Flac => write_flac(path, audio, opts),
        Codec::Mp3 => write_mp3(path, audio, opts),
    }
    .with_context(|| format!("writing {}", path.display()))
}

fn check_bit_depth(bit_depth: u16) -> Result<()> {
    if !matches!(bit_depth, 16 | 24) {
        bail!("bit depth must be 16 or 24, got {bit_depth}");
    }
    Ok(())
}

fn write_wav(path: &Path, audio: &Audio, opts: &EncodeOptions) -> Result<()> {
    check_bit_depth(opts.bit_depth)?;
    let spec = hound::WavSpec {
        channels: u16::try_from(audio.channels()).context("too many channels for wav")?,
        sample_rate: audio.sample_rate,
        bits_per_sample: opts.bit_depth,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;
    let samples = quantize(&audio.to_interleaved(), opts.bit_depth, opts.dither);
    for sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}

fn write_flac(path: &Path, audio: &Audio, opts: &EncodeOptions) -> Result<()> {
    check_bit_depth(opts.bit_depth)?;
    let samples = quantize(&audio.to_interleaved(), opts.bit_depth, opts.dither);

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| anyhow!("invalid flac encoder config: {e}"))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        audio.channels(),
        opts.bit_depth as usize,
        audio.sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| anyhow!("flac encoding failed: {e}"))?;

    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| anyhow!("serialising flac stream: {e}"))?;

    let mut bytes = sink.as_slice().to_vec();
    mark_fixed_block_size(&mut bytes)?;
    std::fs::write(path, &bytes)?;
    Ok(())
}

/// Make STREAMINFO's minimum block size equal its maximum.
///
/// We always encode with a constant block size, but the final block of a
/// stream is whatever samples are left over and is usually shorter. `flacenc`
/// reports that short final block as the stream minimum, which leaves
/// `min_blocksize != max_blocksize`.
///
/// Decoders read those two fields being equal as *the* signal that a stream
/// uses the fixed blocking strategy, and then require every frame to carry a
/// frame number rather than a sample number. Our frames do carry frame numbers,
/// so a decoder that saw unequal sizes would reject all of them and conclude
/// the file has no audio. libFLAC writes them equal for exactly this reason —
/// the final block is excluded from these fields — so we do the same.
fn mark_fixed_block_size(bytes: &mut [u8]) -> Result<()> {
    // "fLaC", then a 4-byte metadata block header, then STREAMINFO itself,
    // whose first two 16-bit fields are the minimum and maximum block size.
    const STREAMINFO: usize = 8;

    if bytes.len() < STREAMINFO + 4 || &bytes[0..4] != b"fLaC" {
        bail!("flac encoder did not produce a flac stream");
    }
    // Low 7 bits of the first block header are the block type; 0 is STREAMINFO.
    if bytes[4] & 0x7f != 0 {
        bail!("flac stream does not start with a stream info block");
    }

    let (min, max) = bytes[STREAMINFO..STREAMINFO + 4].split_at_mut(2);
    min.copy_from_slice(max);
    Ok(())
}

fn write_mp3(path: &Path, audio: &Audio, opts: &EncodeOptions) -> Result<()> {
    use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm};

    let channels = audio.channels();
    if channels > 2 {
        bail!("mp3 output supports at most 2 channels, got {channels}");
    }

    let mut builder = Builder::new().ok_or_else(|| anyhow!("could not create a LAME encoder"))?;
    builder
        .set_num_channels(channels as u8)
        .map_err(|e| anyhow!("LAME rejected channel count: {e}"))?;
    builder
        .set_sample_rate(audio.sample_rate)
        .map_err(|e| anyhow!("LAME rejected sample rate {}: {e}", audio.sample_rate))?;
    builder
        .set_brate(mp3_bitrate(opts.mp3_bitrate)?)
        .map_err(|e| anyhow!("LAME rejected bitrate: {e}"))?;
    builder
        .set_quality(mp3lame_encoder::Quality::Best)
        .map_err(|e| anyhow!("LAME rejected quality setting: {e}"))?;
    let mut encoder = builder.build().map_err(|e| anyhow!("initialising LAME: {e}"))?;

    // LAME wants 16-bit PCM; dither on the way down as with any other
    // truncation to 16 bits.
    let pcm: Vec<i16> =
        quantize(&audio.to_interleaved(), 16, opts.dither).into_iter().map(|s| s as i16).collect();

    let mut out: Vec<u8> = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(pcm.len()));
    encoder
        .encode_to_vec(InterleavedPcm(pcm.as_slice()), &mut out)
        .map_err(|e| anyhow!("mp3 encoding failed: {e}"))?;
    encoder
        .flush_to_vec::<FlushNoGap>(&mut out)
        .map_err(|e| anyhow!("flushing mp3 encoder: {e}"))?;

    std::fs::write(path, &out)?;
    Ok(())
}

fn mp3_bitrate(kbps: u32) -> Result<mp3lame_encoder::Bitrate> {
    use mp3lame_encoder::Bitrate::*;
    Ok(match kbps {
        8 => Kbps8,
        16 => Kbps16,
        24 => Kbps24,
        32 => Kbps32,
        40 => Kbps40,
        48 => Kbps48,
        64 => Kbps64,
        80 => Kbps80,
        96 => Kbps96,
        112 => Kbps112,
        128 => Kbps128,
        160 => Kbps160,
        192 => Kbps192,
        224 => Kbps224,
        256 => Kbps256,
        320 => Kbps320,
        other => bail!(
            "unsupported mp3 bitrate {other} kbps \
             (pick one of 8/16/24/32/40/48/64/80/96/112/128/160/192/224/256/320)"
        ),
    })
}

/// Convert float samples to integers of the requested depth.
///
/// Samples are hard-clipped to the representable range first: normalization
/// should have kept us inside it, but a caller that skipped limiting must not
/// get wraparound distortion. TPDF dither is added at 16 bits, where the
/// quantization floor is audible.
fn quantize(samples: &[f32], bit_depth: u16, dither: bool) -> Vec<i32> {
    let max = ((1i64 << (bit_depth - 1)) - 1) as f32;
    let min = -(1i64 << (bit_depth - 1)) as f32;
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let apply_dither = dither && bit_depth == 16;

    samples
        .iter()
        .map(|&s| {
            let mut scaled = s * (max + 1.0);
            if apply_dither {
                // Triangular PDF spanning +/-1 LSB, from two uniform draws.
                scaled += rng.next_uniform() - rng.next_uniform();
            }
            scaled.round().clamp(min, max) as i32
        })
        .collect()
}

/// xorshift64*, enough for dither noise and keeps the dependency list short.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_uniform(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (bits >> 40) as f32 / (1u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_codec_from_extension() {
        assert_eq!(Codec::from_path(Path::new("a/b.FLAC")), Some(Codec::Flac));
        assert_eq!(Codec::from_path(Path::new("a/b.mp3")), Some(Codec::Mp3));
        assert_eq!(Codec::from_path(Path::new("a/b.ogg")), None);
    }

    #[test]
    fn quantize_clips_instead_of_wrapping() {
        let out = quantize(&[2.0, -2.0], 16, false);
        assert_eq!(out, vec![32767, -32768]);
    }

    #[test]
    fn quantize_scales_full_range() {
        let out = quantize(&[1.0, 0.0, -1.0], 16, false);
        assert_eq!(out, vec![32767, 0, -32768]);
    }

    #[test]
    fn dither_stays_within_one_lsb() {
        let flat = vec![0.0f32; 4096];
        for s in quantize(&flat, 16, true) {
            assert!(s.abs() <= 1, "dither pushed a silent sample to {s}");
        }
    }

    #[test]
    fn rejects_odd_bit_depths() {
        assert!(check_bit_depth(20).is_err());
        assert!(check_bit_depth(24).is_ok());
    }

    #[test]
    fn equalises_the_stream_info_block_sizes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"fLaC");
        bytes.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]); // last block, type 0, len 34
        bytes.extend_from_slice(&[0x0c, 0x44]); // min 3140, the short final block
        bytes.extend_from_slice(&[0x10, 0x00]); // max 4096
        bytes.extend_from_slice(&[0u8; 30]);

        mark_fixed_block_size(&mut bytes).unwrap();
        assert_eq!(&bytes[8..10], &[0x10, 0x00], "minimum was not raised to the maximum");
        assert_eq!(&bytes[10..12], &[0x10, 0x00], "maximum was modified");
    }

    #[test]
    fn rejects_output_that_is_not_a_flac_stream() {
        assert!(mark_fixed_block_size(&mut [0u8; 4]).is_err());

        let mut wrong_first_block = Vec::from(*b"fLaC");
        wrong_first_block.extend_from_slice(&[0x04, 0x00, 0x00, 0x22]); // type 4, not 0
        wrong_first_block.extend_from_slice(&[0u8; 34]);
        assert!(mark_fixed_block_size(&mut wrong_first_block).is_err());
    }
}
