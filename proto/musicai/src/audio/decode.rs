//! Decoding a file into an [`Audio`] buffer: Symphonia for everything it can,
//! rusty-opus for the Opus inside an Ogg it demuxes, and ffmpeg for WMA.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::FormatReader;
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;

use super::Audio;

/// Decode an entire audio file into memory.
///
/// The container is identified by content, with the file extension supplied
/// only as a hint, so a mislabelled file still decodes correctly.
pub fn decode_file(path: &Path) -> Result<Audio> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    if super::ffmpeg::needed_for(extension) {
        return super::ffmpeg::decode(path);
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .with_context(|| format!("identifying audio format of {}", path.display()))?;

    // `default_track` borrows `format`, so pull out everything we need and let
    // the borrow end before we start pulling packets.
    let (track_id, audio_params, delay) = {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| anyhow!("{} contains no audio track", path.display()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| {
                anyhow!("{} has an audio track with no codec parameters", path.display())
            })?
            .clone();
        (track.id, params, track.delay.unwrap_or(0))
    };

    if audio_params.codec == CODEC_ID_OPUS {
        return decode_opus(&mut format, track_id, &audio_params, delay, path);
    }

    let sample_rate = audio_params
        .sample_rate
        .ok_or_else(|| anyhow!("{} does not declare a sample rate", path.display()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .with_context(|| format!("no decoder available for {}", path.display()))?;

    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A truncated final frame is common in the wild; keep what we have.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };

        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(buf) => {
                let channels = buf.spec().channels().count();
                if channels == 0 {
                    continue;
                }
                if planes.is_empty() {
                    planes = vec![Vec::new(); channels];
                } else if planes.len() != channels {
                    bail!(
                        "{} changes channel count mid-stream ({} then {})",
                        path.display(),
                        planes.len(),
                        channels
                    );
                }

                interleaved.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut interleaved);
                for (i, &sample) in interleaved.iter().enumerate() {
                    planes[i % channels].push(sample);
                }
            }
            // Decode errors are recoverable per the Symphonia contract: skip
            // the bad packet and carry on with the next one.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e).with_context(|| format!("decoding {}", path.display())),
        }
    }

    if planes.is_empty() {
        bail!("{} decoded to zero channels", path.display());
    }

    // Guard against a partially written trailing frame leaving planes ragged.
    let frames = planes.iter().map(|p| p.len()).min().unwrap_or(0);
    for plane in &mut planes {
        plane.truncate(frames);
    }

    Audio::new(sample_rate, planes)
}

/// The longest a single Opus packet decodes to: 120 ms at 48 kHz.
const OPUS_MAX_FRAMES: usize = 5_760;

/// Opus, which Symphonia demuxes and cannot decode.
///
/// Always 48 kHz, whatever rate the file was made from: that is the only rate
/// Opus decodes to at full bandwidth, and the header's "input sample rate" is a
/// note about the source rather than an instruction.
///
/// The encoder's delay comes off the front. The container names it in the
/// header as the pre-skip, 312 samples from libopus, and Symphonia hands the
/// first packet over without trimming it: decoded as it comes, every cue and
/// beat on an Opus file lands 6.5 ms late, measured against libopus on a CC
/// record. The padding at the end Symphonia trims itself.
fn decode_opus(
    format: &mut Box<dyn FormatReader>,
    track_id: u32,
    params: &AudioCodecParameters,
    delay: u32,
    path: &Path,
) -> Result<Audio> {
    let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(0);
    if !(1..=2).contains(&channels) {
        bail!(
            "{} is Opus with {channels} channels, and only mono and stereo are decoded here",
            path.display()
        );
    }
    let mut decoder = rusty_opus::OpusDecoder::new(48_000, channels)
        .map_err(|e| anyhow!("starting an Opus decoder for {}: {e:?}", path.display()))?;
    let mut planes: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut pcm = vec![0f32; OPUS_MAX_FRAMES * channels];
    let mut trimmed = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if packet.track_id != track_id {
            continue;
        }
        // A packet that will not decode is skipped, as Symphonia's own
        // decoders do with theirs: one bad packet is 20 ms, not a lost track.
        let Ok(frames) = decoder.decode(&packet.data, OPUS_MAX_FRAMES, &mut pcm) else {
            continue;
        };
        let start = (packet.trim_start.get() as usize).min(frames);
        let end = frames.saturating_sub(packet.trim_end.get() as usize).max(start);
        trimmed += start;
        for frame in start..end {
            for (ch, plane) in planes.iter_mut().enumerate() {
                plane.push(pcm[frame * channels + ch]);
            }
        }
    }
    let skip = (delay as usize).saturating_sub(trimmed);
    for plane in &mut planes {
        plane.drain(..skip.min(plane.len()));
    }
    if planes[0].is_empty() {
        bail!("{} decoded to no audio", path.display());
    }
    Audio::new(48_000, planes)
}
