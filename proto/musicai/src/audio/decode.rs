//! Decoding mp3/flac/wav into an [`Audio`] buffer via Symphonia.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::TrackType;
use symphonia::core::io::MediaSourceStream;

use super::Audio;

/// Decode an entire audio file into memory.
///
/// The container is identified by content, with the file extension supplied
/// only as a hint, so a mislabelled file still decodes correctly.
pub fn decode_file(path: &Path) -> Result<Audio> {
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
    let (track_id, audio_params) = {
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
        (track.id, params)
    };

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
